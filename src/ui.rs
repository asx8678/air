//! The terminal UI: a scrolling log of short step lines and, on a terminal, a status bar pinned to
//! the last row (a scroll region keeps the log above it). Piped or NO_COLOR output gets plain lines.
use serde_json::Value;
use std::io::{IsTerminal, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Bar {
    rows: u16,
    cols: u16,
    started: Instant,
    activity: String,
    plan: String,
    tokens: (u64, u64), // prompt, cached
}

static BAR: Mutex<Option<Bar>> = Mutex::new(None);

fn styled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn paint(code: &str, text: &str) -> String {
    if styled() { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
}

/// Rows and columns of the controlling terminal.
fn size() -> Option<(u16, u16)> {
    let tty = std::fs::File::open("/dev/tty").ok()?;
    let out = std::process::Command::new("stty").arg("size").stdin(tty).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let mut it = text.split_whitespace().map(|n| n.parse().ok());
    Some((it.next()??, it.next()??)).filter(|&(rows, cols)| rows >= 5 && cols >= 20)
}

/// The status bar of one turn; dropping it gives the last row back to the terminal.
pub struct Turn(Option<tokio::task::JoinHandle<()>>);

impl Turn {
    pub fn start() -> Self {
        let Some((rows, cols)) = size().filter(|_| styled()) else { return Turn(None) };
        reserve(rows);
        let started = Instant::now();
        *BAR.lock().unwrap() = Some(Bar {
            rows,
            cols,
            started,
            activity: "waiting for the model".into(),
            plan: String::new(),
            tokens: (0, 0),
        });
        draw();
        Turn(Some(tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                resize();
                draw();
            }
        })))
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if let Some(ticker) = self.0.take() {
            ticker.abort();
            if let Some(bar) = BAR.lock().unwrap().take() {
                print!("\x1b7\x1b[{};1H\x1b[2K\x1b[r\x1b8", bar.rows);
                let _ = std::io::stdout().flush();
            }
        }
    }
}

/// Keep the last row out of the scroll region, leaving the cursor where the log continues.
fn reserve(rows: u16) {
    print!("\n\x1b[1A\x1b7\x1b[1;{}r\x1b8", rows.saturating_sub(1));
}

fn resize() {
    let Some((rows, cols)) = size() else { return };
    let mut bar = BAR.lock().unwrap();
    if let Some(bar) = bar.as_mut().filter(|b| (b.rows, b.cols) != (rows, cols)) {
        (bar.rows, bar.cols) = (rows, cols);
        reserve(rows);
    }
}

fn draw() {
    let bar = BAR.lock().unwrap();
    let Some(bar) = bar.as_ref() else { return };
    let secs = bar.started.elapsed().as_secs();
    let (prompt, cached) = bar.tokens;
    let cache = (cached * 100).checked_div(prompt).map(|p| format!(" · {p}% cached")).unwrap_or_default();
    let lead = if bar.plan.is_empty() { bar.activity.clone() } else { format!("{} · {}", bar.plan, bar.activity) };
    let line = format!(" ▸ {lead} │ {}{cache} │ {}:{:02} ", tokens(prompt), secs / 60, secs % 60);
    let line: String = line.chars().take(bar.cols.saturating_sub(1) as usize).collect();
    print!("\x1b7\x1b[{};1H\x1b[2K\x1b[7m{line}\x1b[0m\x1b8", bar.rows);
    let _ = std::io::stdout().flush();
}

fn update(change: impl FnOnce(&mut Bar)) {
    if let Some(bar) = BAR.lock().unwrap().as_mut() {
        change(bar);
    }
    draw();
}

pub fn tokens(n: u64) -> String {
    if n >= 1000 { format!("{:.1}k tok", n as f64 / 1000.0) } else { format!("{n} tok") }
}

/// What the agent is doing right now, shown in the status bar.
pub fn activity(text: &str) {
    update(|bar| bar.activity = text.to_string());
}

pub fn usage(usage: &Value) {
    let cached = usage["prompt_tokens_details"]["cached_tokens"].as_u64();
    let cached = cached.or(usage["cached_tokens"].as_u64()).or(usage["prompt_cache_hit_tokens"].as_u64());
    let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0);
    update(|bar| bar.tokens = (bar.tokens.0 + prompt, bar.tokens.1 + cached.unwrap_or(0)));
}

/// A reasoning block, collapsed to its first sentence.
pub fn thought(text: &str) {
    let first = text.split_inclusive(['.', '\n']).map(str::trim).find(|s| !s.is_empty()).unwrap_or_default();
    let mut first: String = first.chars().take(110).collect();
    let total = text.trim().chars().count();
    if total > first.chars().count() {
        first += &format!(" (+{} chars)", total - first.chars().count());
    }
    println!("{}", paint("2", &format!("· thinking: {first}")));
}

/// A run starting: its first meaningful line of code.
pub fn run(code: &str) {
    let skip = |l: &&str| l.is_empty() || l.starts_with('#') || l.starts_with("import ") || l.starts_with("from ");
    let first = code.lines().map(str::trim).find(|l| !skip(l)).unwrap_or("(no code)");
    let first: String = first.chars().take(100).collect();
    let more = code.lines().count().saturating_sub(1);
    let more = if more > 0 { paint("2", &format!("  +{more} lines")) } else { String::new() };
    println!("{} {first}{more}", paint("36", "▸ run"));
    activity(&format!("running {first}"));
}

/// What a run did: its edits, writes, checks, criteria and plan, then a short look at its output.
pub fn ran(meta: &Value, out: &str) {
    if let Some(criteria) = meta["accepted"].as_str() {
        println!("{} {}", paint("35", "◆ criteria"), criteria.lines().next().unwrap_or_default());
    }
    if meta["plan"].is_object() {
        plan(&meta["plan"]);
    }
    let actions = meta["actions"].as_array().cloned().unwrap_or_default();
    for act in &actions {
        let (tool, ok) = (act["tool"].as_str().unwrap_or_default(), act["ok"] == true);
        let mark = if ok { paint("32", "✓") } else { paint("31", "✗") };
        match act["path"].as_str() {
            Some(path) if ok => {
                let (a, r) = (act["added"].as_u64().unwrap_or(0), act["removed"].as_u64().unwrap_or(0));
                println!("{mark} {tool} {path}  {} {}", paint("32", &format!("+{a}")), paint("31", &format!("−{r}")));
            }
            Some(path) => println!("{mark} {tool} {path}: {}", act["error"].as_str().unwrap_or_default()),
            None => println!("{mark} check {}", if ok { "passed" } else { "FAILED" }),
        }
    }
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty() && *l != "(no output)").collect();
    let failed_check = actions.iter().any(|a| a["tool"] == "check" && a["ok"] != true);
    let shown: Vec<&str> = if meta["exception"].is_string() || failed_check {
        lines[lines.len().saturating_sub(4)..].to_vec() // the error and its hint are at the end
    } else if actions.is_empty() {
        lines.iter().take(4).copied().collect()
    } else {
        vec![]
    };
    let color = if meta["exception"].is_string() || failed_check { "31" } else { "2" };
    for line in &shown {
        println!("  {}", paint(color, &line.chars().take(160).collect::<String>()));
    }
    if actions.is_empty() && lines.len() > shown.len() {
        println!("  {}", paint("2", &format!("… {} lines of output", lines.len())));
    }
}

/// The plan as a checklist; the first open step is the current one.
pub fn plan(plan: &Value) {
    let steps: Vec<&str> = plan["steps"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let done: Vec<u64> = plan["done"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    let current = (1..=steps.len() as u64).find(|i| !done.contains(i));
    for (i, step) in (1u64..).zip(&steps) {
        let line = if done.contains(&i) {
            paint("32", &format!("  ✓ {step}"))
        } else if Some(i) == current {
            paint("1", &format!("  ▸ {step}"))
        } else {
            paint("2", &format!("  · {step}"))
        };
        println!("{line}");
    }
    let progress = match current {
        Some(i) => format!("{}/{} {}", done.len(), steps.len(), steps[i as usize - 1]),
        None => format!("{}/{} done", done.len(), steps.len()),
    };
    update(|bar| bar.plan = progress);
}

/// A host message, such as compaction or the verify-gate nudge.
pub fn note(text: &str) {
    println!("{}", paint("33", &format!("· {text}")));
}

/// The fixed-shape report that ends a turn.
pub fn report(rows: &[(&str, String)], verdict: &str, good: bool) {
    let head = format!("── {verdict} ").to_string() + &"─".repeat(36);
    println!("\n{}", paint(if good { "1;32" } else { "1;31" }, &head));
    for (label, value) in rows {
        println!("{} {}", paint("2", &format!("{label:<9}")), value.replace('\n', "\n          "));
    }
}
