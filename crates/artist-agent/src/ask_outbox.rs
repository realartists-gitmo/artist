//! Durable ask tools for surfaces with no in-process answerer.
//!
//! The blocking [`AskTool`](crate::ask_tool::AskTool) waits on an in-memory
//! [`AskRegistry`](artist_session::AskRegistry) answered by a picker or canvas
//! in the same process. Over MCP that contract breaks — the connection dies
//! every few minutes and the person answering is not in the MCP process — so a
//! blocked call would outlive the transport and its question with it. These
//! tools speak to the durable [`AskOutbox`](artist_session::AskOutbox) instead:
//! posting is non-blocking, the question survives reconnect, and the answer is
//! recorded by whoever gets to it first.
//!
//! The post-and-poll shape is what the tunnel can actually sustain:
//!
//! * `ask` writes the questions to the outbox and returns immediately with
//!   their ids.
//! * `ask_result` polls by id — "still pending" or the recorded answer.
//! * `ask_answer` records the user's choice, first answer wins.
//! * `ask_list` returns everything currently pending, so a fresh connection
//!   can rediscover work its predecessor posted.

use artist_session::ask::{Answer, Question, QuestionOption};
use rig_core::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{Value, json};

use crate::ask_tool::{AskArgs, into_questions, validate_questions};

/// Build the durable ask tools, bound to `outbox`.
pub fn tools(outbox: artist_session::AskOutbox) -> Vec<PortableDynamicTool> {
    vec![
        ask(outbox.clone()),
        ask_result(outbox.clone()),
        ask_answer(outbox.clone()),
        ask_list(outbox),
    ]
}

/// The on-the-wire shape of a question, shared by `ask_result` (pending) and
/// `ask_list` so both present the same vocabulary.
fn question_json(question: &Question) -> Value {
    json!({
        "id": question.id,
        "header": question.header,
        "question": question.question,
        "multiSelect": question.multi_select,
        "options": question.options.iter().map(|option: &QuestionOption| json!({
            "label": option.label,
            "description": option.description,
            "preview": option.preview,
        })).collect::<Vec<_>>(),
    })
}

fn ask(outbox: artist_session::AskOutbox) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ask",
        "Pose questions to the user and return immediately. The questions are \
         written to a durable outbox — they survive the connection dying — and \
         are answered out-of-band by the user. Do not block waiting for the \
         answer; poll ask_result with the returned question ids. Reserve it for \
         choices where different answers lead to materially different work, and \
         make routine judgement calls yourself.",
        crate::ask_tool::ask_parameters(),
        move |arguments: Value| {
            let outbox = outbox.clone();
            Box::pin(async move {
                let args: AskArgs =
                    serde_json::from_value(arguments).map_err(|error| {
                        ToolExecutionError::invalid_args(error.to_string())
                    })?;
                validate_questions(&args.questions)
                    .map_err(ToolExecutionError::invalid_args)?;
                let questions = into_questions(args);
                for question in &questions {
                    outbox.post(question.clone()).map_err(|error| {
                        ToolExecutionError::new(
                            rig_core::tool::ToolErrorKind::Other,
                            format!("posting question failed: {error}"),
                        )
                    })?;
                }
                Ok(ToolOutput::text(
                    questions
                        .iter()
                        .map(|question| {
                            format!("posted {} (awaiting answer): {}", question.id, question.question)
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ))
            })
        },
    )
}

fn ask_result(outbox: artist_session::AskOutbox) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ask_result",
        "Poll questions previously posted with ask. For each question id \
         returns whether it is still pending or has been answered, and the \
         answer when one exists.",
        json!({
            "type": "object",
            "properties": {
                "questionIds": {
                    "type": "array",
                    "minItems": 1,
                    "items": {"type": "string"}
                }
            },
            "required": ["questionIds"],
            "additionalProperties": false
        }),
        move |arguments: Value| {
            let outbox = outbox.clone();
            Box::pin(async move {
                let ids: Vec<String> = arguments
                    .get("questionIds")
                    .and_then(|value| value.as_array())
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let results: Vec<Value> = ids
                    .iter()
                    .map(|id| match outbox.result(id) {
                        Some(answer) => json!({
                            "questionId": id,
                            "status": "answered",
                            "answer": answer.describe(),
                        }),
                        None => json!({
                            "questionId": id,
                            "status": "pending",
                            "question": outbox.question(id).as_ref().map(question_json),
                        }),
                    })
                    .collect();
                Ok(ToolOutput::text(json!({ "results": results }).to_string()))
            })
        },
    )
}

fn ask_answer(outbox: artist_session::AskOutbox) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ask_answer",
        "Record the user's answer to a question previously posted with ask. \
         First answer wins: a second answer to the same question is ignored. \
         Prefer this over ask_result when the user has already answered.",
        json!({
            "type": "object",
            "properties": {
                "questionId": {"type": "string"},
                "selected": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Chosen option labels. Empty means the question was dismissed."
                },
                "notes": {
                    "type": "string",
                    "description": "Free text the user added alongside their choice."
                }
            },
            "required": ["questionId", "selected"],
            "additionalProperties": false
        }),
        move |arguments: Value| {
            let outbox = outbox.clone();
            Box::pin(async move {
                let question_id = arguments
                    .get("questionId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolExecutionError::invalid_args("questionId is required"))?
                    .to_owned();
                let selected = arguments
                    .get("selected")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let notes = arguments
                    .get("notes")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let answer = Answer {
                    question_id,
                    selected,
                    notes,
                };
                let recorded = outbox
                    .answer(answer.clone())
                    .map_err(|error| {
                        ToolExecutionError::new(
                            rig_core::tool::ToolErrorKind::Other,
                            format!("recording answer failed: {error}"),
                        )
                    })?;
                let stored = if recorded {
                    answer.clone()
                } else {
                    // First answer wins: the existing one is what governs. The
                    // user's later word, relayed here, is the race the outbox
                    // exists to resolve — report the survivor, not the loser.
                    outbox.result(&answer.question_id).unwrap_or(answer.clone())
                };
                let suffix = if recorded {
                    ""
                } else {
                    " (ignored: already answered or unknown question)"
                };
                Ok(ToolOutput::text(format!(
                    "recorded answer for {}{}: {}",
                    stored.question_id,
                    suffix,
                    stored.describe()
                )))
            })
        },
    )
}

fn ask_list(outbox: artist_session::AskOutbox) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ask_list",
        "List every question currently awaiting an answer, so a fresh \
         connection can rediscover work its predecessor posted.",
        json!({"type": "object", "additionalProperties": false}),
        move |_arguments: Value| {
            let outbox = outbox.clone();
            Box::pin(async move {
                let questions: Vec<Value> = outbox
                    .pending()
                    .into_iter()
                    .map(|question| question_json(&question))
                    .collect();
                Ok(ToolOutput::text(json!({ "pending": questions }).to_string()))
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbox_tools() -> Vec<PortableDynamicTool> {
        tools(artist_session::AskOutbox::default())
    }

    async fn text_of(tool: &PortableDynamicTool, args: Value) -> String {
        tool.execute(args)
            .await
            .expect("tool call should succeed")
            .into_content()
            .into_iter()
            .filter_map(|content| match content {
                rig_core::completion::message::ToolResultContent::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn ask_returns_ids_without_blocking_then_ask_result_polls() {
        let all = outbox_tools();
        let ask = all.iter().find(|tool| tool.name() == "ask").unwrap();
        let ask_result = all
            .iter()
            .find(|tool| tool.name() == "ask_result")
            .unwrap();

        let posted = text_of(
            ask,
            json!({"questions": [{
                "question": "Where should canvases live?",
                "header": "Storage",
                "options": [
                    {"label": "Project-local", "description": "Durable"},
                    {"label": "Ephemeral", "description": "Tied to a chat"}
                ]
            }]}),
        )
        .await;
        assert!(posted.contains("posted q-"), "got: {posted}");
        assert!(posted.contains("awaiting answer"), "got: {posted}");

        let id = posted.split("posted ").nth(1).unwrap().split(' ').next().unwrap();
        let pending = text_of(ask_result, json!({"questionIds": [id]})).await;
        assert!(
            pending.contains("\"status\":\"pending\"")
                && pending.contains("Where should canvases live?"),
            "pending must carry the question wording: got: {pending}"
        );
    }

    #[tokio::test]
    async fn ask_answer_records_and_first_wins() {
        let all = outbox_tools();
        let ask = all.iter().find(|tool| tool.name() == "ask").unwrap();
        let ask_answer = all
            .iter()
            .find(|tool| tool.name() == "ask_answer")
            .unwrap();
        let ask_result = all
            .iter()
            .find(|tool| tool.name() == "ask_result")
            .unwrap();

        let posted = text_of(
            ask,
            json!({"questions": [{
                "question": "Which provider?",
                "header": "Provider",
                "options": [
                    {"label": "OpenAI", "description": "Default"},
                    {"label": "Anthropic", "description": "Claude"}
                ]
            }]}),
        )
        .await;
        let id = posted.split("posted ").nth(1).unwrap().split(' ').next().unwrap();

        let first = text_of(
            ask_answer,
            json!({"questionId": id, "selected": ["OpenAI"]}),
        )
        .await;
        assert!(first.contains("OpenAI"), "got: {first}");

        let second = text_of(
            ask_answer,
            json!({"questionId": id, "selected": ["Anthropic"]}),
        )
        .await;
        assert!(
            second.contains("ignored") && second.contains("OpenAI"),
            "first answer must win: got: {second}"
        );

        let polled = text_of(ask_result, json!({"questionIds": [id]})).await;
        assert!(
            polled.contains("\"status\":\"answered\"") && polled.contains("OpenAI"),
            "got: {polled}"
        );
    }

    #[tokio::test]
    async fn ask_list_is_empty_then_round_trips() {
        let all = outbox_tools();
        let ask = all.iter().find(|tool| tool.name() == "ask").unwrap();
        let ask_list = all
            .iter()
            .find(|tool| tool.name() == "ask_list")
            .unwrap();

        assert!(
            text_of(ask_list, json!({})).await.contains("\"pending\":[]")
        );

        text_of(
            ask,
            json!({"questions": [{
                "question": "Which provider?",
                "header": "Provider",
                "options": [
                    {"label": "OpenAI", "description": "Default"},
                    {"label": "Anthropic", "description": "Claude"}
                ]
            }]}),
        )
        .await;
        let listed = text_of(ask_list, json!({})).await;
        assert!(
            listed.contains("Which provider?") && listed.contains("OpenAI"),
            "got: {listed}"
        );
    }
}
