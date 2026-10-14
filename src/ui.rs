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
    tokens: (u64, u64),
}

static BAR: Mutex<Option<Bar>> = Mutex::new(None);
static STEP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static RUNNING: Mutex<Option<(String, Instant)>> = Mutex::new(None);

pub fn styled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

pub fn paint(code: &str, text: &str) -> String {
    if styled() { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
}

pub fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect()
}

fn size() -> Option<(u16, u16)> {
    let tty = std::fs::File::open("/dev/tty").ok()?;
    let out = std::process::Command::new("stty").arg("size").stdin(tty).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let mut it = text.split_whitespace().map(|n| n.parse().ok());
    Some((it.next()??, it.next()??)).filter(|&(rows, cols)| rows >= 5 && cols >= 20)
}

pub struct Turn(Option<tokio::task::JoinHandle<()>>);

impl Turn {
    pub fn start() -> Self {
        STEP.store(0, std::sync::atomic::Ordering::Relaxed);
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

pub fn activity(text: &str) {
    let mut bar = BAR.lock().unwrap();
    match bar.as_mut() {
        Some(bar) if bar.activity != text => bar.activity = text.to_string(),
        _ => return,
    }
    drop(bar);
    draw();
}

pub fn usage(usage: &Value) {
    let cached = usage["prompt_tokens_details"]["cached_tokens"].as_u64();
    let cached = cached.or(usage["cached_tokens"].as_u64()).or(usage["prompt_cache_hit_tokens"].as_u64());
    let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0);
    update(|bar| bar.tokens = (bar.tokens.0 + prompt, bar.tokens.1 + cached.unwrap_or(0)));
}

pub fn width() -> usize {
    BAR.lock().unwrap().as_ref().map_or(120, |b| b.cols as usize)
}

fn flat(text: &str, max: usize) -> String {
    let flat = printable(text).split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { flat.chars().take(max).collect::<String>() + "…" }
}

static LIVE: Mutex<(usize, Option<Instant>)> = Mutex::new((0, None));
const LIVE_LINES: usize = 3;

pub fn thinking(text: &str, secs: u64) {
    activity(&format!("thinking {secs}s"));
    let Some(cols) = BAR.lock().unwrap().as_ref().map(|b| b.cols as usize) else { return };
    let mut live = LIVE.lock().unwrap();
    if live.1.is_some_and(|drawn| drawn.elapsed() < Duration::from_millis(80)) {
        return;
    }
    let text = printable(text);
    let width = cols.saturating_sub(6).max(20);
    let start = text.char_indices().rev().nth(width * (LIVE_LINES + 2)).map_or(0, |(i, _)| i);
    let mut lines: Vec<String> = vec![String::new()];
    for word in text[start..].split_whitespace() {
        let line = lines.last_mut().expect("never empty");
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(String::new());
        }
        let line = lines.last_mut().expect("never empty");
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    let tail = &lines[lines.len().saturating_sub(LIVE_LINES)..];
    let chars = text.chars().count();
    let size = if chars >= 1000 { format!("{:.1}k chars", chars as f64 / 1000.0) } else { format!("{chars} chars") };
    let mut out = if live.0 > 0 { format!("\x1b[{}A", live.0) } else { String::new() };
    out += &format!("\r\x1b[2K{}\n", paint("35", &format!("◇ thinking {secs}s · {size}")));
    for i in 0..LIVE_LINES {
        let line: String = tail.get(i).map_or("", String::as_str).chars().take(width).collect();
        out += &format!("\r\x1b[2K  {}\n", paint("2;3", &line));
    }
    print!("{out}");
    let _ = std::io::stdout().flush();
    *live = (LIVE_LINES + 1, Some(Instant::now()));
}

pub fn writing(lines: usize) {
    activity("writing code");
    if BAR.lock().unwrap().is_none() {
        return;
    }
    if LIVE.lock().unwrap().0 > 1 {
        clear_live();
    }
    let mut live = LIVE.lock().unwrap();
    if live.1.is_some_and(|drawn| drawn.elapsed() < Duration::from_millis(80)) {
        return;
    }
    let up = if live.0 > 0 { "\x1b[1A" } else { "" };
    let text = format!("✎ writing code · {lines} line{}", if lines == 1 { "" } else { "s" });
    print!("{up}\r\x1b[2K    {}\n", paint("36", &text));
    let _ = std::io::stdout().flush();
    *live = (1, Some(Instant::now()));
}

pub fn clear_live() {
    let mut live = LIVE.lock().unwrap();
    if live.0 > 0 {
        let n = live.0;
        print!("\x1b[{n}A{}\x1b[{n}A", "\r\x1b[2K\n".repeat(n));
    }
    *live = (0, None);
}

pub fn thought(text: &str, secs: u64) {
    clear_live();
    let sentences = sentences(text);
    let body = match sentences.as_slice() {
        [] => return,
        [.., before, last] if last.chars().count() < 25 => format!("{before} {last}"),
        [.., last] => last.to_string(),
    };
    step_note(&body, text, secs, "2;3");
}

pub fn narration(text: &str, thinking: &str, secs: u64) {
    clear_live();
    step_note(text, thinking, secs, "2");
}

fn step_note(body: &str, thinking: &str, secs: u64, style: &str) {
    let total = thinking.trim().chars().count();
    let badge = if secs >= 5 || total >= 2000 {
        format!("  ({secs}s · {:.1}k chars · /think)", total as f64 / 1000.0)
    } else {
        String::new()
    };
    let room = width().saturating_sub(8 + badge.chars().count()).max(30);
    println!("    {} {}{}", paint("35", "◇"), paint(style, &flat(body, room)), paint("2", &badge));
}

fn sentences(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next_is_space = chars.peek().is_none_or(|&(_, n)| n.is_whitespace());
        if c == '\n' || (matches!(c, '.' | '?' | '!') && next_is_space) {
            let end = i + c.len_utf8();
            out.push(text[start..end].trim());
            start = end;
        }
    }
    out.push(text[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

fn literal_after<'a>(line: &'a str, call: &str) -> Option<&'a str> {
    let rest = &line[line.find(call)? + call.len()..];
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    rest[1..].split(quote).next()
}

pub fn run(code: &str) {
    let code = printable(code);
    let step = STEP.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let skip = |l: &&str| l.is_empty() || l.starts_with('#') || l.starts_with("import ") || l.starts_with("from ");
    let first = code.lines().map(str::trim).find(|l| !skip(l)).unwrap_or("(no code)");
    let single = code.lines().filter(|l| !skip(&l.trim())).count() == 1;
    let label = match (literal_after(first, "open("), literal_after(first, "sh(")) {
        (Some(path), _) if single && first.contains(".read") => format!("read {path}"),
        (_, Some(cmd)) if single => format!("$ {cmd}"),
        _ if first == "check()" => "check".to_string(),
        _ => {
            let more = code.lines().count().saturating_sub(1);
            let first = flat(first, width().saturating_sub(34).max(30));
            if more > 0 { format!("{first} {}", paint("2", &format!("+{more} lines"))) } else { first }
        }
    };
    let label = if label.contains('\x1b') { label } else { flat(&label, width().saturating_sub(22).max(30)) };
    clear_live();
    let line = format!("{} {} {label}", paint("2", &format!("{step:>3}")), paint("36", "▸"));
    if BAR.lock().unwrap().is_some() {
        print!("{line}  {}", paint("2", "⋯ running"));
    } else {
        print!("{line}");
    }
    let _ = std::io::stdout().flush();
    *RUNNING.lock().unwrap() = Some((line, Instant::now()));
    activity(&format!("step {step} running · {}", flat(first, 60)));
}

pub fn ran(meta: &Value, out: &str) {
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty() && *l != "(no output)").collect();
    let actions = meta["actions"].as_array().cloned().unwrap_or_default();
    let failed = meta["exception"].is_string() || actions.iter().any(|a| a["ok"] != true);
    let exit = lines.last().and_then(|l| l.strip_prefix("[exit ")).map(|l| l.trim_end_matches(']'));
    let result = if let Some(name) = meta["exception"].as_str() {
        paint("31", &format!("✗ {name}"))
    } else if let Some(code) = exit {
        paint("31", &format!("✗ exit {code}"))
    } else if !actions.is_empty() {
        String::new()
    } else if lines.is_empty() {
        paint("2", "· no output")
    } else {
        paint("2", &format!("→ {} line{}", lines.len(), if lines.len() == 1 { "" } else { "s" }))
    };
    let took = match RUNNING.lock().unwrap().take() {
        Some((line, started)) => {
            if BAR.lock().unwrap().is_some() {
                print!("\r\x1b[2K{line}");
            }
            let secs = started.elapsed().as_secs_f64();
            if secs >= 1.0 { paint("2", &format!(" · {secs:.1}s")) } else { String::new() }
        }
        None => String::new(),
    };
    println!("  {result}{took}");
    if let Some(criteria) = meta["accepted"].as_str() {
        println!("      {} {}", paint("35", "◆ criteria"), flat(criteria, width().saturating_sub(18).max(30)));
    }
    if meta["plan"].is_object() {
        plan(&meta["plan"]);
    }
    for act in &actions {
        let (tool, ok) = (act["tool"].as_str().unwrap_or_default(), act["ok"] == true);
        let mark = if ok { paint("32", "✓") } else { paint("31", "✗") };
        match act["path"].as_str().map(|p| flat(p, 120)) {
            Some(path) if ok => {
                let (a, r) = (act["added"].as_u64().unwrap_or(0), act["removed"].as_u64().unwrap_or(0));
                println!(
                    "      {mark} {tool} {path}  {} {}",
                    paint("32", &format!("+{a}")),
                    paint("31", &format!("−{r}"))
                );
            }
            Some(path) => {
                let error = flat(act["error"].as_str().unwrap_or_default(), width().saturating_sub(20).max(30));
                println!("      {mark} {tool} {path}: {error}");
            }
            None => println!("      {mark} check {}", if ok { "passed" } else { "FAILED" }),
        }
    }
    let shown: &[&str] = if failed {
        &lines[lines.len().saturating_sub(4)..]
    } else if actions.is_empty() && lines.len() <= 3 {
        &lines
    } else {
        &[]
    };
    for line in shown {
        println!("      {}", paint(if failed { "31" } else { "2" }, &flat(line, width().saturating_sub(7).max(30))));
    }
}

pub fn prose() {
    if STEP.load(std::sync::atomic::Ordering::Relaxed) > 0 {
        println!();
    }
}

pub fn plan(plan: &Value) {
    let steps: Vec<String> = plan["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|s| flat(s, width().saturating_sub(10).max(30)))
        .collect();
    let done: Vec<u64> = plan["done"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    let current = (1..=steps.len() as u64).find(|i| !done.contains(i));
    for (i, step) in (1u64..).zip(&steps) {
        let line = if done.contains(&i) {
            paint("32", &format!("      ✓ {step}"))
        } else if Some(i) == current {
            paint("1", &format!("      ▸ {step}"))
        } else {
            paint("2", &format!("      · {step}"))
        };
        println!("{line}");
    }
    let progress = match current {
        Some(i) => format!("{}/{} {}", done.len(), steps.len(), steps[i as usize - 1]),
        None => format!("{}/{} done", done.len(), steps.len()),
    };
    update(|bar| bar.plan = progress);
}

pub fn note(text: &str) {
    clear_live();
    println!("{}", paint("33", &format!("· {text}")));
}

pub fn report(rows: &[(&str, String)], verdict: &str, good: bool) {
    let head = format!("── {verdict} ").to_string() + &"─".repeat(36);
    println!("\n{}", paint(if good { "1;32" } else { "1;31" }, &head));
    let room = width().saturating_sub(12).max(30);
    for (label, value) in rows {
        println!("{} {}", paint("2", &format!("{label:<9}")), wrap(value, room).join("\n          "));
    }
}

pub fn wrap(text: &str, room: usize) -> Vec<String> {
    let mut out = vec![];
    for line in printable(text).lines() {
        let mut current = String::new();
        for word in line.split_whitespace() {
            if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > room {
                out.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        out.push(current);
    }
    out
}
