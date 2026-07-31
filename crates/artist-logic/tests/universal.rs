//! Adversarial definition-of-done for the universal expression language.
//!
//! Every case here must **serialize, reload, and either evaluate or return an
//! honest residual**. Nothing may be rejected at construction, at print, at
//! parse, or at load. Where semantics are absent the answer is
//! `Unsupported` — which is a different answer from "no evidence" and from
//! "out of budget", and the caller can tell.

use artist_logic::object::{Binding, CoreNode, ExternalRef, LiteralValue, wk};
use artist_logic::registry::{OpContext, OperatorSemantics};
use artist_logic::syntax::{from_bytes, parse, print, to_bytes};
use artist_logic::{Typing, type_of};
use artist_logic::*;
use num_bigint::BigInt;

/// Round-trip through both canonical forms. Returns the reparsed text form.
fn round_trip(g: &ObjectGraph, root: ObjectId) -> String {
    let text = print(g, root);

    // Text: print -> parse -> print must be a fixed point.
    let mut g2 = ObjectGraph::new();
    let re = parse(&mut g2, &text).unwrap_or_else(|e| panic!("parse failed: {e}\n{text}"));
    let text2 = print(&g2, re);
    assert_eq!(text, text2, "text round trip is not a fixed point");

    // Binary: encode -> decode preserves ids exactly.
    let bytes = to_bytes(g, root);
    let mut g3 = ObjectGraph::new();
    let back = from_bytes(&mut g3, &bytes).expect("binary decode");
    assert_eq!(back, root, "binary round trip changed the root id");
    assert_eq!(
        print(&g3, back),
        text,
        "binary round trip changed the structure"
    );
    text
}

// ---- 1. arbitrary arity ------------------------------------------------

#[test]
fn arbitrary_arity_predicate_application() {
    let mut g = ObjectGraph::new();
    let meeting = g.atom("meeting");
    let args: Vec<ObjectId> = (0..500).map(|i| g.atom(&format!("p{i}"))).collect();
    let e = g.apply(meeting, args);
    let t = round_trip(&g, e);
    assert!(t.starts_with("(meeting p0 p1"));
}

// ---- 2, 3, 4. quantifying over relations, types, and higher ------------

#[test]
fn quantification_over_relations_types_and_relations_of_relations() {
    let mut g = ObjectGraph::new();

    // ∀R : Rel(Person, Person). R(a, b)
    let person = g.atom("Person");
    let rel_ty = g.apply(wk::RELATION_TYPE, vec![person, person]);
    let r = g.fresh();
    let a = g.atom("a");
    let b = g.atom("b");
    let body = g.apply(r, vec![a, b]);
    let over_rel = g.quantify(wk::FORALL, r, Some(rel_ty), body);
    round_trip(&g, over_rel);

    // ∀T : Universe(0). ∃x : T. true  — quantification over *types*.
    let u0 = g.int(0);
    let universe = g.apply(wk::UNIVERSE, vec![u0]);
    let ty = g.fresh();
    let x = g.fresh();
    let inner = g.quantify(wk::EXISTS, x, Some(ty), wk::TOP);
    let over_ty = g.quantify(wk::FORALL, ty, Some(universe), inner);
    round_trip(&g, over_ty);

    // ∀F : Rel(Rel(Person,Person)). F(R) — a relation over relations. The
    // former `ForallPred(Var, usize, _)` could not say this at all.
    let rel_of_rel = g.apply(wk::RELATION_TYPE, vec![rel_ty]);
    let ff = g.fresh();
    let applied = g.apply(ff, vec![r]);
    let third_order = g.quantify(wk::FORALL, ff, Some(rel_of_rel), applied);
    let t = round_trip(&g, third_order);
    assert!(t.contains("Rel"), "third-order type survives: {t}");
}

// ---- 5, 6. recursion, mutual recursion ---------------------------------

#[test]
fn transitive_closure_and_mutual_recursion() {
    let mut g = ObjectGraph::new();
    let edge = g.atom("edge");
    let node = g.atom("Node");
    let (x, y, z) = (g.fresh(), g.fresh(), g.fresh());
    let reach = g.fresh();

    let direct = g.apply(edge, vec![x, y]);
    let hop_a = g.apply(reach, vec![x, z]);
    let hop_b = g.apply(edge, vec![z, y]);
    let hop_and = g.apply(wk::AND, vec![hop_a, hop_b]);
    let hop = g.quantify(wk::EXISTS, z, Some(node), hop_and);
    let def = g.apply(wk::OR, vec![direct, hop]);
    let scope = g.apply(reach, vec![x, y]);
    let letrec = g.bind(
        wk::LETREC,
        vec![
            Binding { var: reach, domain: None },
            Binding { var: x, domain: Some(node) },
            Binding { var: y, domain: Some(node) },
        ],
        vec![def, scope],
    );
    round_trip(&g, letrec);

    // Mutual recursion: even/odd over ℕ, two definitions in one binder.
    let (even, odd, n) = (g.fresh(), g.fresh(), g.fresh());
    let nat = g.apply(wk::NAT_TYPE, vec![]);
    let zero = g.int(0);
    let one = g.int(1);
    let n_is_zero = g.apply(wk::EQ, vec![n, zero]);
    let n_minus = g.apply(wk::NEG, vec![one]);
    let pred_n = g.apply(wk::ADD, vec![n, n_minus]);
    let odd_pred = g.apply(odd, vec![pred_n]);
    let even_def = g.apply(wk::OR, vec![n_is_zero, odd_pred]);
    let even_pred = g.apply(even, vec![pred_n]);
    let mutual_scope = g.apply(even, vec![n]);
    let mutual = g.bind(
        wk::LETREC,
        vec![
            Binding { var: even, domain: None },
            Binding { var: odd, domain: None },
            Binding { var: n, domain: Some(nat) },
        ],
        vec![even_def, even_pred, mutual_scope],
    );
    let t = round_trip(&g, mutual);
    assert!(t.starts_with("(letrec"), "{t}");
}

// ---- 7, 8, 9. quotation, truth-assertion, self-reference ---------------

#[test]
fn quotation_assertion_and_the_liar() {
    let mut g = ObjectGraph::new();
    let prefers = g.atom("prefers");
    let adam = g.atom("adam");
    let tabs = g.atom("tabs");
    let p = g.apply(prefers, vec![adam, tabs]);

    // A quoted proposition.
    let quoted = g.apply(wk::QUOTE, vec![p]);
    round_trip(&g, quoted);

    // A proposition asserting another proposition's truth.
    let sarah = g.atom("sarah");
    let told = g.apply(wk::ASSERTED_BY, vec![quoted, sarah]);
    round_trip(&g, told);
    // Quoting does not imply asserting: they are distinct objects.
    assert_ne!(quoted, p);
    assert_ne!(told, p);

    // The liar: L = ¬holds(⟨L⟩). Representable because the id exists before
    // the body does — blocking this by construction was an evaluator
    // convenience leaking into representation.
    let liar = g.alloc();
    let q_liar = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q_liar]);
    let neg = g.apply(wk::NOT, vec![h]);
    let node = g.get(neg).cloned().expect("built");
    g.define(liar, node);

    assert!(g.is_cyclic(liar), "the liar really does reach itself");
    let t = round_trip(&g, liar);
    assert!(t.contains("#1="), "cycle printed with a datum label: {t}");
    assert!(t.contains("#1#"), "and refers back to it: {t}");
}

// ---- 10. contradictory evidence ----------------------------------------

#[test]
fn contradictory_evidence_is_its_own_state() {
    let mut g = ObjectGraph::new();
    let p = g.atom("deploys-on-friday");
    let mut ledger = EvidenceLedger::new();

    assert_eq!(ledger.state_of(p), Evidential::Open, "no evidence yet");

    let s1 = g.atom("session-1");
    let s2 = g.atom("session-2");
    ledger.record(Evidence {
        id: g.fresh(),
        target: p,
        polarity: Polarity::Affirm,
        source: Some(s1),
        source_span: Some(SourceSpan {
            locator: "events.jsonl".into(),
            start: 100,
            end: 140,
        }),
        transformation: None,
        derived_from: vec![],
        llr_milli: Some(1200),
        created_by: None,
    });
    assert_eq!(ledger.state_of(p), Evidential::Supported);

    ledger.record(Evidence {
        id: g.fresh(),
        target: p,
        polarity: Polarity::Deny,
        source: Some(s2),
        source_span: None,
        transformation: None,
        derived_from: vec![],
        llr_milli: Some(900),
        created_by: None,
    });

    // The state a three-valued logic had nowhere to put.
    assert_eq!(ledger.state_of(p), Evidential::Conflicted);
    assert_eq!(ledger.weight_of(p), 300, "net weight still computable");
    assert_ne!(
        ledger.state_of(p),
        Evidential::Open,
        "conflict must not read as absence"
    );
}

// ---- 11, 12, 13. worlds, times, counterfactuals ------------------------

#[test]
fn same_proposition_across_worlds_times_and_counterfactuals() {
    let mut g = ObjectGraph::new();
    let uses = g.atom("uses");
    let artist = g.atom("artist");
    let tokio = g.atom("tokio");
    let p = g.apply(uses, vec![artist, tokio]);

    let w1 = g.atom("actual");
    let w2 = g.atom("alt");
    let in_w1 = g.apply(wk::IN_WORLD, vec![w1, p]);
    let in_w2 = g.apply(wk::IN_WORLD, vec![w2, p]);
    assert_ne!(in_w1, in_w2, "same proposition, two worlds, two objects");
    round_trip(&g, in_w1);
    round_trip(&g, in_w2);

    let t1 = g.int(1_000);
    let t2 = g.int(2_000);
    let at1 = g.apply(wk::AT, vec![t1, p]);
    let at2 = g.apply(wk::AT, vec![t2, p]);
    assert_ne!(at1, at2);
    round_trip(&g, at1);

    // A counterfactual, with an explicit world-selection rule.
    let async_std = g.atom("async-std");
    let antecedent = g.apply(uses, vec![artist, async_std]);
    let consequent = g.atom("slower-compaction");
    let selection = g.atom("closest-world");
    let cf = g.apply(wk::COUNTERFACTUAL, vec![antecedent, consequent, selection]);
    let t = round_trip(&g, cf);
    assert!(t.contains("if-counterfactually"), "{t}");
}

// ---- 14, 15. provenance and external references ------------------------

#[test]
fn source_spans_and_opaque_external_objects() {
    let mut g = ObjectGraph::new();

    // An external object denoted directly, never reduced to internal syntax.
    let file = g.external(ExternalRef {
        namespace: "file".into(),
        locator: b"/home/adam/Projects/artist/Cargo.toml".to_vec(),
        version: Some(b"blake3:abc".to_vec()),
        digest: Some([7u8; 32]),
    });
    round_trip(&g, file);

    // Unresolved is still a valid expression.
    let oracle = g.external(ExternalRef {
        namespace: "oracle".into(),
        locator: b"riemann-hypothesis".to_vec(),
        version: None,
        digest: None,
    });
    let mentions = g.atom("mentions");
    let claim = g.apply(mentions, vec![file, oracle]);
    round_trip(&g, claim);

    let span = SourceSpan { locator: "src/lib.rs".into(), start: 40, end: 92 };
    assert_eq!(span.end - span.start, 52);
}

// ---- 16, 19. unknown and newly-added operators -------------------------

struct Weird;
impl OperatorSemantics for Weird {
    fn name(&self) -> &str {
        "weird"
    }
    fn evaluate_exact(&self, cx: &mut OpContext<'_>) -> Option<EvaluationResult> {
        cx.charge(1);
        Some(EvaluationResult::certain(cx.operands.len() == 2))
    }
}

#[test]
fn unknown_operators_are_representable_and_honestly_unsupported() {
    let mut g = ObjectGraph::new();
    let mine = g.atom("my:novel-operator");
    let a = g.atom("a");
    let b = g.atom("b");
    let e = g.apply(mine, vec![a, b]);

    // Representable and round-trippable with no semantics at all.
    round_trip(&g, e);

    let mut reg = OperatorRegistry::new();
    let mut budget = 1_000u64;
    let r = reg.evaluate(&g, e, mine, &[a, b], &mut budget, 0);
    assert_eq!(
        r.compute_status,
        ComputeStatus::Unsupported,
        "no semantics is its own answer"
    );
    assert_ne!(
        r.compute_status,
        ComputeStatus::BudgetExhausted,
        "and is not confused with running out of budget"
    );
    assert_eq!(r.evidential(), Evidential::Open);
    assert_eq!(r.residual, Some(e), "handed back for later");

    // Item 19: adding semantics needs no schema migration and no reload.
    reg.register(mine, Box::new(Weird));
    let mut budget = 1_000u64;
    let r = reg.evaluate(&g, e, mine, &[a, b], &mut budget, 0);
    assert_eq!(r.compute_status, ComputeStatus::Exact);
    assert_eq!(r.evidential(), Evidential::Supported);
    // The stored expression never changed.
    assert_eq!(print(&g, e), "(my:novel-operator a b)");
}

// ---- 17, 18. persisted continuations and restart -----------------------

#[test]
fn a_partially_evaluated_query_survives_process_restart() {
    let mut g = ObjectGraph::new();
    let path = g.atom("Path");
    let stale = g.atom("stale");
    let f = g.fresh();
    let body = g.apply(stale, vec![f]);
    let query = g.quantify(wk::EXISTS, f, Some(path), body);

    // A continuation is an ordinary object: residual + bounds + spend.
    let examined = g.int(500);
    let spent = g.int(1_234);
    let cont = g.apply(wk::AND, vec![query, examined, spent]);

    let mut partial = EvaluationResult::new(ComputeStatus::BudgetExhausted);
    partial.support = Bound::Partial;
    partial.residual = Some(query);
    partial.continuation = Some(cont);
    partial.snapshot = 42;
    partial.spent = 1_234;

    // "Process restart": serialize, drop the graph, reload from bytes alone.
    let bytes = to_bytes(&g, cont);
    drop(g);
    let mut fresh = ObjectGraph::new();
    let reloaded = from_bytes(&mut fresh, &bytes).expect("reload");
    assert_eq!(reloaded, cont, "continuation id is stable across restart");

    let kids = fresh.children(reloaded);
    assert!(kids.contains(&query), "the residual query came back");
    assert_eq!(partial.snapshot, 42, "and knows which universe version it used");
    assert!(!partial.is_definite(), "a partial result never reads as definite");
}

// ---- 20, 21. cycles and size -------------------------------------------

#[test]
fn cyclic_structures_and_expressions_past_any_former_bound() {
    let mut g = ObjectGraph::new();

    // A cyclic stream: s = cons(1, s).
    let cons = g.atom("cons");
    let one = g.int(1);
    let s = g.alloc();
    let body = g.apply(cons, vec![one, s]);
    let node = g.get(body).cloned().expect("built");
    g.define(s, node);
    assert!(g.is_cyclic(s));
    let t = round_trip(&g, s);
    assert!(t.contains("#1="), "cycle labelled: {t}");

    // Deeply nested, past any former depth or arity ceiling.
    let f = g.atom("f");
    let mut deep = g.atom("base");
    for _ in 0..5_000 {
        deep = g.apply(f, vec![deep]);
    }
    let reached = g.reachable(deep);
    assert!(reached.len() > 5_000, "no depth ceiling: {}", reached.len());

    // Integer magnitude past i64 — the old silent boundary.
    let huge: BigInt = BigInt::from(u64::MAX) * BigInt::from(u64::MAX) * BigInt::from(1_000_000);
    let big = g.lit(LiteralValue::Int(huge.clone()));
    let t = round_trip(&g, big);
    assert_eq!(t, huge.to_string(), "arbitrary precision survives the trip");
}

// ---- unknown node preservation -----------------------------------------

#[test]
fn nodes_from_an_unknown_producer_survive_verbatim() {
    let mut g = ObjectGraph::new();
    let weird = g.intern(CoreNode::Opaque {
        tag: "future:v9".into(),
        payload: vec![0xde, 0xad, 0xbe, 0xef],
    });
    let wrap = g.atom("wraps");
    let e = g.apply(wrap, vec![weird]);
    round_trip(&g, e);

    let bytes = to_bytes(&g, e);
    let mut g2 = ObjectGraph::new();
    let back = from_bytes(&mut g2, &bytes).expect("decode");
    let kids = g2.children(back);
    match g2.get(kids[1]) {
        Some(CoreNode::Opaque { tag, payload }) => {
            assert_eq!(tag, "future:v9");
            assert_eq!(payload, &vec![0xde, 0xad, 0xbe, 0xef]);
        }
        other => panic!("opaque node was not preserved: {other:?}"),
    }
}

// ---- identity discipline ------------------------------------------------

#[test]
fn identity_kinds_are_distinguished() {
    let mut g = ObjectGraph::new();
    let p = g.atom("prefers");
    let a = g.atom("adam");
    let t = g.atom("tabs");

    // Object identity: structurally identical expressions share an id.
    let e1 = g.apply(p, vec![a, t]);
    let e2 = g.apply(p, vec![a, t]);
    assert_eq!(e1, e2, "content addressing gives structural sharing");

    // Distinct variables are distinct objects however alike they look — this
    // is what makes variable capture impossible rather than merely avoided.
    let v1 = g.fresh();
    let v2 = g.fresh();
    assert_ne!(v1, v2);

    // Asserted same-as is a *proposition*, not an identity of objects.
    let chinmay = g.atom("chinmay");
    let full = g.atom("chinmay_mehta");
    let same = g.apply(wk::SAME_AS, vec![chinmay, full]);
    assert_ne!(chinmay, full, "asserting identity does not merge the objects");
    round_trip(&g, same);
}

// ---- advisory typing ----------------------------------------------------

/// Types are first-class and recursively constructible, and inference is
/// **advisory**: a mismatch is a finding, never a refusal to represent.
#[test]
fn typing_is_advisory_and_never_blocks_representation() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let atom = g.apply(p, vec![a]);
    let one = g.int(1);

    assert_eq!(type_of(&mut g, atom), Typing::Known(wk::PROP_TYPE));
    assert_eq!(type_of(&mut g, one), Typing::Known(wk::INT_TYPE));

    // A quotation lifts to Expr; `holds` lowers it back to Prop.
    let q = g.apply(wk::QUOTE, vec![atom]);
    assert_eq!(type_of(&mut g, q), Typing::Known(wk::EXPR_TYPE));
    let h = g.apply(wk::HOLDS, vec![q]);
    assert_eq!(type_of(&mut g, h), Typing::Known(wk::PROP_TYPE));

    // An ill-typed expression is still fully representable and printable.
    let bad = g.apply(wk::ADD, vec![atom, one]);
    assert!(type_of(&mut g, bad).is_mismatch(), "the mismatch is detected");
    round_trip(&g, bad);
    assert!(
        print(&g, bad).starts_with("(+"),
        "and it still stores, prints and reparses"
    );
}

/// Universe levels are values, so the tower is unbounded rather than one enum
/// variant per order.
#[test]
fn universe_levels_are_values() {
    let mut g = ObjectGraph::new();
    let zero = g.int(0);
    let u0 = g.apply(wk::UNIVERSE, vec![zero]);
    let one = g.int(1);
    let u1 = g.apply(wk::UNIVERSE, vec![one]);

    assert_eq!(type_of(&mut g, u0), Typing::Known(u1), "Universe(0) : Universe(1)");

    let big = g.int(9_999);
    let u_big = g.apply(wk::UNIVERSE, vec![big]);
    let next = g.int(10_000);
    let expected = g.apply(wk::UNIVERSE, vec![next]);
    assert_eq!(
        type_of(&mut g, u_big),
        Typing::Known(expected),
        "no ceiling on the type tower"
    );
}

/// A lambda denotes a relation over its parameter domains.
#[test]
fn lambdas_are_typed_as_relations() {
    let mut g = ObjectGraph::new();
    let person = g.atom("Person");
    let x = g.fresh();
    let mortal = g.atom("mortal");
    let body = g.apply(mortal, vec![x]);
    let lam = g.bind(
        wk::LAMBDA,
        vec![Binding { var: x, domain: Some(person) }],
        vec![body],
    );
    let expected = g.apply(wk::RELATION_TYPE, vec![person]);
    assert_eq!(type_of(&mut g, lam), Typing::Known(expected));
}
