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
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            items: default_items(),
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
    pub fn render(&self) -> Span<'_> {
        Span::styled(
            self.text.as_str(),
            Style::default().fg(Color::Black).bg(Color::Gray),
        )
    }
}

pub(crate) fn render(segments: &[StatusSegment]) -> Line<'_> {
    let mut spans = Vec::new();
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
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(project)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?;
    let branch = branch.trim();
    (!branch.is_empty()).then(|| branch.to_owned())
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
    fn formats_tokens_and_saturates_remaining() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(12_000), "12k");
        assert_eq!(format_tokens(2_000_000), "2m");
        assert_eq!(context_remaining(120, 100), 0);
    }

    #[test]
    fn rendered_segments_have_light_gray_background() {
        let segment = StatusSegment {
            item: StatusItem::Model,
            text: "gpt-5".into(),
        };
        assert_eq!(segment.render().style.bg, Some(Color::Gray));
        assert_eq!(render(&[segment]).spans.len(), 1);
    }
}
