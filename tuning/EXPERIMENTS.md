# Experiments log

Tier 1 only: `deepseek-v4.1-flash` on OpenCode Go, N=5 per task, 15 tasks (75 runs per variant). Tier 2 (Opus) is not run, by the user's instruction (2026-10-16). Every prompt-wording result below is **validated on Flash only**.

## Decision rule (written before any variant ran)

With N=5, one task's pass rate moves in 20-point steps, and a difference of under one request is noise (Flash's p90 is 5 requests per turn). So decisions use pooled numbers across all tasks, with a stratified bootstrap 95% CI (`bench.py report`).

- **Reject** when the Δpass CI lies wholly below 0, or pooled passes drop by more than 2 of 75. Success is never traded away.
- **Keep (win)** when pass doesn't drop and at least one of Δturns, Δuncached input or Δoutput has a CI wholly below 0.
- **Keep (neutral)** when pass doesn't drop, every CI spans 0, and the change *removes* prompt text or code. Fewer instructions is the standing design rule (memory: minimal model instructions), and fewer tokens per request follows by construction.
- **Reject (neutral)** when a change *adds* text or code without a measured win.
- **Entropy check:** a kept change must not raise the pooled turns sd or the distinct outcomes on more than 3 tasks.
- **Rerun** a result that looks too good before believing it. **Re-baseline** after every 3–4 accepted changes.

## Baseline

`main` = 6e5278f (the harness unchanged; `tune-opus` only adds `tuning/`). The external baseline is OpenCode 2.0.29 with the same model, N=1.

| metric (pooled, 75 runs) | main |
|---|---|
| pass | 74/75 (review #4 found 4 of 6 planted bugs) |
| turns per task | 3.4–9.6 (mean 5.8) |
| cache efficiency (cached ÷ previous request's prompt) | 98.5% |
| first request | 1,682 prompt tokens |
| output that is reasoning | 56% |
| tool errors | 17 (5 rejected before acting, 3 failed edits) |
| verdicts | VERIFIED 51 · NO CHANGES 14 · NO CHECK 10 |
| upstreams the gateway picked | `deepseek/…` 52 · `deepseek-ai/…` 23 sessions |

Diffs vary a lot: pairwise normalized edit distance is 0.4–0.5 on fix, feature, review, c-python and c-window, with 5 distinct diffs in 5 runs on most code tasks. So the entropy left is in *what code the model writes*, not in the harness's decisions.

## A0 · Prompt prefix and caching (analysis, no change)

- **Hypothesis:** something air sends breaks the cached prefix mid-turn.
- **Evidence:** each request's `cached_tokens` equals the previous request's prompt, rounded down to the provider's 64–128-token blocks. Cache efficiency is 98.5% over 75 sessions. The per-request "cache hit rate" (79–88%) is low only because each run's new output can't be cached yet.
- **Decision:** nothing to fix.
- Found on the way: the gateway routes sessions to two upstreams. That is run-to-run variance air can't control; mechanics.py now reports it.

## A1 · Nearest-match text on a failed edit (analysis, not built)

- **Hypothesis:** returning the closest lines on "old text not found" saves a re-read turn.
- **Evidence:** of the 13 not-found/occurs failures in all logged sessions, 9 were fixed by the very next run with no re-read in between. One Flash case needed a re-read and 2 runs. The baseline had 1 "occurs 4 times" in 75 runs, fixed the next run.
- **Decision:** not built. It would save at most ~4 requests in ~4,100 runs, and it adds code. Rejected by the decision rule.

## E1 · Say what done means instead of a numbered workflow, and drop the example: REJECTED (Flash)

- **Hypothesis:** a strong model needs goals, not procedure. Replacing Workflow (281 tok) and Example (147 tok) with a "Done" section (186 tok) cuts 242 tokens per request at no cost.
- **Change:** branch `exp/prompt-done` (5559f15).
- **Result (75 vs 95 runs):**

  | | main | prompt-done | Δ (95% CI) |
  |---|---|---|---|
  | pass | 98% | 100% | +2.1pp [0, +4.2] |
  | turns | 5.35 | 6.23 | **+0.9 [+0.5, +1.3]** |
  | uncached input per run | 5.5k | 6.5k | **+1.0k [+0.3, +1.6]** |
  | output per run | 3.0k | 3.6k | **+0.6k [+0.2, +0.9]** |

  The extra turns land on c-compact-at (+2.0), c-overflow (+1.6), review (+1.4), feature and refactor (+1.0). Flash also called plan() on more small fixes.
- **Reading:** for Flash, the example and the numbered steps are load-bearing. They are what make it put the edits and check() into one run. The cut saved 242 prompt tokens per request but cost ~1k more uncached input per run.
- **Decision:** rejected by the rule (cost CIs wholly above 0). Untested on Opus, where the trade may well go the other way. Split into E8 (example only) and E9 (report wording only) to find which part carries the cost.

## E2 · rituals on top of E1: superseded

`exp/rituals` was stacked on E1, so it can't be read against main. Its numbers isolate the ritual effect relative to E1 only. It is re-run on the main prompt as E7 (`exp/rituals2`).

## E3 · An edit reloads the modules runs imported instead of deleting their names: KEPT (judgment call, see below)

- **Hypothesis:** after `from stats import median`, `edit("stats.py", …)` deletes `median` from the worker's globals (`_forget()`). The model's own check on the next line then raises NameError and costs a round trip. In 4 of 75 baseline runs this happened with an edit just applied. The prompt's "import them again after a change" doesn't stop it.
- **Change:** `exp/reload` (0e5eb07, +28 lines in worker.py). Names bound to a reloaded repo module, or to a function or class from one, are rebound to the new code. Instances are dropped as before, and so is a name whose module no longer imports. The prompt sentence shrinks to "an edit or write reloads the ones you imported".
- **Result (7 Python tasks, N=10 each):**

  | | main | reload |
  |---|---|---|
  | pass | 68/70 | 70/70 (main's 2 misses are review runs that left demo files) |
  | NameError right after an edit | 5 | **0** |
  | tool errors | 23 | 13 |
  | turns | 4.17 | 4.14 (Δ −0.0 [−0.3, +0.2]) |
  | turns sd | 1.24 | 1.18 |

- **Decision:** the pre-registered pooled metrics are neutral, which would reject added code. I'm keeping it anyway:
  - every removed error was a harness-caused round trip, and that class went 5 → 0 (Fisher p ≈ 0.06);
  - 5 saved turns over 70 runs (−0.07 per run) is below what N=10 can resolve;
  - it makes the harness tolerate the model's natural habit, which the standing design rule prefers to prompt text.

  **Flagged for the user** as an override of the strict rule. Commit c7f3879 on tune-opus.

## E4 · Ask once about files a turn changed without edit()/write(): KEPT (win on success)

- **Hypothesis:** a review or question turn that runs the code under test leaves demo output in the repo. Examples from the logs: `tmp_demo5.json`, `pwned.txt`, `exports/out.csv`. That fails the "change no files" ask and makes the diff non-minimal. air already knows when this happened: the tree moved, but no edit() or write() did it.
- **Change:** `exp/stray`, applied as 9a4e8fb (+30 lines in agent.rs). At the end of a turn with no edit()/write() whose tree changed, air sends one host message listing the files: "This turn changed files without edit() or write(): …. Delete the ones that are scratch output from running code, keep any that are part of your answer, then finish again."
- **Result:**

  | | main | stray |
  |---|---|---|
  | review pass | 21/25 (84%) | **20/20 (100%)** |
  | review runs leaving files | 7/25 | 0/20 |
  | gate fired | — | 3/20 review runs, 0 on qa, airqa, wide, ops |
  | pooled pass, 5 tasks | 92% | 100%: **+8.0pp [+2.0, +16.0]** |
  | turns | 4.92 | 5.20: +0.3 [−0.2, +0.8] |
  | uncached input | 4.7k | 4.7k: +0.0k [−0.6, +0.6] |

  - The gate costs about 2 turns when it fires (clean up, then finish).
  - The review runs without a nudge were also +0.5 turns over main. Gateway upstream doesn't explain it (both upstreams show it), and all 20 runs fell in one 2-minute window, so I read it as noise. Unproven.
  - Entropy on review: distinct outcomes fell 9 → 2 and diff distance 0.47 → 0.00.
- **Decision:** kept. Pass rose with a CI wholly above 0, and task success outranks turns.

## E5 · `reasoning_effort: "high"`: REJECTED

- **Hypothesis:** more effort lowers variance and turns on the hard (real-commit) tasks.
- **Change:** `exp/effort-high` (e9e830b, an experiment-only default in `settings()`; the real knob would be a config.toml profile key).
- **Result (75 vs main's 125):**

  | | main | effort-high | Δ (95% CI) |
  |---|---|---|---|
  | pass | 98% | 100% | +3.2pp [+0.8, +6.4] (all from review's stray-file misses, which E4 fixes on its own) |
  | turns | — | 5.76 | **+0.7 [+0.4, +1.0]** |
  | uncached input | — | — | **+1.3k [+0.7, +2.0]** |
  | output | — | — | **+0.4k [+0.2, +0.7]** |
  | reasoning share of output | 50% | 57% | |

  Turns sd on the real-commit tasks fell (c-python 2.3 → 0.5, c-compact-at 3.4 → 1.8), but c-compact-at needed more turns (8.6 → 11.4).
- **Decision:** rejected. It costs more per task, and the success gain belongs to E4. The gateway default (thinking on, no effort field) stays.

## E6 · Thinking disabled (`thinking: {type: disabled}`): REJECTED

- **Result:**
  - pass 61/75 (81%), Δ **−15.5pp [−22.9, −8.3]**;
  - turns **+20.3 [+11.6, +29.5]**;
  - 8 runs ran into the 200-request limit.
- **Where it broke:** easy tasks got slightly cheaper (fix 3.0 turns, feature 4.0, qa 4.2). Hard ones looped: c-python 135 turns at 20% pass, ops 84 turns at 60%, c-compact-at 63 turns at 60%. Wide dropped one row in 3 of 5 runs.
- **Decision:** rejected. Reasoning is what carries success on multi-step work.
- The 200-request guard only binds in this broken configuration, never with thinking on (max 20 requests per turn in 461 runs, no limit events), so it stays.
