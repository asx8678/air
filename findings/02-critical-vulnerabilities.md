# 02 — Critical Vulnerabilities / Anti-Patterns

**Review date:** 2026-10-14 · Full file/line references from the current tree.

---

### C1. `sh()` bypasses the write gate and the action ledger (worker.py:112, worker.py:62–84) — *critical*

`edit()`/`write()` enforce `accept()` (worker.py:74–75), repo-escape checks, protected paths, and record `{tool, path, ok, added, removed}` into `_report["actions"]`. `sh()` enforces **nothing**: `sh("rm -rf src")`, `sh("echo bad > file")`, `sh("find . -name '*.rs' -delete")` run with no consent, no validation, no ledger entry. Consequences cascade:

- `compact::summary()` (compact.rs:56–90) builds **Files / Status / Unresolved failures / Latest events** exclusively from `meta.actions` — changes made via `sh()` are invisible to the compaction summary, so the "deterministic state" narrative silently diverges from disk truth.
- The system prompt's "Files change only through `edit` and `write`" (prompt.md) is an honor-system contract, not an enforced boundary.
- Recovery depends on the turn-start snapshot for tracked files; new files destroyed via `sh()` are unrecoverable.

The sandbox bounds the blast radius (repo + tmp only, `.git`/hooks write-denied — which incidentally prevents `git commit`), but **within the repo, destructive actions require zero consent**. There is no read-only mode and no interactive approval anywhere in the design.

**Fix (pick ≥1):** (a) interpose a wrapper around `builtins.open` for write modes in the exec globals enforcing `accept()` + ledger recording; (b) record `sh()` invocations in the action ledger with a before/after tree diff so the summary stays truthful; (c) at minimum emit `[warning: sh() may have changed files]` into the run output when the fingerprint changes, so the model and the summary both see it.

---

### C2. O(session) clone-and-serialize of the full context per request (session.rs:88–127, llm.rs:66–80, llm.rs:227) — *high*

Per LLM request, the code pays: (1) `entry.clone()` — a deep `Value` clone of every retained message plus a fresh clone of the whole system prompt (session.rs:102); (2) `json!({"messages": messages}).to_string()` — full serialization; (3) `body.to_owned()` in `send()` — another full copy per retry attempt (llm.rs:227). At `COMPACT_AT = 100_000` tokens that's ~400KB of allocations copied 3× per request, up to 200 requests per turn — the single largest avoidable cost in the program, and it grows precisely as sessions grow long.

```rust
// llm.rs:227 — clones the entire request body per attempt
let mut request =
    self.http.post(&self.url).header("content-type", "application/json").body(body.to_owned());
```

**Fix:** cache the serialized wire prefix (session is append-only between compactions — serialize each entry once at `push()` time) and build bodies by concatenating cached strings + the tail delta; use `bytes::Bytes` (already a reqwest transitive dep) so retries are `Body::from(bytes.clone())` — a refcount bump instead of a 400KB memcpy. Turns per-request work from O(context) to O(delta) and aligns byte-identical prefixes with provider prompt caching. Concrete code in [03-module-recommendations.md](03-module-recommendations.md#sessionrs).

---

### C3. Blocking subprocess/file I/O on a `current_thread` runtime (agent.rs:360–368, agent.rs:225–233, ui.rs:31–34, session.rs:68–72) — *high*

`#[tokio::main(flavor = "current_thread")]` (main.rs:43) means every blocking call stalls the *entire* executor — SSE buffering, worker stdout reads, the UI ticker, everything:

- `git()` (agent.rs:360–368) uses `std::process::Command::output()`; called from async `turn()` via `snapshot()` (agent.rs:215) and `changes()` (agent.rs:370).
- `fingerprint()` (agent.rs:225–233) runs `git ls-files -co` then **`fs::metadata` on every file sequentially** — O(files) blocking syscalls, executed at turn start, before report, and twice inside `verify()`. On a large repo this is seconds of stall, during which the Python worker's stdout pipe (64KB) can fill and block the child mid-run.
- `ui::size()` (ui.rs:31–34) **forks an `stty size` subprocess** and is called by the 1-second ticker task (ui.rs:59, 82–83) — a blocking fork+exec every second inside a spawned tokio task.
- `Session::push` (session.rs:68–72) does an unbuffered blocking `File::write_all` per entry.

**Fix:** `tokio::task::spawn_blocking` (or `tokio::process::Command`) for git/stat work; replace `stty` with a TIOCGWINSZ ioctl (`terminal_size` crate or a 10-line `libc` call); `BufWriter` on the session file. Better: compute the fingerprint from the snapshot's `git write-tree` SHA (content-addressed, no per-file stat loop, no mtime flakiness).

---

### C4. Platform-divergent interrupt semantics + non-atomic `edit()` writes (worker.rs:70–90, worker.py:85–99, agent.rs:66–77) — *high*

The bwrap sandbox uses `--new-session` (setsid) so the worker **never receives terminal SIGINT**; the macOS `sandbox-exec` path doesn't detach, so it does. On Linux, Ctrl-C during a long `run()` waits the 3s `settle()` window (worker.rs:52–60), then agent.rs:70 drops the worker → `kill_on_drop` → **SIGKILL mid-execution**. Meanwhile `edit()` (worker.py:97–98) does `open(full, "w")` (immediate truncate) then `f.write(result)` — a kill between the two leaves the file truncated/empty. Tracked files recover via the turn snapshot; files created this turn don't. Also, `settle()`'s single reply is attributed to `self.pending` (agent.rs:66–73), which holds only the **last** call id (agent.rs:107 overwrites it per call) — with parallel tool calls, earlier calls' outcomes are misattributed or lost from the log.

**Fix:** forward SIGINT to the worker's process group explicitly on interrupt (`kill(-pid, SIGINT)`); lengthen the interactive `settle()` window; make `edit()` write-atomic (temp file in same dir + `os.replace()`); keep a `Vec` of open call ids and push synthetic results for all unsettled ids on interrupt.

---

### C5. Compaction trigger: trailing measure, one-request lag, hard abort on second overflow, silent when usage unreported (agent.rs:91–100, session.rs:78–81, llm.rs:15–16) — *high*

- `last_prompt_tokens()` returns the **most recent completed request's** count. After a huge tool result, the trigger still sees the pre-bloat number — the request that crosses the window is sent anyway. The overflow fallback recovers, but `compact_at = min(compact_at, seen * 9/10)` (agent.rs:98–99) adapts against the last *successful* size, underestimating the reduction needed when the overflowed request contained the bloat.
- The `!overflowed` guard (agent.rs:94) means a **second** overflow hits `result => result?` and aborts the whole turn, even though compacting again would very likely fix it.
- If the provider ignores `stream_options: {include_usage: true}`, `prompt_tokens` is never present → `last_prompt_tokens()` is always 0 → **automatic compaction never fires at all**; every long session is then governed solely by the error-text sniffer.
- That sniffer (`overflow()`, llm.rs:15–16, 37–40) matches six English substrings against the lowercase error body. A provider with different phrasing or structured `{"error":{"code":"context_length_exceeded"}}` is classified fatal → turn dies instead of compacting.
- `COMPACT_AT` is hard-coded (agent.rs:14); no config knob for small-window models (32k) — first use with such a model guarantees an overflow round-trip.

**Fix:** (a) overflow retry budget of 2 instead of a bool; (b) estimate the *pending* request size locally before sending (the `len()/4` heuristic already exists in compact.rs:10); (c) `compact_at`/`context_window` config keys; (d) warn once when a reply carries no `usage`; (e) structured overflow classification (see M1).

---

### C6. `_last` recovery contract is a lie after worker restarts (worker.py:256–259, worker.py:181–199) — *medium*

Truncated run output says "[… N characters omitted; the full output is in `_last`]" — true only within the worker's lifetime. After a restart (timeout, crash, interrupt-kill, worker error path agent.rs:172–177), `_last` is gone and `recall(n)` returns only the truncated text. The permanently-lost middle is claimed to be recoverable. Since `recall(n)` is advertised in every compaction summary's closing line ("recall(n) returns entry #n exactly"), this is a trust-contract violation, not a cosmetic bug.

**Fix:** append "(the full text was in `_last`; if the worker restarted, re-print the file)" when truncating, or persist oversize outputs to a sidecar file next to the session.

---

### C7. Zero tests, zero CI — *high*

`compact::cut`/`summary`, `Session::messages`/`state`, `answer_missing`, `llm::overflow`, the `block`/`clip`/`tail` boundary logic — all pure, all subtle, all untested. A regression in `cut()`'s tool-boundary invariant or `messages()`'s pairing repair would only surface as provider-side protocol errors in the field.

**Fix:** unit tests pinning: `cut()` never lands between an assistant message and its tool results; `summary()` is byte-stable for a fixed log; `messages()` + `answer_missing` never orphan a `tool_call_id`; `clip`/`tail` char-boundary safety; `overflow()` classification table; worker `edit`/`write`/`accept` gate behavior. Add minimal CI (fmt + clippy + `cargo test`).
