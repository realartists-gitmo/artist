//! One test per finding from the three adversarial audits.
//!
//! Most of these assert the **absence** of a verdict — three do the opposite,
//! deliberately, and are marked where they appear: a repair that answers
//! *nothing* passes every absence test ever written, so the guard against
//! over-correction has to be here too. That is the shape
//! the failures took: not a wrong answer where a right one was available, but a
//! confident answer where the honest output was "I do not know". Thirteen
//! unsound findings and one panic, and they were all the same sentence one
//! level down from the previous repair —
//!
//! > when the evaluator lacks information, it produces a confident answer
//! > instead of an absence
//!
//! — except that this time the information was not absent. It was computed and
//! then dropped: `Bound::Partial` at seven scan sites, `AggregateBounds.exact`
//! in `interval`, `Extension.complete` in `narrow`, `lo` in the tail
//! abstraction, `worst` in `derive`, `self.negated` in `implies`. Each of those
//! sat under a comment saying it was used.

use artist_logic::evidence::{Bound, ComputeStatus, Derivation, Evidential};
use artist_logic::graph_eval::{
    EmptyStructure, Extension, GraphEvaluator, GraphStructure, Knowledge, MapGraphStructure,
};
use artist_logic::object::{Binding, LiteralValue, wk};
use artist_logic::{ObjectGraph, ObjectId};
use num_bigint::BigInt;

// ---------------------------------------------------------------- structures

/// Lists some members without promising they are all of them — the shape of a
/// paged query, a permission filter, or an index that covers part of the
/// argument space.
struct PartialIndex {
    domain: ObjectId,
    members: Vec<ObjectId>,
    holds: Vec<(ObjectId, Vec<ObjectId>)>,
}

impl GraphStructure for PartialIndex {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        if self.holds.iter().any(|(p, a)| *p == pred && a == args) {
            Knowledge::Holds
        } else {
            Knowledge::Unknown
        }
    }
    fn extension(&self, domain: ObjectId) -> Option<Extension> {
        (domain == self.domain).then(|| Extension::partial(self.members.clone()))
    }
}

/// One world, one instant, and **one** default on record — enough to drive
/// every scan site through a defeasible case.
///
/// It used to answer `Holds` for `usually` whatever it was asked, which made it
/// a structure that lies: it asserted a default for `P` *and* for `¬P`
/// simultaneously. That is now correctly reported as `Conflicted`, so the
/// fixture had to become honest before it could test anything.
struct DefaultsEverywhere {
    world: ObjectId,
    default_for: ObjectId,
}

impl GraphStructure for DefaultsEverywhere {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        if pred == wk::USUALLY && args == [self.default_for] {
            Knowledge::Holds
        } else {
            Knowledge::Unknown
        }
    }
    fn worlds(&self) -> Vec<ObjectId> {
        vec![self.world]
    }
    fn instants(&self) -> Vec<i64> {
        vec![1, 2]
    }
    fn closest_worlds(&self, _from: ObjectId, _cond: ObjectId) -> Option<Vec<ObjectId>> {
        Some(vec![self.world])
    }
}

fn defaulted(g: &mut ObjectGraph) -> (DefaultsEverywhere, ObjectId) {
    let w = g.atom("w0");
    let (p, a) = (g.atom("fast"), g.atom("build"));
    let claim = g.apply(p, vec![a]);
    let hedged = g.apply(wk::USUALLY, vec![claim]);
    (
        DefaultsEverywhere {
            world: w,
            default_for: claim,
        },
        hedged,
    )
}

fn eval(
    g: &mut ObjectGraph,
    root: ObjectId,
    s: &dyn GraphStructure,
    budget: u64,
) -> (Evidential, ComputeStatus, Bound, Bound) {
    let r = GraphEvaluator::new().eval(g, root, s, budget);
    (r.evidential(), r.compute_status, r.support, r.refutation)
}

// ------------------------------------------- 1. Partial, invisible to scans

/// The pattern in one assertion: a scan must not promote a default into a
/// *settled reading*. `usually` returns a defeasible `Certain/Exact`, and every scan
/// classified a
/// case by `(support.is_certain(), refutation.is_certain())` — so an undecided
/// case was indistinguishable from a decided one by the time the loop ended.
#[test]
fn a_universal_over_defaults_is_not_definite() {
    let mut g = ObjectGraph::new();
    let (s, hedged) = defaulted(&mut g);
    let (a, b) = (g.atom("a"), g.atom("b"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let v = g.fresh();
    let q = g.quantify(wk::FORALL, v, Some(dom), hedged);

    let (ev, ..) = eval(&mut g, q, &s, 50_000);
    assert_eq!(ev, Evidential::Supported, "the default still supports");
    let r = GraphEvaluator::new().eval(&mut g, q, &s, 50_000);
    assert_eq!(
        r.derivation,
        Derivation::Default,
        "…and the defeasibility travels up the scan"
    );
    assert!(
        !r.is_definite(),
        "a universal over defaults is not a definite reading"
    );
}

/// The worse direction, and the one that should sting: the evaluator refuted a
/// proposition while supporting its only instance.
#[test]
fn an_existential_over_defaults_is_not_refuted() {
    let mut g = ObjectGraph::new();
    let (s, hedged) = defaulted(&mut g);
    let a = g.atom("a");
    let dom = g.apply(wk::SET_DOMAIN, vec![a]);
    let v = g.fresh();
    let q = g.quantify(wk::EXISTS, v, Some(dom), hedged);

    let (instance, ..) = eval(&mut g, hedged, &s, 10_000);
    assert_eq!(
        instance,
        Evidential::Supported,
        "the sole instance is supported"
    );
    let (ev, ..) = eval(&mut g, q, &s, 50_000);
    assert_ne!(
        ev,
        Evidential::Refuted,
        "an existential cannot be refuted while its only instance is supported"
    );
}

/// The same shape at the other six sites.
#[test]
fn no_scan_site_promotes_or_refutes_a_default() {
    for (label, build) in [
        ("possibly", wk::POSSIBLY),
        ("necessarily", wk::NECESSARILY),
        ("always", wk::ALWAYS),
        ("eventually", wk::EVENTUALLY),
    ] {
        let mut g = ObjectGraph::new();
        let (s, hedged) = defaulted(&mut g);
        let q = g.apply(build, vec![hedged]);
        let (ev, ..) = eval(&mut g, q, &s, 50_000);
        assert_ne!(ev, Evidential::Refuted, "{label} must not refute a default");
        let r = GraphEvaluator::new().eval(&mut g, q, &s, 50_000);
        assert!(!r.is_definite(), "{label} must not settle a default");
        assert_eq!(
            r.derivation,
            Derivation::Default,
            "{label} loses the defeasibility"
        );
    }

    // …and the counterfactual, which takes two operands.
    let mut g = ObjectGraph::new();
    let (s, hedged) = defaulted(&mut g);
    let cond = g.atom("merged");
    let q = g.apply(wk::COUNTERFACTUAL, vec![cond, hedged]);
    let r = GraphEvaluator::new().eval(&mut g, q, &s, 50_000);
    assert!(
        !r.is_definite(),
        "a counterfactual must not settle a default"
    );
}

/// A `letrec` body that is merely defaulted leaves the tuple *undetermined*,
/// not out — so the relation cannot then be published complete, which would
/// turn that absence into a refutation.
#[test]
fn letrec_does_not_publish_a_relation_built_from_defaults() {
    let mut g = ObjectGraph::new();
    let (s, hedged) = defaulted(&mut g);
    let a = g.atom("a");
    let dom = g.apply(wk::SET_DOMAIN, vec![a]);
    let rel_ty = g.apply(wk::RELATION_TYPE, vec![dom]);
    let (p, x) = (g.fresh(), g.fresh());
    let scope = g.apply(p, vec![a]);
    let node = g.bind(
        wk::LETREC,
        vec![
            Binding {
                var: p,
                domain: Some(rel_ty),
            },
            Binding {
                var: x,
                domain: Some(dom),
            },
        ],
        vec![hedged, scope],
    );
    let (ev, ..) = eval(&mut g, node, &s, 50_000);
    assert_ne!(
        ev,
        Evidential::Refuted,
        "an undetermined tuple is not an absent one"
    );
}

/// A one-element disjunction lost information relative to its own operand:
/// `or` always passed `Bound::None` as support outside the short-circuit. The
/// level it lost is now carried on `Derivation`, which is where defeasibility
/// belongs — the shape of the defect is the same, the axis is not.
#[test]
fn the_connectives_carry_the_defeasibility_of_their_operands() {
    let mut g = ObjectGraph::new();
    let (s, hedged) = defaulted(&mut g);

    let disj = g.apply(wk::OR, vec![hedged]);
    let r = GraphEvaluator::new().eval(&mut g, disj, &s, 20_000);
    assert_eq!(
        r.evidential(),
        Evidential::Supported,
        "a 1-ary disjunction is its operand"
    );
    assert_eq!(r.support, Bound::Certain);
    assert_eq!(
        r.derivation,
        Derivation::Default,
        "and carries its operand's defeasibility"
    );

    let neg = g.apply(wk::NOT, vec![hedged]);
    let conj = g.apply(wk::AND, vec![neg, wk::TOP]);
    let r = GraphEvaluator::new().eval(&mut g, conj, &s, 20_000);
    assert_eq!(
        r.refutation,
        Bound::Certain,
        "§5.2: refutation of a conjunction is the join over its conjuncts"
    );
    assert_eq!(r.derivation, Derivation::Default);
}

// ------------------------------------- 2. completeness flags nobody read

/// Found independently by two of the three auditors, which is the strongest
/// signal in the set. `interval` forwarded `hi` without consulting the flag, so
/// the laundering the `=` path was repaired for arrived through `<=`.
#[test]
fn a_partial_index_does_not_bound_an_aggregate() {
    let mut g = ObjectGraph::new();
    let broken = g.atom("broken");
    let files = g.atom("Files");
    let a = g.atom("a.rs");
    let s = PartialIndex {
        domain: files,
        members: vec![a],
        holds: vec![(broken, vec![a])],
    };

    let v = g.fresh();
    let body = g.apply(broken, vec![v]);
    let counted = g.bind(
        wk::COUNT,
        vec![Binding {
            var: v,
            domain: Some(files),
        }],
        vec![body],
    );

    let one = g.int(1);
    let at_most_one = g.apply(wk::LEQ, vec![counted, one]);
    let (ev, ..) = eval(&mut g, at_most_one, &s, 50_000);
    assert_ne!(
        ev,
        Evidential::Supported,
        "the index never promised those were all the files"
    );

    let two = g.int(2);
    let at_least_two = g.apply(wk::LEQ, vec![two, counted]);
    let (ev, ..) = eval(&mut g, at_least_two, &s, 50_000);
    assert_ne!(
        ev,
        Evidential::Refuted,
        "…and cannot refute the other direction either"
    );
}

/// A complete enumeration with an undecided *predicate* is the other case, and
/// it must keep working: `hi` really does bound the total there.
#[test]
fn a_complete_enumeration_still_bounds_an_aggregate() {
    let mut g = ObjectGraph::new();
    let fails = g.atom("fails");
    let members: Vec<ObjectId> = ["t1", "t2", "t3"].iter().map(|n| g.atom(n)).collect();
    let dom = g.apply(wk::SET_DOMAIN, members.clone());
    let s = MapGraphStructure::new().fact(fails, vec![members[0]]);

    let v = g.fresh();
    let body = g.apply(fails, vec![v]);
    let counted = g.bind(
        wk::COUNT,
        vec![Binding {
            var: v,
            domain: Some(dom),
        }],
        vec![body],
    );
    let four = g.int(4);
    let at_most_four = g.apply(wk::LEQ, vec![counted, four]);
    let (ev, ..) = eval(&mut g, at_most_four, &s, 50_000);
    assert_eq!(
        ev,
        Evidential::Supported,
        "three members cannot total more than three"
    );
}

/// Suspending and resuming was a soundness *upgrade*: `narrow` rebuilt the
/// remaining members as a `set`, which is exhaustive by construction, so the
/// continuation asked a strictly stronger question than the one suspended.
#[test]
fn a_continuation_is_never_stronger_than_the_question_that_made_it() {
    let mut g = ObjectGraph::new();
    let ok = g.atom("ok");
    let files = g.atom("Files");
    let members: Vec<ObjectId> = (0..40).map(|i| g.atom(&format!("f{i}"))).collect();
    let s = PartialIndex {
        domain: files,
        members: members.clone(),
        holds: members.iter().map(|m| (ok, vec![*m])).collect(),
    };

    let v = g.fresh();
    let body = g.apply(ok, vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(files), body);

    let whole = GraphEvaluator::new().eval(&mut g, q, &s, 1_000_000);
    assert_eq!(
        whole.evidential(),
        Evidential::Open,
        "a partial index decides nothing"
    );

    let cut = GraphEvaluator::new().eval(&mut g, q, &s, 40);
    let Some(k) = cut.continuation else {
        panic!("a cut-short scan must hand back a continuation");
    };
    let resumed = GraphEvaluator::new().eval(&mut g, k, &s, 1_000_000);
    assert_ne!(
        resumed.evidential(),
        Evidential::Supported,
        "resuming must not conclude what the whole query could not"
    );
}

// ------------------------------------------------- 3. the tail abstraction

/// The worst finding: a true existential came back `Refuted/Exact` at one
/// budget and `Supported/Exact` at another. `abstract_holds` answered "does it
/// hold *everywhere*" and the caller read a `no` as "it holds *nowhere*".
#[test]
fn an_infinite_domain_answer_does_not_flip_with_budget() {
    for budget in [100u64, 150, 200, 300, 400, 1_000, 5_000] {
        let mut g = ObjectGraph::new();
        let n = g.fresh();
        let (lo, hi) = (g.int(100), g.int(200));
        let a = g.apply(wk::LEQ, vec![lo, n]);
        let b = g.apply(wk::LEQ, vec![n, hi]);
        let body = g.apply(wk::AND, vec![a, b]);
        let q = g.quantify(wk::EXISTS, n, Some(wk::NAT_TYPE), body);

        let (ev, ..) = eval(&mut g, q, &EmptyStructure, budget);
        assert_ne!(
            ev,
            Evidential::Refuted,
            "n = 150 witnesses this; refuting it at budget {budget} is a wrong answer"
        );
    }
}

/// The universal mirror of the same term, reached through the `not` arm.
#[test]
fn the_universal_mirror_is_not_supported_either() {
    for budget in [100u64, 200, 400, 1_000] {
        let mut g = ObjectGraph::new();
        let n = g.fresh();
        let (lo, hi) = (g.int(100), g.int(200));
        let a = g.apply(wk::LEQ, vec![lo, n]);
        let b = g.apply(wk::LEQ, vec![n, hi]);
        let inner = g.apply(wk::AND, vec![a, b]);
        let body = g.apply(wk::NOT, vec![inner]);
        let q = g.quantify(wk::FORALL, n, Some(wk::NAT_TYPE), body);

        let (ev, ..) = eval(&mut g, q, &EmptyStructure, budget);
        assert_ne!(ev, Evidential::Supported, "n = 150 is a counterexample");
    }
}

/// What the tail analysis must still decide, so the four-valued rewrite is not
/// simply a way of never answering.
#[test]
fn the_tail_still_decides_what_it_can() {
    let mut g = ObjectGraph::new();
    let n = g.fresh();
    let one = g.int(1);
    let succ = g.apply(wk::ADD, vec![n, one]);
    let body = g.apply(wk::LEQ, vec![n, succ]);
    let q = g.quantify(wk::FORALL, n, Some(wk::NAT_TYPE), body);
    let (ev, status, ..) = eval(&mut g, q, &EmptyStructure, 50_000);
    assert_eq!(
        ev,
        Evidential::Supported,
        "n ≤ n + 1 holds throughout the tail"
    );
    assert_eq!(status, ComputeStatus::Exact);
}

// -------------------------------------------------------- 4. negation parity

/// `implies` negated its *result* while leaving `self.negated` at the outer
/// parity, so the cycle detector saw a spurious flip around a stable loop.
/// Classically identical spellings disagreed.
#[test]
fn a_stable_truth_teller_does_not_depend_on_its_spelling() {
    // B := (not (implies B #false)) — classically ¬¬B.
    let mut g = ObjectGraph::new();
    let b = g.alloc();
    let imp = g.apply(wk::IMPLIES, vec![b, wk::BOT]);
    let outer = g.apply(wk::NOT, vec![imp]);
    let node = g.get(outer).cloned().expect("built");
    g.define(b, node);
    let r = GraphEvaluator::new().eval(&mut g, b, &EmptyStructure, 10_000);
    assert_eq!(
        r.evidential(),
        Evidential::Open,
        "a truth-teller never gets a value"
    );
    assert!(!r.is_definite());

    // A := (not (not A)) — the same sentence, spelled differently.
    let mut h = ObjectGraph::new();
    let a = h.alloc();
    let inner = h.apply(wk::NOT, vec![a]);
    let outer = h.apply(wk::NOT, vec![inner]);
    let node = h.get(outer).cloned().expect("built");
    h.define(a, node);
    let r2 = GraphEvaluator::new().eval(&mut h, a, &EmptyStructure, 10_000);
    assert_eq!(
        r2.evidential(),
        Evidential::Open,
        "and neither does this one"
    );
}

/// `occurs_negatively` recognised only `not`, so `p(a) ↔ ¬p(a)` written with
/// `implies` was judged monotone, published complete, and returned a verdict
/// for a definition with no fixpoint.
#[test]
fn a_negative_occurrence_spelled_with_implies_is_still_negative() {
    let mut g = ObjectGraph::new();
    let a = g.atom("a");
    let dom = g.apply(wk::SET_DOMAIN, vec![a]);
    let rel_ty = g.apply(wk::RELATION_TYPE, vec![dom]);
    let (p, x) = (g.fresh(), g.fresh());
    let px = g.apply(p, vec![x]);
    let def = g.apply(wk::IMPLIES, vec![px, wk::BOT]);
    let scope = g.apply(p, vec![a]);
    let node = g.bind(
        wk::LETREC,
        vec![
            Binding {
                var: p,
                domain: Some(rel_ty),
            },
            Binding {
                var: x,
                domain: Some(dom),
            },
        ],
        vec![def, scope],
    );
    let r = GraphEvaluator::new().eval(&mut g, node, &EmptyStructure, 50_000);
    assert!(
        !r.is_definite(),
        "p(a) ↔ ¬p(a) has no fixpoint; no verdict is available"
    );
}

// ------------------------------------------------ 5. derive, and the panic

/// `let _ = worst;` — a derivation whose antecedent ran out of budget reported
/// `Stalled`, documented as "re-asking will stall the same way". More budget was
/// exactly what would have helped.
#[test]
fn derive_reports_budget_exhaustion_as_budget_exhaustion() {
    struct Chained {
        rule: ObjectId,
        goal_pred: ObjectId,
    }
    impl GraphStructure for Chained {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn rules(&self, pred: ObjectId) -> Vec<ObjectId> {
            if pred == self.goal_pred {
                vec![self.rule]
            } else {
                Vec::new()
            }
        }
    }

    let mut g = ObjectGraph::new();
    let (needs, touches, item) = (g.atom("needs"), g.atom("touches"), g.atom("c1"));
    let big: Vec<ObjectId> = (0..60).map(|i| g.atom(&format!("m{i}"))).collect();
    let dom = g.apply(wk::SET_DOMAIN, big);
    let v = g.fresh();
    let inner = g.apply(touches, vec![v]);
    let antecedent = g.quantify(wk::FORALL, v, Some(dom), inner);
    let x = g.fresh();
    let consequent = g.apply(needs, vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![antecedent, consequent]);
    let rule = g.quantify(wk::FORALL, x, None, imp);

    let s = Chained {
        rule,
        goal_pred: needs,
    };
    let goal = g.apply(needs, vec![item]);
    let r = GraphEvaluator::new().eval(&mut g, goal, &s, 40);
    assert_ne!(
        r.compute_status,
        ComputeStatus::Stalled,
        "a budget wall is not a stall — `Stalled` means re-asking will not help"
    );
}

/// A parseable literal could crash the process before the budget was consulted,
/// so "evaluation is total under a budget" was false.
#[test]
fn an_extreme_decimal_scale_does_not_panic() {
    let mut g = ObjectGraph::new();
    for scale in [i32::MIN, i32::MIN + 1, i32::MAX, 1_000_000, -1_000_000] {
        let d = g.lit(LiteralValue::Decimal {
            mantissa: BigInt::from(1),
            scale,
        });
        let one = g.int(1);
        let claim = g.apply(wk::EQ, vec![d, one]);
        let r = GraphEvaluator::new().eval(&mut g, claim, &EmptyStructure, 10_000);
        assert_ne!(
            r.evidential(),
            Evidential::Supported,
            "1e-{scale} is not 1, and asking must not crash"
        );
    }
}

// ------------------------------------------- 6. what the split of ground fixed

/// A user predicate could not take a compound argument at all, so the store
/// could not record *why* anything happened — while the seven intensional
/// operators answered the same shape on the same store.
#[test]
fn a_user_predicate_accepts_a_compound_argument() {
    let mut g = ObjectGraph::new();
    let causes = g.atom("causes");
    let (omits, header, cstdint) = (
        g.atom("omits"),
        g.atom("rocksdb-slice.h"),
        g.atom("cstdint"),
    );
    let (fails, crate_id) = (g.atom("build-fails"), g.atom("mnestic-rocks-0.1.10"));

    let cause = g.apply(omits, vec![header, cstdint]);
    let effect = g.apply(fails, vec![crate_id]);
    let quoted_cause = g.apply(wk::QUOTE, vec![cause]);
    let quoted_effect = g.apply(wk::QUOTE, vec![effect]);
    let claim = g.apply(causes, vec![quoted_cause, quoted_effect]);

    let s = MapGraphStructure::new().fact(causes, vec![quoted_cause, quoted_effect]);
    let (ev, status, ..) = eval(&mut g, claim, &s, 20_000);
    assert_eq!(
        ev,
        Evidential::Supported,
        "the store holds exactly this tuple"
    );
    assert_eq!(status, ComputeStatus::Exact);
}

/// …and the strictness it was protecting is intact: `=` on an uncomputable
/// compound still refuses to refute, which is what stopped the fabricated
/// verdicts in the first place.
#[test]
fn equality_on_a_compound_still_refuses_to_refute() {
    let mut g = ObjectGraph::new();
    let duration = g.atom("build-minutes");
    let artist = g.atom("artist");
    let term = g.apply(duration, vec![artist]);
    let four = g.int(4);
    let claim = g.apply(wk::EQ, vec![term, four]);
    let (ev, ..) = eval(&mut g, claim, &EmptyStructure, 10_000);
    assert_eq!(
        ev,
        Evidential::Open,
        "the ids differ; the denotations may not"
    );
}
