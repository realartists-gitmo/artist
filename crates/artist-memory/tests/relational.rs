//! Round-trip through the real store, evaluated on the universal graph.
//!
//! Sorts and closed-world status are *derived from facts*: nothing declares a
//! schema for `Person` or `Path`, both come into existence by writing `type(…)`,
//! and the negation rule for each follows from whether a `has_resolver`
//! statement exists. Supersession is valid time, so what was once believed stays
//! askable.

use artist_logic::graph_eval::Knowledge;
use artist_logic::{Evidential, GraphEvaluator, GraphStructure, ObjectGraph, wk};
use artist_memory::relational::{
    RelationalView, assert_stmt, assert_stmt_at, intern, oid, retract_stmt_at, well_known,
};
use artist_memory::{MemoryStore};
use tempfile::TempDir;

async fn store() -> (TempDir, MemoryStore) {
    let dir = TempDir::new().expect("tempdir");
    let s = MemoryStore::open(dir.path().join("t.rocks"))
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
    assert_eq!(view.extension(oid(path)).map(|v| v.members.len()), Some(2));
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
    assert_eq!(view.known(oid(wide), &lifted), Knowledge::Holds, "7-ary survives");
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
        Knowledge::Unknown,
        "two names, no asserted link"
    );

    assert_stmt(&s, well_known::IS, &[short, full], "t").await.expect("is");
    let view = RelationalView::load(&s).await.expect("reload");
    assert_eq!(
        view.known(oid(p), &[oid(adam), oid(short)]),
        Knowledge::Holds,
        "one identity statement and the fact is reachable under either name"
    );
}

/// **Units are data, against the real store.**
///
/// The decision was that `(scale minutes 60 seconds)` is an ordinary written
/// fact, so a new unit costs a write and never a recompile. No structure
/// implemented `GraphStructure::scale` — `RelationalView` inherited the empty
/// default — so writing the fact changed nothing and every cross-unit
/// comparison stalled regardless of what the store held.
#[tokio::test]
async fn a_written_scale_fact_converts_units() {
    let (_d, s) = store().await;
    let minutes = intern(&s, "minutes").await.expect("intern");
    let seconds = intern(&s, "seconds").await.expect("intern");
    let sixty = intern(&s, "60").await.expect("intern");
    assert_stmt(&s, well_known::SCALE, &[minutes, sixty, seconds], "t")
        .await
        .expect("scale");

    let view = RelationalView::load(&s).await.expect("load");
    let mut g = ObjectGraph::new();
    let (four, n240) = (g.int(4), g.int(240));
    let a = g.apply(wk::QUANTITY, vec![four, oid(minutes)]);
    let b = g.apply(wk::QUANTITY, vec![n240, oid(seconds)]);
    let same = g.apply(wk::EQ, vec![a, b]);

    let r = GraphEvaluator::new().eval(&mut g, same, &view, 50_000);
    assert_eq!(
        r.evidential(),
        Evidential::Supported,
        "4 minutes is 240 seconds, and one written fact is the whole cost"
    );
}

/// **A term can have a value**, so a function of an entity is comparable.
#[tokio::test]
async fn a_written_value_fact_gives_a_term_a_denotation() {
    let (_d, s) = store().await;
    let duration = intern(&s, "duration-of-suite").await.expect("intern");
    let four_min = intern(&s, "four-minutes").await.expect("intern");
    assert_stmt(&s, well_known::VALUE, &[duration, four_min], "t")
        .await
        .expect("value");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.value(oid(duration)),
        Some(oid(four_min)),
        "the store denotes the term it was told to"
    );
}

/// **Stored rules fire.** A rule is an expression, so it lives in the object
/// table and is unreachable from `stmt`/`arg` — which is why this hook returned
/// an empty default and every rule ever written to the store sat inert.
#[tokio::test]
async fn a_stored_rule_fires_against_the_real_store() {
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let touches = intern(&s, "touches-parser").await.expect("intern");
    let needs = intern(&s, "needs-review").await.expect("intern");
    let commit = intern(&s, "c1").await.expect("intern");
    assert_stmt(&s, touches, &[commit], "t").await.expect("fact");

    // ∀x. touches-parser(x) → needs-review(x)
    let mut g = ObjectGraph::new();
    let x = g.fresh();
    let antecedent = g.apply(oid(touches), vec![x]);
    let consequent = g.apply(oid(needs), vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![antecedent, consequent]);
    let rule = g.quantify(wk::FORALL, x, None, imp);
    store_expression(&s, &g, rule).await.expect("store rule");
    // Storing is not believing: the rule has to be *asserted* to be a rule.
    artist_memory::assertion_store::record_assertion(
        &s,
        &artist_logic::evidence::Assertion::affirm(rule, 1),
    )
    .await
    .expect("assert rule");

    let view = RelationalView::load(&s).await.expect("load");
    assert!(
        !view.rules(oid(needs)).is_empty(),
        "the rule must be indexed by the predicate it concludes"
    );

    // A **fresh** graph, which is the case that matters: this test used to
    // reuse the graph that authored the rule, so it passed while stored rules
    // fired only in the process that wrote them.
    let mut fresh = ObjectGraph::new();
    view.hydrate(&mut fresh);
    let goal = fresh.apply(oid(needs), vec![oid(commit)]);
    let r = GraphEvaluator::new().eval(&mut fresh, goal, &view, 100_000);
    assert_eq!(
        r.evidential(),
        Evidential::Supported,
        "the commit touches the parser, so the rule derives that it needs review"
    );
}

/// **Storing an expression is not believing it.** Every persisted node of the
/// right shape became a live inference rule — including a rule the agent had
/// explicitly denied, and including quoted propositions, residuals and queries
/// it had merely once asked.
#[tokio::test]
async fn an_unasserted_rule_does_not_fire() {
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let touches = intern(&s, "touches-parser").await.expect("intern");
    let needs = intern(&s, "needs-review").await.expect("intern");
    let commit = intern(&s, "c1").await.expect("intern");
    assert_stmt(&s, touches, &[commit], "t").await.expect("fact");

    let mut g = ObjectGraph::new();
    let x = g.fresh();
    let antecedent = g.apply(oid(touches), vec![x]);
    let consequent = g.apply(oid(needs), vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![antecedent, consequent]);
    let rule = g.quantify(wk::FORALL, x, None, imp);
    // Persisted, never asserted.
    store_expression(&s, &g, rule).await.expect("store");

    let view = RelationalView::load(&s).await.expect("load");
    assert!(view.rules(oid(needs)).is_empty(), "presence is not belief");

    let goal = g.apply(oid(needs), vec![oid(commit)]);
    assert_eq!(
        GraphEvaluator::new().eval(&mut g, goal, &view, 100_000).evidential(),
        Evidential::Open,
        "an expression nobody asserted must not derive anything"
    );
}

/// **Coreference reaches what is stored, not only what is asked.**
///
/// Writing one `is(a, b)` used to make every tuple recorded under `a`
/// unreachable under either name — and over a closed predicate that became a
/// confident `Refuted` for a fact still sitting in the store.
#[tokio::test]
async fn learning_a_second_name_does_not_erase_the_fact() {
    let (_d, s) = store().await;
    let trusts = intern(&s, "trusts").await.expect("intern");
    let adam = intern(&s, "adam").await.expect("intern");
    let short = intern(&s, "chinmay").await.expect("intern");
    let full = intern(&s, "chinmay-mehta").await.expect("intern");

    assert_stmt(&s, trusts, &[adam, short], "t").await.expect("fact");
    assert_stmt(&s, well_known::IS, &[short, full], "t").await.expect("is");

    let view = RelationalView::load(&s).await.expect("load");
    for name in [short, full] {
        assert_eq!(
            view.known(oid(trusts), &[oid(adam), oid(name)]),
            Knowledge::Holds,
            "the fact must survive under either name"
        );
    }
}

/// **A sort with no authority is not a complete enumeration.**
#[tokio::test]
async fn an_unclosed_sort_does_not_license_a_universal() {
    let (_d, s) = store().await;
    let person = intern(&s, "Person").await.expect("intern");
    let mortal = intern(&s, "mortal").await.expect("intern");
    let adam = intern(&s, "adam").await.expect("intern");
    assert_stmt(&s, well_known::TYPE, &[adam, person], "t").await.expect("type");
    assert_stmt(&s, mortal, &[adam], "t").await.expect("fact");

    let view = RelationalView::load(&s).await.expect("load");
    let mut g = ObjectGraph::new();
    let v = g.fresh();
    let body = g.apply(oid(mortal), vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(oid(person)), body);
    assert_ne!(
        GraphEvaluator::new().eval(&mut g, q, &view, 50_000).evidential(),
        Evidential::Supported,
        "nothing said those were all the people"
    );
}

/// **A view that models no worlds must say so.** The trait default forwards to
/// `known`, which answers about the *actual* world — so a counterfactual came
/// back with the truth about what really happened, wearing a counterfactual's
/// label.
#[tokio::test]
async fn a_counterfactual_world_is_not_silently_the_actual_one() {
    let (_d, s) = store().await;
    let green = intern(&s, "green").await.expect("intern");
    let artist = intern(&s, "artist").await.expect("intern");
    assert_stmt(&s, green, &[artist], "t").await.expect("fact");

    let view = RelationalView::load(&s).await.expect("load");
    let mut g = ObjectGraph::new();
    let claim = g.apply(oid(green), vec![oid(artist)]);
    let ev = GraphEvaluator::new();

    assert_eq!(
        ev.eval(&mut g, claim, &view, 20_000).evidential(),
        Evidential::Supported,
        "it is green here"
    );

    let elsewhere = g.atom("if-we-had-merged");
    let there = g.apply(wk::IN_WORLD, vec![elsewhere, claim]);
    assert_eq!(
        ev.eval(&mut g, there, &view, 20_000).evidential(),
        Evidential::Open,
        "…and this store knows nothing about a world it does not model"
    );
}

/// **Closure has a start date.** `has_resolver` is a valid-time statement so a
/// predicate can *acquire* an authority; reading closure at the present and
/// applying it backwards refuted claims about instants before any data existed.
#[tokio::test]
async fn an_authority_does_not_reach_back_before_it_existed() {
    let (_d, s) = store().await;
    let green = intern(&s, "green").await.expect("intern");
    let artist = intern(&s, "artist").await.expect("intern");
    assert_stmt_at(&s, green, &[artist], "t", Some(5000)).await.expect("fact");
    assert_stmt_at(&s, well_known::HAS_RESOLVER, &[green], "t", Some(5000))
        .await
        .expect("resolver");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.known_at(oid(green), &[oid(artist)], 1),
        Knowledge::Unknown,
        "no authority existed at t=1, so absence is not refutation"
    );
    assert_eq!(
        view.known_at(oid(green), &[oid(artist)], 9000),
        Knowledge::Holds,
    );
}

/// **Authority is per arity.** Relations are keyed `(pred, arity)` everywhere
/// else, so authority over `meets/2` says nothing about `meets/3`.
#[tokio::test]
async fn authority_does_not_span_arities() {
    let (_d, s) = store().await;
    let meets = intern(&s, "meets").await.expect("intern");
    let (a, b, c) = (
        intern(&s, "a").await.expect("intern"),
        intern(&s, "b").await.expect("intern"),
        intern(&s, "c").await.expect("intern"),
    );
    assert_stmt(&s, meets, &[a, b], "t").await.expect("fact");
    assert_stmt(&s, well_known::HAS_RESOLVER, &[meets], "t").await.expect("resolver");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.known(oid(meets), &[oid(a), oid(c)]),
        Knowledge::Fails,
        "arity 2 is covered"
    );
    assert_eq!(
        view.known(oid(meets), &[oid(a), oid(b), oid(c)]),
        Knowledge::Unknown,
        "…and says nothing about an arity the view has never seen"
    );
}

/// **A dated quantifier ranges over the members the sort had then.**
#[tokio::test]
async fn a_dated_universal_uses_the_membership_of_that_instant() {
    let (_d, s) = store().await;
    let path = intern(&s, "Path").await.expect("intern");
    let compiles = intern(&s, "compiles").await.expect("intern");
    let old = intern(&s, "old.rs").await.expect("intern");
    let new = intern(&s, "new.rs").await.expect("intern");

    assert_stmt_at(&s, well_known::TYPE, &[old, path], "t", Some(1000)).await.expect("t");
    assert_stmt_at(&s, compiles, &[old], "t", Some(1000)).await.expect("t");
    assert_stmt_at(&s, well_known::HAS_RESOLVER, &[compiles], "t", Some(1000)).await.expect("t");
    assert_stmt_at(&s, well_known::HAS_RESOLVER, &[path], "t", Some(1000)).await.expect("t");
    assert_stmt_at(&s, well_known::TYPE, &[new, path], "t", Some(9000)).await.expect("t");

    let view = RelationalView::load(&s).await.expect("load");
    let mut g = ObjectGraph::new();
    let v = g.fresh();
    let body = g.apply(oid(compiles), vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(oid(path)), body);
    let when = g.int(1500);
    let dated = g.apply(wk::AT, vec![when, q]);
    assert_ne!(
        GraphEvaluator::new().eval(&mut g, dated, &view, 50_000).evidential(),
        Evidential::Refuted,
        "new.rs did not exist at t=1500 and cannot be a counterexample there"
    );
}

/// **A version has to move when the store changes.** The last *valid* instant
/// does not: a backdated write leaves it untouched while the answers change.
#[tokio::test]
async fn a_backdated_write_moves_the_snapshot() {
    let (_d, s) = store().await;
    let p = intern(&s, "p").await.expect("intern");
    let (a, b) = (
        intern(&s, "a").await.expect("intern"),
        intern(&s, "b").await.expect("intern"),
    );
    assert_stmt_at(&s, p, &[a], "t", Some(9_000_000)).await.expect("late");
    let before = RelationalView::load(&s).await.expect("load").snapshot();

    assert_stmt_at(&s, p, &[b], "t", Some(1000)).await.expect("backdated");
    let after = RelationalView::load(&s).await.expect("load");
    assert_ne!(before, after.snapshot(), "the universe changed; the version must too");
    assert_eq!(after.known(oid(p), &[oid(b)]), Knowledge::Holds);
}

// ------------------------------------------------- round four, the bridge

/// **Coreference has to reach the change log.** `canonicalise_stored` rewrote
/// the present projection and not `history`, so the *dated* form refuted a fact
/// the undated form affirms — the repaired `is(a,b)` bug surviving on the
/// temporal path.
#[tokio::test]
async fn a_dated_query_sees_the_same_identities_as_an_undated_one() {
    let (_d, s) = store().await;
    let prefers = intern(&s, "prefers").await.expect("i");
    let adam = intern(&s, "adam").await.expect("i");
    let full = intern(&s, "adam-humphrey").await.expect("i");
    let tabs = intern(&s, "tabs").await.expect("i");

    assert_stmt_at(&s, well_known::HAS_RESOLVER, &[prefers], "t", Some(500)).await.expect("r");
    assert_stmt_at(&s, prefers, &[adam, tabs], "t", Some(1000)).await.expect("f");
    assert_stmt_at(&s, well_known::IS, &[adam, full], "t", Some(1500)).await.expect("is");

    let view = RelationalView::load(&s).await.expect("load");
    for name in [adam, full] {
        assert_eq!(
            view.known_at(oid(prefers), &[oid(name), oid(tabs)], 2000),
            Knowledge::Holds,
            "the dated form must not deny what the store asserts"
        );
    }
}

/// **A cycle of identities collapses to one representative.** The old chain
/// walk had no fixed point on a cycle, so `canonical(canonical(a))` differed
/// from `canonical(a)` and a stored tuple and a query landed on different names.
#[tokio::test]
async fn a_cycle_of_identities_has_one_representative() {
    let (_d, s) = store().await;
    let p = intern(&s, "p").await.expect("i");
    let (a, b, c) = (
        intern(&s, "a").await.expect("i"),
        intern(&s, "b").await.expect("i"),
        intern(&s, "c").await.expect("i"),
    );
    assert_stmt(&s, p, &[a], "t").await.expect("f");
    assert_stmt(&s, well_known::HAS_RESOLVER, &[p], "t").await.expect("r");
    for (x, y) in [(a, b), (b, c), (c, a)] {
        assert_stmt(&s, well_known::IS, &[x, y], "t").await.expect("is");
    }

    let view = RelationalView::load(&s).await.expect("load");
    for name in [a, b, c] {
        assert_eq!(
            view.known(oid(p), &[oid(name)]),
            Knowledge::Holds,
            "all three names denote one thing"
        );
    }
}

/// **A contested claim is `Conflicted`, not `Denied`.** Subtracting the denied
/// set from the affirmed one meant a claim on file both ways answered a certain
/// `false` — and since ids are content-addressed, nothing else could ever supply
/// the second polarity, so `Conflicted` was unproducible from data.
#[tokio::test]
async fn a_claim_on_file_both_ways_is_conflicted() {
    use artist_logic::evidence::{Assertion, Polarity};
    use artist_memory::assertion_store::record_assertion;
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let fmt = intern(&s, "run-cargo-fmt").await.expect("i");
    let mut g = ObjectGraph::new();
    let norm = g.apply(wk::OBLIGED, vec![oid(fmt)]);
    store_expression(&s, &g, norm).await.expect("store");

    record_assertion(&s, &Assertion::affirm(norm, 1)).await.expect("affirm");
    let mut deny = Assertion::affirm(norm, 2);
    deny.polarity = Polarity::Deny;
    record_assertion(&s, &deny.sealed()).await.expect("deny");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.known(wk::OBLIGED, &[oid(fmt)]),
        Knowledge::Conflicted,
        "one agent affirms and another denies — that is the signal, not a verdict"
    );
    let r = GraphEvaluator::new().eval(&mut g, norm, &view, 20_000);
    assert_eq!(r.evidential(), Evidential::Conflicted);
    assert!(!r.is_definite());
}

/// **An authority that was retracted does not cover the gap.** Closure was a
/// single start instant, so assert/retract/re-assert backfilled authority
/// across the hole and licensed refutation when no resolver existed.
#[tokio::test]
async fn an_authority_gap_does_not_license_refutation() {
    let (_d, s) = store().await;
    let exists = intern(&s, "exists").await.expect("i");
    let (x, y) = (intern(&s, "x").await.expect("i"), intern(&s, "y").await.expect("i"));
    assert_stmt_at(&s, exists, &[x], "t", Some(50)).await.expect("f");
    let id = assert_stmt_at(&s, well_known::HAS_RESOLVER, &[exists], "t", Some(100))
        .await
        .expect("r");
    retract_stmt_at(&s, id, Some(200)).await.expect("retract");
    assert_stmt_at(&s, well_known::HAS_RESOLVER, &[exists], "t", Some(300)).await.expect("r2");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(view.known_at(oid(exists), &[oid(y)], 150), Knowledge::Fails, "inside");
    assert_eq!(
        view.known_at(oid(exists), &[oid(y)], 250),
        Knowledge::Unknown,
        "no resolver existed in the gap"
    );
    assert_eq!(view.known_at(oid(exists), &[oid(y)], 350), Knowledge::Fails, "after");
}

/// **A norm asserted only in another world is not a current fact**, and neither
/// is one whose validity has lapsed.
#[tokio::test]
async fn a_world_local_or_lapsed_assertion_is_not_current() {
    use artist_logic::evidence::Assertion;
    use artist_memory::assertion_store::record_assertion;
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let act = intern(&s, "rebase").await.expect("i");
    let mut g = ObjectGraph::new();
    let norm = g.apply(wk::OBLIGED, vec![oid(act)]);
    store_expression(&s, &g, norm).await.expect("store");

    let mut a = Assertion::affirm(norm, 1);
    a.world = Some(g.atom("if-we-had-merged"));
    record_assertion(&s, &a.sealed()).await.expect("record");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.known(wk::OBLIGED, &[oid(act)]),
        Knowledge::Unknown,
        "a counterfactual norm is not a norm here"
    );
}

/// **The version has to cover everything the answers depend on.** Counting only
/// `stmt` rows stamped two contradictory results with the same number.
#[tokio::test]
async fn affirming_a_rule_moves_the_version() {
    use artist_logic::evidence::Assertion;
    use artist_memory::assertion_store::record_assertion;
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let touches = intern(&s, "touches-parser").await.expect("i");
    let needs = intern(&s, "needs-review").await.expect("i");
    let c1 = intern(&s, "c1").await.expect("i");
    assert_stmt(&s, touches, &[c1], "t").await.expect("f");
    let before = RelationalView::load(&s).await.expect("load").snapshot();

    let mut g = ObjectGraph::new();
    let x = g.fresh();
    let ante = g.apply(oid(touches), vec![x]);
    let cons = g.apply(oid(needs), vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![ante, cons]);
    let rule = g.quantify(wk::FORALL, x, None, imp);
    store_expression(&s, &g, rule).await.expect("store");
    record_assertion(&s, &Assertion::affirm(rule, 1)).await.expect("affirm");

    let after = RelationalView::load(&s).await.expect("load");
    assert_ne!(before, after.snapshot(), "the answers changed; the version must too");
    let goal = g.apply(oid(needs), vec![oid(c1)]);
    assert_eq!(
        GraphEvaluator::new().eval(&mut g, goal, &after, 100_000).evidential(),
        Evidential::Supported
    );
}

/// **The reserved vocabulary must be reachable under a dated query too.**
/// `known_at` had no reserved branch — it went straight to `desym`, which is
/// `None` for every `wk::` id — so `same-as`, `prefer`, `usually`, the norms and
/// the intensional relations were `Unknown` at *every* instant, including the
/// one they were recorded at.
#[tokio::test]
async fn a_norm_is_reachable_under_a_dated_query() {
    use artist_logic::evidence::Assertion;
    use artist_memory::assertion_store::record_assertion;
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let act = intern(&s, "run-fmt").await.expect("i");
    let mut g = ObjectGraph::new();
    let norm = g.apply(wk::OBLIGED, vec![oid(act)]);
    store_expression(&s, &g, norm).await.expect("store");

    // On file from t = 1000 until t = 2000.
    let mut a = Assertion::affirm(norm, 1);
    a.valid_from = Some(1000);
    a.valid_to = Some(2000);
    record_assertion(&s, &a.sealed()).await.expect("record");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.known_at(wk::OBLIGED, &[oid(act)], 1500),
        Knowledge::Holds,
        "inside its validity the norm is on file"
    );
    for t in [500, 2500] {
        assert_eq!(
            view.known_at(wk::OBLIGED, &[oid(act)], t),
            Knowledge::Unknown,
            "outside it, the store has nothing to say"
        );
    }
    assert_eq!(
        view.known(wk::OBLIGED, &[oid(act)]),
        Knowledge::Unknown,
        "and it has lapsed in the present"
    );
}

// ------------------------------------------------- round five, the bridge

/// **One assertion row, one claim.** Keying beliefs by *proposition* id
/// discarded the multiplicity the `assertion` relation exists to preserve: at
/// most two claims survived per proposition, carrying whichever interval the
/// last row happened to have. An affirmation over `[1000,2000)` and a denial
/// over `[3000,4000)` — which never co-exist — merged to `Conflicted` at 1500.
#[tokio::test]
async fn claims_that_never_coexist_do_not_conflict() {
    use artist_logic::evidence::{Assertion, Polarity};
    use artist_memory::assertion_store::record_assertion;
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let act = intern(&s, "rebase").await.expect("i");
    let mut g = ObjectGraph::new();
    let norm = g.apply(wk::OBLIGED, vec![oid(act)]);
    store_expression(&s, &g, norm).await.expect("store");

    let mut early = Assertion::affirm(norm, 1);
    early.valid_from = Some(1000);
    early.valid_to = Some(2000);
    record_assertion(&s, &early.sealed()).await.expect("affirm");

    let mut late = Assertion::affirm(norm, 2);
    late.polarity = Polarity::Deny;
    late.valid_from = Some(3000);
    late.valid_to = Some(4000);
    record_assertion(&s, &late.sealed()).await.expect("deny");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.known_at(wk::OBLIGED, &[oid(act)], 1500),
        Knowledge::Holds,
        "the affirmation is unopposed at 1500"
    );
    assert_eq!(
        view.known_at(wk::OBLIGED, &[oid(act)], 3500),
        Knowledge::Denied,
        "and the denial is unopposed at 3500"
    );
}

/// **A rule not yet in force does not fire.** The lapse filter checked
/// `valid_to` only, so a rule affirmed to start next century derived today —
/// while a *claim* with the same interval was correctly `Unknown`, because
/// `ReservedClaim::covers` tests both bounds.
#[tokio::test]
async fn a_rule_not_yet_in_force_does_not_fire() {
    use artist_logic::evidence::Assertion;
    use artist_memory::assertion_store::record_assertion;
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let touches = intern(&s, "touches").await.expect("i");
    let needs = intern(&s, "needs-review").await.expect("i");
    let c1 = intern(&s, "c1").await.expect("i");
    assert_stmt(&s, touches, &[c1], "t").await.expect("f");

    let mut g = ObjectGraph::new();
    let x = g.fresh();
    let ante = g.apply(oid(touches), vec![x]);
    let cons = g.apply(oid(needs), vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![ante, cons]);
    let rule = g.quantify(wk::FORALL, x, None, imp);
    store_expression(&s, &g, rule).await.expect("store");

    let mut a = Assertion::affirm(rule, 1);
    a.valid_from = Some(i64::MAX - 1); // in force in the far future only
    record_assertion(&s, &a.sealed()).await.expect("affirm");

    let view = RelationalView::load(&s).await.expect("load");
    assert!(view.rules(oid(needs)).is_empty(), "not yet in force is not in force");
    let goal = g.apply(oid(needs), vec![oid(c1)]);
    assert_eq!(
        GraphEvaluator::new().eval(&mut g, goal, &view, 100_000).evidential(),
        Evidential::Open
    );
}

/// **A merged sort keeps every member.** Re-keying `sorts` through `canonical`
/// into a `BTreeMap` dropped whichever of two merged names collided, and then
/// reported the truncated membership as `complete` — so a universal came back
/// `Supported` in a view that affirmed the missing member's sort and refuted the
/// property of it.
#[tokio::test]
async fn merging_two_sort_names_keeps_both_memberships() {
    let (_d, s) = store().await;
    let person = intern(&s, "Person").await.expect("i");
    let human = intern(&s, "Human").await.expect("i");
    let (alice, bob) = (intern(&s, "alice").await.expect("i"), intern(&s, "bob").await.expect("i"));
    let mortal = intern(&s, "mortal").await.expect("i");

    assert_stmt(&s, well_known::TYPE, &[alice, person], "t").await.expect("t");
    assert_stmt(&s, well_known::TYPE, &[bob, human], "t").await.expect("t");
    assert_stmt(&s, well_known::IS, &[human, person], "t").await.expect("is");
    assert_stmt(&s, well_known::HAS_RESOLVER, &[person], "t").await.expect("r");
    assert_stmt(&s, mortal, &[bob], "t").await.expect("f");
    assert_stmt(&s, well_known::HAS_RESOLVER, &[mortal], "t").await.expect("r");

    let view = RelationalView::load(&s).await.expect("load");
    let mut g = ObjectGraph::new();
    let v = g.fresh();
    let body = g.apply(oid(mortal), vec![v]);
    let q = g.quantify(wk::FORALL, v, Some(oid(person)), body);
    assert_eq!(
        GraphEvaluator::new().eval(&mut g, q, &view, 50_000).evidential(),
        Evidential::Refuted,
        "alice is a Person and is not mortal — she cannot be dropped from the sort"
    );
}

/// **`known` and `known_at(now)` must agree.** The present was folded out of the
/// log *before* coreference was applied to it, so a retraction of one alias and
/// an assertion of another were two tuples to one fold and one tuple to the
/// other.
#[tokio::test]
async fn the_present_and_the_log_agree_about_now() {
    let (_d, s) = store().await;
    let p = intern(&s, "p").await.expect("i");
    let (a, b) = (intern(&s, "a").await.expect("i"), intern(&s, "b").await.expect("i"));

    assert_stmt_at(&s, well_known::HAS_RESOLVER, &[p], "t", Some(500)).await.expect("r");
    assert_stmt_at(&s, p, &[a], "t", Some(1000)).await.expect("f");
    let id = assert_stmt_at(&s, p, &[b], "t", Some(1500)).await.expect("f");
    retract_stmt_at(&s, id, Some(2000)).await.expect("retract");
    assert_stmt_at(&s, well_known::IS, &[a, b], "t", Some(600)).await.expect("is");

    let view = RelationalView::load(&s).await.expect("load");
    for name in [a, b] {
        assert_eq!(
            view.known(oid(p), &[oid(name)]),
            view.known_at(oid(p), &[oid(name)], 2000),
            "one snapshot cannot hold two opinions about its own present"
        );
    }
}

/// **A view answers the same way twice.** The present-tense reserved lookup read
/// the wall clock, so one immutable view gave two answers under one `snapshot`.
#[tokio::test]
async fn a_view_is_not_a_live_cursor() {
    use artist_logic::evidence::Assertion;
    use artist_memory::assertion_store::record_assertion;
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let act = intern(&s, "fmt").await.expect("i");
    let mut g = ObjectGraph::new();
    let norm = g.apply(wk::OBLIGED, vec![oid(act)]);
    store_expression(&s, &g, norm).await.expect("store");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_micros() as i64;
    let mut a = Assertion::affirm(norm, 1);
    a.valid_to = Some(now + 300_000); // lapses 300ms from now
    record_assertion(&s, &a.sealed()).await.expect("affirm");

    let view = RelationalView::load(&s).await.expect("load");
    let first = view.known(wk::OBLIGED, &[oid(act)]);
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert_eq!(
        first,
        view.known(wk::OBLIGED, &[oid(act)]),
        "the same view must not answer differently as the clock moves"
    );
}

/// **Two rows denoting one term differently is a contradiction**, not a race
/// won by interning order. `scale` had this guard; `value` did not.
#[tokio::test]
async fn conflicting_denotations_decide_nothing() {
    let (_d, s) = store().await;
    let term = intern(&s, "duration-of-suite").await.expect("i");
    let (four, nine) = (intern(&s, "four").await.expect("i"), intern(&s, "nine").await.expect("i"));
    assert_stmt(&s, well_known::VALUE, &[term, four], "t").await.expect("v");
    assert_stmt(&s, well_known::VALUE, &[term, nine], "t").await.expect("v");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(view.value(oid(term)), None, "the store holds two answers, so it has none");
}

/// **Sharpness is data.** A predicate declared borderline blocks certified
/// classical inference about it, while an ordinary query still answers.
#[tokio::test]
async fn declared_indeterminacy_reaches_the_evaluator() {
    use artist_logic::evidence::Determinacy;

    let (_d, s) = store().await;
    let heap = intern(&s, "heap").await.expect("i");
    let n = intern(&s, "4783-grains").await.expect("i");

    let mut g = ObjectGraph::new();
    let claim = g.apply(oid(heap), vec![oid(n)]);
    artist_memory::graph_store::store_expression(&s, &g, claim).await.expect("store");
    assert_stmt(&s, well_known::INDETERMINATE, &[], "t").await.ok();

    // Declared over the *proposition*, not the predicate — sharpness is
    // interpretation- and region-relative.
    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.determinacy(claim),
        Determinacy::Unknown,
        "nothing declared yet: presumed for queries, refused for proof"
    );

    let ev = GraphEvaluator::new();
    let nh = g.apply(wk::NOT, vec![claim]);
    let lem = g.apply(wk::OR, vec![claim, nh]);
    assert_eq!(
        ev.eval(&mut g, lem, &view, 20_000).evidential(),
        Evidential::Supported,
        "an ordinary query may presume totality"
    );
    assert_ne!(
        ev.certify(&mut g, lem, &view, 20_000).evidential(),
        Evidential::Supported,
        "…and certification may not, since totality was never established"
    );
}

/// **A name can have several aliases.** `same` held one out-edge per node, so a
/// second `is(a, ·)` row overwrote the first — leaving two names the store
/// declares identical with opposite answers, one of them a confident refutation
/// of a fact the store affirms under the other name.
#[tokio::test]
async fn a_name_with_two_aliases_keeps_both() {
    let (_d, s) = store().await;
    let prefers = intern(&s, "prefers").await.expect("i");
    let nick = intern(&s, "nick").await.expect("i");
    let nicholas = intern(&s, "nicholas").await.expect("i");
    let nicky = intern(&s, "nicky").await.expect("i");

    assert_stmt(&s, well_known::IS, &[nick, nicholas], "t").await.expect("is");
    assert_stmt(&s, well_known::IS, &[nick, nicky], "t").await.expect("is");
    assert_stmt(&s, prefers, &[nicky], "t").await.expect("f");
    assert_stmt(&s, well_known::HAS_RESOLVER, &[prefers], "t").await.expect("r");

    let view = RelationalView::load(&s).await.expect("load");
    for name in [nick, nicholas, nicky] {
        assert_eq!(
            view.known(oid(prefers), &[oid(name)]),
            Knowledge::Holds,
            "all three names denote one thing"
        );
    }
}

/// **Determinacy has to be reachable.** Statement arguments are storage symbols
/// (`IdSpace::WellKnown`); the propositions the evaluator asks about are
/// content ids. The two bands are disjoint, so a declaration written as a `stmt`
/// row could never match — the hook was inert and everything stayed `Unknown`.
#[tokio::test]
async fn a_declared_indeterminacy_reaches_a_query() {
    use artist_logic::evidence::{Assertion, Determinacy};
    use artist_memory::assertion_store::record_assertion;
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let heap = intern(&s, "heap").await.expect("i");
    let n = intern(&s, "grains-4783").await.expect("i");

    let mut g = ObjectGraph::new();
    let claim = g.apply(oid(heap), vec![oid(n)]);
    let decl = g.apply(wk::INDETERMINATE, vec![claim]);
    store_expression(&s, &g, decl).await.expect("store");
    record_assertion(&s, &Assertion::affirm(decl, 1)).await.expect("affirm");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.determinacy(claim),
        Determinacy::Indeterminate,
        "the declaration must reach the proposition it is about"
    );

    let ev = GraphEvaluator::new();
    let nh = g.apply(wk::NOT, vec![claim]);
    let lem = g.apply(wk::OR, vec![claim, nh]);
    assert_ne!(
        ev.certify(&mut g, lem, &view, 20_000).evidential(),
        Evidential::Supported,
        "and must block certified excluded middle over it"
    );
}

/// **Retraction is per statement, not per fact.** Folding the change log by
/// tuple could not tell "this statement was withdrawn" from "this fact no longer
/// holds", so two independent statements of one fact and a retraction of either
/// killed both — a confident `Fails` for a row still live in the same snapshot.
#[tokio::test]
async fn retracting_one_statement_leaves_the_other_standing() {
    let (_d, s) = store().await;
    let p = intern(&s, "p").await.expect("i");
    let a = intern(&s, "a").await.expect("i");
    assert_stmt(&s, well_known::HAS_RESOLVER, &[p], "t").await.expect("r");

    let _first = assert_stmt_at(&s, p, &[a], "t", Some(1000)).await.expect("A");
    let second = assert_stmt_at(&s, p, &[a], "t", Some(1500)).await.expect("B");
    retract_stmt_at(&s, second, Some(2000)).await.expect("retract B");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(
        view.known(oid(p), &[oid(a)]),
        Knowledge::Holds,
        "statement A is live and unretracted"
    );
}

/// **A rule is in force over an interval, and selection has to be dated.**
/// Choosing rules at `now` let one affirmed to start next century derive about
/// the past, and stopped one in force at the queried instant from answering.
#[tokio::test]
async fn rule_selection_is_dated() {
    use artist_logic::evidence::Assertion;
    use artist_memory::assertion_store::record_assertion;
    use artist_memory::graph_store::store_expression;

    let (_d, s) = store().await;
    let touches = intern(&s, "touches").await.expect("i");
    let needs = intern(&s, "needs-review").await.expect("i");
    let c1 = intern(&s, "c1").await.expect("i");
    assert_stmt_at(&s, touches, &[c1], "t", Some(500)).await.expect("f");

    let mut g = ObjectGraph::new();
    let x = g.fresh();
    let ante = g.apply(oid(touches), vec![x]);
    let cons = g.apply(oid(needs), vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![ante, cons]);
    let rule = g.quantify(wk::FORALL, x, None, imp);
    store_expression(&s, &g, rule).await.expect("store");

    // Believed only over [1000, 2000).
    let mut a = Assertion::affirm(rule, 1);
    a.valid_from = Some(1000);
    a.valid_to = Some(2000);
    record_assertion(&s, &a.sealed()).await.expect("affirm");

    let view = RelationalView::load(&s).await.expect("load");
    assert_eq!(view.rules_at(oid(needs), 1500).len(), 1, "in force at 1500");
    assert!(view.rules_at(oid(needs), 500).is_empty(), "not yet believed at 500");
    assert!(view.rules_at(oid(needs), 3000).is_empty(), "no longer believed at 3000");
}
