# 21 — Pi Fabric, second pass: what is left to port (2026-10-14)

**Sources and evidence.**
- **Fabric.** `fabric-runtime/pi-fabric` `up/main` @ `e22cc59` (v0.116.1), cloned into a scratchpad. The user's checkout in `~/projects/project1/pi-fabric` was not touched.
- **Air.** The working tree on `audit-round-12-fixes`, with findings 20 round 2 uncommitted.
- **Logs.** All 332 logs in `~/.air/sessions` (2,731 runs), plus the traces from findings 19–20.

## Status

Items 1–4 were implemented the same day and are uncommitted.

- **Profile `guidance`.** A `guidance` key in a profile adds a "Notes for this model" section to the system prompt and is stored in the header. `--stats` groups sessions by a hash plus the first words of the notes.
- **Turn outcomes.** A "Turn outcomes (host verdicts)" summary section lists `#u task → VERDICT, N files changed`, or "no verdict" for an interrupted turn. The in-progress turn is skipped, and the section is omitted for logs without reports.
- **Read files.** `read path #n` rows join the Files block, taken from `show("path"…)` calls; `#n` is the run whose output `recall(n)` returns.
- **Quoting hint.** A `SyntaxError` hint fires for runs that call edit/write with triple quotes.

Verification:

- `cargo build` and `clippy -D warnings` are clean.
- Every real compaction was replayed, cut at its compaction point. Summaries grew 150–2,300 bytes, all under 11 KB. At turn-start compactions the finished review turn shows `#1 review this app → NO CHANGES`.
- A fake-provider run checked the guidance notes (absent from the request parameters), the hint text and the `--stats` label.
- One real MiMo turn was VERIFIED in 3 requests.
- No MiMo guidance text was added to the user's config: each line still needs the frozen-review A/B.

Items 5–7 were also implemented the same day, on the user's request, and are uncommitted.

- **Repeat guard** (host, `agent.rs`).
  - From the 3rd identical consecutive run in a turn, the output ends with `[the same code has now run N times in a row; at 6 it is blocked]`.
  - The 6th identical run is not executed. It returns an error and is counted as `rejected`, and a `repeat` event is logged (counted as waste by `--stats`).
  - The counter resets every turn.
- **`recall("regex")`** (worker).
  - It lists up to 30 matching session entries (newest last) as `#n role: matching line`. Content and run code are searched; the call's own entry is excluded.
  - An invalid pattern falls back to literal text. The prompt's helper line mentions the new form.
- **"Applied before the error" note** (worker, Fabric's `failure-progress.ts`).
  - A run that raises after successful edits or writes ends with `[applied before the error: edit a.txt; inspect before repeating them]`.
  - It is a runtime note rather than the prompt line S2, so it costs nothing when no run fails.
- **Verification.** A fake-provider run covered all three. Clippy is clean, and one real MiMo turn was VERIFIED in 3 requests.

## Bottom line

- **Upstream has nothing new for air.** Since findings 13 (`c538d3e`) there are 6 commits: a UI activity pane, a jev-fabric bump and regenerated Bend kernels.
- **Most of what fits air has already been ported:**
  - verify-by-probing, multi-edit, the per-exchange dialogue budget and the sandbox-boundary section (findings 13);
  - per-model `compact_at` (Fabric's `tokenThresholds`) and near-miss call repair (helper-as-tool routing, in the spirit of Fabric's `run-display` coercion) (findings 20).
- **Four small items are left that today's data supports,** each under 25 lines. Everything else is Fabric's runtime (agents, mesh, Jev, schema, entropy, speculation, kernels), which does not fit a one-tool CLI.

## Ranked candidates

The score is the net value for air on the evidence, minus the code cost, on the same scale as findings 13 and 17.

| # | Candidate (Fabric source) | Evidence in air's logs | Cost | Score |
|---|---|---|---|---:|
| 1 | **Per-model guidance text in profiles** (`components/model-guidance.ts`, slot text selected by `provider/model` patterns) | MiMo-specific habits: 26 helper-as-tool calls in one fix turn, runaway deliberation, `accept("…"))` typos (13 SyntaxErrors). A per-model line fixes one model without touching the prompt for others. | ~6 lines: `guidance` key in the profile, appended to the system prompt at session creation, so it is pinned in the header and cached | **40** |
| 2 | **Turn outcome rows in the summary** (`compaction/projections.ts` "[Earlier Turns]") | `report` entries are now logged per turn, so this is cheaper than in findings 13 (C4). Long REPL sessions with several tasks lose which earlier tasks were VERIFIED. | ~15 lines in `compact.rs`: append `→ VERIFIED` / `NO CHANGES` / `UNVERIFIED` from the turn's `report` to the "Earlier requests" and dialogue headings | **35** |
| 3 | **Hint for payload-quoting SyntaxErrors** (adapted from `payloads` plus `type-error-guidance.ts`) | 5 of 507 edit/write runs failed on raw triple-quoted strings (one ended in `\'''`, one nested `"""` inside `r"""`). 3 of the 5 were in today's long fix turns, where each wasted request replays about 100k context tokens. | ~4 lines in `_hint`: name the two causes and the fix (use the other quote style, or a plain string with escapes) | **28** |
| 4 | **Read files in the summary's Files section** (`projections.ts` file projection: reads with entry addresses) | Old-build fix turns re-read 15–49 files after compaction. MiMo's 200k profile makes compaction rare, but long REPL sessions still compact. Listing `read path #n` lets the model `recall(n)` exact content or skip files it has ruled out. | ~10 lines: count `show(path…)` calls per path in `summary()` | **25** |
| 5 | Repeat guard, warn at 3, block at 6 identical runs (`repeat-guard.ts`) | The only real streaks (11×) came from a pre-4047dea benchmark arm. The 6× streak in C-2 was routed tool calls, not repeats. The value rises once profiles bring in cheaper models. | ~10 lines | 15 |
| 6 | `recall("regex")` search, or across sessions (`memory.recall`) | `recall()` was used 10 times in 2,731 runs | ~15 lines | 12 |
| 7 | Failure-progress line: "completed before the failure, inspect before repeating" (`failure-progress.ts`, S2) | 4 runs raised after successful edits, and none of them redid the edit | 1 prompt line | 10 |
| 8 | Model-driven thinking level for a turn or N turns (`thinking-control.ts`) | Primalabs ignores `reasoning_effort`, and thinking off looped (159 requests). Useful only with providers that honour levels; it would then be a profile option, not a model tool. | ~40 lines | 8 |
| 9 | Prewalk: a plan checkpoint at the first mutation, then a handoff to a cheaper executor model (`prewalk/`) | One model per session in air. The "one step per finding" prompt arm (C) was worse. A second model is an architectural change. | 300+ lines | 8 |
| 10 | Recursive decomposition of whole-repo audits into child contexts (`fabric-rlm`) | Would widen "review this app" (Pi found 3 extra real bugs), but needs a child-agent runtime | large | 8 |
| 11 | Ambient supervisor or spec supervisor (second model watching the session) | air's deterministic nudge covers the measured stalls (empty replies, stream cuts) | large | 5 |
| 12 | Failure output budget of 20k plus an artifact path (`output-budget.ts`) | 0 failing runs over 20k; `_last` already keeps the full output | small | 3 |
| 13 | Decisions (durable questions to the user), saved programs, state certificates, schema enforce, Jev, entropy/repairs, mesh/actors, speculation, QuickJS/Monty/Bend | no need shown, or against air's one-tool design | large | 0–2 |

## Notes on the top four

1. **Guidance per model.**
   - Profile shape: `[models."primalabs-ai/MiMo-V2.6-Pro"] guidance = "…"`, appended as a `## Model notes` section.
   - It must go into the header's `system` at creation, not into each request, so the prompt stays byte-stable and cached and `--stats` groups sessions by it.
   - Patterns (Fabric matches `provider/*`) can wait until there is a second model.
   - Test each guidance line with the frozen-review harness (findings 20) before keeping it. The C arm showed that well-meant workflow lines can backfire.
2. **Turn outcome rows.**
   - Read `report.verdict` from the role-less `report` entry that ends each turn.
   - "Earlier requests" becomes `#12 fix the findings → VERIFIED (7 files)`, and the dialogue heading gets `(VERIFIED)`.
   - A turn cut off by compaction before its report reads `→ unfinished`.
3. **Quoting hint.**
   - Trigger: a `SyntaxError` in a run whose code calls `edit(`/`write(` with a triple-quoted literal.
   - Text: `hint: a raw string cannot end with a backslash, and r"""…""" cannot contain """; switch the quote style or use a plain string with escapes`.
   - It mirrors the existing `match`/`case` hint.
4. **Read files.**
   - `summary()` already walks every tool call. Add each `show("path"…)` path with its latest entry number to the Files block, as `read path #n`, ranked below modified and created files.
   - The 4 KiB block cap and the omission marker keep it bounded.

## Not ported, on purpose

- **LLM-written or extractive summaries:** violate LLM-free compaction.
- **QA mutation tests:** no tests in air.
- **Background jobs:** reverse the kill-group fix.
- **Parallel `asyncio.gather` batching:** tested and rejected in findings 14.

## Suggested order

1. Items 1–3 together, about 25 lines.
2. A frozen-review A/B of the MiMo guidance line, using the item-1 profile key.
3. Item 4 once a real long session compacts again.

## Verification and benefit check (same day)

### Correctness

- **Diff review.** The whole uncommitted diff was reviewed. One real flaw was found and fixed: the quoting hint matched `f.write(` method calls and fired on any SyntaxError in a run that also had a triple-quoted edit.
  - It now counts only bare `edit(`/`write(` calls, and fires only when the error is on or after the first such call's line.
  - Replayed on all 28 SyntaxErrors in the logs, it fires on 5/5 quoting errors and on 0/23 others. The first version would have misfired on 2 typo cases.
- **Fake-provider runs.** These covered:
  - the guidance notes (absent from the request parameters);
  - the hint;
  - the applied-before-error note;
  - `recall("regex")`, which excludes its own call and falls back to literal text on an invalid pattern;
  - the repeat guard (warns on runs 3–5, blocks the 6th).
- **Real runs.**
  - One MiMo turn was VERIFIED in 3 requests.
  - The 7-task bench ×2 on the final build: 14/14 pass, 52 requests, cost index 130k, against 49 and 132k before, so no regression.

### Overhead

| Item | Overhead |
|---|---|
| `air --stats` | 0.29 s over 332 logs |
| `recall("regex")` | 4 ms on the largest log (1.6 MB) |
| `recall` prompt line | about 20 tokens on every request (cached) |
| Read rows and turn outcomes | 150–2,300 bytes per compaction summary |
| Hint, failure note, repeat guard | 0 unless they fire |

### Benefit

- **Quoting hint: none measured.** In all 5 historical quoting failures (without the hint), MiMo fixed the edit on the very next run.
- **Repeat guard: insurance only.** Real sessions on current builds have no streak of 3 or more and no false positives. The only streaks were two pre-fix bench arms.
- **Failure note: none measured.** 4 historical cases, none led to a bad redo.
- **Forced-compaction A/B on the frozen review** (`compact_at = 100000`, the default for models without a profile, so the fix turn starts with a turn-start compaction; n = 3 each):

| | requests | cost index | re-reads | `recall` calls | verdict |
|---|---|---|---|---|---|
| without the summary items (S0) | 20 (18–21) | 463k (384k–556k) | 17 (14–24) | 0 | 3/3 VERIFIED |
| with them (S1) | 24 (21–42) | 589k (516k–700k) | 14 (14–32) | 0 | 3/3 VERIFIED |

- **No benefit from the compaction items.** The model never calls `recall` (by number or by search) and re-reads files with `show()` regardless. S1 is within noise of S0, if anything slightly worse.
- **Turn outcomes target multi-task REPL sessions,** which this test (one earlier turn) does not cover.
- **Side result.** Both arms, at compaction 100k, beat the old build's 54 requests and 999k cost on the same review (findings 20). The round-2 changes help even without MiMo's 200k profile, although these arms also ran at temperature 1.0.

### Decision (same day)

- **Removed:** on the user's go-ahead, after the benefit check, `recall("regex")` (worker and prompt line) and the `read path #n` Files rows (`shown()`). Both are back to exactly what they were before this round.
- **Kept:** turn outcomes, guidance in profiles, the quoting hint, the failure note and the repeat guard.
- **Verified:** build, fmt and clippy are clean; the replayed summary shows the turn outcome and no read rows; one real MiMo turn passed its check in 3 requests.
