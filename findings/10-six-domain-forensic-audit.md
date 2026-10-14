# Air — Round 10: Six-Domain Principal Forensic Audit

**Reviewed:** 2026-10-14 · **Repo:** `/Users/adam2/projects/air` · **Crate:** `air` v0.1.0 (edition 2024, rust-toolchain 1.99)
**Scope:** all 8 Rust modules (1,772 LOC), `worker.py` (265 LOC), `prompt.md` — every line read.
**Verification:** `cargo clippy --all-targets` clean · zero tests (`grep '#\[test\]' src` = 0) · zero panic hooks · zero jitter / atomic-write / seed / max_tokens usage (grep-verified).

Domains covered: (1) idiomatic & optimal Rust, (2) token economy & deterministic compaction, (3) security / sandbox / provider integration, (4) agent loops & long-session stability, (5) patch mechanics & context routing, (6) code mode & permission boundaries.

> **ID scoping:** finding IDs in this document (K/H/M/L) are round-scoped; in the repo-wide index they appear as `10-K1` … `10-L3`. This round's M/L numbering intentionally differs from rounds 01–07.

> **Independence note:** every claim below was re-derived from current source. New or materially refined relative to rounds 01–07: the prompt-cache prefix invalidation (10-H1), the `SSH_AUTH_SOCK` / env-credential sandbox channel (10-H2), the SSE premature-finish acceptance (10-K3), and the request↔reply misalignment on interrupt (10-H3).

---

## 1. Executive Summary & Severity Matrix

### Architectural assessment

`air` is a remarkably *coherent* small architecture: a JSONL session log as the single source of truth, a code-mode Python worker instead of a tool zoo, deterministic log-derived compaction (no LLM summarization — correct call), a real OS sandbox (`sandbox-exec`/`bwrap`) with `.git`/hooks protection, and a host-side verification gate that fingerprints the tree around `check`. The design instincts are better than most shipping agents.

But the implementation is production-*adjacent*, not production-ready. Three flaws are structural:

1. **The trust model is circular.** The only gate before destructive file writes is `accept(criteria)` — which the *model itself* calls (`worker.py` `_change`). There is no read-only mode, no user-approval policy, no command allowlist. The sandbox is the only real boundary, and it leaks (§2.2).
2. **The stream termination contract is a guess.** `llm.rs` decides "the response is complete" from a heuristic over partial state. A cleanly-closed but truncated stream yields a *believably complete* assistant message — silent partial plan execution (§2.1).
3. **Token economics are correct but uncached.** Compaction is deterministic and reversible via `recall(n)`, yet `Session::messages()` mutates already-sent history at every turn boundary, destroying provider prompt-cache prefixes and paying full re-prefill on the entire session each turn (§2.3).

Everything else — O(N) clones per request, blocking `stty` forks on the async runtime, non-atomic `edit()` truncation, missing backoff jitter, zero tests — is conventional debt layered on a sound skeleton.

### Severity matrix

| ID | Issue | Location | Severity | Category |
|----|-------|----------|----------|----------|
| K1 | No permission model: `accept()` is model self-approval; no read-only mode, no user confirmation for writes/shell, `--yolo` unguarded | `worker.py` `_change()`; `main.rs` `Cli` | **Critical** | Security / Mode boundaries |
| K2 | `edit()` truncates target file then writes: kill/crash/disk-full destroys the file mid-write | `worker.py` `edit()` `open(full,"w")` | **Critical** | Patch reliability |
| K3 | Stream-finish heuristic accepts truncated responses as complete → partial tool-call plans executed silently | `llm.rs` `stream()` (~L193) | **Critical** | Determinism / Patch reliability |
| H1 | `messages()` strips `reasoning_content` from all history when a new user turn starts → provider prompt cache invalidated on **every turn**; full re-prefill at full price | `session.rs` `messages()` (~L104–108) | **High** | Token economy |
| H2 | Sandbox env deny-list leaks `SSH_AUTH_SOCK`, `DATABASE_URL`, `DSN`-style creds; `/tmp` is writable → SSH-agent hijack from sandboxed code | `worker.rs` `sandboxed()` `secret()` | **High** | Security |
| H3 | Interrupt path: 3 s `settle()` window vs. 30-min checks; a turn future dropped mid-`stdin.write_all` corrupts the worker's line protocol; stale replies can mis-align request↔reply | `worker.rs` `settle()`/`run()`; `agent.rs` `run()` | **High** | Agent-loop stability |
| H4 | Orphaned subprocesses: `kill_on_drop` kills only the direct child; `_exec` children run in their own session (`start_new_session=True`) and survive worker SIGKILL; ctrl-c during `verify()` orphans `cargo` | `worker.py` `_exec()`; `agent.rs` `verify()` | **High** | Resource leaks |
| H5 | Only root `.git` + `core.hooksPath` are write-protected; nested/vendored repos' `.git` are writable → hook planting that executes later **as the user, outside the sandbox** | `worker.rs` `protected()` | **High** | Security |
| H6 | Zero tests, zero CI for a tool that rewrites users' files and manages subprocesses | crate-wide (`grep '#\[test\]'` = 0) | **High** | Testing |
| M1 | Token accounting: provider usage *or nothing*; `compact::cut` estimates `len()/4`; fixed `COMPACT_AT = 100_000` untied to the model's real window | `compact.rs` `cut()`; `agent.rs` consts | Medium | Token economy |
| M2 | Consecutive-context-overflow hard-aborts the turn; `compact_to` "nothing to compact" stall loops into that abort | `agent.rs` `turn()` overflow arm | Medium | Compaction |
| M3 | Blocking work on a `current_thread` runtime: `stty` fork+exec every second in the ticker; `std::fs`/`std::process` in `snapshot()`, `fingerprint()`, `changes()` inside async `turn()` | `ui.rs` `size()`; `agent.rs` | Medium | Async safety |
| M4 | O(session) deep-clone + re-serialization of the full context per request; `entries: Vec<Value>` unbounded for the process lifetime | `session.rs` `messages()` | Medium | Performance |
| M5 | Weak oscillation guards: `MAX_REQUESTS = 200`, single check-nudge, no repeated-tool-call / repeated-identical-failure detection | `agent.rs` `turn()` | Medium | Agent loops |
| M6 | Retry backoff without jitter; overflow classification by substring sniffing of error text | `llm.rs` `send()`/`overflow()` | Medium | Provider resilience |
| M7 | No pre-flight syntax/AST validation of edits (only optional post-write `fmt`); mixed-EOL files break `old` matching | `worker.py` `edit()`/`_change()` | Medium | Patch reliability |
| M8 | `AGENTS.md` + all `SKILL.md` descriptions injected into the system prompt uncapped; prompt frozen at session creation (stale on resume) | `agent.rs` `system_prompt()` | Medium | Token economy |
| M9 | `reasoning_content` echoed back to the provider on request (spec-fragile; wasted tokens) | `session.rs` `messages()` | Medium | Provider integration |
| M10 | No panic hook / terminal-state restore guarantee; `Turn::drop` re-locks a possibly poisoned mutex during unwind → abort with scroll region left set | `ui.rs` `Turn::drop` | Medium | Terminal UX safety |
| M11 | Repo-controlled `air.toml` `check`/`fmt` execute as the user; macOS profile permits all localhost network; `--yolo` removes everything silently | `main.rs`; `worker.rs` `sandboxed()` | Medium | Prompt-injection / supply chain |
| M12 | Request body omits `max_tokens`, `seed`, `parallel_tool_calls:false` (unbounded completions, nondeterminism) | `llm.rs` `complete()` | Medium | Determinism |
| M13 | `load()` swallows every config read error as "no config"; `api_key` file permissions never checked | `main.rs` `load()`; `llm.rs` `key_file()` | Medium | Error handling |
| M14 | Multi-file edit batches have no transaction/rollback across `edit()` calls in one run | `worker.py` | Medium | Patch reliability |
| L1 | SSE line scan is O(n²) via `drain`, buffer uncapped; multi-line `data:` events silently dropped by the `else { continue }` parse guard | `llm.rs` `stream()` | Low | Performance / resilience |
| L2 | Latent `max - 40` underflow in `excerpt`; 17 `.unwrap()` / 5 `.expect()` sites, mostly lock-poison panics | `compact.rs`; `ui.rs`; `llm.rs` `Llm::new` | Low | Error handling |
| L3 | `fingerprint()` hashes `mtime` (touch ⇒ phantom change; coarse mtime ⇒ missed change); `compact::tail` cuts mid-line | `agent.rs` `fingerprint()`; `compact.rs` `tail()` | Low | Determinism |

---

## 2. Deep-Dive Vulnerability & Algorithmic Analysis

### 2.1 K3 — The stream termination contract is a heuristic, not a protocol

`llm.rs::stream()` decides completeness as:

```rust
finished |= broken.is_none()
    && !(text.is_empty() && calls.is_empty())
    && calls.iter().all(|c| serde_json::from_str::<Value>(&c[2]).is_ok());
```

`finished` is also set by `[DONE]` and by any `finish_reason`. The disjunction means: **a stream that ends with `Ok(None)` (TCP FIN through a proxy, idle-timeout close, provider restart) after emitting exactly one complete tool call out of three is classified as a finished turn.** The assistant message is recorded to the log as authoritative, `turn()` executes the one tool call, and the model then receives its result and continues as though the plan were complete — at best a detour, at worst the half-applied intent of a multi-file refactor.

The text path is worse: a response truncated mid-sentence after a paragraph break passes `!(text.is_empty() && calls.is_empty())`, and the "answer" is delivered to the user with no marker. There is no way to distinguish *the model stopped* from *the network stopped*, because the only hard signals (`[DONE]`, `finish_reason`) are treated as sufficient-but-not-necessary.

Second-order effect: `Interrupted` is retried from scratch (`complete()` → `stream()`), so genuinely truncated streams that *don't* pass the heuristic cost a full duplicate request — while truncated streams that *do* pass cost correctness. Both branches of the same defect.

**Correct contract:** completion is a positive signal — `[DONE]`, a `finish_reason`, or (for providers that emit neither) an explicit empty-delta grace window — everything else is `Interrupted`.

### 2.2 H2/H5 — Sandbox boundary leaks

`sandboxed()` builds the child environment as `env_clear()` + all non-"secret"-named variables. The filter is a deny-list over four substrings:

```rust
let secret = |name: &str| ["KEY", "TOKEN", "SECRET", "PASSWORD"].iter().any(|s| name.to_uppercase().contains(s));
```

Verified misses: `SSH_AUTH_SOCK`, `DATABASE_URL`/`DB_URL`/`DSN` (credentials embedded in URLs), `NPM_CONFIG__AUTH`, `NETRC`-adjacent vars. Combined with `writable()` granting **all of `/tmp`** (canonicalized `/private/tmp` on macOS), a sandboxed `sh()` can `connect()` to the user's SSH agent socket (`/tmp/ssh-*/agent.*` — a unix socket connect is a file-write op, and `/tmp` is write-allowed) and use the user's loaded keys against their git remotes. The sandbox denies `~/.ssh` *file reads* and then hands the agent socket back through the environment.

Second leak: `protected()` guards exactly one `.git` — the root repo's — plus `core.hooksPath`. A vendored repo, submodule, or nested project checked out under the tree has a writable `.git/hooks`. `sh('echo ... > vendor/foo/.git/hooks/post-checkout && chmod +x ...')` plants code that runs **the next time the user runs git there, outside air, outside the sandbox**. The write gate in `worker.py` blocks `edit`/`write` into `PROTECTED` dirs, but `sh()` is unrestricted inside the sandbox (prompt.md says "never through the shell" — advisory only, zero enforcement).

Residual platform risk: `/usr/bin/sandbox-exec` is deprecated by Apple and is a coarse SBPL profile, not a hardened seatbelt against adversarial code; the localhost-network allowance means a malicious repo's `check` command (see M11) can talk to any local service (unauthenticated Jupyter, dev servers, metadata proxies).

### 2.3 H1 — Prompt-cache prefix invalidation on every turn (the silent token burn)

`Session::messages()` builds each request fresh and *rewrites history*:

```rust
if newest.is_some_and(|u| i < u) {
    fields.remove("reasoning_content");
}
```

`newest` is the index of the latest user task. When the user sends the next task, `newest` advances, and **every earlier assistant message loses its `reasoning_content` field**. Provider prompt caches key on the exact token prefix; mutating any message in the prefix invalidates everything from that point. Consequence: at the start of every user turn, the entire multi-turn history is re-prefilled at full input price. For a 100k-token session with, say, 50 turns, that is on the order of millions of wasted input tokens per session — precisely the cost prefix caching exists to eliminate.

Two smaller prefix destabilizers compound it:

- The summary user message content mutates across turns (the `[Current task, entry #u]` suffix appears/disappears as `newest` crosses `keep`).
- The full system prompt embeds `AGENTS.md` + skills but is rebuilt only at `Session::create`; on `--resume` it is pinned to whatever was captured when the session began (frozen ≠ stable: it drifts from the tree and can't be refreshed).

Correct model: a message, once sent, is **append-only**. Strip `reasoning_content` at *push* time (keep it in the log for `/think`), never at *message-build* time.

Meanwhile the compaction machinery itself is genuinely good and this is stated explicitly for the record: `compact::summary()` is a deterministic extraction over the log — `BTreeMap` for files, index-ordered events, fixed caps, byte-identical across rebuilds — with `recall(n)` restoring any cited entry verbatim from the JSONL. No LLM summarization, no hallucination surface, no syntax loss. The deterministic-trigger question is also answered correctly *structurally* — compaction fires only at the top of the request loop or after a classified overflow, never mid-stream or mid-tool-call. The remaining trigger defects are economic, not corrupting:

- `last_prompt_tokens()` returns **0** when the provider omits usage → `>= compact_at/2` never fires → the *only* compaction trigger is the hard overflow path (M1).
- `cut()` budgets with `entry.to_string().len() / 4` while triggers use provider `prompt_tokens` — two tokenizers that disagree by ±50% depending on language/code density.
- On overflow, `compact_at` shrinks via `min(seen * 9/10)` — good adaptation — but the second *consecutive* overflow (no successful response between) bypasses the retry arm (`!overflowed` guard) and `?`-propagates out of `turn()`, killing the task mid-flight with no user-facing explanation beyond the raw error (M2).

State-drift audit, completed: session state carries no hashes across sessions; the only fingerprint (`DefaultHasher` over `path, len, mtime`) is turn-local, so `DefaultHasher`'s instability and mtime nondeterminism cannot corrupt persisted state — but mtime *can* lie about change detection (touch ⇒ spurious `check` run; coarse-grained mtime + equal size ⇒ missed change ⇒ "NO CHANGES" verdict on a mutated tree, L3).

### 2.4 H3/H4 — Interrupt, protocol, and process-lifecycle holes

The worker protocol is one JSON line per request over a pipe. Three failure modes verified in the code:

1. **Partial write desync.** `tokio::select!` dropping `turn()` mid-`self.stdin.write_all(...)` can leave a partial JSON line in the pipe. The next request appends to it; `json.loads` in `worker.py`'s main loop raises outside any try → the worker dies mid-protocol. Recovery exists (`run_code` Err → `worker = None`) but the turn's tool call is recorded with an error the model never caused.
2. **Stale-reply misalignment.** `settle()` waits 3 s. A legitimate in-flight `check()` runs up to 30 min (`_exec(CHECK, 1800)`). Ctrl-c at t+4 s ⇒ `settle` times out ⇒ `worker = None` kills the worker mid-check. If `settle` *does* return a line after a dropped `run()` future had already consumed the request boundary, replies and requests can pair across generations. There is no request ID in the protocol to detect this.
3. **Orphans.** `_exec()` spawns with `start_new_session=True` (correct for `killpg` on timeout) — but that also means when `kill_on_drop` SIGKILLs the worker, the running `cargo check`/`npm test` process group survives, still mutating the tree, for up to its own timeout. Same for `agent.rs::verify()`: ctrl-c drops the `output()` future; `kill_on_drop` reaps the `/bin/sh`, whose `exec`'d check command's children (cargo's jobserver workers) live on.

Also in `run()`: on `settle()` Err, `self.pending` is left as `Some(id)` and **no tool message is pushed for the interrupted call** — `answer_missing()` in `messages()` papers over this with a synthetic "tool result missing", so the API stays valid, but the log records an assistant tool-call with no outcome and the model is told a lie-shaped truth ("interrupted") only via the generic filler.

### 2.5 K2 — `edit()` is a truncating write, not an atomic replace

```python
with open(full, "w", encoding="utf-8", newline="\r\n" if crlf else "") as f:
    f.write(result)
```

`open(full, "w")` truncates to zero **at open time**; the new content lands at close. Between them: SIGKILL from `kill_on_drop`, a panic in Rust, a full disk, an OS crash. Target file = empty. For a coding agent whose central promise is "git is the undo" — new/modified files that were never committed (and untracked files git can't restore) are exactly the losses the undo story doesn't cover. The same applies to the `fmt` pass and the read-back diff loop afterwards.

EOL handling is thoughtful but binary: `crlf = "\r\n" in raw and raw.count("\r\n") == raw.count("\n")` — a *mixed*-EOL file is treated as LF, so a model-supplied `old` normalized to `\n` won't match `\r\n` lines and vice versa; the error ("old text not found") sends the model into a re-read loop (the `_hint` system mitigates but doesn't fix).

Multi-edit atomicity: `_change` blocks SIGINT around the write (`pthread_sigmask` — good micro-transaction care) and enforces `accept()` first, but each `edit()` commits independently. Run of three edits where #2 fails ⇒ file 1 modified, files 2–3 untouched, no rollback marker in the ledger.

### 2.6 K1/M5 — The loop and the gate

- `MAX_REQUESTS = 200` per turn with exactly **one** check-nudge and no repeated-action detection. An oscillating model that alternates `edit(A→B)` / `edit(B→A)` (each valid, each unique) can burn 200 requests and leave the tree unchanged or thrashing; `report()` will say "NO CHANGES", but the tokens are spent. Nothing hashes consecutive tool invocations or consecutive identical error texts.
- The `GATE` check-nudge is a good pattern, but `nudged` is once-per-turn: a fix that regresses the check on the second pass ends the turn `UNVERIFIED` — acceptable — while the underlying run can still have been 150 requests long.
- `accept()` as the write gate (K1) means "user intent" in the summary/state machinery is *model-authored*: `state.accepted`, `carry`, `plan` are all attacker-controllable in the injection scenario (malicious README content steering the model to call `accept("delete everything")` then `edit` — the summary will dutifully record "Accepted criteria").

Injection surface inventory (verified): `AGENTS.md` and `SKILL.md` descriptions land verbatim in the system prompt (uncapped, M8); `check()` output, `sh()` output, and file contents land in tool results (bounded at 8 KB head+tail — good); `prompt.md` asserts tool-result neutrality nowhere — no marker distinguishes "data the model read" from "instructions the user gave". The `meta.host` flag distinguishes *host* user messages in state accounting — good instinct — but tool output carries no equivalent demotion marker.

---

## 3. Production-Grade Rust Refactoring

### 3.1 K3 — Completion is a positive signal (llm.rs)

```rust
enum StreamEnd { Done, Truncated(String) }

// inside stream(): track hard signals only
let mut saw_done = false;
let mut finish_reason: Option<String> = None;
// ...
// finished |= data.trim() == "[DONE]";   ->  saw_done |= data.trim() == "[DONE]";
if let Some(r) = event["choices"][0]["finish_reason"].as_str() {
    finish_reason = Some(r.to_owned());
}

// after the read loop:
let ended = broken.is_none() && (saw_done || finish_reason.is_some());
if !ended {
    if answering { markdown.finish(); }
    let why = broken.unwrap_or_else(|| {
        format!("stream ended without finish_reason (last signal: {})",
                if calls.is_empty() { "content" } else { "tool_calls" })
    });
    return Err(Interrupted(why).into());   // complete() already retries this from scratch
}
```

For providers emitting neither `[DONE]` nor `finish_reason`, add a bounded idle-grace instead of a guess: wrap `response.chunk()` in `tokio::time::timeout(Duration::from_secs(30), _)` and treat `Ok(None)` within the grace as EOF-only-if-empty, i.e. still `Interrupted` unless a hard signal arrived. Duplicated requests are cheap; silent partial plans are not.

### 3.2 H1 — Append-only message prefix (session.rs)

```rust
pub fn push(&mut self, mut entry: Value) -> Result<()> {
    // reasoning stays in the log for /think; never enters the request prefix
    if entry["role"] == "assistant" {
        if let Some(obj) = entry.as_object_mut() {
            if let Some(reasoning) = obj.remove("reasoning_content") {
                obj.insert("reasoning_content_log".into(), reasoning);
            }
        }
    }
    serde_json::to_writer(&self.file, &entry)?;   // also kills the format!() allocation
    self.file.write_all(b"\n")?;
    self.entries.push(entry);
    Ok(())
}

pub fn messages(&self) -> Vec<Value> {
    // ...
    for (i, entry) in self.entries.iter().enumerate().skip(keep) {
        let mut message = entry.clone();
        if let Some(f) = message.as_object_mut() {
            f.remove("meta");
            f.remove("reasoning_content_log");
        }
        // no conditional stripping: the serialization of entries[..n] is
        // byte-stable as n grows -> provider prefix cache stays valid.
```

`/think` reads `reasoning_content_log`; the summary extractor reads it too if you later want reasoning anchors. Verify with the property test in §4.1.

### 3.3 M6 — Jittered backoff + structured overflow classification (llm.rs)

```rust
fn classify(status: reqwest::StatusCode, body: &str) -> Option<ContextOverflow> {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        let code = v["error"]["code"].as_str().unwrap_or_default();
        let ty = v["error"]["type"].as_str().unwrap_or_default();
        if matches!(code, "context_length_exceeded" | "model_max_prompt_tokens_exceeded")
            || (ty == "invalid_request_error" && overflow(body)) {
            return Some(ContextOverflow(body.chars().take(300).collect()));
        }
    }
    None
}

// retry wait: exponential + full jitter, honoring Retry-After
use std::time::{SystemTime, UNIX_EPOCH};
fn backoff(attempt: u32, retry_after: Option<u64>) -> Duration {
    if let Some(s) = retry_after { return Duration::from_secs(s.min(60)); }
    let base = 2u64.saturating_pow(attempt.min(5)).min(60);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
    Duration::from_secs(base / 2 + u64::from(nanos % (base as u32 / 2).max(1)))
}
```

Jitter matters here beyond etiquette: sessions are resumed and long-running; a provider-wide 429 window plus N restarted agents produces synchronized retries.

### 3.4 M3 — No blocking work on the executor

```rust
// ui.rs::size(): replace the per-second stty fork/exec
fn size() -> Option<(u16, u16)> {
    #[repr(C)] struct WinSize { rows: u16, cols: u16, x: u16, y: u16 }
    extern "C" { fn ioctl(fd: i32, req: u64, ...) -> i32; }
    const TIOCGWINSZ: u64 = 0x4008_7468; // macOS
    let fd = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?.into_raw_fd();
    let mut ws = WinSize { rows: 0, cols: 0, x: 0, y: 0 };
    (unsafe { ioctl(fd, TIOCGWINSZ, &mut ws) } == 0)
        .then_some((ws.rows, ws.cols))
        .filter(|&(r, c)| r >= 5 && c >= 20)
}

// agent.rs::turn(): move git + stat work off the async path
let snapshot = tokio::task::spawn_blocking({
    let root = self.root.clone();
    move || snapshot_in(&root)
}).await??;
```

(Or simply cache `size()` for the 1 s tick — the ticker only needs *change* detection, and a stale width for one second is invisible.)

### 3.5 K2 — Atomic edits (worker.py)

```python
def edit(path, old, new):
    def make(full, rel):
        with open(full, encoding="utf-8", newline="") as f:
            raw = f.read()
        text, nl = _normalize(raw)                    # returns (text, "crlf"|"lf"|"mixed")
        if nl == "mixed":
            raise ValueError(f"{rel} has mixed line endings; rewrite the file with write() first")
        n = text.count(old)
        if n != 1:
            raise ValueError(f"old text {'not found' if n == 0 else f'occurs {n} times'} in {rel}")
        result = text.replace(old, new, 1)
        _atomic_write(full, result, nl)              # tmp + fsync + os.replace
        return text, result
    _change("edit", path, make)

def _atomic_write(full, content, nl):
    d = os.path.dirname(full)
    fd, tmp = tempfile.mkstemp(dir=d, prefix=".air-", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\r\n" if nl == "crlf" else "") as f:
            f.write(content)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, full)                        # atomic on the same filesystem
        dfd = os.open(d, os.O_RDONLY)
        with contextlib.suppress(OSError):
            os.fsync(dfd)                            # journal the rename
        os.close(dfd)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(tmp)
        raise
```

### 3.6 H2/H5 — Sandbox: allowlist env, deny nested `.git` (worker.rs)

```rust
// env: allowlist instead of denylist
const ENV_ALLOW: &[&str] = &["PATH", "HOME", "USER", "TMPDIR", "SHELL", "TERM"];
cmd.env_clear()
   .envs(std::env::vars_os().filter(|(k, _)| {
       let k = k.to_string_lossy();
       ENV_ALLOW.iter().any(|a| k == *a) || k.starts_with("CARGO") || k.starts_with("RUST")
   }))   // SSH_AUTH_SOCK, DATABASE_URL, DSN, *_URL creds all excluded by construction
   .envs(HERMETIC.split_whitespace().filter_map(|pin| pin.split_once('=')));

// macOS profile: every .git under the tree is off limits, not just the root's
let profile = format!(
    "(version 1)(allow default)(deny network*)\
     (allow network* (local ip \"localhost:*\") (remote ip \"localhost:*\"))\
     (deny file-write*)(allow file-write*{dirs} (subpath \"/dev\"))\
     (deny file-write* (regex #\"(^|/)\\\\.git(/|$)\")){protected}\
     (deny file-read-data{secrets})(allow file-read-data (literal {log:?}))\
     (deny file-ioctl (regex #\"^/dev/tty\"))"
);
```

Mirror in `worker.py::_protected()` for the `_change` gate: walk for nested `.git` dirs at worker start (bounded, once), or reject any edit whose realpath contains a `/.git/` component.

### 3.7 M5 — Oscillation breaker (agent.rs)

```rust
let mut recent: std::collections::VecDeque<u64> = std::collections::VecDeque::new();
// after each call():
let sig = {
    let mut h = std::hash::DefaultHasher::new();
    (call["function"]["name"].as_str(), call["function"]["arguments"].as_str()).hash(&mut h);
    h.finish()
};
recent.push_back(sig);
recent.truncate(6);
if recent.iter().filter(|s| **s == sig).count() >= 3 {
    self.session.push(json!({"role": "user", "meta": {"host": true},
        "content": "The same run() has repeated 3 times with no state change. Stop, print the blocker in one line, and finish."}))?;
    // or hard-stop: return Ok(false)
}
```

Plus: drop `MAX_REQUESTS` to ≤ 64 and extend `nudged` to a small counter (2–3) with escalating wording.

### 3.8 M10 — Terminal restore on panic (main.rs)

```rust
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        print!("\x1b[r\x1b[0m\x1b[?25h");          // reset scroll region, SGR, show cursor
        let _ = std::io::stdout().flush();
        prev(info);
    }));
}
// and make Turn::drop poison-proof:
if let Ok(mut guard) = BAR.lock() { if let Some(bar) = guard.take() { /* restore */ } }
```

---

## 4. Actionable Remediation & Testing Strategy

### P0 — this week (correctness & safety)

1. **Atomic `edit()`/`write()`** (K2, §3.5) + crash test: property test that kill -9 at any point leaves the file either old-or-new, never empty (loop: spawn editor child writing a 1 MB file, SIGKILL at random µs, assert content ∈ {old, new}).
2. **Positive stream-completion contract** (K3, §3.1). Test: mock SSE server (tiny hyper/axum listener) serving N scenarios — clean `[DONE]`, `finish_reason`-terminated, **FIN mid-tool-calls**, FIN mid-sentence, 429-then-success, `Retry-After` honored, error-object event. Assert every truncation yields `Err(Interrupted)`, never a partial message.
3. **Env allowlist + nested `.git` protection** (H2/H5, §3.6). Test: run `sh('python3 -c "import os;print(os.environ)"')` with yolo=false, assert `SSH_AUTH_SOCK`/`DATABASE_URL` absent; `sh()` writes into `vendor/x/.git/hooks` must fail the `_change` gate *and* the profile.
4. **Prompt-prefix stability** (H1, §3.2). Property test — the single highest-ROI test in the plan:

   ```rust
   #[test] fn prefix_is_append_only() {
       // for any sequence of turns/tool calls, serialize messages() after each push;
       // assert messages()[..n] at step k+1 == messages()[..n] at step k for n < len_k
   }
   ```

   Fuzz with `proptest` over role/tool_call/compact-record sequences; any counterexample is a cache-invalidation bug.

### P1 — next two weeks

5. **Interrupt hardening** (H3): put a request-generation counter in the worker protocol; `settle()` replies must carry it; always restart the worker after any interrupt (make the `Err(_) => self.worker = None` arm unconditional); raise the `settle` grace or make it "wait until reply or N s *after the current exec completes*" (worker emits a `{"started": ...}` line). Test: fake worker sleeping 10 s in a run; drop the turn future; assert no reply/request mispairing on the next turn.
6. **Orphan sweep** (H4): worker `_exec` records child pgids; `worker.py` installs `atexit`/`SIGTERM` handlers that `killpg` all live groups; `verify()` wraps its child in a process group (`setsid`) and kills the group on drop. Verification test: `sh("sleep 300")`, SIGKILL the worker, `pgrep -f "sleep 300"` empty within 2 s.
7. **Token accounting fallback** (M1/M2): if usage is absent for 2 consecutive responses, switch triggers to a local estimate (measured chars-per-token calibration from observed usage pairs — deterministic, no tokenizer dependency) and always keep 20% headroom against the last known-good prompt size. On `compact_to` "nothing to compact", escalate: hard-truncate the oldest non-tool entries verbatim (never the newest window) rather than stalling into a fatal double-overflow.
8. **Backoff jitter + structured overflow** (§3.3). Unit-test `classify()` against 20 recorded provider error bodies (OpenAI, Anthropic-bridge, vLLM, llama.cpp).

### P2 — month out

9. Oscillation breaker + `MAX_REQUESTS` reduction (§3.7); simulate an A↔B edit loop against the mock server and assert termination < N requests.
10. Pre-flight syntax gate: after `edit()` on known extensions, run a cheap parse (`rustfmt --check` / `python -m py_compile` / `node --check`) before the file lands, surface failures as a helper error with a `_hint`. Property test: every syntactically-broken edit is rejected with the correct line number.
11. EOL fuzzing (`proptest` over strings with `\n`, `\r\n`, mixed, trailing newline, BOM, non-ASCII): `edit` round-trips content exactly; mixed-EOL files produce the explicit error.
12. Session durability: crash-consistency tests on `Session::resume` — truncate the JSONL at every byte offset, assert resume recovers the longest valid prefix and appends cleanly.
13. Prefix-cache economics as a metric: extend `report()`/`/cost` with "cache hit %" per turn and a CI check on a recorded 30-turn session asserting hit rate > 90% after P1-#4.
14. Terminal safety (§3.8) + a PTY integration test (spawn air under `script`/`pty`, panic it, assert no residual `\x1b[1;Nr` scroll region).

### Standing practices

- `cargo clippy -- -D warnings`; `cargo-mutants` on `compact.rs` + `session.rs` (the two modules where silent wrongness is most expensive); `proptest` for `compact::cut` invariants: *the keep boundary never splits an assistant/tool-call pair; summary bytes are a pure function of the log; every cited `#n` is `recall`-able*.
- **Token fuzzing**: synthesize sessions with adversarial sizes (one 10 MB tool output, 10k tiny entries, CJK-heavy text) and assert `messages()` output ≤ budget and compaction preserves the anchors the audit demands — active check failure, `accept()` criteria, carry notes, unresolved failures, current task (all present in `summary()` today; lock them in as golden-file tests so refactors can't drop them).
- **Process-cleanup verification** in CI on every PR (P1-#6), since every lifecycle fix here has regressed-by-omission risk.

### Keep as-is (do not "fix")

The log-derived deterministic compaction with `recall(n)`; the `meta.host` distinction; the sandboxed `check()` with tree-fingerprint dirty detection; the GATE nudge; head/tail output truncation with `_last`; SIGINT masking around file mutation; `PYTHONHASHSEED`/`TZ`/`LC_ALL` pinning; the session-file lock. These are the parts of `air` that are genuinely better than the field.
