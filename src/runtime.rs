use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    domain::{
        ChildKind, EventData, ForkPoint, InitialContext, Node, NodeId, Run, RunHandle, RunId,
        RunStatus,
    },
    python::{PythonError, PythonOutput, PythonRepl},
    store::{SqliteStore, StoreError},
};

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("invalid {kind}: {details}")]
    Schema { kind: &'static str, details: String },
    #[error("run {run_id} is {status:?}; this operation requires an active run")]
    Inactive { run_id: RunId, status: RunStatus },
    #[error("run {0} is not waiting for an answer")]
    NotWaitingForAnswer(RunId),
    #[error("child run {0} has not completed")]
    ChildIncomplete(RunId),
    #[error("run {0} is not waiting for children")]
    NotWaitingForChildren(RunId),
    #[error("a fork needs at least one branch")]
    EmptyFork,
    #[error(transparent)]
    Python(#[from] PythonError),
    #[error("invalid Python harness call: {0}")]
    Rpc(String),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

pub struct Runtime {
    store: SqliteStore,
    system_instructions: String,
    workspace_instructions: String,
}

impl Runtime {
    pub fn new(
        store: SqliteStore,
        system_instructions: impl Into<String>,
        workspace_instructions: impl Into<String>,
    ) -> Self {
        Self {
            store,
            system_instructions: system_instructions.into(),
            workspace_instructions: workspace_instructions.into(),
        }
    }

    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    pub fn register_node(&self, node: &Node) -> Result<()> {
        compile_schema(&node.input_schema, "input schema")?;
        compile_schema(&node.output_schema, "output schema")?;
        self.store.insert_node(node)?;
        Ok(())
    }

    pub fn next_node_version(&self, previous: &Node) -> Node {
        let mut next = previous.clone();
        next.id = Uuid::new_v4();
        next.version += 1;
        next.created_at = chrono::Utc::now();
        next
    }

    pub fn start(&self, node_id: NodeId, input: Value) -> Result<RunHandle> {
        self.create_run(node_id, input, None, None, None)
    }

    fn create_run(
        &self,
        node_id: NodeId,
        input: Value,
        parent_id: Option<RunId>,
        child_kind: Option<ChildKind>,
        fork_point: Option<ForkPoint>,
    ) -> Result<RunHandle> {
        let node = self.store.get_node(node_id)?;
        validate(&node.input_schema, &input, "node input")?;
        let run_id = Uuid::new_v4();
        let initial_context = self.initial_context(&node);
        self.store.append_batch(vec![
            (
                run_id,
                EventData::RunCreated {
                    node_id,
                    input,
                    initial_context,
                    parent_id,
                    child_kind,
                    fork_point,
                },
            ),
            (run_id, EventData::RunStarted),
        ])?;
        Ok(RunHandle { run_id })
    }

    pub fn spawn(&self, parent_id: RunId, node_id: NodeId, input: Value) -> Result<RunHandle> {
        self.require_active(parent_id)?;
        self.create_run(
            node_id,
            input,
            Some(parent_id),
            Some(ChildKind::Spawn),
            None,
        )
    }

    /// Forks from the current event boundary. The returned branches start immediately.
    pub fn fork(&self, parent_id: RunId, inputs: Vec<Value>) -> Result<Vec<RunHandle>> {
        if inputs.is_empty() {
            return Err(RuntimeError::EmptyFork);
        }
        let parent = self.require_active(parent_id)?;
        let node = self.store.get_node(parent.node_id)?;
        for input in &inputs {
            validate(&node.input_schema, input, "fork input")?;
        }
        let point = ForkPoint {
            run_id: parent_id,
            through_sequence: parent.last_sequence,
        };
        let handles: Vec<_> = inputs
            .iter()
            .map(|_| RunHandle {
                run_id: Uuid::new_v4(),
            })
            .collect();
        let mut events = Vec::with_capacity(handles.len() * 2);
        for (handle, input) in handles.iter().zip(inputs) {
            events.push((
                handle.run_id,
                EventData::RunCreated {
                    node_id: parent.node_id,
                    input,
                    initial_context: parent.initial_context.clone(),
                    parent_id: Some(parent_id),
                    child_kind: Some(ChildKind::Fork),
                    fork_point: Some(point.clone()),
                },
            ));
            events.push((handle.run_id, EventData::RunStarted));
        }
        self.store.append_batch(events)?;
        Ok(handles)
    }

    /// Default fork behavior: fork and mark the parent as waiting for every branch.
    pub fn fork_and_join(&self, parent_id: RunId, inputs: Vec<Value>) -> Result<Vec<RunHandle>> {
        let handles = self.fork(parent_id, inputs)?;
        self.wait_for(parent_id, &handles)?;
        Ok(handles)
    }

    pub fn wait_for(&self, parent_id: RunId, handles: &[RunHandle]) -> Result<()> {
        self.require_active(parent_id)?;
        let child_ids = handles.iter().map(|handle| handle.run_id).collect();
        self.store
            .append(parent_id, EventData::ChildrenWaiting { child_ids })?;
        Ok(())
    }

    /// Wakes a waiting parent if all children yielded. Returns `None` if work remains.
    pub fn try_join(&self, parent_id: RunId) -> Result<Option<Vec<Value>>> {
        let parent = self.store.run(parent_id)?;
        if parent.status != RunStatus::WaitingForChildren {
            return Err(RuntimeError::NotWaitingForChildren(parent_id));
        }
        let mut results = Vec::with_capacity(parent.waiting_for.len());
        for child_id in &parent.waiting_for {
            let child = self.store.run(*child_id)?;
            let Some(result) = child.result else {
                return Ok(None);
            };
            results.push(result);
        }
        self.store.append(
            parent_id,
            EventData::ChildrenJoined {
                child_ids: parent.waiting_for,
                results: results.clone(),
            },
        )?;
        Ok(Some(results))
    }

    /// Ends the current run and atomically creates its successors.
    pub fn replace(
        &self,
        run_id: RunId,
        successors: Vec<(NodeId, Value)>,
    ) -> Result<Vec<RunHandle>> {
        let parent = self.require_active(run_id)?;
        if successors.is_empty() {
            return Err(RuntimeError::EmptyFork);
        }
        let mut prepared = Vec::with_capacity(successors.len());
        for (node_id, input) in successors {
            let node = self.store.get_node(node_id)?;
            validate(&node.input_schema, &input, "successor input")?;
            prepared.push((node, input, Uuid::new_v4()));
        }
        let handles: Vec<_> = prepared
            .iter()
            .map(|(_, _, id)| RunHandle { run_id: *id })
            .collect();
        let mut events = Vec::with_capacity(prepared.len() * 2 + 1);
        for (node, input, child_id) in prepared {
            events.push((
                child_id,
                EventData::RunCreated {
                    node_id: node.id,
                    input,
                    initial_context: self.initial_context(&node),
                    parent_id: Some(run_id),
                    child_kind: Some(ChildKind::Replacement),
                    fork_point: None,
                },
            ));
            events.push((child_id, EventData::RunStarted));
        }
        events.push((
            run_id,
            EventData::Replaced {
                successor_ids: handles.iter().map(|handle| handle.run_id).collect(),
            },
        ));
        self.store.append_batch(events)?;
        debug_assert_eq!(parent.id, run_id);
        Ok(handles)
    }

    pub fn note_repl_restart(&self, run_id: RunId, reason: impl Into<String>) -> Result<()> {
        self.require_active(run_id)?;
        self.store.append(
            run_id,
            EventData::ReplRestarted {
                reason: reason.into(),
            },
        )?;
        Ok(())
    }

    pub fn append_context(&self, run_id: RunId, text: impl Into<String>) -> Result<()> {
        self.require_active(run_id)?;
        self.store
            .append(run_id, EventData::ContextAppended { text: text.into() })?;
        Ok(())
    }

    pub fn begin_model_request(&self, run_id: RunId) -> Result<Uuid> {
        self.require_active(run_id)?;
        let request_id = Uuid::new_v4();
        self.store
            .append(run_id, EventData::ModelRequested { request_id })?;
        Ok(request_id)
    }

    pub fn record_model_response(
        &self,
        run_id: RunId,
        request_id: Uuid,
        content: Value,
    ) -> Result<()> {
        self.require_active(run_id)?;
        self.store.append(
            run_id,
            EventData::ModelResponse {
                request_id,
                content,
            },
        )?;
        Ok(())
    }

    pub fn interrupt(
        &self,
        run_id: RunId,
        action: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<()> {
        self.store.append(
            run_id,
            EventData::Interrupted {
                action: action.into(),
                reason: reason.into(),
            },
        )?;
        Ok(())
    }

    pub fn restart(&self, run_id: RunId, reason: impl Into<String>) -> Result<()> {
        let run = self.store.run(run_id)?;
        if run.status != RunStatus::Interrupted {
            return Err(RuntimeError::Inactive {
                run_id,
                status: run.status,
            });
        }
        self.store.append_batch(vec![
            (
                run_id,
                EventData::ReplRestarted {
                    reason: reason.into(),
                },
            ),
            (run_id, EventData::RunStarted),
        ])?;
        Ok(())
    }

    /// Executes one durable Python action and services graph calls from `artist`.
    pub async fn execute_python(
        &self,
        run_id: RunId,
        repl: &mut PythonRepl,
        code: &str,
    ) -> Result<PythonOutput> {
        self.require_active(run_id)?;
        let request_id = Uuid::new_v4();
        self.store.append(
            run_id,
            EventData::PythonRequested {
                request_id,
                code: code.to_owned(),
            },
        )?;
        let output = repl
            .execute(code, |method, params| {
                self.handle_python_rpc(run_id, method, params)
                    .map_err(|error| error.to_string())
            })
            .await;
        match output {
            Ok(output) => {
                self.store.append(
                    run_id,
                    EventData::PythonCompleted {
                        request_id,
                        output: serde_json::to_value(&output)
                            .expect("PythonOutput is serializable"),
                    },
                )?;
                Ok(output)
            }
            Err(error) => {
                self.store.append(
                    run_id,
                    EventData::Interrupted {
                        action: format!("python:{request_id}"),
                        reason: error.to_string(),
                    },
                )?;
                Err(error.into())
            }
        }
    }

    pub fn handle_python_rpc(&self, run_id: RunId, method: &str, params: Value) -> Result<Value> {
        match method {
            "spawn" => {
                let node_id = uuid_param(&params, "node_id")?;
                let input = params.get("input").cloned().unwrap_or(Value::Null);
                Ok(serde_json::to_value(self.spawn(run_id, node_id, input)?)
                    .expect("RunHandle is serializable"))
            }
            "fork" | "fork_and_join" => {
                let inputs = params
                    .get("inputs")
                    .and_then(Value::as_array)
                    .cloned()
                    .ok_or_else(|| RuntimeError::Rpc("inputs must be an array".into()))?;
                let handles = if method == "fork" {
                    self.fork(run_id, inputs)?
                } else {
                    self.fork_and_join(run_id, inputs)?
                };
                Ok(serde_json::to_value(handles).expect("RunHandle is serializable"))
            }
            "join" => {
                let ids = params
                    .get("run_ids")
                    .and_then(Value::as_array)
                    .ok_or_else(|| RuntimeError::Rpc("run_ids must be an array".into()))?;
                let handles = ids
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .ok_or_else(|| RuntimeError::Rpc("run ID must be a string".into()))
                            .and_then(|value| {
                                Uuid::parse_str(value)
                                    .map(|run_id| RunHandle { run_id })
                                    .map_err(|error| RuntimeError::Rpc(error.to_string()))
                            })
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.wait_for(run_id, &handles)?;
                Ok(serde_json::json!({"waiting_for": ids}))
            }
            "replace" => {
                let values = params
                    .get("successors")
                    .and_then(Value::as_array)
                    .ok_or_else(|| RuntimeError::Rpc("successors must be an array".into()))?;
                let successors = values
                    .iter()
                    .map(|item| {
                        Ok((
                            uuid_param(item, "node_id")?,
                            item.get("input").cloned().unwrap_or(Value::Null),
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(serde_json::to_value(self.replace(run_id, successors)?)
                    .expect("RunHandle is serializable"))
            }
            _ => Err(RuntimeError::Rpc(format!("unknown method {method:?}"))),
        }
    }

    pub fn ask(&self, run_id: RunId, question: impl Into<String>) -> Result<()> {
        self.require_active(run_id)?;
        self.store.append(
            run_id,
            EventData::Asked {
                question: question.into(),
            },
        )?;
        Ok(())
    }

    pub fn answer(&self, run_id: RunId, answer: impl Into<String>) -> Result<()> {
        let run = self.store.run(run_id)?;
        if run.status != RunStatus::WaitingForAnswer {
            return Err(RuntimeError::NotWaitingForAnswer(run_id));
        }
        self.store.append(
            run_id,
            EventData::Answered {
                answer: answer.into(),
            },
        )?;
        Ok(())
    }

    pub fn yield_value(&self, run_id: RunId, value: Value) -> Result<()> {
        let run = self.require_active(run_id)?;
        let node = self.store.get_node(run.node_id)?;
        validate(&node.output_schema, &value, "yield value")?;
        self.store.append(run_id, EventData::Yielded { value })?;
        Ok(())
    }

    pub fn cancel(&self, run_id: RunId, reason: impl Into<String>) -> Result<()> {
        self.require_active(run_id)?;
        self.store.append(
            run_id,
            EventData::Cancelled {
                reason: reason.into(),
            },
        )?;
        Ok(())
    }

    fn initial_context(&self, node: &Node) -> InitialContext {
        InitialContext {
            system: self.system_instructions.clone(),
            agents: self.workspace_instructions.clone(),
            node: node.instructions.clone(),
        }
    }

    fn require_active(&self, run_id: RunId) -> Result<Run> {
        let run = self.store.run(run_id)?;
        if run.status != RunStatus::Running {
            return Err(RuntimeError::Inactive {
                run_id,
                status: run.status,
            });
        }
        Ok(run)
    }
}

fn uuid_param(params: &Value, key: &str) -> Result<Uuid> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| RuntimeError::Rpc(format!("{key} must be a UUID string")))?;
    Uuid::parse_str(value).map_err(|error| RuntimeError::Rpc(error.to_string()))
}

fn compile_schema(schema: &Value, kind: &'static str) -> Result<()> {
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|error| RuntimeError::Schema {
            kind,
            details: error.to_string(),
        })
}

fn validate(schema: &Value, value: &Value, kind: &'static str) -> Result<()> {
    let validator = jsonschema::validator_for(schema).map_err(|error| RuntimeError::Schema {
        kind,
        details: format!("invalid schema: {error}"),
    })?;
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|error| error.to_string())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(RuntimeError::Schema {
            kind,
            details: errors.join("; "),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn runtime() -> (Runtime, Node) {
        let runtime = Runtime::new(SqliteStore::memory().unwrap(), "system", "agents");
        let node = Node::new(
            "worker",
            "work",
            json!({"type":"object", "required":["task"], "properties":{"task":{"type":"string"}}}),
            json!({"type":"object", "required":["answer"], "properties":{"answer":{"type":"string"}}}),
        );
        runtime.register_node(&node).unwrap();
        (runtime, node)
    }

    #[test]
    fn yield_is_typed_and_terminal() {
        let (runtime, node) = runtime();
        let run = runtime.start(node.id, json!({"task":"x"})).unwrap();
        assert!(
            runtime
                .yield_value(run.run_id, json!({"wrong": 1}))
                .is_err()
        );
        runtime
            .yield_value(run.run_id, json!({"answer":"done"}))
            .unwrap();
        assert_eq!(
            runtime.store.run(run.run_id).unwrap().status,
            RunStatus::Completed
        );
        assert!(runtime.append_context(run.run_id, "too late").is_err());
    }

    #[test]
    fn default_fork_pauses_and_joins_parent() {
        let (runtime, node) = runtime();
        let parent = runtime.start(node.id, json!({"task":"parent"})).unwrap();
        let children = runtime
            .fork_and_join(
                parent.run_id,
                vec![json!({"task":"a"}), json!({"task":"b"})],
            )
            .unwrap();
        assert_eq!(
            runtime.store.run(parent.run_id).unwrap().status,
            RunStatus::WaitingForChildren
        );
        assert_eq!(runtime.try_join(parent.run_id).unwrap(), None);
        for (index, child) in children.iter().enumerate() {
            runtime
                .yield_value(child.run_id, json!({"answer": index.to_string()}))
                .unwrap();
        }
        let results = runtime.try_join(parent.run_id).unwrap().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            runtime.store.run(parent.run_id).unwrap().status,
            RunStatus::Running
        );
    }
}
