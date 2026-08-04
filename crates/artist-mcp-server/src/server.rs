//! The MCP server: harness tools on the wire.
//!
//! One long-lived process owns the tools and the durable envelope; each MCP
//! connection (stdio, later HTTP) attaches to it. This is the anti-realartist
//! structure: nothing that matters lives in the connection, so a dropped
//! stdio pipe or a tunnel reconnect cannot lose a shell, a job, or an answer.
//!
//! `tools/list` and `tools/call` are implemented directly on
//! [`rmcp::handler::server::ServerHandler`] rather than through rmcp's
//! `Router`, because the router drops the per-call `_meta` that carries the
//! idempotency key the envelope dedups on.

use std::{collections::HashMap, path::Path, sync::Arc};

use artist_tool_api::{ArtistDynamicTool, ArtistToolOutput};
use rig_core::completion::message::{DocumentSourceKind, MimeType, ToolResultContent};
use rmcp::{
    ErrorData, RoleServer,
    handler::server::ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, ContentBlock, ListToolsResult, Meta,
        PaginatedRequestParams, Resource, ResourceContents, ServerInfo, Tool, ToolAnnotations,
    },
    service::RequestContext,
};

use crate::envelope::Envelope;

/// What ChatGPT's web agent is told about how these tools behave. The
/// non-obvious parts are the handle-then-poll shape of long-running tools and
/// the idempotency contract that makes a tunnel reconnect safe.
const INSTRUCTIONS: &str = "\
You are driving the artist harness over MCP: a real project, real files, real \
shells. It is a durable workbench, not a stateless API.

- Prefer the structural tools (code_map, code_show, code_deps, code_trace, \
ast_query) over reading whole files; they are cheaper and they reason about the \
code as a graph.
- `bash` has two shapes. `exec` runs a foreground command and returns when it \
finishes. `background`/`start` begins a persistent session and returns a \
sessionId immediately; read its output with mode `read`, send it input with \
mode `send`, and stop it with mode `stop`. Keep sessions short.
- Work may outlive this connection. If you started something long-running, \
poll for its result rather than re-running it.
- Every call that carries `_meta.idempotencyKey` is deduplicated server-side: \
reusing a key returns the stored result instead of executing again. Never reuse \
a key for two different calls; reuse it only to recover a result you may have \
already gotten.

Asking the user is post-then-poll, not a blocking call. `ask` writes the \
questions to a durable outbox and returns their ids immediately; it does not \
wait for an answer. The user answers out-of-band — relay the questions to them \
in the chat. Then call `ask_result` with the ids to check for the answer, or \
`ask_answer` to record what the user said. A question and its answer survive a \
connection dying, so poll with the ids from your earlier `ask` call rather than \
re-asking. Use `ask_list` to rediscover questions a previous connection posted.

There is a human behind you. If you need a decision, ask rather than guessing, \
and wait for the answer before acting on it.";

/// The MCP server for one project.
#[derive(Clone)]
pub struct McpServer {
    tools: Vec<ArtistDynamicTool>,
    by_name: Arc<HashMap<String, ArtistDynamicTool>>,
    identity: artist_agent::tool_set::McpIdentity,
    envelope: Envelope,
    /// Serializes keyed operations across HTTP sessions so two simultaneous
    /// retries cannot both pass the replay check and execute the same effect.
    idempotency_gate: Arc<tokio::sync::Mutex<()>>,
}

impl McpServer {
    /// Wrap a tool surface (from [`artist_agent::tool_set::mcp_surface`]) as
    /// an MCP server. `state_dir` is where the durable envelope lives; `None`
    /// makes calls volatile across restarts but still replay-safe in-process.
    pub fn new(tools: Vec<ArtistDynamicTool>, state_dir: Option<&Path>) -> anyhow::Result<Self> {
        Self::with_identity(
            tools,
            state_dir,
            artist_agent::tool_set::McpIdentity {
                actor: "mcp".into(),
                profile: "worker".into(),
                project: "unknown".into(),
                name: "mcp".into(),
                registered: false,
            },
        )
    }

    pub fn with_identity(
        mut tools: Vec<ArtistDynamicTool>,
        state_dir: Option<&Path>,
        identity: artist_agent::tool_set::McpIdentity,
    ) -> anyhow::Result<Self> {
        let envelope = Envelope::open(state_dir)?;
        anyhow::ensure!(
            !tools.iter().any(|tool| tool.name() == "operation"),
            "tool surface already defines reserved tool operation"
        );
        tools.push(crate::admin::operation_tool(envelope.clone()));
        let by_name = tools
            .iter()
            .map(|tool| (tool.name().to_owned(), tool.clone()))
            .collect();
        Ok(Self {
            tools,
            by_name: Arc::new(by_name),
            identity,
            envelope,
            idempotency_gate: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// The published tool names, in surface order.
    pub fn names(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect()
    }

    /// Invoke a tool by name, running the same path an MCP `tools/call` takes —
    /// envelope replay, execution, commit — without a transport request context.
    ///
    /// The canvas bridge dispatches through here so a page cannot disagree with
    /// the model about what a tool does: it sees exactly what `call_tool` sees.
    pub async fn invoke(
        &self,
        name: &str,
        arguments: serde_json::Value,
        meta: Meta,
    ) -> CallToolResult {
        let arguments = if arguments.is_object() {
            arguments
        } else {
            serde_json::Value::Object(Default::default())
        };
        if let Some(key) = idempotency_key(&meta) {
            // The replay check and eventual commit are one critical section.
            // Without this, two HTTP sessions carrying the same key can both
            // observe a miss and perform the side effect before either commits.
            let _guard = self.idempotency_gate.lock().await;
            if let Some(record) = self.envelope.get(&key) {
                if record.tool != name || record.arguments != arguments {
                    return CallToolResult::error(vec![ContentBlock::text(format!(
                        "idempotency key {key:?} was already used for {} with different arguments; use a new key for a different logical operation",
                        record.tool
                    ))]);
                }
                return serde_json::from_value(record.result).unwrap_or(CallToolResult::error(
                    vec![ContentBlock::text("corrupt envelope record")],
                ));
            }
            let result = self.execute(name, arguments.clone()).await;
            if let Ok(encoded) = serde_json::to_value(&result)
                && let Err(error) = self.envelope.commit(&key, name, &arguments, &encoded)
            {
                tracing::warn!("envelope commit failed: {error:#}");
            }
            return result;
        }
        self.execute(name, arguments).await
    }

    async fn execute(&self, name: &str, arguments: serde_json::Value) -> CallToolResult {
        let Some(tool) = self.by_name.get(name) else {
            return CallToolResult::error(vec![ContentBlock::text(format!(
                "unknown tool: {name}"
            ))]);
        };
        if let Err(error) =
            validate_schema(&tool.definition().input_schema, &arguments, "input", name)
        {
            return CallToolResult::error(vec![ContentBlock::text(error)]);
        }
        match tool.execute(arguments).await {
            Ok(output) => {
                if let Err(error) = validate_schema(
                    &tool.definition().output_schema,
                    &output.structured,
                    "output",
                    name,
                ) {
                    tracing::error!(tool = name, "{error}");
                    return CallToolResult::error(vec![ContentBlock::text(error)]);
                }
                render_output(output)
            }
            Err(error) => CallToolResult::error(vec![ContentBlock::text(format!("{error}"))]),
        }
    }

    /// Serve over MCP stdio — the transport a Secure MCP Tunnel (`tunnel-client`
    /// with `MCP_COMMAND`) or a local client spawns.
    ///
    /// `serve_server` hands back a [`RunningService`] whose request loop runs on
    /// a spawned task; awaiting `waiting()` is what keeps that loop alive until
    /// the connection closes. Returning without it would drop the runtime and
    /// kill the server right after the initialize handshake.
    pub async fn serve_stdio(self) -> anyhow::Result<()> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let running = rmcp::service::serve_server(self, (stdin, stdout))
            .await
            .map_err(|error| anyhow::anyhow!(format!("{error}")))?;
        running
            .waiting()
            .await
            .map_err(|error| anyhow::anyhow!(format!("{error}")))?;
        Ok(())
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities.tools = Some(Default::default());
        info.server_info.title = Some(format!("Artist — {}", self.identity.name));
        info.server_info.description = Some(format!(
            "Artist MCP harness for {} using profile {}",
            self.identity.project, self.identity.profile
        ));
        info.instructions = Some(format!(
            "You are {name}, Artist actor {actor}, using profile {profile} in {project}. \
Other agents and the user address you as {name}. Identity was {registration}.\n\n{INSTRUCTIONS}",
            name = self.identity.name,
            actor = self.identity.actor,
            profile = self.identity.profile,
            project = self.identity.project,
            registration = if self.identity.registered {
                "registered successfully"
            } else {
                "derived from the actor because the registry was unavailable"
            },
        ));
        let mut meta = serde_json::Map::new();
        meta.insert(
            "artist".into(),
            serde_json::json!({"identity": self.identity}),
        );
        info.meta = Some(Meta(meta));
        info
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.by_name.get(name).map(mcp_tool)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.tools.iter().map(mcp_tool).collect(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = request.name.as_ref();
        if !self.by_name.contains_key(name) {
            return Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            ));
        }
        let arguments = request
            .arguments
            .map(serde_json::Value::Object)
            .unwrap_or_else(|| serde_json::Value::Object(Default::default()));

        // Replay before executing: a tunnel reconnect may re-deliver a call we
        // already ran, and running it again is how a stray double-`bash` happens.
        // rmcp strips `_meta` off the params and carries it on the request
        // context, so the key is read from `context.meta`, not `request.meta`.
        Ok(self.invoke(name, arguments, context.meta.clone()).await)
    }
}

fn validate_schema(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    kind: &str,
    tool: &str,
) -> Result<(), String> {
    let compiled = jsonschema::JSONSchema::compile(schema)
        .map_err(|error| format!("invalid {kind} schema for {tool}: {error}"))?;
    if let Err(errors) = compiled.validate(value) {
        let detail = errors
            .take(8)
            .map(|error| format!("{}: {}", error.instance_path, error))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("{kind} validation failed for {tool}: {detail}"));
    }
    Ok(())
}

/// The per-call idempotency key, from `_meta.idempotencyKey`. A present but
/// blank key is treated as absent: the client is not asking for replay.
fn idempotency_key(meta: &Meta) -> Option<String> {
    let key = meta.0.get("idempotencyKey")?.as_str()?;
    if key.trim().is_empty() {
        None
    } else {
        Some(key.to_owned())
    }
}

/// Adapt one harness tool to the MCP `tools/list` shape.
fn mcp_tool(tool: &ArtistDynamicTool) -> Tool {
    let definition = tool.definition();
    let annotations = ToolAnnotations::from_raw(
        Some(definition.title.clone()),
        Some(definition.annotations.read_only),
        Some(definition.annotations.destructive),
        Some(definition.annotations.idempotent),
        Some(definition.annotations.open_world),
    );
    let mut meta = serde_json::Map::new();
    meta.insert(
        "artist".into(),
        serde_json::json!({
            "category": definition.category,
        }),
    );
    Tool::new(
        definition.name.clone(),
        definition.description.clone(),
        rmcp::model::object(definition.input_schema.clone()),
    )
    .with_title(definition.title.clone())
    .with_raw_output_schema(Arc::new(rmcp::model::object(
        definition.output_schema.clone(),
    )))
    .with_annotations(annotations)
    .with_meta(Meta(meta))
}

/// Preserve the harness's canonical model content while publishing the exact
/// typed object promised by the tool's output schema.
fn render_output(output: ArtistToolOutput) -> CallToolResult {
    let mut result = CallToolResult::default();
    for block in output.presentation.into_content() {
        match block {
            ToolResultContent::Text(text) => result.content.push(ContentBlock::text(text.text)),
            ToolResultContent::Json { value } => {
                result.content.push(ContentBlock::text(value.to_string()));
            }
            ToolResultContent::Image(image) => match image.data {
                DocumentSourceKind::Base64(data) => {
                    let mime = image
                        .media_type
                        .as_ref()
                        .map(MimeType::to_mime_type)
                        .unwrap_or("image/png");
                    result.content.push(ContentBlock::image(data, mime));
                }
                DocumentSourceKind::Raw(data) => {
                    use base64::Engine as _;
                    let data = base64::engine::general_purpose::STANDARD.encode(data);
                    let mime = image
                        .media_type
                        .as_ref()
                        .map(MimeType::to_mime_type)
                        .unwrap_or("image/png");
                    result.content.push(ContentBlock::image(data, mime));
                }
                DocumentSourceKind::Url(url) => {
                    let mime = image
                        .media_type
                        .as_ref()
                        .map(MimeType::to_mime_type)
                        .unwrap_or("application/octet-stream");
                    result.content.push(ContentBlock::resource_link(
                        Resource::new(url, "tool-result-image")
                            .with_title("Tool result image")
                            .with_description("Image returned by the Artist tool. Fetch this URI when the client supports linked resources.")
                            .with_mime_type(mime),
                    ));
                }
                DocumentSourceKind::FileId(file_id) => {
                    result.content.push(ContentBlock::text(format!(
                        "provider file reference: {file_id} (the originating provider must resolve this identifier)"
                    )));
                }
                DocumentSourceKind::String(value) => {
                    result.content.push(ContentBlock::resource(
                        ResourceContents::text(value, "artist://tool-result/text")
                            .with_mime_type("text/plain"),
                    ));
                }
                DocumentSourceKind::Unknown => {
                    result.content.push(ContentBlock::text(
                        "image result omitted because the source was unknown",
                    ));
                }
                other => {
                    result.content.push(ContentBlock::text(format!(
                        "unsupported image source: {other:?}"
                    )));
                }
            },
        }
    }
    result.structured_content = Some(output.structured);
    result
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use serde_json::{Map, json};

    use super::*;

    fn keyed(key: &str) -> Meta {
        let mut meta = Map::new();
        meta.insert("idempotencyKey".into(), json!(key));
        Meta(meta)
    }

    fn counter_tool(calls: Arc<AtomicUsize>) -> ArtistDynamicTool {
        ArtistDynamicTool::new(
            artist_tool_api::ArtistToolDefinition {
                name: "count".into(),
                title: "Count".into(),
                description: "count executions".into(),
                input_schema: json!({"type": "object", "additionalProperties": false}),
                output_schema: artist_tool_api::text_output_schema("count", "Counter result."),
                category: artist_tool_api::ToolCategory::Administration,
                annotations: artist_tool_api::ArtistToolAnnotations::read_only(),
            },
            move |_arguments| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    Ok(artist_tool_api::ArtistToolOutput::text("done"))
                })
            },
        )
    }

    #[tokio::test]
    async fn volatile_envelope_replays_without_a_state_directory() {
        let calls = Arc::new(AtomicUsize::new(0));
        let server = McpServer::new(vec![counter_tool(Arc::clone(&calls))], None).unwrap();

        server.invoke("count", json!({}), keyed("same")).await;
        server.invoke("count", json!({}), keyed("same")).await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reusing_a_key_for_a_different_operation_is_rejected() {
        let calls = Arc::new(AtomicUsize::new(0));
        let server = McpServer::new(vec![counter_tool(Arc::clone(&calls))], None).unwrap();

        server.invoke("count", json!({}), keyed("same")).await;
        let collision = server
            .invoke("count", json!({"different": true}), keyed("same"))
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(collision.is_error, Some(true));
        assert!(
            collision.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("already used")
        );
        let record = server.envelope.get("same").unwrap();
        assert_eq!(record.tool, "count");
        assert_eq!(record.arguments, json!({}));
    }
    #[tokio::test]
    async fn invalid_input_is_rejected_before_execution() {
        let calls = Arc::new(AtomicUsize::new(0));
        let server = McpServer::new(vec![counter_tool(Arc::clone(&calls))], None).unwrap();

        let result = server
            .invoke("count", json!({"unexpected": true}), Meta::default())
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("input validation failed")
        );
    }

    #[tokio::test]
    async fn invalid_structured_output_is_rejected() {
        let tool = ArtistDynamicTool::new(
            artist_tool_api::ArtistToolDefinition {
                name: "bad_output".into(),
                title: "Bad Output".into(),
                description: "returns the wrong shape".into(),
                input_schema: json!({"type": "object", "additionalProperties": false}),
                output_schema: json!({
                    "type": "object",
                    "properties": {"count": {"type": "integer"}},
                    "required": ["count"],
                    "additionalProperties": false
                }),
                category: artist_tool_api::ToolCategory::Administration,
                annotations: artist_tool_api::ArtistToolAnnotations::read_only(),
            },
            |_| Box::pin(async { Ok(artist_tool_api::ArtistToolOutput::text("wrong")) }),
        );
        let server = McpServer::new(vec![tool], None).unwrap();

        let result = server
            .invoke("bad_output", json!({}), Meta::default())
            .await;

        assert_eq!(result.is_error, Some(true));
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("output validation failed")
        );
    }

    #[tokio::test]
    async fn operation_tool_recovers_a_keyed_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let server = McpServer::new(vec![counter_tool(Arc::clone(&calls))], None).unwrap();
        server.invoke("count", json!({}), keyed("build-42")).await;

        let result = server
            .invoke(
                "operation",
                json!({"action": "get", "key": "build-42"}),
                Meta::default(),
            )
            .await;
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["found"], true);
        assert_eq!(structured["operation"]["key"], "build-42");
        assert_eq!(structured["operation"]["tool"], "count");
    }

    #[tokio::test]
    async fn concurrent_retries_execute_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let server = McpServer::new(vec![counter_tool(Arc::clone(&calls))], None).unwrap();

        let left = server.invoke("count", json!({}), keyed("same"));
        let right = server.invoke("count", json!({}), keyed("same"));
        let (_left, _right) = tokio::join!(left, right);

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
