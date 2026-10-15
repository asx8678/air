use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::io::Write;
use std::time::{Duration, Instant};

pub const TEMPERATURE: f64 = 0.0;
/// What air calls itself on the wire.
const USER_AGENT: &str = concat!("air/", env!("CARGO_PKG_VERSION"));
const LONG: Duration = Duration::from_secs(150);
const FIRST_BYTE: Duration = Duration::from_secs(90);
const IDLE: Duration = Duration::from_secs(300);
const TOTAL: Duration = Duration::from_secs(3600);
const LINE_MAX: usize = 16 << 20;
const BODY_MAX: usize = 2000;
const FIELDS: [&str; 2] = ["reasoning_content", "reasoning"];
const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";
const OVERFLOW: &[&str] = &[
    "context_length",
    "context length",
    "context window",
    "context size",
    "maximum context",
    "prompt is too long",
    "too many tokens",
    "exceeds the maximum number of tokens",
];
const QUOTA: &str = "insufficient_quota";
const REJECTED: &[&str] = &["invalid", "authentication", "permission", "not_found", "insufficient_quota"];

#[derive(Debug)]
pub struct ContextOverflow(String);

impl std::fmt::Display for ContextOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "context window exceeded: {}", self.0)
    }
}

impl std::error::Error for ContextOverflow {}

#[derive(Debug, Default)]
pub struct Lost {
    pub thinking: String,
    pub text: usize,
    pub stalled: bool,
    pub ms: u64,
}

impl Lost {
    fn new(thinking: &str, began: Option<Instant>) -> Self {
        Self {
            thinking: thinking.into(),
            ms: began.map_or(0, |b| b.elapsed().as_millis() as u64),
            ..Default::default()
        }
    }

    pub fn extra(&self) -> Value {
        json!({"chars": self.thinking.chars().count(), "text_chars": self.text, "stalled": self.stalled, "think_ms": self.ms})
    }
}

pub fn lost(e: &anyhow::Error) -> Option<&Lost> {
    e.downcast_ref::<Interrupted>().map(|i| &i.1).or_else(|| e.downcast_ref::<Overthought>().map(|o| &o.1))
}

#[derive(Debug)]
pub struct Interrupted(String, Lost);

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "stream interrupted: {}", self.0)
    }
}

impl std::error::Error for Interrupted {}

#[derive(Debug)]
pub struct Overthought(u64, Lost);

impl std::fmt::Display for Overthought {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "reasoned past {} tokens without acting", self.0)
    }
}

impl std::error::Error for Overthought {}

pub fn params(header: &Value) -> Value {
    match &header["params"] {
        Value::Object(params) => params.clone().into(),
        _ => json!({"temperature": header["temperature"].as_f64().unwrap_or(TEMPERATURE)}),
    }
}

/// OpenCode's gateway refuses requests without `x-opencode-session`, a stable id per
/// conversation it routes by. That is its protocol, not air's identity — the User-Agent above
/// is where air names itself.
fn session_id(base_url: &str, header: &Value) -> Option<String> {
    let host = base_url.rsplit_once("://").map_or(base_url, |(_, rest)| rest);
    let host = host.split('/').next().unwrap_or("");
    if host != "opencode.ai" && !host.ends_with(".opencode.ai") {
        return None;
    }
    header["id"].as_str().filter(|id| !id.is_empty()).map(String::from)
}

fn overflow(text: &str) -> bool {
    let lower = text.to_lowercase();
    OVERFLOW.iter().any(|m| lower.contains(m))
}

pub fn key_file() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/air/api_key")
}

pub struct Llm {
    http: reqwest::Client,
    base: String,
    key: Option<String>,
    model: String,
    params: Value,
    think_limit: Option<usize>,
    session: Option<String>,
    field: std::cell::Cell<&'static str>,
    marked: std::cell::Cell<bool>,
}

impl Llm {
    pub fn new(base_url: &str, key: Option<&str>, header: &Value) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .build()
                .expect("a plain rustls client builds"),
            base: base_url.trim_end_matches('/').into(),
            key: key
                .map(String::from)
                .or_else(|| std::env::var("AIR_API_KEY").ok())
                .or_else(|| std::fs::read_to_string(key_file()).ok())
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty()),
            model: header["model"].as_str().unwrap_or_default().into(),
            params: params(header),
            think_limit: header["think_limit"].as_u64().map(|n| n as usize),
            session: session_id(base_url, header),
            field: std::cell::Cell::new(FIELDS[0]),
            marked: Default::default(),
        }
    }

    pub async fn models(&self) -> Result<Vec<String>> {
        let mut request = self
            .http
            .get(format!("{}/models", self.base))
            .timeout(Duration::from_secs(15))
            .header("user-agent", USER_AGENT);
        if let Some(id) = &self.session {
            request = request.header("x-opencode-session", id.as_str());
        }
        if let Some(key) = &self.key {
            request = request.bearer_auth(key);
        }
        let fetch = async { anyhow::Ok(request.send().await?.error_for_status()?.text().await?) };
        let text = tokio::select! {
            text = fetch => text?,
            _ = tokio::signal::ctrl_c() => bail!("interrupted"),
        };
        let list: Value = serde_json::from_str(&text)?;
        let ids = list["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str());
        Ok(ids.map(String::from).collect())
    }

    pub async fn complete(&self, mut messages: Vec<Value>, tools: &Value, limited: bool) -> Result<Value> {
        let field = self.field.get();
        for fields in messages.iter_mut().filter(|_| field != FIELDS[0]).filter_map(Value::as_object_mut) {
            if let Some(reasoning) = fields.remove(FIELDS[0]) {
                fields.insert(field.into(), reasoning);
            }
        }
        let mut body = self.params.as_object().cloned().unwrap_or_default();
        body.insert("model".into(), self.model.as_str().into());
        body.insert("messages".into(), Value::Array(messages));
        body.insert("tools".into(), tools.clone());
        body.insert("stream".into(), true.into());
        body.insert("stream_options".into(), json!({"include_usage": true}));
        let mut request = self
            .http
            .post(format!("{}/chat/completions", self.base))
            .header("content-type", "application/json")
            .header("user-agent", USER_AGENT)
            .body(serde_json::to_vec(&body)?);
        if let Some(id) = &self.session {
            request = request.header("x-opencode-session", id.as_str());
        }
        if let Some(key) = &self.key {
            request = request.bearer_auth(key);
        }
        drop(body);
        let start = Instant::now();
        for attempt in 1.. {
            let began = Instant::now();
            match self.stream(&request, self.think_limit.filter(|_| limited)).await {
                Err(e) if e.is::<Interrupted>() => {
                    let mut extra = lost(&e).map_or(json!({}), Lost::extra);
                    extra["attempt"] = attempt.into();
                    extra["ms"] = (began.elapsed().as_millis() as u64).into();
                    if attempt == 4 || began.elapsed() > LONG {
                        crate::ui::event("interrupted", &e.to_string(), extra);
                        return Err(e);
                    }
                    crate::ui::event("interrupted", &format!("{e}; retrying from the start"), extra);
                    tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
                }
                Ok(mut message) => {
                    message["meta"]["timing"]["ms"] = (start.elapsed().as_millis() as u64).into();
                    message["meta"]["timing"]["attempts"] = attempt.into();
                    return Ok(message);
                }
                result => return result,
            }
        }
        unreachable!()
    }

    async fn stream(&self, request: &reqwest::RequestBuilder, limit: Option<usize>) -> Result<Value> {
        let (mut response, sent) = self.send(request).await?;
        let kind = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default();
        let kind = kind.to_lowercase();
        if kind.contains("html") || kind.contains("json") {
            bail!("the provider did not send an event stream: {kind}: {}", head(response).await);
        }
        let (mut first, mut spoke) = (None, None);
        let (mut finished, mut reason) = (false, Value::Null);
        let (mut buf, mut text, mut thinking, mut usage) = (Vec::new(), String::new(), String::new(), Value::Null);
        let mut calls: Vec<[String; 3]> = vec![];
        let mut slots = std::collections::HashMap::new();
        let mut lines = 0;
        let (mut markdown, mut began) = (crate::render::Markdown::default(), None::<Instant>);
        let (mut held, mut answering, mut narrated) = (String::new(), false, false);
        let (mut broken, mut inline, mut chars, mut via) = (None, Inline::default(), 0, None);
        let (mut scan, mut eof, mut events, mut got, mut stalled, mut rejected) = (0, false, 0, 0, false, false);
        let cap = tokio::time::Instant::from_std(sent) + TOTAL;
        let mut idle = tokio::time::Instant::now() + IDLE;
        'read: while !eof {
            match tokio::time::timeout_at(idle.min(cap), response.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    got += chunk.len();
                    buf.extend_from_slice(&chunk);
                }
                Ok(Ok(None)) => {
                    eof = true;
                    buf.push(b'\n');
                }
                Ok(Err(e)) => {
                    broken = Some(chain(e));
                    break;
                }
                Err(_) => {
                    stalled = true;
                    broken = Some(if idle < cap {
                        format!("no output for {}s", IDLE.as_secs())
                    } else {
                        format!("the reply ran past {} minutes", TOTAL.as_secs() / 60)
                    });
                    break;
                }
            }
            let mut start = 0;
            while let Some(end) = buf[scan..].iter().position(|&b| b == b'\n' || b == b'\r') {
                let line = String::from_utf8_lossy(&buf[start..scan + end]);
                (start, scan) = (scan + end + 1, scan + end + 1);
                let Some(data) = line.trim().strip_prefix("data:") else { continue };
                let data = data.trim();
                finished |= data == "[DONE]";
                let Ok(event) = serde_json::from_str::<Value>(data) else { continue };
                events += 1;
                if let Some(error) = event.get("error").filter(|e| !e.is_null()) {
                    rejected = refused(error);
                    broken = Some(format!("provider error: {}", clip(&crate::agent::redact(&error.to_string()).0)));
                    break 'read;
                }
                if event["usage"].is_object() {
                    usage = event["usage"].clone();
                }
                let why = event["choices"][0]["finish_reason"].as_str().filter(|w| !w.is_empty());
                if let Some(why) = why {
                    (finished, reason) = (true, why.into());
                }
                let delta = &event["choices"][0]["delta"];
                let content = match &delta["content"] {
                    Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect(),
                    content => content.as_str().unwrap_or_default().to_string(),
                };
                let field = FIELDS.into_iter().find(|f| delta[*f].as_str().is_some_and(|t| !t.is_empty()));
                let tools = delta["tool_calls"].as_array().filter(|c| !c.is_empty());
                if why.is_some() || field.is_some() || tools.is_some() || !content.is_empty() {
                    idle = tokio::time::Instant::now() + IDLE;
                }
                let (inner, t) = inline.split(&content);
                let reasoning = field.and_then(|f| delta[f].as_str()).unwrap_or_default().to_string() + &inner;
                if !reasoning.is_empty() {
                    via = via.or(field);
                    thinking += &reasoning;
                    chars += reasoning.chars().count();
                    let began = *began.get_or_insert_with(Instant::now);
                    first.get_or_insert(began);
                    if text.is_empty() {
                        crate::ui::thinking(&thinking, began.elapsed().as_secs());
                    }
                    if let Some(limit) = limit.filter(|l| chars / 4 > *l && text.is_empty() && calls.is_empty()) {
                        return Err(Overthought(limit as u64, Lost::new(&thinking, Some(began))).into());
                    }
                }
                if delta["tool_calls"].is_array() || !t.is_empty() {
                    spoke.get_or_insert_with(Instant::now);
                    first.get_or_insert_with(Instant::now);
                }
                if !t.is_empty() {
                    text += &t;
                    if answering {
                        markdown.push(&t);
                    } else if !narrated {
                        held += &t;
                        crate::ui::activity("writing");
                        if held.len() > 400 || held.contains("\n\n") || held.trim_start().starts_with(['#', '-', '|']) {
                            crate::ui::prose();
                            markdown.push(&std::mem::take(&mut held));
                            answering = true;
                        }
                    }
                }
                if !answering && !narrated && delta["tool_calls"].is_array() {
                    narrated = true;
                    crate::ui::intent(&held, &thinking);
                }
                for call in tools.into_iter().flatten() {
                    let (key, function) = (call["index"].as_u64(), &call["function"]);
                    let fresh = slots.get(&key).is_none_or(|&i: &usize| {
                        let [id, name, args] = &calls[i];
                        let new_id = call["id"].as_str().is_some_and(|i| !i.is_empty() && !id.is_empty() && i != id);
                        (key.is_none() || new_id)
                            && function["name"].is_string()
                            && !name.is_empty()
                            && serde_json::from_str::<Value>(args).is_ok()
                    });
                    if fresh {
                        calls.push(Default::default());
                        slots.insert(key, calls.len() - 1);
                    }
                    let [id, name, args] = &mut calls[slots[&key]];
                    if let Some(given) = call["id"].as_str().filter(|_| id.is_empty()) {
                        *id = given.into();
                    }
                    if let Some(part) = function["name"].as_str() {
                        if part.starts_with(name.as_str()) {
                            *name = part.into();
                        } else {
                            *name += part;
                        }
                    }
                    let part = match &function["arguments"] {
                        Value::String(part) => part.clone(),
                        Value::Null => continue,
                        part => part.to_string(),
                    };
                    *args += &part;
                    lines += part.matches("\\n").count();
                    crate::ui::writing(lines + calls.len());
                }
                std::io::stdout().flush()?;
            }
            buf.drain(..start);
            scan = buf.len();
            if scan > LINE_MAX {
                rejected = true;
                broken = Some(format!("a stream line passed {} MB", LINE_MAX >> 20));
                break;
            }
        }
        let open = inline.inside == Some(true) && via.is_none();
        let (inner, rest) = inline.finish();
        thinking += &inner;
        text += &rest;
        if answering {
            markdown.push(&rest);
        } else {
            held += &rest;
        }
        let close = text.find(CLOSE).filter(|&end| {
            via.is_none() && thinking.is_empty() && !text[..end].contains("<think") && !text[..end].contains('`')
        });
        if let Some(end) = close {
            let after = text[end + CLOSE.len()..].trim_start_matches([' ', '\t']);
            if after.is_empty() || after.starts_with(['\n', '\r']) {
                (thinking, text) = (text[..end].trim_end().to_string(), after.trim_start().to_string());
                if !answering && !narrated {
                    held = text.clone();
                }
            }
        }
        if open && broken.is_none() && reason != "length" {
            let inner = std::mem::take(&mut thinking);
            text += &inner;
            held += &inner;
        }
        let failed: Option<anyhow::Error> = match broken.as_deref() {
            Some(problem) if overflow(problem) => Some(ContextOverflow(problem.into()).into()),
            Some(problem) if rejected => Some(anyhow!("{problem}")),
            None if events == 0 && !finished => {
                Some(anyhow!("the provider did not send an event stream: {got} bytes without a single event"))
            }
            _ => None,
        };
        if let Some(e) = failed {
            if answering {
                markdown.finish();
            }
            return Err(e);
        }
        if finished {
            self.marked.set(true);
        }
        let accepted = !finished
            && broken.is_none()
            && !self.marked.get()
            && !(text.is_empty() && calls.is_empty())
            && calls.iter().all(|c| serde_json::from_str::<Value>(&c[2]).is_ok());
        if accepted {
            reason = "eof".into();
        }
        if !finished && !accepted {
            if answering {
                markdown.finish();
            }
            let why = broken.unwrap_or("the stream ended before the model finished".into());
            let mut lost = Lost::new(&thinking, began);
            lost.text = text.chars().count() + calls.iter().map(|c| c[2].len()).sum::<usize>();
            lost.stalled = stalled;
            return Err(Interrupted(why, lost).into());
        }
        if answering {
            markdown.finish();
        } else if !narrated && !held.trim().is_empty() {
            crate::ui::prose();
            markdown.push(&held);
            markdown.finish();
        }
        let ms = |a: Instant, b: Option<Instant>| b.map(|b| b.duration_since(a).as_millis() as u64);
        let timing =
            json!({"ttft_ms": ms(sent, first), "think_ms": began.and_then(|b| ms(b, spoke.or(Some(Instant::now()))))});
        let mut message =
            json!({"role": "assistant", "content": text, "meta": {"usage": usage, "timing": timing, "finish": reason}});
        if !thinking.is_empty() {
            message["reasoning_content"] = thinking.into();
        }
        if let Some(field) = via {
            self.field.set(field);
        }
        if !calls.is_empty() {
            message["tool_calls"] = calls
                .into_iter()
                .enumerate()
                .map(|(i, [id, name, args])| {
                    let args = if serde_json::from_str::<Value>(&args).is_ok() { args } else { "{}".into() };
                    let id = if id.is_empty() { format!("call_{i}") } else { id };
                    json!({"id": id, "type": "function", "function": {"name": name, "arguments": args}})
                })
                .collect();
        }
        Ok(message)
    }

    async fn send(&self, request: &reqwest::RequestBuilder) -> Result<(reqwest::Response, Instant)> {
        let mut timed_out = false;
        for attempt in 1.. {
            let sent = Instant::now();
            // None when the builder holds an error: a malformed base_url, or an API key that is no valid header.
            let request = request
                .try_clone()
                .context("could not build the request: check the provider's base_url and API key")?;
            let limit = if timed_out { IDLE } else { FIRST_BYTE };
            let (problem, detail, wait) = match tokio::time::timeout(limit, request.send()).await {
                Ok(Ok(r)) if r.status().is_success() => return Ok((r, sent)),
                Ok(Ok(r)) => {
                    let (status, after) = (r.status(), retry_after(r.headers()));
                    let text = head(r).await;
                    let retryable = status == 429 || status.is_server_error();
                    if status == 413 || !retryable && overflow(&text) {
                        return Err(ContextOverflow(format!("{status}: {text}")).into());
                    }
                    if !retryable || text.to_lowercase().contains(QUOTA) {
                        bail!("{status}: {text}")
                    }
                    (status.as_u16().to_string(), format!("{status}: {text}"), after)
                }
                Ok(Err(e)) => {
                    let problem = chain(e);
                    (problem.clone(), problem, None)
                }
                Err(_) if timed_out => {
                    let (first, second) = (FIRST_BYTE.as_secs(), IDLE.as_secs());
                    bail!("the provider stalled: no response within {first}s, then within {second}s");
                }
                Err(_) => {
                    timed_out = true;
                    let problem = format!("no response within {}s", FIRST_BYTE.as_secs());
                    (problem.clone(), problem, Some(1))
                }
            };
            if attempt == 5 {
                bail!("giving up after {attempt} attempts: {detail}")
            }
            let wait = wait.unwrap_or(2 << attempt).clamp(1, 60);
            let jitter = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            let extra = json!({"status": problem, "attempt": attempt, "wait_s": wait});
            crate::ui::event("retry", &format!("{problem}; retrying"), extra);
            tokio::time::sleep(Duration::from_secs(wait) + Duration::from_millis(jitter.subsec_millis() as u64)).await;
        }
        unreachable!()
    }
}

fn chain(e: reqwest::Error) -> String {
    format!("{:#}", anyhow::Error::from(e))
}

fn clip(text: &str) -> String {
    let text: String = text.trim().chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    match text.char_indices().nth(BODY_MAX) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

async fn head(mut response: reqwest::Response) -> String {
    let mut body = Vec::new();
    let read = async {
        while body.len() < BODY_MAX * 4
            && let Ok(Some(chunk)) = response.chunk().await
        {
            body.extend_from_slice(&chunk);
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(10), read).await;
    clip(&crate::agent::redact(&String::from_utf8_lossy(&body)).0)
}

fn refused(error: &Value) -> bool {
    let code = error["code"].as_u64().or(error["status"].as_u64());
    let named = format!("{} {}", error["type"], error["code"]).to_lowercase();
    code.is_some_and(|c| (400..500).contains(&c) && c != 408 && c != 429) || REJECTED.iter().any(|r| named.contains(r))
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers.get("retry-after")?.to_str().ok()?.trim().parse().ok()
}

#[derive(Default)]
struct Inline {
    inside: Option<bool>,
    held: String,
}

impl Inline {
    fn split(&mut self, t: &str) -> (String, String) {
        if self.inside == Some(false) {
            return (String::new(), t.into());
        }
        self.held += t;
        if self.inside.is_none() {
            let lead = self.held.trim_start();
            if OPEN.starts_with(lead) {
                return Default::default();
            }
            let Some(rest) = lead.strip_prefix(OPEN).map(String::from) else {
                self.inside = Some(false);
                return (String::new(), std::mem::take(&mut self.held));
            };
            (self.held, self.inside) = (rest, Some(true));
        }
        if let Some(end) = self.held.find(CLOSE) {
            self.inside = Some(false);
            let after = self.held[end + CLOSE.len()..].trim_start().to_string();
            self.held.truncate(end);
            return (std::mem::take(&mut self.held), after);
        }
        let keep = (1..CLOSE.len()).rev().find(|&k| self.held.ends_with(&CLOSE[..k])).unwrap_or(0);
        let tail = self.held.split_off(self.held.len() - keep);
        (std::mem::replace(&mut self.held, tail), String::new())
    }

    fn finish(&mut self) -> (String, String) {
        let held = std::mem::take(&mut self.held);
        if self.inside == Some(true) { (held, String::new()) } else { (String::new(), held) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_itself() {
        assert!(USER_AGENT.starts_with("air/"));
    }

    #[test]
    fn opencode_gets_a_session_id() {
        assert_eq!(
            session_id("https://opencode.ai/inference/go/v1", &json!({"id": "1792-1"})).as_deref(),
            Some("1792-1")
        );
        assert_eq!(session_id("https://opencode.ai/inference/go/v1", &json!({})), None);
        assert_eq!(session_id("https://api.primalabs.ai/v1", &json!({"id": "1792-1"})), None);
    }
}
