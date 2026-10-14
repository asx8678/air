# Air — Seventh-Pass Principal Forensic Audit (Six Domains)

**Audited:** 2026-10-14 · **Repo:** `/Users/adam2/projects/air` · **Crate:** `air` v0.1.0 (edition 2024, rust-toolchain 1.99)

Full-source, line-by-line forensic audit of all 9 source files (1,772 LOC Rust across 8 modules, 265 LOC `worker.py`, `prompt.md`) against six domains: idiomatic Rust performance, token economy and deterministic compaction, security and provider integration, agent-loop stability, patch mechanics, and code-mode permission boundaries.

**Verification:** `cargo check` clean · `cargo clippy` silent · tests: **none** · CI: **none**.
**Cross-check:** corroborated against `findings/01-10`. Two findings below (A2's `process::exit` leak, B3's `resize` DoS) are **new** relative to the existing corpus.

---

## 1. Executive Summary & Severity Matrix

**Architecture.** `air` is a compact single-model coding agent: a REPL (`main.rs`) drives a turn loop (`agent.rs`) whose only tool is `run(code)` — Python executed in a persistent sandboxed worker (`worker.rs`/`worker.py`). State is an append-only JSONL log (`session.rs`); compaction (`compact.rs`) *rebuilds* a summary from that log rather than asking the LLM to summarize. The LLM client (`llm.rs`) speaks OpenAI-compatible SSE. UI (`ui.rs`/`render.rs`) is raw ANSI — no crossterm, no ratatui, no raw mode.

**Honest strengths** (rare in this class of tool, worth keeping):

- Compaction is **deterministic, log-derived reconstruction** with `recall(n)` verbatim recovery, `carry()`/`accept()` anchors, and byte-capped `clip` respecting char boundaries — no LLM summarization, so no hallucinated history or syntax loss by construction.
- Compaction only fires at loop top (`agent.rs:97-99`) — never mid-tool-call or mid-stream; `answer_missing` (`session.rs:148`) synthesizes results for interrupted calls so the transcript stays API-valid.
- The sandbox is genuinely thoughtful for a small tool: writes confined to repo/tmp, secret paths read-denied, `.git` and `core.hooksPath` write-protected (which structurally prevents hook persistence and makes `git commit` fail), env scrubbing for `*KEY*/*TOKEN*/*SECRET*/*PASSWORD*`, network denied in non-yolo mode.

**But the audit is unsparing, so:** the write-gate has a hole the sandbox does not cover (`sh()`), process trees leak on every abnormal exit path, every request re-clones and re-serializes the entire history, blocking I/O sits on a single-threaded runtime, and there are zero tests and zero CI. The core compaction algorithm is the best part of the codebase; the execution plumbing around it is where the debt lives.

| ID | Severity | Category | Location | Issue |
|---|---|---|---|---|
| A1 | **Critical** | Permissions / Injection | `worker.py:66-69` (`sh`), `worker.py:44` (`_change` gate) | `sh()` bypasses the `accept()` write gate and the actions ledger entirely — `sh("sed -i …")` mutates the tree with no criteria, no `actions` entry, invisible to compaction's Files section |
| A2 | **Critical** | Resource leak | `worker.py:20-22` + `worker.rs:110` + `main.rs:67` | Three independent orphan paths: `start_new_session=True` children survive worker SIGKILL; ctrl-c during `verify()` orphans the check's process tree; `std::process::exit` in task mode skips `Drop`, leaking the worker even on *clean* exit (**new**) |
| B1 | High | Perf / Token cost | `session.rs:88-104` + `llm.rs:37-39,227` | O(session) `entry.clone()` + full-body `json!().to_string()` per request; `body.to_owned()` re-allocates the entire prompt on every retry attempt (up to 20 copies per turn) |
| B2 | High | Async safety | `agent.rs:360-367` (`git`), `agent.rs:225-228` (`fingerprint`), `ui.rs:31-33` | Synchronous `std::process::Command`/`std::fs` on a `current_thread` tokio runtime: stalls the status ticker and delays ctrl-c delivery |
| B3 | High | DoS / Robustness | `llm.rs:157-164` | `calls.resize(index + 1, …)` with `index` taken verbatim from the SSE stream — a malformed/hostile provider payload of `index: 4e9` is a memory-exhaustion DoS (**new**) |
| B4 | High | Token economy | `compact.rs:10` | Trigger token accounting = `entry.to_string().len() / 4` — full JSON re-serialization per entry per compaction attempt, and a byte/4 heuristic that has never seen a tokenizer |
| B5 | High | Token waste / Reliability | `worker.py:19,155` | `_exec` captures subprocess output to an unbounded `TemporaryFile`, then `capture.read()` loads it all into RAM — a chatty `cargo build` fills disk and OOMs the worker |
| B6 | High | Patch mechanics | `worker.py:88-93,121` | No pre-flight syntax validation; `fmt` failure leaves the ledger entry `ok: True`; `new` text's LF newlines are injected into CRLF files verbatim → mixed line endings |
| C1 | Medium | Compaction determinism | `llm.rs:13-14,27-30` | Overflow detection is error-text sniffing (`"context_length"`, `"too many tokens"`, …) — a differently-phrased provider error bypasses the compaction fallback and hard-fails the turn |
| C2 | Medium | Compaction triggers | `agent.rs:97-106` + `session.rs:126-129` | Trigger fires one request late (usage only known after the request that overflowed); silently disabled when the provider omits usage (`last_prompt_tokens() == 0`) |
| C3 | Medium | Stream resilience | `llm.rs:70-76, 220-232` | No jitter anywhere; `Retry-After` parsed only as integer seconds (HTTP-date unsupported); retry loops stack (3 × 5 attempts); a clean EOF without `finish_reason` is accepted as a complete answer — a proxy that closes early yields silently truncated content |
| C4 | Medium | Security / Exfil | `worker.py:60-64` (`SECRETS`), `worker.rs:17-18` | Only `~`-relative secret paths are read-denied; repo-local `.env`, `*.pem`, credentials are readable and shipped wholesale to the third-party provider API; `sandbox-exec` is deprecated by Apple and not a supported security boundary |
| C5 | Medium | Injection surface | `agent.rs:340-362`, `worker.py` (all output) | No untrusted-content delimiting: READMEs, git logs, tool output flow into the context unmarked; the model has no instruction to treat them as data |
| C6 | Medium | Memory growth | `session.rs:56-60` (`resume`), `entries` | Entire session log loaded to a `String` *and* parsed into `Vec<Value>` (2× RAM); entries never evicted — a 100 MB log is 200 MB of RSS |
| C7 | Medium | Loop stability | `agent.rs:86-140` | Only bound is `MAX_REQUESTS = 200`; no repeated-identical-call detector, no worker-restart circuit breaker (a broken `python3` restarts up to 200 times) |
| C8 | Medium | Terminal safety | `ui.rs` (statics), `main.rs` | No panic hook, no SIGTERM handler — a panic or `kill` leaves the scroll region (`\x1b[…r`) set; one poisoned `Mutex` cascades into UI-wide panics via `.unwrap()` |
| D1 | Low | Perf | `llm.rs:105-109` | SSE buffer `drain(..=end)` front-of-Vec is O(n²) on large responses |
| D2 | Low | Perf | `ui.rs:31-33` | `stty size` subprocess fork every second just to learn the terminal size |
| D3 | Low | Perf | `compact.rs:60-66` (`block`) | Greedy refit loop rebuilds the block string up to O(n) times — O(n²) worst case |
| D4 | Low | Latent bug | `compact.rs:155-159` (`excerpt`) | `max - 40` underflows (panic in debug) if ever called with `max < 40`; currently unreachable |
| D5 | Low | API waste | `agent.rs:365-368` (`tools`), `llm.rs:178` | `tools()` Value rebuilt per request; stdout flushed per SSE event |

---

## 2. Deep-Dive Vulnerability & Algorithmic Analysis

### 2.1 A1 — `sh()` is a write-gate bypass and a ledger hole

The entire "code mode" contract rests on `_change` (`worker.py:44-63`): every mutation must pass the `accept(criteria)` gate, the repo-bounds check, the `.git`/hooks protection, and is recorded in `_report["actions"]`. `edit` and `write` route through it. **`sh()` does not** (`worker.py:66-69`) — it is raw `subprocess.Popen(cmd, shell=True)`.

Consequences, in increasing severity:

1. **Policy bypass:** the model (or an injected instruction in a README it reads) can run `sh("sed -i '' 's/foo/bar/' src/main.rs")` with no `accept()` ever called. The sandbox permits writes inside the repo, so nothing stops it.
2. **Ledger corruption → compaction data loss:** `actions` is the *only* source for the compaction summary's Files and Unresolved-failures sections (`compact.rs:60-77` rebuilds them from `meta["actions"]`). A tree mutated via `sh()` is invisible: after compaction, the model believes files it changed via `sh()` were never touched, and re-does or contradicts its own work. The turn-end `fingerprint()` and worker `check()`'s `_tree()` diff catch *that* the tree changed, but attribute nothing.
3. **Injection amplification:** indirect prompt injection in untrusted file content can direct mutations through the one tool with no gate. The `.git` write-deny makes hook persistence fail, which is good — but tree mutation is fully open.

The same hole means `sh()` output is also un-attributed in `ui::ran`'s action display.

### 2.2 A2 — Three orphaned-process paths

1. **Children escape worker death.** `_exec` spawns with `start_new_session=True` (`worker.py:22`). That is exactly right for `killpg(p.pid, SIGKILL)` on *timeout*, but it also means the child is in its own session: when `kill_on_drop` (`worker.rs:110`) SIGKILLs the *worker* (40-min `RUN_TIMEOUT`, error path in `agent.rs:170`, `Agent` drop), the running `cargo`/`npm` process tree survives — still mutating the working tree the agent has meanwhile reported on. On Linux, bwrap's `--die-with-parent` covers bwrap itself, not detached grandchildren.
2. **Ctrl-c during `verify()`** (`agent.rs:185-205`): `tokio::select!` drops the `output()` future; `kill_on_drop` reaps the direct `/bin/sh`, but the check's grandchildren (cargo's jobserver workers) live on, writing to `target/` while the agent tells the user it was interrupted.
3. **`std::process::exit` in task mode** (`main.rs:67`) — new relative to the repo's own `findings/` corpus: `exit()` does **not** run destructors, so the `Worker`'s `Child` (and its `kill_on_drop`) never fires. Every successful one-shot `air "task"` run leaks a sandboxed python3 process (plus any `sh()` children) that lingers until its own timeout. The fix is one line (Section 3.4).

### 2.3 B1/B4 — The per-request quadratic and the fake tokenizer

`Session::messages()` (`session.rs:88-124`) clones every live entry and re-strips `meta`/`reasoning_content` on **every** request; `llm.rs:37-39` then serializes the whole thing with `json!(…).to_string()`; `send()` re-allocates the entire body via `body.to_owned()` on **each** of up to 5 connection attempts, nested inside `complete()`'s 3 retry attempts. A 50k-token session near the compaction threshold does ~O(context) clone+serialize work up to 200 times per turn, and up to 20 full-prompt copies per turn in retry storms. `reqwest::Body` is an `Arc` — the copy is free to eliminate.

The trigger heuristic `entries[i].to_string().len() / 4` (`compact.rs:10`) is doubly wrong: it is a byte/4 guess (wrong by 30-80% for code-heavy or CJK content, in *both* directions), and it re-serializes each entry just to measure it — every compaction attempt. Since `Session::push` is the single ingress, the estimate can be computed once at push time (Section 3.6).

### 2.4 B3 — Wire-controlled `Vec::resize`

`llm.rs:157-164`: `let i = call["index"].as_u64() … calls.resize(i + 1, Default::default())`. The index comes verbatim from the provider stream. A single event with `"index": 4000000000` allocates 4 billion `[String; 3]` triples (~400 GB) before the loop continues — an uncatchable OOM abort. The provider is user-configured (`base_url` in config), so "semi-trusted at best." This is the classic parser-panic class: allocation sized by unvalidated wire data. Cap it (providers never legitimately exceed a handful of parallel calls).

### 2.5 Compaction determinism — mostly good; the trigger is the weak link

The **algorithm** is sound: `cut` is a backward scan with a token budget; the boundary never splits an assistant-call/tool-result pair (`role != "tool"` guard); `messages()` re-attaches the current task when it fell below `keep` (`session.rs:95-99`); the summary is a pure function of the log — same log, same summary, forever. No timestamps, UUIDs, or wall-clock leak into any persisted decision (the session filename uses time+pid but is never hashed into state). This is the strongest part of the codebase.

The **triggers** are the weak link:

- One-request lag: `last_prompt_tokens()` is last-*reported* usage; the request that actually overflows is the detector, and the overflow path depends on error-text sniffing (`llm.rs:13-14`) — `OVERFLOW` is six English substrings matched against a lowercased error body. Miss all six → `bail!` → the turn dies instead of compacting. (The repo's own history shows this was patched once already; it is still fundamentally fragile.)
- If the provider ignores `stream_options.include_usage`, usage stays `Null`, `last_prompt_tokens()` returns 0, and size-triggered compaction **never fires** — only the overflow error path can save the session.
- `overflowed` latching (`agent.rs:98-106`): a *second* overflow after the compaction retry hard-fails. Reasonable, but combined with `compact_at = min(at, seen * 9/10)` where `seen` can be 0, the threshold may never adapt.

### 2.6 Patch mechanics (B6)

- `edit` (`worker.py:71-82`) is exact-match with an occurrence-count guard (`!= 1` → error) — no fuzzy application, no AST guessing. This is the right reliability tradeoff. CRLF is detected as an all-or-nothing property and preserved on write.
- **Hole 1:** `new` is written verbatim. On a CRLF file, `new`'s `\n` become literal LF inside a CRLF document → mixed endings that only the formatter (if configured) repairs, and only *after* the write.
- **Hole 2:** `_change` marks the action `ok: True` *before* the formatter runs (`worker.py:55-57` vs `:59-62`); a formatter syntax failure (i.e., the model wrote broken syntax) becomes a `; fmt failed: …` note on an **ok** ledger entry. The compaction summary then records a "successful" modification of a file that is syntactically broken, and the only thing that catches it is a full `check()` run — up to 1800 s of tokens later.
- **Hole 3:** writes are not transactional across files; a half-applied multi-file edit stays applied. The design answer is "git is the undo" plus the report's printed undo command, which is honest — but the agent itself never rolls back, so a failed multi-file change leaves the model to hand-revert.

### 2.7 Security posture (C4/C5)

- The sandbox profile is best-effort. On macOS, `sandbox-exec` is deprecated by Apple and explicitly not a supported security boundary; the profile uses `(allow default)` with narrow denies, so every read in the user's home except the `SECRETS` list is permitted. `SECRETS` misses `.gitconfig`, `.npmrc`, `.sfdx`, and — most importantly — **repo-local** secrets: `.env`, `*.pem`, `credentials.json` in the project are readable by the worker and therefore shipped, verbatim, to the configured LLM provider. In `--yolo` mode the process additionally has network.
- No untrusted-content discipline: `open()` output, `sh("git log")` text, and tool results enter the context with no delimiters, no "the following is data, not instructions" contract, and `system_prompt` embeds `AGENTS.md` wholesale (`agent.rs:340-348`). The one mitigation is that there is no second-stage execution privilege: mutations still land in the sandbox and are visible in the report. But A1's `sh()` hole plus a hostile README is a working tree-corruption primitive.
- **Path traversal: actually well done.** `_rel` + `realpath` + prefix check (`worker.py:45-50`) blocks `../`, absolute paths, and symlink escapes (post-resolve); `os.path.join(ROOT, "/etc/passwd")` normalizes to an absolute path that fails the prefix check. Residual: the check-then-open race (TOCTOU on symlink swap), and case-folding on APFS is untested.

### 2.8 Stream parsing and terminal

- SSE handling tolerates torn final lines (`Session::resume` truncates a partial JSONL tail and appends a newline — `session.rs:50-57` — good), and retries `Interrupted` streams from scratch with the *partial text discarded*, so no duplicate content enters the log. But `finished` synthesis (`llm.rs:185-188`) means a provider/proxy that closes the socket cleanly mid-answer yields a silently truncated assistant message accepted as complete.
- Retry math: `2 << attempt` → 4/8/16 s and 4/8/16/32 s (capped 60), no jitter; `Retry-After` only parsed as integer seconds. Under a synchronized multi-client 429 storm this collides.
- No raw mode is entered (rustyline handles its own), so there is no cooked-mode restoration debt — but the scroll region (`\x1b[1;{rows}r`, `ui.rs:64`) and the live-line ledger (`LIVE`) are restored only by ordinary `Drop`/explicit calls. A panic while a UI mutex is held poisons it; every later `.lock().unwrap()` in the same process panics again — the whole UI becomes a panic cascade. No `set_hook`, no SIGTERM path: `kill $pid` leaves the terminal with a scroll region and a stale bar.

---

## 3. Production-Grade Rust Refactoring

### 3.1 Eliminate the per-attempt prompt copy and per-request body build (`llm.rs`)

`reqwest::Body` is `Arc`-backed and cheaply `Clone`. Build it once per `complete`, clone per attempt; serialize once into a reusable `Vec<u8>`:

```rust
pub async fn complete(&self, messages: &[Value], tools: &Value) -> Result<Value> {
    let body: Vec<u8> = serde_json::to_vec(&json!({
        "model": self.model, "messages": messages, "tools": tools,
        "temperature": self.temperature,
        "stream": true, "stream_options": {"include_usage": true},
    }))?;
    let body = reqwest::Body::from(body);          // Arc-backed; clone = refcount bump
    for attempt in 1.. {
        match self.stream(body.clone()).await {     // no 50 KB realloc per retry
            Err(e) if attempt < 4 && e.is::<Interrupted>() => { /* unchanged */ }
            result => return result,
        }
    }
    unreachable!()
}

async fn stream(&self, body: reqwest::Body) -> Result<Value> { /* … */ }

async fn send(&self, body: reqwest::Body) -> Result<reqwest::Response> {
    for attempt in 1.. {
        let mut request = self.http.post(&self.url)
            .header("content-type", "application/json")
            .body(body.clone());                   // was: body.to_owned()
        // …unchanged retry ladder, with jitter (3.5)
    }
    unreachable!()
}
```

### 3.2 Stop cloning the session on every request (`session.rs`)

Keep an API-shaped live window, appended incrementally at `push`, invalidated on compaction. `messages()` becomes a prefix-stable concatenation — which also maximizes provider prompt-cache hits:

```rust
pub struct Session {
    pub path: PathBuf,
    pub entries: Vec<Value>,      // full log, for recall(n) and compaction
    live: Vec<Value>,             // API-shaped messages after the latest `keep`
    file: File,
}

fn api_message(entry: &Value) -> Value {
    let mut m = entry.clone();
    if let Some(fields) = m.as_object_mut() {
        fields.remove("meta");
        // strip reasoning echo: most OpenAI-compatible servers reject or
        // silently bill `reasoning_content` on input messages
        fields.remove("reasoning_content");
    }
    m
}

pub fn push(&mut self, entry: Value) -> Result<()> {
    if entry.get("role").is_some() {
        self.live.push(api_message(&entry));
    }
    self.file.write_all(format!("{entry}\n").as_bytes())?;
    self.entries.push(entry);
    Ok(())
}

// On compaction: self.live = entries since the new `keep`, re-shaped once.
pub fn messages(&self) -> Vec<Value> {
    let mut out = Vec::with_capacity(self.live.len() + 2);
    out.push(json!({"role": "system", "content": self.entries[0]["system"]}));
    // …summary + current-task reattachment, then:
    out.extend(self.live.iter().cloned());   // or borrow to avoid even this
    out
}
```

### 3.3 Clamp wire-controlled indices (`llm.rs:157-164`)

```rust
const MAX_TOOL_CALLS: usize = 32;   // no provider legitimately exceeds this

let i = match call["index"].as_u64() {
    Some(i) if (i as usize) < MAX_TOOL_CALLS => i as usize,
    Some(_) => {
        broken = Some(format!("tool call index {i} exceeds {MAX_TOOL_CALLS}"));
        break 'read;
    }
    None => /* existing last-call heuristic, unchanged */,
};
```

### 3.4 Fix the clean-exit worker leak (`main.rs`) and the orphans (`worker.rs`, `worker.py`)

```rust
// main.rs — task mode: let destructors run before exit
if let Some(task) = cli.task {
    let verified = agent.run(&task).await?;
    let code = if verified { 0 } else { 1 };
    drop(agent);                    // Worker Drop -> kill_on_drop reaps python3
    std::process::exit(code);
}
```

```rust
// worker.rs — kill the whole process tree, not just python3.
// SIGTERM first so worker.py's handler can killpg() its own children;
// escalate to SIGKILL after a grace period.
// Spawn the worker itself with .process_group(0) so killpg can reach it.
```

```python
# worker.py — track live children; reap them on any worker exit path
_live = set()  # pids of active _exec children

def _exec(cmd, timeout):
    ...
    _live.add(p.pid)
    try:
        ...
    finally:
        _live.discard(p.pid)

def _cleanup(*_):
    for pid in list(_live):
        with contextlib.suppress(ProcessLookupError, PermissionError):
            os.killpg(pid, signal.SIGKILL)

signal.signal(signal.SIGTERM, lambda *_: (_cleanup(), sys.exit(0)))
signal.signal(signal.SIGHUP, _cleanup)   # terminal closed under us
```

### 3.5 Backoff with jitter, no new dependencies (`llm.rs`)

```rust
fn backoff(attempt: u32, retry_after: Option<u64>) -> Duration {
    let base = retry_after.map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(1u64 << attempt.min(5)));
    // RandomState is seeded per-process/call; hash_one gives a stable-random value
    let jitter = std::hash::RandomState::new().hash_one(attempt) % 500;
    Duration::from_millis(base.as_millis() as u64 + jitter)
}
// …
tokio::time::sleep(backoff(attempt, wait).min(Duration::from_secs(60))).await;
```

### 3.6 Estimate tokens once, at push time (`session.rs` + `compact.rs`)

```rust
pub struct Session {
    ...
    est_tokens: Vec<u64>,   // parallel to entries; one estimate per entry, ever
}

fn estimate(entry: &Value) -> u64 {
    match entry.get("role") {
        Some(_) => serde_json::to_vec(entry).map(|v| v.len() as u64 / 4).unwrap_or(0),
        None => 0,   // compact markers are free
    }
}

// compact::cut — no serialization in the loop:
pub fn cut(est: &[u64], roles: &[Option<&str>], prev: usize, keep_tokens: u64) -> usize {
    let (mut tail, mut keep) = (0, est.len());
    for i in (prev + 1..est.len()).rev() {
        if roles[i].is_none() { continue }
        tail += est[i];
        if tail > keep_tokens { break }
        if roles[i] != Some("tool") { keep = i; }
    }
    keep
}
```

Not a tokenizer, but now the lie is *consistent* and costs O(1) per compaction. Long term: calibrate the divisor per model from reported `prompt_tokens` (the data already arrives in `usage`).

### 3.7 Oscillation breaker in the turn loop (`agent.rs`)

```rust
let mut repeats: std::collections::HashMap<u64, u32> = HashMap::new();
for _ in 0..MAX_REQUESTS {
    // …
    for call in &calls {
        let args = call["function"]["arguments"].as_str().unwrap_or_default();
        let mut h = std::hash::DefaultHasher::new();
        args.hash(&mut h);
        let n = repeats.entry(h.finish()).or_insert(0);
        *n += 1;
        let (mut content, mut meta) = if *n >= 3 {
            ("error: this exact run() executed 3 times unchanged; the result \
              will not differ. Diagnose (print state, read the file) before retrying."
                .into(), json!({"tool": "run", "error": true, "rejected": true}))
        } else {
            self.call(call).await
        };
        // …push as before
    }
}
```

### 3.8 UI lock-poisoning and panic hook (`ui.rs`, `main.rs`)

```rust
// every UI static: survive poisoning instead of cascading
let mut bar = BAR.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
```

```rust
// main.rs — restore the terminal no matter how we die
let default_hook = std::panic::take_hook();
std::panic::set_hook(Box::new(move |info| {
    // clear scroll region, show cursor, drop live lines — best effort
    let _ = std::io::stdout().write_all(b"\x1b[r\x1b[?25h\x1b[0m");
    let _ = std::io::stdout().flush();
    default_hook(info);
}));
```

### 3.9 Close the `sh()` hole and make fmt verdicts honest (`worker.py`)

```python
def sh(cmd, timeout=120):
    if not (_report.get("accepted") or _state.get("accepted")):
        raise ValueError("call accept(criteria) before sh() can change files")
    before = _tree()
    out, code = _exec(cmd, timeout)
    after = _tree()
    for f in sorted(k for k in before.keys() | after.keys() if before.get(k) != after.get(k)):
        _report.setdefault("actions", []).append({"tool": "sh", "path": f, "ok": True})
    return out.rstrip("\n") + (f"\n[exit {code}]" if code else "")

# _change: a failing formatter falsifies the ledger entry it already wrote
    if FMT:
        out, code = _exec(f"{FMT} {shlex.quote(rel)}", 60)
        if code:
            _report["actions"][-1]["ok"] = False
            _report["actions"][-1]["error"] = "fmt failed: " + (out.strip().splitlines() or [""])[0]
    # edit(): if crlf: new = new.replace("\r\n", "\n").replace("\n", "\r\n")
```

The transactional upgrade for B6: write `result` to a sibling temp file, run `FMT` against *it*, and only `os.replace` into place on fmt success — the model then gets a syntax rejection *before* the tree is dirtied, instead of a note after.

---

## 4. Actionable Remediation & Testing Strategy

### P0 — Correctness/security, this week

1. **Gate + ledger `sh()`** (3.9). Gate: `accept()` required; ledger: tree-diff recorded as actions. One `python` patch, closes A1.
2. **Process-tree reaping** (3.4): `drop(agent)` before `process::exit`; `process_group(0)` + SIGTERM→grace→SIGKILL on the worker; `_cleanup()` handler for SIGTERM/SIGHUP in `worker.py`. Closes A2.
3. **Clamp `calls.resize`** (3.3). Two lines. Closes B3.
4. **Cap `_exec` capture**: pipe + reader thread that stops at 8 MiB and reports `[output truncated at 8 MiB]` — kills the disk/OOM path (B5) and bounds `_last`.
5. **Fmt-failure falsifies the ledger** + CRLF normalization of `new` (3.9). Closes the B6 token-waste cycle.

### P1 — Performance and reliability

6. `Body` reuse + `to_vec` serialization (3.1); incremental `live` window (3.2). Also fixes the biggest cache-buster: with `live` appended (never rebuilt), the request prefix is byte-stable across tool results — prompt-cache hit rates become structural, not accidental.
7. Move `git()`, `fingerprint()`, `snapshot()` behind `tokio::task::spawn_blocking` (they take owned `PathBuf`s) so ctrl-c and the ticker stay live (B2). Replace the `stty`-per-second poll with `TIOCGWINSZ` via `libc`, or at minimum cache for the turn (D2).
8. Structured overflow detection: parse provider error JSON (`error.code`/`error.type`) before falling back to the substring list; treat HTTP 400 with any `token`/`context` mention as overflow (C1). Fire compaction pre-emptively from the push-time estimate when usage goes unreported (C2).
9. Jitter + HTTP-date `Retry-After` (3.5); require `finish_reason` for streams that carried tool calls (synthesized `finished` only for plain text, and even then mark the message `meta.truncated` so the next turn can re-ask).
10. Panic hook + SIGTERM handler (3.8); de-`unwrap` UI locks.
11. Strip `reasoning_content` from all echoed messages (interop + token cost).

### P2 — Economy and stability

12. Push-time token estimates (3.6); calibrate divisor against reported `usage`.
13. Oscillation breaker (3.7) + worker-restart circuit breaker (after 3 consecutive spawn failures, end the turn with a diagnostic instead of restarting into MAX_REQUESTS).
14. Resume without doubling memory: stream-parse the log line-by-line into `entries`, drop the intermediate `String` (C6). Bound the `.index` snapshot lifecycle.
15. `memchr` + cursor for the SSE buffer (D1); precompute `tools()` once (D5).

### Testing strategy (the repo currently has **zero tests** — the single largest gap after A1/A2)

- **Property-based (`proptest`)**, aimed at `compact.rs` where the invariants live:
  - `cut` monotonicity: `cut(est, roles, prev, k)` never returns `< prev`, never splits a call/result pair, and is deterministic.
  - `clip`/`tail`/`excerpt`: outputs are always char-boundary-safe (generate arbitrary UTF-8 including 4-byte chars), never longer than `max`, `excerpt` never underflows for any `max > 0`.
  - `block` never exceeds `cap` bytes and always keeps the first and last lines when they fit.
  - Compaction round-trip: for an arbitrary generated log, `summary(entries, cut(...))` is a pure function — byte-identical across two calls (the determinism guarantee, made executable).
- **Token fuzzing:** corpus of real code files (Rust, Python, JSON, CJK text) → assert byte/4 estimate stays within ±50% of a reference tokenizer count; fuzz `complete()` request-body size against `compact_at` thresholds via a mock SSE server.
- **SSE replay tests:** canned streams for: torn chunk mid-UTF-8, provider `error` object mid-stream, duplicate tool-call indices, `index: u64::MAX` (regression for B3), clean-EOF truncation.
- **Process cleanup verification (A2):** integration test spawning `sh("python3 -c 'while True: pass'")`, then interrupting — assert via `ps`/`waitpid` sweep that no descendant of the worker survives; repeat for `verify()` ctrl-c and for task-mode exit.
- **Edit mechanics:** golden tests for LF/CRLF/mixed files; `old` with trailing whitespace; `new` containing the `old` string; 0-match and 2-match rejections; fmt-failure ledger regression (B6).
- **Compaction integration:** a scripted 300-entry session replayed against a mock provider; assert (a) no request is sent with a dangling `tool_call_id`, (b) the summary retains the last `accepted`, all `carry`, and the failing check tail, (c) `recall(n)` for every `#n` cited in a produced summary returns the exact entry.
- **Injection corpus:** a fixture README containing instruction-shaped text; assert the agent's ledger shows only the gate-approved actions and the report's diff matches them exactly.

**Assumptions not validated at runtime:** audited statically (plus `cargo check`/`clippy`); the agent was not executed against a live provider, so provider-behavior claims (usage-report absence, `reasoning_content` rejection, sandbox-exec bypasses) are code-level assessments, not reproduced exploits. Findings A2 (`process::exit` leak) and B3 (`resize` DoS) are new in this pass; the rest corroborate `findings/01-10`.
