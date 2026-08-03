//! Lightweight, read-only configuration catalogs for frontends.
//!
//! This crate deliberately does not depend on the CLI or the agent runtime.
//! Frontends can discover configured routing choices without parsing rendered
//! command output, and without loading model clients or tool implementations.

use anyhow::{Context, Result};
use llm_provider::{ProviderId, ProviderKind, SavedProvider};
use serde::Deserialize;
use std::{collections::BTreeSet, fs, path::Path};

pub const BUILTIN_PROFILES: &[&str] = &["default", "worker", "explorer", "planner", "reviewer"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderChoice {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub is_default: bool,
}

#[derive(Default, Deserialize)]
struct ProviderFile {
    default_provider: Option<ProviderId>,
    #[serde(default)]
    providers: Vec<SavedProvider>,
}

pub fn providers(config_root: &Path) -> Result<Vec<ProviderChoice>> {
    let path = config_root.join("providers.toml");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file: ProviderFile = toml::from_str(
        &fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    Ok(file
        .providers
        .into_iter()
        .map(|provider| ProviderChoice {
            is_default: file.default_provider.as_ref() == Some(&provider.id),
            id: provider.id.as_str().to_owned(),
            name: provider.name,
            kind: provider.provider,
            model: provider.model,
            reasoning_effort: provider.reasoning_effort,
        })
        .collect())
}

#[derive(Default, Deserialize)]
struct ProfileFrontmatter {
    name: Option<String>,
}

pub fn profiles(config_root: &Path, project: &Path) -> Vec<String> {
    let mut names = BUILTIN_PROFILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    for root in [
        config_root.join("profiles"),
        project.join(".artist/profiles"),
    ] {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let fallback = path.file_stem().and_then(|stem| stem.to_str());
            let parsed = fs::read_to_string(&path)
                .ok()
                .and_then(|text| frontmatter(&text).map(str::to_owned))
                .and_then(|yaml| serde_yaml::from_str::<ProfileFrontmatter>(&yaml).ok())
                .and_then(|front| front.name);
            if let Some(name) = parsed.as_deref().or(fallback)
                && !name.trim().is_empty()
            {
                names.insert(name.to_owned());
            }
        }
    }
    names.into_iter().collect()
}

fn frontmatter(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_catalog_layers_custom_names_over_builtins() {
        let root = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let profiles = project.path().join(".artist/profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(
            profiles.join("file-name.md"),
            "---\nname: specialist\n---\nPrompt",
        )
        .unwrap();
        let names = super::profiles(root.path(), project.path());
        assert!(names.iter().any(|name| name == "default"));
        assert!(names.iter().any(|name| name == "specialist"));
    }
}
