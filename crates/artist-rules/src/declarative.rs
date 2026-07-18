//! Parsing of declarative rule files: markdown with YAML frontmatter, the
//! same family as `SKILL.md`. The body is the reminder text.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::types::{
    DEFAULT_WINDOW, DeclarativeRule, FirePolicy, MatchTarget, Persistence, RuleId, RuleScope,
};

pub const RULE_CAP: u64 = 256 * 1024;
/// Reject pathological patterns at load (the regex crate is linear-time, but
/// compiled program size is still bounded per rule).
pub const REGEX_SIZE_LIMIT: usize = 1 << 20;
pub const MAX_PATTERNS: usize = 32;

#[derive(Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    targets: Option<Vec<MatchTarget>>,
    patterns: Option<Vec<String>>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    window: Option<usize>,
    #[serde(default)]
    fire: Option<FirePolicy>,
    #[serde(default)]
    persistence: Option<Persistence>,
    #[serde(default)]
    scope: Option<Vec<String>>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(flatten)]
    _other: BTreeMap<String, serde_yaml::Value>,
}

/// Parse one rule file. Errors are returned as strings destined for the
/// diagnostics list — an invalid rule never fails discovery.
pub fn parse(file: &Path) -> Result<DeclarativeRule, String> {
    let inner = || -> anyhow::Result<DeclarativeRule> {
        let metadata = std::fs::metadata(file)?;
        anyhow::ensure!(metadata.len() <= RULE_CAP, "exceeds {RULE_CAP} bytes");
        let text = std::fs::read_to_string(file)?;
        let (yaml, body) = frontmatter(&text)?;
        parse_parts(yaml, body, Some(file.to_owned()))
    };
    inner().map_err(|error| format!("{}: {error}", file.display()))
}

/// Parse from already-split frontmatter + body (used by built-in rules and
/// `/rules dry-run` on unsaved buffers).
pub fn parse_parts(
    yaml: &str,
    body: &str,
    source: Option<std::path::PathBuf>,
) -> anyhow::Result<DeclarativeRule> {
    let parsed: Frontmatter = serde_yaml::from_str(yaml)?;
    let name = parsed
        .name
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing name"))?;
    let description = parsed
        .description
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing description"))?;
    let patterns = parsed
        .patterns
        .filter(|patterns| !patterns.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing patterns"))?;
    anyhow::ensure!(
        patterns.len() <= MAX_PATTERNS,
        "more than {MAX_PATTERNS} patterns"
    );
    for pattern in &patterns {
        regex::RegexBuilder::new(pattern)
            .size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|error| anyhow::anyhow!("pattern `{pattern}`: {error}"))?;
    }
    let reminder = body.trim();
    anyhow::ensure!(!reminder.is_empty(), "empty reminder body");
    let scope = match parsed.scope {
        None => RuleScope::default(),
        Some(entries) => {
            let mut scope = RuleScope {
                main: false,
                delegate: false,
            };
            for entry in &entries {
                match entry.as_str() {
                    "main" => scope.main = true,
                    "delegate" => scope.delegate = true,
                    other => anyhow::bail!("unknown scope `{other}`"),
                }
            }
            anyhow::ensure!(scope.main || scope.delegate, "empty scope");
            scope
        }
    };
    Ok(DeclarativeRule {
        id: RuleId(name),
        description,
        targets: parsed
            .targets
            .unwrap_or_else(|| vec![MatchTarget::AssistantText]),
        patterns,
        tools: parsed.tools.unwrap_or_default(),
        window: parsed.window.unwrap_or(DEFAULT_WINDOW).clamp(256, 1 << 20),
        fire: parsed.fire.unwrap_or_default(),
        persistence: parsed.persistence.unwrap_or_default(),
        scope,
        enabled: parsed.enabled.unwrap_or(true),
        reminder: reminder.to_owned(),
        source,
    })
}

/// Split `---` YAML frontmatter from the markdown body (same grammar as
/// skills — see `artist-agent/src/resources/skills.rs`).
pub fn frontmatter(text: &str) -> anyhow::Result<(&str, &str)> {
    let (rest, delimiter) = if let Some(rest) = text.strip_prefix("---\n") {
        (rest, "\n---")
    } else if let Some(rest) = text.strip_prefix("---\r\n") {
        (rest, "\r\n---")
    } else {
        anyhow::bail!("missing YAML frontmatter")
    };
    let split = rest
        .find(delimiter)
        .ok_or_else(|| anyhow::anyhow!("unterminated YAML frontmatter"))?;
    let tail = &rest[split + delimiter.len()..];
    let body = tail
        .strip_prefix("\r\n")
        .or_else(|| tail.strip_prefix('\n'))
        .unwrap_or(tail);
    Ok((&rest[..split], body))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"---
name: no-mock-data
description: Stop inventing mock data
targets: [assistant-text, tool-args]
patterns:
  - '(?i)\bmock(ed)?\s+data\b'
tools: [write, edit]
window: 8192
fire: per-turn
persistence: message
scope: [main]
---
Do not invent mock data.
"#;

    #[test]
    fn full_frontmatter_parses() {
        let (yaml, body) = frontmatter(FULL).unwrap();
        let rule = parse_parts(yaml, body, None).unwrap();
        assert_eq!(rule.id, RuleId("no-mock-data".into()));
        assert_eq!(
            rule.targets,
            vec![MatchTarget::AssistantText, MatchTarget::ToolArgs]
        );
        assert_eq!(rule.tools, vec!["write", "edit"]);
        assert_eq!(rule.window, 8192);
        assert_eq!(rule.fire, FirePolicy::PerTurn);
        assert_eq!(rule.persistence, Persistence::Message);
        assert!(rule.scope.main && !rule.scope.delegate);
        assert!(rule.enabled);
        assert_eq!(rule.reminder, "Do not invent mock data.");
    }

    #[test]
    fn defaults_apply() {
        let rule =
            parse_parts("name: x\ndescription: d\npatterns: ['foo']", "remind", None).unwrap();
        assert_eq!(rule.targets, vec![MatchTarget::AssistantText]);
        assert_eq!(rule.window, DEFAULT_WINDOW);
        assert_eq!(rule.fire, FirePolicy::Once);
        assert_eq!(rule.persistence, Persistence::Session);
        assert!(rule.scope.main && rule.scope.delegate);
    }

    #[test]
    fn invalid_regex_is_rejected() {
        let error = parse_parts(
            "name: x\ndescription: d\npatterns: ['(unclosed']",
            "remind",
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("(unclosed"));
    }

    #[test]
    fn missing_patterns_and_empty_body_are_rejected() {
        assert!(parse_parts("name: x\ndescription: d", "remind", None).is_err());
        assert!(parse_parts("name: x\ndescription: d\npatterns: ['a']", "  ", None).is_err());
    }

    #[test]
    fn unknown_frontmatter_keys_are_tolerated() {
        let rule = parse_parts(
            "name: x\ndescription: d\npatterns: ['a']\nfuture_key: 1",
            "remind",
            None,
        )
        .unwrap();
        assert_eq!(rule.id, RuleId("x".into()));
    }
}
