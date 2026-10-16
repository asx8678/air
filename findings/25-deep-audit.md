# Deep audit: run it, trace it, break it (passes 1-9)

2026-10-15 · read-only; repo untouched (scratch in /tmp/audit) · head `fee5e65`
All probing used a local mock OpenAI-compatible server (zero API calls, zero cost).
No subagent facility in this harness — passes ran sequentially with fresh probes each.

## Method

- Pass 1: cargo test (2/2, not flaky), cargo-deny (present), cargo-llvm-cov/hyperfine/
  cargo-bloat/cargo-audit absent (manual substitutes used), startup 5× (`--version`: 10 ms),
  full mock session with RSS sampling (peak 9.2 MB).
- Pass 3 (break it): modes served by /tmp/audit/mock.py — ok, garbage SSE, close-mid-stream,
  500 text/plain, 17 MB single line, hang+SIGINT, worker suicide, two instances, empty/400 KB/
  non-UTF-8 tasks, missing config, non-git dir, read-only session dir.
- Passes 4-8: protocol/state/determinism reads, release-profile A/B build in a /tmp copy.

## Findings

```
[high] [1] Cargo.lock (rustls 0.23.45)  | 4 known TLS vulnerabilities in the pinned tree
        | cargo-deny advisories FAILED: "Debug impls for client session values expose TLS
        secrets"; "Buffered application data sent to outer server after ECH rejection";
        "Brotli certificate decompression can allocate up to 1 GiB before authentication";
        "Unauthenticated ServerHello can downgrade later connections" | cargo update
        (lockfile bump only) | 0
[med]  [3] src/session.rs:33,36 | session create/open errors carry no path | repro:
        read-only ~/.low → `error: Permission denied (os error 13)` with no hint which file
        | .context(path) | +1 line
[med]  [1] Cargo.toml:2 | no `license` field, no LICENSE file in the repo | cargo-deny:
        "low = 0.1.0 is unlicensed" | add both | +1 line
[med]  [4] src/worker.rs:69 | host↔worker reply lines uncapped (llm.rs has a 16 MB LINE_MAX;
        the worker side trusts worker.py's own caps) | BufReader::lines() with no limit
        | cap the read (~4 MB) | +2 lines
[med]  [1] src/ (whole crate) | automated coverage = 2 unit tests (~20 lines of llm.rs);
        ~4.1k lines untested; no CI config in the repo | cargo test: 2 passed; the mock
        harness in /tmp/audit proves cheap e2e tests are possible | integration test with a
        mock provider | +~60 lines (investment — breaks the shrink rule, flagged)
[low]  [3] src/main.rs:59 | SIGINT exits 1 — indistinguishable from a failed task
        (convention: 130) | B5: interrupted run exits 1 | exit(130) on interrupt
        | +1 line — BEHAVIOR CHANGE
[low]  [3] src/main.rs (task mode) | empty task "" runs a full model round-trip on nothing
        | A1: exit 0, 2 requests, 1 run for `low ""` | reject empty task | +2 lines
        — BEHAVIOR CHANGE
[low]  [5] src/worker.py:16 RUN_CAP=570 | prompt.md says "a whole run is cut at 10 minutes"
        but 570 s = 9.5 min | align either side | 0
[low]  [6] src/llm.rs:370 `struct Inline` | name lies — it extracts ︎ blocks, not inline
        markup | rename to ThinkTag | 0
[low]  [8] Cargo.toml [profile.release] | only strip; no lto/codegen-units | measured in
        a /tmp copy: 6.2 MB → 5.4 MB (−13%), build 32 s | add `lto = "thin"`, `codegen-units = 1`
        | +2 lines. Do NOT add panic="abort": Group::drop process cleanup relies on unwinding
[low]  [1] toolchain | cargo-audit/llvm-cov/hyperfine/cargo-bloat absent; no deny.toml
        (license noise from defaults) | install + add a deny.toml allowlist | +~6 lines
        (investment)
[low]  [3] src/llm.rs stream | usage-less streams report "0 tok out" (cost undercount);
        truncated ends accepted silently (by design — "accept unmarked stream ends") | note
        only | 0
```

## Verified clean (kept out of findings)

- Ticker task aborted on drop (src/ui.rs:76-80) — no per-turn task leak
- Worker crash mid-run: clean restart, clear message ("the worker exited; the worker was
  restarted and its variables are gone"), turn survives, exit 0
- Two instances: second `--resume` exits 1 with the exact locked session path
- SIGINT: exit in ~2 s, zero leftover python/mock processes (Group::drop + worker.py `_watch`
  backstop), session files remain valid JSONL
- 17 MB single SSE line: cleanly rejected ("a stream line passed 16 MB") — no OOM
- 500 text/plain: 5 retries with backoff (60 s), then a full informative error with body
- Garbage SSE line: skipped, valid content after it recovered
- 400 KB task: completes; 497 KB session; empty-string arg handled by clap (exit 2)
- Determinism / prompt-cache safety: tools() is a literal json! (stable order), system prompt
  built once per session (agent.rs:111, rebuilt only on switch), wire() strips meta,
  BTreeMap JSON ordering, compaction has no time/random inputs
- EVENTS vec drained on every session push (session.rs:98) — bounded
- Startup 10 ms, peak RSS 9.2 MB, no TODO/FIXME, --help matches commands, git() uses array
  args (no shell injection), no secrets in logs or errors

## Not run (tools absent)

cargo audit (advisories covered via cargo-deny) · cargo llvm-cov (manual coverage above) ·
hyperfine (time-based: 10 ms) · cargo bloat (size via ls; per-crate breakdown n/a) ·
pip-audit (worker.py is stdlib-only — nothing to audit)

## Totals

- New lines removable: ~0 (previous audits 23/24 still own the ~48 removable)
- Lines to add for correctness/security/robustness: ~+8 (all flagged above; 2 behavior changes)
- Dependencies: none removable beyond 23/24's clap note

## Top 5 fixes

1. `cargo update` — patched rustls, closes 4 TLS advisories (0 lines, security)
2. session.rs error context (+1 line)
3. `license` field + LICENSE file (+1 line)
4. worker reply cap (+2 lines)
5. `lto = "thin"`, `codegen-units = 1` in the release profile (−0.8 MB, +2 lines)

Status: proposed only — awaiting decision.
