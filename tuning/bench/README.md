# air bench

A reproducible regression suite for air's harness: 15 tasks, each run N times per variant, graded by exit code.

```sh
make bench VARIANT=HEAD N=5                    # air at a git ref (built once per commit), all tasks
make bench VARIANT=my-branch N=5 TASKS=fix,wide
make bench VARIANT=opencode N=1                # OpenCode itself, same model, same graders
make report BASE=<label> VARIANT=<label> OUT=report.md
```

- **Model:** `deepseek-v4.1-flash` on the OpenCode Go provider from `~/.config/air/providers.toml` (`MODEL=` to change).
- **Labels:** a run's label is the variant's 10-character commit id unless `LABEL=` is given.
- **Results:** one JSON row per run in `results.jsonl`. Scratch repos, logs and diffs live under `$AIR_BENCH_RUNS` (default `$TMPDIR/air-bench/runs`), and build caches under `$AIR_BENCH_CACHE`.

## Tasks

| task | kind | what it tests |
|---|---|---|
| fix, debug, rust | bug fix | failing behaviour or failing tests; graded on hidden cases |
| feature | feature | a CLI flag with sorting and back-compat |
| refactor | refactor | a rename across code, tests and docs, plus a deprecated alias |
| qa, airqa | question | read-only answers checked against a key, with file:line citations; airqa is about air itself |
| review | question | finding 5 of 6 planted bugs |
| ops | ops | a shell pipeline over plain and gzipped logs, run in the sandbox; graded on hidden logs |
| wide | wide read | a config table that needs 16 files read; graded row by row |
| recovery | recovery | unary minus: the obvious tokenizer change breaks binary minus |
| c-window, c-compact-at, c-overflow, c-python | real commit | air's parent tree for 35a5383, b26214f, d4ccc7e and 5050a96, with the commit's intent as the task; graded by the commit's tests appended as a hidden module and run with `cargo test` |

Each `tasks/<name>/` has `task.json` (prompt, timeout, kind), `setup.sh <dest>` and `grade.py <repo> <answer.md>` (exit 0 = pass). Every task repo starts as a fresh one-commit git repo, so the agent can't look the answer up in history. The real-commit tasks start from a warm `target/` that the runner builds once per parent commit.

## Per-run fields

pass, score and detail; turns (model requests, cut streams included); tool calls in total and per helper; tool errors, rejected runs and failed edits; prompt, cached, uncached and output tokens; reasoning tokens; the cache-hit rate of each request; the nominal cost (`PRICES` in bench.py, since Go returns none); wall and model seconds; and the final diff's sha, size and files.

The report adds, per task and across N runs:
- pass rate;
- standard deviation of turns and of tokens;
- mean pairwise normalized edit distance between the final diffs;
- the number of distinct outcomes (distinct diffs, or grader scores for read-only tasks).

Pooled comparisons come with a stratified bootstrap 95% CI.

## Safety

- **air** runs only in its own sandbox. The bench never passes `--yolo` and never changes `HOME`, because air's secret deny list resolves through `HOME`.
- **OpenCode** runs under a `sandbox-exec` profile built from the same secret list as air's. Network is on (it has to reach the gateway). Writes are limited to the run repo, temp and OpenCode's data dir.
  - `--standalone` keeps its tool server inside that sandbox; without it, the shared background service ran tools unsandboxed in another directory.
  - It gets a scratch `HOME` that links only its data dir and `~/.gitconfig`, because it otherwise reads `~/.claude`, which the sandbox hides.
