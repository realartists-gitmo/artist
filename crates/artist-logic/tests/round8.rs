//! Round six's open findings — the ones the first pass reported and did not fix.
//!
//! The common thread is a guard that exists and is not reached. `Certify`'s
//! determinacy check lived on two arms out of six; the never-complete rule for
//! `Prop` was escapable by spelling it `(sort Prop)`; `MAX_DEPTH` was tuned five
//! times past the stack it actually runs on; the registry's snapshot check ran
//! after the field it inspects had been overwritten. A guard nothing reaches is
//! indistinguishable from no guard, and reads better in review.

use artist_logic::evidence::{Credence, Determinacy, Evidential};
use artist_logic::graph_eval::{
    EmptyStructure, Extension, GraphEvaluator, GraphStructure, Knowledge, MapGraphStructure,
};
use artist_logic::object::wk;
use artist_logic::{ObjectGraph, ObjectId};

/// Declares nothing and refuses everything — an authoritative absence over a
/// proposition with no sharp condition.
struct VagueClosed;
impl GraphStructure for VagueClosed {
    fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
        Knowledge::Fails
    }
    fn determinacy(&self, _p: ObjectId) -> Determinacy {
        Determinacy::Indeterminate
    }
}

/// **The gate has to be on every store-answered verdict.** Four reserved
/// relations reached one without meeting it, and certified negation-as-failure
/// over a proposition the store had *declared* borderline.
#[test]
fn certification_gates_the_reserved_relations_too() {
    let mut g = ObjectGraph::new();
    let (a, b) = (g.atom("a"), g.atom("b"));
    let p = g.atom("p");
    let inner = g.apply(p, vec![a]);
    let ev = GraphEvaluator::new();

    let cases = [
        ("same-as", g.apply(wk::SAME_AS, vec![a, b])),
        ("prefer", g.apply(wk::PREFER, vec![a, b])),
        ("obliged", g.apply(wk::OBLIGED, vec![inner])),
        ("permitted", g.apply(wk::PERMITTED, vec![inner])),
    ];
    for (name, claim) in cases {
        let negated = g.apply(wk::NOT, vec![claim]);
        let r = ev.certify(&mut g, negated, &VagueClosed, 20_000);
        assert!(
            !r.is_definite(),
            "{name}: certified a classical step over a declared-indeterminate proposition"
        );
        assert_eq!(
            r.determinacy,
            Determinacy::Indeterminate,
            "{name}: the declaration must survive into the answer"
        );
    }
}

/// **The never-complete rule for `Prop` must not be escapable by spelling.** A
/// store's propositions are never all of them, and `(sort Prop)` reached the
/// store's own completeness flag with the override skipped.
#[test]
fn the_sort_spelling_does_not_complete_prop() {
    struct OneProp(ObjectId);
    impl GraphStructure for OneProp {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Holds
        }
        fn extension(&self, _d: ObjectId) -> Option<Extension> {
            Some(Extension::complete(vec![self.0]))
        }
    }
    let mut g = ObjectGraph::new();
    let (knows, k) = (g.atom("knows"), g.atom("k"));
    let s = OneProp(k);
    let ev = GraphEvaluator::new();

    let v = g.fresh();
    let body = g.apply(knows, vec![v]);
    let bare = g.quantify(wk::FORALL, v, Some(wk::PROP_TYPE), body);
    let spelled = {
        let d = g.apply(wk::SORT_DOMAIN, vec![wk::PROP_TYPE]);
        g.quantify(wk::FORALL, v, Some(d), body)
    };
    assert_ne!(
        ev.eval(&mut g, bare, &s, 50_000).evidential(),
        Evidential::Supported
    );
    assert_ne!(
        ev.eval(&mut g, spelled, &s, 50_000).evidential(),
        Evidential::Supported,
        "a universal over every proposition, from a one-element store"
    );
}

/// **A declared `Total` has to reach the result.** `Unknown` is the default and
/// means *nobody has said*, so a plain `max` over the ordering made it a floor —
/// and the hook's entire positive answer unreachable.
#[test]
fn a_declared_totality_survives_the_merge() {
    struct Sharp;
    impl GraphStructure for Sharp {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Holds
        }
        fn determinacy(&self, _p: ObjectId) -> Determinacy {
            Determinacy::Total
        }
    }
    assert_eq!(
        Determinacy::Unknown.merge(Determinacy::Total),
        Determinacy::Total
    );
    assert_eq!(
        Determinacy::Indeterminate.merge(Determinacy::Total),
        Determinacy::Indeterminate,
        "…but a claim of borderline still dominates"
    );

    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let r = GraphEvaluator::new().eval(&mut g, claim, &Sharp, 10_000);
    assert_eq!(r.determinacy, Determinacy::Total);
}

/// **A crash is not one of the four compute statuses**, and the depth guard has
/// to fit the stack it actually runs on — `cargo test` is a debug build.
#[test]
fn deep_nesting_stalls_rather_than_overflowing() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let mut node = g.apply(p, vec![a]);
    for _ in 0..400 {
        node = g.apply(wk::NOT, vec![node]);
    }
    let r = GraphEvaluator::new().eval(&mut g, node, &EmptyStructure, 100_000);
    assert!(!r.is_definite(), "no verdict from a term too deep to walk");
}

/// A credence at the edge of the range must not panic under negation.
#[test]
fn an_extreme_credence_does_not_panic() {
    struct Extreme;
    impl GraphStructure for Extreme {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Holds
        }
        fn credence(&self, _p: ObjectId) -> Option<Credence> {
            Some(Credence {
                lo: i64::MIN,
                hi: i64::MIN + 1,
            })
        }
    }
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let negated = g.apply(wk::NOT, vec![claim]);
    let r = GraphEvaluator::new().eval(&mut g, negated, &Extreme, 10_000);
    assert_eq!(r.evidential(), Evidential::Refuted);
}

/// **A continuation must not be an easier question than the one it suspended.**
/// Rebuilding over the tail alone restarted the meet side at `Certain` and kept
/// the completeness flag, so a universal that was honestly `Open` resumed to
/// `Supported/Exact` — and which answer you got depended on where the budget ran
/// out.
#[test]
fn a_continuation_does_not_forget_what_the_scan_learned() {
    struct Partial {
        p: ObjectId,
        known_member: ObjectId,
    }
    impl GraphStructure for Partial {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred == self.p && args == [self.known_member] {
                Knowledge::Holds
            } else {
                Knowledge::Unknown
            }
        }
    }
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let (a, b) = (g.atom("a"), g.atom("b"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let s = Partial { p, known_member: b };

    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(dom), body);
    let ev = GraphEvaluator::new();

    let whole = ev.eval(&mut g, q, &s, 100_000);
    assert_eq!(
        whole.evidential(),
        Evidential::Open,
        "nothing is known about a"
    );

    for budget in [2u64, 3, 4, 6, 10] {
        let cut = ev.eval(&mut g, q, &s, budget);
        let Some(k) = cut.continuation else { continue };
        let resumed = ev.eval(&mut g, k, &s, 100_000);
        assert!(
            !resumed.is_definite(),
            "budget {budget}: resuming concluded what the whole query could not"
        );
    }
}

/// The same, with a contested member — where the old behaviour produced a
/// definite verdict in the *opposite* direction from the contested answer.
#[test]
fn a_continuation_does_not_forget_a_conflict() {
    struct Mixed {
        p: ObjectId,
        contested: ObjectId,
    }
    impl GraphStructure for Mixed {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred != self.p || args.len() != 1 {
                return Knowledge::Unknown;
            }
            if args[0] == self.contested {
                Knowledge::Conflicted
            } else {
                Knowledge::Holds
            }
        }
    }
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let (a, b) = (g.atom("a"), g.atom("b"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let s = Mixed { p, contested: a };

    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(dom), body);
    let ev = GraphEvaluator::new();

    let whole = ev.eval(&mut g, q, &s, 100_000);
    assert!(
        !whole.is_definite(),
        "a contested member is not a decided one"
    );

    for budget in [2u64, 3, 4, 6, 10] {
        let cut = ev.eval(&mut g, q, &s, budget);
        let Some(k) = cut.continuation else { continue };
        let resumed = ev.eval(&mut g, k, &s, 100_000);
        assert!(
            !resumed.is_definite(),
            "budget {budget}: the conflict did not survive suspension"
        );
    }
}

/// An operator's own snapshot must reach the guard that inspects it.
#[test]
fn a_stale_registry_answer_is_not_relabelled_current() {
    use artist_logic::evidence::{ComputeStatus, EvaluationResult};
    use artist_logic::registry::{OpContext, OperatorSemantics};

    struct Stale;
    impl OperatorSemantics for Stale {
        fn name(&self) -> &str {
            "stale"
        }
        fn evaluate_exact(&self, _cx: &mut OpContext<'_>) -> Option<EvaluationResult> {
            let mut r = EvaluationResult::certain(true);
            r.compute_status = ComputeStatus::Exact;
            r.snapshot = 999_999;
            Some(r)
        }
    }
    let mut g = ObjectGraph::new();
    let op = g.atom("stale-op");
    let a = g.atom("a");
    let node = g.apply(op, vec![a]);
    let s = MapGraphStructure::new().fact(a, vec![a]).at_version(7);

    let mut ev = GraphEvaluator::new();
    ev.registry.register(op, Box::new(Stale));
    let r = ev.eval(&mut g, node, &s, 10_000);
    assert_ne!(
        (r.evidential(), r.compute_status),
        (Evidential::Supported, ComputeStatus::Exact),
        "an answer from another version of the universe is not this one's"
    );
}

/// **Two derivations from one observation are one piece of evidence.**
///
/// Deduplicating by `source` handles two independent *reports*. It misses two
/// *derivations* sharing a premise: each gets its own record, each names a
/// different immediate source, and log-odds are additive — so one observation is
/// counted twice and confidence grows out of nothing. PLN's own book calls the
/// fix mandatory, their engine never shipped it, and the successor added it
/// eighteen years later.
#[test]
fn shared_ancestry_is_not_independent_corroboration() {
    use artist_logic::evidence::{Assertion, Evidence, EvidenceLedger, Polarity};

    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let observation = g.atom("session-7-transcript");
    let (e1, e2, e3) = (g.atom("e1"), g.atom("e2"), g.atom("e3"));
    let other = g.atom("session-9-transcript");

    let mut led = EvidenceLedger::new();
    led.assert(Assertion::affirm(claim, 1));

    // Two derivations, both tracing to one observation.
    for (id, src) in [(e1, observation), (e2, observation)] {
        led.record(Evidence {
            id,
            target: claim,
            polarity: Polarity::Affirm,
            source: Some(src),
            source_span: None,
            transformation: None,
            derived_from: vec![observation],
            llr_milli: Some(1000),
            created_by: None,
        });
    }
    let doubled = led.weight_of(claim);
    assert_eq!(doubled, 1000, "one observation is worth one observation");

    // A genuinely independent second source does add.
    led.record(Evidence {
        id: e3,
        target: claim,
        polarity: Polarity::Affirm,
        source: Some(other),
        source_span: None,
        transformation: None,
        derived_from: vec![other],
        llr_milli: Some(1000),
        created_by: None,
    });
    assert_eq!(
        led.weight_of(claim),
        2000,
        "…and two observations are worth two"
    );
}
