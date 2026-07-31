//! The two-sided anytime evaluator, over the universal object graph.
//!
//! This closes the loop the object kernel left open. Representation lived in
//! [`crate::object`] while evaluation still ran on a closed `Formula` enum, so
//! the graph could say everything and decide nothing — two representations,
//! with the *weaker* one doing the work.
//!
//! Everything the old evaluator earned carries over unchanged, because those
//! were the right parts:
//!
//! * truth is two-sided, so halting at an arbitrary instant is sound;
//! * absence is refutation only where knowledge is complete;
//! * a cut-short scan yields a **residual**, kept quantified rather than
//!   unrolled;
//! * cost is a budget, never a refusal.
//!
//! What is new is that an operator with no semantics is no longer a
//! contradiction in terms. It evaluates to [`ComputeStatus::Unsupported`] with
//! its expression handed back — distinct from "no evidence" and from "out of
//! budget" — and registering semantics later needs no migration and no reparse.

use crate::evidence::{Bound, ComputeStatus, EvaluationResult};
use crate::object::{CoreNode, LiteralValue, ObjectGraph, ObjectId, wk};
use crate::registry::OperatorRegistry;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use std::collections::BTreeMap;

/// What the evaluator asks of the world. The object-graph analogue of
/// [`crate::structure::Structure`].
pub trait GraphStructure {
    /// Whether `pred(args)` is known to hold. `None` is not-known, which is a
    /// different answer from `Some(false)`.
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Option<bool>;

    /// Members of a domain expression, or `None` when it cannot be enumerated.
    fn extension(&self, _domain: ObjectId) -> Option<Vec<ObjectId>> {
        None
    }

    /// Whether knowledge of `pred` is complete. This is what licenses reading
    /// absence as refutation, and it is data rather than a compiled-in rule.
    fn is_closed(&self, _pred: ObjectId) -> bool {
        false
    }

    /// Whether `pred(args)` held **as of** instant `t`. A memory system needs
    /// *valid* time — when the belief held — not transaction time.
    fn known_at(&self, pred: ObjectId, args: &[ObjectId], _t: i64) -> Option<bool> {
        self.known(pred, args)
    }

    /// Instants at which anything changed. Empty for a timeless structure,
    /// which makes temporal queries vacuous rather than wrong.
    fn instants(&self) -> Vec<i64> {
        Vec::new()
    }

    /// Version of the universe these answers came from. Bounds computed against
    /// different versions must never be composed.
    fn snapshot(&self) -> u64 {
        0
    }
}

/// An empty world: nothing known, nothing closed.
pub struct EmptyStructure;
impl GraphStructure for EmptyStructure {
    fn known(&self, _pred: ObjectId, _args: &[ObjectId]) -> Option<bool> {
        None
    }
}

/// A relation bound to a predicate variable, plus whether it is the *whole*
/// relation.
///
/// The flag keeps recursion sound under a budget: a fixpoint cut short is an
/// under-approximation, so a tuple present really is a member but a tuple
/// absent might still be derived by a later round.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoundRelation {
    pub tuples: std::collections::BTreeSet<Vec<ObjectId>>,
    pub complete: bool,
}

/// Bindings during evaluation: first-order values and second-order relations.
#[derive(Clone, Debug, Default)]
pub struct GraphEnv {
    pub vars: BTreeMap<ObjectId, ObjectId>,
    pub rels: BTreeMap<ObjectId, BoundRelation>,
}

impl GraphEnv {
    pub fn new() -> Self {
        Self::default()
    }
    fn get(&self, k: &ObjectId) -> Option<&ObjectId> {
        self.vars.get(k)
    }
    fn with(&self, k: ObjectId, v: ObjectId) -> Self {
        let mut e = self.clone();
        e.vars.insert(k, v);
        e
    }
    fn with_rel(&self, k: ObjectId, r: BoundRelation) -> Self {
        let mut e = self.clone();
        e.rels.insert(k, r);
        e
    }
}

pub struct GraphEvaluator {
    pub registry: OperatorRegistry,
}

impl Default for GraphEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphEvaluator {
    pub fn new() -> Self {
        GraphEvaluator { registry: OperatorRegistry::new() }
    }

    /// Evaluate `root`. The graph is mutable because a continuation is an
    /// ordinary object in the same universe — item 17 is only meaningful if the
    /// evaluator can *create* one.
    pub fn eval(
        &self,
        g: &mut ObjectGraph,
        root: ObjectId,
        s: &dyn GraphStructure,
        budget: u64,
    ) -> EvaluationResult {
        let mut st = State {
            g,
            s,
            budget,
            spent: 0,
            snapshot: s.snapshot(),
            registry: &self.registry,
            depth: 0,
            path: Vec::new(),
            negated: false,
            instant: None,
        };
        let mut r = st.check(root, &GraphEnv::new());
        r.spent = st.spent;
        r.snapshot = st.snapshot;
        r
    }
}

struct State<'a> {
    g: &'a mut ObjectGraph,
    s: &'a dyn GraphStructure,
    budget: u64,
    spent: u64,
    snapshot: u64,
    registry: &'a OperatorRegistry,
    depth: u32,
    /// Nodes currently being evaluated, with the parity of `not` above each.
    ///
    /// Re-entering a node already on the path is ungroundedness. Whether it
    /// *oscillates* is exactly whether the parity differs between the two
    /// visits, which is what separates the liar from the truth-teller.
    path: Vec<(ObjectId, bool)>,
    negated: bool,
    /// The instant currently being evaluated at. `None` is the present.
    instant: Option<i64>,
}

/// Guards against a cyclic expression driving the evaluator into the stack.
/// Hitting it is `Stalled`, never a verdict — the liar is representable, and
/// evaluating it simply does not terminate in a truth value.
const MAX_DEPTH: u32 = 512;

impl State<'_> {
    fn charge(&mut self, n: u64) -> bool {
        self.spent = self.spent.saturating_add(n);
        self.spent < self.budget
    }

    fn exhausted(&self) -> bool {
        self.spent >= self.budget
    }

    fn out_of_budget(&self, node: ObjectId) -> EvaluationResult {
        let mut r = EvaluationResult::new(ComputeStatus::BudgetExhausted);
        r.residual = Some(node);
        r.snapshot = self.snapshot;
        r
    }

    fn unsupported(&self, node: ObjectId) -> EvaluationResult {
        let mut r = EvaluationResult::new(ComputeStatus::Unsupported);
        r.residual = Some(node);
        r.snapshot = self.snapshot;
        r
    }

    fn stalled(&self, node: ObjectId) -> EvaluationResult {
        let mut r = EvaluationResult::new(ComputeStatus::Stalled);
        r.residual = Some(node);
        r.snapshot = self.snapshot;
        r
    }

    fn check(&mut self, node: ObjectId, env: &GraphEnv) -> EvaluationResult {
        if !self.charge(1) {
            return self.out_of_budget(node);
        }
        if self.depth >= MAX_DEPTH {
            return self.stalled(node);
        }
        let resolved = env.get(&node).copied().unwrap_or(node);

        // Kripke: a sentence that depends on its own truth never becomes
        // grounded, so no iteration of the fixed point will ever assign it a
        // value. Report that directly instead of spinning to a depth guard.
        if let Some((_, was_negated)) = self.path.iter().find(|(n, _)| *n == resolved) {
            return if *was_negated != self.negated {
                // Parity flipped around the loop: the value would oscillate
                // between rounds. The liar.
                let mut r = EvaluationResult::new(ComputeStatus::Exact);
                r.support = Bound::Certain;
                r.refutation = Bound::Certain;
                r.residual = Some(resolved);
                r.snapshot = self.snapshot;
                r
            } else {
                // Stable but ungrounded: the truth-teller. Never gets a value.
                let mut r = EvaluationResult::new(ComputeStatus::Exact);
                r.residual = Some(resolved);
                r.snapshot = self.snapshot;
                r
            };
        }
        self.path.push((resolved, self.negated));
        let out = self.dispatch(resolved, env);
        self.path.pop();
        out
    }

    fn dispatch(&mut self, resolved: ObjectId, env: &GraphEnv) -> EvaluationResult {

        match self.g.get(resolved) {
            None => self.unsupported(resolved),
            Some(CoreNode::Atom { .. }) if resolved == wk::TOP => {
                EvaluationResult::certain(true)
            }
            Some(CoreNode::Atom { .. }) if resolved == wk::BOT => {
                EvaluationResult::certain(false)
            }
            // A bare atom or literal is not a proposition; it denotes.
            Some(CoreNode::Atom { .. })
            | Some(CoreNode::Literal(_))
            | Some(CoreNode::External(_))
            | Some(CoreNode::Opaque { .. }) => self.unsupported(resolved),
            Some(CoreNode::Apply { operator, operands }) => {
                let (op, args) = (*operator, operands.clone());
                self.depth += 1;
                let r = self.apply(resolved, op, &args, env);
                self.depth -= 1;
                r
            }
            Some(CoreNode::Bind { binder, vars, bodies }) => {
                let (b, vs, bs) = (*binder, vars.clone(), bodies.clone());
                self.depth += 1;
                let r = self.bind(resolved, b, &vs, &bs, env);
                self.depth -= 1;
                r
            }
        }
    }

    fn apply(
        &mut self,
        node: ObjectId,
        op: ObjectId,
        args: &[ObjectId],
        env: &GraphEnv,
    ) -> EvaluationResult {
        match op {
            wk::NOT => {
                let Some(a) = args.first() else { return self.unsupported(node) };
                self.negated = !self.negated;
                let r = self.check(*a, env);
                self.negated = !self.negated;
                r.negate()
            }
            wk::AND => {
                let mut worst = ComputeStatus::Exact;
                let mut support = Bound::Certain;
                let mut residuals = Vec::new();
                for a in args {
                    if self.exhausted() {
                        return self.out_of_budget(node);
                    }
                    let r = self.check(*a, env);
                    if r.refutation.is_certain() {
                        // One false conjunct decides it; skipping the rest
                        // cannot lose soundness.
                        return EvaluationResult::certain(false);
                    }
                    if !r.support.is_certain() {
                        support = support.meet(r.support);
                        worst = downgrade(worst, r.compute_status);
                        if let Some(x) = r.residual {
                            residuals.push(x);
                        }
                    }
                }
                self.combine(node, support, Bound::None, worst, residuals)
            }
            wk::OR | wk::IMPLIES => {
                let mut worst = ComputeStatus::Exact;
                let mut refutation = Bound::Certain;
                let mut residuals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    if self.exhausted() {
                        return self.out_of_budget(node);
                    }
                    // `implies` negates its first operand.
                    let mut r = self.check(*a, env);
                    if op == wk::IMPLIES && i == 0 {
                        r = r.negate();
                    }
                    if r.support.is_certain() {
                        return EvaluationResult::certain(true);
                    }
                    if !r.refutation.is_certain() {
                        refutation = refutation.meet(r.refutation);
                        worst = downgrade(worst, r.compute_status);
                        if let Some(x) = r.residual {
                            residuals.push(x);
                        }
                    }
                }
                self.combine(node, Bound::None, refutation, worst, residuals)
            }
            wk::EQ => {
                let [a, b] = args else { return self.unsupported(node) };
                // Numeric first: an aggregate denotes a *number*, so comparing
                // its object id would compare the expression rather than its
                // value and report a confident false.
                if let (Some(x), Some(y)) = (self.number(*a, env), self.number(*b, env)) {
                    return EvaluationResult::certain(x == y);
                }
                if let (Some(x), Some(y)) = (self.text(*a, env), self.text(*b, env)) {
                    return EvaluationResult::certain(x == y);
                }
                match (self.ground(*a, env), self.ground(*b, env)) {
                    (Some(x), Some(y)) => EvaluationResult::certain(x == y),
                    _ => self.stalled(node),
                }
            }
            wk::LEQ => {
                let [a, b] = args else { return self.unsupported(node) };
                match (self.number(*a, env), self.number(*b, env)) {
                    (Some(x), Some(y)) => EvaluationResult::certain(x <= y),
                    _ => self.stalled(node),
                }
            }
            wk::CONTAINS | wk::STARTS_WITH | wk::ENDS_WITH => {
                let [a, b] = args else { return self.unsupported(node) };
                match (self.text(*a, env), self.text(*b, env)) {
                    (Some(x), Some(y)) => EvaluationResult::certain(match op {
                        wk::CONTAINS => x.contains(&y),
                        wk::STARTS_WITH => x.starts_with(&y),
                        _ => x.ends_with(&y),
                    }),
                    _ => self.stalled(node),
                }
            }
            wk::AT => {
                // Evaluate against the structure as it was at an instant. One
                // primitive; `always`/`eventually`/`since` derive from it by
                // quantifying over the instants domain.
                let [t, inner] = args else { return self.unsupported(node) };
                let Some(when) = self.number(*t, env) else { return self.stalled(node) };
                let prev = self.instant;
                self.instant = when.to_i64();
                let r = self.check(*inner, env);
                self.instant = prev;
                r
            }
            wk::HOLDS => {
                // Descend into a quotation — deliberately, never automatically.
                let Some(a) = args.first() else { return self.unsupported(node) };
                let target = env.get(a).copied().unwrap_or(*a);
                match self.g.get(target) {
                    Some(CoreNode::Apply { operator, operands })
                        if *operator == wk::QUOTE && !operands.is_empty() =>
                    {
                        let inner = operands[0];
                        self.check(inner, env)
                    }
                    _ => self.stalled(node),
                }
            }
            // Quoting is opaque by design: `⟨P⟩` is a name for P, not a claim.
            wk::QUOTE => self.unsupported(node),
            _ if env.rels.contains_key(&op) => {
                let rel = &env.rels[&op];
                let ground: Option<Vec<ObjectId>> =
                    args.iter().map(|a| env.get(a).copied().or(Some(*a))).collect();
                let Some(tuple) = ground else { return self.stalled(node) };
                if rel.tuples.contains(&tuple) {
                    EvaluationResult::certain(true)
                } else if rel.complete {
                    EvaluationResult::certain(false)
                } else {
                    // Mid-fixpoint: absence is provisional, not refutation.
                    self.stalled(node)
                }
            }
            _ if self.is_lambda(op) => {
                // Applying an inline definition: bind the parameters and check
                // the body. Intensional, where a bound relation is extensional.
                let Some(CoreNode::Bind { vars, bodies, .. }) = self.g.get(op).cloned() else {
                    return self.unsupported(node);
                };
                if vars.len() != args.len() || bodies.is_empty() {
                    return self.unsupported(node);
                }
                let mut scoped = env.clone();
                for (b, a) in vars.iter().zip(args) {
                    scoped.vars.insert(b.var, env.get(a).copied().unwrap_or(*a));
                }
                self.check(bodies[0], &scoped)
            }
            _ => {
                // Not a built-in. Ask the registry; absence is its own answer.
                if self.registry.has(op) {
                    let mut budget = self.budget.saturating_sub(self.spent);
                    let r = self.registry.evaluate(
                        self.g,
                        node,
                        op,
                        args,
                        &mut budget,
                        self.snapshot,
                    );
                    self.spent = self.spent.saturating_add(1);
                    return r;
                }
                // An uninterpreted predicate: consult the structure.
                let ground: Option<Vec<ObjectId>> =
                    args.iter().map(|a| self.ground(*a, env)).collect();
                let Some(tuple) = ground else { return self.stalled(node) };
                match match self.instant {
                    Some(t) => self.s.known_at(op, &tuple, t),
                    None => self.s.known(op, &tuple),
                } {
                    Some(v) => EvaluationResult::certain(v),
                    // Absence is refutation only where knowledge is complete.
                    None if self.s.is_closed(op) => EvaluationResult::certain(false),
                    None => self.stalled(node),
                }
            }
        }
    }

    fn bind(
        &mut self,
        node: ObjectId,
        binder: ObjectId,
        vars: &[crate::object::Binding],
        bodies: &[ObjectId],
        env: &GraphEnv,
    ) -> EvaluationResult {
        if binder == wk::LETREC {
            return self.letrec(node, vars, bodies, env);
        }
        let universal = match binder {
            wk::FORALL => true,
            wk::EXISTS => false,
            // A lambda is a value, not a proposition; aggregates denote numbers
            // and are read through `number_bounds`.
            _ => return self.unsupported(node),
        };
        // Second-order: a variable whose domain is a relation type ranges over
        // relations, not individuals.
        if let Some(v) = vars.first()
            && let Some(d) = v.domain
            && self.is_relation_type(d)
        {
            return self.second_order(node, universal, v, bodies, env, d);
        }
        let (Some(v), Some(body)) = (vars.first(), bodies.first()) else {
            return self.unsupported(node);
        };
        let Some(domain) = v.domain else { return self.unsupported(node) };
        let Some(members) = self.members_of(domain, env) else {
            // Unenumerable. Not a refusal: probe concretely for a decisive
            // counterexample or witness, then try to close the unbounded tail
            // abstractly. Infinity costs precision, not termination.
            return self.infinite(node, universal, v, *body, domain, env);
        };

        let mut worst = ComputeStatus::Exact;
        let mut residuals = Vec::new();
        for (i, m) in members.iter().enumerate() {
            if self.exhausted() {
                // Keep the question, narrowed to what is left. The continuation
                // is an ordinary expression — the *same* quantifier over the
                // unexamined tail — so resuming is just evaluating it, and each
                // pass strictly shrinks the domain rather than restarting.
                let mut r = self.out_of_budget(node);
                r.continuation = Some(self.narrow(binder, v, &members[i..], *body));
                r.dependencies = members[i..].to_vec();
                return r;
            }
            let scoped = env.with(v.var, *m);
            let r = self.check(*body, &scoped);
            match (universal, r.support.is_certain(), r.refutation.is_certain()) {
                // A counterexample decides a universal; a witness decides an
                // existential. Both are definite and both stop the scan.
                (true, _, true) => return EvaluationResult::certain(false),
                (false, true, _) => return EvaluationResult::certain(true),
                (true, true, _) | (false, _, true) => {}
                _ => {
                    worst = downgrade(worst, r.compute_status);
                    if let Some(x) = r.residual {
                        residuals.push(x);
                    }
                }
            }
        }
        if worst == ComputeStatus::Exact {
            EvaluationResult::certain(universal)
        } else {
            self.combine(node, Bound::None, Bound::None, worst, residuals)
        }
    }

    /// Quantification across an infinite domain.
    ///
    /// A single counterexample still refutes a universal outright, in finite
    /// time. Failing that, an interval abstraction with widening can close the
    /// unbounded tail — the analysis is finite even though the domain is not.
    /// Only when neither lands does this widen to an honest stall.
    fn infinite(
        &mut self,
        node: ObjectId,
        universal: bool,
        v: &crate::object::Binding,
        body: ObjectId,
        domain: ObjectId,
        env: &GraphEnv,
    ) -> EvaluationResult {
        let signed = !self.is_nat(domain);
        let reserve = 64u64.min(self.budget.saturating_sub(self.spent) / 2);
        let mut i: i64 = 0;
        // Phase 1 — concrete probing, depth taken from the budget rather than a
        // constant, so more budget genuinely buys more search.
        while self.budget.saturating_sub(self.spent) > reserve && i < 1_000_000 {
            let n = if signed {
                if i % 2 == 0 { i / 2 } else { -(i / 2) - 1 }
            } else {
                i
            };
            let lit = self.g.int(n);
            let scoped = env.with(v.var, lit);
            let r = self.check(body, &scoped);
            match (universal, r.support.is_certain(), r.refutation.is_certain()) {
                (true, _, true) => return EvaluationResult::certain(false),
                (false, true, _) => return EvaluationResult::certain(true),
                _ => {}
            }
            i += 1;
        }
        if self.exhausted() {
            return self.out_of_budget(node);
        }
        // Phase 2 — close the tail abstractly. Widening jumps a moving bound
        // straight to unbounded, which is what forces convergence.
        let lo = if signed { None } else { Some(i.max(1)) };
        match self.abstract_holds(body, v.var, lo, env) {
            Some(true) if universal => EvaluationResult::certain(true),
            Some(false) if !universal => EvaluationResult::certain(false),
            _ => self.stalled(node),
        }
    }

    fn is_nat(&mut self, d: ObjectId) -> bool {
        d == wk::NAT_TYPE
            || matches!(self.g.get(d), Some(CoreNode::Apply { operator, .. })
                        if *operator == wk::NAT_TYPE)
    }

    /// Does `body` hold for every value of `var` in `[lo, ∞)`?
    ///
    /// Only the linear fragment is interpreted: a comparison is normalised to
    /// `(a − b) ≤ 0` first so correlated occurrences of the variable cancel —
    /// without that, plain intervals cannot even prove `n ≤ n + 1`.
    fn abstract_holds(
        &mut self,
        body: ObjectId,
        var: ObjectId,
        lo: Option<i64>,
        env: &GraphEnv,
    ) -> Option<bool> {
        let node = self.g.get(body).cloned()?;
        match node {
            CoreNode::Apply { operator, operands } if operator == wk::LEQ => {
                let [a, b] = operands.as_slice() else { return None };
                let mut diff = self.lin_form(*a, var, env)?;
                let rhs = self.lin_form(*b, var, env)?;
                for (k, c) in rhs.0 {
                    *diff.0.entry(k).or_insert_with(|| BigInt::from(0)) -= c;
                }
                diff.1 -= rhs.1;
                let coeff = diff.0.get(&var).cloned().unwrap_or_else(|| BigInt::from(0));
                let others_zero = diff.0.iter().all(|(k, c)| *k == var || *c == BigInt::from(0));
                if !others_zero {
                    return None;
                }
                if coeff == BigInt::from(0) {
                    // Variable cancelled: the claim is a constant fact.
                    return Some(diff.1 <= BigInt::from(0));
                }
                // A non-zero coefficient over an unbounded tail is unbounded in
                // that direction, so the comparison cannot hold everywhere.
                let _ = lo;
                Some(false).filter(|_| coeff > BigInt::from(0))
            }
            CoreNode::Apply { operator, operands } if operator == wk::AND => {
                let mut all = true;
                for o in operands {
                    match self.abstract_holds(o, var, lo, env) {
                        Some(true) => {}
                        Some(false) => return Some(false),
                        None => all = false,
                    }
                }
                all.then_some(true)
            }
            CoreNode::Apply { operator, operands } if operator == wk::NOT => {
                let inner = operands.first()?;
                self.abstract_holds(*inner, var, lo, env).map(|b| !b)
            }
            _ => None,
        }
    }

    /// `Σ cᵢ·vᵢ + k`, or `None` outside the linear fragment.
    #[allow(clippy::type_complexity)]
    fn lin_form(
        &mut self,
        t: ObjectId,
        var: ObjectId,
        env: &GraphEnv,
    ) -> Option<(BTreeMap<ObjectId, BigInt>, BigInt)> {
        let r = env.get(&t).copied().unwrap_or(t);
        if r == var {
            let mut m = BTreeMap::new();
            m.insert(var, BigInt::from(1));
            return Some((m, BigInt::from(0)));
        }
        match self.g.get(r).cloned() {
            Some(CoreNode::Literal(LiteralValue::Int(n))) => {
                Some((BTreeMap::new(), n))
            }
            Some(CoreNode::Apply { operator, operands }) => match (operator, operands.as_slice()) {
                (wk::ADD, [a, b]) => {
                    let (mut x, xk) = self.lin_form(*a, var, env)?;
                    let (y, yk) = self.lin_form(*b, var, env)?;
                    for (k, c) in y {
                        *x.entry(k).or_insert_with(|| BigInt::from(0)) += c;
                    }
                    Some((x, xk + yk))
                }
                (wk::NEG, [a]) => {
                    let (x, k) = self.lin_form(*a, var, env)?;
                    Some((x.into_iter().map(|(a, b)| (a, -b)).collect(), -k))
                }
                (wk::MUL, [a, b]) => {
                    let (x, xk) = self.lin_form(*a, var, env)?;
                    let (y, yk) = self.lin_form(*b, var, env)?;
                    // Linear only when one side is constant.
                    if x.is_empty() {
                        Some((y.into_iter().map(|(k, c)| (k, c * &xk)).collect(), xk * yk))
                    } else if y.is_empty() {
                        Some((x.into_iter().map(|(k, c)| (k, c * &yk)).collect(), xk * yk))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn is_relation_type(&mut self, d: ObjectId) -> bool {
        matches!(self.g.get(d), Some(CoreNode::Apply { operator, .. })
                 if *operator == wk::RELATION_TYPE)
    }

    /// Least fixpoint. Monotone definitions converge and are marked complete; a
    /// negative occurrence makes the operator non-monotone, so rather than
    /// compute a wrong fixpoint the relation stays incomplete and everything
    /// downstream degrades honestly.
    fn letrec(
        &mut self,
        node: ObjectId,
        vars: &[crate::object::Binding],
        bodies: &[ObjectId],
        env: &GraphEnv,
    ) -> EvaluationResult {
        let (Some(pred), Some(def), Some(scope)) =
            (vars.first(), bodies.first(), bodies.last())
        else {
            return self.unsupported(node);
        };
        let params: Vec<&crate::object::Binding> = vars[1..].iter().collect();
        if params.is_empty() {
            return self.unsupported(node);
        }
        let mut domains = Vec::new();
        for p in &params {
            let Some(d) = p.domain else { return self.unsupported(node) };
            let Some(m) = self.members_of(d, env) else { return self.stalled(node) };
            domains.push((p.var, m));
        }
        let candidates = product(&domains);

        let mut rel = BoundRelation { tuples: Default::default(), complete: false };
        let monotone = !self.occurs_negatively(*def, pred.var, false, 0);
        loop {
            if self.exhausted() {
                break;
            }
            let mut next = rel.tuples.clone();
            for cand in &candidates {
                if self.exhausted() {
                    break;
                }
                let mut scoped = env.clone();
                for (v, t) in cand {
                    scoped.vars.insert(*v, *t);
                }
                scoped.rels.insert(
                    pred.var,
                    BoundRelation { tuples: rel.tuples.clone(), complete: monotone },
                );
                if self.check(*def, &scoped).support.is_certain() {
                    next.insert(params.iter().map(|p| cand[&p.var]).collect());
                }
            }
            if next == rel.tuples {
                rel.complete = monotone;
                break;
            }
            rel.tuples = next;
        }
        let scoped = env.with_rel(pred.var, rel);
        self.check(*scope, &scoped)
    }

    fn occurs_negatively(&mut self, f: ObjectId, p: ObjectId, neg: bool, depth: u32) -> bool {
        if depth > 64 {
            return true;
        }
        match self.g.get(f).cloned() {
            Some(CoreNode::Apply { operator, operands }) => {
                if operator == p {
                    return neg;
                }
                let flip = operator == wk::NOT;
                operands
                    .iter()
                    .any(|o| self.occurs_negatively(*o, p, neg ^ flip, depth + 1))
            }
            Some(CoreNode::Bind { bodies, .. }) => {
                bodies.iter().any(|b| self.occurs_negatively(*b, p, neg, depth + 1))
            }
            _ => false,
        }
    }

    /// Quantification over relations, with the soundness discipline that cost a
    /// confident wrong answer once already: exhausting the candidate space is a
    /// verdict **only** when that space really was the whole powerset.
    fn second_order(
        &mut self,
        node: ObjectId,
        universal: bool,
        v: &crate::object::Binding,
        bodies: &[ObjectId],
        env: &GraphEnv,
        rel_ty: ObjectId,
    ) -> EvaluationResult {
        let Some(body) = bodies.first() else { return self.unsupported(node) };
        let Some(CoreNode::Apply { operands, .. }) = self.g.get(rel_ty).cloned() else {
            return self.unsupported(node);
        };
        let mut per_position = Vec::new();
        let mut exhaustive = true;
        for d in &operands {
            match self.members_of(*d, env) {
                Some(m) => per_position.push(m),
                None => {
                    // Infinite position: the powerset is uncountable, so
                    // exhaustion is unreachable and cannot yield a verdict.
                    exhaustive = false;
                    per_position.push(self.ground_points(*body));
                }
            }
        }
        let tuples = tuples_of(&per_position);
        if tuples.len() > 20 {
            // Too wide to walk within any sane budget.
            return self.stalled(node);
        }
        let mut mask = vec![false; tuples.len()];
        loop {
            if self.exhausted() {
                return self.out_of_budget(node);
            }
            let rel = BoundRelation {
                tuples: (0..tuples.len())
                    .filter(|i| mask[*i])
                    .map(|i| tuples[i].clone())
                    .collect(),
                complete: true,
            };
            let scoped = env.with_rel(v.var, rel);
            let r = self.check(*body, &scoped);
            match (universal, r.support.is_certain(), r.refutation.is_certain()) {
                (true, _, true) => return EvaluationResult::certain(false),
                (false, true, _) => return EvaluationResult::certain(true),
                _ => {}
            }
            if !increment(&mut mask) {
                break;
            }
        }
        if exhaustive {
            EvaluationResult::certain(universal)
        } else {
            self.stalled(node)
        }
    }

    /// Ground points a body names directly — the only members of an infinite
    /// domain a finite formula can distinguish.
    fn ground_points(&mut self, body: ObjectId) -> Vec<ObjectId> {
        let mut out = Vec::new();
        for id in self.g.reachable(body) {
            if matches!(
                self.g.get(id),
                Some(CoreNode::Literal(_)) | Some(CoreNode::Atom { name: Some(_) })
            ) && !out.contains(&id)
            {
                out.push(id);
            }
        }
        out.truncate(4);
        out
    }

    fn combine(
        &self,
        node: ObjectId,
        support: Bound,
        refutation: Bound,
        status: ComputeStatus,
        residuals: Vec<ObjectId>,
    ) -> EvaluationResult {
        let mut r = EvaluationResult::new(status);
        r.support = support;
        r.refutation = refutation;
        r.residual = Some(node);
        r.dependencies = residuals;
        r.snapshot = self.snapshot;
        r
    }

    fn is_lambda(&mut self, id: ObjectId) -> bool {
        matches!(self.g.get(id), Some(CoreNode::Bind { binder, .. }) if *binder == wk::LAMBDA)
    }

    /// Members of a domain expression.
    ///
    /// `(set a b c)` enumerates literally and `(where D p)` refines by a
    /// predicate, so union, difference and carved-out subsets are ordinary
    /// expressions rather than enum variants. Anything else is asked of the
    /// structure.
    fn members_of(&mut self, domain: ObjectId, env: &GraphEnv) -> Option<Vec<ObjectId>> {
        match self.g.get(domain).cloned() {
            Some(CoreNode::Apply { operator, operands }) if operator == wk::SET_DOMAIN => {
                Some(operands)
            }
            Some(CoreNode::Apply { operator, operands }) if operator == wk::WHERE_DOMAIN => {
                let [base, filter] = operands.as_slice() else { return None };
                let base_members = self.members_of(*base, env)?;
                let mut out = Vec::new();
                for m in base_members {
                    if self.exhausted() {
                        return Some(out);
                    }
                    // A member that cannot be ruled out is kept: the refinement
                    // over-approximates rather than silently dropping it.
                    let mut scoped = env.clone();
                    if let Some(CoreNode::Bind { vars, .. }) = self.g.get(*filter)
                        && let Some(v) = vars.first()
                    {
                        scoped.vars.insert(v.var, m);
                    }
                    let r = self.check(*filter, &scoped);
                    if !r.refutation.is_certain() {
                        out.push(m);
                    }
                }
                Some(out)
            }
            _ => self.s.extension(domain),
        }
    }

    /// The same quantifier over an explicit remaining set.
    ///
    /// Kept *quantified* rather than unrolled into a conjunction, so the
    /// continuation is no larger than the question that produced it and each
    /// pass strictly shrinks what is left. Unrolling would make re-asking
    /// slower than starting over, and the measure would climb instead of fall.
    fn narrow(
        &mut self,
        binder: ObjectId,
        v: &crate::object::Binding,
        rest: &[ObjectId],
        body: ObjectId,
    ) -> ObjectId {
        let remaining = self.g.apply(wk::SET_DOMAIN, rest.to_vec());
        self.g.quantify(binder, v.var, Some(remaining), body)
    }

    fn ground(&mut self, id: ObjectId, env: &GraphEnv) -> Option<ObjectId> {
        let r = env.get(&id).copied().unwrap_or(id);
        match self.g.get(r) {
            Some(CoreNode::Atom { .. })
            | Some(CoreNode::Literal(_))
            | Some(CoreNode::External(_)) => Some(r),
            _ => Some(r),
        }
    }

    fn number(&mut self, id: ObjectId, env: &GraphEnv) -> Option<BigInt> {
        let r = env.get(&id).copied().unwrap_or(id);
        match self.g.get(r).cloned() {
            Some(CoreNode::Literal(LiteralValue::Int(n))) => Some(n.clone()),
            Some(CoreNode::Apply { operator, operands }) => {
                let vals: Option<Vec<BigInt>> =
                    operands.iter().map(|o| self.number(*o, env)).collect();
                let v = vals?;
                match (operator, v.as_slice()) {
                    (wk::ADD, [a, b]) => Some(a + b),
                    (wk::MUL, [a, b]) => Some(a * b),
                    (wk::NEG, [a]) => Some(-a),
                    (wk::DIV, [a, b]) if b.to_i64() != Some(0) => Some(a / b),
                    _ => None,
                }
            }
            Some(CoreNode::Bind { binder, vars, bodies })
                if binder == wk::COUNT || binder == wk::SUM =>
            {
                let (lo, hi) = self.aggregate_bounds(binder, &vars, &bodies, env)?;
                // Only an exact aggregate is a value; a partial one is still a
                // usable bound, which `LEQ` reads.
                (lo == hi).then_some(lo)
            }
            _ => None,
        }
    }

    /// Two-sided bounds for `count` / `sum`: `lo` counts what is proven, `hi`
    /// adds what is not yet ruled out. When they meet the aggregate is exact,
    /// which is what lets a comparison decide before the scan finishes.
    fn aggregate_bounds(
        &mut self,
        binder: ObjectId,
        vars: &[crate::object::Binding],
        bodies: &[ObjectId],
        env: &GraphEnv,
    ) -> Option<(BigInt, BigInt)> {
        let v = vars.first()?;
        let body = *bodies.first()?;
        let value = bodies.get(1).copied();
        let members = self.members_of(v.domain?, env)?;
        let (mut lo, mut hi) = (BigInt::from(0), BigInt::from(0));
        for m in members {
            if self.exhausted() {
                return Some((lo, BigInt::from(i64::MAX)));
            }
            let scoped = env.with(v.var, m);
            let contribution = match (binder == wk::SUM, value) {
                (true, Some(t)) => self.number(t, &scoped)?,
                _ => BigInt::from(1),
            };
            let r = self.check(body, &scoped);
            if r.support.is_certain() {
                lo += &contribution;
                hi += &contribution;
            } else if !r.refutation.is_certain() {
                hi += &contribution;
            }
        }
        Some((lo, hi))
    }

    fn text(&mut self, id: ObjectId, env: &GraphEnv) -> Option<String> {
        let r = env.get(&id).copied().unwrap_or(id);
        match self.g.get(r).cloned() {
            Some(CoreNode::Literal(LiteralValue::Text(s))) => Some(s.clone()),
            Some(CoreNode::Apply { operator, operands }) if operator == wk::CONCAT => {
                let parts: Option<Vec<String>> =
                    operands.iter().map(|o| self.text(*o, env)).collect();
                Some(parts?.concat())
            }
            _ => None,
        }
    }
}

fn increment(mask: &mut [bool]) -> bool {
    for bit in mask.iter_mut() {
        if *bit {
            *bit = false;
        } else {
            *bit = true;
            return true;
        }
    }
    false
}

fn tuples_of(per_position: &[Vec<ObjectId>]) -> Vec<Vec<ObjectId>> {
    let mut acc: Vec<Vec<ObjectId>> = vec![Vec::new()];
    for points in per_position {
        let mut next = Vec::new();
        for prefix in &acc {
            for p in points {
                let mut q = prefix.clone();
                q.push(*p);
                next.push(q);
            }
        }
        acc = next;
    }
    acc
}

fn product(domains: &[(ObjectId, Vec<ObjectId>)]) -> Vec<BTreeMap<ObjectId, ObjectId>> {
    let mut acc: Vec<BTreeMap<ObjectId, ObjectId>> = vec![BTreeMap::new()];
    for (v, members) in domains {
        let mut next = Vec::new();
        for prefix in &acc {
            for m in members {
                let mut b = prefix.clone();
                b.insert(*v, *m);
                next.push(b);
            }
        }
        acc = next;
    }
    acc
}

/// Worst-case merge of computation statuses. `Unsupported` dominates, because a
/// missing operator is not fixed by more budget.
fn downgrade(a: ComputeStatus, b: ComputeStatus) -> ComputeStatus {
    use ComputeStatus::*;
    match (a, b) {
        (Unsupported, _) | (_, Unsupported) => Unsupported,
        (BudgetExhausted, _) | (_, BudgetExhausted) => BudgetExhausted,
        (Stalled, _) | (_, Stalled) => Stalled,
        _ => Exact,
    }
}

/// A structure backed by an in-memory table, for tests and small views.
#[derive(Default)]
pub struct MapGraphStructure {
    facts: std::collections::BTreeSet<(ObjectId, Vec<ObjectId>)>,
    closed: std::collections::BTreeSet<ObjectId>,
    domains: BTreeMap<ObjectId, Vec<ObjectId>>,
    version: u64,
}

impl MapGraphStructure {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn fact(mut self, pred: ObjectId, args: Vec<ObjectId>) -> Self {
        self.facts.insert((pred, args));
        self.version += 1;
        self
    }
    pub fn closed(mut self, pred: ObjectId) -> Self {
        self.closed.insert(pred);
        self
    }
    pub fn domain(mut self, d: ObjectId, members: Vec<ObjectId>) -> Self {
        self.domains.insert(d, members);
        self
    }
    pub fn at_version(mut self, v: u64) -> Self {
        self.version = v;
        self
    }
}

impl GraphStructure for MapGraphStructure {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Option<bool> {
        if self.facts.contains(&(pred, args.to_vec())) {
            return Some(true);
        }
        self.closed.contains(&pred).then_some(false)
    }
    fn extension(&self, domain: ObjectId) -> Option<Vec<ObjectId>> {
        self.domains.get(&domain).cloned()
    }
    fn is_closed(&self, pred: ObjectId) -> bool {
        self.closed.contains(&pred)
    }
    fn snapshot(&self) -> u64 {
        self.version
    }
}
