//! Deterministic, LLM-free compaction. A summary is a pure function of the raw log entries before
//! the cut. It never reads an earlier summary, it is at most 32 KiB, and every #n in it is a log line
//! that recall(n) returns exactly. Facts come from typed fields the host recorded, never from
//! parsing output text.
use serde_json::Value;
use std::collections::BTreeMap;

/// Raw tail kept after a cut, in estimated tokens (4 bytes each).
const KEEP_TOKENS: usize = 20_000;
const MAX_BYTES: usize = 32 * 1024;

/// The earliest user or assistant entry after the previous cut whose raw tail fits KEEP_TOKENS, or
/// entries.len() (compact everything) when none fits. A tool result always directly follows the
/// assistant entry that called it, so cutting at a user or assistant entry never splits a pair.
pub fn cut(entries: &[Value], prev: usize) -> usize {
    let (mut tail, mut keep) = (0, entries.len());
    for i in (prev + 1..entries.len()).rev() {
        let Some(role) = entries[i]["role"].as_str() else { continue }; // markers are not sent
        tail += entries[i].to_string().len() / 4;
        if tail > KEEP_TOKENS {
            break;
        }
        if role != "tool" {
            keep = i;
        }
    }
    // Never cut past the newest user direction: its full text must reach the model.
    let newest =
        (prev + 1..entries.len()).rev().find(|&i| entries[i]["role"] == "user" && entries[i]["meta"]["host"].is_null());
    newest.map_or(keep, |u| keep.min(u))
}

/// Summary of entries #1..keep, read straight from the raw log.
pub fn summary(entries: &[Value], keep: usize) -> String {
    let role = |i: usize| entries[i]["role"].as_str().unwrap_or_default();
    let text = |i: usize| entries[i]["content"].as_str().unwrap_or_default();
    let said = |i: usize| !text(i).trim().is_empty();
    let range = 1..keep;
    let users: Vec<usize> =
        range.clone().filter(|&i| role(i) == "user" && entries[i]["meta"]["host"].is_null()).collect();
    let recent = &users[users.len().saturating_sub(3)..];

    // The last three user directions, each with the assistant text it answered.
    let mut dialogue = vec![];
    for &u in recent {
        if let Some(a) = (1..u).rev().take_while(|&i| role(i) != "user").find(|&i| role(i) == "assistant" && said(i)) {
            dialogue.push(format!("#{a} assistant (historical, not a verified outcome):\n{}", excerpt(text(a), 1000)));
        }
        dialogue.push(format!("#{u} user:\n{}", excerpt(text(u), 2000)));
    }
    let requests: Vec<String> =
        users[..users.len() - recent.len()].iter().map(|&i| format!("#{i} {}", line(text(i), 160))).collect();

    // Criteria, files, failures and status from what each run reported, never parsed from its output.
    let (mut criteria, mut carry) = (vec![], vec![]);
    let mut files: BTreeMap<&str, (usize, usize)> = BTreeMap::new(); // path -> (1 modified | 2 created, last #)
    let mut failures: Vec<(String, String)> = vec![]; // (action, line); dropped by a later success of the same action
    let mut status = vec![];
    for i in range.clone() {
        let meta = &entries[i]["meta"];
        if let Some(c) = meta["accepted"].as_str() {
            criteria = vec![format!("#{i} {}", excerpt(c, 1800))];
        }
        if let Some(notes) = meta["carry"].as_array() {
            carry = notes.iter().filter_map(Value::as_str).map(|n| format!("#{i} {n}")).collect();
        }
        // Each edit, write and check of a run, in order; a failure stays until the same action succeeds.
        for act in meta["actions"].as_array().into_iter().flatten() {
            let (tool, path, ok) = (act["tool"].as_str().unwrap_or_default(), act["path"].as_str(), act["ok"] == true);
            let action = path.map_or_else(|| tool.to_string(), |p| format!("{tool} {p}"));
            match path {
                Some(p) if ok => {
                    touch(&mut files, p, if tool == "edit" { 1 } else { 2 }, i);
                    status.retain(|s: &String| !s.starts_with("last change"));
                    status.push(format!("last change: #{i} {action}"));
                }
                None => {
                    status.retain(|s: &String| !s.starts_with("last check"));
                    status.push(format!("last check: #{i} {}", if ok { "passed" } else { "FAILED" }));
                }
                _ => {}
            }
            failures.retain(|(a, _)| *a != action);
            if !ok {
                let problem = act["error"].as_str().map_or_else(|| format!("output at #{i}"), |e| line(e, 200));
                failures.push((action.clone(), format!("#{i} {action} failed: {problem}")));
            }
        }
    }
    if let Some(i) = range.clone().rev().find(|&i| role(i) == "assistant" && said(i)) {
        status.push(format!("last assistant note: #{i} {}", line(text(i), 160)));
    }
    let mut ranked: Vec<_> = files.iter().collect();
    ranked.sort_by_key(|(_, (rank, _))| std::cmp::Reverse(*rank));
    let files: Vec<String> =
        ranked.iter().map(|(path, (rank, i))| format!("{} {path} #{i}", ["", "modified", "created"][*rank])).collect();

    // One line per event: user and assistant text, each tool call with its result. Results are
    // matched only among the entries right after their call, since providers may reuse call ids.
    let mut events = vec![];
    for i in range {
        match role(i) {
            "user" => {
                let who = if entries[i]["meta"]["host"].is_null() { "user" } else { "host" };
                events.push(format!("#{i} {who}: {}", line(text(i), 100)));
            }
            "assistant" => {
                if said(i) {
                    events.push(format!("#{i} assistant: {}", line(text(i), 100)));
                }
                for call in entries[i]["tool_calls"].as_array().into_iter().flatten() {
                    let name = call["function"]["name"].as_str().unwrap_or_default();
                    let args: Value = serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or_default())
                        .unwrap_or_default();
                    let what = line(args["code"].as_str().unwrap_or_default(), 80);
                    let result = (i + 1..keep)
                        .take_while(|&r| role(r) == "tool")
                        .find(|&r| entries[r]["tool_call_id"] == call["id"]);
                    let outcome = result.map_or_else(|| "no result".into(), |r| format!("#{r} {}", line(text(r), 80)));
                    events.push(format!("#{i} {name} {what} → {outcome}"));
                }
            }
            _ => {}
        }
    }
    let events = events.split_off(events.len().saturating_sub(40));

    let mut out = String::from("[Summary of earlier work, rebuilt from the session log. Newer messages follow it.]\n");
    for (title, lines, cap) in [
        ("Recent user directions", dialogue, 10_240),
        ("Accepted criteria", criteria, 2_048),
        ("Carry forward", carry, 3_072),
        ("Earlier requests", requests, 3_072),
        ("Files", files, 4_096),
        ("Unresolved failures", failures.into_iter().map(|(_, l)| l).collect(), 3_072),
        ("Status", status, 1_536),
        ("Latest events", events, 4_096),
    ] {
        if !lines.is_empty() {
            out += &block(&format!("\n## {title}"), &lines, cap);
            out += "\n";
        }
    }
    out += &format!(
        "\n[Entries #1-#{} are summarized here; recall(n) returns entry #n exactly. Continue the task.]",
        keep - 1
    );
    clip(&out, MAX_BYTES).to_string()
}

fn touch<'a>(files: &mut BTreeMap<&'a str, (usize, usize)>, path: &'a str, rank: usize, i: usize) {
    let seen = files.entry(path).or_insert((rank, i));
    *seen = (seen.0.max(rank), i);
}

/// A titled block within `cap` bytes; when too long, its middle lines give way to an omission note.
fn block(title: &str, lines: &[String], cap: usize) -> String {
    let n = lines.len();
    for keep in (0..=n).rev() {
        let mut kept = vec![title.to_string()];
        kept.extend_from_slice(&lines[..keep.div_ceil(2)]);
        if keep < n {
            kept.push(format!("… {} omitted; recall(n) any #n around here", n - keep));
        }
        kept.extend_from_slice(&lines[n - keep / 2..]);
        let block = kept.join("\n");
        if block.len() <= cap {
            return block;
        }
    }
    clip(title, cap).to_string()
}

/// Text cut to `max` bytes with an explicit loss marker.
fn excerpt(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = clip(s, max - 40);
    format!("{head}\n[… {} bytes omitted]", s.len() - head.len())
}

/// First non-empty line, at most `max` bytes.
fn line(s: &str, max: usize) -> String {
    let first = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if first.len() <= max { first.to_string() } else { format!("{}…", clip(first, max)) }
}

fn clip(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
