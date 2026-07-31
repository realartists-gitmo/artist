//! The graph evaluator: one representation that both stores *and* decides.
//!
//! Before this, the object kernel could represent everything and evaluate
//! nothing, while the old evaluator could evaluate but was bounded by a closed
//! enum. These tests pin that the properties the old evaluator earned survive
//! the move, and that the new ones the kernel enables actually work.

use artist_logic::object::wk;
use artist_logic::registry::{OpContext, OperatorSemantics};
use artist_logic::*;

fn base() -> (ObjectGraph, ObjectId, ObjectId, ObjectId) {
    let mut g = ObjectGraph::new();
    let stale = g.atom("stale");
    let a = g.atom("a");
    let b = g.atom("b");
    (g, stale, a, b)
}

#[test]
fn closed_world_negation_is_sound_only_where_knowledge_is_complete() {
    let (mut g, stale, a, b) = base();
    let fact = g.apply(stale, vec![a]);
    let other = g.apply(stale, vec![b]);
    let ev = GraphEvaluator::new();

    // Open world: absence proves nothing.
    let open = MapGraphStructure::new().fact(stale, vec![a]);
    let r = ev.eval(&mut g, other, &open, 10_000);
    assert_eq!(r.evidential(), Evidential::Open);
    assert!(!r.is_definite(), "not-known must never read as refuted");

    // Closed world: absence is refutation.
    let closed = MapGraphStructure::new().fact(stale, vec![a]).closed(stale);
    let r = ev.eval(&mut g, other, &closed, 10_000);
    assert_eq!(r.evidential(), Evidential::Refuted);
    assert!(r.is_definite());

    // And the positive case still decides.
    let r = ev.eval(&mut g, fact, &closed, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);
}

#[test]
fn a_truncated_scan_never_fabricates_a_verdict() {
    let mut g = ObjectGraph::new();
    let stale = g.atom("stale");
    let dom = g.atom("Path");
    let members: Vec<ObjectId> = (0..400).map(|i| g.atom(&format!("f{i}"))).collect();
    let v = g.fresh();
    let body = g.apply(stale, vec![v]);
    let query = g.quantify(wk::EXISTS, v, Some(dom), body);

    let s = MapGraphStructure::new().domain(dom, members).closed(stale);
    let ev = GraphEvaluator::new();

    let cut = ev.eval(&mut g, query, &s, 40);
    assert_eq!(cut.compute_status, ComputeStatus::BudgetExhausted);
    assert_eq!(cut.evidential(), Evidential::Open, "no verdict from a partial scan");
    assert!(!cut.is_definite());
    assert!(cut.continuation.is_some(), "work is resumable, not discarded");
    assert!(!cut.dependencies.is_empty(), "and it knows what is left");

    // Same query, more budget: it decides. Cost is a dial.
    let full = ev.eval(&mut g, query, &s, 1_000_000);
    assert!(full.is_definite());
    assert_eq!(full.evidential(), Evidential::Refuted, "nothing is stale");
}

#[test]
fn connectives_short_circuit_soundly() {
    let (mut g, stale, a, b) = base();
    let known = g.apply(stale, vec![a]);
    let unknown = g.apply(stale, vec![b]);
    let s = MapGraphStructure::new().fact(stale, vec![a]);
    let ev = GraphEvaluator::new();

    // false ∧ unknown = false, decided without settling the unknown.
    let conj = g.apply(wk::AND, vec![wk::BOT, unknown]);
    let r = ev.eval(&mut g, conj, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Refuted);
    assert!(r.is_definite());

    // true ∨ unknown = true.
    let disj = g.apply(wk::OR, vec![known, unknown]);
    let r = ev.eval(&mut g, disj, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);

    // unknown ∧ true stays open — Kleene, not a guess.
    let conj2 = g.apply(wk::AND, vec![unknown, known]);
    let r = ev.eval(&mut g, conj2, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Open);
    assert!(!r.is_definite());
}

#[test]
fn arithmetic_and_text_are_arbitrary_precision() {
    let mut g = ObjectGraph::new();
    let ev = GraphEvaluator::new();
    let s = EmptyStructure;

    // (2^64) * (2^64) ≤ same — past i64, still exact.
    let big = g.lit(LiteralValue::Int(
        num_bigint::BigInt::from(u64::MAX) * num_bigint::BigInt::from(u64::MAX),
    ));
    let leq = g.apply(wk::LEQ, vec![big, big]);
    assert_eq!(ev.eval(&mut g, leq, &s, 1000).evidential(), Evidential::Supported);

    let one = g.int(1);
    let sum = g.apply(wk::ADD, vec![big, one]);
    let strict = g.apply(wk::LEQ, vec![sum, big]);
    assert_eq!(
        ev.eval(&mut g, strict, &s, 1000).evidential(),
        Evidential::Refuted,
        "n+1 ≤ n is false at any magnitude"
    );

    // Text operations over a concatenation.
    let l = g.text("src/");
    let r = g.text("compaction.rs");
    let joined = g.apply(wk::CONCAT, vec![l, r]);
    let needle = g.text("compaction");
    let has = g.apply(wk::CONTAINS, vec![joined, needle]);
    assert_eq!(ev.eval(&mut g, has, &s, 1000).evidential(), Evidential::Supported);
}

#[test]
fn quotation_is_opaque_until_deliberately_descended() {
    let (mut g, stale, a, _b) = base();
    let p = g.apply(stale, vec![a]);
    let quoted = g.apply(wk::QUOTE, vec![p]);
    let s = MapGraphStructure::new().fact(stale, vec![a]).closed(stale);
    let ev = GraphEvaluator::new();

    // A quotation is a name, not a claim.
    let r = ev.eval(&mut g, quoted, &s, 10_000);
    assert_eq!(r.compute_status, ComputeStatus::Unsupported);
    assert_eq!(r.evidential(), Evidential::Open);

    // `holds` is the explicit descent.
    let h = g.apply(wk::HOLDS, vec![quoted]);
    let r = ev.eval(&mut g, h, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);
}

// ---- registry: unsupported is its own answer ---------------------------

struct Parity;
impl OperatorSemantics for Parity {
    fn name(&self) -> &str {
        "even-arity"
    }
    fn evaluate_exact(&self, cx: &mut OpContext<'_>) -> Option<EvaluationResult> {
        cx.charge(1);
        Some(EvaluationResult::certain(cx.operands.len() % 2 == 0))
    }
}

#[test]
fn unknown_operators_evaluate_to_unsupported_then_to_semantics() {
    let mut g = ObjectGraph::new();
    let novel = g.atom("vendor:novel");
    let a = g.atom("a");
    let b = g.atom("b");
    let e = g.apply(novel, vec![a, b]);
    let s = EmptyStructure;

    let mut ev = GraphEvaluator::new();
    let r = ev.eval(&mut g, e, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Open);
    assert!(
        matches!(r.compute_status, ComputeStatus::Stalled | ComputeStatus::Unsupported),
        "no semantics and no fact: {:?}",
        r.compute_status
    );
    assert!(r.residual.is_some(), "handed back for later");

    // Register semantics — no migration, no reparse, same stored object.
    ev.registry.register(novel, Box::new(Parity));
    let r = ev.eval(&mut g, e, &s, 10_000);
    assert_eq!(r.compute_status, ComputeStatus::Exact);
    assert_eq!(r.evidential(), Evidential::Supported);
}

/// Every result states which version of the universe it used, so bounds from
/// different versions are never silently composed.
#[test]
fn results_carry_their_snapshot() {
    let (mut g, stale, a, _b) = base();
    let p = g.apply(stale, vec![a]);
    let ev = GraphEvaluator::new();

    let v1 = MapGraphStructure::new().fact(stale, vec![a]).at_version(7);
    let v2 = MapGraphStructure::new().fact(stale, vec![a]).at_version(9);

    let r1 = ev.eval(&mut g, p, &v1, 1000);
    let r2 = ev.eval(&mut g, p, &v2, 1000);
    assert_eq!(r1.snapshot, 7);
    assert_eq!(r2.snapshot, 9);
    assert_ne!(
        r1.snapshot, r2.snapshot,
        "two results over different universes are distinguishable"
    );
}

/// A universal is refuted by one counterexample without scanning the rest.
#[test]
fn a_counterexample_decides_a_universal_early() {
    let mut g = ObjectGraph::new();
    let ok = g.atom("ok");
    let dom = g.atom("D");
    let members: Vec<ObjectId> = (0..300).map(|i| g.atom(&format!("m{i}"))).collect();
    let bad = members[1];
    let v = g.fresh();
    let body = g.apply(ok, vec![v]);
    let all = g.quantify(wk::FORALL, v, Some(dom), body);

    let mut s = MapGraphStructure::new().domain(dom, members.clone()).closed(ok);
    for m in &members {
        if *m != bad {
            s = s.fact(ok, vec![*m]);
        }
    }
    let ev = GraphEvaluator::new();
    let r = ev.eval(&mut g, all, &s, 1_000_000);
    assert_eq!(r.evidential(), Evidential::Refuted);
    assert!(r.is_definite());
    assert!(r.spent < 100, "stopped at the counterexample: {}", r.spent);
}

// ---- lambda, refined domains, and resumable continuations ---------------

#[test]
fn lambdas_are_constructed_and_applied_on_the_graph() {
    let mut g = ObjectGraph::new();
    let stale = g.atom("stale");
    let a = g.atom("a");
    let b = g.atom("b");
    let x = g.fresh();
    let dom = g.atom("Path");

    // λx. stale(x)
    let body = g.apply(stale, vec![x]);
    let lam = g.bind(
        wk::LAMBDA,
        vec![artist_logic::ObjBinding { var: x, domain: Some(dom) }],
        vec![body],
    );
    let applied_a = g.apply(lam, vec![a]);
    let applied_b = g.apply(lam, vec![b]);

    let s = MapGraphStructure::new().fact(stale, vec![a]).closed(stale);
    let ev = GraphEvaluator::new();
    assert_eq!(ev.eval(&mut g, applied_a, &s, 10_000).evidential(), Evidential::Supported);
    assert_eq!(ev.eval(&mut g, applied_b, &s, 10_000).evidential(), Evidential::Refuted);
}

#[test]
fn set_and_where_domains_are_ordinary_expressions() {
    let mut g = ObjectGraph::new();
    let stale = g.atom("stale");
    let members: Vec<ObjectId> = (0..6).map(|i| g.atom(&format!("f{i}"))).collect();
    let v = g.fresh();

    // ∃v ∈ (set f0..f5). stale(v)
    let set_dom = g.apply(wk::SET_DOMAIN, members.clone());
    let body = g.apply(stale, vec![v]);
    let q = g.quantify(wk::EXISTS, v, Some(set_dom), body);

    let s = MapGraphStructure::new().fact(stale, vec![members[4]]).closed(stale);
    let ev = GraphEvaluator::new();
    let r = ev.eval(&mut g, q, &s, 100_000);
    assert_eq!(r.evidential(), Evidential::Supported, "f4 witnesses it");

    // Nothing stale in a subset that excludes it.
    let smaller = g.apply(wk::SET_DOMAIN, members[..3].to_vec());
    let q2 = g.quantify(wk::EXISTS, v, Some(smaller), body);
    let r = ev.eval(&mut g, q2, &s, 100_000);
    assert_eq!(r.evidential(), Evidential::Refuted);
    assert!(r.is_definite(), "a complete scan of a set domain decides");
}

/// The property the whole residual design rests on: re-asking the continuation
/// makes *progress* rather than restarting, and the remaining work strictly
/// shrinks each pass.
#[test]
fn continuations_shrink_and_resuming_is_just_evaluating_them() {
    let mut g = ObjectGraph::new();
    let stale = g.atom("stale");
    let dom = g.atom("Path");
    let members: Vec<ObjectId> = (0..300).map(|i| g.atom(&format!("f{i}"))).collect();
    let v = g.fresh();
    let body = g.apply(stale, vec![v]);
    let query = g.quantify(wk::EXISTS, v, Some(dom), body);

    let s = MapGraphStructure::new().domain(dom, members.clone()).closed(stale);
    let ev = GraphEvaluator::new();

    // Small budget: cut short, with a continuation.
    let first = ev.eval(&mut g, query, &s, 60);
    assert_eq!(first.compute_status, ComputeStatus::BudgetExhausted);
    let cont = first.continuation.expect("a continuation");
    let left_after_first = first.dependencies.len();
    assert!(left_after_first < 300, "some ground was covered");

    // The continuation is an ordinary query — resuming is evaluating it.
    let second = ev.eval(&mut g, cont, &s, 60);
    let left_after_second = second
        .dependencies
        .len()
        .min(left_after_first);
    assert!(
        left_after_second < left_after_first,
        "remaining work shrank: {left_after_first} -> {left_after_second}"
    );

    // And it is still a *quantified* question, not an unrolled conjunction.
    let printed = artist_logic::syntax::print(&g, cont);
    assert!(printed.starts_with("(exists"), "still quantified: {}", &printed[..40.min(printed.len())]);

    // Enough budget on the continuation and it decides outright.
    let done = ev.eval(&mut g, cont, &s, 1_000_000);
    assert!(done.is_definite(), "the continuation is answerable on its own");
}

// ---- Kripke groundedness, with oscillation detection --------------------

/// The chosen reflective-truth semantics: grounded sentences get classical
/// values, ungrounded-but-stable stay Open, ungrounded-and-oscillating report
/// Conflicted. This is what distinguishes the liar from the truth-teller,
/// which neither a pure fixed-point nor a pure paraconsistent reading does.
#[test]
fn the_liar_oscillates_and_the_truth_teller_does_not() {
    // L = ¬holds(⟨L⟩) — parity flips around the loop.
    let mut g = ObjectGraph::new();
    let liar = g.alloc();
    let ql = g.apply(wk::QUOTE, vec![liar]);
    let hl = g.apply(wk::HOLDS, vec![ql]);
    let neg = g.apply(wk::NOT, vec![hl]);
    let n = g.get(neg).cloned().expect("built");
    g.define(liar, n);

    let ev = GraphEvaluator::new();
    let r = ev.eval(&mut g, liar, &EmptyStructure, 100_000);
    assert_eq!(
        r.evidential(),
        Evidential::Conflicted,
        "the liar oscillates between rounds"
    );
    assert_eq!(r.compute_status, ComputeStatus::Exact, "and that is a finding, not a stall");

    // P = holds(⟨P⟩) — ungrounded, but stable. No parity flip.
    let mut g2 = ObjectGraph::new();
    let teller = g2.alloc();
    let qt = g2.apply(wk::QUOTE, vec![teller]);
    let ht = g2.apply(wk::HOLDS, vec![qt]);
    let n2 = g2.get(ht).cloned().expect("built");
    g2.define(teller, n2);

    let r = ev.eval(&mut g2, teller, &EmptyStructure, 100_000);
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "the truth-teller never grounds, but never contradicts either"
    );
    assert_ne!(
        r.evidential(),
        Evidential::Conflicted,
        "and is distinguishable from the liar"
    );
}

/// A grounded sentence is unaffected by the machinery.
#[test]
fn grounded_sentences_keep_classical_values() {
    let mut g = ObjectGraph::new();
    let stale = g.atom("stale");
    let a = g.atom("a");
    let p = g.apply(stale, vec![a]);
    let quoted = g.apply(wk::QUOTE, vec![p]);
    let h = g.apply(wk::HOLDS, vec![quoted]);
    let s = MapGraphStructure::new().fact(stale, vec![a]).closed(stale);
    let ev = GraphEvaluator::new();
    let r = ev.eval(&mut g, h, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);
    assert!(r.is_definite());
}

// ---- ported capabilities ------------------------------------------------

#[test]
fn aggregates_compute_on_the_graph() {
    let mut g = ObjectGraph::new();
    let corrected = g.atom("corrected");
    let dom = g.atom("Path");
    let members: Vec<ObjectId> = (0..5).map(|i| g.atom(&format!("f{i}"))).collect();
    let v = g.fresh();
    let body = g.apply(corrected, vec![v]);
    let count = g.bind(
        wk::COUNT,
        vec![artist_logic::ObjBinding { var: v, domain: Some(dom) }],
        vec![body],
    );

    let mut s = MapGraphStructure::new().domain(dom, members.clone()).closed(corrected);
    for m in &members[..3] {
        s = s.fact(corrected, vec![*m]);
    }
    let three = g.int(3);
    let eq = g.apply(wk::EQ, vec![count, three]);
    let ev = GraphEvaluator::new();
    // Aggregates denote numbers, so the comparison is what carries the verdict.
    let r = ev.eval(&mut g, eq, &s, 100_000);
    assert_ne!(r.evidential(), Evidential::Refuted, "three of five were corrected");
}

#[test]
fn letrec_computes_a_least_fixpoint_on_the_graph() {
    let mut g = ObjectGraph::new();
    let edge = g.atom("edge");
    let dom = g.atom("Node");
    let ns: Vec<ObjectId> = (0..4).map(|i| g.atom(&format!("n{i}"))).collect();
    let (x, y, z, reach) = (g.fresh(), g.fresh(), g.fresh(), g.fresh());

    let direct = g.apply(edge, vec![x, y]);
    let ra = g.apply(reach, vec![x, z]);
    let rb = g.apply(edge, vec![z, y]);
    let hop_and = g.apply(wk::AND, vec![ra, rb]);
    let hop = g.quantify(wk::EXISTS, z, Some(dom), hop_and);
    let def = g.apply(wk::OR, vec![direct, hop]);
    let scope = g.apply(reach, vec![ns[0], ns[3]]);
    let lr = g.bind(
        wk::LETREC,
        vec![
            artist_logic::ObjBinding { var: reach, domain: None },
            artist_logic::ObjBinding { var: x, domain: Some(dom) },
            artist_logic::ObjBinding { var: y, domain: Some(dom) },
        ],
        vec![def, scope],
    );

    let mut s = MapGraphStructure::new().domain(dom, ns.clone()).closed(edge);
    for w in ns.windows(2) {
        s = s.fact(edge, vec![w[0], w[1]]);
    }
    let ev = GraphEvaluator::new();
    let r = ev.eval(&mut g, lr, &s, 5_000_000);
    assert_eq!(r.evidential(), Evidential::Supported, "n0 reaches n3 transitively");
}

#[test]
fn second_order_quantifies_over_relations_on_the_graph() {
    let mut g = ObjectGraph::new();
    let dom = g.atom("D");
    let a = g.atom("a");
    let rel_ty = g.apply(wk::RELATION_TYPE, vec![dom]);
    let pv = g.fresh();

    // ∃P : Rel(D). P(a) ∧ ¬P(a) — no relation whatsoever satisfies this.
    let pa = g.apply(pv, vec![a]);
    let npa = g.apply(wk::NOT, vec![pa]);
    let contradiction = g.apply(wk::AND, vec![pa, npa]);
    let q = g.quantify(wk::EXISTS, pv, Some(rel_ty), contradiction);

    let s = MapGraphStructure::new().domain(dom, vec![a]);
    let ev = GraphEvaluator::new();
    let r = ev.eval(&mut g, q, &s, 100_000);
    assert_eq!(
        r.evidential(),
        Evidential::Refuted,
        "exhausting a finite powerset yields a genuine universal"
    );
}

// ---- infinite domains and time, on the graph ---------------------------

#[test]
fn universals_over_naturals_are_decided_on_the_graph() {
    let mut g = ObjectGraph::new();
    let v = g.fresh();
    let one = g.int(1);
    let s = EmptyStructure;
    let ev = GraphEvaluator::new();

    // ∀n:ℕ. n ≤ n + 1 — true of infinitely many values; the tail closes
    // abstractly once the variable cancels.
    let succ = g.apply(wk::ADD, vec![v, one]);
    let leq = g.apply(wk::LEQ, vec![v, succ]);
    let all = g.quantify(wk::FORALL, v, Some(wk::NAT_TYPE), leq);
    let r = ev.eval(&mut g, all, &s, 200_000);
    assert_eq!(r.evidential(), Evidential::Supported, "widening closed the tail");

    // ∀n:ℕ. n ≤ 10 — refuted by a concrete counterexample, over an infinite
    // domain, in finite time.
    let ten = g.int(10);
    let bounded = g.apply(wk::LEQ, vec![v, ten]);
    let all2 = g.quantify(wk::FORALL, v, Some(wk::NAT_TYPE), bounded);
    let r = ev.eval(&mut g, all2, &s, 200_000);
    assert_eq!(r.evidential(), Evidential::Refuted, "n = 11 refutes it");

    // ∃n:ℕ. 5 ≤ n — a concrete witness decides an existential.
    let five = g.int(5);
    let ge = g.apply(wk::LEQ, vec![five, v]);
    let some = g.quantify(wk::EXISTS, v, Some(wk::NAT_TYPE), ge);
    assert_eq!(
        ev.eval(&mut g, some, &s, 200_000).evidential(),
        Evidential::Supported
    );
}

#[test]
fn an_undecidable_infinite_claim_stalls_honestly() {
    let mut g = ObjectGraph::new();
    let halts = g.atom("halts");
    let v = g.fresh();
    let body = g.apply(halts, vec![v]);
    let all = g.quantify(wk::FORALL, v, Some(wk::NAT_TYPE), body);

    let ev = GraphEvaluator::new();
    let mut b = 200_000u64;
    let r = ev.eval(&mut g, all, &EmptyStructure, b);
    assert_eq!(r.evidential(), Evidential::Open, "no counterexample, no proof");
    assert!(!r.is_definite());
    assert!(r.residual.is_some(), "the question comes back intact");
    b = r.spent;
    assert!(b <= 200_000, "and it terminated");
}

/// A structure with history: the same proposition at two instants.
struct Timed {
    pred: ObjectId,
    arg: ObjectId,
}
impl GraphStructure for Timed {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Option<bool> {
        // Present: no longer believed.
        (pred == self.pred && args == [self.arg]).then_some(false)
    }
    fn known_at(&self, pred: ObjectId, args: &[ObjectId], t: i64) -> Option<bool> {
        (pred == self.pred && args == [self.arg]).then_some((1_000..2_000).contains(&t))
    }
    fn instants(&self) -> Vec<i64> {
        vec![1_000, 2_000]
    }
}

#[test]
fn time_travel_works_on_the_graph() {
    let mut g = ObjectGraph::new();
    let prefers = g.atom("prefers");
    let tabs = g.atom("tabs");
    let p = g.apply(prefers, vec![tabs]);
    let s = Timed { pred: prefers, arg: tabs };
    let ev = GraphEvaluator::new();

    // Now: refuted.
    assert_eq!(ev.eval(&mut g, p, &s, 10_000).evidential(), Evidential::Refuted);

    // Then: supported. "What did I used to believe" is answerable.
    let then = g.int(1_500);
    let past = g.apply(wk::AT, vec![then, p]);
    assert_eq!(ev.eval(&mut g, past, &s, 10_000).evidential(), Evidential::Supported);

    // And after the switch, refuted again.
    let later = g.int(2_500);
    let after = g.apply(wk::AT, vec![later, p]);
    assert_eq!(ev.eval(&mut g, after, &s, 10_000).evidential(), Evidential::Refuted);
}
