#!/usr/bin/env python3
"""air's benchmark: run each task N times per variant, grade it, and append one row per run to results.jsonl.

  bench.py run --variant REF --model MODEL --n N [--tasks a,b] [--jobs 4] [--label NAME]
  bench.py report --baseline LABEL --variant LABEL [--model MODEL] [--results FILE]

A variant is a git ref of air (built once per commit), or `opencode` for OpenCode itself on the same model.
Runs live in scratch repos under $AIR_BENCH_RUNS (default $TMPDIR/air-bench/runs); air runs in its own sandbox.
"""
import argparse
import concurrent.futures
import difflib
import fcntl
import hashlib
import json
import os
import random
import re
import shutil
import signal
import statistics
import subprocess
import sys
import threading
import time

BENCH = os.path.dirname(os.path.abspath(__file__))
AIR = os.path.dirname(os.path.dirname(BENCH))
TMP = os.environ.get("TMPDIR", "/tmp")
RUNS = os.environ.get("AIR_BENCH_RUNS", os.path.join(TMP, "air-bench", "runs"))
CACHE = os.environ.get("AIR_BENCH_CACHE", os.path.join(TMP, "air-bench", "cache"))
RESULTS = os.path.join(BENCH, "results.jsonl")
# Nominal $/M tokens (uncached input, cached input, output). OpenCode Go is a subscription and returns no cost, so this
# is DeepSeek's public list price, used as a cost index rather than a bill.
PRICES = {"deepseek-v4.1-flash": (0.28, 0.028, 0.42)}
OPENCODE_MODEL = {"deepseek-v4.1-flash": "opencode-go/deepseek-v4.1-flash"}
LOCK = threading.Lock()
JUNK = re.compile(r"(^|/)(target|__pycache__|\.opencode|\.pi)(/|$)|\.pyc$|^opencode\.json$")


def sh(cmd, cwd=None, env=None, check=True, timeout=None):
    r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout, shell=isinstance(cmd, str))
    if check and r.returncode:
        raise RuntimeError(f"{cmd}: exit {r.returncode}: {(r.stdout + r.stderr)[-2000:]}")
    return r.stdout


def tasks():
    root = os.path.join(BENCH, "tasks")
    return {t: json.load(open(os.path.join(root, t, "task.json"))) for t in sorted(os.listdir(root))
            if os.path.exists(os.path.join(root, t, "task.json"))}


# ---------------------------------------------------------------- build and prepare
def build(variant):
    """The air binary for a git ref, built once per commit in a worktree under the cache."""
    if variant == "opencode":
        return None, "opencode"
    sha = sh(["git", "rev-parse", "--short=10", variant + "^{commit}"], cwd=AIR).strip()
    out = os.path.join(CACHE, "bin", sha, "air")
    os.makedirs(CACHE, exist_ok=True)
    with open(os.path.join(CACHE, "build.lock"), "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)  # one build at a time in the shared worktree
        if not os.path.exists(out):
            _build(sha, out)
    version = sh([out, "--version"])
    if sha[:7] not in version:
        raise RuntimeError(f"{out} reports {version.strip()!r}, not {sha}")
    return out, sha


def _build(sha, out):
    wt = os.path.join(CACHE, "wt")
    if not os.path.isdir(wt):
        sh(["git", "worktree", "add", "-q", "--detach", wt, sha], cwd=AIR)
    else:
        sh(["git", "checkout", "-q", "--detach", sha], cwd=wt)
    print(f"building air {sha} …", flush=True)
    sh(["cargo", "build", "--release", "-q"], cwd=wt, timeout=1800)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    shutil.copy2(os.path.join(wt, "target", "release", "air"), out)


def prepare(names):
    """A warm target/ for each real-commit task's parent tree, so a run's first check() doesn't build every crate."""
    for name in names:
        setup = open(os.path.join(BENCH, "tasks", name, "setup.sh")).read()
        m = re.search(r"rev-parse --short (\S+)\)", setup)
        if not m:
            continue
        parent = sh(["git", "rev-parse", "--short", m.group(1)], cwd=AIR).strip()
        target = os.path.join(CACHE, parent, "target")
        if os.path.isdir(target):
            continue
        print(f"warming target/ for {name} at {parent} …", flush=True)
        src = os.path.join(CACHE, parent, "src")
        shutil.rmtree(src, ignore_errors=True)
        os.makedirs(src)
        subprocess.run(f"git -C {AIR} archive {parent} | tar -x -C {src}", shell=True, check=True)
        sh("cargo build -q && cargo clippy -q --all-targets && cargo test -q --no-run", cwd=src, timeout=3600)
        os.rename(os.path.join(src, "target"), target)
        shutil.rmtree(src)


# ---------------------------------------------------------------- one run
def opencode_home():
    """A scratch HOME for OpenCode holding only links to its own data dir (auth, sessions) and ~/.gitconfig: OpenCode
    otherwise reads Claude Code's ~/.claude, which the sandbox hides, and refuses to start."""
    real = os.path.realpath(os.path.expanduser("~"))
    home = os.path.join(CACHE, "opencode-home")
    for rel in (".local/share/opencode", ".gitconfig"):
        link = os.path.join(home, rel)
        if not os.path.lexists(link) and os.path.lexists(os.path.join(real, rel)):
            os.makedirs(os.path.dirname(link), exist_ok=True)
            os.symlink(os.path.join(real, rel), link)
    return real, home


def opencode_cmd(repo, model, prompt):
    """OpenCode under a sandbox-exec profile of our own: network on (it must reach the gateway), writes only to the run
    repo, temp and OpenCode's own dirs, and the same home secrets as air's sandbox hidden. --standalone keeps its tool
    server a child of this process (the shared background service runs elsewhere, unsandboxed, in its own cwd)."""
    real, home = opencode_home()
    writable = [os.path.realpath(repo), os.path.realpath(TMP), "/private/tmp", "/dev", home, os.path.join(real, ".local/share/opencode")]
    secrets = re.search(r'const SECRETS: &str = "(.*?)";', open(os.path.join(AIR, "src", "worker.rs")).read(), re.S).group(1)
    secrets = [os.path.join(real, s.strip()) for s in re.sub(r"\\\n\s*", "", secrets).split(",") if s.strip()]
    profile = ("(version 1)(allow default)(deny file-write*)(allow file-write*" + "".join(f' (subpath "{d}")' for d in writable) + ")"
               "(deny file-read-data" + "".join(f' (subpath "{s}")' for s in secrets) + ")")
    cmd = ["/usr/bin/sandbox-exec", "-p", profile, "opencode", "run", "--standalone", "-m", OPENCODE_MODEL.get(model, model),
           "--format", "json", "--auto", prompt]
    return cmd, dict(os.environ, HOME=home, XDG_CONFIG_HOME=os.path.join(home, ".config"))


def air_metrics(session):
    es = [json.loads(l) for l in open(session) if l.startswith("{")]
    m = {"requests": 0, "runs": 0, "tool_errors": 0, "rejected": 0, "edit_failures": 0, "prompt": 0, "cached": 0, "out": 0,
         "reasoning": 0, "model_ms": 0, "tools": {}, "events": {}, "cache_rates": []}
    tools = m["tools"]

    def count(key, n=1):
        tools[key] = tools.get(key, 0) + n
    for e in es:
        meta = e.get("meta") or {}
        if e.get("event"):
            m["events"][e["event"]] = m["events"].get(e["event"], 0) + 1
            if e["event"] == "interrupted":  # a cut stream was a model request too, as air's own stats count it
                m["requests"] += 1
        if e.get("role") == "assistant" and meta.get("usage"):
            u = meta["usage"]
            m["requests"] += 1
            p = u.get("prompt_tokens") or 0
            c = (u.get("prompt_tokens_details") or {}).get("cached_tokens") or u.get("prompt_cache_hit_tokens") or 0
            m["prompt"] += p
            m["cached"] += c
            m["out"] += u.get("completion_tokens") or 0
            m["reasoning"] += (u.get("completion_tokens_details") or {}).get("reasoning_tokens") or 0
            m["model_ms"] += (meta.get("timing") or {}).get("ms") or 0
            m["cache_rates"].append(round(c / p, 3) if p else 0)
            for call in e.get("tool_calls") or []:
                try:
                    code = json.loads(call["function"]["arguments"]).get("code", "")
                except (ValueError, AttributeError):
                    code = ""
                for helper in ("accept", "plan", "done"):
                    count(helper, len(re.findall(rf"(?<![\w.]){helper}\(", code)))
        if e.get("role") == "tool":
            m["runs"] += 1
            m["tool_errors"] += int(meta.get("error") is True)
            m["rejected"] += int(meta.get("rejected") is True)
            count("show", len(meta.get("reads") or []))
            count("sh", len(meta.get("cmds") or []))
            for a in meta.get("actions") or []:
                count(a.get("tool", "?"))
                m["edit_failures"] += int(bool(a.get("path")) and a.get("ok") is False)
        if e.get("report"):
            m["verdict"] = e["report"].get("verdict")
    answers = [e.get("content") for e in es if e.get("role") == "assistant" and (e.get("content") or "").strip()]
    m["answer"] = answers[-1] if answers else ""
    m["params"] = es[0].get("params")
    return m


def opencode_metrics(log):
    """OpenCode's --format json event stream: one JSON object per line."""
    m = {"requests": 0, "runs": 0, "tool_errors": 0, "rejected": 0, "edit_failures": 0, "prompt": 0, "cached": 0, "out": 0,
         "reasoning": 0, "model_ms": 0, "tools": {}, "events": {}, "cache_rates": []}
    texts = []
    for line in open(log, errors="replace"):
        try:
            e = json.loads(line)
        except ValueError:
            continue
        kind = e.get("type")
        part = e.get("part") or {}
        if kind == "step_finish":
            t = part.get("tokens") or {}
            cache = t.get("cache") or {}
            m["requests"] += 1
            prompt = (t.get("input") or 0) + (cache.get("read") or 0) + (cache.get("write") or 0)
            m["prompt"] += prompt
            m["cached"] += cache.get("read") or 0
            m["out"] += (t.get("output") or 0) + (t.get("reasoning") or 0)
            m["reasoning"] += t.get("reasoning") or 0
            m["cache_rates"].append(round((cache.get("read") or 0) / prompt, 3) if prompt else 0)
        elif kind == "tool_use":
            m["runs"] += 1
            name = part.get("tool", "?")
            m["tools"][name] = m["tools"].get(name, 0) + 1
            if (part.get("state") or {}).get("status") == "error":
                m["tool_errors"] += 1
        elif kind == "text" and (part.get("text") or "").strip():
            texts.append(part["text"])
        elif kind == "error":
            m["events"]["error"] = m["events"].get("error", 0) + 1
    m["answer"] = texts[-1] if texts else ""
    return m


def diff(repo, out):
    """Everything the run changed against the start commit, untracked files included, build junk left out."""
    idx = out + ".index"
    env = dict(os.environ, GIT_INDEX_FILE=idx)
    sh(["git", "read-tree", "HEAD"], cwd=repo, env=env)
    sh(["git", "add", "-A", "--", "."], cwd=repo, env=env, check=False)
    names = [n for n in sh(["git", "diff", "--cached", "--name-only", "HEAD"], cwd=repo, env=env).splitlines() if not JUNK.search(n)]
    text = sh(["git", "diff", "--cached", "--binary", "HEAD", "--"] + names, cwd=repo, env=env) if names else ""
    os.remove(idx)
    with open(out, "w") as f:
        f.write(text)
    return text, names


def run_one(job):
    task, spec, variant, binary, sha, model, k, label = job
    stamp = time.strftime("%m%d-%H%M%S")
    rundir = os.path.join(RUNS, f"{stamp}-{task}-{label}-{k}-{random.randrange(1 << 16):04x}")
    repo = os.path.join(rundir, "repo")
    os.makedirs(rundir)
    sh(["sh", os.path.join(BENCH, "tasks", task, "setup.sh"), repo])
    if variant == "opencode":
        with open(os.path.join(repo, "opencode.json"), "w") as f:
            json.dump({"$schema": "https://opencode.ai/config.json", "agent": {"build": {"temperature": 0}}}, f)
        cmd, env = opencode_cmd(repo, model, spec["prompt"])
    else:
        cmd, env = [binary, "-m", model, spec["prompt"]], None
    log = os.path.join(rundir, "out.log")
    t0 = time.time()
    with open(log, "w") as out:
        p = subprocess.Popen(cmd, cwd=repo, env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            exit_code = p.wait(timeout=spec["timeout"])
            timed_out = False
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGINT)
            try:
                exit_code = p.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, signal.SIGKILL)
                exit_code = p.wait()
            timed_out = True
    wall = round(time.time() - t0, 1)
    if variant == "opencode":
        m = opencode_metrics(log)
    else:
        found = re.search(r"\[session (\S+\.jsonl)\]", open(log, errors="replace").read())
        m = air_metrics(found.group(1)) if found else opencode_metrics(os.devnull)
        m["session"] = found.group(1) if found else None
    answer = os.path.join(rundir, "answer.md")
    with open(answer, "w") as f:
        f.write(m.pop("answer"))
    text, files = diff(repo, os.path.join(rundir, "diff.patch"))
    g = subprocess.run([sys.executable, os.path.join(BENCH, "tasks", task, "grade.py"), repo, answer], capture_output=True, text=True, timeout=3000)
    try:
        graded = json.loads(g.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        graded = {"score": "grader crashed", "detail": (g.stdout + g.stderr)[-500:]}
    price = PRICES.get(model, (0, 0, 0))
    cost = ((m["prompt"] - m["cached"]) * price[0] + m["cached"] * price[1] + m["out"] * price[2]) / 1e6
    row = {"task": task, "kind": spec["kind"], "variant": label, "build": sha, "model": model, "run": k, "pass": g.returncode == 0,
           "score": graded.get("score"), "detail": graded.get("detail"), "exit": exit_code, "timed_out": timed_out, "wall_s": wall,
           "turns": m["requests"], "tool_calls": m["runs"], "tools": m["tools"], "tool_errors": m["tool_errors"],
           "rejected": m["rejected"], "edit_failures": m["edit_failures"], "events": m["events"], "verdict": m.get("verdict"),
           "prompt": m["prompt"], "cached": m["cached"], "uncached": m["prompt"] - m["cached"], "out": m["out"],
           "reasoning": m["reasoning"], "cache_rates": m["cache_rates"], "model_s": round(m["model_ms"] / 1000, 1),
           "cost": round(cost, 6), "diff_sha": hashlib.sha256(text.encode()).hexdigest()[:16] if text else None,
           "diff_bytes": len(text), "diff_files": files, "answer_sha": hashlib.sha256(open(answer, "rb").read()).hexdigest()[:16],
           "params": m.get("params"), "rundir": rundir, "at": time.strftime("%Y-%m-%dT%H:%M:%S")}
    with LOCK:
        with open(RESULTS, "a") as f:
            f.write(json.dumps(row) + "\n")
    print(f"{task:13} {label:12} #{k} {'PASS' if row['pass'] else 'fail'} {str(row['score'])[:34]:34} turns={row['turns']:3} "
          f"calls={row['tool_calls']:3} uncached={row['uncached'] // 1000:4}k out={row['out'] // 1000:3}k {wall:6.0f}s", flush=True)
    return row


def cmd_run(args):
    every = tasks()
    names = args.tasks.split(",") if args.tasks else list(every)
    unknown = [n for n in names if n not in every]
    if unknown:
        sys.exit(f"unknown tasks: {', '.join(unknown)} (known: {', '.join(every)})")
    binary, sha = build(args.variant)
    prepare(names)
    label = args.label or (args.variant if args.variant == "opencode" else sha)
    # Interleave tasks so slow cargo tasks spread over the run instead of piling up at the end.
    jobs = [(t, every[t], args.variant, binary, sha, args.model, k, label) for k in range(1, args.n + 1) for t in names]
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        rows = list(pool.map(run_one, jobs))
    print(f"done: {sum(r['pass'] for r in rows)}/{len(rows)} passed · results in {RESULTS}")


# ---------------------------------------------------------------- report
def mean(xs):
    return statistics.mean(xs) if xs else 0.0


def sd(xs):
    return statistics.stdev(xs) if len(xs) > 1 else 0.0


def similarity(rows):
    """Mean pairwise normalized edit distance between the runs' final diffs (0 = identical)."""
    texts = []
    for r in rows:
        try:
            texts.append(open(os.path.join(r["rundir"], "diff.patch")).read().splitlines())
        except OSError:
            texts.append([])
    pairs = [(a, b) for i, a in enumerate(texts) for b in texts[i + 1:]]
    if not pairs:
        return 0.0
    return mean([1 - difflib.SequenceMatcher(None, a, b, autojunk=False).ratio() if (a or b) else 0.0 for a, b in pairs])


def outcome(r):
    return (r["pass"], r["diff_sha"] or f"score:{r['score']}")


def summary(rows):
    return {"n": len(rows), "pass": mean([r["pass"] for r in rows]), "turns": mean([r["turns"] for r in rows]),
            "turns_sd": sd([r["turns"] for r in rows]), "uncached": mean([r["uncached"] for r in rows]),
            "tokens_sd": sd([r["uncached"] + r["out"] for r in rows]), "out": mean([r["out"] for r in rows]),
            "cached": sum(r["cached"] for r in rows) / max(1, sum(r["prompt"] for r in rows)), "cost": mean([r["cost"] for r in rows]),
            "wall": mean([r["wall_s"] for r in rows]), "calls": mean([r["tool_calls"] for r in rows]),
            "errors": mean([r["tool_errors"] for r in rows]), "dist": similarity(rows),
            "outcomes": len({outcome(r) for r in rows})}


def bootstrap(base, var, key, n=4000, seed=1):
    """95% CI of the pooled per-run mean difference var − base, resampling runs within each task."""
    rng = random.Random(seed)
    tasks_ = sorted(set(base) & set(var))
    diffs = []
    for _ in range(n):
        b = [rng.choice(base[t])[key] for t in tasks_ for _ in base[t]]
        v = [rng.choice(var[t])[key] for t in tasks_ for _ in var[t]]
        diffs.append(mean([float(x) for x in v]) - mean([float(x) for x in b]))
    diffs.sort()
    return diffs[int(0.025 * n)], diffs[int(0.975 * n)]


def cmd_report(args):
    rows = [json.loads(l) for l in open(args.results) if l.strip()]
    rows = [r for r in rows if r["model"] == args.model]
    by = {}
    for r in rows:
        by.setdefault(r["variant"], {}).setdefault(r["task"], []).append(r)
    labels = [args.baseline] + [v for v in args.variant if v != args.baseline]
    missing = [v for v in labels if v not in by]
    if missing:
        sys.exit(f"no results for {missing}; have {sorted(by)}")
    out = [f"# Bench report · {args.model}", "",
           "Per task: pass rate · turns (sd) · uncached input tokens · output tokens · tokens sd (uncached+out) · cache hit · "
           "nominal cost · wall · diff distance (mean pairwise normalized edit distance) · distinct outcomes.", ""]
    head = "| task | variant | n | pass | turns (sd) | uncached in | out | tok sd | cached | cost $ | wall s | calls | tool err | diff dist | outcomes |"
    out += [head, "|" + "|".join(["---"] * (head.count("|") - 1)) + "|"]
    for task in sorted({t for v in labels for t in by[v]}):
        for v in labels:
            rs = by[v].get(task)
            if not rs:
                continue
            s = summary(rs)
            out.append(f"| {task} | {v} | {s['n']} | {s['pass']:.0%} | {s['turns']:.1f} ({s['turns_sd']:.1f}) | {s['uncached'] / 1000:.1f}k | "
                       f"{s['out'] / 1000:.1f}k | {s['tokens_sd'] / 1000:.1f}k | {s['cached']:.0%} | {s['cost']:.4f} | {s['wall']:.0f} | "
                       f"{s['calls']:.1f} | {s['errors']:.1f} | {s['dist']:.2f} | {s['outcomes']} |")
    out += ["", "## Pooled (tasks every variant ran)", "",
            "| variant | runs | pass | turns | uncached in | out | cached | cost $ | wall s | Δ turns vs base (95% CI) | Δ uncached (95% CI) | Δ out (95% CI) | Δ pass (95% CI) |",
            "|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    common = set.intersection(*[set(by[v]) for v in labels])
    for v in labels:
        rs = [r for t in common for r in by[v][t]]
        s = summary(rs)
        cells = []
        for key, scale in (("turns", 1), ("uncached", 1000), ("out", 1000), ("pass", 0.01)):
            if v == args.baseline:
                cells.append("—")
                continue
            lo, hi = bootstrap({t: by[args.baseline][t] for t in common}, {t: by[v][t] for t in common}, key)
            d = mean([float(r[key]) for r in rs]) - mean([float(r[key]) for t in common for r in by[args.baseline][t]])
            unit = "k" if scale == 1000 else ("pp" if key == "pass" else "")
            cells.append(f"{d / scale:+.1f}{unit} [{lo / scale:+.1f}, {hi / scale:+.1f}]")
        out.append(f"| {v} | {len(rs)} | {s['pass']:.0%} | {s['turns']:.2f} | {s['uncached'] / 1000:.1f}k | {s['out'] / 1000:.1f}k | "
                   f"{s['cached']:.0%} | {s['cost']:.4f} | {s['wall']:.0f} | " + " | ".join(cells) + " |")
    text = "\n".join(out) + "\n"
    if args.out:
        with open(args.out, "w") as f:
            f.write(text)
    print(text)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--variant", default="HEAD")
    r.add_argument("--model", default="deepseek-v4.1-flash")
    r.add_argument("--n", type=int, default=5)
    r.add_argument("--tasks", default="")
    r.add_argument("--jobs", type=int, default=4)
    r.add_argument("--label", default="")
    r.set_defaults(func=cmd_run)
    q = sub.add_parser("report")
    q.add_argument("--baseline", required=True)
    q.add_argument("--variant", nargs="+", default=[])
    q.add_argument("--model", default="deepseek-v4.1-flash")
    q.add_argument("--results", default=RESULTS)
    q.add_argument("--out", default="")
    q.set_defaults(func=cmd_report)
    args = p.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
