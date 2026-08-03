//! Asking the user a structured question.
//!
//! The vocabulary, the registry and the durable events live in
//! [`artist_session::ask`]; this is the model's way in, and
//! `artist-cli/src/ask_ui.rs` is the TUI picker that answers it. A canvas can
//! render the same question through `artist.ask.*` and answer it too — whoever
//! gets there first wins. Both the question and the answer land in the session
//! log, so a resumed or rewound session sees what was decided.
//!
//! Three properties this has to get right, none of them about the question
//! itself:
//!
//! * **A parked agent holds no seat.** Waiting on a human is not work, and N
//!   agents blocked on questions while holding delegation seats stalls the
//!   project — the same reasoning that makes a parent yield while it awaits a
//!   child.
//! * **The wait is cancellable.** A tool call blocked on a person with no way
//!   out is a hole in the agent loop: the turn cannot be cancelled and the
//!   process cannot shut down. Cancelling also retires the question, so a
//!   picker nobody is listening for does not linger on screen.
//! * **No user, no tool.** Headless and one-shot runs have nobody to ask, so
//!   the environment supplies no registry and the tool is absent rather than
//!   present and guaranteed to hang.

use artist_session::ask::{Answer, Question, QuestionOption};
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct AskError(String);

/// How many questions one call may pose.
///
/// Batched rather than one per call because the user answers them together;
/// capped because a picker with a dozen questions stops being a decision and
/// starts being a form.
const MAX_QUESTIONS: usize = 4;
const MIN_OPTIONS: usize = 2;
const MAX_OPTIONS: usize = 4;

#[derive(Clone)]
pub(crate) struct AskTool {
    registry: artist_session::AskRegistry,
    cancel: CancellationToken,
    /// Released while parked on the user — see the module note.
    seat: Option<crate::delegate::PermitSlot>,
}

impl AskTool {
    pub fn new(
        registry: artist_session::AskRegistry,
        cancel: CancellationToken,
        seat: Option<crate::delegate::PermitSlot>,
    ) -> Self {
        Self {
            registry,
            cancel,
            seat,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AskArgs {
    pub questions: Vec<AskQuestion>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AskQuestion {
    pub question: String,
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub multi_select: bool,
    pub options: Vec<AskOption>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AskOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview: Option<String>,
}

/// The JSON schema the model-facing `ask` tool publishes, shared with the
/// durable outbox `ask` tool so they speak the same vocabulary.
pub(crate) fn ask_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_QUESTIONS,
                "description": "Questions to pose together. The user answers them in one pass.",
                "items": {
                    "type": "object",
                    "properties": {
                        "question": {"type": "string", "description": "The complete question. Specific, and ending in a question mark."},
                        "header": {"type": "string", "description": "A couple of words naming the decision, shown as a chip."},
                        "multiSelect": {"type": "boolean", "default": false, "description": "Allow several options to be chosen. Use when the choices are not mutually exclusive."},
                        "options": {
                            "type": "array",
                            "minItems": MIN_OPTIONS,
                            "maxItems": MAX_OPTIONS,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": {"type": "string", "description": "What the user reads. Short."},
                                    "description": {"type": "string", "description": "What choosing it means, and what it costs."},
                                    "preview": {"type": "string", "description": "A concrete artifact to compare against the alternatives — a mockup, a snippet, a config. Rendered monospaced."}
                                },
                                "required": ["label"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["question", "options"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["questions"],
        "additionalProperties": false
    })
}

/// Validate a batch of questions before anything is posted, so a malformed
/// batch never leaves half its questions on the user's screen.
pub(crate) fn validate_questions(questions: &[AskQuestion]) -> Result<(), String> {
    if questions.is_empty() {
        return Err("at least one question is required".into());
    }
    if questions.len() > MAX_QUESTIONS {
        return Err(format!("at most {MAX_QUESTIONS} questions per call"));
    }
    for question in questions {
        if question.options.len() < MIN_OPTIONS {
            return Err(format!(
                "\"{}\" needs at least {MIN_OPTIONS} options; if there is only one way \
                 forward, take it rather than asking",
                question.question
            ));
        }
        if question.options.len() > MAX_OPTIONS {
            return Err(format!(
                "\"{}\" has more than {MAX_OPTIONS} options; narrow them down first",
                question.question
            ));
        }
    }
    Ok(())
}

/// Convert validated args into durable [`Question`]s, one id per question.
pub(crate) fn into_questions(args: AskArgs) -> Vec<Question> {
    args.questions
        .into_iter()
        .map(|asked| Question {
            id: artist_tools::short_id("q"),
            header: asked.header,
            question: asked.question,
            multi_select: asked.multi_select,
            options: asked
                .options
                .into_iter()
                .map(|option| QuestionOption {
                    label: option.label,
                    description: option.description,
                    preview: option.preview,
                })
                .collect(),
        })
        .collect()
}

impl PortableTool for AskTool {
    const NAME: &'static str = "ask";
    type Error = AskError;
    type Args = AskArgs;
    type Output = String;

    fn description(&self) -> String {
        "Ask the user to decide something you cannot resolve from the request, \
         the code, or a sensible default. Reserve it for choices where \
         different answers lead to materially different work — make routine \
         judgement calls yourself. The user can always write a free-text answer \
         instead of picking, so do not add an \"other\" option."
            .into()
    }

    fn parameters(&self) -> Value {
        ask_parameters()
    }

    async fn call(&self, args: AskArgs) -> Result<String, AskError> {
        // Validated before anything is posted, so a malformed batch never
        // leaves half its questions on the user's screen.
        validate_questions(&args.questions).map_err(AskError)?;

        // Kept as parallel vectors rather than pairs so the receivers can be
        // awaited together while the questions stay available to match answers
        // back to afterwards.
        let questions = into_questions(args);
        let mut receivers = Vec::with_capacity(questions.len());
        for question in &questions {
            receivers.push(self.registry.post(question.clone()));
        }

        if let Some(seat) = &self.seat {
            seat.yield_seat().await;
        }
        let outcome = tokio::select! {
            answers = futures::future::join_all(receivers) => Some(answers),
            () = self.cancel.cancelled() => None,
        };
        if let Some(seat) = &self.seat {
            seat.retake().await;
        }

        let Some(answers) = outcome else {
            // Retire only our own questions. `dismiss_all` would also clear a
            // sibling agent's, which is not ours to cancel.
            for question in &questions {
                self.registry
                    .answer_from(Answer::dismissed(question.id.clone()), "cancelled");
            }
            return Err(AskError(
                "the turn was cancelled before the user answered".into(),
            ));
        };

        Ok(questions
            .iter()
            .zip(answers)
            .map(|(question, answer)| {
                let answer = answer
                    .map(|answer: Answer| answer.describe())
                    // A closed channel means the question was retired without
                    // an answer — dismissed from a surface, or the registry
                    // torn down. Reported as a dismissal rather than an error:
                    // the model should carry on with its own judgement, not
                    // treat the tool as broken.
                    .unwrap_or_else(|_| "(dismissed without choosing)".to_owned());
                format!("{}: {answer}", question.question)
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_session::AskRegistry;

    fn args(question: &str, options: &[&str]) -> AskArgs {
        AskArgs {
            questions: vec![AskQuestion {
                question: question.into(),
                header: String::new(),
                multi_select: false,
                options: options
                    .iter()
                    .map(|label| AskOption {
                        label: (*label).into(),
                        description: String::new(),
                        preview: None,
                    })
                    .collect(),
            }],
        }
    }

    fn tool(registry: &AskRegistry, cancel: CancellationToken) -> AskTool {
        AskTool::new(registry.clone(), cancel, None)
    }

    #[tokio::test]
    async fn an_answer_reaches_the_model_with_its_free_text() {
        let registry = AskRegistry::new();
        let asking = {
            let tool = tool(&registry, CancellationToken::new());
            tokio::spawn(async move { tool.call(args("Which store?", &["Sled", "Redb"])).await })
        };

        // Wait for the question to land, then answer it the way a surface would.
        let question = loop {
            if let Some(question) = registry.pending().into_iter().next() {
                break question;
            }
            tokio::task::yield_now().await;
        };
        assert!(registry.answer(Answer {
            question_id: question.id.clone(),
            selected: vec!["Sled".into()],
            notes: Some("but only if it embeds".into()),
        }));

        let rendered = asking.await.unwrap().unwrap();
        assert!(rendered.contains("Which store?"), "{rendered}");
        assert!(rendered.contains("Sled"), "{rendered}");
        assert!(rendered.contains("but only if it embeds"), "{rendered}");
    }

    /// A tool call blocked on a person with no way out would wedge the turn.
    #[tokio::test]
    async fn cancelling_the_turn_releases_the_wait_and_retires_the_question() {
        let registry = AskRegistry::new();
        let cancel = CancellationToken::new();
        let asking = {
            let tool = tool(&registry, cancel.clone());
            tokio::spawn(async move { tool.call(args("Which store?", &["Sled", "Redb"])).await })
        };
        while registry.pending().is_empty() {
            tokio::task::yield_now().await;
        }

        cancel.cancel();
        let error = asking.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("cancelled"), "{error}");
        assert!(
            registry.is_empty(),
            "a question nobody is listening for must not linger on screen"
        );
    }

    /// Dismissal is an answer, not a failure — the model should proceed on its
    /// own judgement rather than treating the tool as broken.
    #[tokio::test]
    async fn a_dismissed_question_reads_as_a_dismissal() {
        let registry = AskRegistry::new();
        let asking = {
            let tool = tool(&registry, CancellationToken::new());
            tokio::spawn(async move { tool.call(args("Which store?", &["Sled", "Redb"])).await })
        };
        while registry.pending().is_empty() {
            tokio::task::yield_now().await;
        }

        registry.dismiss_all();
        let rendered = asking.await.unwrap().unwrap();
        assert!(rendered.contains("dismissed"), "{rendered}");
    }

    /// A batch is answered in one pass, and every answer is matched back to the
    /// question that asked it.
    #[tokio::test]
    async fn a_batch_pairs_every_answer_with_its_question() {
        let registry = AskRegistry::new();
        let asking = {
            let tool = tool(&registry, CancellationToken::new());
            tokio::spawn(async move {
                tool.call(AskArgs {
                    questions: vec![
                        args("Which store?", &["Sled", "Redb"]).questions.remove(0),
                        args("Which runtime?", &["Tokio", "Smol"])
                            .questions
                            .remove(0),
                    ],
                })
                .await
            })
        };
        while registry.pending().len() < 2 {
            tokio::task::yield_now().await;
        }

        for question in registry.pending() {
            let pick = question.options[0].label.clone();
            registry.answer(Answer {
                question_id: question.id.clone(),
                selected: vec![pick],
                notes: None,
            });
        }

        let rendered = asking.await.unwrap().unwrap();
        assert!(rendered.contains("Which store?: Sled"), "{rendered}");
        assert!(rendered.contains("Which runtime?: Tokio"), "{rendered}");
    }

    /// Nothing is posted when the batch is malformed, so a rejected call never
    /// leaves half its questions on the user's screen.
    #[tokio::test]
    async fn a_malformed_batch_posts_nothing() {
        let registry = AskRegistry::new();
        let tool = tool(&registry, CancellationToken::new());

        assert!(tool.call(AskArgs { questions: vec![] }).await.is_err());
        let one_option = tool.call(args("Proceed?", &["Yes"])).await.unwrap_err();
        assert!(
            one_option.to_string().contains("at least 2 options"),
            "{one_option}"
        );
        assert!(
            registry.is_empty(),
            "a rejected batch must leave nothing pending"
        );
    }
}
