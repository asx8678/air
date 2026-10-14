# 03 — Line-by-Line / Module-Specific Recommendations

**Review date:** 2026-10-14 · All line references verified against the current tree.

---

## `session.rs`

**L88–127 `messages()` — cache the wire prefix.** The session is append-only between compactions, so serialization is recomputable incrementally:

```rust
pub struct Session {
    pub entries: Vec<Value>,
    // cache: serialized wire-form of every retained entry, plus the header message
    wire_cache: Vec<Arc<str>>,   // one JSON string per retained entry, meta/reasoning pre-stripped
    cache_valid_from: usize,    // == keep() of the last compaction
}

pub fn push(&mut self, entry: Value) -> Result<()> {
    let line = format!("{entry}\n");
    self.file.write_all(line.as_bytes())?;       // persisted form keeps meta
    if let Some(wire) = self.to_wire(&entry) {    // strips meta, old reasoning — pure fn
        self.wire_cache.push(wire);
    }
    self.entries.push(entry);
    Ok(())
}
```

Then `Llm::complete` concatenates cached `Arc<str>` slices instead of cloning Values, and `send` takes `Bytes` for O(1) retries:

```rust
// llm.rs — serialize once, retry cheap
let body: Bytes = Bytes::from(build_body(&prefix, &tail));   // bytes is already a reqwest transitive dep
// per attempt:
let request = self.http.post(&self.url)
    .header("content-type", "application/json")
    .body(reqwest::Body::from(body.clone()));   // refcount bump, not a 400KB memcpy
```

This drops per-request work from O(context) to O(delta) — and the byte-identical prefix is exactly what maximizes provider prompt-cache hits.

**L35–66 `resume()` — don't die on one bad interior line; don't double memory.** `text.lines().map(serde_json::from_str).collect::<Result<_,_>>()` aborts the entire session on any mid-file corruption, with no line number. Use `.enumerate()` + `.context()`, skip-and-warn interior bad lines, and `drop(text)` after parsing (raw text + parsed `Vec<Value>` are currently held simultaneously).

**L129–149 `state()` — O(n) per `run_code` (agent.rs:166) → O(n²) per turn.** `State` changes only at three known entry shapes (real user task resets `accepted`/`plan`; `meta.accepted`/`carry`/`plan` replace). Maintain it incrementally inside `push()`.

**L151–155 `answer_missing()`** — correct and elegant; keep. Add a unit test pinning the invariant: never emits a tool result before its assistant message, never orphans a `tool_call_id`.

---

## `llm.rs`

**L37–40 `overflow()` — parse before sniffing.** Check structured fields first:

```rust
fn overflow_status(text: &str) -> bool {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        let code = v["error"]["code"].as_str().unwrap_or_default();
        if code.contains("context_length") || v["error"]["type"] == "invalid_request_error" {
            return true;
        }
    }
    OVERFLOW.iter().any(|m| text.to_lowercase().contains(m)) // keep as fallback
}
```

Also honor `finish_reason == "length"` mid-stream: today a model whose tool-call JSON is cut off by the output limit is classified `Interrupted` and **the whole request is retried 3×** (llm.rs:70–77, ~identical truncation each time) instead of falling through to agent.rs:146–150's cheap "run() came without code — resend in smaller steps" recovery. Retry on network breakage; don't retry deterministic truncation — that's pure token burn.

**L108 `buf.drain(..=end).collect()` — O(n²) shifting over a chunk.** Keep a cursor instead:

```rust
let mut pos = 0;
while let Some(rel) = memchr::memchr(b'\n', &buf[pos..]) {
    let end = pos + rel;
    let line = &buf[pos..=end];
    // serde_json::from_slice skips the from_utf8_lossy + revalidation pass
    pos = end + 1;
}
buf.drain(..pos); // one shift per chunk, not per line
```

**L15–16, L57–61** — `Interrupted`/`ContextOverflow` are good structured errors; but `Client::builder()...expect(...)` in `Llm::new` panics on a user-configurable path. Return `Result<Self>`; `Agent::new` already propagates.

**Body hardening (L66–80):** add `"parallel_tool_calls": false` (calls are executed sequentially anyway — agent.rs:107–113 — and the delta-reassembly index heuristic at L133–152 has to guess when providers omit `index`), a configurable `max_tokens` ceiling, and optionally `seed` for the deterministic posture.

---

## `agent.rs`

**L82 + L91 triggers — fix the lag and the abort (see C5).** Concretely: (a) change `overflowed: bool` to a retry budget of 2 so a second compaction-and-retry is possible; (b) estimate the *pending* request locally before sending — the `len()/4` heuristic already exists in `compact.rs:10`:

```rust
let projected = self.session.projected_prompt_tokens(); // system + summary + wire tail, len/4
if projected >= self.compact_at { self.compact()?; }
```

(c) make `compact_at` / model context window a `Config` key; (d) warn once if a reply carries no `usage` so the silent-no-compaction mode is at least visible.

**L143–152 `call()`** — graceful degradation for malformed tool calls is excellent. Keep.

**L215–233 `snapshot()`/`fingerprint()`** — move to `tokio::task::spawn_blocking` or `tokio::process::Command`; better, compute the fingerprint *inside* the snapshot's git call (`git write-tree` hashes content — the per-file `metadata()` loop and its mtime flakiness are unnecessary; the tree SHA already is the fingerprint).

**L107 `pending`** — keep a `Vec<Value>` of open call ids; on interrupt, push synthetic results for *all* unsettled ids, not just the last one.

**L329–351 `system_prompt()`** — `AGENTS.md` is injected verbatim with no cap. A 50KB steering file silently consumes ~12k tokens of every request and pushes every overflow trigger earlier. Cap it (head 16KB + visible note) or load it on demand the way skills are (the skills implementation — description-line only, `open()` on demand — is exemplary progressive disclosure).

**L354 `tools()`** — rebuilt per request; hoist to a `static`/`LazyLock`.

---

## `compact.rs`

This module is the crown jewel: deterministic rebuild from the raw log (no compounding lossiness across successive compactions — round 2 re-reads the original entries, not round 1's summary), verbatim excerpts with the honest "historical, not a verified outcome" marker, unresolved-failure tracking that self-heals when the action later succeeds, `BTreeMap` file ranking, and a 32KB cap. Two defects:

**L149 `clip(&out, MAX_BYTES)` can amputate the recall contract.** Section caps sum to ~31.2KB before titles and multibyte inflation; if the total exceeds 32KB, the closing "[Entries #1-#N are summarized here; recall(n) returns entry #n exactly. Continue the task.]" — the line that makes truncation safe — is the first thing cut. Emit the closing line *unconditionally after* clipping, or clip per-section.

**L118–130 `block()` is O(n²) joins** in the worst case (rebuild the full joined block for each candidate `keep` from n→0). Binary-search `keep` (length is monotone in it) or precompute a per-line byte prefix-sum. Bounded today by the 40-event cap; fix it when you lift that cap.

**L10 `to_string().len() / 4`** — fine as a heuristic; note it counts session-only `meta` (overestimates wire size — conservative, good) and that compaction is rare so the full-JSON serialization per entry is acceptable.

**Missing test targets:** `cut()` must never land between an assistant message and its tool results (it can't, by the `role != "tool"` boundary rule — pin that invariant in a test); `summary()` should be byte-stable for a fixed log (determinism test); `clip`/`tail`/`excerpt` need char-boundary property tests.

---

## `worker.rs`

**L70–90 bwrap vs sandbox-exec signal divergence (see C4).** On interrupt, explicitly signal the worker's process group before falling back to kill: `kill(-pid, SIGINT)`, give `settle()` more than 3s when interactive, and make `edit()` write-atomic in worker.py:

```python
def edit(path, old, new):
    def make(full, rel):
        ...
        tmp = full + ".air.tmp"
        with open(tmp, "w", encoding="utf-8", newline=...) as f:
            f.write(result)
        os.replace(tmp, full)   # atomic within the same dir — no torn-write window
        return text, result
```

**L16–18 / L104–113** — the hermetic env, secret-name filter (fail-closed on `KEY|TOKEN|SECRET|PASSWORD` substrings), `kill_on_drop`, protected `.git`/hooks, and cache redirection are all right. Two notes: `sandbox-exec` is deprecated by Apple and its SBPL semantics drift across macOS versions — document a fallback; and the all-network-deny (except localhost) means a `check` that needs `cargo fetch`/`pip install` fails confusingly in non-yolo mode — document it or provide a check-time network escape.

---

## `worker.py`

**L112–115 `sh()` (see C1)** — minimal honest fix: record `sh()` invocations in the ledger and detect tree changes:

```python
def sh(cmd, timeout=120):
    before = _tree() if _state.get("accepted") or _report.get("accepted") else None
    out, code = _exec(cmd, timeout)
    if before is not None and _tree() != before:
        _report.setdefault("actions", []).append(
            {"tool": "sh", "path": cmd[:120], "ok": code == 0,
             "error": "changed the working tree outside edit()/write()"})
        print("[warning: sh() changed the working tree; actions recorded]")
    return out.rstrip("\n") + (f"\n[exit {code}]" if code else "")
```

(Deeper fix: monkey-patch `builtins.open` for write modes inside the exec globals so the `accept()` gate and ledger cover plain-Python writes too.)

**L85–99 `edit()`** — one-exact-match, CRLF preservation, realpath escape check: excellent. Add the atomic replace above.

**L181–199 `recall()`** — reads from line 0 every call; build a line-offset index once per worker start. And fix the `_last` contract (C6): when truncating, note that `_last` is worker-lifetime-only, or persist oversize outputs to a sidecar.

**L236–243 post-check validation** ("check no longer applies: files changed after it passed") is a great integrity tripwire — keep.

---

## `ui.rs` / `render.rs`

**L31–34 `size()`** — replace the `stty` subprocess with a TIOCGWINSZ ioctl (`terminal_size` crate or a 10-line `libc` call). Removes a blocking fork per second and per turn start.

**L144–176 `thinking()`** — unthrottled per reasoning-delta rendering (llm.rs:126–131 calls it on every chunk while the answer is empty); `writing()` already throttles to 80ms (L180–196) — apply the same gate to `thinking()`.

**Static-mutex unwraps (`BAR`, `LIVE`, `RUNNING`)** — a single panic in a UI callback poisons them and every later UI call panics for the process lifetime. Use `.lock().unwrap_or_else(|e| e.into_inner())` or `parking_lot`.

**render.rs L14 `printable()` per chunk** — return `Cow<'_, str>` (fast path when no control chars); **`visible()` (L132–139)** recomputes `inline()` + four `String::replace` allocations twice per table cell — hoist per-cell.

---

## Cross-cutting

- **`Interrupted` retry backoff `2 << attempt` (llm.rs:74)** = 4/8/16s — fine; but combined with the deterministic-truncation retry it can burn ~30s + 3 full prompts for a guaranteed-identical failure (see M1).
- **`reasoning_content` resubmission (session.rs:104–108)** — stripping pre-task reasoning is good economy; note it causes one full prefix cache-miss per new task, and that *resubmitting* reasoning at all is a provider interop risk (DeepSeek explicitly forbids it; strict servers may 4xx). Make it a config flag.
- **`Session::push` durability** — no fsync is a reasonable tradeoff given resume's torn-line repair; document that a hard crash loses the un-flushed tail.
