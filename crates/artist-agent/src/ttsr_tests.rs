//! Integration harness for the TTSR abort/inject/retry mechanics, driven
//! against rig's scripted `MockCompletionModel` — no network, no provider.
//!
//! The mini driver below mirrors `stream_chat`'s retry contract exactly:
//! hook-side aborts seed from `PromptCancelled.chat_history` (committed
//! turns minus the offending partial one), reasoning-side aborts seed from
//! the history `TtsrShared` captured at the latest `CompletionCall`, and
//! the reminder becomes the retry prompt.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use artist_rules::declarative::parse_parts;
use artist_rules::matcher::RuleSet;
use artist_rules::state::RulesHandle;
use artist_rules::types::Firing;
use futures::StreamExt;
use rig_core::agent::{AgentBuilder, MultiTurnStreamItem, StreamingError};
use rig_core::completion::message::Message;
use rig_core::completion::{CompletionRequest, PromptError};
use rig_core::streaming::{StreamedAssistantContent, StreamingChat};
use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};

use crate::steering::{SteeringHandle, SteeringHook};
use crate::ttsr::{TtsrHook, TtsrShared, reminder_message};

fn rule_set(rules: &[(&str, &str)]) -> Arc<RuleSet> {
    let rules = rules
        .iter()
        .map(|(name, yaml_extra)| {
            parse_parts(
                &format!("name: {name}\ndescription: d\n{yaml_extra}"),
                &format!("reminder for {name}"),
                None,
            )
            .unwrap()
        })
        .collect();
    Arc::new(RuleSet::compile(rules))
}

/// A tool that counts its executions — proves tool-arg aborts happen
/// BEFORE the tool runs.
#[derive(Clone, Default)]
struct CountingTool {
    calls: Arc<AtomicUsize>,
}

#[derive(Debug, thiserror::Error)]
#[error("never")]
struct Never;

impl rig_core::tool::Tool for CountingTool {
    const NAME: &'static str = "write";
    type Error = Never;
    type Args = serde_json::Value;
    type Output = String;
    fn description(&self) -> String {
        "write a file".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    async fn call(&self, _args: serde_json::Value) -> Result<String, Never> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("written".into())
    }
}

struct RunSummary {
    text: String,
    fired: Vec<Firing>,
}

/// Drive the agent with the same retry contract as `stream_chat`.
async fn drive(
    model: &MockCompletionModel,
    handle: &RulesHandle,
    rules: &Arc<RuleSet>,
    steering: Option<&SteeringHandle>,
    tool: &CountingTool,
    prompt: &str,
) -> RunSummary {
    handle.note_user_turn();
    let mut seed_prompt = Message::user(prompt);
    let mut seed_history: Vec<Message> = Vec::new();
    let mut fired = Vec::new();
    let retry_budget = handle.retry_budget();
    let mut retries_used = 0u32;
    loop {
        let ttsr = TtsrShared::new(
            handle.clone(),
            Arc::clone(rules),
            false,
            retries_used < retry_budget,
        );
        let mut builder = AgentBuilder::new(model.clone()).tool(tool.clone());
        if let Some(steering) = steering {
            builder = builder.add_hook(SteeringHook(steering.clone()));
        }
        let agent = builder
            .add_hook(TtsrHook(Arc::clone(&ttsr)))
            .default_max_turns(8)
            .build();
        let mut stream = agent
            .stream_chat(seed_prompt.clone(), seed_history.clone())
            .await;
        let mut text = String::new();
        let mut retried = false;
        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                    delta,
                ))) => text.push_str(&delta.text),
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ReasoningDelta {
                        id: None,
                        reasoning,
                    },
                )) => {
                    if ttsr.push_reasoning(&reasoning) {
                        let firing = ttsr.take_pending().expect("reasoning firing stashed");
                        let (committed, _) = ttsr.committed();
                        seed_history = committed;
                        seed_prompt = reminder_message(&firing);
                        fired.push(firing);
                        retries_used += 1;
                        retried = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    if let Some(firing) = ttsr.take_pending()
                        && let StreamingError::Prompt(boxed) = &error
                        && let PromptError::PromptCancelled { chat_history, .. } = boxed.as_ref()
                    {
                        seed_history = chat_history.clone();
                        seed_prompt = reminder_message(&firing);
                        fired.push(firing);
                        retries_used += 1;
                        retried = true;
                        break;
                    }
                    panic!("unexpected stream error: {error}");
                }
            }
        }
        if retried {
            continue;
        }
        return RunSummary { text, fired };
    }
}

fn request_text(request: &CompletionRequest) -> String {
    request
        .chat_history
        .iter()
        .map(|message| format!("{message:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn text_match_aborts_and_offending_text_never_reenters_context() {
    let model = MockCompletionModel::from_stream_turns([
        // Attempt 1: the model starts down the forbidden path.
        vec![
            MockStreamEvent::text("Let me just use Box::leak here"),
            MockStreamEvent::text(" and move on.\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        // Retry: course-corrected.
        vec![
            MockStreamEvent::text("Using Arc<str> instead.\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);
    let handle = RulesHandle::default();
    let rules = rule_set(&[("no-leak", "patterns: ['Box::leak']")]);

    let summary = drive(
        &model,
        &handle,
        &rules,
        None,
        &CountingTool::default(),
        "allocate a string",
    )
    .await;

    assert_eq!(summary.fired.len(), 1, "rule fired exactly once");
    assert_eq!(summary.fired[0].rule.0, "no-leak");
    assert_eq!(summary.text, "Using Arc<str> instead.\n");

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    // The retry request: original prompt + reminder, and the aborted
    // partial turn is nowhere in it.
    let retry = request_text(&requests[1]);
    assert!(!retry.contains("Box::leak\u{0}"), "sanity");
    assert!(
        !retry.contains("Let me just use"),
        "offending partial output leaked into retry context: {retry}"
    );
    assert!(retry.contains("allocate a string"));
    assert!(retry.contains("system-reminder"));
    assert!(retry.contains("reminder for no-leak"));
}

#[tokio::test(flavor = "multi_thread")]
async fn committed_tool_round_trip_survives_the_abort() {
    let model = MockCompletionModel::from_stream_turns([
        // Turn 1: a tool call (committed).
        vec![
            MockStreamEvent::tool_call("fc_1", "write", serde_json::json!({"path": "a.rs"})),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        // Turn 2: offending text → abort.
        vec![
            MockStreamEvent::text("now some mock data\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        // Retry.
        vec![
            MockStreamEvent::text("done properly\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);
    let handle = RulesHandle::default();
    let rules = rule_set(&[("no-mock", "patterns: ['mock data']")]);
    let tool = CountingTool::default();

    let summary = drive(&model, &handle, &rules, None, &tool, "write a.rs").await;

    assert_eq!(summary.text, "done properly\n");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1, "tool ran once");
    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    let retry = request_text(&requests[2]);
    assert!(
        retry.contains("fc_1") && retry.contains("written"),
        "committed tool call + result must ride into the retry seed: {retry}"
    );
    assert!(!retry.contains("now some mock data"));
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_arg_match_aborts_before_the_tool_executes() {
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::tool_call(
                "fc_1",
                "write",
                serde_json::json!({"content": "except: pass"}),
            ),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("handled the error instead\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);
    let handle = RulesHandle::default();
    let rules = rule_set(&[(
        "no-swallow",
        "targets: [tool-args]\npatterns: ['except: pass']\ntools: [write]",
    )]);
    let tool = CountingTool::default();

    let summary = drive(&model, &handle, &rules, None, &tool, "fix it").await;

    assert_eq!(summary.fired.len(), 1);
    assert_eq!(
        tool.calls.load(Ordering::SeqCst),
        0,
        "the tool must never execute on an arg match"
    );
    assert_eq!(summary.text, "handled the error instead\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn once_per_session_lets_a_repeat_through_with_reminder_in_context() {
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::text("Box::leak attempt one\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        // Retry re-emits the same pattern — the rule already fired, so the
        // turn completes with the reminder in context (oh-my-pi semantics).
        vec![
            MockStreamEvent::text("stubborn: Box::leak again\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);
    let handle = RulesHandle::default();
    let rules = rule_set(&[("no-leak", "patterns: ['Box::leak']")]);

    let summary = drive(
        &model,
        &handle,
        &rules,
        None,
        &CountingTool::default(),
        "go",
    )
    .await;

    assert_eq!(summary.fired.len(), 1);
    assert_eq!(summary.text, "stubborn: Box::leak again\n");
    assert_eq!(model.requests().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn session_persistence_reinjects_on_later_turns() {
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::text("Box::leak oops\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("fixed\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        // A later user turn: the reminder must arrive via extra_context.
        vec![
            MockStreamEvent::text("second answer\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);
    let handle = RulesHandle::default();
    let rules = rule_set(&[("no-leak", "patterns: ['Box::leak']\npersistence: session")]);

    drive(
        &model,
        &handle,
        &rules,
        None,
        &CountingTool::default(),
        "first",
    )
    .await;
    drive(
        &model,
        &handle,
        &rules,
        None,
        &CountingTool::default(),
        "second",
    )
    .await;

    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    let later_docs = requests[2]
        .documents
        .iter()
        .map(|document| document.text.clone())
        .collect::<String>();
    assert!(
        later_docs.contains("reminder for no-leak"),
        "session-persistent reminder must re-inject on later turns: {later_docs:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_budget_exhaustion_degrades_to_inject_only() {
    // Three distinct per-turn rules, budget of 1: the first firing aborts,
    // the second degrades to inject-only and the turn completes.
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::text("alpha trigger\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("beta trigger\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);
    let handle = RulesHandle::new(1);
    let rules = rule_set(&[
        ("alpha", "patterns: ['alpha trigger']"),
        ("beta", "patterns: ['beta trigger']"),
    ]);

    let summary = drive(
        &model,
        &handle,
        &rules,
        None,
        &CountingTool::default(),
        "go",
    )
    .await;

    assert_eq!(summary.fired.len(), 1, "only the first firing aborts");
    assert_eq!(summary.text, "beta trigger\n");
    // Both reminders are active injections for future turns.
    assert_eq!(handle.injections().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn reasoning_summary_match_aborts_from_the_driver_side() {
    let model = MockCompletionModel::from_stream_turns([
        vec![
            MockStreamEvent::reasoning_delta(None::<String>, "I will just mock the data\n"),
            MockStreamEvent::text("never reached\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("using real data\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);
    let handle = RulesHandle::default();
    let rules = rule_set(&[(
        "no-mock-intent",
        "targets: [reasoning-summary]\npatterns: ['mock the data']",
    )]);

    let summary = drive(
        &model,
        &handle,
        &rules,
        None,
        &CountingTool::default(),
        "load the data",
    )
    .await;

    assert_eq!(summary.fired.len(), 1);
    assert_eq!(summary.text, "using real data\n");
    let retry = request_text(&model.requests()[1]);
    assert!(retry.contains("load the data"));
    assert!(retry.contains("system-reminder"));
    assert!(!retry.contains("never reached"));
}

#[tokio::test(flavor = "multi_thread")]
async fn delivered_steering_survives_abort_without_double_delivery() {
    let model = MockCompletionModel::from_stream_turns([
        // Turn 1: tool call — steering delivers on its result.
        vec![
            MockStreamEvent::tool_call("fc_1", "write", serde_json::json!({"path": "a"})),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        // Turn 2: offending text → abort.
        vec![
            MockStreamEvent::text("Box::leak\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        // Retry.
        vec![
            MockStreamEvent::text("ok\n"),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);
    let handle = RulesHandle::default();
    let rules = rule_set(&[("no-leak", "patterns: ['Box::leak']")]);
    let steering = SteeringHandle::default();
    steering.enqueue("also check b".into());
    let tool = CountingTool::default();

    let summary = drive(&model, &handle, &rules, Some(&steering), &tool, "go").await;

    assert_eq!(summary.text, "ok\n");
    let retry = request_text(&model.requests()[2]);
    assert_eq!(
        retry.matches("also check b").count(),
        1,
        "steering must ride the committed tool result into the retry exactly once: {retry}"
    );
}
