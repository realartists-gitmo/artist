//! Determinacy, the two modes, and validity.
//!
//! Three changes that only make sense together. The evaluator could not
//! recognise a single logical truth — `P → P` went looking for facts about `P`
//! and reported an absence. Fixing that naively would have been worse than the
//! bug: `P ∨ ¬P` is a tautology *given bivalence*, and bivalence is exactly what
//! sorites denies to ordinary predicates. A validity recogniser with an
//! optimistic default certifies excluded middle for `heap`.
//!
//! So determinacy became an axis, evaluation gained a certification mode, and
//! validity is decided against both.

use artist_logic::evidence::{Determinacy, Evidential, Grounding};
use artist_logic::graph_eval::{EmptyStructure, GraphEvaluator, GraphStructure, Knowledge};
use artist_logic::object::wk;
use artist_logic::{ObjectGraph, ObjectId};

/// Tautologies are true without asking the store anything.
#[test]
fn logical_truths_do_not_need_evidence() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let pa = g.apply(p, vec![a]);
    let npa = g.apply(wk::NOT, vec![pa]);
    let ev = GraphEvaluator::new();

    let identity = g.apply(wk::IMPLIES, vec![pa, pa]);
    assert_eq!(
        ev.eval(&mut g, identity, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported,
        "P → P holds in every structure, including the empty one"
    );

    let nc = {
        let conj = g.apply(wk::AND, vec![pa, npa]);
        g.apply(wk::NOT, vec![conj])
    };
    assert_eq!(
        ev.eval(&mut g, nc, &EmptyStructure, 10_000).evidential(),
        Evidential::Supported,
        "non-contradiction"
    );

    let contradiction = g.apply(wk::AND, vec![pa, npa]);
    assert_eq!(
        ev.eval(&mut g, contradiction, &EmptyStructure, 10_000).evidential(),
        Evidential::Refuted,
        "…and its dual is false in every structure"
    );

    // A non-tautology is still honestly open.
    let q = g.atom("q");
    let qa = g.apply(q, vec![a]);
    let contingent = g.apply(wk::IMPLIES, vec![pa, qa]);
    assert_eq!(
        ev.eval(&mut g, contingent, &EmptyStructure, 10_000).evidential(),
        Evidential::Open,
        "P → Q depends on the store, and the store says nothing"
    );
}

/// **The bug that gating exists to prevent.** Excluded middle is the tautology
/// that needs bivalence, and a borderline predicate has none.
#[test]
fn excluded_middle_is_not_certified_for_a_vague_predicate() {
    struct Vague;
    impl GraphStructure for Vague {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn determinacy(&self, _p: ObjectId) -> Determinacy {
            Determinacy::Indeterminate
        }
    }
    let mut g = ObjectGraph::new();
    let heap = g.atom("heap");
    let n = g.int(4783);
    let h = g.apply(heap, vec![n]);
    let nh = g.apply(wk::NOT, vec![h]);
    let lem = g.apply(wk::OR, vec![h, nh]);
    let ev = GraphEvaluator::new();

    // An ordinary query may presume totality — and records that it presumed.
    let queried = ev.eval(&mut g, lem, &Vague, 10_000);
    assert_eq!(queried.evidential(), Evidential::Supported);
    assert_eq!(
        queried.determinacy,
        Determinacy::Indeterminate,
        "the presumption must be visible in the answer"
    );

    // Certification must refuse it.
    let certified = ev.certify(&mut g, lem, &Vague, 10_000);
    assert_ne!(
        certified.evidential(),
        Evidential::Supported,
        "P ∨ ¬P for a predicate with no sharp condition is not a theorem"
    );
}

/// …and certification still works where totality is established.
#[test]
fn certification_succeeds_on_an_established_predicate() {
    struct Sharp;
    impl GraphStructure for Sharp {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn determinacy(&self, _p: ObjectId) -> Determinacy {
            Determinacy::Total
        }
    }
    let mut g = ObjectGraph::new();
    let even = g.atom("even");
    let n = g.int(4);
    let e = g.apply(even, vec![n]);
    let ne = g.apply(wk::NOT, vec![e]);
    let lem = g.apply(wk::OR, vec![e, ne]);
    let r = GraphEvaluator::new().certify(&mut g, lem, &Sharp, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);
    assert_eq!(r.determinacy, Determinacy::Total);
}

/// Determinacy is **not** grounding. All four combinations are inhabited, which
/// is the test for whether two axes should be separate.
#[test]
fn determinacy_and_grounding_are_independent() {
    struct Vague;
    impl GraphStructure for Vague {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn determinacy(&self, _p: ObjectId) -> Determinacy {
            Determinacy::Indeterminate
        }
    }
    let ev = GraphEvaluator::new();

    // ungrounded + semantically precise: the liar.
    let mut g = ObjectGraph::new();
    let liar = g.alloc();
    let q = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q]);
    let n = g.apply(wk::NOT, vec![h]);
    let node = g.get(n).cloned().expect("built");
    g.define(liar, node);
    let r = ev.eval(&mut g, liar, &EmptyStructure, 20_000);
    assert_eq!(r.grounding, Grounding::Oscillatory);
    assert_ne!(
        r.determinacy,
        Determinacy::Indeterminate,
        "the liar is broken structurally, not semantically"
    );

    // grounded + indeterminate: a borderline heap.
    let mut h2 = ObjectGraph::new();
    let heap = h2.atom("heap");
    let k = h2.int(4783);
    let claim = h2.apply(heap, vec![k]);
    let and = h2.apply(wk::AND, vec![claim, wk::TOP]);
    let r2 = ev.eval(&mut h2, and, &Vague, 20_000);
    assert_eq!(r2.grounding, Grounding::Grounded, "no loop, evaluation terminates");
    assert_eq!(r2.determinacy, Determinacy::Indeterminate);
}

/// Determinacy gates classical operations; it does not erase evidence. There is
/// real evidence that someone is tall.
#[test]
fn indeterminacy_does_not_destroy_evidence() {
    struct Tall {
        tall: ObjectId,
    }
    impl GraphStructure for Tall {
        fn known(&self, pred: ObjectId, _a: &[ObjectId]) -> Knowledge {
            if pred == self.tall { Knowledge::Holds } else { Knowledge::Unknown }
        }
        fn determinacy(&self, _p: ObjectId) -> Determinacy {
            Determinacy::Indeterminate
        }
    }
    let mut g = ObjectGraph::new();
    let tall = g.atom("tall");
    let adam = g.atom("adam");
    let claim = g.apply(tall, vec![adam]);
    let r = GraphEvaluator::new().eval(&mut g, claim, &Tall { tall }, 10_000);

    assert_eq!(r.evidential(), Evidential::Supported, "the grounds are genuine");
    assert_eq!(r.determinacy, Determinacy::Indeterminate, "and there is no sharp fact");
}

/// The default is `Unknown`, not `Total` — presumed rather than established,
/// and the difference is visible.
#[test]
fn totality_is_presumed_and_says_so() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let claim = g.apply(p, vec![a]);
    let s = artist_logic::graph_eval::MapGraphStructure::new().fact(p, vec![a]);
    let r = GraphEvaluator::new().eval(&mut g, claim, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Supported);
    assert_eq!(
        r.determinacy,
        Determinacy::Unknown,
        "no interpretation established this predicate as sharp"
    );
}

/// **Constructive validity needs no bivalence.** Gating *all* validity on
/// determinacy was wrong: `P → P` holds whatever `P` means, whether or not `P`
/// has a sharp condition, and refusing to certify it over a borderline predicate
/// was a bug rather than caution.
#[test]
fn constructive_tautologies_certify_over_a_vague_predicate() {
    struct Vague;
    impl GraphStructure for Vague {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn determinacy(&self, _p: ObjectId) -> Determinacy {
            Determinacy::Indeterminate
        }
    }
    let mut g = ObjectGraph::new();
    let heap = g.atom("heap");
    let n = g.int(4783);
    let h = g.apply(heap, vec![n]);
    let nh = g.apply(wk::NOT, vec![h]);
    let ev = GraphEvaluator::new();

    // Constructive: certified, borderline predicate and all.
    let identity = g.apply(wk::IMPLIES, vec![h, h]);
    assert_eq!(
        ev.certify(&mut g, identity, &Vague, 20_000).evidential(),
        Evidential::Supported,
        "P → P is intuitionistically valid; sharpness is irrelevant to it"
    );

    let nc = {
        let conj = g.apply(wk::AND, vec![h, nh]);
        g.apply(wk::NOT, vec![conj])
    };
    assert_eq!(
        ev.certify(&mut g, nc, &Vague, 20_000).evidential(),
        Evidential::Supported,
        "non-contradiction is constructive too"
    );

    let with_top = g.apply(wk::AND, vec![h, wk::TOP]);
    let simplifies = g.apply(wk::IMPLIES, vec![with_top, h]);
    assert_eq!(
        ev.certify(&mut g, simplifies, &Vague, 20_000).evidential(),
        Evidential::Supported,
        "P ∧ ⊤ → P"
    );

    // Classical only: still gated.
    let lem = g.apply(wk::OR, vec![h, nh]);
    assert_ne!(
        ev.certify(&mut g, lem, &Vague, 20_000).evidential(),
        Evidential::Supported,
        "excluded middle is exactly what bivalence buys"
    );

    let nn = g.apply(wk::NOT, vec![nh]);
    let dne = g.apply(wk::IMPLIES, vec![nn, h]);
    assert_ne!(
        ev.certify(&mut g, dne, &Vague, 20_000).evidential(),
        Evidential::Supported,
        "and double-negation elimination is the other half of it"
    );

    // …while the intuitionistic direction of double negation is fine.
    let dni = g.apply(wk::IMPLIES, vec![h, nn]);
    assert_eq!(
        ev.certify(&mut g, dni, &Vague, 20_000).evidential(),
        Evidential::Supported,
        "P → ¬¬P is constructive"
    );
}

/// The two cells `determinacy_and_grounding_are_independent` claims and does not
/// show. The four-cell argument is what licenses a *separate axis*, and it is
/// only an argument if every cell is inhabited — so the two that carry it get
/// their own test rather than a sentence.
#[test]
fn the_remaining_two_cells_of_the_independence_argument() {
    struct Vague;
    impl GraphStructure for Vague {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn determinacy(&self, _p: ObjectId) -> Determinacy {
            Determinacy::Indeterminate
        }
    }
    let ev = GraphEvaluator::new();

    // grounded + determinate: the ordinary case, and the control.
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let plain = g.apply(p, vec![a]);
    let r = ev.eval(&mut g, plain, &EmptyStructure, 10_000);
    assert_eq!(r.grounding, Grounding::Grounded);
    assert_ne!(r.determinacy, Determinacy::Indeterminate);

    // **ungrounded *and* indeterminate** — "this sentence is heapish". The cell
    // that actually carries the argument, and the one nothing tested.
    //
    // Note the naive spelling does not work: `s := (heapish ⟨s⟩)` reports
    // `Grounded`, because the loop is only detected through `holds`. That is
    // exactly why the case is easy to leave out of a test and easy to assert in
    // prose.
    let mut h = ObjectGraph::new();
    let heapish = h.atom("heapish");
    let s = h.alloc();
    let quoted = h.apply(wk::QUOTE, vec![s]);
    let vague_part = h.apply(heapish, vec![quoted]);
    let self_part = h.apply(wk::HOLDS, vec![quoted]);
    let body = h.apply(wk::AND, vec![vague_part, self_part]);
    let node = h.get(body).cloned().expect("built");
    h.define(s, node);

    let r2 = ev.eval(&mut h, s, &Vague, 20_000);
    assert_ne!(r2.grounding, Grounding::Grounded, "its own truth is among its premises");
    assert_eq!(r2.determinacy, Determinacy::Indeterminate, "and its predicate is borderline");
}
