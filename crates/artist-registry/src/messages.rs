//! Direct durable inter-artist mail.
//!
//! Artists run in different processes, so a mailbox is a directory rather than a
//! channel. A sender writes one durable message file and the recipient atomically
//! drains its inbox on the next delivery boundary. Persistent messaging groups do not
//! exist; fan-out is expressed as several direct sends by the caller.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{Result, permits::write_atomic};

/// Message audience. Direct delivery is the only supported audience.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Audience {
    Direct,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    /// Display name of the sender, which is what a reply addresses.
    pub from: String,
    pub to: String,
    pub audience: Audience,
    pub body: String,
    /// Set when the sender is blocked waiting for an answer, so the recipient's
    /// harness can tell the model that someone is waiting rather than leaving
    /// it to infer urgency from prose.
    #[serde(default)]
    pub expects_reply: bool,
    pub sent_at: u64,
}

/// The machine-wide direct mailbox store.
#[derive(Clone, Debug)]
pub struct Messages {
    root: PathBuf,
}

impl Messages {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Deliver one message to one recipient.
    pub fn send(&self, message: &Message) -> Result<()> {
        let dir = self.inbox(&message.to);
        fs::create_dir_all(&dir)?;
        // Sequenced by send time then id so a drain returns them in the order
        // they were sent, which is what makes "the last message" well defined.
        write_atomic(
            &dir.join(format!("{:020}-{}", message.sent_at, sanitize(&message.id))),
            &serde_json::to_vec(message)?,
        )
    }

    /// Take everything waiting for `recipient`.
    ///
    /// Draining rather than peeking: a message is delivered exactly once, and
    /// the harness that drains it is responsible for putting it in front of the
    /// model. Leaving it would redeliver it on every turn.
    pub fn drain(&self, recipient: &str) -> Result<Vec<Message>> {
        let dir = self.inbox(recipient);
        fs::create_dir_all(&self.root)?;
        let _guard = MailboxLock::take(&self.root, recipient)?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect();
        paths.sort();

        let mut messages = Vec::with_capacity(paths.len());
        for path in paths {
            if let Ok(bytes) = fs::read(&path)
                && let Ok(message) = serde_json::from_slice(&bytes)
            {
                messages.push(message);
            }
            let _ = fs::remove_file(&path);
        }
        Ok(messages)
    }

    /// Whether anything is waiting, without taking it.
    pub fn has_mail(&self, recipient: &str) -> bool {
        fs::read_dir(self.inbox(recipient))
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    }

    /// Drop every message queued for an agent that will never read them.
    pub fn discard_inbox(&self, recipient: &str) -> Result<()> {
        match fs::remove_dir_all(self.inbox(recipient)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn inbox(&self, recipient: &str) -> PathBuf {
        self.root.join("inbox").join(sanitize(recipient))
    }
}

/// Recipient names and message ids reach this from tool arguments, so neither
/// may name a path outside the store.
struct MailboxLock(fs::File);

impl MailboxLock {
    fn take(root: &Path, recipient: &str) -> Result<Self> {
        let locks = root.join("locks");
        fs::create_dir_all(&locks)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(locks.join(format!("{}.lock", sanitize(recipient))))?;
        file.lock_exclusive()?;
        Ok(Self(file))
    }
}

impl Drop for MailboxLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn sanitize(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(96)
        .collect();
    if cleaned.is_empty() {
        "-".to_owned()
    } else {
        cleaned
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(root: &std::path::Path) -> Messages {
        Messages::new(root.join("messages"))
    }

    fn message(from: &str, to: &str, body: &str, sent_at: u64) -> Message {
        Message {
            id: format!("m-{sent_at}"),
            from: from.into(),
            to: to.into(),
            audience: Audience::Direct,
            body: body.into(),
            expects_reply: false,
            sent_at,
        }
    }

    #[test]
    fn a_message_reaches_its_recipient_and_only_them() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store
            .send(&message("Monet", "Bach", "look at this", 1))
            .unwrap();

        assert!(store.has_mail("Bach"));
        assert!(!store.has_mail("Monet"));
        let received = store.drain("Bach").unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].from, "Monet");
        assert_eq!(received[0].body, "look at this");
    }

    /// Exactly-once: a drained message must not come back on the next turn.
    #[test]
    fn draining_consumes_the_inbox() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store.send(&message("Monet", "Bach", "one", 1)).unwrap();
        assert_eq!(store.drain("Bach").unwrap().len(), 1);
        assert!(store.drain("Bach").unwrap().is_empty());
        assert!(!store.has_mail("Bach"));
    }

    /// "The last message" has to be well defined, because that is what a reply
    /// addresses.
    #[test]
    fn messages_arrive_in_the_order_they_were_sent() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        for (index, body) in ["first", "second", "third"].iter().enumerate() {
            store
                .send(&message("Monet", "Bach", body, index as u64 + 1))
                .unwrap();
        }
        let bodies: Vec<String> = store
            .drain("Bach")
            .unwrap()
            .into_iter()
            .map(|m| m.body)
            .collect();
        assert_eq!(bodies, ["first", "second", "third"]);
    }

    /// The sender may be a different process that has already exited; the
    /// message still has to be there.
    #[test]
    fn a_second_process_reads_what_the_first_one_sent() {
        let root = tempfile::tempdir().unwrap();
        store(root.path())
            .send(&message("Monet", "Bach", "cross-process", 1))
            .unwrap();
        assert_eq!(store(root.path()).drain("Bach").unwrap().len(), 1);
    }

    /// Recipient names are encoded before becoming path components.
    #[test]
    fn a_hostile_recipient_cannot_escape_the_store() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store
            .send(&message("Monet", "../../etc/passwd", "hostile", 1))
            .unwrap();
        assert!(!root.path().parent().unwrap().join("etc").exists());
        assert!(store.has_mail("../../etc/passwd"));
        let delivered = store.drain("../../etc/passwd").unwrap();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].body, "hostile");
    }

    /// An unreadable record must not wedge the mailbox behind it.
    #[test]
    fn an_unreadable_message_is_discarded_rather_than_retried_forever() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store.send(&message("Monet", "Bach", "good", 2)).unwrap();
        let inbox = store.inbox("Bach");
        fs::write(inbox.join("00000000000000000001-corrupt"), b"not json").unwrap();

        assert_eq!(store.drain("Bach").unwrap().len(), 1);
        assert!(
            !store.has_mail("Bach"),
            "the corrupt record was cleared too"
        );
    }

    #[test]
    fn discarding_an_inbox_drops_undelivered_mail() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store
            .send(&message("Monet", "Bach", "never read", 1))
            .unwrap();
        store.discard_inbox("Bach").unwrap();
        assert!(!store.has_mail("Bach"));
        store
            .discard_inbox("Bach")
            .expect("discarding twice is fine");
    }
}
