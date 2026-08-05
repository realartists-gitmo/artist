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
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TellResult {
    pub from: String,
    pub to: String,
    pub delivered: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum QueryResult {
    Answered {
        from: String,
        response: String,
        #[serde(rename = "waitedMs")]
        waited_ms: u64,
    },
    NoResponse {
        target: String,
        detail: String,
        #[serde(rename = "waitedMs")]
        waited_ms: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplyResult {
    pub from: String,
    pub to: String,
    pub delivered: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum GroupChatResult {
    Preview {
        #[serde(rename = "wouldInclude")]
        would_include: Vec<String>,
        unknown: Vec<String>,
    },
    Created {
        #[serde(rename = "groupId")]
        group_id: String,
        members: Vec<String>,
        delivered: usize,
        unknown: Vec<String>,
    },
}

#[derive(Clone)]
pub(crate) struct MessageTools {
    inbox: Inbox,
    /// This agent's worktree, so `project: "current"` can be resolved here
    /// rather than requiring a model to know its own absolute path.
    project: String,
    /// Released while blocked in `query` — see the module note.
    /// Released while blocked in `query` — see the module note.
    seat: Option<crate::delegate::PermitSlot>,
    /// Address resolution and group selection. Injected so tests and isolated
    /// harnesses do not depend on process-global environment mutation.
    names: artist_registry::Names,
}

impl MessageTools {
    pub fn new(inbox: Inbox, project: String, seat: Option<crate::delegate::PermitSlot>) -> Self {
        Self::with_names(inbox, project, seat, artist_registry::names())
    }

    fn with_names(
        inbox: Inbox,
        project: String,
        seat: Option<crate::delegate::PermitSlot>,
        names: artist_registry::Names,
    ) -> Self {
        Self {
            inbox,
            project,
            seat,
            names,
        }
    }

    fn send_to(
        &self,
        audience: &Audience,
        to: &str,
        body: &str,
        expects_reply: bool,
    ) -> Result<usize, MessageError> {
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
    async fn wait_for_reply(&self, deadline_ms: u64) -> Option<crate::messaging::Delivery> {
        if let Some(seat) = &self.seat {
            seat.yield_seat().await;
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
        let answer = loop {
            if let Some(delivery) = self.inbox.collect_delivery() {
                break Some(delivery);
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
    type Output = TellResult;

    fn map_error(&self, error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::other(error.to_string())
            .with_code("message_error")
            .with_retryable(false)
    }

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

    async fn call(&self, args: TellArgs) -> Result<TellResult, MessageError> {
        let audience = if args.group {
            Audience::Group {
                id: args.target.clone(),
            }
        } else {
            Audience::Direct
        };
        // Reject an unknown name rather than writing into an inbox nobody
        // drains: silently accepting would look like delivery.
        if !args.group && self.names.resolve(&args.target).ok().flatten().is_none() {
            return Err(MessageError(format!(
                "no agent named {} on this machine",
                args.target
            )));
        }
        let delivered = self.send_to(&audience, &args.target, &args.message, false)?;
        Ok(TellResult {
            from: self.inbox.name.to_string(),
            to: args.target,
            delivered,
        })
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
    type Output = QueryResult;

    fn map_error(&self, error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::other(error.to_string())
            .with_code("message_error")
            .with_retryable(false)
    }

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

    async fn call(&self, args: QueryArgs) -> Result<QueryResult, MessageError> {
        let audience = if args.group {
            Audience::Group {
                id: args.target.clone(),
            }
        } else {
            Audience::Direct
        };
        if !args.group && self.0.names.resolve(&args.target).ok().flatten().is_none() {
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
            Some(response) => Ok(QueryResult::Answered {
                from: response.from,
                response: response.text,
                waited_ms: budget,
            }),
            None => Ok(QueryResult::NoResponse {
                target: args.target,
                detail: format!("no response within {budget}ms; they may still reply later"),
                waited_ms: budget,
            }),
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
    type Output = ReplyResult;

    fn map_error(&self, error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::other(error.to_string())
            .with_code("message_error")
            .with_retryable(false)
    }

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

    async fn call(&self, args: ReplyArgs) -> Result<ReplyResult, MessageError> {
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
        Ok(ReplyResult {
            from: self.0.inbox.name.to_string(),
            to: target.to,
            delivered,
        })
    }
}

/// `gc`: open a durable group and send to it.
#[derive(Clone)]
pub(crate) struct GroupTool(pub MessageTools);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupArgs {
    /// Restrict to agents working in this project. `"current"` means this
    /// worktree; omit to span every project on the machine.
    #[serde(default)]
    project: Option<String>,
    /// Restrict to agents running this profile.
    #[serde(default)]
    profile: Option<String>,
    /// Restrict to agents beneath this one, transitively.
    #[serde(default)]
    descendant_of: Option<String>,
    /// Names to include regardless of the predicate.
    #[serde(default)]
    members: Vec<String>,
    /// Names to drop, whatever else matched.
    #[serde(default)]
    exclude: Vec<String>,
    /// Resolve the predicate and report who it selects without opening a group
    /// or sending anything. Also the directory read: an agent that does not yet
    /// know who exists asks with no criteria.
    #[serde(default)]
    preview: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

impl PortableTool for GroupTool {
    const NAME: &'static str = "gc";
    type Error = MessageError;
    type Args = GroupArgs;
    type Output = GroupChatResult;

    fn map_error(&self, error: Self::Error) -> ToolExecutionError {
        ToolExecutionError::other(error.to_string())
            .with_code("message_error")
            .with_retryable(false)
    }

    fn description(&self) -> String {
        "Open a group conversation over a set of agents and send the first \
         message. Select them by project, profile, or who spawned them, and/or \
         name them explicitly. Use preview to see who a selection covers — \
         with no criteria, that lists every agent on this machine. Returns a \
         group id; later messages address that id."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "project": {"type": "string", "description": "Restrict to a worktree. \"current\" means this one; omit to span every project."},
                "profile": {"type": "string", "description": "Restrict to agents running this profile, e.g. reviewer."},
                "descendantOf": {"type": "string", "description": "Restrict to agents spawned beneath this agent, transitively."},
                "members": {"type": "array", "items": {"type": "string"}, "description": "Agent names to include regardless of the criteria above."},
                "exclude": {"type": "array", "items": {"type": "string"}, "description": "Agent names to drop, whatever else matched."},
                "preview": {"type": "boolean", "default": false, "description": "Report who the selection covers without opening a group or sending anything."},
                "message": {"type": "string", "description": "The first message. Required unless preview is set."},
                "label": {"type": "string", "description": "What this group is for."}
            },
            "additionalProperties": false
        })
    }

    async fn call(&self, args: GroupArgs) -> Result<GroupChatResult, MessageError> {
        let selector = artist_registry::Selector {
            // "current" is spelled by the caller but resolved here: a model
            // should not have to know the absolute path of its own worktree to
            // address the agents in it.
            project: args.project.map(|project| {
                if project == "current" {
                    self.0.project.clone()
                } else {
                    project
                }
            }),
            profile: args.profile,
            descendant_of: args.descendant_of,
            names: args.members.clone(),
            exclude: args.exclude,
        };

        // The one evaluation. Everything after this addresses the answer, not
        // the question — a standing predicate would mean membership differed
        // between send and delivery, and a reply would have no defined
        // audience.
        let selected = self
            .0
            .names
            .select(&selector)
            .map_err(|error| MessageError(error.to_string()))?;
        let mut members: Vec<String> = selected.into_iter().map(|name| name.name).collect();
        let unknown: Vec<String> = args
            .members
            .iter()
            .filter(|wanted| !members.contains(wanted))
            .cloned()
            .collect();
        // The caller is a member of its own group, but is not an audience for
        // its own messages — `send_to` skips itself.
        members.retain(|member| member != &*self.0.inbox.name);

        if args.preview {
            return Ok(GroupChatResult::Preview {
                would_include: members,
                unknown,
            });
        }

        let Some(message) = args.message else {
            return Err(MessageError(
                "message is required unless preview is set".into(),
            ));
        };
        if members.is_empty() {
            return Err(MessageError(
                "that selection matched no other agents; use preview to see who is available"
                    .into(),
            ));
        }

        // Materialised now, once. Membership is recorded rather than
        // re-derived, so every later message and every reply reaches the same
        // set — and the creator is a member even though a predicate over
        // "my reports" would never have selected them.
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
        let delivered = self.0.send_to(&audience, &group.id, &message, false)?;
        Ok(GroupChatResult::Created {
            group_id: group.id,
            members: group.members,
            delivered,
            unknown,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools(name: &str, root: &std::path::Path) -> MessageTools {
        let registry = artist_registry::Registry::at(root);
        MessageTools::with_names(
            Inbox::with_store(name, registry.messages_for_test()),
            "/p".into(),
            None,
            registry.names_for_test(),
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
        let mapped = reply.map_error(error);
        assert_eq!(mapped.code(), Some("message_error"));
        assert!(
            mapped
                .model_feedback()
                .is_some_and(|message| message.contains("no message to reply to"))
        );
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
            .send_to(
                &Audience::Group { id: "g-1".into() },
                "g-1",
                "standup",
                false,
            )
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
        assert!(
            matches!(answer, QueryResult::NoResponse { waited_ms: 500, .. }),
            "{answer:?}"
        );
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
        assert_eq!(
            answer,
            QueryResult::Answered {
                from: "Bach".into(),
                response: "<agent_message from=\"Bach\" awaiting-reply=\"true\">\nwhich parser?\n</agent_message>"
                    .into(),
                waited_ms: 5_000,
            }
        );
    }

    /// Use one explicit registry root so parallel tests never mutate a
    /// process-global environment variable or the user's real roster.
    fn with_directory<T>(body: impl FnOnce(&std::path::Path) -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        body(dir.path())
    }

    fn register(
        root: &std::path::Path,
        session: &str,
        project: &str,
        profile: &str,
        parent: Option<&str>,
    ) -> String {
        artist_registry::Registry::at(root)
            .names_for_test()
            .claim(&artist_registry::Registration {
                session: session.into(),
                actor: session.into(),
                project: Some(project.into()),
                profile: Some(profile.into()),
                parent: parent.map(str::to_owned),
            })
            .unwrap()
            .name
    }

    /// The predicate that motivated the directory: everyone here, by role,
    /// without naming them.
    #[tokio::test]
    async fn a_group_can_be_opened_by_predicate_rather_than_by_name() {
        let selected = with_directory(|root| {
            let lead = register(root, "s-lead", "/p", "default", None);
            register(root, "s-rev-a", "/p", "reviewer", Some(&lead));
            register(root, "s-rev-b", "/p", "reviewer", Some(&lead));
            register(root, "s-elsewhere", "/other", "reviewer", None);

            let tools = tools(&lead, root);
            futures::executor::block_on(GroupTool(tools).call(GroupArgs {
                project: Some("current".into()),
                profile: Some("reviewer".into()),
                descendant_of: None,
                members: Vec::new(),
                exclude: Vec::new(),
                preview: false,
                message: Some("standup".into()),
                label: None,
            }))
            .unwrap()
        });

        let GroupChatResult::Created {
            members, delivered, ..
        } = selected
        else {
            panic!("expected created group: {selected:?}");
        };
        assert_eq!(members.len(), 2);
        assert_eq!(delivered, 2);
    }

    /// Preview is the directory read: it answers "who is out there" without
    /// creating anything or sending anything.
    #[tokio::test]
    async fn preview_reports_the_selection_without_creating_a_group() {
        let output = with_directory(|root| {
            let me = register(root, "s-me", "/p", "default", None);
            register(root, "s-other", "/p", "worker", None);

            let tools = tools(&me, root);
            futures::executor::block_on(GroupTool(tools.clone()).call(GroupArgs {
                project: None,
                profile: None,
                descendant_of: None,
                members: Vec::new(),
                exclude: Vec::new(),
                preview: true,
                message: None,
                label: None,
            }))
            .unwrap()
        });

        let GroupChatResult::Preview { would_include, .. } = output else {
            panic!("preview created a group: {output:?}");
        };
        assert_eq!(would_include.len(), 1);
    }

    /// A predicate that matches nobody must say so rather than open an empty
    /// group that silently swallows every later message.
    #[tokio::test]
    async fn an_empty_selection_is_refused_with_a_pointer_to_preview() {
        let error = with_directory(|root| {
            let me = register(root, "s-me", "/p", "default", None);
            let tools = tools(&me, root);
            futures::executor::block_on(GroupTool(tools).call(GroupArgs {
                project: Some("current".into()),
                profile: Some("nobody-has-this".into()),
                descendant_of: None,
                members: Vec::new(),
                exclude: Vec::new(),
                preview: false,
                message: Some("hello".into()),
                label: None,
            }))
            .unwrap_err()
        });
        assert!(error.to_string().contains("preview"), "{error}");
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
