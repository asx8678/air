# 01 — Executive Summary

**Review date:** 2026-10-14
**Subject:** `air` v0.1.0 — "Minimal code-mode coding agent" (Rust 2024 edition, toolchain 1.99, single crate, 1,772 LOC)
**Scope reviewed:** `main.rs` (96) · `agent.rs` (383) · `llm.rs` (254) · `compact.rs` (201) · `session.rs` (155) · `worker.rs` (130) · `ui.rs` (414) · `render.rs` (139) · `worker.py` (265) · `prompt.md` (46) — every line read.
**Verification performed:** `cargo check` clean · `cargo clippy` silent · **no tests found** · **no CI config**.

## High-level assessment

`air` is a genuinely well-architected minimal agent: a **single-tool, code-mode design** (`run(code)` → persistent sandboxed Python worker), an **append-only JSONL session log** with flock, torn-line repair, and model pinning, and — the standout — a **fully deterministic compaction system** that rebuilds summaries as a pure function of the session log with verbatim excerpts and a `recall(n)` exact-recovery escape hatch. No LLM summarization anywhere in the compaction path means no hallucinated history, ever. The check-gate loop (`verify()` + one-shot `nudged` gate), bounded `MAX_REQUESTS` (200/turn), `answer_missing()` protocol repair, and head+tail output truncation with `_last` retention show deliberate, mature engineering. `temperature: 0.0`, `PYTHONHASHSEED=0`, hermetic env scrubbing, and git-snapshot-per-turn undo round out a strong deterministic posture.

The real weaknesses are in the **hot paths and enforcement seams**, not the architecture:

1. **Token/context assembly is O(session) per request.** `Session::messages()` deep-clones every retained `serde_json::Value`, `Llm::complete` re-serializes the entire context, and `send()` clones the body string again per retry — three full passes over up to ~400KB of context, ×up to 200 requests per turn.
2. **Blocking work inside a `current_thread` tokio runtime.** Host-side `std::process::Command` git calls, a per-file `metadata()` stat loop in `fingerprint()`, an unbuffered session-file write per entry, and a `stty size` subprocess forked every second by the UI ticker.
3. **Code-mode enforcement has a hole.** `sh()` bypasses the `accept()` gate and the action ledger entirely — the audit trail the deterministic summary depends on under-reports reality.
4. **Interrupt handling diverges by platform** (bwrap `--new-session` vs `sandbox-exec`), and `edit()` truncates-then-writes non-atomically — a Ctrl-C mid-write can leave a torn file.
5. **Compaction triggers lag one full request**, abort on a second overflow, silently never fire if the provider omits usage, and classify overflow by sniffing English error text.

## Token safety score: 8.5 / 10

**Earned:** ~700-token system prompt with progressive-disclosure skills (description line only, `open()` on demand); single tool schema; 8KB head+tail tool-output caps per run; 32KB summary cap; stable append-only message prefix for implicit provider prompt caching with hit-rate displayed; temperature 0.0; whole-step-per-script batching to minimize round trips.

**Deductions:** uncapped `AGENTS.md` injection; no explicit cache-breakpoint strategy; trigger lag of one full request; second-overflow aborts the turn; no `max_tokens`/`parallel_tool_calls`/`seed` in the request body; deterministic truncation retried 3× (burning full prompts) instead of falling through to the graceful "resend in smaller steps" path.

## Production readiness

Strong for a single-user CLI: clean build and clippy, robust error propagation (anyhow + typed `ContextOverflow`/`Interrupted`), session durability (append-only JSONL, flock, torn-line repair, model pinned in header), sandboxing (macOS SBPL / Linux bwrap, `.git`+hooks protected, secrets unreadable, env scrubbing). Not yet hardened for hostile providers (usage-reporting variance, error-dialect variance) or shared use (no tests, no CI, honor-system write gate).

## What this codebase gets right (keep unchanged)

- **Deterministic log-derived compaction** with verbatim excerpts, honest "historical, not a verified outcome" markers, self-healing failure tracking, and `recall(n)` exact recovery — no compounding lossiness (round 2 re-reads original entries, not round 1's summary).
- **Append-only JSONL sessions** (0600/0700, flock, torn-line repair, model pinning) — fully auditable, crash-resumable.
- **`answer_missing()`** — synthesizes "tool result missing (interrupted)" so the wire protocol stays valid after crashes/interrupts.
- **One-shot check gate** (`verify()` + `nudged`) — no infinite fix loops; `MAX_REQUESTS` bounds the turn.
- **Head+tail truncation** of run output with `_last` retention (same-worker-lifetime caveat: C6).
- **Skills progressive disclosure** — only the `description:` frontmatter line enters the system prompt.
- **Hermetic worker env** — `env_clear`, fail-closed secret-name filter, pinned cache dirs, `kill_on_drop`, protected `.git`/hooks (prevents commits and hook-code execution outside the sandbox).
- **Edit safety** — `accept()` gate, unique-match edits, `write()` no-clobber, realpath escape check, CRLF preservation, formatter integration with diff-back verification.
- **Snapshots per turn** — `git write-tree` against a private index file gives every turn a tree-hash identity + an undo line.
