# Bloat and simplification review

2026-10-15 · read-only review, nothing implemented · base `9f9da1c` → head `fee5e65`

## Scope

Reviewed the session's diff line by line (src/agent.rs, src/llm.rs, src/session.rs — +62/−3),
then every file in the repo (agent.rs 1066, llm.rs 616, ui.rs 533, worker.py 386, compact.rs 277,
worker.rs 255, providers.rs 230, session.rs 206, stats.rs 163, main.rs 200, render.rs 143,
build.rs 7). Every candidate removal was grep-verified before being listed.

**Result: no dead code.** All consts, fns, fields and imports have live callers.

## OpenAI-compat surface (recent changes)

- Endpoints `{base}/chat/completions` and `{base}/models`, Bearer auth, `stream` +
  `stream_options: {include_usage: true}`, function-format `tools` — all current. No deprecated
  `max_tokens` is sent. SSE + `[DONE]` parsing is current.
- `x-opencode-session` verified mandatory by live probe (400 MissingSessionID without it), sent
  only to opencode.ai hosts; other providers get a plain request.
- Verified against opencode's own client and docs (the gateway air actually calls) plus this
  session's live 200/400 probes; online docs were unreachable (no browser session).

## Findings, ranked

1. **config provider duplicates providers.toml** — src/main.rs:82,86,131,149 ·
   src/agent.rs:795 · src/providers.rs:68-127
   config.toml `base_url` synthesizes a second provider mechanism (fallback + with_config +
   holding()'s or_else chain + the menu's `fixed`/`config` notes).
   Fix: drop `base_url` support; providers.toml becomes the only provider path; holding()
   collapses to the plain find. **~26 lines. BEHAVIOR CHANGE**: config.toml base_url stops
   making a provider.

2. **request signing duplicated** — src/llm.rs:158-162,199-203
   UA + `x-opencode-session` + bearer applied identically in models() and complete().
   Fix: one 5-line private helper applied at both sites (the identify() shape, removed earlier
   this session — optional). **~5 lines.**

3. **Config.env unset** — src/main.rs:36-46,97 + worker.rs env plumbing
   Passes extra env vars to the worker; nothing sets it.
   Fix: remove the field, allow_env, and the worker-side pass-through.
   **~10 lines. BEHAVIOR CHANGE**: drops a documented option.

4. **stty subprocess per size()** — src/ui.rs:38-44,120-121
   size() shells out to `stty`, also from the resize ticker.
   Fix: TIOCGWINSZ ioctl via libc (dep already present). **~0 lines**, kills a subprocess per poll.

5. **blocking git/fs on the current_thread runtime** — src/agent.rs:624,1042 + git()
   moved/changes/tree stall the ticker and ctrl-c delivery.
   Fix: spawn_blocking. **+3 lines, no shrink** — hot path only.

6. **_tree() stats every file per check()** — src/worker.py:251
   Inherent to its purpose; noted as hot path only. **~0 lines.**

7. **clap for 4 flags + 1 positional** — src/main.rs:22-30 + Cargo.toml
   ~20 lines of std::env parsing replace it. **~0 lines, −1 heavy dep.**
   **BEHAVIOR CHANGE**: hand-rolled help/error text.

## Totals

- ~5 lines removable with zero behavior change
- ~36 with the behavior-changing removals (config provider + env option)
- 1 dependency removable (clap, with tradeoffs)
- 2 hot paths (stty, blocking git/fs)

## Top 3 to do first

1. llm.rs signing dedup — safe, mechanical, ~5 lines
2. stty → TIOCGWINSZ — no line change, removes a repeated subprocess
3. config-provider removal — biggest cut (~26 lines), needs an explicit OK; everything lost is
   replaced by a `[name]` entry in providers.toml

Status: proposed only — awaiting decision.
