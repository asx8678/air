# 19 — Tracing: what air should record for benchmarking (2026-10-14)

## Status

Items 1–3 are implemented (uncommitted, on `audit-round-12-fixes`), plus the `shadowed` half of item 6.

- **Events.** Role-less entries `{"event", "text", "at", ...}` are logged, with kinds `retry` (`status`, `attempt`, `wait_s`), `interrupted` (`attempt`), `empty` (`usage`, `timing`), `overflow` (`seen`), `nudge`, `limit`, `bad_call`, `worker_restart`, `ctrl_c` and `error`. They are queued in `ui::event` and written by `Session::push`/`flush`.
- **Timing.**
  - Assistant entries carry `meta.timing = {ms, attempts, ttft_ms, think_ms}`.
  - Run entries carry `meta.ms` and `meta.chars`, the output length before truncation.
  - A `{"check": {ok, ms, out}}` entry follows the host's final check.
- **Outcome.**
  - `{"report": {verdict, changed, checked, criteria, plan_steps, plan_done, changes, requests, runs, prompt, cached, out, ms}}` ends each turn.
  - The header carries `"air": "<version> <git describe>"`, set by `build.rs`.
  - One-shot runs print `[session <path>]` on stderr.
- **Not done.**
  - The `gate` field (item 6): it can be derived from `actions[].error` starting with `call accept(`.
  - Items 4 and 5.
- **Scripts.** Scripts that read the logs must skip entries without `role`.

## Question

"What could we add to air so you can benchmark it: a mode or interface to opt into and see exactly what happens in the app — model calls, tools, errors, model efficiency?"

## Short answer

- **Most of the needed data is already computed, but air either prints it or throws it away.** The session JSONL (`~/.air/sessions/*.jsonl`) has messages, per-request `usage`, and run `meta` (`actions`, `exception`, `rejected`). It has no timing, no retries, no error events and no outcome.
- **Logging should be always on, not a mode.** `Session::messages()` strips `meta` and skips entries that have no `role`. `compact::cut()` and `summary()` skip them too. So extra fields and role-less event entries cost **0 prompt tokens** and keep air's 1.2k-token edge (findings 15).
- **Only two things deserve an opt-in:**
  - a raw wire dump (`AIR_TRACE=<dir>`), because it is large;
  - a viewer (`air trace`), because it is UI.

## What the benchmark and wishlist scripts had to do without

Findings 15 and 17 were built by scraping stdout and re-deriving facts from the JSONL. These are the gaps:

| Fact | Where it lives today | Lost? |
|---|---|---|
| 429/5xx retries, wait seconds (`llm.rs` `send`) | `ui::note` → stdout | yes; findings 15 counted 429s from captured stdout |
| Stream interrupted + reason + attempt (`complete`) | `ui::note` | yes |
| Empty reply asked again (`agent.rs` `turn`) | `ui::note`, and the empty message is not pushed | yes; the request's usage is lost too |
| Context overflow → compaction | `ui::note` + a `compact` entry | the cause is lost |
| Check-failed nudge | host `user` entry | kept |
| Check that passed: output, duration | printed only | yes |
| `run()` without code, wrong tool name | `ui::note` + error meta | partly |
| Worker crash/restart | tool content text | only as prose |
| Request latency, time to first token, thinking seconds | `Instant`s in `llm.rs`, dropped | yes |
| `run()` duration | not measured | yes |
| Output size before truncation | `G["_last"]` only | yes; findings 17 inferred truncation from the marker text |
| Final verdict, changed files, turn totals (`report`) | printed | yes |
| Which session file a one-shot run wrote | not printed | harness matches on the `cwd` header |
| Build that produced a session | not recorded | yes; findings 17 had to group "by build" from the prompt text |

## Ranked suggestions

Scores use the same scale as findings 17: the net value for measuring air, minus the cost in code. Every item is host-side or worker-side. None touches the prompt, the tool schema or tool-result text, and none needs a sandbox change.

### 1. Record every `ui::note` as an event entry — 80

- **What.** `ui::note` is the single place where retries, interruptions, empty replies, overflows, nudges, malformed calls and "stopped after N requests" all pass. Each note gets queued in a static `Vec`, and the agent drains the queue into the session as `{"event": "<text>", "t": <secs since turn start>}` after each request and run.
- **Better variant.** Give the call sites a `kind` (`retry`, `interrupted`, `empty`, `overflow`, `nudge`, `bad_call`, `limit`, `worker_restart`). A script can then count kinds without parsing English. 429 retries also get `status` and `wait`.
- **Also fixes:** the usage of an empty reply that is retried. Today that request is billed but never logged. The fix logs its usage on the event.
- **Cost.** About 20 lines across `ui.rs`, `llm.rs` and `agent.rs`. 0 prompt tokens.

### 2. Timing on every request, run and check — 75

- **Request.** `meta.timing = {ms, ttft_ms, think_ms, attempts}`. `llm.rs` already keeps `began`; add a start `Instant` in `complete()`.
  - With `completion_tokens`, this gives decode tokens/second per request. That is the "model efficiency" number, and it separates a slow provider from a model that thinks too much.
- **Run.** The worker adds `ms` (wall time of `exec`) and `chars` (output length before truncation) to `_report`. That is one line near `worker.py:322`.
  - `chars > 2*HALF` replaces scraping for the "omitted" marker.
- **Check.** `verify()` records `{check: {ok, ms, out_tail}}` on pass as well as fail.
- **Cost.** About 15 lines.

### 3. Outcome as data, and the session path on stdout — 70

- **What.** `report()` pushes `{"report": {verdict, changed, checked, criteria, plan_done, plan_steps, changes, requests, runs, prompt, cached, out, secs}}`.
- **Header.** The session header gains `"air": env!("CARGO_PKG_VERSION")` plus the git short hash at build time (via `option_env!`, or a 3-line `build.rs`). Sessions can then be grouped by build exactly.
- **One-shot runs** print `[session <path>]` on stderr at start. A harness then reads one file and never scrapes stdout or guesses by `cwd`.
- **Cost.** About 15 lines. This turns `bench_run.py`/`final.py` into a plain JSONL reader.

### 4. `air trace [session]` — a per-request table — 55

- **What.** Read the JSONL (the latest for this repo by default, or a path) and print one row per request:
  - `#`, `ms`, `ttft`, `in`, `cached`, `out`, `reason`, `tok/s`;
  - the tool calls, with each run's `ms`, `chars`, `exception`/`rejected`;
  - events inline.
- **Footer.** Turn totals and the verdict, plus a rollup of exceptions by name and events by kind.
- **Old sessions.** Missing fields print `-`, so the viewer works on all 530 existing sessions.
- **`--json` flag.** Emits the same rows as JSONL for scripts.
- **Cost.** About 80 lines, the largest item. The no-code alternative is a 40-line Python script kept beside the bench harness. Pick that if the code budget matters more than having it at hand in the REPL.
  - A cheaper middle ground: extend `/cost` with timing, retry and exception rollups (about 15 lines).

### 5. `AIR_TRACE=<dir>` — raw wire dump — 45

- **What.** When set, write each request body and the raw SSE bytes to `<dir>/<session>-<n>.req.json` / `.sse`.
- **Why.** It is the only way to debug, after the fact:
  - provider quirks;
  - the tool-call merge logic in `stream()` (`slots`/`fresh`/`marked`);
  - `reasoning` vs `reasoning_content`;
  - usage that arrives late or never.
  - It also shows exactly what `messages()` sent after a compaction.
- **Why opt-in.** It is big (the full prompt on every request) and holds file contents.
- **Cost.** About 12 lines in `llm.rs`.

### 6. Event when a helper is shadowed or a gate blocks — 35

- **What.** The worker already prints `[accept: air's helpers, restored…]` and the gate errors. Put `shadowed: [...]` and `gate: "accept"` into `_report`.
- **Why.** Findings 15's 28-request trap would then show as a count rather than needing a manual read.
- **Cost.** 2 lines in `worker.py`.

## Not recommended

- **A separate trace file or a tracing/OTel crate.** It would duplicate the session JSONL, which is already an append-only, per-session event log, and it would add dependencies. Extend the JSONL instead.
- **Verbose mode that prints more to the terminal.** Benchmark runs are read by scripts, not by eye. The live bar already shows activity.
- **Tracing inside the prompt (showing the model its own timing or cost).** It costs tokens on every request and has no evidence of helping.

## Suggested order

1. Items 1–3 in one change (about 50 lines). They make every future benchmark a plain JSONL query.
2. Item 6 alongside (2 lines).
3. Item 5 when the next provider or stream bug appears.
4. Item 4 (or the `/cost` extension) once 1–3 have produced data worth looking at.

## Note on the harness

`bench/` and `wish/` live in session 742fae11's scratchpad under `/private/tmp`, and macOS clears that folder. If benchmarks are going to be repeated, copy the harness somewhere durable outside the repo, for example `~/projects/air-bench`.
