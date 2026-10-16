# low

A low-entropy, code-mode coding agent for the terminal.

The model gets one tool: `run(code)`. It writes a short Python script, and low runs it in a persistent, sandboxed worker at the root of your git repository. Exploring, editing and testing happen inside those scripts, so a step that would take a dozen tool calls elsewhere is usually one round trip.

- **Sandboxed:** no network, writes only inside the repo and temp dirs, and SSH keys, cloud credentials and shell history are hidden.
- **Verified:** low runs your project's check on the final tree and reports whether the change passed.
- **Reversible:** low never commits. Git is the undo, and every turn report prints the command that restores the tree.
- **Minimal:** a short system prompt, deterministic harness decisions, and no hidden LLM calls (context compaction is rule-based).

## Install

```sh
cargo install low
```

You need:
- macOS or Linux; low's sandbox is `sandbox-exec` on macOS and [bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`) on Linux;
- `git`;
- Python 3 (low uses the newest `python3.N` on your `PATH`).

## Set up a provider

low talks to any OpenAI-compatible `/chat/completions` endpoint. Add one to `~/.config/low/providers.toml`:

```toml
[opencode]
base_url = "https://opencode.ai/inference/go/v1"
api_key = "…"                  # optional: else $LOW_API_KEY, else ~/.config/low/api_key
models = ["deepseek-v4.1-flash"]
```

Inside the REPL, `/providers` adds and edits providers, and `/models` switches models.

`~/.config/low/config.toml` holds your defaults:

```toml
model = "deepseek-v4.1-flash"   # the default model
env = ["MY_VAR"]                # environment variables passed into the sandbox

[models."deepseek-v4.1-flash"]  # per-model request parameters
temperature = 0.0
compact_at = 200000             # compact the context at this many prompt tokens
```

## Configure a repository

A `low.toml` at the repository root tells low how to verify and format changes:

```toml
check = "cargo build -q && cargo test -q"     # run on the final tree; decides the verdict
fmt = "rustfmt --edition 2024"                # run on each edited file (its path is appended)
```

## Use

```sh
low "the median of an even-length list is wrong; fix it"   # one task, then exit
low                                                         # REPL
low --resume                                                # continue the latest session in this repo
low -m some-model "…"                                       # pick a model
```

- **One-shot mode** exits with 1 when a configured check didn't pass on the final tree, or when the turn stopped short, and with 0 otherwise.
- **REPL commands:** `/compact`, `/cost`, `/think` (the model's last reasoning), `/models [filter]`, `/providers`.
- **`--yolo`** turns the sandbox off: network on, writes anywhere. Environment variables stay filtered either way.

Each turn ends with a report:
- **the verdict:** VERIFIED, UNVERIFIED, NO CHANGES or NO CHECK;
- the files changed;
- the token and time cost;
- an undo command, such as `git restore --source=<tree> --worktree -- .`.

Sessions are JSONL files in `~/.low/sessions`.

## How it works

Inside `run(code)`, the model has a few helpers:

| helper | does |
|---|---|
| `sh(cmd)` | runs a shell command; returns stdout+stderr, with `[exit N]` on failure |
| `show(path, start, end)` | prints numbered lines, and pages long files |
| `edit(path, old, new)` | replaces one exact occurrence (or several pairs, all or none), then runs `fmt` |
| `write(path, content)` | creates a new file |
| `check()` | runs the repository's `check` in the sandbox |
| `accept(criteria)`, `plan(steps)`, `done()` | record what done means and track progress for the turn report |

Repository files change only through `edit` and `write`, so low knows exactly what the model touched. A turn that changed files without using either is asked once to clean up its scratch output.

## Coming from air

low was called air. It still reads `~/.config/air`, `air.toml`, `$AIR_API_KEY`, and the sessions in `~/.air/sessions`, so an existing setup keeps working.

## License

MIT
