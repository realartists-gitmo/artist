//! Derivations as checkable objects.
//!
//! The evaluator records *what* it concluded and, since the axes landed,
//! something of *how* — `dependencies`, `defeated_by`, `residual`. None of it is
//! checkable. To find out whether an answer was right you re-run the evaluator,
//! which means trusting the evaluator, which is the thing you wanted to check.
//!
//! This is the **de Bruijn criterion**: a proof assistant's real claim is not
//! that its tactics are clever but that a small, dumb kernel validates the
//! result, so only the kernel has to be trusted. Lean's is a few thousand lines
//! and everything else in the system is untrusted machinery that produces terms
//! for it to check.
//!
//! The same move here, with one difference that matters. Lean's kernel is
//! **total** — every check terminates in yes or no. This evaluator is anytime
//! and two-sided, so a certificate has to be able to say *"here is what I
//! established, here is what I did not, and here is the boundary"*. A
//! certificate is therefore not a proof of a proposition. It is a proof of an
//! **evaluation result**: this bound, from these facts, by these steps.
//!
//! What that buys, concretely: the answer to *"why do you believe this"* becomes
//! an object the memory can store, transmit, and re-check cheaply — rather than
//! a re-derivation whose cost is the original query's and whose trust is the
//! original evaluator's.
//!
//! **The checker below knows nothing about resolvers, budgets, rules, scans or
//! any of the six thousand lines it validates.** It knows the shape of a step
//! and the rule for each shape. That asymmetry is the whole point; if checking
//! were as complicated as evaluating there would be no reason to prefer it.

use crate::evidence::{Bound, Derivation, Determinacy, EvaluationResult, Grounding};
use crate::graph_eval::Skeleton;
use crate::object::{CoreNode, LiteralValue, ObjectGraph, ObjectId, wk};

/// One inference, naming its conclusion and what it rests on.
///
/// Deliberately small. Every variant is checkable by looking at the node and the
/// premises, without consulting a store, a budget, or anything the evaluator
/// knows — which is what makes the checker independent rather than a second
/// copy of the evaluator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The structure was asked and answered. **The one place the checker takes
    /// something on trust**, and it is explicit about it. Whether to believe the
    /// sources is the caller's judgement, not the checker's, and naming them is
    /// what makes that judgement possible at all.
    ///
    /// `sources` names the *agents, sessions or documents* on whose word the
    /// answer rests — never the relation symbol. Naming the predicate, which is
    /// what this field used to hold, made every fact its own authority: an
    /// unfalsifiable circle that read like provenance and carried none, while
    /// the asserting agent, the evidence origin and the source span were all
    /// sitting in the store unasked-for.
    ///
    /// **Empty is a legitimate answer and is not the same as unattributed being
    /// impossible.** A structure that vouches for something and names nobody has
    /// told the reader precisely that, and the checker records it under
    /// [`Checked::assumed`] rather than inventing a name to fill the slot.
    Told { node: ObjectId, holds: bool, sources: Vec<ObjectId> },
    /// A truth atom. `#true` is supported, `#false` refuted, in every structure.
    Axiom { node: ObjectId, holds: bool },
    /// True on its propositional skeleton alone, whatever the atoms mean.
    ///
    /// `constructive` distinguishes the two cases the determinacy axis exists to
    /// keep apart. A constructively valid formula — `P → P`, `¬(P ∧ ¬P)` — owes
    /// nothing to anybody and its totality is *derived*. One valid only under
    /// the classical truth table needs bivalence, which the checker has no store
    /// to verify, so its totality is an **assumption** and is recorded as an
    /// authority rather than returned as a finding.
    Tautology { node: ObjectId, holds: bool, constructive: bool },
    /// `(not P)` from `P`, with the bounds swapped.
    Negation { node: ObjectId, premise: usize },
    /// A conjunct decides a conjunction false; a disjunct decides a disjunction
    /// true. Records which.
    Connective { node: ObjectId, premises: Vec<usize>, holds: bool },
    /// One member settled a scan — the witness of an existential or the
    /// counterexample to a universal.
    ///
    /// `instance` is the binder's body with the bound variable replaced by
    /// `value`, and the premise must be exactly that node. Without it the step
    /// was **unfalsifiable by construction**: the checker knew the node was a
    /// binder and the premise was certain, and nothing tied the two together, so
    /// an existential could be proved from `#true`. That was not a missing
    /// check — it was missing *information*, and no check could have been added
    /// without it.
    Instance {
        node: ObjectId,
        premise: usize,
        value: ObjectId,
        instance: ObjectId,
        holds: bool,
    },
    /// Every member of an enumerated domain agreed, and the enumeration was
    /// **complete**. The completeness claim is part of the step and is exactly
    /// what a reader should be sceptical about.
    ///
    /// Each premise is paired with the member it is about, and each must be the
    /// binder's body under that member — same reason as [`Step::Instance`].
    Exhaustive {
        node: ObjectId,
        premises: Vec<(usize, ObjectId, ObjectId)>,
        holds: bool,
        complete: bool,
    },
    /// A stored rule fired. Defeasible rules mark it, so a reader can see that
    /// the conclusion holds absent a defeater rather than outright.
    Rule { node: ObjectId, rule: ObjectId, premises: Vec<usize>, defeasible: bool },
    /// The store holds both directions. The sources on *either* side, since a
    /// conflict is exactly the case where a reader wants to go and read them.
    Conflict { node: ObjectId, sources: Vec<ObjectId> },
    /// The sentence has no stable value — its own truth is among its premises.
    Ungrounded { node: ObjectId, oscillating: bool },
    /// Establishes nothing. Present so a certificate can be *total* over the
    /// query even where evaluation was not, which is the difference between an
    /// anytime certificate and a proof.
    Unestablished { node: ObjectId },
}

impl Step {
    pub fn node(&self) -> ObjectId {
        match self {
            Step::Told { node, .. }
            | Step::Axiom { node, .. }
            | Step::Tautology { node, .. }
            | Step::Negation { node, .. }
            | Step::Connective { node, .. }
            | Step::Instance { node, .. }
            | Step::Exhaustive { node, .. }
            | Step::Rule { node, .. }
            | Step::Conflict { node, .. }
            | Step::Ungrounded { node, .. }
            | Step::Unestablished { node } => *node,
        }
    }
}

/// A derivation: steps in dependency order, concluding with the last.
///
/// Premises are **backward indices** into the same vector, so a certificate is
/// acyclic by construction and the checker needs no cycle detection. That is not
/// a convenience — a checker that had to detect cycles would need the machinery
/// the evaluator has, and the point is that it needs almost none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Certificate {
    pub steps: Vec<Step>,
}

/// What a checked certificate establishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub node: ObjectId,
    pub support: Bound,
    pub refutation: Bound,
    pub grounding: Grounding,
    pub derivation: Derivation,
    pub determinacy: Determinacy,
    /// Every authority the derivation leaned on — sources you can go and check,
    /// and the rules that fired. The reader decides whether to believe them; the
    /// checker only insists they be named.
    pub authorities: Vec<ObjectId>,
    /// Nodes taken on trust with **nobody to blame**: a structure that answered
    /// and named no source, and a classical tautology whose bivalence the kernel
    /// has no store to verify.
    ///
    /// Kept apart from `authorities` because the reader's move differs. An
    /// authority can be consulted, weighed, or distrusted by name; an assumption
    /// can only be accepted or rejected. Folding the two — which is what the
    /// bivalence marker used to do — made "on nobody's authority" indexable as
    /// though it were somebody's.
    pub assumed: Vec<ObjectId>,
}

/// Why a certificate was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalid {
    Empty,
    /// A premise index pointing forward, or past the end.
    BadReference { step: usize },
    /// The step's shape does not match the node it claims to conclude.
    Mismatched { step: usize },
    /// The step's rule does not license its conclusion from its premises.
    Unlicensed { step: usize },
}

impl Certificate {
    pub fn push(&mut self, step: Step) -> usize {
        self.steps.push(step);
        self.steps.len() - 1
    }

    pub fn conclusion(&self) -> Option<ObjectId> {
        self.steps.last().map(Step::node)
    }

    /// **The kernel.** Validate every step against the graph and its premises.
    ///
    /// Takes `&ObjectGraph` — read-only, so checking cannot mint nodes, intern
    /// anything, or otherwise change what it is checking. It consults no
    /// structure, spends no budget, and has no configuration. Its whole
    /// vocabulary is: does this node have the shape this step claims, and do the
    /// premises license the conclusion.
    pub fn check(&self, g: &ObjectGraph) -> Result<Checked, Invalid> {
        if self.steps.is_empty() {
            return Err(Invalid::Empty);
        }
        let mut out: Vec<Checked> = Vec::with_capacity(self.steps.len());
        for (i, step) in self.steps.iter().enumerate() {
            let premise = |k: usize| -> Result<&Checked, Invalid> {
                // Backward references only: this is what makes the certificate
                // acyclic without a cycle check.
                if k >= i {
                    return Err(Invalid::BadReference { step: i });
                }
                out.get(k).ok_or(Invalid::BadReference { step: i })
            };
            let checked = match step {
                Step::Told { node, holds, sources } => Checked {
                    node: *node,
                    support: if *holds { Bound::Certain } else { Bound::None },
                    refutation: if *holds { Bound::None } else { Bound::Certain },
                    grounding: Grounding::Grounded,
                    derivation: Derivation::Observed,
                    determinacy: Determinacy::Unknown,
                    authorities: sources.clone(),
                    // Vouched for by nobody in particular. Said outright rather
                    // than papered over with the predicate's own name.
                    assumed: if sources.is_empty() { vec![*node] } else { Vec::new() },
                },
                Step::Axiom { node, holds } => {
                    let expected = if *holds { wk::TOP } else { wk::BOT };
                    if *node != expected {
                        return Err(Invalid::Mismatched { step: i });
                    }
                    base(*node, *holds, Determinacy::Total)
                }
                Step::Tautology { node, holds, constructive } => {
                    // Re-decide the skeleton. This is the one place the checker
                    // computes rather than compares — decidable, bounded by the
                    // term, and needing no store.
                    let Some((shape, atoms)) = shape_of(g, *node) else {
                        return Err(Invalid::Unlicensed { step: i });
                    };
                    if atoms > 8 || !(0u32..(1u32 << atoms)).all(|m| shape.eval(m) == *holds) {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    // The claim to be *constructively* valid is checked, not
                    // taken: `P → P` earns its totality, `P ∨ ¬P` assumes it.
                    let target = if *holds {
                        shape
                    } else {
                        Skeleton::Not(Box::new(shape))
                    };
                    if *constructive && !target.intuitionistic() {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    let mut c = base(*node, *holds, Determinacy::Total);
                    if !*constructive {
                        // Bivalence, which the kernel cannot verify. Named as an
                        // assumption so a reader can refuse it — otherwise
                        // excluded middle over a vague predicate validates as
                        // "established total, on nobody's authority", which is
                        // exactly the laundering the determinacy axis exists to
                        // stop, arriving through the kernel.
                        c.assumed.push(*node);
                    }
                    c
                }
                Step::Negation { node, premise: k } => {
                    let p = premise(*k)?;
                    match g.get(*node) {
                        Some(CoreNode::Apply { operator, operands })
                            if *operator == wk::NOT
                                && operands.len() == 1
                                && operands[0] == p.node => {}
                        _ => return Err(Invalid::Mismatched { step: i }),
                    }
                    Checked {
                        node: *node,
                        support: p.refutation,
                        refutation: p.support,
                        ..p.clone()
                    }
                }
                Step::Connective { node, premises, holds } => {
                    let (op, operands) = match g.get(*node) {
                        Some(CoreNode::Apply { operator, operands }) => (*operator, operands),
                        _ => return Err(Invalid::Mismatched { step: i }),
                    };
                    // `implies` decides like a disjunction of the negated
                    // antecedent with the consequent, and the evaluator emits
                    // `Connective` for it — which this arm rejected outright, so
                    // any query containing a decided implication produced a
                    // certificate its own checker refused.
                    let disjunctive = matches!(op, wk::OR | wk::IMPLIES);
                    let conjunctive = op == wk::AND;
                    if !disjunctive && !conjunctive {
                        return Err(Invalid::Mismatched { step: i });
                    }
                    // **Every premise points the way the conclusion does.** A
                    // conjunction is true because its conjuncts are true and
                    // false because one is false; a disjunction is the mirror.
                    // In all four cases the premise's *supporting* side is the
                    // one the step claims — which the previous three-way
                    // expression computed backwards for `(and, true)`, so a
                    // conjunction could be certified **true from proofs its
                    // conjuncts were false**, and an honest conjunction proof
                    // could not be represented at all.
                    let one_decides = (conjunctive && !*holds) || (disjunctive && *holds);
                    let mut acc = decided(*node, *holds);
                    let mut covered: Vec<ObjectId> = Vec::new();
                    for k in premises {
                        let p = premise(*k)?;
                        if !operands.contains(&p.node) {
                            return Err(Invalid::Mismatched { step: i });
                        }
                        // `implies` negates its antecedent, so a premise in
                        // position zero points the other way.
                        let negated = op == wk::IMPLIES && operands.first() == Some(&p.node);
                        let want = if *holds != negated { p.support } else { p.refutation };
                        if want != Bound::Certain {
                            return Err(Invalid::Unlicensed { step: i });
                        }
                        if !covered.contains(&p.node) {
                            covered.push(p.node);
                        }
                        absorb(&mut acc, p);
                    }
                    // **Coverage, not arity.** Counting premises let the same
                    // index be cited twice to satisfy a two-operand
                    // requirement, so one refuted disjunct proved a disjunction
                    // false. What is needed is that every operand is actually
                    // accounted for.
                    if one_decides {
                        if covered.is_empty() {
                            return Err(Invalid::Unlicensed { step: i });
                        }
                    } else if operands.iter().any(|o| !covered.contains(o)) {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    acc
                }
                Step::Instance { node, premise: k, value, instance, holds } => {
                    let p = premise(*k)?;
                    let (binder, body) = match g.get(*node) {
                        Some(CoreNode::Bind { binder, bodies, .. }) => {
                            (*binder, *bodies.first().ok_or(Invalid::Mismatched { step: i })?)
                        }
                        _ => return Err(Invalid::Mismatched { step: i }),
                    };
                    // A witness settles an existential true; a counterexample
                    // settles a universal false. The other two combinations are
                    // not licensed by a single member.
                    if !matches!((binder, holds), (wk::EXISTS, true) | (wk::FORALL, false)) {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    // The premise must be about *this* member of *this* body.
                    if p.node != *instance || !instantiates(g, body, *value, *instance, 0) {
                        return Err(Invalid::Mismatched { step: i });
                    }
                    let want = if *holds { p.support } else { p.refutation };
                    if want != Bound::Certain {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    let mut acc = decided(*node, *holds);
                    absorb(&mut acc, p);
                    acc
                }
                Step::Exhaustive { node, premises, holds, complete } => {
                    // The whole content of this step is the completeness claim.
                    // Without it, agreement among the members you happened to
                    // see licenses nothing.
                    if !*complete {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    let body = match g.get(*node) {
                        Some(CoreNode::Bind { binder, bodies, .. })
                            if matches!(*binder, wk::FORALL | wk::EXISTS) =>
                        {
                            *bodies.first().ok_or(Invalid::Mismatched { step: i })?
                        }
                        _ => return Err(Invalid::Mismatched { step: i }),
                    };
                    if premises.is_empty() {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    let mut acc = decided(*node, *holds);
                    let mut seen: Vec<ObjectId> = Vec::new();
                    for (k, value, instance) in premises {
                        let p = premise(*k)?;
                        if p.node != *instance || !instantiates(g, body, *value, *instance, 0) {
                            return Err(Invalid::Mismatched { step: i });
                        }
                        if seen.contains(value) {
                            // Citing one member twice is not a second member.
                            return Err(Invalid::Unlicensed { step: i });
                        }
                        seen.push(*value);
                        let want = if *holds { p.support } else { p.refutation };
                        if want != Bound::Certain {
                            return Err(Invalid::Unlicensed { step: i });
                        }
                        absorb(&mut acc, p);
                    }
                    acc
                }
                Step::Rule { node, rule, premises, defeasible } => {
                    // A rule that rests on nothing concludes nothing. With an
                    // empty premise list the loop below was empty and *any*
                    // node — including one absent from the graph — came back
                    // `Certain`, which made this a second undeclared trust
                    // point stronger than `Told`.
                    if premises.is_empty() || g.get(*node).is_none() {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    let mut acc = decided(*node, true);
                    acc.derivation =
                        if *defeasible { Derivation::Default } else { Derivation::Derived };
                    for k in premises {
                        let p = premise(*k)?;
                        if p.support != Bound::Certain {
                            return Err(Invalid::Unlicensed { step: i });
                        }
                        absorb(&mut acc, p);
                    }
                    // The rule itself is an authority: a reader who does not
                    // accept the rule does not accept the conclusion.
                    if !acc.authorities.contains(rule) {
                        acc.authorities.push(*rule);
                    }
                    acc.derivation =
                        if *defeasible { Derivation::Default } else { acc.derivation };
                    acc
                }
                Step::Conflict { node, sources } => Checked {
                    node: *node,
                    support: Bound::Certain,
                    refutation: Bound::Certain,
                    grounding: Grounding::Grounded,
                    derivation: Derivation::Observed,
                    determinacy: Determinacy::Unknown,
                    authorities: sources.clone(),
                    assumed: if sources.is_empty() { vec![*node] } else { Vec::new() },
                },
                Step::Ungrounded { node, oscillating } => Checked {
                    node: *node,
                    support: Bound::None,
                    refutation: Bound::None,
                    grounding: if *oscillating {
                        Grounding::Oscillatory
                    } else {
                        Grounding::StableLoop
                    },
                    derivation: Derivation::Observed,
                    determinacy: Determinacy::Unknown,
                    authorities: Vec::new(),
                    assumed: Vec::new(),
                },
                Step::Unestablished { node } => Checked {
                    node: *node,
                    support: Bound::None,
                    refutation: Bound::None,
                    grounding: Grounding::Grounded,
                    derivation: Derivation::Observed,
                    determinacy: Determinacy::Unknown,
                    authorities: Vec::new(),
                    assumed: Vec::new(),
                },
            };
            out.push(checked);
        }
        Ok(out.pop().expect("non-empty"))
    }

    /// Does this certificate establish `result` for `node`?
    ///
    /// A certificate may prove *less* than the evaluator claimed — that is
    /// honest, and it is what happens when part of an answer came from a path
    /// that emits no step. It may never prove **more**, and that is the property
    /// worth testing.
    pub fn justifies(&self, g: &ObjectGraph, node: ObjectId, result: &EvaluationResult) -> bool {
        match self.check(g) {
            Ok(c) => {
                c.node == node
                    && c.support <= result.support
                    && c.refutation <= result.refutation
                    && c.grounding == result.grounding
                    // A certificate claiming a conclusion is *observed* does not
                    // justify a result that is defeasible, and one claiming a
                    // sharp condition does not justify one that has none.
                    && c.derivation >= result.derivation
                    && c.determinacy >= result.determinacy
            }
            Err(_) => false,
        }
    }
}

impl Certificate {
    /// Encode as an ordinary expression: `(proof step …)`.
    ///
    /// The round trip through [`Self::from_term`] is exact, and that is the
    /// property worth stating: a certificate that survives storage as an
    /// approximation is a certificate whose kernel check means nothing after a
    /// restart. Every field of every step is encoded, including the ones the
    /// checker uses to *reject* — because an encoding that drops what makes a
    /// step falsifiable would make every stored certificate valid.
    ///
    /// Booleans and indices become literals, source lists become `seq`. Nothing
    /// here needs a new node kind; a derivation was always expressible in the
    /// language it reasons about, and only the encoding was missing.
    pub fn to_term(&self, g: &mut ObjectGraph) -> ObjectId {
        let steps: Vec<ObjectId> = self.steps.iter().map(|s| encode_step(g, s)).collect();
        g.apply(wk::PROOF, steps)
    }

    /// Decode. `None` for anything that is not a well-formed proof term —
    /// **never a partial certificate**, because a certificate missing the steps
    /// its later ones cite is one the kernel would reject for the wrong reason.
    pub fn from_term(g: &ObjectGraph, id: ObjectId) -> Option<Certificate> {
        let CoreNode::Apply { operator, operands } = g.get(id)? else {
            return None;
        };
        if *operator != wk::PROOF {
            return None;
        }
        let mut steps = Vec::with_capacity(operands.len());
        for o in operands {
            steps.push(decode_step(g, *o)?);
        }
        Some(Certificate { steps })
    }
}

fn seq_of(g: &mut ObjectGraph, ids: &[ObjectId]) -> ObjectId {
    g.apply(wk::SEQ, ids.to_vec())
}

fn read_seq(g: &ObjectGraph, id: ObjectId) -> Option<Vec<ObjectId>> {
    match g.get(id)? {
        CoreNode::Apply { operator, operands } if *operator == wk::SEQ => Some(operands.clone()),
        _ => None,
    }
}

fn read_bool(g: &ObjectGraph, id: ObjectId) -> Option<bool> {
    match g.get(id)? {
        CoreNode::Literal(LiteralValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn read_index(g: &ObjectGraph, id: ObjectId) -> Option<usize> {
    match g.get(id)? {
        CoreNode::Literal(LiteralValue::Int(n)) => usize::try_from(n.clone()).ok(),
        _ => None,
    }
}

fn encode_step(g: &mut ObjectGraph, s: &Step) -> ObjectId {
    let mut parts: Vec<ObjectId> = Vec::new();
    match s {
        Step::Told { node, holds, sources } => {
            let (h, src) = (g.boolean(*holds), seq_of(g, sources));
            parts.extend([wk::BY_TOLD, *node, h, src]);
        }
        Step::Axiom { node, holds } => {
            let h = g.boolean(*holds);
            parts.extend([wk::BY_AXIOM, *node, h]);
        }
        Step::Tautology { node, holds, constructive } => {
            let (h, c) = (g.boolean(*holds), g.boolean(*constructive));
            parts.extend([wk::BY_TAUTOLOGY, *node, h, c]);
        }
        Step::Negation { node, premise } => {
            let p = g.int(*premise as i64);
            parts.extend([wk::BY_NEGATION, *node, p]);
        }
        Step::Connective { node, premises, holds } => {
            let ps: Vec<ObjectId> = premises.iter().map(|k| g.int(*k as i64)).collect();
            let (ps, h) = (seq_of(g, &ps), g.boolean(*holds));
            parts.extend([wk::BY_CONNECTIVE, *node, ps, h]);
        }
        Step::Instance { node, premise, value, instance, holds } => {
            let (p, h) = (g.int(*premise as i64), g.boolean(*holds));
            parts.extend([wk::BY_INSTANCE, *node, p, *value, *instance, h]);
        }
        Step::Exhaustive { node, premises, holds, complete } => {
            // Each premise is a triple, and it stays a triple: flattening them
            // into one list would make `(k, value, instance)` recoverable only
            // by arithmetic on the length, and a mis-grouped decode produces a
            // *valid-looking* certificate for a different claim.
            let triples: Vec<ObjectId> = premises
                .iter()
                .map(|(k, v, i)| {
                    let k = g.int(*k as i64);
                    seq_of(g, &[k, *v, *i])
                })
                .collect();
            let (ps, h, c) = (seq_of(g, &triples), g.boolean(*holds), g.boolean(*complete));
            parts.extend([wk::BY_EXHAUSTIVE, *node, ps, h, c]);
        }
        Step::Rule { node, rule, premises, defeasible } => {
            let ps: Vec<ObjectId> = premises.iter().map(|k| g.int(*k as i64)).collect();
            let (ps, d) = (seq_of(g, &ps), g.boolean(*defeasible));
            parts.extend([wk::BY_RULE, *node, *rule, ps, d]);
        }
        Step::Conflict { node, sources } => {
            let src = seq_of(g, sources);
            parts.extend([wk::BY_CONFLICT, *node, src]);
        }
        Step::Ungrounded { node, oscillating } => {
            let o = g.boolean(*oscillating);
            parts.extend([wk::BY_UNGROUNDED, *node, o]);
        }
        Step::Unestablished { node } => parts.extend([wk::BY_UNESTABLISHED, *node]),
    }
    g.apply(wk::STEP, parts)
}

fn decode_step(g: &ObjectGraph, id: ObjectId) -> Option<Step> {
    let CoreNode::Apply { operator, operands } = g.get(id)? else {
        return None;
    };
    if *operator != wk::STEP {
        return None;
    }
    let a = operands.as_slice();
    match a {
        [k, node, holds, sources] if *k == wk::BY_TOLD => Some(Step::Told {
            node: *node,
            holds: read_bool(g, *holds)?,
            sources: read_seq(g, *sources)?,
        }),
        [k, node, holds] if *k == wk::BY_AXIOM => {
            Some(Step::Axiom { node: *node, holds: read_bool(g, *holds)? })
        }
        [k, node, holds, constructive] if *k == wk::BY_TAUTOLOGY => Some(Step::Tautology {
            node: *node,
            holds: read_bool(g, *holds)?,
            constructive: read_bool(g, *constructive)?,
        }),
        [k, node, premise] if *k == wk::BY_NEGATION => {
            Some(Step::Negation { node: *node, premise: read_index(g, *premise)? })
        }
        [k, node, premises, holds] if *k == wk::BY_CONNECTIVE => Some(Step::Connective {
            node: *node,
            premises: read_seq(g, *premises)?
                .iter()
                .map(|p| read_index(g, *p))
                .collect::<Option<Vec<_>>>()?,
            holds: read_bool(g, *holds)?,
        }),
        [k, node, premise, value, instance, holds] if *k == wk::BY_INSTANCE => {
            Some(Step::Instance {
                node: *node,
                premise: read_index(g, *premise)?,
                value: *value,
                instance: *instance,
                holds: read_bool(g, *holds)?,
            })
        }
        [k, node, premises, holds, complete] if *k == wk::BY_EXHAUSTIVE => {
            let mut ps = Vec::new();
            for t in read_seq(g, *premises)? {
                let [idx, value, instance] = read_seq(g, t)?[..] else {
                    return None;
                };
                ps.push((read_index(g, idx)?, value, instance));
            }
            Some(Step::Exhaustive {
                node: *node,
                premises: ps,
                holds: read_bool(g, *holds)?,
                complete: read_bool(g, *complete)?,
            })
        }
        [k, node, rule, premises, defeasible] if *k == wk::BY_RULE => Some(Step::Rule {
            node: *node,
            rule: *rule,
            premises: read_seq(g, *premises)?
                .iter()
                .map(|p| read_index(g, *p))
                .collect::<Option<Vec<_>>>()?,
            defeasible: read_bool(g, *defeasible)?,
        }),
        [k, node, sources] if *k == wk::BY_CONFLICT => {
            Some(Step::Conflict { node: *node, sources: read_seq(g, *sources)? })
        }
        [k, node, oscillating] if *k == wk::BY_UNGROUNDED => {
            Some(Step::Ungrounded { node: *node, oscillating: read_bool(g, *oscillating)? })
        }
        [k, node] if *k == wk::BY_UNESTABLISHED => Some(Step::Unestablished { node: *node }),
        _ => None,
    }
}

/// Is `instance` the result of replacing the bound variable of `body` with
/// `value`?
///
/// A structural walk over de Bruijn form: wherever `body` has `(bvar 0)` at the
/// current depth, `instance` must have `value`; everywhere else the two must
/// agree, with the depth rising through nested binders. Small, pure, and needing
/// nothing but the graph — which is what lets the kernel check an instantiation
/// without becoming an evaluator.
pub(crate) fn instantiates(
    g: &ObjectGraph,
    body: ObjectId,
    value: ObjectId,
    instance: ObjectId,
    depth: usize,
) -> bool {
    if depth > 32 {
        return false;
    }
    if let Some(k) = g.as_bvar(body) {
        return if k == depth { instance == value } else { instance == body };
    }
    if body == instance {
        // Identical subtrees mention no variable at this level.
        return true;
    }
    match (g.get(body), g.get(instance)) {
        (
            Some(CoreNode::Apply { operator: o1, operands: a1 }),
            Some(CoreNode::Apply { operator: o2, operands: a2 }),
        ) => {
            o1 == o2
                && a1.len() == a2.len()
                && a1.iter().zip(a2).all(|(x, y)| instantiates(g, *x, value, *y, depth))
        }
        (
            Some(CoreNode::Bind { binder: b1, vars: v1, bodies: d1 }),
            Some(CoreNode::Bind { binder: b2, vars: v2, bodies: d2 }),
        ) => {
            b1 == b2
                && v1 == v2
                && d1.len() == d2.len()
                && d1
                    .iter()
                    .zip(d2)
                    .all(|(x, y)| instantiates(g, *x, value, *y, depth + v1.len()))
        }
        _ => false,
    }
}

fn base(node: ObjectId, holds: bool, determinacy: Determinacy) -> Checked {
    Checked {
        node,
        support: if holds { Bound::Certain } else { Bound::None },
        refutation: if holds { Bound::None } else { Bound::Certain },
        grounding: Grounding::Grounded,
        derivation: Derivation::Observed,
        determinacy,
        authorities: Vec::new(),
        assumed: Vec::new(),
    }
}

fn decided(node: ObjectId, holds: bool) -> Checked {
    base(node, holds, Determinacy::Total)
}

/// The axes travel through a checked step exactly as they do through an
/// evaluated one: a conclusion is no better grounded, no less defeasible and no
/// more determinate than what it rests on.
fn absorb(acc: &mut Checked, p: &Checked) {
    acc.grounding = acc.grounding.merge(p.grounding);
    acc.derivation = acc.derivation.merge(p.derivation);
    acc.determinacy = acc.determinacy.merge(p.determinacy);
    for a in &p.authorities {
        if !acc.authorities.contains(a) {
            acc.authorities.push(*a);
        }
    }
    for a in &p.assumed {
        if !acc.assumed.contains(a) {
            acc.assumed.push(*a);
        }
    }
}

/// The propositional skeleton of `node`, and how many distinct atoms it has.
///
/// Shared with the evaluator rather than duplicated. The de Bruijn principle is
/// that the checker must not call *evaluation* — a pure decision procedure over
/// syntax is a different thing, and one implementation both parties agree on is
/// better than two that might not.
fn shape_of(g: &ObjectGraph, node: ObjectId) -> Option<(Skeleton, usize)> {
    let mut atoms = Vec::new();
    let shape = skeleton(g, node, &mut atoms, 0)?;
    Some((shape, atoms.len()))
}

fn skeleton(
    g: &ObjectGraph,
    node: ObjectId,
    atoms: &mut Vec<ObjectId>,
    depth: u32,
) -> Option<Skeleton> {
    if depth > 32 {
        return None;
    }
    if node == wk::TOP {
        return Some(Skeleton::Const(true));
    }
    if node == wk::BOT {
        return Some(Skeleton::Const(false));
    }
    if let Some(CoreNode::Apply { operator, operands }) = g.get(node) {
        let parts = |atoms: &mut Vec<ObjectId>| -> Option<Vec<Skeleton>> {
            operands.iter().map(|o| skeleton(g, *o, atoms, depth + 1)).collect()
        };
        match *operator {
            wk::NOT if operands.len() == 1 => {
                return Some(Skeleton::Not(Box::new(parts(atoms)?.remove(0))));
            }
            wk::AND => return Some(Skeleton::And(parts(atoms)?)),
            wk::OR => return Some(Skeleton::Or(parts(atoms)?)),
            wk::IMPLIES if operands.len() == 2 => {
                let mut p = parts(atoms)?;
                let conseq = p.remove(1);
                let ante = p.remove(0);
                return Some(Skeleton::Imp(Box::new(ante), Box::new(conseq)));
            }
            _ => {}
        }
    }
    let idx = match atoms.iter().position(|a| *a == node) {
        Some(i) => i,
        None => {
            atoms.push(node);
            atoms.len() - 1
        }
    };
    Some(Skeleton::Atom(idx))
}
