//! Round-three findings: the expressiveness audit's, and Sol's.
//!
//! The shape of this round is different from the last two. Those were verdicts
//! fabricated from missing information. These are mostly the opposite failure —
//! machinery that existed and did not *reach* far enough: substitution that
//! stopped at the argument boundary, denotations wired into three readers and
//! not the fourth, an instant index the reserved relations never consulted.
//! Each looks like an absence and is really a gap in coverage.

use artist_logic::evidence::Evidential;
use artist_logic::graph_eval::{
    EmptyStructure, GraphEvaluator, GraphStructure, Knowledge, MapGraphStructure,
};
use artist_logic::object::wk;
use artist_logic::{ObjectGraph, ObjectId};

fn ev(g: &mut ObjectGraph, root: ObjectId, s: &dyn GraphStructure) -> Evidential {
    GraphEvaluator::new().eval(g, root, s, 20_000).evidential()
}

/// Substitution reached only the root of an argument, so a compound argument
/// worked while ground and stopped the moment a quantifier generalised it —
/// which is what quantifiers, rules and aggregates are *for*.
#[test]
fn a_quantified_variable_reaches_inside_a_compound_argument() {
    let mut g = ObjectGraph::new();
    let (causes, omits) = (g.atom("causes"), g.atom("omits"));
    let (header, cstdint, effect) =
        (g.atom("rocksdb-slice.h"), g.atom("cstdint"), g.atom("build-fails"));
    let cause = g.apply(omits, vec![header, cstdint]);
    let s = MapGraphStructure::new().fact(causes, vec![cause, effect]);

    let v = g.fresh();
    let open = g.apply(omits, vec![header, v]);
    let body = g.apply(causes, vec![open, effect]);
    let dom = g.apply(wk::SET_DOMAIN, vec![cstdint]);
    let q = g.quantify(wk::EXISTS, v, Some(dom), body);
    assert_eq!(ev(&mut g, q, &s), Evidential::Supported);
}

/// The same gap under a norm and under a default — the two shapes those
/// operators exist to serve, both storable, indexable and previously inert.
#[test]
fn norms_and_defaults_can_be_quantified() {
    let mut g = ObjectGraph::new();
    let reviewed = g.atom("reviewed");
    let (c1, c2) = (g.atom("commit-1"), g.atom("commit-2"));
    let (r1, r2) = (g.apply(reviewed, vec![c1]), g.apply(reviewed, vec![c2]));
    let s = MapGraphStructure::new()
        .fact(wk::OBLIGED, vec![r1])
        .fact(wk::OBLIGED, vec![r2]);

    let v = g.fresh();
    let body = g.apply(reviewed, vec![v]);
    let norm = g.apply(wk::OBLIGED, vec![body]);
    let dom = g.apply(wk::SET_DOMAIN, vec![c1, c2]);
    let q = g.quantify(wk::FORALL, v, Some(dom), norm);
    assert_eq!(ev(&mut g, q, &s), Evidential::Supported);
}

/// …and inside a quotation, which §7's barrier never meant to block: binding a
/// variable is not rewriting one name into another.
#[test]
fn a_quantifier_reaches_into_an_intensional_context() {
    let mut g = ObjectGraph::new();
    let (believes, adam, tested) =
        (g.atom("believes"), g.atom("adam"), g.atom("tested"));
    let f = g.atom("file-1");
    let inner = g.apply(tested, vec![f]);
    let quoted = g.apply(wk::QUOTE, vec![inner]);
    let s = MapGraphStructure::new().fact(believes, vec![adam, quoted]);

    let v = g.fresh();
    let open = g.apply(tested, vec![v]);
    let oq = g.apply(wk::QUOTE, vec![open]);
    let body = g.apply(believes, vec![adam, oq]);
    let dom = g.apply(wk::SET_DOMAIN, vec![f]);
    let q = g.quantify(wk::EXISTS, v, Some(dom), body);
    assert_eq!(ev(&mut g, q, &s), Evidential::Supported);
}

/// A denotation must be indexed by the same axes as a fact. Answering a dated
/// question with today's value produced `Refuted/Exact` for a true claim.
#[test]
fn a_denotation_is_not_answered_out_of_its_time() {
    struct Present {
        term: ObjectId,
        now: ObjectId,
    }
    impl GraphStructure for Present {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn value(&self, term: ObjectId) -> Option<ObjectId> {
            (term == self.term).then_some(self.now)
        }
    }

    let mut g = ObjectGraph::new();
    let (lines, file) = (g.atom("line-count"), g.atom("src/lib.rs"));
    let term = g.apply(lines, vec![file]);
    let now = g.int(120);
    let then = g.int(40);
    let s = Present { term, now };

    let untensed = g.apply(wk::EQ, vec![term, now]);
    assert_eq!(ev(&mut g, untensed, &s), Evidential::Supported, "the present is known");

    let one = g.int(1);
    let past_true = g.apply(wk::EQ, vec![term, then]);
    let dated = g.apply(wk::AT, vec![one, past_true]);
    assert_eq!(
        ev(&mut g, dated, &s),
        Evidential::Open,
        "a structure with no history must not answer a dated question at all"
    );
    let dated_now = g.apply(wk::AT, vec![one, untensed]);
    assert_eq!(
        ev(&mut g, dated_now, &s),
        Evidential::Open,
        "…and certainly must not answer it with today's value"
    );
}

/// The reserved relations discarded the instant and the world, so no
/// preference, norm, default or coreference could be dated or made local.
#[test]
fn reserved_relations_are_indexed_by_time() {
    struct Dated {
        prefer_after: i64,
    }
    impl GraphStructure for Dated {
        fn known(&self, _p: ObjectId, _a: &[ObjectId]) -> Knowledge {
            Knowledge::Unknown
        }
        fn known_at(&self, pred: ObjectId, _a: &[ObjectId], t: i64) -> Knowledge {
            if pred == wk::PREFER && t >= self.prefer_after {
                Knowledge::Holds
            } else {
                Knowledge::Unknown
            }
        }
    }
    let mut g = ObjectGraph::new();
    let (tabs, spaces) = (g.atom("tabs"), g.atom("spaces"));
    let claim = g.apply(wk::PREFER, vec![tabs, spaces]);
    let s = Dated { prefer_after: 5 };

    let late = g.int(9);
    let after = g.apply(wk::AT, vec![late, claim]);
    assert_eq!(ev(&mut g, after, &s), Evidential::Supported);

    let early = g.int(1);
    let before = g.apply(wk::AT, vec![early, claim]);
    assert_eq!(ev(&mut g, before, &s), Evidential::Open);
}

/// One id is one thing, however uncomputable. `denote` refused every compound
/// before ever comparing, so a node was not equal to itself.
#[test]
fn equality_is_reflexive_on_a_compound() {
    let mut g = ObjectGraph::new();
    let (build, artist) = (g.atom("build"), g.atom("artist"));
    let term = g.apply(build, vec![artist]);
    let claim = g.apply(wk::EQ, vec![term, term]);
    assert_eq!(ev(&mut g, claim, &EmptyStructure), Evidential::Supported);
}

/// A term could denote a number, a duration or a sequence — never an entity.
#[test]
fn a_term_can_denote_an_entity() {
    let mut g = ObjectGraph::new();
    let (author, commit, adam) =
        (g.atom("author"), g.atom("commit-9f2"), g.atom("adam"));
    let term = g.apply(author, vec![commit]);
    let s = MapGraphStructure::new().denotes(term, adam);
    let claim = g.apply(wk::EQ, vec![term, adam]);
    assert_eq!(ev(&mut g, claim, &s), Evidential::Supported);
}

/// Two defaults pointing opposite ways is the "look here" signal, not weak
/// support for whichever was consulted first.
#[test]
fn competing_defaults_are_conflicted() {
    let mut g = ObjectGraph::new();
    let (needs, f) = (g.atom("needs-review"), g.atom("f"));
    let claim = g.apply(needs, vec![f]);
    let negated = g.apply(wk::NOT, vec![claim]);
    let s = MapGraphStructure::new()
        .fact(wk::USUALLY, vec![claim])
        .fact(wk::USUALLY, vec![negated]);

    let hedged = g.apply(wk::USUALLY, vec![claim]);
    let r = GraphEvaluator::new().eval(&mut g, hedged, &s, 20_000);
    assert_eq!(r.evidential(), Evidential::Conflicted);
    assert!(!r.is_definite(), "a contradiction is not a definite reading");
}

/// Quantities could be compared and never combined, so no total over durations,
/// sizes or costs was reachable.
#[test]
fn quantities_combine_and_not_only_compare() {
    let mut g = ObjectGraph::new();
    let (minutes, seconds) = (g.atom("minutes"), g.atom("seconds"));
    let sixty = g.int(60);
    let s = MapGraphStructure::new().scale_fact(minutes, sixty, 60, seconds);

    let (four, thirty, total) = (g.int(4), g.int(30), g.int(270));
    let a = g.apply(wk::QUANTITY, vec![four, minutes]);
    let b = g.apply(wk::QUANTITY, vec![thirty, seconds]);
    let sum = g.apply(wk::ADD, vec![a, b]);
    let expected = g.apply(wk::QUANTITY, vec![total, seconds]);
    let claim = g.apply(wk::EQ, vec![sum, expected]);
    assert_eq!(ev(&mut g, claim, &s), Evidential::Supported);
}

/// A cyclic term walked the linear-form reader until the stack gave out. A
/// crash is not one of the four compute statuses.
#[test]
fn a_cyclic_arithmetic_term_does_not_overflow_the_stack() {
    let mut g = ObjectGraph::new();
    let loop_id = g.alloc();
    let one = g.int(1);
    let body = g.apply(wk::ADD, vec![one, loop_id]);
    let node = g.get(body).cloned().expect("built");
    g.define(loop_id, node);

    let v = g.fresh();
    let cmp = g.apply(wk::LEQ, vec![v, loop_id]);
    let q = g.quantify(wk::FORALL, v, Some(wk::NAT_TYPE), cmp);
    let r = GraphEvaluator::new().eval(&mut g, q, &EmptyStructure, 5_000);
    assert!(!r.is_definite(), "no verdict is available for a cyclic bound");
}

// --------------------------------------------------- evaluator audit, round 3

/// `finish_scan` read `(Partial, Certain)` as one-sided and republished it as
/// `(None, Certain)` — the round-two conflict-flattening bug, reachable again
/// as soon as one operand is a default instead of a truth atom.
#[test]
fn a_defeasible_member_does_not_become_a_definite_refutation() {
    struct Split {
        p: ObjectId,
        conflicted: ObjectId,
        defaulted: ObjectId,
    }
    impl GraphStructure for Split {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred == self.p && args == [self.conflicted] {
                return Knowledge::Conflicted;
            }
            if pred == wk::USUALLY {
                // A default on record for the *other* member's claim.
                return if args.len() == 1 { Knowledge::Holds } else { Knowledge::Unknown };
            }
            Knowledge::Unknown
        }
    }

    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let (m0, m1) = (g.atom("m0"), g.atom("m1"));
    let s = Split { p, conflicted: m0, defaulted: m1 };
    let _ = s.defaulted;

    let v = g.fresh();
    let bare = g.apply(p, vec![v]);
    let body = g.apply(wk::USUALLY, vec![bare]);
    let dom = g.apply(wk::SET_DOMAIN, vec![m0, m1]);
    let q = g.quantify(wk::FORALL, v, Some(dom), body);

    let r = GraphEvaluator::new().eval(&mut g, q, &s, 50_000);
    assert!(
        !r.is_definite(),
        "a scan mixing a contradiction with a default has no definite reading, got {:?}/{:?}",
        r.support,
        r.refutation
    );
}

/// `usually` tested refutation before support, so a contested claim came back a
/// definite `Refuted` — the operator picking whichever half it looked at first.
#[test]
fn usually_over_a_contested_claim_stays_contested() {
    struct Contested(ObjectId);
    impl GraphStructure for Contested {
        fn known(&self, pred: ObjectId, _a: &[ObjectId]) -> Knowledge {
            if pred == self.0 { Knowledge::Conflicted } else { Knowledge::Unknown }
        }
    }
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let claim = g.apply(p, vec![a]);
    let hedged = g.apply(wk::USUALLY, vec![claim]);
    let r = GraphEvaluator::new().eval(&mut g, hedged, &Contested(p), 20_000);
    assert_eq!(r.evidential(), Evidential::Conflicted);
    assert!(!r.is_definite());
}

/// `count` contributions are always `+1`, so what was seen is a floor. A `sum`
/// contribution can be negative, so over a partial enumeration it is not.
#[test]
fn a_partial_sum_has_no_lower_bound_either() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let s = MapGraphStructure::new().fact(p, vec![a]).denotes(a, g.int(5));

    let v = g.fresh();
    let body = g.apply(p, vec![v]);
    let dom = g.apply(wk::SET_PARTIAL, vec![a]);
    let total = g.bind(
        wk::SUM,
        vec![artist_logic::object::Binding { var: v, domain: Some(dom) }],
        vec![body, v],
    );
    let zero = g.int(0);
    let claim = g.apply(wk::LEQ, vec![zero, total]);
    assert_ne!(
        ev(&mut g, claim, &s),
        Evidential::Supported,
        "an unseen member of a `sum` can contribute negatively"
    );

    // …while `count` keeps its documented early decision.
    let counted = g.bind(
        wk::COUNT,
        vec![artist_logic::object::Binding { var: v, domain: Some(dom) }],
        vec![body],
    );
    let one = g.int(1);
    let at_least_one = g.apply(wk::LEQ, vec![one, counted]);
    assert_eq!(ev(&mut g, at_least_one, &s), Evidential::Supported);
}

/// The operand order decided whether a cyclic term aborted the process, because
/// `collect::<Option<_>>` short-circuits on the first `None`. The earlier test
/// used the order that was already safe.
#[test]
fn cyclic_terms_do_not_overflow_in_either_operand_order() {
    for leading_cycle in [true, false] {
        let mut g = ObjectGraph::new();
        let t = g.alloc();
        let one = g.int(1);
        let body = if leading_cycle {
            g.apply(wk::ADD, vec![t, one])
        } else {
            g.apply(wk::ADD, vec![one, t])
        };
        let node = g.get(body).cloned().expect("built");
        g.define(t, node);

        let sym = g.atom("sym");
        for claim in [
            g.apply(wk::EQ, vec![t, one]),
            g.apply(wk::LEQ, vec![t, one]),
            g.apply(wk::BEFORE, vec![t, sym]),
        ] {
            let r = GraphEvaluator::new().eval(&mut g, claim, &EmptyStructure, 5_000);
            assert!(!r.is_definite(), "cyclic arithmetic has no verdict");
        }
    }

    // `S = (seq N)`, `N = (nth S 0)` — the project/denote mutual recursion.
    let mut g = ObjectGraph::new();
    let n = g.alloc();
    let seq = g.apply(wk::SEQ, vec![n]);
    let zero = g.int(0);
    let proj = g.apply(wk::NTH, vec![seq, zero]);
    let node = g.get(proj).cloned().expect("built");
    g.define(n, node);
    let one = g.int(1);
    let claim = g.apply(wk::EQ, vec![n, one]);
    let r = GraphEvaluator::new().eval(&mut g, claim, &EmptyStructure, 5_000);
    assert!(!r.is_definite());

    // `D = (where D f)` — the domain reader recursing on its own base.
    let mut g = ObjectGraph::new();
    let d = g.alloc();
    let p = g.atom("p");
    let v = g.fresh();
    let filter = g.apply(p, vec![v]);
    let lam = g.bind(wk::LAMBDA, vec![artist_logic::object::Binding { var: v, domain: None }], vec![filter]);
    let w = g.apply(wk::WHERE_DOMAIN, vec![d, lam]);
    let node = g.get(w).cloned().expect("built");
    g.define(d, node);
    let y = g.fresh();
    let body = g.apply(p, vec![y]);
    let q = g.quantify(wk::FORALL, y, Some(d), body);
    let r = GraphEvaluator::new().eval(&mut g, q, &EmptyStructure, 5_000);
    assert!(!r.is_definite());
}

/// An authoritative *absence* on the mirror row is not evidence against an
/// identity the store affirms — symmetry is one question, not two sources.
#[test]
fn a_missing_mirror_row_does_not_manufacture_a_conflicted_identity() {
    struct OneWay {
        a: ObjectId,
        b: ObjectId,
    }
    impl GraphStructure for OneWay {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred != wk::SAME_AS {
                return Knowledge::Unknown;
            }
            if args == [self.a, self.b] { Knowledge::Holds } else { Knowledge::Fails }
        }
    }
    let mut g = ObjectGraph::new();
    let (a, b) = (g.atom("a"), g.atom("b"));
    let claim = g.apply(wk::SAME_AS, vec![a, b]);
    assert_eq!(ev(&mut g, claim, &OneWay { a, b }), Evidential::Supported);
}

/// `¬usually(P)` says the default does not hold; `usually(¬P)` is evidence
/// against `P`. Treating the wrappers as commutative made them identical.
#[test]
fn a_negated_default_is_not_a_default_negation() {
    let mut g = ObjectGraph::new();
    let (made, needs) = (g.atom("generated"), g.atom("needs-review"));
    let file = g.atom("f");
    let x = g.fresh();
    let antecedent = g.apply(made, vec![x]);
    let bare = g.apply(needs, vec![x]);

    // ∀x. generated(x) → ¬usually(needs-review x)
    let hedged = g.apply(wk::USUALLY, vec![bare]);
    let outer_not = g.apply(wk::NOT, vec![hedged]);
    let imp = g.apply(wk::IMPLIES, vec![antecedent, outer_not]);
    let rule = g.quantify(wk::FORALL, x, None, imp);
    let s = MapGraphStructure::new().fact(made, vec![file]).rule(needs, rule);

    let goal = g.apply(needs, vec![file]);
    assert_eq!(
        ev(&mut g, goal, &s),
        Evidential::Open,
        "denying that a default applies is not evidence against the claim"
    );
}

// Sol's list: the axes that were smuggled onto the evidence lattice.
/// The liar has no sources and no claims. Reporting it `Conflicted` told a
/// caller to go read evidence that does not exist.
#[test]
fn ungroundedness_is_not_contradictory_evidence() {
    use artist_logic::evidence::Grounding;
    let mut g = ObjectGraph::new();
    let liar = g.alloc();
    let q = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q]);
    let n = g.apply(wk::NOT, vec![h]);
    let node = g.get(n).cloned().expect("built");
    g.define(liar, node);

    let r = GraphEvaluator::new().eval(&mut g, liar, &EmptyStructure, 50_000);
    assert_eq!(r.grounding, Grounding::Oscillatory);
    assert_eq!(r.evidential(), Evidential::Open);

    // …while a genuine disagreement in the store still is `Conflicted`.
    struct Both(ObjectId);
    impl GraphStructure for Both {
        fn known(&self, pred: ObjectId, _a: &[ObjectId]) -> Knowledge {
            if pred == self.0 { Knowledge::Conflicted } else { Knowledge::Unknown }
        }
    }
    let mut h2 = ObjectGraph::new();
    let p = h2.atom("p");
    let a = h2.atom("a");
    let claim = h2.apply(p, vec![a]);
    let r2 = GraphEvaluator::new().eval(&mut h2, claim, &Both(p), 10_000);
    assert_eq!(r2.evidential(), Evidential::Conflicted);
    assert_eq!(r2.grounding, Grounding::Grounded, "a contradiction is perfectly grounded");
}

/// A default is certain that the default applies, and defeasible because a
/// defeater would overturn it. Those are two axes, not one.
#[test]
fn defeasibility_is_not_weakness() {
    use artist_logic::evidence::{Bound, Derivation};
    let mut g = ObjectGraph::new();
    let (fast, build) = (g.atom("fast"), g.atom("build"));
    let claim = g.apply(fast, vec![build]);
    let hedged = g.apply(wk::USUALLY, vec![claim]);
    let s = MapGraphStructure::new().fact(wk::USUALLY, vec![claim]);

    let r = GraphEvaluator::new().eval(&mut g, hedged, &s, 20_000);
    assert_eq!(r.support, Bound::Certain, "the default certainly applies");
    assert_eq!(r.derivation, Derivation::Default, "…and is certainly defeasible");
    assert_eq!(r.defeated_by, vec![claim], "and it names what would defeat it");
    assert!(!r.is_definite());

    // The axis survives every composition, including a short-circuit.
    for wrap in [wk::OR, wk::AND, wk::POSSIBLY, wk::NECESSARILY] {
        let node = g.apply(wrap, vec![hedged]);
        let s2 = MapGraphStructure::new()
            .fact(wk::USUALLY, vec![claim])
            .world(g.atom("w"));
        let out = GraphEvaluator::new().eval(&mut g, node, &s2, 20_000);
        assert_eq!(out.derivation, Derivation::Default, "{wrap:?} dropped the defeasibility");
        assert!(!out.is_definite(), "{wrap:?} settled a default");
    }
}

/// A reserved name that returned `Unsupported` and never reached the store made
/// the obvious spelling for a type fact permanently unretrievable.
#[test]
fn type_of_is_an_ordinary_relation() {
    let mut g = ObjectGraph::new();
    let (field, usize_ty) = (g.atom("Chunk::len"), g.atom("usize"));
    let claim = g.apply(wk::TYPE_OF, vec![field, usize_ty]);
    let s = MapGraphStructure::new().fact(wk::TYPE_OF, vec![field, usize_ty]);
    assert_eq!(ev(&mut g, claim, &s), Evidential::Supported);
    assert_eq!(ev(&mut g, claim, &EmptyStructure), Evidential::Open);
}
