//! Round-trip through the real store, evaluated on the universal graph.
//!
//! Sorts and closed-world status are *derived from facts*: nothing declares a
//! schema for `Person` or `Path`, both come into existence by writing `type(…)`,
//! and the negation rule for each follows from whether a `has_resolver`
//! statement exists. Supersession is valid time, so what was once believed stays
//! askable.

use artist_logic::{Evidential, GraphEvaluator, GraphStructure, ObjectGraph, wk};
use artist_memory::relational::{
    RelationalView, assert_stmt, assert_stmt_at, intern, oid, retract_stmt_at, well_known,
};
use artist_memory::{MemoryStore, Scope};
use tempfile::TempDir;

async fn store() -> (TempDir, MemoryStore) {
    let dir = TempDir::new().expect("tempdir");
    let s = MemoryStore::open(dir.path().join("t.rocks"), Scope::Project)
        .await
        .expect("open");
    (dir, s)
}

#[tokio::test]
async fn sorts_and_closure_are_facts_not_schema() {
    let (_d, s) = store().await;
    let path = intern(&s, "Path").await.expect("intern");
    let a = intern(&s, "src/main.rs").await.expect("intern");
    let b = intern(&s, "src/old.rs").await.expect("intern");
    let exists = intern(&s, "exists").await.expect("intern");

    for e in [a, b] {
        assert_stmt(&s, well_known::TYPE, &[e, path], "t").await.expect("type");
    }
    assert_stmt(&s, exists, &[a], "t").await.expect("exists");

    let mut g = ObjectGraph::new();
    let ev = GraphEvaluator::new();
    let view = RelationalView::load(&s).await.expect("load");
    let missing = g.apply(oid(exists), vec![oid(b)]);

    // No authority yet: absence proves nothing.
    let r = ev.eval(&mut g, missing, &view, 10_000);
    assert_eq!(r.evidential(), Evidential::Open, "not-known, not known-false");

    // One written fact tightens every negation over it.
    assert_stmt(&s, well_known::HAS_RESOLVER, &[exists], "t").await.expect("r");
    let view = RelationalView::load(&s).await.expect("reload");
    let r = ev.eval(&mut g, missing, &view, 10_000);
    assert_eq!(r.evidential(), Evidential::Refuted);

    // And the sort populated itself.
    assert_eq!(view.extension(oid(path)).map(|v| v.len()), Some(2));
}

#[tokio::test]
async fn arity_is_unbounded_in_storage() {
    let (_d, s) = store().await;
    let wide = intern(&s, "meeting").await.expect("intern");
    let mut args = Vec::new();
    for i in 0..7 {
        args.push(intern(&s, &format!("p{i}")).await.expect("intern"));
    }
    assert_stmt(&s, wide, &args, "t").await.expect("assert");

    let view = RelationalView::load(&s).await.expect("load");
    let lifted: Vec<_> = args.iter().map(|a| oid(*a)).collect();
    assert_eq!(view.known(oid(wide), &lifted), Some(true), "7-ary survives");
}

/// The store remembers what it used to believe.
#[tokio::test]
async fn past_beliefs_survive_retraction() {
    let (_d, s) = store().await;
    let p = intern(&s, "prefers").await.expect("intern");
    let adam = intern(&s, "adam").await.expect("intern");
    let tabs = intern(&s, "tabs").await.expect("intern");

    let id = assert_stmt_at(&s, p, &[adam, tabs], "t", Some(1_000)).await.expect("a");
    retract_stmt_at(&s, id, Some(2_000)).await.expect("r");
    assert_stmt(&s, well_known::HAS_RESOLVER, &[p], "t").await.expect("res");

    let view = RelationalView::load(&s).await.expect("load");
    let mut g = ObjectGraph::new();
    let ev = GraphEvaluator::new();
    let held = g.apply(oid(p), vec![oid(adam), oid(tabs)]);

    // Now: no longer believed.
    assert_eq!(ev.eval(&mut g, held, &view, 10_000).evidential(), Evidential::Refuted);

    // Then: believed. Supersession is history the logic can see, not a flag.
    let when = g.int(1_500);
    let past = g.apply(wk::AT, vec![when, held]);
    assert_eq!(
        ev.eval(&mut g, past, &view, 10_000).evidential(),
        Evidential::Supported,
        "the store can be asked what it used to believe"
    );
}

/// `is(a, b)` written to the store makes equality act.
#[tokio::test]
async fn asserted_identity_round_trips() {
    let (_d, s) = store().await;
    let p = intern(&s, "trusts").await.expect("intern");
    let adam = intern(&s, "adam").await.expect("intern");
    let full = intern(&s, "chinmay_mehta").await.expect("intern");
    let short = intern(&s, "chinmay").await.expect("intern");
    assert_stmt(&s, p, &[adam, full], "t").await.expect("fact");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.known(oid(p), &[oid(adam), oid(short)]),
        None,
        "two names, no asserted link"
    );

    assert_stmt(&s, well_known::IS, &[short, full], "t").await.expect("is");
    let view = RelationalView::load(&s).await.expect("reload");
    assert_eq!(
        view.known(oid(p), &[oid(adam), oid(short)]),
        Some(true),
        "one identity statement and the fact is reachable under either name"
    );
}
