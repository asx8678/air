# 17 — What the model would want from air (2026-10-14)

## Question

"As an advanced model, what would you like from the CLI side: more freedom or lower entropy? List suggestions with a score from 1 to 100."

## Short answer

**Lower entropy, by a wide margin.**

- Code mode already gives the model full freedom inside the sandbox: any Python, any shell command, many edits in one run.
- On the latest build, 17 runs raised. Only 2 of them hit a restriction (the `accept()` gate). The other 15 came from the model guessing air's conventions wrong:
  - 12 imported a helper that is already built in;
  - 3 were small API or quoting slips.
- Separately, 20 requests were spent on `done()` alone.
- Freedom matters only at the sandbox's edges: Unix sockets, network, and processes that outlive a run. Each of those trades away some safety.

## How the scores work

- **Score.** Net value: the measured waste a change removes, times how sure the evidence is, minus its cost.
  - Every prompt line goes out with every request. air's 1.2k-token prompt is its main edge over Pi (findings 15), so prompt growth counts against a suggestion.
- **Data.** Taken from `~/.air/sessions`: 187 sessions and 1,489 runs.
  - "Latest build" means the `2decd7b` prompt and its A/B variants: 62 sessions, 218 runs, 273 requests.
  - The committed arm, `airPL` (prompt plus file listing), is 30 of those sessions.
- **Gap.** All 8 real sessions (272 runs) ran on the oldest build, so real-use friction on newer builds is not measured.
- **Model.** All counts come from MiMo V2.6 Pro. Rows that rest on judgment rather than counts say so.
- **Scripts.** In `wish/` in session 742fae11's scratchpad: `load.py`, `fx.py`, `friction.py`, `detail.py`, `bybuild.py`, `desire.py`.

| Per build, bench runs only | before 8k | 8k | 16k | latest |
|---|---:|---:|---:|---:|
| Sessions | 48 | 48 | 21 | 62 |
| Truncated outputs | 35 | 59 | 5 | 0 |
| Exact re-reads of an unchanged file range | 65 | 57 | 5 | 7 |
| SyntaxError on an accept()/plan() line | 7 | 3 | 1 | 0 |
| Runs that raised | 34 | 15 | 7 | 17 |

- The task mix differs between builds: earlier builds include A/B reviews of air itself, so compare loosely.
- The 8 real sessions (oldest build) had:
  - 26 truncated outputs;
  - 4 silent runs whose last line was an unprinted expression.

## Ranked suggestions

Kind: **E** = lower entropy (clearer feedback, conventions that match what the model expects); **F** = more freedom; **speed** = your wall time, not the model's work.

### 1. Make helper imports harmless — 70 (E)

- **Evidence.** 12 of the 17 latest-build runs that raised were the model importing a built-in helper.
  - **Committed arm (`airPL`).** 5 of its 6 failed runs were helper imports: `import plan` (2), `import plan, done`, `import plan_ as _`, and `import plan_ok`. Code alone would fix all 5.
  - **Other arms.** The 4 SyntaxErrors of the form `import sh_helper if False else None` all came from `airP`, the arm without the file listing. Its first run explores with `sh`; the listing seems to remove that habit.
- **Why `sh` works.** `import sh` already succeeds (21 latest-build runs) because the worker sets `sys.modules["sh"] = sh`.
- **Fix.**
  - Register every `TOOLS` entry the way `sh` is registered (2 lines).
  - Add a small import finder that maps helper-prefixed names (`plan_ok`, `plan_`) to the helpers (about 6 lines).
  - Optionally test dropping "don't import them" from the prompt, in case it primes the imports.
- **Cost.** Worker only, 0 prompt tokens.
- **Test.** Run fix and feature × 3 and count helper-import exceptions.

### 2. Stop `done()` costing a request — 65 (E)

- **Evidence.** 20 latest-build runs were `done()` alone, 7% of all requests, even though the prompt forbids it.
  - 15 of them were the last run of the turn.
  - 13 came right after a passing `check()`.
  - The wording "`if check(): done()`" already tested neutral, so the fix has to be structural.
- **Fix.** Drop `done()` from the model's API. A passing `check()` ticks the current step, and the host ticks the rest on VERIFIED.
  - The prompt loses a helper line.
  - The UI still shows the plan, but steps no longer tick one by one mid-turn. That is your call.
- **Test.** 6 tasks × 3; compare requests per session and pass rate.

### 3. Unix sockets under `<root>/tmp` — 55 (F)

- **Evidence.** A real universe session hit `Operation not permitted` on `.universe/sdk.sock`, which blocked that task outright.
- **Status.** Already written and uncommitted in `src/worker.rs`. The `SANDBOX` text in `agent.rs` must change with it, because it still says macOS blocks Unix sockets.
- `/tmp` sockets stay blocked.

### 4. Built-in line-numbered reader — 50 (E; part judgment)

- **What.** `show(path, start=1, end=None)` prints `N: line` plus `[lines a-b of N]`.
- **Evidence.**
  - The model wrote its own `show(path, a, b)` or `find(path, pattern)` in 8 sessions (1 real).
  - 166 runs rebuilt `enumerate(open(...), 1)` by hand, 16 of them on the latest build.
  - Reviews, the main real use, have to cite file:line.
  - As a model, I would want this most of all: it is the `cat -n` view my Read tool gives me.
- **Cost.** About 5 worker lines, plus one prompt line (about 25 tokens per request).
- **Test.** V2's numbered-line *example* lost as part of a bundle, so test this alone: review and qa × 3, measuring tokens and citation accuracy.

### 5. Don't run the check twice on the same tree — 40 (speed, not model-facing)

- **Evidence.**
  - Memory from 2026-10-14: 7 of 20 changing turns ran the check twice on an unchanged tree. The model's `check()` passed, then the host ran it again for the verdict.
  - The `2decd7b` prompt has the model end with `check()` in its last run, so now this likely happens on almost every changing turn (inferred, not counted).
  - It costs seconds on the bench repos, but about 200 s per turn on konstruct.
- **Why not just trust the worker.** Its result is forgeable from model code: the benchmark's model reached `edit.__globals__`.
- **Fix.** The worker's `check()` asks the host to run the check, and the host caches the verdict by tree fingerprint.
- **Cost.** About 20–30 lines across `worker.py`, `worker.rs` and `agent.rs`.

### 6. Name `git grep -n` in the explore line — 35 (E)

- **Evidence.** 46 runs (all builds) hand-wrote Python grep loops over `git ls-files`.
- **Why it helps.** `sh("git grep -n PATTERN")` already prints `path:line: text` and skips ignored files.
- **Cost.** A few prompt words.
- **Test.** A/B on reviews.

### 7. A reasoning-effort setting — 35 (both; measure first)

- **Evidence.**
  - 76% of output tokens are reasoning: median 37 per request, p90 803, max 30.7k.
  - air sends no reasoning setting.
  - Plain Pi ran with thinking `low` and passed 21/21, like air.
- **Unknown.** Whether Primalabs honours the parameter. Check that first.
- **Cost.** About 3 lines: a config field passed through to the request.

### 8. Several line ranges per run — 30 (F)

- **What.** Change the prompt's "one range per run" to "several ranges per run while the total stays under 16,000 characters".
- **Evidence.**
  - 8 latest-build runs read exactly one range; the two oldest builds had 26 and 63.
  - The 16k limit now fits several ranges.
- **Cost.** Prompt only. Needs an A/B.

### 9. `spawn(cmd)` for processes that outlive a run — 30 (F; judgment)

- **What.** Today `sh` kills background jobs when it returns. `spawn` would return a handle with its output so far, and the session's end would kill it.
- **Evidence.** Thin: 2 real runs tried `sleep` or `&`. It matters for dev servers, such as universe's Phoenix app.
- **Cost.** About 15 lines and one prompt line.

### 10. Print a bare last expression, as a REPL does — 30 (E)

- **What.** If a run ends with a bare expression such as `sh("git ls-files")` and its value is not None, print the value.
- **Evidence.** 4 of 272 real runs (1.5%) ended that way, printed nothing, and cost a request.
  - Every case is on the oldest build. Bench tasks never triggered it, and no real session has run since, so whether the newer prompt fixed it is unknown.
- **Cost.** About 5 worker lines, 0 prompt tokens.

### 11. Take `recall`, `carry` and `_last` out of the base prompt — 25 (E)

- **Evidence.**
  - Use across all 187 sessions: `_last` 1 session, `carry` 1, `recall` 4.
  - Only 2 compactions have ever happened.
  - The compaction summary already teaches `recall(n)`, and the truncation marker already names `_last`.
- **Fix.** Keep the helpers. Mention `carry` in the compaction summary instead of the base prompt.
- **Savings.** About 45 tokens per request.
- **Risk.** Your long REPL sessions are exactly where `carry` is meant to pay off, and that is unmeasured.

### 12. Network opt-in short of `--yolo` — 25 (F)

- **Evidence.** One real run failed a `rustup` download. New dependencies (`cargo add`, `pip`) are impossible in the sandbox.
- **What.** `net = true` in `air.toml` would keep the file sandbox but allow network.
- **Cost.** About 4 lines per platform in `worker.rs`.

### 13. `edit`'s ok line names the line range — 25 (E)

- **What.** `edit stats.py:12-16: ok`.
- **Evidence.**
  - Reading a file again after editing it: 25 runs on older builds, 1 on the latest.
  - Final messages cite where the change landed.
- **Cost.** About 3 lines.

### 14. `accept()` warns instead of blocking — 20 (F)

- **Evidence.** On the latest build, 6 runs were `accept()` alone and 2 runs hit the gate.
- **What.** A missing `accept()` gives a one-line reminder, and the report shows "criteria: (not stated)".
- **Trade-off.** This loses the forcing function that makes the model state criteria first. Only worth doing if an A/B shows no quality loss.

### 15. `done("step text")` works; `done()` without a plan does nothing — 20 (E)

- **Evidence.** 2 latest-build errors.
- **Cost.** About 2 lines.
- Moot if #2 drops `done()`.

### 16. `remove(path)` / `move(a, b)` — 20 (F)

- **Evidence.** One real `os.unlink`.
- **Gap.** "Files change only through edit and write" leaves no legal way to delete or rename a file, which some refactors need. Rare.
- **Cost.** About 10 lines and one prompt line.

## Closed by the data (ideas raised earlier)

| Idea | Score | Why |
|---|---:|---|
| Nearest match when `edit`'s old text is not found | 8 | 1 case in 1,489 runs |
| Hint when `'''` closes a `"""` string | 8 | 1 case on the latest build |
| Auto-strip an extra `)` in `accept()` | 5 | 7 → 3 → 1 → 0 across builds |
| Truncation marker names the omitted range | 5 | 0 truncations on the latest bench build; after a truncation the model re-read a range 75 times and used `_last` 0 times |
| Parse folded `description: >-` in skills | 3 | None of your projects has `.agents/skills` |
| Trim `check()` output when it passes | 3 | Passing output is a median of 132 characters |

## Suggested order

1. **#1** (imports). Deterministic, no prompt cost, and it would have removed 5 of the committed arm's 6 failed runs. Confirm with fix and feature × 3.
2. **#2 and #4**, each A/B-tested alone over 6 tasks × 3. Neither ships unless requests or tokens fall with pass rate held.
3. **#3** together with the `SANDBOX` text, as one decision.
