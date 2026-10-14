# 07 — Agent-Loop Stability & Resource Audit: Oscillation, Atomicity, Lifecycle

**Audit date:** 2026-10-14 · Fourth pass, following 01–06. Cross-references use `[→ C#/F#/S#]`.

**Method:** the loop-pathology and lifecycle claims below were verified by live probes against the real `worker.py` — multi-edit batch failure, repeated-failure signal escalation, external-modification races mid-session, orphaned-descendant survival past worker SIGKILL — plus a session-retention census of the real `~/.air/sessions`.

## Probe evidence (live)

| Probe | Result |
|---|---|
| Multi-edit batch: `edit(a ok); edit(b ok); edit(c FAIL)` | **partial application** — a.txt/b.txt modified on disk, no rollback; ledger shows 2 ok + 1 failed |
| Identical failing edit repeated twice | **byte-identical error + identical hint both times** — no escalation signal exists |
| External file overwrite mid-session, then `edit()` with the stale `old` | **loud failure** (old text not found), disk untouched |
| Same race, edit anchored to the user's new content | **ok=true** — clean merge of user + model changes |
| `setsid` child launched via `sh()`, then worker `SIGKILL` | **orphan completed AFTER worker death** — `kill_on_drop` covers only the worker itself |
| Session retention census (`~/.air/sessions`) | **20 files / 1.1MB already accumulated; grep-proven zero GC code** |

---

## 1. Architectural & Algorithmic Failure Analysis

### 1.1 Oscillation: no divergence mitigation beyond two hard bounds (A1, A2)

The turn loop (`agent.rs:90-140`) has exactly two loop-state flags: `nudged` (the check gate fires once, `agent.rs:126-127`) and `overflowed` (one compaction-retry per overflow). The only hard bound is `MAX_REQUESTS = 200` (`agent.rs:90,138`). There is **no signature tracking, no repeat detection, no stagnation detector** — probe-proven: repeating the identical failing edit returns the identical error and hint, byte for byte. A model stuck in an edit-compile-fail-revert cycle therefore burns up to 200 full-context requests (with `compact_at` ≈ 100k, a pathological turn can bill on the order of millions of prompt tokens) before the hard stop, and nothing in the UI or report flags it as pathological.

Existing dampeners worth crediting: the one-shot gate prevents agent-driven fix loops *through the check*; `worker.py:254` invalidates a stale check ("files changed after it passed") which suppresses false "done" claims; `rejected` (worker.py:245-246) classifies syntax/unknown-name failures before acting; and `_hint` gives actionable self-correction. What's missing is any *memory* that a strategy already failed.

### 1.2 Goal drift: structurally sound, one caveat (A8)

This vector is largely **cleared**: the current task is pinned verbatim into the compacted context (`[Current task, entry #u]`, `session.rs:91-97`); `accept()` criteria survive compaction verbatim (1800B); `carry()` notes persist until explicitly cleared; `plan()`/`done()` give the model and the user step tracking. Because `summary()` is rebuilt from the raw log rather than from prior summaries, anchoring does not decay across successive compactions. The one caveat: user-direction excerpts are **head-only** (2000B) — constraints written late in a very long task text are the first thing cut `[→ first-pass M5]`.

### 1.3 Context economy: a clean two-tier memory with a deliberate hole (A10)

Working memory = the retained tail; long-term memory = the deterministic summary; system prompt = stable, built once per session. Tool outputs are capped at 8KB **head+tail** (worker.py:256-259) — deterministic, tail-preserving (exactly where Rust/Python backtraces live), with `_last` for in-worker recall `[→ C6 caveat]`. There is deliberately **no middle tier** — no file-content cache — which is the right economy choice and (see 1.5) a major robustness win. The residual cost traps: transport retries re-send the full prompt up to 3×; current-task `reasoning_content` (10–50KB) rides every request; tool-call arguments are uncapped `[→ F8]`.

### 1.4 Rehydration: near-perfect by design (A9)

The log is the single source of truth: `state()` re-derives accepted/carry/plan deterministically, `compact_at` replays from the last marker, the model stays pinned, and the snapshot index persists beside the log. Deliberately ephemeral on restart: worker Python globals and `_last` (the C6 wording overpromises), the `busy` flag, UI state. Real gap: after the *user* commits between sessions, the stale snapshot baseline misattributes changes on the next turn (ties to A4).

### 1.5 Workspace sync: the standout strength, plus two real holes (A7, A3, A4)

**Cleared (A7):** the agent keeps **no file cache or AST index** — so there is no stale-cache class of bug at all. Edits are exact-unique-match string replacements applied against *disk state at call time*; there are no line-number patches anywhere. Probe-proven mid-session race: after an external overwrite, an edit using the stale context copy **fails loudly** ("old text not found") and an edit anchored to the new content **merges cleanly**. External VS Code edits cannot produce hallucinated or silently-wrong patches.

**Hole 1 (A3):** multi-edit runs are not transactional — probe-proven 2-of-3 partial application with no rollback. The ledger gives the model full visibility and git gives undo, so this is *visible* partial state rather than corruption — but there's no partially-applied summary line and no cheap rollback primitive.

**Hole 2 (A4):** turn-start/turn-end tree comparison cannot attribute changes. A user editing files mid-turn gets their edits (a) run through the check gate, (b) attributed to the agent in the changes row, and (c) **reverted by the printed undo hint** (`git restore --source=<tree>`) if followed. This is the sharpest new footgun found in this pass.

---

## 2. Resource & Memory Leak Audit

**Verdict: no true leaks; unbounded growth by design in four places.**

- **No leaks:** the crate uses no `Arc`/`Rc` cycles — everything is owned; per-request buffers (`buf`, `text`, `thinking`, `calls`, `held`, worker `_report`, capture tempfiles) are all freed per iteration; the worker's per-request `sys.stdout` wrapper churn does not leak fds (`closefd=False`); `_inbox`/`_outbox` are dup'd exactly once; UI statics are replaced, not appended; the ticker task is aborted on `Turn::drop`.
- **Unbounded growth (by design):** (1) `Session.entries` holds the full log in RAM forever `[→ F8]`; (2) `~/.air/sessions` is never pruned — **live census: 20 files / 1.1MB on this machine already**, each containing full code + outputs at 0600, and `resume()` opens every file on every start (A6); (3) the shared `air-cache` `[→ S3]`; (4) orphaned processes (A5).
- **Process lifecycle:** host git calls reap via `output()`; `verify()` uses tokio timeout + `kill_on_drop` ✓; `_exec` uses `killpg` on timeout ✓ — but a `setsid`-ing descendant escapes the group entirely, and worker teardown (`kill_on_drop`, worker.rs:110) kills only the python process. **Probe-proven: an in-flight descendant completed after worker SIGKILL.** A restarted worker can race with that orphan on the same repo. On Linux, `bwrap --unshare-all` PID-namespace containment closes this by design; macOS has no equivalent `[→ S4]`.
- **FD lifecycle:** tempfile-based capture is unlinked at creation and closed per request; the session file is a single append-mode fd; `ui::size()` opens `/dev/tty` + spawns `stty` every second (fd churn, reaped — `[→ C3]`).

---

## 3. Refactoring Code & Architectural Blueprints

### 3.1 LoopGuard: oscillation + stagnation detection (A1, A2)

```rust
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use serde_json::Value;

#[derive(Default)]
struct LoopGuard {
    failures: VecDeque<u64>,   // consecutive identical failed-action signatures
    idle: u32,                  // consecutive runs with no edits and no check
}

impl LoopGuard {
    /// Call once per run() reply; returns an intervention to append to the tool result.
    fn observe(&mut self, meta: &Value) -> Option<String> {
        let actions = meta["actions"].as_array().cloned().unwrap_or_default();
        let acted = actions.iter().any(|a| a["path"].is_string() || a["tool"] == "check");
        self.idle = if acted { 0 } else { self.idle + 1 };

        let mut h = std::hash::DefaultHasher::new();
        actions.to_string().hash(&mut h);
        let sig = h.finish();
        if actions.iter().any(|a| a["ok"] != true) {
            if self.failures.back() == Some(&sig) { self.failures.push_back(sig); }
            else { self.failures.clear(); self.failures.push_back(sig); }
        } else { self.failures.clear(); }

        if self.failures.len() >= 3 {
            self.failures.clear();
            return Some("the same action has failed identically 3 times in a row — \
                        print the current file state and change your approach".into());
        }
        if self.idle >= 4 {
            self.idle = 0;
            return Some("several exploratory runs produced no edits or checks — \
                        state your conclusion or make a change".into());
        }
        None
    }
}
```

Hook in `run_code`'s `Ok` arm: `if let Some(nudge) = self.loop_guard.observe(&meta) { out.push_str(&format!("\n[{nudge}]")); }`. Pair it with a per-turn cumulative prompt-token budget that ends the turn with a diagnostic instead of coasting to `MAX_REQUESTS`.

### 3.2 Transactional edit recovery: pre-images + `rollback()` (A3)

```python
# worker.py — capture the first pre-image per file, expose a rollback primitive
_txn = {}

def _change(tool, path, make):
    rel = _rel(path)
    ...
    if rel not in _txn:
        _txn[rel] = before          # pre-image of the FIRST edit to this file this run
    ...

# in the request loop, after the except handler:
if failed and _txn:
    print(f"[{len(_txn)} file(s) partially edited this run; call rollback() to restore them]")

def rollback():
    """Restore every file touched since the last failed/accepted run boundary."""
    for rel, before in _txn.items():
        with open(os.path.join(ROOT, rel), "w", encoding="utf-8", newline="") as f:
            f.write(before)
    names = sorted(_txn); _txn.clear()
    _report.setdefault("actions", []).append({"tool": "rollback", "ok": True, "paths": names})
    print("rolled back:", ", ".join(names))
```

This keeps the model's recovery cheap and explicit without taking git away.

### 3.3 Honest change attribution + guarded undo hint (A4)

```rust
// agent.rs report(): separate the turn's edits from foreign changes
let ledger: std::collections::HashSet<&str> = turn.iter()
    .flat_map(|e| e["meta"]["actions"].as_array().into_iter().flatten())
    .filter_map(|a| a["path"].as_str()).collect();
let changed_paths = /* paths from the snapshot-vs-worktree diff */;
let external: Vec<_> = changed_paths.iter().filter(|p| !ledger.contains(*p)).collect();
if !external.is_empty() {
    rows.push(("external", format!("changed outside this turn: {}", external.join(", "))));
    // scope the undo hint to ledger paths only — never blindly `git restore` everything
}
```

### 3.4 Orphan containment, best-effort (A5)

Have `_exec` register every spawned pid in a sidecar next to the log (`AIR_LOG` + `.procs`), and have the agent sweep it at worker teardown — re-verifying each pid still belongs to this worker's start-time window first (pid-reuse hazard) before killing. Full closure needs OS-level PID containment, which macOS lacks; document the platform asymmetry (Linux is already safe via `--unshare-all`).

### 3.5 Session retention GC (A6)

```rust
fn gc_sessions(home: &Path, keep_per_root: usize) {
    // group *.jsonl by cwd header; keep the newest `keep_per_root` per root,
    // delete the rest plus their .index/.procs sidecars. Warn before first deletion.
}
```

### 3.6 Line-aligned truncation (minor polish)

The head+tail cap is already deterministic and tail-preserving; align both cut points to line boundaries so the model never receives a half-line at the seam (`text[:half].rsplit('\n',1)[0]` head, `text[-half:].split('\n',1)[-1]` tail).

---

## 4. Stress Testing Strategy

1. **Scripted-oscillation harness:** a mock LLM scripted to repeat the same failing edit N times; assert `LoopGuard` fires by strike 3, the turn ends with a diagnostic, and token spend stays under budget vs. the unmitigated baseline.
2. **Stagnation harness:** 20 read-only runs; assert the idle-streak intervention fires at 4 and that the nudge text reaches the model as a tool-result suffix.
3. **Atomicity/rollback property test:** randomized edit batches over fixture files with an injected failure at position k; assert the ledger reports exactly k-1 applied, `rollback()` restores byte-identical pre-images, and check() results invalidate correctly afterwards.
4. **External-race test:** a host thread mutates files between runs (and mid-run); assert every edit either fails loudly or applies against fresh content — never a silent merge of stale expectations; hash-verify untouched files.
5. **Orphan sweep test:** `setsid`-ing command + worker kill; assert the registry sweep kills it (or, on macOS, that the residual is reported in the turn report).
6. **Long-session soak:** synthesize a 10k-entry session; assert `messages()` latency stays O(live tail) after the wire-cache fix, summary byte-determinism holds, RSS growth is documented (entries are O(log) by design — pin the curve), and `resume()` survives truncation at every byte offset `[→ 05 §4]`.
7. **Session-GC test:** 100 stale sessions across roots; assert retention keeps the newest K per root, never deletes the resumable one, and cleans sidecars.
8. **Token-budget enforcement:** a runaway mock turn; assert the budget cap ends the turn with a diagnostic before `MAX_REQUESTS`.
9. **Undo-attribution test:** concurrent user edits + agent edits in one turn; assert the report separates external vs. agent paths and the undo hint names only the agent's paths.

---

## Bottom line

The agent's core loop discipline is better than most: goal anchoring and rehydration are structurally sound (A8/A9), the no-file-cache design makes workspace sync a *strength* (A7), and existing dampeners (one-shot gate, check invalidation, rejection classification, hints) cover several pathologies. The systemic gaps are all in *memory of failure*: nothing remembers that a strategy already failed (A1/A2), nothing makes multi-edit batches atomic (A3), nothing attributes concurrent external edits (A4), and nothing reaps or retires what the session leaves behind — processes (A5) and logs (A6). Every fix is local and small.
