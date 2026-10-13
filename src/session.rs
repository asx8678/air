//! The append-only session log, the ground truth. Line n of the file is entry #n; line 0 is the
//! header that pins model, sampling and the system prompt. Compaction only appends a marker.
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Session {
    pub path: PathBuf,
    pub entries: Vec<Value>,
    file: File,
}

/// Agent state, derived from the log instead of being kept beside it.
#[derive(Default)]
pub struct State {
    /// accept() criteria of the current user turn
    pub accepted: Option<String>,
    /// tree fingerprint at the last check(), when that check passed
    pub check: Option<String>,
    /// carry() notes, kept in every compaction summary until cleared
    pub carry: Vec<String>,
    /// plan() steps and which are done, for the current user turn
    pub plan: Value,
}

impl Session {
    pub fn create(home: &Path, root: &Path, model: &str, system: &str) -> Result<Self> {
        let dir = home.join(".air/sessions");
        fs::create_dir_all(&dir)?;
        let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let path = dir.join(format!("{secs}-{}.jsonl", std::process::id()));
        let file = OpenOptions::new().create_new(true).append(true).open(&path)?;
        let mut session = Self { path, entries: vec![], file };
        session.push(json!({"cwd": root, "model": model, "temperature": crate::llm::TEMPERATURE, "system": system}))?;
        Ok(session)
    }

    /// The newest session started in this repo.
    pub fn resume(home: &Path, root: &Path) -> Result<Self> {
        let mut paths: Vec<PathBuf> = fs::read_dir(home.join(".air/sessions"))?.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths.into_iter().rev() {
            let first = BufReader::new(File::open(&path)?).lines().next().transpose()?.unwrap_or_default();
            if serde_json::from_str::<Value>(&first).is_ok_and(|header| header["cwd"] == json!(root)) {
                let entries = fs::read_to_string(&path)?.lines().map(serde_json::from_str).collect::<Result<_, _>>()?;
                eprintln!("[resumed {}]", path.display());
                return Ok(Self { file: OpenOptions::new().append(true).open(&path)?, path, entries });
            }
        }
        bail!("no session to resume for {}", root.display())
    }

    pub fn push(&mut self, entry: Value) -> Result<()> {
        writeln!(self.file, "{entry}")?;
        self.entries.push(entry);
        Ok(())
    }

    /// First entry kept raw by the last compaction (1 when nothing has been compacted).
    pub fn keep(&self) -> usize {
        self.entries.iter().rev().find_map(|e| e["compact"]["keep"].as_u64()).map_or(1, |k| k as usize)
    }

    pub fn last_prompt_tokens(&self) -> u64 {
        self.entries.iter().rev().find_map(|e| e["meta"]["usage"]["prompt_tokens"].as_u64()).unwrap_or(0)
    }

    /// The request: system prompt, the latest summary, then the raw entries from the cut on,
    /// with every tool call paired with exactly one result.
    pub fn messages(&self) -> Vec<Value> {
        let mut out = vec![json!({"role": "system", "content": self.entries[0]["system"]})];
        if let Some(summary) = self.entries.iter().rev().find_map(|e| e["compact"]["summary"].as_str()) {
            out.push(json!({"role": "user", "content": summary}));
        }
        let mut open: Vec<Value> = vec![]; // ids of calls still waiting for their result
        for entry in &self.entries[self.keep()..] {
            let Some(role) = entry["role"].as_str() else { continue }; // markers
            let mut message = entry.clone();
            if let Some(fields) = message.as_object_mut() {
                fields.remove("meta");
            }
            if role == "tool" {
                if let Some(i) = open.iter().position(|id| *id == message["tool_call_id"]) {
                    open.remove(i);
                    out.push(message);
                }
                continue;
            }
            answer_missing(&mut out, &mut open);
            if role == "assistant" {
                open = message["tool_calls"]
                    .as_array()
                    .map(|c| c.iter().map(|c| c["id"].clone()).collect())
                    .unwrap_or_default();
            }
            out.push(message);
        }
        answer_missing(&mut out, &mut open);
        out
    }

    pub fn state(&self) -> State {
        let mut state = State::default();
        for entry in &self.entries {
            if entry["role"] == "user" && entry["meta"]["host"].is_null() {
                state.accepted = None;
                state.plan = Value::Null;
            }
            let meta = &entry["meta"];
            if let Some(criteria) = meta["accepted"].as_str() {
                state.accepted = Some(criteria.into());
            }
            if meta["plan"].is_object() {
                state.plan = meta["plan"].clone();
            }
            if let Some(notes) = meta["carry"].as_array() {
                state.carry = notes.iter().filter_map(|n| n.as_str().map(String::from)).collect();
            }
            if let Some(passed) = meta["check"].as_bool() {
                state.check = passed.then(|| meta["fp"].as_str().unwrap_or_default().into());
            }
        }
        state
    }
}

/// An interrupted turn can leave calls without results, which providers reject.
fn answer_missing(out: &mut Vec<Value>, open: &mut Vec<Value>) {
    for id in open.drain(..) {
        out.push(json!({"role": "tool", "tool_call_id": id, "content": "tool result missing (interrupted)"}));
    }
}
