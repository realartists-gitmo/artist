//! Who vouches for a belief, against the real store.
//!
//! `RelationalView::attribution` had **no test anywhere**. Its only coverage was
//! `MapGraphStructure`, an explicitly-populated map with no coreference, no
//! retraction, no belief layer and no valid time — so it could not see any of the
//! behaviour that matters, and an audit found nine defects here, eight confirmed
//! by execution.
//!
//! The specification is `docs/semantics.md` §8: attribution is a function of the
//! same data, at the same instant, under the same equivalence as the verdict.
//! Every test below is one of those three agreeing or failing to.

use artist_logic::graph_eval::Knowledge;
use artist_logic::{GraphStructure, ObjectGraph, ObjectId};
use artist_memory::MemoryStore;
use artist_memory::relational::{RelationalView, assert_stmt, intern, oid, well_known};
use tempfile::TempDir;

/// The node the evaluator hands to `attribution`: `(pred args…)`.
fn proposition(pred: artist_memory::relational::Sym, args: &[ObjectId]) -> ObjectId {
    ObjectGraph::new().apply(oid(pred), args.to_vec())
}

async fn store() -> (TempDir, MemoryStore) {
    let dir = TempDir::new().expect("tempdir");
    let s = MemoryStore::open(dir.path().join("t.rocks")).await.expect("open");
    (dir, s)
}

/// **A coreference merge must not move a belief's sources onto a name nobody
/// wrote.**
///
/// `agent-a` asserts about the full name; a bot later asserts the two names are
/// the same thing. Both spellings are then the same proposition (§8.3), so both
/// must give the same verdict *and* the same sources.
///
/// Before the fix the index was keyed only on the canonical tuple while the
/// evaluator asks with the query's own spelling: the name the agent actually
/// used came back with **no** authority and was downgraded to `assumed`, while a
/// name it had never written gained one. Which alias kept the provenance was
/// decided by `min(ObjectId)` — symbol interning order.
#[tokio::test]
async fn a_coreference_merge_preserves_sources_under_both_names() {
    let (_d, s) = store().await;
    let prefers = intern(&s, "prefers").await.expect("sym");
    let short = intern(&s, "cm").await.expect("sym");
    let full = intern(&s, "compaction-planner").await.expect("sym");
    let tabs = intern(&s, "tabs").await.expect("sym");

    assert_stmt(&s, prefers, &[full, tabs], "agent-a").await.expect("fact");
    let view = RelationalView::load(&s).await.expect("load");
    let before = view.attribution(proposition(prefers, &[oid(full), oid(tabs)]));
    assert!(!before.is_empty(), "agent-a vouches for what it wrote");

    // A pure addition: nobody has retracted anything.
    assert_stmt(&s, well_known::IS, &[short, full], "corefbot").await.expect("merge");
    let view = RelationalView::load(&s).await.expect("reload");

    for (label, args) in [
        ("the name the agent wrote", [oid(full), oid(tabs)]),
        ("the alias it was merged with", [oid(short), oid(tabs)]),
    ] {
        assert_eq!(
            view.known(oid(prefers), &args),
            Knowledge::Holds,
            "{label}: both spellings hold"
        );
        assert_eq!(
            view.attribution(proposition(prefers, &args)),
            before,
            "{label}: and both carry the same sources"
        );
    }
}

/// **A provenance name that collides with the reserved vocabulary is still its
/// own object.**
///
/// `ObjectGraph::new` pre-interns every `wk::NAMES` label and `atom` checks that
/// table first, so an origin of `"source"` returned `wk::SOURCE` — the operator.
/// A reader resolving that authority got a logical connective. §8.4 requires
/// naming to be injective.
#[tokio::test]
async fn a_source_named_like_an_operator_is_not_that_operator() {
    let (_d, s) = store().await;
    let holds = intern(&s, "holds-something").await.expect("sym");
    let thing = intern(&s, "thing").await.expect("sym");

    // "source" is a reserved operator name, and a perfectly ordinary origin.
    assert_stmt(&s, holds, &[thing], "source").await.expect("fact");
    let view = RelationalView::load(&s).await.expect("load");
    let sources = view.attribution(proposition(holds, &[oid(thing)]));

    assert_eq!(sources.len(), 1, "one origin, one source");
    assert_ne!(
        sources[0],
        artist_logic::wk::SOURCE,
        "the origin `source` is a document, not the `source` operator"
    );
    assert_eq!(
        sources[0],
        artist_logic::ObjectGraph::content_atom("source"),
        "and it is the ordinary content-addressed atom for that name"
    );
}

/// **A withdrawn claim stops vouching.**
///
/// Skipping *retraction events* is not the same as skipping retracted
/// *statements*: the original assertion is still in the log, so an agent who
/// wrote a claim and then withdrew it went on being named as its authority.
/// "Go and ask whoever told me this" pointed at someone who had told you the
/// opposite.
///
/// §8.1 makes liveness one predicate that the verdict and the attribution both
/// read, so the fold here is the same one `state_at` uses: the latest event for
/// a statement decides whether its author still stands behind it.
#[tokio::test]
async fn a_retracted_claim_stops_vouching() {
    use artist_memory::relational::retract_stmt;

    let (_d, s) = store().await;
    let fast = intern(&s, "build-is-fast").await.expect("sym");
    let proj = intern(&s, "artist").await.expect("sym");

    let id = assert_stmt(&s, fast, &[proj], "agent-a").await.expect("fact");
    let view = RelationalView::load(&s).await.expect("load");
    assert!(
        !view.attribution(proposition(fast, &[oid(proj)])).is_empty(),
        "while it stands, its author vouches for it"
    );

    retract_stmt(&s, id).await.expect("retract");
    let view = RelationalView::load(&s).await.expect("reload");
    assert!(
        view.attribution(proposition(fast, &[oid(proj)])).is_empty(),
        "withdrawn: nobody is left standing behind it"
    );

    // …and a second, independent assertion of the same fact is unaffected,
    // because the fold is per statement rather than per tuple.
    assert_stmt(&s, fast, &[proj], "agent-b").await.expect("second");
    let view = RelationalView::load(&s).await.expect("reload");
    assert_eq!(
        view.attribution(proposition(fast, &[oid(proj)])).len(),
        1,
        "agent-b vouches; agent-a still does not"
    );
}
