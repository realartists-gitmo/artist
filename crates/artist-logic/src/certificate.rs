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
    ///
    /// **Soundness.** `R_t(n)` is derived from live testimony (semantics §2), so
    /// a `Told` step asserts that the structure's testimony decides `n`. The
    /// kernel cannot see `T`, which is precisely why this is the trust point;
    /// what it *can* do is refuse to let the trust go unrecorded. Per §8, a
    /// source counts only if it names an object the graph actually holds —
    /// otherwise `assumed` is not "nobody vouched" but "the vector was
    /// non-empty", which any producer satisfies with one arbitrary integer.
    Told {
        node: ObjectId,
        holds: bool,
        sources: Vec<ObjectId>,
    },
    /// A truth atom. `⟦#true⟧ = T` and `⟦#false⟧ = F` in every structure, so this
    /// is the only rule that introduces nothing into `Γ` (semantics §7.1, §10).
    ///
    /// It is **not** the only source of an unconditional judgment, and saying so
    /// was wrong twice: `Ungrounded` consults no testimony either, and any
    /// composition over axioms — `Negation` of one, a `Connective` of several —
    /// ends with `Γ` empty. Unconditionality is a property of `Γ`, not a
    /// privilege of a rule.
    ///
    /// It replaces the `Tautology` rule, which enumerated *Boolean* valuations
    /// under a semantics that is Belnap's. A classical tautology need not be `T`
    /// in FOUR: `P ∨ ¬P` at `N` is `N ∨ N = N`, and even `P → P` at `N` is
    /// `¬N ∨ N = N`. The old rule therefore certified formulas this logic does
    /// not validate. A tautology rule for FOUR would have to enumerate FOUR
    /// valuations, and the set it could license is nearly empty — so the rule is
    /// withdrawn rather than narrowed, and its `constructive` flag with it: that
    /// flag distinguished two *classical* notions inside a logic where neither
    /// applies.
    Axiom { node: ObjectId, holds: bool },
    /// `(not P)` from `P`, with the bounds swapped.
    Negation { node: ObjectId, premise: usize },
    /// A conjunct decides a conjunction false; a disjunct decides a disjunction
    /// true. Records which.
    ///
    /// Each premise names the **operand position** it covers, not merely a node.
    /// Under content addressing `(implies P P)` has one id in both positions, so
    /// identifying an operand by its node let a single premise occupy the
    /// antecedent *and* the consequent at the antecedent's polarity — and
    /// `P → P`, a classical validity, certified as refuted. Position is what the
    /// rule quantifies over, so position is what the step must carry. This is the
    /// same repair as [`Step::Instance`]'s `instance` field: not a missing check,
    /// missing *information*, without which no check was possible.
    ///
    /// **Soundness.** `∧` and `∨` are meet and join in FOUR's truth order
    /// (semantics §3), so: one operand at `F` puts a conjunction at `≤_t F`; one
    /// at `T` puts a disjunction at `≥_t T`; and all operands agreeing decides
    /// the other direction. `implies` is `∨` with operand 0 negated, and
    /// negation is the involution swapping `T`/`F`.
    Connective {
        node: ObjectId,
        premises: Vec<(usize, usize)>,
        holds: bool,
    },
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
    /// complete.
    ///
    /// Completeness is **derived from the binder's domain node**, never asserted
    /// by the step. It used to be a `bool` the kernel read and believed, which is
    /// the one thing a kernel may not do: flipping it certified a universal from
    /// one member of a two-member domain. The fact is `S(σ).complete` in the
    /// semantics (§2) and `(set …)` versus `(set-partial …)` in the graph — it
    /// was sitting in the node the kernel already held.
    ///
    /// Each premise is paired with the member it is about, and each must be the
    /// binder's body under that member — same reason as [`Step::Instance`].
    ///
    /// **Soundness.** A universal over a complete, finite domain is the meet of
    /// its instances and an existential is their join (semantics §4). Given every
    /// member covered and every instance certain, the meet or join is decided.
    Exhaustive {
        node: ObjectId,
        premises: Vec<(usize, ObjectId, ObjectId)>,
        holds: bool,
    },
    /// A stored rule fired. Defeasible rules mark it, so a reader can see that
    /// the conclusion holds absent a defeater rather than outright.
    ///
    /// **Modus ponens, and it needs no rule base.** The structure has none: no
    /// defeat relation, no priorities, no applicability order. It does not need
    /// them, because a stored rule is an ordinary universally quantified
    /// implication. `implication` is a premise establishing `⟦A → C⟧ ≥_t T`;
    /// `antecedent` is a premise establishing `A`; the conclusion is `C`. That is
    /// sound by §3 alone — `→` is `¬a ∨ b` — and it puts the rule's own truth
    /// into `Γ`, so a reader who rejects the rule rejects the conclusion.
    ///
    /// Defeasible rules are **not** representable here. Capping them at
    /// `Bound::Partial` was not a fix: by §7.2 `Partial` asserts nothing about
    /// the structure, so a "defeasible conclusion" would carry no information
    /// while looking as though it did. Certifying defeat needs the combined
    /// grounding/argumentation fixpoint, which §12 records as undefined. Without
    /// it, a "defeasible conclusion" is not certifiable at all, and saying so is
    /// the honest position.
    ///
    /// **Soundness.** `→` is Arieli–Avron's strong implication (semantics §3),
    /// so `A ⊃ C = C` whenever `A` is designated. A `support: Certain` bound
    /// asserts exactly designation — `⟦·⟧ ∈ {T,B}` (§7.2) — so from both premises
    /// designated, `⟦A ⊃ C⟧ = ⟦C⟧`, hence `C` is designated. Under
    /// `Γ = Γ_impl ∪ Γ_ante`.
    ///
    /// Read **materially** this rule is unsound, and the counterexample is
    /// small: `A = B`, `C = F` gives `¬B ∨ F = B ∨ F = B`, so the implication and
    /// the antecedent are both designated while the consequent is not. That the
    /// bounds cannot separate `T` from `B` does not matter here, because the
    /// strong implication treats them identically — which is the point of using
    /// it rather than patching detachment with a side condition the judgment
    /// type cannot express.
    ModusPonens {
        node: ObjectId,
        implication: usize,
        antecedent: usize,
    },
    /// The sentence has no stable value — its own truth is among its premises.
    ///
    /// Whether it **oscillates** used to be a `bool` the kernel copied into its
    /// answer. It is now derived: the kernel walks the reference chain from
    /// `node` — through `not`, `holds` and `quote`, the single-operand ways a
    /// sentence mentions another — and requires that it return to `node`. The
    /// verdict is then the parity of the negations around that loop,
    /// which is exactly semantics §4's criterion for the restricted shape this
    /// rule admits — an odd number forces `v(n) = ¬v(n)`, so no fixpoint gives it
    /// a classical value (the liar); an even number leaves classical fixpoints
    /// available (the truth-teller). Loops running through branching connectives
    /// are **refused** rather than guessed at, because parity is not a complete
    /// criterion for them and a kernel may not approximate.
    Ungrounded { node: ObjectId },
    /// Universal instantiation: from `∀x ∈ σ. φ`, conclude `φ[value/x]`.
    ///
    /// The converse direction of [`Step::Instance`], which goes instance to
    /// binder. Without it a stored rule `∀x⃗. A → C` could fire in the evaluator
    /// and never be certified, because [`Step::ModusPonens`] needs the
    /// *instantiated* implication and nothing produced one — so every conclusion
    /// drawn from a stored rule left the certificate unable to conclude its own
    /// root, and `eval_traced` discarded it. Rules are most of what an agent's
    /// memory holds, so that gap mattered more than its size suggested.
    ///
    /// **Soundness.** `∀x ∈ σ. φ` denotes the meet of its instances over `ext`
    /// (semantics §2, §4.3), and a designated meet forces every instance
    /// designated: `F ∧ x = F` and `N ∧ T = N` are both undesignated, so a single
    /// undesignated instance would drag the meet below. Membership is checked
    /// against `enum`, and `enum ⊆ ext`, so this holds over **partially**
    /// enumerable domains too — unlike [`Step::Exhaustive`], which needs
    /// `enum = ext` because it reasons in the other direction.
    Instantiate {
        node: ObjectId,
        premise: usize,
        value: ObjectId,
    },
}

impl Step {
    pub fn node(&self) -> ObjectId {
        match self {
            Step::Told { node, .. }
            | Step::Axiom { node, .. }
            | Step::Negation { node, .. }
            | Step::Connective { node, .. }
            | Step::Instance { node, .. }
            | Step::Exhaustive { node, .. }
            | Step::ModusPonens { node, .. }
            | Step::Ungrounded { node, .. }
            | Step::Instantiate { node, .. } => *node,
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
    /// **Γ** — the testimony leaves this derivation actually used.
    ///
    /// A certificate proves a *conditional* judgment (semantics §7.1): its bounds
    /// hold in every structure satisfying these hypotheses, and in no others. A
    /// kernel with a trust point cannot claim more, and revision 1 of the spec
    /// claimed unconditional soundness while `Told` accepted testimony it could
    /// not verify — recording an assumption does not make it true.
    ///
    /// Carrying Γ explicitly is also what makes §8.5's provenance report an
    /// *equality* rather than a subset relation. Under `authorities ⊆ att(n)`
    /// alone, a derivation resting entirely on verified testimony could name
    /// nobody and still pass. Now `authorities` and `assumed` partition Γ, so
    /// **both empty means `Γ` is empty**, which only [`Step::Axiom`] can produce.
    pub hypotheses: Vec<ObjectId>,
}

/// Why a certificate was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalid {
    Empty,
    /// A premise index pointing forward, or past the end.
    BadReference {
        step: usize,
    },
    /// The step's shape does not match the node it claims to conclude.
    Mismatched {
        step: usize,
    },
    /// The step's rule does not license its conclusion from its premises.
    Unlicensed {
        step: usize,
    },
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
                Step::Told {
                    node,
                    holds,
                    sources,
                } => {
                    // An authority must name something the graph actually holds.
                    // `assumed` means *no source was verified* — not *the vector
                    // was non-empty*, which was satisfiable with one arbitrary
                    // integer, and which made an unattributed claim
                    // indistinguishable from an attributed one to every reader.
                    let named: Vec<ObjectId> = sources
                        .iter()
                        .copied()
                        .filter(|s| g.get(*s).is_some())
                        .collect();
                    Checked {
                        node: *node,
                        // Both bits settled: a structure that says a fact holds
                        // has told you its refutation bit is zero.
                        support: if *holds {
                            Bound::Certain
                        } else {
                            Bound::Excluded
                        },
                        refutation: if *holds {
                            Bound::Excluded
                        } else {
                            Bound::Certain
                        },
                        grounding: Grounding::Grounded,
                        derivation: Derivation::Observed,
                        determinacy: Determinacy::Unknown,
                        assumed: if named.is_empty() {
                            vec![*node]
                        } else {
                            Vec::new()
                        },
                        authorities: named,
                        // This testimony is a hypothesis of everything downstream.
                        hypotheses: vec![*node],
                    }
                }
                Step::Axiom { node, holds } => {
                    let expected = if *holds { wk::TOP } else { wk::BOT };
                    if *node != expected {
                        return Err(Invalid::Mismatched { step: i });
                    }
                    // Γ empty: the only unconditional judgment in the system.
                    base(*node, *holds, Determinacy::Total)
                }
                Step::ModusPonens {
                    node,
                    implication,
                    antecedent,
                } => {
                    let imp = premise(*implication)?;
                    let ante = premise(*antecedent)?;
                    // The implication premise must *be* `A → node`, with `A` the
                    // antecedent premise. Nothing is taken on the step's word.
                    match g.get(imp.node) {
                        Some(CoreNode::Apply { operator, operands })
                            if *operator == wk::IMPLIES
                                && operands.len() == 2
                                && operands[0] == ante.node
                                && operands[1] == *node => {}
                        _ => return Err(Invalid::Mismatched { step: i }),
                    }
                    if imp.support != Bound::Certain || ante.support != Bound::Certain {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    let mut acc = decided(*node, true);
                    acc.derivation = Derivation::Derived;
                    absorb(&mut acc, imp);
                    absorb(&mut acc, ante);
                    acc
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
                Step::Connective {
                    node,
                    premises,
                    holds,
                } => {
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
                    // Arity, before anything reads a position. `(implies)` with
                    // no operands made the coverage loop vacuous and certified
                    // `refutation: Certain` from an empty premise list —
                    // certainty out of nothing, with no authority and no
                    // assumption recorded.
                    let arity_ok = match op {
                        wk::IMPLIES => operands.len() == 2,
                        _ => !operands.is_empty(),
                    };
                    if !arity_ok {
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
                    // Coverage is by **position**, so a repeated operand is two
                    // distinct obligations. `(implies P P)` has one node in both
                    // slots; tracking nodes let one premise discharge both, at
                    // the antecedent's polarity, and certified `P → P` false.
                    let mut covered = vec![false; operands.len()];
                    for (k, pos) in premises {
                        let p = premise(*k)?;
                        if *pos >= operands.len() || operands[*pos] != p.node {
                            return Err(Invalid::Mismatched { step: i });
                        }
                        // `implies` negates its antecedent, so position zero
                        // points the other way — a fact about the *slot*, which
                        // is why it can only be read once slots are tracked.
                        // `implies` reads its antecedent's support bit, and
                        // reads it *negated*: `(a ⊃ b)⁺ = ¬a⁺ ∨ b⁺` and
                        // `(a ⊃ b)⁻ = a⁺ ∧ b⁻`. So position 0 is licensed by
                        // `a⁺ = 0` when concluding true, and by `a⁺ = 1` when
                        // concluding false — never by the antecedent's
                        // *refutation*, which was the old condition and admits
                        // the designated `B`.
                        let negated = op == wk::IMPLIES && *pos == 0;
                        let licensed = if negated {
                            if *holds {
                                p.support.is_excluded()
                            } else {
                                p.support.is_certain()
                            }
                        } else if *holds {
                            p.support.is_certain()
                        } else {
                            p.refutation.is_certain()
                        };
                        if !licensed {
                            return Err(Invalid::Unlicensed { step: i });
                        }
                        covered[*pos] = true;
                        absorb(&mut acc, p);
                    }
                    if one_decides {
                        if !covered.iter().any(|c| *c) {
                            return Err(Invalid::Unlicensed { step: i });
                        }
                    } else if covered.iter().any(|c| !*c) {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    acc
                }
                Step::Instance {
                    node,
                    premise: k,
                    value,
                    instance,
                    holds,
                } => {
                    let p = premise(*k)?;
                    let (binder, body, dom) =
                        binder_parts(g, *node).ok_or(Invalid::Mismatched { step: i })?;
                    // A witness settles an existential true; a counterexample
                    // settles a universal false. The other two combinations are
                    // not licensed by a single member.
                    if !matches!((binder, holds), (wk::EXISTS, true) | (wk::FORALL, false)) {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    // **The member must be in the domain.** The binder's `vars`
                    // were destructured and thrown away, so `∃x ∈ {a,b}. P(x)`
                    // was certified from `P(c)` — a witness from outside the set
                    // it quantifies over. Membership is a side condition of the
                    // rule, and the domain is in the node the kernel holds. A
                    // domain it cannot enumerate is refused rather than assumed:
                    // a kernel that cannot check must not pass.
                    let (members, _) = domain_of(g, dom).ok_or(Invalid::Unlicensed { step: i })?;
                    if !members.contains(value) {
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
                Step::Exhaustive {
                    node,
                    premises,
                    holds,
                } => {
                    let (binder, body, dom) =
                        binder_parts(g, *node).ok_or(Invalid::Mismatched { step: i })?;
                    if !matches!(binder, wk::FORALL | wk::EXISTS) {
                        return Err(Invalid::Mismatched { step: i });
                    }
                    // **Completeness is read, not accepted.** This was a `bool`
                    // on the step; flipping it certified a universal from one
                    // member of a two-member domain, and the fact was in the
                    // domain node all along — `(set …)` is complete by
                    // construction, `(set-partial …)` is not.
                    let (members, complete) =
                        domain_of(g, dom).ok_or(Invalid::Unlicensed { step: i })?;
                    if !complete || members.is_empty() || premises.is_empty() {
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
                        if !members.contains(value) {
                            return Err(Invalid::Unlicensed { step: i });
                        }
                        seen.push(*value);
                        let want = if *holds { p.support } else { p.refutation };
                        if want != Bound::Certain {
                            return Err(Invalid::Unlicensed { step: i });
                        }
                        absorb(&mut acc, p);
                    }
                    // Agreement among the members you happened to cite licenses
                    // nothing. Every member of the domain must be covered.
                    if members.iter().any(|m| !seen.contains(m)) {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    acc
                }
                Step::Instantiate {
                    node,
                    premise: k,
                    value,
                } => {
                    let p = premise(*k)?;
                    let (binder, body, dom) =
                        binder_parts(g, p.node).ok_or(Invalid::Mismatched { step: i })?;
                    if binder != wk::FORALL {
                        return Err(Invalid::Mismatched { step: i });
                    }
                    // `node` must be exactly the body under `value` — the same
                    // information `Instance` carries for the same reason: without
                    // it, nothing ties the conclusion to the premise.
                    if !instantiates(g, body, *value, *node, 0) {
                        return Err(Invalid::Mismatched { step: i });
                    }
                    // `enum ⊆ ext`, so an enumerated member is a real member and
                    // completeness is irrelevant here.
                    let (members, _) = domain_of(g, dom).ok_or(Invalid::Unlicensed { step: i })?;
                    if !members.contains(value) || p.support != Bound::Certain {
                        return Err(Invalid::Unlicensed { step: i });
                    }
                    let mut acc = decided(*node, true);
                    absorb(&mut acc, p);
                    acc
                }
                Step::Ungrounded { node } => {
                    // Parity of the reference loop, walked rather than believed.
                    let oscillating =
                        loop_parity(g, *node).ok_or(Invalid::Unlicensed { step: i })?;
                    Checked {
                        node: *node,
                        support: Bound::None,
                        refutation: Bound::None,
                        grounding: if oscillating {
                            Grounding::Oscillatory
                        } else {
                            Grounding::StableLoop
                        },
                        derivation: Derivation::Observed,
                        determinacy: Determinacy::Unknown,
                        authorities: Vec::new(),
                        assumed: Vec::new(),
                        // Ungroundedness is a fact about the *sentence*, decided
                        // by the graph alone, so it is conditional on nothing.
                        hypotheses: Vec::new(),
                    }
                }
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
                // **They must not disagree**, in either direction.
                //
                // This was `cert ≤ result` on the `Ord`, meaning "the
                // certificate may prove less but never more". Two things broke
                // that. `Ord` is not the information order — it exists to make
                // `meet`/`join` Kleene, so it runs `Excluded < None < Certain` —
                // and, more substantially, the certificate can now be *more*
                // informative than the evaluator: `Told{holds:false}` settles
                // both bits, while a scan suppresses its meet side whenever the
                // enumeration is not exact. A correct certificate was failing a
                // check written when the evaluator was always the stronger of
                // the two.
                //
                // What matters for the de Bruijn criterion is that they never
                // *conflict*: a certificate concluding `Supported` for a result
                // that is `Refuted` is the failure this exists to catch, and
                // one settling a bit the evaluator left open is not.
                let compatible = |c: Bound, r: Bound| !c.is_settled() || !r.is_settled() || c == r;
                c.node == node
                    && compatible(c.support, result.support)
                    && compatible(c.refutation, result.refutation)
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
        Step::Told {
            node,
            holds,
            sources,
        } => {
            let (h, src) = (g.boolean(*holds), seq_of(g, sources));
            parts.extend([wk::BY_TOLD, *node, h, src]);
        }
        Step::Axiom { node, holds } => {
            let h = g.boolean(*holds);
            parts.extend([wk::BY_AXIOM, *node, h]);
        }
        Step::Negation { node, premise } => {
            let p = g.int(*premise as i64);
            parts.extend([wk::BY_NEGATION, *node, p]);
        }
        Step::Connective {
            node,
            premises,
            holds,
        } => {
            // Premise and operand position stay paired, for the same reason
            // `Exhaustive`'s triples do: flattening makes the grouping
            // recoverable only by arithmetic, and a mis-grouped decode yields a
            // valid-looking certificate for a different claim.
            let pairs: Vec<ObjectId> = premises
                .iter()
                .map(|(k, pos)| {
                    let (k, pos) = (g.int(*k as i64), g.int(*pos as i64));
                    seq_of(g, &[k, pos])
                })
                .collect();
            let (ps, h) = (seq_of(g, &pairs), g.boolean(*holds));
            parts.extend([wk::BY_CONNECTIVE, *node, ps, h]);
        }
        Step::Instance {
            node,
            premise,
            value,
            instance,
            holds,
        } => {
            let (p, h) = (g.int(*premise as i64), g.boolean(*holds));
            parts.extend([wk::BY_INSTANCE, *node, p, *value, *instance, h]);
        }
        Step::Exhaustive {
            node,
            premises,
            holds,
        } => {
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
            let (ps, h) = (seq_of(g, &triples), g.boolean(*holds));
            parts.extend([wk::BY_EXHAUSTIVE, *node, ps, h]);
        }
        Step::ModusPonens {
            node,
            implication,
            antecedent,
        } => {
            let (imp, ante) = (g.int(*implication as i64), g.int(*antecedent as i64));
            parts.extend([wk::BY_RULE, *node, imp, ante]);
        }
        Step::Ungrounded { node } => parts.extend([wk::BY_UNGROUNDED, *node]),
        Step::Instantiate {
            node,
            premise,
            value,
        } => {
            let k = g.int(*premise as i64);
            parts.extend([wk::BY_INSTANTIATE, *node, k, *value]);
        }
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
        [k, node, holds] if *k == wk::BY_AXIOM => Some(Step::Axiom {
            node: *node,
            holds: read_bool(g, *holds)?,
        }),
        [k, node, premise] if *k == wk::BY_NEGATION => Some(Step::Negation {
            node: *node,
            premise: read_index(g, *premise)?,
        }),
        [k, node, premises, holds] if *k == wk::BY_CONNECTIVE => {
            let mut ps = Vec::new();
            for t in read_seq(g, *premises)? {
                // Exactly two, enforced by the pattern: a pair that decoded from
                // a longer or shorter list would silently re-associate premises
                // with the wrong operand slots.
                let [idx, pos] = read_seq(g, t)?[..] else {
                    return None;
                };
                ps.push((read_index(g, idx)?, read_index(g, pos)?));
            }
            Some(Step::Connective {
                node: *node,
                premises: ps,
                holds: read_bool(g, *holds)?,
            })
        }
        [k, node, premise, value, instance, holds] if *k == wk::BY_INSTANCE => {
            Some(Step::Instance {
                node: *node,
                premise: read_index(g, *premise)?,
                value: *value,
                instance: *instance,
                holds: read_bool(g, *holds)?,
            })
        }
        [k, node, premises, holds] if *k == wk::BY_EXHAUSTIVE => {
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
            })
        }
        [k, node, imp, ante] if *k == wk::BY_RULE => Some(Step::ModusPonens {
            node: *node,
            implication: read_index(g, *imp)?,
            antecedent: read_index(g, *ante)?,
        }),
        [k, node] if *k == wk::BY_UNGROUNDED => Some(Step::Ungrounded { node: *node }),
        [k, node, premise, value] if *k == wk::BY_INSTANTIATE => Some(Step::Instantiate {
            node: *node,
            premise: read_index(g, *premise)?,
            value: *value,
        }),
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
        return if k == depth {
            instance == value
        } else {
            instance == body
        };
    }
    if body == instance {
        // Identical subtrees mention no variable at this level.
        return true;
    }
    match (g.get(body), g.get(instance)) {
        (
            Some(CoreNode::Apply {
                operator: o1,
                operands: a1,
            }),
            Some(CoreNode::Apply {
                operator: o2,
                operands: a2,
            }),
        ) => {
            o1 == o2
                && a1.len() == a2.len()
                && a1
                    .iter()
                    .zip(a2)
                    .all(|(x, y)| instantiates(g, *x, value, *y, depth))
        }
        (
            Some(CoreNode::Bind {
                binder: b1,
                vars: v1,
                bodies: d1,
            }),
            Some(CoreNode::Bind {
                binder: b2,
                vars: v2,
                bodies: d2,
            }),
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

/// A binder's shape: its kind, its first body, and its first slot's domain.
///
/// The old arms destructured `Bind { binder, bodies, .. }` and dropped `vars` —
/// and with it every domain, which is why a witness from outside the quantified
/// set proved an existential. Returning the domain here is what lets the rules
/// state membership as a side condition instead of omitting it.
fn binder_parts(g: &ObjectGraph, node: ObjectId) -> Option<(ObjectId, ObjectId, ObjectId)> {
    match g.get(node) {
        Some(CoreNode::Bind {
            binder,
            vars,
            bodies,
        }) if vars.len() == 1 => Some((*binder, *bodies.first()?, vars[0].domain?)),
        _ => None,
    }
}

/// Members of an enumerated domain, and whether the enumeration is complete.
///
/// `(set …)` is complete by construction; `(set-partial …)` is explicitly not.
/// Anything else is not enumerable by inspection, and the kernel returns `None`
/// so the caller refuses — the alternative is assuming a completeness it cannot
/// see, which is the defect this function exists to remove.
fn domain_of(g: &ObjectGraph, dom: ObjectId) -> Option<(Vec<ObjectId>, bool)> {
    match g.get(dom) {
        Some(CoreNode::Apply { operator, operands }) if *operator == wk::SET_DOMAIN => {
            Some((operands.clone(), true))
        }
        Some(CoreNode::Apply { operator, operands }) if *operator == wk::SET_PARTIAL => {
            Some((operands.clone(), false))
        }
        _ => None,
    }
}

/// Walk the reference loop from `node` and report whether it oscillates.
///
/// `Some(true)` for an odd number of negations around the loop — `v(n) = ¬v(n)`,
/// which no classical fixpoint satisfies, so the sentence is `Oscillatory`.
/// `Some(false)` for an even number: classical fixpoints exist and the sentence
/// is a `StableLoop`. `None` if the chain leaves `node`'s reference cycle, runs
/// through a branching connective, or exceeds its bound — parity decides nothing
/// there, and a kernel that guesses is a kernel that is sometimes wrong.
pub(crate) fn loop_parity(g: &ObjectGraph, node: ObjectId) -> Option<bool> {
    let mut at = node;
    let mut negations = 0usize;
    for _ in 0..64 {
        let (op, operands) = match g.get(at) {
            Some(CoreNode::Apply { operator, operands }) => (*operator, operands),
            _ => return None,
        };
        // Exactly the single-operand ways one sentence mentions another.
        if !matches!(op, wk::NOT | wk::HOLDS | wk::QUOTE) || operands.len() != 1 {
            return None;
        }
        if op == wk::NOT {
            negations += 1;
        }
        at = operands[0];
        if at == node {
            return Some(negations % 2 == 1);
        }
    }
    None
}

fn base(node: ObjectId, holds: bool, determinacy: Determinacy) -> Checked {
    Checked {
        node,
        support: if holds {
            Bound::Certain
        } else {
            Bound::Excluded
        },
        refutation: if holds {
            Bound::Excluded
        } else {
            Bound::Certain
        },
        grounding: Grounding::Grounded,
        derivation: Derivation::Observed,
        determinacy,
        authorities: Vec::new(),
        assumed: Vec::new(),
        hypotheses: Vec::new(),
    }
}

/// A step that decided its node, claiming **nothing** about totality.
///
/// This used to seed `Determinacy::Total`, which is a field the kernel cannot
/// check: totality is `Δ`'s business (semantics §5) and the kernel has no `Δ`.
/// Every connective, instance and exhaustive step therefore asserted a sharp
/// condition it had never seen — the same "believed field" defect as
/// `Exhaustive`'s old `complete` flag, hiding in a helper. `Unknown` is the
/// honest floor; only `Tautology` earns `Total`, from a truth table it computes.
fn decided(node: ObjectId, holds: bool) -> Checked {
    base(node, holds, Determinacy::Unknown)
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
    // Γ accumulates: a conclusion is conditional on every hypothesis anything
    // beneath it leaned on. Dropping one here would let a derivation quietly
    // become unconditional, which is the strongest claim in the system.
    for h in &p.hypotheses {
        if !acc.hypotheses.contains(h) {
            acc.hypotheses.push(*h);
        }
    }
}
