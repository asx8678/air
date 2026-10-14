use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::io::Write;
use std::time::{Duration, Instant};

pub const TEMPERATURE: f64 = 0.0;
const OVERFLOW: &[&str] =
    &["context_length", "context length", "context window", "maximum context", "prompt is too long", "too many tokens"];

#[derive(Debug)]
pub struct ContextOverflow(String);

impl std::fmt::Display for ContextOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "context window exceeded: {}", self.0)
    }
}

impl std::error::Error for ContextOverflow {}

#[derive(Debug)]
struct Interrupted(String);

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "stream interrupted: {}", self.0)
    }
}

impl std::error::Error for Interrupted {}

fn overflow(text: &str) -> bool {
    let lower = text.to_lowercase();
    OVERFLOW.iter().any(|m| lower.contains(m))
}

pub fn key_file() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/air/api_key")
}

pub struct Llm {
    http: reqwest::Client,
    url: String,
    key: Option<String>,
    model: String,
    temperature: f64,
    marked: std::cell::Cell<bool>,
}

impl Llm {
    pub fn new(base_url: &str, header: &Value) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .read_timeout(Duration::from_secs(300))
                .build()
                .expect("a plain rustls client builds"),
            url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            key: std::env::var("AIR_API_KEY")
                .ok()
                .or_else(|| std::fs::read_to_string(key_file()).ok())
                .map(|k| k.trim().into()),
            model: header["model"].as_str().unwrap_or_default().into(),
            temperature: header["temperature"].as_f64().unwrap_or(TEMPERATURE),
            marked: Default::default(),
        }
    }

    pub async fn complete(&self, messages: &[Value], tools: &Value) -> Result<Value> {
        let body = json!({
            "model": self.model, "messages": messages, "tools": tools, "temperature": self.temperature,
            "stream": true, "stream_options": {"include_usage": true},
        })
        .to_string();
        let start = Instant::now();
        for attempt in 1.. {
            match self.stream(&body).await {
                Err(e) if attempt < 4 && e.is::<Interrupted>() => {
                    let text = format!("{e}; retrying from the start");
                    crate::ui::event("interrupted", &text, json!({"attempt": attempt}));
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

    async fn stream(&self, body: &str) -> Result<Value> {
        let (mut response, sent) = self.send(body).await?;
        let (mut first, mut spoke) = (None, None);
        let mut finished = false;
        let (mut buf, mut text, mut thinking, mut usage) = (Vec::new(), String::new(), String::new(), Value::Null);
        let mut calls: Vec<[String; 3]> = vec![];
        let mut slots = std::collections::HashMap::new();
        let mut lines = 0;
        let (mut markdown, mut began) = (crate::render::Markdown::default(), None);
        let (mut held, mut answering, mut narrated) = (String::new(), false, false);
        let thought = |thinking: &str, began: Option<Instant>| {
            crate::ui::thought(thinking, began.map_or(0, |b: Instant| b.elapsed().as_secs()));
        };
        let mut broken = None;
        'read: loop {
            match response.chunk().await {
                Ok(Some(chunk)) => buf.extend_from_slice(&chunk),
                Ok(None) if buf.last().is_some_and(|&b| b != b'\n') => buf.push(b'\n'),
                Ok(None) => break,
                Err(e) => {
                    broken = Some(format!("{e:#}"));
                    break;
                }
            }
            while let Some(end) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(data) = line.trim().strip_prefix("data:") else { continue };
                finished |= data.trim() == "[DONE]";
                let Ok(event) = serde_json::from_str::<Value>(data.trim()) else { continue };
                if let Some(error) = event.get("error") {
                    broken = Some(format!("provider error: {error}"));
                    break 'read;
                }
                if event["usage"].is_object() {
                    usage = event["usage"].clone();
                }
                finished |= event["choices"][0]["finish_reason"].is_string();
                let delta = &event["choices"][0]["delta"];
                if let Some(t) = delta["reasoning_content"].as_str().or(delta["reasoning"].as_str()) {
                    thinking += t;
                    let began = *began.get_or_insert_with(Instant::now);
                    first.get_or_insert(began);
                    if text.is_empty() {
                        crate::ui::thinking(&thinking, began.elapsed().as_secs());
                    }
                }
                if delta["tool_calls"].is_array() || delta["content"].as_str().is_some_and(|t| !t.is_empty()) {
                    spoke.get_or_insert_with(Instant::now);
                    first.get_or_insert_with(Instant::now);
                }
                if let Some(t) = delta["content"].as_str().filter(|t| !t.is_empty()) {
                    text += t;
                    if answering {
                        markdown.push(t);
                    } else if !narrated {
                        held += t;
                        crate::ui::activity("writing");
                        if held.len() > 400 || held.contains("\n\n") || held.trim_start().starts_with(['#', '-', '|']) {
                            thought(&thinking, began);
                            crate::ui::prose();
                            markdown.push(&std::mem::take(&mut held));
                            answering = true;
                        }
                    }
                }
                if !answering && !narrated && delta["tool_calls"].is_array() {
                    narrated = true;
                    let secs = began.map_or(0, |b: Instant| b.elapsed().as_secs());
                    match held.trim() {
                        "" => crate::ui::thought(&thinking, secs),
                        prose => crate::ui::narration(prose, &thinking, secs),
                    }
                }
                for call in delta["tool_calls"].as_array().into_iter().flatten() {
                    let key = call["index"].as_u64();
                    let fresh = slots.get(&key).is_none_or(|&i: &usize| {
                        let [id, name, args] = &calls[i];
                        let new_id = call["id"].as_str().is_some_and(|i| !i.is_empty() && !id.is_empty() && i != id);
                        let complete = !name.is_empty() && serde_json::from_str::<Value>(args).is_ok();
                        new_id || key.is_none() && call["function"]["name"].is_string() && complete
                    });
                    if fresh {
                        calls.push(Default::default());
                        slots.insert(key, calls.len() - 1);
                    }
                    let i = slots[&key];
                    if let Some(id) = call["id"].as_str() {
                        calls[i][0] = id.into();
                    }
                    if let Some(name) = call["function"]["name"].as_str() {
                        calls[i][1] = name.into();
                    }
                    if let Some(args) = call["function"]["arguments"].as_str() {
                        calls[i][2] += args;
                        lines += args.matches("\\n").count();
                        crate::ui::writing(lines + calls.len());
                    }
                }
                std::io::stdout().flush()?;
            }
        }
        if let Some(problem) = broken.as_deref().filter(|p| overflow(p)) {
            if answering {
                markdown.finish();
            }
            return Err(ContextOverflow(problem.to_string()).into());
        }
        if finished {
            self.marked.set(true);
        }
        finished |= broken.is_none()
            && !self.marked.get()
            && !(text.is_empty() && calls.is_empty())
            && calls.iter().all(|c| serde_json::from_str::<Value>(&c[2]).is_ok());
        if !finished {
            if answering {
                markdown.finish();
            }
            return Err(Interrupted(broken.unwrap_or("the stream ended before the model finished".into())).into());
        }
        if answering {
            markdown.finish();
        } else if !narrated {
            thought(&thinking, began);
            if !held.trim().is_empty() {
                crate::ui::prose();
                markdown.push(&held);
                markdown.finish();
            }
        }
        let ms = |a: Instant, b: Option<Instant>| b.map(|b| b.duration_since(a).as_millis() as u64);
        let timing =
            json!({"ttft_ms": ms(sent, first), "think_ms": began.and_then(|b| ms(b, spoke.or(Some(Instant::now()))))});
        let mut message = json!({"role": "assistant", "content": text, "meta": {"usage": usage, "timing": timing}});
        if !thinking.is_empty() {
            message["reasoning_content"] = thinking.into();
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

    async fn send(&self, body: &str) -> Result<(reqwest::Response, Instant)> {
        for attempt in 1.. {
            let sent = Instant::now();
            let mut request =
                self.http.post(&self.url).header("content-type", "application/json").body(body.to_owned());
            if let Some(key) = &self.key {
                request = request.bearer_auth(key);
            }
            let (problem, wait) = match request.send().await {
                Ok(r) if r.status().is_success() => return Ok((r, sent)),
                Ok(r) if r.status() == 429 || r.status().is_server_error() => {
                    let after = r.headers().get("retry-after").and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
                    (r.status().as_u16().to_string(), after)
                }
                Ok(r) => {
                    let (status, text) = (r.status(), r.text().await.unwrap_or_default());
                    if overflow(&text) {
                        return Err(ContextOverflow(text).into());
                    }
                    bail!("{status}: {text}")
                }
                Err(e) => (e.to_string(), None),
            };
            if attempt == 5 {
                bail!("giving up after {attempt} attempts: {problem}");
            }
            let wait = wait.unwrap_or(2 << attempt).min(60);
            let extra = json!({"status": problem, "attempt": attempt, "wait_s": wait});
            crate::ui::event("retry", &format!("{problem}; retrying"), extra);
            tokio::time::sleep(Duration::from_secs(wait)).await;
        }
        unreachable!()
    }
}
