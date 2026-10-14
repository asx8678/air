mod agent;
mod compact;
mod llm;
mod render;
mod session;
mod stats;
mod ui;
mod worker;

use anyhow::{Context, Result, bail};
use clap::Parser;
use rustyline::error::ReadlineError;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(version, about = "Minimal code-mode coding agent. Without a task it starts a REPL (/compact, /cost, /think).")]
struct Cli {
    #[arg(help = "Run this task once and exit (status 1 when the result is unverified)")]
    task: Option<String>,
    #[arg(long, help = "Resume the latest session in this repo")]
    resume: bool,
    #[arg(short, long, help = "Model for a new session; it stays pinned for the session's whole life")]
    model: Option<String>,
    #[arg(long, help = "No sandbox: network on, writes anywhere")]
    yolo: bool,
    #[arg(long, help = "Compare model settings across all sessions: outcomes, waste and cost per turn")]
    stats: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub check: Option<String>,
    pub fmt: Option<String>,
    #[serde(default)]
    pub models: std::collections::BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
}

fn load(path: &Path) -> Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).with_context(|| path.display().to_string()),
        Err(_) => Ok(Config::default()),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = PathBuf::from(std::env::var("HOME")?);
    if cli.stats {
        return stats::print(&home);
    }
    let top = std::process::Command::new("git").args(["rev-parse", "--show-toplevel"]).output()?;
    if !top.status.success() {
        bail!("air works inside a git repository (git is the undo)");
    }
    let root = PathBuf::from(String::from_utf8(top.stdout)?.trim()).canonicalize()?;
    let user = load(&home.join(".config/air/config.toml"))?;
    let repo = load(&root.join("air.toml"))?;
    if repo.model.is_some() || repo.base_url.is_some() || !repo.models.is_empty() {
        eprintln!("air.toml: model, base_url and models are ignored; set them in ~/.config/air/config.toml");
    }
    let config = Config {
        model: cli.model.or(user.model),
        base_url: user.base_url,
        check: repo.check.or(user.check),
        fmt: repo.fmt.or(user.fmt),
        models: user.models,
    };
    let mut agent = agent::Agent::new(&root, &home, config, cli.resume, cli.yolo)?;
    if let Some(task) = cli.task {
        eprintln!("[session {}]", agent.path().display());
        let verified = agent.run(&task).await?;
        std::process::exit(if verified { 0 } else { 1 });
    }
    let mut editor = rustyline::DefaultEditor::new()?;
    loop {
        let line = match editor.readline("› ") {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => continue,
            Err(_) => return Ok(()),
        };
        let result = match line.trim() {
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
            task => {
                editor.add_history_entry(task)?;
                agent.run(task).await.map(drop)
            }
        };
        if let Err(e) = result {
            eprintln!("error: {}", ui::printable(&format!("{e:#}")));
        }
    }
}
