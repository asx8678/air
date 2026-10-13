//! The turn loop: one tool (run: code mode), and the verify gate.
use crate::llm::{self, Llm};
use crate::session::Session;
use crate::worker::Worker;
use crate::{Config, compact, ui};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Prompt tokens of the last request at which the history is compacted before the next one.
const COMPACT_AT: u64 = 100_000;
const GATE: &str = "check() has not passed on the current tree. Run check(), fix what fails, then finish again. \
                    If it cannot pass, say why in one line.";

pub struct Agent {
    root: PathBuf,
    config: Config,
    yolo: bool,
    llm: Llm,
    session: Session,
    worker: Option<Worker>,
}

impl Agent {
    pub fn new(root: &Path, home: &Path, config: Config, resume: bool, yolo: bool) -> Result<Self> {
        let session = if resume {
            Session::resume(home, root)?
        } else {
            let model =
                config.model.as_deref().context("no model: pass -m or set model in ~/.config/air/config.toml")?;
            Session::create(home, root, model, &system_prompt(root))?
        };
        let base_url = config.base_url.as_deref().context("set base_url in ~/.config/air/config.toml")?;
        let llm = Llm::new(base_url, &session.entries[0]);
        Ok(Self { root: root.into(), config, yolo, llm, session, worker: None })
    }

    /// One user turn; Ctrl-C stops it. True when the result is verified or nothing changed.
    pub async fn run(&mut self, task: &str) -> Result<bool> {
        let _bar = ui::Turn::start();
        let finished = tokio::select! {
            result = self.turn(task) => Some(result),
            _ = tokio::signal::ctrl_c() => None,
        };
        if let Some(result) = finished {
            return result;
        }
        println!();
        ui::note("interrupted");
        // Keep the worker (and its variables) when its interrupted run reports back; else restart it.
        if let Some(worker) = &mut self.worker
            && !worker.settle().await
        {
            self.worker = None;
        }
        Ok(false)
    }

    async fn turn(&mut self, task: &str) -> Result<bool> {
        let (began, clock) = (self.session.entries.len(), Instant::now());
        self.session.push(json!({"role": "user", "content": task}))?;
        let start = self.fingerprint();
        let (mut nudged, mut overflowed) = (false, false);
        loop {
            if self.session.last_prompt_tokens() >= COMPACT_AT {
                self.compact()?;
            }
            let message = match self.llm.complete(&self.session.messages(), &tools()).await {
                // Too long for this model: compact and retry once.
                Err(e) if e.is::<llm::ContextOverflow>() && !overflowed => {
                    overflowed = true;
                    ui::note("context window exceeded; compacting and retrying");
                    self.compact()?;
                    continue;
                }
                result => result?,
            };
            ui::usage(&message["meta"]["usage"]);
            let calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
            self.session.push(message)?;
            for call in &calls {
                let (content, meta) = self.call(call).await;
                self.session
                    .push(json!({"role": "tool", "tool_call_id": call["id"], "content": content, "meta": meta}))?;
            }
            if !calls.is_empty() {
                ui::activity("waiting for the model");
                continue;
            }
            // The verify gate: a turn that changed the tree is done only when check() passed on this tree.
            let now = self.fingerprint();
            let (changed, checked) = (now != start, self.session.state().check == Some(now));
            if changed && !checked && self.config.check.is_some() && !nudged {
                nudged = true;
                ui::note("changes not verified yet: asking the model to run check()");
                ui::activity("waiting for the model");
                self.session.push(json!({"role": "user", "content": GATE, "meta": {"host": true}}))?;
                continue;
            }
            return Ok(self.report(changed, checked, began, clock));
        }
    }

    async fn call(&mut self, call: &Value) -> (String, Value) {
        let args: Value =
            serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or_default()).unwrap_or_default();
        match (call["function"]["name"].as_str(), args["code"].as_str()) {
            (Some("run"), Some(code)) => self.run_code(code).await,
            _ => ("error: the only tool is run(code)".into(), json!({"tool": "run", "error": true})),
        }
    }

    async fn run_code(&mut self, code: &str) -> (String, Value) {
        ui::run(code);
        // The worker enforces accept-before-edit with state derived from the log.
        let state = self.session.state();
        let state = json!({"accepted": state.accepted, "carry": state.carry, "plan": state.plan});
        let reply = async { self.worker()?.run(code, &state).await }.await;
        let mut meta = match reply {
            Ok(meta) => meta,
            Err(e) => {
                self.worker = None;
                let out = format!("error: {e:#}; the worker was restarted and its variables are gone");
                ui::note(&out);
                return (out, json!({"tool": "run", "error": true}));
            }
        };
        let out = meta["out"].as_str().unwrap_or_default().to_string();
        ui::ran(&meta, &out);
        if meta["check"].is_boolean() {
            meta["fp"] = self.fingerprint().into();
        }
        if let Some(fields) = meta.as_object_mut() {
            fields.remove("out");
        }
        meta["tool"] = "run".into();
        (out, meta)
    }

    fn worker(&mut self) -> Result<&mut Worker> {
        if self.worker.is_none() {
            let (check, fmt) = (self.config.check.as_deref(), self.config.fmt.as_deref());
            self.worker = Some(Worker::start(&self.root, self.yolo, check, fmt, &self.session.path)?);
        }
        Ok(self.worker.as_mut().expect("just started"))
    }

    /// Identity of the working tree: path, size and mtime of every file git does not ignore.
    fn fingerprint(&self) -> String {
        let list = git(&self.root, &["ls-files", "-co", "--exclude-standard", "-z"]);
        let mut hasher = std::hash::DefaultHasher::new();
        for file in list.split('\0').filter(|f| !f.is_empty()) {
            file.hash(&mut hasher);
            std::fs::metadata(self.root.join(file)).ok().map(|m| (m.len(), m.modified().ok())).hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    /// The fixed-shape final report, written by the host rather than the model.
    fn report(&self, changed: bool, checked: bool, began: usize, clock: Instant) -> bool {
        let (check, verdict) = match (changed, checked, &self.config.check) {
            (false, _, _) => ("-", "NO CHANGES"),
            (true, true, _) => ("passed on the final tree", "VERIFIED"),
            (true, false, None) => ("none configured", "UNVERIFIED"),
            (true, false, Some(_)) => ("not passed after the last change", "UNVERIFIED"),
        };
        let state = self.session.state();
        let task = self.session.entries[began]["content"].as_str().unwrap_or_default();
        let mut rows = vec![("task", task.lines().next().unwrap_or_default().to_string())];
        rows.push(("criteria", state.accepted.unwrap_or_else(|| "-".into())));
        if let Some(steps) = state.plan["steps"].as_array() {
            let done = state.plan["done"].as_array().map_or(0, Vec::len);
            rows.push(("plan", format!("{done}/{} steps done", steps.len())));
        }
        rows.push(("check", check.into()));
        rows.push(("changes", changes(&self.root)));
        let turn = &self.session.entries[began..];
        let usage: Vec<&Value> = turn.iter().map(|e| &e["meta"]["usage"]).filter(|u| u.is_object()).collect();
        let prompt: u64 = usage.iter().filter_map(|u| u["prompt_tokens"].as_u64()).sum();
        let out: u64 = usage.iter().filter_map(|u| u["completion_tokens"].as_u64()).sum();
        let runs = turn.iter().filter(|e| e["meta"]["tool"] == "run").count();
        let secs = clock.elapsed().as_secs();
        let cost = format!(
            "{} requests · {runs} runs · {} in · {} out · {}:{:02}",
            usage.len(),
            ui::tokens(prompt),
            ui::tokens(out),
            secs / 60,
            secs % 60
        );
        rows.push(("cost", cost));
        ui::report(&rows, verdict, !changed || checked);
        !changed || checked
    }

    pub fn cost(&self) {
        let runs = self.session.entries.iter().filter(|e| e["meta"]["tool"] == "run");
        let (runs, rejected) = runs.fold((0, 0), |(n, r), e| (n + 1, r + usize::from(e["meta"]["rejected"] == true)));
        let (mut requests, mut tokens) = (0, [0; 3]); // prompt, cached, completion
        for u in self.session.entries.iter().map(|e| &e["meta"]["usage"]).filter(|u| u.is_object()) {
            let cached = u["prompt_tokens_details"]["cached_tokens"].as_u64();
            let cached = cached.or(u["cached_tokens"].as_u64()).or(u["prompt_cache_hit_tokens"].as_u64());
            for (sum, n) in
                tokens.iter_mut().zip([u["prompt_tokens"].as_u64(), cached, u["completion_tokens"].as_u64()])
            {
                *sum += n.unwrap_or(0);
            }
            requests += 1;
        }
        println!("{requests} requests · in {} (cached {}) · out {}", tokens[0], tokens[1], tokens[2]);
        println!("{runs} runs · {rejected} rejected before acting (syntax, unknown name, wrong arguments)");
    }

    pub fn compact(&mut self) -> Result<()> {
        let prev = self.session.keep();
        let keep = compact::cut(&self.session.entries, prev);
        if keep <= prev {
            ui::note("nothing to compact");
            return Ok(());
        }
        let summary = compact::summary(&self.session.entries, keep);
        ui::note(&format!("compacted entries #1-#{} into a {} byte summary", keep - 1, summary.len()));
        self.session.push(json!({"compact": {"keep": keep, "summary": summary}}))
    }
}

/// The immutable prefix: prompt, AGENTS.md and the skill index, pinned in the session header.
fn system_prompt(root: &Path) -> String {
    let mut prompt = include_str!("prompt.md").to_string();
    if let Ok(agents) = std::fs::read_to_string(root.join("AGENTS.md")) {
        prompt += "\n\n# Project instructions (AGENTS.md)\n\n";
        prompt += &agents;
    }
    let mut skills: Vec<String> = std::fs::read_dir(root.join(".agents/skills"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|dir| {
            let rel = format!(".agents/skills/{}/SKILL.md", dir.file_name().to_string_lossy());
            let text = std::fs::read_to_string(root.join(&rel)).ok()?;
            let about = text.lines().find_map(|l| l.strip_prefix("description:"))?.trim().to_string();
            Some(format!("- {rel}: {about}"))
        })
        .collect();
    skills.sort();
    if !skills.is_empty() {
        prompt += "\n\n# Skills (read() one when the task needs it)\n";
        prompt += &skills.join("\n");
    }
    prompt
}

fn tools() -> Value {
    json!([{"type": "function", "function": {"name": "run",
        "description": "Run Python in the persistent worker; only printed output comes back.",
        "parameters": {"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"]}}}])
}

/// Host-side git, with repo config unable to run commands through fsmonitor.
fn git(root: &Path, args: &[&str]) -> String {
    let out =
        std::process::Command::new("git").args(["-c", "core.fsmonitor=false"]).args(args).current_dir(root).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// Changed files with line counts, from git: "src/a.rs +3 −1 · notes.md (new)".
fn changes(root: &Path) -> String {
    let mut out: Vec<String> = git(root, &["diff", "--numstat", "HEAD"])
        .lines()
        .filter_map(|l| match l.splitn(3, '\t').collect::<Vec<_>>()[..] {
            [added, removed, path] => Some(format!("{path} +{added} −{removed}")),
            _ => None,
        })
        .collect();
    out.extend(git(root, &["ls-files", "-o", "--exclude-standard"]).lines().map(|f| format!("{f} (new)")));
    if out.is_empty() { "none".into() } else { out.join(" · ") }
}
