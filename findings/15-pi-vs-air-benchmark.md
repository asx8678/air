# 15 — Benchmark: Pi vs air on MiMo V2.6 Pro (2026-10-14)

## Setup

- **Model.** Both tools use `primalabs/primalabs-ai/MiMo-V2.6-Pro`.
- **air.** The working-tree build of `audit-round-12-fixes`: the V1 prompt from findings 14, plus the uncommitted `worker.rs` socket rule. It runs one-shot, sandboxed, with `air.toml` `check` set to the repo's test command.
- **Pi.** Pi 1.2.0 as you run it, `pi -p --approve`, with your global packages, including pi-fabric 0.104.1 (full code mode, TypeScript kernel) and the default thinking level `low`.
  - Each run repo gets `.pi/settings.json` with `{"retry": {"enabled": true, "maxRetries": 5}}`. Your global settings turn retries off, and air retries up to 5 times.
  - Pi runs unsandboxed.
- **Tasks.** 7 tasks, 3 runs per tool, 42 runs, at most 3 running at once.
  - Each task is its own small git repo with the same `AGENTS.md` (naming the test command) and `air.toml` for both tools.
  - Scoring is by hidden checks the agents never see. Every check was validated to fail on the untouched repo and pass on a reference solution.
- **Harness.** `bench/` in session 742fae11's scratchpad: `make_bench.py`, `check_bench.py`, `validate_bench.py`, `bench_run.py`, `final.py`.

| Task | What it asks | Hidden check |
|---|---|---|
| fix | `median()` wrong for even lengths and mutates its input | 4 behaviours + tests pass |
| feature | Priorities for a todo CLI across `cli.py`/tests, with a numbering subtlety | 6 CLI behaviours + tests pass + tests updated |
| debug | Scheduler whose tests fail; fix code, not tests | 9 cases (6 hidden) + tests pass + test file unchanged |
| refactor | Rename `calc_total`→`order_total` in code, tests and README; keep a deprecated alias | No old name in callers/tests/README except as deprecated; alias warns; tests pass |
| rust | `cargo test` fails in a std-only crate | 4 visible + 4 hidden tests; tests unchanged |
| qa | 4 questions about Python 3.9's `logging` (174 KB) with `file:line` | Exact facts + citations within 15 lines of the true line |
| review | Inventory service with 6 planted bugs; don't change files | Each planted bug named; no files changed |

Two check fixes were applied to all runs alike before scoring.

- **refactor** first demanded zero `calc_total` outside `pricing.py`. Both tools rightly kept a test of the deprecated alias, and Pi also kept a README note, so the rule became "callers, tests and README use `order_total`; README mentions of `calc_total` say deprecated".
- **qa citations** now accept a line range such as `1194-1212`.

## Results (medians per run; pass counts out of 3)

| Task | air pass | Requests | Tokens in | Time | Pi pass | Requests | Tokens in | Time |
|---|---|---:|---:|---:|---|---:|---:|---:|
| fix | 3/3 | 5 | 7.6k | 28 s | 3/3 | 5 | 38.0k | 36 s |
| feature | 3/3 | 9 | 27.7k | 108 s | 3/3 | 6 | 54.0k | 50 s |
| debug | 3/3 | 4 | 8.9k | 48 s | 3/3 | 4 | 31.0k | 44 s |
| refactor | 3/3 | 7 | 16.8k | 58 s | 3/3 | 5 | 43.0k | 55 s |
| rust | 3/3 | 4 | 7.7k | 29 s | 3/3 | 4 | 31.0k | 39 s |
| qa | 3/3 | 4 | 11.4k | 49 s | 2/3 | 3 | 31.2k | 49 s |
| review | 3/3 | 5 | 11.5k | 30 s | 3/3 | 4 | 32.1k | 36 s |

| Total (21 runs each) | air | Pi |
|---|---:|---:|
| Passed | **21/21** | 20/21 |
| Requests (excluding failed retries) | 138 | **95** |
| Tool calls | 124 | **74** |
| Input tokens | **0.50M** | 0.78M |
| Cached share | 87% | 78% |
| Uncached input tokens | **64k** (median 2.3k/run) | 174k (median 8.4k/run) |
| First-request prompt (system + tools) | **1.2k** | 6.9k |
| Output tokens | 42k | 37k |
| Wall time | 20.2 min (median 48 s/run) | 16.8 min (median 49 s/run) |
| Rate-limit (429) hits, retried | 21 | 57 |

## What it shows

- **Correctness is a tie.** Pi's one miss was a genuine misread: it answered `%(message)s` as `basicConfig()`'s default, but `_STYLES['%'][1]` is `BASIC_FORMAT`. Both tools found all 6 planted bugs in every review and changed nothing on the read-only tasks.
- **air is about 2.7× cheaper in full-price input tokens.** Pi sends about 6.9k tokens of system prompt and tool schema with every request, against air's 1.2k. This holds on every task, even the feature task, where air needed more requests.
- **Pi needs fewer requests.** Each Fabric program does more per call. On the multi-file feature task Pi was clearly faster: a 50 s median against 108 s, and 6 requests against 9.
- **Wall time is even per run** (48 s against 49 s median). air's total (20.2 min against 16.8) includes one 251 s outlier, the trap described below. Pi's includes the waits from 57 rate-limit retries.

## air bug found by the benchmark (fixed in `4047dea`)

`feature-air-1` passed but cost 28 requests, 234k tokens and 251 s, against 8–9 requests and about 27k tokens for the other two air runs.

1. At step 3 the model wrote its own `def accept(criteria): print(...)` and `def plan(...)` stubs into the persistent worker. These replaced air's helpers.
2. Every `edit()` then failed with "call accept(criteria) before changing files".
3. The model spent about 20 runs inspecting `accept`'s source, globals and bytecode.
4. At step 24 it bypassed the gate with `edit.__globals__["_state"]["accepted"] = ...`.

This is rare: 3 of 91 air sessions ever redefined a helper. It is expensive when it hits `accept`.

Fixed in `4047dea`. Before every run the worker restores its working directory to the repo root (`os.chdir(ROOT)`) and reinstalls the helpers (`G.update(TOOLS)`). A run that shadows a helper gets one line: `[accept: air's helpers, restored before the next run]`. Checked by driving the real worker with JSON requests (chdir+shadow, restore, edit, `import sh`, variable shadowing) and one real one-shot run (VERIFIED, hidden check 5/5). The bypass also shows the worker-side gate is not a security boundary; only the host's own check decides VERIFIED.

## Caveats

- **The tasks are small.** All are solvable by either tool. A harder or longer task set would separate them more, especially on context growth over long REPL sessions (not tested here).
- **Small sample.** One model, 3 runs per task.
- **Rate limits.** Pi hit more 429s (57 against 21). That adds retry waits to its time, not to its tokens.
- **Not identical conditions.** Pi ran with your extensions and thinking level `low`; air sends no reasoning setting. Pi is unsandboxed with network; air is sandboxed.
- **air's prompt was tuned today on similar tasks.** Its review/question line was tested on review and question tasks, and this benchmark has a review and a qa task. Pi's prompt is generic. Read those two rows with that in mind. 429 entries carry no token usage, so Pi's token totals are unaffected by retries.

## Third arm: plain Pi (`--no-extensions`), same 7 tasks × 3

Run after the main benchmark, 2 at a time, beside two air review runs. Same retry settings.

Grading change applied to every arm: the Rust "tests kept" check now ignores whitespace. Plain Pi ran `cargo fmt`, which only re-wrapped two `assert_eq!` lines in the test module.

| Total (21 runs each) | air | Pi + extensions (Fabric) | Plain Pi |
|---|---:|---:|---:|
| Passed | **21/21** | 20/21 | **21/21** |
| Requests | 138 | **95** | 131 |
| Tool calls | 124 | **74** | 156 |
| Input tokens | **0.50M** | 0.78M | 0.94M |
| Uncached input tokens | **64k** (2.3k/run) | 174k (8.4k/run) | 170k (6.9k/run) |
| First-request prompt | **1.2k** | 6.9k | 5.7k |
| Wall time | 20.2 min | **16.8 min** | 20.4 min |

Per-task medians (requests / tokens in / seconds) are in `bench/final3.txt`.

- **Pi's overhead is mostly Pi itself.** Its base prompt and tool schemas are 5.7k of the 6.9k; Fabric adds about 1.2k.
- **Fabric makes Pi more efficient.** With it, Pi needed 27% fewer requests and 17% fewer tokens, and finished faster. Its code mode batches tool calls.
- **air stays the cheapest.** It used about 2.7× fewer uncached tokens than either Pi setup, with equal correctness.
- **Times are not strictly comparable.** The arms ran under different rate-limit load.
