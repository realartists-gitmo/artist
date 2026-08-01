//! `parse(print(x)) == x`, over the whole kernel rather than a convenient
//! subset.
//!
//! This exists because the spec was wrong and nobody could tell. `docs/formalism.md`
//! claimed the reader could not serialise an `Apply` whose first operand is a
//! list of lists — I wrote that from a summary and never ran it, a reviewer
//! reasoned correctly from the false premise, and the conclusion was wrong in
//! both directions. A prose spec is only as true as the tests under it.
//!
//! So: every `CoreNode` shape, every `LiteralValue` variant, sharing, cycles,
//! and the shapes that were actually broken.

use artist_logic::object::{Binding, CoreNode, ExternalRef, LiteralValue, wk};
use artist_logic::syntax::{parse, print};
use artist_logic::{ObjectGraph, ObjectId};
use num_bigint::BigInt;

/// Print, reparse into a *fresh* graph, print again. Comparing printed forms
/// rather than ids is deliberate: a datum-labelled cycle gets a freshly
/// allocated node on the way back, so exact graph equality cannot hold and
/// structure is the thing under test. (A bare skolem *does* survive — it prints
/// as its raw id and reparses to the same one.)
fn round_trip(g: &ObjectGraph, root: ObjectId) -> String {
    let before = print(g, root);
    let mut fresh = ObjectGraph::new();
    let back = parse(&mut fresh, &before)
        .unwrap_or_else(|e| panic!("parse failed for {before}: {e}"));
    let after = print(&fresh, back);
    assert_eq!(before, after, "round trip changed the expression");
    before
}

#[test]
fn every_literal_variant_round_trips() {
    let mut g = ObjectGraph::new();
    let holder = g.atom("holds-all");

    let big = BigInt::from(u64::MAX) * BigInt::from(u64::MAX);
    let cases = vec![
        g.lit(LiteralValue::Int(42.into())),
        g.lit(LiteralValue::Int(BigInt::from(-7))),
        g.lit(LiteralValue::Int(big)),
        g.lit(LiteralValue::Decimal { mantissa: (-12345).into(), scale: 3 }),
        g.lit(LiteralValue::Text("plain".into())),
        g.lit(LiteralValue::Text("with \"quotes\" and \\ backslash".into())),
        g.lit(LiteralValue::Bool(true)),
        g.lit(LiteralValue::Bool(false)),
        g.lit(LiteralValue::Bytes(vec![0, 1, 2, 255])),
        g.lit(LiteralValue::Bytes(Vec::new())),
    ];
    let root = g.apply(holder, cases);
    round_trip(&g, root);
}

/// A `Literal(Bool)` is not the atom `wk::TOP`. The spec claimed they collided.
#[test]
fn literal_bool_is_distinct_from_the_top_atom() {
    let mut g = ObjectGraph::new();
    let lit = g.lit(LiteralValue::Bool(true));
    assert_ne!(lit, wk::TOP, "Literal(Bool(true)) must not be the `true` atom");
    round_trip(&g, lit);
}

/// The shape the spec wrongly claimed was unserialisable.
#[test]
fn an_apply_whose_first_operand_is_a_list_of_lists() {
    let mut g = ObjectGraph::new();
    let (f, gg, x, y) = (g.atom("f"), g.atom("g"), g.atom("x"), g.atom("y"));
    let inner = g.apply(gg, vec![x]);
    let wrapped = g.apply(inner, vec![]);
    let root = g.apply(f, vec![wrapped, y]);
    assert_eq!(round_trip(&g, root), "(f ((g x)) y)");
}

/// The shape that genuinely *was* broken: `external` and `opaque` were reserved
/// in head position, so an ordinary application using either name as its
/// operator could not be read back at all.
#[test]
fn external_and_opaque_are_usable_as_ordinary_operators() {
    for name in ["external", "opaque"] {
        let mut g = ObjectGraph::new();
        let op = g.atom(name);
        let arg = g.atom("a");
        let root = g.apply(op, vec![arg]);
        assert_eq!(round_trip(&g, root), format!("({name} a)"));
    }
}

/// …while the node shapes themselves still round-trip, under `#external` /
/// `#opaque`, which cannot collide with a raw id or a datum label because both
/// of those require a digit straight after the `#`.
#[test]
fn external_and_opaque_nodes_round_trip() {
    let mut g = ObjectGraph::new();
    let full = g.external(ExternalRef {
        namespace: "file".into(),
        locator: b"/etc/hosts".to_vec(),
        version: Some(b"v2".to_vec()),
        digest: Some([3u8; 32]),
    });
    let bare = g.external(ExternalRef {
        namespace: "proc".into(),
        locator: b"1234".to_vec(),
        version: None,
        digest: None,
    });
    let opaque = g.intern(CoreNode::Opaque { tag: "future:v9".into(), payload: vec![1, 2, 3] });
    let empty = g.intern(CoreNode::Opaque { tag: "e".into(), payload: Vec::new() });

    let holder = g.atom("holds");
    let root = g.apply(holder, vec![full, bare, opaque, empty]);
    let text = round_trip(&g, root);
    assert!(text.contains("(#external file"), "{text}");
    assert!(text.contains("(#opaque future:v9"), "{text}");
}

#[test]
fn binders_domains_and_multiple_bodies_round_trip() {
    let mut g = ObjectGraph::new();
    let (path, stale, corrected, adam) =
        (g.atom("Path"), g.atom("stale"), g.atom("corrected"), g.atom("adam"));

    let f = g.fresh();
    let a = g.apply(stale, vec![f]);
    let b = g.apply(corrected, vec![adam, f]);
    let conj = g.apply(wk::AND, vec![a, b]);
    let quantified = g.quantify(wk::EXISTS, f, Some(path), conj);
    round_trip(&g, quantified);

    // No domain, and a binder taking two bodies.
    let r = g.fresh();
    let d1 = g.apply(stale, vec![r]);
    let d2 = g.apply(corrected, vec![adam, r]);
    let rec = g.bind(wk::LETREC, vec![Binding { var: r, domain: None }], vec![d1, d2]);
    round_trip(&g, rec);

    // A refined domain, which is an ordinary expression.
    let w = g.fresh();
    let checked = g.apply(stale, vec![w]);
    let narrowed = g.apply(wk::WHERE_DOMAIN, vec![path, checked]);
    let body = g.apply(corrected, vec![adam, w]);
    let q = g.quantify(wk::FORALL, w, Some(narrowed), body);
    let text = round_trip(&g, q);
    assert!(text.starts_with("(forall"), "{text}");
    assert!(text.contains("where"), "{text}");
}

#[test]
fn sharing_and_cycles_survive() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let shared = g.apply(p, vec![a]);
    let root = g.apply(wk::AND, (0..8).map(|_| shared).collect());
    let text = round_trip(&g, root);
    assert!(text.contains("#1="), "sharing should emit a datum label: {text}");
    assert!(text.contains("#1#"), "and refer back to it: {text}");

    // The liar: a node whose body mentions itself.
    let mut h = ObjectGraph::new();
    let liar = h.alloc();
    let quoted = h.apply(wk::QUOTE, vec![liar]);
    let holds = h.apply(wk::HOLDS, vec![quoted]);
    let neg = h.apply(wk::NOT, vec![holds]);
    let node = h.get(neg).cloned().expect("built");
    h.define(liar, node);

    let printed = print(&h, liar);
    let mut fresh = ObjectGraph::new();
    let back = parse(&mut fresh, &printed).expect("cycle must reparse");
    assert!(fresh.is_cyclic(back), "the cycle did not survive: {printed}");
}

/// Atom names that need quoting, including ones that would otherwise read as
/// numbers or as syntax.
#[test]
fn awkward_atom_names_round_trip() {
    let mut g = ObjectGraph::new();
    let holder = g.atom("holds");
    let names = ["has spaces", "42", "-7", "with(paren", "with\"quote", "with#hash", "with|bar"];
    let atoms: Vec<ObjectId> = names.iter().map(|n| g.atom(n)).collect();
    let root = g.apply(holder, atoms);
    round_trip(&g, root);

    // And each still resolves back to the same name.
    let printed = print(&g, root);
    let mut fresh = ObjectGraph::new();
    let back = parse(&mut fresh, &printed).expect("parse");
    let kids = fresh.children(back);
    for (name, id) in names.iter().zip(kids.iter().skip(1)) {
        match fresh.get(*id) {
            Some(CoreNode::Atom { name: Some(got) }) => assert_eq!(got, name),
            other => panic!("expected atom {name:?}, got {other:?}"),
        }
    }
}

/// An operator with no semantics is still perfectly representable — the point
/// of the open construct list.
#[test]
fn unknown_operators_are_representable() {
    let mut g = ObjectGraph::new();
    let op = g.atom("my:novel-operator");
    let (a, b) = (g.atom("a"), g.atom("b"));
    let root = g.apply(op, vec![a, b]);
    assert_eq!(round_trip(&g, root), "(my:novel-operator a b)");
}

/// The truth **atoms** must survive as themselves.
///
/// `print(wk::TOP)` was `"true"`, and `parse("true")` is a `Literal::Bool`.
/// So a graph containing `TOP` satisfied `print(parse(print(g))) == print(g)`
/// **while changing `Supported/Exact` into `Open/Unsupported`** — text
/// preserved, meaning destroyed. That is exactly the failure this file was
/// written to catch, hiding from the property this file was checking.
#[test]
fn the_truth_atoms_survive_as_themselves() {
    let mut g = ObjectGraph::new();

    assert_eq!(round_trip(&g, wk::TOP), "#true");
    assert_eq!(round_trip(&g, wk::BOT), "#false");

    let mut fresh = ObjectGraph::new();
    assert_eq!(parse(&mut fresh, "#true").unwrap(), wk::TOP);
    assert_eq!(parse(&mut fresh, "#false").unwrap(), wk::BOT);

    // …and Bool literals still round-trip as literals, distinctly.
    let lit = g.lit(LiteralValue::Bool(true));
    assert_eq!(round_trip(&g, lit), "true");
    assert_ne!(parse(&mut fresh, "true").unwrap(), wk::TOP);

    // The shape that was silently broken: a truth atom inside a conjunction.
    let p = g.atom("p");
    let a = g.atom("a");
    let pa = g.apply(p, vec![a]);
    let conj = g.apply(wk::AND, vec![pa, wk::TOP]);
    assert_eq!(round_trip(&g, conj), "(and (p a) #true)");
}

/// Text equality is too weak a round-trip property on its own. What has to
/// survive is what the expression *does*, so this compares evaluated results
/// before and after — the check that would have caught the `TOP` collision.
#[test]
fn round_tripping_preserves_evaluation_not_just_text() {
    use artist_logic::graph_eval::{EmptyStructure, GraphEvaluator};

    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let pa = g.apply(p, vec![a]);

    let cases = vec![
        wk::TOP,
        wk::BOT,
        g.apply(wk::AND, vec![pa, wk::TOP]),
        g.apply(wk::OR, vec![pa, wk::TOP]),
        g.apply(wk::NOT, vec![wk::BOT]),
        {
            let (two, four) = (g.int(2), g.int(4));
            let sum = g.apply(wk::ADD, vec![two, two]);
            g.apply(wk::EQ, vec![sum, four])
        },
    ];

    let ev = GraphEvaluator::new();
    for root in cases {
        let text = print(&g, root);
        let before = ev.eval(&mut g, root, &EmptyStructure, 10_000);

        let mut fresh = ObjectGraph::new();
        let back = parse(&mut fresh, &text).unwrap_or_else(|e| panic!("parse {text}: {e}"));
        let after = ev.eval(&mut fresh, back, &EmptyStructure, 10_000);

        assert_eq!(
            (before.evidential(), before.compute_status),
            (after.evidential(), after.compute_status),
            "round trip changed what {text} means"
        );
    }
}
