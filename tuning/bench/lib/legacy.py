import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.abspath(__file__))
TASKS = os.path.join(os.path.dirname(ROOT), "tasks")


def tmpl(task):
    return os.path.join(TASKS, task, "template")


def sh(cmd, cwd, env=None, timeout=300):
    e = dict(os.environ)
    e.update(env or {})
    e["PYTHONDONTWRITEBYTECODE"] = "1"
    r = subprocess.run(cmd, cwd=cwd, env=e, capture_output=True, text=True, timeout=timeout, shell=isinstance(cmd, str))
    return r.returncode, r.stdout + r.stderr


def py(repo, code):
    return sh([sys.executable, "-E", "-s", "-c", code], repo)


def changed(repo):
    code, out = sh("git status --porcelain --untracked-files=all", repo)
    return [l[3:] for l in out.splitlines() if not re.search(r"(^|/)(\.pi|\.opencode|__pycache__|target)(/|$)|^opencode\.json$|\.pyc$|^todo\.json$|^inventory\.json$|^exports/", l[3:])]


def unittest_ok(repo):
    code, out = sh([sys.executable, "-m", "unittest", "-q"], repo)
    return code == 0


def same_as_template(repo, task, rel):
    a = os.path.join(repo, rel)
    b = os.path.join(tmpl(task), rel)
    return os.path.exists(a) and open(a).read() == open(b).read()


def check_fix(repo, answer):
    code = (
        "import stats\n"
        "xs=[4,1,3,2]; a = stats.median(xs) == 2.5 and xs == [4,1,3,2]\n"
        "ys=[3,1,2]; b = stats.median(ys) == 2 and ys == [3,1,2]\n"
        "c = stats.median([5]) == 5 and stats.median([1.0, 2.0]) == 1.5\n"
        "d = stats.mean([1, 2, 3, 4]) == 2.5\n"
        "print(int(a), int(b), int(c), int(d))"
    )
    rc, out = py(repo, code)
    parts = [int(x) for x in out.split()] if rc == 0 and len(out.split()) == 4 else [0, 0, 0, 0]
    parts.append(int(unittest_ok(repo)))
    return {"pass": all(parts), "score": f"{sum(parts)}/5", "detail": out.strip()[-200:]}


def check_feature(repo, answer):
    results = []
    with tempfile.TemporaryDirectory() as d:
        env = {"TODO_FILE": os.path.join(d, "t.json")}

        def run(*args):
            return sh([sys.executable, "-m", "todo.cli", *args], repo, env)

        run("add", "a")
        run("add", "b", "--priority", "high")
        run("add", "c", "--priority", "low")
        run("add", "d")
        results.append(run("list")[1] == "1. [ ] a (normal)\n2. [ ] b (high)\n3. [ ] c (low)\n4. [ ] d (normal)\n")
        results.append(run("list", "--sort", "priority")[1] == "2. [ ] b (high)\n1. [ ] a (normal)\n4. [ ] d (normal)\n3. [ ] c (low)\n")
        run("done", "3")
        results.append("3. [x] c (low)" in run("list")[1])
        results.append(run("add", "z", "--priority", "urgent")[0] == 2)
        with open(env["TODO_FILE"], "w") as f:
            json.dump([{"title": "old", "done": False}], f)
        run("add", "e", "--priority", "high")
        results.append(run("list")[1] == "1. [ ] old (normal)\n2. [ ] e (high)\n")
        results.append(run("list", "--sort", "priority")[1] == "2. [ ] e (high)\n1. [ ] old (normal)\n")
    results.append(unittest_ok(repo))
    rc, out = sh("git diff --stat -- tests", repo)
    results.append(bool(out.strip()) or any(p.startswith("tests/") for p in changed(repo)))
    return {"pass": all(results), "score": f"{sum(results)}/{len(results)}", "detail": "".join("1" if r else "0" for r in results)}


def check_debug(repo, answer):
    cases = [
        ("[(10, 11), (13, 14)], 9, 17, 1", [(9, 10), (11, 13), (14, 17)]),
        ("[(13, 14), (10, 11)], 9, 17, 1", [(9, 10), (11, 13), (14, 17)]),
        ("[(10, 15), (11, 12)], 9, 17, 1", [(9, 10), (15, 17)]),
        ("[(9, 10), (12, 13)], 9, 15, 2", [(10, 12), (13, 15)]),
        ("[(10, 11), (11, 12)], 9, 17, 1", [(9, 10), (12, 17)]),
        ("[(16, 18)], 9, 17, 1", [(9, 16)]),
        ("[], 9, 17, 1", [(9, 17)]),
        ("[(14, 16), (10, 15), (11, 12)], 9, 17, 1", [(9, 10), (16, 17)]),
        ("[(10, 11), (11.5, 12)], 9, 17, 1", [(9, 10), (12, 17)]),
    ]
    code = "import slots\n" + "".join(f"print(int(list(map(tuple, slots.free_slots({a}))) == {b!r}))\n" for a, b in cases)
    rc, out = py(repo, code)
    parts = [int(x) for x in out.split()] if rc == 0 and len(out.split()) == len(cases) else [0] * len(cases)
    parts.append(int(unittest_ok(repo)))
    parts.append(int(same_as_template(repo, "debug", "test_slots.py")))
    return {"pass": all(parts), "score": f"{sum(parts)}/{len(parts)}", "detail": "".join(map(str, parts))}


def check_refactor(repo, answer):
    parts = []
    hits = []
    for base, dirs, files in os.walk(repo):
        dirs[:] = [x for x in dirs if x not in (".git", ".pi", "__pycache__")]
        for fn in files:
            p = os.path.join(base, fn)
            try:
                text = open(p, encoding="utf-8").read()
            except (UnicodeDecodeError, OSError):
                continue
            if "calc_total" in text:
                hits.append(os.path.relpath(p, repo))
    def read(rel):
        try:
            return open(os.path.join(repo, rel), encoding="utf-8").read()
        except OSError:
            return ""
    readme = read("README.md")
    renamed = ("calc_total" not in read("shop/cart.py") + read("shop/invoice.py") and "order_total" in read("tests/test_shop.py")
               and "order_total" in readme and all("deprecat" in l.lower() for l in readme.splitlines() if "calc_total" in l))
    parts.append(int(renamed))
    code = (
        "import warnings\n"
        "from shop.pricing import order_total, calc_total\n"
        "from shop.cart import Cart\n"
        "from shop.invoice import render\n"
        "a = order_total([(2.5, 2), (1.0, 3)], 0.1) == 8.8\n"
        "with warnings.catch_warnings(record=True) as w:\n"
        "    warnings.simplefilter('always')\n"
        "    b = calc_total([(2.5, 2), (1.0, 3)], 0.1) == 8.8\n"
        "c = any(issubclass(x.category, DeprecationWarning) for x in w)\n"
        "cart = Cart(); cart.add(10.0, 2); d = cart.total() == 20.0\n"
        "e = render([(1.5, 2)]) == '2 x 1.50\\ntotal 3.00'\n"
        "print(int(a), int(b), int(c), int(d), int(e))"
    )
    rc, out = py(repo, code)
    parts += [int(x) for x in out.split()] if rc == 0 and len(out.split()) == 5 else [0] * 5
    parts.append(int(unittest_ok(repo)))
    return {"pass": all(parts), "score": f"{sum(parts)}/{len(parts)}", "detail": f"{''.join(map(str, parts))} calc_total in {hits}"}


def check_rust(repo, answer):
    parts = []
    src = open(os.path.join(repo, "src/lib.rs")).read()
    orig = open(os.path.join(tmpl("rust"), "src/lib.rs")).read()
    squash = lambda t: re.sub(r"\s+", "", t)
    parts.append(int(squash(orig[orig.index("#[cfg(test)]"):]) in squash(src)))
    hidden = '''
#[cfg(test)]
mod hidden_tests {
    use super::*;

    #[test]
    fn hidden_punctuation() {
        assert_eq!(top_words("Hello, world! Hello?", 2), vec![("hello".to_string(), 2), ("world".to_string(), 1)]);
    }

    #[test]
    fn hidden_empty() {
        assert_eq!(top_words("", 3), Vec::<(String, usize)>::new());
    }

    #[test]
    fn hidden_ties() {
        assert_eq!(top_words("b b a a c", 2), vec![("a".to_string(), 2), ("b".to_string(), 2)]);
    }

    #[test]
    fn hidden_small_n() {
        assert_eq!(top_words("a", 5), vec![("a".to_string(), 1)]);
    }
}
'''
    with tempfile.TemporaryDirectory() as d:
        shutil.copytree(repo, os.path.join(d, "r"), ignore=shutil.ignore_patterns("target", ".git", ".pi"))
        with open(os.path.join(d, "r/src/lib.rs"), "a") as f:
            f.write(hidden)
        rc, out = sh("cargo test -q 2>&1", os.path.join(d, "r"), {"CARGO_TARGET_DIR": os.path.join(d, "target")}, timeout=600)
    m = re.search(r"test result: \w+\. (\d+) passed; (\d+) failed", out)
    passed, failed = (int(m.group(1)), int(m.group(2))) if m else (0, 8)
    parts.append(int(rc == 0 and failed == 0 and passed == 8))
    return {"pass": all(parts), "score": f"{passed}/8 tests, tests kept={parts[0]}", "detail": out.strip().splitlines()[-1][:200] if out.strip() else ""}


def anchors(repo):
    found = {}
    for rel, pat, key in (("logging/__init__.py", r"^BASIC_FORMAT = ", "q1"), ("logging/handlers.py", r"def shouldRollover", "q2"),
                          ("logging/__init__.py", r"^def getLogger\(name=None\)", "q3"), ("logging/__init__.py", r"^lastResort = ", "q4")):
        for n, line in enumerate(open(os.path.join(tmpl("qa"), rel)), 1):
            if key not in found and re.search(pat, line):
                found[key] = (rel, n)
    return found


def check_qa(repo, answer):
    a = answer
    correct = {
        "q1": "%(levelname)s:%(name)s:%(message)s" in a,
        "q2": "shouldRollover" in a and "maxBytes" in a and ">=" in a,
        "q3": bool(re.search(r"\broot\b", a, re.I)),
        "q4": "StderrHandler" in a and "WARNING" in a,
    }
    cites = re.findall(r"((?:logging/)?(?:__init__|handlers|config)\.py):(\d+)(?:[-–](\d+))?", a)
    hit = {}
    for key, (rel, line) in anchors(repo).items():
        hit[key] = any(rel.endswith(f.split("/")[-1]) and int(n) - 15 <= line <= int(m or n) + 15 for f, n, m in cites)
    untouched = not changed(repo)
    score = sum(correct.values()) + sum(hit.values())
    return {"pass": all(correct.values()) and untouched, "score": f"answers {sum(correct.values())}/4, cites {sum(hit.values())}/4",
            "detail": f"correct={correct} cites={hit} untouched={untouched}", "points": score}


def check_review(repo, answer):
    a = answer.lower()
    bugs = {
        "mutable_default_tags": "tags" in a and bool(re.search(r"mutable|shared|default", a)),
        "page_off_by_one": bool(re.search(r"\bpage\b", a)) and bool(re.search(r"off.by.one|size - 1|size-1|one (item )?(too )?few|drops|last item|one short", a)),
        "low_stock_inverted": bool(re.search(r"low_stock|low stock", a)) and bool(re.search(r"invert|revers|wrong comparison|backwards|>\s*threshold|instead of|opposite|above", a)),
        "save_swallows_errors": bool(re.search(r"\bsave\b", a)) and bool(re.search(r"swallow|silent|except exception|bare except|pass\b|hides|ignor", a)),
        "discount_twice": "discount" in a and bool(re.search(r"twice|double", a)),
        "export_path_traversal": "export" in a and bool(re.search(r"travers|\.\./|\.\.\\\\|arbitrary|escape|outside", a)),
    }
    untouched = not changed(repo)
    return {"pass": sum(bugs.values()) >= 5 and untouched, "score": f"{sum(bugs.values())}/6 planted bugs", "detail": f"{bugs} untouched={untouched}",
            "points": sum(bugs.values())}


CHECKS = {"fix": check_fix, "feature": check_feature, "debug": check_debug, "refactor": check_refactor, "rust": check_rust, "qa": check_qa, "review": check_review}


def check(task, repo, answer=""):
    result = CHECKS[task](repo, answer)
    if task not in ("qa", "review"):
        result["changed"] = changed(repo)
    return result


if __name__ == "__main__":
    task, repo = sys.argv[1], sys.argv[2]
    answer = open(sys.argv[3]).read() if len(sys.argv) > 3 else ""
    print(json.dumps(check(task, repo, answer)))
