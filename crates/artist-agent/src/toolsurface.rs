//! Session-local model-facing tool state.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, RwLock};

use artist_component::PermissionRegistry;
use artist_component::ToolResultEnvelope;
use artist_component::{ComponentToolRegistry, CompositionUpdate, ToolError};
use artist_session::EventLog;
use llm_provider::{Message, Role, ToolDefinition};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolSurfaceEvent {
    Available {
        definition: ToolDefinition,
    },
    Unavailable {
        name: String,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HarnessPolicy {
    pub yield_schema: serde_json::Value,
    pub allow_fork: bool,
    pub allow_handoff: bool,
}

impl Default for HarnessPolicy {
    fn default() -> Self {
        Self {
            yield_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "complete": {"type": "boolean"},
                    "remaining": {"type": "string"}
                },
                "required": ["complete"],
                "additionalProperties": false
            }),
            allow_fork: false,
            allow_handoff: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolSurfaceError {
    Duplicate(String),
    Unknown(String),
    Execution(ToolError),
    Log(String),
}

impl std::fmt::Display for ToolSurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(name) => write!(f, "tool already exists in surface: {name}"),
            Self::Unknown(name) => write!(f, "tool is not registered for execution: {name}"),
            Self::Execution(error) => error.fmt(f),
            Self::Log(error) => write!(f, "could not persist tool-surface event: {error}"),
        }
    }
}

impl std::error::Error for ToolSurfaceError {}

#[derive(Clone)]
pub struct ToolSurface {
    registry: ComponentToolRegistry,
    static_definitions: Arc<RwLock<BTreeMap<String, ToolDefinition>>>,
    dynamic_definitions: Arc<RwLock<BTreeMap<String, ToolDefinition>>>,
    disabled: Arc<RwLock<BTreeSet<String>>>,
    pending_messages: Arc<Mutex<Vec<Message>>>,
    log: Option<Arc<EventLog>>,
    permissions: Arc<RwLock<Option<(String, PermissionRegistry)>>>,
    harness: Arc<RwLock<BTreeMap<String, ToolDefinition>>>,
}

impl std::fmt::Debug for ToolSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSurface")
            .field("formal_definitions", &self.formal_definitions())
            .field("definitions", &self.definitions())
            .finish()
    }
}

impl ToolSurface {
    pub fn new(registry: ComponentToolRegistry) -> Self {
        Self {
            registry,
            static_definitions: Arc::new(RwLock::new(BTreeMap::new())),
            dynamic_definitions: Arc::new(RwLock::new(BTreeMap::new())),
            disabled: Arc::new(RwLock::new(BTreeSet::new())),
            pending_messages: Arc::new(Mutex::new(Vec::new())),
            log: None,
            permissions: Arc::new(RwLock::new(None)),
            harness: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub fn with_log(mut self, log: Arc<EventLog>) -> Self {
        self.log = Some(log);
        self
    }

    pub fn with_permissions(
        self,
        profile: impl Into<String>,
        permissions: PermissionRegistry,
    ) -> Self {
        *self.permissions.write().unwrap() = Some((profile.into(), permissions));
        self
    }

    /// Replace the active profile authorization without replacing the shared
    /// component registry. Handoff and live profile updates use this seam.
    pub fn set_permissions(&self, profile: impl Into<String>, permissions: PermissionRegistry) {
        *self.permissions.write().unwrap() = Some((profile.into(), permissions));
        self.recompute_enabled();
    }

    pub fn clear_permissions(&self) {
        *self.permissions.write().unwrap() = None;
        self.recompute_enabled();
    }

    /// Replace the profile's static tool contract while preserving dynamic
    /// composition exports. Every definition is checked against the component
    /// registry before it becomes active.
    pub fn replace_static_definitions(
        &self,
        definitions: Vec<ToolDefinition>,
    ) -> Result<(), ToolSurfaceError> {
        for definition in &definitions {
            if !self
                .registry
                .all_names()
                .iter()
                .any(|name| name == &definition.name)
            {
                return Err(ToolSurfaceError::Unknown(definition.name.clone()));
            }
        }
        self.static_definitions.write().unwrap().clear();
        for definition in definitions {
            self.static_definitions
                .write()
                .unwrap()
                .insert(definition.name.clone(), definition);
        }
        self.recompute_enabled();
        Ok(())
    }

    pub fn registry(&self) -> &ComponentToolRegistry {
        &self.registry
    }

    /// Install the profile-derived harness surface. Harness verbs are
    /// capability components at the model boundary, not kernel commands.
    pub fn configure_harness(&self, policy: HarnessPolicy) {
        let old_names: Vec<String> = self.harness.read().unwrap().keys().cloned().collect();
        for name in old_names {
            self.disabled.write().unwrap().remove(&name);
        }
        let mut harness = self.harness.write().unwrap();
        harness.clear();
        harness.insert(
            "yield".into(),
            ToolDefinition {
                name: "yield".into(),
                description: Some("Report completion state to the harness".into()),
                input_schema: policy.yield_schema,
            },
        );
        if policy.allow_fork {
            harness.insert(
                "fork".into(),
                ToolDefinition {
                    name: "fork".into(),
                    description: Some("Run independent task briefs concurrently".into()),
                    input_schema: serde_json::json!({
                        "type":"object",
                        "properties":{"tasks":{"type":"array","items":{"type":"string"}}},
                        "required":["tasks"],
                        "additionalProperties":false
                    }),
                },
            );
        }
        if policy.allow_handoff {
            harness.insert(
                "handoff".into(),
                ToolDefinition {
                    name: "handoff".into(),
                    description: Some("Switch to another profile with a brief".into()),
                    input_schema: serde_json::json!({
                        "type":"object",
                        "properties":{"profile":{"type":"string"},"brief":{"type":"string"}},
                        "required":["profile","brief"],
                        "additionalProperties":false
                    }),
                },
            );
        }
        drop(harness);
        self.recompute_enabled();
    }

    pub fn register_initial(&self, definition: ToolDefinition) -> Result<(), ToolSurfaceError> {
        let name = definition.name.clone();
        if !self.registry.all_names().iter().any(|value| value == &name) {
            return Err(ToolSurfaceError::Unknown(name));
        }
        let mut definitions = self.static_definitions.write().unwrap();
        if definitions.insert(name.clone(), definition).is_some() {
            return Err(ToolSurfaceError::Duplicate(name));
        }
        if !self.allowed(&name) {
            self.disable(&name);
        } else {
            self.disabled.write().unwrap().remove(&name);
        }
        Ok(())
    }

    pub fn apply(&self, event: ToolSurfaceEvent) -> Result<(), ToolSurfaceError> {
        let record = event.clone();
        match event {
            ToolSurfaceEvent::Available { definition } => {
                let name = definition.name.clone();
                if !self.registry.all_names().iter().any(|value| value == &name) {
                    return Err(ToolSurfaceError::Unknown(name));
                }
                let is_static = self.static_definitions.read().unwrap().contains_key(&name);
                let already_disabled = self.disabled.read().unwrap().contains(&name);
                let same_definition = if is_static {
                    self.static_definitions.read().unwrap().get(&name) == Some(&definition)
                } else {
                    self.dynamic_definitions.read().unwrap().get(&name) == Some(&definition)
                };
                if !already_disabled && same_definition {
                    return Ok(());
                }
                if is_static {
                    self.static_definitions
                        .write()
                        .unwrap()
                        .insert(name.clone(), definition.clone());
                } else {
                    self.dynamic_definitions
                        .write()
                        .unwrap()
                        .insert(name.clone(), definition.clone());
                }
                if self.allowed(&name) {
                    self.disabled.write().unwrap().remove(&name);
                    self.pending_messages
                        .lock()
                        .unwrap()
                        .push(availability_message(&definition));
                } else {
                    self.disable(&name);
                }
            }
            ToolSurfaceEvent::Unavailable { name, reason } => {
                let known = self.static_definitions.read().unwrap().contains_key(&name)
                    || self.dynamic_definitions.read().unwrap().contains_key(&name);
                if !known {
                    return Err(ToolSurfaceError::Unknown(name));
                }
                let already_disabled = self.disabled.read().unwrap().contains(&name);
                self.disable(&name);
                let reason = reason.unwrap_or_else(|| "the tool is no longer available".into());
                if !already_disabled {
                    self.pending_messages.lock().unwrap().push(Message::text(
                        Role::User,
                        format!(
                            "Tool unavailable: {name}. {reason}; attempting to use it will error."
                        ),
                    ));
                }
            }
        }
        self.record(record)?;
        Ok(())
    }

    fn allowed(&self, name: &str) -> bool {
        // Harness verbs are supplied by the active profile policy itself.
        // A profile's component-tool allow-list must not accidentally remove
        // the yield/fork/handoff controls that the same profile explicitly
        // enabled.
        if self.harness.read().unwrap().contains_key(name) {
            return true;
        }
        self.permissions
            .read()
            .unwrap()
            .as_ref()
            .is_none_or(|(profile, permissions)| permissions.authorize(profile, name, ""))
    }

    fn recompute_enabled(&self) {
        let names: Vec<String> = self
            .harness
            .read()
            .unwrap()
            .keys()
            .chain(self.static_definitions.read().unwrap().keys())
            .chain(self.dynamic_definitions.read().unwrap().keys())
            .cloned()
            .collect();
        let mut disabled = self.disabled.write().unwrap();
        for name in names {
            if self.allowed(&name) {
                disabled.remove(&name);
            } else {
                disabled.insert(name.clone());
            }
        }
    }

    fn disable(&self, name: &str) {
        self.disabled.write().unwrap().insert(name.to_owned());
    }

    fn record(&self, event: ToolSurfaceEvent) -> Result<(), ToolSurfaceError> {
        let Some(log) = &self.log else {
            return Ok(());
        };
        let event_type = match event {
            ToolSurfaceEvent::Available { .. } => "tool.available",
            ToolSurfaceEvent::Unavailable { .. } => "tool.unavailable",
        };
        let payload = serde_json::to_value(event)
            .map_err(|error| ToolSurfaceError::Log(error.to_string()))?;
        log.append(event_type, payload)
            .map_err(|error| ToolSurfaceError::Log(error.to_string()))?;
        Ok(())
    }

    pub fn apply_composition_update(
        &self,
        update: CompositionUpdate,
    ) -> Result<(), ToolSurfaceError> {
        match update {
            CompositionUpdate::ToolAvailable {
                name,
                description,
                input_schema,
            } => {
                let input_schema = toon_format::decode_default(&input_schema).map_err(|error| {
                    ToolSurfaceError::Execution(ToolError::Internal(format!(
                        "decode tool schema: {error}"
                    )))
                })?;
                self.apply(ToolSurfaceEvent::Available {
                    definition: ToolDefinition {
                        name,
                        description,
                        input_schema,
                    },
                })
            }
            CompositionUpdate::ToolUnavailable { name, reason } => {
                self.apply(ToolSurfaceEvent::Unavailable { name, reason })
            }
            CompositionUpdate::Context(_) => Err(ToolSurfaceError::Execution(ToolError::Internal(
                "context update supplied to tool surface".into(),
            ))),
        }
    }

    /// Definitions sent through the provider's formal tool API. Dynamic tools
    /// intentionally do not appear here; they are announced inline.
    pub fn formal_definitions(&self) -> Vec<ToolDefinition> {
        let disabled = self.disabled.read().unwrap();
        self.harness
            .read()
            .unwrap()
            .values()
            .chain(self.static_definitions.read().unwrap().values())
            .filter(|definition| {
                !disabled.contains(&definition.name) && self.allowed(&definition.name)
            })
            .cloned()
            .collect()
    }

    /// All currently enabled model-facing definitions, including dynamic ones.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let disabled = self.disabled.read().unwrap();
        self.harness
            .read()
            .unwrap()
            .values()
            .chain(self.static_definitions.read().unwrap().values())
            .chain(self.dynamic_definitions.read().unwrap().values())
            .filter(|definition| {
                !disabled.contains(&definition.name) && self.allowed(&definition.name)
            })
            .cloned()
            .collect()
    }

    pub fn take_messages(&self) -> Vec<Message> {
        std::mem::take(&mut *self.pending_messages.lock().unwrap())
    }

    fn invoke_harness(
        &self,
        call: &llm_provider::ToolCall,
    ) -> Result<Option<llm_provider::ToolResult>, crate::ToolError> {
        if !self.harness.read().unwrap().contains_key(&call.name) {
            return Ok(None);
        }
        if !self.allowed(&call.name) || self.disabled.read().unwrap().contains(&call.name) {
            let envelope = ToolResultEnvelope {
                ok: false,
                output: None,
                error: Some(artist_component::ToolFailure {
                    code: "permission_denied".into(),
                    message: format!("permission denied: {}", call.name),
                    details: None,
                }),
            };
            return Ok(Some(llm_provider::ToolResult {
                call_id: call.id.clone(),
                content: vec![llm_provider::ContentPart::Text {
                    text: serde_json::to_string(&envelope).map_err(|error| {
                        crate::ToolError::Failed {
                            message: error.to_string(),
                        }
                    })?,
                }],
                is_error: true,
            }));
        }
        let (event_type, payload) = match call.name.as_str() {
            "yield" => {
                let complete = call
                    .arguments
                    .get("complete")
                    .and_then(serde_json::Value::as_bool)
                    .ok_or_else(|| crate::ToolError::Failed {
                        message: "yield.complete must be a boolean".into(),
                    })?;
                (
                    "harness.yield",
                    serde_json::json!({
                        "complete": complete,
                        "remaining": call.arguments.get("remaining"),
                        "payload": call.arguments,
                    }),
                )
            }
            "fork" => {
                let tasks = call
                    .arguments
                    .get("tasks")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| crate::ToolError::Failed {
                        message: "fork.tasks must be an array".into(),
                    })?;
                ("harness.fork", serde_json::json!({"tasks": tasks}))
            }
            "handoff" => {
                let profile = call
                    .arguments
                    .get("profile")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| crate::ToolError::Failed {
                        message: "handoff.profile is required".into(),
                    })?;
                let brief = call
                    .arguments
                    .get("brief")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| crate::ToolError::Failed {
                        message: "handoff.brief is required".into(),
                    })?;
                (
                    "harness.handoff",
                    serde_json::json!({"profile": profile, "brief": brief}),
                )
            }
            _ => return Ok(None),
        };
        if let Some(log) = &self.log {
            log.append(event_type, payload.clone())
                .map_err(|error| crate::ToolError::Failed {
                    message: format!("persist harness event: {error}"),
                })?;
        }
        Ok(Some(llm_provider::ToolResult {
            call_id: call.id.clone(),
            content: vec![llm_provider::ContentPart::Text {
                text: serde_json::to_string(&ToolResultEnvelope::success(payload)).map_err(
                    |error| crate::ToolError::Failed {
                        message: error.to_string(),
                    },
                )?,
            }],
            is_error: false,
        }))
    }
}

#[async_trait::async_trait]
impl crate::ToolInvoker for ToolSurface {
    async fn invoke(
        &self,
        call: llm_provider::ToolCall,
    ) -> Result<llm_provider::ToolResult, crate::ToolError> {
        if self.disabled.read().unwrap().contains(&call.name) {
            let error = ToolError::Unavailable(call.name.clone());
            let envelope = ToolResultEnvelope::failure(&error);
            return Ok(llm_provider::ToolResult {
                call_id: call.id,
                content: vec![llm_provider::ContentPart::Text {
                    text: serde_json::to_string(&envelope).map_err(|error| {
                        crate::ToolError::Failed {
                            message: error.to_string(),
                        }
                    })?,
                }],
                is_error: true,
            });
        }
        if let Some(result) = self.invoke_harness(&call)? {
            return Ok(result);
        }
        let resource = call
            .arguments
            .get("uri")
            .or_else(|| call.arguments.get("source"))
            .or_else(|| call.arguments.get("target"))
            .or_else(|| call.arguments.get("process"))
            .or_else(|| call.arguments.get("working_directory"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let denied =
            self.permissions
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|(profile, permissions)| {
                    !permissions.authorize(profile, &call.name, resource)
                });
        if denied {
            let envelope = ToolResultEnvelope {
                ok: false,
                output: None,
                error: Some(artist_component::ToolFailure {
                    code: "permission_denied".into(),
                    message: format!("permission denied: {}", call.name),
                    details: None,
                }),
            };
            return Ok(llm_provider::ToolResult {
                call_id: call.id,
                content: vec![llm_provider::ContentPart::Text {
                    text: serde_json::to_string(&envelope).map_err(|error| {
                        crate::ToolError::Failed {
                            message: error.to_string(),
                        }
                    })?,
                }],
                is_error: true,
            });
        }
        let request = toon_format::encode_default(&call.arguments).map_err(|error| {
            crate::ToolError::Failed {
                message: format!("encode tool request: {error}"),
            }
        })?;
        let result = self
            .registry
            .invoke_enveloped(&call.name, request.as_bytes())
            .await;
        let bytes = result;
        let text = std::str::from_utf8(&bytes).map_err(|error| crate::ToolError::Failed {
            message: error.to_string(),
        })?;
        let envelope: ToolResultEnvelope =
            serde_json::from_str(text).map_err(|error| crate::ToolError::Failed {
                message: error.to_string(),
            })?;
        Ok(llm_provider::ToolResult {
            call_id: call.id,
            content: vec![llm_provider::ContentPart::Text {
                text: serde_json::to_string(&envelope).map_err(|error| {
                    crate::ToolError::Failed {
                        message: error.to_string(),
                    }
                })?,
            }],
            is_error: !envelope.ok,
        })
    }
}

fn availability_message(definition: &ToolDefinition) -> Message {
    let schema =
        toon_format::encode_default(&definition.input_schema).unwrap_or_else(|_| "{}".into());
    Message::text(
        Role::User,
        format!(
            "Tool available inline: {}\nDescription: {}\nInput schema: {}",
            definition.name,
            definition
                .description
                .as_deref()
                .unwrap_or("(no description)"),
            schema
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolInvoker;

    struct Echo;
    #[async_trait::async_trait]
    impl artist_component::ToolComponent for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, artist_component::ToolError> {
            Ok(request.to_vec())
        }
    }

    struct Late;
    #[async_trait::async_trait]
    impl artist_component::ToolComponent for Late {
        fn name(&self) -> &str {
            "late"
        }
        async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, artist_component::ToolError> {
            Ok(request.to_vec())
        }
    }

    #[test]
    fn availability_events_update_surface_projection() {
        let registry = ComponentToolRegistry::new();
        registry.register(Echo).unwrap();
        let surface = ToolSurface::new(registry.clone());
        surface
            .register_initial(ToolDefinition {
                name: "echo".into(),
                description: Some("Echo input".into()),
                input_schema: serde_json::json!({"type":"object"}),
            })
            .unwrap();
        surface
            .apply(ToolSurfaceEvent::Unavailable {
                name: "echo".into(),
                reason: None,
            })
            .unwrap();
        assert!(surface.definitions().is_empty());
        assert_eq!(surface.take_messages().len(), 1);
        surface
            .apply(ToolSurfaceEvent::Available {
                definition: ToolDefinition {
                    name: "echo".into(),
                    description: None,
                    input_schema: serde_json::json!({}),
                },
            })
            .unwrap();
        assert_eq!(surface.definitions().len(), 1);
        assert_eq!(surface.take_messages().len(), 1);
    }

    #[test]
    fn dynamic_tools_are_inline_only_while_static_tools_remain_formal() {
        let registry = ComponentToolRegistry::new();
        registry.register(Echo).unwrap();
        let surface = ToolSurface::new(registry);
        surface
            .register_initial(ToolDefinition {
                name: "echo".into(),
                description: Some("static".into()),
                input_schema: serde_json::json!({"type":"object"}),
            })
            .unwrap();

        assert_eq!(
            surface
                .formal_definitions()
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["echo"]
        );
        surface
            .apply(ToolSurfaceEvent::Unavailable {
                name: "echo".into(),
                reason: None,
            })
            .unwrap();
        assert!(surface.formal_definitions().is_empty());
        surface
            .apply(ToolSurfaceEvent::Available {
                definition: ToolDefinition {
                    name: "echo".into(),
                    description: Some("dynamic".into()),
                    input_schema: serde_json::json!({}),
                },
            })
            .unwrap();

        assert_eq!(
            surface.formal_definitions()[0].description.as_deref(),
            Some("dynamic")
        );
        assert_eq!(
            surface.definitions()[0].description.as_deref(),
            Some("dynamic")
        );

        let dynamic_registry = ComponentToolRegistry::new();
        dynamic_registry.register(Late).unwrap();
        let dynamic_surface = ToolSurface::new(dynamic_registry);
        dynamic_surface
            .apply(ToolSurfaceEvent::Available {
                definition: ToolDefinition {
                    name: "late".into(),
                    description: Some("dynamic".into()),
                    input_schema: serde_json::json!({}),
                },
            })
            .unwrap();
        assert!(dynamic_surface.formal_definitions().is_empty());
        assert_eq!(dynamic_surface.definitions()[0].name, "late");
        assert_eq!(dynamic_surface.take_messages().len(), 1);
    }

    #[test]
    fn surface_is_the_execution_authority_for_stale_calls() {
        let registry = ComponentToolRegistry::new();
        registry.register(Echo).unwrap();
        let surface = ToolSurface::new(registry);
        surface
            .register_initial(ToolDefinition {
                name: "echo".into(),
                description: None,
                input_schema: serde_json::json!({}),
            })
            .unwrap();
        surface
            .apply(ToolSurfaceEvent::Unavailable {
                name: "echo".into(),
                reason: None,
            })
            .unwrap();

        let result = futures::executor::block_on(surface.invoke(llm_provider::ToolCall {
            id: "call-1".into(),
            name: "echo".into(),
            arguments: serde_json::json!({"value":"x"}),
        }))
        .unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn composition_tool_updates_use_the_same_surface_path() {
        let registry = ComponentToolRegistry::new();
        registry.register(Echo).unwrap();
        let surface = ToolSurface::new(registry);
        surface
            .apply_composition_update(CompositionUpdate::ToolAvailable {
                name: "echo".into(),
                description: Some("Echo input".into()),
                input_schema: "type: object\n".into(),
            })
            .unwrap();
        assert_eq!(surface.definitions()[0].name, "echo");
        assert_eq!(surface.take_messages().len(), 1);
        surface
            .apply_composition_update(CompositionUpdate::ToolUnavailable {
                name: "echo".into(),
                reason: Some("policy changed".into()),
            })
            .unwrap();
        assert!(surface.definitions().is_empty());
    }

    #[test]
    fn surface_logs_changes_and_hides_whole_verb_denials() {
        let registry = ComponentToolRegistry::new();
        registry.register(Echo).unwrap();
        let permissions =
            artist_component::PermissionRegistry::new([artist_component::PermissionRule {
                effect: artist_component::PermissionEffect::Deny,
                verb: Some("echo".into()),
                pattern: None,
            }]);
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(EventLog::open(dir.path().join("events.jsonl"), "session").unwrap());
        let surface = ToolSurface::new(registry)
            .with_permissions("reviewer", permissions)
            .with_log(log.clone());
        surface
            .register_initial(ToolDefinition {
                name: "echo".into(),
                description: None,
                input_schema: serde_json::json!({}),
            })
            .unwrap();
        assert!(surface.definitions().is_empty());
        surface
            .apply(ToolSurfaceEvent::Unavailable {
                name: "echo".into(),
                reason: None,
            })
            .unwrap();
        assert_eq!(log.records().unwrap()[0].event_type, "tool.unavailable");
    }
}
