# 04 — Actionable Remediation Plan

**Review date:** 2026-10-14 · Prioritized by risk×effort. Each item cites its finding IDs.

---

## P0 — Correctness & safety (do first; all are small diffs)

1. **Atomic `edit()`** via temp-file + `os.replace` in worker.py, and forward SIGINT to the worker's process group on interrupt; lengthen the interactive `settle()` window. *(C4)*
2. **`overflow()` structured-JSON parsing** + treat `finish_reason=="length"` as non-retryable truncation that flows to the existing graceful "resend in smaller steps" path; change the overflow guard to a retry budget of 2. *(C5, M1)*
3. **`sh()` ledger + tree-change tripwire** so the deterministic summary can't silently diverge from disk. *(C1)*
4. **Fix the summary tail**: guarantee the `recall(n)` closing line survives `MAX_BYTES` clipping. *(M5)*
5. **`_last`-after-restart honesty** in truncated-output text, or sidecar persistence. *(C6)*

## P1 — Hot-path performance

6. **Wire-prefix caching in `Session`** (serialize once per entry at `push()`; concatenate `Arc<str>`) + `Bytes`-backed request body for O(1) retries. Turns per-request cost O(context)→O(delta) and maximizes provider cache hits. *(C2)*
7. **De-block the runtime**: `spawn_blocking` (or `tokio::process`) for `git()`/`fingerprint()`/`changes()`; replace `stty` with TIOCGWINSZ; `BufWriter` on the session file. *(C3)*
8. **Incremental `State` cache** in `push()`; hoist `tools()` to a `LazyLock`; SSE line scanning via cursor + `from_slice`. *(L3, L7, L1)*

## P2 — Token economy & determinism

9. Cap `AGENTS.md` injection; add `compact_at`/`context_window`/`max_tokens` config keys; warn when usage isn't reported; add `parallel_tool_calls:false` and optional `seed`. *(M7, C5, M6)*
10. Throttle `ui::thinking()`; `Cow` in `render::push`; de-poison UI mutexes. *(M4, L5, L4)*
11. `resume()`: per-line context, skip interior corruption, drop raw text after parse; `recall()` line-offset index. *(M3, L6)*
12. Make `reasoning_content` resubmission a config flag; document the caching model (per-task and per-compaction cache-miss cost). *(M2)*

## P3 — Productionization

13. **Tests for the pure core** (highest ROI per line in the repo): `cut()` boundary invariant, `summary()` byte-determinism, `messages()`/`answer_missing` pairing repair, `clip`/`tail` char boundaries, `overflow()` classification, worker `edit`/`write`/`accept` gate behavior. *(C7)*
14. **Minimal CI**: fmt + clippy + `cargo test`. *(C7)*
15. **Document the security model**: sandbox-is-the-boundary (not consent), `--yolo` semantics, the `sandbox-exec` deprecation risk on macOS, and the network-deny friction for `check`. *(L9, C1)*
16. **Interrupt bookkeeping**: track all open call ids, not just the last; synthesize results for all unsettled ids on interrupt. *(L10)*

---

## What to keep, unchanged

- The **deterministic log-derived compaction** with `recall(n)` — no LLM summarization, verbatim excerpts, honest "historical, not a verified outcome" markers, self-healing failure tracking.
- The **append-only JSONL + flock + torn-line repair** session model.
- The **one-shot check gate** and bounded `MAX_REQUESTS`.
- **`answer_missing()`** protocol repair.
- **Head+tail truncation** of run output with `_last` retention (after C6's honesty fix).
- The **skills progressive-disclosure** pattern.
- The **hermetic worker env** and protected `.git`/hooks.
- The **temperature-0 / pinned-model / PYTHONHASHSEED=0** deterministic posture.

These are the parts of `air` that are ahead of most coding agents.
