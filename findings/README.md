# Air — Deep Code Review Findings

**Reviewed:** 2026-10-14 · **Repo:** `/Users/adam2/projects/air` · **Crate:** `air` v0.1.0 (edition 2024, rust-toolchain 1.99)

Full-source review of a Rust CLI coding agent against production readiness, architectural soundness, token economy, and deterministic state management.

**Scope:** all 7 Rust modules (1,772 LOC), `worker.py` (265 LOC), `prompt.md` — every line read.
**Verification:** `cargo check` clean · `cargo clippy` silent · tests: **none** · CI: **none**.

## Scores

| Dimension | Verdict |
|---|---|
| Token safety | **8.5 / 10** |
| Production readiness | Strong for single-user CLI; not hardened for hostile providers or shared use |
| Deterministic compaction | Best-in-class design (log-derived, no LLM summarization, `recall(n)` recovery) |
| Async hygiene | Weak — blocking work on a `current_thread` runtime |
| Code-mode enforcement | Gated helpers, but `sh()` bypass is a real hole |
| Test coverage | Zero |

## Documents

- [01-executive-summary.md](01-executive-summary.md) — architecture assessment, scores, strengths, bottlenecks
- [02-critical-vulnerabilities.md](02-critical-vulnerabilities.md) — C1–C7 with file/line references
- [03-module-recommendations.md](03-module-recommendations.md) — line-by-line refactor suggestions per module
- [04-remediation-plan.md](04-remediation-plan.md) — prioritized P0–P3 plan + keep-list
- [05-forensic-audit.md](05-forensic-audit.md) — second forensic pass: cancellation safety, silent corruption, token drift, cleared vectors (F1–F10)
- [06-adversarial-security-review.md](06-adversarial-security-review.md) — third pass: sandbox escapes, prompt injection, exfil channels, ANSI/terminal safety (S0–S9)
- [07-agent-loop-stability-audit.md](07-agent-loop-stability-audit.md) — fourth pass: loop pathologies, edit atomicity, orphaned processes, session growth (A1–A10)

- [08-patch-mechanics-execution-audit.md](08-patch-mechanics-execution-audit.md) — fifth pass: patch mechanics, EOL/encoding edges, syntax gates, concurrency (P1–P8)
- [09-consolidated-executive-audit.md](09-consolidated-executive-audit.md) — capstone synthesis: cross-pass severity matrix, meta-findings, unified remediation roadmap
- [10-six-domain-forensic-audit.md](10-six-domain-forensic-audit.md) — sixth pass: six-domain principal audit (Rust perf, token/compaction, security, loops, patch mechanics, permissions) (10-K/H/M/L) · [json](10-findings.json)
- [11-principal-forensic-audit.md](11-principal-forensic-audit.md) — seventh pass: six-domain principal forensic audit with severity matrix, refactoring snippets, and remediation/testing strategy (11-A/B/C/D)
- [findings.json](findings.json) — machine-readable records (62 findings + strengths + probe evidence)

## Finding index

| ID | Severity | Title |
|---|---|---|
| C1 | critical | `sh()` bypasses the write gate and the action ledger |
| C2 | high | O(session) clone + re-serialization of the full context on every request |
| C3 | high | Blocking subprocess/file I/O inside the async executor |
| C4 | high | Platform-divergent interrupt semantics + non-atomic `edit()` writes |
| C5 | high | Compaction trigger: one-request lag, hard abort on 2nd overflow, silent when usage unreported, error-text sniffing |
| C6 | medium | `_last` recovery contract is false after worker restarts |
| C7 | high | Zero tests, zero CI |
| M1 | medium | Overflow classification needs structured parsing; deterministic truncation retried wastefully |
| M2 | medium | Prompt-cache economics measured but not engineered; `reasoning_content` interop risk |
| M3 | medium | `Session::resume` fragility and memory doubling |
| M4 | low | `ui::thinking` renders unthrottled per reasoning delta |
| M5 | medium | `block()` quadratic fallback; `MAX_BYTES` clip can amputate the recall contract |
| M6 | medium | Request body missing `parallel_tool_calls:false`, `max_tokens`, `seed` |
| M7 | medium | `AGENTS.md` injected uncapped into every request |
| L1 | low | SSE line scanning is O(n²); UTF-8 validated twice |
| L2 | low | `.expect()` panic in `Llm::new` |
| L3 | low | `state()` O(n) per `run_code` → O(n²) per turn |
| L4 | low | UI static-mutex poisoning cascades |
| L5 | low | Streaming-render allocations (`printable` per chunk, `visible` per cell) |
| L6 | low | `recall()` rescans the log from line 0 every call |
| L7 | low | `tools()` Value rebuilt per request |
| L8 | low | `Session::push` unbuffered, no fsync (accepted tradeoff — document) |
| L9 | low | `sandbox-exec` deprecation (macOS) + sandbox network-deny friction for `check` |
| L10 | low | `pending` holds only the last call id — parallel-call outcomes misattributed on interrupt |
| F1 | high | Worker stdin framing: torn/malformed line kills the worker silently (traceback → /dev/null) |
| F2 | high | mtime+len change detection + silent git failures → false "NO CHANGES" verdicts / spurious gates |
| F3 | high | Five-copy serialization pipeline; ~45MB churn per 100KB payload resident across 150 requests |
| F4 | med-high | Token-estimation drift: `len()/4` undercounts code, summary uncounted, one-request lag (~+20k true tokens) |
| F5 | med | Resume bricks on one unreadable session file; lexicographic sort picks wrong session; config errors swallowed |
| F6 | med | Residual SSE buffer discarded → silent usage loss; no-`[DONE]` providers retried 3× then fail |
| F7 | med | No per-turn token budget; no repeated-failure circuit breaker (worst case 200 × ~100k tokens) |
| F8 | low-med | Full session log resident in RAM forever; uncapped tool-call args and reasoning compound growth |
| F9 | low | flock fails on lockless FS; torn-line repair covers only the last line; no fsync (undocumented) |
| F10 | cleared | Compaction determinism, boundary splitting, lock discipline, cancellation, append-only stability — verified safe |
| S0 | critical | Host-side git filter escape: shipped .gitattributes + .git/config execute clean filters unsandboxed via snapshot()/changes() |
| S1 | high | Indirect prompt injection, no defense-in-depth; compaction launders attacker text into permanent context |
| S2 | high | Secret deny-list gaps (~/.git-credentials, ~/.npmrc, ~/.pypirc, ~/.cargo); the LLM request is the exfil channel |
| S3 | med-high | Shared cross-project air-cache poisoning (network-denied builds must consume it) |
| S4 | medium | Daemonized grandchildren escape killpg — time-bomb processes on macOS (Linux PID-ns safe) |
| S5 | medium | Disk-fill DoS: no RLIMIT_FSIZE on writable dirs |
| S6 | medium | ANSI injection via ui::thinking — the only unfiltered output sink (OSC-52 clipboard writes) |
| S7 | low-med | No panic hook / terminal-restore guarantee on crash or SIGKILL |
| S8 | low-med | No jittered backoff, no tracing, no timestamps in the audit log |
| S9 | cleared | Path traversal, tool args, output poisoning, concurrency, key handling — cleared under live probing |
| A1 | high | No oscillation detection — identical failures return identical signals; 200-request/one-nudge are the only bounds |
| A2 | medium | No stagnation detector for no-op exploratory runs |
| A3 | medium | Multi-edit runs are not transactional — probe-proven partial application on mid-batch failure |
| A4 | medium | External mid-turn edits conflated with agent changes; undo hint can revert the user's own work |
| A5 | medium | Worker teardown orphans setsid'd sh descendants (probe-proven survival past SIGKILL; macOS) |
| A6 | low-med | No session GC — 20 files/1.1MB already accumulated on this machine; logs hold code+outputs forever |
| A7 | cleared | No file cache by design — content-anchored edits proven stale-safe against external modification |
| A8 | cleared | Goal anchoring structurally sound: task pinned verbatim, criteria/carry/plan survive compaction |
| A9 | info | Rehydration near-perfect (log-as-truth); worker globals/_last ephemeral by design |
| A10 | low-med | Cost traps: transport retries re-send full prompts; reasoning + uncapped args ride every request |
| P1 | medium | Mixed-EOL multi-line edits and non-UTF-8 files fail loudly; CRLF write-back would double explicit CRLF in `new` |
| P2 | medium | No syntax pre-flight — invalid syntax hits disk; fmt gate is post-hoc and non-reverting (probe-proven) |
| P3 | medium | Negative constraints are prompt-only — no configurable protected-path interceptor in `_change` |
| P4 | medium | No workspace lock — concurrent sessions silently lose updates (probe-proven lost-update race) |
| P5 | low-med | No background-task primitive — synchronous sh() capture; server workflows orphan and lose logs |
| P6 | low | Output truncation is a blind char-slice — no line-aligned seams, no structural windowing |
| P7 | cleared | No index to invalidate; retrieval prompt-delegated; prompt ordering sound; tool schema minimal |
| P8 | cleared | Content-anchored edits have no offset-invalidation class — stale/overlapping edits fail loudly |

## Round 10 — Six-Domain Principal Forensic Audit

Full report: [10-six-domain-forensic-audit.md](10-six-domain-forensic-audit.md) · machine-readable: [10-findings.json](10-findings.json)

26 findings: 3 critical, 6 high, 14 medium, 3 low. Independent re-verification; IDs are round-scoped (`10-` prefix).

| ID | Severity | Title |
|----|----------|-------|
| 10-K1 | critical | No permission model: `accept()` is model self-approval; no read-only mode; `--yolo` unguarded |
| 10-K2 | critical | `edit()` truncates then writes — kill/crash/disk-full destroys the target file |
| 10-K3 | critical | Stream-finish heuristic accepts truncated SSE responses as complete; partial tool-call plans executed |
| 10-H1 | high | `messages()` strips `reasoning_content` at turn boundaries — prompt cache invalidated every turn |
| 10-H2 | high | Sandbox env deny-list leaks `SSH_AUTH_SOCK`/`DATABASE_URL`/`DSN`; `/tmp` writable → SSH-agent hijack |
| 10-H3 | high | 3 s `settle()` vs 30-min checks; torn stdin lines corrupt worker protocol; stale-reply misalignment |
| 10-H4 | high | Orphaned subprocesses: `kill_on_drop` kills only the direct child; `_exec` groups survive |
| 10-H5 | high | Nested/vendored `.git` writable → hook planting executing later outside the sandbox |
| 10-H6 | high | Zero tests, zero CI |
| 10-M1 | med | Usage-or-nothing token accounting; `len()/4` estimator; fixed `COMPACT_AT` |
| 10-M2 | med | Consecutive overflow hard-aborts the turn; "nothing to compact" stall |
| 10-M3 | med | Blocking `stty` fork/exec + `std::fs`/`std::process` inside async turn |
| 10-M4 | med | O(session) clone + re-serialization per request; unbounded `entries` |
| 10-M5 | med | `MAX_REQUESTS=200`; one nudge; no oscillation detection |
| 10-M6 | med | Backoff without jitter; overflow by substring sniffing |
| 10-M7 | med | No pre-flight syntax gate; mixed-EOL breaks `old` matching |
| 10-M8 | med | `AGENTS.md` + skills injected uncapped; system prompt frozen at creation |
| 10-M9 | med | `reasoning_content` echoed back to provider |
| 10-M10 | med | No panic hook / terminal restore guarantee |
| 10-M11 | med | Repo-controlled `check`/`fmt` execute as user; localhost net allowed |
| 10-M12 | med | Request omits `max_tokens`, `seed`, `parallel_tool_calls:false` |
| 10-M13 | med | `load()` swallows config errors; `api_key` perms unchecked |
| 10-M14 | med | Multi-file edit batches non-transactional |
| 10-L1 | low | SSE scan O(n²), uncapped buffer, multi-line `data:` dropped silently |
| 10-L2 | low | Latent `max-40` underflow; 17 `unwrap` / 5 `expect` lock-poison panics |
| 10-L3 | low | `fingerprint()` hashes mtime; `compact::tail` cuts mid-line |

## Round 11 — Seventh-Pass Principal Forensic Audit

Full report: [11-principal-forensic-audit.md](11-principal-forensic-audit.md)

22 findings: 2 critical, 6 high, 8 medium, 5 low. Independent line-by-line re-verification of all 9 source files; `cargo check` + `cargo clippy` clean. IDs are round-scoped (`11-` prefix, also cited as A/B/C/D in the report).

| ID | Severity | Title |
|----|----------|-------|
| 11-A1 | critical | `sh()` bypasses the `accept()` write gate and the actions ledger — mutations invisible to compaction's Files section |
| 11-A2 | critical | Three orphan paths: `setsid` sh-children survive worker SIGKILL; ctrl-c during `verify()` orphans check trees; `std::process::exit` in task mode skips `Drop` and leaks the worker on *clean* exit (**new**) |
| 11-B1 | high | O(session) `entry.clone()` + full-body serialization per request; `body.to_owned()` reallocates the prompt up to 20× per turn |
| 11-B2 | high | Blocking `std::process::Command`/`std::fs` (`git()`, `fingerprint()`, `stty`) on the `current_thread` runtime — stalls ticker, delays ctrl-c |
| 11-B3 | high | `calls.resize(index + 1)` with wire-controlled index — provider payload `"index": 4e9` is a memory-exhaustion DoS (**new**) |
| 11-B4 | high | Trigger accounting = `to_string().len() / 4`, re-serialized per entry per compaction attempt; no tokenizer |
| 11-B5 | high | `_exec` captures to an unbounded TemporaryFile, then reads it fully into RAM — chatty builds fill disk / OOM the worker |
| 11-B6 | high | No pre-flight syntax validation; fmt failure leaves ledger entry `ok: True`; LF injected verbatim into CRLF files |
| 11-C1 | medium | Overflow classification by error-text sniffing; structured `error.code` ignored |
| 11-C2 | medium | Compaction trigger one request late; silently disabled when provider omits usage |
| 11-C3 | medium | No backoff jitter; `Retry-After` integer-only; clean EOF without `finish_reason` accepted as complete → silent truncation |
| 11-C4 | medium | Repo-local secrets (`.env`, `*.pem`) readable and shipped to the provider; `SECRETS` deny-list gaps; `sandbox-exec` not a security boundary |
| 11-C5 | medium | No untrusted-content delimiting for READMEs/git logs/tool output; `AGENTS.md` embedded wholesale |
| 11-C6 | medium | `resume()` doubles memory (String + parsed entries); entries never evicted |
| 11-C7 | medium | Only bound is `MAX_REQUESTS = 200`; no repeated-call or worker-restart circuit breaker |
| 11-C8 | medium | No panic hook / SIGTERM handler — scroll region left set; UI mutex poisoning cascades |
| 11-D1 | low | SSE buffer front-drain O(n²) |
| 11-D2 | low | `stty size` subprocess fork per second |
| 11-D3 | low | `block()` greedy refit loop O(n²) worst case |
| 11-D4 | low | Latent `max - 40` underflow in `excerpt` (currently unreachable) |
| 11-D5 | low | `tools()` Value rebuilt per request; stdout flushed per SSE event |

**New in this round** (not in rounds 1-10): 11-A2's `std::process::exit` clean-exit worker leak (`main.rs:67`), and 11-B3's wire-controlled `calls.resize` DoS (`llm.rs:157-164`). The report also credits verified strengths: deterministic log-derived compaction with `recall(n)` recovery, loop-top-only compaction triggers, path-traversal guards in `_change`, and the `.git`/hooks write-protection that structurally prevents hook persistence.

## Round 12 — probe-verified delta ([12-probe-verified-delta-audit.md](12-probe-verified-delta-audit.md), [12-findings.json](12-findings.json))

| ID | Severity | Status | Title |
|----|----------|--------|-------|
| 12-N1 | **critical** | new | macOS sandbox never denied network: `(local ip "localhost:*")` allows TCP+UDP egress to any IP (proven through the real binary); voids the "network-denied" premise of S2/S3/L9 |
| 12-N2 | medium | new | Turn-start compaction can cut the answer the user is replying to down to 160 chars |
| 12-N3 | low-med | new | Summary + kept user entry → consecutive `user` messages |
| 12-N4 | low-med | new | `ui::note` / top-level `eprintln!` print model/provider bytes raw (corrects S6) |
| 12-N5 | low | new | 3.9 `match` hint attached to every `SyntaxError` |
| 12-C1 | low | refined | 10-H1 cache cost bounded to the previous turn (~1.1k tok measured); its strip-at-push fix would regress the reasoning echo |
| 12-C2 | — | refuted | 10-H2 SSH-agent via `/tmp`: unix connect denied on macOS (Linux untested) |
| 12-C3 | — | refuted | 11-A2 clean-exit worker leak: worker exits on stdin EOF |
| 12-C4 | — | known tradeoff | 10-K3 / 11-C3 unmarked stream end is deliberate (`b2b59d7`); residual: text-only clean close accepted |
