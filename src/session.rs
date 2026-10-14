use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Session {
    pub path: PathBuf,
    pub entries: Vec<Value>,
    file: File,
}

#[derive(Default)]
pub struct State {
    pub accepted: Option<String>,
    pub carry: Vec<String>,
    pub plan: Value,
}

impl Session {
    pub fn create(home: &Path, root: &Path, model: &str, system: &str) -> Result<Self> {
        let dir = home.join(".air/sessions");
        DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        let secs = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let path = dir.join(format!("{secs}-{}.jsonl", std::process::id()));
        let file = OpenOptions::new().create_new(true).append(true).mode(0o600).open(&path)?;
        file.try_lock()?;
        let mut session = Self { path, entries: vec![], file };
        session.push(json!({"cwd": root, "model": model, "temperature": crate::llm::TEMPERATURE, "system": system}))?;
        Ok(session)
    }

    pub fn resume(home: &Path, root: &Path) -> Result<Self> {
        let mut paths: Vec<PathBuf> = fs::read_dir(home.join(".air/sessions"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        paths.sort();
        for path in paths.into_iter().rev() {
            let first = BufReader::new(File::open(&path)?).lines().next().transpose()?.unwrap_or_default();
            if serde_json::from_str::<Value>(&first).is_ok_and(|header| header["cwd"] == json!(root)) {
                let mut file = OpenOptions::new().append(true).open(&path)?;
                if file.try_lock().is_err() {
                    bail!("{} is in use by another air", path.display());
                }
                let mut text = fs::read_to_string(&path)?;
                let cut = text.rfind('\n').map_or(0, |i| i + 1);
                if serde_json::from_str::<Value>(&text[cut..]).is_ok() {
                    file.write_all(b"\n")?;
                    text.push('\n');
                } else {
                    file.set_len(cut as u64)?;
                    text.truncate(cut);
                }
                let entries = text.lines().map(serde_json::from_str).collect::<Result<_, _>>()?;
                eprintln!("[resumed {}]", path.display());
                return Ok(Self { file, path, entries });
            }
        }
        bail!("no session to resume for {}", root.display())
    }

    pub fn push(&mut self, entry: Value) -> Result<()> {
        self.file.write_all(format!("{entry}\n").as_bytes())?;
        self.entries.push(entry);
        Ok(())
    }

    pub fn keep(&self) -> usize {
        self.entries.iter().rev().find_map(|e| e["compact"]["keep"].as_u64()).map_or(1, |k| k as usize)
    }

    pub fn last_prompt_tokens(&self) -> u64 {
        let current = self.entries.iter().rev().take_while(|e| e["compact"].is_null());
        current.map(|e| &e["meta"]["usage"]["prompt_tokens"]).find_map(Value::as_u64).unwrap_or(0)
    }

    fn newest_task(&self) -> Option<usize> {
        let user = |i: &usize| self.entries[*i]["role"] == "user" && self.entries[*i]["meta"]["host"].is_null();
        (1..self.entries.len()).rev().find(user)
    }

    pub fn messages(&self) -> Vec<Value> {
        let mut out = vec![json!({"role": "system", "content": self.entries[0]["system"]})];
        let (keep, newest) = (self.keep(), self.newest_task());
        if let Some(summary) = self.entries.iter().rev().find_map(|e| e["compact"]["summary"].as_str()) {
            let mut content = summary.to_string();
            if let Some(u) = newest.filter(|&u| u < keep) {
                let task = self.entries[u]["content"].as_str().unwrap_or_default();
                content += &format!("\n\n[Current task, entry #{u}]\n{task}");
            }
            out.push(json!({"role": "user", "content": content}));
        }
        let mut open: Vec<Value> = vec![];
        for (i, entry) in self.entries.iter().enumerate().skip(keep) {
            let Some(role) = entry["role"].as_str() else { continue };
            let mut message = entry.clone();
            if let Some(fields) = message.as_object_mut() {
                fields.remove("meta");
                if newest.is_some_and(|u| i < u) {
                    fields.remove("reasoning_content");
                }
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
        }
        state
    }
}

fn answer_missing(out: &mut Vec<Value>, open: &mut Vec<Value>) {
    for id in open.drain(..) {
        out.push(json!({"role": "tool", "tool_call_id": id, "content": "tool result missing (interrupted)"}));
    }
}
