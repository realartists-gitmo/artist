use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ContextRole {
    System,
    Agents,
    Profile,
    Identity,
    #[default]
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolEffect {
    Observe,
    Mutate,
    Execute,
    SessionControl,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyDecision {
    #[default]
    Allow,
    Deny,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePolicy {
    #[serde(default)]
    pub default: PolicyDecision,
    #[serde(default)]
    pub rules: Vec<ProfilePolicyRule>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePolicyRule {
    pub decision: PolicyDecision,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub effects: Vec<ToolEffect>,
    #[serde(default)]
    pub operations: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRoute {
    pub provider: String,
    pub account: Option<String>,
    pub api_variant: Option<String>,
    pub model: String,
    pub reasoning: Option<String>,
    #[serde(default = "empty_object")]
    pub parameters: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileManifest {
    #[serde(default)]
    pub yield_schema: Option<Value>,
    #[serde(default)]
    pub policy: ProfilePolicy,
    #[serde(default)]
    pub models: Vec<ModelRoute>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProfileSnapshot {
    pub name: String,
    pub instructions: String,
    pub yield_schema: Value,
    pub policy: ProfilePolicy,
    pub models: Vec<ModelRoute>,
    pub catalog: Vec<String>,
}

impl ProfileSnapshot {
    pub fn from_manifest(
        name: String,
        instructions: String,
        manifest: ProfileManifest,
        mut catalog: Vec<String>,
    ) -> Result<Self, String> {
        validate_profile_name(&name)?;
        catalog.sort();
        catalog.dedup();
        for candidate in &catalog {
            validate_profile_name(candidate)?;
        }
        if !catalog.iter().any(|candidate| candidate == &name) {
            return Err(format!(
                "active profile `{name}` is absent from its catalog"
            ));
        }
        let yield_schema = manifest.yield_schema.unwrap_or_else(default_yield_schema);
        validate_object_schema(&yield_schema)?;
        if manifest.models.iter().any(|route| {
            route.provider.trim().is_empty()
                || route.model.trim().is_empty()
                || !route.parameters.is_object()
        }) {
            return Err(
                "model routes require non-empty provider/model names and object parameters".into(),
            );
        }
        Ok(Self {
            name,
            instructions,
            yield_schema,
            policy: manifest.policy,
            models: manifest.models,
            catalog,
        })
    }
}

pub fn default_yield_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["completed"],
        "properties": {
            "completed": {"type": "boolean"},
            "remainder": {"type": ["string", "null"]}
        }
    })
}

pub fn validate_profile_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!(
            "invalid profile name `{name}`; use ASCII letters, digits, `-`, or `_`"
        ));
    }
    Ok(())
}

fn validate_object_schema(schema: &Value) -> Result<(), String> {
    if !schema.is_object() {
        return Err("yield_schema must be a JSON Schema object".into());
    }
    Ok(())
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "control", rename_all = "kebab-case")]
pub enum ToolControl {
    Yield { payload: Value },
    Handoff { profile: String, brief: String },
}
