# 18 — Wishlist picks: what was taken, tested and dropped (2026-10-14)

Follows findings 17. You asked me to choose freely from that list.

## Taken outright (no prompt cost)

These three are in the working tree. They are uncommitted.

### #1 Helper imports just work (`src/worker.py`)

- **What.** A finder at the *end* of `sys.meta_path` resolves `plan`, `plan_ok`, `sh_helper`, `import plan, done` and any other `<helper>` or `<helper>_<anything>` name to the helper. It prints one line: `[import plan_ok: air's helpers are built in; no import needed]`.
  - Because the finder sits last, a real module always wins. A repo's own `plan.py` still imports normally (checked).
  - `import sh` keeps its old registration.
- **Not covered.** `import sh_helper if False else None` is a SyntaxError that no code can catch. It appeared 3 times in `airP` and once in this run's `airD`.
- **Known edge.** A *missing* third-party module whose name starts with a helper name (such as `edit_distance`) binds to the helper and prints the note, instead of raising.

### #10 A silent run shows its last value (`src/worker.py`)

- **What.** If a run ends with a bare expression and printed nothing, its value is printed, as in a REPL.
- **Why it's safe.** If anything was printed, nothing is added, so `check()` as the last line does not add a stray `True`.
- **Checked by driving the worker:**
  - `sh('echo x')` alone → `x`;
  - `print('a'); sh('echo b')` → `a` only;
  - `x * 2` → `10`;
  - SyntaxError and NameError output are unchanged.

### #3 Unix sockets under `<root>/tmp` (`src/worker.rs` rule + `SANDBOX` text in `src/agent.rs`)

- **What.** The rule was already in the working tree. The prompt now says "on macOS so does nested sandbox-exec, and Unix sockets work only under the repo's tmp/ directory".
- **Checked.** One real one-shot run on MiMo in a scratch repo:
  - `tmp/probe.sock`: server received `b'ping'`;
  - `/tmp/air-probe-sock-test.sock`: `PermissionError(1, 'Operation not permitted')`.

## Bug found by the benchmark: an empty reply ended the turn (`src/agent.rs`)

- **What happened.** In `refactor-airD-2`:
  1. MiMo wrote stub copies of every helper, which air restored.
  2. The provider then streamed a reply with 0 output tokens: the reasoning stopped at "The helpers", with no text and no tool call.
  3. air took that as the final answer, and the task ended with NO CHANGES.
- **How often.** The same empty reply appears 3 times in all 1,673 replies, always on the refactor task:
  - `refactor-air-3` (findings 15);
  - `refactor-airB-2` in this run (after its edits, so it still passed);
  - `refactor-airD-2`.
- **Not nondeterminism alone.** Identical requests at temperature 0 do vary (the probe saw 289, 297 and 310 reasoning tokens), but a 0-token reply is a glitch rather than an answer.
- **Fix.** On a reply with no text and no tool call, air drops it, notes "the model sent an empty reply; asking again" and asks again, once per turn (5 lines).
- **Checked.** Against a fake streaming server whose first reply is empty: the note shows, the second request answers, and the turn ends normally.

## Tested before deciding: 4 arms × 7 tasks × 3 runs (84 runs)

- **Setup.** Same tasks and hidden checks as findings 15–16, on MiMo V2.6 Pro, 4 runs at a time, arms interleaved.
- **Arms.** Every arm includes the three changes taken above.
  - `airB`: base.
  - `airD`: no `done()`; a passing `check()` ticks the next step.
  - `airS`: base plus `show()`.
  - `airT`: thinking off, via `chat_template_kwargs`.
- **Keep rule.** Written down before the run, in `wish/keep_rule.txt` in the scratchpad.
  - Keep only if the pass count is at least the base's, requests or input tokens fall by 5% or more, and neither rises by more than 5%.
  - For `show()`, qa and review must also pass as often as on the base.

| Total (21 runs each) | Base | No `done()` | `show()` | Thinking off |
|---|---:|---:|---:|---:|
| Passed | **21/21** | 20/21 | **21/21** | 20/21 |
| Requests | 82 | 73 | 78 | 243 |
| Input tokens | 214k | 175k | 194k | 4.72M |
| Uncached input tokens | 51.5k | 48.7k | 48.5k | 97k |
| Output tokens | 31.3k | 26.3k | 26.6k | 23.3k |
| Wall time (sum of runs) | 22.4 min | 19.9 min | **16.4 min** | 38.5 min |

### `show()`: kept

- Same pass rate as the base, including qa and review at 3/3 each.
- Against the base: input tokens −9.4%, uncached −5.8%, output −15%, requests −4.9%, time −27%.
- The model used it in 23 of its 57 runs without being pushed.
- The prompt grew by 58 bytes: one helper line, plus the reading sentence and the example now use `show()`. "One range per run" (#8) went with the old sentence.

### No `done()`: dropped by the rule (20/21)

- The one failure was the empty-reply glitch described above, not the missing `done()`.
- Even so, the base arm had **no** `done()`-alone runs in this sample, against 20 in the 62 earlier latest-build sessions. The saving it targets didn't show up here.
- **Instead: #15.** `done()` now forgives what the model actually does:
  - no plan → it does nothing;
  - a step given as text → it marks the current step;
  - nothing left → it returns None.
- The base arm hit the last case once: `print(done())` after every step was done raised "no step 0".

### Thinking off: dropped, clearly

- `feature-airT-2` looped. It ran `import subprocess` plus the same file read 157 times: 159 requests, 4.5M input tokens, killed at the 20-minute limit.
- Feature and refactor medians needed 7 requests against 4.
- Keep thinking on for MiMo in air.

### Also seen

- `import sh_util if False else None` (a SyntaxError no code can catch) appeared once in `airD`, which has the file listing. So that form is not limited to `airP`.
- No other helper-import error occurred in any arm. The new finder covers every other form.

## Final check with the repo build

| Task | Hidden check | Requests | Time |
|---|---|---:|---:|
| fix | 5/5 | 3 | 10 s |
| feature | 8/8 | 4 | 23 s |
| review | 6/6 planted bugs | 2 | 14 s |

Each session used `show()`. `cargo build` and `clippy -D warnings` are clean, and rustfmt is clean on `agent.rs`.

## Net change (uncommitted, on top of your `08a7a92`)

- **`src/worker.py`**, 294 → 323 lines:
  - helper-import finder;
  - prints a silent run's last value;
  - `show()`;
  - forgiving `done()`;
  - the `read()`/`cat()` hint now points at `show()`.
- **`src/agent.rs`**, 439 → 444 lines:
  - `SANDBOX` text for Unix sockets (the rule itself is in your `08a7a92`);
  - one retry on an empty reply.
- **`src/prompt.md`**: +58 bytes (`show()`).

## Dropped

| # | Idea | Why dropped |
|---|---|---|
| 5 | Host reuses the model's passing `check()` | Needs a worker-to-host round trip (about 25 lines across 3 files) to stay unforgeable. Revisit if konstruct turns (about 200 s per check) feel slow. |
| 6 | Name `git grep -n` in the prompt | Prompt words with no measured harm from hand-written grep loops |
| 8 | Several line ranges per run | Rode along with #4, because it is the same sentence |
| 9 | `spawn()` for long-running processes | 2 runs of evidence |
| 11 | `recall`/`carry`/`_last` out of the prompt | About 45 tokens; `carry` exists for your long REPL sessions |
| 12 | Network opt-in | `--yolo` exists; 1 failure seen |
| 13 | `edit`'s ok line names the line range | Re-reads after edits are down to 1 on the latest build |
| 14 | `accept()` warns instead of blocking | Keeps the "state your criteria first" step |
| 16 | `remove()`/`move()` | Rare |
| 7 | Graded reasoning effort | Primalabs ignores `reasoning_effort` (low, high, minimal and `reasoning.effort` all gave about 290 reasoning tokens on the same prompt). Only `chat_template_kwargs: {"enable_thinking": false}` works, and it is all-or-nothing (0 reasoning tokens). That was tested as arm `airT` below. |
