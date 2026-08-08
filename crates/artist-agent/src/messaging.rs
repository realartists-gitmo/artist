//! Durable direct-mail delivery into an Artist model boundary.
//!
//! Messages are ordinary turn content, never system policy. Universal `send` writes to
//! the machine-wide mailbox keyed by the recipient's bare artist name; the recipient
//! drains that mailbox exactly once at its next model-facing boundary.

use std::sync::Arc;

use artist_registry::Message;

#[derive(Clone)]
pub(crate) struct Inbox {
    /// The public bare artist identity this inbox belongs to.
    pub name: Arc<str>,
    store: artist_registry::Messages,
}

impl Inbox {
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            store: artist_registry::messages(),
        }
    }

    #[cfg(test)]
    pub fn with_store(name: impl Into<Arc<str>>, store: artist_registry::Messages) -> Self {
        Self {
            name: name.into(),
            store,
        }
    }

    /// Drain and render everything waiting for this artist. Draining is the delivery
    /// boundary, so an aborted/retried turn cannot receive the same mailbox record twice.
    pub fn collect(&self) -> Option<String> {
        let messages = self.store.drain(&self.name).ok()?;
        (!messages.is_empty()).then(|| render(&messages))
    }
}

fn render(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|message| {
            let waiting = if message.expects_reply {
                " awaiting-reply=\"true\""
            } else {
                ""
            };
            format!(
                "<agent_message from=\"{}\"{waiting}>\n{}\n</agent_message>",
                message.from, message.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_registry::Audience;

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
    fn empty_inbox_injects_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            Inbox::with_store("Bach", store(root.path()))
                .collect()
                .is_none()
        );
    }

    #[test]
    fn delivered_mail_is_attributed_and_consumed_once() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store
            .send(&message("Monet", "Bach", "the parser is wrong", 1))
            .unwrap();
        let inbox = Inbox::with_store("Bach", store);
        let injected = inbox.collect().expect("one message");
        assert!(injected.contains("from=\"Monet\""), "{injected}");
        assert!(injected.contains("the parser is wrong"), "{injected}");
        assert!(inbox.collect().is_none(), "mail must drain exactly once");
    }

    #[test]
    fn multiple_senders_keep_their_own_attribution_and_order() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        store.send(&message("Monet", "Bach", "first", 1)).unwrap();
        store
            .send(&message("Basquiat", "Bach", "second", 2))
            .unwrap();
        let rendered = Inbox::with_store("Bach", store).collect().unwrap();
        assert!(rendered.find("Monet").unwrap() < rendered.find("Basquiat").unwrap());
    }

    #[test]
    fn waiting_sender_marker_is_preserved() {
        let root = tempfile::tempdir().unwrap();
        let store = store(root.path());
        let mut pending = message("Monet", "Bach", "status?", 1);
        pending.expects_reply = true;
        store.send(&pending).unwrap();
        let rendered = Inbox::with_store("Bach", store).collect().unwrap();
        assert!(rendered.contains("awaiting-reply=\"true\""));
    }
}
