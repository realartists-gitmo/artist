//! Stable, toolkit-independent projection of a session log into workspace objects.

use artist_session::{Envelope, RunFinished, SessionEvent};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FocusableObjectId(String);

impl FocusableObjectId {
    pub fn new(root: &str, lineage: &str, kind: ObjectKind, durable_id: &str) -> Self {
        Self(format!("{root}:{lineage}:{}:{durable_id}", kind.as_str()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Agent,
    Task,
    Stage,
    Canvas,
    Tool,
    Change,
    Ask,
}

impl ObjectKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Task => "task",
            Self::Stage => "stage",
            Self::Canvas => "canvas",
            Self::Tool => "tool",
            Self::Change => "change",
            Self::Ask => "ask",
        }
    }
    pub fn is_session_object(self) -> bool {
        matches!(self, Self::Agent | Self::Task | Self::Stage | Self::Canvas)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectState {
    Active,
    Completed,
    Dormant,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusableObject {
    pub id: FocusableObjectId,
    pub root_session: String,
    pub lineage: String,
    pub kind: ObjectKind,
    pub durable_id: String,
    pub title: String,
    pub state: ObjectState,
    pub updated_seq: u64,
    pub pinned: bool,
    pub parent: Option<FocusableObjectId>,
    pub tool_call_id: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionNode {
    pub object: FocusableObject,
    pub children: Vec<SessionNode>,
    pub hidden_children: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusStack {
    entries: Vec<FocusContext>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusContext {
    pub object: FocusableObjectId,
    /// Logical transcript item used as the stable restoration anchor.
    pub scroll_anchor: usize,
    /// Semantic focus target; frontends map this to their toolkit handle.
    pub keyboard_focus: Option<String>,
}

impl FocusStack {
    pub fn push(&mut self, id: FocusableObjectId) {
        self.push_context(id, 0, None);
    }
    pub fn push_context(
        &mut self,
        object: FocusableObjectId,
        scroll_anchor: usize,
        keyboard_focus: Option<String>,
    ) {
        self.entries.push(FocusContext {
            object,
            scroll_anchor,
            keyboard_focus,
        });
    }
    pub fn pop(&mut self) -> Option<FocusContext> {
        self.entries.pop()
    }
    pub fn jump_to(&mut self, id: &FocusableObjectId) -> bool {
        let Some(index) = self.entries.iter().position(|entry| &entry.object == id) else {
            return false;
        };
        self.entries.truncate(index + 1);
        true
    }
    pub fn current(&self) -> Option<&FocusableObjectId> {
        self.entries.last().map(|entry| &entry.object)
    }
    pub fn entries(&self) -> &[FocusContext] {
        &self.entries
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceProjection {
    objects: BTreeMap<FocusableObjectId, FocusableObject>,
    changes_by_tool: HashMap<String, Vec<FocusableObjectId>>,
}

impl WorkspaceProjection {
    pub fn from_envelopes(envelopes: &[Envelope]) -> Self {
        let mut projection = Self::default();
        for envelope in envelopes {
            projection.apply(envelope);
        }
        projection
    }

    pub fn objects(&self) -> impl Iterator<Item = &FocusableObject> {
        self.objects.values()
    }
    pub fn get(&self, id: &FocusableObjectId) -> Option<&FocusableObject> {
        self.objects.get(id)
    }
    pub fn changes_for_tool(&self, tool_call_id: &str) -> Vec<&FocusableObject> {
        self.changes_by_tool
            .get(tool_call_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.objects.get(id))
            .collect()
    }

    pub fn visible_tree(&self) -> Vec<SessionNode> {
        self.tree(None)
    }
    pub fn search(&self, query: &str) -> Vec<SessionNode> {
        let query = query.to_lowercase();
        let matched: BTreeSet<_> = self
            .objects
            .values()
            .filter(|o| {
                o.title.to_lowercase().contains(&query)
                    || o.detail
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|o| o.id.clone())
            .collect();
        self.tree(Some(&matched))
    }

    fn apply(&mut self, envelope: &Envelope) {
        self.ensure_agent(envelope);
        let parent = Some(Self::agent_id(envelope, &envelope.lineage));
        let event = envelope.event();
        match event {
            SessionEvent::RunStarted(run) => self.upsert(
                envelope,
                ObjectKind::Agent,
                &envelope.lineage,
                run.agent.unwrap_or_else(|| lineage_name(&envelope.lineage)),
                ObjectState::Active,
                None,
                None,
                None,
            ),
            SessionEvent::RunFinished(outcome) => {
                let state = match outcome {
                    RunFinished::Completed => ObjectState::Completed,
                    RunFinished::Cancelled => ObjectState::Interrupted,
                    _ => ObjectState::Failed,
                };
                self.set_state(
                    &Self::agent_id(envelope, &envelope.lineage),
                    state,
                    envelope.seq,
                );
            }
            SessionEvent::DelegateStarted(delegate) => {
                let id = Self::agent_id(envelope, &envelope.lineage);
                if let Some(object) = self.objects.get_mut(&id) {
                    object.state = ObjectState::Active;
                    object.detail = Some(delegate.prompt);
                    object.updated_seq = envelope.seq;
                }
            }
            SessionEvent::DelegateFinished(delegate) => {
                let state = if delegate.outcome == "completed" {
                    ObjectState::Completed
                } else if delegate.outcome == "cancelled" {
                    ObjectState::Interrupted
                } else {
                    ObjectState::Failed
                };
                self.set_state(
                    &Self::agent_id(envelope, &envelope.lineage),
                    state,
                    envelope.seq,
                );
            }
            SessionEvent::TaskStarted(task) => self.upsert(
                envelope,
                ObjectKind::Task,
                &task.task,
                task.command.clone(),
                ObjectState::Active,
                parent,
                Some(task.task.clone()),
                None,
            ),
            SessionEvent::TaskUpdated(task) => {
                self.update_detail(envelope, ObjectKind::Task, &task.task, task.output)
            }
            SessionEvent::TaskFinished(task) => {
                let state = if task.interrupted {
                    ObjectState::Interrupted
                } else if task.exit_code.unwrap_or(0) == 0 {
                    ObjectState::Completed
                } else {
                    ObjectState::Failed
                };
                self.set_state(
                    &FocusableObjectId::new(
                        &envelope.session,
                        &envelope.lineage,
                        ObjectKind::Task,
                        &task.task,
                    ),
                    state,
                    envelope.seq,
                );
            }
            SessionEvent::CanvasCreated(canvas) => self.upsert(
                envelope,
                ObjectKind::Canvas,
                &canvas.slug,
                canvas.title,
                ObjectState::Dormant,
                parent,
                None,
                None,
            ),
            SessionEvent::CanvasOpened(canvas) => self.upsert(
                envelope,
                ObjectKind::Canvas,
                &canvas.slug,
                canvas.slug.clone(),
                ObjectState::Active,
                parent,
                None,
                Some(format!("port {}", canvas.port)),
            ),
            SessionEvent::ComputerStageOpened(stage) => self.upsert(
                envelope,
                ObjectKind::Stage,
                &stage.stage,
                stage.stage.clone(),
                ObjectState::Active,
                parent,
                None,
                Some(stage.backend),
            ),
            SessionEvent::ComputerStageClosed(stage) => self.set_state(
                &FocusableObjectId::new(
                    &envelope.session,
                    &envelope.lineage,
                    ObjectKind::Stage,
                    &stage.stage,
                ),
                ObjectState::Completed,
                envelope.seq,
            ),
            SessionEvent::ChangeRecorded(change) => {
                let durable = format!("{}:{}", change.tool_call_id, change.path);
                let id = FocusableObjectId::new(
                    &envelope.session,
                    &envelope.lineage,
                    ObjectKind::Change,
                    &durable,
                );
                self.upsert(
                    envelope,
                    ObjectKind::Change,
                    &durable,
                    change.path,
                    ObjectState::Completed,
                    parent,
                    Some(change.tool_call_id.clone()),
                    change.diff.or(change.diff_attachment),
                );
                self.changes_by_tool
                    .entry(change.tool_call_id)
                    .or_default()
                    .push(id);
            }
            _ => {}
        }
    }

    fn ensure_agent(&mut self, e: &Envelope) {
        let parts: Vec<_> = e.lineage.split('/').collect();
        for index in 0..parts.len() {
            let lineage = parts[..=index].join("/");
            let parent = (index > 0).then(|| Self::agent_id(e, &parts[..index].join("/")));
            let id = Self::agent_id(e, &lineage);
            self.objects.entry(id.clone()).or_insert(FocusableObject {
                id,
                root_session: e.session.clone(),
                lineage: lineage.clone(),
                kind: ObjectKind::Agent,
                durable_id: lineage.clone(),
                title: lineage_name(&lineage),
                state: ObjectState::Dormant,
                updated_seq: e.seq,
                pinned: false,
                parent,
                tool_call_id: None,
                detail: None,
            });
        }
    }
    fn agent_id(e: &Envelope, lineage: &str) -> FocusableObjectId {
        FocusableObjectId::new(&e.session, lineage, ObjectKind::Agent, lineage)
    }
    fn upsert(
        &mut self,
        e: &Envelope,
        kind: ObjectKind,
        durable: &str,
        title: String,
        state: ObjectState,
        parent: Option<FocusableObjectId>,
        tool: Option<String>,
        detail: Option<String>,
    ) {
        let id = FocusableObjectId::new(&e.session, &e.lineage, kind, durable);
        let object = self.objects.entry(id.clone()).or_insert(FocusableObject {
            id,
            root_session: e.session.clone(),
            lineage: e.lineage.clone(),
            kind,
            durable_id: durable.into(),
            title: title.clone(),
            state,
            updated_seq: e.seq,
            pinned: false,
            parent,
            tool_call_id: tool,
            detail: None,
        });
        object.title = title;
        object.state = state;
        object.updated_seq = e.seq;
        if detail.is_some() {
            object.detail = detail;
        }
    }
    fn set_state(&mut self, id: &FocusableObjectId, state: ObjectState, seq: u64) {
        if let Some(o) = self.objects.get_mut(id) {
            o.state = state;
            o.updated_seq = seq;
        }
    }
    fn update_detail(&mut self, e: &Envelope, kind: ObjectKind, durable: &str, detail: String) {
        let id = FocusableObjectId::new(&e.session, &e.lineage, kind, durable);
        if let Some(o) = self.objects.get_mut(&id) {
            o.detail.get_or_insert_with(String::new).push_str(&detail);
            o.updated_seq = e.seq;
        }
    }
    fn tree(&self, matches: Option<&BTreeSet<FocusableObjectId>>) -> Vec<SessionNode> {
        let mut children: HashMap<Option<FocusableObjectId>, Vec<&FocusableObject>> =
            HashMap::new();
        for object in self.objects.values().filter(|o| o.kind.is_session_object()) {
            children
                .entry(object.parent.clone())
                .or_default()
                .push(object);
        }
        fn build(
            object: &FocusableObject,
            map: &HashMap<Option<FocusableObjectId>, Vec<&FocusableObject>>,
            matches: Option<&BTreeSet<FocusableObjectId>>,
        ) -> Option<SessionNode> {
            let mut raw = map
                .get(&Some(object.id.clone()))
                .cloned()
                .unwrap_or_default();
            raw.sort_by_key(|o| std::cmp::Reverse(o.updated_seq));
            let mut nodes: Vec<_> = raw
                .iter()
                .filter_map(|child| build(child, map, matches))
                .collect();
            let self_matches = matches.is_none_or(|set| set.contains(&object.id));
            if matches.is_some() && !self_matches && nodes.is_empty() {
                return None;
            }
            let hidden_children = if matches.is_none() {
                let keep: BTreeSet<_> = raw
                    .iter()
                    .filter(|o| o.pinned || o.state == ObjectState::Active)
                    .map(|o| o.id.clone())
                    .chain(
                        raw.iter()
                            .filter(|o| !o.pinned && o.state != ObjectState::Active)
                            .take(5)
                            .map(|o| o.id.clone()),
                    )
                    .collect();
                let before = nodes.len();
                nodes.retain(|n| keep.contains(&n.object.id));
                before - nodes.len()
            } else {
                0
            };
            Some(SessionNode {
                object: object.clone(),
                children: nodes,
                hidden_children,
            })
        }
        let mut roots = children.get(&None).cloned().unwrap_or_default();
        roots.sort_by_key(|o| std::cmp::Reverse(o.updated_seq));
        roots
            .into_iter()
            .filter_map(|root| build(root, &children, matches))
            .collect()
    }
}

fn lineage_name(lineage: &str) -> String {
    lineage
        .rsplit('/')
        .next()
        .unwrap_or(lineage)
        .trim_start_matches("delegate-")
        .to_owned()
}
