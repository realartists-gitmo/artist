//! One harness-owned todo tree per durable Artist identity.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use artist_session::{
    Recorder, TodoClosedStatus, TodoItem, TodoOpenStatus, TodoStatus, TodoUpdated,
};
use rig_core::tool::{PortableTool, ToolExecutionError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Node {
    id: String,
    text: String,
    status: TodoStatus,
    #[serde(default)]
    children: Vec<Node>,
}

impl Node {
    fn new(text: String) -> Self {
        Self {
            id: artist_tools::short_id("todo"),
            text,
            status: TodoStatus::Open {
                open: TodoOpenStatus::Idle,
            },
            children: Vec::new(),
        }
    }

    fn public(&self) -> TodoItem {
        TodoItem {
            text: self.text.clone(),
            status: self.status,
            children: self.children.iter().map(Node::public).collect(),
        }
    }

    fn from_public(item: &TodoItem) -> Self {
        Self {
            id: artist_tools::short_id("todo"),
            text: item.text.clone(),
            status: item.status,
            children: item.children.iter().map(Self::from_public).collect(),
        }
    }
}

/// Shared in-process cache with optional durable per-project backing. The tool
/// never exposes an owner argument, so one artist cannot inspect another tree.
#[derive(Clone, Default)]
pub struct TodoStore {
    lists: Arc<Mutex<HashMap<String, Vec<Node>>>>,
    durable_dir: Option<Arc<PathBuf>>,
}

impl TodoStore {
    pub fn for_project(project: &Path) -> Self {
        Self {
            lists: Arc::default(),
            durable_dir: Some(Arc::new(project.join(".artist/state/registry/todos"))),
        }
    }

    pub fn get(&self, owner: &str) -> Vec<TodoItem> {
        self.load(owner)
            .unwrap_or_default()
            .iter()
            .map(Node::public)
            .collect()
    }

    pub fn set(&self, owner: &str, items: Vec<TodoItem>) {
        let nodes = items.iter().map(Node::from_public).collect::<Vec<_>>();
        let _ = self.commit(owner, nodes);
    }

    pub fn restore(&self, events: &[artist_session::Envelope]) {
        for envelope in artist_session::visible_events(events) {
            if let artist_session::SessionEvent::TodoUpdated(update) = envelope.event() {
                self.set(&update.owner, update.items);
            }
        }
    }

    fn load(&self, owner: &str) -> Result<Vec<Node>, TodoError> {
        if let Some(dir) = self.durable_dir.as_deref() {
            let path = todo_path(dir, owner);
            match fs::read(&path) {
                Ok(bytes) => {
                    let nodes = serde_json::from_slice::<Vec<Node>>(&bytes)
                        .map_err(|error| TodoError(error.to_string()))?;
                    self.lists
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(owner.to_owned(), nodes.clone());
                    return Ok(nodes);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(TodoError(error.to_string())),
            }
        }
        Ok(self
            .lists
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(owner)
            .cloned()
            .unwrap_or_default())
    }

    fn commit(&self, owner: &str, nodes: Vec<Node>) -> Result<(), TodoError> {
        if let Some(dir) = self.durable_dir.as_deref() {
            fs::create_dir_all(dir).map_err(|error| TodoError(error.to_string()))?;
            let path = todo_path(dir, owner);
            let tmp = dir.join(format!(".{}.tmp", artist_tools::short_id("todo")));
            fs::write(
                &tmp,
                serde_json::to_vec(&nodes).map_err(|error| TodoError(error.to_string()))?,
            )
            .map_err(|error| TodoError(error.to_string()))?;
            fs::rename(&tmp, &path).map_err(|error| TodoError(error.to_string()))?;
        }
        self.lists
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(owner.to_owned(), nodes);
        Ok(())
    }
}

fn todo_path(dir: &Path, owner: &str) -> PathBuf {
    let encoded = owner
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    dir.join(format!("{encoded}.json"))
}

pub fn render(items: &[TodoItem]) -> String {
    fn walk(items: &[TodoItem], depth: usize, out: &mut String) {
        for item in items {
            let mark = match item.status {
                TodoStatus::Open {
                    open: TodoOpenStatus::Idle,
                } => " ",
                TodoStatus::Open {
                    open: TodoOpenStatus::Active,
                } => "~",
                TodoStatus::Closed {
                    closed: TodoClosedStatus::Done,
                } => "x",
                TodoStatus::Closed {
                    closed: TodoClosedStatus::Failed,
                } => "!",
                TodoStatus::Closed {
                    closed: TodoClosedStatus::Cancelled,
                } => "-",
                TodoStatus::Closed {
                    closed: TodoClosedStatus::Inherited,
                } => "·",
            };
            out.push_str(&format!("{}[{mark}] {}\n", "  ".repeat(depth), item.text));
            walk(&item.children, depth + 1, out);
        }
    }
    if items.is_empty() {
        return String::new();
    }
    let mut lines = String::new();
    walk(items, 0, &mut lines);
    format!("<todos>\n{lines}</todos>")
}

#[derive(Clone)]
pub(crate) struct TodoTool {
    store: TodoStore,
    recorder: Recorder,
    owner: String,
}

impl TodoTool {
    pub fn new(store: TodoStore, recorder: Recorder, owner: String) -> Self {
        Self {
            store,
            recorder,
            owner,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TodoArgs {
    ops: Option<Vec<TodoOp>>,
    get: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TodoOp {
    Add { add: AddOp },
    Remove { remove: PathOp },
    Replace { replace: ReplaceOp },
    Move { r#move: MoveOp },
    Update { update: UpdateOp },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddOp {
    path: String,
    value: TextValue,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceOp {
    path: String,
    value: TextValue,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextValue {
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathOp {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveOp {
    from: String,
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateOp {
    paths: Vec<String>,
    status: RequestedStatus,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
enum RequestedStatus {
    Open { open: TodoOpenStatus },
    Closed { closed: RequestedClosedStatus },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequestedClosedStatus {
    Done,
    Failed,
    Cancelled,
}

impl RequestedStatus {
    fn status(self) -> TodoStatus {
        match self {
            Self::Open { open } => TodoStatus::Open { open },
            Self::Closed { closed } => TodoStatus::Closed {
                closed: match closed {
                    RequestedClosedStatus::Done => TodoClosedStatus::Done,
                    RequestedClosedStatus::Failed => TodoClosedStatus::Failed,
                    RequestedClosedStatus::Cancelled => TodoClosedStatus::Cancelled,
                },
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct TodoError(String);

impl From<TodoError> for ToolExecutionError {
    fn from(value: TodoError) -> Self {
        ToolExecutionError::other(value.to_string()).with_code("todo_error")
    }
}

impl PortableTool for TodoTool {
    const NAME: &'static str = "todo";
    type Error = TodoError;
    type Args = TodoArgs;
    type Output = String;

    fn description(&self) -> String {
        "Maintain this artist session's todo tree. Use exactly one of ops or get. Mutation batches are atomic; paths are zero-based RFC-6901-style child indices such as /3/2 and '-' appends.".into()
    }

    fn parameters(&self) -> Value {
        todo_schema()
    }

    async fn call(&self, args: TodoArgs) -> Result<String, TodoError> {
        match (args.ops, args.get) {
            (Some(ops), None) if !ops.is_empty() => self.apply(ops),
            (None, Some(filter)) => self.get(&filter),
            (Some(_), Some(_)) => Err(TodoError("exactly one of `ops` or `get` is allowed".into())),
            (Some(_), None) => Err(TodoError("`ops` must be non-empty".into())),
            _ => Err(TodoError(
                "exactly one of `ops` or `get` is required".into(),
            )),
        }
    }
}

impl TodoTool {
    fn apply(&self, ops: Vec<TodoOp>) -> Result<String, TodoError> {
        let mut tree = self.store.load(&self.owner)?;
        let original = tree.clone();
        let mut structural = false;
        let mut confirmations = Vec::new();

        // Work only on the clone. Any error drops every earlier operation in this batch.
        for op in ops {
            match op {
                TodoOp::Add { add } => {
                    validate_text(&add.value.text)?;
                    insert_node(&mut tree, &add.path, Node::new(add.value.text))?;
                    reopen_ancestors_for_open_descendants(&mut tree);
                    structural = true;
                }
                TodoOp::Remove { remove } => {
                    remove_node(&mut tree, &remove.path)?;
                    structural = true;
                }
                TodoOp::Replace { replace } => {
                    validate_text(&replace.value.text)?;
                    node_mut(&mut tree, &parse_existing_path(&replace.path)?)?.text =
                        replace.value.text;
                    confirmations.push(format!("replaced {}", replace.path));
                }
                TodoOp::Move { r#move } => {
                    let from = parse_existing_path(&r#move.from)?;
                    let node = remove_node_indices(&mut tree, &from)?;
                    // RFC remove-then-add semantics: resolve destination after source removal.
                    insert_node(&mut tree, &r#move.path, node)?;
                    reopen_ancestors_for_open_descendants(&mut tree);
                    structural = true;
                }
                TodoOp::Update { update } => {
                    apply_update(&mut tree, update)?;
                    confirmations.push("updated status".into());
                }
            }
        }
        assert_open_invariant(&tree)?;
        self.store.commit(&self.owner, tree.clone())?;
        let public = tree.iter().map(Node::public).collect::<Vec<_>>();
        self.recorder.record(TodoUpdated {
            owner: self.owner.clone(),
            items: public,
        });

        if structural {
            let diff = displacement_diff(&original, &tree);
            if diff.is_empty() {
                Ok("updated todo tree; no existing item paths changed".into())
            } else {
                Ok(diff.join("\n"))
            }
        } else {
            Ok(confirmations.join("; "))
        }
    }

    fn get(&self, filter: &str) -> Result<String, TodoError> {
        let tree = self.store.load(&self.owner)?;
        let parsed = Filter::parse(filter)?;
        let roots = if let Some(path) = parsed.path.as_deref() {
            let indices = parse_existing_path(path)?;
            flatten_subtree(path, node_ref(&tree, &indices)?)
        } else {
            flatten(&tree)
        };

        let matches = roots
            .into_iter()
            .filter(|(_, node)| parsed.matches(node))
            .map(|(path, node)| {
                format!("{path}  \"{}\"  [{}]", node.text, status_text(node.status))
            })
            .collect::<Vec<_>>();
        Ok(if matches.is_empty() {
            "No matching todo items.".into()
        } else {
            matches.join("\n")
        })
    }
}

fn todo_schema() -> Value {
    let requested_status = json!({
        "oneOf":[
            {"type":"object","properties":{"open":{"enum":["idle","active"]}},"required":["open"],"additionalProperties":false},
            {"type":"object","properties":{"closed":{"enum":["done","failed","cancelled"]}},"required":["closed"],"additionalProperties":false}
        ]
    });
    let text_value = json!({
        "type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false
    });
    json!({
        "type":"object",
        "properties":{
            "ops":{
                "type":"array","minItems":1,"items":{"oneOf":[
                    {"type":"object","properties":{"add":{"type":"object","properties":{"path":{"type":"string"},"value":text_value.clone()},"required":["path","value"],"additionalProperties":false}},"required":["add"],"additionalProperties":false},
                    {"type":"object","properties":{"remove":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}},"required":["remove"],"additionalProperties":false},
                    {"type":"object","properties":{"replace":{"type":"object","properties":{"path":{"type":"string"},"value":text_value},"required":["path","value"],"additionalProperties":false}},"required":["replace"],"additionalProperties":false},
                    {"type":"object","properties":{"move":{"type":"object","properties":{"from":{"type":"string"},"path":{"type":"string"}},"required":["from","path"],"additionalProperties":false}},"required":["move"],"additionalProperties":false},
                    {"type":"object","properties":{"update":{"type":"object","properties":{"paths":{"type":"array","minItems":1,"items":{"type":"string"}},"status":requested_status},"required":["paths","status"],"additionalProperties":false}},"required":["update"],"additionalProperties":false}
                ]}
            },
            "get":{"type":"string"}
        },
        "oneOf":[
            {"required":["ops"],"not":{"required":["get"]}},
            {"required":["get"],"not":{"required":["ops"]}}
        ],
        "additionalProperties":false
    })
}

fn validate_text(text: &str) -> Result<(), TodoError> {
    if text.trim().is_empty() {
        Err(TodoError("todo text cannot be empty".into()))
    } else {
        Ok(())
    }
}

fn parse_existing_path(path: &str) -> Result<Vec<usize>, TodoError> {
    if path == "/" || !path.starts_with('/') {
        return Err(TodoError(format!("invalid todo path `{path}`")));
    }
    path[1..]
        .split('/')
        .map(|part| {
            if part == "-" || part.is_empty() {
                return Err(TodoError(format!(
                    "path `{path}` must address an existing item"
                )));
            }
            part.parse::<usize>()
                .map_err(|_| TodoError(format!("invalid numeric path `{path}`")))
        })
        .collect()
}

fn parse_insert_path(path: &str) -> Result<(Vec<usize>, Option<usize>), TodoError> {
    if !path.starts_with('/') || path == "/" {
        return Err(TodoError(format!("invalid todo path `{path}`")));
    }
    let mut parts = path[1..].split('/').collect::<Vec<_>>();
    let last = parts.pop().unwrap();
    let parents = parts
        .into_iter()
        .map(|part| {
            if part.is_empty() || part == "-" {
                return Err(TodoError(format!("invalid parent path `{path}`")));
            }
            part.parse::<usize>()
                .map_err(|_| TodoError(format!("invalid numeric path `{path}`")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let index = if last == "-" {
        None
    } else {
        Some(
            last.parse::<usize>()
                .map_err(|_| TodoError(format!("invalid numeric path `{path}`")))?,
        )
    };
    Ok((parents, index))
}

fn children_mut<'a>(
    tree: &'a mut Vec<Node>,
    parents: &[usize],
) -> Result<&'a mut Vec<Node>, TodoError> {
    let mut current = tree;
    for &index in parents {
        let node = current
            .get_mut(index)
            .ok_or_else(|| TodoError("stale todo path".into()))?;
        current = &mut node.children;
    }
    Ok(current)
}

fn node_mut<'a>(tree: &'a mut Vec<Node>, path: &[usize]) -> Result<&'a mut Node, TodoError> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| TodoError("empty todo path".into()))?;
    children_mut(tree, parents)?
        .get_mut(*last)
        .ok_or_else(|| TodoError("stale todo path".into()))
}

fn node_ref<'a>(tree: &'a [Node], path: &[usize]) -> Result<&'a Node, TodoError> {
    let mut current = tree;
    let mut node = None;
    for &index in path {
        let found = current
            .get(index)
            .ok_or_else(|| TodoError("stale todo path".into()))?;
        node = Some(found);
        current = &found.children;
    }
    node.ok_or_else(|| TodoError("empty todo path".into()))
}

fn insert_node(tree: &mut Vec<Node>, path: &str, node: Node) -> Result<(), TodoError> {
    let (parents, index) = parse_insert_path(path)?;
    let children = children_mut(tree, &parents)?;
    match index {
        None => children.push(node),
        Some(index) if index <= children.len() => children.insert(index, node),
        Some(_) => return Err(TodoError("stale todo insertion path".into())),
    }
    Ok(())
}

fn remove_node(tree: &mut Vec<Node>, path: &str) -> Result<Node, TodoError> {
    let path = parse_existing_path(path)?;
    remove_node_indices(tree, &path)
}

fn remove_node_indices(tree: &mut Vec<Node>, path: &[usize]) -> Result<Node, TodoError> {
    let (last, parents) = path
        .split_last()
        .ok_or_else(|| TodoError("empty todo path".into()))?;
    let children = children_mut(tree, parents)?;
    if *last >= children.len() {
        return Err(TodoError("stale todo path".into()));
    }
    Ok(children.remove(*last))
}

fn apply_update(tree: &mut Vec<Node>, update: UpdateOp) -> Result<(), TodoError> {
    if update.paths.is_empty() {
        return Err(TodoError("update.paths must be non-empty".into()));
    }
    // Resolve every target against the pre-operation tree before mutating anything.
    let paths = update
        .paths
        .iter()
        .map(|path| {
            parse_existing_path(path).and_then(|indices| {
                node_ref(tree, &indices)?;
                Ok(indices)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let explicit_ids = paths
        .iter()
        .map(|path| node_ref(tree, path).map(|node| node.id.clone()))
        .collect::<Result<HashSet<_>, _>>()?;
    let status = update.status.status();

    for path in &paths {
        node_mut(tree, path)?.status = status;
    }
    match status {
        TodoStatus::Open { .. } => reopen_ancestors_for_open_descendants(tree),
        TodoStatus::Closed { .. } => {
            fn cascade(node: &mut Node, explicit: &HashSet<String>) {
                for child in &mut node.children {
                    if child.status.is_open() && !explicit.contains(&child.id) {
                        child.status = TodoStatus::Closed {
                            closed: TodoClosedStatus::Inherited,
                        };
                    }
                    cascade(child, explicit);
                }
            }
            for path in &paths {
                cascade(node_mut(tree, path)?, &explicit_ids);
            }
        }
    }
    Ok(())
}

fn reopen_ancestors_for_open_descendants(tree: &mut [Node]) {
    fn repair(node: &mut Node) -> bool {
        let descendant_open = node.children.iter_mut().any(repair);
        let self_open = node.status.is_open();
        if descendant_open && !self_open {
            node.status = TodoStatus::Open {
                open: TodoOpenStatus::Idle,
            };
        }
        self_open || descendant_open || node.status.is_open()
    }
    for node in tree {
        repair(node);
    }
}

fn assert_open_invariant(tree: &[Node]) -> Result<(), TodoError> {
    fn walk(node: &Node) -> Result<bool, TodoError> {
        let child_open = node
            .children
            .iter()
            .map(walk)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .any(|open| open);
        if child_open && !node.status.is_open() {
            return Err(TodoError("todo open-descendant invariant violated".into()));
        }
        Ok(node.status.is_open() || child_open)
    }
    for node in tree {
        walk(node)?;
    }
    Ok(())
}

fn flatten(tree: &[Node]) -> Vec<(String, &Node)> {
    fn walk<'a>(nodes: &'a [Node], prefix: &str, out: &mut Vec<(String, &'a Node)>) {
        for (index, node) in nodes.iter().enumerate() {
            let path = format!("{prefix}/{index}");
            out.push((path.clone(), node));
            walk(&node.children, &path, out);
        }
    }
    let mut out = Vec::new();
    walk(tree, "", &mut out);
    out
}

fn flatten_subtree<'a>(root_path: &str, root: &'a Node) -> Vec<(String, &'a Node)> {
    fn walk<'a>(node: &'a Node, path: String, out: &mut Vec<(String, &'a Node)>) {
        out.push((path.clone(), node));
        for (index, child) in node.children.iter().enumerate() {
            walk(child, format!("{path}/{index}"), out);
        }
    }
    let mut out = Vec::new();
    walk(root, root_path.to_owned(), &mut out);
    out
}

fn positions(tree: &[Node]) -> BTreeMap<String, (String, String, TodoStatus)> {
    flatten(tree)
        .into_iter()
        .map(|(path, node)| (node.id.clone(), (path, node.text.clone(), node.status)))
        .collect()
}

fn displacement_diff(before: &[Node], after: &[Node]) -> Vec<String> {
    let old = positions(before);
    let new = positions(after);
    old.into_iter()
        .filter_map(|(id, (old_path, text, status))| {
            new.get(&id).and_then(|(new_path, _, _)| {
                (new_path != &old_path).then(|| {
                    format!(
                        "{old_path} -> {new_path}  \"{text}\"  [{}]",
                        status_text(status)
                    )
                })
            })
        })
        .collect()
}

fn status_text(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Open {
            open: TodoOpenStatus::Idle,
        } => "open idle",
        TodoStatus::Open {
            open: TodoOpenStatus::Active,
        } => "open active",
        TodoStatus::Closed {
            closed: TodoClosedStatus::Done,
        } => "closed done",
        TodoStatus::Closed {
            closed: TodoClosedStatus::Failed,
        } => "closed failed",
        TodoStatus::Closed {
            closed: TodoClosedStatus::Cancelled,
        } => "closed cancelled",
        TodoStatus::Closed {
            closed: TodoClosedStatus::Inherited,
        } => "closed inherited",
    }
}

struct Filter {
    path: Option<String>,
    statuses: HashSet<&'static str>,
    text: Vec<String>,
}

impl Filter {
    fn parse(input: &str) -> Result<Self, TodoError> {
        let tokens = filter_tokens(input);
        let mut path = None;
        let mut statuses = HashSet::new();
        let mut text = Vec::new();
        for (token, literal) in tokens {
            if !literal && token.starts_with('/') && path.is_none() {
                parse_existing_path(&token)?;
                path = Some(token);
                continue;
            }
            if !literal && is_status_token(&token) {
                statuses.insert(Box::leak(token.into_boxed_str()) as &'static str);
                continue;
            }
            let term = token.strip_prefix('~').unwrap_or(&token).to_owned();
            if !term.is_empty() {
                text.push(term);
            }
        }
        Ok(Self {
            path,
            statuses,
            text,
        })
    }

    fn matches(&self, node: &Node) -> bool {
        if self.statuses.is_empty() {
            if self.path.is_none() && self.text.is_empty() && !node.status.is_open() {
                return false;
            }
        } else if !self
            .statuses
            .iter()
            .any(|status| status_matches(node.status, status))
        {
            return false;
        }
        self.text
            .iter()
            .all(|term| fuzzy_contains(term, &node.text))
    }
}

fn is_status_token(token: &str) -> bool {
    matches!(
        token,
        "open" | "closed" | "idle" | "active" | "done" | "failed" | "cancelled" | "inherited"
    )
}

fn status_matches(status: TodoStatus, token: &str) -> bool {
    match token {
        "open" => status.is_open(),
        "closed" => !status.is_open(),
        "idle" => matches!(
            status,
            TodoStatus::Open {
                open: TodoOpenStatus::Idle
            }
        ),
        "active" => matches!(
            status,
            TodoStatus::Open {
                open: TodoOpenStatus::Active
            }
        ),
        "done" => matches!(
            status,
            TodoStatus::Closed {
                closed: TodoClosedStatus::Done
            }
        ),
        "failed" => matches!(
            status,
            TodoStatus::Closed {
                closed: TodoClosedStatus::Failed
            }
        ),
        "cancelled" => matches!(
            status,
            TodoStatus::Closed {
                closed: TodoClosedStatus::Cancelled
            }
        ),
        "inherited" => matches!(
            status,
            TodoStatus::Closed {
                closed: TodoClosedStatus::Inherited
            }
        ),
        _ => false,
    }
}

fn fuzzy_contains(needle: &str, haystack: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let config = neo_frizbee::Config {
        max_typos: Some((needle.chars().count() / 3).max(1).min(u16::MAX as usize) as u16),
        casing: neo_frizbee::CaseMatching::Smart,
        ..Default::default()
    };
    !neo_frizbee::match_list(needle, &[haystack], &config).is_empty()
}

fn filter_tokens(input: &str) -> Vec<(String, bool)> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut token_quoted = false;
    for ch in input.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                token_quoted = true;
            }
            ch if ch.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    result.push((std::mem::take(&mut current), token_quoted));
                    token_quoted = false;
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        result.push((current, token_quoted));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> (TodoStore, TodoTool) {
        let store = TodoStore::default();
        let tool = TodoTool::new(store.clone(), Recorder::noop(), "Goethe".into());
        (store, tool)
    }

    async fn call(tool: &TodoTool, value: Value) -> Result<String, TodoError> {
        tool.call(serde_json::from_value(value).unwrap()).await
    }

    #[tokio::test]
    async fn closing_parent_inherits_open_descendants_and_reopen_does_not_reopen_them() {
        let (_, tool) = tool();
        call(
            &tool,
            json!({"ops":[
                {"add":{"path":"/-","value":{"text":"parent"}}},
                {"add":{"path":"/0/-","value":{"text":"child"}}}
            ]}),
        )
        .await
        .unwrap();
        call(
            &tool,
            json!({"ops":[{"update":{"paths":["/0"],"status":{"closed":"done"}}}]}),
        )
        .await
        .unwrap();
        let tree = tool.store.get("Goethe");
        assert_eq!(
            tree[0].status,
            TodoStatus::Closed {
                closed: TodoClosedStatus::Done
            }
        );
        assert_eq!(
            tree[0].children[0].status,
            TodoStatus::Closed {
                closed: TodoClosedStatus::Inherited
            }
        );
        call(
            &tool,
            json!({"ops":[{"update":{"paths":["/0"],"status":{"open":"active"}}}]}),
        )
        .await
        .unwrap();
        let tree = tool.store.get("Goethe");
        assert_eq!(
            tree[0].status,
            TodoStatus::Open {
                open: TodoOpenStatus::Active
            }
        );
        assert_eq!(
            tree[0].children[0].status,
            TodoStatus::Closed {
                closed: TodoClosedStatus::Inherited
            }
        );
    }

    #[tokio::test]
    async fn opening_descendant_repairs_closed_ancestors() {
        let (_, tool) = tool();
        call(
            &tool,
            json!({"ops":[
                {"add":{"path":"/-","value":{"text":"parent"}}},
                {"add":{"path":"/0/-","value":{"text":"child"}}},
                {"update":{"paths":["/0"],"status":{"closed":"done"}}},
                {"update":{"paths":["/0/0"],"status":{"open":"active"}}}
            ]}),
        )
        .await
        .unwrap();
        let tree = tool.store.get("Goethe");
        assert_eq!(
            tree[0].status,
            TodoStatus::Open {
                open: TodoOpenStatus::Idle
            }
        );
        assert_eq!(
            tree[0].children[0].status,
            TodoStatus::Open {
                open: TodoOpenStatus::Active
            }
        );
    }

    #[tokio::test]
    async fn a_failed_batch_commits_nothing() {
        let (_, tool) = tool();
        let error = call(
            &tool,
            json!({"ops":[
                {"add":{"path":"/-","value":{"text":"kept only on clone"}}},
                {"remove":{"path":"/9"}}
            ]}),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("stale"));
        assert!(tool.store.get("Goethe").is_empty());
    }

    #[tokio::test]
    async fn displacement_includes_descendants() {
        let (_, tool) = tool();
        call(
            &tool,
            json!({"ops":[
                {"add":{"path":"/-","value":{"text":"A"}}},
                {"add":{"path":"/0/-","value":{"text":"A child"}}},
                {"add":{"path":"/-","value":{"text":"B"}}}
            ]}),
        )
        .await
        .unwrap();
        let result = call(
            &tool,
            json!({"ops":[{"add":{"path":"/0","value":{"text":"new"}}}]}),
        )
        .await
        .unwrap();
        assert!(result.contains("/0 -> /1  \"A\""));
        assert!(result.contains("/0/0 -> /1/0  \"A child\""));
        assert!(result.contains("/1 -> /2  \"B\""));
    }

    #[tokio::test]
    async fn reserved_status_words_are_status_unless_escaped() {
        let (_, tool) = tool();
        call(
            &tool,
            json!({"ops":[{"add":{"path":"/-","value":{"text":"open the gate"}}}]}),
        )
        .await
        .unwrap();
        let status = call(&tool, json!({"get":"open"})).await.unwrap();
        assert!(status.contains("open the gate"));
        let literal = call(&tool, json!({"get":"~open"})).await.unwrap();
        assert!(literal.contains("open the gate"));
    }

    #[tokio::test]
    async fn empty_filter_returns_only_open_items_and_path_returns_the_subtree() {
        let (_, tool) = tool();
        call(
            &tool,
            json!({"ops":[
                {"add":{"path":"/-","value":{"text":"open root"}}},
                {"add":{"path":"/0/-","value":{"text":"open child"}}},
                {"add":{"path":"/-","value":{"text":"closed root"}}},
                {"update":{"paths":["/1"],"status":{"closed":"done"}}}
            ]}),
        )
        .await
        .unwrap();

        let open = call(&tool, json!({"get":""})).await.unwrap();
        assert!(open.contains("/0  \"open root\""));
        assert!(open.contains("/0/0  \"open child\""));
        assert!(!open.contains("closed root"));

        let subtree = call(&tool, json!({"get":"/0"})).await.unwrap();
        assert!(subtree.contains("/0  \"open root\""));
        assert!(subtree.contains("/0/0  \"open child\""));
    }

    #[test]
    fn schema_has_no_mode_and_cannot_request_inherited() {
        let schema = todo_schema();
        assert!(schema["properties"].get("mode").is_none());
        let rendered = schema.to_string();
        assert!(!rendered.contains("inherited"));
        assert!(rendered.contains("\"ops\""));
        assert!(rendered.contains("\"get\""));
    }
}
