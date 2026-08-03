//! Graded belief that composes, and a term that can read it.
//!
//! `EvaluationResult::credence` was written at exactly one site and read at
//! none; `Credence::combine` had no caller at all. So the store could grade a
//! proposition, the evaluator could carry the grade through a single lookup, and
//! `(and (p a) (q a))` threw it away — with no operator anywhere able to ask
//! about it. A field that only ever gets written is a claim about capability.
//!
//! The composition rules here are deliberately the ones that need **no
//! independence assumption**: a conjunction is no likelier than its least likely
//! conjunct, a disjunction no less likely than its likeliest disjunct. Adding
//! log-odds across a conjunction — which is what `combine` does, correctly, for
//! independent evidence bearing on *one* proposition — would be how a system
//! ends up confident that six 90%-likely things are all true at once.

use artist_logic::ObjectGraph;
use artist_logic::evidence::{ComputeStatus, Credence, Evidential};
use artist_logic::graph_eval::{EmptyStructure, GraphEvaluator, MapGraphStructure};
use artist_logic::object::wk;

/// A conjunction is no more likely than its least likely part, and the floor is
/// honestly vacuous — two individually near-certain claims can be jointly
/// impossible, and no bound below says otherwise without an assumption nobody
/// made.
#[test]
fn a_conjunction_takes_the_weakest_ceiling() {
    let mut g = ObjectGraph::new();
    let (p, q, a) = (g.atom("p"), g.atom("q"), g.atom("a"));
    let (pa, qa) = (g.apply(p, vec![a]), g.apply(q, vec![a]));
    let both = g.apply(wk::AND, vec![pa, qa]);
    let s = MapGraphStructure::new()
        .fact(p, vec![a])
        .fact(q, vec![a])
        .graded(pa, 3_000, 4_000)
        .graded(qa, 1_000, 2_000);

    let r = GraphEvaluator::new().eval(&mut g, both, &s, 100_000);
    let c = r
        .credence
        .expect("a conjunction of graded claims is graded");
    assert_eq!(c.hi, 2_000, "no likelier than the least likely conjunct");
    assert_eq!(
        c.lo,
        Credence::VACUOUS.lo,
        "and no floor without an independence claim"
    );
}

/// The mirror. A disjunction inherits its likeliest disjunct as a floor.
#[test]
fn a_disjunction_takes_the_strongest_floor() {
    let mut g = ObjectGraph::new();
    let (p, q, a) = (g.atom("p"), g.atom("q"), g.atom("a"));
    let (pa, qa) = (g.apply(p, vec![a]), g.apply(q, vec![a]));
    let either = g.apply(wk::OR, vec![pa, qa]);
    let s = MapGraphStructure::new()
        .fact(p, vec![a])
        .fact(q, vec![a])
        .graded(pa, 3_000, 4_000)
        .graded(qa, 1_000, 2_000);

    let c = GraphEvaluator::new()
        .eval(&mut g, either, &s, 100_000)
        .credence
        .expect("graded");
    assert_eq!(c.lo, 3_000);
    assert_eq!(c.hi, Credence::VACUOUS.hi);
}

/// An ungraded part does not poison the composite. The bounds taken hold over
/// **any** subset of the parts, so skipping the ungraded ones weakens the
/// interval and cannot falsify it.
#[test]
fn an_ungraded_conjunct_is_skipped_not_fatal() {
    let mut g = ObjectGraph::new();
    let (p, q, a) = (g.atom("p"), g.atom("q"), g.atom("a"));
    let (pa, qa) = (g.apply(p, vec![a]), g.apply(q, vec![a]));
    let both = g.apply(wk::AND, vec![pa, qa]);
    let s = MapGraphStructure::new()
        .fact(p, vec![a])
        .fact(q, vec![a])
        .graded(pa, 3_000, 4_000);

    let c = GraphEvaluator::new()
        .eval(&mut g, both, &s, 100_000)
        .credence
        .expect("graded");
    assert_eq!(c.hi, 4_000);
    assert_eq!(
        c.lo,
        Credence::VACUOUS.lo,
        "and the graded conjunct's floor is *not* published as the conjunction's — \
         it says nothing about a conjunct nobody graded"
    );
}

/// Negation flips the interval, which is what log-odds are for.
#[test]
fn negation_reflects_the_interval() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let not_pa = g.apply(wk::NOT, vec![pa]);
    let s = MapGraphStructure::new()
        .fact(p, vec![a])
        .graded(pa, 1_000, 4_000);

    let c = GraphEvaluator::new()
        .eval(&mut g, not_pa, &s, 100_000)
        .credence
        .expect("graded");
    assert_eq!((c.lo, c.hi), (-4_000, -1_000));
}

/// `(likely P)` reads it. Supported when the whole interval is above even odds,
/// refuted when it is below, and genuinely `Open` when it straddles.
#[test]
fn likely_reads_the_interval() {
    let mut g = ObjectGraph::new();
    let (p, a, b, c) = (g.atom("p"), g.atom("a"), g.atom("b"), g.atom("c"));
    let (pa, pb, pc) = (
        g.apply(p, vec![a]),
        g.apply(p, vec![b]),
        g.apply(p, vec![c]),
    );
    let s = MapGraphStructure::new()
        .fact(p, vec![a])
        .fact(p, vec![b])
        .fact(p, vec![c])
        .graded(pa, 1_000, 4_000)
        .graded(pb, -4_000, -1_000)
        .graded(pc, -1_000, 1_000);

    let ev = |g: &mut ObjectGraph, node| GraphEvaluator::new().eval(g, node, &s, 100_000);
    let q = g.apply(wk::LIKELY, vec![pa]);
    assert_eq!(ev(&mut g, q).evidential(), Evidential::Supported);
    let q = g.apply(wk::LIKELY, vec![pb]);
    assert_eq!(ev(&mut g, q).evidential(), Evidential::Refuted);
    let q = g.apply(wk::LIKELY, vec![pc]);
    let r = ev(&mut g, q);
    assert_eq!(r.evidential(), Evidential::Open);
    assert_eq!(
        r.compute_status,
        ComputeStatus::Exact,
        "graded, and it straddles"
    );
}

/// **Ungraded is not even odds.** A store asked about something it cannot grade
/// returns nothing, and that must stay distinguishable from a wide interval — it
/// is the one thing the unawareness literature agrees on, and the reason the
/// field is an `Option` rather than defaulting to `VACUOUS`.
#[test]
fn an_ungraded_proposition_stalls_rather_than_straddling() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let q = g.apply(wk::LIKELY, vec![pa]);
    let r = GraphEvaluator::new().eval(&mut g, q, &EmptyStructure, 100_000);
    assert_eq!(r.evidential(), Evidential::Open);
    assert_eq!(
        r.compute_status,
        ComputeStatus::Stalled,
        "no interval at all, which a caller can tell from a wide one"
    );
}

/// The ledger pools independent evidence in log-odds — and that *is* where
/// `combine` belongs, unlike across a conjunction.
#[test]
fn the_ledger_pools_independent_evidence() {
    use artist_logic::ObjectId;
    use artist_logic::evidence::{Assertion, Evidence, EvidenceLedger, Polarity};

    let prop = ObjectId(1);
    let mut led = EvidenceLedger::new();
    led.assert(Assertion {
        id: ObjectId(10),
        proposition: prop,
        polarity: Polarity::Affirm,
        asserting_agent: Some(ObjectId(99)),
        world: None,
        valid_from: None,
        valid_to: None,
        modality: None,
        interpretation: None,
        scope: None,
        recorded_at: 0,
    });
    for (id, src) in [(ObjectId(20), ObjectId(2)), (ObjectId(21), ObjectId(3))] {
        led.record(Evidence {
            id,
            target: prop,
            polarity: Polarity::Affirm,
            source: Some(src),
            source_span: None,
            transformation: None,
            derived_from: Vec::new(),
            llr_milli: Some(1_000),
            created_by: None,
        });
    }
    let c = led.credence_of(prop).expect("evidence bears on it");
    assert_eq!(
        c,
        Credence::point(2_000),
        "two independent sources compound"
    );

    // …and the agent who asserted it is nameable, which is what a certificate
    // needs and what naming the predicate never gave.
    assert_eq!(
        led.attribution(prop),
        vec![ObjectId(99), ObjectId(2), ObjectId(3)]
    );
}
