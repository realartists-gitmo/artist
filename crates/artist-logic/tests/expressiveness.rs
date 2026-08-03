//! The propositions the expressiveness audit could not write.
//!
//! Each test is a real thing a coding agent learns in a day, encoded in the
//! concrete syntax and evaluated. The audit's method was to take such a
//! proposition, attempt it, and report where it got stuck; this is the same
//! list with the gaps closed, so the closure is checked rather than asserted.

use artist_logic::evidence::{ComputeStatus, Evidential};
use artist_logic::graph_eval::{
    EmptyStructure, GraphEvaluator, GraphStructure, Knowledge, MapGraphStructure,
};
use artist_logic::object::{Binding, wk};
use artist_logic::{ObjectGraph, ObjectId};

fn ev(g: &mut ObjectGraph, root: ObjectId, s: &dyn GraphStructure) -> Evidential {
    GraphEvaluator::new().eval(g, root, s, 200_000).evidential()
}

// ------------------------------------------------------------------- values

/// *"The test suite takes 4 minutes."* — the spec's own example, and the one
/// encoding §9 permits was `Open/Stalled` because nothing could give a term a
/// value.
#[test]
fn a_term_can_have_a_value() {
    let mut g = ObjectGraph::new();
    let (duration, suite) = (g.atom("duration"), g.atom("test-suite"));
    let (minutes, seconds) = (g.atom("minutes"), g.atom("seconds"));
    let term = g.apply(duration, vec![suite]);

    let four = g.int(4);
    let four_minutes = g.apply(wk::QUANTITY, vec![four, minutes]);
    let s = MapGraphStructure::new().denotes(term, four_minutes);

    let claim = g.apply(wk::EQ, vec![term, four_minutes]);
    assert_eq!(ev(&mut g, claim, &s), Evidential::Supported);

    // …and it compares across units, given only a `scale` fact.
    let sixty = g.int(60);
    let s = s.scale_fact(minutes, sixty, 60, seconds);
    let two_forty = g.int(240);
    let in_seconds = g.apply(wk::QUANTITY, vec![two_forty, seconds]);
    let same = g.apply(wk::EQ, vec![term, in_seconds]);
    assert_eq!(
        ev(&mut g, same, &s),
        Evidential::Supported,
        "4 minutes is 240 seconds, and the store said so in one fact"
    );
}

/// A unit with no `scale` fact yields an absence, never a verdict.
#[test]
fn an_unknown_unit_decides_nothing() {
    let mut g = ObjectGraph::new();
    let (furlongs, metres) = (g.atom("furlongs"), g.atom("metres"));
    let (one, two) = (g.int(1), g.int(2));
    let a = g.apply(wk::QUANTITY, vec![one, furlongs]);
    let b = g.apply(wk::QUANTITY, vec![two, metres]);
    let claim = g.apply(wk::EQ, vec![a, b]);
    assert_eq!(ev(&mut g, claim, &EmptyStructure), Evidential::Open);
}

// ------------------------------------------------------- negatives, conflict

/// *"`-C target-cpu=native` did not fix the segfault."* The most common thing a
/// coding agent learns, and the only route to `Refuted` used to be declaring the
/// whole relation closed — which simultaneously supported the same claim about
/// every flag nobody had tried.
#[test]
fn a_recorded_negative_does_not_require_omniscience() {
    struct Denials {
        fixes: ObjectId,
        flag: ObjectId,
        bug: ObjectId,
    }
    impl GraphStructure for Denials {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred == self.fixes && args == [self.flag, self.bug] {
                Knowledge::Denied
            } else {
                Knowledge::Unknown
            }
        }
    }

    let mut g = ObjectGraph::new();
    let fixes = g.atom("fixes");
    let (native, lto) = (g.atom("target-cpu-native"), g.atom("lto"));
    let bug = g.atom("segfault-in-rten");
    let s = Denials {
        fixes,
        flag: native,
        bug,
    };

    let tried = g.apply(fixes, vec![native, bug]);
    assert_eq!(
        ev(&mut g, tried, &s),
        Evidential::Refuted,
        "we tried it; it did not work"
    );

    let never_tried = g.apply(fixes, vec![lto, bug]);
    assert_eq!(
        ev(&mut g, never_tried, &s),
        Evidential::Open,
        "and that says nothing whatever about a flag nobody tried"
    );
}

/// Two sessions disagree. `Conflicted` was unreachable from stored facts — the
/// only expression in the language that could produce it was the liar.
#[test]
fn disagreeing_sources_are_conflicted_not_arbitrated() {
    struct Disagreement(ObjectId);
    impl GraphStructure for Disagreement {
        fn known(&self, pred: ObjectId, _args: &[ObjectId]) -> Knowledge {
            if pred == self.0 {
                Knowledge::Conflicted
            } else {
                Knowledge::Unknown
            }
        }
    }
    let mut g = ObjectGraph::new();
    let race = g.atom("is-race");
    let flake = g.atom("flake-7");
    let s = Disagreement(race);
    let claim = g.apply(race, vec![flake]);
    let r = GraphEvaluator::new().eval(&mut g, claim, &s, 10_000);
    assert_eq!(r.evidential(), Evidential::Conflicted);
    assert!(
        !r.is_definite(),
        "a contradiction is not a definite reading"
    );
}

/// …and a conflict must not be laundered back into a verdict by a connective.
#[test]
fn a_conjunction_does_not_flatten_a_conflict() {
    struct Disagreement(ObjectId);
    impl GraphStructure for Disagreement {
        fn known(&self, pred: ObjectId, _args: &[ObjectId]) -> Knowledge {
            if pred == self.0 {
                Knowledge::Conflicted
            } else {
                Knowledge::Unknown
            }
        }
    }
    let mut g = ObjectGraph::new();
    let race = g.atom("is-race");
    let flake = g.atom("flake-7");
    let s = Disagreement(race);
    let claim = g.apply(race, vec![flake]);
    let conj = g.apply(wk::AND, vec![claim, wk::TOP]);
    let r = GraphEvaluator::new().eval(&mut g, conj, &s, 10_000);
    assert!(
        !r.is_definite(),
        "`(and Conflicted #true)` came back a definite Refuted, so its negation \
         was a definite Supported — a verdict manufactured from a contradiction"
    );
}

// ------------------------------------------------------------------- norms

/// *"Run `cargo fmt` before committing."* Smuggled into `necessarily`, a norm
/// was reported **false** the moment it was broken. A memory that records what
/// actually happened cannot model rules that way.
#[test]
fn a_norm_survives_being_broken() {
    let mut g = ObjectGraph::new();
    let (formatted, adam) = (g.atom("formatted"), g.atom("adam"));
    let ran_fmt = g.apply(formatted, vec![adam]);
    let norm = g.apply(wk::OBLIGED, vec![ran_fmt]);

    // The norm is on file; the world does not match it.
    let s = MapGraphStructure::new()
        .fact(wk::OBLIGED, vec![ran_fmt])
        .closed(formatted);

    assert_eq!(
        ev(&mut g, norm, &s),
        Evidential::Supported,
        "the rule holds on the day it is skipped"
    );
    let broke_it = g.apply(wk::VIOLATED, vec![ran_fmt]);
    assert_eq!(
        ev(&mut g, broke_it, &s),
        Evidential::Supported,
        "…and what changes is that `violated` starts holding"
    );
}

/// An obligation entails a permission; nothing else is entailed.
#[test]
fn obligation_entails_permission_and_no_more() {
    let mut g = ObjectGraph::new();
    let (rebase, adam) = (g.atom("rebases"), g.atom("adam"));
    let act = g.apply(rebase, vec![adam]);
    let s = MapGraphStructure::new().fact(wk::OBLIGED, vec![act]);

    let may = g.apply(wk::PERMITTED, vec![act]);
    assert_eq!(ev(&mut g, may, &s), Evidential::Supported);

    // Obliging something does not make it true.
    assert_eq!(
        ev(&mut g, act, &s),
        Evidential::Open,
        "an `ought` is not an `is`"
    );
}

// -------------------------------------------------------------- sequences

/// A stack trace, an argv, the steps of a release. There was no term-level
/// sequence at all — `set` is a domain and cannot be an argument.
#[test]
fn ordered_data_is_representable_and_projectable() {
    let mut g = ObjectGraph::new();
    let (cargo, test, release) = (g.atom("cargo"), g.atom("test"), g.atom("--release"));
    let argv = g.apply(wk::SEQ, vec![cargo, test, release]);

    let one = g.int(1);
    let second = g.apply(wk::NTH, vec![argv, one]);
    let claim = g.apply(wk::EQ, vec![second, test]);
    assert_eq!(ev(&mut g, claim, &EmptyStructure), Evidential::Supported);

    let three = g.int(3);
    let n = g.apply(wk::LEN, vec![argv]);
    let counted = g.apply(wk::EQ, vec![n, three]);
    assert_eq!(ev(&mut g, counted, &EmptyStructure), Evidential::Supported);

    // Out of range decides nothing rather than inventing an element.
    let nine = g.int(9);
    let missing = g.apply(wk::NTH, vec![argv, nine]);
    let bad = g.apply(wk::EQ, vec![missing, test]);
    assert_eq!(ev(&mut g, bad, &EmptyStructure), Evidential::Open);
}

/// …and a sequence can be an *argument*, which is what `set` never could be.
#[test]
fn a_sequence_can_be_a_fact_argument() {
    let mut g = ObjectGraph::new();
    let (ran, t1, t2) = (g.atom("failing-set"), g.atom("t1"), g.atom("t2"));
    let run = g.atom("run-4402");
    let failures = g.apply(wk::SEQ, vec![t1, t2]);
    let claim = g.apply(ran, vec![run, failures]);
    let s = MapGraphStructure::new().fact(ran, vec![run, failures]);
    assert_eq!(ev(&mut g, claim, &s), Evidential::Supported);
}

// ----------------------------------------------------------------- ordering

/// *"Run `cargo fmt` before committing"* orders two **actions**. `before`
/// resolved both sides through `instant_of`, so ordering anything that is not a
/// timestamp — dependency order, review order, process steps — was out.
#[test]
fn before_orders_actions_not_only_instants() {
    let mut g = ObjectGraph::new();
    let (fmt, commit) = (g.atom("run-cargo-fmt"), g.atom("git-commit"));
    let claim = g.apply(wk::BEFORE, vec![fmt, commit]);
    let s = MapGraphStructure::new().fact(wk::BEFORE, vec![fmt, commit]);
    assert_eq!(ev(&mut g, claim, &s), Evidential::Supported);
    assert_eq!(
        ev(&mut g, claim, &EmptyStructure),
        Evidential::Open,
        "and an unordered pair is an absence"
    );
}

/// `since` was declared in `wk::`, matched nowhere, and covered by §5.5's
/// convergence claim. `always`/`eventually` cannot stand in: they range over
/// every instant with no way to restrict to a window.
#[test]
fn since_restricts_to_a_window() {
    struct Timeline {
        green: ObjectId,
        refactor: ObjectId,
    }
    impl GraphStructure for Timeline {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn instants(&self) -> Vec<i64> {
            vec![1, 2, 3, 4]
        }
        fn known_at(&self, pred: ObjectId, _args: &[ObjectId], t: i64) -> Knowledge {
            if pred == self.refactor {
                // The refactor landed at t=2.
                return if t == 2 {
                    Knowledge::Holds
                } else {
                    Knowledge::Fails
                };
            }
            if pred == self.green {
                // CI was red at t=1 and green from t=2 on.
                return if t >= 2 {
                    Knowledge::Holds
                } else {
                    Knowledge::Fails
                };
            }
            Knowledge::Unknown
        }
    }

    let mut g = ObjectGraph::new();
    let (green, refactor) = (g.atom("suite-green"), g.atom("the-refactor"));
    let artist = g.atom("artist");
    let p = g.apply(green, vec![artist]);
    let q = g.apply(refactor, vec![artist]);
    let claim = g.apply(wk::SINCE, vec![p, q]);
    let s = Timeline { green, refactor };
    assert_eq!(
        ev(&mut g, claim, &s),
        Evidential::Supported,
        "green at every instant from the refactor on — the red one at t=1 is outside the window"
    );

    // `always` is the operator that cannot express this, and must still say so.
    let everywhere = g.apply(wk::ALWAYS, vec![p]);
    assert_eq!(
        ev(&mut g, everywhere, &s),
        Evidential::Refuted,
        "…because it was not green at t = 1"
    );
}

// ------------------------------------------------------------ defeasibility

/// *"Usually, a file under `tests/` doesn't need review."* A default's natural
/// form is a **rule**, and `match_goal` required the conclusion's operator to be
/// the goal predicate — so a default could only ever be a ground fact.
#[test]
fn a_defeasible_rule_fires_and_stays_defeasible() {
    let mut g = ObjectGraph::new();
    let (under_tests, needs_review) = (g.atom("under-tests-dir"), g.atom("needs-review"));
    let file = g.atom("tests/gaps.rs");

    let x = g.fresh();
    let antecedent = g.apply(under_tests, vec![x]);
    let bare = g.apply(needs_review, vec![x]);
    let hedged = g.apply(wk::USUALLY, vec![bare]);
    let imp = g.apply(wk::IMPLIES, vec![antecedent, hedged]);
    let rule = g.quantify(wk::FORALL, x, None, imp);

    let s = MapGraphStructure::new()
        .fact(under_tests, vec![file])
        .rule(needs_review, rule);

    let goal = g.apply(needs_review, vec![file]);
    let r = GraphEvaluator::new().eval(&mut g, goal, &s, 100_000);
    assert_eq!(r.evidential(), Evidential::Supported, "the rule fires");
    assert_eq!(
        r.derivation,
        artist_logic::evidence::Derivation::Default,
        "…and a defeasible conclusion stays defeasible"
    );
    assert!(!r.is_definite());
}

/// A rule concluding a *negation* fires too, in the other direction.
#[test]
fn a_rule_may_conclude_a_negation() {
    let mut g = ObjectGraph::new();
    let (generated, needs_review) = (g.atom("generated"), g.atom("needs-review"));
    let file = g.atom("bindings.rs");

    let x = g.fresh();
    let antecedent = g.apply(generated, vec![x]);
    let bare = g.apply(needs_review, vec![x]);
    let negated = g.apply(wk::NOT, vec![bare]);
    let imp = g.apply(wk::IMPLIES, vec![antecedent, negated]);
    let rule = g.quantify(wk::FORALL, x, None, imp);

    let s = MapGraphStructure::new()
        .fact(generated, vec![file])
        .rule(needs_review, rule);
    let goal = g.apply(needs_review, vec![file]);
    assert_eq!(ev(&mut g, goal, &s), Evidential::Refuted);
}

// ------------------------------------------------------------- composition

/// `at` and `in-world` are separate indices and must compose. The dispatch
/// dropped the world whenever an instant was set, so a counterfactual about a
/// past state was silently answered against the present one.
#[test]
fn at_and_in_world_compose() {
    struct Indexed {
        green: ObjectId,
        world: ObjectId,
    }
    impl GraphStructure for Indexed {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn worlds(&self) -> Vec<ObjectId> {
            vec![self.world]
        }
        fn known_at(&self, _p: ObjectId, _a: &[ObjectId], _t: i64) -> Knowledge {
            // Nothing is known about the *actual* past.
            Knowledge::Unknown
        }
        fn known_at_in(
            &self,
            pred: ObjectId,
            _a: &[ObjectId],
            t: i64,
            world: ObjectId,
        ) -> Knowledge {
            if pred == self.green && t == 3 && world == self.world {
                Knowledge::Holds
            } else {
                Knowledge::Unknown
            }
        }
    }

    let mut g = ObjectGraph::new();
    let (green, artist) = (g.atom("green"), g.atom("artist"));
    let w = g.atom("if-we-had-merged");
    let claim = g.apply(green, vec![artist]);
    let in_w = g.apply(wk::IN_WORLD, vec![w, claim]);
    let three = g.int(3);
    let both = g.apply(wk::AT, vec![three, in_w]);

    let s = Indexed { green, world: w };
    assert_eq!(
        ev(&mut g, both, &s),
        Evidential::Supported,
        "a counterfactual about a past state must reach the index that holds it"
    );
}

// -------------------------------------------------------------- aggregates

/// Totalling durations, sizes or coverage percentages was out of reach because
/// the contribution extractor read integers only.
#[test]
fn aggregates_are_not_integers_only() {
    let mut g = ObjectGraph::new();
    let counts = g.atom("counts");
    let (a, b) = (g.atom("a"), g.atom("b"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let s = MapGraphStructure::new()
        .fact(counts, vec![a])
        .fact(counts, vec![b]);

    let v = g.fresh();
    let body = g.apply(counts, vec![v]);
    // Each member contributes 0.5, so the total is exactly 1 — and would be 0
    // under integer truncation.
    let half = g.lit(artist_logic::object::LiteralValue::Decimal {
        mantissa: num_bigint::BigInt::from(5),
        scale: 1,
    });
    let total = g.bind(
        wk::SUM,
        vec![Binding {
            var: v,
            domain: Some(dom),
        }],
        vec![body, half],
    );
    let one = g.int(1);
    let claim = g.apply(wk::EQ, vec![total, one]);
    assert_eq!(ev(&mut g, claim, &s), Evidential::Supported);
}

/// The status axis stays honest across all of the above: nothing new claims
/// `Exact` for a question it could not reach.
#[test]
fn the_new_vocabulary_reports_unsupported_rather_than_guessing() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let claim = g.apply(p, vec![a]);
    for op in [wk::OBLIGED, wk::PERMITTED, wk::FORBIDDEN, wk::VIOLATED] {
        let node = g.apply(op, vec![claim]);
        let r = GraphEvaluator::new().eval(&mut g, node, &EmptyStructure, 10_000);
        assert_eq!(
            r.evidential(),
            Evidential::Open,
            "{op:?} must not invent a norm"
        );
        assert_ne!(r.compute_status, ComputeStatus::Exact);
    }
    // Wrong arity everywhere, as the rest of the vocabulary already does.
    for op in [wk::OBLIGED, wk::VIOLATED, wk::SINCE, wk::NTH] {
        let node = g.apply(op, vec![claim, claim, claim]);
        let r = GraphEvaluator::new().eval(&mut g, node, &EmptyStructure, 10_000);
        assert_eq!(r.compute_status, ComputeStatus::Unsupported, "{op:?} arity");
    }
}
