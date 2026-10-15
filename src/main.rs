mod agent;
mod compact;
mod llm;
mod providers;
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
#[command(version = concat!(env!("CARGO_PKG_VERSION"), " ", env!("AIR_BUILD")), about = "Minimal code-mode coding agent. Without a task it starts a REPL (/compact, /cost, /think, /models [filter], /providers).")]
struct Cli {
    #[arg(help = "Run this task once and exit (status 1 when the result is unverified or the turn stopped short)")]
    task: Option<String>,
    #[arg(long, help = "Resume the latest session in this repo")]
    resume: bool,
    #[arg(short, long, help = "Model for a new session; with --resume it switches the session to it (so does /models)")]
    model: Option<String>,
    #[arg(long, help = "No sandbox: network on, writes anywhere (env vars stay filtered; env = [...] in ~/.config/air/config.toml passes more)")]
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
        eprintln!("error: {}", agent::redact(&ui::printable(&format!("{e:#}"))).0);
        std::process::exit(1);
    }
}

async fn start() -> Result<()> {
    let cli = Cli::parse();
    let home = PathBuf::from(std::env::var("HOME")?);
    if cli.stats {
        return stats::print(&home);
    }
    if cli.task.as_deref().is_some_and(|t| t.trim().is_empty()) {
        bail!("the task is empty: give air something to do, or run it without a task for the REPL");
    }
    let top = std::process::Command::new("git").args(["rev-parse", "--show-toplevel"]).output()?;
    if !top.status.success() {
        bail!("air works inside a git repository (git is the undo)");
    }
    let root = PathBuf::from(String::from_utf8(top.stdout)?.trim()).canonicalize()?;
    let user = load(&home.join(".config/air/config.toml"))?;
    let repo = load(&root.join("air.toml"))?;
    let warn = |text: &str| {
        eprintln!("{text}");
        ui::record("note", text, serde_json::Value::Null);
    };
    if repo.model.is_some() || repo.base_url.is_some() || !repo.models.is_empty() {
        warn("air.toml: model, base_url and models are ignored; set them in ~/.config/air/config.toml");
    }
    if !repo.env.is_empty() {
        warn("air.toml: env is ignored; set env in ~/.config/air/config.toml");
    }
    let fallback = user.base_url.map(|base_url| {
        let models = user.model.iter().cloned().collect();
        providers::Provider { base_url, api_key: None, models }
    });
    let providers = with_config(providers::load(&home)?.unwrap_or_default(), &fallback);
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
    let mut agent = agent::Agent::new(&root, &home, config, cli.resume, cli.yolo)?;
    if let Some(model) = picked {
        agent.resume_with(&model)?;
    }
    if !cli.resume {
        eprintln!("[session {}]", agent.path().display());
    }
    if let Some(task) = cli.task {
        let verified = agent.run(&task).await?;
        drop(agent);
        std::process::exit(if verified { 0 } else { 1 });
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
            "/providers" => match providers::load(&home) {
                Ok(loaded) => {
                    let mut providers = loaded.unwrap_or_default();
                    let fixed = fallback.as_ref().filter(|_| !providers.contains_key("config"));
                    let result = providers::menu(&home, &mut providers, fixed, agent.provider(), &mut editor).await;
                    agent.set_providers(with_config(providers, &fallback)).and(result)
                }
                Err(e) => Err(e),
            },
            line if line == "/models" || line.starts_with("/models ") => {
                pick(&mut agent, &mut editor, line["/models".len()..].trim()).await
            }
            task => {
                editor.add_history_entry(task)?;
                agent.run(task).await.map(drop)
            }
        };
        let error = result.err().map(|e| agent::redact(&ui::printable(&format!("{e:#}"))).0);
        if COMMANDS.contains(&line.split_whitespace().next().unwrap_or_default()) {
            ui::record("command", line, serde_json::json!({"error": error}));
        }
        if let Some(error) = error {
            eprintln!("error: {error}");
        }
    }
}

fn with_config(mut providers: providers::Providers, fallback: &Option<providers::Provider>) -> providers::Providers {
    if let Some(provider) = fallback {
        if providers.contains_key("config") {
            ui::note("providers.toml: its [config] entry replaces base_url and model from config.toml");
        }
        providers.entry("config".into()).or_insert_with(|| provider.clone());
    }
    providers
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
    let (provider, model) = match choice.parse::<usize>() {
        Ok(n) => match models.get(n.wrapping_sub(1)) {
            Some((provider, model, _)) => (provider.clone(), model.clone()),
            None => bail!("no model numbered {n}"),
        },
        Err(_) => match all
            .iter()
            .find(|(p, m, _)| p == agent.provider() && *m == choice)
            .or_else(|| all.iter().find(|(_, m, _)| *m == choice))
        {
            Some((provider, model, _)) => (provider.clone(), model.clone()),
            None => {
                ui::note(&format!("{} is not listed; trying it on {}", ui::printable(&choice), agent.provider()));
                (agent.provider().to_string(), choice)
            }
        },
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

#[cfg(test)]
mod tests {
    use super::unknown;

    #[test]
    fn typos_are_not_tasks() {
        assert_eq!(unknown("/model").as_deref(), Some("unknown command /model; did you mean /models?"));
        assert!(unknown("/help").is_some_and(|m| m.ends_with("/compact /cost /think /models /providers")));
        assert!(unknown("/").is_some_and(|m| m.contains("the commands are")));
        assert_eq!(unknown("/models gpt"), None);
        assert_eq!(unknown("/etc/hosts has a typo"), None);
        assert_eq!(unknown("fix /model"), None);
    }
}
