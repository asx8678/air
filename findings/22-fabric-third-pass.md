# 22 — Pi Fabric, third pass: what still fits air exactly (2026-10-14)

The filter this time is stricter than in findings 13 and 21. A candidate must:

- live on the host or in the worker;
- be deterministic, with no LLM call;
- add no tool and no prompt text that costs tokens on every request;
- target a problem that air's own logs show.

The evidence is all 332+ logs in `~/.air/sessions`; the Fabric source is `e22cc59` (v0.116.1).

## Result

One Fabric idea passes the filter and is worth building: **host-run, tree-bound check certificates** (Fabric `state.verify` plus `state/evidence-runner.ts`). A second, small one fits the original "lower the entropy" goal: **spread per setting in `--stats`**, in the spirit of Fabric's entropy meter. Everything else that is left either has no evidence in air's logs or does not fit a one-tool CLI.

## 1. Check certificates — 55 (repos whose check runs tests) / 10 (compile-only checks)

**Fabric.** The model asks for verification and the host runs the evidence commands itself (`evidence-runner.ts`: spawn, timeout, output digest). The host binds the certificate to its own observation of the repo (`binding.ts`: HEAD and dirty status) and fails closed when the state differs. Its guidance adds: "Inspect failures and iterate instead of rerunning unchanged passing checks."

**air today.** The model's `check()` runs inside the sandboxed worker. The host cannot trust it, because model code shares that Python process and once reached the gate state through `edit.__globals__` (findings 15). So the host runs the check again on the final tree.

**Evidence.**

- In **224 of 242 changing turns (93%)**, the model's `check()` had already passed after its last edit, so the host's run repeated it on the same files.
- The cost of that repeat depends on the check:
  - air's own check (`cargo build` + clippy) re-runs in **0.3 s** on an unchanged tree, because cargo caches it;
  - bench `unittest` checks take 140 ms (median);
  - konstruct's check includes **7,751 tests at about 200 s**, and tests are not cached, so **every changing turn spends an extra ~200 s**.

**Fit.**

- `check()` in the worker sends a check request to the host over the existing stdin/stdout protocol, instead of running the command itself.
- The host runs its existing `verify()` (its own sandboxed process group, with a tree-change guard), records `(fingerprint, ok, output)` and answers the worker. The worker prints the output exactly as today.
- At the end of the turn the host reuses the certificate when the fingerprint is unchanged. The log records `{"check": {..., "reused": true}}`; otherwise the check runs as now.

**Effects.**

- One check per changed tree instead of two. The model's own `check()` results become host-verified and tamper-proof.
- The verdict wording is unchanged.
- No prompt change.

**Cost.** About 40 lines: a request loop in `Worker::run`, the certificate on `Agent`, and a 6-line `check()` in the worker.

**Care needed.** Ctrl-C while the host check runs:

- the worker is blocked reading the answer;
- the terminal's SIGINT makes it raise `KeyboardInterrupt` (already the behaviour during a run);
- on Linux (bwrap, where Ctrl-C does not reach the worker) the worker must be restarted.

## 2. Spread per setting in `--stats` — 30

**Fabric.** `entropy/meter.ts` computes Shannon entropy over tool-call shapes, repairs and retries. In air, call-shape entropy is near zero by design: one tool with one string argument, and 0 misuse repairs. The entropy that remains is behavioural: how much a turn's requests and cost vary for the same kind of task. Across six identical fix turns that was 4–40 requests (findings 20).

**Fit.** `--stats` already groups turns by model and settings. Add a spread to each group:

- requests per turn as p10 / median / p90;
- cost per turn the same way.

A setting that lowers the variance (for example temperature 1.0) then shows up directly. This adds about 10 lines in `stats.rs` and 0 prompt tokens.

## Checked and rejected this pass

| Fabric idea | air evidence | Verdict |
|---|---|---|
| Filesystem-drift detection (`prewalk/fs-drift.ts`) | In 101 reported turns, tracked files changed outside `edit`/`write` 6 times, all legitimate (`Cargo.lock` from builds) or test-harness logs. The model never edits source through the shell. | Nothing to catch |
| Entropy normal forms, learned repairs, schema enforce | one tool, one string argument, 0 misuse repairs | No fit |
| Prewalk handoff, child agents, supervisors, mesh, Jev, decisions, saved programs | single model, single session | No fit (findings 21) |
| Search in `recall()`, read files in the summary | A/B: no benefit, never used (findings 21) | Removed |

## Other improvements (not from Fabric), ranked

1. **Overlong thinking remains the biggest cost** (76% of output tokens). The new per-model `guidance` key makes it cheap to test MiMo-only lines with the frozen-review A/B (findings 20 harness), for example "decide quickly: take the simpler approach and verify by running it". Untested.
2. **Scratch files left in the repo: 9 of 101 turns.** Examples: `tmp/` probe repos (372 files in one run), a `mkdtemp(dir=".")` folder in the repo root, an `exports/` probe directory. Options, cheapest first:
   - a report row counting new untracked files;
   - a one-time host nudge, like the check gate, asking the model to delete its scratch files (one request when it fires);
   - a prompt line.

   Not from Fabric.
3. **Output clarity against Pi** (the earlier comparison): a final-answer contract (fix suggestions for review findings, no references to run output the user cannot see) and a review checklist, plus a clarity score in the bench. Untested.
4. **Pi CLI features:** steering while it works, `!cmd`, prompt templates, `/new` with a session picker, `/out`, `/copy`, a retry on a length-cut reply.
