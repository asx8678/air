use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

const MIN_TURNS: u64 = 5;
const WASTE: &[&str] =
    &["empty", "length", "interrupted", "overthought", "bad_call", "repeat", "limit", "worker_restart", "error"];
const TEMP: &[&str] = &["/tmp/", "/private/tmp/", "/var/folders/", "/private/var/folders/"];

#[derive(Default)]
struct Tally {
    turns: u64,
    verified: u64,
    changed: u64,
    stalled: u64,
    requests: u64,
    prompt: u64,
    cached: u64,
    out: u64,
    reasoning: u64,
    waste: u64,
    ms: u64,
}

impl Tally {
    fn failed(&self) -> u64 {
        self.changed - self.verified + self.stalled
    }

    fn cost(&self) -> f64 {
        let price = (self.prompt - self.cached) as f64 + self.cached as f64 / 10.0 + self.out as f64 * 4.0;
        let good = (self.turns - self.failed().min(self.turns)).max(1);
        price / good as f64
    }
}

fn label(header: &Value) -> (String, String) {
    let model = header["model"].as_str().unwrap_or("?");
    let model = format!("{model} @ {}", header["provider"].as_str().unwrap_or("config"));
    let mut settings = crate::llm::params(header).to_string();
    if let Some(at) = header["compact_at"].as_u64() {
        settings += &format!(" compact_at={at}");
    }
    if let Some(limit) = header["think_limit"].as_u64() {
        settings += &format!(" think_limit={limit} tok");
    }
    if let Some(notes) = header["guidance"].as_str() {
        let mut hasher = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(notes, &mut hasher);
        let first: String = notes.chars().take_while(|&c| c != '\n').take(40).collect();
        let more = if first.len() < notes.len() { "…" } else { "" };
        settings += &format!(" guidance {:08x} \"{first}{more}\"", std::hash::Hasher::finish(&hasher) as u32);
    }
    (model, settings)
}

/// One run's totals, for the end record of its session log: requests, tokens and speed, what the runs and checks
/// did, and every event by kind (notes are the warnings shown to the user).
pub fn totals(entries: &[Value]) -> Value {
    let (mut n, mut events, mut models) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let (mut verdicts, mut exceptions, mut cost) = (BTreeMap::new(), BTreeMap::new(), None::<f64>);
    let count = |map: &mut BTreeMap<String, u64>, key: &str, by: u64| *map.entry(key.to_string()).or_default() += by;
    for e in entries {
        let (meta, usage, timing) = (&e["meta"], &e["meta"]["usage"], &e["meta"]["timing"]);
        if let Some(kind) = e["event"].as_str() {
            count(&mut events, kind, 1);
        }
        if let Some(verdict) = e["report"]["verdict"].as_str() {
            count(&mut verdicts, verdict, 1);
        }
        count(&mut n, "turns", u64::from(e["role"] == "user" && meta["host"].is_null()));
        count(&mut n, "compactions", u64::from(e["compact"].is_object()));
        count(&mut n, "masks", u64::from(e["mask"].is_object()));
        if usage.is_object() {
            let out = usage["completion_tokens"].as_u64();
            count(&mut n, "replies", 1);
            count(&mut n, "prompt", usage["prompt_tokens"].as_u64().unwrap_or(0));
            count(&mut n, "cached", crate::ui::cached(usage).unwrap_or(0));
            count(&mut n, "out", out.unwrap_or(0));
            count(&mut n, "reasoning", usage["completion_tokens_details"]["reasoning_tokens"].as_u64().unwrap_or(0));
            cost = usage["cost"].as_f64().map(|c| cost.unwrap_or(0.0) + c).or(cost);
            if let Some(model) = meta["request"]["model"].as_str() {
                count(&mut models, model, 1);
            }
            if let Some(ttft) = timing["ttft_ms"].as_u64() {
                count(&mut n, "timed", 1);
                count(&mut n, "ttft_ms", ttft);
                if let (Some(out), Some(stream)) = (out, timing["stream_ms"].as_u64()) {
                    count(&mut n, "decoded", out);
                    count(&mut n, "decode_ms", stream.saturating_sub(ttft));
                }
            }
        }
        if meta["tool"] == "run" {
            count(&mut n, "runs", 1);
            count(&mut n, "run_errors", u64::from(meta["error"] == true));
            count(&mut n, "rejected", u64::from(meta["rejected"] == true));
            count(&mut n, "redacted", meta["redacted"].as_u64().unwrap_or(0));
            if let Some(name) = meta["exception"].as_str() {
                count(&mut exceptions, name, 1);
            }
            for act in meta["actions"].as_array().into_iter().flatten().filter(|a| a["path"].is_string()) {
                count(&mut n, if act["ok"] == true { "edits_ok" } else { "edits_failed" }, 1);
            }
            for cmd in meta["cmds"].as_array().into_iter().flatten() {
                count(&mut n, if cmd[1] == 0 { "shell_ok" } else { "shell_failed" }, 1);
            }
        }
        let check = &e["check"];
        if check.is_object() {
            let outcome = match (check["reused"] == true, check["ok"] == true) {
                (true, _) => "checks_reused",
                (false, true) => "checks_passed",
                (false, false) => "checks_failed",
            };
            count(&mut n, outcome, 1);
        }
    }
    let get = |key: &str| n.get(key).copied().unwrap_or(0);
    let lost: u64 = ["overthought", "interrupted"].iter().filter_map(|k| events.get(*k)).sum();
    let tps =
        (get("decode_ms") > 0).then(|| (get("decoded") as f64 * 10_000.0 / get("decode_ms") as f64).round() / 10.0);
    json!({
        "turns": get("turns"), "verdicts": verdicts, "requests": get("replies") + lost, "models": models,
        "tokens": {"prompt": get("prompt"), "cached": get("cached"), "out": get("out"), "reasoning": get("reasoning")},
        "cost": cost, "ttft_ms": get("ttft_ms").checked_div(get("timed")), "tps": tps,
        "runs": get("runs"), "run_errors": get("run_errors"), "rejected": get("rejected"), "exceptions": exceptions,
        "edits": {"ok": get("edits_ok"), "failed": get("edits_failed")},
        "shell": {"ok": get("shell_ok"), "failed": get("shell_failed")},
        "checks": {"passed": get("checks_passed"), "failed": get("checks_failed"), "reused": get("checks_reused")},
        "compactions": get("compactions"), "masks": get("masks"), "redacted": get("redacted"), "events": events,
    })
}

pub fn print(home: &Path) -> Result<()> {
    let mut groups: BTreeMap<(String, String), Tally> = BTreeMap::new();
    let mut skipped = 0;
    let mut paths: Vec<_> = std::fs::read_dir(home.join(".air/sessions"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    paths.sort();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let mut entries = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok());
        let Some(header) = entries.next() else { continue };
        if header["cwd"].as_str().is_some_and(|cwd| TEMP.iter().any(|t| cwd.starts_with(t))) {
            skipped += 1;
            continue;
        }
        let (mut model, mut settings) = label(&header);
        let (mut wasted, mut reasoning, mut stuck) = (0, 0, false);
        for entry in entries {
            if entry["switch"].is_object() {
                (model, settings) = label(&entry["switch"]);
            }
            if entry["role"] == "user" && entry["meta"]["host"].is_null() {
                (wasted, reasoning, stuck) = (0, 0, false);
            }
            if entry["role"] == "assistant" {
                stuck = entry["tool_calls"].is_null() && entry["content"].as_str().is_none_or(|c| c.trim().is_empty());
            }
            stuck |= entry["event"] == "limit" || entry["event"] == "error";
            let usage = &entry["meta"]["usage"];
            reasoning += usage["completion_tokens_details"]["reasoning_tokens"].as_u64().unwrap_or(0);
            wasted += u64::from(entry["event"].as_str().is_some_and(|k| WASTE.contains(&k)));
            let report = &entry["report"];
            if !report.is_object() {
                continue;
            }
            let t = groups.entry((model.clone(), settings.clone())).or_default();
            let n = |k: &str| report[k].as_u64().unwrap_or(0);
            t.turns += 1;
            t.changed += u64::from(report["changed"] == true);
            t.verified += u64::from(report["verdict"] == "VERIFIED");
            t.stalled += u64::from(report["changed"] == false && stuck);
            t.requests += n("requests");
            t.prompt += n("prompt");
            t.cached += n("cached");
            t.out += n("out");
            t.ms += n("ms");
            t.reasoning += reasoning;
            t.waste += wasted;
        }
    }
    if groups.is_empty() {
        println!(
            "no turns with a report yet ({skipped} sessions under temp dirs skipped; builds before 6e71980 log none)"
        );
        return Ok(());
    }
    let mut best: BTreeMap<&str, (f64, &str, usize)> = BTreeMap::new();
    for ((model, settings), t) in groups.iter().filter(|(_, t)| t.turns >= MIN_TURNS) {
        let entry = best.entry(model).or_insert((f64::MAX, settings, 0));
        entry.2 += 1;
        if t.cost() < entry.0 {
            (entry.0, entry.1) = (t.cost(), settings);
        }
    }
    let mut last = "";
    for ((model, settings), t) in &groups {
        if model != last {
            println!("\n{}", crate::ui::printable(model));
            last = model;
        }
        let won = best.get(model.as_str()).is_some_and(|&(_, s, n)| n > 1 && s == settings);
        let star = if won { "  ★ best" } else { "" };
        let per = |x: u64| x as f64 / t.turns as f64;
        let secs = per(t.ms) as u64 / 1000;
        println!("  {}{star}", crate::ui::printable(settings));
        println!(
            "    {} turns · verified {}/{} changing · stalled {} · {:.1} req/turn · in {} ({}% cached) · out {} ({}% reasoning) per turn",
            t.turns,
            t.verified,
            t.changed,
            t.stalled,
            per(t.requests),
            crate::ui::tokens(per(t.prompt) as u64),
            100 * t.cached / t.prompt.max(1),
            crate::ui::tokens(per(t.out) as u64),
            100 * t.reasoning / t.out.max(1),
        );
        println!(
            "    waste {:.1} per 100 requests · {}:{:02} per turn · cost index {:.0}k per good turn",
            100.0 * t.waste as f64 / t.requests.max(1) as f64,
            secs / 60,
            secs % 60,
            t.cost() / 1000.0
        );
    }
    println!(
        "\nstalled = ended on an empty reply, the request limit or an error, with no change; \
         cost index = uncached + cached/10 + 4 × out.\n★ needs {MIN_TURNS}+ turns per setting; settings compare fairly \
         only on similar work. {skipped} sessions under temp dirs (benchmarks) skipped."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::totals;
    use serde_json::json;

    #[test]
    fn totals_count_what_happened() {
        let usage =
            json!({"prompt_tokens": 1000, "completion_tokens": 200, "prompt_tokens_details": {"cached_tokens": 800}});
        let timing = json!({"ttft_ms": 500, "stream_ms": 2500});
        let entries = [
            json!({"role": "user", "content": "fix it", "meta": {}}),
            json!({"event": "retry"}),
            json!({"role": "assistant", "meta": {"usage": usage, "timing": timing, "request": {"model": "m"}}}),
            json!({"role": "tool", "meta": {"tool": "run", "error": true, "exception": "NameError", "rejected": true}}),
            json!({"role": "assistant", "meta": {"usage": usage, "timing": timing, "request": {"model": "m"}}}),
            json!({"role": "tool", "meta": {"tool": "run", "actions": [{"tool": "edit", "path": "a", "ok": true}, {"tool": "check", "ok": false}], "cmds": [["ls", 0, []], ["make", 2, []]]}}),
            json!({"check": {"ok": false, "gate": false}}),
            json!({"event": "interrupted"}),
            json!({"role": "user", "content": "the check failed", "meta": {"host": true}}),
            json!({"check": {"ok": true, "gate": true, "reused": true}}),
            json!({"report": {"verdict": "VERIFIED"}}),
        ];
        let t = totals(&entries);
        assert_eq!(t["turns"], 1);
        assert_eq!(t["requests"], 3);
        assert_eq!(t["models"], json!({"m": 2}));
        assert_eq!(t["tokens"], json!({"prompt": 2000, "cached": 1600, "out": 400, "reasoning": 0}));
        assert_eq!((t["ttft_ms"].clone(), t["tps"].clone()), (json!(500), json!(100.0)));
        assert_eq!((t["runs"].clone(), t["run_errors"].clone(), t["rejected"].clone()), (json!(2), json!(1), json!(1)));
        assert_eq!(t["exceptions"], json!({"NameError": 1}));
        assert_eq!(
            (t["edits"].clone(), t["shell"].clone()),
            (json!({"ok": 1, "failed": 0}), json!({"ok": 1, "failed": 1}))
        );
        assert_eq!(t["checks"], json!({"passed": 0, "failed": 1, "reused": 1}));
        assert_eq!(t["events"], json!({"interrupted": 1, "retry": 1}));
        assert_eq!(t["verdicts"], json!({"VERIFIED": 1}));
        assert_eq!(t["cost"], json!(null));
    }
}
