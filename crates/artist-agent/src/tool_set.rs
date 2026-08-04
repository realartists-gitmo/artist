//! The one place a tool becomes available to a model.
//!
//! Before this module the main agent and the subagent path each kept their own
//! hand-written list of tools to construct, and each applied profile policy in
//! its own way. They drifted: the read-only built-ins were widened to allow
//! every structural navigation tool, and a test pinned that widening — but it
//! asserted [`Profile::permits`], which is policy, while the subagent path
//! never constructed those tools at all. A `reviewer` running as a subagent
//! silently had none of them, and `tools.allow: ["mcp:github/*"]` was inert
//! there because the child applied no glob pass whatsoever.
//!
//! The split that fixes it, and keeps it fixed:
//!
//! * **Availability is the environment's job.** [`ToolEnv`] carries every
//!   ingredient a tool could need. Its fields are required, so a caller with no
//!   canvas must say `None` rather than quietly omit it — the root and the
//!   child are then obliged to answer the same questions.
//! * **Policy is exactly one `retain`.** [`build`] applies `permits` once, over
//!   built-in, MCP and extension tools alike. "Did both paths filter the same
//!   way" stops being a question that can have two answers.
//!
//! Adding a tool means adding a [`Tool`] variant, at which point [`Tool::name`]
//! and [`Tool::construct`] stop compiling until both are written. Nothing
//! forces a brand new tool to be *registered* here — that is not a property
//! Rust can give us — but there is one list to remember instead of three.

use std::{path::PathBuf, sync::Arc};

use artist_session::Recorder;
use artist_tool_api::ArtistDynamicTool;
use artist_tools::ToolBundle;
use rig_core::completion::Message;
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    PromptEvent, SessionHandles,
    profiles::{Profile, Profiles},
    resources::Resources,
    tool_prompt,
};

/// Everything a tool might be built from.
///
/// An optional field means "this environment cannot provide the subsystem",
/// which is distinct from "the profile denied it". A tool the model can see but
/// that has nothing behind it is a failure it discovers by calling, which is
/// worse than its absence.
pub(crate) struct ToolEnv {
    pub bundle: ToolBundle,
    pub recorder: Recorder,
    pub resources: Resources,
    pub todos: crate::todo::TodoStore,
    /// Whose list `todo` reads and writes, and whose it may read but not write.
    /// A child owns its own and may read its parent's; concurrent siblings
    /// sharing one list would race with no obvious merge.
    pub todo_owner: String,
    pub todo_parent: Option<String>,
    pub attachments: Option<artist_session::AttachmentStore>,
    pub computer: Option<artist_computer::SurfaceRegistry>,
    pub memory: Option<crate::memory::MemoryWriter>,
    pub canvas: Option<Arc<artist_canvas::server::Lazy>>,
    /// `None` in a subagent, deliberately. A subagent's run *is* its return
    /// value to the parent, so there is no session beneath it to continue;
    /// handing off would either break the parent's contract — it asked for a
    /// `reviewer` and would receive a `planner`'s output — or mean nothing. The
    /// parent already has the mechanism: the child returns and it delegates
    /// again. `permits("handoff")` stays true there; the tool is simply not
    /// available, which is what this field is for.
    pub handoff: Option<HandoffEnv>,
    /// `None` where a session cannot delegate at all. Depth is otherwise
    /// unbounded — see [`crate::delegate::PermitSlot`] for how it is paid for
    /// without a cap.
    pub delegation: Option<DelegationEnv>,
    /// Somewhere to put a question to the user, and a token to abandon the
    /// wait on.
    ///
    /// `None` where there is nobody to ask — a headless or one-shot run — so
    /// the tool is absent rather than present and guaranteed to hang. Also
    /// `None` in a subagent: a background child blocking on a human nobody is
    /// watching is the worst version of this, and it has a better option in
    /// `query`, whose target is the parent that spawned it and is right there.
    pub ask: Option<artist_session::AskRegistry>,
    /// Abandons any wait that outlives the turn.
    pub cancel: tokio_util::sync::CancellationToken,
    /// This agent's mailbox, and the name other agents address it by.
    ///
    /// `None` where the agent has no identity to receive mail at, which is what
    /// makes the message tools absent rather than present-and-broken. They all
    /// need exactly this, so they stand or fall together — a subset would leave
    /// an agent able to send and unable to be answered.
    pub inbox: Option<crate::messaging::Inbox>,
    /// MCP and extension tools. Not enumerable at compile time, but subject to
    /// exactly the same policy pass as everything else.
    pub dynamic: Vec<ArtistDynamicTool>,
    /// The session's live tool toggles. Applied after profile policy because
    /// they are the user's override rather than the profile author's intent.
    pub disabled: Vec<String>,
}

pub(crate) struct HandoffEnv {
    pub pending: crate::handoff::HandoffShared,
    pub profiles: Profiles,
    /// The profile handing off, so it is never offered itself as a target.
    pub current: String,
}

/// What a nested [`crate::delegate::Delegate`] is built from.
///
/// Deliberately plain data: holding a `Delegate` here would make [`ToolEnv`]
/// recursive, and the delegate is reconstructed per child run anyway.
pub(crate) struct DelegationEnv {
    pub provider: llm_provider::SavedProvider,
    /// The history a `fork=true` child is seeded with. Rebuilt per attempt from
    /// the *current* seed, so a child spawned after a TTSR retry inherits the
    /// reminder-injected history rather than the stale original turn.
    pub context: Arc<Vec<Message>>,
    pub handles: SessionHandles,
    pub events: UnboundedSender<PromptEvent>,
    pub profiles: Profiles,
    /// The spawning run's seat on the delegation semaphore, yielded while it
    /// waits on a child. `None` at the session root, which holds no seat.
    pub parent_permit: Option<crate::delegate::PermitSlot>,
}

/// Every tool built into the harness.
///
/// The exhaustive matches in [`Tool::name`] and [`Tool::construct`] are the
/// point: a new variant fails to compile until both its name and its
/// construction are written, and there is nowhere else either could be written
/// instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    Bash,
    Read,
    Find,
    Grep,
    Edit,
    Write,
    Skill,
    Todo,
    Memory,
    CodeMap,
    CodeShow,
    CodeSurface,
    CodeImplements,
    CodeDeps,
    CodeCycles,
    CodeCalls,
    CodeTrace,
    CodeImpact,
    CodeSearch,
    CodeRelated,
    AstQuery,
    AstRewrite,
    Computer,
    Canvas,
    Handoff,
    Subagent,
    Tell,
    Query,
    Reply,
    GroupChat,
    Ask,
}

impl Tool {
    pub(crate) const ALL: [Tool; 31] = [
        Tool::Bash,
        Tool::Read,
        Tool::Find,
        Tool::Grep,
        Tool::Edit,
        Tool::Write,
        Tool::Skill,
        Tool::Todo,
        Tool::Memory,
        Tool::CodeMap,
        Tool::CodeShow,
        Tool::CodeSurface,
        Tool::CodeImplements,
        Tool::CodeDeps,
        Tool::CodeCycles,
        Tool::CodeCalls,
        Tool::CodeTrace,
        Tool::CodeImpact,
        Tool::CodeSearch,
        Tool::CodeRelated,
        Tool::AstQuery,
        Tool::AstRewrite,
        Tool::Computer,
        Tool::Canvas,
        Tool::Handoff,
        Tool::Subagent,
        Tool::Tell,
        Tool::Query,
        Tool::Reply,
        Tool::GroupChat,
        Tool::Ask,
    ];

    /// The name profile policy addresses this tool by.
    ///
    /// `const` so the built-in allow-lists in [`crate::profiles`] can be written
    /// in terms of it: a profile that means to permit `code_map` then names the
    /// variant rather than retyping the string, and a rename moves both.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Tool::Bash => "bash",
            Tool::Read => "read",
            Tool::Find => "find",
            Tool::Grep => "grep",
            Tool::Edit => "edit",
            Tool::Write => "write",
            Tool::Skill => "skill",
            Tool::Todo => "todo",
            Tool::Memory => "memory",
            Tool::CodeMap => "code_map",
            Tool::CodeShow => "code_show",
            Tool::CodeSurface => "code_surface",
            Tool::CodeImplements => "code_implements",
            Tool::CodeDeps => "code_deps",
            Tool::CodeCycles => "code_cycles",
            Tool::CodeCalls => "code_calls",
            Tool::CodeTrace => "code_trace",
            Tool::CodeImpact => "code_impact",
            Tool::CodeSearch => "code_search",
            Tool::CodeRelated => "code_related",
            Tool::AstQuery => "ast_query",
            Tool::AstRewrite => "ast_rewrite",
            Tool::Computer => "computer",
            Tool::Canvas => "canvas",
            Tool::Handoff => "handoff",
            Tool::Subagent => "subagent",
            Tool::Tell => "tell",
            Tool::Query => "query",
            Tool::Reply => "reply",
            Tool::GroupChat => "gc",
            Tool::Ask => "ask",
        }
    }

    /// Construct this tool, or `None` if the environment cannot provide it.
    ///
    /// This is the *only* definition of availability: there is no parallel
    /// table of what is gated on what that could disagree with it.
    fn construct(self, env: &ToolEnv) -> Option<ArtistDynamicTool> {
        let bundle = &env.bundle;
        Some(match self {
            Tool::Bash => tool_prompt::dynamic(bundle.bash.clone()),
            Tool::Read => tool_prompt::dynamic(bundle.read.clone()),
            Tool::Find => tool_prompt::dynamic(bundle.find.clone()),
            Tool::Grep => tool_prompt::dynamic(bundle.grep.clone()),
            Tool::Edit => tool_prompt::dynamic(bundle.edit.clone()),
            Tool::Write => tool_prompt::dynamic(bundle.write.clone()),
            Tool::Skill => tool_prompt::dynamic(env.resources.skill_tool()),
            Tool::Todo => tool_prompt::dynamic(crate::todo::TodoTool::new(
                env.todos.clone(),
                env.recorder.clone(),
                env.todo_owner.clone(),
                env.todo_parent.clone(),
            )),
            Tool::Memory => {
                tool_prompt::dynamic(crate::memory::MemoryTool::new(env.memory.clone()?))
            }
            Tool::CodeMap => tool_prompt::dynamic(bundle.code_map.clone()),
            Tool::CodeShow => tool_prompt::dynamic(bundle.code_show.clone()),
            Tool::CodeSurface => tool_prompt::dynamic(bundle.code_surface.clone()),
            Tool::CodeImplements => tool_prompt::dynamic(bundle.code_implements.clone()),
            Tool::CodeDeps => tool_prompt::dynamic(bundle.code_deps.clone()),
            Tool::CodeCycles => tool_prompt::dynamic(bundle.code_cycles.clone()),
            Tool::CodeCalls => tool_prompt::dynamic(bundle.code_calls.clone()),
            Tool::CodeTrace => tool_prompt::dynamic(bundle.code_trace.clone()),
            Tool::CodeImpact => tool_prompt::dynamic(bundle.code_impact.clone()),
            // Code retrieval rides on the memory index, so it exists only where
            // that index does — a search tool with nothing to search would be a
            // failure the model discovers by calling it.
            Tool::CodeSearch => tool_prompt::dynamic(crate::code_search::CodeSearchTool::new(
                env.memory.as_ref()?.handle().clone(),
                env.project_root(),
            )),
            Tool::CodeRelated => tool_prompt::dynamic(crate::code_search::CodeRelatedTool::new(
                env.memory.as_ref()?.handle().clone(),
                env.project_root(),
            )),
            Tool::AstQuery => tool_prompt::dynamic(bundle.ast_query.clone()),
            Tool::AstRewrite => tool_prompt::dynamic(bundle.ast_rewrite.clone()),
            Tool::Computer => tool_prompt::dynamic(artist_computer::ComputerTool::with_recorder(
                env.computer.clone()?,
                env.recorder.clone(),
                env.attachments.clone(),
            )),
            Tool::Canvas => tool_prompt::dynamic(crate::canvas::CanvasTool::new(
                env.project_root(),
                Arc::clone(env.canvas.as_ref()?),
                env.recorder.clone(),
            )),
            Tool::Handoff => {
                let handoff = env.handoff.as_ref()?;
                tool_prompt::dynamic(crate::handoff::HandoffTool::new(
                    handoff.pending.clone(),
                    handoff.profiles.clone(),
                    handoff.current.clone(),
                ))
            }
            Tool::Subagent => tool_prompt::dynamic(crate::delegate::Delegate::new(
                env,
                env.delegation.as_ref()?,
            )),
            // The message tools all need the same thing — this agent's mailbox
            // — so they stand or fall together, and an environment with no
            // identity has none of them rather than a confusing subset.
            Tool::Tell => tool_prompt::dynamic(env.message_tools()?),
            Tool::Query => {
                tool_prompt::dynamic(crate::message_tools::QueryTool(env.message_tools()?))
            }
            Tool::Reply => {
                tool_prompt::dynamic(crate::message_tools::ReplyTool(env.message_tools()?))
            }
            Tool::GroupChat => {
                tool_prompt::dynamic(crate::message_tools::GroupTool(env.message_tools()?))
            }
            Tool::Ask => tool_prompt::dynamic(crate::ask_tool::AskTool::new(
                env.ask.clone()?,
                env.cancel.clone(),
                env.delegation
                    .as_ref()
                    .and_then(|delegation| delegation.parent_permit.clone()),
            )),
        })
    }
}

impl ToolEnv {
    pub(crate) fn project_root(&self) -> PathBuf {
        self.bundle.project_root().to_path_buf()
    }

    /// The shared half of every message tool.
    ///
    /// Carries the delegation seat so a blocking `query` can release it: an
    /// agent parked waiting on another agent is not working, and N of them
    /// holding seats fills the project's pool. Same reasoning as a parent
    /// yielding while it awaits a child.
    fn message_tools(&self) -> Option<crate::message_tools::MessageTools> {
        Some(crate::message_tools::MessageTools::new(
            self.inbox.clone()?,
            self.project_root().display().to_string(),
            self.delegation
                .as_ref()
                .and_then(|delegation| delegation.parent_permit.clone()),
        ))
    }
}

/// What a web session may delegate to: the ingredients a `subagent` child is
/// built from, carried in a form the MCP server can assemble without reaching
/// into artist-agent's crate-private [`DelegationEnv`].
pub struct McpDelegation {
    /// The parent account a candidate that names no provider inherits.
    pub provider: llm_provider::SavedProvider,
    /// The history a `fork=true` child is seeded with. Empty over MCP: there is
    /// no running conversation to fork, so the child starts from its prompt.
    pub context: Arc<Vec<Message>>,
    /// The shared handles a child run needs (recorder, providers, cancel, ...).
    pub handles: SessionHandles,
    /// Where subagent display events go. The MCP server drops the receiver;
    /// delivery is best-effort by design.
    pub events: UnboundedSender<PromptEvent>,
    pub profiles: Profiles,
}

/// The durable identity claimed for an MCP actor before any connection serves.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpIdentity {
    /// Claim key and actor in one: reconnects re-claim the same name because
    /// registry claims are idempotent on the session.
    pub actor: String,
    pub profile: String,
    pub project: String,
    pub name: String,
    /// False when the registry was unavailable and `name` fell back to actor.
    pub registered: bool,
}

/// Everything the MCP surface is built from. An optional field means "the
/// process could not provide this subsystem", which keeps its tools absent
/// rather than present-and-broken — the availability rule applied honestly.
pub struct McpSurface {
    pub workspace: artist_tools::Workspace,
    pub profile: Profile,
    pub recorder: Option<Recorder>,
    pub outbox: Option<artist_session::AskOutbox>,
    pub attachments: Option<artist_session::AttachmentStore>,
    pub computer: Option<artist_computer::SurfaceRegistry>,
    pub canvas: Option<Arc<artist_canvas::server::Lazy>>,
    pub memory: Option<crate::memory::MemoryWriter>,
    pub delegation: Option<McpDelegation>,
    pub identity: Option<McpIdentity>,
}

/// Build the tool surface a headless MCP server should publish for a web user
/// driving `workspace`.
///
/// The environment is the web-shaped one: a recorder (a real one records tool
/// effects into the session log; `None` is a noop), file-backed todos, and
/// discovered skills — plus, when the daemon can provide them, the durable ask
/// outbox, a live recorder with attachments, a computer registry, a canvas, the
/// memory subsystem, delegation, and a durable identity. Everything absent is
/// `None`, which is the availability rule applied honestly rather than a list
/// to keep in sync: those tools are absent instead of present-and-broken.
///
/// Profile policy is applied exactly once, over the same constructed set as the
/// main agent and subagent paths — this is the same [`build`], with a different
/// environment, so the "one place a tool becomes available" invariant holds
/// for the MCP surface too.
pub fn mcp_surface(surface: McpSurface) -> Vec<ArtistDynamicTool> {
    let mut dynamic = Vec::new();
    if let Some(outbox) = &surface.outbox {
        // The blocking in-process ask tool is deliberately not available over
        // MCP (nobody in the MCP process answers it, and a blocked call
        // outlives the short-lived connection); the durable ask tools replace
        // it. `ask` in `Tool::ALL` stays absent because `env.ask` is None.
        dynamic.extend(crate::ask_outbox::tools(outbox.clone()));
    }
    let inbox = surface
        .identity
        .as_ref()
        .map(|identity| crate::messaging::Inbox::new(identity.name.clone()));
    let env = ToolEnv {
        bundle: ToolBundle::new(surface.workspace.clone()),
        recorder: surface
            .recorder
            .clone()
            .unwrap_or_else(artist_session::Recorder::noop),
        resources: Resources::discover(surface.workspace.root()),
        todos: crate::todo::TodoStore::default(),
        todo_owner: "mcp".into(),
        todo_parent: None,
        attachments: surface.attachments.clone(),
        computer: surface.computer.clone(),
        memory: surface.memory.clone(),
        canvas: surface.canvas.clone(),
        handoff: None,
        delegation: surface.delegation.as_ref().map(|delegation| DelegationEnv {
            provider: delegation.provider.clone(),
            context: Arc::clone(&delegation.context),
            handles: delegation.handles.clone(),
            events: delegation.events.clone(),
            profiles: delegation.profiles.clone(),
            parent_permit: None,
        }),
        ask: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        inbox,
        dynamic,
        disabled: Vec::new(),
    };
    build(&surface.profile, &env)
}

/// Build the tools a profile may use in this environment.
///
/// Order of operations, and why: construct everything the environment can
/// provide, fold in MCP and extension tools, then apply profile policy **once**
/// over all of them — which is what lets a profile trim a bloated MCP server
/// down to the handful it needs, in a subagent as much as at the root. Session
/// tool toggles come last because they are the user's live override rather than
/// the profile author's intent.
pub(crate) fn build(profile: &Profile, env: &ToolEnv) -> Vec<ArtistDynamicTool> {
    let mut tools: Vec<ArtistDynamicTool> = Tool::ALL
        .into_iter()
        .filter_map(|tool| tool.construct(env))
        .collect();
    tools.extend(env.dynamic.iter().cloned());
    tools.retain(|tool| profile.permits(tool.name()));
    tool_prompt::retain_enabled(&mut tools, &env.disabled);
    // Every tool learns to report files that moved underneath the model, in the
    // one place they all pass through — so this cannot be forgotten by a tool
    // added later, and it covers changes the harness did not make. A subagent
    // needs this more than the root does, not less: it runs concurrently with
    // whoever spawned it, in the same worktree.
    let drift = Some(env.bundle.edit.0.drift_watch());
    tools
        .into_iter()
        .map(|tool| tool_prompt::guard(tool, drift.clone()))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::profiles::NAVIGATION_TOOLS;
    use std::collections::BTreeSet;

    /// An environment with no optional subsystem present. Shared by the root
    /// and child shapes below, so a difference in the resulting tool set is
    /// necessarily a difference in the code under test rather than the fixture.
    pub(crate) fn env(root: &std::path::Path, actor: &str) -> ToolEnv {
        let workspace =
            artist_tools::Workspace::open(root, root.join(".artist/state"), actor).unwrap();
        ToolEnv {
            bundle: ToolBundle::new(workspace),
            recorder: Recorder::noop(),
            resources: Resources::discover(root),
            todos: crate::todo::TodoStore::default(),
            todo_owner: actor.to_owned(),
            todo_parent: None,
            attachments: None,
            computer: None,
            memory: None,
            canvas: None,
            handoff: None,
            delegation: None,
            ask: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            inbox: None,
            dynamic: Vec::new(),
            disabled: Vec::new(),
        }
    }

    fn names(profile: &Profile, env: &ToolEnv) -> BTreeSet<String> {
        build(profile, env)
            .into_iter()
            .map(|tool| tool.name().to_owned())
            .collect()
    }

    /// The bug this module exists to make impossible: `reviewer` was widened to
    /// allow every structural navigation tool, the widening was pinned by a
    /// test asserting `permits`, and the subagent path constructed none of
    /// them. Policy agreed; the surface did not.
    #[test]
    fn a_read_only_profile_gets_every_navigation_tool_it_permits() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = Profiles::discover_from(dir.path(), None);
        let env = env(dir.path(), "a-child");
        // The fixture has no memory index, and code retrieval rides on it. The
        // absence is the environment's answer, not policy's — so name the pair
        // here rather than loosening the assertion into "whatever we got".
        let memory_backed = [Tool::CodeSearch.name(), Tool::CodeRelated.name()];
        for name in ["explorer", "planner", "reviewer"] {
            let profile = profiles.get(name).unwrap();
            let registered = names(&profile, &env);
            for tool in NAVIGATION_TOOLS {
                assert_eq!(
                    registered.contains(*tool),
                    !memory_backed.contains(tool),
                    "{name}: {tool} present={} in an environment with no memory index",
                    registered.contains(*tool),
                );
            }
            assert!(!registered.contains("write"), "{name} must stay read-only");
            assert!(
                !registered.contains("ast_rewrite"),
                "{name} must not be offered a rewrite tool"
            );
        }
    }

    /// Registration and policy cannot disagree, because policy is one pass over
    /// what registration produced. Anything a profile permits and the
    /// environment can build is present; nothing else is.
    #[test]
    fn what_is_registered_is_exactly_what_is_permitted_and_available() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = Profiles::discover_from(dir.path(), None);
        let env = env(dir.path(), "a-child");
        for name in profiles.names() {
            let profile = profiles.get(&name).unwrap();
            let registered = names(&profile, &env);
            for tool in Tool::ALL {
                let available = tool.construct(&env).is_some();
                assert_eq!(
                    registered.contains(tool.name()),
                    available && profile.permits(tool.name()),
                    "{name}: {} permitted={} available={available}",
                    tool.name(),
                    profile.permits(tool.name()),
                );
            }
        }
    }

    /// The root and a subagent differ only in what their environments can
    /// provide — never in how policy is applied to it.
    #[test]
    fn the_root_and_a_child_differ_only_by_availability() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = Profiles::discover_from(dir.path(), None);
        let profile = profiles.get("worker").unwrap();

        let child = env(dir.path(), "a-child");
        let mut root = env(dir.path(), "session");
        root.handoff = Some(HandoffEnv {
            pending: crate::handoff::HandoffShared::default(),
            profiles: profiles.clone(),
            current: "worker".into(),
        });

        let child_names = names(&profile, &child);
        let root_names = names(&profile, &root);
        assert!(root_names.contains("handoff"));
        assert!(
            !child_names.contains("handoff"),
            "a subagent's run is its return value; there is no session beneath it to continue"
        );
        assert_eq!(
            root_names
                .difference(&child_names)
                .cloned()
                .collect::<Vec<_>>(),
            vec!["handoff".to_owned()],
            "the only difference must be the one the environment declared"
        );
    }

    /// MCP and extension tools go through the same glob pass as everything
    /// else. This was inert on the subagent path, which applied no pass at all.
    #[test]
    fn dynamic_tools_obey_profile_policy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".artist/profiles")).unwrap();
        std::fs::write(
            dir.path().join(".artist/profiles/gh.md"),
            "---\ndescription: gh\ntools:\n  allow: [read, \"mcp:github/*\"]\n  deny: [\"mcp:github/create_*\"]\n---\nprompt\n",
        )
        .unwrap();
        let profiles = Profiles::discover_from(dir.path(), None);
        let profile = profiles.get("gh").unwrap();

        let mut env = env(dir.path(), "a-child");
        env.dynamic = ["mcp:github/list_issues", "mcp:github/create_issue"]
            .into_iter()
            .map(|name| {
                ArtistDynamicTool::from_portable(
                    rig_core::tool::PortableDynamicTool::new(
                        name,
                        "an mcp tool",
                        serde_json::json!({"type": "object"}),
                        |_| Box::pin(async { Ok(rig_core::tool::ToolOutput::text("")) }),
                    ),
                    artist_tool_api::text_output_schema(name, "MCP test output."),
                    artist_tool_api::ToolCategory::External,
                    artist_tool_api::ArtistToolAnnotations::external_read(),
                )
            })
            .collect();

        let registered = names(&profile, &env);
        assert!(registered.contains("mcp:github/list_issues"));
        assert!(!registered.contains("mcp:github/create_issue"), "deny wins");
        assert!(!registered.contains("bash"));
    }

    #[test]
    fn a_disabled_tool_is_withheld_whatever_the_profile_allows() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = Profiles::discover_from(dir.path(), None);
        let profile = profiles.get("worker").unwrap();
        let mut env = env(dir.path(), "a-child");
        env.disabled = vec!["bash".to_owned()];
        let registered: BTreeSet<String> = build(&profile, &env)
            .into_iter()
            .map(|tool| tool.name().to_owned())
            .collect();
        assert!(!registered.contains("bash"));
        assert!(registered.contains("read"));
    }

    /// Names are what policy addresses; a duplicate would make one of the pair
    /// unaddressable, and is otherwise entirely silent.
    #[test]
    fn every_tool_name_is_distinct() {
        let names: BTreeSet<&str> = Tool::ALL.iter().map(|tool| tool.name()).collect();
        assert_eq!(names.len(), Tool::ALL.len());
    }
}
