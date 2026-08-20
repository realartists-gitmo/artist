//! Markdown profile documents: YAML metadata plus a model-facing prompt body.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use gray_matter::{Matter, engine::YAML};
use serde::Deserialize;

use crate::{PermissionEffect, PermissionRegistry, PermissionRule};

#[derive(Clone, Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PolicyMetadata {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProfileMetadata {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tools: PolicyMetadata,
    #[serde(default)]
    resources: PolicyMetadata,
}

/// A parsed profile document. The body is the profile's model-facing prompt;
/// the frontmatter compiles into the existing authorization predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileDocument {
    pub name: String,
    pub description: Option<String>,
    pub prompt: String,
    pub permissions: PermissionRegistry,
}

impl ProfileDocument {
    pub fn parse(expected_name: &str, markdown: &str) -> anyhow::Result<Self> {
        validate_profile_name(expected_name)?;
        let parsed = Matter::<YAML>::new().parse(markdown);
        let data = parsed
            .data
            .ok_or_else(|| anyhow!("profile {expected_name:?} is missing YAML frontmatter"))?
            .deserialize::<ProfileMetadata>()
            .context("deserialize profile frontmatter")?;
        if data.name != expected_name {
            return Err(anyhow!(
                "profile frontmatter name {:?} does not match directory name {:?}",
                data.name,
                expected_name
            ));
        }
        if parsed.content.trim().is_empty() {
            return Err(anyhow!(
                "profile {expected_name:?} has an empty Markdown prompt body"
            ));
        }
        Ok(Self {
            name: data.name,
            description: data.description,
            prompt: parsed.content.trim().to_owned(),
            permissions: compile_permissions(&data.tools, &data.resources),
        })
    }

    pub fn load(global_root: &Path, local_root: &Path, name: &str) -> anyhow::Result<Self> {
        validate_profile_name(name)?;
        let relative = PathBuf::from(name).join("PROFILE.md");
        let local = local_root.join(&relative);
        let global = global_root.join(&relative);
        let path = if local.is_file() { local } else { global };
        let markdown = fs::read_to_string(&path)
            .with_context(|| format!("read profile document {}", path.display()))?;
        Self::parse(name, &markdown)
    }
}

fn validate_profile_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(anyhow!("invalid profile name {name:?}"));
    }
    Ok(())
}

fn compile_permissions(tools: &PolicyMetadata, resources: &PolicyMetadata) -> PermissionRegistry {
    let mut rules = Vec::new();
    if !tools.allow.is_empty() {
        rules.push(PermissionRule {
            effect: PermissionEffect::Deny,
            verb: None,
            pattern: None,
        });
        rules.extend(tools.allow.iter().map(|verb| PermissionRule {
            effect: PermissionEffect::Allow,
            verb: Some(verb.clone()),
            pattern: None,
        }));
    }
    rules.extend(tools.deny.iter().map(|verb| PermissionRule {
        effect: PermissionEffect::Deny,
        verb: Some(verb.clone()),
        pattern: None,
    }));
    if !resources.allow.is_empty() {
        rules.push(PermissionRule {
            effect: PermissionEffect::Deny,
            verb: None,
            pattern: Some("*".into()),
        });
        rules.extend(resources.allow.iter().map(|pattern| PermissionRule {
            effect: PermissionEffect::Allow,
            verb: None,
            pattern: Some(pattern.clone()),
        }));
    }
    rules.extend(resources.deny.iter().map(|pattern| PermissionRule {
        effect: PermissionEffect::Deny,
        verb: None,
        pattern: Some(pattern.clone()),
    }));
    PermissionRegistry::new(rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const REVIEWER: &str = r#"---
name: reviewer
description: Review changes.
tools:
  allow: [read, find, grep]
  deny: [write, move, edit]
resources:
  deny:
    - "agent://*"
    - "profile://{current-profile}/.artist/**"
---
Review the change and report concrete findings.
"#;

    #[test]
    fn parses_prompt_and_compiles_tool_and_resource_policy() {
        let profile = ProfileDocument::parse("reviewer", REVIEWER).unwrap();
        assert_eq!(
            profile.prompt,
            "Review the change and report concrete findings."
        );
        assert!(
            profile
                .permissions
                .authorize("reviewer", "read", "file:///src/lib.rs")
        );
        assert!(
            !profile
                .permissions
                .authorize("reviewer", "write", "file:///src/lib.rs")
        );
        assert!(
            !profile
                .permissions
                .authorize("reviewer", "read", "agent://worker/stdin")
        );
        assert!(!profile.permissions.authorize(
            "reviewer",
            "read",
            "profile://reviewer/.artist/tools/x"
        ));
    }

    #[test]
    fn local_profile_document_replaces_global_document() {
        let root = tempdir().unwrap();
        let global = root.path().join("global/reviewer");
        let local = root.path().join("local/reviewer");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::write(global.join("PROFILE.md"), REVIEWER).unwrap();
        fs::write(
            local.join("PROFILE.md"),
            REVIEWER.replace("Review the change", "Review locally"),
        )
        .unwrap();
        let profile = ProfileDocument::load(
            &root.path().join("global"),
            &root.path().join("local"),
            "reviewer",
        )
        .unwrap();
        assert_eq!(
            profile.prompt,
            "Review locally and report concrete findings."
        );
    }
}
