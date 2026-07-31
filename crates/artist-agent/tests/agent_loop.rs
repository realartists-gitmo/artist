//! End-to-end coverage of the agent loop against a scripted Responses server.
//!
//! Every stage has unit tests, but the expensive bugs have all lived in the
//! seams between them: a tool call parsed from one turn's stream, dispatched,
//! answered, persisted to the provider sidecar, and replayed into the next
//! turn's request. No unit test spans that path. These do.

use artist_agent::{ChatInput, SessionHandles, ToolContext};
use llm_provider::{Auth, ProviderId, SavedProvider, Secret};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serve one canned body per request, in order, capturing what was sent.
///
/// Multi-turn is the point: a single-response stub cannot show what the second
/// request carried, which is exactly where the pairing bugs lived.
async fn scripted(bodies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);

    tokio::spawn(async move {
        for body in bodies {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut raw = Vec::new();
            let mut buffer = [0u8; 8192];
            // Read until the declared body has arrived; one read() can split a
            // request whose history has grown past the buffer.
            loop {
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&raw);
                if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                    let declared = head
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("Content-Length: ")
                                .or_else(|| line.strip_prefix("content-length: "))
                        })
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if rest.len() >= declared {
                        break;
                    }
                }
            }
            captured
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&raw).into_owned());
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    (format!("http://{address}"), requests)
}

fn terminal(id: &str) -> String {
    format!(
        r#"{{"id":"{id}","object":"response","created_at":1,"status":"completed","error":null,"incomplete_details":null,"instructions":null,"max_output_tokens":null,"model":"codex","usage":{{"input_tokens":1,"output_tokens":1,"total_tokens":2}},"output":[],"tools":[]}}"#
    )
}

/// A turn that calls one tool and stops.
fn calls_tool(response_id: &str, call_id: &str, name: &str, arguments: &str) -> String {
    let escaped = serde_json::to_string(arguments).unwrap();
    format!(
        "data: {{\"type\":\"response.output_item.done\",\"item\":{{\"type\":\"function_call\",\"id\":\"fc_{call_id}\",\"call_id\":\"{call_id}\",\"name\":\"{name}\",\"arguments\":{escaped},\"status\":\"completed\"}}}}\n\ndata: {{\"type\":\"response.completed\",\"response\":{}}}\n\ndata: [DONE]\n\n",
        terminal(response_id)
    )
}

/// A turn that answers in text and stops.
fn says(response_id: &str, text: &str) -> String {
    format!(
        "data: {{\"type\":\"response.output_text.delta\",\"delta\":{}}}\n\ndata: {{\"type\":\"response.completed\",\"response\":{}}}\n\ndata: [DONE]\n\n",
        serde_json::to_string(text).unwrap(),
        terminal(response_id)
    )
}

fn provider(base_url: &str) -> SavedProvider {
    let mut provider = SavedProvider::chatgpt(
        ProviderId::new("test").unwrap(),
        "test",
        Auth {
            access_token: Secret::new("token"),
            refresh_token: Secret::new("refresh"),
            account_id: "acct".into(),
            email: None,
            expires_at: None,
        },
    );
    provider.base_url = base_url.parse().unwrap();
    provider.model = Some("codex".into());
    provider
}

struct Harness {
    _project: tempfile::TempDir,
    _config: tempfile::TempDir,
    native: artist_tools::ToolBundle,
    mcp: artist_agent::mcp::McpManager,
}

impl Harness {
    async fn new() -> Self {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let workspace = artist_tools::Workspace::open(
            project.path().to_path_buf(),
            config.path().to_path_buf(),
        )
        .unwrap();
        let mcp = artist_agent::mcp::McpManager::load(config.path()).await.unwrap();
        Self {
            native: artist_tools::ToolBundle::new(workspace),
            _project: project,
            _config: config,
            mcp,
        }
    }

    fn context(&self) -> ToolContext<'_> {
        ToolContext {
            native: &self.native,
            mcp: &self.mcp,
            extensions: None,
            disabled: &[],
            canvas: None,
        }
    }

    fn project(&self) -> &std::path::Path {
        self._project.path()
    }
}

/// The seam that broke twice: a call streamed in turn one has to come back as a
/// `function_call_output` paired by call_id in turn two, or the provider
/// rejects the whole request.
#[tokio::test]
async fn a_tool_result_is_paired_into_the_next_request() {
    let harness = Harness::new().await;
    std::fs::write(harness.project().join("hello.txt"), "the file body").unwrap();

    let (url, requests) = scripted(vec![
        calls_tool("resp_1", "call_1", "read", r#"{"path":"hello.txt"}"#),
        says("resp_2", "done"),
    ])
    .await;

    let outcome = artist_agent::stream_chat(
        &provider(&url),
        &ChatInput::from("read the file".to_owned()),
        harness.context(),
        SessionHandles::default(),
        |_| Ok(()),
    )
    .await;
    assert!(outcome.is_ok(), "{outcome:?}");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "the tool result should drive a second turn");
    let second = &requests[1];
    assert!(
        second.contains("function_call_output"),
        "second request carries no tool output: {second}"
    );
    assert!(
        second.contains("call_1"),
        "the output is not paired by call_id: {second}"
    );
    assert!(
        second.contains("the file body"),
        "the tool actually ran and its output was sent: {second}"
    );
}

/// A model can name a tool we do not register — a hallucinated name, or the
/// `multi_tool_use.parallel` wrapper OpenAI injects. That must be an ordinary
/// tool failure the model can recover from, not the end of the run.
#[tokio::test]
async fn a_call_to_an_unregistered_tool_is_reported_back_to_the_model() {
    let harness = Harness::new().await;
    let (url, requests) = scripted(vec![
        calls_tool("resp_1", "call_1", "no_such_tool", "{}"),
        says("resp_2", "understood"),
    ])
    .await;

    let outcome = artist_agent::stream_chat(
        &provider(&url),
        &ChatInput::from("use a tool that does not exist".to_owned()),
        harness.context(),
        SessionHandles::default(),
        |_| Ok(()),
    )
    .await;

    assert!(outcome.is_ok(), "an unknown tool must not end the run: {outcome:?}");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "the model should get a chance to recover");
    assert!(
        requests[1].contains("call_1"),
        "the failure must be reported against the call: {}",
        requests[1]
    );
}

/// A panic inside a tool used to kill the agent task and take the turn with it,
/// leaving the call unanswered in the provider's record. The read path panics
/// on an offset past the end of the file, which makes this a real case rather
/// than a synthetic one.
#[tokio::test]
async fn a_panicking_tool_fails_that_call_rather_than_the_run() {
    let harness = Harness::new().await;
    std::fs::write(harness.project().join("short.txt"), "one\ntwo\nthree\n").unwrap();

    let (url, requests) = scripted(vec![
        calls_tool(
            "resp_1",
            "call_1",
            "read",
            r#"{"path":"short.txt","offset":900}"#,
        ),
        says("resp_2", "recovered"),
    ])
    .await;

    let outcome = artist_agent::stream_chat(
        &provider(&url),
        &ChatInput::from("read past the end".to_owned()),
        harness.context(),
        SessionHandles::default(),
        |_| Ok(()),
    )
    .await;

    assert!(outcome.is_ok(), "a tool panic must not end the run: {outcome:?}");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "the model should get a chance to recover");
    assert!(
        requests[1].contains("call_1"),
        "the failed call must still be answered: {}",
        requests[1]
    );
}
