# agent.rs review, Tier 2: robustness fixes

2026-10-15 · work order for a later run · base: branch `freedom` after the Tier 1 commits

A max-effort review of `src/agent.rs` found 15 problems plus about 25 smaller ones, each checked against the code. They were split into tiers:

- **Tier 1** (observed in the logs, or severe) is done on `freedom`.
- **Tier 2** is this file: latent problems with cheap fixes.
- **Tier 3** (partial fixes, or code out of proportion to a rare trigger) stays out.

None of the Tier 2 problems has fired in the owner's 437 local sessions. They are fixed for robustness, so keep each change small.

## Rules

- **Branch and commits:** work on `freedom`, or a branch from it. Make one commit per item. Do not merge or push.
- **How to run it:** as an ultracode workflow. Implement inline, then have independent verifiers try to break each item before it is committed.
- **No new tests:** the owner asked for none. Verify each fix with throwaway experiments outside the repo instead:
  - Use a scratch HOME and a local mock provider: an `http.server` answering `/v1/chat/completions` with server-sent events.
  - Use the built binary.
  - Never touch the real `~/.low` or `~/.config/low`.
- **Checks before each commit:**
  - `rustfmt --edition 2024` on the changed `.rs` files, except `src/main.rs`.
  - `cargo build -q && cargo clippy -q --all-targets -- -D warnings && cargo test -q`
- **Style:** match the surrounding code: dense Rust, few comments, short doc comments.

## Items

### 1. Validate a provider before recording it

**Where:** `switch()` and `Agent::new`.

**Problem:**
- `switch()` writes the `{switch}` record before `connect()` checks the base_url. A bad URL leaves a half-switched session that `--resume` can no longer open.
- `Agent::new` creates the session file before the same check. A failed start leaves a `[header, end]` stub, which `--resume` then picks over the real session.

**Fix:** move the base_url check out of `connect()` into a helper. Call it before `Session::create` and before `self.session.switch(...)`.

### 2. Close the redaction gaps

**Where:** `verify()`, `finish()`, `redact()`, `secret()`.

**Problems and fixes:**
- **Check command:** `verify()` appends the raw check command to the check output (`[check FAILED: <command>]`), which reaches the provider in the GATE message. Redact it.
- **Shell commands:** `finish()` redacts the run output but not `meta.cmds`, the `sh()` command lines. Redact those too.
- **Escapes:** `redact()` only starts a match where the previous byte is not `[A-Za-z0-9_-]`. A key right after an escape such as `\n`, `\t`, `\r` or `%20` is missed. Treat those escapes as boundaries.
- **Missing formats:**
  - Add the prefixes `ghs_`, `ghu_` and `ghr_` (36 characters), plus `xoxe-` and `xapp-`.
  - Match PGP blocks, whose first line contains `PRIVATE KEY` but not `PRIVATE KEY-----`.
- **False positives:**
  - Identifiers such as `oc_cluster_config_v2_name` are redacted today. For `oc_`, require an uppercase letter as well as a digit (OpenCode keys look like `oc_sk_4f9Qx2Lm...`).
  - Leave `sk-` as it is, because DeepSeek keys are lowercase hex.

### 3. Keep a timed-out check's output

**Where:** `verify()` and `grouped()`.

**Problem:** on CHECK_TIMEOUT, low drops the future that owns the output file. The model then sees only `timeout after 1800s`, with no hint of which step hung.

**Fix:** move the deadline into `grouped()`'s select loop. When it passes, kill the group, read the file as usual, and append `[timeout after Ns]`.

### 4. Ctrl-C during end-of-turn work

**Where:** `turn()`, `run()` and `unfinished()`.

**Problem:**
- `turn()` clears `self.current` before awaiting `report()`. A Ctrl-C at that point writes no report.
- After a Ctrl-C, `unfinished()` can wait on the INDEX lock held by an abandoned git process, while further Ctrl-C presses are swallowed.

**Fix:** keep `self.current` until `report()` returns, and let a second Ctrl-C in `run()` skip the report.

### 5. Leftovers from snapshot failures

Tier 1 already handles fatal `git add` errors: the private index is rebuilt once, and low's git runs with `core.safecrlf=false`. Two problems remain.

**Where:** `changes()` and `verify()`.

**Problem:**
- An untracked file that git cannot add (a nested repo without commits, an unreadable file) appears as "(new)", with an "outside" row, in every report.
- When the snapshot fails during a check, `verify()` reports `check FAILED` with the git error, and the GATE nudge asks the model to fix a check failure it cannot fix.

**Fix:**
- In `changes()`, list an untracked file as "(new)" only if its mtime is at or after the turn's start.
- In `verify()`, report a snapshot failure as low's (`[check passed, but low could not snapshot the tree: ...]`) and send no GATE nudge for it.

### 6. Small fixes

These can go in one commit or several.

- **Resume with a different `--yolo`** (`Agent::new` on `--resume`): if the stored system prompt's sandbox section doesn't match the current `--yolo`, rebuild it, as a switch to the same provider and model does.
- **Replies cut while calling a tool** (`turn()`): a reply with finish `length` and tool calls never gets the LENGTH message. Tell the model its reply was cut.
- **Repeat-block count** (`run_code()`): the message says "already ran N times" with N growing past REPEAT_BLOCK. Say REPEAT_BLOCK - 1.
- **`/cost` totals** (`cost()`): count requests lost to over-long reasoning or cut streams (the `overthought` and `interrupted` events), as `report()` does.
- **Typed model names** (`main.rs` `pick()`): a typed model name picks the first provider in name order, so `config` wins. Prefer named providers, as `holding()` does.
- **The prompt's folder listing** (`system_prompt()`): it is cut silently past 200 folders. Add `… N more`.
- **Window sizes** (`window()`): `32k` parses as 32. Read a `k` suffix as thousands.
- **`compact_at` after resume** (`overflow()`, `Agent::new`): `overflow()` records compact_at in `{compact}` before raising it to the floor, so `--resume` reloads a value below the remaining context. On load, raise compact_at to at least the current context size.
- **`compact_at = 0`**: a profile with this stops every turn before any request. Treat 0 as unset.
- **Interrupted tool entry** (`run()`): it is built in two places. Build it once.

## Out of scope (Tier 3)

- Submodule and skip-worktree contents in snapshots.
- A complete list of the files a check depends on.
- Ctrl-C forwarding through bwrap on Linux.
- Files over 16 MB.
- Paths with quotes or tabs in `changes()`.
- Process-group PID reuse in `Group`.
- Edits made through symlinks or case variants of a path.
- Naming the files a check changed.
- The turn-start compaction estimate.
- The second snapshot in `verify()`.
- An low that exits during a snapshot leaves its git process running. An immediate `--resume` removes that git's lock and races it. The index is a cache, so the worst case is a rebuild.
- Writes made from Python itself (`open()`, `subprocess`) after a passing check are not seen. Shell commands are: any `sh()` after a check makes it stale.
- A SKILL.md larger than 32 KB whose `description:` line comes after the first 32 KB is not listed.
- An AGENTS.md or skills folder linked from outside the repo is left out of the prompt, by design. Such a link could otherwise point at host-only files such as `/proc/self/environ` on Linux.
