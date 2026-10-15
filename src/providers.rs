use crate::{llm::Llm, ui};
use anyhow::{Context, Result};
use rustyline::DefaultEditor;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
}

pub type Providers = BTreeMap<String, Provider>;

fn file(home: &Path) -> PathBuf {
    home.join(".config/air/providers.toml")
}

pub fn load(home: &Path) -> Result<Option<Providers>> {
    let path = file(home);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(toml::from_str(&text).with_context(|| path.display().to_string())?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context(path.display().to_string()),
    }
}

fn save(home: &Path, providers: &Providers) -> Result<()> {
    let path = file(home);
    let dir = path.parent().context("providers.toml has a parent")?;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let tmp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
    let mut out = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    out.write_all(toml::to_string(providers)?.as_bytes())?;
    out.sync_all()?;
    std::fs::rename(&tmp, &path).with_context(|| path.display().to_string())?;
    Ok(())
}

pub fn ask(editor: &mut DefaultEditor, prompt: &str) -> Option<String> {
    editor.readline(prompt).ok().map(|line| line.trim().to_string())
}

fn number(i: usize) -> String {
    ui::paint("2", &format!("{i:>3}"))
}

pub async fn menu(home: &Path, providers: &mut Providers, current: &str, editor: &mut DefaultEditor) -> Result<()> {
    loop {
        let names: Vec<String> = providers.keys().cloned().collect();
        if names.is_empty() {
            ui::note("no providers yet");
        }
        for (i, name) in (1..).zip(&names) {
            let p = &providers[name];
            let key = if p.api_key.is_some() { "own key" } else { "shared key" };
            let mark = if name == current { ui::paint("32", " ● current") } else { String::new() };
            let info = ui::paint("2", &format!("{}  {key}  {} model(s)", p.base_url, p.models.len()));
            println!("{} {}  {info}{mark}", number(i), ui::printable(name));
        }
        let Some(line) = ask(editor, "a add · e <n> edit · m <n> models · d <n> delete · enter done › ") else {
            return Ok(());
        };
        let (cmd, arg) = line.split_once(' ').unwrap_or((&line, ""));
        if cmd.is_empty() {
            return Ok(());
        }
        if cmd == "a" {
            add(home, providers, editor).await?;
            continue;
        }
        let Some(name) = arg.trim().parse::<usize>().ok().and_then(|n| names.get(n.wrapping_sub(1))) else {
            ui::note("pick a provider by its number, e.g. e 1");
            continue;
        };
        match cmd {
            "e" => {
                edit(providers.get_mut(name).context("listed provider exists")?, editor);
                save(home, providers)?;
                models(home, providers, name, editor).await?;
            }
            "m" => models(home, providers, name, editor).await?,
            "d" => {
                if ask(editor, &format!("delete {name}? [y/N] › ")).is_some_and(|a| a.eq_ignore_ascii_case("y")) {
                    providers.remove(name);
                    save(home, providers)?;
                    ui::note(&format!("deleted {name}"));
                }
            }
            _ => ui::note("unknown command"),
        }
    }
}

async fn add(home: &Path, providers: &mut Providers, editor: &mut DefaultEditor) -> Result<()> {
    let Some(name) = ask(editor, "name › ").filter(|n| !n.is_empty()) else { return Ok(()) };
    if providers.contains_key(&name) {
        ui::note(&format!("{name} already exists; edit it with e <n>"));
        return Ok(());
    }
    let Some(base_url) =
        ask(editor, "base url (OpenAI-compatible, e.g. https://api.example.com/v1) › ").filter(|u| !u.is_empty())
    else {
        return Ok(());
    };
    let key = ask(editor, "api key (enter uses AIR_API_KEY or ~/.config/air/api_key) › ").filter(|k| !k.is_empty());
    providers.insert(name.clone(), Provider { base_url, api_key: key, models: vec![] });
    save(home, providers)?;
    ui::note(&format!("added {name}; now add its models"));
    models(home, providers, &name, editor).await
}

fn edit(provider: &mut Provider, editor: &mut DefaultEditor) {
    let prompt = format!("base url [{}] (enter keeps) › ", provider.base_url);
    if let Some(url) = ask(editor, &prompt).filter(|u| !u.is_empty()) {
        provider.base_url = url;
    }
    let state = if provider.api_key.is_some() { "own key set" } else { "shared key" };
    match ask(editor, &format!("api key [{state}] (enter keeps, - uses the shared key) › ")).as_deref() {
        Some("-") => provider.api_key = None,
        Some(key) if !key.is_empty() => provider.api_key = Some(key.into()),
        _ => {}
    }
}

async fn models(home: &Path, providers: &mut Providers, name: &str, editor: &mut DefaultEditor) -> Result<()> {
    loop {
        let provider = providers.get_mut(name).context("provider exists")?;
        println!("models of {}:", ui::printable(name));
        if provider.models.is_empty() {
            ui::note("none yet");
        }
        for (i, model) in (1..).zip(&provider.models) {
            println!("{} {}", number(i), ui::printable(model));
        }
        let Some(line) = ask(editor, "a [filter] add · d <n> delete · enter done › ") else { return Ok(()) };
        let (cmd, arg) = line.split_once(' ').map_or((line.as_str(), ""), |(c, a)| (c, a.trim()));
        match cmd {
            "" => return Ok(()),
            "a" => {
                let llm = Llm::new(&provider.base_url, provider.api_key.as_deref(), &serde_json::json!({}));
                let needle = arg.to_lowercase();
                let listed: Vec<String> = match llm.models().await {
                    Ok(listed) => listed
                        .into_iter()
                        .filter(|m| m.to_lowercase().contains(&needle) && !provider.models.contains(m))
                        .collect(),
                    Err(e) => {
                        ui::note(&format!("could not list {name}'s models: {e:#}"));
                        vec![]
                    }
                };
                for (i, model) in (1..).zip(&listed) {
                    println!("{} {}", number(i), ui::printable(model));
                }
                let prompt = if listed.is_empty() { "model names › " } else { "numbers or model names › " };
                let Some(picked) = ask(editor, prompt) else { continue };
                for word in picked.split([',', ' ']).filter(|w| !w.is_empty()) {
                    let model = match word.parse::<usize>() {
                        Ok(n) => match listed.get(n.wrapping_sub(1)) {
                            Some(model) => model.clone(),
                            None => {
                                ui::note(&format!("no model numbered {n}"));
                                continue;
                            }
                        },
                        Err(_) => word.to_string(),
                    };
                    if !provider.models.contains(&model) {
                        provider.models.push(model);
                    }
                }
                save(home, providers)?;
            }
            "d" => match arg.parse::<usize>().ok().filter(|n| (1..=provider.models.len()).contains(n)) {
                Some(n) => {
                    let model = provider.models.remove(n - 1);
                    save(home, providers)?;
                    ui::note(&format!("removed {model}"));
                }
                None => ui::note("pick a model by its number, e.g. d 1"),
            },
            _ => ui::note("unknown command"),
        }
    }
}
