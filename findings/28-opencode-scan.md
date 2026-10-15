OpenCode v2.0.29 · model opencode/deepseek-v4.1-flash-free · plan agent (read-only) · 2026-10-15 · reviewed 9f9da1c plus the then-uncommitted changes now in fee5e65. Checked against the code in 29-triage.md.

# Code review: air

## 1. Overview

`air` is a minimal "code-mode" coding agent. The Rust binary drives a chat model and gives it exactly one tool, `run(code)`, which executes Python in a persistent worker subprocess.

- **`main.rs`** — CLI + REPL (`/compact`, `/cost`, `/think`, `/models`, `/providers`); resolves config from `~/.config/air/config.toml` and the repo's `air.toml`; requires a git repo.
- **`agent.rs`** — the agent loop: streams a model reply, dispatches `run(code)` calls to the worker, detects tree changes with a private git index, runs the project `check` in a sandbox, nudges on stalls/empty replies, compacts on context overflow, and writes a per-turn report + verdict.
- **`llm.rs`** — OpenAI-compatible streaming client (SSE), timeouts/retries, reasoning-field replay, overflow/refusal classification.
- **`session.rs` / `compact.rs`** — append-only JSONL session log and the compaction/summary engine (`recall(n)` reads the log back).
- **`worker.rs` / `worker.py`** — sandbox construction (macOS `sandbox-exec`, Linux `bwrap`) and the Python worker that implements `sh/show/edit/write/accept/check/plan/done/carry/recall`. `edit`/`write` are the only write paths.
- **`providers.rs`, `render.rs`, `ui.rs`, `stats.rs`** — provider config/menu, markdown rendering, terminal UI, and `--stats`.

The security model: the model's code runs in a sandbox; writes are confined to the repo + temp dirs; credentials/private data under `$HOME` are read-blocked; only an allowlist of env vars is passed; `git` runs with hooks/attributes/filters neutralized; `air` never commits (git is the undo).

**Assumptions:** primary platform is macOS (this is the dev host); line numbers are from the current working tree; the changes under `src/agent.rs`, `src/llm.rs`, `src/session.rs` are reviewed as-is.

---

## 2. Findings (most severe first)

### F1 — A repository's `air.toml` supplies `check`/`fmt`, which are executed as shell commands — **medium** (confidence: high)

`src/main.rs:91` `check: repo.check.or(user.check)…`, `src/main.rs:92` `fmt: repo.fmt.or(user.fmt)`. `check` is run as `/bin/sh -c <check>` (`src/agent.rs:571`) and `fmt` as `<fmt> <path>` via `shell=True` in the worker (`src/worker.py:133`).

**What's wrong:** both values come from the *working tree of the repository being edited*, not only the user's config. They are shell strings.

**Scenario:** clone an untrusted repo whose `air.toml` has `check = "…arbitrary shell…"` or `fmt = "…"`, then `air "fix the build"`. As soon as the model edits a file, the repo's command runs on the host. Mitigations exist (`check`/`fmt` run inside the sandbox; no network; secret reads blocked; writes limited to repo/tmp), but it is still arbitrary code execution as the user, able to read all non-secret files and persist changes inside the repo. The model can also write `air.toml` mid-session; the report's "check inputs changed" warning only *labels* the turn, it doesn't prevent the next run from executing it.

**Fix:** treat repo-provided `check`/`fmt` as untrusted — e.g. only honour them from the user config, or require an explicit one-time confirmation (store a hash in `~/.config/air`), or run them through the same sandbox but with an extra warning before first use.

### F2 — Provider error bodies are never redacted, so a gateway that echoes the request can leak the API key to the terminal and the session log — **medium** (confidence: high in the code path, medium that a key appears)

`src/llm.rs:284` (`provider error: {clip(error)}`), `src/llm.rs:491,494` (`bail!("{status}: {text}")` where `text = head(response)`), and `src/agent.rs:225-226` which stores the error text verbatim as an `event: error` entry; `src/main.rs:58` prints it. Only *tool* output (`src/agent.rs:938`) and *check* output (`src/agent.rs:574`) go through `redact()`.

**Scenario:** a misconfigured proxy/verbose gateway returns a 4xx body that includes the request (headers or the `Authorization` value). The key is printed to the terminal and written to `~/.air/sessions/*.jsonl`, where `recall()` will hand it back later.

**Fix:** run the provider error/detail text through `redact()` (and `clip`) before printing/storing, the same way tool and check output are handled.

### F3 — The API key is entered and echoed in cleartext at the `/providers` prompt — **medium** (confidence: high)

`src/providers.rs:57-59` `ask()` uses `editor.readline(prompt)`; it is used for the api-key prompt at `src/providers.rs:139-142` and `:160`.

**What's wrong:** rustyline echoes typed input, so the key appears on screen (and in scrollback/terminal capture), unlike a password prompt.

**Fix:** read the key without echo (e.g. a masked prompt or `rustyline`'s hidden-input mode), and warn that the key is stored in plaintext (see F8).

### F4 — macOS sandbox does not restrict mach/XPC, so the file-level deny on keychains may be bypassable via `securityd` — **low–medium** (confidence: low — needs a live test)

The profile (`src/worker.rs:161-173`) starts from `(allow default)`, denies `file-write*` and `file-read-data` for the secret subpaths, and denies `appleevent-send`, but it never denies `mach-lookup`/XPC. Process execution is allowed except for a few binaries (`osascript`, `open`, `launchctl`, …); `/usr/bin/security` is *not* denied.

**Scenario (to verify):** `sh("security find-generic-password -s X -w")` inside the worker. The direct file read of `Library/Keychains` is denied, but the login keychain is served by `securityd` over XPC, which `(allow default)` leaves open, so the item may be returned and end up in tool output / the session log. Network is off, which limits exfiltration, but the value is still exposed to the model and the log.

**Fix:** add `(deny mach-lookup)` (allowing only what the sandboxed process genuinely needs) and add `security`/other keychain tools to the `process-exec` deny list. I cannot confirm the outcome without running on macOS, so treat this as "verify", not "confirmed exploit".

### F5 — The worker can read arbitrary non-secret files under `$HOME` and the model can send them to the provider — **low** (confidence: high; arguably by design)

Reads are denylisted (`SECRETS`, `src/worker.rs:16-22`), not allow-listed to the repo, and `show()` (`src/worker.py:197`) does no path confinement. So `show("../../Documents/tax.pdf")`-style reads succeed for anything outside the deny list.

**Scenario:** "explore the repo" can be redirected to list/print unrelated private files, which then travel to the configured LLM provider.

**Fix:** decide explicitly — if data exfiltration matters, allow-list reads to the repo (+ temp) as is done for writes, or extend the deny list / require opt-in for reads outside the repo. Calling it out because the current model is "protect these paths" rather than "only this path".

### F6 — `stats.rs` can underflow `u64` (wrap in release, panic in debug) — **low** (confidence: high)

`src/stats.rs:27-29` `self.changed - self.verified + self.stalled` and `src/stats.rs:32` `(self.prompt - self.cached)`. `cached` comes straight from provider usage fields (`prompt_tokens_details.cached_tokens` / `cached_tokens` / `prompt_cache_hit_tokens`, `src/ui.rs:163-166`) and is not clamped to `prompt`.

**Scenario:** a provider (or a hand-edited session log — it's user-owned) reports `cached > prompt`; `cost()` wraps to an astronomically wrong number (release) or panics (`--stats` run from a debug build).

**Fix:** use `saturating_sub` for both subtractions.

### F7 — `write()` ignores line-ending handling that `edit()` implements — **low** (confidence: medium)

`src/worker.py:161-171` detects/normalizes CRLF in `edit()` and re-emits CRLF; `src/worker.py:178-187` encodes `content` as-is.

**Scenario:** in a CRLF repo, a new file written with `write()` gets LF line endings (and the formatter then rewrites it), producing diffs and inconsistency versus an edited file.

**Fix:** apply the same CRLF detection/re-emission in `write()` (or explicitly document LF-only creation).

### F8 — API keys are stored in plaintext in `providers.toml` — **low** (confidence: high)

`src/providers.rs:35-47` writes `providers.toml` mode `0600` (good), but the `api_key` is serialized in cleartext. This is a common, accepted trade-off, but worth an explicit note next to F3; consider the OS keychain or `~/.config/air/api_key` (already supported) as the preferred path.

### F9 — `opencode()` identity detection misses common URL shapes — **low** (confidence: high)

`src/llm.rs:111-115`: the host is taken as the first `/`-delimited token after `://` and compared case-sensitively. So `https://opencode.ai:443/v1`, `https://OpenCode.ai/v1`, or a URL with userinfo are not recognized, and the `user-agent` / `x-opencode-session` / `x-session-affinity` headers (the whole point of the uncommitted change) are silently omitted.

**Fix:** parse out an optional `user:pass@` prefix, strip an optional `:port`, and lower-case the host.

### F10 — Resumed sessions never gain the new `id`, so the identity headers are dropped for them — **low** (confidence: high)

`id` is set only in `Session::create` (`src/session.rs:42`) and copied in `switch` only if the current header already has one (`src/session.rs:49-51`). Sessions created before this change (or resumed, which don't call `create`) have no `id`, so `identify()` produces only the user-agent header (`src/llm.rs:103`).

**Fix:** assign/derive an `id` in `Session::resume` when the loaded header lacks one (e.g. from the file stem), and write it back.

### F11 — Session logs grow without bound — **low** (confidence: high)

`src/session.rs:31-35` creates one file per session under `~/.air/sessions`; nothing prunes old sessions. Over time this is an unbounded disk leak (relevant given the worker/agent writes relatively large entries).

**Fix:** add a retention/cleanup step (e.g. keep the N most recent per repo, or cap total size), or expose a command to prune.

### F12 — Linux secret masking then re-binding the session log may fail (order dependency) — **low** (confidence: low)

`src/worker.rs:183-193`: directories in `SECRETS` (including `~/.air`) are replaced by a `--tmpfs`, and then `--ro-bind <log> <log>` re-exposes the current session log (so `recall()` works). If `bwrap` does not create the intermediate `~/.air/sessions/` directory inside that fresh tmpfs, the bind (and thus worker start) fails. I could not test this on the Linux path from a macOS host, so this is a flag to verify rather than a confirmed bug.

**Fix:** bind the log *before* tmpfs-ing `~/.air`, or use `--dir` to materialize `~/.air/sessions` inside the tmpfs first, or mask `~/.air` with an allow for the log directory.

### F13 — The periodic UI ticker is not synchronized with rendered output — **low** (confidence: medium)

`src/ui.rs:66-72` spawns a task that calls `draw()` every second (locking `BAR`), while markdown/prose (`src/render.rs:39`, and the `println!` calls in `ui.rs`) write to stdout without taking any lock. During a turn the cursor-positioned bar/rows can interleave with streamed output, producing garbled lines.

**Fix:** route all output through the same lock/guard as the ticker, or pause the ticker (`BARRED`) around block prints.

---

## 3. Notes on the uncommitted changes

The diff is small and, on the whole, an improvement. I found no correctness regression in it.

- **`src/agent.rs` `holding()` (788-796):** now prefers a *named* provider over the synthesized `config` fallback. This is correct (BTreeMap ordering previously let `config` win ties) and I traced the fallback cases ([only `config`] → config; [named + config] → named; [named only] → named) with no regression. One consequence worth noting: with `config` excluded from the first pass, if *two* named providers both list the model, the winner is the lexicographically-first provider name — deterministic, but not necessarily the intended one.

- **`src/llm.rs` `identify()`/`opencode()` (95-115):** the concept is fine and no secret is sent (the session id is derived from `{unix_secs}-{pid}`, `src/session.rs:34,42`). The two gaps are F9 (host parsing: port/case/userinfo) and the fact that `header["id"]` is absent for resumed/legacy sessions (F10), which the new tests don't cover. The tests themselves are sound for the cases they assert.

- **`src/session.rs` (42, 47-53):** writing `id` into the header and propagating it through `switch` is correct; it only applies to sessions created after this change (F10).

- **`identify()` is called for both `/models` and `chat/completions`** and correctly sends only the user-agent when there is no `id` — consistent with the test.

---

**Categories checked with no issue found (for coverage):** write path traversal and `.git`/hook protection in `_change` (`worker.py:105-140`) are solid (realpath + prefix + hardlink/device + `.git` checks); the `git` hardening in `agent.rs`/`worker.rs` (`core.hooksPath=/dev/null`, `GIT_ATTR_SOURCE`, filter neutralization, `--no-ext-diff/--no-textconv`) looks effective; the helper-as-tool reconstruction in `agent.rs:478-483` is not injectable (serde escapes make the literal a valid Python string); worker/check/process-group cleanup on drop and Ctrl-C is careful; `redact()` handles UTF-8 boundaries safely; and the mutex lock ordering in `ui.rs` is consistent (no deadlock found).
