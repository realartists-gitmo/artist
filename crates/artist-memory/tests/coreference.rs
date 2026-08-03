//! `=` versus `same-as`, and where substitution stops.
//!
//! They are different relations and conflating them is a soundness bug in both
//! directions.
//!
//! * `=` is **computational value equality**: `(= (+ 2 2) 4)`. It is decided,
//!   not asserted, and nothing revises it.
//! * `same-as` is **defeasible coreference**: a claim that two names denote one
//!   thing. It is asserted, can be wrong, can be retracted, and can be
//!   contradicted. It is exactly as trustworthy as the evidence behind it.
//!
//! Two consequences this file pins down. Identity is applied as a *view*, never
//! by rewriting stored ids — a retraction has to be able to put the world back.
//! And substitution stops at intensional contexts: knowing the morning star is
//! the evening star does not license rewriting *what someone believes* about
//! one into a belief about the other.

use artist_logic::graph_eval::Knowledge;
use artist_logic::{GraphStructure, ObjectGraph, ObjectId, wk};
use artist_memory::MemoryStore;
use artist_memory::relational::{RelationalView, assert_stmt, intern, oid, well_known};
use tempfile::TempDir;

async fn store() -> (TempDir, MemoryStore) {
    let dir = TempDir::new().expect("tempdir");
    let s = MemoryStore::open(dir.path().join("t.rocks"))
        .await
        .expect("open");
    (dir, s)
}

/// An asserted identity makes two names interchangeable in *extensional*
/// position — which is the whole point of recording it.
#[tokio::test]
async fn same_as_licenses_substitution_in_extensional_position() {
    let (_d, s) = store().await;
    let prefers = intern(&s, "prefers").await.expect("sym");
    let short = intern(&s, "cm").await.expect("sym");
    let full = intern(&s, "compaction-planner").await.expect("sym");
    let tabs = intern(&s, "tabs").await.expect("sym");

    assert_stmt(&s, prefers, &[full, tabs], "t")
        .await
        .expect("fact");

    // Before the identity, the short name is simply not known to prefer tabs.
    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.known(oid(prefers), &[oid(short), oid(tabs)]),
        Knowledge::Unknown,
        "no identity asserted yet, so nothing is known — and crucially not refuted"
    );

    assert_stmt(&s, well_known::IS, &[short, full], "t")
        .await
        .expect("is");
    let view = RelationalView::load(&s).await.expect("reload");
    assert_eq!(
        view.known(oid(prefers), &[oid(short), oid(tabs)]),
        Knowledge::Holds,
        "an asserted identity must make the names interchangeable"
    );
}

/// Identity is a *view* over the store, not a rewrite of it. The original
/// symbols survive, which is what makes retraction possible at all — a
/// destructive merge would have thrown away the information needed to undo it.
#[tokio::test]
async fn identity_never_destroys_the_original_terms() {
    let (_d, s) = store().await;
    let prefers = intern(&s, "prefers").await.expect("sym");
    let short = intern(&s, "cm").await.expect("sym");
    let full = intern(&s, "compaction-planner").await.expect("sym");
    let tabs = intern(&s, "tabs").await.expect("sym");

    assert_stmt(&s, prefers, &[full, tabs], "t")
        .await
        .expect("fact");
    assert_stmt(&s, well_known::IS, &[short, full], "t")
        .await
        .expect("is");

    let view = RelationalView::load(&s).await.expect("load");
    // Both names still resolve to something nameable; neither was overwritten.
    assert!(
        view.name(short).is_some(),
        "the aliased symbol still exists"
    );
    assert!(view.name(full).is_some(), "the representative still exists");
    assert_ne!(
        short, full,
        "distinct symbols must stay distinct in storage"
    );
}

/// The boundary: substitution must not cross into a quotation.
///
/// `(quote (p a))` mentions the expression rather than using it. Even given
/// `same-as(a, b)`, `(quote (p a))` and `(quote (p b))` are different objects —
/// they are different *expressions*, and a claim about one is not a claim about
/// the other. Rewriting inside a quotation is how "Lois believes Superman
/// flies" silently becomes "Lois believes Clark Kent flies".
#[tokio::test]
async fn substitution_does_not_cross_into_quotation() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let a = g.atom("a");
    let b = g.atom("b");

    let pa = g.apply(p, vec![a]);
    let pb = g.apply(p, vec![b]);
    let quoted_a = g.apply(wk::QUOTE, vec![pa]);
    let quoted_b = g.apply(wk::QUOTE, vec![pb]);

    assert_ne!(
        quoted_a, quoted_b,
        "quoted propositions differ as expressions regardless of what is coreferent"
    );

    // The same holds one level up, inside an attitude report.
    let lois = g.atom("lois");
    let believes_a = g.apply(wk::BELIEVES, vec![lois, quoted_a]);
    let believes_b = g.apply(wk::BELIEVES, vec![lois, quoted_b]);
    assert_ne!(
        believes_a, believes_b,
        "an attitude report is about the expression, not only its denotation"
    );
}

/// `=` is decided by computation; `same-as` is asserted. They must not be the
/// same operator, and the well-known vocabulary keeps them apart.
#[test]
fn equality_and_coreference_are_different_operators() {
    assert_ne!(wk::EQ, wk::SAME_AS);
    let mut g = ObjectGraph::new();
    let (a, b): (ObjectId, ObjectId) = (g.atom("a"), g.atom("b"));
    let equality = g.apply(wk::EQ, vec![a, b]);
    let coreference = g.apply(wk::SAME_AS, vec![a, b]);
    assert_ne!(
        equality, coreference,
        "`(= a b)` and `(same-as a b)` are different claims and must not intern together"
    );
}
