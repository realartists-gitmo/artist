//! `docs/formalism.md` §5.2, executed.
//!
//! Eight of the findings in the last audit were the spec — or a code comment —
//! describing a capability the code did not have. `(lo == hi).then_some(lo)`
//! carried the comment *"a partial one is still a usable bound, which `LEQ`
//! reads"*; `LEQ` read `number()`, which called that and got `None`. The
//! document said one thing, the source said the same thing, and neither was
//! true.
//!
//! Fixing the instances does not fix the class. §5.2 is already a table, so
//! this file *is* that table: one case per row, so a future claim cannot drift
//! from the implementation silently. When a row here and the document disagree,
//! one of them is a bug and both are visible.
//!
//! **That header was itself an overclaim when it was written.** The table had
//! fourteen rows and this file covered nine — `count`/`sum`, `usually`,
//! `unless`, `quantity` and `at` had no case at all — and one of the tests was
//! named for a property it did not assert. An artifact built to stop
//! overclaims, shipped with an overclaim at the top of it.
//!
//! Row tests are also not sufficient on their own, which is the deeper lesson:
//! they check the cases somebody thought of. The three audits that followed
//! found their bugs by *differential probing*, and those live in
//! `properties.rs`. This file pins the table; that one pins the invariants.

use artist_logic::evidence::{Bound, ComputeStatus, Evidential};
use artist_logic::graph_eval::{
    EmptyStructure, Extension, GraphEvaluator, GraphStructure, Knowledge,
};
use artist_logic::object::wk;
use artist_logic::{ObjectGraph, ObjectId};

/// A structure that answers exactly what it is told to and nothing else.
struct Told {
    holds: Vec<(ObjectId, Vec<ObjectId>)>,
    fails: Vec<(ObjectId, Vec<ObjectId>)>,
    domains: Vec<(ObjectId, Vec<ObjectId>, bool)>,
}

impl Told {
    fn new() -> Told {
        Told { holds: Vec::new(), fails: Vec::new(), domains: Vec::new() }
    }
    fn holding(mut self, p: ObjectId, args: Vec<ObjectId>) -> Told {
        self.holds.push((p, args));
        self
    }
    fn failing(mut self, p: ObjectId, args: Vec<ObjectId>) -> Told {
        self.fails.push((p, args));
        self
    }
    fn domain(mut self, d: ObjectId, members: Vec<ObjectId>, complete: bool) -> Told {
        self.domains.push((d, members, complete));
        self
    }
}

impl GraphStructure for Told {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        if self.holds.iter().any(|(p, a)| *p == pred && a == args) {
            Knowledge::Holds
        } else if self.fails.iter().any(|(p, a)| *p == pred && a == args) {
            Knowledge::Fails
        } else {
            Knowledge::Unknown
        }
    }
    fn extension(&self, domain: ObjectId) -> Option<Extension> {
        self.domains.iter().find(|(d, _, _)| *d == domain).map(|(_, m, c)| Extension {
            members: m.clone(),
            complete: *c,
        })
    }
}

/// Build three atoms whose evaluation is known, unknown and refuted.
fn fixture(g: &mut ObjectGraph) -> (Told, ObjectId, ObjectId, ObjectId) {
    let p = g.atom("p");
    let (t, u, f) = (g.atom("t"), g.atom("u"), g.atom("f"));
    let (yes, unknown, no) =
        (g.apply(p, vec![t]), g.apply(p, vec![u]), g.apply(p, vec![f]));
    let s = Told::new().holding(p, vec![t]).failing(p, vec![f]);
    (s, yes, unknown, no)
}

fn eval(g: &mut ObjectGraph, root: ObjectId, s: &dyn GraphStructure) -> Evidential {
    GraphEvaluator::new().eval(g, root, s, 50_000).evidential()
}

/// §5.2 — `(not P)`: support and refutation swap.
#[test]
fn row_not() {
    let mut g = ObjectGraph::new();
    let (s, yes, unknown, no) = fixture(&mut g);
    for (operand, expect) in
        [(yes, Evidential::Refuted), (no, Evidential::Supported), (unknown, Evidential::Open)]
    {
        let e = g.apply(wk::NOT, vec![operand]);
        assert_eq!(eval(&mut g, e, &s), expect);
    }
}

/// §5.2 — `(and P…)`: support is the meet, refutation the join. One refuted
/// conjunct refutes the whole thing even when the others are unknown.
#[test]
fn row_and() {
    let mut g = ObjectGraph::new();
    let (s, yes, unknown, no) = fixture(&mut g);
    for (operands, expect) in [
        (vec![yes, yes], Evidential::Supported),
        (vec![yes, no], Evidential::Refuted),
        (vec![unknown, no], Evidential::Refuted),
        (vec![yes, unknown], Evidential::Open),
        (vec![unknown, unknown], Evidential::Open),
        // Vacuous conjunction.
        (vec![], Evidential::Supported),
    ] {
        let e = g.apply(wk::AND, operands.clone());
        assert_eq!(eval(&mut g, e, &s), expect, "and{operands:?}");
    }
}

/// §5.2 — `(or P…)`: dual of `and`. One supported disjunct decides it.
#[test]
fn row_or() {
    let mut g = ObjectGraph::new();
    let (s, yes, unknown, no) = fixture(&mut g);
    for (operands, expect) in [
        (vec![no, no], Evidential::Refuted),
        (vec![yes, no], Evidential::Supported),
        (vec![unknown, yes], Evidential::Supported),
        (vec![no, unknown], Evidential::Open),
        (vec![], Evidential::Refuted),
    ] {
        let e = g.apply(wk::OR, operands.clone());
        assert_eq!(eval(&mut g, e, &s), expect, "or{operands:?}");
    }
}

/// §5.2 — `(implies P Q)` behaves as `(or (not P) Q)`.
#[test]
fn row_implies() {
    let mut g = ObjectGraph::new();
    let (s, yes, unknown, no) = fixture(&mut g);
    // A *second* unknown, distinct from the first. `(implies unknown unknown)`
    // over one node is `P → P` — a tautology, and now recognised as one, which
    // is a different row of the table.
    let other = {
        let (q, b) = (g.atom("q-other"), g.atom("b-other"));
        g.apply(q, vec![b])
    };
    // **The false-antecedent row, lost and recovered.** `→` is the strong
    // implication (docs/semantics.md §3), so `A ⊃ C` is `C` when `A` is
    // designated and `T` otherwise — and establishing "A is not designated"
    // needs `⟦A⟧⁺ = 0`.
    //
    // That was inexpressible while `Bound::None` meant both *the bit is zero*
    // and *the bit is unknown*, so this row went `Open` for a while: materially
    // it held, and materially the evaluator was unsound, since with `A`
    // conflicted `¬B ∨ F = B` reads designated while `B ⊃ F = F`. The §7.1b
    // repair separates *fails* from *is refuted*, and the row is derivable again
    // — without the unsoundness that used to come with it.
    for (a, b, expect) in [
        (no, unknown, Evidential::Supported),   // false antecedent: `⟦A⟧⁺ = 0`
        (yes, yes, Evidential::Supported),      // true consequent
        (yes, no, Evidential::Refuted),         // the only refuting case
        (unknown, other, Evidential::Open),     // both open, and unrelated
    ] {
        let e = g.apply(wk::IMPLIES, vec![a, b]);
        assert_eq!(eval(&mut g, e, &s), expect);
    }

    // …and the tautologous case, which needs no store at all.
    //
    // This survived a change of logic underneath it, and the reason is worth
    // recording. It used to hold because `→` was decided by a *Boolean* truth
    // table — which a soundness property test showed certifies tautologies *and*
    // contradictions the model leaves open, since the committed semantics is
    // Belnap's FOUR (docs/semantics.md §3). Reading `→` materially there makes
    // `P → P` at `N` equal `N`, and the tautology set empties.
    //
    // It holds now for a better reason: `→` is Arieli–Avron's strong
    // implication, and validity is *always designated* rather than always `T`.
    // Excluded middle is still not valid — `P ∨ ¬P` at `N` is `N` — so the
    // vague-predicate protection is untouched. See `round6.rs`.
    let identity = g.apply(wk::IMPLIES, vec![unknown, unknown]);
    assert_eq!(
        eval(&mut g, identity, &s),
        Evidential::Supported,
        "P → P holds whatever the store knows about P"
    );
}

/// §5.2 — `forall`: **any member refuted** refutes, but support additionally
/// requires the domain to be enumerable *and complete*. The asymmetry is the
/// row, and it is what a partial index gets wrong.
#[test]
fn row_forall() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let (a, b) = (g.atom("a"), g.atom("b"));
    let d = g.atom("D");

    let complete = Told::new()
        .holding(p, vec![a])
        .holding(p, vec![b])
        .domain(d, vec![a, b], true);
    let partial = Told::new()
        .holding(p, vec![a])
        .holding(p, vec![b])
        .domain(d, vec![a, b], false);
    let counterexample = Told::new()
        .holding(p, vec![a])
        .failing(p, vec![b])
        .domain(d, vec![a, b], false);

    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let claim = g.quantify(wk::FORALL, v, Some(d), body);

    assert_eq!(eval(&mut g, claim, &complete), Evidential::Supported);
    assert_eq!(
        eval(&mut g, claim, &partial),
        Evidential::Open,
        "an incomplete domain cannot support a universal"
    );
    assert_eq!(
        eval(&mut g, claim, &counterexample),
        Evidential::Refuted,
        "one counterexample refutes regardless of completeness"
    );
}

/// §5.2 — `exists`: the mirror image.
#[test]
fn row_exists() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let (a, b) = (g.atom("a"), g.atom("b"));
    let d = g.atom("D");

    let witness = Told::new().holding(p, vec![a]).domain(d, vec![a, b], false);
    let none_complete = Told::new()
        .failing(p, vec![a])
        .failing(p, vec![b])
        .domain(d, vec![a, b], true);
    let none_partial = Told::new()
        .failing(p, vec![a])
        .failing(p, vec![b])
        .domain(d, vec![a, b], false);

    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let claim = g.quantify(wk::EXISTS, v, Some(d), body);

    assert_eq!(
        eval(&mut g, claim, &witness),
        Evidential::Supported,
        "one witness decides regardless of completeness"
    );
    assert_eq!(eval(&mut g, claim, &none_complete), Evidential::Refuted);
    assert_eq!(
        eval(&mut g, claim, &none_partial),
        Evidential::Open,
        "the witness may be a member never listed"
    );
}

/// §5.2 — an atom: `Holds` supports, `Fails` refutes, `Unknown` decides
/// nothing. Only an authoritative resolver licenses the refutation.
#[test]
fn row_atom() {
    let mut g = ObjectGraph::new();
    let (s, yes, unknown, no) = fixture(&mut g);
    assert_eq!(eval(&mut g, yes, &s), Evidential::Supported);
    assert_eq!(eval(&mut g, no, &s), Evidential::Refuted);
    assert_eq!(eval(&mut g, unknown, &s), Evidential::Open);
}

/// §5.2 — `(quote E)`: E is **not** evaluated. `(holds (quote P))` descends.
#[test]
fn row_quote_and_holds() {
    let mut g = ObjectGraph::new();
    let (s, yes, _, _) = fixture(&mut g);

    let quoted = g.apply(wk::QUOTE, vec![yes]);
    assert_ne!(
        eval(&mut g, quoted, &s),
        Evidential::Supported,
        "a quotation is a term, not a claim"
    );

    let held = g.apply(wk::HOLDS, vec![quoted]);
    assert_eq!(eval(&mut g, held, &s), Evidential::Supported);
}

/// §5.1 — the information ordering. Evaluation only ever moves *up* it, and
/// `must` grows while `may` shrinks.
///
/// This test used to assert that three bounds are ordered and four states are
/// reachable, under a name promising monotonicity — and no monotonicity was
/// tested anywhere. That is why the `Partial`-promotion cluster and the
/// conflict-flattening bug both walked past a green suite. The ordering claim
/// is checked here; the *monotonicity* claim needs varying inputs and lives in
/// `properties.rs::more_budget_never_retracts_a_conclusion`.
#[test]
fn the_lattice_is_ordered_by_information() {
    assert!(Bound::None < Bound::Partial);
    assert!(Bound::Partial < Bound::Certain);

    // The four states are exactly the four corners of the two bounds.
    let mut g = ObjectGraph::new();
    let (s, yes, unknown, no) = fixture(&mut g);
    assert_eq!(eval(&mut g, unknown, &s), Evidential::Open);
    assert_eq!(eval(&mut g, yes, &s), Evidential::Supported);
    assert_eq!(eval(&mut g, no, &s), Evidential::Refuted);

    // The liar is *ungrounded*, which is a different axis from contradictory
    // evidence — `Conflicted` means the store holds claims both ways and you
    // should go read them; `Oscillatory` means stop asking.
    let liar = g.alloc();
    let q = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q]);
    let n = g.apply(wk::NOT, vec![h]);
    let node = g.get(n).cloned().expect("built");
    g.define(liar, node);
    let r = GraphEvaluator::new().eval(&mut g, liar, &EmptyStructure, 10_000);
    assert_eq!(r.evidential(), Evidential::Open, "no evidence either way exists");
    assert_eq!(r.grounding, artist_logic::evidence::Grounding::Oscillatory);
    assert!(!r.is_definite());
}

/// §5.1 — computational status is a *separate axis* from evidence. "No
/// evidence", "no semantics" and "out of budget" are three different answers
/// and a caller can tell them apart.
#[test]
fn compute_status_is_orthogonal_to_evidence() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let unknown = g.apply(p, vec![a]);
    let ev = GraphEvaluator::new();

    // No evidence, but the evaluator understood the question.
    let r = ev.eval(&mut g, unknown, &EmptyStructure, 50_000);
    assert_eq!(r.evidential(), Evidential::Open);
    assert_eq!(r.compute_status, ComputeStatus::Stalled);

    // No semantics — a well-known operator with no frame behind it.
    let modal = g.apply(wk::NECESSARILY, vec![unknown]);
    let r = ev.eval(&mut g, modal, &EmptyStructure, 50_000);
    assert_eq!(r.compute_status, ComputeStatus::Unsupported);

    // Out of budget.
    let d = g.atom("D");
    let members: Vec<ObjectId> = (0..64).map(|i| g.atom(&format!("m{i}"))).collect();
    let s = Told::new().domain(d, members, true);
    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let big = g.quantify(wk::FORALL, v, Some(d), body);
    let r = ev.eval(&mut g, big, &s, 8);
    assert_eq!(r.compute_status, ComputeStatus::BudgetExhausted);
    assert!(r.residual.is_some(), "a cut-short scan hands back what is left");
}

/// §5.4 — termination under a budget is unconditional, and a partial result is
/// a true statement about a weaker claim rather than a guess.
#[test]
fn halting_early_is_sound() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let d = g.atom("D");
    let members: Vec<ObjectId> = (0..32).map(|i| g.atom(&format!("m{i}"))).collect();
    // Every member satisfies `p`, so the full answer is Supported.
    let mut s = Told::new().domain(d, members.clone(), true);
    for m in &members {
        s = s.holding(p, vec![*m]);
    }

    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let claim = g.quantify(wk::FORALL, v, Some(d), body);
    let ev = GraphEvaluator::new();

    // Cut short, it must not claim the answer it has not reached.
    let partial = ev.eval(&mut g, claim, &s, 10);
    assert_ne!(partial.evidential(), Evidential::Refuted, "never a wrong verdict");
    assert!(!partial.is_definite(), "a truncated scan is not definite");

    // Given budget, it reaches it.
    let full = ev.eval(&mut g, claim, &s, 100_000);
    assert_eq!(full.evidential(), Evidential::Supported);
}

// ---------------------------------------------------------------------------
// The five rows that had no case.
// ---------------------------------------------------------------------------

/// §5.2 — `(count …)` / `(sum …)`: `lo` counts what is proven, `hi` adds what
/// is not yet ruled out, and a bound decides a comparison before the scan is
/// exhaustive. `hi` is a bound only when the *enumeration* was exhaustive.
#[test]
fn row_aggregates_bound_from_both_sides() {
    use artist_logic::object::Binding;

    let mut g = ObjectGraph::new();
    let fails = g.atom("fails");
    let ms: Vec<ObjectId> = ["t1", "t2", "t3"].iter().map(|n| g.atom(n)).collect();
    let dom = g.atom("D");
    let v = g.fresh();
    let body = g.apply(fails, vec![v]);
    let counted = g.bind(wk::COUNT, vec![Binding { var: v, domain: Some(dom) }], vec![body]);

    // One proven, two undecided, over a *complete* enumeration: lo = 1, hi = 3.
    let complete = Told::new().holding(fails, vec![ms[0]]).domain(dom, ms.clone(), true);
    let one = g.int(1);
    let at_least_one = g.apply(wk::LEQ, vec![one, counted]);
    assert_eq!(eval(&mut g, at_least_one, &complete), Evidential::Supported, "lo decides");
    let three = g.int(3);
    let at_most_three = g.apply(wk::LEQ, vec![counted, three]);
    assert_eq!(eval(&mut g, at_most_three, &complete), Evidential::Supported, "hi decides");

    // The same numbers over a *partial* enumeration bound only from below.
    let partial = Told::new().holding(fails, vec![ms[0]]).domain(dom, ms.clone(), false);
    assert_eq!(eval(&mut g, at_least_one, &partial), Evidential::Supported);
    assert_ne!(
        eval(&mut g, at_most_three, &partial),
        Evidential::Supported,
        "unseen members may contribute, so `hi` bounds nothing"
    );
}

/// §5.2 — `(usually P)`: certain that the default applies, marked
/// `Derivation::Default` so nothing reads it as settled, and needing a default
/// *on record* to say anything at all.
#[test]
fn row_usually_is_defeasible_support() {
    use artist_logic::graph_eval::MapGraphStructure;

    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let claim = g.apply(p, vec![a]);
    let hedged = g.apply(wk::USUALLY, vec![claim]);

    let s = MapGraphStructure::new().fact(wk::USUALLY, vec![claim]);
    let r = GraphEvaluator::new().eval(&mut g, hedged, &s, 20_000);
    // Certain that the default applies; defeasible because a defeater would
    // overturn it. Strength and defeasibility are separate axes.
    assert_eq!(r.support, Bound::Certain);
    assert_eq!(r.derivation, artist_logic::evidence::Derivation::Default);
    assert!(!r.defeated_by.is_empty(), "a default names what would defeat it");
    assert!(!r.is_definite());

    // Nothing on record: `usually` reports the absence it found.
    let r = GraphEvaluator::new().eval(&mut g, hedged, &EmptyStructure, 20_000);
    assert_eq!(r.evidential(), Evidential::Open);

    // Certain counter-evidence defeats it outright.
    let s = Told::new().failing(p, vec![a]);
    assert_eq!(eval(&mut g, hedged, &s), Evidential::Refuted);
}

/// §5.2 — `(unless E P)`: an exception that merely *might* hold does not
/// defeat, or no default would survive contact with an incomplete store.
#[test]
fn row_unless_needs_a_certain_exception() {
    let mut g = ObjectGraph::new();
    let (p, e, a) = (g.atom("p"), g.atom("e"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let exception = g.apply(e, vec![a]);
    let guarded = g.apply(wk::UNLESS, vec![exception, claim]);

    // Exception unknown, claim holds → the claim stands.
    let s = Told::new().holding(p, vec![a]);
    assert_eq!(eval(&mut g, guarded, &s), Evidential::Supported);

    // Exception certain → defeated, and *not* refuted: the rule did not fire,
    // which is different from the claim being false.
    let s = Told::new().holding(p, vec![a]).holding(e, vec![a]);
    assert_eq!(eval(&mut g, guarded, &s), Evidential::Open);
}

/// §5.2 — `(quantity n u)` under `=` and `<=`: normalised to a base unit via
/// stored `scale` facts, and undecided when no conversion is known.
#[test]
fn row_quantities_normalise_through_stored_scale_facts() {
    use artist_logic::graph_eval::MapGraphStructure;

    let mut g = ObjectGraph::new();
    let (minutes, seconds) = (g.atom("minutes"), g.atom("seconds"));
    let (four, sixty, n240) = (g.int(4), g.int(60), g.int(240));
    let a = g.apply(wk::QUANTITY, vec![four, minutes]);
    let b = g.apply(wk::QUANTITY, vec![n240, seconds]);
    let same = g.apply(wk::EQ, vec![a, b]);

    let s = MapGraphStructure::new().scale_fact(minutes, sixty, 60, seconds);
    assert_eq!(
        GraphEvaluator::new().eval(&mut g, same, &s, 20_000).evidential(),
        Evidential::Supported,
        "one stored fact is the whole of what a new unit costs"
    );
    assert_eq!(
        eval(&mut g, same, &EmptyStructure),
        Evidential::Open,
        "and with no conversion on record, nothing is decided"
    );
}

/// §5.2 — `(at t P)`: evaluate against the structure as it stood at `t`.
#[test]
fn row_at_indexes_by_valid_time() {
    struct Timed(ObjectId);
    impl GraphStructure for Timed {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn known_at(&self, pred: ObjectId, _a: &[ObjectId], t: i64) -> Knowledge {
            if pred == self.0 && t >= 5 { Knowledge::Holds } else { Knowledge::Fails }
        }
    }

    let mut g = ObjectGraph::new();
    let (green, artist) = (g.atom("green"), g.atom("artist"));
    let claim = g.apply(green, vec![artist]);
    let s = Timed(green);

    let late = g.int(9);
    let then = g.apply(wk::AT, vec![late, claim]);
    assert_eq!(eval(&mut g, then, &s), Evidential::Supported);

    let early = g.int(1);
    let before = g.apply(wk::AT, vec![early, claim]);
    assert_eq!(eval(&mut g, before, &s), Evidential::Refuted);

    // Unindexed, the present knows nothing — `at` is the only thing that
    // reaches valid time.
    assert_eq!(eval(&mut g, claim, &s), Evidential::Open);
}
