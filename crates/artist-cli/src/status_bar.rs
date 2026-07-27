use llm_provider::SavedProvider;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

const POWERLINE_SEPARATOR: &str = "";
const SEGMENT_COLORS: [Color; 6] = [
    Color::Rgb(255, 204, 225), // #ffcce1
    Color::Rgb(242, 241, 237), // #f2f1ed
    Color::Rgb(205, 229, 217), // #cde5d9
    Color::Rgb(242, 235, 204), // #f2ebcc
    Color::Rgb(198, 226, 231), // #c6e2e7
    Color::Rgb(247, 221, 232), // #f7dde8
];
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

    fn icon(self) -> &'static str {
        match self {
            Self::ProjectDirectory => "",
            Self::GitBranch => "",
            Self::Model => "",
            Self::Reasoning => "",
            Self::Context => "",
            Self::SessionTokens => "",
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
}

impl StatusSegment {
    fn content(&self) -> String {
        format!(" {} {} ", self.item.icon(), self.text)
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
                StatusItem::Context => match (used_tokens, context_capacity) {
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
                },
                StatusItem::SessionTokens => {
                    if session_tokens > 0 {
                        format!("{} total", format_tokens(session_tokens))
                    } else {
                        "— total".into()
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
    let mut spans = Vec::with_capacity(segments.len().saturating_mul(2));
    for (index, segment) in segments.iter().enumerate() {
        let color = SEGMENT_COLORS[index % SEGMENT_COLORS.len()];
        let next_color = segments
            .get(index + 1)
            .map(|_| SEGMENT_COLORS[(index + 1) % SEGMENT_COLORS.len()]);
        spans.push(Span::styled(
            segment.content(),
            Style::default().fg(Color::Black).bg(color),
        ));
        spans.push(Span::styled(
            POWERLINE_SEPARATOR,
            Style::default()
                .fg(color)
                .bg(next_color.unwrap_or(Color::Reset)),
        ));
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
        assert_eq!(segments[0].text, "project");
        assert_eq!(segments[1].text, "main");
        assert_eq!(segments[4].text, "75%/100");
        assert_eq!(segments[5].text, "— total");
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

        assert_eq!(render(vec![StatusItem::Context])[0].text, "75%/100");
        assert_eq!(
            render(vec![StatusItem::SessionTokens])[0].text,
            "1.5k total"
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
    fn renders_powerline_segments_with_icons_and_cycling_palette() {
        let segments = (0..7)
            .map(|index| StatusSegment {
                item: if index == 0 {
                    StatusItem::GitBranch
                } else {
                    StatusItem::Model
                },
                text: format!("value-{index}"),
            })
            .collect::<Vec<_>>();
        let rendered = render(&segments);

        assert_eq!(rendered.spans.len(), 14);
        assert_eq!(rendered.spans[0].content, "  value-0 ");
        assert_eq!(rendered.spans[0].style.fg, Some(Color::Black));
        assert_eq!(rendered.spans[0].style.bg, Some(SEGMENT_COLORS[0]));
        assert_eq!(rendered.spans[1].content, POWERLINE_SEPARATOR);
        assert_eq!(rendered.spans[1].style.fg, Some(SEGMENT_COLORS[0]));
        assert_eq!(rendered.spans[1].style.bg, Some(SEGMENT_COLORS[1]));
        assert_eq!(rendered.spans[12].style.bg, Some(SEGMENT_COLORS[0]));
        assert_eq!(rendered.spans[13].style.bg, Some(Color::Reset));
    }
}
