# 31 — Whole-app review

Scope: full read of `src/` (~4,800 lines) — agent turn loop, LLM streaming, Python
sandbox worker, compaction, session persistence, UI. Verified with `cargo build`,
`cargo clippy -D warnings`, `cargo test` (13 passed), and `guard_git` parsing
checked against real `git config --show-scope -z` output
(`scope\0key\nvalue\0` — the `chunks(2)` parse at src/agent.rs:960 is correct).

**Verdict:** unusually careful code. No correctness bugs affecting editing, checks,
or API payloads. The interrupt path (src/agent.rs:230–272) distinguishes a worker
that settles vs. one that must be killed and records the difference honestly;
file writes block SIGINT mid-edit (src/worker.py:108–129) so a change is never
half-applied; the check cache is invalidated on *any* post-check edit or shell
command (src/worker.py:376–378 + src/agent.rs:1010–1015); and `messages()` keeps
tool pairing valid across compaction cuts (src/session.rs:153–189).

## Findings (ranked)

### 1. Mutating shell commands displayed as "inspections" (low, display-only)
`src/ui.rs:213–219` classifies a command as a quiet read if every `;`/`|`-separated
part *starts with* a read-ish name — `sed`, `find`, `echo`, `cat` are all in
`INSPECT`, and redirection is not parsed (`src/ui.rs:219` splits only on
`; | \n`). So `sed -i s/a/b/ f`, `find . -exec rm {} +`, `echo hi > README.md`,
and `cat a > b` are folded into the reads group at `src/ui.rs:368` and never shown
as commands in the live transcript. The worker's own accounting is unaffected
(`src/worker.py:193–196` records every cmd with its exit code), so stats and
compaction summaries stay accurate — but the transcript under-reports writes.
Fix: check for `>`/`>>` in the command, or drop `sed`/`find`/`echo` from the list.

### 2. `done(step)` silently ignores a non-integer step (low)
`src/worker.py:263` — `n = step if isinstance(step, int) else (left[0] if left else None)`.
`done(3)` works, but `done("wire the button")` (plausible given the helper is
documented as `done(step)`) silently marks the *first undone* step instead of
raising — unlike `edit()`, which rejects bad shapes with a clear `TypeError`
(src/worker.py:153–159). Out-of-range ints do raise (src/worker.py:266–267), so
only the string case mis-marks plan progress.

### 3. Latent underflow in `excerpt()` (low)
`src/compact.rs:274–280` computes `clip(s, max - 40)` on `usize`; any caller with
`max < 40` panics on debug builds and wraps on release. All current call sites pass
≥ 40 by construction (the wire budgets at src/compact.rs:78–90 keep
`budget - user.len()` ≥ `EXCHANGE/2`), so it is unreachable today — but one new
caller away from a panic in a compaction path.

### 4. Worker startup depends on `git` without a guard (low)
`src/worker.py:94` calls `subprocess.run(["git", "config", "core.hooksPath"])` at
import time; on a machine without `git` this raises `FileNotFoundError` before the
`ready` handshake, surfacing as a worker start failure. Cheap fix: wrap in
`contextlib.suppress(OSError)` like the other git calls (src/worker.py:39–43).

### 5. Design notes (not bugs)
- Ctrl-C on an empty prompt just redraws it (`src/main.rs:121`); some users expect
  an exit there.
- `block()` duplicates one center line when `keep` is odd (`keep.div_ceil(2)` +
  `keep/2` = `keep + 1`, src/compact.rs:261–262).
- `sent()` estimates tokens as bytes/4 of serialized JSON (src/agent.rs:439),
  overestimating UTF-8-heavy sessions in the `prompt_tokens` fallback
  (src/agent.rs:363). Both cosmetic.

## Notably good
- **Secret handling** (src/agent.rs:893–945): PEM blocks redacted as whole spans
  with closing-marker search, secrets require a boundary before them, redaction is
  counted and surfaced rather than silent.
- **SSE robustness** (src/llm.rs:303–440): partial-line buffering across chunks,
  index-less *and* indexed `tool_calls` accumulation, partial-close-marker holdback
  in `Inline`, truncated-but-valid JSON args treated as "eof" only when everything
  parses.
- **Sandbox depth** (src/worker.py:93–130): `.git` + `core.hooksPath` protection,
  realpath escape check, CRLF round-trip in `edit`, formatter-diff reporting, and
  repo-local module eviction (`_forget`, src/worker.py:144–150).

## Status
Review-only; no code changes made. Verification at time of writing:
`cargo build` OK, `cargo clippy -- -D warnings` OK, `cargo test` 13/13 passed.
