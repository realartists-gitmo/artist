use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use artist_core::{PolicyDecision, ProfilePolicyRule, ProfileSnapshot, ToolControl, ToolEffect};
use async_trait::async_trait;
use globset::Glob;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::ResourceError;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub effects: Vec<ToolEffect>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationContext {
    pub correlation_id: Uuid,
    pub stack: Vec<String>,
    pub profile: Option<Arc<ProfileSnapshot>>,
}

impl InvocationContext {
    pub fn root() -> Self {
        Self {
            correlation_id: Uuid::new_v4(),
            stack: Vec::new(),
            profile: None,
        }
    }

    pub fn for_profile(profile: Arc<ProfileSnapshot>) -> Self {
        Self {
            profile: Some(profile),
            ..Self::root()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    pub value: Value,
    pub control: Option<ToolControl>,
}

impl ToolOutput {
    pub fn value(value: Value) -> Self {
        Self {
            value,
            control: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    Unknown(String),
    #[error("recursive tool invocation: {cycle}")]
    Recursive { cycle: String },
    #[error("invalid tool arguments: {0}")]
    Arguments(String),
    #[error(transparent)]
    Resource(#[from] ResourceError),
    #[error("tool failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    async fn call(
        &self,
        arguments: Value,
        context: InvocationContext,
    ) -> Result<ToolOutput, ToolError>;
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<RwLock<BTreeMap<String, RegisteredTool>>>,
}

#[derive(Clone)]
struct RegisteredTool {
    definition: ToolDefinition,
    handler: Arc<dyn ToolHandler>,
    owner: Option<String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        definition: ToolDefinition,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<(), ToolError> {
        self.register_inner(definition, handler, None)
    }

    pub fn register_owned(
        &self,
        definition: ToolDefinition,
        handler: Arc<dyn ToolHandler>,
        owner: impl Into<String>,
    ) -> Result<(), ToolError> {
        self.register_inner(definition, handler, Some(owner.into()))
    }

    fn register_inner(
        &self,
        definition: ToolDefinition,
        handler: Arc<dyn ToolHandler>,
        owner: Option<String>,
    ) -> Result<(), ToolError> {
        validate_schema(&definition.input_schema)?;
        let mut tools = self.tools.write().expect("tool registry lock poisoned");
        if tools.contains_key(&definition.name) {
            return Err(ToolError::Failed(format!(
                "tool already registered: {}",
                definition.name
            )));
        }
        tools.insert(
            definition.name.clone(),
            RegisteredTool {
                definition,
                handler,
                owner,
            },
        );
        Ok(())
    }

    /// Replace every tool owned by one plugin as a single registry mutation.
    /// Candidate schemas and name conflicts are checked before active entries
    /// are touched, so a rejected plugin leaves the running registry intact.
    pub fn replace_owner(
        &self,
        owner: &str,
        replacements: Vec<(ToolDefinition, Arc<dyn ToolHandler>)>,
    ) -> Result<(), ToolError> {
        let mut names = std::collections::BTreeSet::new();
        for (definition, _) in &replacements {
            validate_schema(&definition.input_schema)?;
            if !names.insert(definition.name.clone()) {
                return Err(ToolError::Failed(format!(
                    "candidate defines tool more than once: {}",
                    definition.name
                )));
            }
        }
        let mut tools = self.tools.write().expect("tool registry lock poisoned");
        for (definition, _) in &replacements {
            if let Some(existing) = tools.get(&definition.name)
                && existing.owner.as_deref() != Some(owner)
            {
                return Err(ToolError::Failed(format!(
                    "tool already registered by another plugin: {}",
                    definition.name
                )));
            }
        }
        tools.retain(|_, tool| tool.owner.as_deref() != Some(owner));
        for (definition, handler) in replacements {
            tools.insert(
                definition.name.clone(),
                RegisteredTool {
                    definition,
                    handler,
                    owner: Some(owner.to_owned()),
                },
            );
        }
        Ok(())
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .read()
            .expect("tool registry lock poisoned")
            .values()
            .map(|t| t.definition.clone())
            .collect()
    }

    pub fn definitions_for(&self, profile: &ProfileSnapshot) -> Vec<ToolDefinition> {
        self.definitions()
            .into_iter()
            .filter_map(|mut definition| {
                policy_allows_tool(profile, &definition).then(|| {
                    if definition.name == "yield" {
                        definition.input_schema = profile.yield_schema.clone();
                    } else if definition.name == "handoff" {
                        definition.input_schema = handoff_schema(&profile.catalog);
                    }
                    definition
                })
            })
            .collect()
    }

    pub fn owner(&self, name: &str) -> Option<String> {
        self.tools
            .read()
            .expect("tool registry lock poisoned")
            .get(name)
            .and_then(|tool| tool.owner.clone())
    }

    pub fn definition(&self, name: &str) -> Option<ToolDefinition> {
        self.tools
            .read()
            .expect("tool registry lock poisoned")
            .get(name)
            .map(|tool| tool.definition.clone())
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Result<Value, ToolError> {
        self.call_output_with_context(name, arguments, InvocationContext::root())
            .await
            .map(|output| output.value)
    }

    pub async fn call_with_context(
        &self,
        name: &str,
        arguments: Value,
        context: InvocationContext,
    ) -> Result<Value, ToolError> {
        self.call_output_with_context(name, arguments, context)
            .await
            .map(|output| output.value)
    }

    pub async fn call_output_with_context(
        &self,
        name: &str,
        arguments: Value,
        mut context: InvocationContext,
    ) -> Result<ToolOutput, ToolError> {
        if let Some(start) = context.stack.iter().position(|entry| entry == name) {
            let mut cycle = context.stack[start..].to_vec();
            cycle.push(name.to_owned());
            return Err(ToolError::Recursive {
                cycle: cycle.join(" -> "),
            });
        }
        let registered = self
            .tools
            .read()
            .expect("tool registry lock poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::Unknown(name.into()))?;
        if context
            .profile
            .as_deref()
            .is_some_and(|profile| !policy_allows_tool(profile, &registered.definition))
        {
            return Err(ToolError::Failed(format!(
                "profile policy denies tool `{name}`"
            )));
        }
        let input_schema = match (name, context.profile.as_deref()) {
            ("yield", Some(profile)) => &profile.yield_schema,
            ("handoff", Some(profile)) => {
                let schema = handoff_schema(&profile.catalog);
                validate_arguments(&schema, &arguments)?;
                context.stack.push(name.into());
                return registered.handler.call(arguments, context).await;
            }
            _ => &registered.definition.input_schema,
        };
        validate_arguments(input_schema, &arguments)?;
        context.stack.push(name.into());
        registered.handler.call(arguments, context).await
    }
}

pub fn validate_schema(schema: &Value) -> Result<(), ToolError> {
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|error| ToolError::Arguments(format!("invalid JSON Schema: {error}")))
}

fn validate_arguments(schema: &Value, arguments: &Value) -> Result<(), ToolError> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| ToolError::Arguments(format!("invalid JSON Schema: {error}")))?;
    validator
        .validate(arguments)
        .map_err(|error| ToolError::Arguments(error.to_string()))
}

pub fn policy_allows_tool(profile: &ProfileSnapshot, tool: &ToolDefinition) -> bool {
    evaluate_policy(profile, tool, None, None)
}

pub fn policy_allows_resource(
    profile: &ProfileSnapshot,
    tool: &ToolDefinition,
    operation: &str,
    resource: &str,
) -> bool {
    evaluate_policy(profile, tool, Some(operation), Some(resource))
}

fn evaluate_policy(
    profile: &ProfileSnapshot,
    tool: &ToolDefinition,
    operation: Option<&str>,
    resource: Option<&str>,
) -> bool {
    let mut decision = profile.policy.default;
    for rule in &profile.policy.rules {
        if rule_matches(rule, tool, operation, resource) {
            decision = rule.decision;
        }
    }
    decision == PolicyDecision::Allow
}

fn rule_matches(
    rule: &ProfilePolicyRule,
    tool: &ToolDefinition,
    operation: Option<&str>,
    resource: Option<&str>,
) -> bool {
    matches_any(&rule.tools, &tool.name)
        && (rule.effects.is_empty()
            || rule
                .effects
                .iter()
                .any(|effect| tool.effects.contains(effect)))
        && match_optional(&rule.operations, operation)
        && match_optional_globs(&rule.resources, resource)
}

fn matches_any(patterns: &[String], value: &str) -> bool {
    patterns.is_empty() || patterns.iter().any(|pattern| glob_matches(pattern, value))
}

fn match_optional(patterns: &[String], value: Option<&str>) -> bool {
    patterns.is_empty()
        || value.is_some_and(|value| patterns.iter().any(|pattern| glob_matches(pattern, value)))
}

fn match_optional_globs(patterns: &[String], value: Option<&str>) -> bool {
    match_optional(patterns, value)
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    Glob::new(pattern)
        .map(|glob| glob.compile_matcher().is_match(value))
        .unwrap_or(false)
}

fn handoff_schema(catalog: &[String]) -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["profile", "brief"],
        "properties": {
            "profile": {"type": "string", "enum": catalog},
            "brief": {"type": "string"}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_core::{ModelRoute, ProfilePolicy, ProfileSnapshot};
    use serde_json::json;
    use std::sync::Mutex;

    struct Nested {
        registry: ToolRegistry,
        next: &'static str,
    }
    #[async_trait]
    impl ToolHandler for Nested {
        async fn call(&self, _: Value, ctx: InvocationContext) -> Result<ToolOutput, ToolError> {
            self.registry
                .call_output_with_context(self.next, json!({}), ctx)
                .await
        }
    }

    #[tokio::test]
    async fn rejects_indirect_recursion_and_preserves_correlation() {
        let registry = ToolRegistry::new();
        registry
            .register(
                ToolDefinition {
                    name: "a".into(),
                    description: "".into(),
                    input_schema: json!({}),
                    effects: vec![ToolEffect::Unknown],
                },
                Arc::new(Nested {
                    registry: registry.clone(),
                    next: "b",
                }),
            )
            .unwrap();
        registry
            .register(
                ToolDefinition {
                    name: "b".into(),
                    description: "".into(),
                    input_schema: json!({}),
                    effects: vec![ToolEffect::Unknown],
                },
                Arc::new(Nested {
                    registry: registry.clone(),
                    next: "a",
                }),
            )
            .unwrap();
        assert!(
            matches!(registry.call("a", json!({})).await, Err(ToolError::Recursive { cycle }) if cycle == "a -> b -> a")
        );
    }

    struct Probe {
        registry: ToolRegistry,
        next: Option<&'static str>,
        seen: Arc<Mutex<Vec<InvocationContext>>>,
    }
    #[async_trait]
    impl ToolHandler for Probe {
        async fn call(&self, _: Value, ctx: InvocationContext) -> Result<ToolOutput, ToolError> {
            self.seen.lock().unwrap().push(ctx.clone());
            if let Some(next) = self.next {
                self.registry
                    .call_output_with_context(next, json!({}), ctx)
                    .await
            } else {
                Ok(ToolOutput::value(json!({"ok": true})))
            }
        }
    }

    #[tokio::test]
    async fn nested_calls_share_one_correlation_id_and_grow_the_stack() {
        let registry = ToolRegistry::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        for (name, next) in [("outer", Some("inner")), ("inner", None)] {
            registry
                .register(
                    ToolDefinition {
                        name: name.into(),
                        description: String::new(),
                        input_schema: json!({}),
                        effects: vec![ToolEffect::Unknown],
                    },
                    Arc::new(Probe {
                        registry: registry.clone(),
                        next,
                        seen: seen.clone(),
                    }),
                )
                .unwrap();
        }
        assert_eq!(
            registry.call("outer", json!({})).await.unwrap(),
            json!({"ok": true})
        );
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].correlation_id, seen[1].correlation_id);
        assert_eq!(seen[0].stack, ["outer"]);
        assert_eq!(seen[1].stack, ["outer", "inner"]);
    }

    fn profile(policy: ProfilePolicy, yield_schema: Value) -> ProfileSnapshot {
        ProfileSnapshot {
            name: "planner".into(),
            instructions: "plan".into(),
            yield_schema,
            policy,
            models: Vec::<ModelRoute>::new(),
            catalog: vec!["planner".into(), "worker".into()],
        }
    }

    #[test]
    fn policy_supports_effect_traits_and_ordered_resource_exceptions() {
        let read = ToolDefinition {
            name: "read".into(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            effects: vec![ToolEffect::Observe],
        };
        let write = ToolDefinition {
            name: "write".into(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            effects: vec![ToolEffect::Mutate],
        };
        let policy = ProfilePolicy {
            default: PolicyDecision::Deny,
            rules: vec![
                ProfilePolicyRule {
                    decision: PolicyDecision::Allow,
                    tools: Vec::new(),
                    effects: vec![ToolEffect::Observe],
                    operations: Vec::new(),
                    resources: Vec::new(),
                },
                ProfilePolicyRule {
                    decision: PolicyDecision::Deny,
                    tools: vec!["read".into()],
                    effects: Vec::new(),
                    operations: vec!["read".into()],
                    resources: vec!["file:///secret/**".into()],
                },
                ProfilePolicyRule {
                    decision: PolicyDecision::Allow,
                    tools: vec!["read".into()],
                    effects: Vec::new(),
                    operations: vec!["read".into()],
                    resources: vec!["file:///secret/public/**".into()],
                },
            ],
        };
        let profile = profile(policy, artist_core::default_yield_schema());
        assert!(policy_allows_tool(&profile, &read));
        assert!(!policy_allows_tool(&profile, &write));
        assert!(!policy_allows_resource(
            &profile,
            &read,
            "read",
            "file:///secret/private/key"
        ));
        assert!(policy_allows_resource(
            &profile,
            &read,
            "read",
            "file:///secret/public/guide"
        ));
    }

    #[tokio::test]
    async fn profile_replaces_the_yield_schema_at_advertisement_and_execution() {
        let registry = ToolRegistry::new();
        registry
            .register(
                ToolDefinition {
                    name: "yield".into(),
                    description: String::new(),
                    input_schema: artist_core::default_yield_schema(),
                    effects: vec![ToolEffect::SessionControl],
                },
                Arc::new(Probe {
                    registry: registry.clone(),
                    next: None,
                    seen: Arc::new(Mutex::new(Vec::new())),
                }),
            )
            .unwrap();
        let custom = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["status"],
            "properties": {"status": {"enum": ["ready"]}}
        });
        let profile = Arc::new(profile(ProfilePolicy::default(), custom.clone()));
        assert_eq!(registry.definitions_for(&profile)[0].input_schema, custom);
        assert!(
            registry
                .call_output_with_context(
                    "yield",
                    json!({"status": "ready"}),
                    InvocationContext::for_profile(profile.clone()),
                )
                .await
                .is_ok()
        );
        assert!(matches!(
            registry
                .call_output_with_context(
                    "yield",
                    json!({"completed": true}),
                    InvocationContext::for_profile(profile),
                )
                .await,
            Err(ToolError::Arguments(_))
        ));
    }
}
