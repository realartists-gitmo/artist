//! End-to-end stdio tests: spawn the real `artist-mcp serve` binary and drive
//! it over MCP the way a tunnel or local client would.
//!
//! The worker profile is permissive, so each test observes the complete set of
//! tools its explicitly enabled environment can build. Optional machine, memory,
//! provider, and messaging capabilities remain absent unless configured.

use std::{collections::BTreeSet, path::Path};

use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{StreamableHttpClientTransport, TokioChildProcess},
};
use serde_json::{Map, Value, json};
use tokio::process::Command;

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
        assert_eq!(schema.get("type"), Some(&json!("object")), "{}", tool.name);
        assert_eq!(
            schema
                .get("properties")
                .and_then(Value::as_object)
                .and_then(|properties| properties.get("tool"))
                .and_then(Value::as_object)
                .and_then(|tool_property| tool_property.get("const")),
            Some(&json!(tool.name.as_ref())),
            "{}",
            tool.name
        );
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

    let script = "echo blank >> .e2e/blank.txt";
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
        "init",
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

    let first = client.call_tool(call("init", json!({}))).await?;
    let second = client.call_tool(call("init", json!({}))).await?;
    let first = text_content(&first);
    let second = text_content(&second);
    assert!(first.starts_with("You are "), "got: {first}");
    assert_eq!(first, second, "identity must be stable across re-claims");

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
