//! Round four: the axes, and the structural guarantee that carries them.
//!
//! Every finding in the round-four evaluator audit had one root cause —
//! `EvaluationResult::absorb`, a helper written to fold `grounding`,
//! `derivation` and `defeated_by` at composition sites, was called from
//! **nowhere**. Eight sites reached a verdict through a bare
//! `EvaluationResult::certain(…)` and dropped three axes silently.
//!
//! Fixing eight sites would have been the fourth time in this project that a
//! class of defect was repaired instance by instance. The class is *"a verdict
//! can be constructed without consulting what it was derived from"*, and the
//! repair is that it no longer can: `dispatch` stamps every compound verdict
//! from an accumulator that every `check` feeds, above every arm, where no
//! operator can bypass it. These tests are the class, not the instances.

use artist_logic::evidence::{Derivation, Evidential, Grounding};
use artist_logic::graph_eval::{
    EmptyStructure, GraphEvaluator, GraphStructure, Knowledge, MapGraphStructure,
};
use artist_logic::object::{Binding, wk};
use artist_logic::{ObjectGraph, ObjectId};

/// A default on record for one claim, and a rule that concludes it defeasibly.
fn defaulted(g: &mut ObjectGraph) -> (MapGraphStructure, ObjectId, ObjectId) {
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let s = MapGraphStructure::new().fact(wk::USUALLY, vec![claim]);
    let hedged = g.apply(wk::USUALLY, vec![claim]);
    (s, hedged, claim)
}

/// **The class.** Wrap a default in anything at all and the defeasibility
/// survives. Fifteen shapes, including the three that dropped it.
#[test]
fn no_composition_loses_the_provenance_of_a_default() {
    let mut g = ObjectGraph::new();
    let (s, hedged, _) = defaulted(&mut g);
    let w = g.atom("w");
    let s = s.world(w).instant(1).instant(2);
    let (one, tru) = (g.int(1), wk::TOP);
    let dom = {
        let m = g.atom("m");
        g.apply(wk::SET_DOMAIN, vec![m])
    };

    let unary = [
        wk::NOT,
        wk::USUALLY,
        wk::VIOLATED,
        wk::NECESSARILY,
        wk::POSSIBLY,
        wk::ALWAYS,
        wk::EVENTUALLY,
        wk::OR,
        wk::AND,
    ];
    for op in unary {
        let node = g.apply(op, vec![hedged]);
        let r = GraphEvaluator::new().eval(&mut g, node, &s, 50_000);
        assert_eq!(
            r.derivation,
            Derivation::Default,
            "{op:?} dropped the defeasibility of its operand"
        );
        assert!(!r.is_definite(), "{op:?} settled a default");
    }

    for (op, extra) in [
        (wk::AT, one),
        (wk::IN_WORLD, w),
        (wk::UNLESS, wk::BOT),
        (wk::IMPLIES, tru),
    ] {
        let node = g.apply(op, vec![extra, hedged]);
        let r = GraphEvaluator::new().eval(&mut g, node, &s, 50_000);
        assert_eq!(r.derivation, Derivation::Default, "{op:?} dropped it");
        assert!(!r.is_definite(), "{op:?} settled a default");
    }

    // …and through a quantifier and an aggregate.
    let v = g.fresh();
    let q = g.quantify(wk::EXISTS, v, Some(dom), hedged);
    let r = GraphEvaluator::new().eval(&mut g, q, &s, 50_000);
    assert_eq!(r.derivation, Derivation::Default, "the scan dropped it");

    let counted = g.bind(
        wk::COUNT,
        vec![Binding {
            var: v,
            domain: Some(dom),
        }],
        vec![hedged],
    );
    let zero = g.int(0);
    let cmp = g.apply(wk::LEQ, vec![zero, counted]);
    let r = GraphEvaluator::new().eval(&mut g, cmp, &s, 50_000);
    assert_eq!(
        r.derivation,
        Derivation::Default,
        "an aggregate is a reader — it never hands its sub-results back, which is \
         exactly why the accumulator and not the arm has to carry them"
    );
    assert!(!r.is_definite());
}

/// One extra rule hop erased both the defeasibility and the defeater list, so
/// §7's "retraction removes what depended on it" was unimplementable.
#[test]
fn a_rule_chain_does_not_launder_a_default_into_a_fact() {
    let mut g = ObjectGraph::new();
    let (a, b, c, k) = (g.atom("a"), g.atom("b"), g.atom("c"), g.atom("k"));

    let x = g.fresh();
    let ax = g.apply(a, vec![x]);
    let bx = g.apply(b, vec![x]);
    let hedged = g.apply(wk::USUALLY, vec![bx]);
    let r1 = {
        let imp = g.apply(wk::IMPLIES, vec![ax, hedged]);
        g.quantify(wk::FORALL, x, None, imp)
    };
    let y = g.fresh();
    let by = g.apply(b, vec![y]);
    let cy = g.apply(c, vec![y]);
    let r2 = {
        let imp = g.apply(wk::IMPLIES, vec![by, cy]);
        g.quantify(wk::FORALL, y, None, imp)
    };
    let s = MapGraphStructure::new()
        .fact(a, vec![k])
        .rule(b, r1)
        .rule(c, r2);

    let goal_b = g.apply(b, vec![k]);
    let rb = GraphEvaluator::new().eval(&mut g, goal_b, &s, 100_000);
    assert_eq!(rb.derivation, Derivation::Default);

    let goal_c = g.apply(c, vec![k]);
    let rc = GraphEvaluator::new().eval(&mut g, goal_c, &s, 100_000);
    assert_eq!(
        rc.derivation,
        Derivation::Default,
        "one more hop must not turn a default into an observed fact"
    );
    assert!(!rc.is_definite());
    assert!(
        !rc.defeated_by.is_empty(),
        "and the defeater must survive the hop"
    );
}

/// A `letrec` fixpoint built from defaults was published as an observed
/// relation, and its own regression test had stopped detecting that because it
/// only asserted `!= Refuted`.
#[test]
fn a_fixpoint_built_from_defaults_stays_defeasible() {
    let mut g = ObjectGraph::new();
    let (s, hedged, _) = defaulted(&mut g);
    let m = g.atom("m");
    let dom = g.apply(wk::SET_DOMAIN, vec![m]);
    let rel_ty = g.apply(wk::RELATION_TYPE, vec![dom]);
    let (pv, xv) = (g.fresh(), g.fresh());
    let scope = g.apply(pv, vec![m]);
    let node = g.bind(
        wk::LETREC,
        vec![
            Binding {
                var: pv,
                domain: Some(rel_ty),
            },
            Binding {
                var: xv,
                domain: Some(dom),
            },
        ],
        vec![hedged, scope],
    );
    let r = GraphEvaluator::new().eval(&mut g, node, &s, 50_000);
    assert!(
        !r.is_definite(),
        "a relation whose membership rests on a default is not observed fact, \
         got {:?}/{:?} {:?}",
        r.support,
        r.refutation,
        r.derivation
    );
}

/// The same question in two spellings disagreed, because one went through
/// `infinite()` and the other through `Scan`.
#[test]
fn an_infinite_domain_carries_the_axes_a_finite_one_does() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let three = g.int(3);
    let claim = g.apply(p, vec![three]);
    let s = MapGraphStructure::new().fact(wk::USUALLY, vec![claim]);

    let v = g.fresh();
    let inner = g.apply(p, vec![v]);
    let body = g.apply(wk::USUALLY, vec![inner]);
    let dom = g.apply(wk::SET_DOMAIN, vec![three]);

    let finite = g.quantify(wk::EXISTS, v, Some(dom), body);
    let infinite = g.quantify(wk::EXISTS, v, Some(wk::NAT_TYPE), body);
    let ev = GraphEvaluator::new();
    let (a, b) = (
        ev.eval(&mut g, finite, &s, 100_000),
        ev.eval(&mut g, infinite, &s, 100_000),
    );
    assert_eq!(
        (a.derivation, a.is_definite()),
        (b.derivation, b.is_definite()),
        "one sentence, two domains, two answers"
    );
}

/// Ungroundedness propagates through a reader too, so a conclusion resting on
/// the liar cannot report itself grounded.
#[test]
fn ungroundedness_reaches_the_verdict_it_supports() {
    let mut g = ObjectGraph::new();
    let liar = g.alloc();
    let q = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q]);
    let n = g.apply(wk::NOT, vec![h]);
    let node = g.get(n).cloned().expect("built");
    g.define(liar, node);

    let p = g.atom("p");
    let a = g.atom("a");
    let fact = g.apply(p, vec![a]);
    let s = MapGraphStructure::new().fact(p, vec![a]);

    let guarded = g.apply(wk::UNLESS, vec![liar, fact]);
    let r = GraphEvaluator::new().eval(&mut g, guarded, &s, 50_000);
    assert_eq!(
        r.grounding,
        Grounding::Oscillatory,
        "a claim whose defeat condition has no stable value is not grounded"
    );
    assert!(!r.is_definite());
}

/// `same-as` must not flatten a contested identity, in either direction.
#[test]
fn a_contested_identity_is_neither_flattened_nor_discarded() {
    struct Table {
        fwd: Knowledge,
        rev: Knowledge,
        a: ObjectId,
        b: ObjectId,
    }
    impl GraphStructure for Table {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred != wk::SAME_AS {
                return Knowledge::Unknown;
            }
            if args == [self.a, self.b] {
                self.fwd
            } else if args == [self.b, self.a] {
                self.rev
            } else {
                Knowledge::Unknown
            }
        }
    }
    use Knowledge::*;
    let mut g = ObjectGraph::new();
    let (a, b) = (g.atom("a"), g.atom("b"));
    let claim = g.apply(wk::SAME_AS, vec![a, b]);

    for (fwd, rev) in [
        (Holds, Conflicted),
        (Denied, Conflicted),
        (Conflicted, Unknown),
        (Conflicted, Conflicted),
        (Holds, Denied),
    ] {
        let s = Table { fwd, rev, a, b };
        let r = GraphEvaluator::new().eval(&mut g, claim, &s, 10_000);
        assert_eq!(
            r.evidential(),
            Evidential::Conflicted,
            "({fwd:?}, {rev:?}) must stay contested"
        );
        assert!(!r.is_definite());
    }

    // …and the cases the table was written for still work.
    let s = Table {
        fwd: Holds,
        rev: Fails,
        a,
        b,
    };
    assert_eq!(
        GraphEvaluator::new()
            .eval(&mut g, claim, &s, 10_000)
            .evidential(),
        Evidential::Supported,
        "a missing mirror row is not evidence against"
    );
}

/// The reserved `instants` domain read an empty instant set as an empty
/// *complete* domain, so a structure with no temporal index refuted an
/// existential — absence read as refutation through a newly-interpreted domain.
#[test]
fn a_timeless_structure_decides_nothing_temporal() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let claim = g.apply(p, vec![a]);
    let t = g.fresh();
    let body = g.apply(wk::AT, vec![t, claim]);

    for binder in [wk::FORALL, wk::EXISTS] {
        let q = g.quantify(binder, t, Some(wk::INSTANTS_DOMAIN), body);
        let r = GraphEvaluator::new().eval(&mut g, q, &EmptyStructure, 20_000);
        assert!(
            !r.is_definite(),
            "no instants means no answer, not a vacuous one — {binder:?} said {:?}",
            r.evidential()
        );
    }
}

// ------------------------------------------------- round four, expressiveness

/// **Term denotation must survive a binder.** The six denotation readers still
/// did the root-only `env.get(id).unwrap_or(id)` lookup that round three
/// removed from `key`, so the whole `value` channel — added so
/// `(= (duration test-suite) (quantity 4 minutes))` would be answerable — went
/// dark inside quantifiers, aggregates and rules, which is exactly where a
/// memory generalises.
#[test]
fn a_denotation_survives_a_quantifier() {
    let mut g = ObjectGraph::new();
    let (duration, ta, tb) = (g.atom("duration"), g.atom("t-a"), g.atom("t-b"));
    let (da, db) = (g.apply(duration, vec![ta]), g.apply(duration, vec![tb]));
    let (three, nine, twelve, ten, eight) = (g.int(3), g.int(9), g.int(12), g.int(10), g.int(8));
    let dom = g.apply(wk::SET_DOMAIN, vec![ta, tb]);
    let s = MapGraphStructure::new()
        .denotes(da, three)
        .denotes(db, nine);
    let ev = GraphEvaluator::new();

    // ∀t. duration(t) ≤ 10
    let v = g.fresh();
    let dv = g.apply(duration, vec![v]);
    let body = g.apply(wk::LEQ, vec![dv, ten]);
    let q = g.quantify(wk::FORALL, v, Some(dom), body);
    assert_eq!(
        ev.eval(&mut g, q, &s, 100_000).evidential(),
        Evidential::Supported
    );

    // ∃t. 8 ≤ duration(t)
    let body2 = g.apply(wk::LEQ, vec![eight, dv]);
    let q2 = g.quantify(wk::EXISTS, v, Some(dom), body2);
    assert_eq!(
        ev.eval(&mut g, q2, &s, 100_000).evidential(),
        Evidential::Supported
    );

    // Σ duration(t) = 12
    let total = g.bind(
        wk::SUM,
        vec![Binding {
            var: v,
            domain: Some(dom),
        }],
        vec![wk::TOP, dv],
    );
    let claim = g.apply(wk::EQ, vec![total, twelve]);
    assert_eq!(
        ev.eval(&mut g, claim, &s, 100_000).evidential(),
        Evidential::Supported
    );
}

/// …and a rule whose antecedent reads a denoted value fires.
#[test]
fn a_rule_can_read_a_denoted_value() {
    let mut g = ObjectGraph::new();
    let (duration, long, t) = (g.atom("duration"), g.atom("long-running"), g.atom("t-a"));
    let dt = g.apply(duration, vec![t]);
    let (nine, five) = (g.int(9), g.int(5));

    let x = g.fresh();
    let dx = g.apply(duration, vec![x]);
    let ante = g.apply(wk::LEQ, vec![five, dx]);
    let cons = g.apply(long, vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![ante, cons]);
    let rule = g.quantify(wk::FORALL, x, None, imp);

    let s = MapGraphStructure::new().denotes(dt, nine).rule(long, rule);
    let goal = g.apply(long, vec![t]);
    assert_eq!(
        GraphEvaluator::new()
            .eval(&mut g, goal, &s, 100_000)
            .evidential(),
        Evidential::Supported
    );
}

/// **Text operators must see denotations too.** `=` routed through `denote` and
/// saw a denoted string; the six text operators fell through nothing and did
/// not.
#[test]
fn text_operators_read_a_denoted_string() {
    use artist_logic::object::LiteralValue;
    let mut g = ObjectGraph::new();
    let (path, f) = (g.atom("path"), g.atom("f1"));
    let term = g.apply(path, vec![f]);
    let lit = g.lit(LiteralValue::Text("src/lib.rs".into()));
    let s = MapGraphStructure::new().denotes(term, lit);
    let ev = GraphEvaluator::new();

    let prefix = g.lit(LiteralValue::Text("src/".into()));
    for op in [wk::STARTS_WITH, wk::CONTAINS] {
        let claim = g.apply(op, vec![term, prefix]);
        assert_eq!(
            ev.eval(&mut g, claim, &s, 20_000).evidential(),
            Evidential::Supported,
            "{op:?} could not see a denoted path"
        );
    }
    let n = g.apply(wk::LEN, vec![term]);
    let ten = g.int(10);
    let claim = g.apply(wk::EQ, vec![n, ten]);
    assert_eq!(
        ev.eval(&mut g, claim, &s, 20_000).evidential(),
        Evidential::Supported
    );
}

/// **A rule may conclude a reserved relation.** `derive` was called from one
/// site — the user-predicate arm — so a conditional norm, a derived type fact,
/// derived coreference and a derived preference were storable, indexable,
/// satisfiable and inert.
#[test]
fn a_rule_can_conclude_a_reserved_relation() {
    let mut g = ObjectGraph::new();
    let (on_ci, uses, flag, job) = (
        g.atom("on-ci"),
        g.atom("uses"),
        g.atom("release-flag"),
        g.atom("job-7"),
    );

    // ∀x. on-ci(x) → forbidden(uses(x, --release))
    let x = g.fresh();
    let ante = g.apply(on_ci, vec![x]);
    let act = g.apply(uses, vec![x, flag]);
    let cons = g.apply(wk::FORBIDDEN, vec![act]);
    let imp = g.apply(wk::IMPLIES, vec![ante, cons]);
    let rule = g.quantify(wk::FORALL, x, None, imp);

    let s = MapGraphStructure::new()
        .fact(on_ci, vec![job])
        .rule(wk::FORBIDDEN, rule);
    let concrete = g.apply(uses, vec![job, flag]);
    let goal = g.apply(wk::FORBIDDEN, vec![concrete]);
    assert_eq!(
        GraphEvaluator::new()
            .eval(&mut g, goal, &s, 100_000)
            .evidential(),
        Evidential::Supported,
        "a norm that can only ever be a ground term cannot govern a class"
    );

    // …and a derived type fact.
    let mut h = ObjectGraph::new();
    let (named_id, u64_ty, field) = (h.atom("named-id"), h.atom("u64"), h.atom("Chunk::len"));
    let y = h.fresh();
    let a2 = h.apply(named_id, vec![y]);
    let c2 = h.apply(wk::TYPE_OF, vec![y, u64_ty]);
    let i2 = h.apply(wk::IMPLIES, vec![a2, c2]);
    let r2 = h.quantify(wk::FORALL, y, None, i2);
    let s2 = MapGraphStructure::new()
        .fact(named_id, vec![field])
        .rule(wk::TYPE_OF, r2);
    let goal2 = h.apply(wk::TYPE_OF, vec![field, u64_ty]);
    assert_eq!(
        GraphEvaluator::new()
            .eval(&mut h, goal2, &s2, 100_000)
            .evidential(),
        Evidential::Supported
    );
}

/// §4 states that forbidding **is** obliging a negation. An identity that holds
/// in one direction only is not one.
#[test]
fn forbidding_and_obliging_a_negation_are_the_same_claim() {
    let mut g = ObjectGraph::new();
    let (uses, job, flag) = (g.atom("uses"), g.atom("job"), g.atom("release"));
    let act = g.apply(uses, vec![job, flag]);
    let negated = g.apply(wk::NOT, vec![act]);
    let ev = GraphEvaluator::new();

    let from_obliged = MapGraphStructure::new().fact(wk::OBLIGED, vec![negated]);
    let forbidden = g.apply(wk::FORBIDDEN, vec![act]);
    assert_eq!(
        ev.eval(&mut g, forbidden, &from_obliged, 20_000)
            .evidential(),
        Evidential::Supported
    );

    let from_forbidden = MapGraphStructure::new().fact(wk::FORBIDDEN, vec![act]);
    let obliged_not = g.apply(wk::OBLIGED, vec![negated]);
    assert_eq!(
        ev.eval(&mut g, obliged_not, &from_forbidden, 20_000)
            .evidential(),
        Evidential::Supported,
        "the identity must read left-to-right as well"
    );

    // …and forbidding still does not permit.
    let permitted = g.apply(wk::PERMITTED, vec![act]);
    assert_eq!(
        ev.eval(&mut g, permitted, &from_forbidden, 20_000)
            .evidential(),
        Evidential::Open
    );
}
