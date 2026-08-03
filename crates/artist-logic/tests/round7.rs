//! Round seven: the slots that were dropped, and the checker that was fooled.
//!
//! The worst finding of the round needed no exotic input at all —
//! `(exists [(x M) (y M)] (calls x y))` is documented surface syntax, it
//! round-trips through the printer, and the evaluator read `vars.first()` and
//! dropped the rest. Slots 1..n stayed as the fresh nominals `open_binder`
//! minted, so the body was evaluated with a name the store has never seen, and
//! a closed resolver answered `Fails` about it. That is the fabricated-`Fails`
//! pathway §5.2 calls the worst in the system, reached without any resolver
//! misbehaving.
//!
//! The storage layer had it right the whole time — `abstract_over`,
//! `instantiate` and `content_id` all handle n slots, and alpha-equivalence
//! holds over them. One binder shape, two implementations, one of them wrong.

use artist_logic::evidence::Evidential;
use artist_logic::graph_eval::{GraphEvaluator, GraphStructure, Knowledge};
use artist_logic::object::{Binding, wk};
use artist_logic::{ObjectGraph, ObjectId};

/// Authoritative about one relation, so an unrecognised argument gets `Fails`.
struct Closed {
    rel: ObjectId,
    pairs: Vec<(ObjectId, ObjectId)>,
}
impl GraphStructure for Closed {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        if pred != self.rel {
            return Knowledge::Unknown;
        }
        if args.len() == 2 && self.pairs.contains(&(args[0], args[1])) {
            Knowledge::Holds
        } else {
            Knowledge::Fails
        }
    }
}

fn fixture() -> (ObjectGraph, Closed, ObjectId, ObjectId, ObjectId, ObjectId) {
    let mut g = ObjectGraph::new();
    let calls = g.atom("calls");
    let (a, b) = (g.atom("mod-a"), g.atom("mod-b"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let s = Closed {
        rel: calls,
        pairs: vec![(a, b)],
    };
    (g, s, calls, a, b, dom)
}

/// A two-slot existential must find the pair that is on file.
#[test]
fn a_two_slot_existential_is_not_refuted() {
    let (mut g, s, calls, ..) = fixture();
    let dom = {
        let (a, b) = (g.atom("mod-a"), g.atom("mod-b"));
        g.apply(wk::SET_DOMAIN, vec![a, b])
    };
    let (x, y) = (g.fresh(), g.fresh());
    let body = g.apply(calls, vec![x, y]);
    let q = g.bind(
        wk::EXISTS,
        vec![
            Binding {
                var: x,
                domain: Some(dom),
            },
            Binding {
                var: y,
                domain: Some(dom),
            },
        ],
        vec![body],
    );
    assert_eq!(
        GraphEvaluator::new()
            .eval(&mut g, q, &s, 200_000)
            .evidential(),
        Evidential::Supported,
        "(calls mod-a mod-b) is on file"
    );
}

/// …and a two-slot universal must be refuted only by a pair that really fails.
#[test]
fn a_two_slot_universal_is_refuted_only_by_a_real_counterexample() {
    let mut g = ObjectGraph::new();
    let calls = g.atom("calls");
    let (a, b) = (g.atom("mod-a"), g.atom("mod-b"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    // Every ordered pair on file.
    let s = Closed {
        rel: calls,
        pairs: vec![(a, a), (a, b), (b, a), (b, b)],
    };

    let (x, y) = (g.fresh(), g.fresh());
    let body = g.apply(calls, vec![x, y]);
    let q = g.bind(
        wk::EXISTS,
        vec![
            Binding {
                var: x,
                domain: Some(dom),
            },
            Binding {
                var: y,
                domain: Some(dom),
            },
        ],
        vec![body],
    );
    let ev = GraphEvaluator::new();
    assert_eq!(
        ev.eval(&mut g, q, &s, 200_000).evidential(),
        Evidential::Supported
    );

    let all = g.bind(
        wk::FORALL,
        vec![
            Binding {
                var: x,
                domain: Some(dom),
            },
            Binding {
                var: y,
                domain: Some(dom),
            },
        ],
        vec![body],
    );
    assert_eq!(
        ev.eval(&mut g, all, &s, 200_000).evidential(),
        Evidential::Supported,
        "all four pairs hold, so the universal holds"
    );

    // Remove one pair and the universal must fall — for the right reason.
    let partial = Closed {
        rel: calls,
        pairs: vec![(a, a), (a, b), (b, a)],
    };
    assert_eq!(
        ev.eval(&mut g, all, &partial, 200_000).evidential(),
        Evidential::Refuted
    );
}

/// A multi-slot aggregate must not silently undercount. There is no honest
/// partial answer to a *value*, so it declines rather than returning zero.
#[test]
fn a_multi_slot_aggregate_does_not_undercount() {
    let mut g = ObjectGraph::new();
    let calls = g.atom("calls");
    let (a, b) = (g.atom("mod-a"), g.atom("mod-b"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let s = Closed {
        rel: calls,
        pairs: vec![(a, a), (a, b), (b, a), (b, b)],
    };

    let (x, y) = (g.fresh(), g.fresh());
    let body = g.apply(calls, vec![x, y]);
    let counted = g.bind(
        wk::COUNT,
        vec![
            Binding {
                var: x,
                domain: Some(dom),
            },
            Binding {
                var: y,
                domain: Some(dom),
            },
        ],
        vec![body],
    );
    let zero = g.int(0);
    let claim = g.apply(wk::EQ, vec![counted, zero]);
    assert_ne!(
        GraphEvaluator::new()
            .eval(&mut g, claim, &s, 200_000)
            .evidential(),
        Evidential::Supported,
        "four pairs are on file; zero is not the count"
    );
}

/// Telescoping — §2's own example shape, where an inner domain mentions an outer
/// variable — must survive the re-association.
#[test]
fn telescoping_domains_still_work() {
    struct PerModule {
        tests_of: ObjectId,
        passes: ObjectId,
        t1: ObjectId,
    }
    impl GraphStructure for PerModule {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred == self.passes && args == [self.t1] {
                Knowledge::Holds
            } else {
                Knowledge::Unknown
            }
        }
        fn extension(&self, _domain: ObjectId) -> Option<artist_logic::graph_eval::Extension> {
            let _ = self.tests_of;
            Some(artist_logic::graph_eval::Extension::complete(vec![self.t1]))
        }
    }
    let mut g = ObjectGraph::new();
    let (tests_of, passes) = (g.atom("tests-of"), g.atom("passes"));
    let (m1, t1) = (g.atom("mod-1"), g.atom("t1"));
    let modules = g.apply(wk::SET_DOMAIN, vec![m1]);

    let m = g.fresh();
    let inner_dom = g.apply(tests_of, vec![m]);
    let t = g.fresh();
    let body = g.apply(passes, vec![t]);
    let q = g.bind(
        wk::FORALL,
        vec![
            Binding {
                var: m,
                domain: Some(modules),
            },
            Binding {
                var: t,
                domain: Some(inner_dom),
            },
        ],
        vec![body],
    );
    let s = PerModule {
        tests_of,
        passes,
        t1,
    };
    assert_eq!(
        GraphEvaluator::new()
            .eval(&mut g, q, &s, 200_000)
            .evidential(),
        Evidential::Supported,
        "the inner domain mentions the outer variable, which is the point of telescoping"
    );
}

/// The certificate a two-slot query produces must check — the fabricated
/// refutation was previously certified.
#[test]
fn a_two_slot_query_produces_a_checkable_derivation() {
    let (mut g, s, calls, ..) = fixture();
    let dom = {
        let (a, b) = (g.atom("mod-a"), g.atom("mod-b"));
        g.apply(wk::SET_DOMAIN, vec![a, b])
    };
    let (x, y) = (g.fresh(), g.fresh());
    let body = g.apply(calls, vec![x, y]);
    let q = g.bind(
        wk::EXISTS,
        vec![
            Binding {
                var: x,
                domain: Some(dom),
            },
            Binding {
                var: y,
                domain: Some(dom),
            },
        ],
        vec![body],
    );
    let (r, cert) = GraphEvaluator::new().eval_traced(&mut g, q, &s, 200_000);
    assert_eq!(r.evidential(), Evidential::Supported);

    // **The evaluator answers; the kernel does not yet certify this shape.**
    //
    // `Step::Instance` checks an instantiation by substituting *one* bound
    // variable, so a two-slot binder is outside the rule set. The honest
    // consequence is an empty certificate rather than a step the kernel would
    // have to take on trust — and `eval_traced` discards a derivation that does
    // not conclude the root, so nothing claims to have proved this.
    //
    // This test previously asserted the derivation checks, which it did only
    // because `Instance` ignored the binder's `vars` entirely — the same defect
    // that let a witness from outside the domain prove an existential. Closing
    // that hole narrowed what is certifiable, and multi-slot instantiation is
    // recorded as a gap in docs/semantics.md §12 rather than papered over here.
    assert!(
        cert.check(&g).is_err(),
        "a two-slot binder is outside the eight rules, and says so"
    );
}
