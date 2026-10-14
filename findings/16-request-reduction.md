# 16 — Reducing air's request count (2026-10-14)

## Where the requests went

The 21 air benchmark runs from findings 15 used 138 requests; Pi + Fabric used 95. Classifying every air request by what its code did:

| Cause | Requests | Pi + Fabric |
|---|---:|---|
| `accept()` / `plan()` / `done()` alone in a run | 15 (11%) | none: no bookkeeping tools |
| One change's edits split across runs | about 4 | all edits in one program |
| First request only lists files | 18 of 21 runs | also lists first |
| Final answer | 1 per task | 1 per task |

Pi + Fabric's pattern on a change task:

1. list files
2. read every relevant file
3. make every edit
4. test and probe
5. answer

## Variants (MiMo, hidden checks from findings 15)

- **P (prompt):** Workflow step 2 becomes "in one run: accept(criteria), every edit the change needs, and check()". Add `plan()` there, put `done()` in the run that finishes each step, and never spend a run on `accept()`, `plan()` or `done()` alone.
- **L (system prompt):** append the `git ls-files` list with byte sizes at session start. Above 200 files, show a two-level folder summary with counts instead (universe 54 lines, konstruct 74 lines).
- **PL2:** PL plus "`if check(): done()`" for the last step.

### Round 1 (6 tasks × 3 runs per build)

| | Current (`4047dea`) | P | PL |
|---|---:|---:|---:|
| Passed | 18/18 | 18/18 | 18/18 |
| Requests | 108 | 94 | **77 (−29%)** |
| Tokens in | 293k | 250k | **198k (−32%)** |
| Uncached tokens | 55k | 51k | **46k** |
| Time | 23.3 min | 20.0 min | **15.9 min** |
| First request only lists files | 15/18 | 15/18 | 2/18 |
| Rejected runs | 3 | 5 | 0 |

Per task, mean requests:

| Task | Current | PL |
|---|---:|---:|
| feature | 9.0 | 4.7 |
| refactor | 8.0 | 4.7 |
| review | 5.7 | 2.7 |
| debug | 4.7 | 3.7 |
| fix | 4.0 | 5.3 |
| qa | 4.7 | 4.7 |

PL's fix runs spent their extra requests on `done()` alone right before the answer. P's rejected runs were MiMo noise: `import sh_helper if False else None` in the first run.

### Round 2 (4 change tasks × 3 runs per build)

| | PL | PL2 |
|---|---:|---:|
| Requests | 50 | 47 |
| Tokens in | 118k | 113k |

The difference is within noise, so the `done()` wording was dropped.

## Shipped (uncommitted)

PL, with PL2's folder summary for repos over 200 files. On the benchmark repos this is exactly the tested PL, because the summary only applies above 200 files.

- **Code:** `src/agent.rs` +20 lines (`FILES = 200`) and `src/prompt.md` Workflow steps 2–4.
- **Checks:** `cargo build` and clippy clean, rustfmt clean.
- **Folder summary:** checked on a synthetic 301-file repo (4 lines).
- **End-to-end:** a feature run with the repo build came back VERIFIED, hidden check 8/8, in 4 requests and 15.7k tokens. The same task averaged 9 requests and 30k tokens on the current build, and 6 requests and 54k tokens on Pi + Fabric.

PL's 77 requests for these 6 tasks are below Pi + Fabric's roughly 83 on the same tasks (findings 15).

## Caveats

- **Context grows.** The file list adds to the system prompt (mostly cached): about 1 KB for universe and 2 KB for konstruct. It is written once per session, so a resumed or long REPL session sees the list from its start.
- **The first `git ls-files` habit sometimes remains** (the end-to-end run above still listed first).
