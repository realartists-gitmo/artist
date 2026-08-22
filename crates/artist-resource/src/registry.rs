use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    pub uri: String,
    #[schemars(range(min = 1))]
    pub start_line: Option<u64>,
    #[schemars(range(min = 1))]
    pub line_count: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindArgs {
    pub uri: String,
    pub glob: Option<String>,
    pub max_depth: Option<usize>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrepArgs {
    pub uri: String,
    pub regex: String,
    pub include_glob: Option<String>,
    pub context: Option<usize>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteArgs {
    pub uri: String,
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArgs {
    pub from: String,
    pub to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PollArgs {
    pub uri: String,
    #[serde(rename = "match")]
    #[schemars(rename = "match")]
    pub pattern: Option<String>,
    pub timeout_ms: Option<u64>,
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

    fn replace(&self, definition: ToolDefinition, handler: Arc<dyn ToolHandler>) {
        self.tools
            .write()
            .expect("tool registry lock poisoned")
            .insert(
                definition.name.clone(),
                RegisteredTool {
                    definition,
                    handler,
                    owner: None,
                },
            );
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

    pub fn register(self, registry: &ToolRegistry) -> Result<(), ToolError> {
        let this = Arc::new(self);
        for definition in universal_definitions() {
            registry.register(definition, this.clone())?;
        }
        Ok(())
    }

    pub fn install(self, registry: &ToolRegistry) {
        let this = Arc::new(self);
        for definition in universal_definitions() {
            registry.replace(definition, this.clone());
        }
    }

    fn uri(&self, text: &str) -> Result<ResourceUri, ToolError> {
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
            "read" => {
                let args: ReadArgs = arguments(a)?;
                if args.start_line == Some(0) || args.line_count == Some(0) {
                    return Err(ToolError::Arguments(
                        "start_line and line_count must be positive".into(),
                    ));
                }
                reply(
                    self.router
                        .handle(ResourceRequest::Read {
                            uri: self.uri(&args.uri)?,
                            start_line: args.start_line,
                            line_count: args.line_count,
                        })
                        .await?,
                )
            }
            "write" => {
                let args: WriteArgs = arguments(a)?;
                let result = self
                    .router
                    .handle(ResourceRequest::Write {
                        uri: self.uri(&args.uri)?,
                        text: args.text,
                    })
                    .await?;
                if self.search.is_some() {
                    self.search()?;
                }
                reply(result)
            }
            "move" => {
                let args: MoveArgs = arguments(a)?;
                let to = args.to.as_deref().map(|to| self.uri(to)).transpose()?;
                let result = self
                    .router
                    .handle(ResourceRequest::Move {
                        from: self.uri(&args.from)?,
                        to,
                    })
                    .await?;
                if self.search.is_some() {
                    self.search()?;
                }
                reply(result)
            }
            "poll" => {
                let args: PollArgs = arguments(a)?;
                let timeout = args.timeout_ms.map(Duration::from_millis);
                reply(
                    self.router
                        .handle(ResourceRequest::Poll {
                            uri: self.uri(&args.uri)?,
                            pattern: args.pattern,
                            timeout,
                        })
                        .await?,
                )
            }
            "find" => {
                let args: FindArgs = arguments(a)?;
                self.search()?
                    .find(
                        &self.uri(&args.uri)?,
                        args.glob.as_deref(),
                        args.max_depth,
                        args.cursor.as_deref(),
                        args.limit.unwrap_or(50),
                    )
                    .map_err(ToolError::Failed)
            }
            "grep" => {
                let args: GrepArgs = arguments(a)?;
                self.search()?
                    .grep(
                        &self.uri(&args.uri)?,
                        &args.regex,
                        args.include_glob.as_deref(),
                        args.context.unwrap_or(0),
                        args.cursor.as_deref(),
                        args.limit.unwrap_or(100),
                    )
                    .map_err(ToolError::Failed)
            }
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

fn arguments<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ToolError> {
    serde_json::from_value(value).map_err(|error| ToolError::Arguments(error.to_string()))
}

fn universal_definitions() -> Vec<ToolDefinition> {
    vec![
        definition::<ReadArgs>("read", "Read UTF-8 text from a resource URI."),
        definition::<FindArgs>(
            "find",
            "Find descendants of a resource URI. With no glob and depth one, lists immediate children.",
        ),
        definition::<GrepArgs>("grep", "Search text resources with a regular expression."),
        definition::<WriteArgs>("write", "Write UTF-8 text to a resource URI."),
        definition::<MoveArgs>("move", "Move a resource, or remove it when `to` is null."),
        definition::<PollArgs>(
            "poll",
            "Wait for subsequently appended text, a match, close, or timeout.",
        ),
    ]
}

fn definition<T: JsonSchema>(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: serde_json::to_value(schemars::schema_for!(T))
            .expect("JSON schema is serializable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn universal_schemas_are_derived_closed_contracts() {
        let definitions = universal_definitions();
        assert_eq!(
            definitions
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["read", "find", "grep", "write", "move", "poll"]
        );
        for definition in &definitions {
            assert_eq!(definition.input_schema["additionalProperties"], false);
        }
        assert_eq!(definitions[0].input_schema["required"], json!(["uri"]));
        assert_eq!(
            definitions[2].input_schema["required"],
            json!(["uri", "regex"])
        );
        assert!(arguments::<ReadArgs>(json!({"uri":"x", "extra": true})).is_err());
        assert!(arguments::<GrepArgs>(json!({"uri":"x"})).is_err());
    }
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
