//! Durable ask outbox: questions and answers that survive transport death.
//!
//! The in-memory [`AskRegistry`](crate::ask::AskRegistry) is the coordination
//! point when asker and answerer share a process: the TUI picker, a canvas, and
//! the model all reach the same registry. Over MCP that assumption breaks —
//! the connection is short-lived (a tunnel reconnects every few minutes), and
//! the person answering is not in the MCP process at all. A question posted
//! into a process-local registry is gone when the connection dies, and the
//! model that asked can never retrieve the answer from a later connection.
//!
//! The outbox is the durable counterpart: an append-only JSONL log of posted
//! questions and recorded answers, replayed into memory at open. Posting and
//! answering are fsynced before they are acknowledged, so a crash between the
//! client seeing a response and the store receiving it cannot lose the record —
//! the same discipline as the request envelope. Whoever answers first wins; a
//! second answer to the same question is refused, matching the registry.
//!
//! The vocabulary is [`Question`]/[`Answer`], shared with the registry, so the
//! same question can be shown by a picker, a canvas, or a polled tool result,
//! and answered from wherever the user happens to be.

use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ask::{Answer, Question};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One append in the outbox log.
#[derive(Serialize, Deserialize)]
enum Record {
    Posted {
        question: Question,
        ts: u64,
    },
    Answered {
        answer: Answer,
        ts: u64,
    },
}

/// Durable store of posted questions and their answers.
///
/// Append-only JSONL under `state_dir/ask/outbox.jsonl`, replayed into memory
/// at [`open`](AskOutbox::open). `None` state dir means volatile: posting and
/// answering work within the process, but nothing survives a restart.
#[derive(Clone, Default)]
pub struct AskOutbox {
    path: Option<Arc<PathBuf>>,
    pending: Arc<RwLock<HashMap<String, Question>>>,
    answered: Arc<RwLock<HashMap<String, Answer>>>,
}

impl AskOutbox {
    /// Open the outbox under `state_dir`, recovering every posted question and
    /// recorded answer from a previous process.
    pub fn open(state_dir: Option<&Path>) -> Result<Self> {
        let Some(state_dir) = state_dir else {
            return Ok(Self::default());
        };
        let dir = state_dir.join("ask");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("outbox.jsonl");
        let mut pending = HashMap::new();
        let mut answered = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let Ok(record) = serde_json::from_str::<Record>(line) else {
                    continue;
                };
                match record {
                    Record::Posted { question, .. } => {
                        pending.insert(question.id.clone(), question);
                    }
                    Record::Answered { answer, .. } => {
                        pending.remove(&answer.question_id);
                        answered.insert(answer.question_id.clone(), answer);
                    }
                }
            }
        }
        Ok(Self {
            path: Some(Arc::new(path)),
            pending: Arc::new(RwLock::new(pending)),
            answered: Arc::new(RwLock::new(answered)),
        })
    }

    /// Append a `question` to the log and make it pending. The id is the
    /// caller's choice; reuse of an id already on the log is a caller error.
    pub fn post(&self, question: Question) -> Result<()> {
        self.append(&Record::Posted {
            ts: now(),
            question: question.clone(),
        })?;
        self.pending
            .write()
            .expect("ask outbox poisoned")
            .insert(question.id.clone(), question);
        Ok(())
    }
    pub fn pending(&self) -> Vec<Question> {
        let mut questions: Vec<_> = self
            .pending
            .read()
            .expect("ask outbox poisoned")
            .values()
            .cloned()
            .collect();
        questions.sort_by(|a, b| a.id.cmp(&b.id));
        questions
    }

    /// The answer recorded for `id`, if any.
    pub fn result(&self, id: &str) -> Option<Answer> {
        self.answered
            .read()
            .expect("ask outbox poisoned")
            .get(id)
            .cloned()
    }

    /// The pending question for `id`, if any. Unlike [`Self::result`] this
    /// returns the question itself, so a fresh connection that only has the id
    /// can recover the wording to relay.
    pub fn question(&self, id: &str) -> Option<Question> {
        self.pending
            .read()
            .expect("ask outbox poisoned")
            .get(id)
            .cloned()
    }

    /// Record an `answer` durably. First answer wins: `Ok(true)` when it
    /// answered a pending question, `Ok(false)` for an unknown or already
    /// answered id — a second answer is a race between two surfaces, not an
    /// error, matching the in-process registry.
    pub fn answer(&self, answer: Answer) -> Result<bool> {
        let id = answer.question_id.clone();
        let already: bool = {
            let pending = self.pending.read().expect("ask outbox poisoned");
            let answered = self.answered.read().expect("ask outbox poisoned");
            !pending.contains_key(&id) || answered.contains_key(&id)
        };
        if already {
            return Ok(false);
        }
        // Durability before memory: a failed append must not leave the question
        // silently answered in memory but not on disk.
        self.append(&Record::Answered { ts: now(), answer: answer.clone() })?;
        self.pending
            .write()
            .expect("ask outbox poisoned")
            .remove(&id);
        self.answered
            .write()
            .expect("ask outbox poisoned")
            .insert(id, answer);
        Ok(true)
    }

    pub fn is_empty(&self) -> bool {
        self.pending
            .read()
            .expect("ask outbox poisoned")
            .is_empty()
    }

    fn append(&self, record: &Record) -> Result<()> {
        // Volatile outbox: nothing on disk, but posting and answering still
        // work within the process.
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        serde_json::to_writer(&mut file, record).context("write outbox record")?;
        file.write_all(b"\n")?;
        file.sync_all().context("fsync outbox record")?;
        Ok(())
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
                crate::ask::QuestionOption {
                    label: "Project-local".into(),
                    description: "Durable across sessions".into(),
                    preview: None,
                },
                crate::ask::QuestionOption {
                    label: "Ephemeral".into(),
                    description: "Tied to one conversation".into(),
                    preview: None,
                },
            ],
        }
    }

    fn answer(id: &str, label: &str) -> Answer {
        Answer {
            question_id: id.to_owned(),
            selected: vec![label.to_owned()],
            notes: None,
        }
    }

    #[test]
    fn posting_makes_a_question_pending() {
        let outbox = AskOutbox::default();
        outbox.post(question("q1")).unwrap();
        assert_eq!(outbox.pending().len(), 1);
        assert_eq!(outbox.pending()[0].id, "q1");
        assert!(outbox.result("q1").is_none());
    }

    #[test]
    fn answering_resolves_the_question() {
        let outbox = AskOutbox::default();
        outbox.post(question("q1")).unwrap();
        assert!(outbox.answer(answer("q1", "Project-local")).unwrap());
        assert!(outbox.pending().is_empty());
        assert_eq!(
            outbox.result("q1").unwrap().selected,
            ["Project-local"]
        );
    }

    #[test]
    fn the_first_answer_wins() {
        let outbox = AskOutbox::default();
        outbox.post(question("q1")).unwrap();
        assert!(outbox.answer(answer("q1", "Project-local")).unwrap());
        assert!(!outbox.answer(answer("q1", "Ephemeral")).unwrap());
        assert_eq!(outbox.result("q1").unwrap().selected, ["Project-local"]);
    }

    #[test]
    fn answering_unknown_or_answered_ids_is_not_an_error() {
        let outbox = AskOutbox::default();
        assert!(!outbox.answer(answer("nope", "x")).unwrap());
    }

    #[test]
    fn an_outbox_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        {
            let outbox = AskOutbox::open(Some(dir.path())).unwrap();
            outbox.post(question("q1")).unwrap();
            outbox.answer(answer("q1", "Project-local")).unwrap();
            outbox.post(question("q2")).unwrap();
        }
        let reopened = AskOutbox::open(Some(dir.path())).unwrap();
        assert_eq!(reopened.result("q1").unwrap().selected, ["Project-local"]);
        assert_eq!(reopened.pending().len(), 1);
        assert_eq!(reopened.pending()[0].id, "q2");
    }

    #[test]
    fn a_pending_question_is_lookupable_by_id() {
        let outbox = AskOutbox::default();
        outbox.post(question("q1")).unwrap();
        let found = outbox.question("q1").expect("posted question");
        assert_eq!(found.question, "Where should canvases live?");
        assert!(outbox.question("nope").is_none());

        outbox.answer(answer("q1", "Project-local")).unwrap();
        // Answered questions are no longer pending, so the id no longer
        // resolves to a question — only to its answer.
        assert!(outbox.question("q1").is_none());
    }

    #[test]
    fn pending_order_is_stable() {
        let outbox = AskOutbox::default();
        for id in ["q3", "q1", "q2"] {
            outbox.post(question(id)).unwrap();
        }
        let ids: Vec<_> = outbox.pending().into_iter().map(|q| q.id).collect();
        assert_eq!(ids, ["q1", "q2", "q3"]);
    }
}
