# CLI audit: bloat, dead code, performance, design, practices

2026-10-15 · read-only audit, nothing implemented · head `fee5e65`

## Map

| Entry | `main.rs` — REPL default; `air [TASK]` one-shot; flags `-m/--model --resume --yolo --stats`;
  REPL commands `/compact /cost /think /models /providers` |
|---|---|
| `agent.rs` 1066 | turn loop, retries/nudges, compaction trigger, verify+check, git snapshot/diff,
  secrets scan, report, stats glue |
| `llm.rs` 616 | OpenAI-compat client: `/chat/completions`, `/models`, SSE parse, reasoning fields,
  timeouts, `x-opencode-session` |
| `session.rs` 206 | JSONL session log, wire-message builder, header/switch, state |
| `compact.rs` 277 | compaction cut-point + summary builder |
| `providers.rs` 230 | providers.toml CRUD + `/providers`·`/models` menus |
| `worker.rs` 255 | sandbox host side: spawn (sandbox-exec/bwrap), env, cwd guard, line protocol |
| `worker.py` 386 | sandbox runtime: `run/edit/write/show/sh/check/carry/plan`, output capture, kill-on-exit |
| `ui.rs` 533 | paint, status ticker, events, report |
| `render.rs` 143 | markdown renderer |
| `stats.rs` 163 | `--stats` comparison |

Dependencies: reqwest(rustls), tokio, serde/serde_json, anyhow, clap, libc, rustyline, toml.

Host ↔ worker: persistent python child; one JSON line per `run()`: request `{code, state}` →
reply `{report, out, error}`; `check()` = extra round-trip `{needs_check}` → `{ok, out}`.

## Tool results (real findings only)

- `cargo clippy --all-targets -W clippy::pedantic`: **92 warnings**, mostly benign (token-math
casts, name similarity). Distribution: 15× format!-append, 9× u64→f64, 9× u128→u64, 7× u64→usize,
  7× similar names, 6× {:?}→{}, 4× u32→i32, 3× iterator-format, 3× f64→u64 ×2, 2× match→if-let,
  2× Duration units, 2× trailing semicolon, 1× needless return. Real residue ≈ 26 sites.
- `cargo tree -d`: only shared libc transitives via rustls-platform-verifier — nothing removable.
- `ruff --select ALL` (via uvx): style noise (E501×46, ANN, PTH, T201) fitting the file's dense
  style; SIM115 = intentional persistent streams. No defects.
- `vulture` (via uvx): only importlib loader protocol hooks (false positives). **No dead Python.**
- `cargo machete`: not installed; manual grep confirms every crate dep has live callers.
  **No dead Rust.**
- Release build: **6.2 MB** binary, 3.3 s rebuild, **272 ms startup** (`--version`). Healthy;
  reqwest/tokio justified by streaming + async.
- Worker spawning: persistent child, state round-tripped per run — **not restarted needlessly**.

## Findings

```
[high]  src/agent.rs:624,1042 + git()      | blocking subprocess+fs on the current_thread runtime stalls the ticker
                                            and delays Ctrl-C | tokio::task::spawn_blocking (move owned paths)
                                            | ~0 (+3 lines)
[high]  src/ui.rs:38-44,120-121            | size() shells out to stty, polled by the resize ticker
                                            | TIOCGWINSZ via libc (dep already present) | ~0, −1 subprocess/poll
[high]  src/main.rs:82-155, src/agent.rs:795, src/providers.rs:68-127
                                            | config.toml base_url synthesizes a second provider mechanism
                                            duplicating providers.toml | drop it; providers.toml becomes the
                                            only path | ~26 — BEHAVIOR CHANGE
[med]   src/worker.py:251 + src/agent.rs:624,1042
                                            | change detection implemented twice (worker stat-tree vs host
                                            git-diff); can disagree after edits via sh() | single source: drop
                                            _tree(), trust host git | ~12 — BEHAVIOR CHANGE
[med]   src/llm.rs:158-162,199-203         | UA+session+bearer signing duplicated in models() and complete()
                                            | one 5-line private helper (the identify() shape — optional) | ~5
[med]   src/main.rs:36-46,97 + worker.rs   | Config.env: no config sets it | remove field, allow_env, worker
                                            plumbing | ~10 — BEHAVIOR CHANGE
[med]   src/ui.rs:19 + 24 lock().unwrap()  | ui::CHECK global mutated from agent.rs (hidden coupling); a poisoned
                                            mutex after a ticker panic cascades into every ui:: call | fold CHECK
                                            into Bar/Turn; lock().unwrap_or_else(|p| p.into_inner()) | ~0
[med]   src/agent.rs (module)              | god module: loop, retry, verify, git, secrets, report, stats in one
                                            file | none under the no-new-files rule; design debt | 0
[low]   src/llm.rs:143                    | .expect() panics in non-test path on client build failure |
                                            acceptable (backend init, effectively infallible) — note only | 0
[low]   src/main.rs:22-30 + Cargo.toml     | clap for 4 flags + 1 positional | ~20 lines of std::env
                                            | ~0, −1 dep — BEHAVIOR CHANGE (help text)
[low]   pedantic residue (15+3+2+6 sites)  | micro: format!-append, iterator-format, match→if-let, {:?}
                                            | mechanical cleanup | ~10
[low]   verified sound, no findings        | agent.rs SECRETS redaction, llm timeouts (connect/first-byte/idle/
                                            total), two-stage Ctrl-C, sandbox + kill-on-exit (worker.py _watch,
                                            agent Group::drop), 0600 session/key files, git() array-args (no
                                            shell injection) | — | 0
```

## Totals

- ~15 lines removable with zero behavior change
- ~48 with the behavior-changing removals (provider dup ~26, env ~10, _tree ~12)
- 1 dependency removable (clap, with help-text tradeoff)
- 2 hot-path fixes (+3 lines, no shrink)

## Top 3 fixes

1. `spawn_blocking` for agent.rs git/fs — restores Ctrl-C/ticker liveness (user-facing)
2. `stty` → TIOCGWINSZ — removes a subprocess per poll, equal lines
3. config-provider removal — biggest cut (~26), everything lost is replaced by a `[name]`
   block in providers.toml

Status: proposed only — awaiting decision.
