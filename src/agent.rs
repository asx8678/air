use crate::llm::{self, Llm};
use crate::session::Session;
use crate::worker::{self, Worker};
use crate::{Config, compact, ui};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::signal::unix::{SignalKind, signal};

const COMPACT_AT: u64 = 100_000;
const MAX_REQUESTS: usize = 200;
const CHECK_TIMEOUT: Duration = Duration::from_secs(1800);
const GATE: &str = "The project check failed on the current tree. Fix what fails, run check(), then finish again. \
                    If it cannot pass, say why in one line. The check output ended with:";

pub struct Agent {
    root: PathBuf,
    config: Config,
    yolo: bool,
    llm: Llm,
    session: Session,
    worker: Option<Worker>,
    compact_at: u64,
    pending: Option<Value>,
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
        let compact_at = session.entries.iter().rev().find_map(|e| e["compact"]["at"].as_u64()).unwrap_or(COMPACT_AT);
        Ok(Self { root: root.into(), config, yolo, llm, session, worker: None, compact_at, pending: None })
    }

    pub async fn run(&mut self, task: &str) -> Result<bool> {
        let bar = ui::Turn::start();
        let (mut term, mut hup) = (signal(SignalKind::terminate())?, signal(SignalKind::hangup())?);
        let mut quit = false;
        let finished = tokio::select! {
            result = self.turn(task) => Some(result),
            _ = tokio::signal::ctrl_c() => None,
            _ = term.recv() => { quit = true; None }
            _ = hup.recv() => { quit = true; None }
        };
        ui::clear_live();
        if quit {
            drop(bar);
            self.worker = None;
            std::process::exit(143);
        }
        if let Some(result) = finished {
            return result;
        }
        let settled = match &mut self.worker {
            Some(worker) => worker.settle().await,
            None => Ok(None),
        };
        let mut shown = false;
        match settled {
            Ok(Some(reply)) => {
                let (out, mut meta) = finish(reply);
                meta["interrupted"] = true.into();
                ui::ran(&meta, &out);
                shown = true;
                if let Some(id) = self.pending.take() {
                    let content = format!("{out}\n[interrupted by the user]");
                    self.session.push(json!({"role": "tool", "tool_call_id": id, "content": content, "meta": meta}))?;
                }
            }
            Ok(None) => {}
            Err(_) => self.worker = None,
        }
        if !shown {
            println!();
        }
        ui::note("interrupted");
        Ok(false)
    }

    async fn turn(&mut self, task: &str) -> Result<bool> {
        self.pending = None;
        if self.session.last_prompt_tokens() >= self.compact_at / 2 {
            self.compact_to(self.compact_at as usize / 10)?;
        }
        let (began, clock) = (self.session.entries.len(), Instant::now());
        let snapshot = self.snapshot();
        self.session.push(json!({"role": "user", "content": task, "meta": {"snapshot": snapshot}}))?;
        let start = self.fingerprint();
        let (mut nudged, mut overflowed) = (false, false);
        for _ in 0..MAX_REQUESTS {
            if self.session.last_prompt_tokens() >= self.compact_at {
                self.compact()?;
            }
            let message = match self.llm.complete(&self.session.messages(), &tools()).await {
                Err(e) if e.is::<llm::ContextOverflow>() && !overflowed => {
                    overflowed = true;
                    let seen = self.session.last_prompt_tokens();
                    if seen > 0 {
                        self.compact_at = self.compact_at.min(seen * 9 / 10);
                    }
                    ui::note("context window exceeded; compacting and retrying");
                    self.compact()?;
                    continue;
                }
                result => result?,
            };
            overflowed = false;
            ui::usage(&message["meta"]["usage"]);
            let calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
            self.session.push(message)?;
            for call in &calls {
                self.pending = Some(call["id"].clone());
                let (content, meta) = self.call(call).await;
                self.session
                    .push(json!({"role": "tool", "tool_call_id": call["id"], "content": content, "meta": meta}))?;
                self.pending = None;
            }
            if !calls.is_empty() {
                ui::activity("waiting for the model");
                continue;
            }
            let changed = self.fingerprint() != start;
            let mut checked = false;
            if changed && let Some(check) = &self.config.check {
                let (ok, out) = self.verify(check).await;
                if !ok && !nudged {
                    nudged = true;
                    ui::note("the check failed on the final tree: asking the model to fix it");
                    ui::activity("waiting for the model");
                    let gate = format!("{GATE}\n{}", compact::tail(&out, 1500));
                    self.session.push(json!({"role": "user", "content": gate, "meta": {"host": true}}))?;
                    continue;
                }
                checked = ok;
            }
            return Ok(self.report(changed, checked, began, clock));
        }
        ui::note(&format!("stopped after {MAX_REQUESTS} model requests in one turn"));
        self.report(self.fingerprint() != start, false, began, clock);
        Ok(false)
    }

    async fn call(&mut self, call: &Value) -> (String, Value) {
        let args: Value =
            serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or_default()).unwrap_or_default();
        match (call["function"]["name"].as_str(), args["code"].as_str()) {
            (Some("run"), Some(code)) => self.run_code(code).await,
            (Some("run"), None) => {
                ui::note("the model's run() came without code (cut off by the output limit?)");
                let out = "error: run() arrived without a code argument, probably cut off by the output limit; \
                           send it again in smaller steps";
                (out.into(), json!({"tool": "run", "error": true}))
            }
            (name, _) => {
                ui::note(&format!(
                    "the model called {}, but the only tool is run(code)",
                    name.unwrap_or("a nameless tool")
                ));
                ("error: the only tool is run(code)".into(), json!({"tool": "run", "error": true}))
            }
        }
    }

    async fn run_code(&mut self, code: &str) -> (String, Value) {
        ui::run(code);
        let state = self.session.state();
        let state = json!({"accepted": state.accepted, "carry": state.carry, "plan": state.plan});
        match async { self.worker()?.run(code, &state).await }.await {
            Ok(reply) => {
                let (out, meta) = finish(reply);
                ui::ran(&meta, &out);
                (out, meta)
            }
            Err(e) => {
                self.worker = None;
                let out = format!("error: {e:#}; the worker was restarted and its variables are gone");
                println!();
                ui::note(&out);
                (out, json!({"tool": "run", "error": true}))
            }
        }
    }

    fn worker(&mut self) -> Result<&mut Worker> {
        if self.worker.is_none() {
            let (check, fmt) = (self.config.check.as_deref(), self.config.fmt.as_deref());
            self.worker = Some(Worker::start(&self.root, self.yolo, check, fmt, &self.session.path)?);
        }
        Ok(self.worker.as_mut().expect("just started"))
    }

    async fn verify(&self, check: &str) -> (bool, String) {
        ui::run("check()");
        let before = self.fingerprint();
        let script = format!("exec 2>&1\n{check}");
        let cmd = worker::sandboxed(&self.root, self.yolo, &self.session.path, "/bin/sh", &["-c", &script]);
        let (passed, out) = match tokio::time::timeout(CHECK_TIMEOUT, grouped(cmd)).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => (false, format!("{e:#}")),
            Err(_) => (false, format!("timeout after {}s", CHECK_TIMEOUT.as_secs())),
        };
        let clean = self.fingerprint() == before;
        let ok = passed && clean;
        let why = if clean { check } else { "the check changed the working tree" };
        let verdict = if ok { "passed" } else { "FAILED" };
        let out = format!("{}\n[check {verdict}: {why}]", out.trim_end()).trim_start().to_string();
        ui::ran(&json!({"actions": [{"tool": "check", "ok": ok}]}), &out);
        (ok, out)
    }

    fn index(&self) -> PathBuf {
        self.session.path.with_extension("index")
    }

    fn snapshot(&self) -> Option<String> {
        let index = self.index();
        if !index.exists() {
            let current = git(&self.root, None, &["rev-parse", "--git-path", "index"]).unwrap_or_default();
            let _ = std::fs::copy(self.root.join(current.trim()), &index);
        }
        git(&self.root, Some(&index), &["add", "-A"])?;
        git(&self.root, Some(&index), &["write-tree"]).map(|tree| tree.trim().to_string())
    }

    fn fingerprint(&self) -> String {
        let list = git(&self.root, None, &["ls-files", "-co", "--exclude-standard", "-z"]).unwrap_or_default();
        let mut hasher = std::hash::DefaultHasher::new();
        for file in list.split('\0').filter(|f| !f.is_empty()) {
            file.hash(&mut hasher);
            std::fs::metadata(self.root.join(file)).ok().map(|m| (m.len(), m.modified().ok())).hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    fn report(&self, changed: bool, checked: bool, began: usize, clock: Instant) -> bool {
        let (check, verdict) = match (changed, checked, &self.config.check) {
            (false, _, _) => ("-", "NO CHANGES"),
            (true, true, _) => ("passed on the final tree", "VERIFIED"),
            (true, false, None) => ("none configured", "UNVERIFIED"),
            (true, false, Some(_)) => ("not passed on the final tree", "UNVERIFIED"),
        };
        let state = self.session.state();
        let first = &self.session.entries[began];
        let task = first["content"].as_str().unwrap_or_default();
        let mut rows = vec![("task", task.lines().next().unwrap_or_default().to_string())];
        rows.push(("criteria", state.accepted.unwrap_or_else(|| "-".into())));
        if let Some(steps) = state.plan["steps"].as_array() {
            let done = state.plan["done"].as_array().map_or(0, Vec::len);
            rows.push(("plan", format!("{done}/{} steps done", steps.len())));
        }
        rows.push(("check", check.into()));
        let snapshot = first["meta"]["snapshot"].as_str();
        let index = self.index();
        rows.push(("changes", changes(&self.root, snapshot.map(|tree| (tree, index.as_path())))));
        if let Some(tree) = snapshot.filter(|_| changed) {
            let short = tree.get(..12).unwrap_or(tree);
            rows.push(("undo", format!("git restore --source={short} --worktree -- .  (new files stay)")));
        }
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
        let (mut requests, mut tokens) = (0, [0; 3]);
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

    pub fn think(&self) {
        let last = self.session.entries.iter().rev().find_map(|e| e["reasoning_content"].as_str());
        let mut markdown = crate::render::Markdown::default();
        markdown.push(last.unwrap_or("no reasoning recorded yet"));
        markdown.finish();
    }

    pub fn compact(&mut self) -> Result<()> {
        self.compact_to(self.compact_at as usize / 5)
    }

    fn compact_to(&mut self, keep_tokens: usize) -> Result<()> {
        let prev = self.session.keep();
        let keep = compact::cut(&self.session.entries, prev, keep_tokens);
        if keep <= prev {
            ui::note("nothing to compact");
            return Ok(());
        }
        let summary = compact::summary(&self.session.entries, keep);
        ui::note(&format!("compacted entries #1-#{} into a {} byte summary", keep - 1, summary.len()));
        self.session.push(json!({"compact": {"keep": keep, "summary": summary, "at": self.compact_at}}))
    }
}

struct Group(Option<u32>);

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let group = format!("-{pid}");
            let _ = std::process::Command::new("kill").args(["-KILL", "--", &group]).stderr(Stdio::null()).status();
        }
    }
}

async fn grouped(mut cmd: tokio::process::Command) -> Result<(bool, String)> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).process_group(0).spawn()?;
    let group = Group(child.id());
    let mut stdout = child.stdout.take().context("check stdout")?;
    let mut out = vec![];
    let wait = async {
        let status = child.wait().await;
        drop(group);
        status
    };
    let (status, read) = tokio::join!(wait, stdout.read_to_end(&mut out));
    read?;
    Ok((status?.success(), String::from_utf8_lossy(&out).into_owned()))
}

fn finish(mut meta: Value) -> (String, Value) {
    let out = meta["out"].as_str().unwrap_or_default().to_string();
    if let Some(fields) = meta.as_object_mut() {
        fields.remove("out");
    }
    meta["tool"] = "run".into();
    (out, meta)
}

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
        prompt += "\n\n# Skills (open() one when the task needs it)\n";
        prompt += &skills.join("\n");
    }
    prompt
}

fn tools() -> Value {
    json!([{"type": "function", "function": {"name": "run",
        "description": "Run Python in the persistent worker; only printed output comes back.",
        "parameters": {"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"]}}}])
}

fn git(root: &Path, index: Option<&Path>, args: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["-c", "core.fsmonitor=false"]).args(args).current_dir(root);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn changes(root: &Path, snapshot: Option<(&str, &Path)>) -> String {
    let (base, index) = snapshot.map_or(("HEAD", None), |(tree, index)| (tree, Some(index)));
    let mut out: Vec<String> = git(root, index, &["diff", "--numstat", base])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| match l.splitn(3, '\t').collect::<Vec<_>>()[..] {
            [added, removed, path] => Some(format!("{path} +{added} −{removed}")),
            _ => None,
        })
        .collect();
    let new = git(root, index, &["ls-files", "-o", "--exclude-standard"]).unwrap_or_default();
    out.extend(new.lines().map(|f| format!("{f} (new)")));
    if out.is_empty() { "none".into() } else { out.join("\n") }
}
