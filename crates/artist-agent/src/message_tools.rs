//! The tools an agent uses to talk to another agent.
//!
//! `tell` fires and returns. `query` blocks until the target says something
//! back — and a reply is *any* communication back to the caller, not a specific
//! tool: if B answers A's query with a question of its own, that question is
//! the reply that unblocks A. `reply` addresses whoever was last delivered.
//!
//! Two rules carry the blocking case:
//!
//! * **A deadline is required.** Two-cycles resolve themselves — A queries B, B
//!   queries A, B's message satisfies A, A answers, B unblocks. Longer ones do
//!   not: A→B, B→C, C→A leaves everyone waiting on someone waiting on someone
//!   else, and no message reaches an agent that is waiting for it. There is no
//!   general cycle detection worth building, so the deadline is the answer.
//! * **A blocked agent gives up its delegation seat.** An agent waiting on
//!   another is not working, and N agents blocked while holding seats fills the
//!   pool and stalls the project. This is the same reasoning that makes a
//!   parent yield while awaiting a child.

use artist_registry::{Audience, Group, Message};
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::messaging::Inbox;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct MessageError(String);

/// How long `query` waits before giving up, and the ceiling a caller may ask
/// for. Bounded because an unbounded wait is how a cycle becomes a hang.
const DEFAULT_QUERY_MS: u64 = 120_000;
const MAX_QUERY_MS: u64 = 600_000;
const POLL: std::time::Duration = std::time::Duration::from_millis(200);

#[derive(Clone)]
pub(crate) struct MessageTools {
    inbox: Inbox,
    /// Released while blocked in `query` — see the module note.
    seat: Option<crate::delegate::PermitSlot>,
}

impl MessageTools {
    pub fn new(inbox: Inbox, seat: Option<crate::delegate::PermitSlot>) -> Self {
        Self { inbox, seat }
    }

    fn send_to(&self, audience: &Audience, to: &str, body: &str, expects_reply: bool) -> Result<usize, MessageError> {
        let recipients = match audience {
            Audience::Direct => vec![to.to_owned()],
            Audience::Group { id } => self
                .inbox
                .store()
                .group(id)
                .map_err(|error| MessageError(error.to_string()))?
                .ok_or_else(|| MessageError(format!("unknown group: {id}")))?
                .audience(),
        };

        let mut delivered = 0;
        for recipient in recipients {
            // Never post to yourself: a group you are in would otherwise echo
            // every message straight back into your own next turn.
            if recipient.as_str() == &*self.inbox.name {
                continue;
            }
            let message = Message {
                id: artist_tools::short_id("m"),
                from: self.inbox.name.to_string(),
                to: recipient,
                audience: audience.clone(),
                body: body.to_owned(),
                expects_reply,
                sent_at: artist_registry::now(),
            };
            self.inbox
                .store()
                .send(&message)
                .map_err(|error| MessageError(error.to_string()))?;
            delivered += 1;
        }
        Ok(delivered)
    }

    /// Wait for anything addressed back to us.
    ///
    /// Yields the delegation seat for the duration: this agent is parked, and
    /// holding a seat while parked is what turns a conversation between two
    /// agents into a stall for everyone else on the project.
    async fn wait_for_reply(&self, deadline_ms: u64) -> Option<String> {
        if let Some(seat) = &self.seat {
            seat.yield_seat().await;
        }
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
        let answer = loop {
            if let Some(text) = self.inbox.collect() {
                break Some(text);
            }
            if tokio::time::Instant::now() >= deadline {
                break None;
            }
            tokio::time::sleep(POLL).await;
        };
        if let Some(seat) = &self.seat {
            seat.retake().await;
        }
        answer
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TellArgs {
    /// Agent name, or a group id when `group` is set.
    target: String,
    message: String,
    /// Address a durable group rather than one agent.
    #[serde(default)]
    group: bool,
}

impl PortableTool for MessageTools {
    const NAME: &'static str = "tell";
    type Error = MessageError;
    type Args = TellArgs;
    type Output = String;

    fn description(&self) -> String {
        "Send a message to another agent, or to a group. Returns immediately \
         without waiting for a response."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": "Agent name, or group id when group is true."},
                "message": {"type": "string", "description": "What to say."},
                "group": {"type": "boolean", "default": false, "description": "Address a group rather than one agent."}
            },
            "required": ["target", "message"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: TellArgs) -> Result<String, MessageError> {
        let audience = if args.group {
            Audience::Group {
                id: args.target.clone(),
            }
        } else {
            Audience::Direct
        };
        // Reject an unknown name rather than writing into an inbox nobody
        // drains: silently accepting would look like delivery.
        if !args.group && artist_registry::names().resolve(&args.target).ok().flatten().is_none() {
            return Err(MessageError(format!(
                "no agent named {} on this machine",
                args.target
            )));
        }
        let delivered = self.send_to(&audience, &args.target, &args.message, false)?;
        Ok(json!({"delivered": delivered}).to_string())
    }
}

/// `query`: send, then block until the target says something back.
#[derive(Clone)]
pub(crate) struct QueryTool(pub MessageTools);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueryArgs {
    target: String,
    message: String,
    #[serde(default)]
    group: bool,
    /// Milliseconds to wait. Capped, because an unbounded wait is how a cycle
    /// of queries becomes a hang.
    #[serde(default)]
    wait_ms: Option<u64>,
}

impl PortableTool for QueryTool {
    const NAME: &'static str = "query";
    type Error = MessageError;
    type Args = QueryArgs;
    type Output = String;

    fn description(&self) -> String {
        "Ask another agent something and wait for their response. Any message \
         they send back satisfies the wait, including a question of their own."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": "Agent name, or group id when group is true."},
                "message": {"type": "string", "description": "What to ask."},
                "group": {"type": "boolean", "default": false},
                "waitMs": {"type": "integer", "description": "Milliseconds to wait for a response. Defaults to 120000, capped at 600000."}
            },
            "required": ["target", "message"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: QueryArgs) -> Result<String, MessageError> {
        let audience = if args.group {
            Audience::Group {
                id: args.target.clone(),
            }
        } else {
            Audience::Direct
        };
        if !args.group && artist_registry::names().resolve(&args.target).ok().flatten().is_none() {
            return Err(MessageError(format!(
                "no agent named {} on this machine",
                args.target
            )));
        }
        let delivered = self
            .0
            .send_to(&audience, &args.target, &args.message, true)?;
        if delivered == 0 {
            return Err(MessageError("nobody to ask".into()));
        }

        let budget = args.wait_ms.unwrap_or(DEFAULT_QUERY_MS).min(MAX_QUERY_MS);
        match self.0.wait_for_reply(budget).await {
            Some(text) => Ok(text),
            // Timing out is reported as a result, not an error: the question
            // was asked and may still be answered later, which is a different
            // situation from the send having failed.
            None => Ok(json!({
                "status": "no_response",
                "detail": format!("no response within {budget}ms; they may still reply later"),
            })
            .to_string()),
        }
    }
}

/// `reply`: answer whoever was last delivered.
#[derive(Clone)]
pub(crate) struct ReplyTool(pub MessageTools);

#[derive(Debug, Deserialize)]
pub(crate) struct ReplyArgs {
    message: String,
}

impl PortableTool for ReplyTool {
    const NAME: &'static str = "reply";
    type Error = MessageError;
    type Args = ReplyArgs;
    type Output = String;

    fn description(&self) -> String {
        "Respond to the most recent message you were shown, whether it came \
         from one agent or a group."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: ReplyArgs) -> Result<String, MessageError> {
        // Fails cleanly rather than guessing a recipient. A reply with nothing
        // to reply to is a model mistake worth surfacing, not a message worth
        // inventing an audience for.
        let target = self
            .0
            .inbox
            .reply_target()
            .ok_or_else(|| MessageError("no message to reply to".into()))?;
        let delivered = self
            .0
            .send_to(&target.audience, &target.to, &args.message, false)?;
        Ok(json!({"delivered": delivered, "to": target.to}).to_string())
    }
}

/// `gc`: open a durable group and send to it.
#[derive(Clone)]
pub(crate) struct GroupTool(pub MessageTools);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupArgs {
    /// Agent names to include. The caller is always a member.
    members: Vec<String>,
    message: String,
    #[serde(default)]
    label: Option<String>,
}

impl PortableTool for GroupTool {
    const NAME: &'static str = "gc";
    type Error = MessageError;
    type Args = GroupArgs;
    type Output = String;

    fn description(&self) -> String {
        "Open a group conversation with several agents and send the first \
         message. Returns a group id; later messages address that id."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "members": {"type": "array", "items": {"type": "string"}, "description": "Agent names to include."},
                "message": {"type": "string"},
                "label": {"type": "string", "description": "What this group is for."}
            },
            "required": ["members", "message"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: GroupArgs) -> Result<String, MessageError> {
        let known = artist_registry::names();
        let mut members = Vec::new();
        let mut unknown = Vec::new();
        for member in args.members {
            match known.resolve(&member).ok().flatten() {
                Some(name) => members.push(name.name),
                None => unknown.push(member),
            }
        }
        if members.is_empty() {
            return Err(MessageError(format!(
                "no agents found for: {}",
                unknown.join(", ")
            )));
        }

        // Materialised now, once. Membership is recorded rather than
        // re-derived, so every later message and every reply reaches the same
        // set — and the creator is a member even though they were not in the
        // list that defined it.
        let group = Group {
            id: artist_tools::short_id("g"),
            creator: self.0.inbox.name.to_string(),
            members,
            label: args.label.unwrap_or_default(),
        };
        self.0
            .inbox
            .store()
            .create_group(&group)
            .map_err(|error| MessageError(error.to_string()))?;

        let audience = Audience::Group {
            id: group.id.clone(),
        };
        let delivered = self.0.send_to(&audience, &group.id, &args.message, false)?;
        Ok(json!({
            "groupId": group.id,
            "delivered": delivered,
            "unknown": unknown,
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools(name: &str, root: &std::path::Path) -> MessageTools {
        MessageTools::new(
            Inbox::with_store(name, artist_registry::Registry::at(root).messages_for_test()),
            None,
        )
    }

    #[tokio::test]
    async fn a_reply_needs_something_to_reply_to() {
        let root = tempfile::tempdir().unwrap();
        let reply = ReplyTool(tools("Bach", root.path()));
        let error = reply
            .call(ReplyArgs {
                message: "sure".into(),
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no message to reply to"));
    }

    /// A reply goes to the sender of the message that was actually shown.
    #[tokio::test]
    async fn a_reply_reaches_the_agent_who_was_last_delivered() {
        let root = tempfile::tempdir().unwrap();
        let bach = tools("Bach", root.path());
        bach.inbox
            .store()
            .send(&Message {
                id: "m-1".into(),
                from: "Monet".into(),
                to: "Bach".into(),
                audience: Audience::Direct,
                body: "how goes it".into(),
                expects_reply: true,
                sent_at: 1,
            })
            .unwrap();
        bach.inbox.collect().expect("delivered");

        ReplyTool(bach.clone())
            .call(ReplyArgs {
                message: "nearly done".into(),
            })
            .await
            .unwrap();

        let monet = tools("Monet", root.path());
        let received = monet.inbox.collect().expect("the reply arrived");
        assert!(received.contains("nearly done"), "{received}");
        assert!(received.contains("from=\"Bach\""), "{received}");
    }

    /// A group message reaches every member and the creator, and never echoes
    /// back to its sender.
    #[tokio::test]
    async fn a_group_message_reaches_members_but_not_its_sender() {
        let root = tempfile::tempdir().unwrap();
        let monet = tools("Monet", root.path());
        let group = Group {
            id: "g-1".into(),
            creator: "Monet".into(),
            members: vec!["Bach".into(), "Basquiat".into()],
            label: "team".into(),
        };
        monet.inbox.store().create_group(&group).unwrap();

        let delivered = monet
            .send_to(&Audience::Group { id: "g-1".into() }, "g-1", "standup", false)
            .unwrap();
        assert_eq!(delivered, 2, "both members, not the sender");
        assert!(!monet.inbox.store().has_mail("Monet"));
        assert!(monet.inbox.store().has_mail("Bach"));
        assert!(monet.inbox.store().has_mail("Basquiat"));
    }

    /// Replying to a group reaches the group, not just the one who spoke.
    #[tokio::test]
    async fn replying_to_a_group_reaches_the_group() {
        let root = tempfile::tempdir().unwrap();
        let bach = tools("Bach", root.path());
        bach.inbox
            .store()
            .create_group(&Group {
                id: "g-1".into(),
                creator: "Monet".into(),
                members: vec!["Bach".into(), "Basquiat".into()],
                label: "team".into(),
            })
            .unwrap();
        bach.inbox
            .store()
            .send(&Message {
                id: "m-1".into(),
                from: "Monet".into(),
                to: "Bach".into(),
                audience: Audience::Group { id: "g-1".into() },
                body: "status?".into(),
                expects_reply: false,
                sent_at: 1,
            })
            .unwrap();
        bach.inbox.collect().expect("delivered");

        ReplyTool(bach.clone())
            .call(ReplyArgs {
                message: "green".into(),
            })
            .await
            .unwrap();

        assert!(bach.inbox.store().has_mail("Monet"), "the creator hears it");
        assert!(
            bach.inbox.store().has_mail("Basquiat"),
            "so does the other member"
        );
        assert!(!bach.inbox.store().has_mail("Bach"), "but not the replier");
    }

    /// A query that nobody answers reports the timeout as a result rather than
    /// an error: the question was asked and may yet be answered.
    #[tokio::test(start_paused = true)]
    async fn an_unanswered_query_times_out_as_a_result() {
        let root = tempfile::tempdir().unwrap();
        let monet = tools("Monet", root.path());
        monet
            .inbox
            .store()
            .create_group(&Group {
                id: "g-1".into(),
                creator: "Monet".into(),
                members: vec!["Bach".into()],
                label: "pair".into(),
            })
            .unwrap();

        let answer = QueryTool(monet)
            .call(QueryArgs {
                target: "g-1".into(),
                message: "anyone?".into(),
                group: true,
                wait_ms: Some(500),
            })
            .await
            .unwrap();
        assert!(answer.contains("no_response"), "{answer}");
    }

    /// The blocked caller unblocks the moment anything comes back, and what it
    /// gets is the message itself.
    #[tokio::test]
    async fn a_query_returns_whatever_comes_back() {
        let root = tempfile::tempdir().unwrap();
        let monet = tools("Monet", root.path());
        monet
            .inbox
            .store()
            .create_group(&Group {
                id: "g-1".into(),
                creator: "Monet".into(),
                members: vec!["Bach".into()],
                label: "pair".into(),
            })
            .unwrap();

        let store = monet.inbox.store().clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            store
                .send(&Message {
                    id: "m-2".into(),
                    from: "Bach".into(),
                    to: "Monet".into(),
                    audience: Audience::Direct,
                    // A question of their own is still a reply — it is any
                    // communication back, not a particular tool.
                    body: "which parser?".into(),
                    expects_reply: true,
                    sent_at: 2,
                })
                .unwrap();
        });

        let answer = QueryTool(monet)
            .call(QueryArgs {
                target: "g-1".into(),
                message: "status?".into(),
                group: true,
                wait_ms: Some(5_000),
            })
            .await
            .unwrap();
        assert!(answer.contains("which parser?"), "{answer}");
        assert!(answer.contains("from=\"Bach\""), "{answer}");
    }

    /// Writing into an inbox nobody drains would look like delivery, so an
    /// unknown name is refused.
    #[tokio::test]
    async fn telling_an_unknown_agent_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let error = tools("Monet", root.path())
            .call(TellArgs {
                target: "NotAnAgent".into(),
                message: "hello".into(),
                group: false,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no agent named"));
    }
}
