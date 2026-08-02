//! Talking to other agents.
//!
//! Delivery rides the steering path rather than inventing a second one. TTSR
//! aborts a run and re-injects from the current seed, so "was this already put
//! in front of the model" is a solved problem here and nowhere else — a message
//! delivered during an aborted turn must not arrive twice on the retry, and
//! `ttsr_tests::delivered_steering_survives_abort_without_double_delivery`
//! already pins that. A parallel delivery path would reintroduce a bug we have
//! already fixed.
//!
//! Messages are **content, never policy**. They are injected as ordinary turn
//! content and never as system-role text: a message from a peer carries that
//! peer's authority, and delivering it as system content would give every agent
//! operator-level authority over every other one.

use std::sync::{Arc, Mutex};

use artist_registry::{Audience, Message};

/// Who a `reply` from this agent should go to.
///
/// Captured when a message is **delivered into context**, never read from the
/// mailbox when `reply` is called. A message sitting undelivered must not
/// become the target of a reply the model wrote before it could have seen it —
/// so the target rides the turn, not the inbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReplyTarget {
    pub to: String,
    pub audience: Audience,
}

/// This agent's mailbox, and what it last saw.
#[derive(Clone)]
pub(crate) struct Inbox {
    /// The name this agent answers to.
    pub name: Arc<str>,
    store: artist_registry::Messages,
    last_delivered: Arc<Mutex<Option<ReplyTarget>>>,
}

impl Inbox {
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            store: artist_registry::messages(),
            last_delivered: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub fn with_store(name: impl Into<Arc<str>>, store: artist_registry::Messages) -> Self {
        Self {
            name: name.into(),
            store,
            last_delivered: Arc::new(Mutex::new(None)),
        }
    }

    pub fn store(&self) -> &artist_registry::Messages {
        &self.store
    }

    /// Take anything waiting and record who to reply to.
    ///
    /// Returns the text to inject, or `None` when the inbox is empty. Draining
    /// and recording happen together so there is no window in which a message
    /// has been consumed but its sender is not yet the reply target.
    pub fn collect(&self) -> Option<String> {
        let messages = self.store.drain(&self.name).ok()?;
        if messages.is_empty() {
            return None;
        }
        // The last message in the batch is what `reply` answers. Stated as a
        // rule rather than left implicit, because a batch can hold several
        // senders and "the last one you were shown" is the only reading a
        // person would predict.
        if let Some(last) = messages.last() {
            *self
                .last_delivered
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(ReplyTarget {
                to: last.from.clone(),
                audience: last.audience.clone(),
            });
        }
        Some(render(&messages))
    }

    /// Who `reply` addresses, if anything has been delivered.
    pub fn reply_target(&self) -> Option<ReplyTarget> {
        self.last_delivered
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

/// Render delivered messages for injection.
///
/// Attributed and fenced so the model can tell a peer's words from its user's.
/// A waiting sender is called out explicitly rather than left for the model to
/// infer from tone, because whether someone is blocked changes what a good
/// response looks like.
fn render(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|message| {
            let group = match &message.audience {
                Audience::Direct => String::new(),
                Audience::Group { id } => format!(" group=\"{id}\""),
            };
            let waiting = if message.expects_reply {
                " awaiting-reply=\"true\""
            } else {
                ""
            };
            format!(
                "<agent_message from=\"{}\"{group}{waiting}>\n{}\n</agent_message>",
                message.from, message.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(root: &std::path::Path) -> artist_registry::Messages {
        artist_registry::Registry::at(root).messages_for_test()
    }

    fn message(from: &str, to: &str, body: &str, at: u64) -> Message {
        Message {
            id: format!("m-{at}"),
            from: from.into(),
            to: to.into(),
            audience: Audience::Direct,
            body: body.into(),
            expects_reply: false,
            sent_at: at,
        }
    }

    #[test]
    fn an_empty_inbox_injects_nothing() {
        let root = tempfile::tempdir().unwrap();
        let inbox = Inbox::with_store("Bach", store(root.path()));
        assert!(inbox.collect().is_none());
        assert!(inbox.reply_target().is_none());
    }

    #[test]
    fn a_delivered_message_is_attributed_to_its_sender() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store.send(&message("Monet", "Bach", "the parser is wrong", 1)).unwrap();

        let inbox = Inbox::with_store("Bach", store);
        let injected = inbox.collect().expect("one message");
        assert!(injected.contains("from=\"Monet\""), "{injected}");
        assert!(injected.contains("the parser is wrong"), "{injected}");
    }

    /// The invariant that makes `reply` predictable: the target is fixed when
    /// the message is put in front of the model, not when `reply` runs.
    #[test]
    fn the_reply_target_is_bound_at_delivery_not_at_call_time() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store.send(&message("Monet", "Bach", "first", 1)).unwrap();

        let inbox = Inbox::with_store("Bach", store.clone());
        inbox.collect().expect("delivered");
        assert_eq!(inbox.reply_target().unwrap().to, "Monet");

        // A message that arrives after delivery must not steal the reply: the
        // model has not seen it, so answering it would answer the wrong agent.
        store.send(&message("Basquiat", "Bach", "later", 2)).unwrap();
        assert_eq!(
            inbox.reply_target().unwrap().to,
            "Monet",
            "an undelivered message must not become the reply target"
        );
    }

    /// A batch has one reply target, and it has to be the one a person would
    /// predict: the last message shown.
    #[test]
    fn a_batch_replies_to_the_last_message_shown() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store.send(&message("Monet", "Bach", "first", 1)).unwrap();
        store.send(&message("Basquiat", "Bach", "second", 2)).unwrap();

        let inbox = Inbox::with_store("Bach", store);
        let injected = inbox.collect().expect("two messages");
        assert!(injected.contains("Monet") && injected.contains("Basquiat"));
        assert_eq!(inbox.reply_target().unwrap().to, "Basquiat");
    }

    /// Replying to a group has to reach the group, not just the sender.
    #[test]
    fn a_group_message_replies_to_the_group() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store
            .send(&Message {
                audience: Audience::Group { id: "g-1".into() },
                ..message("Monet", "Bach", "team update", 1)
            })
            .unwrap();

        let inbox = Inbox::with_store("Bach", store);
        let injected = inbox.collect().unwrap();
        assert!(injected.contains("group=\"g-1\""), "{injected}");
        assert_eq!(
            inbox.reply_target().unwrap().audience,
            Audience::Group { id: "g-1".into() }
        );
    }

    /// Whether the sender is blocked changes what a good answer looks like, so
    /// it is stated rather than left to be inferred.
    #[test]
    fn a_waiting_sender_is_marked_in_the_injected_text() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store
            .send(&Message {
                expects_reply: true,
                ..message("Monet", "Bach", "are you done?", 1)
            })
            .unwrap();

        let inbox = Inbox::with_store("Bach", store);
        assert!(inbox.collect().unwrap().contains("awaiting-reply=\"true\""));
    }

    /// Exactly-once delivery: a message already shown must not reappear on the
    /// next turn boundary.
    #[test]
    fn a_delivered_message_is_not_delivered_again() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store.send(&message("Monet", "Bach", "once", 1)).unwrap();

        let inbox = Inbox::with_store("Bach", store);
        assert!(inbox.collect().is_some());
        assert!(inbox.collect().is_none());
    }
}
