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
