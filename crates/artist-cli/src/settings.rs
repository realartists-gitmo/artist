//! Layered configuration merged from a global `~/.artist/settings.toml`, a
//! project `<repo>/.artist/settings.toml`, and an optional highest-precedence
//! override layer (CLI flags / in-session changes).
//!
//! Resolution rules:
//! - **Scalars** (`model`, `reasoning_effort`) take the value from the
//!   highest-precedence layer that sets them: override > project > global.
//! - **Restriction lists** (denied tools) are **unioned** across every layer,
//!   including the pre-existing global `disabled_tools` in `providers.toml`, so
//!   a project can only ever *tighten* access, never silently loosen it.
//!
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
    #[serde(default, skip_serializing_if = "Permissions::is_empty")]
    pub permissions: Permissions,
}

/// The access-policy section. Today the only primitive is a tool denylist; it
/// is structured as its own table so richer policy can be added without
/// reshaping the file.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    /// Tool names the agent may not use in this scope. Unioned with the global
    /// `disabled_tools` and across layers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

impl Permissions {
    fn is_empty(&self) -> bool {
        self.deny.is_empty()
    }
}

impl Settings {
    /// Load one settings file. A missing file is an empty layer (not an error),
    /// so settings are entirely optional; a malformed file *is* an error,
    /// because silently ignoring a typo'd policy would be worse than failing.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    /// Write these settings atomically, creating the parent directory. Empty
    /// fields are omitted so the file stays minimal.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialize settings")?;
        let temp = path.with_extension("toml.tmp");
        std::fs::write(&temp, text).with_context(|| format!("write {}", temp.display()))?;
        std::fs::rename(&temp, path).with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    }
}

/// Set the model/reasoning defaults in the global settings file
/// (`<config_root>/settings.toml`), preserving any other settings already
/// there. `None` clears the field. This is what `artist model` writes to.
pub fn write_provider_defaults(
    config_root: &Path,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
) -> Result<()> {
    let path = config_root.join(SETTINGS_FILE);
    let mut settings = Settings::load(&path)?;
    settings.model = model.map(str::to_owned);
    settings.reasoning_effort = reasoning_effort.map(str::to_owned);
    settings.save(&path)
}

/// One-time move of a provider's model/reasoning (the pre-settings location in
/// `providers.toml`) into the global settings file. It runs only when the
/// settings file has no model/reasoning yet, so it is idempotent — once moved,
/// `providers.toml` drops the fields on its next save and this becomes a no-op.
/// Returns whether it wrote anything.
pub fn migrate_provider_defaults(
    config_root: &Path,
    provider_model: Option<&str>,
    provider_reasoning: Option<&str>,
) -> Result<bool> {
    if provider_model.is_none() && provider_reasoning.is_none() {
        return Ok(false);
    }
    let path = config_root.join(SETTINGS_FILE);
    let mut settings = Settings::load(&path)?;
    if settings.model.is_some() || settings.reasoning_effort.is_some() {
        return Ok(false);
    }
    settings.model = provider_model.map(str::to_owned);
    settings.reasoning_effort = provider_reasoning.map(str::to_owned);
    settings.save(&path)?;
    Ok(true)
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
    /// The full set of tool names the agent may not use — the union of the
    /// global `disabled_tools` and every layer's `permissions.deny`.
    pub denied_tools: Vec<String>,
}

impl EffectiveSettings {
    /// Resolve the global and project layers plus the override layer.
    /// `base_denied` is the pre-existing global tool gating (the
    /// `providers.toml` `disabled_tools`), folded into the union.
    pub fn resolve(
        global: &Settings,
        project: &Settings,
        overrides: &Overrides,
        base_denied: &[String],
    ) -> Self {
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
        let mut denied_tools = Vec::new();
        for name in base_denied
            .iter()
            .chain(&global.permissions.deny)
            .chain(&project.permissions.deny)
        {
            if !denied_tools.iter().any(|existing| existing == name) {
                denied_tools.push(name.clone());
            }
        }
        Self {
            model,
            reasoning_effort,
            denied_tools,
        }
    }

    /// Apply the resolved model/reasoning override onto a provider for the
    /// lifetime of a session, without touching the persisted store. A `None`
    /// override leaves the provider's own value in place, so this is safe to
    /// call on every provider (including one just switched to via `/accounts`).
    pub fn apply_to(&self, mut provider: SavedProvider) -> SavedProvider {
        if self.model.is_some() {
            provider.model = self.model.clone();
        }
        if self.reasoning_effort.is_some() {
            provider.reasoning_effort = self.reasoning_effort.clone();
        }
        provider
    }
}

/// Load and resolve the effective settings for a project: the global file at
/// `<config_root>/settings.toml` and the project file at
/// `<project>/.artist/settings.toml`, folding in `base_denied` (the global
/// `disabled_tools`) and any CLI/session `overrides`.
pub fn load_effective(
    config_root: &Path,
    project: &Path,
    overrides: &Overrides,
    base_denied: &[String],
) -> Result<EffectiveSettings> {
    let global = Settings::load(&config_root.join(SETTINGS_FILE))?;
    let project = Settings::load(&project.join(".artist").join(SETTINGS_FILE))?;
    Ok(EffectiveSettings::resolve(
        &global, &project, overrides, base_denied,
    ))
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
        let effective =
            EffectiveSettings::resolve(&global, &project, &Overrides::default(), &[]);
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
        let effective = EffectiveSettings::resolve(&global, &project, &overrides, &[]);
        assert_eq!(effective.model.as_deref(), Some("cli-model"));
    }

    #[test]
    fn absent_layers_fall_through_to_default() {
        let effective = EffectiveSettings::resolve(
            &Settings::default(),
            &Settings::default(),
            &Overrides::default(),
            &[],
        );
        assert_eq!(effective.model, None);
        assert_eq!(effective.reasoning_effort, None);
        assert!(effective.denied_tools.is_empty());
    }

    #[test]
    fn denied_tools_union_across_layers_and_base_without_duplicates() {
        let global = from_str("[permissions]\ndeny = [\"write\", \"bash\"]\n");
        let project = from_str("[permissions]\ndeny = [\"edit\", \"bash\"]\n");
        // providers.toml already disabled `write`.
        let base = ["write".to_owned()];
        let effective = EffectiveSettings::resolve(&global, &project, &Overrides::default(), &base);
        assert_eq!(effective.denied_tools, ["write", "bash", "edit"]);
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
            "model = \"global\"\nreasoning_effort = \"low\"\n[permissions]\ndeny = [\"bash\"]\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".artist/settings.toml"),
            "model = \"project\"\n[permissions]\ndeny = [\"write\"]\n",
        )
        .unwrap();

        let effective = load_effective(
            &config_root,
            &project,
            &Overrides::default(),
            &["edit".to_owned()],
        )
        .unwrap();

        assert_eq!(effective.model.as_deref(), Some("project"));
        assert_eq!(effective.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(effective.denied_tools, ["edit", "bash", "write"]);
    }

    #[test]
    fn migrate_provider_defaults_moves_then_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // First run moves the provider's model/reasoning into settings.
        assert!(migrate_provider_defaults(root, Some("gpt-5"), Some("high")).unwrap());
        let settings = Settings::load(&root.join(SETTINGS_FILE)).unwrap();
        assert_eq!(settings.model.as_deref(), Some("gpt-5"));
        assert_eq!(settings.reasoning_effort.as_deref(), Some("high"));

        // Second run is a no-op — settings already has a model.
        assert!(!migrate_provider_defaults(root, Some("gpt-4"), None).unwrap());
        let settings = Settings::load(&root.join(SETTINGS_FILE)).unwrap();
        assert_eq!(settings.model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn migrate_provider_defaults_noop_when_nothing_to_move() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!migrate_provider_defaults(dir.path(), None, None).unwrap());
        assert!(!dir.path().join(SETTINGS_FILE).exists());
    }

    #[test]
    fn write_provider_defaults_preserves_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Seed a file that also carries a permissions denylist.
        Settings::load(&root.join(SETTINGS_FILE)).unwrap();
        std::fs::write(
            root.join(SETTINGS_FILE),
            "model = \"old\"\n[permissions]\ndeny = [\"bash\"]\n",
        )
        .unwrap();

        write_provider_defaults(root, Some("new"), Some("low")).unwrap();

        let settings = Settings::load(&root.join(SETTINGS_FILE)).unwrap();
        assert_eq!(settings.model.as_deref(), Some("new"));
        assert_eq!(settings.reasoning_effort.as_deref(), Some("low"));
        // The denylist we didn't touch survives the round-trip.
        assert_eq!(settings.permissions.deny, ["bash"]);
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
            &[],
        )
        .unwrap();
        assert_eq!(effective, EffectiveSettings::default());
    }

    #[test]
    fn project_cannot_loosen_a_global_denial() {
        // A project setting has no way to *remove* a global/base denial — the
        // union only adds. `write` stays denied even though the project omits it.
        let global = from_str("[permissions]\ndeny = [\"write\"]\n");
        let project = from_str("model = \"m\"\n");
        let effective =
            EffectiveSettings::resolve(&global, &project, &Overrides::default(), &[]);
        assert!(effective.denied_tools.iter().any(|t| t == "write"));
    }
}
