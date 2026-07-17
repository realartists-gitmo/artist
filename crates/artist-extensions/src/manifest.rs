use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub tools: Vec<ToolDeclaration>,
    #[serde(default)]
    pub statusbar: Vec<StatusDeclaration>,
    #[serde(default)]
    pub commands: Vec<CommandDeclaration>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    #[serde(default = "empty_schema")]
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StatusDeclaration {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_interval")]
    pub refresh_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandDeclaration {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub usage: String,
}

fn default_true() -> bool {
    true
}
fn default_interval() -> u64 {
    1_000
}
fn empty_schema() -> serde_json::Value {
    serde_json::json!({"type":"object"})
}

impl Manifest {
    pub fn load(path: &Path, directory_id: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read extension manifest {}", path.display()))?;
        let manifest: Self = toml::from_str(&text)
            .with_context(|| format!("parse extension manifest {}", path.display()))?;
        if manifest.id != directory_id {
            bail!(
                "manifest id {:?} does not match directory {:?}",
                manifest.id,
                directory_id
            );
        }
        if manifest.id.is_empty() || manifest.id.contains(['/', '\\']) {
            bail!("invalid extension id {:?}", manifest.id);
        }
        for command in &manifest.commands {
            if !command.name.starts_with('/') || command.name[1..].contains('/') {
                bail!("invalid slash command {:?}", command.name);
            }
        }
        Ok(manifest)
    }
}
