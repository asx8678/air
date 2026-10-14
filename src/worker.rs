use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const RUN_TIMEOUT: Duration = Duration::from_secs(40 * 60);
const HERMETIC: &str = "TZ=UTC LANG=C.UTF-8 LC_ALL=C.UTF-8 NO_COLOR=1 CLICOLOR=0 CARGO_TERM_COLOR=never CI=1 TERM=dumb \
                        PAGER=cat GIT_PAGER=cat GIT_TERMINAL_PROMPT=0 PYTHONHASHSEED=0 PYTHONUTF8=1 PYTHONDONTWRITEBYTECODE=1";
const CACHES: &str =
    "XDG_CACHE_HOME GOCACHE UV_CACHE_DIR PIP_CACHE_DIR PRE_COMMIT_HOME npm_config_cache PYTHONPYCACHEPREFIX";
const SECRETS: &str = ".air .config/air .ssh .gnupg .aws .azure .kube .docker .netrc .pi .config/gh .config/gcloud \
                       .password-store Library/Keychains .git-credentials .config/git/credentials .npmrc .pypirc \
                       .cargo/credentials .cargo/credentials.toml";

pub struct Worker {
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    busy: bool,
    _child: Child,
}

impl Worker {
    pub fn start(root: &Path, yolo: bool, check: Option<&str>, fmt: Option<&str>, log: &Path) -> Result<Self> {
        let mut child = sandboxed(root, yolo, log, "python3", &["-I", "-B", "-u", "-c", include_str!("worker.py")])
            .env("AIR_CHECK", check.unwrap_or_default())
            .env("AIR_FMT", fmt.unwrap_or_default())
            .env("AIR_LOG", log)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("starting the python3 worker")?;
        let stdin = child.stdin.take().context("worker stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("worker stdout")?).lines();
        Ok(Self { stdin, stdout, busy: false, _child: child })
    }

    pub async fn run(&mut self, code: &str, state: &Value) -> Result<Value> {
        self.busy = true;
        self.stdin.write_all(format!("{}\n", json!({"code": code, "state": state})).as_bytes()).await?;
        let reply = tokio::time::timeout(RUN_TIMEOUT, self.stdout.next_line()).await.context("run() timed out")??;
        self.busy = false;
        Ok(serde_json::from_str(&reply.context("the worker exited")?)?)
    }

    pub async fn settle(&mut self) -> Result<Option<Value>> {
        if !self.busy {
            return Ok(None);
        }
        let reply = tokio::time::timeout(Duration::from_secs(3), self.stdout.next_line()).await;
        self.busy = false;
        match reply {
            Ok(Ok(Some(line))) => Ok(Some(serde_json::from_str(&line)?)),
            _ => bail!("the worker did not report back"),
        }
    }
}

pub fn sandboxed(root: &Path, yolo: bool, log: &Path, program: &str, args: &[&str]) -> Command {
    let writable = writable(root);
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let home = home.canonicalize().unwrap_or(home);
    let secrets = SECRETS.split_whitespace().map(|s| home.join(s));
    let log = log.canonicalize().unwrap_or_else(|_| log.into());
    let protected = protected(root);
    let mut cmd = if yolo {
        Command::new(program)
    } else if cfg!(target_os = "macos") {
        let dirs: String = writable.iter().map(|d| format!(" (subpath {d:?})")).collect();
        let secrets: String = secrets.map(|d| format!(" (subpath {d:?})")).collect();
        let protected: String = protected.iter().map(|d| format!(" (subpath {d:?})")).collect();
        let profile = format!(
            "(version 1)(allow default)(deny network*)\
             (allow network-outbound (remote ip \"localhost:*\"))\
             (allow network-bind network-inbound (local ip \"localhost:*\"))\
             (deny file-write*)(allow file-write*{dirs} (subpath \"/dev\"))(deny file-write*{protected})\
             (deny file-write* (require-all (subpath {root:?}) (regex #\"/\\.git(/|$)\")))\
             (deny file-read-data{secrets})(allow file-read-data (literal {log:?}))\
             (deny file-ioctl (regex #\"^/dev/tty\"))"
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
        for dir in protected.iter().filter(|d| d.exists()) {
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
    let secret = |name: &str| {
        ["KEY", "TOKEN", "SECRET", "PASS", "CREDENTIAL", "AUTH"].iter().any(|s| name.to_uppercase().contains(s))
    };
    cmd.args(args)
        .current_dir(root)
        .env_clear()
        .envs(std::env::vars_os().filter(|(name, _)| !secret(&name.to_string_lossy())))
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

fn writable(root: &Path) -> Vec<PathBuf> {
    let dirs = [root.into(), std::env::temp_dir(), "/tmp".into()];
    dirs.into_iter().filter_map(|d| d.canonicalize().ok()).collect()
}
