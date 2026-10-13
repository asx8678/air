# mini's code-mode worker: persistent, standard library only, Python 3.9+.
import contextlib, difflib, inspect, json, linecache, os, re, shlex, signal, subprocess, sys, tempfile, traceback

ROOT = os.getcwd()
CHECK = os.environ.pop("MINI_CHECK", "")
FMT = os.environ.pop("MINI_FMT", "")
LOG = os.environ.pop("MINI_LOG", "")
HALF = 4000  # characters of output kept from each end
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\r(?!\n)")

# The protocol keeps private copies of the host pipes. Fds 0/1/2 point elsewhere, so nothing
# the model's code starts can write into it.
_inbox = os.fdopen(os.dup(0), encoding="utf-8")
_outbox = os.fdopen(os.dup(1), "w", encoding="utf-8")
_null = os.open(os.devnull, os.O_RDWR)
os.dup2(_null, 0)
_report = {}  # what this run did, for the host
_state = {}  # accepted criteria and carry notes, derived by the host from its log


def _exec(cmd, timeout):
    p = subprocess.Popen(cmd, shell=True, cwd=ROOT, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, start_new_session=True)
    try:
        out, code = p.communicate(timeout=timeout)[0], p.returncode
    except BaseException as e:  # timeout or Ctrl-C: stop the whole process group
        with contextlib.suppress(OSError):
            os.killpg(p.pid, signal.SIGKILL)
        out = p.communicate()[0]
        if not isinstance(e, subprocess.TimeoutExpired):
            raise
        code = f"timeout after {timeout}s"
    return ANSI.sub("", out.decode("utf-8", "replace")), code


def _rel(path):
    return os.path.relpath(os.path.join(ROOT, path), ROOT)


def _change(tool, path, make):
    """The one way files change: inside the repo, after accept(), exact, then formatted."""
    rel = _rel(path)
    try:
        if rel.startswith("..") or not os.path.realpath(os.path.join(ROOT, rel)).startswith(ROOT + os.sep):
            raise ValueError(f"{path} is outside the repo")
        if not (_report.get("accepted") or _state.get("accepted")):
            raise ValueError("call accept(criteria) before changing files")
        written = make(os.path.join(ROOT, rel), rel)
        note = ""
        if FMT:
            out, code = _exec(f"{FMT} {shlex.quote(rel)}", 60)
            note = f"; fmt failed: {out.strip().splitlines()[0] if out.strip() else code}" if code else ""
        with open(os.path.join(ROOT, rel), encoding="utf-8") as f:
            after = f.read()
        if after != written:
            diff = difflib.unified_diff(written.splitlines(True), after.splitlines(True), n=1)
            note += "; the formatter changed it:\n" + "".join(list(diff)[2:]).rstrip("\n")
    except Exception as e:
        _report.setdefault("actions", []).append({"tool": tool, "path": rel, "ok": False, "error": str(e)})
        raise
    _report.setdefault("actions", []).append({"tool": tool, "path": rel, "ok": True})
    print(f"{tool} {rel}: ok{note}")


def edit(path, old, new):
    """Replace the one exact occurrence of old with new."""
    def make(full, rel):
        with open(full, encoding="utf-8") as f:
            text = f.read()
        n = text.count(old)
        if n != 1:
            raise ValueError(f"old text {'not found' if n == 0 else f'occurs {n} times'} in {rel}")
        with open(full, "w", encoding="utf-8") as f:
            f.write(text.replace(old, new, 1))
        return text.replace(old, new, 1)
    _change("edit", path, make)


def write(path, content):
    """Create a new file; fails if it exists."""
    def make(full, rel):
        if os.path.lexists(full):
            raise ValueError(f"{rel} exists; use edit")
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "x", encoding="utf-8") as f:
            f.write(content)
        return content
    _change("write", path, make)


def sh(cmd, timeout=120):
    """Run a shell command; returns stdout+stderr, ending in '[exit N]' when it fails."""
    out, code = _exec(cmd, timeout)
    return out.rstrip("\n") + (f"\n[exit {code}]" if code else "")


def accept(criteria):
    """Declare what done means. Required before the first edit; check() and the report measure against it."""
    _report["accepted"] = str(criteria)


def check():
    """Run the project's check command, print its output, return True when it passes."""
    if not CHECK:
        print("no check command configured (set check in mini.toml)")
        return None
    before = _tree()
    out, code = _exec(CHECK, 1800)
    after = _tree()
    changed = sorted(f for f in before.keys() | after.keys() if before.get(f) != after.get(f))
    ok = code == 0 and not changed
    why = f"check changed the working tree: {', '.join(changed[:10])}" if changed else CHECK
    _report["check"] = ok
    _report.setdefault("actions", []).append({"tool": "check", "ok": ok})
    print(f"{out.rstrip()}\n[check {'passed' if ok else 'FAILED'}: {why}]".lstrip("\n"))
    return ok


def _tree():
    """Size and mtime of every file git does not ignore; a check must leave this unchanged."""
    out = subprocess.run(["git", "ls-files", "-co", "--exclude-standard", "-z"], cwd=ROOT, capture_output=True).stdout
    tree = {}
    for f in filter(None, out.decode("utf-8", "replace").split("\0")):
        with contextlib.suppress(OSError):
            st = os.stat(os.path.join(ROOT, f))
            tree[f] = (st.st_size, st.st_mtime_ns)
    return tree


def carry(note=None, clear=False):
    """Keep a note in every compaction summary until cleared; carry() alone lists the notes."""
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

def recall(n):
    """Session log entry #n exactly as recorded; summaries cite entries as #n."""
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
    """One fixed hint per known mistake, keyed on the exception and our own helpers' messages."""
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

G = {"__name__": "__main__", "_last": "", "sh": sh, "edit": edit, "write": write, "accept": accept, "carry": carry, "check": check, "recall": recall}
HELPERS = tuple(f"{name}()" for name in G if not name.startswith("_"))
signal.signal(signal.SIGINT, signal.SIG_IGN)  # Ctrl-C only interrupts a running script
for request in _inbox:
    request = json.loads(request)
    code, _state = request["code"], request.get("state") or {}
    _report.clear()
    capture = tempfile.TemporaryFile()  # prints and stray child output, in order
    os.dup2(capture.fileno(), 1)
    os.dup2(capture.fileno(), 2)
    sys.stdout = sys.stderr = open(1, "w", encoding="utf-8", errors="replace", buffering=1, closefd=False)
    failed = False
    signal.signal(signal.SIGINT, signal.default_int_handler)
    try:
        linecache.cache["<run>"] = (len(code), None, code.splitlines(True), "<run>")
        exec(compile(code, "<run>", "exec"), G)
    except BaseException as e:  # show only the model's own frames
        failed = True
        tb = traceback.extract_tb(e.__traceback__)
        frames = [f for f in tb if f.filename == "<run>"]
        # Rejection: the code never got to act (bad syntax, an unknown name, or a helper called with the
        # wrong arguments, which Python raises in the caller's frame). Anything else is an effect failure.
        misuse = isinstance(e, TypeError) and tb and tb[-1].filename == "<run>" and str(e).startswith(HELPERS)
        rejected = isinstance(e, (SyntaxError, NameError)) or misuse
        _report["exception"], _report["rejected"] = type(e).__name__, bool(rejected)
        print("".join(traceback.format_list(frames) + traceback.format_exception_only(type(e), e)), end="")
        hint = _hint(e, misuse)
        if hint:
            print(f"hint: {hint}")
    signal.signal(signal.SIGINT, signal.SIG_IGN)
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
