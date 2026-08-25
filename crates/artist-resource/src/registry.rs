use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use artist_core::{
    ContentPart, InvocationScope, PolicyDecision, ProfilePolicyRule, ProfileSnapshot, ToolControl,
    ToolEffect, ToolProgress,
};
use artist_kernel::{ExecutionExtensions, HookPhase, LifecycleHookDecision, LifecycleHookEvent};
use async_trait::async_trait;
use globset::Glob;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::ResourceError;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolAnnotations {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

impl ToolAnnotations {
    pub fn validate(&self, effects: &[ToolEffect]) -> Result<(), ToolError> {
        let mutating = effects.iter().any(|effect| {
            matches!(
                effect,
                ToolEffect::Mutate
                    | ToolEffect::Execute
                    | ToolEffect::SessionControl
                    | ToolEffect::Unknown
            )
        });
        if self.read_only == mutating || (self.destructive && self.read_only) {
            return Err(ToolError::Definition(
                "tool annotations contradict declared effects".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub category: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub effects: Vec<ToolEffect>,
    pub annotations: ToolAnnotations,
}

pub trait ToolProgressSink: Send + Sync + 'static {
    fn emit(&self, progress: ToolProgress);
}

#[derive(Clone)]
pub struct InvocationContext {
    pub correlation_id: Uuid,
    pub stack: Vec<String>,
    pub profile: Option<Arc<ProfileSnapshot>>,
    pub scope: Option<InvocationScope>,
    pub extensions: Option<Arc<dyn ExecutionExtensions>>,
    pub progress: Option<Arc<dyn ToolProgressSink>>,
}

impl InvocationContext {
    pub fn root() -> Self {
        Self {
            correlation_id: Uuid::new_v4(),
            stack: Vec::new(),
            profile: None,
            scope: None,
            extensions: None,
            progress: None,
        }
    }

    pub fn for_profile(profile: Arc<ProfileSnapshot>) -> Self {
        Self {
            profile: Some(profile),
            ..Self::root()
        }
    }

    pub fn for_execution(
        profile: Option<Arc<ProfileSnapshot>>,
        scope: InvocationScope,
        extensions: Arc<dyn ExecutionExtensions>,
        progress: Option<Arc<dyn ToolProgressSink>>,
    ) -> Self {
        Self {
            profile,
            scope: Some(scope),
            extensions: Some(extensions),
            progress,
            ..Self::root()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextActionHint {
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    pub value: Value,
    pub content: Vec<ContentPart>,
    pub control: Option<ToolControl>,
    pub next_actions: Vec<NextActionHint>,
}

impl ToolOutput {
    pub fn value(value: Value) -> Self {
        Self {
            content: vec![ContentPart::Json {
                value: value.clone(),
            }],
            value,
            control: None,
            next_actions: Vec::new(),
        }
    }

    pub fn content(value: Value, content: Vec<ContentPart>) -> Self {
        Self {
            value,
            content,
            control: None,
            next_actions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldViolation {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolFailure {
    pub code: String,
    pub message: String,
    pub retriable: bool,
    #[serde(default)]
    pub details: Value,
    #[serde(default)]
    pub violations: Vec<FieldViolation>,
    #[serde(default)]
    pub next_actions: Vec<NextActionHint>,
}

impl std::fmt::Display for ToolFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "tool failed ({}): {}", self.code, self.message)
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
    #[error("invalid tool definition: {0}")]
    Definition(String),
    #[error("{0}")]
    Failed(Box<ToolFailure>),
    #[error("tool output failed schema validation: {0}")]
    Output(String),
}

impl ToolError {
    pub fn failed(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Failed(Box::new(ToolFailure {
            code: code.into(),
            message: message.into(),
            retriable: false,
            details: Value::Null,
            violations: Vec::new(),
            next_actions: Vec::new(),
        }))
    }
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
        validate_definition(&definition)?;
        let mut tools = self.tools.write().expect("tool registry lock poisoned");
        if tools.contains_key(&definition.name) {
            return Err(ToolError::failed(
                "tool-name-conflict",
                format!("tool already registered: {}", definition.name),
            ));
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
            validate_definition(definition)?;
            if !names.insert(definition.name.clone()) {
                return Err(ToolError::failed(
                    "duplicate-candidate-tool",
                    format!("candidate defines tool more than once: {}", definition.name),
                ));
            }
        }
        let mut tools = self.tools.write().expect("tool registry lock poisoned");
        for (definition, _) in &replacements {
            if let Some(existing) = tools.get(&definition.name)
                && existing.owner.as_deref() != Some(owner)
            {
                return Err(ToolError::failed(
                    "tool-owner-conflict",
                    format!(
                        "tool already registered by another plugin: {}",
                        definition.name
                    ),
                ));
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

    /// Snapshot the live handler for `name`. Holding the returned `Arc`
    /// represents an in-flight invocation: later hot activations replace the
    /// registration but never disturb this instance.
    pub fn handler(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.tools
            .read()
            .expect("tool registry lock poisoned")
            .get(name)
            .map(|tool| Arc::clone(&tool.handler))
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
            return Err(ToolError::failed(
                "profile-policy-denied",
                format!("profile policy denies tool `{name}`"),
            ));
        }
        let mut arguments = arguments;
        if let (Some(scope), Some(extensions)) = (&context.scope, &context.extensions) {
            let decisions = extensions
                .hook(LifecycleHookEvent {
                    phase: HookPhase::BeforeToolExecution,
                    scope: scope.clone(),
                    payload: serde_json::json!({"tool": name, "arguments": arguments}),
                })
                .await
                .map_err(|error| ToolError::failed("before-tool-hook", error.to_string()))?;
            for decision in decisions {
                match decision {
                    LifecycleHookDecision::Proceed => {}
                    LifecycleHookDecision::Stop { reason } => {
                        return Err(ToolError::failed("tool-stopped-by-hook", reason));
                    }
                    LifecycleHookDecision::Rewrite { value } => {
                        let Some(rewritten) = value.get("arguments") else {
                            return Err(ToolError::failed(
                                "invalid-tool-hook-rewrite",
                                "before-tool rewrite requires an `arguments` field",
                            ));
                        };
                        arguments = rewritten.clone();
                    }
                }
            }
        }
        let input_schema = match (name, context.profile.as_deref()) {
            ("yield", Some(profile)) => &profile.yield_schema,
            ("handoff", Some(profile)) => {
                let schema = handoff_schema(&profile.catalog);
                validate_arguments(&schema, &arguments)?;
                context.stack.push(name.into());
                let mut output = registered.handler.call(arguments, context.clone()).await?;
                apply_after_tool_hooks(name, &context, &mut output).await?;
                validate_output(&registered.definition.output_schema, &output.value)?;
                return Ok(output);
            }
            _ => &registered.definition.input_schema,
        };
        validate_arguments(input_schema, &arguments)?;
        context.stack.push(name.into());
        let mut output = registered.handler.call(arguments, context.clone()).await?;
        apply_after_tool_hooks(name, &context, &mut output).await?;
        validate_output(&registered.definition.output_schema, &output.value)?;
        Ok(output)
    }
}

async fn apply_after_tool_hooks(
    name: &str,
    context: &InvocationContext,
    output: &mut ToolOutput,
) -> Result<(), ToolError> {
    let (Some(scope), Some(extensions)) = (&context.scope, &context.extensions) else {
        return Ok(());
    };
    let decisions = extensions
        .hook(LifecycleHookEvent {
            phase: HookPhase::AfterToolResult,
            scope: scope.clone(),
            payload: serde_json::json!({
                "tool": name,
                "value": output.value,
                "content": output.content,
                "next_actions": output.next_actions,
            }),
        })
        .await
        .map_err(|error| ToolError::failed("after-tool-hook", error.to_string()))?;
    for decision in decisions {
        match decision {
            LifecycleHookDecision::Proceed => {}
            LifecycleHookDecision::Stop { reason } => {
                return Err(ToolError::failed("tool-result-stopped-by-hook", reason));
            }
            LifecycleHookDecision::Rewrite { value } => {
                let Some(rewritten) = value.get("value") else {
                    return Err(ToolError::failed(
                        "invalid-tool-result-rewrite",
                        "after-tool rewrite requires a `value` field",
                    ));
                };
                output.value = rewritten.clone();
                output.content = vec![ContentPart::Json {
                    value: rewritten.clone(),
                }];
            }
        }
    }
    Ok(())
}

pub fn validate_definition(definition: &ToolDefinition) -> Result<(), ToolError> {
    if definition.name.is_empty()
        || definition.description.trim().is_empty()
        || definition.category.trim().is_empty()
    {
        return Err(ToolError::Definition(
            "tool name, description, and category must not be empty".into(),
        ));
    }
    validate_schema(&definition.input_schema)?;
    validate_schema(&definition.output_schema)?;
    definition.annotations.validate(&definition.effects)
}

pub fn validate_schema(schema: &Value) -> Result<(), ToolError> {
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|error| ToolError::Arguments(format!("invalid JSON Schema: {error}")))
}

fn validate_output(schema: &Value, output: &Value) -> Result<(), ToolError> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| ToolError::Definition(format!("invalid output JSON Schema: {error}")))?;
    validator
        .validate(output)
        .map_err(|error| ToolError::Output(error.to_string()))
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

    fn definition(name: &str, effects: Vec<ToolEffect>) -> ToolDefinition {
        let read_only = !effects.iter().any(|effect| {
            matches!(
                effect,
                ToolEffect::Mutate
                    | ToolEffect::Execute
                    | ToolEffect::SessionControl
                    | ToolEffect::Unknown
            )
        });
        ToolDefinition {
            name: name.into(),
            description: format!("{name} test tool"),
            category: "test".into(),
            input_schema: json!({}),
            output_schema: json!({}),
            effects,
            annotations: ToolAnnotations {
                read_only,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
        }
    }

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
                definition("a", vec![ToolEffect::Unknown]),
                Arc::new(Nested {
                    registry: registry.clone(),
                    next: "b",
                }),
            )
            .unwrap();
        registry
            .register(
                definition("b", vec![ToolEffect::Unknown]),
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
                    definition(name, vec![ToolEffect::Unknown]),
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
        let read = definition("read", vec![ToolEffect::Observe]);
        let write = definition("write", vec![ToolEffect::Mutate]);
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
                    input_schema: artist_core::default_yield_schema(),
                    ..definition("yield", vec![ToolEffect::SessionControl])
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

#[cfg(test)]
mod hot_swap_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Tagged {
        tag: &'static str,
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl ToolHandler for Tagged {
        async fn call(&self, _: Value, _: InvocationContext) -> Result<ToolOutput, ToolError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(ToolOutput::value(json!({"tag": self.tag})))
        }
    }

    fn probe_definition(description: &str) -> ToolDefinition {
        ToolDefinition {
            name: "probe".into(),
            description: description.into(),
            category: "test".into(),
            input_schema: json!({}),
            output_schema: json!({}),
            effects: vec![ToolEffect::Observe],
            annotations: ToolAnnotations {
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
        }
    }

    /// Hot activation must never disturb an in-flight invocation: callers
    /// hold the old handler `Arc` until their call completes, while new
    /// lookups immediately observe the replacement.
    #[tokio::test]
    async fn in_flight_calls_retain_the_old_revision_during_replacement() {
        let registry = ToolRegistry::new();
        let v1_calls = Arc::new(AtomicUsize::new(0));
        registry
            .register_owned(
                probe_definition("v1"),
                Arc::new(Tagged {
                    tag: "v1",
                    calls: v1_calls.clone(),
                }),
                "artist.probe",
            )
            .unwrap();

        // Simulate an in-flight invocation by snapshotting the registered
        // handler before activation swaps the registration underneath it.
        let in_flight = registry.handler("probe").expect("v1 handler");
        let ctx = InvocationContext::root();

        // Hot activation replaces the live registration with revision 2.
        registry
            .replace_owner(
                "artist.probe",
                vec![(
                    probe_definition("v2"),
                    Arc::new(Tagged {
                        tag: "v2",
                        calls: Arc::new(AtomicUsize::new(0)),
                    }) as Arc<dyn ToolHandler>,
                )],
            )
            .unwrap();

        // New calls see revision 2...
        let fresh = registry
            .call_output_with_context("probe", json!({}), ctx.clone())
            .await
            .unwrap();
        assert_eq!(fresh.value, json!({"tag": "v2"}));

        // ...while the in-flight caller still runs against revision 1.
        let old = in_flight.call(json!({}), ctx).await.unwrap();
        assert_eq!(old.value, json!({"tag": "v1"}));
        assert_eq!(v1_calls.load(Ordering::Acquire), 1);
    }
}
