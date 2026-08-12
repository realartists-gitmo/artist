//! HTTP MCP transport ownership.
//!
//! MCP transport sessions are not Artist identities, but each session is bound
//! to one lazily: the first ordinary tool call that arrives without an explicit
//! transport identity mints a durable Artist name and remembers it for the
//! `Mcp-Session-Id`. Vanilla MCP clients run `initialize`, `tools/list`, and
//! `tools/call` with no identity plumbing at all. The transport-owned
//! `identity` tool remains for clients that want to reclaim a specific durable
//! name via `{"resume":"ArtistName"}`.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use artist_agent::tool_set::McpIdentity;
use rmcp::{
    ErrorData, RoleServer,
    handler::server::ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, ContentBlock, ListToolsResult, Meta,
        PaginatedRequestParams, ServerInfo, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Map, Value, json};

use crate::server::McpServer;

pub(crate) type SurfaceFactory =
    Arc<dyn Fn(Option<McpIdentity>) -> anyhow::Result<McpServer> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct HttpMcpServer {
    factory: SurfaceFactory,
    identities: artist_registry::HttpIdentities,
    tools: Arc<Vec<Tool>>,
    normal_names: Arc<HashSet<String>>,
    profile: Arc<str>,
    project: Arc<str>,
    profile_instructions: Arc<str>,
    /// Durable identity names bound to MCP transport sessions, keyed by the
    /// transport `Mcp-Session-Id`. Ordinary clients never send an identity
    /// field; the first anonymous tool call in a session binds one here and
    /// every later call in the same session reuses it.
    session_bindings: Arc<tokio::sync::RwLock<HashMap<String, String>>>,
}

impl HttpMcpServer {
    pub(crate) fn new(
        factory: SurfaceFactory,
        identities: artist_registry::HttpIdentities,
        profile: impl Into<Arc<str>>,
        project: impl Into<Arc<str>>,
        profile_instructions: impl Into<Arc<str>>,
    ) -> anyhow::Result<Self> {
        let prototype = factory(None)?;
        let mut tools = Vec::with_capacity(prototype.published_tools().len() + 1);
        tools.push(identity_tool());
        let mut normal_names = HashSet::new();
        for tool in prototype.published_tools() {
            normal_names.insert(tool.name.to_string());
            tools.push(tool);
        }
        Ok(Self {
            factory,
            identities,
            tools: Arc::new(tools),
            normal_names: Arc::new(normal_names),
            profile: profile.into(),
            project: project.into(),
            profile_instructions: profile_instructions.into(),
            session_bindings: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        })
    }

    /// Resolve an explicit transport identity name. Missing or mismatched
    /// identities are errors and never mint a replacement.
    fn resolve(&self, name: &str) -> Result<McpIdentity, ErrorData> {
        let identity = self
            .identities
            .resume(name)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("unknown or expired artistSession `{name}`; call identity without resume to create a new identity"),
                    None,
                )
            })?;
        if identity.project != self.project.as_ref() || identity.profile != self.profile.as_ref() {
            return Err(ErrorData::invalid_params(
                format!("artistSession `{name}` belongs to a different project or profile"),
                None,
            ));
        }
        Ok(McpIdentity {
            actor: identity.actor,
            profile: identity.profile,
            project: identity.project,
            name: identity.name,
        })
    }

    /// Bind `name` to the transport session, when the request belongs to one.
    async fn bind_session(&self, session: Option<&str>, name: &str) {
        if let Some(session) = session.filter(|value| !value.trim().is_empty()) {
            self.session_bindings
                .write()
                .await
                .insert(session.to_owned(), name.to_owned());
        }
    }

    /// The identity name already bound to this transport session, if any.
    async fn session_identity(&self, session: &str) -> Result<Option<String>, ErrorData> {
        Ok(self.session_bindings.read().await.get(session).cloned())
    }

    /// Mint a fresh durable identity, bound to the transport session when there
    /// is one. This is the lazy binding a vanilla anonymous call triggers.
    async fn mint_identity(&self, session: Option<&str>) -> Result<McpIdentity, ErrorData> {
        let created = self
            .identities
            .create(&self.profile, &self.project)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        self.bind_session(session, &created.name).await;
        Ok(McpIdentity {
            actor: created.actor,
            profile: created.profile,
            project: created.project,
            name: created.name,
        })
    }

    async fn establish_identity(
        &self,
        arguments: Value,
        session: Option<&str>,
    ) -> Result<CallToolResult, ErrorData> {
        let object = arguments.as_object().ok_or_else(|| {
            ErrorData::invalid_params("identity arguments must be an object", None)
        })?;
        if object.keys().any(|key| key != "resume") {
            return Err(ErrorData::invalid_params(
                "identity accepts only optional `resume`",
                None,
            ));
        }
        let identity = match object.get("resume") {
            None => self
                .identities
                .create(&self.profile, &self.project)
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?,
            Some(Value::String(name)) if !name.trim().is_empty() => self
                .identities
                .resume(name)
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?
                .ok_or_else(|| {
                    ErrorData::invalid_params(
                        format!("cannot resume missing or expired artist identity `{name}`"),
                        None,
                    )
                })?,
            Some(_) => {
                return Err(ErrorData::invalid_params(
                    "identity.resume must be a non-empty bare artist name",
                    None,
                ));
            }
        };
        if identity.project != self.project.as_ref() || identity.profile != self.profile.as_ref() {
            return Err(ErrorData::invalid_params(
                "resumed identity belongs to a different project or profile",
                None,
            ));
        }
        self.bind_session(session, &identity.name).await;
        let data = json!({
            "artistSession": identity.name,
            "profile": identity.profile,
            "project": identity.project,
        });
        let mut result = CallToolResult::success(vec![ContentBlock::text(format!(
            "artistSession: {}
You are {}, using profile {} in {}.",
            data["artistSession"].as_str().unwrap_or_default(),
            data["artistSession"].as_str().unwrap_or_default(),
            data["profile"].as_str().unwrap_or_default(),
            data["project"].as_str().unwrap_or_default(),
        ))]);
        result.structured_content = Some(data);
        Ok(result)
    }
}

impl HttpMcpServer {
    /// The shared call path. `session` is the transport `Mcp-Session-Id` the
    /// call arrived under, or `None` for a stateless request.
    async fn call_tool_for_session(
        &self,
        request: CallToolRequestParams,
        session: Option<String>,
        meta: Meta,
    ) -> Result<CallToolResult, ErrorData> {
        let name = request.name.as_ref();
        let mut arguments = request
            .arguments
            .map(Value::Object)
            .unwrap_or_else(|| Value::Object(Map::new()));
        if name == "identity" {
            return self.establish_identity(arguments, session.as_deref()).await;
        }
        if !self.normal_names.contains(name) {
            return Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            ));
        }
        let object = arguments
            .as_object_mut()
            .ok_or_else(|| ErrorData::invalid_params("tool arguments must be an object", None))?;
        // A client that already holds a durable name may keep sending it; it is
        // stripped before the underlying tool sees it. Vanilla clients never
        // send it: the transport session's lazily-bound identity is used.
        let explicit = object
            .remove("artistSession")
            .and_then(|value| value.as_str().map(str::to_owned))
            .filter(|value| !value.trim().is_empty());
        let identity = match explicit {
            Some(artist) => {
                let identity = self.resolve(&artist)?;
                self.bind_session(session.as_deref(), &identity.name).await;
                identity
            }
            None => match session.as_deref() {
                Some(session) => match self.session_identity(session).await? {
                    Some(name) => self.resolve(&name)?,
                    None => self.mint_identity(Some(session)).await?,
                },
                None => self.mint_identity(None).await?,
            },
        };
        let server = (self.factory)(Some(identity.clone()))
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        let mut result = server.invoke(name, arguments, meta).await;
        if !result.is_error.unwrap_or(false) {
            append_mail(&mut result, &identity.name)?;
        }
        Ok(result)
    }
}

impl ServerHandler for HttpMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities.tools = Some(Default::default());
        info.server_info.title = Some("Artist".into());
        info.server_info.description = Some(format!(
            "Artist MCP harness for {} using profile {}",
            self.project, self.profile
        ));
        let mut instruction_blocks = vec![
            crate::server::INSTRUCTIONS.to_owned(),
            crate::server::HTTP_INSTRUCTIONS.to_owned(),
        ];
        if !self.profile_instructions.trim().is_empty() {
            instruction_blocks.push(self.profile_instructions.to_string());
        }
        info.instructions = Some(instruction_blocks.join("\n\n"));
        info
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.tools.as_ref().clone(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.call_tool_for_session(
            request,
            transport_session_id(&context),
            context.meta.clone(),
        )
        .await
    }
}

/// The transport session id of a request, from the `Mcp-Session-Id` header the
/// stateful transport injects into the request extensions. Stateless requests
/// carry no session and return `None`.
fn transport_session_id(context: &RequestContext<RoleServer>) -> Option<String> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.headers.get("mcp-session-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty())
}

fn identity_tool() -> Tool {
    let schema = json!({
        "type":"object",
        "properties":{
            "resume":{"type":"string","description":"Existing bare artist name to resume. Omit to allocate a new durable HTTP Artist identity. Missing or mismatched identities are errors and never mint replacements."}
        },
        "additionalProperties":false
    });
    Tool::new(
        "identity",
        "Optional: create or resume the durable Artist identity for this HTTP MCP transport session. Ordinary tool calls bind an identity lazily on their own; use `resume` only to reclaim a specific durable name. Discovery and initialize are anonymous and never allocate an identity.",
        rmcp::model::object(schema),
    )
    .with_title("Identity")
    .with_annotations(ToolAnnotations::from_raw(
        Some("Identity".into()),
        Some(false),
        Some(false),
        Some(false),
        Some(false),
    ))
}

fn append_mail(result: &mut CallToolResult, recipient: &str) -> Result<(), ErrorData> {
    let messages = artist_registry::messages()
        .drain(recipient)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
    if messages.is_empty() {
        return Ok(());
    }
    let text = messages
        .iter()
        .map(|message| {
            let waiting = if message.expects_reply {
                " awaiting-reply=\"true\""
            } else {
                ""
            };
            format!(
                "<agent_message from=\"{}\"{waiting}>\n{}\n</agent_message>",
                message.from, message.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    result.content.push(ContentBlock::text(text));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use artist_tool_api::{
        ArtistDynamicTool, ArtistToolAnnotations, ArtistToolDefinition, ArtistToolOutput,
    };
    use rmcp::model::Meta;
    use serde_json::json;

    use super::*;

    fn counter_tool(calls: Arc<AtomicUsize>) -> ArtistDynamicTool {
        ArtistDynamicTool::new(
            ArtistToolDefinition {
                name: "count".into(),
                title: "Count".into(),
                description: "count executions".into(),
                input_schema: json!({"type": "object", "additionalProperties": false}),
                output_schema: artist_tool_api::text_output_schema("count", "Counter result."),
                category: artist_tool_api::ToolCategory::Administration,
                annotations: ArtistToolAnnotations::read_only(),
            },
            move |_arguments| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(ArtistToolOutput::text("done"))
                })
            },
        )
    }

    fn count_call(arguments: Value) -> CallToolRequestParams {
        CallToolRequestParams::new("count").with_arguments(
            arguments
                .as_object()
                .expect("count arguments must be an object")
                .clone(),
        )
    }

    /// A factory that records every identity it is asked to build, and serves a
    /// trivial counter tool so a call can be observed end to end. The `None`
    /// prototype probe used to size the published surface is not recorded.
    fn recording_factory(seen: Arc<Mutex<Vec<String>>>, calls: Arc<AtomicUsize>) -> SurfaceFactory {
        Arc::new(move |identity: Option<McpIdentity>| {
            if let Some(identity) = identity {
                seen.lock().unwrap().push(identity.name.clone());
            }
            McpServer::new(vec![counter_tool(Arc::clone(&calls))], None)
        })
    }

    fn http_server(factory: SurfaceFactory, root: &tempfile::TempDir) -> HttpMcpServer {
        let registry = artist_registry::Registry::for_project(root.path());
        let identities = registry
            .http_identities()
            .with_names(registry.names_for_test());
        HttpMcpServer::new(
            factory,
            identities,
            "worker",
            root.path().display().to_string(),
            "",
        )
        .unwrap()
    }

    #[test]
    fn anonymous_http_instructions_end_with_profile_and_have_no_identity() {
        let root = tempfile::tempdir().unwrap();
        let registry = artist_registry::Registry::for_project(root.path());
        let factory: SurfaceFactory = Arc::new(|_| McpServer::new(Vec::new(), None));
        let server = HttpMcpServer::new(
            factory,
            registry
                .http_identities()
                .with_names(registry.names_for_test()),
            "worker",
            root.path().display().to_string(),
            "PROFILE RULES",
        )
        .unwrap();
        let instructions = server.get_info().instructions.unwrap();
        assert!(instructions.starts_with(crate::server::INSTRUCTIONS));
        assert!(instructions.contains(crate::server::HTTP_INSTRUCTIONS));
        assert!(instructions.ends_with("PROFILE RULES"));
        assert!(!instructions.contains("using profile worker in"));
        assert!(!instructions.contains("Other agents and the user address you only as"));
    }

    #[test]
    fn mail_is_not_consumed_by_a_failed_result() {
        let recipient = format!("mail-test-{}", std::process::id());
        artist_registry::messages()
            .send(&artist_registry::Message {
                id: artist_tools::short_id("m"),
                from: "Monet".into(),
                to: recipient.clone(),
                audience: artist_registry::Audience::Direct,
                body: "keep me".into(),
                expects_reply: false,
                sent_at: artist_registry::now(),
            })
            .unwrap();
        let mut failure = CallToolResult::error(vec![ContentBlock::text("failed")]);
        if !failure.is_error.unwrap_or(false) {
            append_mail(&mut failure, &recipient).unwrap();
        }
        let waiting = artist_registry::messages().drain(&recipient).unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].body, "keep me");
    }

    #[tokio::test]
    async fn identity_rejects_null_resume_instead_of_treating_it_as_create() {
        let root = tempfile::tempdir().unwrap();
        let factory: SurfaceFactory = Arc::new(|_| McpServer::new(Vec::new(), None));
        let server = http_server(factory, &root);
        assert!(
            server
                .establish_identity(json!({"resume": null}), None)
                .await
                .is_err()
        );
    }

    #[test]
    fn identity_schema_creates_on_empty_object_and_only_exposes_optional_resume() {
        let tool = identity_tool();
        let properties = tool.input_schema["properties"].as_object().unwrap();
        assert_eq!(properties.len(), 1);
        assert!(properties.get("resume").is_some());
        assert!(properties.get("start").is_none());
        assert!(tool.input_schema.get("required").is_none());
        assert!(tool.input_schema.get("oneOf").is_none());
    }

    #[test]
    fn ordinary_tool_schemas_no_longer_advertise_transport_identity() {
        let root = tempfile::tempdir().unwrap();
        let factory: SurfaceFactory =
            Arc::new(|_| McpServer::new(vec![counter_tool(Arc::new(AtomicUsize::new(0)))], None));
        let server = http_server(factory, &root);

        let count = server.get_tool("count").expect("count is advertised");
        let properties = count
            .input_schema
            .get("properties")
            .and_then(Value::as_object);
        assert!(
            properties.is_none() || !properties.unwrap().contains_key("artistSession"),
            "ordinary tool schemas must not expose artistSession"
        );
        let required = count
            .input_schema
            .get("required")
            .cloned()
            .unwrap_or_else(|| json!([]));
        assert!(
            !required
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "artistSession"),
            "ordinary tools must not require the transport identity"
        );
        assert!(
            server.get_tool("identity").is_some(),
            "the transport identity tool is still advertised"
        );
    }

    #[tokio::test]
    async fn vanilla_session_binds_one_identity_and_reuses_it() {
        let root = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let server = http_server(
            recording_factory(Arc::clone(&seen), Arc::clone(&calls)),
            &root,
        );

        // Two vanilla tools/call requests in the same transport session, neither
        // carrying any identity plumbing.
        server
            .call_tool_for_session(count_call(json!({})), Some("s1".into()), Meta::default())
            .await
            .expect("first anonymous call");
        server
            .call_tool_for_session(count_call(json!({})), Some("s1".into()), Meta::default())
            .await
            .expect("second anonymous call");

        assert_eq!(calls.load(Ordering::SeqCst), 2, "both calls executed");
        let recorded = seen.lock().unwrap();
        assert_eq!(
            recorded.len(),
            2,
            "exactly one identity was minted for the session"
        );
        assert_eq!(
            recorded[0], recorded[1],
            "the second call reuses the bound identity"
        );
        let first = recorded[0].clone();
        drop(recorded);

        // A different transport session mints a distinct identity.
        server
            .call_tool_for_session(count_call(json!({})), Some("s2".into()), Meta::default())
            .await
            .expect("second session call");
        let recorded = seen.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        assert_ne!(recorded[2], first, "sessions never share an identity");
    }

    #[tokio::test]
    async fn explicit_resume_binds_the_transport_session_and_lazy_calls_follow() {
        let root = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let server = http_server(
            recording_factory(Arc::clone(&seen), Arc::clone(&calls)),
            &root,
        );

        let created = server
            .establish_identity(json!({}), Some("s1"))
            .await
            .expect("create identity");
        let name = created.structured_content.as_ref().unwrap()["artistSession"]
            .as_str()
            .expect("artistSession")
            .to_owned();

        // A vanilla call after the explicit identity follows the resumed name
        // instead of minting a replacement.
        server
            .call_tool_for_session(count_call(json!({})), Some("s1".into()), Meta::default())
            .await
            .expect("anonymous call in resumed session");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "no replacement identity was minted");
        assert_eq!(
            seen[0], name,
            "the session stays bound to the resumed identity"
        );
    }

    #[tokio::test]
    async fn explicit_transport_identity_is_still_honored_and_stripped() {
        let root = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let server = http_server(
            recording_factory(Arc::clone(&seen), Arc::clone(&calls)),
            &root,
        );

        let created = server
            .establish_identity(json!({}), None)
            .await
            .expect("create identity");
        let name = created.structured_content.as_ref().unwrap()["artistSession"]
            .as_str()
            .expect("artistSession")
            .to_owned();

        // Passing the transport identity explicitly still resolves it and wins.
        server
            .call_tool_for_session(
                count_call(json!({"artistSession": name})),
                Some("s1".into()),
                Meta::default(),
            )
            .await
            .expect("explicit identity call");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0], name, "the explicit identity wins");
    }
}
