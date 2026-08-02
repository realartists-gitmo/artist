//! End-to-end coverage of the agent loop against a scripted Responses server.
//!
//! Every stage has unit tests, but the expensive bugs have all lived in the
//! seams between them: a tool call parsed from one turn's stream, dispatched,
//! answered, persisted to the provider sidecar, and replayed into the next
//! turn's request. No unit test spans that path. These do.

use artist_agent::{ChatInput, SessionHandles, ToolContext};
use base64::Engine as _;
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
            "test",
        )
        .unwrap();
        let mcp = artist_agent::mcp::McpManager::load(config.path())
            .await
            .unwrap();
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
        // A real run always carries one; anchor state is scoped by it, and the
        // placeholder is refused precisely so a session cannot reach a turn
        // still sharing an actor with every other session.
        SessionHandles {
            conversation_id: "test-conversation".to_owned(),
            ..SessionHandles::default()
        },
        |_| Ok(()),
    )
    .await;
    assert!(outcome.is_ok(), "{outcome:?}");

    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "the tool result should drive a second turn"
    );
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
        // A real run always carries one; anchor state is scoped by it, and the
        // placeholder is refused precisely so a session cannot reach a turn
        // still sharing an actor with every other session.
        SessionHandles {
            conversation_id: "test-conversation".to_owned(),
            ..SessionHandles::default()
        },
        |_| Ok(()),
    )
    .await;

    assert!(
        outcome.is_ok(),
        "an unknown tool must not end the run: {outcome:?}"
    );
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "the model should get a chance to recover"
    );
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
        // A real run always carries one; anchor state is scoped by it, and the
        // placeholder is refused precisely so a session cannot reach a turn
        // still sharing an actor with every other session.
        SessionHandles {
            conversation_id: "test-conversation".to_owned(),
            ..SessionHandles::default()
        },
        |_| Ok(()),
    )
    .await;

    assert!(
        outcome.is_ok(),
        "a tool panic must not end the run: {outcome:?}"
    );
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "the model should get a chance to recover"
    );
    assert!(
        requests[1].contains("call_1"),
        "the failed call must still be answered: {}",
        requests[1]
    );
}

/// The system prompt a request actually carried.
fn instructions(raw: &str) -> String {
    let (_, body) = raw.split_once("\r\n\r\n").expect("request has a body");
    let parsed: serde_json::Value = serde_json::from_str(body).expect("body is JSON");
    parsed["instructions"]
        .as_str()
        .expect("request carries a system prompt")
        .to_owned()
}

/// The prompt-cache rule, enforced generically rather than field by field.
///
/// Prompt caching keys on a prefix, so a preamble that moves between turns
/// reprices the entire conversation at full rate. The preamble is assembled
/// from disk — instruction files, the skill catalogue — and used to be rebuilt
/// on every turn *and* every retry, so any of those moving underneath a live
/// session invalidated the cache. The agent itself could cause it, since it can
/// edit an `AGENTS.md`.
///
/// This test does not enumerate the inputs. It mutates one and asserts the
/// bytes did not move, so an input added years from now is covered by it
/// without anyone remembering to come back here.
#[tokio::test]
async fn the_system_prompt_never_moves_within_a_session() {
    let harness = Harness::new().await;
    std::fs::write(
        harness.project().join("AGENTS.md"),
        "always answer in haiku",
    )
    .unwrap();

    let (url, requests) = scripted(vec![says("resp_1", "one"), says("resp_2", "two")]).await;
    let handles = SessionHandles {
        conversation_id: "test-conversation".to_owned(),
        ..SessionHandles::default()
    };

    artist_agent::stream_chat(
        &provider(&url),
        &ChatInput::from("first".to_owned()),
        harness.context(),
        handles.clone(),
        |_| Ok(()),
    )
    .await
    .expect("first turn");

    // The user edits their instructions mid-session — or the agent does.
    std::fs::write(
        harness.project().join("AGENTS.md"),
        "actually, answer in limerick",
    )
    .unwrap();

    artist_agent::stream_chat(
        &provider(&url),
        &ChatInput::from("second".to_owned()),
        harness.context(),
        handles.clone(),
        |_| Ok(()),
    )
    .await
    .expect("second turn");

    let requests = requests.lock().unwrap();
    let first = instructions(&requests[0]);
    let second = instructions(&requests[1]);

    assert_eq!(
        first, second,
        "the preamble moved between turns; every cached token behind it is now repriced"
    );
    assert!(
        first.contains("always answer in haiku"),
        "the first preamble should carry the instructions as they were: {first}"
    );
    assert!(
        !second.contains("actually, answer in limerick"),
        "the changed instructions must not reach the preamble: {second}"
    );
}

/// Freezing must not mean losing. The changed instructions still have to reach
/// the model — on the user turn, where changing content is free — or the user
/// would be running on a file they believe they replaced.
#[tokio::test]
async fn changed_instructions_ride_the_user_turn_instead() {
    let harness = Harness::new().await;
    std::fs::write(harness.project().join("AGENTS.md"), "always answer in haiku").unwrap();

    let (url, requests) = scripted(vec![says("resp_1", "one"), says("resp_2", "two")]).await;
    let handles = SessionHandles {
        conversation_id: "test-conversation".to_owned(),
        ..SessionHandles::default()
    };

    artist_agent::stream_chat(
        &provider(&url),
        &ChatInput::from("first".to_owned()),
        harness.context(),
        handles.clone(),
        |_| Ok(()),
    )
    .await
    .expect("first turn");

    std::fs::write(
        harness.project().join("AGENTS.md"),
        "actually, answer in limerick",
    )
    .unwrap();

    artist_agent::stream_chat(
        &provider(&url),
        &ChatInput::from("second".to_owned()),
        harness.context(),
        handles.clone(),
        |_| Ok(()),
    )
    .await
    .expect("second turn");

    let requests = requests.lock().unwrap();
    let second = &requests[1];
    assert!(
        second.contains("actually, answer in limerick"),
        "the change never reached the model at all: {second}"
    );
    assert!(
        second.contains("changed on disk"),
        "the change arrived unexplained: {second}"
    );
}

/// A scripted server that knows every endpoint the transport can reach.
///
/// One helper rather than one per feature: the transport may probe `/files` or
/// `/prompts` before a completion, and a server that mistook either for a
/// completion would hand it an SSE body and throw the whole script out of step.
async fn scripted_endpoints(
    bodies: Vec<String>,
    file_id: Option<&'static str>,
    stored_prompt: Option<(&'static str, &'static str)>,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);

    tokio::spawn(async move {
        let mut bodies = bodies.into_iter();
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut raw = Vec::new();
            let mut buffer = [0u8; 8192];
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
            let text = String::from_utf8_lossy(&raw).into_owned();
            let route = if text.starts_with("POST /files") {
                Route::Files
            } else if text.starts_with("POST /prompts") {
                Route::Prompts
            } else {
                Route::Responses
            };
            captured.lock().unwrap().push(text);

            let json = |body: String| {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            const MISSING: &str =
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

            let reply = match route {
                Route::Files => match file_id {
                    Some(id) => json(format!(r#"{{"id":"{id}","object":"file"}}"#)),
                    None => MISSING.to_owned(),
                },
                Route::Prompts => match stored_prompt {
                    Some((id, version)) => json(format!(r#"{{"id":"{id}","version":"{version}"}}"#)),
                    None => MISSING.to_owned(),
                },
                Route::Responses => {
                    let Some(body) = bodies.next() else { return };
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                }
            };
            let _ = socket.write_all(reply.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    (format!("http://{address}"), requests)
}

enum Route {
    Files,
    Prompts,
    Responses,
}


/// A provider on the API-key Responses route.
///
/// `provider()` builds the ChatGPT/Codex one, which publishes only
/// `/responses` — probed live: `/files` and `/prompts` answer 403 from the edge
/// there. Anything exercising a side endpoint needs this instead.
fn api_key_provider(url: &str) -> SavedProvider {
    let base = provider(url);
    llm_provider::SavedProvider {
        id: ProviderId::new("test").unwrap(),
        name: "test".into(),
        provider: llm_provider::ProviderKind::Openai,
        base_url: base.base_url.clone(),
        api: Some(llm_provider::OpenAiApi::Responses),
        api_version: None,
        model: Some("codex".into()),
        reasoning_effort: None,
        credentials: llm_provider::Credentials::ApiKey {
            api_key: Secret::new("sk-test"),
        },
    }
}

fn handles_with_attachments(dir: &std::path::Path) -> SessionHandles {
    SessionHandles {
        conversation_id: "test-conversation".to_owned(),
        attachments: Some(artist_session::AttachmentStore::new(dir.join("attachments"))),
        ..SessionHandles::default()
    }
}

/// Handles with one provider-side handle enabled.
///
/// Passed in rather than set in the environment, which is the whole point of
/// the change: a test that enables chaining cannot alter a test running beside
/// it, so these need no serialisation and no lock.
fn handles_with(dir: &std::path::Path, statefulness: artist_agent::Statefulness) -> SessionHandles {
    SessionHandles {
        statefulness,
        ..handles_with_attachments(dir)
    }
}

/// The point of externalizing images: the bytes go up once, and every later
/// request references them instead of restating them.
///
/// A stateless API resends the whole conversation each turn, so an inlined
/// screenshot is re-uploaded for the rest of the session — measured at 95.4% of
/// all bytes on a real 48-turn session, and that one had no images at all.
#[tokio::test]
async fn an_image_is_uploaded_once_and_referenced_thereafter() {
    let harness = Harness::new().await;
    let session = tempfile::tempdir().unwrap();
    std::fs::write(harness.project().join("shot.png"), b"pretend png bytes").unwrap();

    let (url, requests) = scripted_endpoints(
        vec![
            calls_tool("resp_1", "call_1", "read", r#"{"path":"shot.png"}"#),
            says("resp_2", "I can see it"),
        ],
        Some("file-uploaded-once"),
        None,
    )
    .await;

    artist_agent::stream_chat(
        &api_key_provider(&url),
        &ChatInput::from("look at shot.png".to_owned()),
        harness.context(),
        handles_with_attachments(session.path()),
        |_| Ok(()),
    )
    .await
    .expect("run completes");

    let requests = requests.lock().unwrap();
    let uploads = requests
        .iter()
        .filter(|r| r.starts_with("POST /files"))
        .count();
    assert_eq!(uploads, 1, "the image should be uploaded exactly once");

    let second = requests
        .iter()
        .filter(|r| r.starts_with("POST /responses"))
        .nth(1)
        .expect("a second completion request");
    assert!(
        second.contains("file-uploaded-once"),
        "the image was not referenced by handle: {second}"
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"pretend png bytes");
    assert!(
        !second.contains(&encoded),
        "the image bytes were still sent inline alongside the handle"
    );
}

/// A backend with no files endpoint must cost one failed probe for the whole
/// session — not one per image — and must still send a working request.
///
/// Two images across two turns, so a client that retried per image would show
/// two uploads rather than one.
#[tokio::test]
async fn a_backend_without_uploads_probes_once_then_inlines() {
    let harness = Harness::new().await;
    let session = tempfile::tempdir().unwrap();
    std::fs::write(harness.project().join("one.png"), b"first image").unwrap();
    std::fs::write(harness.project().join("two.png"), b"second image").unwrap();

    let (url, requests) = scripted_endpoints(
        vec![
            calls_tool("resp_1", "call_1", "read", r#"{"path":"one.png"}"#),
            calls_tool("resp_2", "call_2", "read", r#"{"path":"two.png"}"#),
            says("resp_3", "I can see both"),
        ],
        None,
        None,
    )
    .await;

    artist_agent::stream_chat(
        &api_key_provider(&url),
        &ChatInput::from("look at both images".to_owned()),
        harness.context(),
        handles_with_attachments(session.path()),
        |_| Ok(()),
    )
    .await
    .expect("a missing files endpoint must not fail the run");

    let requests = requests.lock().unwrap();
    let probes = requests
        .iter()
        .filter(|r| r.starts_with("POST /files"))
        .count();
    assert_eq!(
        probes, 1,
        "a missing endpoint should be probed once and then left alone"
    );

    let last = requests
        .iter()
        .filter(|r| r.starts_with("POST /responses"))
        .next_back()
        .expect("a completion request");
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"first image");
    assert!(
        last.contains(&encoded),
        "the image should have degraded to inline bytes: {last}"
    );
}

/// Chaining must *replace* the history it references, not accompany it.
///
/// The whole saving comes from omitting what the provider already holds; a
/// request that sets `previous_response_id` and still restates the conversation
/// would double it, which is worse than sending nothing at all. Measured on a
/// real 48-turn session, 95.4% of uploaded bytes were resends.
///
/// Chaining needs an API key — the Codex backend does not retain responses —
/// and an explicit opt-in, so this test supplies both.
#[tokio::test]
async fn a_chained_request_omits_what_the_provider_already_holds() {
    let harness = Harness::new().await;
    std::fs::write(harness.project().join("hello.txt"), "the file body").unwrap();

    let (url, requests) = scripted(vec![
        calls_tool("resp_1", "call_1", "read", r#"{"path":"hello.txt"}"#),
        says("resp_2", "done"),
    ])
    .await;

    let provider = api_key_provider(&url);

    let outcome = artist_agent::stream_chat(
        &provider,
        &ChatInput::from("read the file".to_owned()),
        harness.context(),
        SessionHandles {
            conversation_id: "test-conversation".to_owned(),
            statefulness: artist_agent::Statefulness::default().with_chaining(true),
            ..SessionHandles::default()
        },
        |_| Ok(()),
    )
    .await;
    assert!(outcome.is_ok(), "{outcome:?}");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "the tool result should drive a second turn");

    let first_body: serde_json::Value =
        serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    let second_body: serde_json::Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();

    // Turn one has nothing to chain from.
    assert!(
        first_body.get("previous_response_id").is_none(),
        "a cold chain must send everything: {first_body}"
    );
    assert_eq!(
        second_body["previous_response_id"], "resp_1",
        "the second turn should continue the first: {second_body}"
    );
    assert_eq!(
        second_body["store"], true,
        "a chained request must ask the provider to retain it"
    );

    // The user's opening message belongs to the chained-from response, so it
    // must not be restated.
    let restated = second_body["input"].to_string();
    assert!(
        !restated.contains("read the file"),
        "the chained request duplicated history the provider already holds: {restated}"
    );
    // What is genuinely new — the tool's output — must still be sent.
    assert!(
        restated.contains("the file body"),
        "the chained request dropped the new tool result: {restated}"
    );
}

/// A scripted server that also stores prompts.
async fn scripted_with_prompts(
    bodies: Vec<String>,
    stored: Option<(&'static str, &'static str)>,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);

    tokio::spawn(async move {
        let mut bodies = bodies.into_iter();
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut raw = Vec::new();
            let mut buffer = [0u8; 8192];
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
            let text = String::from_utf8_lossy(&raw).into_owned();
            let is_publish = text.starts_with("POST /prompts");
            captured.lock().unwrap().push(text);

            let reply = if is_publish {
                match stored {
                    Some((id, version)) => {
                        let body = format!(r#"{{"id":"{id}","version":"{version}"}}"#);
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                }
            } else {
                let Some(body) = bodies.next() else { return };
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = socket.write_all(reply.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    (format!("http://{address}"), requests)
}

/// The strongest form of the prompt-cache rule: the preamble is not in the
/// request at all, so it cannot be resent and cannot drift.
#[tokio::test]
async fn a_stored_prompt_replaces_the_preamble_on_the_wire() {
    let harness = Harness::new().await;
    let session = tempfile::tempdir().unwrap();
    std::fs::write(harness.project().join("AGENTS.md"), "a distinctive instruction").unwrap();

    let (url, requests) =
        scripted_endpoints(vec![says("resp_1", "one"), says("resp_2", "two")], None, Some(("pmpt_abc", "3"))).await;

    let handles = handles_with(
        session.path(),
        artist_agent::Statefulness::default().with_stored_prompt(true),
    );
    for message in ["first", "second"] {
        artist_agent::stream_chat(
            &api_key_provider(&url),
            &ChatInput::from(message.to_owned()),
            harness.context(),
            handles.clone(),
            |_| Ok(()),
        )
        .await
        .expect("turn completes");
    }

    let requests = requests.lock().unwrap();
    let publishes = requests.iter().filter(|r| r.starts_with("POST /prompts")).count();
    assert_eq!(publishes, 1, "the prompt should be stored once, then referenced");

    let completions: Vec<&String> = requests
        .iter()
        .filter(|r| r.starts_with("POST /responses"))
        .collect();
    assert_eq!(completions.len(), 2);
    for request in &completions {
        let body: serde_json::Value =
            serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["prompt"]["id"], "pmpt_abc", "{body}");
        assert_eq!(body["prompt"]["version"], "3", "pinned to a revision");
        assert!(
            body.get("instructions").is_none_or(serde_json::Value::is_null),
            "the preamble must leave the request entirely: {body}"
        );
        assert!(
            !request.contains("a distinctive instruction"),
            "the preamble text was still on the wire"
        );
    }
}

/// An endpoint that cannot store prompts must cost one probe and keep working.
#[tokio::test]
async fn an_endpoint_without_stored_prompts_keeps_sending_instructions() {
    let harness = Harness::new().await;
    let session = tempfile::tempdir().unwrap();
    std::fs::write(harness.project().join("AGENTS.md"), "a distinctive instruction").unwrap();

    let (url, requests) =
        scripted_endpoints(vec![says("resp_1", "one"), says("resp_2", "two")], None, None).await;

    let handles = handles_with(
        session.path(),
        artist_agent::Statefulness::default().with_stored_prompt(true),
    );
    for message in ["first", "second"] {
        artist_agent::stream_chat(
            &api_key_provider(&url),
            &ChatInput::from(message.to_owned()),
            harness.context(),
            handles.clone(),
            |_| Ok(()),
        )
        .await
        .expect("a missing prompts endpoint must not fail the run");
    }

    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.iter().filter(|r| r.starts_with("POST /prompts")).count(),
        1,
        "probe once, then stop asking"
    );
    let last = requests
        .iter()
        .filter(|r| r.starts_with("POST /responses"))
        .next_back()
        .unwrap();
    assert!(
        last.contains("a distinctive instruction"),
        "the preamble should have degraded to being sent inline"
    );
}

/// The Codex backend publishes only `/responses`; probing it for a files
/// endpoint can never succeed, so it must not be probed at all.
///
/// Verified against the live backend: `GET /responses` there answers 405 with a
/// JSON body — a real route — while `/files` answers 403 with an HTML error
/// page, which is the edge refusing an unpublished path.
#[tokio::test]
async fn the_chatgpt_backend_is_never_probed_for_side_endpoints() {
    let harness = Harness::new().await;
    let session = tempfile::tempdir().unwrap();
    std::fs::write(harness.project().join("shot.png"), b"pretend png bytes").unwrap();

    let (url, requests) = scripted_endpoints(
        vec![
            calls_tool("resp_1", "call_1", "read", r#"{"path":"shot.png"}"#),
            says("resp_2", "I can see it"),
        ],
        // Answer if asked — the point is that it never is.
        Some("file-should-not-be-used"),
        None,
    )
    .await;

    artist_agent::stream_chat(
        &provider(&url),
        &ChatInput::from("look at shot.png".to_owned()),
        harness.context(),
        handles_with_attachments(session.path()),
        |_| Ok(()),
    )
    .await
    .expect("run completes");

    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.iter().filter(|r| r.starts_with("POST /files")).count(),
        0,
        "a backend without a files endpoint should not be asked"
    );
    let last = requests
        .iter()
        .filter(|r| r.starts_with("POST /responses"))
        .next_back()
        .expect("a completion request");
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"pretend png bytes");
    assert!(
        last.contains(&encoded),
        "the image should ride inline as it always did"
    );
}

/// The project skeleton opens a session and is never reissued.
///
/// It rides the first user turn rather than the preamble, which is what makes
/// "frozen for the session" structural: a message already in the history cannot
/// be rewritten, so nothing later can move it and repriced the conversation
/// behind it. Reissuing it every turn is precisely what aider's repo map does
/// and precisely what this must not.
#[tokio::test]
async fn the_project_shape_arrives_once_and_stays_put() {
    let harness = Harness::new().await;
    // Two crates, so there is an architecture to describe at all.
    for unit in ["core", "app"] {
        std::fs::create_dir_all(harness.project().join(format!("crates/{unit}/src"))).unwrap();
        std::fs::write(
            harness.project().join(format!("crates/{unit}/Cargo.toml")),
            format!("[package]\nname = \"{unit}\"\n"),
        )
        .unwrap();
    }
    std::fs::write(
        harness.project().join("crates/core/src/lib.rs"),
        "//! Shared primitives everything else builds on.\npub fn helper() {}\n",
    )
    .unwrap();
    std::fs::write(
        harness.project().join("crates/app/src/lib.rs"),
        "//! The entry point.\nuse core::helper;\npub fn run() { helper() }\n",
    )
    .unwrap();

    let (url, requests) = scripted(vec![says("resp_1", "one"), says("resp_2", "two")]).await;
    let handles = SessionHandles {
        conversation_id: "test-conversation".to_owned(),
        ..SessionHandles::default()
    };

    for message in ["first", "second"] {
        artist_agent::stream_chat(
            &provider(&url),
            &ChatInput::from(message.to_owned()),
            harness.context(),
            handles.clone(),
            |_| Ok(()),
        )
        .await
        .expect("turn completes");
    }

    let requests = requests.lock().unwrap();
    assert!(
        requests[0].contains("project shape"),
        "the opening turn should carry the project shape: {}",
        requests[0]
    );
    assert!(
        requests[0].contains("Shared primitives"),
        "the shape should carry each unit's own doc comment: {}",
        requests[0]
    );

    // Turn two must not send it again. It is already in the history the second
    // request replays, so counting occurrences catches a reissue.
    let second: serde_json::Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    let occurrences = second["input"].to_string().matches("project shape").count();
    assert_eq!(
        occurrences, 1,
        "the shape should appear once, carried by history — not reissued"
    );
}
