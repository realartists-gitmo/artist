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

use std::{collections::HashMap, path::Path, sync::Arc, time::Instant};

use artist_tool_api::{
    ArtistDynamicTool, ArtistToolOutput, FieldError, NextAction, ResultMeta, ToolCallContext,
    failure_envelope, failure_parts, set_page, set_result_meta,
};
use rig_core::{
    completion::message::{DocumentSourceKind, MimeType, ToolResultContent},
    tool::{ToolErrorKind, ToolExecutionError, ToolOutput},
};
use rmcp::{
    ErrorData, RoleServer,
    handler::server::ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, ContentBlock, ListToolsResult, Meta,
        PaginatedRequestParams, Resource, ResourceContents, ServerInfo, Tool, ToolAnnotations,
    },
    service::RequestContext,
};

use crate::{envelope::Envelope, pagination::PageStore, progress::ProgressSession};

/// What ChatGPT's web agent is told about how these tools behave. The
/// non-obvious parts are the handle-then-poll shape of long-running tools and
/// the idempotency contract that makes a tunnel reconnect safe.
pub(crate) const INSTRUCTIONS: &str = "\
You are driving the Artist harness over MCP. Tool schemas are authoritative. Long-running work returns durable session identifiers; use `poll`, `send`, `abort`, and `list` rather than tool-specific lifecycle modes. `bash` and `subagent` are spawn-only. `ask` is non-blocking and returns an `ask:<slug>` session. `canvas` and `skill` are one-query search-first tools. `computer` uses one shallow action/session/args envelope and injects its advanced action reference on first use. Internal actor/process identifiers are never user-facing.\n\nEvery call carrying `_meta.idempotencyKey` is deduplicated server-side; reuse a key only to recover the same operation.";

pub(crate) const HTTP_INSTRUCTIONS: &str = "\
HTTP MCP discovery is anonymous. An Artist identity is bound to your MCP transport session the first time an ordinary tool call arrives, so no setup is required. To reclaim a specific durable name, call the transport-owned `identity` tool with `{\"resume\":\"ArtistName\"}`; the resumed identity then stays bound to the session. A missing resume target is an error and never creates a replacement identity. MCP transport session ids and clientInfo are not Artist identities.\n\nLong-running work uses the universal `poll`, `send`, `abort`, and `list` tools. Mail addressed to this Artist is appended only to ordinary HTTP tool-call results.";

/// The MCP server for one project.
#[derive(Clone)]
pub struct McpServer {
    tools: Vec<ArtistDynamicTool>,
    by_name: Arc<HashMap<String, ArtistDynamicTool>>,
    identity: artist_agent::tool_set::McpIdentity,
    profile_instructions: Arc<str>,
    envelope: Envelope,
    pages: PageStore,
    /// Serializes keyed operations across HTTP sessions so two simultaneous
    /// retries cannot both pass the replay check and execute the same effect.
    idempotency_gate: Arc<tokio::sync::Mutex<()>>,
}

impl McpServer {
    /// Wrap a tool surface (from [`artist_agent::tool_set::mcp_surface`]) as
    /// an MCP server. `state_dir` is where the durable envelope lives; `None`
    /// makes calls volatile across restarts but still replay-safe in-process.
    pub fn new(tools: Vec<ArtistDynamicTool>, state_dir: Option<&Path>) -> anyhow::Result<Self> {
        Self::with_identity_profile_and_visibility(
            tools,
            state_dir,
            artist_agent::tool_set::McpIdentity {
                actor: "mcp".into(),
                profile: "worker".into(),
                project: "unknown".into(),
                name: "mcp".into(),
            },
            "",
            true,
            true,
        )
    }

    pub fn with_identity(
        tools: Vec<ArtistDynamicTool>,
        state_dir: Option<&Path>,
        identity: artist_agent::tool_set::McpIdentity,
    ) -> anyhow::Result<Self> {
        Self::with_identity_profile_and_visibility(tools, state_dir, identity, "", true, true)
    }

    #[cfg(test)]
    pub(crate) fn with_identity_and_profile(
        tools: Vec<ArtistDynamicTool>,
        state_dir: Option<&Path>,
        identity: artist_agent::tool_set::McpIdentity,
        profile_instructions: impl Into<Arc<str>>,
    ) -> anyhow::Result<Self> {
        Self::with_identity_profile_and_visibility(
            tools,
            state_dir,
            identity,
            profile_instructions,
            true,
            true,
        )
    }

    pub(crate) fn with_identity_profile_and_visibility(
        mut tools: Vec<ArtistDynamicTool>,
        state_dir: Option<&Path>,
        identity: artist_agent::tool_set::McpIdentity,
        profile_instructions: impl Into<Arc<str>>,
        permit_operation: bool,
        permit_page: bool,
    ) -> anyhow::Result<Self> {
        let envelope = Envelope::open(state_dir)?;
        let pages = PageStore::open(state_dir)?;
        anyhow::ensure!(
            !tools.iter().any(|tool| tool.name() == "operation"),
            "tool surface already defines the reserved operation tool"
        );
        for tool in &tools {
            validate_annotations(tool)?;
        }
        tools.retain(|tool| permit_page || tool.name() != "page");
        if permit_operation {
            tools.push(crate::admin::operation_tool(envelope.clone()));
        }
        if permit_page && !tools.iter().any(|tool| tool.name() == "page") {
            tools.push(crate::admin::page_tool(pages.clone()));
        }
        let by_name = tools
            .iter()
            .map(|tool| (tool.name().to_owned(), tool.clone()))
            .collect();
        Ok(Self {
            tools,
            by_name: Arc::new(by_name),
            identity,
            profile_instructions: profile_instructions.into(),
            envelope,
            pages,
            idempotency_gate: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    #[cfg(test)]
    pub(crate) fn identity(&self) -> &artist_agent::tool_set::McpIdentity {
        &self.identity
    }
    /// The published tool names, in surface order.
    /// Published MCP tools in surface order. Used by the HTTP transport wrapper,
    /// which adds transport-owned identity fields without changing the underlying
    /// tool contract seen by stdio or canvas-internal dispatch.
    pub(crate) fn published_tools(&self) -> Vec<Tool> {
        self.tools.iter().map(mcp_tool).collect()
    }

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
        let operation_id = operation_id();
        self.invoke_with_context(
            name,
            arguments,
            meta,
            ToolCallContext {
                operation_id: Some(operation_id),
                progress: Default::default(),
            },
        )
        .await
    }

    async fn invoke_with_context(
        &self,
        name: &str,
        arguments: serde_json::Value,
        meta: Meta,
        context: ToolCallContext,
    ) -> CallToolResult {
        let arguments = if arguments.is_object() {
            arguments
        } else {
            serde_json::Value::Object(Default::default())
        };
        if let Some(key) = idempotency_key(&meta) {
            let _guard = self.idempotency_gate.lock().await;
            if let Some(record) = self.envelope.get(&key) {
                if record.tool != name || record.arguments != arguments {
                    let error = ToolExecutionError::other(format!(
                        "idempotency key {key:?} was already used for {} with different arguments",
                        record.tool
                    ))
                    .with_code("idempotency_conflict")
                    .with_retryable(false);
                    return render_failure(
                        &error,
                        vec![NextAction::UseNewIdempotencyKey],
                        result_meta(&context, None),
                        Vec::new(),
                    );
                }
                return serde_json::from_value(record.result).unwrap_or_else(|_| {
                    let error = ToolExecutionError::other("corrupt envelope record")
                        .with_code("operation_corrupt")
                        .with_retryable(false);
                    render_failure(&error, Vec::new(), result_meta(&context, None), Vec::new())
                });
            }
            let result = self
                .execute(name, arguments.clone(), context, Some(&key))
                .await;
            if let Ok(encoded) = serde_json::to_value(&result) {
                if let Err(error) = self.envelope.commit(&key, name, &arguments, &encoded) {
                    tracing::warn!("envelope commit failed: {error:#}");
                }
            }
            return result;
        }
        self.execute(name, arguments, context, None).await
    }

    /// A transport-neutral dispatcher safe to retain inside the canvas host.
    /// The `canvas` tool itself is excluded so this clone cannot contain the
    /// `CanvasTool -> Lazy -> McpCanvasHost -> McpServer` reference cycle.
    pub(crate) fn canvas_dispatcher(&self) -> Self {
        let tools = self
            .tools
            .iter()
            .filter(|tool| tool.name() != "canvas")
            .cloned()
            .collect::<Vec<_>>();
        let by_name = tools
            .iter()
            .map(|tool| (tool.name().to_owned(), tool.clone()))
            .collect();
        Self {
            tools,
            by_name: Arc::new(by_name),
            identity: self.identity.clone(),
            profile_instructions: Arc::clone(&self.profile_instructions),
            envelope: self.envelope.clone(),
            pages: self.pages.clone(),
            idempotency_gate: Arc::clone(&self.idempotency_gate),
        }
    }

    async fn execute(
        &self,
        name: &str,
        arguments: serde_json::Value,
        context: ToolCallContext,
        recovery_key: Option<&str>,
    ) -> CallToolResult {
        let started = Instant::now();
        let Some(tool) = self.by_name.get(name) else {
            let error = ToolExecutionError::not_found(format!("unknown tool: {name}"));
            return render_failure(
                &error,
                Vec::new(),
                result_meta(&context, Some(started.elapsed())),
                Vec::new(),
            );
        };
        // Computer owns top-level validation so a malformed first invocation still
        // reaches the wrapper that appends its mandatory one-time reference. Every
        // other tool remains transport-validated before execution.
        if name != "computer" {
            if let Err(validation) =
                validate_schema(&tool.definition().input_schema, &arguments, "input", name)
            {
                let error = ToolExecutionError::invalid_args(validation.message.clone())
                    .with_code("input_validation_failed")
                    .with_retryable(false);
                return render_failure(
                    &error,
                    recovery_actions(name, &arguments, &error, recovery_key),
                    result_meta(&context, Some(started.elapsed())),
                    validation.field_errors,
                );
            }
        }

        let operation_id = context.operation_id.clone();
        match tool.execute_with_context(arguments.clone(), context).await {
            Ok(mut output) => {
                if let Some((error, partial_data)) = promoted_failure(name, &output.structured) {
                    return render_failure_with_partial(
                        &error,
                        recovery_actions(name, &arguments, &error, recovery_key),
                        ResultMeta {
                            operation_id: operation_id.clone(),
                            duration_ms: Some(started.elapsed().as_millis() as u64),
                        },
                        Vec::new(),
                        partial_data,
                    );
                }
                set_result_meta(
                    &mut output.structured,
                    ResultMeta {
                        operation_id: operation_id.clone(),
                        duration_ms: Some(started.elapsed().as_millis() as u64),
                    },
                );
                if let Err(validation) = validate_schema(
                    &tool.definition().output_schema,
                    &output.structured,
                    "output",
                    name,
                ) {
                    tracing::error!(tool = name, error = %validation.message, "tool contract violation");
                    let error = ToolExecutionError::other(validation.message)
                        .with_code("output_contract_violation")
                        .with_retryable(false);
                    return render_failure(
                        &error,
                        Vec::new(),
                        ResultMeta {
                            operation_id: output
                                .structured
                                .get("meta")
                                .and_then(|meta| meta.get("operationId"))
                                .and_then(serde_json::Value::as_str)
                                .map(ToOwned::to_owned),
                            duration_ms: Some(started.elapsed().as_millis() as u64),
                        },
                        validation.field_errors,
                    );
                }

                if name != "page" {
                    let rendered = output.presentation.render();
                    match self.pages.paginate(name, &output.structured, &rendered) {
                        Ok(Some(page)) => {
                            output.presentation = ToolOutput::text(format!(
                                "{}\n\n[Result bounded for transport. Continue with page cursor {}.]",
                                page.preview, page.cursor
                            ));
                            set_page(&mut output.structured, page);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            return render_failure(
                                &error,
                                recovery_actions(name, &arguments, &error, recovery_key),
                                ResultMeta {
                                    operation_id: operation_id.clone(),
                                    duration_ms: Some(started.elapsed().as_millis() as u64),
                                },
                                Vec::new(),
                            );
                        }
                    }
                }

                if let Err(validation) = validate_schema(
                    &tool.definition().output_schema,
                    &output.structured,
                    "output",
                    name,
                ) {
                    let error = ToolExecutionError::other(validation.message)
                        .with_code("output_contract_violation")
                        .with_retryable(false);
                    return render_failure(
                        &error,
                        Vec::new(),
                        ResultMeta {
                            operation_id,
                            duration_ms: Some(started.elapsed().as_millis() as u64),
                        },
                        validation.field_errors,
                    );
                }
                render_output(output)
            }
            Err(error) => render_failure(
                &error,
                recovery_actions(name, &arguments, &error, recovery_key),
                ResultMeta {
                    operation_id,
                    duration_ms: Some(started.elapsed().as_millis() as u64),
                },
                Vec::new(),
            ),
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
        let mut instruction_blocks = vec![INSTRUCTIONS.to_owned()];
        if !self.profile_instructions.trim().is_empty() {
            instruction_blocks.push(self.profile_instructions.to_string());
        }
        instruction_blocks.push(format!(
            "You are {name}, using profile {profile} in {project}.",
            name = self.identity.name,
            profile = self.identity.profile,
            project = self.identity.project,
        ));
        info.instructions = Some(instruction_blocks.join("\n\n"));
        let mut meta = serde_json::Map::new();
        meta.insert(
            "artist".into(),
            serde_json::json!({"identity": {
                "name": self.identity.name,
                "profile": self.identity.profile,
                "project": self.identity.project
            }}),
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
        // already ran. The progress token and cancellation scope live on the
        // request context rather than on CallToolRequestParams.
        let operation_id = operation_id();
        let (tool_context, progress) = ProgressSession::start(
            &context.meta,
            context.peer.clone(),
            operation_id,
            name,
            &arguments,
        );
        let result = self
            .invoke_with_context(name, arguments, context.meta.clone(), tool_context)
            .await;
        if let Some(progress) = progress {
            progress
                .finish(name, !result.is_error.unwrap_or(false))
                .await;
        }
        Ok(result)
    }
}

#[derive(Debug)]
struct ValidationFailure {
    message: String,
    field_errors: Vec<FieldError>,
}

fn validate_schema(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    kind: &str,
    tool: &str,
) -> Result<(), ValidationFailure> {
    let compiled = jsonschema::JSONSchema::compile(schema).map_err(|error| ValidationFailure {
        message: format!("invalid {kind} schema for {tool}: {error}"),
        field_errors: Vec::new(),
    })?;
    if let Err(errors) = compiled.validate(value) {
        let errors = errors.take(8).collect::<Vec<_>>();
        let field_errors = errors
            .iter()
            .map(|error| FieldError {
                field: error.instance_path.to_string(),
                message: error.to_string(),
            })
            .collect::<Vec<_>>();
        let detail = field_errors
            .iter()
            .map(|error| format!("{}: {}", error.field, error.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ValidationFailure {
            message: format!("{kind} validation failed for {tool}: {detail}"),
            field_errors,
        });
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

fn validate_annotations(tool: &ArtistDynamicTool) -> anyhow::Result<()> {
    let annotations = tool.definition().annotations;
    anyhow::ensure!(
        !(annotations.read_only && annotations.destructive),
        "tool {} cannot be both read-only and destructive",
        tool.name()
    );
    anyhow::ensure!(
        !annotations.read_only || annotations.idempotent,
        "read-only tool {} must be idempotent",
        tool.name()
    );
    Ok(())
}

fn operation_id() -> String {
    format!("op_{}", uuid::Uuid::new_v4().simple())
}

fn result_meta(context: &ToolCallContext, elapsed: Option<std::time::Duration>) -> ResultMeta {
    ResultMeta {
        operation_id: context.operation_id.clone(),
        duration_ms: elapsed.map(|duration| duration.as_millis() as u64),
    }
}

fn recovery_actions(
    tool: &str,
    arguments: &serde_json::Value,
    error: &ToolExecutionError,
    recovery_key: Option<&str>,
) -> Vec<NextAction> {
    let mut actions = Vec::new();
    if error.code() == Some("idempotency_conflict") {
        actions.push(NextAction::UseNewIdempotencyKey);
        return actions;
    }
    if let Some(key) = recovery_key {
        actions.push(NextAction::RecoverOperation {
            key: key.to_owned(),
        });
    }
    match error.kind() {
        ToolErrorKind::Timeout => {
            actions.push(NextAction::Retry { after_ms: None });
        }
        ToolErrorKind::RateLimited => {
            actions.push(NextAction::Retry {
                after_ms: Some(1_000),
            });
        }
        ToolErrorKind::Network | ToolErrorKind::Provider if error.retryable() != Some(false) => {
            actions.push(NextAction::Retry { after_ms: None });
        }
        _ => {}
    }
    match error.code() {
        Some("cursor_invalid" | "cursor_stale") => {
            let mut restart = arguments.clone();
            if let Some(object) = restart.as_object_mut() {
                object.remove("cursor");
            }
            actions.push(NextAction::RestartPagination {
                tool: tool.to_owned(),
                arguments: restart,
            });
        }
        Some("capability_unavailable") => actions.push(NextAction::EnableCapability {
            capability: tool.to_owned(),
            flag: format!("--allow-{tool}"),
        }),
        _ => {}
    }
    actions
}

fn promoted_failure(
    tool: &str,
    structured: &serde_json::Value,
) -> Option<(ToolExecutionError, serde_json::Value)> {
    if tool != "bash" {
        return None;
    }
    let data = structured.get("data")?.clone();
    let status = data.get("status")?.as_str()?;
    let error = match status {
        "timed_out" => ToolExecutionError::timeout(
            "The command exceeded its foreground timeout and was terminated.",
        )
        .with_code("command_timed_out"),
        "failed" => ToolExecutionError::other(format!(
            "The command exited unsuccessfully{}.",
            data.get("exitCode")
                .and_then(serde_json::Value::as_i64)
                .map(|code| format!(" with exit code {code}"))
                .unwrap_or_default()
        ))
        .with_code("process_failed")
        .with_retryable(false),
        "superseded" => ToolExecutionError::cancelled(
            "The command was superseded by a newer coalesced operation.",
        )
        .with_code("operation_superseded")
        .with_retryable(true),
        _ => return None,
    };
    Some((error, data))
}

fn render_failure_with_partial(
    error: &ToolExecutionError,
    next_actions: Vec<NextAction>,
    meta: ResultMeta,
    field_errors: Vec<FieldError>,
    partial_data: serde_json::Value,
) -> CallToolResult {
    let mut result = render_failure(error, next_actions, meta, field_errors);
    if let Some(structured) = result.structured_content.as_mut() {
        if let Some(error) = structured
            .get_mut("error")
            .and_then(serde_json::Value::as_object_mut)
        {
            error.insert("partialData".into(), partial_data);
        }
    }
    result
}

fn render_failure(
    error: &ToolExecutionError,
    next_actions: Vec<NextAction>,
    meta: ResultMeta,
    field_errors: Vec<FieldError>,
) -> CallToolResult {
    let (mut failure, mut authored_actions) = failure_parts(error);
    failure.field_errors = field_errors;
    authored_actions.extend(next_actions);
    let structured = failure_envelope(failure, authored_actions, Some(meta));
    let mut result = CallToolResult::error(vec![ContentBlock::text(
        error
            .model_feedback()
            .unwrap_or_else(|| error.message())
            .to_owned(),
    )]);
    result.structured_content = Some(structured);
    result
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

    #[test]
    fn profile_visibility_can_hide_administrative_tools() {
        let identity = artist_agent::tool_set::McpIdentity {
            actor: "test-actor".into(),
            profile: "restricted".into(),
            project: "/project".into(),
            name: "Goethe".into(),
        };
        let hidden = McpServer::with_identity_profile_and_visibility(
            Vec::new(),
            None,
            identity.clone(),
            "",
            false,
            false,
        )
        .unwrap();
        assert!(!hidden.names().iter().any(|name| name == "operation"));
        assert!(!hidden.names().iter().any(|name| name == "page"));

        let visible = McpServer::with_identity_profile_and_visibility(
            Vec::new(),
            None,
            identity,
            "",
            true,
            true,
        )
        .unwrap();
        assert!(visible.names().iter().any(|name| name == "operation"));
        assert!(visible.names().iter().any(|name| name == "page"));
    }

    #[test]
    fn stdio_instructions_are_shared_then_profile_then_identity() {
        let server = McpServer::with_identity_and_profile(
            Vec::new(),
            None,
            artist_agent::tool_set::McpIdentity {
                actor: "a-secret".into(),
                profile: "worker".into(),
                project: "/project".into(),
                name: "Goethe".into(),
            },
            "PROFILE RULES",
        )
        .unwrap();
        let instructions = server.get_info().instructions.unwrap();
        assert!(instructions.starts_with(INSTRUCTIONS));
        assert!(
            instructions.find("PROFILE RULES").unwrap() > instructions.find(INSTRUCTIONS).unwrap()
        );
        assert!(instructions.ends_with("You are Goethe, using profile worker in /project."));
        assert!(!instructions.contains("a-secret"));
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
        let structured = collision.structured_content.as_ref().unwrap();
        assert_eq!(structured["ok"], false);
        assert_eq!(structured["error"]["code"], "idempotency_conflict");
        assert_eq!(
            structured["nextActions"][0]["kind"],
            "use_new_idempotency_key"
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
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["ok"], false);
        assert_eq!(structured["error"]["code"], "input_validation_failed");
        assert!(
            !structured["error"]["fieldErrors"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            structured["meta"]["operationId"]
                .as_str()
                .unwrap()
                .starts_with("op_")
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
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["ok"], false);
        assert_eq!(structured["error"]["code"], "output_contract_violation");
    }

    #[tokio::test]
    async fn every_success_uses_the_common_envelope() {
        let calls = Arc::new(AtomicUsize::new(0));
        let server = McpServer::new(vec![counter_tool(Arc::clone(&calls))], None).unwrap();
        let result = server.invoke("count", json!({}), Meta::default()).await;
        let structured = result.structured_content.as_ref().unwrap();
        assert_ne!(result.is_error, Some(true));
        assert_eq!(structured["ok"], true);
        assert_eq!(structured["data"]["text"], "done");
        assert!(
            structured["meta"]["operationId"]
                .as_str()
                .unwrap()
                .starts_with("op_")
        );
        assert!(structured["meta"]["durationMs"].as_u64().is_some());
    }

    #[test]
    fn every_advertised_tool_resolves_to_a_dispatcher() {
        let server =
            McpServer::new(vec![counter_tool(Arc::new(AtomicUsize::new(0)))], None).unwrap();
        let advertised = server.names();
        assert!(!advertised.is_empty());
        for name in &advertised {
            assert!(
                server.by_name.contains_key(name),
                "advertised tool {name} has no dispatcher"
            );
        }
        for name in server.by_name.keys() {
            assert!(
                advertised.iter().any(|candidate| candidate == name),
                "dispatcher {name} is not advertised"
            );
        }
        // Identity is claimed at session start, not through an `init` tool, so
        // no client or plugin metadata may expect one. Guard against the tool
        // being advertised again (the historical "unknown tool: init" drift).
        assert!(
            !advertised.iter().any(|name| name == "init"),
            "the init tool must not be advertised"
        );
    }

    #[test]
    fn invalid_annotation_combinations_are_rejected() {
        let tool = ArtistDynamicTool::new(
            artist_tool_api::ArtistToolDefinition {
                name: "unsafe_read".into(),
                title: "Unsafe Read".into(),
                description: "invalid annotation fixture".into(),
                input_schema: json!({"type":"object"}),
                output_schema: artist_tool_api::text_output_schema("unsafe_read", "fixture"),
                category: artist_tool_api::ToolCategory::Administration,
                annotations: artist_tool_api::ArtistToolAnnotations {
                    read_only: true,
                    destructive: true,
                    idempotent: true,
                    open_world: false,
                },
            },
            |_| Box::pin(async { Ok(ArtistToolOutput::text("no")) }),
        );
        let error = match McpServer::new(vec![tool], None) {
            Ok(_) => panic!("invalid annotations must be rejected"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("read-only and destructive"), "{error}");
    }

    #[tokio::test]
    async fn legacy_bash_timeout_status_never_suggests_deleted_background_arguments() {
        let tool = ArtistDynamicTool::new(
            artist_tool_api::ArtistToolDefinition {
                name: "bash".into(),
                title: "Bash".into(),
                description: "timeout fixture".into(),
                input_schema: json!({"type":"object"}),
                output_schema: artist_tool_api::schema_for::<artist_tools::BashResult>(),
                category: artist_tool_api::ToolCategory::Shell,
                annotations: artist_tool_api::ArtistToolAnnotations {
                    read_only: false,
                    destructive: true,
                    idempotent: false,
                    open_world: true,
                },
            },
            |_| {
                Box::pin(async {
                    Ok(ArtistToolOutput {
                        presentation: ToolOutput::text("timed out"),
                        structured: serde_json::to_value(artist_tools::BashResult {
                            status: artist_tools::BashStatus::TimedOut,
                            exit_code: None,
                            session_id: None,
                            stdout: "partial".into(),
                            stderr: String::new(),
                            output: "partial".into(),
                            duration_ms: Some(1_000),
                            timeout_secs: Some(1),
                            terminated_by: Some("SIGKILL".into()),
                            truncated: false,
                            retry_as_background: true,
                        })
                        .unwrap(),
                    })
                })
            },
        );
        let server = McpServer::new(vec![tool], None).unwrap();
        let result = server
            .invoke(
                "bash",
                json!({"command":"sleep 5", "timeout":1}),
                Meta::default(),
            )
            .await;
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(structured["ok"], false);
        assert_eq!(structured["error"]["code"], "command_timed_out");
        assert_eq!(structured["error"]["partialData"]["status"], "timed_out");
        let actions = structured["nextActions"].as_array().unwrap();
        assert!(actions.iter().any(|action| action["kind"] == "retry"));
        assert!(
            !actions
                .iter()
                .any(|action| action["kind"] == "start_background")
        );
    }

    #[tokio::test]
    async fn oversized_results_are_recovered_through_the_page_tool() {
        let huge = "x".repeat(crate::pagination::MAX_INLINE_RESULT_BYTES + 4096);
        let tool = ArtistDynamicTool::new(
            artist_tool_api::ArtistToolDefinition {
                name: "huge".into(),
                title: "Huge".into(),
                description: "oversized fixture".into(),
                input_schema: json!({"type":"object"}),
                output_schema: artist_tool_api::text_output_schema("huge", "huge fixture"),
                category: artist_tool_api::ToolCategory::Administration,
                annotations: artist_tool_api::ArtistToolAnnotations::read_only(),
            },
            move |_| {
                let huge = huge.clone();
                Box::pin(async move { Ok(ArtistToolOutput::text(huge)) })
            },
        );
        let state = tempfile::tempdir().unwrap();
        let server = McpServer::new(vec![tool], Some(state.path())).unwrap();
        let result = server.invoke("huge", json!({}), Meta::default()).await;
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["ok"], true);
        assert!(structured["data"].is_null());
        let cursor = structured["page"]["cursor"].as_str().unwrap();
        assert_eq!(structured["nextActions"][0]["kind"], "read_page");

        let page = server
            .invoke(
                "page",
                json!({"cursor": cursor, "maxBytes": 4096}),
                Meta::default(),
            )
            .await;
        let page = page.structured_content.as_ref().unwrap();
        assert_eq!(page["ok"], true);
        assert_eq!(page["data"]["returnedBytes"], 4096);
        assert!(page["data"]["content"].as_str().unwrap().contains('x'));
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
        assert_eq!(structured["data"]["found"], true);
        assert_eq!(structured["data"]["operation"]["key"], "build-42");
        assert_eq!(structured["data"]["operation"]["tool"], "count");
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
