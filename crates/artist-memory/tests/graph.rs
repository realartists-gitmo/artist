//! Persistence for the universal object graph, against real RocksDB.
//!
//! The defect this closes: `assert_stmt` took `pred: Sym` and `args: &[Sym]`,
//! so a formula had no storage path at all — and the residual of a partially
//! evaluated query *is* a formula. The shrinking-residual design depended on
//! persisting an object the store could not represent.

use artist_logic::object::{Binding, CoreNode, ExternalRef, LiteralValue, wk};
use artist_logic::syntax::print;
use artist_logic::{ObjectGraph, ObjectId};
use artist_memory::{MemoryStore, Scope, load_expression, store_expression};
use num_bigint::BigInt;
use tempfile::TempDir;

async fn store() -> (TempDir, MemoryStore) {
    let dir = TempDir::new().expect("tempdir");
    let s = MemoryStore::open(dir.path().join("t.rocks"), Scope::Project)
        .await
        .expect("open");
    (dir, s)
}

/// Store, drop the in-memory graph entirely, reload from disk, compare printed
/// forms. Ids must be preserved or nothing composes.
async fn round_trip(s: &MemoryStore, g: &ObjectGraph, root: ObjectId) -> String {
    let before = print(g, root);
    store_expression(s, g, root).await.expect("store");
    let mut fresh = ObjectGraph::new();
    let back = load_expression(s, &mut fresh, root).await.expect("load");
    assert_eq!(back, root, "root id changed across persistence");
    let after = print(&fresh, back);
    assert_eq!(before, after, "structure changed across persistence");
    after
}

#[tokio::test]
async fn a_fact_is_an_ordinary_expression() {
    let (_d, s) = store().await;
    let mut g = ObjectGraph::new();
    let prefers = g.atom("prefers");
    let adam = g.atom("adam");
    let tabs = g.atom("tabs");
    let fact = g.apply(prefers, vec![adam, tabs]);

    let printed = round_trip(&s, &g, fact).await;
    assert_eq!(printed, "(prefers adam tabs)");
}

/// The one that was impossible before: a formula with binders, a lambda, an
/// aggregate and a quotation, all persisted through the same path as a fact.
#[tokio::test]
async fn formulas_queries_and_residuals_persist() {
    let (_d, s) = store().await;
    let mut g = ObjectGraph::new();

    let path = g.atom("Path");
    let stale = g.atom("stale");
    let corrected = g.atom("corrected");
    let adam = g.atom("adam");
    let f = g.fresh();

    // ∃f:Path. stale(f) ∧ corrected(adam, f)
    let a = g.apply(stale, vec![f]);
    let b = g.apply(corrected, vec![adam, f]);
    let conj = g.apply(wk::AND, vec![a, b]);
    let query = g.quantify(wk::EXISTS, f, Some(path), conj);
    round_trip(&s, &g, query).await;

    // λx. stale(x)
    let x = g.fresh();
    let lam_body = g.apply(stale, vec![x]);
    let lam = g.bind(wk::LAMBDA, vec![Binding { var: x, domain: Some(path) }], vec![lam_body]);
    round_trip(&s, &g, lam).await;

    // count over a refined domain
    let c = g.fresh();
    let cnt_body = g.apply(corrected, vec![adam, c]);
    let count = g.bind(wk::COUNT, vec![Binding { var: c, domain: Some(path) }], vec![cnt_body]);
    round_trip(&s, &g, count).await;

    // A quoted proposition, and an assertion about it.
    let quoted = g.apply(wk::QUOTE, vec![query]);
    let sarah = g.atom("sarah");
    let told = g.apply(wk::ASSERTED_BY, vec![quoted, sarah]);
    round_trip(&s, &g, told).await;
}

/// A residual stored on one "run" and reloaded on the next — the mechanism the
/// shrinking-residual design needs and could not have.
#[tokio::test]
async fn a_residual_survives_and_is_requeryable() {
    let (_d, s) = store().await;
    let mut g = ObjectGraph::new();
    let path = g.atom("Path");
    let done = g.atom("already-checked");
    let stale = g.atom("stale");
    let f = g.fresh();

    // ∃f : (where Path (not (already-checked f))). stale(f)
    let checked = g.apply(done, vec![f]);
    let not_checked = g.apply(wk::NOT, vec![checked]);
    let narrowed = g.apply(wk::WHERE_DOMAIN, vec![path, not_checked]);
    let body = g.apply(stale, vec![f]);
    let residual = g.quantify(wk::EXISTS, f, Some(narrowed), body);

    store_expression(&s, &g, residual).await.expect("store");
    drop(g);

    // A later process reloads it knowing only the id.
    let mut later = ObjectGraph::new();
    let back = load_expression(&s, &mut later, residual).await.expect("load");
    let text = print(&later, back);
    assert!(text.starts_with("(exists"), "still a query: {text}");
    assert!(text.contains("where"), "domain refinement survived: {text}");
    assert!(
        later.reachable(back).len() > 5,
        "the whole subgraph came back, not just the root"
    );
}

/// Cycles, external references, opaque nodes and bignums all survive the store.
#[tokio::test]
async fn cycles_externals_and_unknown_nodes_persist() {
    let (_d, s) = store().await;
    let mut g = ObjectGraph::new();

    // A self-referential proposition.
    let liar = g.alloc();
    let q = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q]);
    let neg = g.apply(wk::NOT, vec![h]);
    let node = g.get(neg).cloned().expect("built");
    g.define(liar, node);
    assert!(g.is_cyclic(liar));

    store_expression(&s, &g, liar).await.expect("store cycle");
    let mut fresh = ObjectGraph::new();
    let back = load_expression(&s, &mut fresh, liar).await.expect("load cycle");
    assert!(fresh.is_cyclic(back), "the cycle survived the database");

    // External ref, opaque node, and an integer past i64.
    let ext = g.external(ExternalRef {
        namespace: "file".into(),
        locator: b"/etc/hosts".to_vec(),
        version: None,
        digest: Some([3u8; 32]),
    });
    let opaque = g.intern(CoreNode::Opaque { tag: "future:v9".into(), payload: vec![1, 2, 3] });
    let huge = g.lit(LiteralValue::Int(
        BigInt::from(u64::MAX) * BigInt::from(u64::MAX),
    ));
    let holder = g.atom("holds-all");
    let all = g.apply(holder, vec![ext, opaque, huge]);
    round_trip(&s, &g, all).await;
}

/// Sharing is preserved: a subterm referenced many times is stored once.
#[tokio::test]
async fn sharing_is_preserved_not_expanded() {
    let (_d, s) = store().await;
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let shared = g.apply(p, vec![a]);
    let many: Vec<ObjectId> = (0..64).map(|_| shared).collect();
    let root = g.apply(wk::AND, many);

    store_expression(&s, &g, root).await.expect("store");
    let mut fresh = ObjectGraph::new();
    let back = load_expression(&s, &mut fresh, root).await.expect("load");

    let kids = fresh.children(back);
    assert_eq!(kids.len(), 65, "operator plus 64 operands");
    assert!(
        kids[1..].iter().all(|k| *k == shared),
        "every operand is the same object, stored once"
    );
    assert_eq!(
        fresh.reachable(back).len(),
        5,
        "root, the `and` operator, the shared node, p, a — five objects for a \
         64-way conjunction, so sharing survived rather than expanding"
    );
}
