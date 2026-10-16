"""Graders. Each prints one JSON line {pass, score, detail} and exits 0 on a pass, 1 otherwise."""
import gzip
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile

LIB = os.path.dirname(os.path.abspath(__file__))
TASKS = os.path.join(os.path.dirname(LIB), "tasks")
sys.path.insert(0, LIB)
import legacy as old  # noqa: E402


def done(passed, score, detail):
    print(json.dumps({"pass": bool(passed), "score": score, "detail": detail}))
    sys.exit(0 if passed else 1)


def sh(cmd, cwd, env=None, timeout=900):
    e = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", CARGO_NET_OFFLINE="true", **(env or {}))
    r = subprocess.run(cmd, cwd=cwd, env=e, capture_output=True, text=True, timeout=timeout, shell=isinstance(cmd, str))
    return r.returncode, r.stdout + r.stderr


def answer_of(path):
    try:
        return open(path, encoding="utf-8").read()
    except (OSError, TypeError):
        return ""


def legacy(task):
    r = old.check(task, sys.argv[1], answer_of(sys.argv[2] if len(sys.argv) > 2 else None))
    done(r["pass"], r["score"], r.get("detail", ""))


# ---------------------------------------------------------------- ops
LINE = re.compile(r'^\S+ \S+ \S+ \[[^\]]+\] "\S+ (\S+) [^"]*" (\d{3}) ')


def expected_errors(logdir):
    counts = {}
    for name in sorted(os.listdir(logdir)):
        path = os.path.join(logdir, name)
        data = gzip.open(path, "rt").read() if name.endswith(".gz") else open(path).read()
        for line in data.splitlines():
            m = LINE.match(line)
            if m and m.group(2).startswith("5"):
                endpoint = m.group(1).split("?")[0]
                counts[endpoint] = counts.get(endpoint, 0) + 1
    rows = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0].encode()))
    return "endpoint,count\n" + "".join(f"{k},{v}\n" for k, v in rows)


def hidden_logs(logdir):
    rng = random.Random(1234)
    paths = ["/", "/api/v2/users", "/api/v2/users/7", "/api/items", "/checkout", "/api/pay", "/static/x.css", "/api/a", "/api/b"]
    os.makedirs(logdir)
    for k in range(4):
        lines = []
        for _ in range(300):
            path = rng.choice(paths)
            if rng.random() < 0.5:
                path += f"?q={rng.randrange(99)}&page={rng.randrange(5)}"
            status = rng.choice([200, 200, 200, 404, 500, 502, 503, 301])
            lines.append(f'10.1.1.{rng.randrange(250)} - - [12/Oct/2026:0{k}:00:00 +0000] "GET {path} HTTP/1.1" {status} 12 "-" "x"')
        lines.append('10.1.1.1 - - [12/Oct/2026:09:00:00 +0000] "-" 400 0 "-" "-"')
        text = "\n".join(lines) + "\n"
        name = "access.log" + ("" if k == 0 else f".{k}")
        if k >= 2:
            with open(os.path.join(logdir, name + ".gz"), "wb") as f:
                f.write(gzip.compress(text.encode(), mtime=0))
        else:
            with open(os.path.join(logdir, name), "w") as f:
                f.write(text)


def ops(repo):
    parts = {}
    template_logs = os.path.join(TASKS, "ops", "template", "logs")
    want = expected_errors(template_logs)
    try:
        got = open(os.path.join(repo, "reports", "errors.csv")).read()
    except OSError:
        got = ""
    parts["report_up_to_date"] = got == want
    parts["logs_untouched"] = all(
        open(os.path.join(repo, "logs", n), "rb").read() == open(os.path.join(template_logs, n), "rb").read()
        for n in os.listdir(template_logs))
    with tempfile.TemporaryDirectory() as d:
        shutil.copytree(os.path.join(repo, "scripts"), os.path.join(d, "scripts"))
        hidden_logs(os.path.join(d, "logs"))
        rc, out = sh("sh scripts/report.sh", d, {"LC_ALL": "C", "LANG": "C"}, timeout=120)
        try:
            hidden = open(os.path.join(d, "reports", "errors.csv")).read()
        except OSError:
            hidden = ""
        parts["hidden_logs"] = rc == 0 and hidden == expected_errors(os.path.join(d, "logs"))
    done(all(parts.values()), f"{sum(parts.values())}/{len(parts)}", parts)


# ---------------------------------------------------------------- wide
def norm(cell):
    cell = cell.strip().strip("`").strip()
    if len(cell) >= 2 and cell[0] == cell[-1] and cell[0] in "'\"":
        cell = cell[1:-1]
    return re.sub(r"(?<=\d)_(?=\d)", "", cell).replace(" ", "")


def wide(repo):
    want = [tuple(norm(c) for c in row) for row in json.load(open(os.path.join(TASKS, "wide", "expected.json")))]
    try:
        text = open(os.path.join(repo, "docs", "CONFIG.md"), encoding="utf-8").read()
    except OSError:
        text = ""
    rows = []
    for line in text.splitlines():
        if not line.strip().startswith("|"):
            continue
        cells = [c for c in line.strip().strip("|").split("|")]
        if len(cells) != 3 or all(set(c.strip()) <= set("-: ") for c in cells) or cells[0].strip().strip("`").lower() == "key":
            continue
        rows.append(tuple(norm(c) for c in cells))
    others = [p for p in old.changed(repo) if p != "docs/CONFIG.md"]
    missing, extra = sorted(set(want) - set(rows)), sorted(set(rows) - set(want))
    ordered = rows == want
    done(ordered and not others, f"{len(set(want) & set(rows))}/{len(want)} rows, ordered={ordered}",
         {"missing": missing[:8], "extra": extra[:8], "other_files": others})


# ---------------------------------------------------------------- recovery
HIDDEN_CALC = [("-3", -3), ("-(2+3)", -5), ("2*-3", -6), ("--4", 4), ("4 - - - 1", 3), ("-2*3", -6), ("3--2", 5),
               ("1 - -(2)", 3), ("2-3", -1), ("10-4-3", 3), ("(1+2)*-(3-1)", -6), ("-1.5*2", -3.0)]


def recovery(repo):
    code = "import calc\n" + "".join(
        f"try:\n    print(int(calc.evaluate({src!r}) == {val!r}))\nexcept Exception:\n    print(0)\n" for src, val in HIDDEN_CALC)
    rc, out = sh([sys.executable, "-E", "-s", "-c", code], repo)
    hidden = [int(x) for x in out.split()] if rc == 0 and len(out.split()) == len(HIDDEN_CALC) else [0] * len(HIDDEN_CALC)
    tests_ok = old.unittest_ok(repo)
    kept = open(os.path.join(repo, "test_calc.py")).read() == open(os.path.join(TASKS, "recovery", "template", "test_calc.py")).read()
    done(all(hidden) and tests_ok and kept, f"{sum(hidden)}/{len(hidden)} hidden, tests={tests_ok}, tests kept={kept}",
         [src for (src, _), ok in zip(HIDDEN_CALC, hidden) if not ok])


# ---------------------------------------------------------------- lowqa
QA_KEY = {
    1: (r"100[,_ ]?000|100\s?k\b", [("src/agent.rs", r"^const COMPACT_AT")]),
    2: (r"32[,_ ]?000", [("src/worker.py", r"^HALF = "), ("src/worker.py", r"if len\(text\) > 2 \* HALF"), ("src/prompt.md", r"32,000")]),
    3: (r"LOW_CHECK", [("src/worker.py", r'LOW_CHECK'), ("src/worker.rs", r'"LOW_CHECK"')]),
    4: (r"(?<![\d,._])200(?![\d,._]*\d)", [("src/agent.rs", r"^const MAX_REQUESTS")]),
}


def sections(answer):
    """The answer split at its question numbers 1-4, else the whole answer for each."""
    starts = []
    for n in range(1, 5):
        m = re.search(rf"(?m)^\W{{0,6}}(?:Q|Question\s*)?{n}\s*[.):]", answer[starts[-1] if starts else 0:])
        if not m:
            return {n: answer for n in range(1, 5)}
        starts.append((starts[-1] if starts else 0) + m.start())
    starts.append(len(answer))
    return {n: answer[starts[n - 1]:starts[n]] for n in range(1, 5)}


def lowqa(repo, answer_path):
    answer = answer_of(answer_path)
    parts, cites = sections(answer), {}
    correct = {n: bool(re.search(pat, parts[n], re.I)) for n, (pat, _) in QA_KEY.items()}
    if correct[2]:
        correct[2] = bool(re.search(r"middle|first (and|\+) last|head and tail|keeps? the (first|start)|16[,_ ]?000", parts[2], re.I))
    for n, (_, anchors) in QA_KEY.items():
        found = re.findall(r"([\w./-]+\.(?:rs|py|md)):(\d+)(?:[-–](\d+))?", parts[n])
        hit = False
        for rel, pat in anchors:
            lines = [i for i, l in enumerate(open(os.path.join(repo, rel), encoding="utf-8"), 1) if re.search(pat, l)]
            hit |= any(f.endswith(os.path.basename(rel)) and any(int(a) - 15 <= L <= int(b or a) + 15 for L in lines)
                       for f, a, b in found)
        cites[n] = hit
    untouched = not old.changed(repo)
    done(all(correct.values()) and untouched, f"answers {sum(correct.values())}/4, cites {sum(cites.values())}/4",
         {"correct": correct, "cites": cites, "untouched": untouched})


# ---------------------------------------------------------------- real commits: hidden tests appended to the agent's tree
def cargo_hidden(repo, task_dir):
    spec = json.load(open(os.path.join(task_dir, "hidden.json")))
    hidden = open(os.path.join(task_dir, "hidden.rs")).read()
    detail = {}
    with tempfile.TemporaryDirectory() as d:
        copy = os.path.join(d, "r")
        if subprocess.run(["cp", "-cR", repo, copy], capture_output=True).returncode:
            shutil.copytree(repo, copy, symlinks=True)
        with open(os.path.join(copy, spec["file"]), "a") as f:
            f.write("\n" + hidden)
        rc, out = sh("cargo test -q hidden_tests 2>&1", copy, timeout=1500)
        m = re.search(r"test result: \w+\. (\d+) passed; (\d+) failed", out)
        detail["hidden"] = f"{m.group(1)} passed, {m.group(2)} failed" if m else out.strip().splitlines()[-3:] if out.strip() else "no output"
        hidden_ok = rc == 0 and m is not None and int(m.group(1)) == spec["tests"] and int(m.group(2)) == 0
        rc2, out2 = sh("cargo test -q 2>&1", copy, timeout=1500)
        detail["all_tests"] = rc2 == 0
        if rc2:
            detail["failures"] = [l for l in out2.splitlines() if "FAILED" in l or "panicked" in l or l.startswith("error")][:6]
    for rel, needle in spec.get("absent", {}).items():
        detail[f"no {needle!r} in {rel}"] = needle not in open(os.path.join(repo, rel)).read()
    ok = hidden_ok and detail["all_tests"] and all(v for k, v in detail.items() if k.startswith("no "))
    done(ok, f"hidden={'ok' if hidden_ok else 'FAIL'} all_tests={detail['all_tests']}", detail)
