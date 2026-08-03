//! Identity: what is globally unique, what is globally *stable*, and the gap
//! between them.
//!
//! Two id spaces share one 128-bit range and they answer opposite questions.
//! `content_id` must give the *same* id for the same structure in every store
//! and forever — that is what makes dedup and sharing free. `alloc` must give a
//! *different* id in every store, because it names an entity that has no
//! structure to be identified by: a variable, a skolem, a node whose body does
//! not exist yet.
//!
//! `alloc` used to be a counter starting at `FIRST_FREE`, so every freshly
//! opened graph handed out 4096, 4097, … Two stores each minting `#4096` for
//! unrelated entities is not a merge-time hazard to plan for; `Memory::search`
//! already queries `global.rocks` and `project.rocks` together and merges the
//! hits, so the collision was live.

use artist_logic::object::{CoreNode, IdSpace, LiteralValue, wk};
use artist_logic::{ObjectGraph, ObjectId};
use std::collections::BTreeSet;

/// Nominal ids from independent graphs must not overlap. This is the property
/// whose absence was the bug.
#[test]
fn independent_graphs_mint_disjoint_nominal_ids() {
    const PER_GRAPH: usize = 512;
    let mut seen: BTreeSet<ObjectId> = BTreeSet::new();
    let mut collisions = 0;

    for _ in 0..8 {
        let mut g = ObjectGraph::new();
        for _ in 0..PER_GRAPH {
            if !seen.insert(g.fresh()) {
                collisions += 1;
            }
        }
    }

    assert_eq!(collisions, 0, "nominal ids collided across graphs");
    assert_eq!(seen.len(), 8 * PER_GRAPH);
}

/// A clone that kept its parent's seed and counter would mint the parent's ids
/// over again — the same defect as the old counter, reintroduced by a derive.
#[test]
fn cloning_a_graph_reseeds_the_allocator() {
    let mut parent = ObjectGraph::new();
    let mut child = parent.clone();

    let from_parent: BTreeSet<ObjectId> = (0..64).map(|_| parent.fresh()).collect();
    let from_child: BTreeSet<ObjectId> = (0..64).map(|_| child.fresh()).collect();

    assert!(
        from_parent.is_disjoint(&from_child),
        "a cloned graph re-minted its parent's ids"
    );
}

/// `Default` must be `new`, not a zeroed struct: a zero seed makes every graph
/// allocate identically, and an empty graph has none of the well-known atoms.
#[test]
fn default_is_a_usable_graph() {
    let mut a = ObjectGraph::default();
    let mut b = ObjectGraph::default();

    assert_eq!(a.atom("and"), wk::AND, "well-known atoms must be preloaded");
    let ids_a: BTreeSet<ObjectId> = (0..64).map(|_| a.fresh()).collect();
    let ids_b: BTreeSet<ObjectId> = (0..64).map(|_| b.fresh()).collect();
    assert!(
        ids_a.is_disjoint(&ids_b),
        "Default graphs share an allocator seed"
    );
}

/// The other half of the contract, stated narrowly: **closed structural**
/// expressions must content-address identically everywhere, or nothing dedups.
///
/// Closed matters. An expression containing a free variable or a skolem carries
/// a nominal id, and nominal ids are *supposed* to differ between graphs — so
/// the agreement property holds for the structural fragment only, not for
/// expressions generally.
#[test]
fn closed_structural_expressions_agree_across_graphs() {
    fn build(g: &mut ObjectGraph) -> ObjectId {
        let prefers = g.atom("prefers");
        let adam = g.atom("adam");
        let tabs = g.atom("tabs");
        let n = g.lit(LiteralValue::Int(42.into()));
        let inner = g.apply(prefers, vec![adam, tabs]);
        g.apply(wk::AND, vec![inner, n])
    }

    let (mut one, mut two) = (ObjectGraph::new(), ObjectGraph::new());
    assert_eq!(
        build(&mut one),
        build(&mut two),
        "closed structure must content-address identically in every store"
    );
}

/// …and the converse, which is a feature rather than a shortfall: an expression
/// carrying an independently minted skolem must *not* agree, because the two
/// skolems name different unknowns.
#[test]
fn expressions_over_independent_skolems_deliberately_differ() {
    fn build(g: &mut ObjectGraph) -> ObjectId {
        let p = g.atom("p");
        let sk = g.fresh();
        g.apply(p, vec![sk])
    }

    let (mut one, mut two) = (ObjectGraph::new(), ObjectGraph::new());
    assert_ne!(
        build(&mut one),
        build(&mut two),
        "two graphs' skolems name different things and must not be conflated"
    );
}

/// The three ways an id is minted occupy disjoint halves of the space, so a
/// random nominal id cannot land on a content-derived one — a skolem silently
/// *becoming* a stored expression is impossible, not merely unlikely.
#[test]
fn the_id_spaces_are_disjoint() {
    let mut g = ObjectGraph::new();

    assert_eq!(wk::AND.space(), IdSpace::WellKnown);
    assert_eq!(wk::FORALL.space(), IdSpace::WellKnown);

    let structural = {
        let p = g.atom("p");
        let a = g.atom("a");
        g.apply(p, vec![a])
    };
    assert_eq!(structural.space(), IdSpace::Content);

    for _ in 0..2048 {
        let id = g.fresh();
        assert_eq!(
            id.space(),
            IdSpace::Nominal,
            "nominal id {id} left its namespace"
        );
        assert!(
            id.0 >= wk::FIRST_FREE.0,
            "nominal id {id} entered the reserved range"
        );
    }
}

/// The point of storing binders in de Bruijn form: two facts that differ only
/// in what the variable was called are **one object**.
#[test]
fn alpha_equivalent_facts_are_one_object() {
    fn fact(g: &mut ObjectGraph, var_name: &str) -> ObjectId {
        let path = g.atom("Path");
        let stale = g.atom("stale");
        let v = g.atom(var_name);
        let body = g.apply(stale, vec![v]);
        g.quantify(wk::FORALL, v, Some(path), body)
    }

    let mut g = ObjectGraph::new();
    assert_eq!(
        fact(&mut g, "f"),
        fact(&mut g, "x"),
        "renaming a bound variable must not produce a second fact"
    );

    // And across stores, since that is where duplicates would accumulate.
    let (mut one, mut two) = (ObjectGraph::new(), ObjectGraph::new());
    assert_eq!(fact(&mut one, "f"), fact(&mut two, "zzz"));
}

/// Alpha-equivalence must not flatten a real difference: two *different*
/// quantified facts still get different ids.
#[test]
fn alpha_equivalence_does_not_merge_different_facts() {
    let mut g = ObjectGraph::new();
    let path = g.atom("Path");
    let (stale, fresh_p) = (g.atom("stale"), g.atom("fresh"));
    let v = g.atom("v");

    let a_body = g.apply(stale, vec![v]);
    let a = g.quantify(wk::FORALL, v, Some(path), a_body);
    let b_body = g.apply(fresh_p, vec![v]);
    let b = g.quantify(wk::FORALL, v, Some(path), b_body);
    assert_ne!(a, b, "different predicates must stay different facts");

    // Different binder, same body.
    let c = g.quantify(wk::EXISTS, v, Some(path), a_body);
    assert_ne!(a, c, "forall and exists must stay different facts");
}

/// A variable free in the whole expression is not bound by anything, so it
/// keeps its nominal identity and two such expressions stay distinct.
#[test]
fn free_variables_are_not_abstracted() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let free = g.fresh();
    let bound = g.fresh();
    let dom = g.atom("D");

    let body = {
        let a = g.apply(p, vec![bound]);
        let b = g.apply(p, vec![free]);
        g.apply(wk::AND, vec![a, b])
    };
    let q = g.quantify(wk::FORALL, bound, Some(dom), body);

    // Reopening yields a fresh bound variable but the *same* free one.
    let (_, vars, bodies) = g.open_binder(q).expect("a binder");
    assert_eq!(vars.len(), 1);
    assert_ne!(vars[0].var, bound, "opening mints a fresh bound variable");
    assert!(
        g.reachable(bodies[0]).contains(&free),
        "the free variable must survive abstraction untouched"
    );
}

/// Shadowing: an inner binder reusing a name captures it, and the outer
/// variable remains reachable in the outer scope only.
#[test]
fn an_inner_binder_shadows_an_outer_one() {
    let mut g = ObjectGraph::new();
    let (p, d) = (g.atom("p"), g.atom("D"));
    let v = g.atom("v");

    let inner_body = g.apply(p, vec![v]);
    let inner = g.quantify(wk::EXISTS, v, Some(d), inner_body);
    let outer = g.quantify(wk::FORALL, v, Some(d), inner);

    // Same shape with distinct names must be the same object — which is only
    // true if the inner binder captured the occurrence.
    let (a, b) = (g.atom("a"), g.atom("b"));
    let inner_body2 = g.apply(p, vec![b]);
    let inner2 = g.quantify(wk::EXISTS, b, Some(d), inner_body2);
    let outer2 = g.quantify(wk::FORALL, a, Some(d), inner2);
    assert_eq!(
        outer, outer2,
        "shadowing must resolve to the innermost binder"
    );
}

/// Allocation is what makes cycles representable, and it has to keep working
/// now that ids are random rather than sequential.
#[test]
fn allocation_still_supports_cycles() {
    let mut g = ObjectGraph::new();
    let liar = g.alloc();
    let quoted = g.apply(wk::QUOTE, vec![liar]);
    let holds = g.apply(wk::HOLDS, vec![quoted]);
    let negated = g.apply(wk::NOT, vec![holds]);
    let body = g.get(negated).cloned().expect("built");
    g.define(liar, body);

    assert!(
        g.is_cyclic(liar),
        "alloc-before-define must still close a cycle"
    );
    assert!(matches!(g.get(liar), Some(CoreNode::Apply { .. })));
}
