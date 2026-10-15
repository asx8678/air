use crate::llm::{self, Llm};
use crate::providers::Providers;
use crate::session::Session;
use crate::worker::{self, Worker};
use crate::{Config, compact, ui};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const COMPACT_AT: u64 = 100_000;
const MAX_REQUESTS: usize = 200;
const REPEAT_WARN: usize = 3;
const REPEAT_BLOCK: usize = 6;
const FILES: usize = 200;
const AGENTS_MAX: usize = 32 * 1024;
const RESTARTS: usize = 2;
const WINDOW: &[&str] = &["context length is", "context length of", "context window is", "context window of"];
const CHECK_TIMEOUT: Duration = Duration::from_secs(1800);
const CHECK_OUT: u64 = 1 << 20;
const BIG: u64 = 16 << 20;
const TURN_TOKENS: u64 = 20_000_000;
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const SECRETS: &[(&str, &str, usize, &str)] = &[
    ("AKIA", "aws key", 16, ""),
    ("ghp_", "github token", 36, ""),
    ("gho_", "github token", 36, ""),
    ("github_pat_", "github token", 22, "_"),
    ("sk-", "api key", 20, "_-"),
    ("xoxb-", "slack token", 10, "-"),
    ("xoxp-", "slack token", 10, "-"),
    ("xoxa-", "slack token", 10, "-"),
    ("xoxs-", "slack token", 10, "-"),
];
const GATE: &str = "The project check failed on the current tree. Fix what fails, run check(), then finish again. \
                    If it cannot pass, say why in one line. The check output ended with:";
const NUDGE: &str = "Don't work out every change in advance: send one run() now with the next concrete step, \
                     and adjust after you see its output.";
const AGAIN: &str =
    "That happened twice in a row: send one run() now with the smallest next step, and reason less first.";
const LENGTH: &str = "Your last reply hit the output limit and was cut off. Continue from where it stopped, in smaller \
                      steps: one run() at a time, or a shorter answer.";
const HELPERS: &[&str] = &["sh", "show", "accept", "edit", "write", "check", "carry", "plan", "done", "recall"];
const SANDBOX: &str = "\n\n## Sandbox\nYour code runs in a sandbox. Writes outside the repo and temp dirs fail, as do \
                       writes into any .git directory and reads of credentials and private data in the home directory \
                       (ssh and cloud keys, shell history, browser data, other tools' configs). Only a fixed set of \
                       environment variables is passed through (env = [\"NAME\"] in ~/.config/air/config.toml adds more).";
static GIT_GUARD: OnceLock<(String, Vec<String>)> = OnceLock::new();
const MACOS: &str = " Internet and DNS are off (localhost works); nested sandbox-exec fails, and Unix sockets work only \
                     under the repo's tmp/ directory.";
const LINUX: &str = " The network is fully isolated: Internet, DNS and the host's localhost services are off; only \
                     servers you start inside the sandbox are reachable on localhost.";
const PERMANENT: &str = " These failures (Operation not permitted, EPERM, Read-only file system, DNS lookup errors) are \
                         permanent. Don't retry them and don't search the machine for another copy of what was \
                         blocked: skip that step, finish the rest, and say in your final message what you could not run.";

pub struct Agent {
    root: PathBuf,
    config: Config,
    yolo: bool,
    llm: Llm,
    provider: String,
    session: Session,
    worker: Option<Worker>,
    compact_at: u64,
    pending: Option<Value>,
    repeat: (u64, usize),
    current: Option<(usize, Instant)>,
    discarded: Option<(usize, String)>,
    restarts: usize,
    broken: Option<String>,
    passed: Option<(String, String)>,
}

impl Agent {
    pub fn new(root: &Path, home: &Path, config: Config, resume: bool, yolo: bool) -> Result<Self> {
        guard_git(root);
        let session = if resume {
            Session::resume(home, root)?
        } else {
            let model = config
                .model
                .clone()
                .or_else(|| config.providers.values().find_map(|p| p.models.first().cloned()))
                .context("no model: pass -m or set model in ~/.config/air/config.toml")?;
            let only = config.providers.keys().next().filter(|_| config.providers.len() == 1);
            let Some(provider) = holding(&config.providers, &model).or(only) else {
                let names: Vec<&str> = config.providers.keys().map(String::as_str).collect();
                anyhow::bail!(
                    "no provider for {model}: set base_url in ~/.config/air/config.toml or add one to \
                     ~/.config/air/providers.toml; to use one of [{}], list {model} under it there",
                    names.join(", ")
                );
            };
            let profile = config.models.get(&model).cloned().unwrap_or_default();
            let system = system_prompt(root, yolo, profile.get("guidance").and_then(Value::as_str));
            Session::create(home, root, provider, &model, &system, profile)?
        };
        let (provider, llm) = connect(&config.providers, session.header())?;
        let since = || session.entries.iter().rev().take_while(|e| e["switch"].is_null());
        if let Some(field) = since().find_map(|e| e["meta"]["reasoning_field"].as_str()) {
            llm.replay_as(field);
        }
        let learned = since().find_map(|e| e["compact"]["at"].as_u64());
        let compact_at = learned.or(session.header()["compact_at"].as_u64()).unwrap_or(COMPACT_AT);
        let repeat = (0, 0);
        Ok(Self {
            root: root.into(),
            config,
            yolo,
            llm,
            provider,
            session,
            worker: None,
            compact_at,
            pending: None,
            repeat,
            current: None,
            discarded: None,
            restarts: 0,
            broken: None,
            passed: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.session.path
    }

    pub fn model(&self) -> &str {
        self.session.header()["model"].as_str().unwrap_or_default()
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn set_providers(&mut self, providers: Providers) {
        self.config.providers = providers;
        match connect(&self.config.providers, self.session.header()) {
            Ok((provider, llm)) => (self.provider, self.llm) = (provider, llm),
            Err(e) => ui::note(&format!("{e:#}; still connected as before until a provider is added")),
        }
    }

    pub async fn models(&self) -> Vec<(String, String, bool)> {
        let mut known: Vec<(String, String)> = self
            .config
            .providers
            .iter()
            .flat_map(|(name, p)| p.models.iter().map(move |m| (name.clone(), m.clone())))
            .collect();
        match self.llm.models().await {
            Ok(listed) => known.extend(listed.into_iter().map(|m| (self.provider.clone(), m))),
            Err(e) => ui::note(&format!("could not list {}'s models: {e:#}", self.provider)),
        }
        let mut seen = std::collections::HashSet::new();
        known.retain(|pair| seen.insert(pair.clone()));
        known
            .into_iter()
            .map(|(p, m)| {
                let profiled = self.config.models.contains_key(&m);
                (p, m, profiled)
            })
            .collect()
    }

    pub fn switch(&mut self, provider: &str, model: &str) -> Result<()> {
        if !self.config.providers.contains_key(provider) {
            anyhow::bail!("no provider named {provider}");
        }
        let profile = self.config.models.get(model).cloned().unwrap_or_default();
        let system = system_prompt(&self.root, self.yolo, profile.get("guidance").and_then(Value::as_str));
        self.session.switch(provider, model, &system, profile)?;
        (self.provider, self.llm) = connect(&self.config.providers, self.session.header())?;
        self.compact_at = self.session.header()["compact_at"].as_u64().unwrap_or(COMPACT_AT);
        ui::note(&format!("now using {model} on {provider}"));
        Ok(())
    }

    pub async fn run(&mut self, task: &str) -> Result<bool> {
        let _bar = ui::Turn::start();
        let finished = tokio::select! {
            result = self.turn(task) => Some(result),
            _ = tokio::signal::ctrl_c() => None,
        };
        ui::clear_live();
        if let Some(result) = finished {
            if let Err(e) = &result {
                let text = ui::printable(&format!("{e:#}"));
                self.session.push(json!({"event": "error", "text": text, "at": ui::now_ms()}))?;
                if let Err(e) = self.unfinished() {
                    ui::note(&format!("could not write the turn report: {e:#}"));
                }
            }
            return result;
        }
        let settled = match self.worker.as_mut() {
            Some(worker) => tokio::select! {
                settled = worker.settle() => settled,
                _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("a second Ctrl-C stopped it")),
            },
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
            Err(e) => {
                self.worker = None;
                if let Some(id) = self.pending.take() {
                    let content = format!(
                        "[interrupted by the user] the run did not stop ({e:#}); the worker was restarted and its \
                         variables are gone"
                    );
                    let meta = json!({"tool": "run", "error": true, "interrupted": true});
                    self.session.push(json!({"role": "tool", "tool_call_id": id, "content": content, "meta": meta}))?;
                }
            }
        }
        if !shown {
            println!();
        }
        ui::event("ctrl_c", "interrupted", Value::Null);
        self.unfinished()?;
        Ok(false)
    }

    fn unfinished(&mut self) -> Result<()> {
        match self.current.take() {
            Some((began, clock)) => self.report(None, false, began, clock).map(drop),
            None => self.session.flush(),
        }
    }

    async fn turn(&mut self, task: &str) -> Result<bool> {
        (self.pending, self.repeat, self.restarts, self.broken, self.passed) = (None, (0, 0), 0, None, None);
        if self.session.last_prompt_tokens() >= self.compact_at / 2 {
            self.compact_to(self.compact_at as usize / 10)?;
        }
        self.session.flush()?;
        let (began, clock) = (self.session.entries.len(), Instant::now());
        let start = self.tree()?;
        self.session.push(json!({"role": "user", "content": task, "meta": {"snapshot": start}}))?;
        self.current = Some((began, clock));
        let (mut nudged, mut overflowed, mut nudges, mut cut) = (false, false, 0, false);
        let ceiling = self.session.header()["turn_tokens"].as_u64().unwrap_or(TURN_TOKENS);
        let mut spent = 0;
        let mut limit = format!("stopped after {MAX_REQUESTS} model requests in one turn");
        for _ in 0..MAX_REQUESTS {
            if spent >= ceiling {
                limit = format!("stopped after {} in one turn (turn_tokens = {ceiling})", ui::tokens(spent));
                break;
            }
            if self.session.last_prompt_tokens() >= self.compact_at {
                self.compact()?;
            }
            let messages = self.session.messages();
            let sent = serde_json::to_string(&messages).map_or(0, |m| m.len() as u64);
            let result = self.llm.complete(messages, &tools(), nudges < 2).await;
            if let Some(e) = result.as_ref().err()
                && let Some(lost) = llm::lost(e)
            {
                let why =
                    if e.is::<llm::Overthought>() { "it went over think_limit" } else { "the provider cut the stream" };
                self.discard(why, &lost.thinking);
            }
            let mut message = match result {
                Err(e) if e.is::<llm::ContextOverflow>() && !overflowed => {
                    overflowed = true;
                    self.overflow(&e.to_string())?;
                    continue;
                }
                Err(e) if e.is::<llm::ContextOverflow>() => {
                    let system = self.session.header()["system"].as_str().map_or(0, str::len);
                    let agents = std::fs::metadata(self.root.join("AGENTS.md")).map_or(0, |m| m.len() as usize);
                    anyhow::bail!(
                        "{e}; still over after compacting: the system prompt is {} KB, including {} KB of AGENTS.md",
                        system / 1024,
                        agents.min(AGENTS_MAX) / 1024
                    );
                }
                Err(e) if e.is::<llm::Overthought>() => {
                    nudges += 1;
                    ui::event("overthought", &e.to_string(), llm::lost(&e).map_or(Value::Null, llm::Lost::extra));
                    self.nudge(&format!("You {e}; that reasoning was discarded."))?;
                    continue;
                }
                Err(e) if e.is::<llm::Interrupted>() && nudges < 2 && llm::lost(&e).is_some_and(|l| !l.stalled) => {
                    nudges += 1;
                    let reasoning = llm::lost(&e).is_some_and(|l| !l.thinking.is_empty());
                    let during = if reasoning { " during long reasoning" } else { "" };
                    self.nudge(&format!("Your last reply was lost: the provider cut the stream{during}."))?;
                    continue;
                }
                result => result?,
            };
            overflowed = false;
            let usage = &mut message["meta"]["usage"];
            if !usage["prompt_tokens"].is_u64() {
                usage["prompt_tokens"] = (sent / 4).into();
                usage["estimated"] = true.into();
            }
            spent += usage["prompt_tokens"].as_u64().unwrap_or(0) + usage["completion_tokens"].as_u64().unwrap_or(0);
            ui::usage(&message["meta"]["usage"]);
            let calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
            if calls.is_empty() && message["content"].as_str().is_none_or(|c| c.trim().is_empty()) {
                let meta = &message["meta"];
                let extra =
                    json!({"meta": {"usage": meta["usage"], "timing": meta["timing"]}, "finish": meta["finish"]});
                if nudges < 2 {
                    nudges += 1;
                    ui::event("empty", "the model sent an empty reply; asking it to act", extra);
                    let thinking = message["reasoning_content"].as_str().unwrap_or_default();
                    self.discard("the reply was empty", thinking);
                    self.nudge("Your last reply was empty: you reasoned at length and sent nothing.")?;
                    continue;
                }
                ui::event("empty", "the model sent an empty reply again; ending the turn", extra);
            }
            if calls.is_empty() && message["meta"]["finish"] == "length" && !cut {
                cut = true;
                ui::event("length", "the reply hit the output limit; asking the model to continue", Value::Null);
                self.session.push(message)?;
                self.session.push(json!({"role": "user", "content": LENGTH, "meta": {"host": true}}))?;
                continue;
            }
            self.session.push(message)?;
            for call in &calls {
                self.pending = Some(call["id"].clone());
                let (content, meta) = self.call(call).await;
                self.session
                    .push(json!({"role": "tool", "tool_call_id": call["id"], "content": content, "meta": meta}))?;
                self.pending = None;
            }
            if let Some(why) = self.broken.take() {
                anyhow::bail!(why);
            }
            if !calls.is_empty() {
                ui::activity("waiting for the model");
                continue;
            }
            let changed = self.tree()? != start;
            let mut checked = false;
            if changed && let Some(check) = &self.config.check {
                let clock = Instant::now();
                let (ok, out) = self.verify(&check.clone(), true).await;
                let ms = clock.elapsed().as_millis() as u64;
                self.session.push(json!({"check": {"ok": ok, "ms": ms, "out": compact::tail(&out, 1500)}}))?;
                if !ok && !nudged {
                    nudged = true;
                    ui::event("nudge", "the check failed on the final tree: asking the model to fix it", Value::Null);
                    ui::activity("waiting for the model");
                    let gate = format!("{GATE}\n{}", compact::tail(&out, 1500));
                    self.session.push(json!({"role": "user", "content": gate, "meta": {"host": true}}))?;
                    continue;
                }
                checked = ok;
            }
            self.current = None;
            return self.report(Some(changed), checked, began, clock);
        }
        ui::event("limit", &limit, Value::Null);
        self.current = None;
        self.report(Some(self.moved(&start)), false, began, clock)?;
        Ok(false)
    }

    fn overflow(&mut self, error: &str) -> Result<()> {
        let seen = self.session.last_prompt_tokens();
        let entries = &self.session.entries;
        let compacted = entries.iter().rposition(|e| e["compact"].is_object()).map_or(0, |i| i + 1);
        let used = (compacted..entries.len()).rev().find(|&i| entries[i]["meta"]["usage"]["prompt_tokens"].is_u64());
        let estimate = match used {
            Some(i) => {
                let sent = entries[i..].iter().filter(|e| e["role"].is_string()).map(|e| {
                    let mut e = e.clone();
                    e.as_object_mut().map(|fields| fields.remove("meta"));
                    e.to_string().len() as u64
                });
                seen + sent.sum::<u64>() / 4
            }
            None => Value::from(self.session.messages()).to_string().len() as u64 / 4,
        };
        let window = window(error);
        self.compact_at = self.compact_at.min(window.unwrap_or(estimate) * 9 / 10);
        self.compact()?;
        let floor = Value::from(self.session.messages()).to_string().len() as u64 / 4;
        self.compact_at = self.compact_at.max(floor);
        let extra = json!({"seen": seen, "estimate": estimate, "window": window, "compact_at": self.compact_at});
        ui::event("overflow", "context window exceeded; compacted and retrying", extra);
        Ok(())
    }

    fn discard(&mut self, why: &str, thinking: &str) {
        if !thinking.trim().is_empty() {
            self.discarded = Some((self.session.entries.len(), format!("*Discarded: {why}.*\n\n{thinking}")));
        }
    }

    fn nudge(&mut self, why: &str) -> Result<()> {
        let last = self.session.entries.iter().rev().find(|e| e["role"].is_string());
        let again = last.is_some_and(|e| e["meta"]["nudge"] == true);
        ui::event("nudge", why, Value::Null);
        let content = format!("{why} {}", if again { AGAIN } else { NUDGE });
        self.session.push(json!({"role": "user", "content": content, "meta": {"host": true, "nudge": true}}))
    }

    async fn call(&mut self, call: &Value) -> (String, Value) {
        let args: Value =
            serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or_default()).unwrap_or_default();
        match (call["function"]["name"].as_str(), args["code"].as_str()) {
            (Some("run"), Some(code)) => self.run_code(code).await,
            (Some("run"), None) => {
                ui::event(
                    "bad_call",
                    "the model's run() came without code (cut off by the output limit?)",
                    Value::Null,
                );
                let out = "error: run() arrived without a code argument, probably cut off by the output limit; \
                           send it again in smaller steps";
                (out.into(), json!({"tool": "run", "error": true}))
            }
            (Some(name), _) if HELPERS.contains(&name) => {
                let args = call["function"]["arguments"].as_str().unwrap_or("{}");
                let literal = serde_json::to_string(args).unwrap_or_default();
                ui::event("bad_call", &format!("the model called {name} as a tool; running it in run()"), Value::Null);
                self.run_code(&format!("{name}(**__import__('json').loads({literal}))")).await
            }
            (name, _) => {
                let text =
                    format!("the model called {}, but the only tool is run(code)", name.unwrap_or("a nameless tool"));
                ui::event("bad_call", &text, Value::Null);
                ("error: the only tool is run(code)".into(), json!({"tool": "run", "error": true}))
            }
        }
    }

    async fn run_code(&mut self, code: &str) -> (String, Value) {
        let mut hasher = std::hash::DefaultHasher::new();
        code.hash(&mut hasher);
        let key = hasher.finish();
        self.repeat = (key, if self.repeat.0 == key { self.repeat.1 + 1 } else { 1 });
        if self.repeat.1 >= REPEAT_BLOCK {
            let out = format!(
                "error: this exact code already ran {} times in a row, so it was blocked: change the approach or finish",
                self.repeat.1 - 1
            );
            ui::event("repeat", &out, Value::Null);
            return (out, json!({"tool": "run", "error": true, "rejected": true}));
        }
        ui::run(code);
        let state = self.session.state();
        let state = json!({"accepted": state.accepted, "carry": state.carry, "plan": state.plan});
        let clock = Instant::now();
        let ran = async {
            let mut reply = self.worker()?.run(code, &state).await?;
            while reply["needs_check"] == true {
                let (ok, out) = match self.config.check.clone() {
                    Some(check) => self.verify(&check, false).await,
                    None => (false, "no check command configured (set check in air.toml)".into()),
                };
                reply = self.worker()?.resume(ok, &out).await?;
            }
            anyhow::Ok(reply)
        };
        match ran.await {
            Ok(reply) => {
                self.restarts = 0;
                let (out, mut meta) = finish(reply);
                let (mut out, hidden) = redact(&out);
                if hidden > 0 {
                    meta["redacted"] = hidden.into();
                }
                if self.repeat.1 >= REPEAT_WARN {
                    let n = self.repeat.1;
                    out +=
                        &format!("\n[the same code has now run {n} times in a row; at {REPEAT_BLOCK} it is blocked]");
                }
                meta["ms"] = (clock.elapsed().as_millis() as u64).into();
                ui::ran(&meta, &out);
                (out, meta)
            }
            Err(e) => {
                self.worker = None;
                self.restarts += usize::from(e.downcast_ref::<worker::StartFailed>().is_some());
                if self.restarts >= RESTARTS {
                    self.broken = Some(format!("{e:#} ({RESTARTS} times in a row; ending the turn)"));
                }
                let out = format!("error: {e:#}; the worker was restarted and its variables are gone");
                println!();
                ui::event("worker_restart", &out, Value::Null);
                (out, json!({"tool": "run", "error": true, "ms": clock.elapsed().as_millis() as u64}))
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

    async fn verify(&mut self, check: &str, gate: bool) -> (bool, String) {
        let before = self.tree();
        if let (Ok(tree), Some((passed, out))) = (&before, &self.passed)
            && tree == passed
        {
            let out = format!("{out}\n[reused: the tree is unchanged since this check passed]");
            if gate {
                ui::ran(&json!({"actions": [{"tool": "check", "ok": true}]}), &out);
            }
            return (true, out);
        }
        if gate {
            ui::run("check()");
        } else {
            ui::activity("running check()");
        }
        let script = format!("exec 2>&1\n{check}");
        let cmd = worker::sandboxed(&self.root, self.yolo, &self.session.path, "/bin/sh", &["-c", &script]);
        let (passed, out) = match tokio::time::timeout(CHECK_TIMEOUT, grouped(cmd)).await {
            Ok(Ok((passed, out))) => (passed, redact(&out).0),
            Ok(Err(e)) => (false, format!("{e:#}")),
            Err(_) => (false, format!("timeout after {}s", CHECK_TIMEOUT.as_secs())),
        };
        let after = self.tree();
        let why = match (&before, &after) {
            (Err(e), _) | (_, Err(e)) => format!("{e:#}"),
            (Ok(a), Ok(b)) if a != b => "the check changed the working tree".into(),
            _ => check.into(),
        };
        let ok = passed && matches!((&before, &after), (Ok(a), Ok(b)) if a == b);
        let verdict = if ok { "passed" } else { "FAILED" };
        let out = format!("{}\n[check {verdict}: {why}]", out.trim_end()).trim_start().to_string();
        if gate {
            ui::ran(&json!({"actions": [{"tool": "check", "ok": ok}]}), &out);
        }
        self.passed = after.ok().filter(|_| ok).map(|tree| (tree, out.clone()));
        (ok, out)
    }

    fn index(&self) -> PathBuf {
        self.session.path.with_extension("index")
    }

    fn tree(&self) -> Result<String> {
        let index = self.index();
        if !index.exists() {
            let current = git(&self.root, None, &["rev-parse", "--git-path", "index"]).unwrap_or_default();
            let _ = std::fs::copy(self.root.join(current.trim()), &index);
        }
        let others = git(&self.root, Some(&index), &["ls-files", "-z", "-o", "--exclude-standard"])?;
        let mut args = vec!["add".to_string(), "-A".into(), "--".into(), ".".into()];
        let mut hasher = std::hash::DefaultHasher::new();
        for path in others.split('\0').filter(|p| !p.is_empty()) {
            let Ok(meta) = std::fs::symlink_metadata(self.root.join(path)) else { continue };
            if meta.len() > BIG {
                (path, meta.len(), meta.modified().ok()).hash(&mut hasher);
                args.push(format!(":(exclude,literal){path}"));
            }
        }
        git(&self.root, Some(&index), &args.iter().map(String::as_str).collect::<Vec<_>>())?;
        let tree = git(&self.root, Some(&index), &["write-tree"])?.trim().to_string();
        Ok(if args.len() > 4 { format!("{tree} {:016x}", hasher.finish()) } else { tree })
    }

    fn moved(&self, start: &str) -> bool {
        self.tree().map_or_else(
            |e| {
                ui::event("error", &format!("could not tell whether the tree changed: {e:#}"), Value::Null);
                true
            },
            |tree| tree != start,
        )
    }

    fn report(&mut self, changed: Option<bool>, checked: bool, began: usize, clock: Instant) -> Result<bool> {
        self.session.flush()?;
        let first = &self.session.entries[began];
        let snapshot = first["meta"]["snapshot"].as_str().and_then(|s| s.split(' ').next());
        let index = self.index();
        let since = std::time::SystemTime::now() - clock.elapsed();
        let changes = changes(&self.root, snapshot.map(|tree| (tree, index.as_path())), since);
        let changed = changed.unwrap_or(changes != "none");
        let path =
            |l: &str| l.strip_suffix(" (new)").or_else(|| l.rsplit_once(" +").map(|(path, _)| path)).map(String::from);
        let turn = &self.session.entries[began..];
        let actions = turn.iter().flat_map(|e| e["meta"]["actions"].as_array().into_iter().flatten());
        let own: Vec<&str> = actions.filter(|a| a["ok"] == true).filter_map(|a| a["path"].as_str()).collect();
        let (mine, outside): (Vec<&str>, Vec<&str>) =
            changes.lines().filter(|l| *l != "none").partition(|l| path(l).is_some_and(|p| own.contains(&p.as_str())));
        let outside: Vec<String> = outside.into_iter().filter_map(path).collect();
        let named = |path: &str| {
            let words = self
                .config
                .check
                .as_deref()
                .unwrap_or_default()
                .split(|c: char| c.is_whitespace() || "'\"();&|<>=`".contains(c));
            path == "air.toml" || words.map(|w| w.trim_start_matches("./")).any(|w| w == path)
        };
        let inputs: Vec<String> = changes.lines().filter_map(path).filter(|p| named(p)).collect();
        let (check, verdict) = match (changed, checked, &self.config.check) {
            (false, _, _) => ("-", "NO CHANGES"),
            (true, true, _) if !inputs.is_empty() => ("passed, but this turn changed what it runs", "UNVERIFIED"),
            (true, true, _) => ("passed on the final tree", "VERIFIED"),
            (true, false, None) => ("none configured: set check in air.toml", "UNVERIFIED"),
            (true, false, Some(_)) => ("not passed on the final tree", "UNVERIFIED"),
        };
        let checked = checked && inputs.is_empty();
        let state = self.session.state();
        let task = first["content"].as_str().unwrap_or_default();
        let mut rows = vec![("task", task.lines().next().unwrap_or_default().to_string())];
        rows.push(("criteria", state.accepted.clone().unwrap_or_else(|| "-".into())));
        let steps = state.plan["steps"].as_array().map(Vec::len);
        let done = state.plan["done"].as_array().map_or(0, Vec::len);
        if let Some(steps) = steps {
            rows.push(("plan", format!("{done}/{steps} steps done")));
        }
        rows.push(("check", check.into()));
        if !inputs.is_empty() {
            rows.push(("inputs", format!("check inputs changed: {}", inputs.join(", "))));
        }
        rows.push(("changes", changes.clone()));
        if !outside.is_empty() {
            let text = format!("not through air's edit()/write(), maybe outside air: {}", outside.join(", "));
            rows.push(("outside", text));
        }
        if let Some(tree) = snapshot.filter(|_| changed) {
            let short = tree.get(..12).unwrap_or(tree);
            let mine: Vec<String> = mine.into_iter().filter_map(path).collect();
            let mut args = vec!["--literal-pathspecs", "ls-tree", "-z", "--name-only", tree, "--"];
            args.extend(mine.iter().map(String::as_str));
            let known = if mine.is_empty() { String::new() } else { git(&self.root, None, &args).unwrap_or_default() };
            let modified: Vec<&str> = known.split('\0').filter(|p| !p.is_empty()).collect();
            let undo = match (outside.is_empty(), modified.is_empty()) {
                (true, _) => format!("git restore --source={short} --worktree -- .  (new files stay)"),
                (false, false) => format!(
                    "git restore --source={short} --worktree -- {}  (air's edit()/write() paths; new files stay)",
                    modified.iter().map(|p| quoted(p)).collect::<Vec<_>>().join(" ")
                ),
                (false, true) => format!("git restore --source={short} --worktree -- <path>  (new files stay)"),
            };
            rows.push(("undo", undo));
        }
        let usage: Vec<&Value> = turn.iter().map(|e| &e["meta"]["usage"]).filter(|u| u.is_object()).collect();
        let prompt: u64 = usage.iter().filter_map(|u| u["prompt_tokens"].as_u64()).sum();
        let out: u64 = usage.iter().filter_map(|u| u["completion_tokens"].as_u64()).sum();
        let cached: u64 = usage.iter().filter_map(|u| ui::cached(u)).sum();
        let runs = turn.iter().filter(|e| e["meta"]["tool"] == "run").count();
        let ms = clock.elapsed().as_millis() as u64;
        let secs = ms / 1000;
        let lost = turn.iter().filter(|e| matches!(e["event"].as_str(), Some("overthought" | "interrupted")));
        let requests = usage.len() + lost.count();
        let cost = format!(
            "{requests} requests · {runs} runs · {} in · {} out · {}:{:02}",
            ui::tokens(prompt),
            ui::tokens(out),
            secs / 60,
            secs % 60
        );
        rows.push(("cost", cost));
        ui::report(&rows, verdict, !changed || checked);
        let changes: Vec<&str> = changes.lines().filter(|l| *l != "none").collect();
        self.session.push(json!({"report": {
            "verdict": verdict, "changed": changed, "checked": checked, "check_inputs": inputs, "outside": outside, "criteria": state.accepted,
            "plan_steps": steps, "plan_done": done, "changes": changes, "requests": requests, "runs": runs,
            "prompt": prompt, "cached": cached, "out": out, "ms": ms,
        }}))?;
        Ok(!changed || checked)
    }

    pub fn cost(&self) {
        let runs = self.session.entries.iter().filter(|e| e["meta"]["tool"] == "run");
        let (runs, rejected) = runs.fold((0, 0), |(n, r), e| (n + 1, r + usize::from(e["meta"]["rejected"] == true)));
        let (mut requests, mut tokens) = (0, [0; 3]);
        for u in self.session.entries.iter().map(|e| &e["meta"]["usage"]).filter(|u| u.is_object()) {
            let cached = ui::cached(u);
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
        let entries = self.session.entries.iter().enumerate().rev();
        let stored = entries.filter_map(|(i, e)| e["reasoning_content"].as_str().map(|r| (i, r))).next();
        let last = match (&self.discarded, stored) {
            (Some((at, lost)), stored) if stored.is_none_or(|(i, _)| *at > i) => Some(lost.as_str()),
            (_, stored) => stored.map(|(_, r)| r),
        };
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

struct Group(Vec<i32>);

impl Drop for Group {
    fn drop(&mut self) {
        ui::CHECK.store(0, Ordering::Relaxed);
        let own = unsafe { libc::getpgrp() };
        for &group in self.0.iter().filter(|&&g| g > 1 && g != own) {
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
    }
}

fn holding<'a>(providers: &'a Providers, model: &str) -> Option<&'a String> {
    providers.iter().find(|(_, p)| p.models.iter().any(|m| m == model)).map(|(name, _)| name)
}

fn connect(providers: &Providers, header: &Value) -> Result<(String, Llm)> {
    let model = header["model"].as_str().unwrap_or_default();
    let named = header["provider"].as_str().filter(|n| providers.contains_key(*n));
    let name = match named.or(holding(providers, model).map(String::as_str)) {
        Some(name) => name,
        None => {
            let first = providers.keys().next().with_context(
                || "no provider: set base_url in ~/.config/air/config.toml or add one to ~/.config/air/providers.toml",
            )?;
            if providers.len() > 1 || header["provider"].is_string() {
                ui::note(&format!("no provider lists {model}; using {first} (/models or /providers changes it)"));
            }
            first.as_str()
        }
    };
    let p = &providers[name];
    Ok((name.into(), Llm::new(&p.base_url, p.api_key.as_deref(), header)))
}

async fn grouped(mut cmd: tokio::process::Command) -> Result<(bool, String)> {
    let path = std::env::temp_dir().join(format!("air-check-{}-{}", std::process::id(), ui::now_ms()));
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&path)?;
    let _ = std::fs::remove_file(&path);
    let mut child = cmd.stdin(Stdio::null()).stdout(file.try_clone()?).process_group(0).spawn()?;
    let pid = child.id().context("the check exited before it started")? as i32;
    ui::CHECK.store(pid, Ordering::Relaxed);
    let mut group = Group(vec![pid]);
    let status = loop {
        tokio::select! {
            status = child.wait() => break status?,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                let seen = worker::descendants(pid);
                group.0.extend(seen.into_iter().filter(|g| !group.0.contains(g)).collect::<Vec<_>>());
            }
        }
    };
    group.0.extend(worker::descendants(pid));
    drop(group);
    let size = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(size.saturating_sub(CHECK_OUT)))?;
    let mut out = vec![];
    file.read_to_end(&mut out)?;
    let out = String::from_utf8_lossy(&out);
    let out = if size > CHECK_OUT { format!("[… {} bytes omitted]\n{out}", size - CHECK_OUT) } else { out.into() };
    Ok((status.success(), out))
}

fn redact(text: &str) -> (String, usize) {
    let (mut out, mut hidden, mut copied, mut i) = (String::new(), 0, 0, 0);
    let bytes = text.as_bytes();
    while i < bytes.len() {
        let boundary = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || matches!(bytes[i - 1], b'_' | b'-'));
        if boundary
            && matches!(bytes[i], b'A' | b'g' | b's' | b'x' | b'-')
            && let Some((len, kind)) = secret(&text[i..])
        {
            out += &text[copied..i];
            out += &format!("[redacted {kind}]");
            (hidden, i) = (hidden + 1, i + len);
            copied = i;
            continue;
        }
        i += 1;
    }
    if hidden == 0 {
        return (text.into(), 0);
    }
    out += &text[copied..];
    (out, hidden)
}

fn secret(s: &str) -> Option<(usize, &'static str)> {
    if s.starts_with("-----BEGIN ") && s.lines().next().is_some_and(|l| l.contains("PRIVATE KEY-----")) {
        let end = s
            .find("-----END ")
            .map_or(s.len(), |e| e + 9 + s[e + 9..].find("-----").map_or(s.len() - e - 9, |k| k + 5));
        return Some((end, "private key"));
    }
    SECRETS.iter().find_map(|&(prefix, kind, min, extra)| {
        let body = s.strip_prefix(prefix)?;
        let upper = prefix == "AKIA";
        let allowed = |c: char| c.is_ascii_alphanumeric() && !(upper && c.is_ascii_lowercase()) || extra.contains(c);
        let len = body.find(|c| !allowed(c)).unwrap_or(body.len());
        (len >= min).then_some((prefix.len() + len, kind))
    })
}

fn quoted(path: &str) -> String {
    if path.chars().all(|c| c.is_ascii_alphanumeric() || "._/-+".contains(c)) {
        path.into()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

fn guard_git(root: &Path) {
    let out = std::process::Command::new("git")
        .args(["config", "--show-scope", "-z", "--get-regexp", r"^(filter|diff)\."])
        .current_dir(root)
        .process_group(0)
        .output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let fields: Vec<&str> = text.split('\0').collect();
    let (mut args, mut local) = (vec![], vec![]);
    for pair in fields.chunks(2) {
        let [scope, entry] = pair else { continue };
        let key = entry.split('\n').next().unwrap_or_default();
        let Some((kind, name)) = key.split_once('.').and_then(|(kind, rest)| Some((kind, rest.rsplit_once('.')?.0)))
        else {
            continue;
        };
        if *scope == "local" && !local.contains(&name) {
            local.push(name);
        }
        if kind == "filter" && !name.contains(['=', '\n']) && !args.contains(&format!("filter.{name}.clean=")) {
            for var in ["clean=", "smudge=", "process=", "required=false"] {
                args.extend(["-c".to_string(), format!("filter.{name}.{var}")]);
            }
        }
    }
    if !local.is_empty() {
        ui::note(&format!(
            ".git/config defines filter or diff drivers ({}); air's own git calls skip them",
            local.join(", ")
        ));
    }
    let empty =
        std::process::Command::new("git").args(["hash-object", "-t", "tree", "/dev/null"]).current_dir(root).output();
    let empty = empty.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let _ = GIT_GUARD.set((if empty.is_empty() { EMPTY_TREE.into() } else { empty }, args));
}

fn finish(mut meta: Value) -> (String, Value) {
    let out = meta["out"].as_str().unwrap_or_default().to_string();
    if let Some(fields) = meta.as_object_mut() {
        fields.remove("out");
    }
    meta["tool"] = "run".into();
    (out, meta)
}

fn system_prompt(root: &Path, yolo: bool, notes: Option<&str>) -> String {
    let mut prompt = include_str!("prompt.md").to_string();
    if !yolo {
        prompt += SANDBOX;
        prompt += if cfg!(target_os = "macos") { MACOS } else { LINUX };
        prompt += PERMANENT;
    }
    if let Some(notes) = notes.map(str::trim).filter(|n| !n.is_empty()) {
        prompt += "\n\n## Notes for this model\n";
        prompt += notes;
    }
    if let Ok(agents) = std::fs::read_to_string(root.join("AGENTS.md")) {
        prompt += "\n\n# Project instructions (AGENTS.md)\n\n";
        prompt += &agents[..agents.floor_char_boundary(AGENTS_MAX)];
        if agents.len() > AGENTS_MAX {
            let (kept, all) = (AGENTS_MAX / 1024, agents.len() / 1024);
            prompt +=
                &format!("\n\n[AGENTS.md clipped to its first {kept} KB of {all} KB; read the file for the rest]");
        }
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
    let files: Vec<String> = git(root, None, &["ls-files"]).unwrap_or_default().lines().map(String::from).collect();
    if files.len() > FILES {
        let mut dirs = std::collections::BTreeMap::new();
        for file in &files {
            let parts: Vec<&str> = file.splitn(3, '/').collect();
            let key = if parts.len() == 3 { format!("{}/{}/", parts[0], parts[1]) } else { file.clone() };
            *dirs.entry(key).or_insert(0) += 1;
        }
        prompt += &format!("\n\n# Repository at session start ({} tracked files; folders show counts)\n", files.len());
        for (path, n) in dirs.iter().take(FILES) {
            prompt += &if path.ends_with('/') { format!("{path} ({n} files)\n") } else { format!("{path}\n") };
        }
    } else if !files.is_empty() {
        prompt += &format!("\n\n# Repository files at session start ({} tracked, size in bytes)\n", files.len());
        for file in &files {
            let size = std::fs::metadata(root.join(file)).map_or(0, |m| m.len());
            prompt += &format!("{file} {size}\n");
        }
    }
    prompt
}

fn tools() -> Value {
    json!([{"type": "function", "function": {"name": "run",
        "description": "Run Python in the persistent worker; only printed output comes back.",
        "parameters": {"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"]}}}])
}

fn window(error: &str) -> Option<u64> {
    let lower = error.to_lowercase();
    WINDOW.iter().find_map(|marker| {
        let rest = &lower[lower.find(marker)? + marker.len()..];
        let digits = rest.trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == ',');
        digits.filter(char::is_ascii_digit).collect::<String>().parse().ok()
    })
}

fn git(root: &Path, index: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    let guard = GIT_GUARD.get();
    cmd.args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null", "-c", "core.attributesFile=/dev/null"])
        .args(["-c", "core.quotePath=false"])
        .args(guard.into_iter().flat_map(|g| &g.1))
        .args(args)
        .env("GIT_ATTR_SOURCE", guard.map_or(EMPTY_TREE, |g| g.0.as_str()))
        .current_dir(root)
        .process_group(0);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd.output().context("running git")?;
    if !out.status.success() {
        anyhow::bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn changes(root: &Path, snapshot: Option<(&str, &Path)>, since: std::time::SystemTime) -> String {
    let (base, index) = snapshot.map_or(("HEAD", None), |(tree, index)| (tree, Some(index)));
    let mut out: Vec<String> =
        git(root, index, &["diff", "--numstat", "--no-renames", "--no-ext-diff", "--no-textconv", base])
            .unwrap_or_default()
            .lines()
            .filter_map(|l| match l.splitn(3, '\t').collect::<Vec<_>>()[..] {
                [added, removed, path] => Some(format!("{path} +{added} −{removed}")),
                _ => None,
            })
            .collect();
    let new = git(root, index, &["ls-files", "-o", "--exclude-standard"]).unwrap_or_default();
    let fresh = |f: &&str| {
        let meta = std::fs::symlink_metadata(root.join(f));
        meta.is_ok_and(|m| m.len() <= BIG || m.modified().is_ok_and(|t| t >= since))
    };
    out.extend(new.lines().filter(fresh).map(|f| format!("{f} (new)")));
    if out.is_empty() { "none".into() } else { out.join("\n") }
}
