//! Executable, serializable, untrusted theory libraries above the Artist kernel.
//!
//! These libraries never extend kernel definitional equality. They validate finite
//! certificates and produce ordinary kernel terms, theories, or explicit evidence.

#![forbid(unsafe_code)]

mod package;
mod universal;

pub use package::{
    AssumptionVisibility, ElaborationRule, NotationRule, PackageError, PackageExtensions,
    PackageId, PackageImport, SemanticArtifact, THEORY_PACKAGE_FORMAT,
};
pub use universal::{
    CheckerCompilation, ComputationCertificate, Register, UniversalCertificate, UniversalError,
    UniversalInstruction, UniversalProgram, UniversalRun, UniversalState, UniversalStatus,
    UniversalVerifier, instruction_inputs,
};

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use artist_kernel::{
    Certificate, ConstructorField, DependencyReport, InductiveConstructor, InductiveDeclaration,
    Kernel, KernelError, KernelRequest, KernelResult, Provenance, Term, Theory, TheoryBuilder,
    TheoryError,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable symbolic identifier used by reflected libraries.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Symbol(pub String);

impl Symbol {
    /// Creates a symbol.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl From<&str> for Symbol {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Arbitrary finite first-order tree used to reflect statements, states, and terms.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "object", rename_all = "snake_case")]
pub enum Object {
    /// Atomic object.
    Atom(Symbol),
    /// Constructor-headed object.
    Node { head: Symbol, children: Vec<Object> },
}

impl Object {
    /// Constructs an atom.
    #[must_use]
    pub fn atom(value: impl Into<Symbol>) -> Self {
        Self::Atom(value.into())
    }

    /// Constructs a node.
    #[must_use]
    pub fn node(head: impl Into<Symbol>, children: Vec<Self>) -> Self {
        Self::Node {
            head: head.into(),
            children,
        }
    }
}

/// Pattern for a reflected object. Variables are consistently shared across a rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "pattern", rename_all = "snake_case")]
pub enum Pattern {
    /// Pattern variable.
    Var(Symbol),
    /// Exact atom.
    Atom(Symbol),
    /// Exact constructor with recursive child patterns.
    Node {
        head: Symbol,
        children: Vec<Pattern>,
    },
}

impl Pattern {
    /// Constructs a pattern variable.
    #[must_use]
    pub fn var(value: impl Into<Symbol>) -> Self {
        Self::Var(value.into())
    }

    /// Constructs an exact atom pattern.
    #[must_use]
    pub fn atom(value: impl Into<Symbol>) -> Self {
        Self::Atom(value.into())
    }

    /// Constructs a node pattern.
    #[must_use]
    pub fn node(head: impl Into<Symbol>, children: Vec<Self>) -> Self {
        Self::Node {
            head: head.into(),
            children,
        }
    }
}

type Substitution = BTreeMap<Symbol, Object>;

fn match_pattern(pattern: &Pattern, object: &Object, substitution: &mut Substitution) -> bool {
    match (pattern, object) {
        (Pattern::Var(variable), object) => {
            if let Some(existing) = substitution.get(variable) {
                existing == object
            } else {
                substitution.insert(variable.clone(), object.clone());
                true
            }
        }
        (Pattern::Atom(expected), Object::Atom(actual)) => expected == actual,
        (
            Pattern::Node {
                head: expected_head,
                children: expected_children,
            },
            Object::Node {
                head: actual_head,
                children: actual_children,
            },
        ) => {
            expected_head == actual_head
                && expected_children.len() == actual_children.len()
                && expected_children
                    .iter()
                    .zip(actual_children)
                    .all(|(child_pattern, child)| match_pattern(child_pattern, child, substitution))
        }
        _ => false,
    }
}

fn instantiate_pattern(pattern: &Pattern, substitution: &Substitution) -> Option<Object> {
    match pattern {
        Pattern::Var(variable) => substitution.get(variable).cloned(),
        Pattern::Atom(atom) => Some(Object::Atom(atom.clone())),
        Pattern::Node { head, children } => Some(Object::Node {
            head: head.clone(),
            children: children
                .iter()
                .map(|child| instantiate_pattern(child, substitution))
                .collect::<Option<Vec<_>>>()?,
        }),
    }
}

/// One finitary rule schema in a reflected proof system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSchema {
    /// Rule identifier.
    pub name: Symbol,
    /// Premise schemas.
    pub premises: Vec<Pattern>,
    /// Conclusion schema.
    pub conclusion: Pattern,
}

/// A finite proof tree. `rule = None` denotes an explicit assumption leaf.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofNode {
    /// Judgment established by this node.
    pub conclusion: Object,
    /// Applied rule, or `None` for an assumption.
    pub rule: Option<Symbol>,
    /// Proofs of rule premises.
    pub premises: Vec<ProofNode>,
}

/// Serializable arbitrary finitary proof system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectedProofSystem {
    /// Stable system name.
    pub name: Symbol,
    /// Finite rule schemas. Rules may describe derivations in any object logic.
    pub rules: Vec<RuleSchema>,
}

/// Reflected-proof rejection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ProofError {
    /// Assumption leaf was not supplied in the explicit context.
    #[error("undeclared assumption {0:?}")]
    UndeclaredAssumption(Object),
    /// An assumption leaf contained subproofs.
    #[error("assumption leaf contains premises")]
    AssumptionHasPremises,
    /// Named rule does not exist.
    #[error("unknown rule {0:?}")]
    UnknownRule(Symbol),
    /// Wrong number of premise proofs.
    #[error("rule {rule:?} expects {expected} premises, found {actual}")]
    PremiseCount {
        rule: Symbol,
        expected: usize,
        actual: usize,
    },
    /// Rule schema does not instantiate to the supplied judgment tree.
    #[error("rule {0:?} does not match the supplied proof node")]
    SchemaMismatch(Symbol),
}

impl ReflectedProofSystem {
    /// Checks a complete finite derivation against explicit assumptions.
    pub fn check(
        &self,
        assumptions: &BTreeSet<Object>,
        proof: &ProofNode,
    ) -> Result<(), ProofError> {
        self.check_node(assumptions, proof)
    }

    fn check_node(
        &self,
        assumptions: &BTreeSet<Object>,
        proof: &ProofNode,
    ) -> Result<(), ProofError> {
        let Some(rule_name) = &proof.rule else {
            if !proof.premises.is_empty() {
                return Err(ProofError::AssumptionHasPremises);
            }
            return assumptions
                .contains(&proof.conclusion)
                .then_some(())
                .ok_or_else(|| ProofError::UndeclaredAssumption(proof.conclusion.clone()));
        };
        let rule = self
            .rules
            .iter()
            .find(|candidate| &candidate.name == rule_name)
            .ok_or_else(|| ProofError::UnknownRule(rule_name.clone()))?;
        if proof.premises.len() != rule.premises.len() {
            return Err(ProofError::PremiseCount {
                rule: rule_name.clone(),
                expected: rule.premises.len(),
                actual: proof.premises.len(),
            });
        }
        for premise in &proof.premises {
            self.check_node(assumptions, premise)?;
        }
        let mut substitution = Substitution::new();
        if !match_pattern(&rule.conclusion, &proof.conclusion, &mut substitution) {
            return Err(ProofError::SchemaMismatch(rule_name.clone()));
        }
        for (schema, premise) in rule.premises.iter().zip(&proof.premises) {
            if !match_pattern(schema, &premise.conclusion, &mut substitution) {
                return Err(ProofError::SchemaMismatch(rule_name.clone()));
            }
        }
        Ok(())
    }
}

/// One arbitrary rewrite schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewriteRule {
    /// Rule name.
    pub name: Symbol,
    /// Matched subobject.
    pub lhs: Pattern,
    /// Replacement; all variables must have been bound by `lhs`.
    pub rhs: Pattern,
}

/// One certified rewrite at an explicit tree path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewriteStep {
    /// Rule name.
    pub rule: Symbol,
    /// Child indices from root to rewritten subobject.
    pub path: Vec<usize>,
    /// Complete input object.
    pub before: Object,
    /// Complete output object.
    pub after: Object,
}

/// Finite trace through a potentially nonterminating or nonconfluent rewrite system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewriteTrace {
    /// Initial object.
    pub start: Object,
    /// Explicit steps.
    pub steps: Vec<RewriteStep>,
    /// Claimed result.
    pub finish: Object,
}

/// Rewrite-trace rejection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RewriteError {
    /// Consecutive states do not connect.
    #[error("rewrite trace is disconnected at step {0}")]
    Disconnected(usize),
    /// Rule is unknown.
    #[error("unknown rewrite rule {0:?}")]
    UnknownRule(Symbol),
    /// Path does not select a subobject.
    #[error("invalid rewrite path at step {0}")]
    InvalidPath(usize),
    /// Left pattern does not match selected subobject.
    #[error("rewrite left side does not match at step {0}")]
    LeftMismatch(usize),
    /// Right pattern contains an unbound variable.
    #[error("rewrite right side has an unbound variable at step {0}")]
    UnboundRightVariable(usize),
    /// Claimed output is not the exact replacement result.
    #[error("rewrite output differs from rule result at step {0}")]
    WrongOutput(usize),
    /// Trace does not end at its claimed result.
    #[error("rewrite trace has the wrong finish")]
    WrongFinish,
}

/// Checks a finite trace without requiring normalization or equivalence to be decidable.
pub fn check_rewrite_trace(
    rules: &[RewriteRule],
    trace: &RewriteTrace,
) -> Result<(), RewriteError> {
    let mut current = trace.start.clone();
    for (position, step) in trace.steps.iter().enumerate() {
        if step.before != current {
            return Err(RewriteError::Disconnected(position));
        }
        let rule = rules
            .iter()
            .find(|candidate| candidate.name == step.rule)
            .ok_or_else(|| RewriteError::UnknownRule(step.rule.clone()))?;
        let selected =
            object_at_path(&step.before, &step.path).ok_or(RewriteError::InvalidPath(position))?;
        let mut substitution = Substitution::new();
        if !match_pattern(&rule.lhs, selected, &mut substitution) {
            return Err(RewriteError::LeftMismatch(position));
        }
        let replacement = instantiate_pattern(&rule.rhs, &substitution)
            .ok_or(RewriteError::UnboundRightVariable(position))?;
        let expected = replace_at_path(&step.before, &step.path, replacement)
            .ok_or(RewriteError::InvalidPath(position))?;
        if expected != step.after {
            return Err(RewriteError::WrongOutput(position));
        }
        current = step.after.clone();
    }
    if current == trace.finish {
        Ok(())
    } else {
        Err(RewriteError::WrongFinish)
    }
}

fn object_at_path<'a>(object: &'a Object, path: &[usize]) -> Option<&'a Object> {
    let mut cursor = object;
    for index in path {
        let Object::Node { children, .. } = cursor else {
            return None;
        };
        cursor = children.get(*index)?;
    }
    Some(cursor)
}

fn replace_at_path(object: &Object, path: &[usize], replacement: Object) -> Option<Object> {
    let Some((first, rest)) = path.split_first() else {
        return Some(replacement);
    };
    let Object::Node { head, children } = object else {
        return None;
    };
    let child = children.get(*first)?;
    let replaced = replace_at_path(child, rest, replacement)?;
    let mut output = children.clone();
    output[*first] = replaced;
    Some(Object::Node {
        head: head.clone(),
        children: output,
    })
}

/// Operational rule for a program or state machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionRule {
    /// Transition label.
    pub label: Symbol,
    /// State schema before the transition.
    pub before: Pattern,
    /// State schema afterward.
    pub after: Pattern,
}

/// One explicit program transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// Applied transition label.
    pub label: Symbol,
    /// Input state.
    pub before: Object,
    /// Output state.
    pub after: Object,
}

/// Finite program execution certificate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionTrace {
    /// Initial state.
    pub initial: Object,
    /// Explicit transitions.
    pub steps: Vec<ExecutionStep>,
    /// Final state.
    pub final_state: Object,
}

/// Program-trace rejection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ExecutionError {
    /// Trace states do not connect.
    #[error("execution trace is disconnected at step {0}")]
    Disconnected(usize),
    /// Transition label is unknown.
    #[error("unknown transition {0:?}")]
    UnknownTransition(Symbol),
    /// Transition schema does not produce the claimed state.
    #[error("transition does not match at step {0}")]
    TransitionMismatch(usize),
    /// Claimed final state is wrong.
    #[error("wrong final program state")]
    WrongFinalState,
}

/// Checks a program trace. Search or execution itself remains untrusted.
pub fn check_execution(
    rules: &[TransitionRule],
    trace: &ExecutionTrace,
) -> Result<(), ExecutionError> {
    let mut current = trace.initial.clone();
    for (position, step) in trace.steps.iter().enumerate() {
        if step.before != current {
            return Err(ExecutionError::Disconnected(position));
        }
        let rule = rules
            .iter()
            .find(|candidate| candidate.label == step.label)
            .ok_or_else(|| ExecutionError::UnknownTransition(step.label.clone()))?;
        let mut substitution = Substitution::new();
        if !match_pattern(&rule.before, &step.before, &mut substitution)
            || instantiate_pattern(&rule.after, &substitution).as_ref() != Some(&step.after)
        {
            return Err(ExecutionError::TransitionMismatch(position));
        }
        current = step.after.clone();
    }
    if current == trace.final_state {
        Ok(())
    } else {
        Err(ExecutionError::WrongFinalState)
    }
}

/// Finite transition graph used for invariant and liveness certificates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionGraph {
    /// Declared states.
    pub states: BTreeSet<Symbol>,
    /// Directed transitions.
    pub edges: BTreeSet<(Symbol, Symbol)>,
}

/// Finite inductive certificate that every reachable transition remains safe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyInvariant {
    /// Initial state.
    pub initial: Symbol,
    /// Inductive invariant containing the initial state.
    pub invariant: BTreeSet<Symbol>,
    /// States satisfying the represented safety predicate.
    pub safe: BTreeSet<Symbol>,
}

/// Finite prefix-plus-cycle certificate for one infinite fair execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FairLasso {
    /// Finite path before the repeating cycle. The final prefix state connects to
    /// the first cycle state. An empty prefix starts at the first cycle state.
    pub prefix: Vec<Symbol>,
    /// Nonempty repeating cycle. The final state must transition to the first.
    pub cycle: Vec<Symbol>,
    /// Each fairness set must occur in the repeating cycle.
    pub fairness_sets: Vec<BTreeSet<Symbol>>,
    /// A liveness/acceptance set that must occur in the repeating cycle.
    pub accepting: BTreeSet<Symbol>,
}

/// Rejection of a finite program-property certificate.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TransitionPropertyError {
    /// A transition mentions an undeclared state.
    #[error("transition graph contains a dangling state")]
    DanglingState,
    /// Certificate mentions a state absent from the transition graph.
    #[error("certificate references undeclared state {0:?}")]
    UnknownCertificateState(Symbol),
    /// Initial state is not in the invariant.
    #[error("safety invariant omits its initial state")]
    MissingInitial,
    /// Invariant contains an unsafe state.
    #[error("invariant state {0:?} is not safe")]
    UnsafeInvariantState(Symbol),
    /// Invariant is not closed under a transition.
    #[error("invariant is not closed under transition {from:?} -> {to:?}")]
    OpenInvariant { from: Symbol, to: Symbol },
    /// Infinite lasso has no cycle.
    #[error("fair lasso has an empty cycle")]
    EmptyCycle,
    /// Prefix/cycle contains an undeclared state or missing transition.
    #[error("fair lasso is not a path in the transition graph")]
    InvalidLassoPath,
    /// A declared fairness obligation is absent from the repeating cycle.
    #[error("fair lasso does not satisfy fairness set {0}")]
    Unfair(usize),
    /// The liveness acceptance set is absent from the repeating cycle.
    #[error("fair lasso never visits an accepting state")]
    NoAcceptingState,
}

impl TransitionGraph {
    /// Validates transition endpoints.
    pub fn validate(&self) -> Result<(), TransitionPropertyError> {
        if self
            .edges
            .iter()
            .any(|(from, to)| !self.states.contains(from) || !self.states.contains(to))
        {
            Err(TransitionPropertyError::DanglingState)
        } else {
            Ok(())
        }
    }
}

/// Checks a finite inductive safety invariant.
pub fn check_safety_invariant(
    graph: &TransitionGraph,
    certificate: &SafetyInvariant,
) -> Result<(), TransitionPropertyError> {
    graph.validate()?;
    if !graph.states.contains(&certificate.initial) {
        return Err(TransitionPropertyError::UnknownCertificateState(
            certificate.initial.clone(),
        ));
    }
    if !certificate.invariant.contains(&certificate.initial) {
        return Err(TransitionPropertyError::MissingInitial);
    }
    for state in &certificate.invariant {
        if !graph.states.contains(state) {
            return Err(TransitionPropertyError::UnknownCertificateState(
                state.clone(),
            ));
        }
        if !certificate.safe.contains(state) {
            return Err(TransitionPropertyError::UnsafeInvariantState(state.clone()));
        }
    }
    for (from, to) in &graph.edges {
        if certificate.invariant.contains(from) && !certificate.invariant.contains(to) {
            return Err(TransitionPropertyError::OpenInvariant {
                from: from.clone(),
                to: to.clone(),
            });
        }
    }
    Ok(())
}

/// Checks a finite fair lasso witnessing an infinite execution that revisits an
/// accepting state forever. This proves existence of that execution, not universal
/// liveness of every scheduler.
pub fn check_fair_lasso(
    graph: &TransitionGraph,
    certificate: &FairLasso,
) -> Result<(), TransitionPropertyError> {
    graph.validate()?;
    let Some(first_cycle) = certificate.cycle.first() else {
        return Err(TransitionPropertyError::EmptyCycle);
    };
    let mut path = certificate.prefix.clone();
    path.extend(certificate.cycle.iter().cloned());
    if path.iter().any(|state| !graph.states.contains(state)) {
        return Err(TransitionPropertyError::InvalidLassoPath);
    }
    if path
        .windows(2)
        .any(|edge| !graph.edges.contains(&(edge[0].clone(), edge[1].clone())))
    {
        return Err(TransitionPropertyError::InvalidLassoPath);
    }
    let last_cycle = certificate
        .cycle
        .last()
        .ok_or(TransitionPropertyError::EmptyCycle)?;
    if !graph
        .edges
        .contains(&(last_cycle.clone(), first_cycle.clone()))
    {
        return Err(TransitionPropertyError::InvalidLassoPath);
    }
    let cycle_states: BTreeSet<_> = certificate.cycle.iter().cloned().collect();
    for (index, fairness) in certificate.fairness_sets.iter().enumerate() {
        if cycle_states.is_disjoint(fairness) {
            return Err(TransitionPropertyError::Unfair(index));
        }
    }
    if cycle_states.is_disjoint(&certificate.accepting) {
        return Err(TransitionPropertyError::NoAcceptingState);
    }
    Ok(())
}

/// Search/evaluation policy. `Unrestricted` permits divergence but never changes
/// the requirement that accepted evidence itself is finite and checkable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryPolicy {
    /// The producer promises its discovery procedure terminates.
    GuaranteedTerminating,
    /// Search may diverge, be paused, or hit a resource bound.
    Unrestricted,
}

/// Finite Kripke frame with named accessibility relations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldFrame {
    /// Worlds.
    pub worlds: BTreeSet<Symbol>,
    /// Relation name to directed edges.
    pub relations: BTreeMap<Symbol, BTreeSet<(Symbol, Symbol)>>,
}

/// Atomic valuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Valuation {
    /// Proposition to worlds where it holds.
    pub true_at: BTreeMap<Symbol, BTreeSet<Symbol>>,
}

/// Modal and temporal formula. Temporal operators use a named successor relation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "formula", rename_all = "snake_case")]
pub enum Formula {
    /// Atomic proposition.
    Atom(Symbol),
    /// Negation.
    Not(Box<Formula>),
    /// Conjunction.
    And(Vec<Formula>),
    /// Disjunction.
    Or(Vec<Formula>),
    /// Universal accessibility modality.
    Box {
        relation: Symbol,
        body: Box<Formula>,
    },
    /// Existential accessibility modality.
    Diamond {
        relation: Symbol,
        body: Box<Formula>,
    },
    /// Formula holds at every state reachable through the named temporal relation.
    Always {
        relation: Symbol,
        body: Box<Formula>,
    },
    /// Formula holds at some state reachable through the named temporal relation.
    Eventually {
        relation: Symbol,
        body: Box<Formula>,
    },
}

/// Modal-model rejection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ModelError {
    /// Queried world is absent.
    #[error("unknown world {0:?}")]
    UnknownWorld(Symbol),
    /// A formula names a relation absent from the frame.
    #[error("unknown accessibility relation {0:?}")]
    UnknownRelation(Symbol),
    /// An edge references an absent world.
    #[error("relation {relation:?} references absent world {world:?}")]
    DanglingWorld { relation: Symbol, world: Symbol },
    /// A valuation assigns truth at an absent world.
    #[error("valuation for {proposition:?} references absent world {world:?}")]
    DanglingValuation { proposition: Symbol, world: Symbol },
}

impl WorldFrame {
    /// Checks all relation endpoints.
    pub fn validate(&self) -> Result<(), ModelError> {
        for (relation, edges) in &self.relations {
            for (from, to) in edges {
                for world in [from, to] {
                    if !self.worlds.contains(world) {
                        return Err(ModelError::DanglingWorld {
                            relation: relation.clone(),
                            world: world.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Checks all relation endpoints and valuation worlds.
    pub fn validate_model(&self, valuation: &Valuation) -> Result<(), ModelError> {
        self.validate()?;
        for (proposition, worlds) in &valuation.true_at {
            for world in worlds {
                if !self.worlds.contains(world) {
                    return Err(ModelError::DanglingValuation {
                        proposition: proposition.clone(),
                        world: world.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Evaluates a modal/temporal formula in a finite model.
    pub fn holds(
        &self,
        valuation: &Valuation,
        world: &Symbol,
        formula: &Formula,
    ) -> Result<bool, ModelError> {
        self.validate_model(valuation)?;
        if !self.worlds.contains(world) {
            return Err(ModelError::UnknownWorld(world.clone()));
        }
        match formula {
            Formula::Atom(proposition) => Ok(valuation
                .true_at
                .get(proposition)
                .is_some_and(|worlds| worlds.contains(world))),
            Formula::Not(body) => Ok(!self.holds(valuation, world, body)?),
            Formula::And(parts) => {
                for part in parts {
                    if !self.holds(valuation, world, part)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Formula::Or(parts) => {
                for part in parts {
                    if self.holds(valuation, world, part)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Formula::Box { relation, body } => {
                self.ensure_relation(relation)?;
                for successor in self.successors(relation, world) {
                    if !self.holds(valuation, successor, body)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Formula::Diamond { relation, body } => {
                self.ensure_relation(relation)?;
                for successor in self.successors(relation, world) {
                    if self.holds(valuation, successor, body)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Formula::Always { relation, body } => {
                self.ensure_relation(relation)?;
                for reachable in self.reachable(relation, world) {
                    if !self.holds(valuation, &reachable, body)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Formula::Eventually { relation, body } => {
                self.ensure_relation(relation)?;
                for reachable in self.reachable(relation, world) {
                    if self.holds(valuation, &reachable, body)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    fn ensure_relation(&self, relation: &Symbol) -> Result<(), ModelError> {
        self.relations
            .contains_key(relation)
            .then_some(())
            .ok_or_else(|| ModelError::UnknownRelation(relation.clone()))
    }

    fn successors<'a>(&'a self, relation: &Symbol, world: &'a Symbol) -> Vec<&'a Symbol> {
        self.relations
            .get(relation)
            .into_iter()
            .flatten()
            .filter_map(|(from, to)| (from == world).then_some(to))
            .collect()
    }

    fn reachable(&self, relation: &Symbol, initial: &Symbol) -> BTreeSet<Symbol> {
        let mut seen = BTreeSet::from([initial.clone()]);
        let mut pending = VecDeque::from([initial.clone()]);
        while let Some(world) = pending.pop_front() {
            for successor in self.successors(relation, &world) {
                if seen.insert(successor.clone()) {
                    pending.push_back(successor.clone());
                }
            }
        }
        seen
    }
}

/// Finite presentation of a stream-like coalgebra.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coalgebra {
    /// Observable output at each state.
    pub output: BTreeMap<Symbol, Object>,
    /// Next state.
    pub next: BTreeMap<Symbol, Symbol>,
}

/// Finite productivity witness for an infinite generated behavior.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductivityCertificate {
    /// Initial state.
    pub initial: Symbol,
    /// Finite invariant set closed under `next` and carrying outputs.
    pub invariant: BTreeSet<Symbol>,
}

/// Finite bisimulation witness between two coalgebras.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bisimulation {
    /// Initial states whose infinite observations are being compared.
    pub initial: (Symbol, Symbol),
    /// Related state pairs.
    pub pairs: BTreeSet<(Symbol, Symbol)>,
}

/// Coinductive certificate rejection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum CoinductionError {
    /// Initial state is outside the invariant.
    #[error("productivity invariant omits the initial state")]
    MissingInitial,
    /// State has no output.
    #[error("state {0:?} has no output")]
    MissingOutput(Symbol),
    /// State has no successor.
    #[error("state {0:?} has no successor")]
    MissingSuccessor(Symbol),
    /// Successor leaves the finite invariant.
    #[error("successor {successor:?} of {state:?} leaves the productivity invariant")]
    OpenInvariant { state: Symbol, successor: Symbol },
    /// Distinguished initial pair is absent from the relation.
    #[error("bisimulation relation omits its initial pair")]
    MissingBisimulationInitial,
    /// Bisimulation pair has unequal outputs.
    #[error("bisimulation output mismatch at {0:?}, {1:?}")]
    OutputMismatch(Symbol, Symbol),
    /// Successor pair is absent.
    #[error("bisimulation is not closed after {0:?}, {1:?}")]
    BisimulationNotClosed(Symbol, Symbol),
}

/// Checks a finite productivity invariant, certifying arbitrarily long observation.
pub fn check_productivity(
    coalgebra: &Coalgebra,
    certificate: &ProductivityCertificate,
) -> Result<(), CoinductionError> {
    if !certificate.invariant.contains(&certificate.initial) {
        return Err(CoinductionError::MissingInitial);
    }
    for state in &certificate.invariant {
        if !coalgebra.output.contains_key(state) {
            return Err(CoinductionError::MissingOutput(state.clone()));
        }
        let successor = coalgebra
            .next
            .get(state)
            .ok_or_else(|| CoinductionError::MissingSuccessor(state.clone()))?;
        if !certificate.invariant.contains(successor) {
            return Err(CoinductionError::OpenInvariant {
                state: state.clone(),
                successor: successor.clone(),
            });
        }
    }
    Ok(())
}

/// Checks finite bisimulation closure, certifying equality of infinite observations.
pub fn check_bisimulation(
    left: &Coalgebra,
    right: &Coalgebra,
    bisimulation: &Bisimulation,
) -> Result<(), CoinductionError> {
    if !bisimulation.pairs.contains(&bisimulation.initial) {
        return Err(CoinductionError::MissingBisimulationInitial);
    }
    for (left_state, right_state) in &bisimulation.pairs {
        if left.output.get(left_state) != right.output.get(right_state) {
            return Err(CoinductionError::OutputMismatch(
                left_state.clone(),
                right_state.clone(),
            ));
        }
        let left_next = left
            .next
            .get(left_state)
            .ok_or_else(|| CoinductionError::MissingSuccessor(left_state.clone()))?;
        let right_next = right
            .next
            .get(right_state)
            .ok_or_else(|| CoinductionError::MissingSuccessor(right_state.clone()))?;
        if !bisimulation
            .pairs
            .contains(&(left_next.clone(), right_next.clone()))
        {
            return Err(CoinductionError::BisimulationNotClosed(
                left_state.clone(),
                right_state.clone(),
            ));
        }
    }
    Ok(())
}

/// Identity of a clock or time scale.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClockId(pub Symbol);

/// Instant on one explicit clock.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Instant {
    /// Clock identity.
    pub clock: ClockId,
    /// Signed ticks in that clock's declared scale.
    pub ticks: i128,
}

/// Closed temporal interval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    /// Inclusive start.
    pub start: Instant,
    /// Inclusive end.
    pub end: Instant,
}

impl Interval {
    /// Checks clock agreement and temporal ordering.
    #[must_use]
    pub fn well_formed(&self) -> bool {
        self.start.clock == self.end.clock && self.start.ticks <= self.end.ticks
    }
}

/// Epistemic status reported by the harness. This is data, not kernel truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EpistemicStatus {
    /// Harness attests certainty at the specified observation time.
    CertainAtObservation,
    /// Revisable assessment, represented in millionths.
    Revisable { confidence_ppm: u32 },
}

/// Immutable observation event. Revision creates a new event naming `supersedes`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    /// Stable observation identity.
    pub id: Symbol,
    /// Arbitrary represented claim.
    pub proposition: Object,
    /// Adapter, person, model, sensor, or authority.
    pub source: Symbol,
    /// Time at which the source issued the observation.
    pub observed_at: Instant,
    /// Time for which the represented claim applies.
    pub valid_during: Option<Interval>,
    /// Reported epistemic status.
    pub status: EpistemicStatus,
    /// Earlier immutable observation revised by this event.
    pub supersedes: Option<Symbol>,
    /// Opaque provenance object retained for higher layers.
    pub provenance: BTreeMap<Symbol, Object>,
}

/// Observation-ledger rejection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ObservationError {
    /// Duplicate immutable event identity.
    #[error("duplicate observation {0:?}")]
    Duplicate(Symbol),
    /// Invalid interval.
    #[error("observation {0:?} has a malformed validity interval")]
    MalformedInterval(Symbol),
    /// Confidence exceeds one million parts per million.
    #[error("observation {0:?} has invalid confidence")]
    InvalidConfidence(Symbol),
    /// Supersession target is absent or not earlier in the ledger.
    #[error("observation {observation:?} supersedes unknown/non-prior event {target:?}")]
    InvalidSupersession { observation: Symbol, target: Symbol },
}

/// Validates immutable temporal observations and acyclic prior-event supersession.
pub fn check_observation_ledger(observations: &[Observation]) -> Result<(), ObservationError> {
    let mut seen = BTreeSet::new();
    for observation in observations {
        if !seen.insert(observation.id.clone()) {
            return Err(ObservationError::Duplicate(observation.id.clone()));
        }
        if observation
            .valid_during
            .as_ref()
            .is_some_and(|interval| !interval.well_formed())
        {
            return Err(ObservationError::MalformedInterval(observation.id.clone()));
        }
        if matches!(
            observation.status,
            EpistemicStatus::Revisable { confidence_ppm } if confidence_ppm > 1_000_000
        ) {
            return Err(ObservationError::InvalidConfidence(observation.id.clone()));
        }
        if let Some(target) = &observation.supersedes {
            if !seen.contains(target) || target == &observation.id {
                return Err(ObservationError::InvalidSupersession {
                    observation: observation.id.clone(),
                    target: target.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Explicit selection of an observation as a premise for a deduction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationAssumption {
    /// Observation event selected by policy above the kernel.
    pub observation: Symbol,
    /// Exact open-graph proposition before elaboration.
    pub represented_proposition: Object,
    /// Exact core proposition inserted into the certificate context.
    pub elaborated_proposition: Term,
    /// Index into the certificate context, ordered oldest to newest.
    pub context_index: usize,
    /// Selection time, distinct from observation validity time.
    pub selected_at: Instant,
}

/// Complete successful formal-answer envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormalAnswer {
    /// Query witnesses, when the goal had existential answer positions.
    pub witnesses: Vec<Term>,
    /// Exact accepted judgment.
    pub certificate: Certificate,
    /// Complete transitive declaration and local-assumption disclosure.
    pub dependencies: DependencyReport,
    /// External observations explicitly linked to local context entries.
    pub observation_assumptions: Vec<ObservationAssumption>,
    /// Optional identity/version of the untrusted discovery procedure.
    pub procedure: Option<Symbol>,
}

/// Formal-answer envelope rejection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum FormalAnswerError {
    /// The underlying certificate was rejected.
    #[error("formal-answer certificate is invalid: {0}")]
    InvalidCertificate(KernelError),
    /// Kernel returned an impossible success shape.
    #[error("kernel returned an unexpected result while checking a formal answer")]
    UnexpectedKernelResult,
    /// Stored dependency disclosure differs from the exact certificate closure.
    #[error("formal-answer dependency report does not match the certificate")]
    DependencyMismatch,
    /// Observation points outside the certificate context.
    #[error("observation {observation:?} points to absent context entry {index}")]
    MissingContextEntry { observation: Symbol, index: usize },
    /// Elaborated observation proposition differs from the linked context entry.
    #[error("observation {observation:?} does not match context entry {index}")]
    ContextEntryMismatch { observation: Symbol, index: usize },
}

impl FormalAnswer {
    /// Verifies the certificate, exact dependency closure, and every observation-to-context link.
    pub fn verify(&self, theory: &Theory) -> Result<(), FormalAnswerError> {
        match Kernel::run_to_completion(
            theory.clone(),
            KernelRequest::Certificate {
                certificate: self.certificate.clone(),
            },
        ) {
            Ok(KernelResult::Certified) => {}
            Ok(_) => return Err(FormalAnswerError::UnexpectedKernelResult),
            Err(error) => return Err(FormalAnswerError::InvalidCertificate(error)),
        }
        if self.dependencies != self.certificate.dependencies(theory) {
            return Err(FormalAnswerError::DependencyMismatch);
        }
        for assumption in &self.observation_assumptions {
            let Some(context_entry) = self.certificate.context.0.get(assumption.context_index)
            else {
                return Err(FormalAnswerError::MissingContextEntry {
                    observation: assumption.observation.clone(),
                    index: assumption.context_index,
                });
            };
            if context_entry != &assumption.elaborated_proposition {
                return Err(FormalAnswerError::ContextEntryMismatch {
                    observation: assumption.observation.clone(),
                    index: assumption.context_index,
                });
            }
        }
        Ok(())
    }
}

/// Honest outcome of potentially nonterminating proof discovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum DiscoveryStatus<C> {
    /// Proof/certificate found.
    Proved(C),
    /// Counter-certificate found in a logic that defines one.
    Disproved(C),
    /// Search still has a continuation.
    Running(C),
    /// Deliberately paused with a continuation.
    Paused(C),
    /// Cancelled without settling the goal.
    Cancelled,
    /// Resource bound reached with a continuation.
    ResourceLimitReached(C),
    /// A finite declared search space was exhausted.
    SearchExhausted,
    /// No conclusion.
    Unknown,
}

/// Serializable package of kernel theory plus untrusted extension machinery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TheoryPackage {
    /// Canonical package schema version.
    #[serde(default = "default_package_format")]
    pub format_version: u32,
    /// Stable package identity.
    pub name: Symbol,
    /// Human version; `kernel.id()` remains content-authoritative.
    pub version: String,
    /// Checked declarations and inductives.
    pub kernel: Theory,
    /// Reflected object logics.
    pub proof_systems: Vec<ReflectedProofSystem>,
    /// Rewrite systems, grouped and named.
    pub rewrite_systems: BTreeMap<Symbol, Vec<RewriteRule>>,
    /// Program transition systems.
    pub transition_systems: BTreeMap<Symbol, Vec<TransitionRule>>,
    /// Discovery policy for named untrusted procedures.
    pub discovery_policies: BTreeMap<Symbol, DiscoveryPolicy>,
    /// Complete package imports, elaborators, certificate systems, translations, and policies.
    #[serde(default)]
    pub extensions: PackageExtensions,
    /// Arbitrary package metadata with no logical force.
    pub metadata: BTreeMap<Symbol, Object>,
}

fn default_package_format() -> u32 {
    THEORY_PACKAGE_FORMAT
}

/// Builds a representative classical logic package entirely above the fixed kernel.
///
/// `Empty` and binary `Sum` are native inductive families. Excluded middle is an
/// explicit axiom, so every dependent certificate discloses its classical assumption.
pub fn classical_logic_theory() -> Result<Theory, TheoryError> {
    let mut builder = TheoryBuilder::new("artist.logic.classical", "1");
    builder.inductive(InductiveDeclaration {
        name: "Empty".into(),
        parameters: vec![],
        indices: vec![],
        universe: 0,
        constructors: vec![],
        provenance: Provenance::new("artist-theories/classical"),
    })?;
    builder.inductive(InductiveDeclaration {
        name: "Sum".into(),
        parameters: vec![Term::universe(0), Term::universe(0)],
        indices: vec![],
        universe: 0,
        constructors: vec![
            InductiveConstructor {
                name: "Sum.inl".into(),
                fields: vec![ConstructorField::Plain { ty: Term::var(1) }],
                result_indices: vec![],
            },
            InductiveConstructor {
                name: "Sum.inr".into(),
                fields: vec![ConstructorField::Plain { ty: Term::var(0) }],
                result_indices: vec![],
            },
        ],
        provenance: Provenance::new("artist-theories/classical"),
    })?;
    let proposition = Term::var(0);
    let negation = Term::pi(proposition.clone(), Term::constant("Empty"));
    let excluded_middle_result = Term::apply_many(Term::constant("Sum"), [proposition, negation]);
    builder.axiom(
        "classical.excluded_middle",
        Term::pi(Term::universe(0), excluded_middle_result),
        Provenance::new("explicit classical axiom"),
    )?;
    Ok(builder.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(value: &str) -> Symbol {
        Symbol::from(value)
    }

    #[test]
    fn reflected_modus_ponens_checks_a_finite_proof_tree() {
        let p = Object::atom("P");
        let q = Object::atom("Q");
        let implication = Object::node("implies", vec![p.clone(), q.clone()]);
        let system = ReflectedProofSystem {
            name: symbol("propositional"),
            rules: vec![RuleSchema {
                name: symbol("modus_ponens"),
                premises: vec![
                    Pattern::node("implies", vec![Pattern::var("p"), Pattern::var("q")]),
                    Pattern::var("p"),
                ],
                conclusion: Pattern::var("q"),
            }],
        };
        let assumptions = BTreeSet::from([implication.clone(), p.clone()]);
        let proof = ProofNode {
            conclusion: q,
            rule: Some(symbol("modus_ponens")),
            premises: vec![
                ProofNode {
                    conclusion: implication,
                    rule: None,
                    premises: vec![],
                },
                ProofNode {
                    conclusion: p,
                    rule: None,
                    premises: vec![],
                },
            ],
        };
        assert_eq!(system.check(&assumptions, &proof), Ok(()));
    }

    #[test]
    fn arbitrary_rewrite_trace_checks_at_a_subterm_path() {
        let x = Pattern::var("x");
        let rules = vec![RewriteRule {
            name: symbol("double_negation"),
            lhs: Pattern::node("not", vec![Pattern::node("not", vec![x.clone()])]),
            rhs: x,
        }];
        let before = Object::node(
            "and",
            vec![
                Object::node("not", vec![Object::node("not", vec![Object::atom("P")])]),
                Object::atom("Q"),
            ],
        );
        let after = Object::node("and", vec![Object::atom("P"), Object::atom("Q")]);
        let trace = RewriteTrace {
            start: before.clone(),
            steps: vec![RewriteStep {
                rule: symbol("double_negation"),
                path: vec![0],
                before,
                after: after.clone(),
            }],
            finish: after,
        };
        assert_eq!(check_rewrite_trace(&rules, &trace), Ok(()));
    }

    #[test]
    fn program_trace_is_checked_without_running_search() {
        let rules = vec![TransitionRule {
            label: symbol("increment"),
            before: Pattern::node("counter", vec![Pattern::var("n")]),
            after: Pattern::node(
                "counter",
                vec![Pattern::node("succ", vec![Pattern::var("n")])],
            ),
        }];
        let initial = Object::node("counter", vec![Object::atom("zero")]);
        let final_state = Object::node(
            "counter",
            vec![Object::node("succ", vec![Object::atom("zero")])],
        );
        let trace = ExecutionTrace {
            initial: initial.clone(),
            steps: vec![ExecutionStep {
                label: symbol("increment"),
                before: initial,
                after: final_state.clone(),
            }],
            final_state,
        };
        assert_eq!(check_execution(&rules, &trace), Ok(()));
    }

    #[test]
    fn modal_and_temporal_semantics_are_library_level() {
        let w0 = symbol("w0");
        let w1 = symbol("w1");
        let next = symbol("next");
        let frame = WorldFrame {
            worlds: BTreeSet::from([w0.clone(), w1.clone()]),
            relations: BTreeMap::from([(
                next.clone(),
                BTreeSet::from([(w0.clone(), w1.clone()), (w1.clone(), w1.clone())]),
            )]),
        };
        let valuation = Valuation {
            true_at: BTreeMap::from([(symbol("ready"), BTreeSet::from([w1.clone()]))]),
        };
        let eventually_ready = Formula::Eventually {
            relation: next,
            body: Box::new(Formula::Atom(symbol("ready"))),
        };
        assert_eq!(frame.holds(&valuation, &w0, &eventually_ready), Ok(true));
    }

    #[test]
    fn modal_model_rejects_unknown_relations_and_dangling_valuations() {
        let world = symbol("w");
        let frame = WorldFrame {
            worlds: BTreeSet::from([world.clone()]),
            relations: BTreeMap::new(),
        };
        let empty = Valuation {
            true_at: BTreeMap::new(),
        };
        let formula = Formula::Box {
            relation: symbol("missing"),
            body: Box::new(Formula::Atom(symbol("p"))),
        };
        assert_eq!(
            frame.holds(&empty, &world, &formula),
            Err(ModelError::UnknownRelation(symbol("missing")))
        );

        let dangling = Valuation {
            true_at: BTreeMap::from([(symbol("p"), BTreeSet::from([symbol("ghost")]))]),
        };
        assert_eq!(
            frame.holds(&dangling, &world, &Formula::Atom(symbol("p"))),
            Err(ModelError::DanglingValuation {
                proposition: symbol("p"),
                world: symbol("ghost"),
            })
        );
    }

    #[test]
    fn finite_productivity_and_bisimulation_certify_infinite_behavior() {
        let left = symbol("left");
        let right = symbol("right");
        let output = Object::atom("tick");
        let left_coalgebra = Coalgebra {
            output: BTreeMap::from([(left.clone(), output.clone())]),
            next: BTreeMap::from([(left.clone(), left.clone())]),
        };
        let right_coalgebra = Coalgebra {
            output: BTreeMap::from([(right.clone(), output)]),
            next: BTreeMap::from([(right.clone(), right.clone())]),
        };
        assert_eq!(
            check_productivity(
                &left_coalgebra,
                &ProductivityCertificate {
                    initial: left.clone(),
                    invariant: BTreeSet::from([left.clone()]),
                },
            ),
            Ok(())
        );
        assert_eq!(
            check_bisimulation(
                &left_coalgebra,
                &right_coalgebra,
                &Bisimulation {
                    initial: (left.clone(), right.clone()),
                    pairs: BTreeSet::from([(left, right)]),
                },
            ),
            Ok(())
        );
    }

    #[test]
    fn safety_and_fair_liveness_have_finite_certificates() {
        let idle = symbol("idle");
        let work = symbol("work");
        let graph = TransitionGraph {
            states: BTreeSet::from([idle.clone(), work.clone()]),
            edges: BTreeSet::from([(idle.clone(), work.clone()), (work.clone(), idle.clone())]),
        };
        assert_eq!(
            check_safety_invariant(
                &graph,
                &SafetyInvariant {
                    initial: idle.clone(),
                    invariant: BTreeSet::from([idle.clone(), work.clone()]),
                    safe: BTreeSet::from([idle.clone(), work.clone()]),
                },
            ),
            Ok(())
        );
        assert_eq!(
            check_fair_lasso(
                &graph,
                &FairLasso {
                    prefix: vec![],
                    cycle: vec![idle.clone(), work.clone()],
                    fairness_sets: vec![BTreeSet::from([work.clone()])],
                    accepting: BTreeSet::from([work]),
                },
            ),
            Ok(())
        );
    }

    #[test]
    fn certificates_cannot_be_vacuous_or_name_undeclared_states() {
        let state = symbol("s");
        let ghost = symbol("ghost");
        let graph = TransitionGraph {
            states: BTreeSet::from([state.clone()]),
            edges: BTreeSet::from([(state.clone(), state.clone())]),
        };
        assert_eq!(
            check_safety_invariant(
                &graph,
                &SafetyInvariant {
                    initial: state.clone(),
                    invariant: BTreeSet::from([state.clone(), ghost.clone()]),
                    safe: BTreeSet::from([state.clone(), ghost.clone()]),
                },
            ),
            Err(TransitionPropertyError::UnknownCertificateState(ghost))
        );

        let coalgebra = Coalgebra {
            output: BTreeMap::from([(state.clone(), Object::atom("tick"))]),
            next: BTreeMap::from([(state.clone(), state.clone())]),
        };
        assert_eq!(
            check_bisimulation(
                &coalgebra,
                &coalgebra,
                &Bisimulation {
                    initial: (state.clone(), state),
                    pairs: BTreeSet::new(),
                },
            ),
            Err(CoinductionError::MissingBisimulationInitial)
        );
    }

    #[test]
    fn observations_are_temporal_immutable_and_revisable() {
        let clock = ClockId(symbol("monotonic"));
        let first = Observation {
            id: symbol("o1"),
            proposition: Object::node("exists", vec![Object::atom("/tmp/x")]),
            source: symbol("filesystem"),
            observed_at: Instant {
                clock: clock.clone(),
                ticks: 10,
            },
            valid_during: Some(Interval {
                start: Instant {
                    clock: clock.clone(),
                    ticks: 10,
                },
                end: Instant {
                    clock: clock.clone(),
                    ticks: 10,
                },
            }),
            status: EpistemicStatus::CertainAtObservation,
            supersedes: None,
            provenance: BTreeMap::new(),
        };
        let revision = Observation {
            id: symbol("o2"),
            proposition: Object::node("missing", vec![Object::atom("/tmp/x")]),
            source: symbol("filesystem"),
            observed_at: Instant { clock, ticks: 20 },
            valid_during: None,
            status: EpistemicStatus::Revisable {
                confidence_ppm: 900_000,
            },
            supersedes: Some(symbol("o1")),
            provenance: BTreeMap::new(),
        };
        assert_eq!(check_observation_ledger(&[first, revision]), Ok(()));
    }

    #[test]
    fn formal_answer_links_observation_provenance_to_exact_context() {
        let mut builder = TheoryBuilder::new("answers", "1");
        builder
            .axiom("A", Term::universe(0), Provenance::new("fixture"))
            .unwrap();
        let theory = builder.finish();
        let certificate = Certificate::new(
            &theory,
            artist_kernel::Context(vec![Term::constant("A")]),
            Term::constant("A"),
            Term::var(0),
        );
        let answer = FormalAnswer {
            witnesses: vec![],
            dependencies: certificate.dependencies(&theory),
            certificate,
            observation_assumptions: vec![ObservationAssumption {
                observation: symbol("o1"),
                represented_proposition: Object::atom("A"),
                elaborated_proposition: Term::constant("A"),
                context_index: 0,
                selected_at: Instant {
                    clock: ClockId(symbol("utc")),
                    ticks: 10,
                },
            }],
            procedure: Some(symbol("proof-search-v1")),
        };
        assert_eq!(answer.verify(&theory), Ok(()));
    }

    #[test]
    fn classical_logic_is_an_explicit_theory_package() {
        let theory = classical_logic_theory().unwrap();
        assert!(theory.declaration(&"Empty".into()).is_some());
        assert!(theory.declaration(&"Sum".into()).is_some());
        assert!(
            theory
                .declaration(&"classical.excluded_middle".into())
                .is_some()
        );
    }
}
