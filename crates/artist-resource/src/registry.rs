use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    PollOutcome, ResourceError, ResourceReply, ResourceRequest, ResourceRouter, ResourceUri,
    SearchEngine,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvocationContext {
    pub correlation_id: Uuid,
    pub stack: Vec<String>,
}

impl InvocationContext {
    pub fn root() -> Self {
        Self {
            correlation_id: Uuid::new_v4(),
            stack: Vec::new(),
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
    async fn call(&self, arguments: Value, context: InvocationContext) -> Result<Value, ToolError>;
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

    pub async fn register(
        &self,
        definition: ToolDefinition,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<(), ToolError> {
        self.register_inner(definition, handler, None).await
    }

    pub async fn register_owned(
        &self,
        definition: ToolDefinition,
        handler: Arc<dyn ToolHandler>,
        owner: impl Into<String>,
    ) -> Result<(), ToolError> {
        self.register_inner(definition, handler, Some(owner.into()))
            .await
    }

    async fn register_inner(
        &self,
        definition: ToolDefinition,
        handler: Arc<dyn ToolHandler>,
        owner: Option<String>,
    ) -> Result<(), ToolError> {
        let mut tools = self.tools.write().await;
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

    async fn replace(&self, definition: ToolDefinition, handler: Arc<dyn ToolHandler>) {
        self.tools.write().await.insert(
            definition.name.clone(),
            RegisteredTool {
                definition,
                handler,
                owner: None,
            },
        );
    }

    pub async fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .read()
            .await
            .values()
            .map(|t| t.definition.clone())
            .collect()
    }

    pub async fn owner(&self, name: &str) -> Option<String> {
        self.tools
            .read()
            .await
            .get(name)
            .and_then(|tool| tool.owner.clone())
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Result<Value, ToolError> {
        self.call_with_context(name, arguments, InvocationContext::root())
            .await
    }

    pub async fn call_with_context(
        &self,
        name: &str,
        arguments: Value,
        mut context: InvocationContext,
    ) -> Result<Value, ToolError> {
        if let Some(start) = context.stack.iter().position(|entry| entry == name) {
            let mut cycle = context.stack[start..].to_vec();
            cycle.push(name.to_owned());
            return Err(ToolError::Recursive {
                cycle: cycle.join(" -> "),
            });
        }
        let handler = self
            .tools
            .read()
            .await
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::Unknown(name.into()))?
            .handler;
        context.stack.push(name.into());
        handler.call(arguments, context).await
    }
}

pub struct UniversalTools {
    router: ResourceRouter,
    search: Option<Arc<SearchEngine>>,
    working_directory: PathBuf,
}

impl UniversalTools {
    pub fn new(router: ResourceRouter, working_directory: impl Into<PathBuf>) -> Self {
        Self {
            router,
            search: None,
            working_directory: working_directory.into(),
        }
    }
    pub fn with_search(mut self, search: Arc<SearchEngine>) -> Self {
        self.search = Some(search);
        self
    }

    pub async fn register(self, registry: &ToolRegistry) -> Result<(), ToolError> {
        let this = Arc::new(self);
        for definition in universal_definitions() {
            registry.register(definition, this.clone()).await?;
        }
        Ok(())
    }

    pub async fn install(self, registry: &ToolRegistry) {
        let this = Arc::new(self);
        for definition in universal_definitions() {
            registry.replace(definition, this.clone()).await;
        }
    }

    fn uri(&self, value: &Value, key: &str) -> Result<ResourceUri, ToolError> {
        let text = value
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Arguments(format!("missing string argument: {key}")))?;
        ResourceUri::resolve(text, &self.working_directory)
            .map_err(|e| ToolError::Arguments(e.to_string()))
    }

    fn search(&self) -> Result<&SearchEngine, ToolError> {
        let search = self
            .search
            .as_deref()
            .ok_or_else(|| ToolError::Failed("search index is not mounted".into()))?;
        search
            .synchronize(self.router.generation())
            .map_err(ToolError::Failed)?;
        Ok(search)
    }
}

#[async_trait]
impl ToolHandler for UniversalTools {
    async fn call(&self, a: Value, context: InvocationContext) -> Result<Value, ToolError> {
        let name = context
            .stack
            .last()
            .expect("dispatcher pushed tool name")
            .as_str();
        match name {
            "read" => reply(
                self.router
                    .handle(ResourceRequest::Read {
                        uri: self.uri(&a, "uri")?,
                        start_line: u64_arg(&a, "start_line")?,
                        line_count: u64_arg(&a, "line_count")?,
                    })
                    .await?,
            ),
            "write" => {
                let text = string_arg(&a, "text")?;
                let result = self
                    .router
                    .handle(ResourceRequest::Write {
                        uri: self.uri(&a, "uri")?,
                        text,
                    })
                    .await?;
                if self.search.is_some() {
                    self.search()?;
                }
                reply(result)
            }
            "move" => {
                let to = a
                    .get("to")
                    .filter(|v| !v.is_null())
                    .map(|_| self.uri(&a, "to"))
                    .transpose()?;
                let result = self
                    .router
                    .handle(ResourceRequest::Move {
                        from: self.uri(&a, "from")?,
                        to,
                    })
                    .await?;
                if self.search.is_some() {
                    self.search()?;
                }
                reply(result)
            }
            "poll" => {
                let pattern = a.get("match").and_then(Value::as_str).map(str::to_owned);
                let timeout = u64_arg(&a, "timeout_ms")?.map(Duration::from_millis);
                reply(
                    self.router
                        .handle(ResourceRequest::Poll {
                            uri: self.uri(&a, "uri")?,
                            pattern,
                            timeout,
                        })
                        .await?,
                )
            }
            "find" => self
                .search()?
                .find(
                    &self.uri(&a, "uri")?,
                    a.get("glob").and_then(Value::as_str),
                    u64_arg(&a, "max_depth")?.map(|v| v as usize),
                    a.get("cursor").and_then(Value::as_str),
                    u64_arg(&a, "limit")?.map(|v| v as usize).unwrap_or(50),
                )
                .map_err(ToolError::Failed),
            "grep" => self
                .search()?
                .grep(
                    &self.uri(&a, "uri")?,
                    &string_arg(&a, "regex")?,
                    a.get("include_glob").and_then(Value::as_str),
                    u64_arg(&a, "context")?.map(|v| v as usize).unwrap_or(0),
                    a.get("cursor").and_then(Value::as_str),
                    u64_arg(&a, "limit")?.map(|v| v as usize).unwrap_or(100),
                )
                .map_err(ToolError::Failed),
            _ => Err(ToolError::Unknown(name.into())),
        }
    }
}

fn reply(reply: ResourceReply) -> Result<Value, ToolError> {
    Ok(match reply {
        ResourceReply::Text { text } => json!({"text": text}),
        ResourceReply::Children { children } => json!({"children": children}),
        ResourceReply::Written => json!({"written": true}),
        ResourceReply::Moved => json!({"moved": true}),
        ResourceReply::Poll { text, outcome } => {
            json!({"text": text, "outcome": match outcome { PollOutcome::Matched => "matched", PollOutcome::Closed => "closed", PollOutcome::TimedOut => "timed-out" }})
        }
    })
}

fn string_arg(value: &Value, key: &str) -> Result<String, ToolError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ToolError::Arguments(format!("missing string argument: {key}")))
}
fn u64_arg(value: &Value, key: &str) -> Result<Option<u64>, ToolError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| ToolError::Arguments(format!("{key} must be an unsigned integer"))),
    }
}

fn universal_definitions() -> Vec<ToolDefinition> {
    let object = |required: &[&str], properties: Value| json!({"type":"object", "required": required, "properties": properties, "additionalProperties": false});
    vec![
        ToolDefinition { name: "read".into(), description: "Read UTF-8 text from a resource URI.".into(), input_schema: object(&["uri"], json!({"uri":{"type":"string"},"start_line":{"type":"integer","minimum":1},"line_count":{"type":"integer","minimum":1}})) },
        ToolDefinition { name: "find".into(), description: "Find descendants of a resource URI. With no glob and depth one, lists immediate children.".into(), input_schema: object(&["uri"], json!({"uri":{"type":"string"},"glob":{"type":"string"},"max_depth":{"type":"integer"},"cursor":{"type":"string"},"limit":{"type":"integer"}})) },
        ToolDefinition { name: "grep".into(), description: "Search text resources with a regular expression.".into(), input_schema: object(&["uri","regex"], json!({"uri":{"type":"string"},"regex":{"type":"string"},"include_glob":{"type":"string"},"context":{"type":"integer"},"cursor":{"type":"string"},"limit":{"type":"integer"}})) },
        ToolDefinition { name: "write".into(), description: "Write UTF-8 text to a resource URI.".into(), input_schema: object(&["uri","text"], json!({"uri":{"type":"string"},"text":{"type":"string"}})) },
        ToolDefinition { name: "move".into(), description: "Move a resource, or remove it when `to` is null.".into(), input_schema: object(&["from"], json!({"from":{"type":"string"},"to":{"type":["string","null"]}})) },
        ToolDefinition { name: "poll".into(), description: "Wait for subsequently appended text, a match, close, or timeout.".into(), input_schema: object(&["uri"], json!({"uri":{"type":"string"},"match":{"type":"string"},"timeout_ms":{"type":"integer"}})) },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Nested {
        registry: ToolRegistry,
        next: &'static str,
    }
    #[async_trait]
    impl ToolHandler for Nested {
        async fn call(&self, _: Value, ctx: InvocationContext) -> Result<Value, ToolError> {
            self.registry
                .call_with_context(self.next, json!({}), ctx)
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
                },
                Arc::new(Nested {
                    registry: registry.clone(),
                    next: "b",
                }),
            )
            .await
            .unwrap();
        registry
            .register(
                ToolDefinition {
                    name: "b".into(),
                    description: "".into(),
                    input_schema: json!({}),
                },
                Arc::new(Nested {
                    registry: registry.clone(),
                    next: "a",
                }),
            )
            .await
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
        async fn call(&self, _: Value, ctx: InvocationContext) -> Result<Value, ToolError> {
            self.seen.lock().unwrap().push(ctx.clone());
            if let Some(next) = self.next {
                self.registry.call_with_context(next, json!({}), ctx).await
            } else {
                Ok(json!({"ok": true}))
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
                    },
                    Arc::new(Probe {
                        registry: registry.clone(),
                        next,
                        seen: seen.clone(),
                    }),
                )
                .await
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
}
