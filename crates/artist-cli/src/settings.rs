//! Layered configuration merged from a global `~/.config/artist/settings.toml`, a
//! project `<repo>/.artist/settings.toml`, and an optional highest-precedence
//! override layer (CLI flags / in-session changes).
//!
//! Resolution rules:
//! - **Scalars** (`model`, `reasoning_effort`, compaction fields) take the
//!   value from the highest-precedence layer that sets them: override > project > global.
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
    #[serde(default, skip_serializing_if = "CompactionSettings::is_empty")]
    pub compaction: CompactionSettings,
    #[serde(default, skip_serializing_if = "MemorySettings::is_empty")]
    pub memory: MemorySettings,
    #[serde(default, skip_serializing_if = "ComputerSettings::is_empty")]
    pub computer: ComputerSettings,
    #[serde(default, skip_serializing_if = "Permissions::is_empty")]
    pub permissions: Permissions,
    #[serde(default, skip_serializing_if = "StatefulnessSettings::is_empty")]
    pub statefulness: StatefulnessSettings,
}

/// Optional per-layer statefulness values.
///
/// Each enables a provider-side handle that replaces content artist would
/// otherwise restate on every request. All are off unless a layer says
/// otherwise: every one creates a durable, billed artifact on the user's
/// provider account, which is not something to inherit by default.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StatefulnessSettings {
    /// Continue a provider-held conversation rather than restating history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chaining: Option<bool>,
    /// Reference the system prompt by id rather than sending its text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored_prompt: Option<bool>,
    /// Hold the system prompt in a Gemini explicit context cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini_cache: Option<bool>,
}

impl StatefulnessSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Optional per-layer computer-use values.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComputerSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// How many observations stay materialized in model context before older
    /// ones decay to a stub. `0` disables decay entirely.
    #[serde(
        default,
        alias = "keepRecentObservations",
        skip_serializing_if = "Option::is_none"
    )]
    pub keep_recent_observations: Option<usize>,
    /// Which `Stage` backend to use. Absent means "pick the best available".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// The isolated display's size: `WIDTHxHEIGHT`, or a preset name.
    ///
    /// Worth configuring: viewport size changes what an application shows, so a
    /// responsive page lays out differently and a list renders a different
    /// number of rows.
    ///
    /// Presets are `desktop`, `laptop`, `tablet` and `mobile`.
    ///
    /// **When a narrow viewport is worth it, and when it is a trap.** A mobile
    /// viewport is reported elsewhere as a token saving, and it is one — a
    /// responsive site renders far fewer nodes. But it buys that by *hiding*
    /// things: navigation collapses into a menu, columns stack, secondary
    /// actions move behind a disclosure. Every one of those is a round trip the
    /// agent now has to spend to reach something that was previously on screen,
    /// and round trips are the dominant cost of driving a UI. So it pays when
    /// the task is a narrow path through a site that has a real mobile layout,
    /// and it costs when the task is exploratory or the application merely
    /// squashes rather than reflows. Default to `desktop`; reach for `mobile`
    /// when you know the shape of the task in advance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

impl ComputerSettings {
    fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.keep_recent_observations.is_none()
            && self.stage.is_none()
            && self.screen.is_none()
    }
}

/// Resolved computer-use policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerConfig {
    pub enabled: bool,
    pub keep_recent_observations: usize,
    pub stage: Option<String>,
    /// Virtual screen size for the isolated display.
    pub screen: (i32, i32),
}

impl Default for ComputerConfig {
    fn default() -> Self {
        Self {
            // On by default: unlike memory, the subsystem needs no model and no
            // download, and a stage that cannot start reports that plainly
            // rather than the tool silently not existing.
            enabled: true,
            keep_recent_observations: 3,
            stage: None,
            screen: (1920, 1080),
        }
    }
}

/// Parse a `WIDTHxHEIGHT` screen setting.
///
/// A malformed value falls back to the default rather than failing the whole
/// settings load: a typo in an optional display size should not stop the agent
/// from starting.
fn parse_screen(value: &str) -> Option<(i32, i32)> {
    let value = value.trim();
    // Presets first, so `mobile` is not read as a malformed `WIDTHxHEIGHT`.
    // The sizes are ordinary CSS viewports rather than device pixels: what
    // decides a responsive layout is the logical width, and the stage runs at
    // scale 1 so the two are the same here.
    if let Some(size) = SCREEN_PRESETS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(value))
        .map(|(_, size)| *size)
    {
        return Some(size);
    }
    let (width, height) = value.split_once(['x', 'X'])?;
    Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
}

/// Named viewports, so a setting can say what it means.
pub const SCREEN_PRESETS: &[(&str, (i32, i32))] = &[
    ("desktop", (1920, 1080)),
    ("laptop", (1440, 900)),
    ("tablet", (820, 1180)),
    ("mobile", (390, 844)),
];

/// Optional per-layer memory values.
///
/// Behaviour only — the embedding model runs locally, so there is no credential
/// here and nothing that belongs in `providers.toml`.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemorySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Directory holding `model.onnx` and `tokenizer.json`. Relative paths
    /// resolve against the config root.
    #[serde(default, alias = "modelDir", skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<String>,
    /// Embedding width. Must match the model; changing it forces a rebuild.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<usize>,
    /// Facts returned per recall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recall_limit: Option<usize>,
    /// Whether the automatic write triggers may store facts. With this off the
    /// `memory` tool still works, so the model can record deliberately while
    /// nothing is captured behind your back.
    #[serde(default, alias = "autoWrite", skip_serializing_if = "Option::is_none")]
    pub auto_write: Option<bool>,
    /// Whether to maintain the embedded code index. Separable from facts
    /// because a full index costs close to two hours on a large repository.
    #[serde(default, alias = "indexCode", skip_serializing_if = "Option::is_none")]
    pub index_code: Option<bool>,
}

impl MemorySettings {
    fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.model_dir.is_none()
            && self.dim.is_none()
            && self.recall_limit.is_none()
            && self.auto_write.is_none()
            && self.index_code.is_none()
    }
}

/// Resolved memory policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub model_dir: Option<String>,
    pub dim: usize,
    pub recall_limit: usize,
    pub auto_write: bool,
    pub index_code: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            // Off until a model is present: the subsystem needs a local
            // embedding model, and silently degrading to lexical-only recall
            // would look like memory simply not working.
            enabled: false,
            model_dir: None,
            dim: 768,
            recall_limit: 8,
            auto_write: true,
            index_code: false,
        }
    }
}

/// Optional per-layer compaction values. The effective defaults match Pi.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompactionSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        alias = "reserveTokens",
        skip_serializing_if = "Option::is_none"
    )]
    pub reserve_tokens: Option<u64>,
    #[serde(
        default,
        alias = "keepRecentTokens",
        skip_serializing_if = "Option::is_none"
    )]
    pub keep_recent_tokens: Option<u64>,
}

impl CompactionSettings {
    fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.reserve_tokens.is_none() && self.keep_recent_tokens.is_none()
    }
}

/// Resolved compaction policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
        }
    }
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
    pub compaction: CompactionConfig,
    pub memory: MemoryConfig,
    pub computer: ComputerConfig,
    /// The full set of tool names the agent may not use — the union of the
    /// global `disabled_tools` and every layer's `permissions.deny`.
    pub denied_tools: Vec<String>,
    /// Which provider-side handles this session may use.
    pub statefulness: artist_agent::Statefulness,
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
        let defaults = CompactionConfig::default();
        let compaction = CompactionConfig {
            enabled: project
                .compaction
                .enabled
                .or(global.compaction.enabled)
                .unwrap_or(defaults.enabled),
            reserve_tokens: project
                .compaction
                .reserve_tokens
                .or(global.compaction.reserve_tokens)
                .unwrap_or(defaults.reserve_tokens),
            keep_recent_tokens: project
                .compaction
                .keep_recent_tokens
                .or(global.compaction.keep_recent_tokens)
                .unwrap_or(defaults.keep_recent_tokens),
        };
        let memory_defaults = MemoryConfig::default();
        let memory = MemoryConfig {
            enabled: project
                .memory
                .enabled
                .or(global.memory.enabled)
                .unwrap_or(memory_defaults.enabled),
            model_dir: project
                .memory
                .model_dir
                .clone()
                .or_else(|| global.memory.model_dir.clone()),
            dim: project
                .memory
                .dim
                .or(global.memory.dim)
                .unwrap_or(memory_defaults.dim),
            recall_limit: project
                .memory
                .recall_limit
                .or(global.memory.recall_limit)
                .unwrap_or(memory_defaults.recall_limit),
            auto_write: project
                .memory
                .auto_write
                .or(global.memory.auto_write)
                .unwrap_or(memory_defaults.auto_write),
            index_code: project
                .memory
                .index_code
                .or(global.memory.index_code)
                .unwrap_or(memory_defaults.index_code),
        };
        let computer_defaults = ComputerConfig::default();
        let computer = ComputerConfig {
            enabled: project
                .computer
                .enabled
                .or(global.computer.enabled)
                .unwrap_or(computer_defaults.enabled),
            keep_recent_observations: project
                .computer
                .keep_recent_observations
                .or(global.computer.keep_recent_observations)
                .unwrap_or(computer_defaults.keep_recent_observations),
            stage: project
                .computer
                .stage
                .clone()
                .or_else(|| global.computer.stage.clone()),
            screen: project
                .computer
                .screen
                .as_deref()
                .or(global.computer.screen.as_deref())
                .and_then(parse_screen)
                .unwrap_or(computer_defaults.screen),
        };
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
        // Project layer wins over global, and absent means off. Deliberately
        // no override layer: these are not the sort of switch to flip from a
        // command line, since each one creates something durable on the user's
        // provider account.
        let statefulness = artist_agent::Statefulness::default()
            .with_chaining(pick(
                project.statefulness.chaining,
                global.statefulness.chaining,
            ))
            .with_stored_prompt(pick(
                project.statefulness.stored_prompt,
                global.statefulness.stored_prompt,
            ))
            .with_gemini_cache(pick(
                project.statefulness.gemini_cache,
                global.statefulness.gemini_cache,
            ));
        Self {
            model,
            reasoning_effort,
            compaction,
            memory,
            computer,
            denied_tools,
            statefulness,
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
/// `<project>/.artist/settings.toml`, folding in `base_denied` (the global
/// `disabled_tools`) and any CLI/session `overrides`.
/// Nearest layer that expressed an opinion, defaulting to off.
fn pick(project: Option<bool>, global: Option<bool>) -> bool {
    project.or(global).unwrap_or(false)
}

pub fn load_effective(
    config_root: &Path,
    project: &Path,
    overrides: &Overrides,
    base_denied: &[String],
) -> Result<EffectiveSettings> {
    let global = Settings::load(&config_root.join(SETTINGS_FILE))?;
    let project = Settings::load(&project.join(".artist").join(SETTINGS_FILE))?;
    Ok(EffectiveSettings::resolve(
        &global,
        &project,
        overrides,
        base_denied,
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
        let effective = EffectiveSettings::resolve(&global, &project, &Overrides::default(), &[]);
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
    fn compaction_defaults_and_project_overrides_resolve_per_field() {
        let global = from_str(
            "[compaction]\nenabled = false\nreserve_tokens = 12000\nkeepRecentTokens = 9000\n",
        );
        let project = from_str("[compaction]\nenabled = true\nreserveTokens = 8000\n");

        let effective = EffectiveSettings::resolve(&global, &project, &Overrides::default(), &[]);

        assert_eq!(
            effective.compaction,
            CompactionConfig {
                enabled: true,
                reserve_tokens: 8_000,
                keep_recent_tokens: 9_000,
            }
        );
        assert_eq!(
            EffectiveSettings::default().compaction,
            CompactionConfig::default()
        );
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

    /// Absent means off. Every one of these creates a durable, billed artifact
    /// on the user's provider account, so silence must never be consent.
    #[test]
    fn statefulness_is_off_when_no_layer_mentions_it() {
        let effective = EffectiveSettings::resolve(
            &Settings::default(),
            &Settings::default(),
            &Overrides::default(),
            &[],
        );
        assert_eq!(effective.statefulness, artist_agent::Statefulness::default());
        assert!(!effective.statefulness.chaining);
        assert!(!effective.statefulness.stored_prompt);
        assert!(!effective.statefulness.gemini_cache);
    }

    #[test]
    fn a_global_opt_in_reaches_the_session() {
        let global = from_str("[statefulness]\nchaining = true\n");
        let effective = EffectiveSettings::resolve(
            &global,
            &Settings::default(),
            &Overrides::default(),
            &[],
        );
        assert!(effective.statefulness.chaining);
        // Enabling one must not enable the others: they need different
        // endpoints and carry different risks.
        assert!(!effective.statefulness.stored_prompt);
        assert!(!effective.statefulness.gemini_cache);
    }

    /// The nearest layer wins, in both directions — including a project that
    /// turns something off which the user enabled globally. A project that
    /// cannot opt out is not a layer.
    #[test]
    fn the_project_layer_overrides_the_global_one() {
        let global = from_str("[statefulness]\nchaining = true\nstored_prompt = true\n");
        let project = from_str("[statefulness]\nchaining = false\ngemini_cache = true\n");
        let effective =
            EffectiveSettings::resolve(&global, &project, &Overrides::default(), &[]);

        assert!(!effective.statefulness.chaining, "project must be able to opt out");
        // Untouched by the project layer, so the global opinion stands.
        assert!(effective.statefulness.stored_prompt);
        assert!(effective.statefulness.gemini_cache);
    }

    /// The same resolution, through the on-disk path callers actually use.
    #[test]
    fn statefulness_resolves_through_both_settings_files() {
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("config");
        let project = dir.path().join("project");
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(project.join(".artist")).unwrap();
        std::fs::write(
            config_root.join("settings.toml"),
            "[statefulness]\nchaining = true\nstored_prompt = true\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".artist/settings.toml"),
            "[statefulness]\nstored_prompt = false\n",
        )
        .unwrap();

        let effective =
            load_effective(&config_root, &project, &Overrides::default(), &[]).unwrap();

        assert!(effective.statefulness.chaining, "global opinion survives");
        assert!(
            !effective.statefulness.stored_prompt,
            "project opinion overrides global"
        );
        assert!(!effective.statefulness.gemini_cache);
    }

    /// An unknown key under `[statefulness]` must be an error rather than a
    /// silent no-op: a typo in a security-relevant switch that reads as
    /// "enabled" while behaving as "disabled" is the worst of both.
    #[test]
    fn a_misspelled_statefulness_key_is_rejected() {
        assert!(toml::from_str::<Settings>("[statefulness]\nchainning = true\n").is_err());
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
        let effective = EffectiveSettings::resolve(&global, &project, &Overrides::default(), &[]);
        assert!(effective.denied_tools.iter().any(|t| t == "write"));
    }

    #[test]
    fn computer_settings_layer_project_over_global() {
        let global = from_str("[computer]\nkeep_recent_observations = 5\nstage = \"wayland\"\n");
        let project = from_str("[computer]\nkeep_recent_observations = 1\n");
        let effective = EffectiveSettings::resolve(&global, &project, &Overrides::default(), &[]);

        assert_eq!(effective.computer.keep_recent_observations, 1);
        // An absent project value defers to the lower layer rather than resetting.
        assert_eq!(effective.computer.stage.as_deref(), Some("wayland"));
        assert!(effective.computer.enabled);
    }

    #[test]
    fn a_screen_size_parses_and_a_typo_falls_back() {
        let global = from_str("[computer]\nscreen = \"1280x800\"\n");
        let effective =
            EffectiveSettings::resolve(&global, &Settings::default(), &Overrides::default(), &[]);
        assert_eq!(effective.computer.screen, (1280, 800));

        // A typo in an optional display size must not stop the agent starting.
        let broken = from_str("[computer]\nscreen = \"enormous\"\n");
        let effective =
            EffectiveSettings::resolve(&broken, &Settings::default(), &Overrides::default(), &[]);
        assert_eq!(effective.computer.screen, (1920, 1080));
    }

    #[test]
    fn computer_defaults_apply_when_unset() {
        let effective = EffectiveSettings::resolve(
            &Settings::default(),
            &Settings::default(),
            &Overrides::default(),
            &[],
        );
        assert_eq!(effective.computer, ComputerConfig::default());
        assert_eq!(effective.computer.keep_recent_observations, 3);
    }

    #[test]
    fn a_screen_preset_resolves_to_a_real_viewport() {
        assert_eq!(parse_screen("mobile"), Some((390, 844)));
        assert_eq!(parse_screen("Desktop"), Some((1920, 1080)));
        assert_eq!(parse_screen("  tablet "), Some((820, 1180)));
    }

    #[test]
    fn an_explicit_size_still_works_and_beats_nothing_named() {
        assert_eq!(parse_screen("1280x800"), Some((1280, 800)));
        assert_eq!(parse_screen("1280X800"), Some((1280, 800)));
        // A preset name is checked first, so it is never read as a malformed
        // WIDTHxHEIGHT and silently discarded.
        assert!(parse_screen("laptop").is_some());
    }

    #[test]
    fn a_nonsense_screen_setting_falls_back_rather_than_failing_the_load() {
        // A typo in an optional display size must not stop the agent starting.
        assert_eq!(parse_screen("enormous"), None);
        assert_eq!(parse_screen("1920"), None);
        assert_eq!(parse_screen("axb"), None);
    }

    #[test]
    fn every_preset_is_a_size_the_stage_will_accept() {
        // The stage clamps to 320..=7680 by 240..=4320; a preset outside that
        // would silently become a different screen than the one named.
        for (name, (width, height)) in SCREEN_PRESETS {
            assert!(
                (320..=7680).contains(width) && (240..=4320).contains(height),
                "preset {name} is outside what the stage will allocate"
            );
        }
    }
}
