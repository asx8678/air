# Air — Round 12: Probe-Verified Delta Audit

**Reviewed:** 2026-10-14 · **Tree:** `91d63d4` (src unchanged since) · every line of `src/` read.
**Method:** each claim below is backed by a live probe run on this machine (macOS, `sandbox-exec`, real `air` debug binary against a scripted fake SSE server). Probe commands and their raw output are inlined so they can be rerun without the scratchpad.

> **Scope.** Rounds 01–11 already map all six requested domains against this tree. This round does not re-list them. It reports (a) new findings with probe evidence and (b) corrections to earlier rounds where a probe contradicts a claim or a proposed fix would regress the design. Rounds 10 and 11 were written by another process while this round was in progress; they are cross-checked below.

---

## 1. Executive Summary & Severity Matrix

The most important result of the whole series is **N1: the macOS sandbox has never denied network.** The profile clause meant to allow localhost, `(local ip "localhost:*")`, matches every outbound socket, so sandboxed code can open TCP and UDP connections to any IP address. It has been in place since the first commit (`630117a`). It survived five audit rounds and the earlier smoke tests because the **system resolver** is still blocked: `curl https://example.com` fails with `000` and looks like a denial. Code that connects by IP, or sends its own DNS query over UDP, gets out. Several earlier findings assumed the network was denied (S2 "the exfil channel is the LLM request", S3 "network-denied builds", L9), so their severity needs to be reassessed.

The second-order results correct the record. Two high-severity claims from round 10 and one from round 11 do not survive probing. Round 10's prompt-cache fix would also remove the within-turn reasoning echo that the provider integration relies on.

| ID | Severity | Status | Location | Finding |
|----|----------|--------|----------|---------|
| **N1** | **Critical** | **new** · refutes "network denied" premise of S2/S3/L9 | `worker.rs:73-74` | `(allow network* (local ip "localhost:*") …)` matches all outbound sockets → TCP + UDP egress to any IP from the worker, `sh()` and the host-run check. Proven end-to-end through the real binary. |
| C1 | Low (was High) | **refined** · R10-H1's fix is a **regression** | `session.rs:105-107` | Reasoning stripping costs one re-prefill of the *previous turn only*, not the session. Measured: ~1.1k uncached tokens over all 3 real turn boundaries. Stripping at `push()` (R10 §3.2) would drop the within-turn `reasoning_content` echo the provider accepts and the tool-call continuation relies on. No action. |
| C2 | — | **refuted** (macOS) | R10-H2 | A sandboxed Unix-socket connect to a socket under `/private/tmp` is denied, so the SSH-agent hijack does not work on macOS. Linux/bwrap is untested and may differ, because `--unshare-all` does not hide filesystem sockets under a bind-mounted `/tmp`. |
| C3 | — | **refuted** | R11-A2 (clean-exit leak) | `std::process::exit` skips `Drop`, but the worker exits on stdin EOF (`worker.py:227`). 0 workers left after probe runs. |
| **N2** | **Medium** | **new** | `agent.rs:82-83`, `compact.rs:6-19, 76-78` | Turn-start compaction can cut the answer the user is replying to down to **160 chars** (probe: 62,638 B → 160 chars). `cut()` also counts bytes that `messages()` never sends (`reasoning_content`, `meta`). |
| **N3** | Low-Med | **new** · corrects a design assumption | `compact.rs:14`, `session.rs:97` | The summary is sent as a `user` message. When `cut()` lands on a user entry, two `user` messages go back to back. Probe-confirmed. This is common, not an edge case. |
| **N4** | Low-Med | **new** · corrects R06-S6 | `ui.rs:384-387`, `agent.rs:155`, `main.rs:93` | `ui::note` and the top-level `eprintln!` print model- and provider-controlled bytes raw. Probe: a tool name carrying `ESC ] 0;… BEL` reached the terminal. (`thinking()` *is* filtered via `printable()`.) |
| N5 | Low | **new** | `worker.py:200-201` | On Python 3.9 the "no match statements or X \| Y types" hint is attached to *every* `SyntaxError`. Probe: shown for a plain unterminated string, which misdirects the model. |
| C4 | — | known tradeoff | R10-K3 / R11-C3 | Accepting an unmarked stream end was a deliberate change (`b2b59d7`). The remaining gap is narrow: a text-only answer cut off by a clean close is accepted as final. Not re-argued here. |

Domains 1, 4, 5 and 6 were read in full. Apart from N5, nothing turned up that rounds 01–11 don't already list.

---

## 2. Deep-Dive Analysis

### 2.1 N1 — the localhost clause is an allow-all

`worker.rs:73-74` emits:

```
(deny network*)
(allow network* (local ip "localhost:*") (remote ip "localhost:*"))
```

Filters inside one `allow` are OR'd. The `(local ip "localhost:*")` filter matches outbound connects whatever the remote address is. Isolated by clause:

```
$ for a in '(allow network* (local ip "localhost:*"))' '(allow network* (remote ip "localhost:*"))'; do
    sandbox-exec -p "(version 1)(allow default)(deny network*)$a" python3 -I -c \
      'import socket;s=socket.socket();s.settimeout(3);s.connect(("1.1.1.1",443));print("OUT OPEN")'; done
OUT OPEN                                   ← local ip clause alone
PermissionError: [Errno 1] Operation not permitted   ← remote ip clause alone
```

Real data flows, over TCP and over UDP:

```
HTTP egress: b'HTTP/1.1 301 Moved Permanently'
DNS: gaierror [Errno 8] nodename nor servname provided, or not known   ← system resolver blocked
UDP DNS to 1.1.1.1: 61 bytes                                           ← own DNS query answered
```

**End-to-end through the real binary.** A one-shot `air "probe"` run, with the fake server scripting a single `run()`, connected to `1.1.1.1:80` from the sandboxed worker. The tool result that went back to the model was:

```
EGRESS b'HTTP/1.1 301 Moved Permanently\r\nServer: '
```

**Why every prior check "saw" denial:** anything that resolves names through the system (curl, pip, cargo, git over https) fails at DNS. Raw IP connects, a hand-rolled DNS query, or a resolver library that talks UDP/53 directly all get through. An injected instruction in a README of the form "run this one-liner" is exactly that shape.

**Impact on earlier findings.** S2's claim that "the network-deny never engages: the exfil channel is the sanctioned LLM request" does not hold: a direct channel exists, so secrets readable through the deny-list gaps can leave without passing through the provider. S3's "network-denied builds must consume the shared cache" and L9's "network-deny friction" rest on the same false premise. Every session since `630117a` ran with network open. That matters for any repository air was pointed at that the user does not trust.

**Linux** uses `--unshare-all`, which removes the network namespace entirely, localhost included. So the two platforms already diverge, and after the fix macOS is the more permissive of the two (localhost only).

### 2.2 C1 — the cache cost of reasoning stripping is bounded, and the proposed fix regresses

`messages()` drops `reasoning_content` from assistant entries with `i < newest`. When a new user task arrives, only the entries between the **previous** task and the new one lose the field. Earlier turns lost it at their own boundary, and their bytes have been stable (and cached) since. The re-prefill is therefore at most one previous turn, paid once, and it buys a permanently smaller prefix. Earlier measurements put reasoning at 54% of the largest real prompt.

Measured from the usage records in the 20 real logs in `~/.air/sessions`:

```
sessions=20 multi-turn=2 turns=23
first request of a turn: prompt=23282 uncached=6514 (27%)    ← includes 20 cold session starts
other requests:          prompt=2716236 uncached=254652 (9%)
reasoning bytes newly stripped at turn boundaries: 4611 (~1152 tok)
```

The sample is small: 2 multi-turn sessions and 3 non-first boundaries. It contradicts "millions of wasted input tokens per session" by roughly three orders of magnitude.

Round 10's fix, which strips at `push()` and renames the field to `reasoning_content_log`, would remove `reasoning_content` from in-turn assistant messages as well. The MiMo integration was verified with that echo present ("reasoning_content echo accepted"), and interleaved-thinking providers expect it on tool-call continuations. **Do not apply it.**

### 2.3 N2 + N3 — the turn-start compaction boundary

`turn()` compacts *before* pushing the new task when `last_prompt_tokens() ≥ compact_at/2`, keeping a tail of `compact_at/10` tokens. `cut()` walks backward from the newest entry and stops as soon as the estimated tail exceeds that budget. At a turn boundary the newest entry is the previous turn's final answer.

* If that single entry is larger than `compact_at/10 × 4` bytes, **counting `reasoning_content` and `meta` that are never sent**, nothing fits and `keep = len`. The answer is then summarized. `summary()` only quotes assistants that come *before* a recent user message (`compact.rs:30-36`), so a final answer that comes after the last user message appears only in `Status` as `line(…, 160)` (`compact.rs:76-78`).
* The threshold shrinks with the learned `compact_at`. For a model with a 32k window, `compact_at` settles at about 29k and the cutoff drops to about 12 KB. The largest real final entry measured so far is 6.4 KB.

Probe: turn 1 returns a 62,638-byte answer with `prompt_tokens: 60000`; turn 2 is run with `--resume "task2: go with option 2"`. The request captured for turn 2:

```
[system] 2901 bytes
[user]   633 bytes      ← summary
[user]   23 bytes       ← "task2: go with option 2"
```

and the entire record of the answer in the summary:

```
## Status
last assistant note: #2 Option 1: this paragraph explains the tradeoffs of the approach in detail. this paragraph explains the tradeoffs of the approach in detail. this paragraph expla…
```

Option 2, which the user just chose, is gone. The model's only way to get it back is to notice the gap and call `recall(2)`.

**N3** is independent of size. `cut()` accepts any non-tool entry as the boundary (`role != "tool"`), and `messages()` always emits the summary as `role: user`. Whenever the boundary entry is a user task or a host gate message, the provider receives `user, user`. For an in-turn compaction this happens whenever the current task fits inside the kept tail. The earlier fix ("append the task to the summary; never two consecutive user messages") only covers the case where the task falls *before* `keep`. Whether this matters depends on the provider: OpenAI-compatible endpoints accept it, while strict-alternation endpoints reject it. One real request against MiMo with this shape would settle it for the configured provider.

### 2.4 N4 — the unfiltered terminal sinks

`printable()` is applied to thinking, markdown, run labels, tool output and the report. It is not applied to:

* `ui::note` (`ui.rs:384`). Its callers include `agent.rs:155`, with the **model-supplied tool name**; `llm.rs:76` and `llm.rs:249`, with transport and provider error text; and `agent.rs:176`, with worker errors.
* `main.rs:93` `eprintln!("error: {e:#}")`, which carries `llm.rs:242` `bail!("{status}: {text}")`, i.e. the raw provider error body.

Probe (fake server returns the tool name `"\u001b]0;AIR-TITLE-INJECTED\u0007evil"`), viewed through `cat -v`:

```
· the model called ^[]0;AIR-TITLE-INJECTED^Gevil, but the only tool is run(code)
```

`OSC 0` sets the window title. The same path carries `OSC 52`, which writes the clipboard on terminals that allow it, and cursor or scroll-region sequences that can hide or forge report lines. A prompt-injected model controls the tool name. This corrects R06-S6, which named `thinking()` as the only unfiltered sink.

### 2.5 N5 — the Python 3.9 hint misfires

`_hint` returns the "no match statements or X | Y types" advice for **every** `SyntaxError` on 3.9. In the probe it was attached to `SyntaxError: EOL while scanning string literal`. The model gets a wrong diagnosis on the error it hits most often, which costs a round-trip. (On 3.9, `X | Y` in annotations raises `TypeError`, not `SyntaxError`, so only `match`/`case` belongs in this hint.)

---

## 3. Refactoring (smallest diffs, comment-free per house style)

### 3.1 N1 — `worker.rs:73-74`

```rust
            "(version 1)(allow default)(deny network*)\
             (allow network-outbound (remote ip \"localhost:*\"))\
             (allow network-bind network-inbound (local ip \"localhost:*\"))\
```

Verified with the full profile (writable dirs, `.git` deny and tty ioctl deny included):

```
tcp egress blocked: PermissionError
udp egress blocked: PermissionError
self-serve 127.0.0.1 b'ok'          ← tests that start a local server still work
self-serve ::1 b'ok'
unix own socket blocked: PermissionError
write repo ok
write .git blocked: PermissionError
```

Unix sockets were already blocked before the fix and stay blocked, so anything that needs a local daemon socket (Docker, for example) was never available inside the sandbox.

### 3.2 N4 — `ui.rs:384` and `main.rs:93`

```rust
pub fn note(text: &str) {
    clear_live();
    println!("{}", paint("33", &format!("· {}", printable(text))));
}
```

```rust
        if let Err(e) = result {
            eprintln!("error: {}", ui::printable(&format!("{e:#}")));
        }
```

### 3.3 N2 + N3 — `compact.rs`

Make the boundary land only on assistant entries, so the summary is always followed by an assistant message, and measure only the bytes that are actually sent:

```rust
pub fn cut(entries: &[Value], prev: usize, keep_tokens: usize) -> usize {
    let (mut tail, mut keep) = (0, entries.len());
    for i in (prev + 1..entries.len()).rev() {
        let Some(role) = entries[i]["role"].as_str() else { continue };
        tail += (entries[i]["content"].as_str().map_or(0, str::len) + entries[i]["tool_calls"].to_string().len()) / 4;
        if tail > keep_tokens {
            break;
        }
        if role == "assistant" {
            keep = i;
        }
    }
    keep
}
```

A user task just before `keep` is already appended verbatim to the summary by `session.rs:93-96`. Older user directions keep their 2000-byte excerpt.

Quote the latest reply when it falls into the summarized range. In `summary()`, after the `for &u in recent` loop:

```rust
    let after = users.last().map_or(1, |&u| u + 1);
    if let Some(a) = (after..keep).rev().find(|&i| role(i) == "assistant" && said(i)) {
        dialogue.push(format!("#{a} assistant, latest reply (historical, not a verified outcome):\n{}", excerpt(text(a), 4000)));
    }
```

and raise the `Recent user directions` cap from `8_192` to `12_288` so `block()` doesn't immediately drop the middle.

### 3.4 N5 — `worker.py:200`

```python
    if isinstance(e, SyntaxError) and sys.version_info < (3, 10) and (e.text or "").lstrip().startswith(("match ", "case ")):
```

---

## 4. Remediation & Verification

**On testing.** This repo has a standing rule: no tests in `air`; verify with `cargo build` plus manual runs. The property-based tests and token fuzzing the brief asks for are therefore replaced by the rerunnable probes below. Keep them in a scratch directory, not the repo.

| # | Action | Verify |
|---|--------|--------|
| 1 | **N1 now.** Apply 3.1. Separately, review any untrusted repo air has been run on, since network was open in those sessions. | Rerun the clause probe in 2.1 with the new profile and expect `PermissionError` for TCP and UDP. Rerun the self-serve probe. Then run one real `air` turn whose `check` binds localhost (if the project has one). |
| 2 | Re-rate S2/S3 with network actually denied, and drop the "network-denied" premise from L9. | n/a (documentation) |
| 3 | **N4.** Apply 3.2. | Fake server returns a tool name with `\u001b]0;X\u0007`; `cat -v` of the output must show no `^[`. |
| 4 | **N2 + N3.** Apply 3.3. | Rerun the two-turn probe (appendix). Expect the summary to be followed by an assistant message and at least 4000 B of the answer present. Also replay `cut`/`summary` at every turn boundary of the logs in `~/.air/sessions` with a scratch script and check alternation. |
| 5 | **N5.** Apply 3.4. | Run `print("x` through the worker; expect no 3.9 hint. Run `match x:` and expect the hint. |
| 6 | **C1: do nothing.** Reject R10 §3.2. | Keep watching `uncached` on the first request of each turn in long REPL sessions; act only if it exceeds about 10% of a turn's total. |

### Appendix — fake SSE server used for the end-to-end probes

```python
import json, sys, http.server, itertools
script = json.load(open(sys.argv[1])); logdir = sys.argv[2]; n = itertools.count()
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        i = next(n); json.dump(body, open(f"{logdir}/req{i}.json", "w"), indent=1)
        step = script[min(i, len(script) - 1)]
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.end_headers()
        def ev(d): self.wfile.write(b"data: " + json.dumps(d).encode() + b"\n\n")
        if "text" in step:
            ev({"choices": [{"index": 0, "delta": {"content": step["text"]}}]})
        if "call" in step:
            name, code = step["call"]
            ev({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": f"c{i}", "type": "function", "function": {"name": name, "arguments": json.dumps({"code": code})}}]}}]})
        ev({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls" if "call" in step else "stop"}]})
        ev({"choices": [], "usage": {"prompt_tokens": step.get("prompt", 1000), "completion_tokens": 10}})
        self.wfile.write(b"data: [DONE]\n\n")
http.server.HTTPServer(("127.0.0.1", int(sys.argv[3])), H).serve_forever()
```

Run it as `python3 fake.py script.json reqs/ PORT`, with a scratch `HOME` whose `.config/air/config.toml` has `model = "fake"` and `base_url = "http://127.0.0.1:PORT/v1"`, `AIR_API_KEY=x`, inside a throwaway git repo. The N2 script: `[{"text": <62 KB of "Option k: …\n\n" paragraphs>, "prompt": 60000}, …]`, then `air "task1"` followed by `air --resume "task2: go with option 2"`.
