//! Markdown profile documents: YAML metadata plus a model-facing prompt body.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use gray_matter::{Matter, engine::YAML};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

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
    #[serde(default)]
    post_system: Option<String>,
    #[serde(default)]
    identity: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    provider_config: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    required_tools: Vec<String>,
    #[serde(default)]
    yield_schema: Option<Value>,
    #[serde(default)]
    allow_fork: bool,
    #[serde(default)]
    allow_handoff: bool,
}

/// A parsed profile document. The body is the profile's model-facing prompt;
/// the frontmatter compiles into the existing authorization predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileDocument {
    pub name: String,
    pub description: Option<String>,
    pub prompt: String,
    pub permissions: PermissionRegistry,
    pub post_system: Option<String>,
    pub identity: Option<String>,
    pub provider: Option<String>,
    pub provider_config: Option<String>,
    pub model: Option<String>,
    pub required_tools: Vec<String>,
    pub yield_schema: Option<Value>,
    pub allow_fork: bool,
    pub allow_handoff: bool,
}

/// Component-owned profile/prompt resolver. The daemon stores only the stable
/// profile identity selected for a session; loading and interpreting a profile
/// remains behind this socket.
#[async_trait]
pub trait ProfileComponent: Send + Sync {
    fn resource_id(&self) -> &str;
    async fn resolve(&self, profile_id: &str) -> anyhow::Result<ProfileDocument>;
}

/// Native filesystem-backed profile component used by the default host. It is
/// still selected through the profile socket; the daemon does not parse
/// Markdown or decide profile precedence itself.
pub struct FileProfileComponent {
    resource_id: String,
    global_root: PathBuf,
    local_root: PathBuf,
}

impl FileProfileComponent {
    pub fn new(
        resource_id: impl Into<String>,
        global_root: impl Into<PathBuf>,
        local_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            resource_id: resource_id.into(),
            global_root: global_root.into(),
            local_root: local_root.into(),
        }
    }
}

#[async_trait]
impl ProfileComponent for FileProfileComponent {
    fn resource_id(&self) -> &str {
        &self.resource_id
    }

    async fn resolve(&self, profile_id: &str) -> anyhow::Result<ProfileDocument> {
        ProfileDocument::load(&self.global_root, &self.local_root, profile_id)
    }
}

#[derive(Clone, Default)]
pub struct ProfileSocket {
    components: Arc<std::collections::BTreeMap<String, Arc<dyn ProfileComponent>>>,
}

impl ProfileSocket {
    pub fn new(components: impl IntoIterator<Item = Arc<dyn ProfileComponent>>) -> Self {
        let mut values = std::collections::BTreeMap::new();
        for component in components {
            values.insert(component.resource_id().to_owned(), component);
        }
        Self {
            components: Arc::new(values),
        }
    }

    pub fn selected(
        &self,
        resource: Option<&str>,
    ) -> anyhow::Result<Option<Arc<dyn ProfileComponent>>> {
        let Some(resource) = resource else {
            return Ok(None);
        };
        self.components
            .get(resource)
            .cloned()
            .map(Some)
            .ok_or_else(|| anyhow!("profile component {resource:?} is unavailable"))
    }

    pub fn resource_ids(&self) -> Vec<String> {
        self.components.keys().cloned().collect()
    }
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
            post_system: data.post_system,
            identity: data.identity,
            provider: data.provider,
            provider_config: data.provider_config,
            model: data.model,
            required_tools: data.required_tools,
            yield_schema: data.yield_schema,
            allow_fork: data.allow_fork,
            allow_handoff: data.allow_handoff,
        })
    }

    pub fn yield_schema(&self) -> Value {
        self.yield_schema.clone().unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "complete": {"type": "boolean"},
                    "remaining": {"type": "string"}
                },
                "required": ["complete"],
                "additionalProperties": false
            })
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
