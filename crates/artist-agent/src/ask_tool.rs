//! Non-blocking structured human-decision session spawn.

use std::{path::PathBuf, sync::Arc, time::Duration};

use artist_session::ask::{AskRegistry, DurableAskState, Question, QuestionOption};
use futures::future::BoxFuture;
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::session_tools::{OwnedSession, OwnedState, SessionHub};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct AskError(String);

impl From<AskError> for ToolExecutionError {
    fn from(value: AskError) -> Self {
        ToolExecutionError::other(value.to_string()).with_code("ask_error")
    }
}

#[derive(Clone)]
pub(crate) struct AskTool {
    registry: AskRegistry,
    sessions: SessionHub,
    project: PathBuf,
}

impl AskTool {
    pub fn new(registry: AskRegistry, sessions: SessionHub, project: PathBuf) -> Self {
        Self {
            registry,
            sessions,
            project,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AskArgs {
    pub questions: Vec<AskQuestion>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AskQuestion {
    pub question: String,
    #[serde(default)]
    pub options: Vec<AskOption>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AskOption {
    pub label: String,
    pub recommended: bool,
}

pub(crate) fn ask_parameters() -> Value {
    json!({
        "type":"object",
        "properties":{
            "questions":{
                "type":"array",
                "minItems":1,
                "items":{
                    "type":"object",
                    "properties":{
                        "question":{"type":"string"},
                        "options":{
                            "type":"array",
                            "items":{
                                "type":"object",
                                "properties":{
                                    "label":{"type":"string"},
                                    "recommended":{"type":"boolean"}
                                },
                                "required":["label","recommended"],
                                "additionalProperties":false
                            }
                        }
                    },
                    "required":["question"],
                    "additionalProperties":false
                }
            }
        },
        "required":["questions"],
        "additionalProperties":false
    })
}

struct AskSession {
    registry: AskRegistry,
    sessions: artist_registry::Sessions,
    id: String,
}

impl OwnedSession for AskSession {
    fn state(&self) -> BoxFuture<'_, Result<OwnedState, String>> {
        Box::pin(async move {
            let record = self
                .sessions
                .get(&self.id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("ask session `{}` disappeared", self.id))?;
            match record.lifecycle {
                artist_registry::SessionLifecycle::Live => Ok(OwnedState::live(record.snapshot)),
                artist_registry::SessionLifecycle::Stopped { status } => {
                    Ok(OwnedState::stopped(status, record.snapshot))
                }
            }
        })
    }

    fn send(&self, _input: Value) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Err("model-to-ask input is unsupported".into()) })
    }

    fn abort(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.registry.cancel_session(&self.id);
            Ok(())
        })
    }
}

impl PortableTool for AskTool {
    const NAME: &'static str = "ask";
    type Error = AskError;
    type Args = AskArgs;
    type Output = String;

    fn description(&self) -> String {
        "Post one or more human questions as a durable ask session and return ask:<slug> immediately. Options are always multi-select; omit options for free response. Every option must include recommended.".into()
    }

    fn parameters(&self) -> Value {
        ask_parameters()
    }

    async fn call(&self, args: AskArgs) -> Result<String, AskError> {
        validate_questions(&args.questions)?;
        let first = args
            .questions
            .first()
            .expect("validated non-empty")
            .question
            .clone();
        let record = self
            .sessions
            .registry()
            .create_content(
                "ask",
                &first,
                self.sessions.artist(),
                None,
                json!({"initializing":true}),
            )
            .map_err(|error| AskError(error.to_string()))?;

        let questions = into_questions(&record.id, args);
        let state = DurableAskState::new(questions.clone());
        self.sessions
            .registry()
            .set_snapshot(
                &record.id,
                serde_json::to_value(&state).map_err(|error| AskError(error.to_string()))?,
            )
            .map_err(|error| AskError(error.to_string()))?;
        self.registry.post_batch(&questions);
        self.sessions.own(
            record.id.clone(),
            Arc::new(AskSession {
                registry: self.registry.clone(),
                sessions: self.sessions.registry().clone(),
                id: record.id.clone(),
            }),
        );

        if let Some(timeout) = auto_resolve_timeout(&self.project) {
            let registry = self.registry.clone();
            let id = record.id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                let _ = registry.auto_resolve(&id);
            });
        }
        Ok(record.id)
    }
}

pub(crate) fn model_snapshot(snapshot: &Value) -> Value {
    serde_json::from_value::<DurableAskState>(snapshot.clone())
        .map(|state| state.model)
        .unwrap_or_else(|_| snapshot.clone())
}

fn validate_questions(questions: &[AskQuestion]) -> Result<(), AskError> {
    if questions.is_empty() {
        return Err(AskError("at least one question is required".into()));
    }
    for question in questions {
        if question.question.trim().is_empty() {
            return Err(AskError("question text cannot be empty".into()));
        }
        if question
            .options
            .iter()
            .any(|option| option.label.trim().is_empty())
        {
            return Err(AskError("option labels cannot be empty".into()));
        }
    }
    Ok(())
}

fn into_questions(session: &str, args: AskArgs) -> Vec<Question> {
    args.questions
        .into_iter()
        .map(|asked| Question {
            id: artist_tools::short_id("q"),
            ask_session: session.to_owned(),
            question: asked.question,
            options: asked
                .options
                .into_iter()
                .map(|option| QuestionOption {
                    id: artist_tools::short_id("o"),
                    label: option.label,
                    recommended: option.recommended,
                })
                .collect(),
        })
        .collect()
}

#[derive(Default, Deserialize)]
struct ProjectConfig {
    #[serde(default)]
    ask: AskConfig,
}
#[derive(Default, Deserialize)]
struct AskConfig {
    #[serde(rename = "autoResolveMs")]
    auto_resolve_ms: Option<u64>,
}

fn auto_resolve_timeout(project: &std::path::Path) -> Option<Duration> {
    let config = std::fs::read_to_string(project.join(".artist/config.toml")).ok()?;
    let parsed: ProjectConfig = toml::from_str(&config).ok()?;
    parsed.ask.auto_resolve_ms.map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_session::ask::{Answer, Selection};

    #[test]
    fn schema_has_no_caps_or_legacy_question_fields() {
        let schema = ask_parameters();
        let q = &schema["properties"]["questions"];
        assert!(q.get("maxItems").is_none());
        let props = &q["items"]["properties"];
        assert!(props.get("header").is_none());
        assert!(props.get("multiSelect").is_none());
        assert!(props["options"].get("minItems").is_none());
        assert!(props["options"].get("maxItems").is_none());
        assert_eq!(
            props["options"]["items"]["required"],
            json!(["label", "recommended"])
        );
    }

    #[test]
    fn zero_option_question_is_valid_free_response() {
        validate_questions(&[AskQuestion {
            question: "What should it say?".into(),
            options: Vec::new(),
        }])
        .unwrap();
    }

    #[test]
    fn model_projection_never_contains_internal_ids_or_source() {
        let q = Question {
            id: "q-secret".into(),
            ask_session: "ask:pick".into(),
            question: "Pick".into(),
            options: vec![QuestionOption {
                id: "o-secret".into(),
                label: "One".into(),
                recommended: false,
            }],
        };
        let mut state = DurableAskState::new(vec![q.clone()]);
        state.answers.push(artist_session::ask::DurableAnswer {
            answer: Answer {
                question_id: q.id,
                selections: vec![Selection {
                    option_id: Some("o-secret".into()),
                    note: None,
                }],
            },
            source: artist_session::ask::AnswerSource::Human,
        });
        state.refresh_model();
        let rendered = model_snapshot(&serde_json::to_value(state).unwrap()).to_string();
        assert!(!rendered.contains("q-secret"));
        assert!(!rendered.contains("o-secret"));
        assert!(!rendered.contains("Human"));
        assert!(rendered.contains("chose 1"));
    }
}
