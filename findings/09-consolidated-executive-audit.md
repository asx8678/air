# 09 — Consolidated Executive Audit: The Full Series Synthesis

**Audit date:** 2026-10-14 · Capstone consolidating passes 01–08.
**Certified against:** tree `91d63d4` — 1,772 Rust LOC + 265 worker.py LOC, unchanged across all five audit passes; every claim below is backed by one of 32 live probe results recorded in `findings.json`.
**Evidence base:** 62 findings (C/M/L, F, S, A, P series), 32 probe transcripts, 10 recorded strengths.

---

## 1. Executive Summary & Severity Matrix

### Architectural assessment

`air` is a small, deliberately minimal agent with an unusually strong core: a **deterministic, log-derived compaction system** (pure function of the session log, verbatim excerpts, `recall(n)` exact recovery — no LLM summarization, hence no hallucinated history), an **append-only JSONL session model** (flock, torn-line repair, model pinning, stable entry indices), and a **content-anchored edit engine** (exact-unique-match, no offsets, no file cache — structurally immune to the wrong-line-patch and stale-index corruption classes). Prompt architecture is the anti-bloat ideal (~700-token system prompt, one tool, one parameter, ~60-token schema). **Token safety: 8.5/10.**

The failure modes do not live in the core algorithms — five passes of probing could not corrupt a single edit, could not make compaction non-deterministic, and could not break the session log. They cluster at the **boundary layers** where the design meets the outside world: the worker framing protocol (F1), the sandbox↔host seam (S0, S2), the model↔shell gate (C1/S1), the agent↔user attribution seam (A4/F2), and the single-writer assumptions that break under any concurrency (P4, S3).

### Cross-pass meta-findings (visible only in synthesis)

- **Meta-1 — Boundary clustering:** every Critical/High failure sits at a process, sandbox, host, or human-attribution boundary; core logic is uniformly sound.
- **Meta-2 — Detection without prevention:** the codebase *detects* superbly (fmt-failure notes P2, rejected counters A1, check-no-longer-applies invalidation, mtime fingerprints F2) but almost never *prevents or acts* — signals are recorded, not enforced.
- **Meta-3 — Single-writer assumptions:** everything presumes one agent, one session, one machine; concurrency (two sessions, an editor, a second checkout) silently breaks attribution (A4), loses updates (P4), poisons shared caches (S3), and misresumes sessions (F5).
- **Meta-4 — Estimation without calibration:** the trigger stack guesses tokens (`len()/4`), guesses margins, and guesses overflow from error-text sniffing — while receiving exact ground truth (`usage`) every request and never feeding it back (C5/F4/F6).
- **Meta-5 — The verification spine is the attack surface:** "git is the undo" — per-turn snapshots, write-tree identities, the check gate — is the design's greatest strength and simultaneously its largest exposure (S0 host-side filter execution, F2 mtime false verdicts, A4 undo-hint footgun).

### Consolidated severity matrix (top tier; full ledger in findings.json — 62 findings)

| Priority | ID | Location | Finding | Category |
|---|---|---|---|---|
| **CRITICAL** | S0 | `agent.rs:215-223, 370-382` | Host-side git filter escape: `git add -A` / `git diff --numstat` on the untrusted repo run **outside every sandbox**; shipped `.gitattributes`+`.git/config` clean filters execute with user privileges on the first turn (probe-proven; plumbing fix verified) | Sandbox escape / host RCE |
| HIGH | C1+S1 | `worker.py:112`, `compact.rs` | `sh()` bypasses the accept() gate and the action ledger; injected build/file text is laundered into permanent context by the compaction summary (check tails, failures, carry) | Security / compaction integrity |
| HIGH | F1 | `worker.py:227-228, 234, 256` | Worker framing has no failure isolation: a torn request line kills the worker; the traceback routes to /dev/null (probe-proven exit 1, empty stderr) — interrupt ⇒ silent total state loss | Silent failure / cancellation |
| HIGH | F2 | `agent.rs:225-233, 215-223` | Change detection is mtime+len with silent git defaults — can print false "NO CHANGES" verdicts or fire spurious gates; snapshot copy failures ignored | State integrity |
| HIGH | C2/F3 | `session.rs:102`, `llm.rs:227` | Five redundant serializations/copies of the context per request cycle — ~45MB allocator churn per 100KB payload resident across 150 requests; O(context) per request | Performance / token economy |
| HIGH | C3 | `ui.rs:31-34`, `agent.rs:360-368`, `session.rs:68` | Blocking work on the `current_thread` runtime: host git subprocesses, per-file stat storms, per-second `stty` spawn, unbuffered session writes | Async safety |
| HIGH | C5+F4 | `agent.rs:91-100`, `session.rs:78-81`, `llm.rs:15-16` | Trigger fragility: one-request lag, hard abort on second overflow, blind when usage unreported, overflow sniffed from English error text; margins stack ~+20k tokens over `compact_at` | Compaction determinism |
| HIGH | S2 | `worker.rs:17-18, 47-49` | Secret deny-list gaps (`~/.git-credentials`, `.npmrc`, `.pypirc`, `.cargo/credentials`, `.terraform.d` — probe-proven readable); the LLM request itself is the exfiltration channel | Security / exfiltration |
| HIGH | A1 | `agent.rs:90-140` | No oscillation/divergence detection: repeated identical failures return byte-identical feedback (probe-proven); worst-case turn ≈ 200 × ~100k prompt tokens; no per-turn token budget | Agent loops / token economy |
| HIGH | C4 | `worker.py:97`, `worker.rs:70-90` | Platform-divergent interrupt semantics (bwrap `--new-session` vs sandbox-exec) + truncate-then-write edit path — kill mid-write leaves torn files | Resilience |
| HIGH | C7 | repo-wide | Zero tests, zero CI while the highest-value modules (compact/session/llm) are pure and trivially testable | Production readiness |
| MED | S3, S4/A5, A3, P1, P2, P3, P4, P5, P6, F5, F6, M2, M3, M5, S6, S7, S8, A4, A6, C6 | — | See the index in README.md and findings.json: cache poisoning, orphaned descendants, non-transactional edits, EOL brittleness, missing syntax gate, prompt-only constraints, no workspace lock, no background primitive, resume brick-risks, cache economics, MAX_BYTES amputation, ANSI injection, terminal restore, observability, attribution footgun, no session GC, `_last` overpromise, residual SSE buffer | — |
| **CLEARED** | F10, A7, A8, P7, P8, S9 | — | Verified under live probing: compaction determinism + request-boundary triggers + append-only anchors; no file cache ⇒ no stale-index class; content-anchored edits cannot corrupt (no offsets, no fuzzy matches); goal anchoring survives compaction; path traversal/symlink escape rejected; API key never logged; repo cannot redirect base_url; lock discipline sound; LLM-stream cancellation clean | Strengths |

---

## 2. Deep-Dive Vulnerability & Algorithmic Analysis

### 2.1 The token pipeline: O(context) by construction (C2/F3, F4)

Every LLM request pays: deep-`Value` clone of all retained messages (`session.rs:102`) → full body serialization (`llm.rs:68`) → `body.to_owned()` per retry attempt (`llm.rs:227`) — on top of the log serialization at push. A 100KB code payload resident for 150 requests generates ~450 redundant copies on a single-threaded runtime where every memcpy is latency. The trigger stack compounds the waste: `len()/4` undercounts code 15–30%, the 32KB summary rides uncounted, the trigger lags a full request, and adaptation reacts to the last *successful* size — stacking ~+20k real tokens above nominal before compaction lands, recovered only by the overflow path whose classification is English-text sniffing. Meanwhile `usage.prompt_tokens` — exact ground truth — arrives every request and is never used for calibration (F6 also drops the residual SSE line that carries it).

### 2.2 Compaction: the verified core and the fragile edges (F10, M5)

Probed and cleared: `summary()`/`cut()` are pure functions of the log (zero nondeterminism sources; BTreeMap ordering; byte-identical across replays); triggers exist only at request boundaries (never mid-stream); entries are atomic; `clip`/`tail` are char-boundary-safe; the marker is a derived checkpoint — losing it to a crash loses nothing. Fragile edges: the `MAX_BYTES` clip can amputate the `recall(n)` instruction line; user-direction excerpts are head-only (late constraints in long tasks get cut); the anchor set is only as truthful as `meta.actions` — which `sh()` falsifies (C1), making S1 a compaction-integrity bug, not just an audit gap.

### 2.3 Security boundary failures (S0, C1/S1, S2)

S0 is the series' most serious result: the sandbox wraps the worker and check — but **not** the agent's own host-side git. A working copy shipped with `.git/config`+`.gitattributes` defining `filter.*.clean` executes attacker commands unsandboxed during the per-turn snapshot (probe-proven; `update-index`/`write-tree` plumbing verified filter-free). The exfil reframe matters as much: sandbox network-deny does not prevent data theft — anything the model reads (including the probe-proven unlisted credential files) is transmitted to the provider in the next request; the sanctioned channel is the exfil channel.

### 2.4 Agent-loop pathologies (A1, A2, A4)

The turn loop has exactly two bounds — `MAX_REQUESTS=200` and the one-shot check-gate nudge — and zero memory of failure: the second identical failing edit returns the identical error and hint, byte for byte. Existing dampeners deserve credit (gate fires once, stale-check invalidation at `worker.py:254`, `rejected` pre-classification, `_hint` self-correction) but nothing escalates or budgets. A4 adds the human seam: mid-turn user edits are conflated with agent edits, run through the check gate, and would be reverted by the printed undo hint if followed.

### 2.5 Patch mechanics: sound core, brittle edges (P8/A7, P1/P2)

Two passes of probes could not make the edit engine corrupt: exact-unique-match + per-call re-reads mean stale/overlapping anchors fail loudly and the wrong-offset corruption class is structurally absent; external mid-session file edits produce loud failures or clean merges, never hallucinated patches (no cache, no line numbers). The edges: mixed-EOL multi-line edits and non-UTF-8 files fail loudly (P1); invalid syntax hits disk with at most a post-hoc, non-reverting fmt note (P2); multi-edit runs are non-transactional (A3); and concurrent sessions lose updates silently through the same read-modify-write window (P4, probe-proven).

---

## 3. Production-Grade Rust Refactoring (consolidated; full versions in 03/05–08)

**(a) Wire-prefix cache + `Bytes` retries — O(delta) requests (C2/F3):** serialize each entry once at `push()` into `Arc<str>`; build bodies by concatenating cached prefix + delta; `Body::from(bytes.clone())` makes retries refcount-priced and maximizes provider prefix-cache hits.

**(b) Self-calibrating `TokenMeter` + leading trigger (F4):** EMA of `wire_chars / reported_prompt_tokens` observed at the existing `ui::usage` hook; project the pending request before sending instead of comparing the last reported count.

**(c) Content-addressed change detection (F2):** replace `fingerprint()` with `tree_id()` (`git add -A`-equivalent via plumbing `update-index --add --remove` + `write-tree`); `bail!` loudly when git fails rather than defaulting to a verdict.

**(d) Worker framing isolation (F1):** wrap `json.loads` in try/except + skip (never die on framing); restore `sys.stderr = sys.__stderr__` after each run; forward SIGINT to the worker's process group on interrupt.

**(e) `LoopGuard` (A1/A2):** hash failed-action signatures; on 3 identical strikes append a forced strategy-change note; idle streaks (no edits, no check) trigger a conclude-or-change nudge; add a per-turn prompt-token budget.

**(f) `WorkspaceGuard` (P4):** flock `.git/air.lock` (host-writable, model-unreachable) in `Agent::new`; refuse a second instance per worktree.

**(g) Git hardening (S0):** plumbing-only host git + a startup tripwire refusing repos whose `.gitattributes`/`.git/config` define filter/diff drivers; run host git inside the same OS sandbox.

**(h) Edit hardening (P1/P2/P3):** EOL-map matching (match LF-normalized, restore per-line endings, no translation on write); per-extension parse gate before the disk commit; `AIR_PROTECTED` globs enforced in `_change`.

---

## 4. Actionable Remediation & Testing Strategy

### Prioritized roadmap (unified across all passes)

**P0 — safety and correctness (small, local diffs):** git filter escape fix (g) · framing isolation (d) · structured overflow parsing + non-retryable `finish_reason=length` (C5/M1) · `sh()` ledger + tree-diff tripwire (C1) · summary-tail guarantee (M5) · atomic `edit()` + SIGINT forwarding (C4) · secret deny-list expansion (S2) · `_last` honesty note (C6).

**P1 — hot-path performance:** wire cache + Bytes (a) · de-block the runtime — spawn_blocking/ioctl/BufWriter (C3) · TokenMeter + leading trigger (b) · tree_id change detection (c).

**P2 — agentics and economy:** LoopGuard + token budget (e) · syntax gate (P2) · EOL-flexible edit (P1) · WorkspaceGuard (P4) · protected globs (P3) · jittered backoff + timestamps + tracing (S8) · session GC (A6) · orphan registry (A5) · per-session cache namespace (S3) · background spawn/logs/stop with ring buffer (P5) · ANSI-safe thinking + panic hook (S6/S7) · AGENTS.md cap (M7).

**P3 — productionization:** the full test suite + CI (C7) · document the security model (sandbox-is-the-boundary, yolo semantics, platform asymmetries).

### Testing strategy (consolidated master list)

1. **Torn-write property:** truncate a session file at every byte offset — resume must never panic, must drop at most the torn line.
2. **Compaction invariants:** `cut()` never splits assistant↔tool pairs; `summary()` byte-determinism across processes; `messages()` protocol validator (every `tool_call_id` resolves, in order).
3. **Worker framing fuzz:** random/torn/garbage lines — the worker must never exit before EOF (F1 regression).
4. **Malicious-repo fixture:** filter/textconv/hooks definitions — zero marker executions, tripwire fires (S0 regression).
5. **Sandbox conformance matrix:** enumerated read/write/exec/network probes under both profiles, including the canonicalization invariant (non-canonical subpaths fail loudly in CI).
6. **EOL/encoding fuzz:** randomized per-line ending mixtures × edit regions — never corrupts, all failures loud, endings round-trip (P1).
7. **Multi-edit composition property:** overlapping/stale anchors raise; composition equals the serialized reference; disk never diverges from the ledger (P8/A3).
8. **Concurrency:** staggered multi-writer races — the lock refuses instance 2; lost-update captured as the pre-fix baseline (P4).
9. **Oscillation/stagnation harness:** scripted mock LLM repeating a failing edit — breaker fires by strike 3; idle runs intervened at 4 (A1/A2).
10. **Stream-chaos proxy:** mid-stream drops, garbage, missing `[DONE]`, newline-less finals, 429/5xx storms — bounded retries, residual drain, correct overflow classification, jitter bounds (C5/F6).
11. **Long-session soak:** 10k-entry synthetic log — latency, determinism, RSS curve pinned (F8).
12. **Token-estimator calibration fixtures:** recorded `(wire_chars, prompt_tokens)` pairs — ±15% on held-out entries (F4).
13. **Exfil canary:** planted tokens in unlisted secret paths never appear in outbound request bodies (S2).
14. **ANSI fuzz through every UI sink** — no raw escapes except air's own paint codes (S6); **terminal crash suite** — SIGKILL mid-turn, healed on next start (S7).
15. **Audit completeness:** every entry timestamped; API-key value grep across all artifacts — zero hits (S8).

---

## Bottom line

Five passes, 32 probes, 62 findings — and the verdict is consistent: **the core is exemplary; the boundaries are not.** Deterministic compaction, append-only sessions, content-anchored edits, and a 60-token tool schema put this codebase ahead of most agents. Every Critical/High defect sits where the design meets an outside force — the host's git, the process's children, the provider's dialect, the user's editor, a second instance, a malicious repo. All of them close with small, local, already-sketched fixes; none require re-architecting. The single highest-leverage act is writing the test suite (C7) — the modules that most deserve it are pure, small, and currently completely unguarded.
