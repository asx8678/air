# Low-entropy review: the smallest codebase that does exactly this

2026-10-15 · read-only · head `fee5e65` · total 4,120 lines over 12 files

## 1. The real spec, as delivered

```
[core]      one-shot task mode; exit 0 verified / 1 unverified or failed
[core]      REPL (rustyline): /compact /cost /think /models /providers
[core]      run(code): persistent sandboxed python worker; variables survive; 16k output cap; _last
[core]      in-worker helpers: accept/edit/write/show/sh/check/carry/plan/done
[core]      sandbox: sandbox-exec/bwrap, no egress, cwd-scoped writes, secret dirs hidden, kill-on-exit
[core]      streaming UI: markdown, live thinking, status ticker, plan box
[core]      session JSONL: resume, model/provider switch mid-session, stable conversation id
[core]      auto-compaction at compact_at; summary carried through compaction
[core]      check() verification on change, failed-check gate, turn report
[core]      retries: stream interruption, 500/429 backoff, empty-reply handling, context-overflow
            auto-compact + retry
[core]      multi-provider: providers.toml (+menus), config.toml default model + profiles
[core]      opencode.ai identification: User-Agent low/<version>, x-opencode-session
[core]      --yolo
[rarely]    --stats cross-session comparison (163 lines)
[rarely]    markdown table rendering (~60 lines)
[rarely]    repeat suppression REPEAT_WARN/REPEAT_BLOCK
[rarely]    reasoning-field replay FIELDS/"reasoning" (alternate providers only)
[half]      config.toml base_url → synthesized "config" provider (second mechanism)
[half]      FIELDS remap: field/marked cells for providers that name the key "reasoning"
[half]      profile keys nothing sets today: think_limit, turn_tokens, guidance
[dead]      header["low"] build stamp — written (session.rs:40), never read anywhere
[dead-ish]  Overthought/reasoning cap — reachable only via think_limit, which nothing sets
```

## 2. Entropy: current vs minimal design

```
metric        current                        minimal
lines         4,120                          ~1,900
structs       20 (enums 0, traits 0)         ~12
fns           ~210                           ~120
consts        48                             ~28
error types   anyhow + Interrupted,          anyhow + Interrupted(String),
              Overthought, ContextOverflow,  ContextOverflow
              Lost
CLI flags     5                              5 (same spec)
config keys   config: model,base_url,check,  config: model,check,fmt
              fmt,env,models[6 profile keys]; low.toml: check,fmt; providers: 3
              low.toml: check,fmt; providers: base_url,api_key,models
deps          8 (+clap droppable)            7–8
modules       agent 1064 · llm 615 · ui 533 · worker.py 386 · compact 277 ·
              worker.rs 255 · providers 230 · session 204 · main 200 ·
              stats 163 · render 143 · prompt.md 50
minimal       agent ~330 · llm ~300 · ui ~260 · worker.py ~340 · worker.rs ~230 ·
              compact ~130 · providers ~140 · session ~170 · main ~130 · render ~80 ·
              prompt.md 50 · stats 0
```

Worst lines-per-feature: status ticker+plan box ~180 (pure UX) · self-heal tiers ~120 ·
git snapshot/diff for report detail ~150 · --stats 163 · compaction quality dial ~120.

## 3. Findings

```
[max]  src/llm.rs:558-595 | Inline ︎ tag state machine | strip ︎…
[max]  src/agent.rs:20-55,285-360 | self-heal tier machinery: NUDGE/AGAIN/GATE/LENGTH consts,
       nudged/nudges/cut state, REPEAT_WARN/BLOCK repeat suppression | one empty-reply retry,
       plain error otherwise | ~120 — BEHAVIOR CHANGE
[max]  src/agent.rs:25,423,1013-1021 + src/llm.rs:18-24,109-112 | two context-overflow sniffers
       (WINDOW+window() in agent, OVERFLOW+overflow()+ContextOverflow in llm) | one: the llm
       error carries the parsed limit | ~25
[big]  src/agent.rs git()+changes()+moved()+EMPTY_TREE+index juggling (~150 lines) | host git
       diff duplicates the worker's stat tree (_tree, worker.py:251) as a second
       change-detection concept | one source: the worker tree; report loses +N/−M detail
       | ~140 — BEHAVIOR CHANGE
[big]  src/llm.rs:43-85 | Interrupted/Overthought/Lost trio + lost()/extra() | one
       Interrupted(String) payload | ~35
[big]  src/llm.rs FIELDS/field/marked (~25) | reasoning-key remap for a provider family
       nothing uses | always reasoning_content | ~25 — BEHAVIOR CHANGE (rare providers)
[big]  src/main.rs:82-155 + agent.rs:795 + providers.rs:68-127 | two provider stores
       (config.toml base_url synthesis + providers.toml) | one store | ~26 — BEHAVIOR CHANGE
[med]  src/worker.rs:69 | uncapped worker reply lines | cap the read | ~0 (correctness)
[med]  src/ui.rs:7-140+ | ticker Bar/resize/plan-box (~180 lines, one UX concept) | notes-only
       UI | ~180 — BEHAVIOR CHANGE (visible)
[med]  src/render.rs:85-140 | markdown table renderer | plain text | ~60 — BEHAVIOR CHANGE
[med]  src/stats.rs (163) | --stats flag, one user | drop or keep as-is | 163 — BEHAVIOR CHANGE
[med]  src/session.rs:40 + build.rs | header["low"] stamp + LOW_BUILD git describe:
       written, never read | delete both | ~15
[low]  src/main.rs:36-46,97 | Config.env: nothing sets it | remove | ~10 — BEHAVIOR CHANGE
[low]  profile keys think_limit/turn_tokens/guidance: reachable only via config profiles
       that no user sets | keep or drop with their read sites | ~30 — BEHAVIOR CHANGE
[low]  session entries as stringly-typed serde Values | typed Entry enum would delete
       role-string guards in session.rs/compact.rs | Entry enum | ~10 net, compiler-checked
```

## 4. References

- pi (this harness): npm package ships dist only (20 MB compiled; no readable src) — core tools
  are shell + file ops with a sandbox profile; no embedded interpreter. The one architecture
  that would shrink low most (~700 lines: worker.rs+worker.py+protocol) but changes the
  product — code-mode IS low. Not recommended; noted for honesty.
- OpenAI Codex CLI (github.com/openai/codex, codex-rs/core/src): client.rs alone is ~125 KB;
  compact_* totals hundreds of KB; agent_message_board 11 KB; code_mode_host.rs 5.8 KB.
  Ideas worth taking: apply_patch.rs (3.4 KB, one self-contained patch tool), single-file
  client. low is already 10-50× smaller than codex in every comparable area.

## 5. Top 3 structural changes, in order

1. Delete the ︎ Inline state machine (llm.rs:558-595) — pure removal, no behavior visible
   beyond thinking-box granularity, smallest risk
2. Collapse the self-heal tiers into one retry + one error path (agent.rs:20-55,285-360)
3. Merge the two overflow sniffers and the three interruption error types (llm.rs:43-85,
   agent.rs:1013-1021) — one concept each

Then, if you accept visible behavior changes: drop the git-diff change detection (~140),
the ticker (~180), --stats (163), tables (~60) → lands at ~2,100 lines.

Status: proposed only — awaiting decision.
