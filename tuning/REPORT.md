# air tuning report

Branch `tune-opus` from `main` (6e5278f), 2026-10-16. The details for every result are in [EXPERIMENTS.md](EXPERIMENTS.md), the harness as found is in [INVENTORY.md](INVENTORY.md), and the suite is [bench/](bench/README.md). Every number below comes from `bench.py report` and `mechanics.py` over `bench/results.jsonl` (1,675 runs).

**Scope.** The brief asked to tune air for Opus 5.5, with Flash as a cheap first tier. By your instruction, everything was measured on `deepseek-v4.1-flash` (OpenCode Go) only. **Tier 2 was not run,** so every prompt-wording result below holds for Flash and is unvalidated on Opus.

## Summary

- **air was already close to the floor for Flash.** At baseline it passed 74/75, averaging 3.4–9.6 turns per task, with ~99% cache efficiency (cached ÷ the previous request's prompt) and no mid-turn prefix breaks.
- **OpenCode on the same model** passed 15/15 but took 1.5× the turns, 2.7× the uncached input and 4× the prompt tokens.
- **Three changes were kept, one commit each:**
  - **E3** · an edit reloads the modules runs imported instead of deleting their names;
  - **E4** · a turn that changed files without edit()/write() is asked once to clean up;
  - **E9** · the final message reports what changed, how it was verified, and what is still open.
- **Against main, interleaved at N=10 per task:** pass **150/150 vs 147/150** (+2.0pp [0.0, +4.0]), with turns, tokens and wall time unchanged within noise. Tool errors per run fell 0.17 → 0.10.
- **Rejected, with numbers:** every prompt cut (E1, E2, E7, E8), both effort settings (E5, E6), and the nearest-match edit error (A1). For Flash, the example and the numbered workflow are what make it batch its work: removing the example costs +0.8 turns per task at N=10.
- **At N=5, Flash's run-to-run noise is as large as most prompt effects.** The same build's per-task turns move ±0.4–1.4 between batches, and the gateway splits sessions over two upstreams. Close calls were settled with an interleaved N=10 head-to-head (E10).
- **One analysis bug, found and fixed mid-way:** pooled numbers weren't task-balanced. All decisions were recomputed.

## Before / after (tier 1)

Primary comparison: `main-h2h` (6e5278f) vs `final-h2h` (9e275a9 = main + E3 + E4 + E9), N=10 per task × 15 tasks, run at the same time. Pooled figures weigh every task the same; the CIs are a stratified bootstrap.

| metric | before (main) | after (final) | Δ (95% CI) |
|---|---|---|---|
| pass | 147/150 (98%) | **150/150 (100%)** | +2.0pp [0.0, +4.0] |
| turns (model requests) per task | 5.86 | 5.74 | −0.1 [−0.4, +0.2] |
| turns sd, mean per task | 1.26 | 1.22 | |
| tool calls (runs) per task | 5.07 | 5.05 | |
| tool errors per run | 0.17 | **0.10** | |
| runs rejected before acting | 10 | **3** | |
| failed edits | 7 | 7 | |
| uncached input per run | 6.3k | 6.8k | +0.5k [−0.2, +1.2] |
| cached input per run | 33.4k | 32.8k | |
| cache efficiency | 99.2% | 98.9% | |
| output per run | 3.4k | 3.5k | +0.1k [−0.2, +0.4] |
| reasoning share of output | 56% | 55% | |
| tokens sd, mean per task (uncached + out) | 2.5k | 3.6k | one ops outlier (that task's sd: 16.0k) |
| nominal cost per run | $0.0041 | $0.0043 | |
| wall time per run | 35 s | 35 s | |
| model time per run | 32.7 s | 32.7 s | |
| diff distance (mean pairwise normalized edit distance) | 0.29 | 0.26 | |
| distinct outcomes per task (of 10) | 7.3 | 7.1 | |
| final answer | 1,109 chars | 1,222 chars | |

Helper calls per run:

| | accept | plan | done | show | sh | edit+write | check() |
|---|---|---|---|---|---|---|---|
| before | 0.83 | 0.42 | 0.98 | 3.1 | 2.9 | 2.6 | 0.76 |
| after | 0.82 | 0.49 | 0.97 | 2.9 | 2.7 | 2.6 | 0.79 |

Per-task table (generated; columns as in bench/README.md):

| task | variant | n | pass | turns (sd) | uncached in | out | tok sd | cached | cost $ | wall s | calls | tool err | diff dist | outcomes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| airqa | main-h2h | 10 | 100% | 6.0 (1.6) | 9.6k | 1.9k | 4.6k | 77% | 0.0044 | 23 | 6.5 | 0.0 | 0.00 | 1 |
| airqa | final-h2h | 10 | 100% | 6.1 (1.4) | 8.9k | 2.1k | 2.7k | 78% | 0.0043 | 27 | 6.6 | 0.1 | 0.00 | 1 |
| c-compact-at | main-h2h | 10 | 100% | 9.8 (1.5) | 16.7k | 8.5k | 5.9k | 84% | 0.0108 | 77 | 10.0 | 0.1 | 0.34 | 10 |
| c-compact-at | final-h2h | 10 | 100% | 9.2 (1.8) | 16.4k | 8.9k | 7.3k | 84% | 0.0108 | 78 | 9.7 | 0.0 | 0.32 | 10 |
| c-overflow | main-h2h | 10 | 100% | 9.7 (2.8) | 14.7k | 6.8k | 4.8k | 85% | 0.0093 | 64 | 8.8 | 0.3 | 0.09 | 8 |
| c-overflow | final-h2h | 10 | 100% | 8.3 (1.3) | 15.4k | 6.7k | 5.2k | 81% | 0.0090 | 63 | 7.3 | 0.1 | 0.09 | 9 |
| c-python | main-h2h | 10 | 100% | 12.1 (1.9) | 16.5k | 11.0k | 8.2k | 89% | 0.0131 | 94 | 11.1 | 0.3 | 0.63 | 10 |
| c-python | final-h2h | 10 | 100% | 10.1 (2.1) | 18.2k | 9.6k | 7.9k | 86% | 0.0122 | 89 | 10.2 | 0.0 | 0.62 | 10 |
| c-window | main-h2h | 10 | 100% | 6.8 (1.8) | 5.9k | 4.9k | 1.7k | 86% | 0.0047 | 49 | 6.1 | 0.0 | 0.46 | 10 |
| c-window | final-h2h | 10 | 100% | 7.1 (2.1) | 7.2k | 5.2k | 3.7k | 82% | 0.0051 | 48 | 6.1 | 0.0 | 0.45 | 10 |
| debug | main-h2h | 10 | 100% | 3.3 (0.7) | 1.8k | 0.9k | 0.4k | 77% | 0.0011 | 14 | 2.3 | 0.2 | 0.03 | 3 |
| debug | final-h2h | 10 | 100% | 3.5 (0.5) | 1.9k | 0.9k | 0.5k | 78% | 0.0011 | 13 | 2.5 | 0.0 | 0.01 | 2 |
| feature | main-h2h | 10 | 100% | 4.7 (0.9) | 3.9k | 2.4k | 1.6k | 79% | 0.0025 | 23 | 3.7 | 0.2 | 0.42 | 10 |
| feature | final-h2h | 10 | 100% | 5.0 (1.3) | 4.0k | 2.8k | 2.9k | 80% | 0.0028 | 29 | 4.0 | 0.0 | 0.43 | 10 |
| fix | main-h2h | 10 | 100% | 3.4 (0.7) | 1.1k | 0.7k | 0.4k | 83% | 0.0008 | 12 | 2.4 | 0.3 | 0.37 | 10 |
| fix | final-h2h | 10 | 100% | 3.0 (0.0) | 0.9k | 0.7k | 0.2k | 83% | 0.0007 | 9 | 2.0 | 0.0 | 0.46 | 10 |
| ops | main-h2h | 10 | 100% | 6.5 (1.2) | 5.6k | 3.8k | 2.8k | 81% | 0.0039 | 40 | 5.5 | 0.3 | 0.32 | 10 |
| ops | final-h2h | 10 | 100% | 7.2 (2.6) | 10.1k | 4.5k | 16.0k | 85% | 0.0063 | 52 | 6.2 | 0.3 | 0.31 | 10 |
| qa | main-h2h | 10 | 90% | 4.8 (1.6) | 4.1k | 1.1k | 1.4k | 78% | 0.0020 | 19 | 3.8 | 0.0 | 0.00 | 2 |
| qa | final-h2h | 10 | 100% | 4.6 (0.5) | 4.3k | 1.1k | 1.5k | 75% | 0.0020 | 14 | 4.5 | 0.0 | 0.00 | 1 |
| recovery | main-h2h | 10 | 100% | 3.5 (0.7) | 1.7k | 0.8k | 0.4k | 80% | 0.0010 | 14 | 2.5 | 0.1 | 0.47 | 7 |
| recovery | final-h2h | 10 | 100% | 3.5 (0.5) | 1.4k | 0.8k | 0.4k | 84% | 0.0009 | 11 | 2.5 | 0.3 | 0.37 | 6 |
| refactor | main-h2h | 10 | 100% | 4.0 (0.7) | 2.2k | 1.5k | 0.8k | 80% | 0.0015 | 19 | 3.0 | 0.2 | 0.09 | 8 |
| refactor | final-h2h | 10 | 100% | 4.0 (0.7) | 2.2k | 1.6k | 0.9k | 79% | 0.0015 | 17 | 3.0 | 0.4 | 0.09 | 10 |
| review | main-h2h | 10 | 80% | 3.7 (0.5) | 3.1k | 2.8k | 1.5k | 74% | 0.0023 | 28 | 2.7 | 0.0 | 0.37 | 4 |
| review | final-h2h | 10 | 100% | 5.1 (1.5) | 3.6k | 3.1k | 1.7k | 82% | 0.0027 | 31 | 3.8 | 0.0 | 0.00 | 2 |
| rust | main-h2h | 10 | 100% | 3.5 (0.5) | 2.0k | 0.7k | 0.3k | 78% | 0.0010 | 15 | 2.5 | 0.0 | 0.25 | 7 |
| rust | final-h2h | 10 | 100% | 3.1 (0.3) | 1.6k | 0.7k | 0.2k | 78% | 0.0009 | 11 | 2.1 | 0.0 | 0.19 | 6 |
| wide | main-h2h | 10 | 100% | 6.1 (1.7) | 5.5k | 3.7k | 3.1k | 83% | 0.0038 | 35 | 5.2 | 0.5 | 0.47 | 9 |
| wide | final-h2h | 10 | 100% | 6.3 (1.6) | 5.5k | 4.1k | 3.3k | 84% | 0.0041 | 36 | 5.3 | 0.3 | 0.55 | 10 |

**Tier 2 (Opus 5.5): not run, by your instruction.**

External baseline, OpenCode 2.0.29 with the same model (N=1 per task), against air main (all runs):

| | pass | turns | tool calls | uncached in | total prompt | out | wall |
|---|---|---|---|---|---|---|---|
| OpenCode | 15/15 | 7.5 | 12.5 | 13.4k | 114k | 3.3k | 41 s |
| air main | 121/125 | 5.1 | 4.3 | 4.9k | 28k | 2.8k | 25 s |

OpenCode's first request is 5.5k tokens against air's 1.7k, and it reads files one call each: 20 calls on wide.

## Accepted changes

| # | change | commit | measured effect |
|---|---|---|---|
| E3 | `_forget()` in worker.py rebinds names imported from a reloaded repo module (modules, functions, classes) to the new code. Instances and names whose module no longer imports are dropped, as before. Prompt: "…an edit or write reloads the ones you imported". | c7f3879 | 7 Python tasks × 10 runs: NameError right after an edit **5 → 0**, tool errors 23 → 13, turns 4.17 → 4.14 (Δ −0.0 [−0.3, +0.2]). The pass difference there (68 → 70/70) is main's review stray-file misses, not E3. The prompt sentence slightly overstates: instances are dropped (no occurrence in 70 runs). |
| E4 | agent.rs: at the end of a turn with no edit()/write() whose tree still changed, one host message lists the files and asks the model to delete scratch output and keep its answer. | 9a4e8fb | review: pass 21/25 → **20/20** in the targeted run; runs leaving demo files 7/25 → 0/20. 5-task pooled pass **+3.2pp [+0.8, +6.4]**. It fired only on review (14 times across all variants), costing about +2 turns each time. At N=10, review turns went 3.7 → 5.1 with 3 firings; the firings explain about +0.6 of that. |
| E9 | Workflow step 4: "End with a short report: what changed, how you verified it, and what is still open." | 9e275a9 | **Cost-neutral:** interleaved vs tune1-late, output +0.3k [−0.2, +0.8]. The earlier −0.4k back-to-back "win" did not replicate. Kept because it is the report format the brief asks for. |

## Rejected experiments, and why

| # | change | result (task-balanced) | why it failed |
|---|---|---|---|
| E1 | Workflow + Example replaced by a "Done" section (−242 tok/request) | turns +0.47 [+0.04, +0.89] vs main; borderline vs main-late | Its parts, tested separately, both failed (E7, E8). |
| E2 | E1 + optional rituals | superseded | It stacked on E1. |
| E5 | `reasoning_effort: high` | Δturns +0.0 [−0.4, +0.4]; costs neutral | It adds a setting with no win; its pass gain was main's review misses, which E4 fixes. |
| E6 | thinking disabled | pass **−17.6pp**, turns **+19.6** | Hard tasks loop: 8 runs hit the 200-request limit, c-python averaged 135 turns. |
| E7 | accept() optional, no plan()/done() asked (on the tip, N=10) | Δturns **+0.3 [−0.0, +0.6]**, costs neutral | Fails E10's rule: a +0.3-turn cost can't be ruled out. |
| E8 | drop the Example (on the tip, N=10) | Δturns **+0.8 [+0.5, +1.1]**, pass −2.0pp | The example is what makes Flash batch its work. |
| A1 | nearest-match text when edit()'s old text isn't found | not built | 9 of 13 logged failures were fixed by the next run without a re-read: at most ~4 requests saved in ~4,100 runs. |

Levers from the brief, checked and not built:
- **Parallel tool calls.** They run one after another in one stateful worker. Concurrency would break shared variables, and code mode already batches inside a run.
- **Auto-check after edits.** 244 of 725 edit runs had no check(), and only 62 of those were followed by a check-only run: a ceiling of ~0.08 turns per task. Automating it would add a cargo check to the other 182.
- **Saving the full output to a file on truncation.** 1 of 3,539 logged runs went past 32k characters.
- **Masking superseded results.** Already dropped before this round, because models re-read what it hid.
- **Compaction.** It never triggered in 765 bench sessions, so the bench can't measure it.
- **Caching.** Efficiency is ~99%; each request's cached tokens equal the previous prompt to within 64–128-token blocks.
- **Subagents.** Not tried. The wide-read task takes ~6 turns in one agent, which leaves no room for a win.

## Remaining bottlenecks

1. **Model time is 93% of wall time.** Turns matter more than tool speed.
2. **Diff entropy belongs to the model.** At N=10, code tasks give 2–10 distinct diffs per 10 runs (most give 10), at a pairwise edit distance of 0.01–0.63. The harness's own decisions are deterministic. A formatter narrows Rust diffs; the Python templates configure none.
3. **Noise at N=5.** The same build moves ±0.4–1.4 turns per task between batches, and the gateway splits sessions over two upstreams (`deepseek/…` and `deepseek-ai/…`). Effects under ~0.5 turns per task need N=10 and interleaved runs.
4. **The bench is at the ceiling on pass rate.** At N=10, main misses only review (stray files) and qa. Showing a success gain needs harder tasks; more real air commits are the natural source.
5. **Real-commit tasks are the costly ones:** 7–12 turns and 48–94 s per run, mostly cargo check.
6. **E4's cost on review:** +2 turns when it fires (cleanup, then finish). A cheaper form would let the model clean up and answer in one reply. Not tried.

## Decisions that need you

**Where I departed from the decision rule I wrote before the runs (three places):**
1. **E3 was kept by judgment.** Its pooled metrics are neutral, and the strict rule rejects neutral additions. The error class it removes was entirely harness-caused, and the fix is the "tolerate the model's habit in the harness" your memory rule prefers. Keep, or revert c7f3879?
2. **E9 was kept on the brief's grounds.** It is cost-neutral, and the rule has no category for a rewording; the brief asks for exactly this report format.
3. **E10 replaced the entropy check.** The pre-written check (pooled per-task turns sd) turned out to be as noisy as the effects at N=5: the same build scored 1.11 vs 1.36. The close calls were re-decided by a rule written before the N=10 head-to-head ran. E7 and E8 were rejected under it.

**Other decisions:**
4. **E4 adds a host message** (model-facing text), like the existing check gate. It is the change that moved pass rate. OK?
5. **Opus 5.5 transport.** air speaks only `chat/completions`.
   - The stored OpenCode key is Go-only: Zen answers "Model access is disabled" for every model.
   - Through an OpenAI-compatible shim, Claude would lose prompt-cache breakpoints, its thinking blocks and signatures (interleaved thinking), and the effort setting; `max_tokens` is also unset.
   - A native Messages-API path in llm.rs is the prerequisite for any Opus tuning. It is untestable without access, so it isn't built.
6. **E7 (optional rituals) is the change most likely to help a stronger model.** It failed here only by not ruling out +0.3 turns on Flash. Worth one Opus run if Opus comes into scope.
7. **Merging `tune-opus`.** It's one commit per accepted change (c7f3879, 9a4e8fb, 9e275a9) plus `tuning/`. The `exp/*` branches hold the rejected variants and can be deleted.

## Final system prompt and tool description

Exact Flash token counts (macOS, check configured). The system prompt is `prompt.md` (951 tok, main: 953) + sandbox (179 tok) + AGENTS.md + the repo listing.

| section | tokens | why it stays as it is |
|---|---:|---|
| Opening line ("smallest correct diff, verified") | 22 | The goal in one line. |
| Code mode | 145 | The one tool, its persistence, the output cap, and (E3) that edits reload imported modules. |
| Helpers | 353 | The whole API with its signatures; the model can't act without it. |
| Example | 147 | Removing it costs +0.8 turns per task (E8, N=10). |
| Workflow | 284 | The numbered steps make Flash put its edits and check() in one run (E1, E7). Step 4 reworded (E9). |
| Sandbox | 179 | Messaging for the guards (what fails, and that it's permanent). The guards are unchanged. |
| AGENTS.md, repo listing | repo-dependent (~200 for air) | The listing saves an exploring turn; frozen at session start, so the prefix stays cached. |
| `run` tool schema | 263 on the wire (15-word description) | Unchanged; the helpers are documented in the prompt, once. |
| Host messages | — | GATE (check failed), RETRY (lost, empty or cut-off reply), **STRAY (new, E4)**. Each fires at most once or twice per turn. |

## Incidents during the work

- **OpenCode edited a bench template.** In the first smoke test, its unsandboxed background service took its project from `$PWD` and edited `tuning/bench/tasks/fix/template/stats.py`. I restored the file and diffed it against the original. The bench now runs OpenCode with `--standalone`, under a sandbox-exec profile built from air's secret list, with `PWD` and `HOME` set to scratch paths. Nothing tracked was touched. The 32 rows from the broken attempts were removed.
- **17 result rows were briefly lost.** A `git commit -a` on an experiment branch swept up `results.jsonl`, and the branch switch reverted it. I restored the rows from that commit and reconciled every row against its run directory. Since then, branch work happens in a separate worktree and no commit uses `-a`.

## Reproduce

```sh
cd tuning/bench
make bench VARIANT=6e5278f LABEL=before N=10 JOBS=2 &   # run the variants at the same time
make bench VARIANT=tune-opus LABEL=after N=10 JOBS=2 &
wait; make report BASE=before VARIANT=after
python3 mechanics.py before after
```
