//! Inter-agent messages and groups.
//!
//! Agents run in different processes, so a mailbox is a directory rather than a
//! channel: the sender writes a file, the recipient drains it on its next turn
//! boundary. Nothing to keep alive, nothing to reconnect, and a message
//! survives the sender exiting immediately after sending it.
//!
//! **A group is a durable set, not a live predicate.** The selector that
//! creates one is evaluated once and the resulting membership — plus the
//! creator — is recorded. A standing predicate breaks three things at once:
//! membership differs between send and delivery, "reply to the group" has no
//! defined audience, and every message re-runs a query over mutable state. It
//! also makes the manager case fall out rather than needing a carve-out: a
//! manager who opens a group of their reports is not in the selector that
//! defines it, but is a member because membership is recorded, and recording it
//! is what a group *is*.

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{Result, permits::write_atomic};

/// Who a message was addressed to, which is what `reply` needs to answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Audience {
    /// One agent, by name.
    Direct,
    /// Everyone in a group, identified durably so a reply reaches the same set
    /// that received the original even if the world has moved on.
    Group { id: String },
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

/// A durable group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    /// The agent that opened it. Recorded explicitly because the creator is
    /// frequently outside the selector that defined the group, and a reply that
    /// skipped them would drop the one participant guaranteed to care.
    pub creator: String,
    pub members: Vec<String>,
    pub label: String,
}

impl Group {
    /// Everyone who should receive traffic on this group, creator included,
    /// without duplicates.
    pub fn audience(&self) -> Vec<String> {
        let mut all = self.members.clone();
        if !all.contains(&self.creator) {
            all.push(self.creator.clone());
        }
        all.sort();
        all.dedup();
        all
    }
}

/// The machine-wide mailbox and group store.
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
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|entry| Some(entry.ok()?.path())).collect();
        paths.sort();

        let mut messages = Vec::with_capacity(paths.len());
        for path in paths {
            if let Ok(bytes) = fs::read(&path)
                && let Ok(message) = serde_json::from_slice(&bytes)
            {
                messages.push(message);
            }
            // Removed either way: a record we cannot parse would otherwise be
            // retried forever and block nothing but itself.
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

    /// Record a group. The selector has already been evaluated by the caller —
    /// this stores the answer, which is the whole point.
    pub fn create_group(&self, group: &Group) -> Result<()> {
        let dir = self.root.join("groups");
        fs::create_dir_all(&dir)?;
        write_atomic(
            &dir.join(sanitize(&group.id)),
            &serde_json::to_vec(group)?,
        )
    }

    pub fn group(&self, id: &str) -> Result<Option<Group>> {
        match fs::read(self.root.join("groups").join(sanitize(id))) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
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
        store.send(&message("Monet", "Bach", "look at this", 1)).unwrap();

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

    /// The manager case: the creator is in the audience even though they were
    /// not in the selector that defined the group.
    #[test]
    fn a_group_includes_its_creator() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        let group = Group {
            id: "g-1".into(),
            creator: "Monet".into(),
            members: vec!["Bach".into(), "Basquiat".into()],
            label: "my reports".into(),
        };
        store.create_group(&group).unwrap();

        let stored = store.group("g-1").unwrap().expect("recorded");
        assert_eq!(stored.audience(), ["Bach", "Basquiat", "Monet"]);
    }

    /// Membership is frozen at creation. A group whose members changed roles
    /// afterwards still delivers to the set that was recorded, which is what
    /// makes a reply's audience well defined.
    #[test]
    fn group_membership_is_frozen_at_creation() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store
            .create_group(&Group {
                id: "g-1".into(),
                creator: "Monet".into(),
                members: vec!["Bach".into()],
                label: "pair".into(),
            })
            .unwrap();

        // A later arrival changes nothing about the recorded group.
        store.send(&message("Basquiat", "Bach", "unrelated", 2)).unwrap();
        assert_eq!(store.group("g-1").unwrap().unwrap().members, ["Bach"]);
    }

    #[test]
    fn a_group_audience_never_double_delivers_to_its_creator() {
        let group = Group {
            id: "g-1".into(),
            creator: "Monet".into(),
            members: vec!["Monet".into(), "Bach".into()],
            label: "self-included".into(),
        };
        assert_eq!(group.audience(), ["Bach", "Monet"]);
    }

    #[test]
    fn a_hostile_recipient_cannot_escape_the_store() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store
            .send(&message("Monet", "../../etc/passwd", "hostile", 1))
            .unwrap();
        assert!(!root.path().parent().unwrap().join("etc").exists());
        assert!(store.group("../../etc/passwd").unwrap().is_none());
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
        assert!(!store.has_mail("Bach"), "the corrupt record was cleared too");
    }

    #[test]
    fn discarding_an_inbox_drops_undelivered_mail() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store.send(&message("Monet", "Bach", "never read", 1)).unwrap();
        store.discard_inbox("Bach").unwrap();
        assert!(!store.has_mail("Bach"));
        store.discard_inbox("Bach").expect("discarding twice is fine");
    }
}
