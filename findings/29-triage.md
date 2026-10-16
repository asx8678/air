# low findings triage: OpenCode scan + findings/23–27

2026-10-15 · code at `fee5e65` · read-only

**Scope:** OpenCode's scan (28-opencode-scan.md, OC-F1–F13) and `findings/` docs 23, 24, 25 and 27. Doc 26 is background notes on the OpenCode provider and has no findings. The deleted docs 01–22 were not checked.

**Method:** a verifier checked each finding against the current code, then a skeptic re-checked it and tried to refute it. Where it helped, they ran probes: sandbox-exec, a mock provider with a scratch HOME and fake keys, cargo-deny, and builds in copied trees. The skeptics disputed 3 of 54 verdicts, all about how much a finding matters; those are resolved below. The fix-now and fix-soon items were also re-checked by hand.

**Result:** nothing critical or high. 1 fix now, 10 cheap fix-soon items (mostly one-liners), 25 optional, 17 not worth doing. Several simplification proposals in docs 23, 24 and 27 rest on wrong premises; don't apply those (see "Wrong or overstated claims").

## Fix now

1. **rustls 0.23.45 has 4 advisories** (25-1) in `Cargo.lock`. `cargo update -p rustls` moves it to 0.23.46 and clears all four. This was tested in a copy, where `cargo deny check advisories` then passed. Prefer it to a bare `cargo update`. Real exposure for low is close to nil (brotli isn't compiled in, ECH isn't used, session values are never Debug-printed), but the lockfile bump is zero-risk. Effort S.

## Fix soon (cheap)

2. **A bad base_url or API key crashes low** (new). `src/llm.rs:479` `Llm::send()` calls `request.try_clone().expect(...)`, which panics whenever the request builder holds an error. Reproduced with `api.example.invalid/v1` (no scheme), `http://127.0.0.1:99999/v1`, and a key containing `\x01`: all exit 101. In the REPL this kills the session, and `--resume` crashes again until the config is fixed by hand. Fix: `.context("could not build the request: check base_url and the API key")?`, plus a URL check in `connect()` and in `/providers` add/edit. S.
3. **`low --version` hides the build stamp** (new). The binary embeds `LOW_BUILD`, but `src/main.rs:18` prints only `low 0.1.0`, so today's stale install looked current. Fix: `#[command(version = concat!(env!("CARGO_PKG_VERSION"), " ", env!("LOW_BUILD")), ...)]`. Tested: it prints `low 0.1.0 fee5e65-dirty`. S.
4. **Resuming an older session fails with OpenCode** (OC-F10). In `src/session.rs`, only `create()` writes `header["id"]`, so sessions made by the old build have no id. `resume()` never adds one, so no `x-opencode-session` header is sent and the request fails with 400 MissingSessionID. Fix: in `resume()`, when the header has no id, derive it from the file stem as `create()` does. S.
5. **Provider error text skips secret redaction** (OC-F2; also contradicts doc 25's "no secrets in logs or errors"). Error text built from `head()`/`clip()` in `src/llm.rs` `send()`/`stream()` reaches `agent.rs:224-226` (stored as `event:error`) and `main.rs:58` without `redact()`. Redaction runs only at `agent.rs:574` (check output) and `agent.rs:938` (tool output). A gateway that echoes the request would put your key on screen and in the session log. Fix: run the text through `redact()` before printing or storing it. While there, add the opencode key prefixes (`oc_sk_`, `oc_`) to the redaction table at `agent.rs:38-48`, which covers `sk-`, `ghp_`, `AKIA` and `xox*` but not OpenCode's own keys (new, low). S.
6. **Blocking git calls freeze Ctrl-C and the status bar** (23-5 = 24-1). `src/agent.rs:1022` `git()` uses `std::process::Command::output()` on the `current_thread` runtime (`main.rs:54`). On a big repo, `git add -A`/diff blocks for seconds and Ctrl-C isn't polled meanwhile. Fix: `tokio::task::spawn_blocking` with owned paths. M.
7. **`stty` subprocess every second** (23-4 = 24-2). `src/ui.rs:38-44` `size()` is polled by the ticker. Fix: a `TIOCGWINSZ` ioctl through `libc`, which is already a dependency. S.
8. **`--stats` can underflow** (OC-F6, partly true). At `src/stats.rs:32`, `prompt - cached` wraps in release and panics in debug if a provider reports cached > prompt. The `changed - verified` case it also names can't happen. Fix: `saturating_sub`. S.
9. **Session errors don't name the file** (25-2). `src/session.rs` `create()` uses a bare `?` for the directory create, the open and the lock. Fix: `.with_context(|| path.display().to_string())`. S.
10. **`low ""` runs a full model round-trip** (25-7). At `src/main.rs:102`, task mode doesn't reject an empty task the way the REPL does. Fix: trim it and reject it. S.
11. **The keychain is reachable from the macOS sandbox** (OC-F4, reproduced by two independent probes). The `src/worker.rs:161-173` profile is `(allow default)` with no mach-lookup deny, so `security find-generic-password` reaches securityd. Prompt-injected code could read keychain items that have no per-app access control; most sensitive items still prompt for permission. The fix needs research first: denying the obvious service names (`com.apple.SecurityServer`, securityd) did **not** block the lookup, and denying exec of `/usr/bin/security` is bypassed by copying the binary. M, medium confidence.

## Optional (your call)

- **OC-F1** A repo's `low.toml` check/fmt run automatically. Low: they run in the same sandbox as the model's own code, are read once at startup, and the report flags low.toml as a check input. Cheap extra: print a note when check/fmt come from the repo.
- **OC-F3** The API key is echoed while you type it in `/providers`. It shows on screen only and isn't saved to history.
- **OC-F5** The worker can read non-secret files under $HOME. Allow-listing reads would break toolchains; Documents/Desktop/Downloads could be added to the deny list.
- **OC-F7** `write()` creates LF files in CRLF repos.
- **OC-F9** The host check misses a `:port`, uppercase letters and userinfo. The documented URL works, and a deviating one fails with a visible 400.
- **OC-F11** Session logs are never pruned. At KB–MB per session it's a slow leak.
- **OC-F13** The ticker and streamed output share no lock, but on the single-threaded runtime visible garbling is unlikely.
- **25-4** Cap the worker reply line at `worker.rs:69`. The worker is low's own code, so this is defense in depth.
- **25-6** Exit 130 on SIGINT (behavior change).
- **25-8** RUN_CAP is 570 s while the prompt says "10 minutes". The gap is a deliberate margin under the 600 s host timeout, so this is only a doc nit.
- **24-7** Lock poisoning in `ui.rs`. `CHECK` itself is an `AtomicI32`, not a mutex, so the doc's framing is wrong.
- **23-2** One signing helper for `models()`/`complete()`: ~5 lines, no behavior change.
- **24-4** Drop the worker's `_tree()` (~13 lines). It only gives the model a hint, and the host re-checks the final tree anyway. Between docs 24 and 27, doc 24's direction is the right one.
- **23-1** Drop config.toml `base_url` (~25 lines). You deliberately restored this this morning in 21fe434, so it's your call. If dropped, show a clear migration error.
- **27-14** Retire `turn_tokens` (never set), `think_limit` and `guidance` (used in one experiment). Removed keys must be rejected, because `settings()` forwards unknown profile keys to the provider as request params.
- **27-1** A ~12-line strip could replace the ~40-line `Inline` `<think>` parser with only a display difference. It does nothing for your current providers (0 of 986 replies had inline tags).
- **27-11** `--stats` currently compares nothing: it skips sessions in temp dirs, which is 380 of your 392. Fix the filter or drop it.
- **27-10** The markdown table renderer (~55 lines, used in about 1% of answers).
- **27-3** Move `window()` next to `OVERFLOW`. Cohesion only, ~0 lines saved.
- **25-3** Add `publish = false` or a license.
- **25-5** A few unit tests, and `cargo test` in the check.
- **25-9** Rename `Inline` to `ThinkTag`.
- **25-10** `lto = "thin"`: 12% smaller binary, ~6 s slower installs.
- **25-11** deny.toml isn't needed; cargo-deny's advisory check already works without a config.

## Not worth fixing

- **OC-F8** Plaintext keys in providers.toml. File mode 0600 plus the sandbox read-block is the standard tradeoff.
- **OC-F12** Linux bwrap ordering. It can't run on this Mac; code analysis says bwrap creates the parent directories.
- **25-12** Streams without usage report 0 tokens out. This is by design.
- **24-9** `.expect()` in `Llm::new` is effectively infallible.
- **23-6** `_tree()`'s per-file stats have no measurable cost.
- **23-7** Dropping clap: a faithful replacement is net **+46** lines (not ~0), for −387 KB and worse error messages.
- **24-8** The agent.rs "god module" is debt, not a defect.
- **24-11** Pedantic clippy residue: 9 of the 26 sites (`worker.rs:157-169`) use `{:?}` on purpose to quote the SBPL profile. Clippy's suggested fix breaks sandbox-exec (exit 65), so the worker wouldn't start.
- **27-2** Collapsing the self-heal tiers: they are separate recoveries. GATE is what turns a fixable check failure into VERIFIED; a probe ended VERIFIED with it and UNVERIFIED without it.
- **27-5** Merging the error types would retry think_limit hits up to 3 times, re-spending the tokens each time.
- **27-6** Dropping the `reasoning` field remap: gateways such as OpenRouter and Groq stream under `reasoning`, and 555f1b0 added the remap on purpose.
- **27-9** Removing the ticker and status bar: it is the main interactive UX.
- **27-12** Deleting the build stamp: it is what identified today's stale binary. Show it instead (item 3).
- **27-15** A typed `Entry` enum means ~240 sites to change and a risk of breaking `--resume` on old logs.

## Wrong or overstated claims

- **23-3 / 24-6 / 27-13, "Config.env: nothing sets it", is wrong.** It's read from config.toml `env = [...]`, added on purpose in 555f1b0 and documented in the `--yolo` help and the sandbox prompt. A probe confirmed it passes variables through. Removing it would also make configs containing `env` fail to parse.
- **27-4, "drop the host git snapshot", is wrong.** The snapshot is the trust anchor for the verdict and the undo hint. In a probe, low's `git restore --source=<snapshot>` hint kept the user's pre-turn uncommitted edit, while a plain `git restore` lost it. The sandboxed worker can't even write snapshot objects.
- **27-1, "pure removal, nothing visible", is wrong as written.** Simply deleting the parser makes think-only replies end the turn, and on inline-think providers it puts reasoning into answers and history.
- **Smaller misses:** 23-7's "~0 lines" is really +46. 27-3's "two detectors" do different jobs: one classifies the error, the other extracts the limit. In 24-7, `CHECK` is an atomic.
- **Doc 25** says "no secrets in logs or errors"; that doesn't hold for provider errors (item 5).
- **OpenCode (OC-F1–F13):** 10 confirmed, 2 partly true (F6, F13), 1 not verifiable here (F12, Linux). It rated F1–F3 medium; all three are low on this code. None is high.
