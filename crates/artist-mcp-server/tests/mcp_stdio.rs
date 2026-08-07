//! End-to-end stdio tests: spawn the real `artist-mcp serve` binary and drive
//! it over MCP the way a tunnel or local client would.
//!
//! The worker profile is permissive, so each test observes the complete set of
//! tools its explicitly enabled environment can build. Optional machine, memory,
//! provider, and messaging capabilities remain absent unless configured.

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};

use rmcp::{
    ClientHandler, RoleClient, ServiceExt,
    model::{CallToolRequestParams, ProgressNotificationParam},
    transport::{StreamableHttpClientTransport, TokioChildProcess},
};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
};

#[derive(Clone, Default)]
struct ProgressClient {
    notifications: Arc<Mutex<Vec<ProgressNotificationParam>>>,
}

impl ClientHandler for ProgressClient {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.notifications
            .lock()
            .expect("progress mutex poisoned")
            .push(params);
    }
}

fn binary_available() -> bool {
    std::env::var("CARGO_BIN_EXE_artist-mcp").is_ok()
}

fn base_command(mode: &str, project: &Path, state: &Path) -> Command {
    let binary = env!("CARGO_BIN_EXE_artist-mcp");
    let mut command = Command::new(binary);
    command
        .arg(mode)
        .arg("--project")
        .arg(project)
        .arg("--state-dir")
        .arg(state)
        .arg("--profile")
        .arg("worker")
        .arg("--log")
        .arg("debug");
    command
}

async fn raw_http_get(port: u16, path: &str) -> Result<String, std::io::Error> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

/// Spawn `artist-mcp serve` against a throwaway project + state dir.
fn isolated_server(
    project: &tempfile::TempDir,
    state: &tempfile::TempDir,
) -> Result<TokioChildProcess, std::io::Error> {
    TokioChildProcess::new(base_command("serve", project.path(), state.path()))
}

fn fully_enabled_server(
    project: &tempfile::TempDir,
    state: &tempfile::TempDir,
    config: &tempfile::TempDir,
) -> Result<TokioChildProcess, std::io::Error> {
    let mut command = base_command("serve", project.path(), state.path());
    command
        .arg("--actor")
        .arg("mcp-e2e")
        .arg("--allow-computer")
        .arg("--allow-canvas")
        .arg("--allow-memory")
        .arg("--allow-subagent")
        .arg("--allow-comms")
        .env("ARTIST_CONFIG_DIR", config.path())
        .env("ARTIST_STATE_DIR", state.path().join("registry"));
    TokioChildProcess::new(command)
}

fn call(name: &str, args: Value) -> CallToolRequestParams {
    let arguments = args
        .as_object()
        .expect("tool arguments must be an object")
        .clone();
    CallToolRequestParams::new(name.to_owned()).with_arguments(arguments)
}

fn with_idempotency_key(mut params: CallToolRequestParams, key: &str) -> CallToolRequestParams {
    let mut meta = Map::new();
    meta.insert("idempotencyKey".into(), json!(key));
    params.meta = Some(rmcp::model::Meta(meta));
    params
}

fn text_content(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            rmcp::model::ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn assert_output_schemas(tools: &[rmcp::model::Tool]) {
    for tool in tools {
        let schema = tool
            .output_schema
            .as_ref()
            .unwrap_or_else(|| panic!("{} has no outputSchema", tool.name));
        assert_eq!(
            schema.get("x-artist-envelope"),
            Some(&json!(true)),
            "{} does not publish the common Artist result envelope: {schema:?}",
            tool.name
        );
        assert!(
            schema.get("oneOf").and_then(Value::as_array).is_some(),
            "{} has no success/failure union: {schema:?}",
            tool.name
        );
        let annotations = tool
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("{} has no annotations", tool.name));
        assert!(
            annotations.read_only_hint.is_some(),
            "{} lacks readOnlyHint",
            tool.name
        );
        assert!(
            annotations.destructive_hint.is_some(),
            "{} lacks destructiveHint",
            tool.name
        );
        assert!(
            annotations.idempotent_hint.is_some(),
            "{} lacks idempotentHint",
            tool.name
        );
        assert!(
            annotations.open_world_hint.is_some(),
            "{} lacks openWorldHint",
            tool.name
        );
        assert!(
            !(annotations.read_only_hint == Some(true)
                && annotations.destructive_hint == Some(true)),
            "{} is both read-only and destructive",
            tool.name
        );
    }

    let annotation = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("missing annotation fixture {name}"))
            .annotations
            .clone()
            .expect("annotations")
    };
    let read = annotation("read");
    assert_eq!(read.read_only_hint, Some(true));
    assert_eq!(read.destructive_hint, Some(false));
    assert_eq!(read.idempotent_hint, Some(true));
    assert_eq!(read.open_world_hint, Some(false));

    let bash = annotation("bash");
    assert_eq!(bash.read_only_hint, Some(false));
    assert_eq!(bash.destructive_hint, Some(true));
    assert_eq!(bash.idempotent_hint, Some(false));
    assert_eq!(bash.open_world_hint, Some(true));

    for name in ["operation", "page"] {
        let admin = annotation(name);
        assert_eq!(admin.read_only_hint, Some(true));
        assert_eq!(admin.destructive_hint, Some(false));
        assert_eq!(admin.idempotent_hint, Some(true));
        assert_eq!(admin.open_world_hint, Some(false));
    }
}

#[tokio::test]
async fn publishes_the_worker_surface_and_runs_a_tool() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;
    let tools = client.list_all_tools().await?;
    assert_output_schemas(&tools);
    let names: BTreeSet<String> = tools.iter().map(|tool| tool.name.to_string()).collect();

    // The headless worker surface: the structural + file + shell core is
    // present, the display/index/messaging extras are absent by availability.
    for expected in [
        "bash",
        "read",
        "find",
        "grep",
        "edit",
        "write",
        "todo",
        "code_map",
        "code_show",
        "ast_query",
        "ask",
        "ask_result",
        "ask_answer",
        "ask_list",
        "operation",
        "page",
    ] {
        assert!(names.contains(expected), "missing {expected}");
    }
    for absent in ["computer", "canvas", "memory", "subagent", "handoff", "gc"] {
        assert!(!names.contains(absent), "{absent} must be absent headless");
    }

    let result = client
        .call_tool(call(
            "bash",
            json!({"mode": "exec", "command": "printf 'hello-mcp\\n'"}),
        ))
        .await?;
    let output = text_content(&result);
    assert!(output.contains("hello-mcp"), "got: {output}");
    let structured = result
        .structured_content
        .as_ref()
        .expect("structured result");
    assert_eq!(structured["ok"], true);
    assert_eq!(structured["data"]["status"], "completed");
    assert!(
        structured["meta"]["operationId"]
            .as_str()
            .unwrap()
            .starts_with("op_")
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn an_idempotency_key_replays_the_stored_result() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;

    // A side-effecting command: appending to a file. If the second call with
    // the same key re-executed, the file would have two lines.
    let script = "mkdir -p .e2e && echo once >> .e2e/idempotent.txt";
    let first = client
        .call_tool(with_idempotency_key(
            call("bash", json!({"mode": "exec", "command": script})),
            "test-idi-1",
        ))
        .await?;
    assert!(
        text_content(&first).contains("status: completed"),
        "bash failed"
    );

    let second = client
        .call_tool(with_idempotency_key(
            call("bash", json!({"mode": "exec", "command": script})),
            "test-idi-1",
        ))
        .await?;
    assert!(
        text_content(&second).contains("status: completed"),
        "bash failed"
    );

    let lines = client
        .call_tool(call(
            "bash",
            json!({"mode": "exec", "command": "wc -l < .e2e/idempotent.txt"}),
        ))
        .await?;
    assert!(
        text_content(&lines).contains("1"),
        "same key re-ran the command: got {}",
        text_content(&lines)
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn a_blank_idempotency_key_is_ignored() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;

    let script = "mkdir -p .e2e && echo blank >> .e2e/blank.txt";
    for _ in 0..2 {
        client
            .call_tool(with_idempotency_key(
                call("bash", json!({"mode": "exec", "command": script})),
                "   ",
            ))
            .await?;
    }

    let lines = client
        .call_tool(call(
            "bash",
            json!({"mode": "exec", "command": "wc -l < .e2e/blank.txt"}),
        ))
        .await?;
    assert!(
        text_content(&lines).contains("2"),
        "blank key must not dedup: got {}",
        text_content(&lines)
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn invalid_arguments_return_a_structured_recovery_failure()
-> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;
    let result = client
        .call_tool(call(
            "read",
            json!({"path": "Cargo.toml", "unexpected": true}),
        ))
        .await?;
    assert_eq!(result.is_error, Some(true));
    let structured = result
        .structured_content
        .as_ref()
        .expect("structured failure");
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
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn oversized_results_continue_through_the_public_page_tool()
-> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;
    let result = client
        .call_tool(call(
            "bash",
            json!({
                "mode": "exec",
                "command": "python3 -c 'print(\"x\" * 90000)'",
                "maxBytes": 200000
            }),
        ))
        .await?;
    let structured = result.structured_content.as_ref().expect("paged result");
    assert_eq!(structured["ok"], true);
    assert!(structured["data"].is_null());
    let cursor = structured["page"]["cursor"].as_str().expect("page cursor");
    assert_eq!(structured["nextActions"][0]["kind"], "read_page");

    let page = client
        .call_tool(call("page", json!({"cursor": cursor, "maxBytes": 4096})))
        .await?;
    let page = page.structured_content.as_ref().expect("page result");
    assert_eq!(page["ok"], true);
    assert_eq!(page["data"]["returnedBytes"], 4096);
    assert!(page["data"]["content"].as_str().unwrap().contains('x'));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn tool_calls_emit_monotonic_progress_when_the_client_supplies_a_token()
-> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let handler = ProgressClient::default();
    let notifications = Arc::clone(&handler.notifications);
    let client = handler.serve(isolated_server(&project, &state)?).await?;
    let result = client
        .call_tool(call(
            "bash",
            json!({"mode": "exec", "command": "printf progress-ok"}),
        ))
        .await?;
    assert!(text_content(&result).contains("progress-ok"));
    let events = notifications
        .lock()
        .expect("progress mutex poisoned")
        .clone();
    assert!(
        events.len() >= 4,
        "expected start, tool phases, and completion: {events:?}"
    );
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].progress < pair[1].progress),
        "progress was not monotonic: {events:?}"
    );
    assert!(
        events
            .first()
            .and_then(|event| event.message.as_deref())
            .unwrap_or("")
            .contains("Started")
    );
    assert!(
        events
            .last()
            .and_then(|event| event.message.as_deref())
            .unwrap_or("")
            .contains("completed")
    );
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn unknown_tool_is_an_invalid_params_error() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;

    let error = client.call_tool(call("no_such_tool", json!({}))).await;
    let error = error.expect_err("unknown tool must error");
    let message = error.to_string();
    assert!(message.contains("unknown tool"), "got: {message}");

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn ask_posts_then_polls_then_answers() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;

    // Post a question durably. The tool must return immediately with an id,
    // not block on an in-process answerer that does not exist over MCP.
    let posted = client
        .call_tool(call(
            "ask",
            json!({"questions": [{
                "question": "Where should canvases live?",
                "header": "Storage",
                "options": [
                    {"label": "Project-local", "description": "Durable across sessions"},
                    {"label": "Ephemeral", "description": "Tied to one conversation"}
                ]
            }]}),
        ))
        .await?;
    let posted_text = text_content(&posted);
    assert!(posted_text.contains("posted q-"), "got: {posted_text}");
    assert!(
        posted_text.contains("awaiting answer"),
        "got: {posted_text}"
    );

    // Poll while unanswered: still pending.
    let id = posted_text
        .split("posted ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .expect("question id in output");
    let pending = client
        .call_tool(call("ask_result", json!({"questionIds": [id]})))
        .await?;
    let pending_text = text_content(&pending);
    assert!(
        pending_text.contains("\"status\":\"pending\""),
        "got: {pending_text}"
    );

    // The model relays the user's answer.
    let answered = client
        .call_tool(call(
            "ask_answer",
            json!({"questionId": id, "selected": ["Project-local"]}),
        ))
        .await?;
    let answered_text = text_content(&answered);
    assert!(
        answered_text.contains("Project-local"),
        "got: {answered_text}"
    );

    // Poll again: answered now.
    let polled = client
        .call_tool(call("ask_result", json!({"questionIds": [id]})))
        .await?;
    let polled_text = text_content(&polled);
    assert!(
        polled_text.contains("\"status\":\"answered\"") && polled_text.contains("Project-local"),
        "got: {polled_text}"
    );

    // ask_list is empty now.
    let list = client.call_tool(call("ask_list", json!({}))).await?;
    assert!(
        text_content(&list).contains("\"pending\":[]"),
        "got: {}",
        text_content(&list)
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn a_posted_question_survives_connection_death() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;

    // First connection: post a question, then let the process die.
    let id = {
        let client = ().serve(isolated_server(&project, &state)?).await?;
        let posted = client
            .call_tool(call(
                "ask",
                json!({"questions": [{
                    "question": "Which provider?",
                    "header": "Provider",
                    "options": [
                        {"label": "OpenAI", "description": "Default"},
                        {"label": "Anthropic", "description": "Claude"}
                    ]
                }]}),
            ))
            .await?;
        let posted_text = text_content(&posted);
        let id = posted_text
            .split("posted ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .expect("question id in output");
        client.cancel().await?;
        id.to_owned()
    };

    // A fresh connection on the same state dir must still see the question.
    let client = ().serve(isolated_server(&project, &state)?).await?;
    let list = client.call_tool(call("ask_list", json!({}))).await?;
    let list_text = text_content(&list);
    assert!(
        list_text.contains(&format!("\"id\":\"{id}\"")) && list_text.contains("Which provider?"),
        "question lost across connection death: got {list_text}"
    );

    let polled = client
        .call_tool(call("ask_result", json!({"questionIds": [id]})))
        .await?;
    let polled_text = text_content(&polled);
    assert!(
        polled_text.contains("\"status\":\"pending\"") && polled_text.contains("Which provider?"),
        "expected pending with the wording: got {polled_text}"
    );

    // And a fresh connection can still record the answer.
    let answered = client
        .call_tool(call(
            "ask_answer",
            json!({"questionId": id, "selected": ["OpenAI"]}),
        ))
        .await?;
    assert!(
        text_content(&answered).contains("OpenAI"),
        "got: {}",
        text_content(&answered)
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn publishes_every_enabled_harness_surface() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let config = tempfile::tempdir()?;
    std::fs::write(
        config.path().join("settings.toml"),
        "[memory]\nenabled = true\nmodel_dir = 'missing-test-model'\ndim = 8\n",
    )?;
    std::fs::write(
        config.path().join("providers.toml"),
        r#"version = 4
default_provider = "test"
[[providers]]
id = "test"
name = "Test"
provider = "openai"
base_url = "https://example.invalid/v1/"
model = "test-model"
[providers.credentials]
type = "api_key"
api_key = "not-used"
"#,
    )?;

    let client = ().serve(fully_enabled_server(&project, &state, &config)?).await?;
    let tools = client.list_all_tools().await?;
    assert_output_schemas(&tools);
    let names: BTreeSet<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
    for expected in [
        "computer",
        "canvas",
        "memory",
        "code_search",
        "code_related",
        "subagent",
        "tell",
        "query",
        "reply",
        "gc",
    ] {
        assert!(
            names.contains(expected),
            "missing enabled tool {expected}: {names:?}"
        );
    }
    assert!(
        !names.contains("handoff"),
        "handoff is meaningless over MCP"
    );

    let info = client.peer_info().expect("server initialization metadata");
    let instructions = info.instructions.as_deref().expect("identity instructions");
    assert!(
        instructions.contains("Artist actor mcp-e2e"),
        "{instructions}"
    );
    assert!(instructions.contains("Other agents and the user address you as"));
    let identity = &info.meta.as_ref().expect("server metadata").0["artist"]["identity"];
    assert_eq!(identity["actor"], "mcp-e2e");
    assert_eq!(identity["profile"], "worker");
    assert_eq!(identity["registered"], true);

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn simultaneous_idempotent_calls_execute_once() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;
    let script = "mkdir -p .e2e; sleep 0.1; echo once >> .e2e/concurrent.txt";
    let left = client.call_tool(with_idempotency_key(
        call("bash", json!({"mode": "exec", "command": script})),
        "concurrent-key",
    ));
    let right = client.call_tool(with_idempotency_key(
        call("bash", json!({"mode": "exec", "command": script})),
        "concurrent-key",
    ));
    let (left, right) = tokio::join!(left, right);
    left?;
    right?;

    let lines = client
        .call_tool(call(
            "bash",
            json!({"mode": "exec", "command": "wc -l < .e2e/concurrent.txt"}),
        ))
        .await?;
    assert!(
        text_content(&lines).contains("1"),
        "got: {}",
        text_content(&lines)
    );
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn streamable_http_daemon_serves_the_same_tools() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = socket.local_addr()?.port();
    drop(socket);

    let mut command = base_command("daemon", project.path(), state.path());
    command
        .env("ARTIST_MCP_HOST", "127.0.0.1")
        .env("ARTIST_MCP_PORT", port.to_string())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let uri = format!("http://127.0.0.1:{port}/mcp");

    let client = {
        let mut connected = None;
        for _ in 0..100 {
            let transport = StreamableHttpClientTransport::from_uri(uri.clone());
            match ().serve(transport).await {
                Ok(client) => {
                    connected = Some(client);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
            }
        }
        connected.ok_or("daemon did not accept MCP connections")?
    };

    for path in [
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-protected-resource",
    ] {
        let response = raw_http_get(port, path).await?;
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
        assert!(response.contains("not configured"), "{response}");
    }
    let tools = client.list_all_tools().await?;
    assert!(tools.iter().any(|tool| tool.name == "bash"));
    let output = client
        .call_tool(call(
            "bash",
            json!({"mode": "exec", "command": "printf http-mcp"}),
        ))
        .await?;
    assert!(text_content(&output).contains("http-mcp"));

    client.cancel().await?;
    child.kill().await?;
    Ok(())
}

async fn raw_http_post(port: u16, path: &str, body: &str) -> Result<String, std::io::Error> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .as_bytes(),
        )
        .await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

/// The 2026-07-28 MCP revision makes `server/discover` mandatory, and OpenAI's
/// tunnel control plane probes it when a connector is created. rmcp 2.2 has no
/// handler for it, so the daemon must answer it at the HTTP boundary or
/// connector creation fails. This guards that the wire still advertises the
/// legacy revisions the server genuinely speaks.
#[tokio::test]
async fn streamable_http_daemon_answers_server_discover() -> Result<(), Box<dyn std::error::Error>>
{
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = socket.local_addr()?.port();
    drop(socket);

    let mut command = base_command("daemon", project.path(), state.path());
    command
        .env("ARTIST_MCP_HOST", "127.0.0.1")
        .env("ARTIST_MCP_PORT", port.to_string())
        .kill_on_drop(true);
    let mut child = command.spawn()?;

    let request = r#"{"jsonrpc":"2.0","id":"openai-mcp-discover","method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#;
    let mut response = None;
    for _ in 0..100 {
        let raw = raw_http_post(port, "/mcp", request).await;
        match raw {
            Ok(body) if body.starts_with("HTTP/1.1 200") => {
                response = Some(body);
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
        }
    }
    let response = response.ok_or("daemon never answered server/discover")?;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = response.split("\r\n\r\n").nth(1).ok_or("no body")?;
    let value: Value = serde_json::from_str(body)?;
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], "openai-mcp-discover");
    assert_eq!(value["result"]["resultType"], "complete");
    let versions = value["result"]["supportedVersions"]
        .as_array()
        .ok_or("supportedVersions must be a list")?;
    assert!(
        versions.iter().any(|version| version == "2025-06-18"),
        "server must advertise the legacy revision it negotiates: {versions:?}"
    );
    assert!(
        versions.iter().any(|version| version == "2026-07-28"),
        "the server is a dual-era gateway and must advertise modern 2026-07-28: {versions:?}"
    );

    child.kill().await?;
    Ok(())
}

/// The modern (2026-07-28) revision is served statelessly: a request naming it
/// must be answered with a JSON tool list built from a fresh server, not routed
/// into the stateful initialize handshake.
#[tokio::test]
async fn streamable_http_daemon_serves_modern_tools_list() -> Result<(), Box<dyn std::error::Error>>
{
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = socket.local_addr()?.port();
    drop(socket);

    let mut command = base_command("daemon", project.path(), state.path());
    command
        .env("ARTIST_MCP_HOST", "127.0.0.1")
        .env("ARTIST_MCP_PORT", port.to_string())
        .kill_on_drop(true);
    let mut child = command.spawn()?;

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#;
    let mut response = None;
    for _ in 0..100 {
        let raw = raw_http_post(port, "/mcp", request).await;
        match raw {
            Ok(body) if body.starts_with("HTTP/1.1 200") => {
                response = Some(body);
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
        }
    }
    let response = response.ok_or("daemon never answered modern tools/list")?;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = response.split("\r\n\r\n").nth(1).ok_or("no body")?;
    let value: Value = serde_json::from_str(body)?;
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 1);
    let tools = value["result"]["tools"]
        .as_array()
        .ok_or("modern tools/list must return a tool array")?;
    assert!(
        tools.iter().any(|tool| tool["name"] == "bash"),
        "modern tools/list must expose the real tool surface"
    );

    child.kill().await?;
    Ok(())
}
