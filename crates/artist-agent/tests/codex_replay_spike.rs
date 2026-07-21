//! Manual spike: does the Codex backend accept full-fidelity history
//! (tool calls, tool results, encrypted reasoning) replayed from a FRESH
//! process — i.e. the exact thing the event-sourced session store does on
//! resume?
//!
//! This is the riskiest assumption of the session-store design. Run it by
//! hand against a logged-in provider:
//!
//! ```sh
//! cargo test -p artist-agent --test codex_replay_spike -- --ignored --nocapture
//! ```
//!
//! Interpretation:
//! - Both phases pass → Rig-native conversation-memory resume is viable.
//! - Phase 2 fails on the replay request → enable the degrade path
//!   (`HistoryOptions::drop_encrypted_reasoning = true` on cross-run replay)
//!   and re-run; if that passes, resume keeps text + tool round-trips and
//!   drops only encrypted reasoning.

use anyhow::{Context, Result};
use futures::StreamExt;
use llm_provider::SavedProvider;
use rig_core::OneOrMany;
use rig_core::agent::MultiTurnStreamItem;
use rig_core::client::CompletionClient;
use rig_core::completion::message::{
    AssistantContent, Message, Text, ToolResult, ToolResultContent, UserContent,
};
use rig_core::providers::chatgpt;
use rig_core::streaming::{StreamedAssistantContent, StreamingChat};

fn load_provider() -> Result<SavedProvider> {
    let config_dir = std::env::var("ARTIST_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| dirs::config_dir().expect("config dir").join("artist"));
    let contents = std::fs::read_to_string(config_dir.join("providers.toml"))
        .context("read providers.toml — log in with `artist provider --login chatgpt` first")?;
    #[derive(serde::Deserialize)]
    struct Store {
        providers: Vec<SavedProvider>,
    }
    let store: Store = toml::from_str(&contents)?;
    store
        .providers
        .into_iter()
        .find(|provider| provider.model.is_some())
        .context("no provider with a selected model")
}

fn client(provider: &SavedProvider) -> Result<chatgpt::Client> {
    chatgpt::Client::builder()
        .api_key(chatgpt::ChatGPTAuth::AccessToken {
            access_token: provider.auth.access_token.expose().to_owned(),
            account_id: Some(provider.auth.account_id.clone()),
        })
        .base_url(provider.base_url.as_str())
        .originator("artist")
        .user_agent(concat!("artist/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build client")
}

/// Phase 1: run a turn that uses a tool, capturing the committed messages
/// in the same generic message shape persisted by Rig conversation memory.
/// Phase 2: build a brand-new client (simulating a fresh process) and send
/// a follow-up referencing the tool output, with the captured history
/// replayed verbatim.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual spike: needs a logged-in ChatGPT provider and network"]
async fn replayed_tool_history_is_accepted_cross_process() -> Result<()> {
    let provider = load_provider()?;
    let model = provider.model.as_deref().unwrap();

    // A trivial tool the model is instructed to call.
    #[derive(serde::Deserialize)]
    struct LookupArgs {}
    #[derive(Debug, thiserror::Error)]
    #[error("never")]
    struct Never;
    struct Lookup;
    impl rig_core::tool::Tool for Lookup {
        const NAME: &'static str = "lookup_build_id";
        type Error = Never;
        type Args = LookupArgs;
        type Output = String;
        fn description(&self) -> String {
            "Look up the current build id.".into()
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object","properties":{},"additionalProperties":false})
        }
        async fn call(&self, _args: LookupArgs) -> Result<String, Never> {
            Ok("build id: artichoke-7291".into())
        }
    }

    // ---- Phase 1: capture the Rig-native committed history.
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<Message>>>);
    impl<M: rig_core::completion::CompletionModel> rig_core::agent::AgentHook<M> for Capture {
        async fn on_event(
            &self,
            _ctx: &rig_core::agent::HookContext,
            event: rig_core::agent::StepEvent<'_, M>,
        ) -> rig_core::agent::Flow {
            match event {
                rig_core::agent::StepEvent::ModelTurnFinished { content, .. } => {
                    self.0.lock().unwrap().push(Message::Assistant {
                        id: None,
                        content: content.clone(),
                    });
                }
                rig_core::agent::StepEvent::ToolResult {
                    tool_call_id,
                    result,
                    ..
                } => {
                    // Pair with the committed assistant tool call, as the
                    // history builder does.
                    let mut messages = self.0.lock().unwrap();
                    let call = messages.iter().rev().find_map(|message| match message {
                        Message::Assistant { content, .. } => {
                            content.iter().find_map(|item| match item {
                                AssistantContent::ToolCall(call) => Some(call.clone()),
                                _ => None,
                            })
                        }
                        _ => None,
                    });
                    if let Some(call) = call {
                        let _ = tool_call_id;
                        messages.push(Message::User {
                            content: OneOrMany::one(UserContent::ToolResult(ToolResult {
                                id: call.id.clone(),
                                call_id: call.call_id.clone(),
                                content: OneOrMany::one(ToolResultContent::Text(Text::new(result))),
                            })),
                        });
                    }
                }
                _ => {}
            }
            rig_core::agent::Flow::cont()
        }
    }

    let capture = Capture::default();
    let agent = client(&provider)?
        .agent(model)
        .preamble("You are a test agent. Use the lookup_build_id tool when asked about the build id, then state it.")
        .tool(Lookup)
        .add_hook(capture.clone())
        .default_max_turns(8)
        .build();
    let prompt = Message::user("What is the current build id? Use your tool.");
    let mut stream = agent
        .stream_chat(prompt.clone(), Vec::<Message>::new())
        .await;
    let mut phase1_text = String::new();
    while let Some(item) = stream.next().await {
        if let MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text)) =
            item?
        {
            phase1_text.push_str(&text.text);
        }
    }
    drop(stream);
    println!("phase 1 answer: {phase1_text}");
    assert!(
        phase1_text.contains("artichoke-7291"),
        "model should have used the tool"
    );

    // The replayed history: original prompt + captured committed messages.
    let mut history = vec![prompt];
    history.extend(capture.0.lock().unwrap().iter().cloned());
    let encrypted = history.iter().any(|message| {
        matches!(message, Message::Assistant { content, .. } if content.iter().any(
            |item| matches!(item, AssistantContent::Reasoning(_))))
    });
    println!(
        "replaying {} messages (reasoning items present: {encrypted})",
        history.len()
    );

    // ---- Phase 2: fresh client (fresh process stand-in), replay verbatim.
    let agent2 = client(&provider)?
        .agent(model)
        .preamble("You are a test agent.")
        .tool(Lookup)
        .default_max_turns(4)
        .build();
    let mut stream = agent2
        .stream_chat(
            Message::user(
                "Without calling any tool: repeat the build id you looked up earlier, verbatim.",
            ),
            history,
        )
        .await;
    let mut phase2_text = String::new();
    while let Some(item) = stream.next().await {
        if let MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text)) =
            item?
        {
            phase2_text.push_str(&text.text);
        }
    }
    println!("phase 2 answer: {phase2_text}");
    assert!(
        phase2_text.contains("artichoke-7291"),
        "replayed tool context was not usable: {phase2_text}"
    );
    Ok(())
}
