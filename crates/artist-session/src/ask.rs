//! Asking the user a structured question.
//!
//! One vocabulary, two renderers. The same [`Question`] is drawn as a picker in
//! the TUI and as a component in a canvas, and either can answer it. That is
//! the point of putting it here rather than next to whichever feature happened
//! to need it first: a question raised by a tool must be answerable wherever
//! the user happens to be looking.
//!
//! The types live in the session crate because an answer is part of the
//! conversation — it has to survive resume, rewind, and compaction alongside
//! the turn that prompted it.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// One choice offered for a [`Question`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestionOption {
    /// What the user reads. Short.
    pub label: String,
    /// What choosing it means, and what it costs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Concrete artifact to compare against the alternatives — a mockup, a
    /// snippet, a config. Rendered monospaced when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// A question posed to the user, awaiting an answer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    /// Short chip label — a couple of words naming the decision.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

/// What the user chose.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Answer {
    pub question_id: String,
    /// Chosen option labels. Empty means the question was dismissed.
    #[serde(default)]
    pub selected: Vec<String>,
    /// Free text the user added alongside their choice — often where the real
    /// answer is, when none of the options were quite right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Answer {
    pub fn dismissed(question_id: impl Into<String>) -> Self {
        Answer {
            question_id: question_id.into(),
            selected: Vec::new(),
            notes: None,
        }
    }

    /// Render for the model. Reads as prose because it lands in a tool result.
    pub fn describe(&self) -> String {
        let choice = if self.selected.is_empty() {
            "(dismissed without choosing)".to_owned()
        } else {
            self.selected.join(", ")
        };
        match &self.notes {
            Some(notes) if !notes.trim().is_empty() => format!("{choice} — {}", notes.trim()),
            _ => choice,
        }
    }
}

/// Questions currently awaiting an answer.
///
/// Shared by every surface that can pose or answer one. First answer wins: if
/// the user picks in the TUI while a canvas is showing the same question, the
/// canvas's later answer is discarded rather than overwriting.
#[derive(Clone, Default)]
pub struct AskRegistry {
    pending: Arc<Mutex<HashMap<String, Waiting>>>,
    /// Recording lives here rather than at each call site so that every future
    /// poster — the planned `ask` tool, a canvas, anything else — is recorded
    /// by construction instead of by remembering to.
    recorder: Option<crate::Recorder>,
}

struct Waiting {
    question: Question,
    respond: Option<oneshot::Sender<Answer>>,
}

impl AskRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record posts and answers into the session log, so a resumed or rewound
    /// session can see what was asked and what the user chose.
    pub fn with_recorder(recorder: crate::Recorder) -> Self {
        Self {
            pending: Arc::default(),
            recorder: Some(recorder),
        }
    }

    /// Post a question and hand back the receiver its answer will arrive on.
    pub fn post(&self, question: Question) -> oneshot::Receiver<Answer> {
        if let Some(recorder) = &self.recorder {
            recorder.record(crate::AskPosted {
                question: question.clone(),
            });
        }
        let (respond, receive) = oneshot::channel();
        self.pending.lock().expect("ask registry poisoned").insert(
            question.id.clone(),
            Waiting {
                question,
                respond: Some(respond),
            },
        );
        receive
    }

    /// Everything still waiting, in a stable order so two surfaces agree on it.
    pub fn pending(&self) -> Vec<Question> {
        let mut questions: Vec<_> = self
            .pending
            .lock()
            .expect("ask registry poisoned")
            .values()
            .map(|waiting| waiting.question.clone())
            .collect();
        questions.sort_by(|a, b| a.id.cmp(&b.id));
        questions
    }

    pub fn get(&self, id: &str) -> Option<Question> {
        self.pending
            .lock()
            .expect("ask registry poisoned")
            .get(id)
            .map(|waiting| waiting.question.clone())
    }

    /// Answer a question. Returns false if it was already answered or unknown —
    /// a second answer is a race between two surfaces, not an error.
    pub fn answer(&self, answer: Answer) -> bool {
        self.answer_from(answer, "tui")
    }

    /// Answer, naming the surface the user answered on.
    pub fn answer_from(&self, answer: Answer, surface: &str) -> bool {
        let mut pending = self.pending.lock().expect("ask registry poisoned");
        let Some(mut waiting) = pending.remove(&answer.question_id) else {
            return false;
        };
        // Recorded before delivery: the answer is part of the conversation
        // whether or not the asker is still listening for it.
        if let Some(recorder) = &self.recorder {
            recorder.record(crate::AskAnswered {
                answer: answer.clone(),
                surface: surface.to_owned(),
            });
        }
        match waiting.respond.take() {
            // A dropped receiver means the asker gave up (cancelled turn); the
            // question is still correctly retired.
            Some(respond) => respond.send(answer).is_ok(),
            None => false,
        }
    }

    /// Retire everything, dismissing each. Used when a turn is cancelled: a
    /// question nobody is listening for must not linger on screen.
    pub fn dismiss_all(&self) {
        let waiting: Vec<_> = self
            .pending
            .lock()
            .expect("ask registry poisoned")
            .drain()
            .collect();
        for (id, mut entry) in waiting {
            if let Some(respond) = entry.respond.take() {
                let _ = respond.send(Answer::dismissed(id));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pending
            .lock()
            .expect("ask registry poisoned")
            .is_empty()
    }
}

impl std::fmt::Debug for AskRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AskRegistry")
            .field("pending", &self.pending().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(id: &str) -> Question {
        Question {
            id: id.to_owned(),
            header: "Storage".into(),
            question: "Where should canvases live?".into(),
            multi_select: false,
            options: vec![
                QuestionOption {
                    label: "Project-local".into(),
                    description: "Durable across sessions".into(),
                    preview: None,
                },
                QuestionOption {
                    label: "Ephemeral".into(),
                    description: "Tied to one conversation".into(),
                    preview: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn an_answer_reaches_whoever_asked() {
        let registry = AskRegistry::new();
        let waiting = registry.post(question("q1"));

        assert_eq!(registry.pending().len(), 1);
        assert!(registry.answer(Answer {
            question_id: "q1".into(),
            selected: vec!["Project-local".into()],
            notes: Some("durable is the point".into()),
        }));

        let answer = waiting.await.expect("answered");
        assert_eq!(answer.selected, ["Project-local"]);
        assert!(registry.is_empty());
    }

    /// The TUI and a canvas can show the same question at once. Whoever gets
    /// there first decides; the loser must not overwrite the answer.
    #[tokio::test]
    async fn the_first_surface_to_answer_wins() {
        let registry = AskRegistry::new();
        let waiting = registry.post(question("q1"));

        assert!(registry.answer(Answer {
            question_id: "q1".into(),
            selected: vec!["Project-local".into()],
            notes: None,
        }));
        assert!(!registry.answer(Answer {
            question_id: "q1".into(),
            selected: vec!["Ephemeral".into()],
            notes: None,
        }));

        assert_eq!(waiting.await.expect("answered").selected, ["Project-local"]);
    }

    #[test]
    fn answering_something_unknown_is_not_an_error() {
        assert!(!AskRegistry::new().answer(Answer::dismissed("nope")));
    }

    /// A cancelled turn must not leave a question on screen that nothing is
    /// listening for.
    #[tokio::test]
    async fn cancelling_dismisses_everything_outstanding() {
        let registry = AskRegistry::new();
        let first = registry.post(question("q1"));
        let second = registry.post(question("q2"));

        registry.dismiss_all();

        assert!(registry.is_empty());
        assert!(first.await.expect("dismissed").selected.is_empty());
        assert!(second.await.expect("dismissed").selected.is_empty());
    }

    #[test]
    fn pending_order_is_stable_so_two_surfaces_agree() {
        let registry = AskRegistry::new();
        for id in ["q3", "q1", "q2"] {
            let _ = registry.post(question(id));
        }
        let ids: Vec<_> = registry.pending().into_iter().map(|q| q.id).collect();
        assert_eq!(ids, ["q1", "q2", "q3"]);
    }

    #[test]
    fn an_answer_reads_as_prose_for_the_model() {
        assert_eq!(
            Answer {
                question_id: "q".into(),
                selected: vec!["A".into(), "B".into()],
                notes: Some("  and also C  ".into()),
            }
            .describe(),
            "A, B — and also C"
        );
        assert_eq!(
            Answer::dismissed("q").describe(),
            "(dismissed without choosing)"
        );
    }
}
