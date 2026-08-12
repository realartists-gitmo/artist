//! Durable, typed navigational shortcuts between Artist resources.
//!
//! This is intentionally not the Muse fact store and does not infer ownership,
//! scheduling, or truth from an edge.  It is a small, separately stored
//! Mnestic projection for model-facing navigation only. Cycles are valid:
//! projections are one hop and never recursively expand a graph.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use cozo::{DataValue, DbInstance, ScriptMutability, ScriptRunOptions};
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

static STORES: OnceLock<dashmap::DashMap<std::path::PathBuf, RelationshipStore>> = OnceLock::new();
/// RocksDB refuses a second open while the first handle is being constructed.
/// Serialise just construction; normal relationship queries stay concurrent.
static STORE_OPEN: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub(crate) struct RelationshipStore {
    db: Arc<DbInstance>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Edge {
    pub id: String,
    pub source: String,
    pub kind: String,
    pub target: String,
    pub created_at_ms: i64,
}

impl RelationshipStore {
    pub(crate) fn for_project(project: &Path) -> Result<Self> {
        let path = project.join(".artist/state/relationships");
        if let Some(store) = STORES.get_or_init(dashmap::DashMap::new).get(&path) {
            return Ok(store.clone());
        }
        let _opening = STORE_OPEN
            .lock()
            .expect("relationship store open lock poisoned");
        if let Some(store) = STORES.get_or_init(dashmap::DashMap::new).get(&path) {
            return Ok(store.clone());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let db = DbInstance::new("rocksdb", &path, "").map_err(|error| {
            anyhow!(
                "opening relationship store at {}: {error:?}",
                path.display()
            )
        })?;
        if !db.set_durable_writes(true) {
            eprintln!(
                "warning: relationship store at {} does not support durable writes",
                path.display()
            );
        }
        let store = Self { db: Arc::new(db) };
        store.script(EDGE_DDL, BTreeMap::new(), true)?;
        let stores = STORES.get_or_init(dashmap::DashMap::new);
        match stores.entry(path) {
            dashmap::mapref::entry::Entry::Occupied(entry) => Ok(entry.get().clone()),
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(store.clone());
                Ok(store)
            }
        }
    }

    pub(crate) fn add(&self, source: &str, kind: &str, target: &str) -> Result<Edge> {
        validate_path(source, "source")?;
        validate_path(target, "target")?;
        validate_kind(kind)?;
        // `child` is the one relationship spelling with an immediate-parent
        // cardinality contract. Its source is intentionally any valid Artist
        // or real resource path, not just an agent. Other edge kinds remain
        // ordinary independent navigational shortcuts.
        if kind == "child" {
            let existing = self.list(None, Some(target), Some("child"))?;
            if let Some(edge) = existing.into_iter().next() {
                if edge.source == source {
                    return Ok(edge);
                }
                return Err(anyhow!(
                    "{target} already has immediate parent {}; remove relation://{} before assigning another",
                    edge.source,
                    edge.id
                ));
            }
        }
        let edge = Edge {
            id: Uuid::new_v4().to_string(),
            source: source.to_owned(),
            kind: kind.to_owned(),
            target: target.to_owned(),
            created_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        };
        let mut params = BTreeMap::new();
        params.insert("id".into(), DataValue::from(edge.id.as_str()));
        params.insert("source".into(), DataValue::from(edge.source.as_str()));
        params.insert("kind".into(), DataValue::from(edge.kind.as_str()));
        params.insert("target".into(), DataValue::from(edge.target.as_str()));
        params.insert("created_at_ms".into(), DataValue::from(edge.created_at_ms));
        self.script(
            "?[id, source, kind, target, created_at_ms] <- [[$id, $source, $kind, $target, $created_at_ms]] :put edge {id => source, kind, target, created_at_ms}",
            params,
            true,
        )?;
        Ok(edge)
    }

    pub(crate) fn remove(&self, id: &str) -> Result<bool> {
        let mut params = BTreeMap::new();
        params.insert("id".into(), DataValue::from(id));
        let result = self.script(
            "?[id] := *edge{id, source, kind, target, created_at_ms}, id == $id :rm edge {id}",
            params,
            true,
        )?;
        Ok(!result.rows.is_empty())
    }

    /// Removing a resource's shortcut edges is deliberately explicit and has
    /// no effect on the target resource itself.
    pub(crate) fn remove_for_resource(&self, resource: &str) -> Result<usize> {
        let mut params = BTreeMap::new();
        params.insert("resource".into(), DataValue::from(resource));
        let result = self.script(
            "?[id] := *edge{id, source, target}, source == $resource or target == $resource :rm edge {id}",
            params,
            true,
        )?;
        Ok(result.rows.len())
    }

    pub(crate) fn get(&self, id: &str) -> Result<Option<Edge>> {
        let mut params = BTreeMap::new();
        params.insert("id".into(), DataValue::from(id));
        Ok(self.rows_to_edges(self.script(
            "?[id, source, kind, target, created_at_ms] := *edge{id, source, kind, target, created_at_ms}, id == $id",
            params,
            false,
        )?)?.into_iter().next())
    }

    pub(crate) fn list(
        &self,
        source: Option<&str>,
        target: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<Edge>> {
        if let Some(source) = source {
            validate_path(source, "source")?;
        }
        if let Some(target) = target {
            validate_path(target, "target")?;
        }
        if let Some(kind) = kind {
            validate_kind(kind)?;
        }
        // The relation projection is intentionally small and navigation-first.
        // Filtering after one immutable Mnestic query keeps source and target
        // traversal symmetrical without giving model inputs a query language.
        let mut edges = self.rows_to_edges(self.script(
            "?[id, source, kind, target, created_at_ms] := *edge{id, source, kind, target, created_at_ms}",
            BTreeMap::new(),
            false,
        )?)?;
        edges.retain(|edge| {
            source.is_none_or(|value| edge.source == value)
                && target.is_none_or(|value| edge.target == value)
                && kind.is_none_or(|value| edge.kind == value)
        });
        Ok(edges)
    }

    fn rows_to_edges(&self, rows: cozo::NamedRows) -> Result<Vec<Edge>> {
        rows.rows
            .into_iter()
            .map(|row| {
                Ok(Edge {
                    id: row
                        .first()
                        .and_then(DataValue::get_str)
                        .ok_or_else(|| anyhow!("relationship id missing"))?
                        .to_owned(),
                    source: row
                        .get(1)
                        .and_then(DataValue::get_str)
                        .ok_or_else(|| anyhow!("relationship source missing"))?
                        .to_owned(),
                    kind: row
                        .get(2)
                        .and_then(DataValue::get_str)
                        .ok_or_else(|| anyhow!("relationship kind missing"))?
                        .to_owned(),
                    target: row
                        .get(3)
                        .and_then(DataValue::get_str)
                        .ok_or_else(|| anyhow!("relationship target missing"))?
                        .to_owned(),
                    created_at_ms: row
                        .get(4)
                        .and_then(DataValue::get_int)
                        .ok_or_else(|| anyhow!("relationship timestamp missing"))?,
                })
            })
            .collect()
    }

    fn script(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
        mutable: bool,
    ) -> Result<cozo::NamedRows> {
        self.db
            .run_script_with_options(
                script,
                params,
                if mutable {
                    ScriptMutability::Mutable
                } else {
                    ScriptMutability::Immutable
                },
                ScriptRunOptions::default(),
            )
            .map_err(|error| anyhow!("relationship query failed: {error:?}"))
    }
}

/// Explicit shortcut mutations.  Relationships never piggyback on `edit` or
/// `delete`: the model can see that it is changing navigation metadata rather
/// than the resource at either end of an edge.
#[derive(Clone)]
pub(crate) struct RelationshipTool {
    store: RelationshipStore,
}

impl RelationshipTool {
    pub(crate) fn new(store: RelationshipStore) -> Self {
        Self { store }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct RelationshipArgs {
    action: RelationshipAction,
    source: Option<String>,
    kind: Option<String>,
    target: Option<String>,
    id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum RelationshipAction {
    Add,
    Remove,
    List,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct RelationshipError(String);

impl From<RelationshipError> for ToolExecutionError {
    fn from(value: RelationshipError) -> Self {
        ToolExecutionError::other(value.to_string()).with_code("relationship_error")
    }
}

impl PortableTool for RelationshipTool {
    const NAME: &'static str = "relationship";
    type Error = RelationshipError;
    type Args = RelationshipArgs;
    type Output = Value;

    fn description(&self) -> String {
        "Create, list, or remove typed navigational shortcuts between Artist resource paths. These links never imply ownership, scheduling, or semantic truth.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["add", "remove", "list"]},
                "source": {"type": "string", "description": "Source resource path; required for add and optional outgoing-edge filter for list."},
                "kind": {"type": "string", "description": "Named shortcut type such as parent, child, predecessor, successor, or a project-defined label; required for add and optional list filter."},
                "target": {"type": "string", "description": "Target resource path; required for add and optional incoming-edge filter for list."},
                "id": {"type": "string", "description": "Relationship edge id; required for remove."}
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: RelationshipArgs) -> Result<Value, RelationshipError> {
        match args.action {
            RelationshipAction::Add => {
                let source = required(args.source, "source")?;
                let kind = required(args.kind, "kind")?;
                let target = required(args.target, "target")?;
                let edge = self
                    .store
                    .add(&source, &kind, &target)
                    .map_err(|error| RelationshipError(error.to_string()))?;
                Ok(json!({"path": format!("relation://{}", edge.id), "edge": edge}))
            }
            RelationshipAction::Remove => {
                let id = required(args.id, "id")?;
                if self
                    .store
                    .remove(&id)
                    .map_err(|error| RelationshipError(error.to_string()))?
                {
                    Ok(json!({"path": format!("relation://{id}"), "removed": true}))
                } else {
                    Err(RelationshipError(format!(
                        "unknown relationship relation://{id}"
                    )))
                }
            }
            RelationshipAction::List => {
                let edges = self
                    .store
                    .list(
                        args.source.as_deref(),
                        args.target.as_deref(),
                        args.kind.as_deref(),
                    )
                    .map_err(|error| RelationshipError(error.to_string()))?;
                Ok(
                    json!({"relationships": edges.into_iter().map(|edge| json!({"path": format!("relation://{}", edge.id), "edge": edge})).collect::<Vec<_>>() }),
                )
            }
        }
    }
}

fn required(value: Option<String>, name: &str) -> Result<String, RelationshipError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RelationshipError(format!("relationship {name} is required")))
}

const EDGE_DDL: &str = ":create edge { id: String => source: String, kind: String, target: String, created_at_ms: Int }";

fn validate_path(value: &str, field: &str) -> Result<()> {
    let path = artist_tools::resource_path::ResourcePath::parse(value).map_err(|error| {
        anyhow!("relationship {field} must be a valid Artist or real path: {error}")
    })?;
    if matches!(path, artist_tools::resource_path::ResourcePath::Real(ref path) if path.is_empty())
    {
        return Err(anyhow!("relationship {field} must not be empty"));
    }
    Ok(())
}

fn validate_kind(kind: &str) -> Result<()> {
    if kind.is_empty()
        || kind.len() > 64
        || !kind
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(anyhow!(
            "relationship kind must be 1-64 ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_are_typed_shortcuts_and_can_be_removed() {
        let project = tempfile::tempdir().unwrap();
        let store = RelationshipStore::for_project(project.path()).unwrap();
        let edge = store
            .add("agent://ada", "predecessor", "bash://build")
            .unwrap();
        assert_eq!(store.get(&edge.id).unwrap(), Some(edge.clone()));
        assert_eq!(
            store.list(Some("agent://ada"), None, None).unwrap(),
            vec![edge.clone()]
        );
        assert_eq!(
            store
                .list(None, Some("bash://build"), Some("predecessor"))
                .unwrap(),
            vec![edge.clone()]
        );
        assert!(store.remove(&edge.id).unwrap());
        assert!(store.get(&edge.id).unwrap().is_none());
    }

    #[test]
    fn child_has_one_immediate_parent_which_can_be_any_resource_kind() {
        let project = tempfile::tempdir().unwrap();
        let store = RelationshipStore::for_project(project.path()).unwrap();
        let edge = store
            .add("bash://build", "child", "agent://worker")
            .unwrap();
        assert_eq!(
            store
                .add("bash://build", "child", "agent://worker")
                .unwrap(),
            edge
        );
        let error = store
            .add("agent://other", "child", "agent://worker")
            .unwrap_err()
            .to_string();
        assert!(error.contains("already has immediate parent bash://build"));
        // Execution continuity is separate from parentage.
        store
            .add("agent://other", "predecessor", "agent://worker")
            .unwrap();
    }
}
