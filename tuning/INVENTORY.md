# air harness inventory (Phase 0)

2026-10-16 · branch `tune-opus` from `6e5278f` · read-only pass over all of `src/` (4,500 lines: Rust + `worker.py` + `prompt.md`).

Token counts are **exact for DeepSeek V4.1 Flash**: each comes from a `max_tokens=1` request to the OpenCode Go gateway, minus an empty baseline. Claude's tokenizer differs, so treat them as ±15% for Claude. Per the user's instruction (2026-10-16), all measurement uses `deepseek-v4.1-flash` on OpenCode Go only.

## 1. What the model receives

### 1.1 System prompt (composed once per session, frozen in the session header)

`agent::system_prompt()` = `src/prompt.md`
+ `"\nNo check command is configured, so check() runs nothing."` (only without a check)
+ `SANDBOX` + `MACOS`|`LINUX` + `PERMANENT` (not with `--yolo`)
+ `# Project instructions (AGENTS.md)` + the repo's AGENTS.md (≤ 32 KB, with a clip note past that)
+ the repo listing (`listing()`: path + size per file ≤ 8 KB; else a folder tree ≤ 10 KB; else ≤ 200 two-level folder counts).

| section | bytes | Flash tokens |
|---|---:|---:|
| opening line ("You are air … smallest correct diff, verified.") | 103 | 22 |
| `## Code mode` | 623 | 150 |
| `## Helpers` (signature block + edit/write rule) | 1,405 | 353 |
| `## Example` (two runs) | 477 | 147 |
| `## Workflow` (4 numbered steps + closing line) | 1,172 | 281 |
| `## Sandbox` (SANDBOX + MACOS + PERMANENT) | 783 | 179 |
| AGENTS.md block | header + file | 8 + file |
| repo listing | 730 for air itself (29 files) | ~200 for air; 46 for a 5-file bench repo |
| **whole static part** | **4,561** | **~1,130** |
| `tools` array (one function) | — | **263** (the gateway's chat template adds a tool preamble; the description itself is 15 words) |

First request on a bench task: median **1,563** prompt tokens (114 Flash sessions).

Verbatim static part (prompt.md + sandbox text, macOS, check configured):

````text
You are air, a coding agent working in a git repository. Aim for the smallest correct diff, verified.

## Code mode
Your one tool is `run(code)`: Python (standard library) in a persistent worker whose working directory is the repo root; variables survive between calls. The repo's own modules import too; every edit or write unloads them, so import them again after a change. Only what you print comes back, at most 32,000 characters per run; longer output keeps its first and last 16,000. Do a whole step in one script. Read files with show().

Explore with plain Python: `open()`, `pathlib`, `re`, and `sh("git ls-files")` to list files without ignored ones. Print only what you need, with paths relative to the repo root.

## Helpers
These are already defined in the worker; call them directly, don't import them.
```python
sh(cmd, timeout=120) -> str   # stdout+stderr, ANSI stripped, "[exit N]" on failure; kills the command on timeout; background jobs die when it returns; a whole run is cut at 10 minutes, so split longer work
show(path, start=1, end=None) # print lines start..end, numbered (default: to the end); stops at the output limit and says how to continue
accept(criteria)              # what done means; required before the first edit or write
edit(path, old, new)          # replace the one exact occurrence of old; raises on 0 or 2+ matches
edit(path, [(old, new), ...]) # several replacements in one file, in order: all or none, formatted once
write(path, content)          # create a new file; raises if it exists
check() -> bool               # runs the project check and prints its result
plan(steps)                   # this task's steps, shown to the user; replaces any earlier plan
done(step=None) -> str|None   # mark a step done (default: current); returns the next step
```
Repo files change only through `edit` and `write`, never through `open(..., "w")` or the shell; scratch files go in a temp dir. Both run the formatter and print one line; if the formatter changed your text, the line shows how. Use raw triple-quoted strings (`r"""..."""`) for multi-line `old`/`new` so backslashes stay exact.

## Example
```python
import re
for path in sh("git ls-files '*.rs'").splitlines():
    for n, line in enumerate(open(path), 1):
        if re.search(r"fn parse_date", line):
            print(f"{path}:{n}: {line.rstrip()}")
show("src/date.rs", 40, 100)
```
```python
accept("parse_date accepts ISO week dates such as 2026-W41-2; other inputs behave as before")
edit("src/date.rs", r"""    let parts = s.split('-');""", r"""    let parts = s.split(['-', 'W']);""")
check()
```

## Workflow
For a review or a question, answer with file:line evidence and change no files unless the user asks. For a change:
1. Explore in as few runs as you can, printing only what you need.
2. Then, in one run: accept(criteria) with concrete, checkable criteria, the first step's edits (the smallest change that meets them, in the existing style), and check().
   For anything bigger than a one-line fix, also call plan([...]) with 2-6 short steps there, and done() in the run that finishes each step.
   accept(), plan() and done() return nothing worth reading: never spend a run on them alone.
3. check() must pass after your last change; if it fails, fix and run it again.
   check() is evidence only for what it runs. If it does not exercise your change (no check configured, or it only builds or lints), also run the changed code directly (import it, run the command, replay the failing case) and print what shows each criterion holds.
4. End with a short message: what changed, why, and what showed it works. If check() cannot pass, say why in one line.

Don't repeat steps that worked, don't re-read files you already have, and don't commit: git is the undo.


## Sandbox
Your code runs in a sandbox. Writes outside the repo and temp dirs fail, as do writes into any .git directory and reads of credentials and private data in the home directory (ssh and cloud keys, shell history, browser data, other tools' configs). Only a fixed set of environment variables is passed through (env = ["NAME"] in ~/.config/air/config.toml adds more). Internet and DNS are off (localhost works); nested sandbox-exec fails, and Unix sockets work only under the repo's tmp/ directory. These failures (Operation not permitted, EPERM, Read-only file system, DNS lookup errors) are permanent. Don't retry them and don't search the machine for another copy of what was blocked: skip that step, finish the rest, and say in your final message what you could not run.
````

Nothing dynamic sits in the prompt beyond what is frozen at session start. There is no date, no git status and no cwd listing that changes between requests. The listing is "at session start" and stays byte-identical for the whole session.

### 1.2 Other text air injects into the conversation

| where | text (abridged) | source |
|---|---|---|
| user role, after compaction | `[Summary of earlier work, rebuilt from the session log. Newer messages follow it.]` + sections (recent user directions, accepted criteria, unresolved failures, failing check output, status, files, earlier requests, turn verdicts, last 40 events; ≤ 32 KB) + `[Entries #1-#N are summarized here. Continue the task.]` + the current task verbatim | `compact::summary`, `session::messages` |
| user role, merged messages | `[Next user message, entry #N]` when two user messages are adjacent | `session::messages` |
| user role, end-of-turn gate | `The project check failed on the current tree. Fix what fails, run check(), then finish again. If it cannot pass, say why in one line. The check output ended with:` + last 1,500 chars (once per turn) | `agent::GATE` |
| user role, retry nudge | `Your last reply {was lost: the provider cut the stream \| was empty \| hit the output limit and was cut off; continue from where it stopped}. Send one run() now with the next step, or a shorter answer.` (≤ 2 in a row) | `agent::retry` |
| tool role | `tool result missing (interrupted)`; `[interrupted by the user]`; `error: run() arrived without a code argument, probably cut off by the output limit; send it again in smaller steps`; `error: the only tool is run(code)`; `error: <e>; the worker was restarted and its variables are gone` | `session.rs`, `agent::call/run_code` |
| tool role, from the worker | `(no output)`; traceback limited to `<run>` frames; `[applied before the error: edit a.py; inspect before repeating them]`; `[sh: air's helpers, restored before the next run]`; `[check no longer applies: files changed after it passed; run check() again]`; `[… N characters omitted]`; `[import X: air's helpers are built in; no import needed]`; show() header `[p: lines a-b of N; show('p', k) continues]`; `edit p: ok` (+ `; fmt failed: …` / `; the formatter changed it:` + diff); check output + `[check passed\|FAILED: <cmd>]` (+ `[reused: the tree is unchanged since this check passed]`); sh `[exit N]` / `timeout after Ns` | `worker.py`, `agent::verify` |
| tool-call shim | a helper name called as a tool (`edit` instead of `run`) is run as `name(**args)` inside run() | `agent::call` |

## 2. Tools

One function tool, sent identically on every request (`agent::tools()`):

```json
{"type":"function","function":{"name":"run","description":"Run Python in the persistent worker; only printed output comes back.","parameters":{"type":"object","properties":{"code":{"type":"string"}},"required":["code"]}}}
```

The rest of the surface is Python helpers defined in the worker (`src/worker.py`). The system prompt documents them; the tool schema does not.

| helper | does | output | failure text |
|---|---|---|---|
| `sh(cmd, timeout=120)` | `sh -c` in its own process group, stdin /dev/null, cwd = repo | stdout+stderr, ANSI stripped, `[exit N]`; capture capped at 8 MB head+tail | `timeout after Ns`; the whole run is cut at 570 s inside, 600 s outside |
| `show(path, start=1, end=None)` | numbered lines | `[p: lines a-b of N]` header; stops at the run's output room and prints the call that continues | Python exception (FileNotFoundError …) |
| `edit(path, old, new)` / `edit(path, [(old,new),…])` / `edit(path, [old,new])` | exact unique replacement, CRLF-aware, all-or-none, then fmt | `edit p: ok` + formatter diff if it changed the text | `ValueError: old text not found in p` / `occurs N times in p` (+ `(pair k of n; nothing was changed)`). **No nearest match is returned.** |
| `write(path, content)` | create only | `write p: ok` | `p exists; use edit` |
| `accept(criteria)` | records done-criteria; **required before the first edit/write of a task** | nothing | `call accept(criteria) before changing files` |
| `check()` | asks the host to run `check` from air.toml in the sandbox (30 min limit, 1 MB tail kept) | check output + verdict line; reused when the tree is unchanged | `no check command configured` |
| `plan(steps)` / `done(step=None)` | UI progress only | nothing / next step | `no step n` |

Edits and writes refuse anything outside the repo, in `.git`, in the hooks path, hard-linked, or on another volume. Writing repo files through the shell or `open(...,'w')` is forbidden by the prompt, not by code; the turn report flags such changes as "outside".

**Truncation.** Run output is capped at 32,000 chars (head 16k + tail 16k, with a marker). sh() is capped at 8 MB before that, check output at a 1 MB tail, and the gate nudge at a 1,500-char tail. show() pages instead of being cut. **No full-output-to-file fallback exists.** Need: 1 of 3,539 logged runs went past 32k chars (`meta.chars`).

## 3. The loop (`agent::turn`)

- **Counting.** One turn is one user task. A request is one streamed `/chat/completions` call; the report counts replies plus lost streams. Runs = tool calls executed.
- **Each iteration:** compact if `upcoming() ≥ compact_at`. Then `complete()`. Then execute every tool call **sequentially** in the one stateful worker. Any call means continue; a reply with no calls ends the turn.
- **Parallel tool calls.** They are accepted and run one after another in a single Python process. Concurrency would break the shared variables, and code mode already batches inside one run. `parallel_tool_calls` is not sent. Flash sends 2 calls in 10 of 554 replies, MiMo in 558 of 2,721.
- **End of turn.** If files changed and a check is configured, the host runs the check on the final tree. If it fails, the GATE nudge goes out once per turn; otherwise the turn ends. The report verdict is VERIFIED / UNVERIFIED / NO CHANGES / NO CHECK. The one-shot exit code is 0 when the verdict is good and the turn didn't stop short.
- **Stop conditions.** No tool calls; 200 requests; `60 × compact_at` tokens spent; 2 empty or cut-off replies in a row after nudges; the worker failing to start twice in a row; still over the context limit after one compaction; Ctrl-C.
- **Retries.**
  - HTTP: 5 attempts on 429/5xx/connect errors/first-byte timeout (90 s, then 300 s), backoff `2<<n` s capped at 60, `retry-after` honored, quota errors not retried.
  - Lost stream: restart from scratch up to 4 attempts unless the attempt ran > 150 s; then one host retry nudge (≤ 2 in a row).
  - Stream timeouts: idle 300 s, total 3,600 s.
- **Not set:** `max_tokens` (the provider default applies) and any reasoning or effort field.

## 4. Context management

- **compact_at:** the profile's `compact_at`, else 80% of a window learned from an overflow error, else 100,000. The Flash profile in `~/.config/air/config.toml` sets 200,000.
- **Trigger:** before each request, when the provider's last `prompt_tokens` plus bytes/4 of the entries added since reaches compact_at. No turn-start compaction (removed in 4cb8830). `/compact` is manual.
- **Compaction is deterministic and LLM-free** (`compact.rs`). It keeps the newest entries up to compact_at/5 tokens, cut at an assistant boundary, and puts the summary from §1.2 before them. What survives: the system prompt, the current task verbatim, criteria, files touched, the last failing check output, unresolved failures, and the last 40 events.
- **Superseded results are not elided.** Masking was removed in 9da80fa: each logged mask was followed by re-reading what it hid.
- **Caching.** No explicit breakpoints; the OpenAI-compatible providers cache prefixes automatically (DeepSeek in 64-token blocks). `x-opencode-session` gives a stable per-session id the gateway routes by. Prefix stability:
  - meta is stripped from every entry;
  - reasoning is replayed only from the newest task on, cut deterministically to 16 KB head+tail, so each entry always sends the same bytes;
  - a **new user turn drops the earlier turns' reasoning**, so the prefix changes once per turn;
  - compaction and model switches change it too, as expected.
- **Measured (current builds, Flash):** 89% of prompt tokens cached over 163 turns, with **0 mid-turn cache breaks**. All 5 Flash breaks in the logs come from one old fee5e65 session that switched from cat-jam-free, before the mask was dropped.

## 5. Thinking and effort

- air sends `temperature` (default 0.0) plus any profile keys verbatim (`settings()` passes everything except `compact_at`). It sends no `reasoning_effort` or `thinking` and no `max_tokens`.
- Reasoning streams in `reasoning_content` (or `reasoning`) and is shown live. It is stored and replayed within the current task; that is interleaved thinking, kept.
- Gateway probe (same arithmetic prompt; reasoning tokens):

  | setting | reasoning tokens |
  |---|---|
  | default (thinking on) | 107 / 107 / 133 |
  | `reasoning_effort: low` | 102 |
  | `reasoning_effort: high` | 157 |
  | `reasoning_effort: max` | 124 |
  | `thinking: {type: enabled}` | 122 |
  | `thinking: {type: disabled}` | 0 (works) |

  All of them are accepted and none errors. Effort shows no clear effect on a trivial prompt: run-to-run noise is about the same size. The gateway returns no `usage.cost`.
- Logged Flash output is 39% reasoning (MiMo: 86%).

## 6. Safety guards (unchanged by this work)

- **Sandbox.** `sandbox-exec` on macOS, `bwrap` on Linux, for both the worker and the check.
  - Network: no internet or DNS; localhost only; unix sockets only under `repo/tmp`.
  - Writes: only the repo and temp dirs, and never `.git`, the hooks path or nested `.git` dirs.
  - Reads: denied on 40 secret paths, including `.ssh .aws .azure .kube .config/gcloud .config/helm .docker .terraform.d .oci .vault-token`, shell histories and keychains, plus their symlink targets.
  - Also denied: tty ioctl and writes, AppleEvents, `osascript/open/launchctl/automator`.
- **Environment.** An allowlist, so no `KUBECONFIG`, `AWS_*`, `AZURE_*` or `ARM_*`, plus hermetic pins.
- **Redaction.** Secret redaction (AWS/GitHub/Slack/sk-/oc_ keys, PEM private keys) on run output, check output, errors and logged headers.
- **git hardening.** git filter and diff drivers are neutralized for air's own git calls.
- **`--yolo`** disables the sandbox; the bench never uses it.
- **No command filter.** air has no destructive-command filter: the sandbox plus git-as-undo carry that.
- On this machine `~/.kube`, `~/.azure`, `~/.aws` and `~/.config/gcloud` don't exist, and no cloud env vars are set.

## 7. Existing tests and evals

- `cargo test`: 16 unit tests. The check is `cargo build && cargo clippy --all-targets -D warnings && cargo test` (air.toml).
- `~/projects/air-bench`, all synthetic:
  - `bench/templates` holds 7 tasks: fix, feature, debug, refactor, qa, review, rust;
  - `check_bench.py` graders, `bench_run.py` runner (air vs pi), `trace.py`/`turns.py` log rollups;
  - `sim.py` reasoning-replay simulator, `replay/` compaction A/B, `real/flow.sh` review→fix on an air clone.
- The last results (memory, 2026-10-16) are 14/14 and 28/28 passes on Flash at a median of 4 requests per turn: **at the floor.** These tasks can't show an improvement.

## 8. Measured profile (Flash, current builds)

- 163 turns; requests per turn: mean 4.4, median 4, p90 5.
- Prompt tokens 6.2M, 89% cached.
- Final message: median 801 chars, p90 2,454.
- Error rates:
  - edit/write failures: 8 in 573 runs (5 "not found", 3 "exists");
  - accept-gate rejections: 0;
  - runs rejected before acting (SyntaxError, NameError or helper misuse): 8 in 573.
- Across all models: 95 of 4,113 runs did only `accept/plan/done`, and plan() appears in 238 runs. That's a round-trip each, almost all of them MiMo.
