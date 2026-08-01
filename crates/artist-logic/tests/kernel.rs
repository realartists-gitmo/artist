//! The kernel, rule by rule — and every counterexample an auditor found.
//!
//! The interesting tests here are the **negative** ones. That a certificate the
//! evaluator produced checks out proves little; both halves were written by the
//! same hand on the same afternoon. What matters is that a certificate claiming
//! more than its premises license is *rejected*, because that is the only thing
//! standing between "checkable derivation" and "second copy of the evaluator
//! that agrees with the first".
//!
//! Every rejection below was, at some commit, an acceptance. An adversarial
//! audit built each one and watched the kernel certify it. They are here so the
//! kernel cannot quietly relearn them.

use artist_logic::ObjectGraph;
use artist_logic::certificate::{Certificate, Step};
use artist_logic::evidence::{Bound, Grounding};
use artist_logic::object::wk;

fn cert(steps: Vec<Step>) -> Certificate {
    Certificate { steps }
}

/// A source that names nothing the graph holds is **not** an authority.
///
/// `assumed` has to mean *no source was verified*. When it meant *the vector was
/// non-empty*, any producer bought the appearance of provenance with one
/// arbitrary integer, and a reader could not tell a vouched-for claim from an
/// invented one.
#[test]
fn a_source_that_does_not_exist_is_not_an_authority() {
    let mut g = ObjectGraph::new();
    let (p, a, adam) = (g.atom("p"), g.atom("a"), g.atom("adam"));
    let claim = g.apply(p, vec![a]);

    let real = cert(vec![Step::Told { node: claim, holds: true, sources: vec![adam] }])
        .check(&g)
        .expect("valid");
    assert_eq!(real.authorities, vec![adam]);
    assert!(real.assumed.is_empty());

    let forged = cert(vec![Step::Told {
        node: claim,
        holds: true,
        sources: vec![artist_logic::ObjectId(0xdead_beef)],
    }])
    .check(&g)
    .expect("the step is well-formed; the *source* is what is bogus");
    assert!(forged.authorities.is_empty(), "an id nobody minted vouches for nothing");
    assert_eq!(forged.assumed, vec![claim], "and the trust is recorded instead");
}

/// **Γ empty means unconditional.** Semantics §7.1: a certificate proves a
/// conditional judgment, and the hypotheses are the testimony it leaned on. If
/// both `authorities` and `assumed` could be empty for a rule that consulted the
/// structure, "rests on nothing" would be indistinguishable from "rests on
/// something nobody recorded" — which is exactly how dated queries used to
/// certify to nobody.
///
/// `Axiom` is the only *rule* that introduces nothing into `Γ`, but it is not
/// the only derivation ending with `Γ` empty: `Negation` over an axiom, a
/// `Connective` over axioms, and `Ungrounded` are unconditional too, because
/// they consult no testimony. The invariant is about `Γ`, not about a rule.
#[test]
fn an_empty_gamma_means_unconditional() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);

    let axiom = cert(vec![Step::Axiom { node: wk::TOP, holds: true }]).check(&g).expect("valid");
    assert!(axiom.hypotheses.is_empty(), "true in every structure");
    assert!(axiom.authorities.is_empty() && axiom.assumed.is_empty());

    // …and so is a derivation built over axioms, which consults no testimony
    // either. Unconditionality is a property of Γ, not a privilege of one rule.
    let not_top = g.apply(wk::NOT, vec![wk::TOP]);
    let derived = cert(vec![
        Step::Axiom { node: wk::TOP, holds: true },
        Step::Negation { node: not_top, premise: 0 },
    ])
    .check(&g)
    .expect("valid");
    assert!(derived.hypotheses.is_empty(), "negating an axiom consults nobody");

    let told = cert(vec![Step::Told { node: claim, holds: true, sources: vec![] }])
        .check(&g)
        .expect("valid");
    assert_eq!(told.hypotheses, vec![claim], "testimony is a hypothesis");
    assert!(!told.assumed.is_empty(), "and an unattributed one is visibly assumed");
}

/// **`(implies P P)` must not certify as refuted.**
///
/// Content addressing gives both operands one id. Identifying a premise by its
/// *node* let a single proof of `P` discharge the antecedent and the consequent
/// at the antecedent's polarity, so a classical validity came back
/// `refutation: Certain` — with an authority named and nothing in `assumed`, the
/// shape a reader is told means fully grounded.
#[test]
fn a_self_implication_cannot_be_refuted() {
    let mut g = ObjectGraph::new();
    let (p, a, adam) = (g.atom("p"), g.atom("a"), g.atom("adam"));
    let pa = g.apply(p, vec![a]);
    let identity = g.apply(wk::IMPLIES, vec![pa, pa]);

    let attack = cert(vec![
        Step::Told { node: pa, holds: true, sources: vec![adam] },
        // Position 0 only: the consequent is never discharged.
        Step::Connective { node: identity, premises: vec![(0, 0)], holds: false },
    ]);
    assert!(attack.check(&g).is_err(), "one premise cannot cover two positions");
}

/// Zero operands make the coverage loop vacuous. `(implies)` used to certify
/// `refutation: Certain` from an **empty** premise list — certainty from nothing,
/// with no authority and no assumption recorded.
#[test]
fn a_connective_of_no_operands_certifies_nothing() {
    let mut g = ObjectGraph::new();
    let empty = g.apply(wk::IMPLIES, vec![]);
    let attack = cert(vec![Step::Connective { node: empty, premises: vec![], holds: false }]);
    assert!(attack.check(&g).is_err());
}

/// An honest conjunction still checks: every position covered, every premise
/// certain. A rule that rejected everything would pass the tests above and be
/// useless.
#[test]
fn an_honest_conjunction_checks() {
    let mut g = ObjectGraph::new();
    let (p, q, a, adam) = (g.atom("p"), g.atom("q"), g.atom("a"), g.atom("adam"));
    let (pa, qa) = (g.apply(p, vec![a]), g.apply(q, vec![a]));
    let both = g.apply(wk::AND, vec![pa, qa]);

    let c = cert(vec![
        Step::Told { node: pa, holds: true, sources: vec![adam] },
        Step::Told { node: qa, holds: true, sources: vec![adam] },
        Step::Connective { node: both, premises: vec![(0, 0), (1, 1)], holds: true },
    ])
    .check(&g)
    .expect("valid");
    assert_eq!(c.support, Bound::Certain);
    assert_eq!(c.authorities, vec![adam]);
    assert_eq!(c.hypotheses.len(), 2, "conditional on both pieces of testimony");
}

/// **A witness must come from the domain.** The binder's `vars` were
/// destructured and dropped, so `∃x ∈ {a,b}. P(x)` certified from `P(c)`.
#[test]
fn a_witness_from_outside_the_domain_is_refused() {
    use artist_logic::object::Binding;
    let mut g = ObjectGraph::new();
    let (p, a, b, c, adam) =
        (g.atom("p"), g.atom("a"), g.atom("b"), g.atom("c"), g.atom("adam"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let exists = g.bind(wk::EXISTS, vec![Binding { var: v, domain: Some(dom) }], vec![body]);

    let outside = g.apply(p, vec![c]);
    let attack = cert(vec![
        Step::Told { node: outside, holds: true, sources: vec![adam] },
        Step::Instance { node: exists, premise: 0, value: c, instance: outside, holds: true },
    ]);
    assert!(attack.check(&g).is_err(), "c is not in {{a, b}}");

    let inside = g.apply(p, vec![a]);
    let ok = cert(vec![
        Step::Told { node: inside, holds: true, sources: vec![adam] },
        Step::Instance { node: exists, premise: 0, value: a, instance: inside, holds: true },
    ]);
    assert!(ok.check(&g).is_ok(), "a witness from inside it is fine");
}

/// **Completeness is read from the domain, not asserted by the step.** This was
/// a `bool`; flipping it certified a universal from one member of two.
#[test]
fn a_universal_needs_every_member_and_a_complete_domain() {
    use artist_logic::object::Binding;
    let mut g = ObjectGraph::new();
    let (p, a, b, adam) = (g.atom("p"), g.atom("a"), g.atom("b"), g.atom("adam"));
    let (pa, pb) = (g.apply(p, vec![a]), g.apply(p, vec![b]));

    let mk = |g: &mut ObjectGraph, dom| {
        let v = g.fresh();
        let body = g.apply(p, vec![v]);
        g.bind(wk::FORALL, vec![Binding { var: v, domain: Some(dom) }], vec![body])
    };
    let complete = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let partial = g.apply(wk::SET_PARTIAL, vec![a, b]);
    let (all_c, all_p) = (mk(&mut g, complete), mk(&mut g, partial));

    let told = |n| Step::Told { node: n, holds: true, sources: vec![adam] };
    let (t_a, t_b) = (told(pa), told(pb));

    // One member of two: refused however the step is spelled.
    let short = cert(vec![
        t_a.clone(),
        Step::Exhaustive { node: all_c, premises: vec![(0, a, pa)], holds: true },
    ]);
    assert!(short.check(&g).is_err(), "half a domain is not a domain");

    // Both members, but the enumeration is explicitly partial.
    let incomplete = cert(vec![
        t_a.clone(),
        t_b.clone(),
        Step::Exhaustive { node: all_p, premises: vec![(0, a, pa), (1, b, pb)], holds: true },
    ]);
    assert!(incomplete.check(&g).is_err(), "`set-partial` licenses no universal");

    // Both members over a complete domain.
    let ok = cert(vec![
        t_a,
        t_b,
        Step::Exhaustive { node: all_c, premises: vec![(0, a, pa), (1, b, pb)], holds: true },
    ]);
    assert!(ok.check(&g).is_ok());
}

/// **Modus ponens relates its conclusion to its implication.** The old `Rule`
/// step related them in no way at all: any node followed from any premise by
/// naming an arbitrary `rule` id, and the result was `Derived` — *stronger* on
/// the derivation axis than honest testimony — so a forged rule laundered
/// testimony into deduction.
#[test]
fn modus_ponens_cannot_conclude_an_unrelated_node() {
    let mut g = ObjectGraph::new();
    let (p, q, r, a, adam) =
        (g.atom("p"), g.atom("q"), g.atom("r"), g.atom("a"), g.atom("adam"));
    let (pa, qa, ra) = (g.apply(p, vec![a]), g.apply(q, vec![a]), g.apply(r, vec![a]));
    let imp = g.apply(wk::IMPLIES, vec![pa, qa]);

    let honest = cert(vec![
        Step::Told { node: imp, holds: true, sources: vec![adam] },
        Step::Told { node: pa, holds: true, sources: vec![adam] },
        Step::ModusPonens { node: qa, implication: 0, antecedent: 1 },
    ]);
    let c = honest.check(&g).expect("valid");
    assert_eq!(c.support, Bound::Certain);
    assert_eq!(c.derivation, artist_logic::evidence::Derivation::Derived);
    assert_eq!(c.hypotheses.len(), 2, "the rule's own truth is a hypothesis");

    // Same premises, different conclusion.
    let forged = cert(vec![
        Step::Told { node: imp, holds: true, sources: vec![adam] },
        Step::Told { node: pa, holds: true, sources: vec![adam] },
        Step::ModusPonens { node: ra, implication: 0, antecedent: 1 },
    ]);
    assert!(forged.check(&g).is_err(), "the implication does not conclude `r`");
}

/// The liar is `Oscillatory`, the truth-teller is a `StableLoop`, and the kernel
/// **derives** which by walking the reference loop rather than reading a flag.
#[test]
fn ungroundedness_is_derived_from_the_loop_not_asserted() {
    let mut g = ObjectGraph::new();

    let liar = g.alloc();
    let q = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q]);
    let n = g.apply(wk::NOT, vec![h]);
    let body = g.get(n).cloned().expect("built");
    g.define(liar, body);

    let teller = g.alloc();
    let tq = g.apply(wk::QUOTE, vec![teller]);
    let th = g.apply(wk::HOLDS, vec![tq]);
    let tbody = g.get(th).cloned().expect("built");
    g.define(teller, tbody);

    let l = cert(vec![Step::Ungrounded { node: liar }]).check(&g).expect("valid");
    assert_eq!(l.grounding, Grounding::Oscillatory, "odd negation parity");

    let t = cert(vec![Step::Ungrounded { node: teller }]).check(&g).expect("valid");
    assert_eq!(t.grounding, Grounding::StableLoop, "even parity: classical fixpoints exist");
}

/// **A cycle is not ungroundedness.** `P ↔ P ∨ ⊤` loops and grounds to true in
/// one step of the fixpoint, because `⊥ ∨ T = T` (semantics §4.1). Parity proves
/// nothing off the reference fragment, so the rule refuses rather than guessing.
#[test]
fn a_cycle_through_a_connective_is_refused() {
    let mut g = ObjectGraph::new();
    let p = g.alloc();
    let disj = g.apply(wk::OR, vec![p, wk::TOP]);
    let body = g.get(disj).cloned().expect("built");
    g.define(p, body);

    let attack = cert(vec![Step::Ungrounded { node: p }]);
    assert!(attack.check(&g).is_err(), "a reachable cycle is not an unfounded set");
}

/// Backward references only, so a certificate is acyclic by construction and the
/// checker needs no cycle detection of its own.
#[test]
fn premises_must_point_backwards() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let not_pa = g.apply(wk::NOT, vec![pa]);

    for k in [0usize, 1, 99] {
        let bad = cert(vec![Step::Negation { node: not_pa, premise: k }]);
        assert!(bad.check(&g).is_err(), "premise {k} is not a backward reference");
    }
}

/// The kernel must survive the graphs the object language is *designed* to hold.
/// A self-referential node used to abort the process — stack overflow, exit 134 —
/// which for a checker fed by untrusted parties is a remote kill.
#[test]
fn checking_a_cyclic_graph_terminates() {
    let mut g = ObjectGraph::new();
    let liar = g.alloc();
    let q = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q]);
    let n = g.apply(wk::NOT, vec![h]);
    let body = g.get(n).cloned().expect("built");
    g.define(liar, body);

    // Whatever the verdicts, the point is that these return at all.
    let _ = cert(vec![Step::Ungrounded { node: liar }]).check(&g);
    let _ = cert(vec![Step::Told { node: liar, holds: true, sources: vec![] }]).check(&g);
    let _ = cert(vec![
        Step::Told { node: liar, holds: true, sources: vec![] },
        Step::Negation { node: n, premise: 0 },
    ])
    .check(&g);
}

/// Round trip through the term encoding, including into a **fresh** graph — the
/// real test of "store, transmit, re-check". Every field the checker uses to
/// *reject* must survive, or a stored certificate would be valid by amnesia.
#[test]
fn every_step_shape_round_trips() {
    use artist_logic::object::Binding;
    let mut g = ObjectGraph::new();
    let (p, a, b, adam) = (g.atom("p"), g.atom("a"), g.atom("b"), g.atom("adam"));
    let (pa, pb) = (g.apply(p, vec![a]), g.apply(p, vec![b]));
    let both = g.apply(wk::AND, vec![pa, pb]);
    let not_pa = g.apply(wk::NOT, vec![pa]);
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let all = g.bind(wk::FORALL, vec![Binding { var: v, domain: Some(dom) }], vec![body]);
    let hedged = g.apply(wk::UNLESS, vec![pb, pa]);

    let original = cert(vec![
        Step::Told { node: pa, holds: true, sources: vec![adam] },
        Step::Told { node: pb, holds: true, sources: vec![] },
        Step::Axiom { node: wk::TOP, holds: true },
        Step::Negation { node: not_pa, premise: 0 },
        Step::Connective { node: both, premises: vec![(0, 0), (1, 1)], holds: true },
        Step::Instance { node: all, premise: 0, value: a, instance: pa, holds: false },
        Step::Exhaustive { node: all, premises: vec![(0, a, pa), (1, b, pb)], holds: true },
        Step::ModusPonens { node: pb, implication: 4, antecedent: 0 },
        Step::Ungrounded { node: pa },
        Step::Defeasible { node: hedged, body: 0, defeater: 1 },
    ]);

    let term = original.to_term(&mut g);
    let back = Certificate::from_term(&g, term).expect("decodes");
    assert_eq!(back, original, "every field survives, including the rejecting ones");
}

/// A term that is not a proof, or a step whose parts are malformed, decodes to
/// `None` — never to a partial certificate, which the kernel would reject for
/// the wrong reason.
#[test]
fn malformed_terms_decode_to_nothing() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);

    let not_a_proof = g.apply(p, vec![a]);
    assert!(Certificate::from_term(&g, not_a_proof).is_none());

    // A `step` with an unknown kind.
    let bogus = g.apply(wk::STEP, vec![p, pa]);
    let proof = g.apply(wk::PROOF, vec![bogus]);
    assert!(Certificate::from_term(&g, proof).is_none());

    // A negation whose premise index is negative.
    let neg = g.int(-1);
    let step = g.apply(wk::STEP, vec![wk::BY_NEGATION, pa, neg]);
    let proof = g.apply(wk::PROOF, vec![step]);
    assert!(Certificate::from_term(&g, proof).is_none(), "a negative index is not an index");
}

/// **A default survives only when its defeater is ruled out.**
///
/// `(unless E P)` is *P, defeated when E*. Semantics §11 pairs Kripke's operator
/// with Dung's characteristic function; the grounded extension is the second
/// component of the least fixpoint, and an argument is in it exactly when every
/// attacker is answered. A kernel cannot compute that fixpoint — it does not have
/// to, because membership has a local witness.
#[test]
fn a_default_needs_its_defeater_refuted() {
    let mut g = ObjectGraph::new();
    let (p, e, a, adam) = (g.atom("p"), g.atom("e"), g.atom("a"), g.atom("adam"));
    let (pa, ea) = (g.apply(p, vec![a]), g.apply(e, vec![a]));
    let hedged = g.apply(wk::UNLESS, vec![ea, pa]);

    // The exception is established false: the attack fails, the default holds.
    let ok = cert(vec![
        Step::Told { node: pa, holds: true, sources: vec![adam] },
        Step::Told { node: ea, holds: false, sources: vec![adam] },
        Step::Defeasible { node: hedged, body: 0, defeater: 1 },
    ])
    .check(&g)
    .expect("valid");
    assert_eq!(ok.support, Bound::Certain);
    assert_eq!(
        ok.derivation,
        artist_logic::evidence::Derivation::Default,
        "and it is visibly a default, not an observation"
    );

    // The exception is merely *unheard of*. An attacker nobody has ruled out is
    // exactly the case where a default must not be certified — accepting it
    // would make every default unconditional, which is the whole failure mode
    // defeasible reasoning exists to avoid.
    let open_defeater = cert(vec![
        Step::Told { node: pa, holds: true, sources: vec![adam] },
        Step::Told { node: ea, holds: true, sources: vec![adam] },
        Step::Defeasible { node: hedged, body: 0, defeater: 1 },
    ]);
    assert!(open_defeater.check(&g).is_err(), "an exception that holds defeats");
}

/// The attacker is read **off the node**, so a step cannot nominate a convenient
/// one. Pointing the defeater premise at something that is not the `unless`
/// node's exception is a mismatch, not a weaker proof.
#[test]
fn a_default_cannot_choose_its_own_attacker() {
    let mut g = ObjectGraph::new();
    let (p, e, other, a, adam) =
        (g.atom("p"), g.atom("e"), g.atom("other"), g.atom("a"), g.atom("adam"));
    let (pa, ea, oa) =
        (g.apply(p, vec![a]), g.apply(e, vec![a]), g.apply(other, vec![a]));
    let hedged = g.apply(wk::UNLESS, vec![ea, pa]);

    // `other` is refuted, `e` is not even mentioned. If the step could name its
    // own attacker set this would certify — the same defect as `Exhaustive`'s
    // old `complete` flag, one rule along.
    let attack = cert(vec![
        Step::Told { node: pa, holds: true, sources: vec![adam] },
        Step::Told { node: oa, holds: false, sources: vec![adam] },
        Step::Defeasible { node: hedged, body: 0, defeater: 1 },
    ]);
    assert!(attack.check(&g).is_err(), "the exception is `e`, and the graph says so");
}

/// **The operator count in the spec is a claim, so it is tested.**
///
/// §4 of `docs/formalism.md` calls the vocabulary "the closed part of the
/// language" and states a number. An audit found the prose saying 100 while the
/// table listed 80 — the overclaim defect, in the one place that had no
/// executable check. A count nobody verifies drifts the moment a rule is added,
/// which is exactly what adding `by-defeasible` would have done.
#[test]
fn the_reserved_vocabulary_is_the_size_the_spec_says() {
    assert_eq!(
        artist_logic::object::wk::NAMES.len(),
        101,
        "docs/formalism.md §4 and §9 state this number; change both together"
    );
}
