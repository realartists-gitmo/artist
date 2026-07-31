//! Four-valued evidence, and evaluation results that keep the reasons apart.
//!
//! Three-valued truth was wrong for a *memory*. `Unknown` was doing four jobs
//! at once: nothing is known, the evaluator ran out of budget, the operator has
//! no semantics, and — the one with no home at all — **both sides have
//! evidence**. A store that ingests claims from many sessions will hold support
//! for `P` and for `¬P` simultaneously, and collapsing that into the same value
//! as "no idea" destroys the only signal that says *look here*.
//!
//! So support and refutation are tracked independently, and computational
//! status is a separate axis again. "I have no evidence", "I have contradictory
//! evidence", "I ran out of budget" and "I cannot interpret this operator" are
//! four different answers and the caller can tell them apart.

use crate::object::ObjectId;
use std::collections::BTreeMap;

/// One side of the evidence, as a monotone bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Bound {
    /// Nothing establishes this side.
    #[default]
    None,
    /// Some evidence, not conclusive.
    Partial,
    /// Established.
    Certain,
}

impl Bound {
    pub fn meet(self, other: Bound) -> Bound {
        self.min(other)
    }
    pub fn join(self, other: Bound) -> Bound {
        self.max(other)
    }
    pub fn is_certain(self) -> bool {
        self == Bound::Certain
    }
}

/// The four evidential states, independent of how computation went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidential {
    /// Neither supported nor refuted.
    Open,
    Supported,
    Refuted,
    /// Both — a genuine contradiction in the store, not an absence.
    Conflicted,
}

/// Why evaluation is in the state it is in. Orthogonal to the evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComputeStatus {
    /// Ran to completion.
    Exact,
    /// Budget ran out; more budget would do more.
    BudgetExhausted,
    /// An operator here has no registered semantics. More budget will not help;
    /// registering semantics will.
    Unsupported,
    /// Ran to completion and made no progress — the abstraction is too weak or
    /// the question is undecidable. Re-asking will stall the same way.
    Stalled,
}

/// The result of evaluating an expression.
#[derive(Clone, Debug)]
pub struct EvaluationResult {
    pub support: Bound,
    pub refutation: Bound,
    pub compute_status: ComputeStatus,
    /// What is left to do, as an object in the same universe.
    pub residual: Option<ObjectId>,
    /// Serialized evaluator state, so work resumes rather than restarting.
    pub continuation: Option<ObjectId>,
    /// Objects this result depends on — the invalidation set.
    pub dependencies: Vec<ObjectId>,
    /// Which version of the universe this was computed against. Bounds from
    /// different versions must not be composed.
    pub snapshot: u64,
    pub spent: u64,
}

impl EvaluationResult {
    pub fn new(status: ComputeStatus) -> Self {
        EvaluationResult {
            support: Bound::None,
            refutation: Bound::None,
            compute_status: status,
            residual: None,
            continuation: None,
            dependencies: Vec::new(),
            snapshot: 0,
            spent: 0,
        }
    }

    pub fn certain(v: bool) -> Self {
        let mut r = EvaluationResult::new(ComputeStatus::Exact);
        if v {
            r.support = Bound::Certain;
        } else {
            r.refutation = Bound::Certain;
        }
        r
    }

    pub fn evidential(&self) -> Evidential {
        match (self.support, self.refutation) {
            (Bound::None, Bound::None) => Evidential::Open,
            (_, Bound::None) => Evidential::Supported,
            (Bound::None, _) => Evidential::Refuted,
            _ => Evidential::Conflicted,
        }
    }

    /// Only an exact, uncontradicted result licenses a definite reading.
    pub fn is_definite(&self) -> bool {
        self.compute_status == ComputeStatus::Exact
            && matches!(self.evidential(), Evidential::Supported | Evidential::Refuted)
    }

    pub fn negate(mut self) -> Self {
        std::mem::swap(&mut self.support, &mut self.refutation);
        self
    }
}

/// Polarity of an assertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Polarity {
    Affirm,
    Deny,
}

/// A proposition asserted *by someone, somewhere, at some time, under some
/// modality*.
///
/// The proposition is not the record. `prefers(adam, tabs)` is timeless and
/// agent-free; what a memory actually holds is that a particular agent asserted
/// it in a particular world over a particular interval. A `live` flag can exist
/// as a search-index projection, but it cannot be the semantic record — that was
/// exactly the conflation that made "what did I used to believe" unaskable.
#[derive(Clone, Debug)]
pub struct Assertion {
    pub id: ObjectId,
    pub proposition: ObjectId,
    pub polarity: Polarity,
    pub asserting_agent: Option<ObjectId>,
    pub world: Option<ObjectId>,
    /// Valid time: when the claim holds, in microseconds.
    pub valid_from: Option<i64>,
    pub valid_to: Option<i64>,
    /// Transaction time: when we learned it.
    pub recorded_at: i64,
    pub modality: Option<ObjectId>,
    pub interpretation: Option<ObjectId>,
    pub scope: Option<ObjectId>,
}

/// Evidence bearing on an assertion, stored separately from the proposition so
/// that retraction removes a *relation* and never the proposition itself.
#[derive(Clone, Debug)]
pub struct Evidence {
    pub id: ObjectId,
    pub target: ObjectId,
    pub polarity: Polarity,
    pub source: Option<ObjectId>,
    pub source_span: Option<SourceSpan>,
    pub transformation: Option<ObjectId>,
    pub derived_from: Vec<ObjectId>,
    /// Log-likelihood ratio, scaled by 1000 so it stays exact and hashable.
    pub llr_milli: Option<i64>,
    pub created_by: Option<ObjectId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    pub locator: String,
    pub start: u64,
    pub end: u64,
}

/// A ledger over assertions and evidence.
#[derive(Clone, Debug, Default)]
pub struct EvidenceLedger {
    assertions: BTreeMap<ObjectId, Assertion>,
    evidence: Vec<Evidence>,
}

impl EvidenceLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn assert(&mut self, a: Assertion) {
        self.assertions.insert(a.id, a);
    }

    pub fn record(&mut self, e: Evidence) {
        self.evidence.push(e);
    }

    pub fn assertion(&self, id: ObjectId) -> Option<&Assertion> {
        self.assertions.get(&id)
    }

    /// Evidence bearing on a proposition, in both directions.
    pub fn bearing_on(&self, proposition: ObjectId) -> (Vec<&Evidence>, Vec<&Evidence>) {
        let relevant: Vec<&Evidence> = self
            .evidence
            .iter()
            .filter(|e| {
                e.target == proposition
                    || self
                        .assertions
                        .get(&e.target)
                        .is_some_and(|a| a.proposition == proposition)
            })
            .collect();
        let (mut pro, mut con) = (Vec::new(), Vec::new());
        for e in relevant {
            match e.polarity {
                Polarity::Affirm => pro.push(e),
                Polarity::Deny => con.push(e),
            }
        }
        (pro, con)
    }

    /// The evidential state of a proposition.
    ///
    /// Deduplicated by **origin**, not content: independent corroboration
    /// compounds, hearing the same thing twice does not.
    pub fn state_of(&self, proposition: ObjectId) -> Evidential {
        let (pro, con) = self.bearing_on(proposition);
        let distinct = |v: &[&Evidence]| -> usize {
            let mut seen = std::collections::BTreeSet::new();
            for e in v {
                seen.insert(e.source.map(|s| s.0).unwrap_or(e.id.0));
            }
            seen.len()
        };
        match (distinct(&pro) > 0, distinct(&con) > 0) {
            (false, false) => Evidential::Open,
            (true, false) => Evidential::Supported,
            (false, true) => Evidential::Refuted,
            (true, true) => Evidential::Conflicted,
        }
    }

    /// Net weight in log-odds milli-units, origin-deduplicated.
    pub fn weight_of(&self, proposition: ObjectId) -> i64 {
        let (pro, con) = self.bearing_on(proposition);
        let sum = |v: &[&Evidence]| -> i64 {
            let mut by_origin: BTreeMap<u128, i64> = BTreeMap::new();
            for e in v {
                let key = e.source.map(|s| s.0).unwrap_or(e.id.0);
                by_origin.insert(key, e.llr_milli.unwrap_or(0));
            }
            by_origin.values().sum()
        };
        sum(&pro) - sum(&con)
    }

    pub fn evidence_count(&self) -> usize {
        self.evidence.len()
    }

    /// Retract an assertion. The proposition object is untouched — only the
    /// claim that someone asserted it stops holding.
    pub fn retract(&mut self, assertion: ObjectId, at: i64) {
        if let Some(a) = self.assertions.get_mut(&assertion) {
            a.valid_to = Some(at);
        }
    }
}
