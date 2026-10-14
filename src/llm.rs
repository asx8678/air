//! One OpenAI-compatible chat-completions client (GLM, Kimi, OpenRouter, local). We own the wire format.
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::io::Write;
use std::time::Duration;

/// Sampling for new sessions; the session header pins it.
pub const TEMPERATURE: f64 = 0.0;
/// How providers word a request that no longer fits the model's context window.
const OVERFLOW: &[&str] =
    &["context_length", "context length", "context window", "maximum context", "prompt is too long", "too many tokens"];

/// The request did not fit the model's context window.
#[derive(Debug)]
pub struct ContextOverflow(String);

impl std::fmt::Display for ContextOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "context window exceeded: {}", self.0)
    }
}

impl std::error::Error for ContextOverflow {}

/// The stream broke off before the model finished; nothing was logged, so the request is retried.
#[derive(Debug)]
struct Interrupted(String);

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "stream interrupted: {}", self.0)
    }
}

impl std::error::Error for Interrupted {}

/// Where the API key lives when AIR_API_KEY is unset; the sandbox denies reading it.
pub fn key_file() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/air/api_key")
}

pub struct Llm {
    http: reqwest::Client,
    url: String,
    key: Option<String>,
    model: String,
    temperature: f64,
}

impl Llm {
    /// `header` is the session's first log entry, which pins model and sampling.
    pub fn new(base_url: &str, header: &Value) -> Self {
        Self {
            // No overall timeout (a long answer is fine), but a connection silent for 5 minutes is dead.
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
        }
    }

    /// One streamed completion. Text is printed as it arrives; usage goes in the message's meta.
    pub async fn complete(&self, messages: &[Value], tools: &Value) -> Result<Value> {
        // serde_json maps are sorted, so the same history always serializes to the same bytes.
        let body = json!({
            "model": self.model, "messages": messages, "tools": tools, "temperature": self.temperature,
            "stream": true, "stream_options": {"include_usage": true},
        })
        .to_string();
        for attempt in 1.. {
            match self.stream(&body).await {
                Err(e) if attempt < 4 && e.is::<Interrupted>() => {
                    crate::ui::note(&format!("{e}; retrying from the start"));
                    tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
                }
                result => return result,
            }
        }
        unreachable!()
    }

    async fn stream(&self, body: &str) -> Result<Value> {
        let mut response = self.send(body).await?;
        let mut finished = false; // a finish_reason or [DONE] arrived: the model ended the reply
        let (mut buf, mut text, mut thinking, mut usage) = (Vec::new(), String::new(), String::new(), Value::Null);
        let mut calls: Vec<[String; 3]> = vec![]; // id, name, arguments
        let (mut markdown, mut began) = (crate::render::Markdown::default(), None);
        // Prose is held back until it is clearly an answer (long, or structured) or a tool call
        // arrives, which makes it narration for that step.
        let (mut held, mut answering, mut narrated) = (String::new(), false, false);
        // Always ends the reasoning block, so its live box is erased even for an empty one.
        let thought = |thinking: &str, began: Option<std::time::Instant>| {
            crate::ui::thought(thinking, began.map_or(0, |b: std::time::Instant| b.elapsed().as_secs()));
        };
        let mut broken = None; // the connection failed mid-body
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(e) => {
                    broken = Some(format!("{e:#}"));
                    break;
                }
            };
            buf.extend_from_slice(&chunk);
            while let Some(end) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=end).collect();
                let Some(data) = std::str::from_utf8(&line)?.trim().strip_prefix("data:") else { continue };
                finished |= data.trim() == "[DONE]";
                let Ok(event) = serde_json::from_str::<Value>(data.trim()) else { continue };
                if let Some(error) = event.get("error") {
                    bail!("provider error: {error}");
                }
                if event["usage"].is_object() {
                    usage = event["usage"].clone();
                }
                finished |= event["choices"][0]["finish_reason"].is_string();
                let delta = &event["choices"][0]["delta"];
                if let Some(t) = delta["reasoning_content"].as_str().or(delta["reasoning"].as_str()) {
                    thinking += t;
                    let began = *began.get_or_insert_with(std::time::Instant::now);
                    if text.is_empty() {
                        crate::ui::thinking(&thinking, began.elapsed().as_secs());
                    }
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
                    let secs = began.map_or(0, |b: std::time::Instant| b.elapsed().as_secs());
                    match held.trim() {
                        "" => crate::ui::thought(&thinking, secs),
                        prose => crate::ui::narration(prose, &thinking, secs),
                    }
                }
                for call in delta["tool_calls"].as_array().into_iter().flatten() {
                    // Some providers (Gemini) omit index: then a chunk starts a new call when it brings a
                    // different id, or a name once the current call's arguments are already complete.
                    let i = match (call["index"].as_u64(), calls.last()) {
                        (Some(i), _) => i as usize,
                        (None, None) => 0,
                        (None, Some([id, name, args])) => {
                            let new_id =
                                call["id"].as_str().is_some_and(|i| !i.is_empty() && !id.is_empty() && i != id);
                            let complete = !name.is_empty() && serde_json::from_str::<Value>(args).is_ok();
                            calls.len() - 1 + usize::from(new_id || call["function"]["name"].is_string() && complete)
                        }
                    };
                    if calls.len() <= i {
                        calls.resize(i + 1, Default::default());
                    }
                    if let Some(id) = call["id"].as_str() {
                        calls[i][0] = id.into();
                    }
                    if let Some(name) = call["function"]["name"].as_str() {
                        calls[i][1] = name.into();
                    }
                    if let Some(args) = call["function"]["arguments"].as_str() {
                        calls[i][2] += args;
                        crate::ui::writing(calls.iter().map(|c| c[2].matches("\\n").count() + 1).sum());
                    }
                }
                std::io::stdout().flush()?;
            }
        }
        // Some servers close without finish_reason or [DONE]; a clean close still counts as the end when
        // something arrived and every tool call's arguments are complete JSON.
        finished |= broken.is_none()
            && !(text.is_empty() && calls.is_empty())
            && calls.iter().all(|c| serde_json::from_str::<Value>(&c[2]).is_ok());
        if !finished {
            // What was shown of this attempt stays on screen; the retry starts over below it.
            if answering {
                markdown.finish();
            }
            return Err(Interrupted(broken.unwrap_or("the stream ended before the model finished".into())).into());
        }
        if answering {
            markdown.finish();
        } else if !narrated {
            // No tool call came: whatever was held back is the answer.
            thought(&thinking, began);
            if !held.trim().is_empty() {
                crate::ui::prose();
                markdown.push(&held);
                markdown.finish();
            }
        }
        let mut message = json!({"role": "assistant", "content": text, "meta": {"usage": usage}});
        if !thinking.is_empty() {
            message["reasoning_content"] = thinking.into();
        }
        if !calls.is_empty() {
            message["tool_calls"] = calls
                .into_iter()
                .enumerate()
                .map(|(i, [id, name, args])| {
                    // The log is replayed on every request, so it keeps only valid JSON arguments; a call
                    // whose arguments were cut off is logged with none, and its result says so.
                    let args = if serde_json::from_str::<Value>(&args).is_ok() { args } else { "{}".into() };
                    let id = if id.is_empty() { format!("call_{i}") } else { id };
                    json!({"id": id, "type": "function", "function": {"name": name, "arguments": args}})
                })
                .collect();
        }
        Ok(message)
    }

    /// POST, retrying rate limits, server errors and dropped connections.
    async fn send(&self, body: &str) -> Result<reqwest::Response> {
        for attempt in 1.. {
            let mut request =
                self.http.post(&self.url).header("content-type", "application/json").body(body.to_owned());
            if let Some(key) = &self.key {
                request = request.bearer_auth(key);
            }
            let problem = match request.send().await {
                Ok(r) if r.status().is_success() => return Ok(r),
                Ok(r) if r.status() == 429 || r.status().is_server_error() => r.status().to_string(),
                Ok(r) => {
                    let (status, text) = (r.status(), r.text().await.unwrap_or_default());
                    let lower = text.to_lowercase();
                    if OVERFLOW.iter().any(|m| lower.contains(m)) {
                        return Err(ContextOverflow(text).into());
                    }
                    bail!("{status}: {text}")
                }
                Err(e) => e.to_string(),
            };
            if attempt == 5 {
                bail!("giving up after {attempt} attempts: {problem}");
            }
            crate::ui::note(&format!("{problem}; retrying"));
            tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
        }
        unreachable!()
    }
}
