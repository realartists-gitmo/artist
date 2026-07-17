use llm_provider::SavedProvider;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Values that may be displayed in the status bar, in configured order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatusItem {
    ProjectDirectory,
    GitBranch,
    Model,
    Reasoning,
    Context,
}

impl StatusItem {
    pub const ALL: [Self; 5] = [
        Self::ProjectDirectory,
        Self::GitBranch,
        Self::Model,
        Self::Reasoning,
        Self::Context,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ProjectDirectory => "Project directory",
            Self::GitBranch => "Git branch",
            Self::Model => "Model",
            Self::Reasoning => "Reasoning",
            Self::Context => "Context remaining / total",
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
}

impl StatusSegment {
    pub fn render(&self) -> Span<'static> {
        Span::styled(
            self.text.clone(),
            Style::default().fg(Color::Black).bg(Color::Gray),
        )
    }
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
        .map(|item| StatusSegment {
            item: *item,
            text: match item {
                StatusItem::ProjectDirectory => project
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_else(|| project.to_str().unwrap_or("—"))
                    .to_owned(),
                StatusItem::GitBranch => git_branch.unwrap_or("—").to_owned(),
                StatusItem::Model => provider.model.clone().unwrap_or_else(|| "—".into()),
                StatusItem::Reasoning => provider
                    .reasoning_effort
                    .clone()
                    .unwrap_or_else(|| "default".into()),
                StatusItem::Context => {
                    // Current-context view (last request), plus the session's
                    // cumulative billed volume so a long tool loop or TTSR
                    // retries aren't misread as context size.
                    let context = match (used_tokens, context_capacity) {
                        (Some(used), Some(capacity)) if capacity > 0 => {
                            let remaining = context_remaining(used, capacity);
                            format!(
                                "{}%/{}",
                                remaining.saturating_mul(100) / capacity,
                                format_tokens(capacity)
                            )
                        }
                        (None, Some(capacity)) => format!("—%/{}", format_tokens(capacity)),
                        _ => "—/—".into(),
                    };
                    if session_tokens > 0 {
                        format!("{context} · {} total", format_tokens(session_tokens))
                    } else {
                        context
                    }
                }
            },
        })
        .collect::<Vec<_>>();
    segments.extend(config.extension_items.iter().map(|name| {
        StatusSegment {
            item: StatusItem::Model,
            text: extension_values
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| "—".into()),
        }
    }));
    segments
}

pub(crate) fn render(segments: &[StatusSegment]) -> Line<'static> {
    let mut spans = if segments.is_empty() {
        Vec::new()
    } else {
        vec![Span::raw(" ")]
    };
    for (index, segment) in segments.iter().enumerate() {
        if index != 0 {
            spans.push(Span::styled(
                " | ",
                Style::default().fg(Color::DarkGray).bg(Color::Gray),
            ));
        }
        spans.push(segment.render());
    }
    Line::from(spans)
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
        let config: StatusBarConfig = toml::from_str("items = ['model', 'git_branch']").unwrap();
        assert_eq!(config.items, [StatusItem::Model, StatusItem::GitBranch]);
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
        assert_eq!(segments[0].text, "project");
        assert_eq!(segments[1].text, "main");
        assert_eq!(segments[4].text, "75%/100");
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
        assert_eq!(
            segments(
                &config,
                Path::new("."),
                &provider,
                None,
                None,
                None,
                0,
                &values
            )[0]
            .text,
            "42%"
        );
    }

    #[test]
    fn rendered_segments_have_light_gray_background() {
        let segment = StatusSegment {
            item: StatusItem::Model,
            text: "gpt-5".into(),
        };
        assert_eq!(segment.render().style.bg, Some(Color::Gray));
        let rendered = render(&[segment]);
        assert_eq!(rendered.spans.len(), 2);
        assert_eq!(rendered.spans[0].content, " ");
    }
}
