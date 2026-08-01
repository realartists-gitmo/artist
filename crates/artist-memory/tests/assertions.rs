//! Storing an expression is not believing it.
//!
//! The spec used to say "a stored belief and a question about it are the same
//! object". That is true of the *expression* and false of the *record*: one
//! graph serves as a queried proposition, a quoted one inside
//! `(believes sarah (quote P))`, a residual from a budget-exhausted evaluation,
//! and an asserted belief. Persisting the nodes distinguishes none of them.
//!
//! It also used to be tempting to key an assertion by its proposition. These
//! tests are why that loses data.

use artist_logic::evidence::{Assertion, Polarity};
use artist_logic::object::wk;
use artist_logic::{ObjectGraph, ObjectId};
use artist_memory::{MemoryStore, assertions_about, record_assertion, store_expression};
use tempfile::TempDir;

async fn store() -> (TempDir, MemoryStore) {
    let dir = TempDir::new().expect("tempdir");
    let s = MemoryStore::open(dir.path().join("t.rocks"))
        .await
        .expect("open");
    (dir, s)
}

fn proposition(g: &mut ObjectGraph) -> ObjectId {
    let prefers = g.atom("prefers");
    let adam = g.atom("adam");
    let tabs = g.atom("tabs");
    g.apply(prefers, vec![adam, tabs])
}

/// The case that keying on the proposition would destroy: two agents, one
/// proposition, opposite polarity. Both are real records and both must survive.
#[tokio::test]
async fn one_proposition_carries_many_independent_assertions() {
    let (_d, s) = store().await;
    let mut g = ObjectGraph::new();
    let p = proposition(&mut g);
    store_expression(&s, &g, p).await.expect("store expression");

    let sarah = g.atom("sarah");
    let adam = g.atom("adam");

    let affirmed = Assertion {
        asserting_agent: Some(adam),
        ..Assertion::affirm(p, 1_000)
    }
    .sealed();
    let denied = Assertion {
        asserting_agent: Some(sarah),
        polarity: Polarity::Deny,
        ..Assertion::affirm(p, 2_000)
    }
    .sealed();

    assert_ne!(
        affirmed.id, denied.id,
        "different agents and polarity must be different assertions"
    );

    record_assertion(&s, &affirmed).await.expect("record");
    record_assertion(&s, &denied).await.expect("record");

    let found = assertions_about(&s, p).await.expect("query");
    assert_eq!(found.len(), 2, "both assertions must survive: {found:?}");
    assert!(found.iter().any(|a| a.polarity == Polarity::Affirm));
    assert!(found.iter().any(|a| a.polarity == Polarity::Deny));
}

/// Identity is derived from every field, so re-recording the same claim is
/// idempotent while changing any field is a new record.
#[tokio::test]
async fn assertion_identity_is_derived_from_all_of_its_fields() {
    let (_d, s) = store().await;
    let mut g = ObjectGraph::new();
    let p = proposition(&mut g);
    store_expression(&s, &g, p).await.expect("store");

    let base = Assertion::affirm(p, 500);
    record_assertion(&s, &base).await.expect("record");
    record_assertion(&s, &base).await.expect("record again");
    assert_eq!(
        assertions_about(&s, p).await.expect("query").len(),
        1,
        "the same claim recorded twice is one record"
    );

    // Any field moving makes it a different claim.
    for variant in [
        Assertion { recorded_at: 501, ..base.clone() }.sealed(),
        Assertion { polarity: Polarity::Deny, ..base.clone() }.sealed(),
        Assertion { valid_from: Some(9), ..base.clone() }.sealed(),
        Assertion { world: Some(wk::TOP), ..base.clone() }.sealed(),
    ] {
        assert_ne!(variant.id, base.id, "a changed field must change identity");
        record_assertion(&s, &variant).await.expect("record variant");
    }
    assert_eq!(assertions_about(&s, p).await.expect("query").len(), 5);
}

/// The mention/use boundary: persisting a proposition — even inside a quotation
/// someone else is credited with — does not make it a belief of ours.
#[tokio::test]
async fn storing_a_proposition_does_not_assert_it() {
    let (_d, s) = store().await;
    let mut g = ObjectGraph::new();
    let p = proposition(&mut g);

    // Store the proposition, and store a *quotation* attributing it to someone.
    let quoted = g.apply(wk::QUOTE, vec![p]);
    let sarah = g.atom("sarah");
    let attributed = g.apply(wk::ASSERTED_BY, vec![quoted, sarah]);
    store_expression(&s, &g, attributed).await.expect("store");

    assert!(
        assertions_about(&s, p).await.expect("query").is_empty(),
        "an expression in the graph is not a claim about the world"
    );

    // Asserting it is a separate, explicit act.
    let a = Assertion::affirm(p, 42);
    record_assertion(&s, &a).await.expect("record");
    assert_eq!(assertions_about(&s, p).await.expect("query").len(), 1);
}

/// Valid time is carried on the assertion, so "what did I used to believe" is
/// answerable without a liveness flag standing in for history.
#[tokio::test]
async fn assertions_carry_valid_time_independently() {
    let (_d, s) = store().await;
    let mut g = ObjectGraph::new();
    let p = proposition(&mut g);
    store_expression(&s, &g, p).await.expect("store");

    let held_then = Assertion {
        valid_from: Some(100),
        valid_to: Some(200),
        ..Assertion::affirm(p, 100)
    }
    .sealed();
    let holds_now = Assertion { valid_from: Some(200), ..Assertion::affirm(p, 200) }.sealed();

    record_assertion(&s, &held_then).await.expect("record");
    record_assertion(&s, &holds_now).await.expect("record");

    let found = assertions_about(&s, p).await.expect("query");
    assert_eq!(found.len(), 2);
    assert!(found.iter().any(|a| a.valid_to == Some(200)));
    assert!(found.iter().any(|a| a.valid_to.is_none()));
}
