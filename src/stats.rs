use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

const MIN_TURNS: u64 = 5;
const WASTE: &[&str] = &["empty", "interrupted", "bad_call", "repeat", "limit", "worker_restart", "error"];
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
        let model = header["model"].as_str().unwrap_or("?").to_string();
        let mut settings = crate::llm::params(&header).to_string();
        if let Some(at) = header["compact_at"].as_u64() {
            settings += &format!(" compact_at={at}");
        }
        if let Some(notes) = header["guidance"].as_str() {
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(notes, &mut hasher);
            let first: String = notes.chars().take_while(|&c| c != '\n').take(40).collect();
            let more = if first.len() < notes.len() { "…" } else { "" };
            settings += &format!(" guidance {:08x} \"{first}{more}\"", std::hash::Hasher::finish(&hasher) as u32);
        }
        let (mut wasted, mut reasoning, mut stuck) = (0, 0, false);
        for entry in entries {
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
            println!("\n{model}");
            last = model;
        }
        let won = best.get(model.as_str()).is_some_and(|&(_, s, n)| n > 1 && s == settings);
        let star = if won { "  ★ best" } else { "" };
        let per = |x: u64| x as f64 / t.turns as f64;
        let secs = per(t.ms) as u64 / 1000;
        println!("  {settings}{star}");
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
