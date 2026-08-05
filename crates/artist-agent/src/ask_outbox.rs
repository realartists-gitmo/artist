//! Durable post-and-poll questions for MCP and other disconnected surfaces.

use artist_session::ask::{Answer, Question, QuestionOption};
use artist_tool_api::{
    ArtistDynamicTool, ArtistToolAnnotations, ArtistToolDefinition, ArtistToolOutput, ToolCategory,
    schema_for,
};
use rig_core::tool::{ToolErrorKind, ToolExecutionError, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ask_tool::{AskArgs, into_questions, validate_questions};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct QuestionOptionView {
    label: String,
    description: String,
    preview: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct QuestionView {
    id: String,
    header: String,
    question: String,
    multi_select: bool,
    options: Vec<QuestionOptionView>,
}

impl From<&Question> for QuestionView {
    fn from(question: &Question) -> Self {
        Self {
            id: question.id.clone(),
            header: question.header.clone(),
            question: question.question.clone(),
            multi_select: question.multi_select,
            options: question
                .options
                .iter()
                .map(|option: &QuestionOption| QuestionOptionView {
                    label: option.label.clone(),
                    description: option.description.clone(),
                    preview: option.preview.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PostedQuestion {
    question_id: String,
    question: String,
    status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
struct AskPostedResult {
    questions: Vec<PostedQuestion>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
enum AskPollItem {
    Pending {
        #[serde(rename = "questionId")]
        question_id: String,
        question: Option<QuestionView>,
    },
    Answered {
        #[serde(rename = "questionId")]
        question_id: String,
        answer: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
struct AskPollResult {
    results: Vec<AskPollItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct AskAnswerResult {
    question_id: String,
    recorded: bool,
    answer: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
struct AskListResult {
    pending: Vec<QuestionView>,
}

pub fn tools(outbox: artist_session::AskOutbox) -> Vec<ArtistDynamicTool> {
    vec![
        ask(outbox.clone()),
        ask_result(outbox.clone()),
        ask_answer(outbox.clone()),
        ask_list(outbox),
    ]
}

fn definition<T: JsonSchema>(
    name: &str,
    title: &str,
    description: &str,
    input_schema: Value,
    idempotent: bool,
) -> ArtistToolDefinition {
    ArtistToolDefinition {
        name: name.to_owned(),
        title: title.to_owned(),
        description: description.to_owned(),
        input_schema,
        output_schema: schema_for::<T>(),
        category: ToolCategory::UserInteraction,
        annotations: ArtistToolAnnotations {
            read_only: name == "ask_result" || name == "ask_list",
            destructive: false,
            idempotent,
            open_world: true,
        },
    }
}

fn response<T: Serialize>(value: &T, text: String) -> Result<ArtistToolOutput, ToolExecutionError> {
    Ok(ArtistToolOutput {
        presentation: ToolOutput::text(text),
        structured: serde_json::to_value(value).map_err(ToolExecutionError::from_error)?,
    })
}

fn ask(outbox: artist_session::AskOutbox) -> ArtistDynamicTool {
    ArtistDynamicTool::new(
        definition::<AskPostedResult>(
            "ask",
            "Ask User",
            "Post durable questions and return immediately with their identifiers.",
            crate::ask_tool::ask_parameters(),
            false,
        ),
        move |arguments: Value| {
            let outbox = outbox.clone();
            Box::pin(async move {
                let args: AskArgs = serde_json::from_value(arguments)
                    .map_err(|error| ToolExecutionError::invalid_args(error.to_string()))?;
                validate_questions(&args.questions).map_err(ToolExecutionError::invalid_args)?;
                let questions = into_questions(args);
                for question in &questions {
                    outbox.post(question.clone()).map_err(|error| {
                        ToolExecutionError::new(
                            ToolErrorKind::Other,
                            format!("posting question failed: {error}"),
                        )
                    })?;
                }
                let result = AskPostedResult {
                    questions: questions
                        .iter()
                        .map(|question| PostedQuestion {
                            question_id: question.id.clone(),
                            question: question.question.clone(),
                            status: "pending".into(),
                        })
                        .collect(),
                };
                let text = result
                    .questions
                    .iter()
                    .map(|question| {
                        format!(
                            "posted {} (awaiting answer): {}",
                            question.question_id, question.question
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                response(&result, text)
            })
        },
    )
}

fn ask_result(outbox: artist_session::AskOutbox) -> ArtistDynamicTool {
    ArtistDynamicTool::new(
        definition::<AskPollResult>(
            "ask_result",
            "Poll User Questions",
            "Return pending or answered state for each durable question identifier.",
            json!({
                "type": "object",
                "properties": {
                    "questionIds": {"type": "array", "minItems": 1, "items": {"type": "string"}}
                },
                "required": ["questionIds"],
                "additionalProperties": false
            }),
            true,
        ),
        move |arguments: Value| {
            let outbox = outbox.clone();
            Box::pin(async move {
                let ids: Vec<String> = arguments
                    .get("questionIds")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let result = AskPollResult {
                    results: ids
                        .iter()
                        .map(|id| match outbox.result(id) {
                            Some(answer) => AskPollItem::Answered {
                                question_id: id.clone(),
                                answer: answer.describe(),
                            },
                            None => AskPollItem::Pending {
                                question_id: id.clone(),
                                question: outbox.question(id).as_ref().map(QuestionView::from),
                            },
                        })
                        .collect(),
                };
                response(
                    &result,
                    serde_json::to_string(&result).map_err(ToolExecutionError::from_error)?,
                )
            })
        },
    )
}

fn ask_answer(outbox: artist_session::AskOutbox) -> ArtistDynamicTool {
    ArtistDynamicTool::new(
        definition::<AskAnswerResult>(
            "ask_answer",
            "Record User Answer",
            "Record an answer to a durable question; the first answer wins.",
            json!({
                "type": "object",
                "properties": {
                    "questionId": {"type": "string"},
                    "selected": {"type": "array", "items": {"type": "string"}},
                    "notes": {"type": "string"}
                },
                "required": ["questionId", "selected"],
                "additionalProperties": false
            }),
            false,
        ),
        move |arguments: Value| {
            let outbox = outbox.clone();
            Box::pin(async move {
                let question_id = arguments
                    .get("questionId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolExecutionError::invalid_args("questionId is required"))?
                    .to_owned();
                if outbox.question(&question_id).is_none() && outbox.result(&question_id).is_none()
                {
                    return Err(ToolExecutionError::not_found(format!(
                        "no pending or answered question with id {question_id}"
                    ))
                    .with_code("question_not_found")
                    .with_retryable(false));
                }

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
                let recorded = outbox.answer(answer.clone()).map_err(|error| {
                    ToolExecutionError::new(
                        ToolErrorKind::Other,
                        format!("recording answer failed: {error}"),
                    )
                })?;
                let stored = if recorded {
                    answer
                } else {
                    outbox.result(&answer.question_id).unwrap_or(answer)
                };
                let result = AskAnswerResult {
                    question_id: stored.question_id.clone(),
                    recorded,
                    answer: stored.describe(),
                };
                let qualifier = if recorded {
                    ""
                } else {
                    " (ignored: already answered)"
                };
                response(
                    &result,
                    format!(
                        "recorded answer for {}{}: {}",
                        result.question_id, qualifier, result.answer
                    ),
                )
            })
        },
    )
}

fn ask_list(outbox: artist_session::AskOutbox) -> ArtistDynamicTool {
    ArtistDynamicTool::new(
        definition::<AskListResult>(
            "ask_list",
            "List Pending Questions",
            "List every durable question currently awaiting an answer.",
            json!({"type": "object", "additionalProperties": false}),
            true,
        ),
        move |_arguments: Value| {
            let outbox = outbox.clone();
            Box::pin(async move {
                let result = AskListResult {
                    pending: outbox.pending().iter().map(QuestionView::from).collect(),
                };
                response(
                    &result,
                    serde_json::to_string(&result).map_err(ToolExecutionError::from_error)?,
                )
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbox_tools() -> Vec<ArtistDynamicTool> {
        tools(artist_session::AskOutbox::default())
    }

    async fn output(tool: &ArtistDynamicTool, args: Value) -> ArtistToolOutput {
        tool.execute(args)
            .await
            .unwrap_or_else(|error| panic!("{} should succeed: {error:?}", tool.name()))
    }

    #[tokio::test]
    async fn ask_returns_ids_without_blocking_then_ask_result_polls() {
        let all = outbox_tools();
        let ask = all.iter().find(|tool| tool.name() == "ask").unwrap();
        let ask_result = all.iter().find(|tool| tool.name() == "ask_result").unwrap();
        let posted = output(
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
        let id = posted.structured["data"]["questions"][0]["questionId"]
            .as_str()
            .unwrap();
        assert!(posted.presentation.render().contains("awaiting answer"));
        let pending = output(ask_result, json!({"questionIds": [id]})).await;
        assert_eq!(
            pending.structured["data"]["results"][0]["status"],
            "pending"
        );
        assert_eq!(
            pending.structured["data"]["results"][0]["question"]["question"],
            "Where should canvases live?"
        );
    }

    #[tokio::test]
    async fn ask_answer_records_and_first_wins() {
        let all = outbox_tools();
        let ask = all.iter().find(|tool| tool.name() == "ask").unwrap();
        let ask_answer = all.iter().find(|tool| tool.name() == "ask_answer").unwrap();
        let ask_result = all.iter().find(|tool| tool.name() == "ask_result").unwrap();
        let posted = output(
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
        let id = posted.structured["data"]["questions"][0]["questionId"]
            .as_str()
            .unwrap();
        let first = output(
            ask_answer,
            json!({"questionId": id, "selected": ["OpenAI"]}),
        )
        .await;
        assert_eq!(first.structured["data"]["recorded"], true);
        let second = output(
            ask_answer,
            json!({"questionId": id, "selected": ["Anthropic"]}),
        )
        .await;
        assert_eq!(second.structured["data"]["recorded"], false);
        assert!(
            second.structured["data"]["answer"]
                .as_str()
                .unwrap()
                .contains("OpenAI")
        );
        let polled = output(ask_result, json!({"questionIds": [id]})).await;
        assert_eq!(
            polled.structured["data"]["results"][0]["status"],
            "answered"
        );
    }

    #[tokio::test]
    async fn ask_answer_rejects_an_unknown_question_id_explicitly() {
        let all = outbox_tools();
        let ask_answer = all.iter().find(|tool| tool.name() == "ask_answer").unwrap();
        let error = ask_answer
            .execute(json!({"questionId": "q-missing", "selected": []}))
            .await
            .unwrap_err();

        assert_eq!(error.kind(), ToolErrorKind::NotFound);
        assert_eq!(error.code(), Some("question_not_found"));
        assert!(
            error
                .model_feedback()
                .is_some_and(|message| message.contains("q-missing"))
        );
    }
    #[tokio::test]
    async fn ask_list_is_empty_then_round_trips() {
        let all = outbox_tools();
        let ask = all.iter().find(|tool| tool.name() == "ask").unwrap();
        let ask_list = all.iter().find(|tool| tool.name() == "ask_list").unwrap();
        assert_eq!(
            output(ask_list, json!({})).await.structured["data"]["pending"],
            json!([])
        );
        output(
            ask,
            json!({"questions": [{
                "question": "Which provider?",
                "header": "Provider",
                "options": [
                    {"label": "OpenAI", "description": "Default"},
                    {"label": "Anthropic", "description": "Alternative"}
                ]
            }]}),
        )
        .await;
        let listed = output(ask_list, json!({})).await;
        assert_eq!(
            listed.structured["data"]["pending"][0]["question"],
            "Which provider?"
        );
    }
}
