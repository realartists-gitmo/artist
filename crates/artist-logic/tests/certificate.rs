//! The de Bruijn criterion: a derivation is an object a dumb checker validates.
//!
//! The interesting tests here are the **negative** ones. That a certificate the
//! evaluator produced checks out proves little — both halves were written by the
//! same hand on the same afternoon. What matters is that a certificate which
//! claims more than its premises license is *rejected*, because that is the only
//! thing standing between "checkable derivation" and "second copy of the
//! evaluator that agrees with the first."

use artist_logic::certificate::{Certificate, Invalid, Step};
use artist_logic::evidence::{Bound, Derivation, Evidential, Grounding};
use artist_logic::graph_eval::{EmptyStructure, GraphEvaluator, MapGraphStructure};
use artist_logic::object::wk;
use artist_logic::ObjectGraph;

/// A derivation the evaluator produced checks out, and concludes what was asked.
#[test]
fn an_emitted_derivation_checks() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let s = MapGraphStructure::new().fact(p, vec![a]);

    let (r, cert) = GraphEvaluator::new().eval_traced(&mut g, claim, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);

    let checked = cert.check(&g).expect("valid");
    assert_eq!(checked.node, claim);
    assert_eq!(checked.support, Bound::Certain);
    // **A structure that names nobody gets no authority invented for it.** This
    // used to assert `authorities == [p]` — the *predicate*, so `(p a)` was
    // certified on the authority of `p`, which is not an authority: it cannot be
    // doubted, consulted or retracted, and every fact in the store named itself.
    assert!(checked.authorities.is_empty(), "nobody vouched for this");
    assert_eq!(checked.assumed, vec![claim], "so it is on file as taken on trust");
    assert!(cert.justifies(&g, claim, &r));
}

/// …and a structure that *does* name a source puts it in the certificate.
#[test]
fn a_told_step_names_its_source() {
    let mut g = ObjectGraph::new();
    let (p, a, adam) = (g.atom("p"), g.atom("a"), g.atom("adam"));
    let claim = g.apply(p, vec![a]);
    let s = MapGraphStructure::new().fact(p, vec![a]).attributed(claim, vec![adam]);

    let (_, cert) = GraphEvaluator::new().eval_traced(&mut g, claim, &s, 10_000);
    let checked = cert.check(&g).expect("valid");
    assert_eq!(checked.authorities, vec![adam], "the agent who said so, not the relation");
    assert!(checked.assumed.is_empty(), "nothing here is unattributed");
}

/// A tautology's certificate needs no authority at all — that is the whole
/// difference between deduction and testimony.
#[test]
fn a_tautology_rests_on_nothing() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let identity = g.apply(wk::IMPLIES, vec![pa, pa]);

    let (r, cert) = GraphEvaluator::new().eval_traced(&mut g, identity, &EmptyStructure, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);
    let checked = cert.check(&g).expect("valid");
    assert!(checked.authorities.is_empty(), "P → P owes nobody anything");
}

/// A witness travels in the derivation, so a reader can check *which* member
/// settled it without re-scanning the domain.
#[test]
fn a_witness_appears_in_the_derivation() {
    let mut g = ObjectGraph::new();
    let fails = g.atom("fails");
    let (t1, t2) = (g.atom("t1"), g.atom("t2"));
    let dom = g.apply(wk::SET_DOMAIN, vec![t1, t2]);
    let s = MapGraphStructure::new().fact(fails, vec![t2]);
    let v = g.fresh();
    let body = g.apply(fails, vec![v]);
    let q = g.quantify(wk::EXISTS, v, Some(dom), body);

    let (r, cert) = GraphEvaluator::new().eval_traced(&mut g, q, &s, 50_000);
    assert_eq!(r.evidential(), Evidential::Supported);
    assert!(
        cert.steps.iter().any(|st| matches!(st, Step::Instance { value, .. } if *value == t2)),
        "the derivation names t2"
    );
    cert.check(&g).expect("valid");
}

// ------------------------------------------------------------- the real tests

/// **A forged universal is rejected.** One agreeing member does not license
/// "every member agrees" — and the checker knows that without knowing what a
/// domain is.
#[test]
fn one_member_does_not_prove_a_universal() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a]);
    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(dom), body);
    let inner = g.apply(p, vec![a]);

    let mut cert = Certificate::default();
    let k = cert.push(Step::Told { node: inner, holds: true, sources: vec![p] });
    // A universal settled *true* by a single instance: not licensed.
    cert.push(Step::Instance {
        node: q,
        premise: k,
        value: a,
        instance: inner,
        holds: true,
    });
    assert_eq!(cert.check(&g), Err(Invalid::Unlicensed { step: 1 }));
}

/// **An exhaustive step that admits it was not exhaustive is rejected.** The
/// completeness claim is the entire content of the step.
#[test]
fn a_partial_scan_does_not_prove_a_universal() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a]);
    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(dom), body);
    let inner = g.apply(p, vec![a]);

    let mut cert = Certificate::default();
    let k = cert.push(Step::Told { node: inner, holds: true, sources: vec![p] });
    cert.push(Step::Exhaustive {
        node: q,
        premises: vec![(k, a, inner)],
        holds: true,
        complete: false,
    });
    assert_eq!(cert.check(&g), Err(Invalid::Unlicensed { step: 1 }));
}

/// **A tautology step is re-decided, not believed.** The checker computes the
/// skeleton itself, so claiming a contingent sentence is valid fails.
#[test]
fn a_forged_tautology_is_rejected() {
    let mut g = ObjectGraph::new();
    let (p, q, a) = (g.atom("p"), g.atom("q"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let qa = g.apply(q, vec![a]);
    let contingent = g.apply(wk::IMPLIES, vec![pa, qa]);

    let mut cert = Certificate::default();
    cert.push(Step::Tautology { node: contingent, holds: true, constructive: false });
    assert_eq!(cert.check(&g), Err(Invalid::Unlicensed { step: 0 }));
}

/// **A step must be about the node it claims.** A negation whose premise is some
/// other proposition is rejected on shape alone.
#[test]
fn a_step_must_match_its_node() {
    let mut g = ObjectGraph::new();
    let (p, q, a) = (g.atom("p"), g.atom("q"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let qa = g.apply(q, vec![a]);
    let not_pa = g.apply(wk::NOT, vec![pa]);

    let mut cert = Certificate::default();
    let k = cert.push(Step::Told { node: qa, holds: true, sources: vec![q] });
    cert.push(Step::Negation { node: not_pa, premise: k });
    assert_eq!(cert.check(&g), Err(Invalid::Mismatched { step: 1 }));
}

/// **Premises point backwards only**, so a certificate cannot justify itself and
/// the checker needs no cycle detection.
#[test]
fn a_certificate_cannot_cite_itself() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let not_pa = g.apply(wk::NOT, vec![pa]);

    let mut cert = Certificate::default();
    cert.push(Step::Negation { node: not_pa, premise: 0 });
    assert_eq!(cert.check(&g), Err(Invalid::BadReference { step: 0 }));

    let mut forward = Certificate::default();
    forward.push(Step::Negation { node: not_pa, premise: 5 });
    assert_eq!(forward.check(&g), Err(Invalid::BadReference { step: 0 }));
}

/// **A conjunction cannot be proved true by one conjunct.**
#[test]
fn one_conjunct_does_not_prove_a_conjunction() {
    let mut g = ObjectGraph::new();
    let (p, q, a) = (g.atom("p"), g.atom("q"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let qa = g.apply(q, vec![a]);
    let conj = g.apply(wk::AND, vec![pa, qa]);

    let mut cert = Certificate::default();
    let k = cert.push(Step::Told { node: pa, holds: true, sources: vec![p] });
    cert.push(Step::Connective { node: conj, premises: vec![k], holds: true });
    assert_eq!(cert.check(&g), Err(Invalid::Unlicensed { step: 1 }));
}

/// The axes travel through a checked derivation as they do through an evaluated
/// one: a conclusion resting on a defeasible step is defeasible.
#[test]
fn a_checked_derivation_carries_the_axes() {
    let mut g = ObjectGraph::new();
    let (touches, needs, c1) = (g.atom("touches"), g.atom("needs"), g.atom("c1"));
    let ante = g.apply(touches, vec![c1]);
    let goal = g.apply(needs, vec![c1]);
    let rule = g.atom("the-rule");

    let mut cert = Certificate::default();
    let k = cert.push(Step::Told { node: ante, holds: true, sources: vec![touches] });
    cert.push(Step::Rule { node: goal, rule, premises: vec![k], defeasible: true });

    let checked = cert.check(&g).expect("valid");
    assert_eq!(checked.derivation, Derivation::Default);
    assert!(checked.authorities.contains(&rule), "the rule is an authority too");
    assert!(checked.authorities.contains(&touches));
}

/// An ungrounded step reports on the grounding axis and establishes no evidence.
#[test]
fn an_ungrounded_step_establishes_nothing() {
    let mut g = ObjectGraph::new();
    let liar = g.alloc();
    let q = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q]);
    let n = g.apply(wk::NOT, vec![h]);
    let node = g.get(n).cloned().expect("built");
    g.define(liar, node);

    let mut cert = Certificate::default();
    cert.push(Step::Ungrounded { node: liar, oscillating: true });
    let checked = cert.check(&g).expect("valid");
    assert_eq!(checked.grounding, Grounding::Oscillatory);
    assert_eq!(checked.support, Bound::None);
    assert_eq!(checked.refutation, Bound::None);
}

/// An empty derivation proves nothing, and says so rather than vacuously
/// succeeding.
#[test]
fn an_empty_certificate_is_rejected() {
    let g = ObjectGraph::new();
    assert_eq!(Certificate::default().check(&g), Err(Invalid::Empty));
}

// ------------------------------------------- round six: the forgeries that landed

/// **A conjunction cannot be certified true from refuted conjuncts.** The
/// polarity test was written as a three-way expression that computed the wrong
/// branch for `(and, true)` — so every conjunction was certifiable true from
/// proofs its conjuncts were *false*, and an honest conjunction proof could not
/// be represented at all. The correct rule is one line: every premise points the
/// way the conclusion does.
#[test]
fn a_conjunction_is_not_proved_true_by_refuted_conjuncts() {
    let mut g = ObjectGraph::new();
    let (p, q, a) = (g.atom("p"), g.atom("q"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let qa = g.apply(q, vec![a]);
    let conj = g.apply(wk::AND, vec![pa, qa]);

    let mut forged = Certificate::default();
    let k0 = forged.push(Step::Told { node: pa, holds: false, sources: vec![p] });
    let k1 = forged.push(Step::Told { node: qa, holds: false, sources: vec![q] });
    forged.push(Step::Connective { node: conj, premises: vec![k0, k1], holds: true });
    assert!(forged.check(&g).is_err(), "refuted conjuncts do not prove a conjunction");

    // …and the honest version is representable, which it previously was not.
    let mut honest = Certificate::default();
    let k0 = honest.push(Step::Told { node: pa, holds: true, sources: vec![p] });
    let k1 = honest.push(Step::Told { node: qa, holds: true, sources: vec![q] });
    honest.push(Step::Connective { node: conj, premises: vec![k0, k1], holds: true });
    let checked = honest.check(&g).expect("valid");
    assert_eq!(checked.support, Bound::Certain);
}

/// **Citing one premise twice is not citing two operands.** The check was on
/// premise *count*, so a duplicated index satisfied a two-operand requirement
/// and one refuted disjunct proved a disjunction false.
#[test]
fn a_duplicate_premise_does_not_cover_two_operands() {
    let mut g = ObjectGraph::new();
    let (p, q, a) = (g.atom("p"), g.atom("q"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let qa = g.apply(q, vec![a]);
    let disj = g.apply(wk::OR, vec![pa, qa]);

    let mut cert = Certificate::default();
    let k = cert.push(Step::Told { node: pa, holds: false, sources: vec![p] });
    cert.push(Step::Connective { node: disj, premises: vec![k, k], holds: false });
    assert!(cert.check(&g).is_err(), "q was never accounted for");
}

/// **An existential cannot be proved from an unrelated premise.** The step knew
/// the node was a binder and the premise was certain, and nothing tied them
/// together — so `#true` proved anything. That was missing *information*, not a
/// missing check: the format now carries the instantiated premise.
#[test]
fn an_existential_is_not_proved_from_an_unrelated_premise() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a]);
    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let q = g.quantify(wk::EXISTS, v, Some(dom), body);
    let inner = g.apply(p, vec![a]);

    let mut forged = Certificate::default();
    let k = forged.push(Step::Axiom { node: wk::TOP, holds: true });
    forged.push(Step::Instance {
        node: q,
        premise: k,
        value: a,
        instance: inner,
        holds: true,
    });
    assert!(forged.check(&g).is_err(), "#true is not a witness for anything");

    // The genuine article: the premise really is the body under `a`.
    let mut honest = Certificate::default();
    let k = honest.push(Step::Told { node: inner, holds: true, sources: vec![p] });
    honest.push(Step::Instance {
        node: q,
        premise: k,
        value: a,
        instance: inner,
        holds: true,
    });
    honest.check(&g).expect("a real witness checks");
}

/// The same for a universal: unrelated premises do not exhaust a domain, and one
/// member cited twice is not two members.
#[test]
fn a_universal_is_not_proved_from_unrelated_premises() {
    let mut g = ObjectGraph::new();
    let (p, a, b) = (g.atom("p"), g.atom("a"), g.atom("b"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(dom), body);
    let pa = g.apply(p, vec![a]);

    let mut forged = Certificate::default();
    let k = forged.push(Step::Axiom { node: wk::TOP, holds: true });
    forged.push(Step::Exhaustive {
        node: q,
        premises: vec![(k, a, wk::TOP)],
        holds: true,
        complete: true,
    });
    assert!(forged.check(&g).is_err());

    let mut doubled = Certificate::default();
    let k = doubled.push(Step::Told { node: pa, holds: true, sources: vec![p] });
    doubled.push(Step::Exhaustive {
        node: q,
        premises: vec![(k, a, pa), (k, a, pa)],
        holds: true,
        complete: true,
    });
    assert!(doubled.check(&g).is_err(), "one member twice is not two members");
}

/// **A rule that rests on nothing concludes nothing** — and cannot conclude it
/// about a node the graph has never heard of.
#[test]
fn a_rule_with_no_premises_proves_nothing() {
    let g = ObjectGraph::new();
    let mut cert = Certificate::default();
    cert.push(Step::Rule {
        node: artist_logic::ObjectId(0xfeed_face),
        rule: artist_logic::ObjectId(0xdead_beef),
        premises: vec![],
        defeasible: false,
    });
    assert!(cert.check(&g).is_err());
}

/// **A tautology's determinacy is an assumption, not a finding.** The checker
/// has no store, so it cannot verify bivalence — claiming `Total` for excluded
/// middle must be recorded as something the certificate *assumes*, or the
/// kernel becomes the bivalence launderer the determinacy axis exists to stop.
#[test]
fn a_tautology_cannot_silently_claim_bivalence() {
    let mut g = ObjectGraph::new();
    let (heap, n) = (g.atom("heap"), g.int(4783));
    let h = g.apply(heap, vec![n]);
    let nh = g.apply(wk::NOT, vec![h]);
    let lem = g.apply(wk::OR, vec![h, nh]);

    let mut cert = Certificate::default();
    cert.push(Step::Tautology { node: lem, holds: true, constructive: false });
    let checked = cert.check(&g).expect("the skeleton really is classically valid");
    assert!(
        !checked.assumed.is_empty(),
        "…but the totality it rests on is an assumption, and must be named"
    );
    assert!(
        checked.authorities.is_empty(),
        "and an assumption is not an authority: there is nobody here to go and ask"
    );

    // Claiming it constructively valid is a claim the checker re-decides.
    let mut lying = Certificate::default();
    lying.push(Step::Tautology { node: lem, holds: true, constructive: true });
    assert!(lying.check(&g).is_err(), "excluded middle is not intuitionistically valid");
}

/// The evaluator must not emit a certificate its own checker rejects. An
/// implication decided by the scan was emitted as a `Connective` the checker had
/// no arm for, so every query containing one produced an unusable derivation.
#[test]
fn a_decided_implication_produces_a_checkable_derivation() {
    let mut g = ObjectGraph::new();
    let (p, q, a) = (g.atom("p"), g.atom("q"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let qa = g.apply(q, vec![a]);
    let imp = g.apply(wk::IMPLIES, vec![pa, qa]);
    let s = MapGraphStructure::new().fact(p, vec![a]).fact(q, vec![a]);

    let (r, cert) = GraphEvaluator::new().eval_traced(&mut g, imp, &s, 20_000);
    assert_eq!(r.evidential(), Evidential::Supported);
    cert.check(&g).expect("the evaluator's own derivation must check");
}

/// **`justifies` compares every axis.** A certificate claiming a conclusion is
/// observed and sharp does not justify a result that is defeasible or
/// indeterminate — claiming *more* on an axis is the same defect as claiming
/// more on the evidence.
#[test]
fn justifies_does_not_ignore_the_provenance_axes() {
    use artist_logic::evidence::{ComputeStatus, EvaluationResult};
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);

    let mut cert = Certificate::default();
    cert.push(Step::Told { node: claim, holds: true, sources: vec![p] });

    let mut defeasible = EvaluationResult::certain(true);
    defeasible.compute_status = ComputeStatus::Exact;
    defeasible.derivation = Derivation::Default;
    assert!(
        !cert.justifies(&g, claim, &defeasible),
        "an observed derivation does not justify a defeasible result"
    );
}
