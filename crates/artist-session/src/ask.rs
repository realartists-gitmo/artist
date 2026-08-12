//! Durable structured human decisions.
//!
//! A posted question is part of an `ask:<slug>` universal session. The live UI
//! registry is only a renderer/answer route; the complete posted questions and
//! answers live in the project session registry, so reconnects and other Artist
//! processes can reconstruct them.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
};

use artist_registry::{SessionLifecycle, SessionStatus, Sessions};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestionOption {
    /// Harness-internal stable id. Never projected into model-facing results.
    pub id: String,
    pub label: String,
    pub recommended: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Question {
    /// Harness-internal stable id. Never projected into model-facing results.
    pub id: String,
    /// Owning public `ask:<slug>` session id.
    pub ask_session: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Selection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Answer {
    pub question_id: String,
    #[serde(default)]
    pub selections: Vec<Selection>,
}

impl Answer {
    pub fn dismissed(question_id: impl Into<String>) -> Self {
        Self {
            question_id: question_id.into(),
            selections: Vec::new(),
        }
    }

    /// Model-facing projection is by original option index, never by internal id.
    pub fn describe_for(&self, question: &Question) -> String {
        if self.selections.is_empty() {
            return "(dismissed)".into();
        }
        if question.options.is_empty() {
            let free = self
                .selections
                .iter()
                .filter_map(|selection| selection.note.as_deref())
                .map(str::trim)
                .filter(|note| !note.is_empty())
                .collect::<Vec<_>>();
            return if free.is_empty() {
                "(dismissed)".into()
            } else {
                free.join("\n")
            };
        }

        let by_id = question
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| (option.id.as_str(), index + 1))
            .collect::<HashMap<_, _>>();
        let choices = self
            .selections
            .iter()
            .filter_map(|selection| {
                let index = by_id.get(selection.option_id.as_deref()?)?;
                let note = selection
                    .note
                    .as_deref()
                    .map(str::trim)
                    .filter(|note| !note.is_empty());
                Some(match note {
                    Some(note) => format!("{index} — {note}"),
                    None => index.to_string(),
                })
            })
            .collect::<Vec<_>>();
        if choices.is_empty() {
            "(dismissed)".into()
        } else {
            format!("chose {}", choices.join(", "))
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AnswerSource {
    Human,
    AutoResolve,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableAnswer {
    pub answer: Answer,
    pub source: AnswerSource,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableAskState {
    pub questions: Vec<Question>,
    #[serde(default)]
    pub answers: Vec<DurableAnswer>,
    /// Precomputed model-facing projection. It deliberately contains no q-/o-
    /// ids, but retains Human versus AutoResolve provenance because those are
    /// semantically distinct for the live model and Muse normalization.
    pub model: serde_json::Value,
}

impl DurableAskState {
    pub fn new(questions: Vec<Question>) -> Self {
        let mut state = Self {
            questions,
            answers: Vec::new(),
            model: serde_json::Value::Null,
        };
        state.refresh_model();
        state
    }

    pub fn refresh_model(&mut self) {
        let answers = self
            .questions
            .iter()
            .map(|question| {
                self.answers
                    .iter()
                    .find(|answered| answered.answer.question_id == question.id)
                    .map(|answered| {
                        serde_json::json!({
                            "value": answered.answer.describe_for(question),
                            "source": match answered.source {
                                AnswerSource::Human => "Human",
                                AnswerSource::AutoResolve => "AutoResolve",
                            }
                        })
                    })
            })
            .collect::<Vec<_>>();
        self.model = serde_json::json!({
            "questions": self.questions.iter().map(|question| serde_json::json!({
                "question": question.question,
                "options": question.options.iter().map(|option| &option.label).collect::<Vec<_>>()
            })).collect::<Vec<_>>(),
            "answers": answers,
            "pending": self.questions.len().saturating_sub(self.answers.len())
        });
    }

    pub fn complete(&self) -> bool {
        self.answers.len() == self.questions.len()
    }
}

#[derive(Clone, Default)]
pub struct AskRegistry {
    pending: Arc<Mutex<HashMap<String, Waiting>>>,
    recorder: Arc<Mutex<Option<crate::Recorder>>>,
    sessions: Option<Sessions>,
}

struct Waiting {
    question: Question,
    respond: Option<oneshot::Sender<Answer>>,
}

impl AskRegistry {
    /// Process-local registry, primarily for isolated tests/surfaces without a project.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_recorder(recorder: crate::Recorder) -> Self {
        Self {
            pending: Arc::default(),
            recorder: Arc::new(Mutex::new(Some(recorder))),
            sessions: None,
        }
    }

    /// Project-backed registry. Pending asks are read from the same universal
    /// records used by `poll`, so another process can answer them after restart.
    pub fn for_project(project: &Path, recorder: Option<crate::Recorder>) -> Self {
        Self {
            pending: Arc::default(),
            recorder: Arc::new(Mutex::new(recorder)),
            sessions: Some(artist_registry::Registry::for_project(project).sessions()),
        }
    }

    pub fn attach_recorder(&self, recorder: crate::Recorder) {
        *self.recorder.lock().expect("ask registry poisoned") = Some(recorder);
    }

    fn recorder(&self) -> Option<crate::Recorder> {
        self.recorder.lock().expect("ask registry poisoned").clone()
    }

    /// Post one question for legacy surface callers. Universal ask tool callers use
    /// `post_batch`, which retains original question order in one durable snapshot.
    pub fn post(&self, question: Question) -> oneshot::Receiver<Answer> {
        let (respond, receive) = oneshot::channel();
        if let Some(recorder) = self.recorder() {
            recorder.record(crate::AskPosted {
                question: question.clone(),
            });
        }
        self.pending.lock().expect("ask registry poisoned").insert(
            question.id.clone(),
            Waiting {
                question,
                respond: Some(respond),
            },
        );
        receive
    }

    pub fn post_batch(&self, questions: &[Question]) {
        let mut pending = self.pending.lock().expect("ask registry poisoned");
        for question in questions {
            if let Some(recorder) = self.recorder() {
                recorder.record(crate::AskPosted {
                    question: question.clone(),
                });
            }
            pending.insert(
                question.id.clone(),
                Waiting {
                    question: question.clone(),
                    respond: None,
                },
            );
        }
    }

    pub fn pending(&self) -> Vec<Question> {
        let mut questions = Vec::new();
        let mut seen = HashSet::new();
        if let Some(sessions) = &self.sessions {
            if let Ok(records) = sessions.list() {
                for record in records.into_iter().rev() {
                    if record.kind != "ask" || !record.lifecycle.is_live() {
                        continue;
                    }
                    let Ok(state) = serde_json::from_value::<DurableAskState>(record.snapshot)
                    else {
                        continue;
                    };
                    let answered = state
                        .answers
                        .iter()
                        .map(|answer| answer.answer.question_id.as_str())
                        .collect::<HashSet<_>>();
                    for question in state.questions {
                        if !answered.contains(question.id.as_str())
                            && seen.insert(question.id.clone())
                        {
                            questions.push(question);
                        }
                    }
                }
            }
        }
        let mut local = self
            .pending
            .lock()
            .expect("ask registry poisoned")
            .values()
            .map(|waiting| waiting.question.clone())
            .collect::<Vec<_>>();
        local.sort_by(|a, b| a.id.cmp(&b.id));
        for question in local {
            if seen.insert(question.id.clone()) {
                questions.push(question);
            }
        }
        questions
    }

    pub fn get(&self, id: &str) -> Option<Question> {
        self.pending()
            .into_iter()
            .find(|question| question.id == id)
    }

    pub fn answer(&self, answer: Answer) -> bool {
        self.answer_with_source(answer, AnswerSource::Human, "tui")
    }

    pub fn answer_from(&self, answer: Answer, surface: &str) -> bool {
        self.answer_with_source(answer, AnswerSource::Human, surface)
    }

    pub fn answer_with_source(&self, answer: Answer, source: AnswerSource, surface: &str) -> bool {
        let Some(question) = self.get(&answer.question_id) else {
            return false;
        };
        if !valid_answer(&question, &answer) {
            return false;
        }

        let mut won = true;
        if let Some(sessions) = &self.sessions {
            let result = sessions.mutate(&question.ask_session, |record| {
                if !record.lifecycle.is_live() {
                    won = false;
                    return Ok(());
                }
                let mut state: DurableAskState = serde_json::from_value(record.snapshot.clone())
                    .map_err(|error| artist_registry::Error::Corrupt(error.to_string()))?;
                if state
                    .answers
                    .iter()
                    .any(|answered| answered.answer.question_id == answer.question_id)
                {
                    won = false;
                    return Ok(());
                }
                state.answers.push(DurableAnswer {
                    answer: answer.clone(),
                    source,
                });
                state.refresh_model();
                let complete = state.complete();
                record.snapshot = serde_json::to_value(&state)
                    .map_err(|error| artist_registry::Error::Corrupt(error.to_string()))?;
                if complete {
                    record.lifecycle = SessionLifecycle::Stopped {
                        status: SessionStatus::Completed,
                    };
                    record.cancel_requested = false;
                }
                Ok(())
            });
            if result.is_err() || !won {
                return false;
            }
        }

        if let Some(recorder) = self.recorder() {
            recorder.record(crate::AskAnswered {
                answer: answer.clone(),
                source,
                surface: surface.to_owned(),
            });
        }
        if let Some(mut waiting) = self
            .pending
            .lock()
            .expect("ask registry poisoned")
            .remove(&answer.question_id)
        {
            if let Some(respond) = waiting.respond.take() {
                let _ = respond.send(answer);
            }
        }
        true
    }

    /// Auto-resolve exactly every recommended option, with no notes.
    pub fn auto_resolve(&self, session: &str) -> bool {
        let questions = self
            .pending()
            .into_iter()
            .filter(|question| question.ask_session == session)
            .collect::<Vec<_>>();
        if questions.is_empty() {
            return false;
        }
        for question in questions {
            let answer = Answer {
                question_id: question.id.clone(),
                selections: question
                    .options
                    .iter()
                    .filter(|option| option.recommended)
                    .map(|option| Selection {
                        option_id: Some(option.id.clone()),
                        note: None,
                    })
                    .collect(),
            };
            let _ = self.answer_with_source(answer, AnswerSource::AutoResolve, "auto_resolve");
        }
        true
    }

    /// Remove a cancelled ask from every human-facing picker. Cancellation is a
    /// lifecycle outcome, not an answer and therefore records no AskAnswered event.
    pub fn cancel_session(&self, session: &str) {
        let ids = self
            .pending()
            .into_iter()
            .filter(|question| question.ask_session == session)
            .map(|question| question.id)
            .collect::<HashSet<_>>();
        let mut pending = self.pending.lock().expect("ask registry poisoned");
        for id in ids {
            if let Some(mut waiting) = pending.remove(&id) {
                if let Some(respond) = waiting.respond.take() {
                    let _ = respond.send(Answer::dismissed(id));
                }
            }
        }
    }

    pub fn dismiss_all(&self) {
        let sessions = self
            .pending()
            .into_iter()
            .map(|question| question.ask_session)
            .collect::<HashSet<_>>();
        for session in sessions {
            self.cancel_session(&session);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pending().is_empty()
    }
}

fn valid_answer(question: &Question, answer: &Answer) -> bool {
    let valid_ids = question
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect::<HashSet<_>>();
    answer
        .selections
        .iter()
        .all(|selection| match selection.option_id.as_deref() {
            Some(id) => !question.options.is_empty() && valid_ids.contains(id),
            None => {
                question.options.is_empty()
                    && selection
                        .note
                        .as_deref()
                        .is_some_and(|note| !note.trim().is_empty())
            }
        })
}

impl std::fmt::Debug for AskRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AskRegistry")
            .field("pending", &self.pending().len())
            .field("durable", &self.sessions.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(session: &str, id: &str) -> Question {
        Question {
            id: id.into(),
            ask_session: session.into(),
            question: "Pick?".into(),
            options: vec![
                QuestionOption {
                    id: "o-a".into(),
                    label: "A".into(),
                    recommended: false,
                },
                QuestionOption {
                    id: "o-b".into(),
                    label: "B".into(),
                    recommended: true,
                },
            ],
        }
    }

    #[test]
    fn model_projection_uses_original_option_indices() {
        let q = question("ask:pick", "q-1");
        let answer = Answer {
            question_id: q.id.clone(),
            selections: vec![Selection {
                option_id: Some("o-b".into()),
                note: Some("because X".into()),
            }],
        };
        assert_eq!(answer.describe_for(&q), "chose 2 — because X");
        assert_eq!(Answer::dismissed(&q.id).describe_for(&q), "(dismissed)");
    }

    #[test]
    fn model_projection_keeps_each_note_attached_to_its_choice() {
        let q = question("ask:pick", "q-1");
        let answer = Answer {
            question_id: q.id.clone(),
            selections: vec![
                Selection {
                    option_id: Some("o-a".into()),
                    note: Some("first note".into()),
                },
                Selection {
                    option_id: Some("o-b".into()),
                    note: Some("second note".into()),
                },
            ],
        };
        assert_eq!(
            answer.describe_for(&q),
            "chose 1 — first note, 2 — second note"
        );
    }

    #[test]
    fn free_response_is_only_valid_for_zero_option_questions() {
        let q = question("ask:pick", "q-1");
        assert!(!valid_answer(
            &q,
            &Answer {
                question_id: q.id.clone(),
                selections: vec![Selection {
                    option_id: None,
                    note: Some("free response".into()),
                }],
            }
        ));

        let free = Question {
            id: "q-free".into(),
            ask_session: "ask:free".into(),
            question: "Explain".into(),
            options: Vec::new(),
        };
        let answer = Answer {
            question_id: free.id.clone(),
            selections: vec![Selection {
                option_id: None,
                note: Some("full response".into()),
            }],
        };
        assert!(valid_answer(&free, &answer));
        assert_eq!(answer.describe_for(&free), "full response");
    }

    #[test]
    fn durable_registry_can_be_answered_by_another_handle() {
        let root = tempfile::tempdir().unwrap();
        let sessions = artist_registry::Registry::for_project(root.path()).sessions();
        let q = question("ask:pick", "q-1");
        let state = DurableAskState::new(vec![q.clone()]);
        sessions
            .create_exact(
                "ask:pick",
                "ask",
                "Goethe",
                None,
                serde_json::to_value(state).unwrap(),
            )
            .unwrap();
        let first = AskRegistry::for_project(root.path(), None);
        let second = AskRegistry::for_project(root.path(), None);
        assert_eq!(first.pending().len(), 1);
        assert!(second.answer(Answer {
            question_id: q.id,
            selections: vec![Selection {
                option_id: Some("o-b".into()),
                note: None
            }]
        }));
        let record = sessions.get("ask:pick").unwrap().unwrap();
        assert_eq!(
            record.lifecycle,
            SessionLifecycle::Stopped {
                status: SessionStatus::Completed
            }
        );
        assert!(first.pending().is_empty());
    }

    #[test]
    fn auto_resolve_chooses_all_and_only_recommended_options() {
        let root = tempfile::tempdir().unwrap();
        let sessions = artist_registry::Registry::for_project(root.path()).sessions();
        let q = question("ask:pick", "q-1");
        let state = DurableAskState::new(vec![q]);
        sessions
            .create_exact(
                "ask:pick",
                "ask",
                "Goethe",
                None,
                serde_json::to_value(state).unwrap(),
            )
            .unwrap();
        let registry = AskRegistry::for_project(root.path(), None);
        assert!(registry.auto_resolve("ask:pick"));
        let record = sessions.get("ask:pick").unwrap().unwrap();
        let state: DurableAskState = serde_json::from_value(record.snapshot).unwrap();
        assert_eq!(state.model["answers"][0]["value"], "chose 2");
        assert_eq!(state.model["answers"][0]["source"], "AutoResolve");
        assert_eq!(state.answers[0].source, AnswerSource::AutoResolve);
        assert_eq!(
            record.lifecycle,
            SessionLifecycle::Stopped {
                status: SessionStatus::Completed
            }
        );
    }

    #[test]
    fn auto_resolve_without_recommendations_settles_as_dismissed() {
        let root = tempfile::tempdir().unwrap();
        let sessions = artist_registry::Registry::for_project(root.path()).sessions();
        let q = Question {
            id: "q-1".into(),
            ask_session: "ask:pick".into(),
            question: "Pick".into(),
            options: vec![QuestionOption {
                id: "o-1".into(),
                label: "One".into(),
                recommended: false,
            }],
        };
        sessions
            .create_exact(
                "ask:pick",
                "ask",
                "Goethe",
                None,
                serde_json::to_value(DurableAskState::new(vec![q])).unwrap(),
            )
            .unwrap();
        let registry = AskRegistry::for_project(root.path(), None);
        assert!(registry.auto_resolve("ask:pick"));
        let record = sessions.get("ask:pick").unwrap().unwrap();
        let state: DurableAskState = serde_json::from_value(record.snapshot).unwrap();
        assert!(state.answers[0].answer.selections.is_empty());
        assert_eq!(state.answers[0].source, AnswerSource::AutoResolve);
        assert_eq!(state.model["answers"][0]["value"], "(dismissed)");
        assert_eq!(
            record.lifecycle,
            SessionLifecycle::Stopped {
                status: SessionStatus::Completed
            }
        );
    }
}
