# 20 — What the new tracing shows: where turns, tokens and variance go (2026-10-14)

## Status

Round 2 below supersedes the "next round" plan at the end: the nudge, `COMPACT_AT` 200k, helper routing, profiles and `--stats` are implemented and measured.


- **Implemented, uncommitted:**
  - `meta.finish` (the provider's `finish_reason`) on every assistant entry and on `empty` events.
  - The compaction summary gives the reply that the newest summarized direction answers the room the older exchanges leave. It used to be capped at 3,968 bytes.
- **Verified:**
  - `cargo build` and `clippy -D warnings` are clean.
  - Old and new `summary()` were replayed on every real compaction in `~/.air/sessions`: only the two summaries that cut a review changed (7,954 → 8,932 and 8,052 → 10,009 bytes), and the full review is kept.
- **Harness.** The harness is now in `~/projects/air-bench` (bench, `trace.py`, `turns.py`, `sim.py`, `real/flow.sh`, `replay/`), out of `/private/tmp`.
- **Not adopted.** Never replaying reasoning (item 3) is not adopted.

## Round 2 (same day): implemented and A/B-tested from a frozen review

### What changed (uncommitted)

- **Compaction at 200k for MiMo, via its profile (`compact_at = 200000`).** MiMo accepted a 300,010-token prompt (one probe request), so the window is at least 300k. Turn-start compaction then needs 100k, so a 51–67k review stays in context for the fix turn. The code default stays at 100k because other models may have 128k windows. (The A/B build had the constant at 200k, which is the same effective value.)
- **Empty reply → host nudge.** An empty reply now adds a host user message instead of resending the same request: "Your last reply was empty: you reasoned at length and sent nothing. Don't work out every change in advance: send one run() now with the next concrete step…".
  - At most 2 nudges per turn.
  - When the last message is the user's own task, air plainly retries instead, so two user messages never follow each other.
- **Stream cut after long thinking → nudge.** `complete()` no longer resends a stream that was cut after more than 240 s, or on the 4th cut. It returns `Interrupted`, and the agent adds the same kind of nudge.
  - `interrupted` events now carry the attempt's `ms`, which was a trace gap.
- **A helper called as a tool (`edit`, `plan`, …) runs inside `run()`** as `name(**json.loads(args))`, logged as `bad_call`.
- **Per-model profiles.**
  - `[models."<id>"]` in `~/.config/air/config.toml` holds any request parameter (temperature, top_p, chat_template_kwargs, …) plus `compact_at`.
  - The profile is pinned into the session header (`params`, `compact_at`) at creation, so `--resume` keeps it and every log records it.
  - Old headers that have only `temperature` still work.
- **`air --stats`.** It groups every reported turn in `~/.air/sessions` by model + settings and shows:
  - turns, verified/changing, stalled (no change plus a waste event);
  - requests per turn, input with cache %, output with reasoning %;
  - waste per 100 requests (empty, interrupted, bad_call, limit, worker_restart, error);
  - time per turn and a cost index (uncached + cached/10 + 4 × out per good turn).
  - It stars the best setting per model once 2 or more settings have at least 5 turns.
  - "Stalled" means the turn ended on an empty reply, the request limit or an error, and changed nothing.
  - Sessions under temp dirs (benchmarks) are skipped, so the numbers reflect real work. On copies of this round's sessions it found exactly the 2 real stalls and starred temperature 1.0.
  - It describes; it does not switch settings by itself.
- **MiMo profile added to the user's config:** `temperature = 1.0`, `top_p = 0.95`, `compact_at = 200000` (Xiaomi's recommended sampling for V2.6 Pro with thinking on). Until now air sent 0.0.

### A/B: 12 fix turns from one identical review

- **Setup.**
  - The review is base-2's, frozen (session entries 0–27, no compact entry), copied per replicate onto a fresh clone, then `air --resume "fix the findings"`.
  - **A** = old build (6e71980); A0 is base-2's own original fix turn.
  - **B** = new build.
  - **C** = B + a workflow line "one plan() step per item, each run fixes one item…".
  - **D** = B + temperature 1.0.
- All 12 turns were VERIFIED, with 5–8 files changed each.

| arm | requests (median, range) | cost index* | output | re-reads | wall | waste events |
|---|---|---|---|---|---|---|
| A old | 54 (33–54) | 999k (865k–1,328k) | 166k | 32–49 | 1,292 s | 5 |
| **B new** | **14** (14–23) | **500k** (261k–573k) | 68k | **0** | **511 s** | 3 |
| C new + prompt line | 25 (16–44) | 790k (684k–1,513k) | 97k | 0–2 | 864 s | 28 (26 = `edit` called as a tool, all rescued by routing) |
| **D new + temp 1.0** | 17 (13–22) | **444k** (418k–556k) | 66k | 0 | **448 s** | 2 |

*Uncached + cached/10 + 4 × output tokens.

- **The new build halves the fix turn's cost index, cuts requests about 4×, takes 60% less wall time and has no re-reads.**
  - Raw prompt tokens did not fall much (0.9–2.0M against 1.8–2.7M), because a larger context is replayed. It is replayed from cache, so the cost index and the requests are the right measures.
- **The nudge fired and worked.** In B-2, an empty reply was followed by a nudge, and the turn then finished VERIFIED.
- **Temperature 1.0 (D) had the tightest spread, the lowest median cost and the least waste.** With n = 3 it is not clearly ahead of B, but it is the vendor's setting, so it went into the profile rather than the code default. `air --stats` will show whether it holds in real use.
- **The prompt line (C) is rejected.** It was worse on every measure, and the model began calling helpers as tools.

## Question

Use the new tracing to find what would lower the variance ("entropy"), raise output quality, cut the number of turns, use fewer tokens, and get more from the cache.

## Data

- **Bench.** The 7-task bench was run ×3 on 6e71980.
- **Real flow.** The flow real use actually has, `review this app` then `air --resume "fix the findings"`, was run 6 times on a clone of air at 91d63d4 with `air.toml` (check = cargo build + clippy):
  - 3 runs on the current behaviour;
  - 3 runs with the stored `reasoning_content` never replayed (scratch build, `AIR_REASONING=none`).
- **Model.** MiMo V2.6 Pro via Primalabs.

### Bench (21 runs): at the floor

- **Outcome.** 21/21 pass. 81 requests: 3 per task for fix, debug and review; 4 for refactor and qa; 3–5 for rust; 6 for feature.
- **Tokens.** 223k prompt, 80% cached, 28k out.
- **Variance.** The spread between replicates is at most 2 requests per task.
- **Cache is at its ceiling.**
  - 0 breaks caused by air (no request had cached < 80% of the previous prompt).
  - Uncached tokens (44.6k) equal the tokens written to cache (44.0k): every uncached token is new content.
  - Even the first request of a session is about 98% cached across sessions (for example 1,392 of 1,416).
  - **There is no caching lever left. The lever is fewer and smaller prompts.**
- **Wall-time variance comes from the provider.** There were 12 HTTP 429s, and their `retry-after` waits of 20–46 s add up to about 430 s of the 669 s total. Run benches at concurrency 3 or lower, or report wall time net of the `wait_s` field. `timing.ms` includes these waits.

### Real flow: all the cost and variance are in the fix turn

| run | review: req / prompt / out | fix: req / prompt (cached) / out (reasoning) | fix: re-reads, compactions | fix verdict, wall |
|---|---|---|---|---|
| cur base-1 | 15 / 606k / 35k | 4 / 55k (29%) / 10k (49k) | 16 files re-read, 1 compaction at turn start | **NO CHANGES**, 558 s |
| cur base-2 | 9 / 249k / 26k | 33 / 1,831k (91%) / 166k (158k) | 32, turn start + mid-turn | VERIFIED, 1,292 s |
| cur cur-3 | 7 / 151k / 24k (1 empty) | 27 / 1,278k (89%) / 96k (86k) | 15, turn start + mid-turn | VERIFIED, 606 s |
| none-1 | 9 / 188k / 54k | 17 / 703k (98%) / 164k (155k) | 0, none | VERIFIED, 688 s |
| none-2 | 7 / 122k / 36k | 40 / 1,856k (96%) / 323k (309k) | 5, none | VERIFIED, 2,867 s |
| none-3 | 8 / 168k / 67k | 4 / 145k (51%) / 222k (258k) | 0, none | **NO CHANGES**, 3,972 s (4 stream cuts at ~590 s, 1 empty) |

The reasoning column also counts the reasoning tokens of empty replies, which their usage reports as 0 completion tokens. Fix-turn requests ranged from 4 to 40 and output from 10k to 323k across identical inputs. That spread is the entropy.

## Ranked findings

Scores use the scale from findings 17 and 19: the net value for air minus the cost in code.

### 1. Runaway deliberation is the dominant cost and the main source of variance — 85

- **Size.**
  - Across all 2,145 logged requests, reasoning is 76% of output tokens.
  - 5% of requests carry 78% of it; 0.7% of requests think for more than 16k tokens.
  - The largest completed requests in this round reached 51k tokens of reasoning (none-1, none-2). One stream that was cut (none-3) had thought for 597 s and 356k characters, and its usage was never reported.
  - The tail lives in "fix the findings": with 7–9 findings of mixed severity, the model designs every fix in its head before it acts.
- **Three ways it ends a turn badly.**
  - **Empty reply.** It ends with `finish: "stop"` after 13–25k reasoning tokens and 0 completion tokens. This is not a length cap. In base-1, air's identical resend thought for 25.5k tokens and came back empty again, and the turn ended NO CHANGES after 9 minutes.
  - **Stream cut.** The provider cuts the stream at about 590 s ("error decoding response body"), and `complete()` resends from the start. none-3 lost about 10 minutes twice.
  - **Wrong tool name.** After long thinking the model calls `plan` as a tool name (3 `bad_call` events).
- **Candidates, none built yet:**
  - **(a) Empty reply → host nudge instead of an identical resend.** Push a host user message, for example: "Your last reply was empty after long reasoning. Act now: one run() with the next concrete step." Cost: about 5 lines in `agent.rs`. The current retry is shown to fail the same way. Whether a nudge shortens the thinking is untested.
  - **(b) A workflow line for findings lists:** "fixing several findings: plan() one step per finding, then fix and check one per run; skip one that needs a design decision and say so". Test it from a frozen review (below) before adopting.
  - **(c) On a second stream cut after more than 300 s of thinking, nudge as in (a)** rather than resending a third time.
- **Do not cap `max_tokens`.** The empty replies end with `stop`, not `length`.

### 2. Turn-start compaction throws away the review that the fix turn needs — 75

- **Mechanism.** A review ends at 51–67k prompt tokens. That is at least `compact_at/2` (50k), so the next turn starts by compacting the reviewed code into a summary of about 2 KB.
- **Measured.** The fix turn then re-reads every file: 15–32 re-reads. It grows past 100k and compacts again mid-turn: 1.28–1.83M prompt tokens over 27–33 requests.
- **Comparison.** none-1's review ended at 34k, nothing compacted, and its fix turn had 0 re-reads at 98% cached: 703k prompt tokens over 17 requests.
- **Why it costs.** Keeping that context is cheap, because it is all cached. Compacting it costs re-read requests plus a new cache write.
- **The lever is `COMPACT_AT`, not the turn-start trigger alone.**
  - Raising only the turn-start trigger (for example to `compact_at * 8/10`) moves the compaction into the middle of the turn; it does not remove it.
  - Both VERIFIED current-behaviour fix turns crossed 100k mid-turn even though they started from a 2 KB summary. Starting from a 51–67k review context, they would cross it sooner, and the mid-turn cut has the same re-read problem.
  - The review→fix pair needs about 150–200k tokens of headroom, so `COMPACT_AT` has to sit near the model's real window.
  - The learned limit (0.9 × the last good prompt after the first overflow) already makes a too-high constant cost one failed request.
- **Unknowns.**
  - **MiMo's context window is unknown.** Primalabs' `GET /v1/models` lists only the id. Find it with one synthetic over-long request: the overflow error usually states the limit.
  - Long-context quality needs an A/B on the real flow.
- **Cost.** One constant, plus the turn-start trigger if the window allows both.
- **Dependency.** Item 3 was one accidental way of staying under the trigger. This is the direct way.

### 3. Never replaying `reasoning_content` — rejected for now — 20

- **Simulation.** On real sessions (`sim.py`), dropping all replayed reasoning cuts prompt tokens by 37–52% and uncached tokens by 50–70%.
  - The cheaper-looking middle grounds lose on the cache. Keeping only the newest message's reasoning, or stripping in epochs, saves 14–56% of prompt tokens. But they invalidate the cache from the stripped message on, so uncached tokens rise.
- **Measured.** Output did not fall; it rose:
  - fix-turn reasoning 155k / 309k / 258k with stripping, against 49k / 158k / 86k on the current behaviour;
  - 2 of 3 stripped runs hit stream cuts, and none-3 ended NO CHANGES after 66 minutes.
  - Without its earlier reasoning, MiMo appears to re-derive its plan.
- **Verdict.** n = 3 and the variance is huge, but nothing supports the change. Revisit only after item 1 tames the tail.

### 4. DONE: the newest direction's referent survives compaction — 70

- **Bug.** `summary()` gave every exchange 3,968 bytes, so a review of 4.9–7.1 KB (quoted) was cut before its later findings.
- **Evidence.** base-2's fix turn said: "The third smaller nit … was cut off in the review text I received".
- **Fix.**
  - Older exchanges keep their 3,968-byte cap. The newest exchange gets what is left of the same 3 × 3,968 total.
  - The dialogue section still fits its 16 KiB cap by construction.
- **Verified.** Replaying every real compaction changed only the 2 that cut a review, and kept those reviews whole.

### 5. DONE: `finish_reason` in the trace — 60

- Without it, the empty replies in item 1 could not be diagnosed: length cap, stream cut or a real stop.
- It is now in `meta.finish` and on `empty` events, at 0 prompt tokens, because `messages()` strips `meta`.
- **Remaining trace gap:** the usage and reasoning of an interrupted stream are not logged. A 590 s attempt leaves no trace except an `interrupted` event.

### 6. Smaller items — 10–25

- **Bookkeeping-only runs.** `plan()`+`accept()` alone happened in 2 of 21 bench runs, both in feature. It costs 1 request each (one spent 1,042 output tokens). Leave it.
- **Helper called as a tool.** The fix: map a tool call named after a helper (`plan`, `accept`, `show`…) to `run(code="<name>(**args)")`. It is a 3-run problem so far; the `bad_call` text already tells the model.
- **Dropped `sh()` output.** A bare `sh()` that is not the last statement loses its output: 1 bench session (feature-air-2). Printing every top-level expression statement's non-None value (IPython's "all") would fix it. It is rare.
- **Double checks.** The model ran `check()` 18 times and the host ran its check 15 times. This is trivial on these repos; on cargo/mix repos see findings/19's note on a host cache by fingerprint.
- **The 16k output limit.** Whole-file `show()` of agent.rs and ui.rs is cut in the middle, so every review spends 1 request re-reading the middle. A cleaner result needs the limit, not show(); leave it.

## Suggested next round

1. **Item 1(a), the empty-reply nudge** (a few lines), then find MiMo's window and set `COMPACT_AT` from it (item 2).
2. **Test the fix turn from a frozen review** instead of the whole flow, which costs 15–45 minutes per replicate and mixes review variance into fix variance:
   - copy a finished review session JSONL under a newer name;
   - point its `cwd` at a fresh clone;
   - optionally edit the header's `system` for prompt arms;
   - run `air --resume "fix the findings"`.
   Every replicate then starts from the same review. Use it for item 1(b).
3. **Measure each arm on fix-turn reasoning tokens, requests, re-reads and verdict.** n ≥ 3 per arm; the spread is 4–40 requests.

## Pi vs air, short rerun (new build, MiMo profile temperature 1.0)

7 bench tasks × 2 runs per agent, concurrency 3. Pi is Pi + Fabric. Both used the same MiMo model.

| | pass | requests | uncached in | cached in | out | cost index | median wall |
|---|---|---|---|---|---|---|---|
| air | 14/14 | 49 | 39k | 93k | 21k | 132k | 27.5 s |
| Pi | 14/14 | 117 | 127k | 467k | 29k | 288k | 52 s |

- Correctness is tied.
- air needs 2.4× fewer requests, 3.3× fewer uncached input tokens and about 2.2× less cost, and finishes in about half the time.
- Compared with findings 15, air went from about 6.6 requests per task (138/21) to 3.5 (49/14).
- These are small tasks. The long-turn gains (round 2) don't show on them.
