//! End-to-end MCP contract tests for the final Artist harness surface.

use std::{collections::BTreeSet, path::Path};

use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
};

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
        .arg("warn")
        .env("ARTIST_STATE_DIR", state.join("registry"));
    command
}

fn isolated_server(
    project: &tempfile::TempDir,
    state: &tempfile::TempDir,
) -> Result<TokioChildProcess, std::io::Error> {
    TokioChildProcess::new(base_command("serve", project.path(), state.path()))
}

fn call(name: &str, args: Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name.to_owned()).with_arguments(
        args.as_object()
            .expect("tool arguments must be an object")
            .clone(),
    )
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

fn data(result: &rmcp::model::CallToolResult) -> &Value {
    &result
        .structured_content
        .as_ref()
        .expect("Artist calls publish structured result envelopes")["data"]
}

fn spawned_id(result: &rmcp::model::CallToolResult, prefix: &str) -> String {
    let text = text_content(result);
    text.split(|c: char| c.is_whitespace() || c == '"' || c == ',' || c == '}')
        .find(|part| part.starts_with(prefix))
        .unwrap_or_else(|| panic!("missing {prefix} session id in {text:?}"))
        .trim_matches(|c: char| c == ':' && prefix.ends_with(':'))
        .to_owned()
}

#[tokio::test]
async fn stdio_publishes_only_the_final_native_surface() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;
    let tools = client.list_all_tools().await?;
    let names = tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<BTreeSet<_>>();

    for expected in [
        "bash",
        "poll",
        "abort",
        "send",
        "list",
        "page",
        "ask",
        "todo",
        "skill",
        "computer",
        "canvas",
        "read",
        "find",
        "grep",
        "edit",
        "write",
        "code_map",
        "code_show",
    ] {
        assert!(names.contains(expected), "missing {expected}: {names:?}");
    }
    for deleted in [
        "await",
        "tell",
        "query",
        "reply",
        "gc",
        "ask_result",
        "ask_answer",
        "ask_list",
    ] {
        assert!(!names.contains(deleted), "deleted tool leaked: {deleted}");
    }

    let bash = tools.iter().find(|tool| tool.name == "bash").unwrap();
    assert!(bash.input_schema["properties"].get("command").is_some());
    for deleted in [
        "mode",
        "sessionId",
        "background",
        "waitMs",
        "maxBytes",
        "input",
    ] {
        assert!(
            bash.input_schema["properties"].get(deleted).is_none(),
            "bash still exposes {deleted}"
        );
    }

    let computer = tools.iter().find(|tool| tool.name == "computer").unwrap();
    let properties = computer.input_schema["properties"].as_object().unwrap();
    assert_eq!(
        properties
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        ["action", "args", "session"].into_iter().collect()
    );
    let actions = properties["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actions,
        [
            "launch",
            "attach",
            "observe",
            "find",
            "extract",
            "zoom",
            "screenshot",
            "focus",
            "resize",
            "watch",
            "do"
        ]
        .into_iter()
        .collect()
    );
    assert!(!actions.contains("surfaces"));
    assert!(!actions.contains("close"));

    let info = client.peer_info().expect("server info");
    let instructions = info.instructions.as_deref().unwrap_or_default();
    assert!(
        !instructions.contains("Artist actor"),
        "actor identity leaked: {instructions}"
    );
    assert!(
        !instructions.contains("actor mcp"),
        "actor identity leaked: {instructions}"
    );
    let identity = &info.meta.as_ref().expect("identity metadata").0["artist"]["identity"];
    assert!(identity.get("name").and_then(Value::as_str).is_some());
    assert_eq!(identity["profile"], "worker");
    assert!(
        identity.get("actor").is_none(),
        "internal actor leaked in metadata"
    );

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn stdio_bash_is_spawn_then_poll() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;

    let spawned = client
        .call_tool(call("bash", json!({"command":"printf 'hello-mcp\\n'"})))
        .await?;
    let id = data(&spawned)
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| spawned_id(&spawned, "bash:"));
    assert!(id.starts_with("bash:"), "{id}");

    let polled = client
        .call_tool(call("poll", json!({"session":id})))
        .await?;
    let text = text_content(&polled);
    assert!(text.contains("hello-mcp"), "{text}");
    assert!(text.to_ascii_lowercase().contains("completed"), "{text}");

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn ask_survives_reconnect_and_poll_projects_indices() -> Result<(), Box<dyn std::error::Error>>
{
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let client = ().serve(isolated_server(&project, &state)?).await?;
    let spawned = client
        .call_tool(call(
            "ask",
            json!({"questions":[{"question":"Which?","options":[
                {"label":"Project-local","recommended":true},
                {"label":"Global","recommended":false}
            ]}]}),
        ))
        .await?;
    let id = data(&spawned)
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| spawned_id(&spawned, "ask:"));

    // Answer while the live stdio owner still exists. A later process reconnect must
    // preserve the completed durable ask state; killing a still-live pending owner
    // would correctly produce the universal Abandoned tombstone instead.
    let registry = artist_session::AskRegistry::for_project(project.path(), None);
    let question = registry
        .pending()
        .into_iter()
        .find(|question| question.ask_session == id)
        .expect("durable pending question");
    assert!(registry.answer(artist_session::Answer {
        question_id: question.id.clone(),
        selections: vec![artist_session::ask::Selection {
            option_id: Some(question.options[0].id.clone()),
            note: Some("keep it local".into()),
        }],
    }));
    client.cancel().await?;

    let client = ().serve(isolated_server(&project, &state)?).await?;
    let polled = client
        .call_tool(call("poll", json!({"session":id,"timeoutMs":0})))
        .await?;
    let text = text_content(&polled);
    assert!(text.contains("chose 1"), "{text}");
    assert!(text.contains("keep it local"), "{text}");
    assert!(!text.contains("q-"), "internal question id leaked: {text}");
    assert!(!text.contains("o-"), "internal option id leaked: {text}");
    assert!(
        !text.contains("AutoResolve"),
        "answer source leaked: {text}"
    );

    client.cancel().await?;
    Ok(())
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

async fn raw_http_post(port: u16, body: &Value) -> Result<String, std::io::Error> {
    let body = body.to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    stream
        .write_all(
            format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            )
            .as_bytes(),
        )
        .await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

fn response_json(response: &str) -> Value {
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_else(|| panic!("HTTP response has no body: {response}"));
    serde_json::from_str(body).unwrap_or_else(|error| panic!("invalid JSON body {body:?}: {error}"))
}

fn modern_request(id: Value, method: &str, params: Value) -> Value {
    let mut params = params;
    params.as_object_mut().expect("params object").insert(
        "_meta".into(),
        json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28"}),
    );
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

async fn start_daemon(
    project: &tempfile::TempDir,
    state: &tempfile::TempDir,
) -> Result<(u16, tokio::process::Child), Box<dyn std::error::Error>> {
    let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = socket.local_addr()?.port();
    drop(socket);
    let mut command = base_command("daemon", project.path(), state.path());
    command
        .env("ARTIST_MCP_HOST", "127.0.0.1")
        .env("ARTIST_MCP_PORT", port.to_string())
        .kill_on_drop(true);
    let child = command.spawn()?;
    for _ in 0..100 {
        if raw_http_get(port, "/.well-known/oauth-protected-resource")
            .await
            .is_ok()
        {
            return Ok((port, child));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Err("daemon did not start".into())
}

#[tokio::test]
async fn http_vanilla_clients_are_bound_lazily_and_explicit_identity_is_honored(
) -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let (port, mut child) = start_daemon(&project, &state).await?;

    let listed = response_json(
        &raw_http_post(port, &modern_request(json!(1), "tools/list", json!({}))).await?,
    );
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    let identity = tools
        .iter()
        .find(|tool| tool["name"] == "identity")
        .expect("identity tool");
    assert!(identity["inputSchema"]["properties"].get("start").is_none());
    assert!(
        identity["inputSchema"]["properties"]
            .get("resume")
            .is_some()
    );
    let bash = tools
        .iter()
        .find(|tool| tool["name"] == "bash")
        .expect("bash tool");
    assert!(
        bash["inputSchema"]["properties"]
            .get("artistSession")
            .is_none(),
        "ordinary tools must not advertise the transport identity"
    );
    assert!(bash["inputSchema"]["properties"].get("command").is_some());
    assert!(
        !bash["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "artistSession")
    );

    // A vanilla tools/call with no identity plumbing works: the transport
    // session binds an identity lazily rather than failing.
    let anonymous = response_json(
        &raw_http_post(
            port,
            &modern_request(
                json!(2),
                "tools/call",
                json!({"name":"bash","arguments":{"command":"printf http-mcp"}}),
            ),
        )
        .await?,
    );
    assert!(
        anonymous.get("error").is_none(),
        "vanilla call unexpectedly failed: {anonymous}"
    );
    let anonymous_session = anonymous.to_string();
    let session = anonymous_session
        .split(|c: char| c.is_whitespace() || c == '"' || c == ',' || c == '}')
        .find(|part| part.starts_with("bash:"))
        .expect("bash session")
        .to_owned();

    let polled = response_json(
        &raw_http_post(
            port,
            &modern_request(
                json!(3),
                "tools/call",
                json!({"name":"poll","arguments":{"session":session}}),
            ),
        )
        .await?,
    );
    assert!(polled.to_string().contains("http-mcp"), "{polled}");

    // The explicit transport identity still creates a durable identity.
    let started = response_json(
        &raw_http_post(
            port,
            &modern_request(
                json!(4),
                "tools/call",
                json!({"name":"identity","arguments":{}}),
            ),
        )
        .await?,
    );
    let artist = started["result"]["structuredContent"]["artistSession"]
        .as_str()
        .expect("artistSession")
        .to_owned();
    let started_text = started.to_string();
    assert!(
        !started_text.contains("\"actor\""),
        "actor leaked: {started_text}"
    );

    // Passing the transport identity explicitly is still honored and stripped.
    let spawned = response_json(&raw_http_post(
        port,
        &modern_request(
            json!(5),
            "tools/call",
            json!({"name":"bash","arguments":{"artistSession":artist,"command":"printf http-mcp"}}),
        ),
    ).await?);
    assert!(
        spawned.get("error").is_none(),
        "explicit identity call failed: {spawned}"
    );

    let resumed = response_json(
        &raw_http_post(
            port,
            &modern_request(
                json!(6),
                "tools/call",
                json!({"name":"identity","arguments":{"resume":artist}}),
            ),
        )
        .await?,
    );
    assert_eq!(
        resumed["result"]["structuredContent"]["artistSession"],
        started["result"]["structuredContent"]["artistSession"]
    );

    let missing_resume = response_json(
        &raw_http_post(
            port,
            &modern_request(
                json!(7),
                "tools/call",
                json!({"name":"identity","arguments":{"resume":"DefinitelyMissingArtist"}}),
            ),
        )
        .await?,
    );
    assert!(
        missing_resume.get("error").is_some(),
        "resume minted a replacement: {missing_resume}"
    );

    child.kill().await?;
    Ok(())
}

#[tokio::test]
async fn http_discovery_is_anonymous_and_dual_revision() -> Result<(), Box<dyn std::error::Error>> {
    if !binary_available() {
        return Ok(());
    }
    let project = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    let (port, mut child) = start_daemon(&project, &state).await?;
    let response = response_json(
        &raw_http_post(
            port,
            &json!({
                "jsonrpc":"2.0",
                "id":"discover",
                "method":"server/discover",
                "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}
            }),
        )
        .await?,
    );
    let versions = response["result"]["supportedVersions"]
        .as_array()
        .expect("supported versions");
    assert!(versions.iter().any(|version| version == "2025-06-18"));
    assert!(versions.iter().any(|version| version == "2026-07-28"));
    child.kill().await?;
    Ok(())
}
