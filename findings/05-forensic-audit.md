# 05 — Forensic Audit: Compaction Integrity, Cancellation Safety & Silent Corruption

**Audit date:** 2026-10-14 · Second-pass forensic review, following documents 01–04. Cross-references to first-pass findings use `[→ C#]`.

**Method:** full-path tracing of all 9 source files; live protocol probes against the real `worker.py` spawned under its real env contract; grep-verified determinism/append-only evidence; line-level trigger-path analysis of compaction, interrupt, and persistence paths.

## Probe evidence (live, reproducible)

| Probe | Result |
|---|---|
| Torn request line + pipe EOF (simulated interrupt mid-`write_all`) | worker **exit 1, stderr empty** — fatal traceback silently routed to `/dev/null` |
| Garbage line mid-session (air still alive) | worker **exit 1, stderr empty** |
| Worker variables across runs (`x = 41` → `print(x + 1)`) | persist — `42` ✓ |
| 20k-char run output | truncated to 8059 chars; head+tail kept with omission marker ✓ |
| Same-second session sort (`-100` vs `-99`) | `sorted()[-1]` → `…-99.jsonl` — resume picks the wrong newest |

---

## 1. Forensic Findings Matrix

| ID | Severity | File:Line | Finding | Evidence |
|----|----------|-----------|---------|----------|
| **F1** | **HIGH** | `worker.py:227-228` + `worker.py:234,256` | Worker stdin framing has no failure isolation: `json.loads(request)` sits **outside** the per-run `try/except`. Any torn or malformed request line **kills the worker process** — and the death is *fully silent*: the traceback goes to `/dev/null` because `sys.stderr` was rebound to fd 1 during the previous request's capture dance (`worker.py:234`), and fd 1 is `dup2`'d to `/dev/null` at end-of-run (`worker.py:256`). All worker variables + `_last` lost; agent reports only "the worker was restarted." | **Live probe**: torn line → exit 1, stderr **empty**; garbage line mid-session → exit 1, stderr **empty** |
| **F2** | **HIGH** | `agent.rs:225-233, 215-223, 360-368` | Change detection is mtime+len based (`DefaultHasher` over `metadata()`), not content-addressed — and every git failure silently defaults. mtime churn (`touch`, `stash pop`, no-op formatter rewrite) → false *changed* → spurious check-gate runs; a runtime git failure (lock, index corruption) → `fingerprint()` both sides become hash-of-empty → **false "NO CHANGES" verdict** while files were modified. `snapshot()`'s `let _ = std::fs::copy(...)` (`agent.rs:219`) silently drops the undo baseline. | Code trace: `git()` → `Option<String>`, all call sites `unwrap_or_default()` |
| **F3** | **HIGH** | `session.rs:69,102`, `llm.rs:66-80,227` | **Five redundant serializations/copies of every large payload per request cycle**: (1) Value build, (2) `format!("{entry}\n")` at push, (3) `entry.clone()` per message per request, (4) body `to_string()`, (5) `body.to_owned()` per retry. A 100KB code payload resident across 150 requests ≈ **45MB of transient allocations**. Compaction itself is a full-history rebuild (3 passes over the range + `to_string()` per entry in `cut`), not incremental — tolerable only because it's rare. `[→ C2]` | Code trace with quantification |
| **F4** | **MED-HIGH** | `compact.rs:10`, `agent.rs:82,91`, `session.rs:78-81` | Token-estimation drift: `len()/4` undercounts code ~15–30% (real tokenizers: ~3.1–3.5 chars/token); the 32KB summary marker is *excluded* from the tail budget (`cut` skips role-None entries); trigger lags one request. Stacked margins put actual context ~+20k tokens above `compact_at` before compaction lands — safe only because 100k < typical 128k windows. The system *receives* true usage every request but never calibrates the heuristic with it. `[→ C5]` | Margin math, code trace |
| **F5** | **MED** | `session.rs:43-45`, `main.rs:36-40` | Session recovery fragility: (a) `File::open(&path)?` inside the resume scan — **one unreadable file in `~/.air/sessions` aborts `--resume` for every repo**; (b) `paths.sort()` is lexicographic — same-second sessions `…-99` vs `…-100` resume the **wrong newest** (probe-proven); (c) `load()` swallows all read errors → "no model" mystery failure with no hint of a config problem. | **Live probe**: `sorted(["…-100.jsonl","…-99.jsonl"])[-1]` → `…-99` |
| **F6** | **MED** | `llm.rs:107,187` | The SSE loop only processes complete `\n`-terminated lines; **the residual buffer is discarded at stream end**. A final usage line without trailing newline → silent usage loss → compaction trigger blind. A final `finish_reason` line without newline → `finished` stays false → "stream ended before the model finished" → **3 full-prompt retries of a deterministic condition** → turn failure. | Code trace of `'read` loop + residual `buf` |
| **F7** | **MED** | `agent.rs:13,111` | No per-turn token budget (`MAX_REQUESTS=200` is the only bound — worst case ≈ 200 × ~100k prompt tokens billed) and **no repeated-failure circuit breaker**: a model retrying the identical failing `edit()` burns full-context requests until the cap. The worker's `rejected` counter observes the loop but nothing acts on it. | Code trace |
| **F8** | **LOW-MED** | `session.rs:19` | The entire session log stays resident in RAM forever (append-only `Vec<Value>`, required for `recall(n)`/`state()` — never evicted or spilled). Tool outputs are capped at 8KB, but tool-call **arguments** and current-task `reasoning_content` are uncapped (ties into F3) → multi-MB sessions grow unboundedly in a long-lived REPL. | Grep-proven: no `remove/truncate/drain/retain` on `entries` |
| **F9** | **LOW** | `session.rs:29`, `session.rs:54-57` | Durability nits: `flock` (via `try_lock`) fails outright on NFS/lockless filesystems — session creation impossible; the `set_len` repair only handles a torn *last* line, so mid-file corruption = resume abort (compounds F5a); no `fsync` — accepted tradeoff, but undocumented. | Code trace |
| **F10** | **CLEARED** | `compact.rs`, `ui.rs`, `agent.rs` | Verified-safe (see §2.6): compaction determinism, boundary-splitting safety, lock discipline, no shared-mutable agent state across threads, LLM-stream cancellation safety, append-only index stability. | Grep + trace evidence |

---

## 2. Deep-Dive Vulnerability Analysis

### 2.1 The worker framing protocol: a cancellation-safety hole with a *silent* failure mode (F1)

The Rust↔worker protocol is newline-delimited JSON over a pipe. On the Rust side, `Worker::run` does `stdin.write_all(...)` then awaits the reply — a request can be torn **only** when the future is dropped mid-write (exactly what the `select!` on Ctrl-C in `Agent::run` does). But the worker side turns that rare tear into a permanent process death:

```python
# worker.py:227-228 — OUTSIDE any try/except
for request in _inbox:
    request = json.loads(request)          # ← raises on torn/garbage → process exits
```

The live probe confirms both failure shapes — torn-line-then-EOF (the interrupt signature) and garbage-while-alive — each exit code 1. The forensic kicker is the **silence**: probe stderr was empty. Root cause chain:

1. During request *N*, `worker.py:234` rebinds `sys.stderr = open(1, …)` — fd 1 is the per-run capture file.
2. At end of request *N*, `worker.py:256-257` points fds 1 **and 2** at `/dev/null` — but the Python-level `sys.stderr` object still wraps fd 1.
3. Request *N+1* fails `json.loads` → CPython prints the fatal traceback to `sys.stderr` → **/dev/null**.

So the operator sees only `error: the worker exited; the worker was restarted and its variables are gone` — no cause, no traceback, nothing on disk. Every accumulated variable and every `_last` buffer vanishes, and the model is left to re-derive state it believed was durable. This also converts any future protocol deviation from a one-request degradation into permanent worker death.

Note the contrast: the *code-under-execution* is wrapped in `except BaseException` with careful hints (worker.py:239+) — the designers failure-isolated the model's code but not their own framing layer.

### 2.2 Change detection: mtime is not content, and git failure is not an event (F2)

```rust
// agent.rs:225-233 — the "did anything change" oracle
let mut hasher = std::hash::DefaultHasher::new();
for file in list.split('\0').filter(|f| !f.is_empty()) {
    file.hash(&mut hasher);
    std::fs::metadata(self.root.join(file)).ok().map(|m| (m.len(), m.modified().ok())).hash(&mut hasher);
}
```

Two independent corruption vectors:

- **False negatives (verdict lies).** `git()` returns `Option` and every call site collapses `None` to a default (`unwrap_or_default()`, `?` in `snapshot`). If git fails transiently at turn start *and* turn end, both fingerprints hash an empty file list → identical → `changed = false` → **the report prints "NO CHANGES" and the turn exits 0** while the model's edits sit on disk. The same silent default erases the undo baseline: `agent.rs:219` ignores the index copy failure, and a subsequent `add -A` failure returns `None` from `snapshot()` with no log line.
- **False positives (spurious gating).** mtime+len flips on content-preserving events: `git stash pop`, `touch`, a formatter that rewrites byte-identical files, an editor save with no change. Each false positive triggers `verify()` (a full check run) and can misreport "UNVERIFIED" or — worse inside `verify()` — flag "the check changed the working tree" because the check's own benign mtime touches flipped `before != after`.

The irony: the codebase *already computes* a content-addressed identity — `snapshot()`'s `git write-tree` SHA — and then doesn't use it for change detection.

### 2.3 The five-copy pipeline & compaction's full-rebuild cost model (F3)

Tracing one large `run(code)` payload (say 100KB of streamed code) through a turn:

| Stage | Location | Cost per occurrence |
|---|---|---|
| 1. Value construction | `llm.rs:160+` (delta assembly) | 1 × 100KB |
| 2. Log line serialize | `session.rs:69` `format!("{entry}\n")` | 1 × 100KB (needed — it *is* the log) |
| 3. Wire clone | `session.rs:102` `entry.clone()` | 1 × 100KB **per request** |
| 4. Body serialize | `llm.rs:68` `json!(…).to_string()` | 1 × 100KB per request |
| 5. Retry copy | `llm.rs:227` `body.to_owned()` | 1 × 100KB **per attempt** |

Stages 3–5 repeat for every request the payload stays in context (until compaction): a payload resident for 150 requests generates ~450 redundant 100KB allocations — pure allocator churn on the critical path of a single-threaded runtime, where every `memcpy` is also latency. Compaction is a **full rebuild** each time it fires (`cut` serializes every entry with `to_string()`; `summary` makes three passes over the summarized range; `messages()` never caches) — acceptable at current scale precisely because compaction is rare, but it means the system's O(N) behaviors all concentrate in the one place N grows: the request loop. There is no incremental/append-only block strategy anywhere on the wire path, despite the session itself being append-only — the ideal substrate for prefix caching.

### 2.4 Token accounting: a heuristic drifting against reality, with the truth arriving unused (F4)

The estimator stack, end to end:

- `cut()` sizes the retained tail in "tokens" as `to_string().len() / 4` **including session-only `meta`** (overcounts structure, undercounts nothing), but **excluding the 32KB compact marker itself** (role-None entries are skipped in the sizing loop) — the summary rides in-context uncounted.
- Code-heavy content tokenizes at ~3.1–3.5 chars/token, so an "80KB tail budget" (`compact_at/5` at the 20k keep) is really ~23–26k wire tokens — a 15–30% overshoot baked into every compaction.
- The trigger (`agent.rs:91`) compares against `last_prompt_tokens()` — the **last completed request's** count. Between a huge tool result and the next request, the trigger sees the pre-bloat number, so the request that crosses the window is always sent first; recovery is delegated to the overflow path, whose adaptation (`min(seen * 9/10)`) uses the last *successful* size and thus under-retracts when the bloat was in the failed request. `[→ C5]`

Stacked: `compact_at` + tail overshoot (~+5k) + summary (~+8–10k) + one-request lag (~+3–12k) ≈ **+20k tokens of true context above the nominal trigger** before compaction lands. With the default 100k on a 128k window this is survivable margin; the system never *states or checks* that margin, so any user who pins `compact_at` to their model's true window gets deterministic overflow. The system receives ground truth (`usage.prompt_tokens`) after every request and throws it away — the fix is a self-calibrating estimator (§3.4).

### 2.5 Session recovery: three independent brick-risks (F5, F9)

- `session.rs:45`: `File::open(&path)?` inside the scan loop — one unreadable file (permissions, a directory named `foo.jsonl`, dangling symlink) propagates and **aborts resume for every repo**, even though the loop's *intent* is clearly "skip non-matching sessions" (it already skips files whose header doesn't match cwd).
- `session.rs:43`: `paths.sort()` compares filenames as bytes. Same-second creation with pids 99 vs 100 → `"-99" > "-100"` lexicographically → **resume picks the older session** (probe-proven). One-in-a-thousand, but it silently loads the wrong transcript.
- `main.rs:36-40`: `load()` maps *any* read error to `Config::default()` — a permission-denied `config.toml` surfaces later as a baffling "no model" error with zero indication the config was unreadable.

Durability posture is otherwise sound (see §2.6), but the torn-line repair only covers the *last* line; mid-file corruption (bit rot, partial disk write of an interior line) makes `serde_json::from_str` fail the whole `collect::<Result<…>>` — session unrecoverable despite 99.9% intact data.

### 2.6 What the audit **cleared** (F10) — with evidence

A forensic pass that only lists flaws isn't an audit; these high-suspicion vectors were investigated and **cleared**:

1. **Compaction determinism.** `compact.rs` contains zero nondeterminism sources (grep-proven: no `SystemTime`/`Instant`/`rand`/`uuid`/`HashMap` — `BTreeMap` throughout, and serde_json's default `BTreeMap` key ordering makes `json!` output stable). `summary()`/`cut()` are pure functions of the log: same log in, same bytes out — across restarts, resumes, and replays. The only usage-derived value (`compact_at` adaptation, persisted in the marker's `at`) varies *between* logical sessions, but within a session the log is the single source of truth and marker replay is deterministic.
2. **Mid-stream boundary splitting.** Compaction has exactly three trigger sites — turn start (`agent.rs:82`), loop head (`agent.rs:91`), post-overflow (`agent.rs:95`) — all at request boundaries; there is **no compaction path reachable during streaming**. Entries are atomic units; nothing truncates inside an entry except the deliberate `excerpt`/`line`/`tail` cuts (all char-boundary-safe: `clip` walks back, `tail` walks forward). The post-compaction wire prompt is well-formed by construction (summary as one user message + intact tail + `answer_missing` repair). No malformed-JSON or mid-thought truncation hazard exists. Sole caveat: the `MAX_BYTES` tail-clip can amputate the recall instruction (first-pass M5).
3. **Semantic anchors.** Accepted criteria (verbatim 1800B), carry notes (explicit guardrails that survive every compaction until cleared), failing-check tail (1536B), files map with modified/created ranking, self-healing unresolved-failure list, and 40 entry-numbered events are all preserved. Two real weaknesses: user directions are **head-only** excerpts (end-of-spec constraints in a long task can be cut), and the anchor set is only as truthful as `meta.actions` — which `sh()` falsifies (first-pass C1 is therefore a *compaction-integrity* bug, not just an audit gap: the summary can positively assert "no file changes" while the tree changed).
4. **Locks/threads.** Single-lock-at-a-time discipline holds throughout `ui.rs` (never two statics held simultaneously; `drop(bar)` before `draw()`); **no lock is ever held across an await**; the agent's mutable state never crosses a thread boundary (no `Arc`, one spawned task — the ticker — touching only display statics). Deadlock is structurally impossible in the current design.
5. **LLM-stream cancellation.** Dropping the `complete()` future mid-stream closes the reqwest response cleanly; no partial state is committed to the session. `Session::push` is synchronous and completes atomically within the event loop.
6. **Append-only index stability.** Grep-proven: `entries` has no `remove/truncate/drain/retain/insert` — only `push`. Every index anchor (`#n` recall numbering, `began`, `keep`) is stable for the life of the session, across compactions and resumes. This invariant silently underpins `recall(n)`'s correctness and deserves a test pinning it.
7. **Compaction crash-safety.** The marker is one atomic append; losing it to a torn write loses *nothing* — the summary is derived state, deterministically rebuilt from the log on the next compaction, and the lost `compact_at` adaptation self-heals via the overflow path. This "checkpoint-is-derived, log-is-truth" design is genuinely excellent.

---

## 3. Production-Grade Rust Refactoring

### 3.1 Failure-isolated worker framing (F1)

The protocol must satisfy: *no input line may kill the worker.* Minimal Python fix:

```python
# worker.py — request loop, failure-isolated
for request in _inbox:
    try:
        request = json.loads(request)
    except ValueError as e:
        # A torn line means air was interrupted mid-write; never die on framing.
        _outbox.write(json.dumps({"out": f"[worker: discarded a malformed request ({e})]",
                                  "error": True}) + "\n")
        _outbox.flush()
        continue          # fresh line, fresh request — worker state survives
    ...
```

Additionally, stop the silent-traceback class entirely by restoring the real stderr after each run:

```python
sys.stdout = sys.stderr = open(1, "w", ..., closefd=False)
# ... end of run:
sys.stderr = sys.__stderr__      # fatal errors must always reach the parent
```

And make interrupts non-fatal for worker state by signaling rather than discarding — in `Agent::run`'s interrupt path, before the 3s `settle()`:

```rust
// worker.rs — forward the interrupt instead of relying on terminal signal delivery
impl Worker {
    pub fn signal(&self) {
        // start_new_session'd on bwrap; send to the whole group so a run
        // blocked in sh() aborts the way KeyboardInterrupt would on macOS.
        let _ = unsafe { libc::kill(-(self._child.id().unwrap() as i32), libc::SIGINT) };
    }
}
```

### 3.2 Content-addressed change detection (F2) — reuse the identity you already compute

```rust
// agent.rs — replace fingerprint() with the snapshot's own content hash
fn tree_id(&self) -> Option<String> {
    let index = self.index();
    git(&self.root, Some(&index), &["add", "-A"])?;          // private index: never touches the real one
    git(&self.root, Some(&index), &["write-tree"]).map(|t| t.trim().to_owned())
}

// turn():
let start = self.tree_id();          // spawn_blocking around both git calls
...
let changed = self.tree_id().as_ref() != start.as_ref();   // content, not mtime
```

This deletes the O(files) `metadata()` loop (also fixing the first-pass C3 stall), is immune to mtime churn, and — critically — makes git failure *observable*:

```rust
let changed = match (start, end) {
    (Some(a), Some(b)) => a != b,
    _ => bail!("git failed; refusing to report a change verdict I cannot verify"),
};
```

That single `bail!` converts the "NO CHANGES while files changed" verdict-lie into a visible error.

### 3.3 Append-only wire cache + `Bytes` retries (F3) — O(delta) requests

```rust
pub struct Session {
    pub entries: Vec<Value>,
    file: File,
    /// Serialized wire form of every live entry (meta stripped), in push order.
    wire: Vec<Arc<str>>,
    wire_bytes: usize,                 // O(1) size accounting for F4's estimator
    state_cache: State,                // incremental (fixes the O(n²) state() scan)
}

impl Session {
    pub fn push(&mut self, entry: Value) -> Result<()> {
        self.file.write_all(format!("{entry}\n").as_bytes())?;
        self.absorb_state(&entry);                       // incremental, O(entry)
        if let Some(wire) = wire_form(&entry) {           // pure: strip meta, old reasoning
            self.wire_bytes += wire.len();
            self.wire.push(wire);
        }
        self.entries.push(entry);
        Ok(())
    }

    /// The request body's message array, concatenated — no per-request clones.
    pub fn wire_prefix(&self, from: usize) -> impl Iterator<Item = Arc<str>> + '_ {
        self.wire[from.min(self.wire.len())..].iter().cloned()
    }
}
```

```rust
// llm.rs — serialize once; retries cost a refcount
pub async fn complete(&self, session: &Session, tools: &Value) -> Result<Value> {
    let body: Bytes = Bytes::from(self.build_body(session, tools));  // one pass, O(delta) via wire cache
    for attempt in 1.. {
        match self.stream_with(Body::from(body.clone())).await {     // O(1) clone
            Err(e) if attempt < 4 && e.is::<Interrupted>() => { /* backoff */ }
            result => return result,
        }
    }
    unreachable!()
}
```

### 3.4 Self-calibrating token estimator (F4) — learn the ratio from the usage you already receive

```rust
/// Calibrated chars-per-token from the provider's own usage reports.
pub struct TokenMeter {
    ratio: f64, // EMA of wire_chars / prompt_tokens, seeded at 4.0
}

impl TokenMeter {
    pub fn observe(&mut self, wire_chars: usize, reported: Option<u64>) {
        if let Some(t) = reported.filter(|&t| t > 1024) {
            let r = wire_chars as f64 / t as f64;
            self.ratio = 0.7 * self.ratio + 0.3 * r;
        }
    }
    /// O(1): session already tracks wire_bytes incrementally.
    pub fn estimate(&self, wire_bytes: usize) -> u64 {
        (wire_bytes as f64 / self.ratio) as u64
    }
}

// agent.rs turn loop — project BEFORE sending; close the one-request lag:
let projected = self.meter.estimate(self.session.wire_bytes() + self.pending_delta());
if projected >= self.compact_at { self.compact()?; }
```

This converts the trigger from trailing to *leading*, adapts per-model/per-language automatically, and the `observe()` call rides the existing `ui::usage` hook site.

### 3.5 Drain the residual SSE buffer (F6)

```rust
// llm.rs stream(): after the 'read loop ends
if !buf.is_empty() {
    if let Ok(line) = std::str::from_utf8(&buf) {       // last line, no trailing \n
        if let Some(data) = line.trim().strip_prefix("data:") { /* same handler as above */ }
    }
}
```

Extracting the per-line handler into a closure makes both the loop body and the tail-drain share one path — usage loss and spurious "stream ended" retries both disappear.

### 3.6 Repeated-failure circuit breaker (F7)

```rust
/// Identical failing actions get one sharp retry, then a forced strategy change.
struct FailureBreaker { last: Option<(u64, String)>, strikes: u8 }

impl FailureBreaker {
    fn check(&mut self, meta: &Value) -> Option<String> {
        let failed = meta["actions"].as_array().map(|a| a.iter().any(|x| x["ok"] != true));
        if failed != Some(true) { self.strikes = 0; return None; }
        let mut h = DefaultHasher::new();
        meta["actions"].to_string().hash(&mut h);
        let key = (h.finish(), meta["error"].as_str().unwrap_or("").to_owned());
        if self.last.as_ref().is_some_and(|(k, _)| *k == key.0) {
            self.strikes += 1;
        } else { self.strikes = 1; }
        self.last = Some(key);
        (self.strikes >= 3).then(|| format!(
            "this exact action has failed {} times in a row; change approach — \
             print the current file state before editing again", self.strikes))
    }
}
```

Appending the returned string to the tool result (or as a host note) breaks the loop for ~the cost of one message instead of dozens of full-context requests.

### 3.7 Fault-isolated resume + numeric ordering (F5)

```rust
for path in paths.into_iter().rev() {
    let first = match File::open(&path).map(BufReader::new) {   // per-file isolation
        Ok(r) => r.lines().next().transpose().with_context(|| path.display().to_string())?,
        Err(_) => continue,                                      // one bad file must not brick resume
    };
    ...
}

// and sort numerically on the embedded timestamp:
paths.sort_by_key(|p| {
    p.file_name().and_then(|n| n.to_str())
        .and_then(|n| n.split('-').next())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
});
```

---

## 4. Resilience & Testing Recommendations

The crate has **zero tests**; its architecture (pure compaction functions, append-only log, protocol boundaries) makes it unusually testable. Highest-ROI order:

1. **Torn-write property test (session durability).** For a fixture session file, truncate at *every* byte offset and assert `Session::resume` either succeeds or cleanly drops exactly the last line — never panics, never yields a mid-line entry:
   ```rust
   proptest! {
       #[test]
       fn resume_survives_any_truncation(fix in session_fixture()) {
           for cut in 0..=fix.len() {
               write_truncated(&fix, cut);
               if let Ok(s) = Session::resume(&home, &root) {
                   prop_assert!(log_is_line_aligned(&s));
               }
           }
       }
   }
   ```
   Then extend to *interior* corruption: the current code fails the whole resume; the recommended fix (skip-and-warn) changes the property to "resume succeeds, dropping only corrupt lines, with a warning" — write the test first.

2. **Compaction invariant tests (deterministic state).**
   - `cut()` boundary: over randomized synthetic sessions, assert `entries[keep].role != "tool"` **and** that no assistant-with-`tool_calls` in the kept tail lacks its results in the kept tail — the protocol-pairing invariant that `answer_missing` currently guarantees only at wire time.
   - `summary()` byte-determinism: `summary(&entries) == summary(&entries)` across two fresh processes (catches any future `HashMap`/iteration-order regression — the test is cheap insurance against the one line of refactor that would silently break cross-replay determinism).
   - `messages()` protocol validator: every `tool` message must be preceded by an assistant carrying its `tool_call_id`; every assistant `tool_calls` id must resolve — run this over post-compaction, post-interrupt, and post-resume sessions alike.

3. **Worker framing fuzz (F1 regression).** Spawn the real `worker.py`; feed it pseudorandom line sequences — truncated JSON, raw garbage, valid-then-torn, empty lines, megabyte lines — and assert the invariant that fixes F1: **the worker never exits before stdin EOF and always emits a reply or skips**. This is a 20-line test that permanently guards the most fragile protocol seam.

4. **Token-estimator calibration fixtures.** Record `(wire_chars, provider_reported_prompt_tokens)` pairs from real sessions as fixtures; assert the calibrated estimator lands within ±15% on held-out entries, and that `estimate` is monotone in wire size. Fuzz the boundary: inputs sized exactly at `compact_at ± 1`.

5. **Interrupt-injection tests (cancellation safety).** With `tokio::time::pause()` and a mock LLM whose future is parked at each await point, drop the `turn()` future at every suspension site (before body send, mid-stream, before/after `Worker::run`'s write, mid-verify) and assert: session log remains protocol-valid (validator from #2), no torn JSONL line, `pending` bookkeeping produces a synthetic result for the open call, and — with the F1 fix — a subsequent `Worker::run` still finds the worker alive with variables intact.

6. **Change-detection truth tests (F2 regression).** Script: `touch` every file → `changed == false` under tree-SHA detection; git shim that fails (`PATH` with a failing `git`) → the agent *errors loudly* rather than reporting a verdict; formatter that rewrites byte-identical content → `changed == false`. These pin the semantics that mtime-based detection currently violates.

7. **Loop-economics guard.** A mocked-LLM integration test where the model emits the same failing `edit()` call 10 times: assert the circuit breaker (§3.6) fires by request 3 and the turn ends with a diagnostic, total prompt tokens under a budget cap.

8. **Session-selection test (F5b).** Create two same-second sessions with pids `99` and `100`; assert `resume` selects the later one — the exact tie the probe demonstrated failing today.

**Priority:** #3 (framing fuzz) and #1 (truncation property) guard the two findings that destroy user state silently; #2 guards the determinism guarantees that make `recall(n)` trustworthy; the rest are cheap pins on semantics the current code only holds by accident.

---

## Bottom line

The architecture's core commitments — deterministic log-derived compaction, append-only sessions, request-boundary-only triggers — all *survive* forensic scrutiny (§2.6), which is rare. The exposures are concentrated at the process boundaries where the design's assumptions meet failure: the worker framing layer that converts an interrupt into silent state destruction (F1), the mtime-and-silent-git change oracle that can print false verdicts (F2), and the five-copy request pipeline that makes long sessions expensive by construction (F3). All three are fixable with the small, local refactors in §3.
