use serde_json::Value;
use std::collections::BTreeMap;

const MAX_BYTES: usize = 32 * 1024;
const EXCHANGE: usize = 3968;

pub fn cut(entries: &[Value], prev: usize, keep_tokens: usize) -> usize {
    let (mut tail, mut keep) = (0, entries.len());
    for i in (prev + 1..entries.len()).rev() {
        let Some(role) = entries[i]["role"].as_str() else { continue };
        tail += entries[i].to_string().len() / 4;
        if tail > keep_tokens {
            break;
        }
        if role == "assistant" {
            keep = i;
        }
    }
    keep
}

pub fn summary(entries: &[Value], keep: usize) -> String {
    let role = |i: usize| entries[i]["role"].as_str().unwrap_or_default();
    let text = |i: usize| entries[i]["content"].as_str().unwrap_or_default();
    let said = |i: usize| !text(i).trim().is_empty();
    let range = 1..keep;
    let users: Vec<usize> =
        range.clone().filter(|&i| role(i) == "user" && entries[i]["meta"]["host"].is_null()).collect();
    let recent = &users[users.len().saturating_sub(3)..];

    let mut dialogue = vec![];
    for &u in recent {
        let a = (1..u).rev().take_while(|&i| role(i) != "user").find(|&i| role(i) == "assistant" && said(i));
        let room = a.map_or(EXCHANGE, |a| (EXCHANGE / 2).max(EXCHANGE.saturating_sub(text(a).len())));
        let user = excerpt(text(u), room);
        if let Some(a) = a {
            let reply = excerpt(text(a), EXCHANGE - user.len());
            dialogue.push(format!("#{a} assistant (historical, not a verified outcome):\n{reply}"));
        }
        dialogue.push(format!("#{u} user:\n{user}"));
    }
    let after = users.last().map_or(1, |&u| u + 1);
    if let Some(a) = (after..keep).rev().find(|&i| role(i) == "assistant" && said(i)) {
        dialogue.push(format!(
            "#{a} assistant, latest reply (historical, not a verified outcome):\n{}",
            excerpt(text(a), 4000)
        ));
    }
    let requests: Vec<String> =
        users[..users.len() - recent.len()].iter().map(|&i| format!("#{i} {}", line(text(i), 160))).collect();

    let (mut criteria, mut carry) = (vec![], vec![]);
    let mut files: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut failures: Vec<(String, String)> = vec![];
    let mut status = vec![];
    let mut failing_check = None;
    for i in range.clone() {
        let meta = &entries[i]["meta"];
        if let Some(c) = meta["accepted"].as_str() {
            criteria = vec![format!("#{i} {}", excerpt(c, 1800))];
        }
        if let Some(notes) = meta["carry"].as_array() {
            carry = notes.iter().filter_map(Value::as_str).map(|n| format!("#{i} {n}")).collect();
        }
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
                    failing_check = (!ok).then_some(i);
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
    let check_output: Vec<String> =
        failing_check.map(|i| format!("#{i}\n{}", tail(text(i), 1536))).into_iter().collect();

    let skip = |l: &&str| l.is_empty() || l.starts_with('#') || l.starts_with("import ") || l.starts_with("from ");
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
                    let code = args["code"].as_str().unwrap_or_default();
                    let what = line(code.lines().map(str::trim).find(|l| !skip(l)).unwrap_or_default(), 80);
                    let result = (i + 1..keep)
                        .take_while(|&r| role(r) == "tool")
                        .find(|&r| entries[r]["tool_call_id"] == call["id"]);
                    let outcome = result.map_or_else(
                        || "no result".into(),
                        |r| {
                            let out = text(r);
                            let shown = if entries[r]["meta"]["error"] == true {
                                let last = |l: &&str| !l.is_empty() && !l.starts_with("hint:");
                                out.lines().map(str::trim).rev().find(last).unwrap_or_default()
                            } else {
                                out
                            };
                            format!("#{r} {}", line(shown, 80))
                        },
                    );
                    events.push(format!("#{i} {name} {what} → {outcome}"));
                }
            }
            _ => {}
        }
    }
    let events = events.split_off(events.len().saturating_sub(40));

    let footer = format!(
        "\n[Entries #1-#{} are summarized here; recall(n) returns entry #n exactly. Continue the task.]",
        keep - 1
    );
    let mut out = String::from("[Summary of earlier work, rebuilt from the session log. Newer messages follow it.]\n");
    for (title, lines, cap) in [
        ("Recent user directions", dialogue, 16_384),
        ("Accepted criteria", criteria, 2_048),
        ("Carry forward", carry, 3_072),
        ("Earlier requests", requests, 3_072),
        ("Files", files, 4_096),
        ("Unresolved failures", failures.into_iter().map(|(_, l)| l).collect(), 3_072),
        ("Status", status, 1_536),
        ("Failing check output", check_output, 2_048),
        ("Latest events", events, 4_096),
    ] {
        let left = MAX_BYTES.saturating_sub(out.len() + footer.len() + 1);
        let section = block(&format!("\n## {title}"), &lines, cap.min(left));
        if !lines.is_empty() && !section.is_empty() {
            out += &section;
            out += "\n";
        }
    }
    out + &footer
}

fn touch<'a>(files: &mut BTreeMap<&'a str, (usize, usize)>, path: &'a str, rank: usize, i: usize) {
    let seen = files.entry(path).or_insert((rank, i));
    *seen = (seen.0.max(rank), i);
}

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
    String::new()
}

fn excerpt(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = clip(s, max - 40);
    format!("{head}\n[… {} bytes omitted]", s.len() - head.len())
}

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

pub fn tail(s: &str, max: usize) -> &str {
    let mut start = s.len().saturating_sub(max);
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}
