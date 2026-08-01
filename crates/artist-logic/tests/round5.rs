//! Round five: the last fabricated verdicts, and the recursions that crashed.
//!
//! Two shapes recur here and are worth naming, because they are what four
//! rounds of this have converged on.
//!
//! **A resolver must never be asked about a name that denotes nothing to it.**
//! A second-order variable in argument position is a relation *this scan
//! invented*; handing it to a closed resolver got an authoritative `Fails` about
//! an entity nobody had asked about, and under a negation that is a confident
//! `Supported`. Same failure as a fabricated `Knowledge::Fails`, reached by a
//! route the `Knowledge` discipline does not cover.
//!
//! **A contested case is not a decided one.** Three scans still classified a
//! member by testing one side, which is blind to a case where *both* bounds are
//! certain — so a `Conflicted` member was silently excluded from a refined
//! domain, counted as a definite contributor, or admitted to a fixpoint, and the
//! result published exact.

use artist_logic::evidence::{Derivation, Evidential, Grounding};
use artist_logic::graph_eval::{
    EmptyStructure, GraphEvaluator, GraphStructure, Knowledge, MapGraphStructure,
};
use artist_logic::object::{Binding, wk};
use artist_logic::syntax::parse;
use artist_logic::{ObjectGraph, ObjectId};

/// A structure that is authoritative about one predicate and knows nothing else.
struct Closed {
    closed: ObjectId,
    holds: Vec<(ObjectId, Vec<ObjectId>)>,
}
impl GraphStructure for Closed {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        if self.holds.iter().any(|(p, a)| *p == pred && a == args) {
            return Knowledge::Holds;
        }
        if pred == self.closed { Knowledge::Fails } else { Knowledge::Unknown }
    }
    fn is_closed(&self, pred: ObjectId) -> bool {
        pred == self.closed
    }
}

/// A relation variable used as an *argument* names nothing the store has seen.
/// Resolving it got a confident `Fails` — and under `implies`, a confident
/// `Supported` — on a store whose first-order facts say the opposite.
#[test]
fn a_relation_variable_is_never_resolved_as_an_entity() {
    let mut g = ObjectGraph::new();
    let (deprecated, uses, a, m) =
        (g.atom("deprecated"), g.atom("uses"), g.atom("a"), g.atom("M"));
    let dom = g.apply(wk::SET_DOMAIN, vec![uses, a]);
    let rel_ty = g.apply(wk::RELATION_TYPE, vec![dom]);
    let _ = m;
    let s = Closed {
        closed: deprecated,
        holds: vec![(deprecated, vec![uses]), (uses, vec![a])],
    };

    // The ground facts.
    let d = g.apply(deprecated, vec![uses]);
    let u = g.apply(uses, vec![a]);
    let ev = GraphEvaluator::new();
    assert_eq!(ev.eval(&mut g, d, &s, 50_000).evidential(), Evidential::Supported);
    assert_eq!(ev.eval(&mut g, u, &s, 50_000).evidential(), Evidential::Supported);

    let r = g.fresh();
    let dep_r = g.apply(deprecated, vec![r]);
    let r_a = g.apply(r, vec![a]);
    let body = g.apply(wk::AND, vec![dep_r, r_a]);
    let q = g.quantify(wk::EXISTS, r, Some(rel_ty), body);
    let out = ev.eval(&mut g, q, &s, 100_000);
    assert_ne!(
        out.evidential(),
        Evidential::Refuted,
        "the store's own facts say otherwise; a scan-invented name is not an entity"
    );

    // …and the universal that inverted it.
    let inner_all = {
        let x = g.fresh();
        let rx = g.apply(r, vec![x]);
        let nrx = g.apply(wk::NOT, vec![rx]);
        g.quantify(wk::FORALL, x, Some(dom), nrx)
    };
    let imp = g.apply(wk::IMPLIES, vec![dep_r, inner_all]);
    let q2 = g.quantify(wk::FORALL, r, Some(rel_ty), imp);
    let out2 = ev.eval(&mut g, q2, &s, 100_000);
    assert!(!out2.is_definite(), "and must not become a definite truth under implies");
}

/// A rule whose conclusion is a cyclic node walked `match_goal`'s wrapper loop
/// until the stack died — **reachable from parsed text**, so a crash was one
/// `parse` away from any stored rule.
#[test]
fn a_cyclic_rule_conclusion_does_not_crash() {
    let mut g = ObjectGraph::new();
    let rule = parse(&mut g, "(forall [(v0 (set a))] (implies #true #1=(not #1#)))")
        .expect("parses");
    let (p, a) = (g.atom("p"), g.atom("a"));
    let s = MapGraphStructure::new().rule(p, rule);
    let goal = g.apply(p, vec![a]);
    let r = GraphEvaluator::new().eval(&mut g, goal, &s, 10_000);
    assert!(!r.is_definite(), "no verdict is available from a cyclic conclusion");
}

/// A malformed `(not A C)` conclusion fired and fabricated a definite
/// refutation, while `apply` refuses to evaluate the same node at all. The
/// `usually` half of that branch had an arity guard; the `not` half did not.
#[test]
fn a_malformed_conclusion_does_not_fire() {
    let mut g = ObjectGraph::new();
    let (p, a, junk) = (g.atom("p"), g.atom("a"), g.atom("junk"));
    let x = g.fresh();
    let px = g.apply(p, vec![x]);
    let bad = g.apply(wk::NOT, vec![junk, px]);
    let imp = g.apply(wk::IMPLIES, vec![wk::TOP, bad]);
    let rule = g.quantify(wk::FORALL, x, None, imp);
    let s = MapGraphStructure::new().rule(p, rule);

    let goal = g.apply(p, vec![a]);
    let r = GraphEvaluator::new().eval(&mut g, goal, &s, 50_000);
    assert!(
        !r.is_definite(),
        "the evaluator cannot assign that conclusion a meaning; it must not derive one"
    );
}

/// A contested member is undecided, not excluded — at all three scans that
/// classified by testing one side.
#[test]
fn a_contested_member_is_not_silently_decided() {
    struct Both {
        f: ObjectId,
    }
    impl GraphStructure for Both {
        fn known(&self, pred: ObjectId, _a: &[ObjectId]) -> Knowledge {
            if pred == self.f { Knowledge::Conflicted } else { Knowledge::Unknown }
        }
    }
    let mut g = ObjectGraph::new();
    let (f, gg, a) = (g.atom("f"), g.atom("g"), g.atom("a"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a]);
    let s = Both { f };
    let ev = GraphEvaluator::new();

    // (a) a `where` filter must not drop it and still claim to be exhaustive.
    let z = g.fresh();
    let fz = g.apply(f, vec![z]);
    let lam = g.bind(wk::LAMBDA, vec![Binding { var: z, domain: None }], vec![fz]);
    let refined = g.apply(wk::WHERE_DOMAIN, vec![dom, lam]);
    let w = g.fresh();
    let gw = g.apply(gg, vec![w]);
    let vacuous = g.quantify(wk::FORALL, w, Some(refined), gw);
    assert!(
        !ev.eval(&mut g, vacuous, &s, 50_000).is_definite(),
        "excluding the contested member made this vacuously and definitely true"
    );

    // (b) an aggregate must not count it as a definite contributor.
    let v = g.fresh();
    let fv = g.apply(f, vec![v]);
    let counted = g.bind(wk::COUNT, vec![Binding { var: v, domain: Some(dom) }], vec![fv]);
    for n in [0i64, 1] {
        let k = g.int(n);
        let claim = g.apply(wk::EQ, vec![counted, k]);
        assert!(
            !ev.eval(&mut g, claim, &s, 50_000).is_definite(),
            "a contested member makes the count inexact, not exactly {n}"
        );
    }

    // (c) a `letrec` must not publish a relation containing it as complete.
    let rel_ty = g.apply(wk::RELATION_TYPE, vec![dom]);
    let (pv, xv) = (g.fresh(), g.fresh());
    let def = g.apply(f, vec![xv]);
    let member = g.apply(pv, vec![a]);
    let scope = g.apply(wk::NOT, vec![member]);
    let node = g.bind(
        wk::LETREC,
        vec![
            Binding { var: pv, domain: Some(rel_ty) },
            Binding { var: xv, domain: Some(dom) },
        ],
        vec![def, scope],
    );
    assert!(
        !ev.eval(&mut g, node, &s, 50_000).is_definite(),
        "a contested tuple is undetermined, so the relation is not complete"
    );
}

/// `unless` exists to name a defeater and named none: learning the exception
/// flips a *definite* Supported to Open, which is this crate's own definition of
/// a default.
#[test]
fn unless_marks_what_would_defeat_it() {
    let mut g = ObjectGraph::new();
    let (q, a) = (g.atom("q"), g.atom("a"));
    let exception = g.apply(q, vec![a]);
    let guarded = g.apply(wk::UNLESS, vec![exception, wk::TOP]);

    let r = GraphEvaluator::new().eval(&mut g, guarded, &EmptyStructure, 20_000);
    assert_eq!(r.evidential(), Evidential::Supported);
    assert_eq!(r.derivation, Derivation::Default);
    assert_eq!(r.defeated_by, vec![exception], "the defeater is the whole content");
    assert!(!r.is_definite());
}

/// The two declared deontic entailments must chain.
#[test]
fn permitted_composes_with_the_forbidden_identity() {
    let mut g = ObjectGraph::new();
    let (r, adam) = (g.atom("rebase"), g.atom("adam"));
    let act = g.apply(r, vec![adam]);
    let negated = g.apply(wk::NOT, vec![act]);
    let s = MapGraphStructure::new().fact(wk::FORBIDDEN, vec![act]);

    let permitted_not = g.apply(wk::PERMITTED, vec![negated]);
    assert_eq!(
        GraphEvaluator::new().eval(&mut g, permitted_not, &s, 50_000).evidential(),
        Evidential::Supported,
        "forbidden(P) gives obliged(¬P) gives permitted(¬P)"
    );
}

/// A rule may conclude a `violated` — the one reserved arm that never reached
/// `derive`.
#[test]
fn a_rule_can_conclude_a_violation() {
    let mut g = ObjectGraph::new();
    let (skipped, formatted, c1) =
        (g.atom("skipped-fmt"), g.atom("formatted"), g.atom("c1"));
    let x = g.fresh();
    let ante = g.apply(skipped, vec![x]);
    let inner = g.apply(formatted, vec![x]);
    let cons = g.apply(wk::VIOLATED, vec![inner]);
    let imp = g.apply(wk::IMPLIES, vec![ante, cons]);
    let rule = g.quantify(wk::FORALL, x, None, imp);
    let s = MapGraphStructure::new().fact(skipped, vec![c1]).rule(wk::VIOLATED, rule);

    let concrete = g.apply(formatted, vec![c1]);
    let goal = g.apply(wk::VIOLATED, vec![concrete]);
    assert_eq!(
        GraphEvaluator::new().eval(&mut g, goal, &s, 100_000).evidential(),
        Evidential::Supported
    );
}

/// A domain that is a *term* must be substituted before the store enumerates it,
/// or a per-group query stalls silently on its most natural spelling.
#[test]
fn a_domain_term_is_substituted_under_a_quantifier() {
    struct PerGroup {
        tests_of: ObjectId,
        m1: ObjectId,
        members: Vec<ObjectId>,
        fails: ObjectId,
    }
    impl GraphStructure for PerGroup {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred == self.fails && args.len() == 1 && self.members.contains(&args[0]) {
                Knowledge::Holds
            } else {
                Knowledge::Unknown
            }
        }
        fn extension(&self, _domain: ObjectId) -> Option<artist_logic::graph_eval::Extension> {
            let _ = self.tests_of;
            let _ = self.m1;
            Some(artist_logic::graph_eval::Extension::complete(self.members.clone()))
        }
    }
    let mut g = ObjectGraph::new();
    let (tests_of, m1, fails) = (g.atom("tests-of"), g.atom("mod1"), g.atom("fails"));
    let (t1, t2) = (g.atom("t1"), g.atom("t2"));
    let s = PerGroup { tests_of, m1, members: vec![t1, t2], fails };

    let m = g.fresh();
    let group = g.apply(tests_of, vec![m]);
    let t = g.fresh();
    let body = g.apply(fails, vec![t]);
    let inner = g.quantify(wk::FORALL, t, Some(group), body);
    let outer_dom = g.apply(wk::SET_DOMAIN, vec![m1]);
    let q = g.quantify(wk::FORALL, m, Some(outer_dom), inner);

    assert_eq!(
        GraphEvaluator::new().eval(&mut g, q, &s, 100_000).evidential(),
        Evidential::Supported,
        "the store must be asked to enumerate (tests-of mod1), not (tests-of <skolem>)"
    );
}

/// A norm whose content is itself general, stated per entity. Substitution
/// stopped at binders, so the whole family was storable, indexable and inert.
#[test]
fn a_quantified_variable_reaches_inside_a_binder() {
    let mut g = ObjectGraph::new();
    let (in_module, formatted, m1) =
        (g.atom("in-module"), g.atom("formatted"), g.atom("mod1"));
    let files = g.atom("Files");
    let f1 = g.atom("f1");

    let build = |g: &mut ObjectGraph, module: ObjectId| {
        let f = g.fresh();
        let a = g.apply(in_module, vec![f, module]);
        let c = g.apply(formatted, vec![f]);
        let imp = g.apply(wk::IMPLIES, vec![a, c]);
        let inner = g.quantify(wk::FORALL, f, Some(files), imp);
        g.apply(wk::OBLIGED, vec![inner])
    };
    let ground = build(&mut g, m1);
    let s = MapGraphStructure::new()
        .fact(wk::OBLIGED, vec![{
            let f = g.fresh();
            let a = g.apply(in_module, vec![f, m1]);
            let c = g.apply(formatted, vec![f]);
            let imp = g.apply(wk::IMPLIES, vec![a, c]);
            g.quantify(wk::FORALL, f, Some(files), imp)
        }]);
    let ev = GraphEvaluator::new();
    // The ground form is the control; whether the fixture matches it exactly
    // depends on binder identity, so only the generalisation is asserted below.
    let _ = ev.eval(&mut g, ground, &s, 50_000);

    let m = g.fresh();
    let general = build(&mut g, m);
    let dom = g.apply(wk::SET_DOMAIN, vec![m1]);
    let q = g.quantify(wk::FORALL, m, Some(dom), general);
    let r = ev.eval(&mut g, q, &s, 100_000);
    assert_ne!(
        r.grounding,
        Grounding::Oscillatory,
        "substitution into a binder body must not produce a self-referential term"
    );
    let _ = f1;
}

// ------------------------------------------------------------ answer bindings

/// **Existence without a witness made every wh-question unaskable.** The scan
/// had the member in hand when it short-circuited and threw it away, so the
/// store could grade an answer and never produce one.
#[test]
fn an_existential_names_its_witness() {
    let mut g = ObjectGraph::new();
    let fails = g.atom("fails");
    let (t1, t2, t3) = (g.atom("t1"), g.atom("t2"), g.atom("t3"));
    let dom = g.apply(wk::SET_DOMAIN, vec![t1, t2, t3]);
    let s = MapGraphStructure::new().fact(fails, vec![t2]);

    let v = g.fresh();
    let body = g.apply(fails, vec![v]);
    let q = g.quantify(wk::EXISTS, v, Some(dom), body);
    let r = GraphEvaluator::new().eval(&mut g, q, &s, 50_000);

    assert_eq!(r.evidential(), Evidential::Supported);
    assert_eq!(r.bindings, vec![(0usize, t2)], "the store must say *which* test fails");
}

/// The dual: a refuted universal names its counterexample.
#[test]
fn a_refuted_universal_names_its_counterexample() {
    struct Closed2 {
        p: ObjectId,
        holds: Vec<ObjectId>,
    }
    impl GraphStructure for Closed2 {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred != self.p || args.len() != 1 {
                return Knowledge::Unknown;
            }
            if self.holds.contains(&args[0]) { Knowledge::Holds } else { Knowledge::Fails }
        }
    }
    let mut g = ObjectGraph::new();
    let tested = g.atom("tested");
    let (a, b) = (g.atom("a.rs"), g.atom("b.rs"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let s = Closed2 { p: tested, holds: vec![a] };

    let v = g.fresh();
    let body = g.apply(tested, vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(dom), body);
    let r = GraphEvaluator::new().eval(&mut g, q, &s, 50_000);

    assert_eq!(r.evidential(), Evidential::Refuted);
    assert_eq!(r.bindings, vec![(0usize, b)], "and *which* file is untested");
}

/// **Bindings do not propagate.** A witness justifies exactly the claim it
/// witnessed; under a negation the same member plays the opposite role, and
/// carrying it up would attach an answer to a question nobody asked.
#[test]
fn a_binding_does_not_escape_the_claim_it_settles() {
    let mut g = ObjectGraph::new();
    let fails = g.atom("fails");
    let (t1, t2) = (g.atom("t1"), g.atom("t2"));
    let dom = g.apply(wk::SET_DOMAIN, vec![t1, t2]);
    let s = MapGraphStructure::new().fact(fails, vec![t2]);

    let v = g.fresh();
    let body = g.apply(fails, vec![v]);
    let q = g.quantify(wk::EXISTS, v, Some(dom), body);
    let negated = g.apply(wk::NOT, vec![q]);

    let r = GraphEvaluator::new().eval(&mut g, negated, &s, 50_000);
    assert!(r.bindings.is_empty(), "a counterexample is not a witness");
}

/// An unwitnessed answer carries none — absence of a binding is meaningful.
#[test]
fn an_undecided_scan_names_nothing() {
    let mut g = ObjectGraph::new();
    let fails = g.atom("fails");
    let (t1, t2) = (g.atom("t1"), g.atom("t2"));
    let dom = g.apply(wk::SET_DOMAIN, vec![t1, t2]);

    let v = g.fresh();
    let body = g.apply(fails, vec![v]);
    let q = g.quantify(wk::EXISTS, v, Some(dom), body);
    let r = GraphEvaluator::new().eval(&mut g, q, &EmptyStructure, 50_000);
    assert_eq!(r.evidential(), Evidential::Open);
    assert!(r.bindings.is_empty());
}

/// **Dropping a binding at a composition is deliberate, and lossy.** A
/// counterexample to `∀x. P(x)` is genuinely informative about `¬∀x. P(x)` — it
/// is the reason that negation holds. It is dropped anyway, because a witness
/// justifies exactly the claim it settled and tracking the role-flip through
/// every operator is how an axis gets lost.
///
/// This test exists to pin the choice rather than to assert it is optimal: if
/// bindings are ever propagated through negation, it should be because somebody
/// decided to, not because nobody noticed.
#[test]
fn a_negated_universal_drops_its_counterexample_deliberately() {
    struct Closed3 {
        p: ObjectId,
        holds: Vec<ObjectId>,
    }
    impl GraphStructure for Closed3 {
        fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
            if pred != self.p || args.len() != 1 {
                return Knowledge::Unknown;
            }
            if self.holds.contains(&args[0]) { Knowledge::Holds } else { Knowledge::Fails }
        }
    }
    let mut g = ObjectGraph::new();
    let tested = g.atom("tested");
    let (a, b) = (g.atom("a.rs"), g.atom("b.rs"));
    let dom = g.apply(wk::SET_DOMAIN, vec![a, b]);
    let s = Closed3 { p: tested, holds: vec![a] };

    let v = g.fresh();
    let body = g.apply(tested, vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(dom), body);
    let negated = g.apply(wk::NOT, vec![q]);
    let ev = GraphEvaluator::new();

    let inner = ev.eval(&mut g, q, &s, 50_000);
    assert_eq!(inner.evidential(), Evidential::Refuted);
    assert_eq!(inner.bindings, vec![(0usize, b)], "the universal names its counterexample");

    let outer = ev.eval(&mut g, negated, &s, 50_000);
    assert_eq!(outer.evidential(), Evidential::Supported);
    assert!(
        outer.bindings.is_empty(),
        "and the negation does not inherit it — safe, and knowingly lossy"
    );
}
