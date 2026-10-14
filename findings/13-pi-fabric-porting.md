# 13 — Pi Fabric → Air porting analysis (scored)

**Date:** 2026-10-14 · **Air:** branch `audit-round-12-fixes` @ `c74ee90` (2.2k LOC: 1.9k Rust + 277 Python + 46-line prompt) · **Pi Fabric:** `main` @ `c538d3e` v0.116.0 (~127k LOC TypeScript, 509 files)

## Bottom line

Air's compactor is already a lean port of Fabric's deterministic compactor: same section layout, 32 KiB cap, three protected recent exchanges, "not a verified outcome" labels, carry list, exact-identity failure resolution, closure-safe cut, orphan repair, log as ground truth, `recall(n)` by entry id. **Focus Area 1 is about 85% done.** The rest of Fabric is mostly runtime (multi-agent, mesh, QuickJS/Monty kernels, Bend proofs, speculation) that does not fit a 2k-line single-tool CLI.

The ports that actually pay off are small: **two to four prompt lines, an atomic multi-edit, and a per-turn outcome row in the summary.** Everything below a score of 20 should be skipped.

## Inventory

| | Air | Pi Fabric |
|---|---|---|
| Entry point | `src/main.rs` (clap REPL / one-shot) → `Agent::run` → `turn()` loop | `src/index.ts` / `src/extension.ts` register a Pi extension; `pi-fabric` CLI in `src/cli/` |
| Model I/O | `src/llm.rs` (one OpenAI-compatible SSE client with retries) | Pi host plus `src/providers/` (MCP, memory, agents, mesh, state, …) |
| Session state | `src/session.rs` (append-only JSONL, flock, `messages()`, `state()`) | Pi session JSONL plus `src/state/`, `src/storage/`, `src/memory/` |
| Compaction | `src/compact.rs` (208 lines) | `src/compaction/` (17 files: normalize, projections, render, hook, carry, qa, …) |
| Code execution | `src/worker.rs` (sandbox) + `src/worker.py` (persistent CPython worker, helpers) | `src/fabric-exec-tool.ts`, `src/execution-service.ts`, `src/runtime/` (QuickJS, Monty, CPython), `src/speculation/` |
| Guidance | `src/prompt.md` plus the repo's `AGENTS.md` and `.agents/skills` descriptions | `promptGuidelines` in `fabric-exec-tool.ts`, `src/components/` guidance slots, `skillsets/{python,typescript}/*/SKILL.md` |
| UI | `src/ui.rs`, `src/render.rs` | `src/ui/` (100 files: dashboard, topology, child chats) |
| Unique to Fabric | — | `src/entropy/`, `src/repairs/`, `src/jev/`, `src/agents/`, `src/actors/`, `src/mesh/`, `src/verified/` + `proofs/*.bend` |

There is no `.kiro/steering/` directory in either repo. Fabric's steering lives in the `promptGuidelines` array in `src/fabric-exec-tool.ts:173`, the `skillsets/*/SKILL.md` files, and its developer `AGENTS.md`.

## Evidence base

From `~/.air/sessions` (21 sessions, 246 `run` calls, 23 `edit` calls):

| Signal | Value | What it says |
|---|---|---|
| Rejected runs (syntax, unknown name, helper misuse) | **0** | Fabric's entropy, normal-form and repair machinery has nothing to fix |
| Longest run of identical consecutive code | **1** | A repeat guard is insurance only |
| `edit` failures (not found, multiple matches) | **0** | Edit mechanics are sound; the gap is atomicity, not matching |
| Run errors | 10 (8 × `ModuleNotFoundError`, of which 5 were `import sh` and are already fixed) | — |
| Outputs truncated by head/tail | 25 of 246 (10%), the largest by 119,946 chars | Output filtering in the prompt is worth tightening |
| Host check-gate nudges | 1 | The verify gate works but rarely fires |
| Peak prompt | 73k tokens (`COMPACT_AT` is 100k) | **Compaction has never fired in a real session.** Compaction items are scored on risk, not on observed failures |
| `compact::cut` estimate ÷ real `prompt_tokens` | **0.90–0.94** (69 samples above 20k) | Fabric's token calibration would add almost nothing |

The sample is small and from one model (MiMo V2.6 Pro). A weaker model would raise the value of the loop and rejection guards.

## Scoring rubric

`Score = Benefit × Fit × Cost`, rounded:

- **Benefit (0–100):** expected improvement to Air's results, discounted when the evidence above shows no need.
- **Fit (0–1):** how well the item respects Air's constraints: LLM-free compaction, one `run(code)` tool, no tests, no comments, Python 3.9 worker, recent sandbox decisions, tight LOC.
- **Cost (0.6–1.0):** about 10 LOC = 1.0, 25 LOC = 0.9, 60 LOC = 0.75, 200+ LOC = 0.6.

## 1. Executive summary: capability matrix

| Capability | Pi Fabric | Air | Gap |
|---|---|---|---|
| Tool surface | One `fabric_exec` (TS or Python), flat schema plus `payloads` map | One `run(code)`, Python 3.9 stdlib, persistent worker | none |
| Execution isolation | QuickJS / Monty sandbox; CPython opt-in under sandbox-exec/bwrap | sandbox-exec / bwrap around a CPython worker, egress denied, secrets unreadable | none (Air's is tighter for its scope) |
| Variables across calls | Fresh per call; `store`/`load` helpers | Persistent globals, `_last` holds full output | Air ahead |
| File mutation | `pi.edit(edits=[...])` atomic multi-replace, `pi.write` | `edit(path, old, new)` single, `write`, formatter diff fed back, accept-gate | **multi-edit atomicity** |
| Verification | Acceptance ledger in guidance, `state.verify`, Schema enforce | `accept()`, `check()`, host re-runs the check on the final tree, VERIFIED/UNVERIFIED report | Air ahead on the host gate; Fabric's guidance asks for behavioural probes |
| Compaction engine | Deterministic, log-derived, 32 KiB, 10 bounded blocks | Deterministic, log-derived, 32 KiB, 9 bounded blocks | ~equal |
| Token budgeting | Calibrated against `tokensBefore`, occupancy ceiling, output reserve | bytes ÷ 4 (measured 0.90–0.94 accurate); learns `compact_at` from an overflow | negligible |
| Per-turn history in summary | `[Earlier Turns]`: one-liner, tool counts, latest named run + outcome | "Earlier requests" one-liners only; turn verdict never logged | **turn outcome row** |
| Intent labels | `display.name/description` paired with outcome | first non-import code line | small |
| Model-driven compaction | `compact.request` / `carry` / `pressure` / `cancel` | `carry()` only; `/compact` for the user | small |
| Exact recall | `memory.expand` by entry id plus BM25 `memory.recall` search | `recall(n)` by entry id | **search** |
| Orphan tool-result repair | yes | yes (`answer_missing`, unmatched results dropped) | none |
| Loop breaker | Warn at 3 identical runs, block at 6 | `MAX_REQUESTS = 200` per turn | insurance only |
| Rejection metrics / argument repair | Entropy meter, schema normal forms, learned aliases | Rejected count in `/cost`, `_hint()` with signature | not needed (0 rejections) |
| Background jobs | Detach with a pid and a live output path | Process group killed after each `sh()` (a deliberate security fix) | conflicts with a decision |
| Multi-agent / mesh / advisor | Extensive | none | out of scope |

## 2. Ranked candidates

| # | Candidate | Area | B | F | C | **Score** | Verdict |
|---|---|---|---:|---:|---:|---:|---|
| 1 | Verification-ledger prompt lines (S1) | steering | 70 | 0.95 | 1.0 | **67** | **Do** |
| 2 | Atomic multi-edit `edit(path, [(old, new), …])` (M2) | code mode | 60 | 0.95 | 0.95 | **54** | **Do** |
| 3 | Per-turn outcome row in the summary (C4) | compaction | 55 | 0.95 | 0.85 | **44** | **Do** |
| 4 | Recovery line: applied edits stay applied (S2) | steering | 40 | 1.0 | 1.0 | **40** | Do (one line) |
| 5 | Filter-output prompt line (S3) | steering | 40 | 1.0 | 1.0 | **40** | Do (one line) |
| 6 | `recall("regex")` search over the session log (C9) | compaction | 45 | 0.9 | 0.95 | **38** | Worth it once compaction fires |
| 7 | One-line intent comment at the top of each run, used by summary events (C8) | compaction | 35 | 0.85 | 1.0 | **30** | Optional |
| 8 | Repeat guard: warn at 3, block at 6 identical runs (M1) | code mode | 20 | 0.95 | 1.0 | **19** | Only if you switch to weaker models |
| 9 | Model-requested compaction at a turn boundary (C5) | compaction | 25 | 0.8 | 0.85 | **17** | Skip; the turn-start trigger covers it |
| 10 | `payloads` map for multiline content (M3) | code mode | 20 | 0.6 | 0.85 | **10** | Skip; raw triple quotes have 0 failures |
| 11 | Smaller failure-output budget plus full-output file (M5) | code mode | 10 | 0.9 | 0.9 | **8** | Skip; `_last` already covers it |
| 12 | Token-estimate calibration (C2) | compaction | 8 | 0.95 | 0.95 | **7** | Skip; measured within 10% |
| 13 | Context-pressure readout to the model (C6) | compaction | 8 | 0.9 | 0.95 | **7** | Skip |
| 14 | Expansion guard: cancel if the result exceeds 95% of before (C3) | compaction | 5 | 0.95 | 0.95 | **5** | Skip; it cannot expand at Air's ratios |
| 15 | Background jobs with a live output path (M6) | code mode | 25 | 0.3 | 0.7 | **5** | Skip; reverses the kill-group fix |
| 16 | Output-reserve / headroom trigger (C7) | compaction | 5 | 0.9 | 0.9 | **4** | Skip; the learned `compact_at` covers it |
| 17 | Learned argument aliases / catalog repairs (C14) | entropy | 5 | 0.7 | 0.9 | **3** | Skip; 0 misuses |
| 18 | Multi-agent skills (advisor, spec, council, swarm, graph, foreman) | steering | 30 | 0.1 | 0.6 | **2** | Skip; no actor runtime |
| 19 | Entropy meter plus schema normal forms (C13) | entropy | 3 | 0.3 | 0.6 | **1** | Skip; one string argument, nothing to normalise |
| 20 | Speculative PTC, QuickJS/Monty kernels, Bend kernels | code mode | 5 | 0.2 | 0.6 | **1** | Skip |
| 21 | Extractive history (classifier) | compaction | — | 0 | — | **0** | Violates LLM-free compaction |
| 22 | Reconstruction QA mutation tests | compaction | — | 0 | — | **0** | Violates no-tests |

**Best fit and most benefit:** #1 + #4 + #5 together (about 4 prompt lines, score 40–67 each, zero code risk), then #2 (about 12 Python lines), then #3 (about 25 Rust lines). Together these are under 45 LOC.

## 3. Porting blueprint: compaction and low entropy (deltas only)

### Already ported (no action)

These map one to one onto `src/compaction/*` in Fabric:

- Log-derived summary
- Bounded blocks with head-plus-tail sampling and `… N omitted; recall(n)` markers
- Three protected exchanges
- "historical, not a verified outcome" labels
- Carry-forward list
- Failure resolved by an exact action identity
- Cut only at assistant boundaries, so call and result pairs never split
- Reasoning stripped before the newest user message
- Missing tool results synthesised
- Byte-deterministic output (BTreeMap, no clock in the summary)

One difference: Fabric's footer carries a source timestamp taken from the entries (still deterministic); Air's footer has none.

### C4: per-turn outcome row (score 44)

Fabric's `[Earlier Turns]` gives each summarised turn a one-liner with its outcome. Air computes a verdict in `Agent::report` but never logs it, so after compaction the model cannot tell which earlier tasks were VERIFIED.

Air already ignores role-less entries in `messages()`, `cut()` and `summary()`, so a metadata entry is safe.

`src/agent.rs`: make `report()` take `&mut self`, then at its end (it already computes `verdict`, and the `changes` row value is in `rows`):

```rust
let _ = self.session.push(json!({"turn": {"task": began, "verdict": verdict}}));
```

`src/compact.rs`, where `requests` is built:

```rust
let verdict = |u: usize| {
    entries[u..].iter().find_map(|e| (e["turn"]["task"] == u).then(|| e["turn"]["verdict"].as_str()).flatten())
};
let requests: Vec<String> = users[..users.len() - recent.len()]
    .iter()
    .map(|&i| format!("#{i} {} → {}", line(text(i), 140), verdict(i).unwrap_or("unfinished")))
    .collect();
```

Also add the verdict to the `recent` dialogue headings (`#{u} user ({verdict}):`). This adds about 20 LOC. The "Earlier requests" block keeps its 3 KiB cap.

### C9: `recall` search (score 38)

Fabric's `memory.recall` is BM25 over session files. Air needs only a stdlib regex over its own log, which the sandbox already lets the worker read. `src/worker.py`:

```python
def recall(n):
    if isinstance(n, str):
        hits = []
        with open(LOG, encoding="utf-8") as f:
            for i, line in enumerate(f):
                entry = json.loads(line)
                text = entry.get("content") or "".join(c["function"]["arguments"] for c in entry.get("tool_calls") or [])
                m = re.search(n, text) if isinstance(text, str) else None
                if m:
                    start = text.rfind("\n", 0, m.start()) + 1
                    hits.append(f"#{i} {entry.get('role', 'record')}: {(text[start:start + 160].splitlines() or [''])[0]}")
        return "\n".join(hits[-30:]) or f"no entry matches {n!r}"
    ...
```

Change the prompt helper line to: `recall(n) -> str   # exact session entry #n cited by a summary; recall("regex") lists matching entries`.

### C8: intent line (score 30, optional)

This stands in for Fabric's `display.name`. `compact.rs` `skip` currently drops `#` lines. Instead, take a leading `# …` comment as the event's `what`, and add one prompt line: "Start each run with a `# what this run does` comment." The comment is in the model's code, not Air's source, so the no-comments rule is not affected. Its cost is a few tokens per run.

### Not ported, on evidence

- **Calibration (C2):** 0.90–0.94 accuracy measured.
- **Expansion guard (C3):** at Air's ratios the summary (8k tokens max) plus tail (20k) cannot exceed the 100k trigger.
- **Output reserve (C7):** `compact_at` learns 0.9 × last good prompt from an overflow.
- **Extractive history and QA (#21, #22):** each violates a standing rule.

## 4. Steering and prompt import list

These are diffed against `src/prompt.md`. Already present, so not imported:

- search/explore first, print only what you need
- acceptance criteria before edits
- check must pass after the last change
- don't repeat steps, don't re-read files
- smallest diff in the existing style

### S1 (score 67): replace Workflow step 4

This is adapted from Fabric's acceptance-ledger guideline (`fabric-exec-tool.ts:180`) and the spec-supervisor rule "Plans and intent are not evidence."

```markdown
4. Run check(). It must pass after your last change; if it fails, read the failure, fix it and run it again. A passing build is not proof the task works: when you can, also exercise the changed behavior directly (call the function, run the command, replay the failing case) and print the evidence. Don't rerun checks that already passed on unchanged files.
```

### S2 (score 40): add after the Helpers paragraph

This is adapted from Fabric's `failure-progress.ts` note and the fabric-exec skill's Recovery section.

```markdown
If a run raises, the `edit`/`write` lines it already printed as ok are applied: fix only the failing call, and re-read a file before editing it again.
```

### S3 (score 40): extend Code mode paragraph 2

This is adapted from Fabric guideline 4 (`fabric-exec-tool.ts:183`).

```markdown
Filter noisy output inside the run (pick the failing lines, count, slice) and print decisions and evidence, not raw logs.
```

Net prompt growth is +3 lines, about 90 tokens. After the one-time change the system prompt is byte-stable again, so prompt caching is unaffected. Resumed sessions keep the old prompt from `entries[0]`, so S1–S3 only apply to new sessions.

### Deliberately not imported

- **Advisor and spec-supervisor prompts:** both are good prompts, but they need an ambient actor runtime. Revisit only if Air grows a second model loop.
- **Fabric's `AGENTS.md`:** its rules are specific to the Fabric repo (bun, Bend, lazy graph).
- **`fabric-guide` and the other `SKILL.md` files:** they route to Fabric-only features.

## 5. Code mode upgrade plan

Air already has what Fabric's code mode needs, minus the TS kernels:

- an OS sandbox with egress denied
- gated `edit`/`write` that require `accept()`, block `.git` and hooks, keep CRLF, and feed formatter diffs back
- a host-side check on the final tree
- head/tail output with the full text in `_last`
- misuse hints with signatures
- `rejected` accounting
- SIGINT-safe mutation
- killing process groups after `sh()`

### Step 1: atomic multi-edit (M2, score 54)

Fabric's `pi.edit(edits=[...])` applies every replacement to one snapshot or none. Today, three `edit()` calls in one run that fail on the second leave the first applied. Each successful call also runs the formatter, so a later `old` can stop matching. The change is backward compatible: `edit(path, old, new)` still works, and `edit(path, [(old, new), ...])` is new.

```python
def edit(path, old, new=None):
    if isinstance(old, str) and new is None:
        raise TypeError("edit() missing 1 required positional argument: 'new'")
    pairs = [(old, new)] if isinstance(old, str) else [tuple(p) for p in old]
    def make(full, rel):
        with open(full, encoding="utf-8", newline="") as f:
            raw = f.read()
        crlf = "\r\n" in raw and raw.count("\r\n") == raw.count("\n")
        eol = (lambda s: s.replace("\r\n", "\n")) if crlf else (lambda s: s)
        text = result = eol(raw)
        for k, (o, n) in enumerate(pairs, 1):
            find = eol(o)
            count = text.count(find) if result.count(find) == 1 else 0
            if count != 1:
                where = f" (pair {k})" if len(pairs) > 1 else ""
                raise ValueError(f"old text{where} {'not found' if count == 0 else f'occurs {count} times'} in {rel}")
            result = result.replace(find, eol(n), 1)
        with open(full, "w", encoding="utf-8", newline="\r\n" if crlf else "") as f:
            f.write(result)
        return text, result
    _change("edit", path, make)
```

Every `old` must occur exactly once in the original snapshot (`text`), which is Fabric's semantics, and still exactly once in the running `result`. An earlier pair that destroyed or duplicated a later anchor therefore fails as "not found" rather than editing the wrong spot. The function writes once and formats once.

Prompt helper line:

```
edit(path, old, new)          # replace the one exact occurrence of old; raises on 0 or 2+ matches
edit(path, [(old, new), ...]) # several replacements in one file, all or none, formatted once
```

This adds about 12 LOC. The `_hint` markers keep working because the error text keeps "old text not found" and "old text … occurs".

### Step 2: prompt lines S1–S3 (section 4)

### Step 3 (optional, for weaker models): repeat guard

```rust
let mut h = std::hash::DefaultHasher::new();
code.hash(&mut h);
let key = h.finish();
let n = match self.last_code { Some((k, n)) if k == key => n + 1, _ => 1 };
self.last_code = Some((key, n));
if n >= 6 {
    return ("error: this exact code already ran 5 times in a row and was blocked; change the approach or finish".into(),
            json!({"tool": "run", "error": true, "rejected": true}));
}
```

This needs a new `last_code: Option<(u64, usize)>` field on `Agent`. The check goes at the top of `run_code`, plus a warning appended to the output when `n >= 3`, for about 10 LOC. Zero repeats were observed, so treat this as insurance against cheaper models.

### Explicitly rejected for Air's code mode

- **Background jobs (M6):** this reverses the round-12 kill-group fix.
- **`payloads` (M3):** a second tool argument, with 0 escaping failures observed.
- **Speculative PTC, entropy normal forms and repairs:** these need a typed action registry that Air does not have and does not need.

## Suggested order

1. S1 + S2 + S3 in `prompt.md` (one commit, about 4 lines).
2. M2 multi-edit in `worker.py` plus a helper line.
3. C4 turn verdict in `agent.rs` / `compact.rs`.
4. Re-evaluate C9 and C8 after the first real compaction (the pending long REPL session past 100k).

Verify each step as usual with `cargo build` and a manual run (S1 needs a real model turn to see whether probing raises the run count too much).

## Implementation log

### Candidate 1: implemented 2026-10-14 (uncommitted on `audit-round-12-fixes`)

Checking the premise changed the implementation. The model already tries to test behaviour on its own, by importing the repo's module into the worker. That always failed, because the worker runs `python3 -I` and the repo root is not on `sys.path`. Prompt text alone would have asked for a call that fails on the first try.

| Change | File | Why |
|---|---|---|
| Workflow step 4: "check() is evidence only for what it runs…"; step 5 adds "what showed it works" | `src/prompt.md` | The candidate itself |
| `sys.path.append(ROOT)`, appended so stdlib still wins over same-named repo files | `src/worker.py` | Makes the model's natural test (`import calc`) work |
| `_forget()` after each successful `edit`/`write`: drops repo modules (not site-packages) and the `G` names bound to them or defined in them | `src/worker.py` | A test can never use stale code; a stale name fails loudly with NameError |
| One prompt sentence: repo modules import, and every edit or write unloads them | `src/prompt.md` | Tells the model to import again |
| **Bug (pre-existing at HEAD):** after `import sh`, every `sh()` raised RecursionError, because the shim module's `__call__` looked up `G["sh"]`, which `import sh` had rebound to the shim. Fixed with `sh.sh = sys.modules["sh"] = sh`, which removes the `_Helper` class | `src/worker.py` | MiMo opened 2 of 6 recent sessions with `import sh`; one burned 9 runs debugging the helper |
| **Bug (pre-existing):** Apple CLT `python3` defaults `sys.pycache_prefix` to `~/Library/Caches/com.apple.python`, which the sandbox denies, so a check using `py_compile`/`compileall` fails on every tree. Fixed by adding `PYTHONPYCACHEPREFIX` to `CACHES` | `src/worker.rs` | Found when a scenario-A check failed for both variants |

A/B on MiMo V2.6 Pro. The baseline is HEAD plus both bug fixes, so only candidate 1 differs. Scenario A: `median()` bug, check is `py_compile` only. Scenario B: `slugify()` bug, no check configured.

| Run | Verdict | Runs | Tokens in | ModuleNotFoundError |
|---|---|---:|---:|---:|
| A base #1 / #2 | VERIFIED / VERIFIED | 5 / 5 | 7.0k / 7.4k | 1 / 1 |
| A new #1 / #2 | VERIFIED / VERIFIED | 9 / 4 | 14.6k / 6.1k | 0 / 0 |
| B base / new | UNVERIFIED (no check) / UNVERIFIED (no check) | 5 / 4 | 7.5k / 6.1k | 1 / 0 |

All six fixes were correct (median 2.5 with the caller's list unmodified; `'hello-world'`), and no stray files were left. The 9-run outlier came from model noise unrelated to testing: an empty first run, separate reads, a typo in `accept()`, and a final `git diff`.

On the earlier pre-fix run of scenario A, the old prompt rewrote `air.toml`'s check to get a pass (175k tokens). The new prompt refused to change the check and reported direct evidence instead (88k tokens). That is one sample.

These tasks are small, so they show the mechanics (no wasted import run, the test runs, evidence is reported) rather than a quality gain. The quality case is a task where the check does not cover the criteria and a wrong fix compiles. That still needs a real session to observe.

## Correction and re-ranking (2026-10-14, after candidate 1)

**Compaction has fired for real.** The first evidence scan skipped `1791969028-41225.jsonl` (the universe repo), because the session was live and its last line was incomplete. It has two compactions: #88, the turn-start trigger at 69k before "can you fix the findings", and #171, mid-turn at 100.8k (dropping to 25.6k). The "peak 73k, never fired" statements above are wrong.

What the real compaction shows:

1. **Dialogue budget (new, was wrongly listed as "already ported").** The task "can you fix the findings" refers to the review at #87 (3,458 bytes). Summary #171 cut that review to 1,000 bytes, because `compact.rs` caps the assistant reply before each user message at a fixed 1000 bytes and the user text at 2000. Fabric gives each exchange 3,968 bytes and lets a short user message hand its unused space to the reply before it. Here the model recovered because its own `accept()` criteria (#135) restated the defects, which is luck rather than design. **B 60 × F 1.0 × C 1.0 = 60.**
2. **Sandbox boundary line in the prompt (new; this is Fabric's "Runtime boundary" section).** The prompt never says what the sandbox forbids. universe spent about 9 runs (#180–#198) retrying a network-dependent CSS build ("network failures are often transient") and about 4 discovering that Unix-socket bind is blocked; air's own session spent 3 on blocked writes. Proposed line: *"The sandbox allows network only to localhost, Unix sockets not at all, and writes only inside the repo and temp dirs; a command that needs more fails with Operation not permitted or a DNS error. Don't retry it: say what it needed."* **B 45 × F 1.0 × C 1.0 = 45.**
3. **Candidate 2 (multi-edit) downgraded to about 22.** 8 runs edited one file several times sequentially without trouble, there were 0 partial applications, and the one failed edit came from a stale view of the file, which atomicity does not prevent.

Revised order: dialogue budget (60) → sandbox boundary line (45) → per-turn verdict row (44) → recovery and filter-output lines (40 each) → multi-edit (22).

### Dialogue budget: implemented 2026-10-14 (uncommitted), `src/compact.rs` +19 −12

Design check against the session data:

- User messages: max 231 bytes.
- Turn-ending replies: median 466 bytes, p90 2.8 KB, max 6.0 KB; 3 of 41 are over 3,968 bytes.
- Real follow-up referents: 152, 326, 458 and 3,478 bytes.

Fabric's flat per-exchange split (3,968 bytes, user keeps ≥ half, a short user hands the rest to the reply) covers 93% of replies whole and all four observed referents.

Rejected alternatives:

- **Newest-first shared pool:** covers the last 7% but adds more code.
- **Pinning the referent in `messages()`:** duplicates the reply and splits the logic across two files.
- **Head-plus-tail clipping:** Fabric is head-only too.
- **Static cap cuts to fit 32 KiB:** wastes space in the common case.

Changes:

- Per-exchange budget replaces the fixed 1,000 (reply) and 2,000 (user) caps.
- Dialogue cap goes from 12,288 to 16,384, so 3 full exchanges plus the latest reply never get sampled.
- Sections are budgeted in render order, so the summary stays at or under 32 KiB and keeps the `recall(n)` footer by construction (before, it was a tail `clip` that could cut the footer mid-line).
- `block()` returns an empty string instead of a clipped title when even its omission marker does not fit.

Verified with `cargo build`, `cargo clippy`, and a throwaway replay harness in the scratchpad that runs the real `compact.rs`:

| Case | Old | New |
|---|---|---|
| Real #88 | 4,437 bytes | identical |
| Real #171 | 6,555 bytes; review cut to 1,000 bytes | 9,048 bytes; full 3,458-byte review kept |
| Synthetic, 6 / 40 / 400 turns, every field oversized | dropped the middle exchange's user entry by sampling | keeps all 7 dialogue entries; ≤ 32,741 bytes; footer always present |

Under extreme pressure, "Latest events" shrinks to about 0.7 KB. That is the intended trade, since the raw tail carries the newest events anyway.

Still out of scope:

- `##` headings inside quoted replies look like summary sections.
- A summarised current task appears twice (in the dialogue and as "[Current task]").

### Sandbox-limits prompt section: implemented 2026-10-14 (uncommitted), `src/agent.rs` +10 −2

`system_prompt(root, yolo)` appends a `## Sandbox` section only when the sandbox is on; `--yolo` sessions omit it (checked).

Every claim in it was checked against Air's real `worker::sandboxed()` through a scratch harness on macOS:

- DNS and public TCP/UDP fail (`nodename nor servname`, `Could not resolve host`, `Operation not permitted`); localhost works.
- Unix-socket bind fails everywhere.
- Writes work in the repo, /tmp and `$TMPDIR`, and fail in ~ and `.git`.
- Nested `sandbox-exec` fails with exit 71.

The Linux-only behaviour (Unix sockets allowed, `Read-only file system` errors) is covered by naming the macOS-only limits explicitly.

The first wording ("use what is already installed") halved network retries but caused machine-wide hunting for an installed copy. The final wording forbids searching the machine and says to skip the step, finish the rest, and report it.

Task "fix slugify, then format with black (install if missing)" on MiMo; black is not installed:

| Variant | Runs | Tokens in | Network attempts | Machine searches | Time |
|---|---:|---:|---:|---:|---:|
| Baseline #1 / #2 | 21 / 27 | 113k / 240k | 4 / 10 | 13 / 11 | 3:47 / 9:37 |
| First wording #1 / #2 | 21 / 17 | 134k / 105k | 4 / 2 | 12 / 10 | 8:54 / 5:41 |
| **Final wording #1 / #2** | **7 / 9** | **15k / 20k** | 1 install each | 2 / 0 | 0:24 / 0:24 |

All fixes were correct, and every run reported that black could not be installed. The final runs also explained that the file is already in black's style.

### Re-check of the remaining candidates (2026-10-14)

- **Per-turn verdict row: 44 → ~15.** Real sessions have 1–3 user turns, so "Earlier requests" (users older than the last 3) is always empty.
- **Filter-output line: 40 → ~15.** After 22 truncated outputs the model never re-ran the command or read `_last`.
- **Recovery line: 40 → ~10.** There were 0 partial applications.
- **Multi-edit: stays at ~22.**

The Fabric list is exhausted. Further items should come from Air's own logs.

**Strongest non-Fabric item:** the macOS sandbox blocks every Unix socket. universe's SDK listener cannot start, 165 of 270 tests fail, and `mix precommit` cannot pass, so every universe turn ends UNVERIFIED.

A repo-scoped rule was tested in `sandbox-exec`:

```
(allow network-bind network-outbound network-inbound (local unix-socket (subpath ROOT)) (remote unix-socket (subpath ROOT)))
```

With it, bind and connect work under the repo, while `/tmp` binds, connects to an outside listener in `/tmp`, and public TCP stay blocked.

The remaining risk is that a daemon run *outside* air whose socket lives inside the repo would become reachable, for example universe's own `.universe/sdk.sock` while it runs, or Rails `tmp/sockets/*.sock`. This needs the user's decision.

### Host check never ran in real work: fixed 2026-10-14 (uncommitted)

None of the real repos (air, universe, konstruct; 8 sessions) had an `air.toml`, so the host verification gate never ran outside toy repos. Every changing turn ended UNVERIFIED.

Each `air.toml` below was parsed with Air's own TOML rules and run through the real `worker::sandboxed()`, and each passed with the tree unchanged.

| Repo | check | fmt | Run time |
|---|---|---|---|
| air | `cargo build -q && cargo clippy -q -- -D warnings` | rustfmt on `*.rs` except `src/main.rs` | 1 s warm |
| universe | read-only form of `precommit`, in the test env: `compile --warnings-as-errors`, `deps.unlock --check-unused`, `format --check-formatted`. **No `mix test`**: the sandbox blocks the SDK's Unix socket | `mix format` on .ex/.exs/.heex | 0 s warm |
| konstruct | `mix precommit` minus `deps.audit`/`hex.audit` (both need the network), run from `konstruct/`; includes 1,209 asset tests and 7,751 ExUnit tests on local Postgres | `mix format` inside `konstruct/` | ~197 s warm |

The report's check row now reads "none configured: set check in air.toml". A real turn on a scratch clone of air came back **VERIFIED** (3 runs, 20 s).

### Summary heading collision: fixed 2026-10-14 (uncommitted), `src/compact.rs` +14 −9

Quoted multi-line text could pose as summary structure. That covered the user and assistant text in the dialogue, the latest reply, accepted criteria, carry notes and failing check output. The universe review added five fake `##` sections to summary #171, and untrusted check output could forge `## Accepted criteria`, `#3 user:` directions or the `[Entries …]` footer.

Every line of quoted text is now prefixed with `> `, so no quoted line can start like a heading, an entry or the footer.

- Head-clipped text (dialogue, latest reply, criteria) is quoted first and clipped after, so the byte budgets stay exact.
- Check output is tail-clipped, so it is clipped first and quoted after; a partial first line still gets its prefix. Its section cap rises from 2,048 to 3,200 to cover the worst-case 2-bytes-per-line overhead (768 one-byte lines measured at 3,096 bytes).
- Single-line fields already carry Air's `#n` prefix and are unchanged, as is the current-task text pinned after the summary.

Rejected alternatives:

- **Fabric's `[Header]` style:** does not stop `#n user:` forgery.
- **Escaping `#` lines only:** partial.
- **Code fences:** break on replies that contain fences.

Replay results:

- Real #171: 15 → 10 structure lines, +62 bytes, review still whole.
- Real #88: unchanged apart from +4 bytes.
- Forged criteria, carry and check output: every fake structure line neutralised.
- Synthetic worst case at 6, 40 and 400 turns: ≤ 32,518 bytes, footer present, zero fake headings (HEAD had over 1,000), no dialogue drops.

### Current task duplicated after compaction: fixed 2026-10-14 (uncommitted), `src/session.rs` +2, `compact::quote` made public

The duplicate appeared only after a mid-turn compaction, because a turn-start compaction runs before the new task is logged. `messages()` keeps the full pinned task at the end of the summary message, where it is most salient. At render time it replaces the summary's own whole copy (`#u user:\n> …`) with `#u user: [the current task, in full below]`. The stored summary never changes, so once a new turn starts the summary renders whole again and nothing is lost.

A clipped copy, or a summary from before quoting, does not match and renders as before.

Checked by resuming a copy of the universe log with the real `session.rs`: the stored old-format summary still shows the task twice (unchanged), and a freshly built summary shows it once.

### Multi-edit: implemented 2026-10-14 (uncommitted), `src/worker.py`, prompt helper line

`edit(path, [(old, new), ...])` sits beside the unchanged `edit(path, old, new)`.

- Pairs apply in order against the running text, as repeated `edit()` calls would.
- It is all or nothing: a miss raises before anything is written, and the error says "pair k of n; nothing was changed".
- The file is written once and formatted once, and logged as one action.
- The error wording keeps "old text not found / occurs", so the existing hints still fire.
- Malformed arguments raise a clear `TypeError`.

Checked through the worker:

- single edit
- two pairs
- miss on the second pair (file untouched)
- chained pairs
- a CRLF file
- string without `new`, an empty list, and dicts instead of pairs
