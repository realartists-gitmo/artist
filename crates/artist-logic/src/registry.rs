//! Dynamically registrable operator semantics.
//!
//! Adding a construct must not require a storage migration, an enum variant, or
//! a recompile of the kernel. An operator is an object; its *meaning* is a
//! registry entry keyed by that object's id.
//!
//! The important consequence is the negative one: an operator with **no** entry
//! is still fully representable. It parses, prints, hashes, stores, round-trips
//! and can be quantified over. Evaluation reports
//! [`ComputeStatus::Unsupported`] and hands back a residual — which is a
//! different answer from "no evidence" and from "out of budget", and the caller
//! can tell.

use crate::evidence::{ComputeStatus, EvaluationResult};
use crate::object::{ObjectGraph, ObjectId};
use std::collections::BTreeMap;

/// What an evaluator needs while interpreting one node.
pub struct OpContext<'a> {
    pub graph: &'a ObjectGraph,
    pub operands: &'a [ObjectId],
    pub budget: &'a mut u64,
    pub snapshot: u64,
}

impl OpContext<'_> {
    /// Charge steps; `false` once the budget is gone.
    pub fn charge(&mut self, n: u64) -> bool {
        *self.budget = self.budget.saturating_sub(n);
        *self.budget > 0
    }
}

/// Semantics for one operator. Every method has a default that declines
/// honestly, so a partial implementation is legal and never unsound.
pub trait OperatorSemantics: Send + Sync {
    fn name(&self) -> &str;

    /// Type rule: the type of an application, if known.
    fn type_rule(&self, _cx: &OpContext<'_>) -> Option<ObjectId> {
        None
    }

    /// Structural simplification, independent of the structure.
    fn simplify(&self, _cx: &OpContext<'_>) -> Option<ObjectId> {
        None
    }

    fn evaluate_exact(&self, _cx: &mut OpContext<'_>) -> Option<EvaluationResult> {
        None
    }

    /// A sound lower bound on support.
    fn evaluate_lower(&self, _cx: &mut OpContext<'_>) -> Option<EvaluationResult> {
        None
    }

    /// A sound upper bound — what cannot be ruled out.
    fn evaluate_upper(&self, _cx: &mut OpContext<'_>) -> Option<EvaluationResult> {
        None
    }

    /// Resume from a persisted continuation.
    fn resume(&self, _cx: &mut OpContext<'_>, _continuation: ObjectId) -> Option<EvaluationResult> {
        None
    }

    /// Objects this application depends on, for invalidation.
    fn dependencies(&self, cx: &OpContext<'_>) -> Vec<ObjectId> {
        cx.operands.to_vec()
    }
}

/// Operator id → semantics.
#[derive(Default)]
pub struct OperatorRegistry {
    ops: BTreeMap<ObjectId, Box<dyn OperatorSemantics>>,
}

impl OperatorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register semantics. Sources may be built-ins, plugins, WASM extensions,
    /// proof engines, databases, external tools or models — the registry does
    /// not care where an implementation came from.
    pub fn register(&mut self, op: ObjectId, sem: Box<dyn OperatorSemantics>) {
        self.ops.insert(op, sem);
    }

    pub fn get(&self, op: ObjectId) -> Option<&dyn OperatorSemantics> {
        self.ops.get(&op).map(|b| b.as_ref())
    }

    pub fn has(&self, op: ObjectId) -> bool {
        self.ops.contains_key(&op)
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Evaluate an application, degrading honestly when semantics are absent.
    pub fn evaluate(
        &self,
        graph: &ObjectGraph,
        node: ObjectId,
        operator: ObjectId,
        operands: &[ObjectId],
        budget: &mut u64,
        snapshot: u64,
    ) -> EvaluationResult {
        let Some(sem) = self.get(operator) else {
            // Representable, not interpretable. A different answer from
            // "unknown" and from "out of budget".
            let mut r = EvaluationResult::new(ComputeStatus::Unsupported);
            r.residual = Some(node);
            r.dependencies = operands.to_vec();
            r.snapshot = snapshot;
            return r;
        };
        let mut cx = OpContext { graph, operands, budget, snapshot };
        if let Some(mut r) = sem.evaluate_exact(&mut cx) {
            r.snapshot = snapshot;
            return r;
        }
        let lower = sem.evaluate_lower(&mut cx);
        let upper = sem.evaluate_upper(&mut cx);
        let mut r = EvaluationResult::new(if *cx.budget == 0 {
            ComputeStatus::BudgetExhausted
        } else {
            ComputeStatus::Stalled
        });
        if let Some(l) = lower {
            r.support = l.support;
        }
        if let Some(u) = upper {
            r.refutation = u.refutation;
        }
        r.residual = Some(node);
        r.dependencies = sem.dependencies(&OpContext {
            graph,
            operands,
            budget: &mut 0,
            snapshot,
        });
        r.snapshot = snapshot;
        r
    }
}

impl std::fmt::Debug for OperatorRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperatorRegistry").field("operators", &self.ops.len()).finish()
    }
}
