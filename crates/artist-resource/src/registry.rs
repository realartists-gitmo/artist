use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
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

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .read()
            .expect("tool registry lock poisoned")
            .values()
            .map(|t| t.definition.clone())
            .collect()
    }

    pub fn owner(&self, name: &str) -> Option<String> {
        self.tools
            .read()
            .expect("tool registry lock poisoned")
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
            .expect("tool registry lock poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::Unknown(name.into()))?
            .handler;
        context.stack.push(name.into());
        handler.call(arguments, context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
