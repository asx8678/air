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
