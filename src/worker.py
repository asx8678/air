import contextlib, difflib, inspect, json, types, linecache, os, re, shlex, signal, subprocess, sys, tempfile, traceback

ROOT = os.getcwd()
CHECK = os.environ.pop("AIR_CHECK", "")
FMT = os.environ.pop("AIR_FMT", "")
LOG = os.environ.pop("AIR_LOG", "")
HALF = 4000
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\r(?!\n)")

_inbox = os.fdopen(os.dup(0), encoding="utf-8")
_outbox = os.fdopen(os.dup(1), "w", encoding="utf-8")
_null = os.open(os.devnull, os.O_RDWR)
os.dup2(_null, 0)
_report = {}
_state = {}
_checked = None


def _exec(cmd, timeout):
    with tempfile.TemporaryFile() as out:
        p = subprocess.Popen(cmd, shell=True, cwd=ROOT, stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT,
                             start_new_session=True)
        try:
            code = p.wait(timeout=timeout)
        except BaseException as e:
            with contextlib.suppress(OSError):
                os.killpg(p.pid, signal.SIGKILL)
            p.wait()
            if not isinstance(e, subprocess.TimeoutExpired):
                raise
            code = f"timeout after {timeout}s"
        out.seek(0)
        return ANSI.sub("", out.read().decode("utf-8", "replace")), code


def _rel(path):
    return os.path.relpath(os.path.join(ROOT, path), ROOT)


def _protected():
    hooks = subprocess.run(["git", "config", "core.hooksPath"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    dirs = [os.path.join(ROOT, ".git")]
    if hooks:
        hooks = os.path.realpath(os.path.join(ROOT, os.path.expanduser(hooks)))
        dirs.append(os.path.dirname(hooks) if os.path.basename(hooks) == "_" else hooks)
    return [os.path.realpath(d) for d in dirs]


PROTECTED = _protected()


def _change(tool, path, make):
    rel = _rel(path)
    real = os.path.realpath(os.path.join(ROOT, rel))
    signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT})
    try:
        if rel.startswith("..") or not real.startswith(ROOT + os.sep):
            raise ValueError(f"{path} is outside the repo")
        if any(real == d or real.startswith(d + os.sep) for d in PROTECTED):
            raise ValueError(f"{path} is off limits: git runs code from there outside the sandbox")
        if not (_report.get("accepted") or _state.get("accepted")):
            raise ValueError("call accept(criteria) before changing files")
        before, written = make(os.path.join(ROOT, rel), rel)
        delta = [l[0] for l in difflib.unified_diff(before.splitlines(), written.splitlines(), lineterm="")][2:]
        _report.setdefault("actions", []).append(
            {"tool": tool, "path": rel, "ok": True, "added": delta.count("+"), "removed": delta.count("-")})
    except Exception as e:
        _report.setdefault("actions", []).append({"tool": tool, "path": rel, "ok": False, "error": str(e)})
        raise
    finally:
        signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGINT})
    note = ""
    if FMT:
        out, code = _exec(f"{FMT} {shlex.quote(rel)}", 60)
        note = f"; fmt failed: {out.strip().splitlines()[0] if out.strip() else code}" if code else ""
    with open(os.path.join(ROOT, rel), encoding="utf-8") as f:
        after = f.read()
    written = written.replace("\r\n", "\n")
    if after != written:
        diff = difflib.unified_diff(written.splitlines(True), after.splitlines(True), n=1)
        note += "; the formatter changed it:\n" + "".join(list(diff)[2:]).rstrip("\n")
    print(f"{tool} {rel}: ok{note}")


def edit(path, old, new):
    def make(full, rel):
        with open(full, encoding="utf-8", newline="") as f:
            raw = f.read()
        crlf = "\r\n" in raw and raw.count("\r\n") == raw.count("\n")
        text = raw.replace("\r\n", "\n") if crlf else raw
        n = text.count(old)
        if n != 1:
            raise ValueError(f"old text {'not found' if n == 0 else f'occurs {n} times'} in {rel}")
        result = text.replace(old, new, 1)
        with open(full, "w", encoding="utf-8", newline="\r\n" if crlf else "") as f:
            f.write(result)
        return text, result
    _change("edit", path, make)


def write(path, content):
    def make(full, rel):
        if os.path.lexists(full):
            raise ValueError(f"{rel} exists; use edit")
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "x", encoding="utf-8") as f:
            f.write(content)
        return "", content
    _change("write", path, make)


def sh(cmd, timeout=120):
    out, code = _exec(cmd, timeout)
    return out.rstrip("\n") + (f"\n[exit {code}]" if code else "")


def accept(criteria):
    _report["accepted"] = str(criteria)


def check():
    if not CHECK:
        print("no check command configured (set check in air.toml)")
        return None
    before = _tree()
    out, code = _exec(CHECK, 1800)
    after = _tree()
    changed = sorted(f for f in before.keys() | after.keys() if before.get(f) != after.get(f))
    ok = code == 0 and not changed
    why = f"check changed the working tree: {', '.join(changed[:10])}" if changed else CHECK
    global _checked
    _checked = after if ok else None
    _report["check"] = ok
    _report.setdefault("actions", []).append({"tool": "check", "ok": ok})
    print(f"{out.rstrip()}\n[check {'passed' if ok else 'FAILED'}: {why}]".lstrip("\n"))
    return ok


def _tree():
    out = subprocess.run(["git", "ls-files", "-co", "--exclude-standard", "-z"], cwd=ROOT, capture_output=True).stdout
    tree = {}
    for f in filter(None, out.decode("utf-8", "replace").split("\0")):
        with contextlib.suppress(OSError):
            st = os.stat(os.path.join(ROOT, f))
            tree[f] = (st.st_size, st.st_mtime_ns)
    return tree


def carry(note=None, clear=False):
    notes = [] if clear else list(_report.get("carry", _state.get("carry") or []))
    if note is not None:
        note = str(note)
        if len(note.encode()) > 2048:
            raise ValueError("a carry note is limited to 2 KiB")
        if note not in notes:
            notes.append(note)
        if len(notes) > 16:
            raise ValueError("at most 16 carry notes; carry(clear=True) and add back what still matters")
    if clear or note is not None:
        _report["carry"] = notes
    return notes


def plan(steps):
    _report["plan"] = {"steps": [str(s) for s in steps], "done": []}


def done(step=None):
    p = _report.get("plan") or _state.get("plan")
    if not p:
        raise ValueError("call plan([...]) first")
    left = [i for i in range(1, len(p["steps"]) + 1) if i not in p["done"]]
    n = step or (left[0] if left else 0)
    if not 1 <= n <= len(p["steps"]):
        raise ValueError(f"no step {n}; the plan has {len(p['steps'])}")
    _report["plan"] = {"steps": p["steps"], "done": sorted(set(p["done"]) | {n})}
    rest = [i for i in left if i != n]
    return p["steps"][rest[0] - 1] if rest else None


def recall(n):
    with open(LOG, encoding="utf-8") as f:
        for i, line in enumerate(f):
            if i == n:
                entry = json.loads(line)
                parts = [f"#{n} {entry.get('role', 'record')}", entry.get("content") or ""]
                for call in entry.get("tool_calls") or []:
                    name, args = call["function"]["name"], call["function"]["arguments"]
                    with contextlib.suppress(ValueError, AttributeError):
                        args = "\n".join(f"{k}:\n{v}" for k, v in json.loads(args).items())
                    parts.append(f"{name}\n{args}")
                return "\n".join(p for p in parts if p)
    raise IndexError(f"no entry #{n}")


def _hint(e, misuse):
    text = str(e)
    if isinstance(e, NameError) and re.search(r"'(read|files|grep|ls|cat)'", text):
        return 'explore with plain Python: open(), re, pathlib and sh("git ls-files")'
    if isinstance(e, SyntaxError) and sys.version_info < (3, 10):
        return f"the worker runs Python {sys.version_info[0]}.{sys.version_info[1]}: no match statements or X | Y types"
    if misuse:
        name = text.split("(")[0]
        return f"signature: {name}{inspect.signature(G[name])}"
    for marker, advice in (("old text not found", "print the exact current lines first, then copy them into old"),
                           ("old text occurs", "include more surrounding lines so old matches exactly once"),
                           ("call accept(", "call accept(criteria) before edit() or write()"),
                           ("exists; use edit", "use edit() to change an existing file")):
        if marker in text:
            return advice
    return None


G = {"__name__": "__main__", "_last": "", "sh": sh, "edit": edit, "write": write, "accept": accept, "carry": carry, "plan": plan, "done": done, "check": check, "recall": recall}
HELPERS = tuple(f"{name}()" for name in G if not name.startswith("_"))


class _Helper(types.ModuleType):
    def __call__(self, *args, **kwargs):
        return G[self.__name__](*args, **kwargs)


for _name in ("sh",):
    sys.modules[_name] = _Helper(_name)
    setattr(sys.modules[_name], _name, G[_name])
signal.signal(signal.SIGINT, signal.SIG_IGN)
for request in _inbox:
    request = json.loads(request)
    code, _state = request["code"], request.get("state") or {}
    _report.clear()
    capture = tempfile.TemporaryFile()
    os.dup2(capture.fileno(), 1)
    os.dup2(capture.fileno(), 2)
    sys.stdout = sys.stderr = open(1, "w", encoding="utf-8", errors="replace", buffering=1, closefd=False)
    failed = False
    signal.signal(signal.SIGINT, signal.default_int_handler)
    try:
        linecache.cache["<run>"] = (len(code), None, code.splitlines(True), "<run>")
        exec(compile(code, "<run>", "exec"), G)
    except BaseException as e:
        failed = True
        tb = traceback.extract_tb(e.__traceback__)
        frames = [f for f in tb if f.filename == "<run>"]
        misuse = isinstance(e, TypeError) and tb and tb[-1].filename == "<run>" and str(e).startswith(HELPERS)
        rejected = isinstance(e, (SyntaxError, NameError)) or misuse
        _report["exception"], _report["rejected"] = type(e).__name__, bool(rejected)
        print("".join(traceback.format_list(frames) + traceback.format_exception_only(type(e), e)), end="")
        hint = _hint(e, misuse)
        if hint:
            print(f"hint: {hint}")
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    if _report.get("check") and _tree() != _checked:
        _report["check"] = False
        print("[check no longer applies: files changed after it passed; run check() again]")
    sys.stdout.flush()
    os.dup2(_null, 1)
    os.dup2(_null, 2)
    capture.seek(0)
    text = ANSI.sub("", capture.read().decode("utf-8", "replace"))
    capture.close()
    G["_last"] = text
    if len(text) > 2 * HALF:
        text = f"{text[:HALF]}\n[… {len(text) - 2 * HALF} characters omitted; the full output is in _last]\n{text[-HALF:]}"
    _outbox.write(json.dumps(dict(_report, out=text or "(no output)", error=failed)) + "\n")
    _outbox.flush()
