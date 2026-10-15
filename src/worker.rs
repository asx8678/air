use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const RUN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const HERMETIC: &str = "TZ=UTC LANG=C.UTF-8 LC_ALL=C.UTF-8 NO_COLOR=1 CLICOLOR=0 CARGO_TERM_COLOR=never CI=1 TERM=dumb \
                        PAGER=cat GIT_PAGER=cat GIT_TERMINAL_PROMPT=0 PYTHONHASHSEED=0 PYTHONUTF8=1 PYTHONDONTWRITEBYTECODE=1";
const CACHES: &str =
    "XDG_CACHE_HOME GOCACHE UV_CACHE_DIR PIP_CACHE_DIR PRE_COMMIT_HOME npm_config_cache PYTHONPYCACHEPREFIX";
const SECRETS: &str = ".air, .config/air, .ssh, .gnupg, .aws, .azure, .kube, .docker, .netrc, .pi, .config/gh, \
                       .config/gcloud, .config/op, .password-store, .git-credentials, .config/git/credentials, \
                       .npmrc, .pypirc, .cargo/credentials, .cargo/credentials.toml, .vault-token, .claude, \
                       .claude.json, .codex, .zsh_history, .zsh_sessions, .bash_history, .python_history, \
                       .node_repl_history, Library/Keychains, Library/Cookies, Library/Application Support/Google, \
                       Library/Application Support/Firefox, .terraform.d, .m2/settings.xml, .gradle/gradle.properties, \
                       .oci, .subversion/auth, .config/helm";
const ENV: &str = "PATH HOME USER LOGNAME SHELL LANG LC_* TERM TMPDIR TZ PWD CARGO_HOME RUSTUP_HOME RUSTUP_TOOLCHAIN \
                   GOPATH GOROOT GOFLAGS VIRTUAL_ENV CONDA_PREFIX PYENV_ROOT NVM_DIR JAVA_HOME MIX_HOME HEX_HOME ERL_* \
                   ELIXIR_* DEVELOPER_DIR SDKROOT";

static EXTRA_ENV: OnceLock<Vec<String>> = OnceLock::new();

pub fn allow_env(names: Vec<String>) {
    let _ = EXTRA_ENV.set(names);
}

#[derive(Debug)]
pub struct StartFailed;

impl std::fmt::Display for StartFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("the worker could not start")
    }
}

pub struct Worker {
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    busy: bool,
    ready: bool,
    child: Child,
    started: std::time::Instant,
}

impl Worker {
    pub fn start(root: &Path, yolo: bool, check: Option<&str>, fmt: Option<&str>, log: &Path) -> Result<Self> {
        let bwrap = || std::process::Command::new("bwrap").arg("--version").output().is_ok_and(|o| o.status.success());
        if !yolo && !cfg!(target_os = "macos") && !bwrap() {
            return Err(
                anyhow!("bwrap (bubblewrap) is not installed; install it or run with --yolo").context(StartFailed)
            );
        }
        let mut child = sandboxed(root, yolo, log, "python3", &["-I", "-B", "-u", "-c", include_str!("worker.py")])
            .env("AIR_CHECK", if check.is_some() { "1" } else { "" })
            .env("AIR_FMT", fmt.unwrap_or_default())
            .env("AIR_LOG", log)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .context("starting the python3 worker")
            .context(StartFailed)?;
        let stdin = child.stdin.take().context("worker stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("worker stdout")?).lines();
        Ok(Self { stdin, stdout, busy: false, ready: false, child, started: std::time::Instant::now() })
    }

    pub async fn run(&mut self, code: &str, state: &Value) -> Result<Value> {
        if !self.ready {
            let line = tokio::time::timeout(Duration::from_secs(60), self.stdout.next_line()).await;
            let Ok(Ok(Some(line))) = line else {
                return Err(anyhow!("the worker exited before it was ready").context(StartFailed));
            };
            self.ready = true;
            let python = serde_json::from_str::<Value>(&line).unwrap_or_default()["python"].take();
            let ms = self.started.elapsed().as_millis() as u64;
            let text = format!("python {} worker ready in {ms} ms", python.as_str().unwrap_or("?"));
            crate::ui::record("worker", &text, json!({"pid": self.child.id(), "python": python, "ms": ms}));
        }
        self.send(json!({"code": code, "state": state})).await
    }

    pub async fn resume(&mut self, ok: bool, out: &str) -> Result<Value> {
        self.send(json!({"ok": ok, "out": out})).await
    }

    async fn send(&mut self, request: Value) -> Result<Value> {
        self.busy = true;
        self.stdin.write_all(format!("{request}\n").as_bytes()).await?;
        let reply = tokio::time::timeout(RUN_TIMEOUT, self.stdout.next_line()).await.context("run() timed out")??;
        let reply: Value = serde_json::from_str(&reply.context("the worker exited")?)?;
        self.busy = reply["needs_check"] == true;
        Ok(reply)
    }

    pub async fn settle(&mut self) -> Result<Option<Value>> {
        if !self.busy {
            return Ok(None);
        }
        self.busy = false;
        if let Some(pid) = self.child.id() {
            unsafe { libc::kill(pid as i32, libc::SIGINT) };
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            match tokio::time::timeout_at(deadline, self.stdout.next_line()).await {
                Ok(Ok(Some(line))) => {
                    let reply: Value = serde_json::from_str(&line)?;
                    if reply["needs_check"] != true {
                        return Ok(Some(reply));
                    }
                }
                _ => bail!("the worker did not report back"),
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let Some(pid) = self.child.id() else { return };
        let own = unsafe { libc::getpgrp() };
        for group in descendants(pid as i32).into_iter().chain([pid as i32]).filter(|&g| g > 1 && g != own) {
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
    }
}

pub fn descendants(pid: i32) -> Vec<i32> {
    let ps = std::process::Command::new("ps").args(["-axo", "pid=,ppid=,pgid="]).output();
    let table = ps.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let rows: Vec<Vec<i32>> =
        table.lines().map(|l| l.split_whitespace().filter_map(|n| n.parse().ok()).collect()).collect();
    let (mut tree, mut groups, mut i) = (vec![pid], vec![], 0);
    while i < tree.len() {
        let parent = tree[i];
        for row in rows.iter().filter(|r| r.len() == 3 && r[1] == parent && r[0] != parent) {
            tree.push(row[0]);
            groups.push(row[2]);
        }
        i += 1;
    }
    groups
}

pub fn sandboxed(root: &Path, yolo: bool, log: &Path, program: &str, args: &[&str]) -> Command {
    let writable = writable(root);
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let home = home.canonicalize().unwrap_or(home);
    let secrets = SECRETS.split(',').map(|s| home.join(s.trim()));
    let log = log.canonicalize().unwrap_or_else(|_| log.into());
    let protected = protected(root);
    let mut cmd = if yolo {
        Command::new(program)
    } else if cfg!(target_os = "macos") {
        let dirs: String = writable.iter().map(|d| format!(" (subpath {d:?})")).collect();
        let secrets: String = secrets.map(|d| format!(" (subpath {d:?})")).collect();
        let protected: String = protected.iter().map(|d| format!(" (subpath {d:?})")).collect();
        let sockets = root.join("tmp");
        let profile = format!(
            "(version 1)(allow default)(deny network*)\
             (allow network-outbound (remote ip \"localhost:*\"))\
             (allow network-bind network-inbound (local ip \"localhost:*\"))\
             (allow network-bind network-outbound network-inbound \
              (local unix-socket (subpath {sockets:?})) (remote unix-socket (subpath {sockets:?})))\
             (deny file-write*)(allow file-write*{dirs} (subpath \"/dev\"))(deny file-write*{protected})\
             (deny file-write* (require-all (subpath {root:?}) (regex #\"/\\.[gG][iI][tT](/|$)\")))\
             (deny file-read-data{secrets})(allow file-read-data (literal {log:?}))\
             (deny file-ioctl (regex #\"^/dev/tty\"))(deny file-write* (regex #\"^/dev/tty\"))(deny appleevent-send)\
             (deny process-exec (literal \"/usr/bin/osascript\") (literal \"/usr/bin/osacompile\") \
              (literal \"/usr/bin/open\") (literal \"/bin/launchctl\") (literal \"/usr/bin/automator\"))"
        );
        let mut cmd = Command::new("/usr/bin/sandbox-exec");
        cmd.arg("-p").arg(profile).arg(program);
        cmd
    } else {
        let mut cmd = Command::new("bwrap");
        cmd.args(["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/run"]);
        for dir in &writable {
            cmd.arg("--bind").arg(dir).arg(dir);
        }
        for path in secrets.filter(|p| p.exists()) {
            if path.is_dir() {
                cmd.arg("--tmpfs").arg(&path);
            } else {
                cmd.arg("--ro-bind").arg("/dev/null").arg(&path);
            }
        }
        for dir in protected.iter().chain(&nested(root)).filter(|d| d.exists()) {
            cmd.arg("--ro-bind").arg(dir).arg(dir);
        }
        cmd.arg("--ro-bind").arg(&log).arg(&log);
        cmd.args(["--unshare-all", "--die-with-parent", "--new-session", "--chdir"]).arg(root).arg(program);
        cmd
    };
    let mut hasher = std::hash::DefaultHasher::new();
    root.hash(&mut hasher);
    let cache = std::env::temp_dir().join(format!("air-cache/{:016x}", hasher.finish()));
    let _ = std::fs::create_dir_all(&cache);
    let extra = EXTRA_ENV.get().into_iter().flatten().map(String::as_str);
    let allowed: Vec<&str> = ENV.split_whitespace().chain(extra).collect();
    let allowed =
        |name: &str| allowed.iter().any(|p| p.strip_suffix('*').map_or(*p == name, |prefix| name.starts_with(prefix)));
    cmd.args(args)
        .current_dir(root)
        .env_clear()
        .envs(std::env::vars_os().filter(|(name, _)| name.to_str().is_some_and(allowed)))
        .envs(HERMETIC.split_whitespace().filter_map(|pin| pin.split_once('=')))
        .envs(CACHES.split_whitespace().map(|name| (name, cache.as_os_str())))
        .kill_on_drop(true);
    cmd
}

fn protected(root: &Path) -> Vec<PathBuf> {
    let out = std::process::Command::new("git").arg("-C").arg(root).args(["config", "core.hooksPath"]).output();
    let hooks = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let mut dirs = vec![root.join(".git")];
    if !hooks.is_empty() {
        let home = std::env::var("HOME").unwrap_or_default();
        let hooks = root.join(hooks.strip_prefix("~/").map_or(hooks.clone(), |rest| format!("{home}/{rest}")));
        let hooks = hooks.canonicalize().unwrap_or(hooks);
        dirs.push(if hooks.ends_with("_") { hooks.parent().unwrap_or(&hooks).into() } else { hooks });
    }
    dirs
}

fn nested(root: &Path) -> Vec<PathBuf> {
    let mut git = std::process::Command::new("git");
    let out = git.args(["-c", "core.fsmonitor=false", "-C"]).arg(root).args(["ls-files", "--stage", "-z"]).output();
    let list = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut found: Vec<PathBuf> = list
        .split('\0')
        .filter_map(|e| e.strip_prefix("160000 ")?.split_once('\t').map(|(_, path)| root.join(path).join(".git")))
        .collect();
    let mut dirs = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_name() == ".git" {
                if dir != root {
                    found.push(path);
                }
            } else if depth < 3 && entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push((path, depth + 1));
            }
        }
    }
    found
}

fn writable(root: &Path) -> Vec<PathBuf> {
    let dirs = [root.into(), std::env::temp_dir(), "/tmp".into()];
    dirs.into_iter().filter_map(|d| d.canonicalize().ok()).collect()
}
