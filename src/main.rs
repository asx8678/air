mod agent;
mod compact;
mod llm;
mod providers;
mod render;
mod session;
mod stats;
mod ui;
mod worker;

use anyhow::{Context, Result, bail, ensure};
use rustyline::error::ReadlineError;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The build `low --version` names and the session log records.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " ", env!("LOW_BUILD"));
const ABOUT: &str = "A low-entropy, code-mode coding agent. Without a task it starts a REPL (/compact, /cost, /think, /models [filter], /providers).";
const USAGE: &str = "Usage: low [OPTIONS] [TASK]";
const HELP: &str = "\
Arguments:
  [TASK]  Run this task once and exit (status 1 when a configured check did not pass on the final tree or the turn stopped short)

Options:
      --resume         Resume the latest session in this repo
  -m, --model <MODEL>  Model for a new session; with --resume it switches the session to it (so does /models)
      --yolo           No sandbox: network on, writes anywhere (env vars stay filtered; env = [...] in ~/.config/low/config.toml passes more)
  -h, --help           Print help
  -V, --version        Print version";

struct Cli {
    task: Option<String>,
    resume: bool,
    model: Option<String>,
    yolo: bool,
}

impl Cli {
    /// Exits on --help and --version, and with status 2 on an argument low does not take.
    fn parse() -> Self {
        Self::args().unwrap_or_else(|e| {
            eprintln!("error: {e}\n\n{USAGE}\n\nFor more information, try '--help'.");
            std::process::exit(2)
        })
    }

    fn args() -> Result<Self, lexopt::Error> {
        use lexopt::prelude::*;
        let mut cli = Self { task: None, resume: false, model: None, yolo: false };
        let mut parser = lexopt::Parser::from_env();
        while let Some(arg) = parser.next()? {
            match arg {
                Value(task) if cli.task.is_none() => cli.task = Some(task.string()?),
                Long("resume") => cli.resume = true,
                Short('m') | Long("model") => cli.model = Some(parser.value()?.string()?),
                Long("yolo") => cli.yolo = true,
                Short('h') | Long("help") => {
                    println!("{ABOUT}\n\n{USAGE}\n\n{HELP}");
                    std::process::exit(0)
                }
                Short('V') | Long("version") => {
                    println!("low {VERSION}");
                    std::process::exit(0)
                }
                _ => return Err(arg.unexpected()),
            }
        }
        Ok(cli)
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub check: Option<String>,
    pub fmt: Option<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub models: std::collections::BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
    #[serde(skip)]
    pub providers: providers::Providers,
}

fn load(path: &Path) -> Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).with_context(|| path.display().to_string()),
        Err(_) => Ok(Config::default()),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    ui::guard();
    if let Err(e) = start().await {
        eprintln!("error: {}", agent::error_text(&e));
        std::process::exit(1);
    }
}

async fn start() -> Result<()> {
    let cli = Cli::parse();
    let home = PathBuf::from(std::env::var("HOME")?);
    ensure!(
        cli.task.as_deref().is_none_or(|t| !t.trim().is_empty()),
        "the task is empty: give low something to do, or run it without a task for the REPL"
    );
    let top = std::process::Command::new("git").args(["rev-parse", "--show-toplevel"]).output();
    let top = top.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let repo_dir = top.is_some();
    let root = top.map_or_else(std::env::current_dir, |t| Ok(PathBuf::from(t)))?.canonicalize()?;
    let user = load(&providers::config_dir(&home).join("config.toml"))?;
    check_config(&user)?;
    let repo = load(&root.join("low.toml"))?;
    let warn = |text: &str| {
        eprintln!("{text}");
        ui::record("note", text, serde_json::Value::Null);
    };
    if !repo_dir {
        warn("not a git repository: low runs without undo here, so whatever it changes stays changed (git init adds undo)");
    }
    if !cli.yolo && worker::read_only(&root) {
        warn("this is your home directory or above it: low can read files here but not change them; cd into a folder to let it edit");
    }
    if repo.model.is_some() || repo.base_url.is_some() || !repo.models.is_empty() {
        warn(
            "low.toml: model, base_url and models are ignored; models go in ~/.config/low/config.toml, providers in \
             ~/.config/low/providers.toml",
        );
    }
    if !repo.env.is_empty() {
        warn("low.toml: env is ignored; set env in ~/.config/low/config.toml");
    }
    let providers = providers::load(&home)?.unwrap_or_default();
    let picked = cli.model.clone().filter(|_| cli.resume);
    let config = Config {
        model: cli.model.or(user.model),
        base_url: None,
        check: repo.check.or(user.check).filter(|c| !c.trim().is_empty()),
        fmt: repo.fmt.or(user.fmt),
        env: user.env,
        models: user.models,
        providers,
    };
    worker::allow_env(config.env.clone());
    let mut agent = agent::Agent::new(&root, &home, config, cli.resume, cli.yolo, repo_dir)?;
    if let Some(model) = picked {
        agent.resume_with(&model)?;
    }
    if !cli.resume {
        eprintln!("[session {}]", agent.path().display());
    }
    if let Some(task) = cli.task {
        let verified = agent.run(&task).await?;
        drop(agent);
        std::process::exit(i32::from(!verified));
    }
    let mut editor = rustyline::DefaultEditor::new()?;
    loop {
        let line = match editor.readline("› ") {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => continue,
            Err(_) => return Ok(()),
        };
        let line = line.trim();
        if let Some(why) = unknown(line) {
            let _ = editor.add_history_entry(line);
            eprintln!("error: {why}");
            continue;
        }
        let result = match line {
            "" => Ok(()),
            "/compact" => agent.compact(),
            "/cost" => {
                agent.cost();
                Ok(())
            }
            "/think" => {
                agent.think();
                Ok(())
            }
            "/providers" => {
                async {
                    let mut providers = providers::load(&home)?.unwrap_or_default();
                    let result = providers::menu(&home, &mut providers, agent.provider(), &mut editor).await;
                    agent.set_providers(providers).and(result)
                }
                .await
            }
            _ if let Some(filter) = line.strip_prefix("/models").filter(|f| f.is_empty() || f.starts_with(' ')) => {
                pick(&mut agent, &mut editor, filter.trim()).await
            }
            task => {
                editor.add_history_entry(task)?;
                agent.run(task).await.map(drop)
            }
        };
        let error = result.err().map(|e| agent::error_text(&e));
        if COMMANDS.contains(&line.split_whitespace().next().unwrap_or_default()) {
            ui::record("command", line, serde_json::json!({"error": error}));
        }
        if let Some(error) = error {
            eprintln!("error: {error}");
        }
    }
}

async fn pick(agent: &mut agent::Agent, editor: &mut rustyline::DefaultEditor, filter: &str) -> Result<()> {
    let needle = filter.to_lowercase();
    let all = agent.models().await;
    let models: Vec<&(String, String, bool)> =
        all.iter().filter(|(_, m, _)| m.to_lowercase().contains(&needle)).collect();
    for (i, (provider, model, profiled)) in (1..).zip(models.iter().copied()) {
        let current = provider == agent.provider() && model == agent.model();
        let current = if current { ui::paint("32", " ● current") } else { String::new() };
        let profile = if *profiled { ui::paint("2", " (profile)") } else { String::new() };
        let on = ui::paint("2", &format!("  {provider}"));
        let model = ui::printable(model);
        println!("{} {model}{on}{profile}{current}", ui::paint("2", &format!("{i:>3}")));
    }
    let choice = match editor.readline("model number or name (enter keeps the current one) › ") {
        Ok(choice) => choice.trim().to_string(),
        Err(_) => return Ok(()),
    };
    if choice.is_empty() {
        return Ok(());
    }
    let listed = match choice.parse::<usize>() {
        Ok(n) => Some(*models.get(n.wrapping_sub(1)).with_context(|| format!("no model numbered {n}"))?),
        Err(_) => all
            .iter()
            .find(|(p, m, _)| p == agent.provider() && *m == choice)
            .or_else(|| all.iter().find(|(_, m, _)| *m == choice)),
    };
    let (provider, model) = match listed {
        Some((provider, model, _)) => (provider.clone(), model.clone()),
        None => {
            ui::note(&format!("{} is not listed; trying it on {}", ui::printable(&choice), agent.provider()));
            (agent.provider().to_string(), choice)
        }
    };
    if provider == agent.provider() && model == agent.model() {
        return Ok(());
    }
    agent.switch(&provider, &model)
}

const COMMANDS: &[&str] = &["/compact", "/cost", "/think", "/models", "/providers"];

/// A "/word" line that names no command is a typo, and must not reach the model as a task.
fn unknown(line: &str) -> Option<String> {
    let word = line.split_whitespace().next()?;
    let name = word.strip_prefix('/')?;
    if name.contains('/') || COMMANDS.contains(&word) {
        return None;
    }
    let near = COMMANDS.iter().copied().find(|c| !name.is_empty() && (c.starts_with(word) || word.starts_with(c)));
    Some(match near {
        Some(near) => format!("unknown command {word}; did you mean {near}?"),
        None => format!("unknown command {word}; the commands are {}", COMMANDS.join(" ")),
    })
}

/// Stops on a setting low no longer reads, or on a compact_at it cannot use.
fn check_config(user: &Config) -> Result<()> {
    if user.base_url.is_some() {
        let model = user.model.as_deref().unwrap_or("model-id");
        bail!("~/.config/low/config.toml sets base_url, which low no longer reads; {}", providers::example(model));
    }
    for (model, profile) in &user.models {
        let at = format!("~/.config/low/config.toml: [models.\"{model}\"]");
        if let Some(key) = ["think_limit", "turn_tokens", "guidance"].into_iter().find(|k| profile.contains_key(*k)) {
            bail!("{at} sets {key}, which low no longer supports; remove it");
        }
        if let Some(value) = profile.get("compact_at").filter(|v| v.as_u64().is_none_or(|n| n < 1024)) {
            bail!("{at} sets compact_at = {value}; it takes a whole number of tokens, at least 1024");
        }
    }
    Ok(())
}
