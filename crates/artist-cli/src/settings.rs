//! Layered configuration merged from a global `~/.config/artist/settings.toml`, a
//! project `<repo>/.artist/settings.toml`, and an optional highest-precedence
//! override layer (CLI flags / in-session changes).
//!
//! Resolution rules:
//! - **Scalars** (`model`, `reasoning_effort`) take the
//!   value from the highest-precedence layer that sets them: override > project > global.
//! `settings.toml` is deliberately separate from `providers.toml`: the latter
//! holds provider identity and secrets, the former holds overridable behaviour.

use anyhow::{Context, Result};
use llm_provider::SavedProvider;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The settings file name, used for both the global and project locations.
pub const SETTINGS_FILE: &str = "settings.toml";

/// One settings layer, as read from a single `settings.toml`. Every field is
/// optional so an absent field means "defer to a lower layer", not "reset".
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The model to use in this scope. This is the sole home for model choice
    /// (moved out of `providers.toml`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The reasoning effort in this scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

impl Settings {
    /// Load one settings file. A missing file is an empty layer (not an error),
    /// so settings are entirely optional; a malformed file *is* an error,
    /// because silently ignoring a typo'd policy would be worse than failing.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parse {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }
}

/// The highest-precedence layer: values from CLI flags or in-session changes
/// that win over both files. Empty by default.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

/// The resolved configuration a session actually runs with.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EffectiveSettings {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

impl EffectiveSettings {
    /// Resolve the global and project layers plus the override layer.
    /// `base_denied` is the pre-existing global tool gating (the
    pub fn resolve(global: &Settings, project: &Settings, overrides: &Overrides) -> Self {
        let model = overrides
            .model
            .clone()
            .or_else(|| project.model.clone())
            .or_else(|| global.model.clone());
        let reasoning_effort = overrides
            .reasoning_effort
            .clone()
            .or_else(|| project.reasoning_effort.clone())
            .or_else(|| global.reasoning_effort.clone());
        Self {
            model,
            reasoning_effort,
        }
    }

    /// Apply legacy global model/reasoning compatibility to ChatGPT only.
    /// Every other provider owns and persists its selection locally.
    pub fn apply_to(&self, mut provider: SavedProvider) -> SavedProvider {
        if provider.provider == llm_provider::ProviderKind::Chatgpt {
            if self.model.is_some() {
                provider.model = self.model.clone();
            }
            if self.reasoning_effort.is_some() {
                provider.reasoning_effort = self.reasoning_effort.clone();
            }
        }
        provider
    }
}

/// Load and resolve the effective settings for a project: the global file at
/// `<config_root>/settings.toml` and the project file at
/// `<project>/.artist/settings.toml` and any CLI/session `overrides`.
pub fn load_effective(
    config_root: &Path,
    project: &Path,
    overrides: &Overrides,
) -> Result<EffectiveSettings> {
    let global = Settings::load(&config_root.join(SETTINGS_FILE))?;
    let project = Settings::load(&project.join(".artist").join(SETTINGS_FILE))?;
    Ok(EffectiveSettings::resolve(&global, &project, overrides))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_layer() {
        let settings = Settings::load(Path::new("/no/such/settings.toml")).unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn malformed_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "model = [not a string").unwrap();
        assert!(Settings::load(&path).is_err());
    }

    #[test]
    fn unknown_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "modle = \"typo\"\n").unwrap();
        assert!(Settings::load(&path).is_err());
    }

    fn from_str(text: &str) -> Settings {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn project_scalar_overrides_global() {
        let global = from_str("model = \"global-model\"\nreasoning_effort = \"low\"\n");
        let project = from_str("model = \"project-model\"\n");
        let effective = EffectiveSettings::resolve(&global, &project, &Overrides::default());
        // Project wins for model; global fills in the unset reasoning.
        assert_eq!(effective.model.as_deref(), Some("project-model"));
        assert_eq!(effective.reasoning_effort.as_deref(), Some("low"));
    }

    #[test]
    fn overrides_win_over_both_files() {
        let global = from_str("model = \"global-model\"\n");
        let project = from_str("model = \"project-model\"\n");
        let overrides = Overrides {
            model: Some("cli-model".to_owned()),
            reasoning_effort: None,
        };
        let effective = EffectiveSettings::resolve(&global, &project, &overrides);
        assert_eq!(effective.model.as_deref(), Some("cli-model"));
    }

    #[test]
    fn absent_layers_fall_through_to_default() {
        let effective = EffectiveSettings::resolve(
            &Settings::default(),
            &Settings::default(),
            &Overrides::default(),
        );
        assert_eq!(effective.model, None);
        assert_eq!(effective.reasoning_effort, None);
    }

    #[test]
    fn load_effective_reads_both_locations_and_merges() {
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("config");
        let project = dir.path().join("project");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(project.join(".artist")).unwrap();
        std::fs::write(
            config_root.join("settings.toml"),
            "model = \"global\"\nreasoning_effort = \"low\"\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".artist/settings.toml"),
            "model = \"project\"\n",
        )
        .unwrap();

        let effective = load_effective(&config_root, &project, &Overrides::default()).unwrap();

        assert_eq!(effective.model.as_deref(), Some("project"));
        assert_eq!(effective.reasoning_effort.as_deref(), Some("low"));
    }

    #[test]
    fn empty_settings_serializes_to_nothing() {
        // A default Settings must not emit stray empty tables/keys.
        assert_eq!(toml::to_string_pretty(&Settings::default()).unwrap(), "");
    }

    #[test]
    fn load_effective_with_no_files_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let effective = load_effective(
            &dir.path().join("config"),
            &dir.path().join("project"),
            &Overrides::default(),
        )
        .unwrap();
        assert_eq!(effective, EffectiveSettings::default());
    }
}
