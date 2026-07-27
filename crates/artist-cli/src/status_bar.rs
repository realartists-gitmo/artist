use llm_provider::SavedProvider;
use serde::{Deserialize, Serialize};
use std::path::Path;

mod icons;
mod row;
mod view;

pub(crate) use view::{HEIGHT, StatusView, view};

/// Values that may be displayed in the status bar, in configured order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatusItem {
    ProjectDirectory,
    GitBranch,
    Model,
    Reasoning,
    Context,
    SessionTokens,
}

impl StatusItem {
    pub const ALL: [Self; 6] = [
        Self::ProjectDirectory,
        Self::GitBranch,
        Self::Model,
        Self::Reasoning,
        Self::Context,
        Self::SessionTokens,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ProjectDirectory => "Project directory",
            Self::GitBranch => "Git branch",
            Self::Model => "Model",
            Self::Reasoning => "Reasoning",
            Self::Context => "Context remaining / capacity",
            Self::SessionTokens => "Session tokens",
        }
    }
}

fn default_items() -> Vec<StatusItem> {
    vec![
        StatusItem::ProjectDirectory,
        StatusItem::GitBranch,
        StatusItem::Model,
        StatusItem::Reasoning,
        StatusItem::Context,
        StatusItem::SessionTokens,
    ]
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StatusBarConfig {
    #[serde(default = "default_items")]
    pub items: Vec<StatusItem>,
    #[serde(default)]
    pub extension_items: Vec<String>,
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            items: default_items(),
            extension_items: Vec::new(),
        }
    }
}

/// A status value ready for presentation by the terminal UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StatusSegment {
    pub item: StatusItem,
    pub text: String,
    pub compact: Option<String>,
    pub palette_index: usize,
}

#[allow(clippy::too_many_arguments)] // display params accrete; a struct refactor is follow-up
pub(crate) fn segments(
    config: &StatusBarConfig,
    project: &Path,
    provider: &SavedProvider,
    git_branch: Option<&str>,
    used_tokens: Option<u64>,
    context_capacity: Option<u64>,
    session_tokens: u64,
    extension_values: &[(String, String)],
) -> Vec<StatusSegment> {
    let mut segments = config
        .items
        .iter()
        .enumerate()
        .filter_map(|(palette_index, item)| {
            let (text, compact) = match item {
                StatusItem::ProjectDirectory => (
                    format!(
                        "{} {}",
                        icons::PROJECT,
                        project
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or_else(|| project.to_str().unwrap_or("—"))
                    ),
                    None,
                ),
                StatusItem::GitBranch => (format!("{} {}", icons::BRANCH, git_branch?), None),
                StatusItem::Model => (
                    format!("{} {}", icons::MODEL, provider.model.as_deref()?),
                    None,
                ),
                StatusItem::Reasoning => (
                    format!(
                        "{} {}",
                        icons::REASONING,
                        provider.reasoning_effort.as_deref()?
                    ),
                    None,
                ),
                StatusItem::Context => {
                    let capacity = context_capacity.filter(|capacity| *capacity > 0);
                    let percent = capacity
                        .map(|capacity| {
                            context_remaining(used_tokens.unwrap_or(0), capacity)
                                .saturating_mul(100)
                                / capacity
                        })
                        .unwrap_or(100);
                    let capacity = capacity
                        .map(|capacity| format!(" · {}", format_tokens(capacity)))
                        .unwrap_or_default();
                    (
                        format!(
                            "{} ctx {} {percent}%{capacity}",
                            icons::CONTEXT,
                            context_gauge(percent)
                        ),
                        Some(format!("{} ctx {percent}%", icons::CONTEXT)),
                    )
                }
                StatusItem::SessionTokens => (
                    format!(
                        "{} {} total",
                        icons::SESSION_TOKENS,
                        format_tokens(session_tokens)
                    ),
                    None,
                ),
            };
            Some(StatusSegment {
                item: *item,
                text,
                compact,
                palette_index,
            })
        })
        .collect::<Vec<_>>();
    segments.extend(config.extension_items.iter().enumerate().filter_map(
        |(extension_index, name)| {
            Some(StatusSegment {
                item: StatusItem::Model,
                text: extension_values
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone())?,
                compact: None,
                palette_index: config.items.len() + extension_index,
            })
        },
    ));
    segments
}

fn context_gauge(percent: u64) -> String {
    const CELLS: usize = 8;
    let filled = (percent.min(100) as usize * CELLS + 50) / 100;
    format!("{}{}", "█".repeat(filled), "░".repeat(CELLS - filled))
}

/// Finds the branch checked out by the repository containing `project`.
pub(crate) fn git_branch(project: &Path) -> Option<String> {
    // Unlike `rev-parse --abbrev-ref HEAD`, symbolic-ref also works before the
    // repository has its first commit.
    git_output(project, &["symbolic-ref", "--quiet", "--short", "HEAD"]).or_else(|| {
        git_output(project, &["rev-parse", "--short", "HEAD"])
            .map(|commit| format!("detached@{commit}"))
    })
}

fn git_output(project: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(project)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub(crate) fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    if tokens < 1_000_000 {
        let value = tokens as f64 / 1_000.0;
        return format_compact(value, "k");
    }
    format_compact(tokens as f64 / 1_000_000.0, "m")
}

fn format_compact(value: f64, suffix: &str) -> String {
    if value >= 10.0 || value.fract() < 0.05 {
        format!("{value:.0}{suffix}")
    } else {
        format!("{value:.1}{suffix}")
    }
}

pub(crate) fn context_remaining(used: u64, capacity: u64) -> u64 {
    capacity.saturating_sub(used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_all_items_in_order() {
        assert_eq!(StatusBarConfig::default().items, default_items());
    }

    #[test]
    fn config_uses_stable_snake_case_names() {
        let config: StatusBarConfig =
            toml::from_str("items = ['model', 'git_branch', 'session_tokens']").unwrap();
        assert_eq!(
            config.items,
            [
                StatusItem::Model,
                StatusItem::GitBranch,
                StatusItem::SessionTokens
            ]
        );
    }

    #[test]
    fn detects_branch_before_first_commit() {
        let project = tempfile::tempdir().unwrap();
        let output = std::process::Command::new("git")
            .args(["init", "--quiet", "--initial-branch", "other"])
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(git_branch(project.path()).as_deref(), Some("other"));
    }

    #[test]
    fn formats_tokens_and_saturates_remaining() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(12_000), "12k");
        assert_eq!(format_tokens(2_000_000), "2m");
        assert_eq!(context_remaining(120, 100), 0);
    }

    #[test]
    fn builds_default_runtime_values_and_context_percentage() {
        let provider: SavedProvider = serde_json::from_value(serde_json::json!({
            "id":"x", "name":"x", "base_url":"https://example.com/",
            "model":"gpt-test", "reasoning_effort":"high",
            "auth":{"access_token":"token","refresh_token":"refresh","account_id":"account"}
        }))
        .unwrap();
        let segments = segments(
            &StatusBarConfig::default(),
            Path::new("/tmp/project"),
            &provider,
            Some("main"),
            Some(25),
            Some(100),
            0,
            &[],
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>(),
            [
                " project",
                " main",
                " gpt-test",
                " high",
                " ctx ██████░░ 75% · 100",
                " 0 total",
            ]
        );
        assert_eq!(segments[4].compact.as_deref(), Some(" ctx 75%"));
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.palette_index)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn displays_full_unused_context_before_provider_metadata_arrives() {
        let provider: SavedProvider = serde_json::from_value(serde_json::json!({
            "id":"x", "name":"x", "base_url":"https://example.com/", "model":"m",
            "auth":{"access_token":"t","refresh_token":"r","account_id":"a"}
        }))
        .unwrap();
        let render = |capacity| {
            segments(
                &StatusBarConfig {
                    items: vec![StatusItem::Context, StatusItem::SessionTokens],
                    extension_items: Vec::new(),
                },
                Path::new("."),
                &provider,
                None,
                None,
                capacity,
                0,
                &[],
            )
        };

        let unknown = render(None);
        let context = unknown
            .iter()
            .find(|segment| segment.item == StatusItem::Context)
            .unwrap();
        let session = unknown
            .iter()
            .find(|segment| segment.item == StatusItem::SessionTokens)
            .unwrap();
        assert_eq!(context.text, " ctx ████████ 100%");
        assert_eq!(context.compact.as_deref(), Some(" ctx 100%"));
        assert_eq!(session.text, " 0 total");

        let known = render(Some(100));
        let context = known
            .iter()
            .find(|segment| segment.item == StatusItem::Context)
            .unwrap();
        assert_eq!(context.text, " ctx ████████ 100% · 100");
    }

    #[test]
    fn context_and_session_tokens_can_be_enabled_independently() {
        let provider: SavedProvider = serde_json::from_value(serde_json::json!({
            "id":"x", "name":"x", "base_url":"https://example.com/", "model":"m",
            "auth":{"access_token":"t","refresh_token":"r","account_id":"a"}
        }))
        .unwrap();
        let render = |items| {
            segments(
                &StatusBarConfig {
                    items,
                    extension_items: Vec::new(),
                },
                Path::new("."),
                &provider,
                None,
                Some(25),
                Some(100),
                1_500,
                &[],
            )
        };

        assert_eq!(
            render(vec![StatusItem::Context])[0].text,
            " ctx ██████░░ 75% · 100"
        );
        assert_eq!(
            render(vec![StatusItem::SessionTokens])[0].text,
            " 1.5k total"
        );
    }

    #[test]
    fn appends_configured_cached_extension_values() {
        let provider: SavedProvider = serde_json::from_value(serde_json::json!({
            "id":"x", "name":"x", "base_url":"https://example.com/", "model":"m",
            "auth":{"access_token":"t","refresh_token":"r","account_id":"a"}
        }))
        .unwrap();
        let config = StatusBarConfig {
            items: vec![],
            extension_items: vec!["quota".into()],
        };
        let values = vec![("quota".into(), "42%".into())];
        let segment = &segments(
            &config,
            Path::new("."),
            &provider,
            None,
            None,
            None,
            0,
            &values,
        )[0];
        assert_eq!(segment.text, "42%");
        assert_eq!(segment.item, StatusItem::Model);
        assert_eq!(segment.palette_index, 0);
    }
}
