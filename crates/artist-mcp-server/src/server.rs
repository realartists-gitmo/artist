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

use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
};

use rig_core::tool::{PortableDynamicTool, ToolOutput};
use rmcp::{
    ErrorData, RoleServer,
    handler::server::ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, ContentBlock, ListToolsResult, Meta,
        PaginatedRequestParams, ServerInfo, Tool,
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
    tools: Vec<PortableDynamicTool>,
    by_name: Arc<HashMap<String, PortableDynamicTool>>,
    envelope: Envelope,
}

impl McpServer {
    /// Wrap a tool surface (from [`artist_agent::tool_set::mcp_surface`]) as
    /// an MCP server. `state_dir` is where the durable envelope lives; `None`
    /// makes calls volatile across restarts but still replay-safe in-process.
    pub fn new(tools: Vec<PortableDynamicTool>, state_dir: Option<&Path>) -> anyhow::Result<Self> {
        let by_name = tools
            .iter()
            .map(|tool| (tool.name().to_owned(), tool.clone()))
            .collect();
        Ok(Self {
            tools,
            by_name: Arc::new(by_name),
            envelope: Envelope::open(state_dir)?,
        })
    }

    /// The published tool names, in surface order.
    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|tool| tool.name().to_owned()).collect()
    }

    /// Invoke a tool by name, running the same path an MCP `tools/call` takes —
    /// envelope replay, execution, commit — without a transport request context.
    ///
    /// The canvas bridge dispatches through here so a page cannot disagree with
    /// the model about what a tool does: it sees exactly what `call_tool` sees.
    pub async fn invoke(&self, name: &str, arguments: serde_json::Value, meta: Meta) -> CallToolResult {
        let arguments = if arguments.is_object() {
            arguments
        } else {
            serde_json::Value::Object(Default::default())
        };
        if let Some(key) = idempotency_key(&meta) {
            if let Some(replayed) = self.envelope.replay(&key) {
                return serde_json::from_value(replayed).unwrap_or(CallToolResult::error(vec![
                    ContentBlock::text("corrupt envelope record"),
                ]));
            }
            let result = self.execute(name, arguments).await;
            if let Ok(encoded) = serde_json::to_value(&result) {
                if let Err(error) = self.envelope.commit(&key, name, &arguments, &encoded) {
                    tracing::warn!("envelope commit failed: {error:#}");
                }
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
        match tool.execute(arguments).await {
            Ok(output) => render_output(output),
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
        info.instructions = Some(INSTRUCTIONS.into());
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
fn mcp_tool(tool: &PortableDynamicTool) -> Tool {
    let definition = tool.definition();
    Tool::new(
        definition.name.clone(),
        definition.description.clone(),
        rmcp::model::object(definition.parameters),
    )
}

/// Render a harness tool output as MCP content: the model-visible text, plus
/// the raw JSON as `structuredContent` when the tool produced one.
fn render_output(output: ToolOutput) -> CallToolResult {
    let mut result = CallToolResult::default();
    result.content.push(ContentBlock::text(output.render()));
    if let Some(json) = output.as_json() {
        result.structured_content = Some(json.clone());
    }
    result
}
