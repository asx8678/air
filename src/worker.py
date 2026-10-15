import ast, contextlib, ctypes, difflib, importlib.util, inspect, json, linecache, os, re, shlex, signal, stat, subprocess, sys, tempfile, threading, time, traceback

ROOT = os.getcwd()
sys.path.append(ROOT)
CHECK = os.environ.pop("AIR_CHECK", "")
FMT = os.environ.pop("AIR_FMT", "")
LOG = os.environ.pop("AIR_LOG", "")
HALF = 16000
LIMIT = 8 << 20
RUN_CAP = 570
IGNORABLE = re.compile("[\u200c-\u200f\u202a-\u202e\u206a-\u206f\ufeff]")
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\r(?!\n)")

_inbox = os.fdopen(os.dup(0), encoding="utf-8")
_outbox = os.fdopen(os.dup(1), "w", encoding="utf-8")
_null = os.open(os.devnull, os.O_RDWR)
os.dup2(_null, 0)
_report = {}
_state = {}
_checked = None
_groups = set()
_deadline = time.monotonic() + RUN_CAP


def _children(pid):
    if sys.platform == "darwin":
        buf = (ctypes.c_int * 4096)()
        return list(buf[:max(0, ctypes.CDLL("/usr/lib/libSystem.B.dylib").proc_listchildpids(pid, buf, 16384))])
    with contextlib.suppress(OSError, ValueError), open(f"/proc/{pid}/task/{pid}/children") as f:
        return [int(n) for n in f.read().split()]
    return []


def _kill():
    groups, tree = set(_groups), [os.getpid()]
    for parent in tree:
        for pid in _children(parent):
            tree.append(pid)
            with contextlib.suppress(OSError):
                groups.add(os.getpgid(pid))
    for group in groups - {0, 1, os.getpgrp()}:
        with contextlib.suppress(OSError):
            os.killpg(group, signal.SIGKILL)
    os.killpg(0, signal.SIGKILL)


def _watch(parent):
    while os.getppid() == parent:
        time.sleep(1)
    _kill()


threading.Thread(target=_watch, args=(os.getppid(),), daemon=True).start()


def _exec(cmd, timeout):
    left = _deadline - time.monotonic()
    if not timeout or timeout > left:
        timeout = max(1, int(left))
    with tempfile.TemporaryFile() as out:
        p = subprocess.Popen(cmd, shell=True, cwd=ROOT, stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT,
                             start_new_session=True)
        _groups.add(p.pid)
        try:
            code = p.wait(timeout=timeout)
        except BaseException as e:
            with contextlib.suppress(OSError):
                os.killpg(p.pid, signal.SIGKILL)
            p.wait()
            if not isinstance(e, subprocess.TimeoutExpired):
                raise
            code = f"timeout after {timeout}s"
        with contextlib.suppress(OSError):
            os.killpg(p.pid, signal.SIGKILL)
        _groups.discard(p.pid)
        return ANSI.sub("", _read(out).decode("utf-8", "replace")), code


def _read(f):
    size = f.seek(0, 2)
    f.seek(0)
    if size <= LIMIT:
        return f.read()
    head = f.read(LIMIT // 2)
    f.seek(-LIMIT // 2, 2)
    return head + f"\n[… {size - LIMIT} bytes omitted]\n".encode() + f.read()


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
        parts = (rel + os.sep + os.path.relpath(real, ROOT)).split(os.sep)
        if any(d == real or real.startswith(d + os.sep) for d in PROTECTED) or any(
                IGNORABLE.sub("", p).casefold() == ".git" for p in parts):
            raise ValueError(f"{path} is off limits: git runs code from there outside the sandbox")
        st = os.stat(real) if os.path.lexists(real) else None
        if st and stat.S_ISREG(st.st_mode) and (st.st_nlink > 1 or st.st_dev != os.stat(ROOT).st_dev):
            raise ValueError(f"{path} has other hard links or lives on another volume; air will not change it")
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
    _forget()
    note = ""
    if FMT:
        out, code = _exec(f"{FMT} {shlex.quote('./' + rel if rel.startswith('-') else rel)}", 60)
        note = f"; fmt failed: {out.strip().splitlines()[0] if out.strip() else code}" if code else ""
    with open(os.path.join(ROOT, rel), encoding="utf-8") as f:
        after = f.read()
    written = written.replace("\r\n", "\n")
    if after != written:
        diff = difflib.unified_diff(written.splitlines(True), after.splitlines(True), n=1)
        note += "; the formatter changed it:\n" + "".join(list(diff)[2:]).rstrip("\n")
    print(f"{tool} {rel}: ok{note}")


def _forget():
    for name, mod in list(sys.modules.items()):
        path = getattr(mod, "__file__", None)
        if path and "site-packages" not in path and os.path.realpath(path).startswith(ROOT + os.sep):
            del sys.modules[name]
            for key in [k for k, v in G.items() if v is mod or getattr(v, "__module__", None) == name]:
                del G[key]


def edit(path, old, new=None):
    pairs = [(old, new)] if new is not None else old
    if isinstance(pairs, str) or not pairs or not all(
            isinstance(p, (list, tuple)) and len(p) == 2 and all(isinstance(s, str) for s in p) for p in pairs):
        raise TypeError("edit() takes (path, old, new) or (path, [(old, new), ...]) with string old and new")
    def make(full, rel):
        with open(full, encoding="utf-8", newline="") as f:
            raw = f.read()
        crlf = "\r\n" in raw and raw.count("\r\n") == raw.count("\n")
        eol = (lambda s: s.replace("\r\n", "\n")) if crlf else (lambda s: s)
        text = result = eol(raw)
        for k, (o, n) in enumerate(pairs, 1):
            find = eol(o)
            count = result.count(find)
            if count != 1:
                which = f" (pair {k} of {len(pairs)}; nothing was changed)" if len(pairs) > 1 else ""
                raise ValueError(f"old text {'not found' if count == 0 else f'occurs {count} times'} in {rel}{which}")
            result = result.replace(find, eol(n), 1)
        data = (result.replace("\n", "\r\n") if crlf else result).encode("utf-8")
        with open(full, "wb") as f:
            f.write(data)
        return text, result
    _change("edit", path, make)


def write(path, content):
    def make(full, rel):
        if os.path.lexists(full):
            raise ValueError(f"{rel} exists; use edit")
        data = content.encode("utf-8")
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "xb") as f:
            f.write(data)
        return "", content
    _change("write", path, make)


def sh(cmd, timeout=120):
    entry = [str(cmd), None, []]
    _report.setdefault("cmds", []).append(entry)
    out, code = _exec(cmd, timeout)
    entry[1:] = [code, [_rel(w) for w in str(cmd).split() if os.path.isfile(os.path.join(ROOT, w))]]
    return out.rstrip("\n") + (f"\n[exit {code}]" if code else "")


def show(path, start=1, end=None):
    with open(os.path.join(ROOT, path), encoding="utf-8", errors="replace") as f:
        lines = f.read().splitlines()
    _report.setdefault("reads", []).append(_rel(path))
    start, end = max(start, 1), min(end or len(lines), len(lines))
    print(f"[{_rel(path)}: lines {start}-{end} of {len(lines)}]")
    print("\n".join(f"{n}: {lines[n - 1]}" for n in range(start, end + 1)))


def accept(criteria):
    _report["accepted"] = str(criteria)


def check():
    if not CHECK:
        print("no check command configured (set check in air.toml)")
        return None
    ran = len(_report.get("cmds", []))
    _outbox.write(json.dumps({"needs_check": True, "actions": _report.get("actions", []), "cmds": _report.get("cmds", [])}) + "\n")
    _outbox.flush()
    answer = json.loads(_inbox.readline() or "{}")
    ok = answer.get("ok") is True
    global _checked, _deadline
    _deadline = time.monotonic() + RUN_CAP
    _checked = _tree() if ok else None
    _report["check"] = ok
    _report.setdefault("actions", []).append({"tool": "check", "ok": ok, "cmds": ran})
    print(answer.get("out") or f"[check {'passed' if ok else 'FAILED'}]")
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
    p = _report.get("plan") or _state.get("plan") or {"steps": [], "done": []}
    left = [i for i in range(1, len(p["steps"]) + 1) if i not in p["done"]]
    n = step if isinstance(step, int) else (left[0] if left else None)
    if n is None:
        return None
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
        return 'read files with show(path, start, end); explore with plain Python: open(), re, pathlib and sh("git ls-files")'
    if isinstance(e, SyntaxError) and sys.version_info < (3, 10) and (e.text or "").lstrip().startswith(("match ", "case ")):
        return f"the worker runs Python {sys.version_info[0]}.{sys.version_info[1]}: no match statements or X | Y types"
    code = "".join(linecache.cache.get("<run>", (0, None, [], ""))[2])
    calls = [code.count("\n", 0, m.start()) + 1 for m in re.finditer(r"(?<![\w.])(edit|write)\(", code)]
    if isinstance(e, SyntaxError) and calls and (e.lineno or 0) >= calls[0] and ('"""' in code or "'''" in code):
        return ("a raw triple-quoted string cannot end with a backslash or contain its own quotes: "
                "switch to the other quote style (''' or \"\"\") or use a plain string with escapes")
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


TOOLS = {"sh": sh, "show": show,"edit": edit, "write": write, "accept": accept, "carry": carry, "plan": plan, "done": done, "check": check, "recall": recall}
G = {"__name__": "__main__", "_last": "", **TOOLS}
HELPERS = tuple(f"{name}()" for name in TOOLS)


class _Helpers:
    def find_spec(self, name, path=None, target=None):
        if path is None and re.fullmatch(rf"({'|'.join(TOOLS)})(_\w*)?", name):
            return importlib.util.spec_from_loader(name, self)
        return None

    def create_module(self, spec):
        return TOOLS[spec.name.split("_")[0]]

    def exec_module(self, module):
        print(f"[import {module.__spec__.name}: air's helpers are built in; no import needed]")


sh.sh = sys.modules["sh"] = sh
sys.meta_path.append(_Helpers())
signal.signal(signal.SIGINT, signal.SIG_IGN)
_outbox.write(json.dumps({"ready": True, "python": sys.version.split()[0]}) + "\n")
_outbox.flush()
for request in _inbox:
    request = json.loads(request)
    code, _state = request["code"], request.get("state") or {}
    _deadline = time.monotonic() + RUN_CAP
    _report.clear()
    os.chdir(ROOT)
    G.update(TOOLS)
    capture = tempfile.TemporaryFile()
    os.dup2(capture.fileno(), 1)
    os.dup2(capture.fileno(), 2)
    sys.stdout = sys.stderr = open(1, "w", encoding="utf-8", errors="replace", buffering=1, closefd=False)
    failed = False
    signal.signal(signal.SIGINT, signal.default_int_handler)
    try:
        linecache.cache["<run>"] = (len(code), None, code.splitlines(True), "<run>")
        tree = ast.parse(code, "<run>")
        tail = tree.body.pop() if tree.body and isinstance(tree.body[-1], ast.Expr) else None
        exec(compile(tree, "<run>", "exec"), G)
        if tail:
            value = eval(compile(ast.Expression(tail.value), "<run>", "eval"), G)
            sys.stdout.flush()
            if value is not None and not os.fstat(1).st_size:
                print(value)
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
        applied = [f"{a['tool']} {a['path']}" for a in _report.get("actions", []) if a.get("ok") and a.get("path")]
        if applied:
            print(f"[applied before the error: {', '.join(applied)}; inspect before repeating them]")
    shadowed = [name for name, tool in TOOLS.items() if G.get(name) is not tool]
    if shadowed:
        _report["shadowed"] = shadowed
        print(f"[{', '.join(shadowed)}: air's helpers, restored before the next run]")
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    if _report.get("check") and _tree() != _checked:
        _report["check"] = False
        print("[check no longer applies: files changed after it passed; run check() again]")
    sys.stdout.flush()
    os.dup2(_null, 1)
    os.dup2(_null, 2)
    text = ANSI.sub("", _read(capture).decode("utf-8", "replace"))
    capture.close()
    G["_last"] = text
    _report["chars"] = len(text)
    if len(text) > 2 * HALF:
        text = f"{text[:HALF]}\n[… {len(text) - 2 * HALF} characters omitted; the full output is in _last]\n{text[-HALF:]}"
    _outbox.write(json.dumps(dict(_report, out=text or "(no output)", error=failed)) + "\n")
    _outbox.flush()
_kill()
