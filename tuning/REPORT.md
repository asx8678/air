# air tuning report

Branch `tune-opus` from `main` (6e5278f), 2026-10-16. The details for every result are in [EXPERIMENTS.md](EXPERIMENTS.md), and the harness as found is in [INVENTORY.md](INVENTORY.md). The suite that produced the numbers is [bench/](bench/README.md).

**Scope.** The brief asked to tune air for Opus 5.5, with Flash as a cheap first tier. By your instruction, everything was measured on `deepseek-v4.1-flash` (OpenCode Go) only. **Tier 2 was not run,** so every prompt-wording result below holds for Flash and is unvalidated on Opus.

## Summary

- **air was already close to the floor for Flash.** At baseline it passed 74/75, averaging 3.4–9.6 turns per task, with 98.5% cache efficiency and no mid-turn prefix breaks.
- **OpenCode on the same model** passed 15/15 but took 1.5× the turns, 2.7× the uncached input and 4× the prompt tokens.
- **Three changes were kept.** The two that matter are harness mechanics:
  - **E3** · an edit reloads the repo modules runs imported instead of deleting their names;
  - **E4** · a turn that changed files without edit()/write() is asked once to clean up.
  - **E9** · rewords the final-report line.
- **Rejected, with numbers:** all four prompt cuts (E1, E2, E7, E8) and both effort settings (E5, E6). For Flash, the example and the numbered workflow steady it; cutting them added turns or variance.
- **At N=5 per task, Flash's run-to-run noise is as large as most prompt effects.** The same build's per-task turns move ±0.4–1.4 between batches, and its per-task turns sd moved 1.11 → 1.36. The gateway also routes sessions to two different upstreams. Close calls were therefore settled with interleaved N=10 runs (E10).
- **One analysis bug, found and fixed:** the report's pooled numbers weren't task-balanced, which made some variants look worse than main. All decisions were recomputed (see the Correction in EXPERIMENTS.md).

## Before / after (tier 1, interleaved)

<!-- FINAL-TABLE -->

Tier 2 (Opus 5.5): not run, by instruction.

## Accepted changes

| # | change | commit | measured effect |
|---|---|---|---|
| E3 | worker.py `_forget()` rebinds names imported from a reloaded repo module (modules, functions, classes) to the new code; instances and failed imports are dropped as before. Prompt: "an edit or write reloads the ones you imported". | c7f3879 | 7 Python tasks × 10 runs: NameError right after an edit 5 → **0**, tool errors 23 → 13, pass 68 → 70/70, turns 4.17 → 4.14. The pooled CIs span 0, so this is a judgment call (see Decisions). |
| E4 | agent.rs: at the end of a turn with no edit()/write() whose tree still changed, one host message lists the files and asks the model to delete scratch output and keep its answer. | 9a4e8fb | review: pass 21/25 → **20/20**, runs leaving demo files 7/25 → 0/20. Pooled pass **+8.0pp [+2.0, +16.0]**. It fired in 3/20 review runs and 0 times on qa, airqa, wide and ops; about +2 turns when it fires. |
| E9 | Workflow step 4: "End with a short report: what changed, how you verified it, and what is still open." | 9e275a9 | vs tune1 back to back: output **−0.4k/run [−0.7, −0.2]**, other CIs span 0. Nearly all of it is reasoning, so it's suspect for drift; the interleaved check is in the table above. |

## Rejected experiments, and why

| # | change | result | why it failed |
|---|---|---|---|
| E1 | Replace Workflow + Example with a "Done" section (−242 tok/request) | turns **+0.9 [+0.5, +1.3]**, uncached +1.0k, output +0.6k | Without the example and the steps, Flash spreads one step over more runs. |
| E2 | E1 + optional rituals | superseded | It stacked on E1. |
| E5 | `reasoning_effort: high` | turns **+0.7 [+0.4, +1.0]**, uncached +1.3k | Costs more, and the pass gain was E4's review fix. |
| E6 | thinking disabled | pass **−15.5pp**, turns **+20** | Hard tasks looped; 8 runs hit the 200-request limit. |
| E7 | accept() optional, no plan()/done() asked (main prompt) | costs neutral; per-task turns sd 1.12 → 1.37, worse on 5 tasks | Failed the pre-written entropy check. |
| E8 | drop the Example only | turns +0.3 [−0.1, +0.7]; turns sd → 1.40, worse on 9 tasks | Failed the entropy check; the example steadies Flash. |
| A1 | nearest-match text when edit()'s old text isn't found | not built | 9 of 13 logged failures were fixed by the next run without a re-read: at most ~4 requests saved in ~4,100 runs. |

Levers from the brief, checked and not built:
- **Parallel tool calls.** They run one after another in one stateful worker. Concurrency would break shared variables, and code mode already batches inside a run: OpenCode spent 20 calls on wide, air ~5.
- **Auto-check after edits.** 244 of 725 edit runs had no check(), and only 62 of those were followed by a check-only run: a ceiling of ~0.08 turns per task. Running it automatically would add a cargo check to the other 182.
- **Saving the full output to a file on truncation.** 1 of 3,539 logged runs went past 32k characters.
- **Masking superseded results and compaction changes.** Masking was dropped before this round because models re-read what it hid. Compaction never triggered in 765 bench sessions, so the bench can't measure it.
- **Caching.** Efficiency is 98.5%; each request's cached tokens equal the previous prompt to within 64–128-token blocks.
- **Subagents.** Not tried. The wide-read task takes 5–6 turns in one agent, which leaves no room for a win.

## Remaining bottlenecks

1. **Model time is 93% of wall time.** Turns matter more than tool speed.
2. **Diff entropy belongs to the model.** Most code tasks give 5 distinct diffs in 5 runs, at a pairwise edit distance of 0.2–0.5. The harness's own decisions are already deterministic.
   - A formatter narrows Rust diffs; the Python templates have none configured.
3. **Noise at N=5.** The same build moves ±0.4–1.4 turns per task between batches, and the gateway splits sessions over two upstreams. Effects under ~0.5 turns per task need N=10 and interleaved runs (`make bench` per variant, at the same time).
4. **The bench is at the ceiling on pass rate.** Every task but review passes ≥95% at baseline. Success can only be shown with harder tasks, and more real commits are the natural source.
5. **Real-commit tasks are the costly ones:** 8–11 turns and 40–80 s of cargo check per run.

## Decisions that need you

1. **E3 is kept against the strict rule.** The pooled metrics are neutral (5 saved turns in 70 runs is below what N=10 resolves). The error class it removes was 100% harness-caused, and it is "tolerate the model's habit in the harness" (your memory rule). Keep, or revert c7f3879?
2. **E4 adds a host message,** a model-facing text, in the spirit of the existing check gate. It's the one change that moved pass rate. OK with that?
3. **Opus 5.5 transport.** air speaks only `chat/completions`.
   - The stored OpenCode key is Go-only: Zen answers "Model access is disabled" for every model.
   - Through an OpenAI-compatible shim, Claude would lose prompt-cache breakpoints, its thinking blocks and signatures (interleaved thinking), and the effort setting; `max_tokens` is unset too.
   - A native Messages-API path in llm.rs is the prerequisite for any Opus tuning. It is untestable without access, so it isn't built.
4. **E7 (optional rituals) is the change most likely to help a stronger model.** It was rejected here only on Flash's run-to-run spread. Worth one Opus run if Opus comes into scope.
5. **Merging tune-opus.** It's one commit per accepted change plus `tuning/`. The `exp/*` branches hold the rejected variants and can be deleted.

## Final system prompt and tool description

Exact Flash token counts (macOS, check configured). The system prompt is `prompt.md` (951 tok) + sandbox (179 tok) + AGENTS.md + the repo listing.

| section | tokens | why it stays as it is |
|---|---:|---|
| Opening line ("smallest correct diff, verified") | 22 | The goal in one line. |
| Code mode | 145 | The one tool, its persistence, the output cap, and (new, E3) that edits reload imported modules. |
| Helpers | 353 | The whole API with its signatures; the model can't act without it. |
| Example | 147 | Removing it raised the per-task turns sd 25% (E8) and, with Workflow, turns +0.9 (E1). |
| Workflow | 284 | The numbered steps make Flash batch accept/edits/check into one run (E1). Step 4 reworded (E9). |
| Sandbox | 179 | Messaging for the guards: what fails and that it's permanent. The guards are unchanged. |
| AGENTS.md, repo listing | repo-dependent (~200 for air) | The listing saves an exploring turn; frozen at session start, so the prefix stays cached. |
| `run` tool schema | 263 on the wire (15-word description) | Unchanged; the helpers are documented in the prompt, once. |
| Host messages | — | GATE (check failed), RETRY (lost, empty or cut-off reply), **STRAY (new, E4)**. Each fires at most once or twice per turn. |

## Incidents during the work

- **OpenCode edited a bench template.** In the first smoke test, its unsandboxed background service took its project from `$PWD` and edited `tuning/bench/tasks/fix/template/stats.py`. I restored the file and diffed it against the original. The bench now runs OpenCode with `--standalone`, under a sandbox-exec profile built from air's secret list, with `PWD` and `HOME` set to scratch paths. Nothing tracked was touched.
- **17 result rows were briefly lost.** A `git commit -a` on an experiment branch swept up `results.jsonl`, and the branch switch reverted it. I restored the rows from that commit. Since then, branch work happens in a separate worktree and no commit uses `-a`.

## Reproduce

```sh
cd tuning/bench
make bench VARIANT=6e5278f LABEL=main N=5 &   # run variants at the same time to cancel drift
make bench VARIANT=tune-opus LABEL=final N=5 &
wait; make report BASE=main VARIANT=final
python3 mechanics.py main final
```
