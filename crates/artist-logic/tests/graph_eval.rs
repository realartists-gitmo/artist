//! The graph evaluator: one representation that both stores *and* decides.
//!
//! Before this, the object kernel could represent everything and evaluate
//! nothing, while the old evaluator could evaluate but was bounded by a closed
//! enum. These tests pin that the properties the old evaluator earned survive
//! the move, and that the new ones the kernel enables actually work.

use artist_logic::graph_eval::{Extension, Knowledge};
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
    assert_eq!(
        cut.evidential(),
        Evidential::Open,
        "no verdict from a partial scan"
    );
    assert!(!cut.is_definite());
    assert!(
        cut.continuation.is_some(),
        "work is resumable, not discarded"
    );
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
    assert_eq!(
        ev.eval(&mut g, leq, &s, 1000).evidential(),
        Evidential::Supported
    );

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
    assert_eq!(
        ev.eval(&mut g, has, &s, 1000).evidential(),
        Evidential::Supported
    );
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
        Some(EvaluationResult::certain(
            cx.operands.len().is_multiple_of(2),
        ))
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
        matches!(
            r.compute_status,
            ComputeStatus::Stalled | ComputeStatus::Unsupported
        ),
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

    let mut s = MapGraphStructure::new()
        .domain(dom, members.clone())
        .closed(ok);
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
        vec![artist_logic::ObjBinding {
            var: x,
            domain: Some(dom),
        }],
        vec![body],
    );
    let applied_a = g.apply(lam, vec![a]);
    let applied_b = g.apply(lam, vec![b]);

    let s = MapGraphStructure::new().fact(stale, vec![a]).closed(stale);
    let ev = GraphEvaluator::new();
    assert_eq!(
        ev.eval(&mut g, applied_a, &s, 10_000).evidential(),
        Evidential::Supported
    );
    assert_eq!(
        ev.eval(&mut g, applied_b, &s, 10_000).evidential(),
        Evidential::Refuted
    );
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

    let s = MapGraphStructure::new()
        .fact(stale, vec![members[4]])
        .closed(stale);
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

    let s = MapGraphStructure::new()
        .domain(dom, members.clone())
        .closed(stale);
    let ev = GraphEvaluator::new();

    // Small budget: cut short, with a continuation.
    let first = ev.eval(&mut g, query, &s, 60);
    assert_eq!(first.compute_status, ComputeStatus::BudgetExhausted);
    let cont = first.continuation.expect("a continuation");
    let left_after_first = first.dependencies.len();
    assert!(left_after_first < 300, "some ground was covered");

    // The continuation is an ordinary query — resuming is evaluating it.
    let second = ev.eval(&mut g, cont, &s, 60);
    let left_after_second = second.dependencies.len().min(left_after_first);
    assert!(
        left_after_second < left_after_first,
        "remaining work shrank: {left_after_first} -> {left_after_second}"
    );

    // And it is still a *quantified* question, not an unrolled conjunction.
    let printed = artist_logic::syntax::print(&g, cont);
    assert!(
        printed.starts_with("(exists"),
        "still quantified: {}",
        &printed[..40.min(printed.len())]
    );

    // Enough budget on the continuation and it decides outright.
    let done = ev.eval(&mut g, cont, &s, 1_000_000);
    assert!(
        done.is_definite(),
        "the continuation is answerable on its own"
    );
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
        r.grounding,
        artist_logic::evidence::Grounding::Oscillatory,
        "the liar oscillates between rounds"
    );
    assert_eq!(
        r.compute_status,
        ComputeStatus::Exact,
        "and that is a finding, not a stall"
    );
    // Ungroundedness is not evidence. Reporting it as `Conflicted` made one
    // value mean "the store holds claims both ways, go read them" and "this
    // sentence has no stable value, stop asking" — two different instructions
    // to the caller, and §5.3 already said in bold they were different things.
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "nothing is on file either way"
    );
    assert!(!r.is_definite());

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
        vec![artist_logic::ObjBinding {
            var: v,
            domain: Some(dom),
        }],
        vec![body],
    );

    let mut s = MapGraphStructure::new()
        .domain(dom, members.clone())
        .closed(corrected);
    for m in &members[..3] {
        s = s.fact(corrected, vec![*m]);
    }
    let three = g.int(3);
    let eq = g.apply(wk::EQ, vec![count, three]);
    let ev = GraphEvaluator::new();
    // Aggregates denote numbers, so the comparison is what carries the verdict.
    let r = ev.eval(&mut g, eq, &s, 100_000);
    assert_ne!(
        r.evidential(),
        Evidential::Refuted,
        "three of five were corrected"
    );
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
            artist_logic::ObjBinding {
                var: reach,
                domain: None,
            },
            artist_logic::ObjBinding {
                var: x,
                domain: Some(dom),
            },
            artist_logic::ObjBinding {
                var: y,
                domain: Some(dom),
            },
        ],
        vec![def, scope],
    );

    let mut s = MapGraphStructure::new()
        .domain(dom, ns.clone())
        .closed(edge);
    for w in ns.windows(2) {
        s = s.fact(edge, vec![w[0], w[1]]);
    }
    let ev = GraphEvaluator::new();
    let r = ev.eval(&mut g, lr, &s, 5_000_000);
    assert_eq!(
        r.evidential(),
        Evidential::Supported,
        "n0 reaches n3 transitively"
    );
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
    assert_eq!(
        r.evidential(),
        Evidential::Supported,
        "widening closed the tail"
    );

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
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "no counterexample, no proof"
    );
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
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        // Present: no longer believed. This structure is authoritative about
        // its one proposition, so it may say Fails rather than Unknown.
        if pred == self.pred && args == [self.arg] {
            Knowledge::Fails
        } else {
            Knowledge::Unknown
        }
    }
    fn known_at(&self, pred: ObjectId, args: &[ObjectId], t: i64) -> Knowledge {
        if pred == self.pred && args == [self.arg] {
            if (1_000..2_000).contains(&t) {
                Knowledge::Holds
            } else {
                Knowledge::Fails
            }
        } else {
            Knowledge::Unknown
        }
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
    let s = Timed {
        pred: prefers,
        arg: tabs,
    };
    let ev = GraphEvaluator::new();

    // Now: refuted.
    assert_eq!(
        ev.eval(&mut g, p, &s, 10_000).evidential(),
        Evidential::Refuted
    );

    // Then: supported. "What did I used to believe" is answerable.
    let then = g.int(1_500);
    let past = g.apply(wk::AT, vec![then, p]);
    assert_eq!(
        ev.eval(&mut g, past, &s, 10_000).evidential(),
        Evidential::Supported
    );

    // And after the switch, refuted again.
    let later = g.int(2_500);
    let after = g.apply(wk::AT, vec![later, p]);
    assert_eq!(
        ev.eval(&mut g, after, &s, 10_000).evidential(),
        Evidential::Refuted
    );
}

// ---- resolver completeness ------------------------------------------------

/// A resolver that holds data but is **not authoritative for this query** — it
/// timed out, its index is partial, the caller lacks permission, or the
/// argument pattern is one it cannot answer. It has no data and says so
/// honestly rather than reporting absence as falsity.
struct Incomplete;
impl GraphStructure for Incomplete {
    fn known(&self, _pred: ObjectId, _args: &[ObjectId]) -> Knowledge {
        Knowledge::Unknown
    }
    /// The trap: the relation *is* closed in general. Under the old contract
    /// this flag alone licensed reading absence as refutation, regardless of
    /// whether the resolver could actually answer the call in front of it.
    fn is_closed(&self, _pred: ObjectId) -> bool {
        true
    }
}

/// A resolver that genuinely was authoritative and found nothing.
struct Authoritative;
impl GraphStructure for Authoritative {
    fn known(&self, _pred: ObjectId, _args: &[ObjectId]) -> Knowledge {
        Knowledge::Fails
    }
}

#[test]
fn an_incomplete_resolver_never_manufactures_a_refutation() {
    let mut g = ObjectGraph::new();
    let p = g.atom("indexed-somewhere");
    let a = g.atom("a");
    let q = g.apply(p, vec![a]);

    let r = GraphEvaluator::new().eval(&mut g, q, &Incomplete, 10_000);
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "a resolver that could not answer must not be read as refuting"
    );

    // And the compounding case, which is why this matters: a bogus Refuted
    // under a negation becomes a confident Supported the store never earned.
    let negated = g.apply(wk::NOT, vec![q]);
    let r = GraphEvaluator::new().eval(&mut g, negated, &Incomplete, 10_000);
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "incompleteness must propagate through negation, not invert into support"
    );
}

#[test]
fn an_authoritative_resolver_still_refutes() {
    let mut g = ObjectGraph::new();
    let p = g.atom("fully-known");
    let a = g.atom("a");
    let q = g.apply(p, vec![a]);

    let r = GraphEvaluator::new().eval(&mut g, q, &Authoritative, 10_000);
    assert_eq!(
        r.evidential(),
        Evidential::Refuted,
        "closed-world negation must still work"
    );

    let negated = g.apply(wk::NOT, vec![q]);
    let r = GraphEvaluator::new().eval(&mut g, negated, &Authoritative, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);
}

// ---- conformance with the written semantics -------------------------------

/// Mutually recursive propositions through `and` / `or`, which Sol flagged as
/// the case path-parity was never shown to handle. The spec (§5.3) claims
/// termination with an `Exact` finding, not a timeout.
#[test]
fn mutual_recursion_through_connectives_terminates() {
    let mut g = ObjectGraph::new();

    // A = (and true (not B)) ; B = (and true (not A))
    // Each depends on the other through a negation, so the parity flips on the
    // way round and the pair oscillates.
    let a = g.alloc();
    let b = g.alloc();
    let quoted_b = g.apply(wk::QUOTE, vec![b]);
    let holds_b = g.apply(wk::HOLDS, vec![quoted_b]);
    let not_b = g.apply(wk::NOT, vec![holds_b]);
    let a_body = g.apply(wk::AND, vec![wk::TOP, not_b]);
    let node_a = g.get(a_body).cloned().expect("built");
    g.define(a, node_a);

    let quoted_a = g.apply(wk::QUOTE, vec![a]);
    let holds_a = g.apply(wk::HOLDS, vec![quoted_a]);
    let not_a = g.apply(wk::NOT, vec![holds_a]);
    let b_body = g.apply(wk::AND, vec![wk::TOP, not_a]);
    let node_b = g.get(b_body).cloned().expect("built");
    g.define(b, node_b);

    let r = GraphEvaluator::new().eval(&mut g, a, &EmptyStructure, 100_000);
    assert_ne!(
        r.compute_status,
        ComputeStatus::BudgetExhausted,
        "mutual recursion must terminate rather than burn the budget"
    );
    assert!(
        r.spent < 100_000,
        "should settle well inside the budget, spent {}",
        r.spent
    );
}

/// An even cycle — parity does *not* flip — is stably ungrounded, so `Open`
/// rather than `Conflicted`. This is the truth-teller shape one level up.
#[test]
fn an_even_negation_cycle_is_open_not_conflicted() {
    let mut g = ObjectGraph::new();
    let p = g.alloc();
    let quoted = g.apply(wk::QUOTE, vec![p]);
    let holds = g.apply(wk::HOLDS, vec![quoted]);
    let twice = g.apply(wk::NOT, vec![holds]);
    let body = g.apply(wk::NOT, vec![twice]);
    let node = g.get(body).cloned().expect("built");
    g.define(p, node);

    let r = GraphEvaluator::new().eval(&mut g, p, &EmptyStructure, 100_000);
    assert_ne!(r.compute_status, ComputeStatus::BudgetExhausted);
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "an even number of negations round the cycle does not oscillate"
    );
}

/// §5.2: one counterexample refutes a universal, but supporting one needs the
/// domain to be enumerable *and* complete. Absence of a counterexample in a
/// domain that might have more members is not support.
#[test]
fn a_universal_over_an_unenumerable_domain_is_not_supported() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let unknown_domain = g.atom("SomeUnenumerableSort");
    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(unknown_domain), body);

    let r = GraphEvaluator::new().eval(&mut g, q, &EmptyStructure, 10_000);
    assert_ne!(
        r.evidential(),
        Evidential::Supported,
        "a universal cannot be supported over a domain nothing can enumerate"
    );
}

// ---- soundness: absence must not become a verdict --------------------------
//
// Every case here comes from an adversarial audit that found the evaluator
// answering confidently where it had no information. The shared shape: when the
// evaluator cannot compute something it must be *partial*, never decisive. The
// spec already says so — "representation is total; evaluation is what is
// allowed to be partial" — and the code did the inverse in six places.

/// `=` over a term it cannot compute must stall, not refute.
///
/// The old `ground()` returned `Some` for every node shape, so two structurally
/// different compounds compared unequal by object id. "The build takes 4
/// minutes" came back `Refuted/Exact`, and its negation `Supported/Exact`.
#[test]
fn equality_on_uncomputable_terms_stalls_rather_than_refuting() {
    let mut g = ObjectGraph::new();
    let build_minutes = g.atom("build-minutes");
    let artist = g.atom("artist");
    let lhs = g.apply(build_minutes, vec![artist]);
    let four = g.int(4);
    let claim = g.apply(wk::EQ, vec![lhs, four]);

    let r = GraphEvaluator::new().eval(&mut g, claim, &EmptyStructure, 10_000);
    assert_ne!(
        r.evidential(),
        Evidential::Refuted,
        "an uncomputable term must not be refuted by comparing expression ids"
    );

    // The compounding case: a fabricated refutation inverts under negation into
    // a confident claim the store never had grounds for.
    let negated = g.apply(wk::NOT, vec![claim]);
    let r = GraphEvaluator::new().eval(&mut g, negated, &EmptyStructure, 10_000);
    assert_ne!(
        r.evidential(),
        Evidential::Supported,
        "negating an unknown must not manufacture support"
    );
}

/// The constraint the fix had to preserve: two *distinct atoms* are still
/// unequal, because `where`-based exception sets depend on it. Loosening
/// `ground()` too far would have broken working functionality while fixing the
/// bug above.
#[test]
fn equality_on_distinct_atoms_still_refutes() {
    let mut g = ObjectGraph::new();
    let (a, b) = (g.atom("mod.rs"), g.atom("lib.rs"));
    let same = g.apply(wk::EQ, vec![a, a]);
    let different = g.apply(wk::EQ, vec![a, b]);

    let ev = GraphEvaluator::new();
    assert_eq!(
        ev.eval(&mut g, same, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );
    assert_eq!(
        ev.eval(&mut g, different, &EmptyStructure, 10_000)
            .evidential(),
        Evidential::Refuted,
        "distinct atoms must stay unequal — exception sets rely on this"
    );
}

/// Literals and externals keep decidable identity too.
#[test]
fn equality_on_literals_and_externals_still_decides() {
    let mut g = ObjectGraph::new();
    let (four, five) = (g.int(4), g.int(5));
    let eq = g.apply(wk::EQ, vec![four, four]);
    let ne = g.apply(wk::EQ, vec![four, five]);

    let ev = GraphEvaluator::new();
    assert_eq!(
        ev.eval(&mut g, eq, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );
    assert_eq!(
        ev.eval(&mut g, ne, &EmptyStructure, 10_000).evidential(),
        Evidential::Refuted
    );
}

/// A refinement type ranges over a *subset*, so probing the whole integer line
/// and calling an excluded value a counterexample is unsound.
///
/// `(forall [(x (Refine Int (λn. 0 ≤ n)))] (0 ≤ x))` — "every non-negative
/// integer is non-negative" — came back **Refuted**, because `infinite()` fired
/// on any domain `members_of` could not enumerate and walked to n = −1.
#[test]
fn a_refinement_domain_is_not_probed_as_the_whole_integer_line() {
    let mut g = ObjectGraph::new();
    let n = g.fresh();
    let zero = g.int(0);
    let nonneg = g.apply(wk::LEQ, vec![zero, n]);
    let pred = g.bind(
        wk::LAMBDA,
        vec![artist_logic::ObjBinding {
            var: n,
            domain: None,
        }],
        vec![nonneg],
    );
    let refined = g.apply(wk::REFINEMENT_TYPE, vec![wk::INT_TYPE, pred]);

    let x = g.fresh();
    let body = g.apply(wk::LEQ, vec![zero, x]);
    let claim = g.quantify(wk::FORALL, x, Some(refined), body);

    let r = GraphEvaluator::new().eval(&mut g, claim, &EmptyStructure, 50_000);
    assert_ne!(
        r.evidential(),
        Evidential::Refuted,
        "a value excluded by the refinement is not a counterexample"
    );
}

/// A domain nothing recognises is unknown, not the integers. Previously any
/// unenumerable domain — an unmodelled sort, a typo — was probed as `Int`.
#[test]
fn an_unrecognised_domain_stalls_rather_than_becoming_the_integers() {
    let mut g = ObjectGraph::new();
    let sort = g.atom("SomeSortNothingModels");
    let p = g.atom("p");
    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let claim = g.quantify(wk::FORALL, v, Some(sort), body);

    let r = GraphEvaluator::new().eval(&mut g, claim, &EmptyStructure, 20_000);
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "an unmodelled domain yields no verdict in either direction"
    );
}

/// Wrong arity must be rejected, not silently truncated into a different
/// proposition that then gets a confident answer.
///
/// `(not P Q)` used to evaluate as `(not P)` — so `(not (p a) (p b))` and
/// `(not (p b) (p a))` gave *opposite* verdicts on the same operand set.
#[test]
fn wrong_arity_is_unsupported_rather_than_truncated() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let (a, b) = (g.atom("a"), g.atom("b"));
    let (pa, pb) = (g.apply(p, vec![a]), g.apply(p, vec![b]));

    let ev = GraphEvaluator::new();
    for (label, expr) in [
        ("not/2", g.apply(wk::NOT, vec![pa, pb])),
        ("implies/1", g.apply(wk::IMPLIES, vec![pa])),
        ("implies/3", g.apply(wk::IMPLIES, vec![pa, pb, pa])),
        ("holds/2", g.apply(wk::HOLDS, vec![pa, pb])),
    ] {
        let r = ev.eval(&mut g, expr, &EmptyStructure, 10_000);
        assert_eq!(
            r.compute_status,
            ComputeStatus::Unsupported,
            "{label} should be rejected, not reinterpreted"
        );
    }
}

/// A modal operator with **no frame behind it** is `Unsupported`, never decided
/// by a resolver.
///
/// `necessarily` and `possibly` mean nothing without an accessibility relation,
/// and `if-counterfactually` means nothing without a similarity ordering over
/// worlds. They used to fall through to the fact table as ordinary binary
/// predicates over node ids, so a resolver marked closed turned "I have no
/// modal semantics" into a confident refutation.
#[test]
fn modal_operators_without_a_frame_are_unsupported() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let inner = g.apply(p, vec![a]);

    let ev = GraphEvaluator::new();
    for op in [wk::NECESSARILY, wk::POSSIBLY] {
        let expr = g.apply(op, vec![inner]);
        // `Authoritative` claims `Fails` for everything — the worst case, since
        // it would happily manufacture a refutation.
        let r = ev.eval(&mut g, expr, &Authoritative, 10_000);
        assert_eq!(
            r.compute_status,
            ComputeStatus::Unsupported,
            "{:?} has no frame here and must say so",
            wk::name_of(op)
        );
        assert_ne!(r.evidential(), Evidential::Refuted);
    }

    let cf = g.apply(wk::COUNTERFACTUAL, vec![inner, inner]);
    let r = ev.eval(&mut g, cf, &Authoritative, 10_000);
    assert_eq!(
        r.compute_status,
        ComputeStatus::Unsupported,
        "no similarity ordering means no counterfactual semantics"
    );
}

// ---- incompleteness must propagate, not be discarded -----------------------

/// A resolver that can list *some* members of a sort but cannot promise there
/// are no others. Real indexes are like this: paged, partial, permission-filtered.
struct PartialSort {
    domain: ObjectId,
    seen: Vec<ObjectId>,
    pred: ObjectId,
}
impl GraphStructure for PartialSort {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        if pred == self.pred && args.len() == 1 && self.seen.contains(&args[0]) {
            Knowledge::Holds
        } else {
            Knowledge::Unknown
        }
    }
    fn extension(&self, domain: ObjectId) -> Option<Extension> {
        (domain == self.domain).then(|| Extension::partial(self.seen.clone()))
    }
}

/// Every member the index could show satisfies the predicate — and that
/// establishes nothing, because the index is not the world.
#[test]
fn a_partial_extension_cannot_support_a_universal() {
    let mut g = ObjectGraph::new();
    let files = g.atom("Files");
    let tested = g.atom("tested");
    let (a, b) = (g.atom("a.rs"), g.atom("b.rs"));
    let s = PartialSort {
        domain: files,
        seen: vec![a, b],
        pred: tested,
    };

    let v = g.fresh();
    let body = g.apply(tested, vec![v]);
    let claim = g.quantify(wk::FORALL, v, Some(files), body);

    let r = GraphEvaluator::new().eval(&mut g, claim, &s, 20_000);
    assert_ne!(
        r.evidential(),
        Evidential::Supported,
        "a partial index must not license a universal over the whole sort"
    );
}

/// …and an existential over a partial extension cannot be refuted, since the
/// witness may be among the members never listed.
#[test]
fn a_partial_extension_cannot_refute_an_existential() {
    let mut g = ObjectGraph::new();
    let files = g.atom("Files");
    let broken = g.atom("broken");
    let tested = g.atom("tested");
    let (a, b) = (g.atom("a.rs"), g.atom("b.rs"));
    let s = PartialSort {
        domain: files,
        seen: vec![a, b],
        pred: tested,
    };

    let v = g.fresh();
    let body = g.apply(broken, vec![v]);
    let claim = g.quantify(wk::EXISTS, v, Some(files), body);

    let r = GraphEvaluator::new().eval(&mut g, claim, &s, 20_000);
    assert_ne!(
        r.evidential(),
        Evidential::Refuted,
        "the witness may be a member the index never showed"
    );
}

/// `where` over-approximates by keeping members it cannot rule out. That guess
/// must not be laundered into an exact count.
#[test]
fn a_guessed_refinement_does_not_produce_an_exact_count() {
    let mut g = ObjectGraph::new();
    let untested = g.atom("untested");
    let (a, b, c) = (g.atom("a"), g.atom("b"), g.atom("c"));
    let base = g.apply(wk::SET_DOMAIN, vec![a, b, c]);

    let x = g.fresh();
    let filter_body = g.apply(untested, vec![x]);
    let filter = g.bind(
        wk::LAMBDA,
        vec![artist_logic::ObjBinding {
            var: x,
            domain: None,
        }],
        vec![filter_body],
    );
    let refined = g.apply(wk::WHERE_DOMAIN, vec![base, filter]);

    let f = g.fresh();
    let counted = g.bind(
        wk::COUNT,
        vec![artist_logic::ObjBinding {
            var: f,
            domain: Some(refined),
        }],
        vec![wk::TOP],
    );
    let three = g.int(3);
    let claim = g.apply(wk::EQ, vec![counted, three]);

    // Nothing is known about `untested`, so all three members are *kept* by the
    // over-approximation — and the count must not therefore be exactly three.
    let r = GraphEvaluator::new().eval(&mut g, claim, &EmptyStructure, 20_000);
    assert_ne!(
        r.evidential(),
        Evidential::Supported,
        "an over-approximated set must not yield an exact cardinality"
    );
}

/// The other half of §5.2's promise: a *bound* decides a comparison long before
/// the scan is exhaustive. Previously every comparison over a partial aggregate
/// stalled, because `number()` returned `None` for anything inexact.
#[test]
fn a_lower_bound_decides_a_comparison_early() {
    let mut g = ObjectGraph::new();
    let fails = g.atom("fails");
    let members: Vec<ObjectId> = ["t1", "t2", "t3", "t4", "t5"]
        .iter()
        .map(|n| g.atom(n))
        .collect();
    let dom = g.apply(wk::SET_DOMAIN, members.clone());

    // Three known failures; two unknown. So lo = 3, hi = 5.
    let mut s = MapGraphStructure::new();
    for m in members.iter().take(3) {
        s = s.fact(fails, vec![*m]);
    }

    let t = g.fresh();
    let body = g.apply(fails, vec![t]);
    let counted = g.bind(
        wk::COUNT,
        vec![artist_logic::ObjBinding {
            var: t,
            domain: Some(dom),
        }],
        vec![body],
    );

    let three = g.int(3);
    let at_least_three = g.apply(wk::LEQ, vec![three, counted]);
    let r = GraphEvaluator::new().eval(&mut g, at_least_three, &s, 50_000);
    assert_eq!(
        r.evidential(),
        Evidential::Supported,
        "lo = 3 settles `3 <= count` without needing the scan to be exact"
    );

    let six = g.int(6);
    let at_most_six = g.apply(wk::LEQ, vec![counted, six]);
    let r = GraphEvaluator::new().eval(&mut g, at_most_six, &s, 50_000);
    assert_eq!(
        r.evidential(),
        Evidential::Supported,
        "hi = 5 settles `count <= 6` the same way"
    );
}

// ---- arithmetic and text: declared in wk::, now actually interpreted -------

/// Each of these was a *well-known* operator with no evaluator case, so it fell
/// through to identity comparison under `=` and came back **Refuted** — a
/// confident wrong answer, not a missing one.
#[test]
fn the_arithmetic_gaps_are_closed() {
    let mut g = ObjectGraph::new();
    let ev = GraphEvaluator::new();
    let n = |g: &mut ObjectGraph, k: i64| g.int(k);

    // Binary `-` (was unary negation only).
    let (ten, six, four) = (n(&mut g, 10), n(&mut g, 6), n(&mut g, 4));
    let sub = g.apply(wk::NEG, vec![ten, six]);
    let claim = g.apply(wk::EQ, vec![sub, four]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    // Unary `-` still negates.
    let neg = g.apply(wk::NEG, vec![four]);
    let minus_four = n(&mut g, -4);
    let claim = g.apply(wk::EQ, vec![neg, minus_four]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    // n-ary `+` and `*`, matching the unbounded arity the kernel advertises.
    let (one, two, three, six2) = (n(&mut g, 1), n(&mut g, 2), n(&mut g, 3), n(&mut g, 6));
    let sum = g.apply(wk::ADD, vec![one, two, three]);
    let claim = g.apply(wk::EQ, vec![sum, six2]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    let prod = g.apply(wk::MUL, vec![two, three, four]);
    let twenty_four = n(&mut g, 24);
    let claim = g.apply(wk::EQ, vec![prod, twenty_four]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    // `mod`.
    let seven = n(&mut g, 7);
    let m = g.apply(wk::MOD, vec![seven, three]);
    let claim = g.apply(wk::EQ, vec![m, one]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    // Division by zero has no value and must stall, never refute.
    let zero = n(&mut g, 0);
    let div0 = g.apply(wk::DIV, vec![four, zero]);
    let claim = g.apply(wk::EQ, vec![div0, four]);
    assert_ne!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Refuted
    );
}

/// §3 calls decimals "exact, ordered" and §5.4 puts arithmetic on literals in
/// the Exact fragment. Neither was true: `number()` handled `Int` only, so
/// `40d1 = 4` was **Refuted** and every decimal comparison stalled.
#[test]
fn decimals_are_exact_and_ordered() {
    let mut g = ObjectGraph::new();
    let ev = GraphEvaluator::new();

    // 40 × 10⁻¹ = 4
    let forty_tenths = g.lit(LiteralValue::Decimal {
        mantissa: 40.into(),
        scale: 1,
    });
    let four = g.int(4);
    let claim = g.apply(wk::EQ, vec![forty_tenths, four]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    // Ordered.
    let forty_one_tenths = g.lit(LiteralValue::Decimal {
        mantissa: 41.into(),
        scale: 1,
    });
    let le = g.apply(wk::LEQ, vec![forty_tenths, forty_one_tenths]);
    assert_eq!(
        ev.eval(&mut g, le, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    // Exact: 1.5 + 1.5 = 3, with no floating-point drift anywhere.
    let one_five = g.lit(LiteralValue::Decimal {
        mantissa: 15.into(),
        scale: 1,
    });
    let sum = g.apply(wk::ADD, vec![one_five, one_five]);
    let three = g.int(3);
    let claim = g.apply(wk::EQ, vec![sum, three]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    // And division stays exact rather than truncating: 1/3 * 3 = 1.
    let (one, three2) = (g.int(1), g.int(3));
    let third = g.apply(wk::DIV, vec![one, three2]);
    let back = g.apply(wk::MUL, vec![third, three2]);
    let claim = g.apply(wk::EQ, vec![back, one]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported,
        "rationals must not truncate the way integer division does"
    );
}

#[test]
fn text_operators_are_interpreted() {
    let mut g = ObjectGraph::new();
    let ev = GraphEvaluator::new();

    let s = g.text("crates/artist-logic");
    let len = g.apply(wk::LEN, vec![s]);
    let nineteen = g.int(19);
    let claim = g.apply(wk::EQ, vec![len, nineteen]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    // `substr` counts characters, not bytes, so a multi-byte character cannot
    // be split in half.
    let uni = g.text("café — naïve");
    let (zero, four) = (g.int(0), g.int(4));
    let head = g.apply(wk::SUBSTR, vec![uni, zero, four]);
    let expect = g.text("café");
    let claim = g.apply(wk::EQ, vec![head, expect]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported,
        "substr must be character-indexed"
    );

    let cafe_len = g.apply(wk::LEN, vec![expect]);
    let claim = g.apply(wk::EQ, vec![cafe_len, four]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported,
        "len counts characters: café is 4, not 5 bytes"
    );
}

// ---- time: derivations that were declared and never performed --------------

/// A structure with a real timeline: the suite fails, then passes.
struct Timeline {
    passes: ObjectId,
    suite: ObjectId,
    /// A symbolic instant the structure can denote.
    session_start: ObjectId,
}
impl GraphStructure for Timeline {
    fn known(&self, _pred: ObjectId, _args: &[ObjectId]) -> Knowledge {
        Knowledge::Unknown
    }
    fn known_at(&self, pred: ObjectId, args: &[ObjectId], t: i64) -> Knowledge {
        if pred == self.passes && args == [self.suite] {
            if t >= 100 {
                Knowledge::Holds
            } else {
                Knowledge::Fails
            }
        } else {
            Knowledge::Unknown
        }
    }
    fn instants(&self) -> Vec<i64> {
        vec![50, 100]
    }
    fn denote_instant(&self, term: ObjectId) -> Option<i64> {
        (term == self.session_start).then_some(50)
    }
}

/// `at` used to demand an integer literal, so the spec's own §9 exemplar
/// `(at t3 …)` did not evaluate and every temporal claim had to carry a raw
/// epoch number.
#[test]
fn a_symbolic_instant_resolves_through_the_structure() {
    let mut g = ObjectGraph::new();
    let passes = g.atom("passes");
    let suite = g.atom("suite");
    let session_start = g.atom("session-start-7");
    let s = Timeline {
        passes,
        suite,
        session_start,
    };

    let claim = g.apply(passes, vec![suite]);
    let then = g.apply(wk::AT, vec![session_start, claim]);
    let r = GraphEvaluator::new().eval(&mut g, then, &s, 10_000);
    assert_eq!(
        r.evidential(),
        Evidential::Refuted,
        "at the named instant the suite was failing"
    );
}

/// `always` / `eventually` quantify over the structure's instants. The comment
/// on `at` has claimed this derivation since the file was written; nothing
/// performed it, and `instants()` was never called by the evaluator at all.
#[test]
fn always_and_eventually_quantify_over_instants() {
    let mut g = ObjectGraph::new();
    let passes = g.atom("passes");
    let suite = g.atom("suite");
    let session_start = g.atom("session-start-7");
    let s = Timeline {
        passes,
        suite,
        session_start,
    };
    let ev = GraphEvaluator::new();

    let claim = g.apply(passes, vec![suite]);

    let ever = g.apply(wk::EVENTUALLY, vec![claim]);
    assert_eq!(
        ev.eval(&mut g, ever, &s, 10_000).evidential(),
        Evidential::Supported,
        "it passes at t=100"
    );

    let forever = g.apply(wk::ALWAYS, vec![claim]);
    assert_eq!(
        ev.eval(&mut g, forever, &s, 10_000).evidential(),
        Evidential::Refuted,
        "it was failing at t=50"
    );
}

#[test]
fn before_and_during_order_instants() {
    let mut g = ObjectGraph::new();
    let passes = g.atom("passes");
    let suite = g.atom("suite");
    let session_start = g.atom("session-start-7");
    let s = Timeline {
        passes,
        suite,
        session_start,
    };
    let ev = GraphEvaluator::new();

    // Symbolic on the left, literal on the right.
    let hundred = g.int(100);
    let ord = g.apply(wk::BEFORE, vec![session_start, hundred]);
    assert_eq!(
        ev.eval(&mut g, ord, &s, 10_000).evidential(),
        Evidential::Supported
    );

    let back = g.apply(wk::BEFORE, vec![hundred, session_start]);
    assert_eq!(
        ev.eval(&mut g, back, &s, 10_000).evidential(),
        Evidential::Refuted
    );

    // Half-open, so adjacent intervals tile without overlap.
    let (zero, ninety) = (g.int(0), g.int(90));
    let iv = g.apply(wk::INTERVAL, vec![zero, ninety]);
    let inside = g.apply(wk::DURING, vec![session_start, iv]);
    assert_eq!(
        ev.eval(&mut g, inside, &s, 10_000).evidential(),
        Evidential::Supported
    );

    let outside = g.apply(wk::DURING, vec![hundred, iv]);
    assert_eq!(
        ev.eval(&mut g, outside, &s, 10_000).evidential(),
        Evidential::Refuted
    );
}

// ---- modal and reflective, given a structure that supplies a frame ---------

/// Two worlds, one accessible from the other, with the proposition true in
/// exactly one of them.
struct Frame {
    green: ObjectId,
    build: ObjectId,
    w_now: ObjectId,
    w_alt: ObjectId,
}
impl GraphStructure for Frame {
    fn known(&self, _pred: ObjectId, _args: &[ObjectId]) -> Knowledge {
        Knowledge::Unknown
    }
    fn known_in(&self, pred: ObjectId, args: &[ObjectId], world: ObjectId) -> Knowledge {
        if pred == self.green && args == [self.build] {
            if world == self.w_now {
                Knowledge::Holds
            } else {
                Knowledge::Fails
            }
        } else {
            Knowledge::Unknown
        }
    }
    fn worlds(&self) -> Vec<ObjectId> {
        vec![self.w_now, self.w_alt]
    }
    /// Everything reaches everything — the simplest frame that is still a frame.
    fn accessible(&self, _from: ObjectId, _to: ObjectId) -> bool {
        true
    }
    fn closest_worlds(&self, _from: ObjectId, _condition: ObjectId) -> Option<Vec<ObjectId>> {
        Some(vec![self.w_alt])
    }
}

#[test]
fn modal_operators_quantify_over_accessible_worlds() {
    let mut g = ObjectGraph::new();
    let green = g.atom("green");
    let build = g.atom("build");
    let (w_now, w_alt) = (g.atom("w-now"), g.atom("w-alt"));
    let s = Frame {
        green,
        build,
        w_now,
        w_alt,
    };
    let ev = GraphEvaluator::new();

    let claim = g.apply(green, vec![build]);

    let maybe = g.apply(wk::POSSIBLY, vec![claim]);
    assert_eq!(
        ev.eval(&mut g, maybe, &s, 20_000).evidential(),
        Evidential::Supported,
        "it is green in w-now, so possibly green"
    );

    let must = g.apply(wk::NECESSARILY, vec![claim]);
    assert_eq!(
        ev.eval(&mut g, must, &s, 20_000).evidential(),
        Evidential::Refuted,
        "it is not green in w-alt, so not necessarily green"
    );

    // `in-world` pins evaluation to one world.
    let here = g.apply(wk::IN_WORLD, vec![w_now, claim]);
    assert_eq!(
        ev.eval(&mut g, here, &s, 20_000).evidential(),
        Evidential::Supported
    );
    let there = g.apply(wk::IN_WORLD, vec![w_alt, claim]);
    assert_eq!(
        ev.eval(&mut g, there, &s, 20_000).evidential(),
        Evidential::Refuted
    );
}

/// A counterfactual is evaluated in the closest worlds satisfying its
/// antecedent — an ordering the *structure* supplies, because there is no
/// agreed one and inventing one silently would be worse than saying nothing.
#[test]
fn counterfactuals_use_the_structures_similarity_ordering() {
    let mut g = ObjectGraph::new();
    let green = g.atom("green");
    let build = g.atom("build");
    let (w_now, w_alt) = (g.atom("w-now"), g.atom("w-alt"));
    let s = Frame {
        green,
        build,
        w_now,
        w_alt,
    };

    let claim = g.apply(green, vec![build]);
    let cond = g.atom("had-we-reverted");
    let cf = g.apply(wk::COUNTERFACTUAL, vec![cond, claim]);

    // The closest antecedent-world is w-alt, where the build is not green.
    let r = GraphEvaluator::new().eval(&mut g, cf, &s, 20_000);
    assert_eq!(r.evidential(), Evidential::Refuted);
}

/// `eval` is the inverse of `quote`, and the pair is what makes the language
/// reflective rather than merely quotational.
#[test]
fn eval_undoes_quotation() {
    let mut g = ObjectGraph::new();
    let ev = GraphEvaluator::new();

    let (two, four) = (g.int(2), g.int(4));
    let sum = g.apply(wk::ADD, vec![two, two]);
    let claim = g.apply(wk::EQ, vec![sum, four]);
    let quoted = g.apply(wk::QUOTE, vec![claim]);

    // Quoted, it is inert.
    let r = ev.eval(&mut g, quoted, &EmptyStructure, 10_000);
    assert_ne!(
        r.evidential(),
        Evidential::Supported,
        "a quotation is not evaluated"
    );

    // Evaluated, it decides.
    let run = g.apply(wk::EVAL, vec![quoted]);
    assert_eq!(
        ev.eval(&mut g, run, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );
}

/// `provable` reports derivability *within the budget*, so it can support but
/// must never refute — failing to find a derivation is not a proof that none
/// exists.
#[test]
fn provable_supports_but_never_refutes() {
    let mut g = ObjectGraph::new();
    let ev = GraphEvaluator::new();

    let (two, four) = (g.int(2), g.int(4));
    let sum = g.apply(wk::ADD, vec![two, two]);
    let truth = g.apply(wk::EQ, vec![sum, four]);
    let quoted = g.apply(wk::QUOTE, vec![truth]);
    let claim = g.apply(wk::PROVABLE, vec![quoted]);
    assert_eq!(
        ev.eval(&mut g, claim, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported
    );

    // Something unknowable is not *disprovable*.
    let p = g.atom("p");
    let a = g.atom("a");
    let unknown = g.apply(p, vec![a]);
    let q2 = g.apply(wk::QUOTE, vec![unknown]);
    let claim = g.apply(wk::PROVABLE, vec![q2]);
    let r = ev.eval(&mut g, claim, &EmptyStructure, 10_000);
    assert_ne!(
        r.evidential(),
        Evidential::Refuted,
        "absence of a derivation is not a derivation of absence"
    );
}

// ---- quantities: units defined as data, not built in -----------------------

/// The point of §4: a new unit costs a *write*, never a migration.
struct ScaleTable {
    rows: Vec<(ObjectId, i64, ObjectId)>,
}
impl GraphStructure for ScaleTable {
    fn known(&self, _pred: ObjectId, _args: &[ObjectId]) -> Knowledge {
        Knowledge::Unknown
    }
    fn scale(&self, unit: ObjectId) -> Option<(num_bigint::BigInt, ObjectId)> {
        self.rows
            .iter()
            .find(|(u, _, _)| *u == unit)
            .map(|(_, f, base)| (num_bigint::BigInt::from(*f), *base))
    }
}

#[test]
fn quantities_compare_across_units_given_only_scale_facts() {
    let mut g = ObjectGraph::new();
    let (minutes, seconds) = (g.atom("minutes"), g.atom("seconds"));
    let (mb, bytes) = (g.atom("mb"), g.atom("bytes"));
    let gb = g.atom("gb");
    let s = ScaleTable {
        rows: vec![
            (minutes, 60, seconds),
            (mb, 1_048_576, bytes),
            (gb, 1_073_741_824, bytes),
        ],
    };
    let ev = GraphEvaluator::new();

    // "The build takes 4 minutes" versus "240 seconds" — equal, and the
    // evaluator was told nothing about time.
    let (four, two_forty) = (g.int(4), g.int(240));
    let q1 = g.apply(wk::QUANTITY, vec![four, minutes]);
    let q2 = g.apply(wk::QUANTITY, vec![two_forty, seconds]);
    let eq = g.apply(wk::EQ, vec![q1, q2]);
    assert_eq!(
        ev.eval(&mut g, eq, &s, 20_000).evidential(),
        Evidential::Supported
    );

    // And ordered across units: 300 MB < 1 GB.
    let (three_hundred, one) = (g.int(300), g.int(1));
    let a = g.apply(wk::QUANTITY, vec![three_hundred, mb]);
    let b = g.apply(wk::QUANTITY, vec![one, gb]);
    let le = g.apply(wk::LEQ, vec![a, b]);
    assert_eq!(
        ev.eval(&mut g, le, &s, 20_000).evidential(),
        Evidential::Supported
    );

    let ge = g.apply(wk::LEQ, vec![b, a]);
    assert_eq!(
        ev.eval(&mut g, ge, &s, 20_000).evidential(),
        Evidential::Refuted
    );
}

/// A unit with no conversion known yields no verdict — never a wrong one.
#[test]
fn an_unknown_unit_stalls_rather_than_comparing() {
    let mut g = ObjectGraph::new();
    let (minutes, seconds) = (g.atom("minutes"), g.atom("seconds"));
    let furlongs = g.atom("furlongs");
    let s = ScaleTable {
        rows: vec![(minutes, 60, seconds)],
    };

    let (four, two_forty) = (g.int(4), g.int(240));
    let q1 = g.apply(wk::QUANTITY, vec![four, furlongs]);
    let q2 = g.apply(wk::QUANTITY, vec![two_forty, seconds]);
    let eq = g.apply(wk::EQ, vec![q1, q2]);

    let r = GraphEvaluator::new().eval(&mut g, eq, &s, 20_000);
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "no conversion is known, so nothing is decided in either direction"
    );
}

/// A cyclic `scale` chain is something a store can perfectly well contain, and
/// must not spin the evaluator forever.
#[test]
fn a_cyclic_scale_chain_terminates() {
    let mut g = ObjectGraph::new();
    let (a, b) = (g.atom("a"), g.atom("b"));
    let s = ScaleTable {
        rows: vec![(a, 2, b), (b, 3, a)],
    };

    let one = g.int(1);
    let qa = g.apply(wk::QUANTITY, vec![one, a]);
    let qb = g.apply(wk::QUANTITY, vec![one, b]);
    let eq = g.apply(wk::EQ, vec![qa, qb]);

    // The only requirement is that it returns.
    let r = GraphEvaluator::new().eval(&mut g, eq, &s, 20_000);
    let _ = r.evidential();
}

// ---- rules that actually fire, and defaults that can be defeated -----------

/// A store holding both facts and rules — the shape a memory actually has.
struct WithRules {
    facts: Vec<(ObjectId, Vec<ObjectId>)>,
    /// Rules keyed by the predicate they conclude.
    rules: Vec<(ObjectId, ObjectId)>,
}
impl GraphStructure for WithRules {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        if self.facts.iter().any(|(p, a)| *p == pred && a == args) {
            Knowledge::Holds
        } else {
            Knowledge::Unknown
        }
    }
    fn rules(&self, pred: ObjectId) -> Vec<ObjectId> {
        self.rules
            .iter()
            .filter(|(p, _)| *p == pred)
            .map(|(_, r)| *r)
            .collect()
    }
}

/// The example from the plan. Store the rule and the fact, ask the question,
/// get an answer — which required *deriving*, not looking up.
#[test]
fn a_stored_rule_fires() {
    let mut g = ObjectGraph::new();
    let touches = g.atom("touches-parser");
    let needs = g.atom("needs-review");
    let commits = g.atom("Commits");
    let c = g.atom("commit-4dac419");

    // (forall [(x Commits)] (implies (touches-parser x) (needs-review x)))
    let x = g.fresh();
    let ante = g.apply(touches, vec![x]);
    let conseq = g.apply(needs, vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![ante, conseq]);
    let rule = g.quantify(wk::FORALL, x, Some(commits), imp);

    let s = WithRules {
        facts: vec![(touches, vec![c])],
        rules: vec![(needs, rule)],
    };

    let goal = g.apply(needs, vec![c]);
    let r = GraphEvaluator::new().eval(&mut g, goal, &s, 50_000);
    assert_eq!(
        r.evidential(),
        Evidential::Supported,
        "the rule plus the fact should derive the conclusion"
    );
}

/// A rule whose antecedent is not satisfied derives nothing — and in
/// particular must not *refute* the goal, since another route might establish
/// it.
#[test]
fn a_rule_whose_antecedent_fails_derives_nothing() {
    let mut g = ObjectGraph::new();
    let touches = g.atom("touches-parser");
    let needs = g.atom("needs-review");
    let commits = g.atom("Commits");
    let other = g.atom("commit-unrelated");

    let x = g.fresh();
    let ante = g.apply(touches, vec![x]);
    let conseq = g.apply(needs, vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![ante, conseq]);
    let rule = g.quantify(wk::FORALL, x, Some(commits), imp);

    let s = WithRules {
        facts: Vec::new(),
        rules: vec![(needs, rule)],
    };
    let goal = g.apply(needs, vec![other]);
    let r = GraphEvaluator::new().eval(&mut g, goal, &s, 50_000);
    assert_eq!(r.evidential(), Evidential::Open);
}

/// Chains, and a rule that leads back to its own conclusion must terminate
/// rather than spin.
#[test]
fn rule_chains_terminate() {
    let mut g = ObjectGraph::new();
    let (a, b, c) = (g.atom("a"), g.atom("b"), g.atom("c"));
    let d = g.atom("D");
    let item = g.atom("item");

    let mk = |g: &mut ObjectGraph, from: ObjectId, to: ObjectId| {
        let v = g.fresh();
        let ante = g.apply(from, vec![v]);
        let conseq = g.apply(to, vec![v]);
        let imp = g.apply(wk::IMPLIES, vec![ante, conseq]);
        g.quantify(wk::FORALL, v, Some(d), imp)
    };
    let a_to_b = mk(&mut g, a, b);
    let b_to_c = mk(&mut g, b, c);
    // …and a cycle back.
    let c_to_a = mk(&mut g, c, a);

    let s = WithRules {
        facts: vec![(a, vec![item])],
        rules: vec![(b, a_to_b), (c, b_to_c), (a, c_to_a)],
    };

    let goal = g.apply(c, vec![item]);
    let r = GraphEvaluator::new().eval(&mut g, goal, &s, 100_000);
    assert_eq!(
        r.evidential(),
        Evidential::Supported,
        "a → b → c should chain"
    );
    assert_ne!(
        r.compute_status,
        ComputeStatus::BudgetExhausted,
        "the cycle back to `a` must not spin"
    );
}

/// `usually` is a default: it supports *defeasibly*, so nothing
/// downstream can promote it into a *settled reading*. Defeasibility lives on
/// `Derivation`, not on the strength lattice: a default can be overwhelmingly
/// well attested and still revisable.
///
/// The default has to be **on record**. This test used to run against
/// `EmptyStructure` and assert `Supported/Partial`, which made wrapping a claim
/// in `usually` a free upgrade from "no idea" — and reported `Exact` while doing
/// it, so the evaluator claimed to have finished thinking about a proposition
/// nothing had ever mentioned.
#[test]
fn a_default_supports_defeasibly() {
    let mut g = ObjectGraph::new();
    let fast = g.atom("fast");
    let build = g.atom("build");
    let claim = g.apply(fast, vec![build]);
    let def = g.apply(wk::USUALLY, vec![claim]);

    let s = MapGraphStructure::new().fact(wk::USUALLY, vec![claim]);
    let r = GraphEvaluator::new().eval(&mut g, def, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);
    assert_eq!(r.support, Bound::Certain);
    assert_eq!(
        r.derivation,
        artist_logic::evidence::Derivation::Default,
        "a default is defeasible, not weak — those are different axes"
    );
    assert!(!r.is_definite(), "a default is not a definite reading");
}

/// …and with no default on record, `usually` adds nothing.
#[test]
fn hedging_a_claim_nobody_has_made_is_not_evidence_for_it() {
    let mut g = ObjectGraph::new();
    let fast = g.atom("fast");
    let build = g.atom("build");
    let claim = g.apply(fast, vec![build]);
    let def = g.apply(wk::USUALLY, vec![claim]);

    let r = GraphEvaluator::new().eval(&mut g, def, &EmptyStructure, 10_000);
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "`usually` over an empty store must report the absence it found"
    );
    assert_ne!(
        r.compute_status,
        ComputeStatus::Exact,
        "and must not claim to have finished thinking about it"
    );
}

/// …and explicit counter-evidence defeats it outright.
#[test]
fn counter_evidence_defeats_a_default() {
    let mut g = ObjectGraph::new();
    let fast = g.atom("fast");
    let build = g.atom("build");
    let claim = g.apply(fast, vec![build]);
    let def = g.apply(wk::USUALLY, vec![claim]);

    // A structure authoritative that the build is *not* fast.
    let r = GraphEvaluator::new().eval(&mut g, def, &Authoritative, 10_000);
    assert_eq!(r.evidential(), Evidential::Refuted);
}

/// "Prefer X unless Y" — the shape most of this memory is made of.
#[test]
fn an_exception_defeats_the_rule_it_guards() {
    let mut g = ObjectGraph::new();
    let use_tabs = g.atom("use-tabs");
    let generated = g.atom("generated-file");
    let f = g.atom("f");

    let rule = g.apply(use_tabs, vec![f]);
    let exception = g.apply(generated, vec![f]);
    let guarded = g.apply(wk::UNLESS, vec![exception, rule]);
    let ev = GraphEvaluator::new();

    // No exception in force: the rule applies.
    let s = WithRules {
        facts: vec![(use_tabs, vec![f])],
        rules: Vec::new(),
    };
    assert_eq!(
        ev.eval(&mut g, guarded, &s, 20_000).evidential(),
        Evidential::Supported
    );

    // Exception holds: defeated, and specifically *not* refuted — the rule did
    // not become false, it stopped applying.
    let s = WithRules {
        facts: vec![(use_tabs, vec![f]), (generated, vec![f])],
        rules: Vec::new(),
    };
    let r = ev.eval(&mut g, guarded, &s, 20_000);
    assert_eq!(r.evidential(), Evidential::Open);
}

#[test]
fn preference_is_a_strict_order() {
    let mut g = ObjectGraph::new();
    let (tabs, spaces) = (g.atom("tabs"), g.atom("spaces"));
    let s = WithRules {
        facts: vec![(wk::PREFER, vec![tabs, spaces])],
        rules: Vec::new(),
    };
    let ev = GraphEvaluator::new();

    let p = g.apply(wk::PREFER, vec![tabs, spaces]);
    assert_eq!(
        ev.eval(&mut g, p, &s, 10_000).evidential(),
        Evidential::Supported
    );

    // Asymmetric: the stored preference refutes its converse.
    let back = g.apply(wk::PREFER, vec![spaces, tabs]);
    assert_eq!(
        ev.eval(&mut g, back, &s, 10_000).evidential(),
        Evidential::Refuted
    );

    // Irreflexive.
    let self_pref = g.apply(wk::PREFER, vec![tabs, tabs]);
    assert_eq!(
        ev.eval(&mut g, self_pref, &s, 10_000).evidential(),
        Evidential::Refuted
    );
}
