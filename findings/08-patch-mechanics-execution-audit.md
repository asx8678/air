# 08 — Patch Mechanics & Execution Audit: EOL Edges, Syntax Gates, Concurrency

**Audit date:** 2026-10-14 · Fifth pass, following 01–07. Cross-references use `[→ C#/F#/S#/A#]`.

**Method:** every mechanics claim was verified by live probes against the real `worker.py` — an EOL/encoding edit matrix, syntax-gate behavior with and without a configured formatter, and a two-worker lost-update race — plus code-trace analysis of prompt assembly and background execution.

## Probe evidence (live)

| Probe | Result |
|---|---|
| Pure-CRLF multi-line edit | **ok** — byte-exact CRLF write-back (`X\r\ngamma\r\n`) |
| Mixed-EOL single-line edit | **ok** — per-line endings preserved |
| Mixed-EOL multi-line edit | **fails loudly** (old text not found) — heuristic covers only pure CRLF |
| Non-UTF-8 (latin-1) file edit | **fails loudly on decode** — uneditable, never corrupted |
| Syntax-error write with `FMT=py_compile` | action **ok=True**; `fmt failed:` note surfaced; **invalid file persists on disk** |
| Syntax-error write without FMT | **completely silent** (`write bad2.py: ok`) — no gate exists |
| Two workers, staggered read-modify-write on one repo | **silent lost update** — final file holds only the second writer's content; no exceptions |
| Ledger check during the race | **both action ledgers empty** — raw `open(w)` bypasses accept() and the audit ledger (C1 at the mechanics layer) |

---

## 1. Critical Structural Defect Matrix

| ID | Severity | Module | Finding | Impact |
|----|----------|--------|---------|--------|
| **P1** | MED | `worker.py:85-99` | Edit mechanics are corruption-proof but brittle: mixed-EOL multi-line `old` fails loudly (CRLF heuristic only detects pure-CRLF files); non-UTF-8 files uneditable; the unconditional `newline` translation on write-back would double an explicit CRLF inside model-provided `new`. Every mismatch fails loudly — never corrupts. | Edit brittleness on messy real-world files; loud failures cost turns/tokens |
| **P2** | MED | `worker.py:52-99` | No syntax pre-flight: invalid syntax hits disk. A configured formatter surfaces the parser line post-hoc (`fmt failed: …`) but the action stays ok=True and the file is never reverted; with no formatter the write is silent. The model pays a full check() cycle to discover typos. | Token waste; broken intermediate states |
| **P3** | MED | `worker.py:52-83`, `main.rs` Config | Negative constraints are prompt-only: `do-not-touch Cargo.lock` style rules have no deterministic interceptor. The `_change` gate exists and consults `PROTECTED` (.git + hooks only) — no user-configurable globs; raw writes/sh() bypass even that (C1). | Rule-following degradation; unenforced guardrails |
| **P4** | MED | `worker.py:87-97`, `session.rs:29` | No workspace-level lock: the flock guards only the session file. Two air instances on one repo interleave edit()'s read→write window with no coordination — probe-proven silent lost update, both ledgers reporting success. | Silent lost updates; divergent ledgers |
| **P5** | LOW-MED | `worker.py:19-30` | No background-task primitive: `sh()` is synchronous capture-to-completion (120s default; 40min run cap). Dev servers / `cargo watch` cannot be started and observed; the `cmd &` workaround orphans (A5) and loses logs after the capture file is read; the capture tempfile is unbounded during the run (S5). | Capability gap; process leaks; lost logs |
| **P6** | LOW | `worker.py:256-259` | Output truncation is a blind char-slice head+tail: seams can cut mid-line/mid-token; no line alignment, no structural windowing. Only tool-output framing is affected (no file injection exists — A7). | Context cosmetics; avoidable re-queries |
| **P7** | CLEARED | `agent.rs:329-354`, `session.rs:88-127`, `prompt.md` | No index to invalidate (stale-AST class structurally absent); retrieval is prompt-delegated with explicit print-only-what-you-need + line-slicing examples; prompt ordering sound (system first always, summary at position 2, gate messages at the tail, accept-criteria re-injected through every summary); tool schema minimal (~60 tokens, one tool, one param). | — |
| **P8** | CLEARED | `worker.py:85-99` | Content-anchored edits have no offset-invalidation class: per-call re-reads mean a second edit whose region was changed by the first fails loudly; non-overlapping sequential edits compose. The apply-patch-at-wrong-line corruption mode is structurally absent (verified across pass-4 and pass-5 probes). | — |

---

## 2. Detailed Failure Modes & Code Scenarios

### 2.1 EOL/encoding: corruption-proof but brittle (P1)

`edit()` (worker.py:85-99) detects CRLF only when `count(CRLF) == count(LF)` — i.e., pure-CRLF files — then matches in an LF-normalized view and writes back with `newline="\r\n"` translation. Probe: pure-CRLF multi-line edit succeeded with byte-exact `X\r\ngamma\r\n` on disk. But a **mixed-EOL** file (common after cross-platform edits) fails the equality, so no normalization happens and a multi-line `old` (the model always emits LF) cannot match across a CRLF boundary → probe-proven loud `old text not found`. Single-token edits still work (they don't span the boundary). Non-UTF-8 files (latin-1 probe byte `0xe9`) raise `UnicodeDecodeError` inside the read — loud, safe, but the file is simply uneditable. Exotic edge: the unconditional `\n → \r\n` write translation would double any explicit `\r\n` the model places inside `new` (`\r\r\n`). Net: the design never corrupts — every mismatch fails loudly — at the price of edit success rate on messy files.

### 2.2 The syntax gate that isn't (P2)

Nothing parses the result before the disk write commits. Probe transcript: writing `def f(:` with `AIR_FMT=python3 -m py_compile` produced action **ok=True** plus `write bad.py: ok; fmt failed: File "bad.py", line 1` — the formatter surfaces the parser error *post-hoc* while the invalid file **stays on disk**; without a formatter the output was just `write bad2.py: ok`, completely silent. The philosophy (check() verifies at the end) covers semantic errors but wastes a full check cycle on typos a 30ms parse would have caught.

### 2.3 The silent lost update (P4 + C1)

Race timeline (probe-proven): worker A reads `race.txt` at t0; worker B reads at t0.5 (same base content); A writes `base+A` at t2.0; B **overwrites** with `base+B` at t2.5 built from its stale read → final file `base\nB\n`, A's write silently gone. Both runs reported no exception, and **both action ledgers were empty** — because raw `open(..., "w")` bypasses `accept()` and the ledger entirely (C1 re-proven at the mechanics layer). `edit()` has the same read→write window (worker.py:87-97), just narrower: two concurrent sessions editing different regions of one file both report ok and the last writer silently erases the first. The session flock (session.rs:29) guards only the log file, not the worktree.

### 2.4 Multi-edit composition: no offset class (P8 — cleared)

There are no line offsets to invalidate: each `edit()` re-reads the file at apply time, so a second edit whose `old` overlaps the first edit's changed region fails loudly (stale anchor), and non-overlapping edits compose sequentially. The classic wrong-line patch corruption mode requires offset arithmetic — structurally absent here.

### 2.5 Background execution: no primitive (P5)

`sh()` captures stdout+stderr to a completion tempfile and blocks until the timeout. A dev server can only be run as `cmd &` inside one sh invocation — the sandbox even allows localhost for exactly this — but then the server is orphaned when sh returns (A5) and its post-capture logs are lost. No `spawn/logs/stop` surface exists.

### 2.6 Prompt architecture & attention (P7 cleared; P3 gap)

Assembly order is fixed: system (prompt.md + AGENTS.md + skill descriptions, built once, ~700 tokens) → compact summary (position 2, current task pinned verbatim) → retained tail (recent tool results at the end — high attention) → gate messages land at the tail. Constraints that matter most (accept criteria) are re-injected verbatim through every compaction summary, acting as durable rule re-statement — the strongest anti-degradation mechanism in the codebase. The tool schema is the anti-bloat ideal: one tool, one parameter, ~60 tokens (agent.rs:354). The gap: negative constraints have **no deterministic enforcement** (P3) — they ride on prompt compliance only.

---

## 3. Rust Remediation & Refactoring

### 3.1 EOL-flexible `edit()` — match normalized, write original endings (P1)

```python
def edit(path, old, new):
    def make(full, rel):
        with open(full, encoding="utf-8", newline="") as f:
            raw = f.read()
        # 1. Match in an LF-normalized view — pure-CRLF and mixed-EOL alike.
        eol = [ln.endswith("\r\n") for ln in raw.split("\n")[:-1]]   # per-line CRLF map
        text = raw.replace("\r\n", "\n")
        n = text.count(old)
        if n != 1:
            raise ValueError(f"old text {'not found' if n == 0 else f'occurs {n} times'} in {rel}")
        head, _, tail = text.partition(old)
        old_n, new_n = old.count("\n"), new.count("\n")
        at = head.count("\n")
        style = eol[at] if at < len(eol) else False          # ending of the first replaced line
        merged = head + new + tail                           # all-LF view
        # 2. Restore endings: head keeps its map entries, the new block inherits `style`,
        #    tail re-indexes past the replacement.
        out = []
        for k, ln in enumerate(merged.split("\n")[:-1]):
            if k < at:
                crlf = eol[k]
            elif k < at + new_n:
                crlf = style
            else:
                j = k + (old_n - new_n)
                crlf = eol[j] if 0 <= j < len(eol) else style
            out.append(ln + ("\r\n" if crlf else "\n"))
        result = "".join(out) + merged.split("\n")[-1]
        with open(full, "w", encoding="utf-8", newline="") as f:   # no translation: no CR doubling
            f.write(result)
        return text, result
    _change("edit", path, make)
```

### 3.2 Syntax pre-flight gate — block the write, return the parser line (P2)

```python
PARSERS = {".py": f"{sys.executable} -I -m py_compile"}   # add tree-sitter or rustfmt-based drivers

def _parses(rel, content):
    driver = PARSERS.get(os.path.splitext(rel)[1])
    if driver is None:
        return
    tmp = os.path.join(ROOT, rel + ".air-parse-check")
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(content)
    out, code = _exec(f"{driver} {shlex.quote(tmp)}", 30)
    os.remove(tmp)
    if code != 0:
        last = out.strip().splitlines()[-1] if out.strip() else f"exit {code}"
        raise ValueError(f"refusing to write {rel}: it does not parse — {last}")
```

Hoist `make()` into compute-then-gate-then-write (two-phase `_change`) so the gate runs before the disk commit; the existing error path feeds the parser line straight back to the model.

### 3.3 Workspace lock — one agent per worktree (P4)

```rust
use std::fs::{File, OpenOptions};

/// The lock lives in .git: host-writable, model-unreachable (the sandbox denies
/// worker writes there), and invisible to the diff.
struct WorkspaceGuard(File);

impl WorkspaceGuard {
    fn acquire(root: &Path) -> Result<Self> {
        let f = OpenOptions::new().create(true).write(true)
            .open(root.join(".git/air.lock"))
            .context("cannot open the workspace lock")?;
        f.try_lock()                       // same std API as session.rs:29
            .context("another air is already running in this repository")?;
        Ok(Self(f))
    }
}

impl Drop for WorkspaceGuard {
    fn drop(&mut self) { let _ = self.0.unlock(); }
}
// Agent::new: hold WorkspaceGuard::acquire(&root)? for the process lifetime.
```

### 3.4 Hard negative-constraint interceptor (P3)

```rust
// main.rs — Config gains an enforcement list (air.toml: protected = ["Cargo.lock", "migrations/*"])
pub protected: Vec<String>,
// worker.rs — passed to the worker at start, never editable from the repo:
cmd.env("AIR_PROTECTED", config.protected.join("\n"));
```

```python
# worker.py — inside _change, before anything else:
if any(fnmatch.fnmatch(rel, g) for g in _PROTECTED_GLOBS):
    raise ValueError(f"{rel} is protected by configuration; refusing to modify")
```

### 3.5 Background tasks with a bounded ring buffer (P5)

```python
_tasks = {}

def spawn(name, cmd, cap=2000):
    if name in _tasks: raise ValueError(f"task {name} is already running")
    p = subprocess.Popen(cmd, shell=True, cwd=ROOT, stdin=subprocess.DEVNULL,
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                         start_new_session=True)
    buf = collections.deque(maxlen=cap)                 # bounded: memory O(cap) lines
    threading.Thread(target=lambda: [buf.append(l.rstrip("\n")) for l in p.stdout], daemon=True).start()
    _tasks[name] = (p, buf)
    print(f"spawned {name} (pid {p.pid}); logs({name!r}) to read, stop({name!r}) to end")

def logs(name, tail=40):
    p, buf = _tasks[name]
    return "\n".join(list(buf)[-tail:]) + f"\n[alive={p.poll() is None}]"

def stop(name):
    p, _ = _tasks.pop(name)
    with contextlib.suppress(ProcessLookupError):
        os.killpg(p.pid, signal.SIGTERM)
    print(f"stopped {name}")
```

### 3.6 Line-aligned truncation (P6)

```python
if len(text) > 2 * HALF:
    head = text[:HALF].rsplit("\n", 1)[0]        # cut on a line boundary
    tail = text[-HALF:].split("\n", 1)[-1]
    text = f"{head}\n[… {len(text) - len(head) - len(tail)} chars omitted; full output in _last]\n{tail}"
```

---

## 4. Verification & Property Testing Strategy

1. **EOL fuzz (P1):** generate files with randomized per-line EOL mixtures (LF/CRLF, BOM, latin-1) × randomized edit regions; assert invariants: never corrupts (no `\r\r\n`, untouched lines byte-identical), every failure is loud, pure-CRLF round-trips, mixed-EOL multi-line edits succeed post-fix.
2. **Multi-edit composition property (P8):** random edit sequences with overlapping/stale anchors; assert sequential composition equals a serialized reference and stale anchors raise — disk never diverges from the ledger.
3. **Syntax-gate corpus (P2):** valid/invalid files per gated extension; assert invalid writes are refused with the parser line, valid admitted, ungated extensions pass through.
4. **Concurrency (P4):** N staggered writers on one repo; assert the workspace lock refuses the second instance post-fix (and that the pre-fix lost-update is caught by the test as the regression baseline).
5. **Truncation seam fuzz (P6):** outputs at cap boundaries; assert cuts are line-aligned and omission markers exact.
6. **Background-task harness (P5):** a process emitting 100k lines/min; assert bounded deque memory, `logs(tail)` correctness, and `stop()` reaps the tree (with the A5 setsid caveat documented or gated).
7. **Prompt-assembly golden (P7):** fixed log → byte-identical system+summary+messages and tools() across rebuilds — extends 05's determinism tests with gate/summary placement.
8. **Raw-write tripwire (C1/P4):** assert any `open(..., "w")` outside `_change` produces a ledger entry and a warning — closing the bypass the race probe re-exposed.

---

## Bottom line

The patch engine's core contract holds under fire: **content-anchored, exact-unique-match edits structurally cannot corrupt** — there is no offset class, no silent fuzzy matching, no AST index to go stale, and every mismatch fails loudly (P8, A7, probes across two passes). The real defects live at the edges the design didn't anticipate: mixed-EOL and non-UTF-8 files (P1), a syntax gate that reports but doesn't prevent (P2), prompt-only guardrails with no interceptor (P3), no worktree lock for concurrent sessions (P4), and no background-task surface (P5). All five close with local, well-scoped fixes — the EOL map, the parse gate, the config interceptor, the workspace flock, and a bounded spawn/logs/stop trio.
