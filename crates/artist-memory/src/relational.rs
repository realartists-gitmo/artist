//! The bridge from the fact store to [`artist_logic::Structure`].
//!
//! Evaluation runs against a **snapshot**, not a live cursor. Two reasons, and
//! both are load-bearing rather than convenient:
//!
//! * Model checking is defined against a *fixed* structure. A store that shifts
//!   mid-query would make `must` and `may` bounds over different worlds, and the
//!   soundness argument for early stopping evaporates.
//! * The evaluator's inner loop touches predicates thousands of times per query.
//!   Per-atom async I/O would dominate everything the budget is trying to
//!   control.
//!
//! Loading is one pass over the live statements, which is also why the schema
//! keeps `stmt`/`arg` narrow.

use anyhow::{Result, anyhow};
use artist_logic::{GraphStructure, ObjectGraph, ObjectId, wk};
use artist_logic::graph_eval::{Extension, Knowledge};

/// Storage-level symbol id. Lifted into the universal identity space at the
/// boundary, so the store stays compact while the *language* keeps one
/// 128-bit namespace for entities, predicates, formulas and everything else.
pub type Sym = u32;

/// Lift a storage symbol into an object id, clear of the reserved operators.
pub fn oid(s: Sym) -> ObjectId {
    ObjectId(wk::FIRST_FREE.0 + s as u128)
}

/// Lower an object id back to a storage symbol, when it is one.
pub fn desym(o: ObjectId) -> Option<Sym> {
    o.0.checked_sub(wk::FIRST_FREE.0).and_then(|v| u32::try_from(v).ok())
}
use cozo::{DataValue, NamedRows};
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::{BTreeMap, BTreeSet};

use crate::store::MemoryStore;

/// Well-known predicates. Everything else is interned from the store.
pub mod well_known {
    use super::Sym;
    /// `type(x, S)` — sort membership, an ordinary statement.
    pub const TYPE: Sym = 16;
    /// `has_resolver(S)` — whether `S` has an authority, and therefore whether
    /// negation over it is refutation or merely not-known.
    pub const HAS_RESOLVER: Sym = 17;
    /// The sort of sorts.
    pub const SORT: Sym = 18;
    /// `is(a, b)` — an asserted identity. Not a judgement the logic makes; a
    /// proposition like any other, which the view then applies so that equality
    /// stops being merely syntactic.
    pub const IS: Sym = 19;
    /// `scale(unit, factor, base)` — a unit conversion, as an ordinary fact.
    ///
    /// This is the whole of "units are data". `wk::SCALE` cannot be used as the
    /// predicate here: `desym` maps the *storage* symbol space, and every
    /// well-known operator id is below `FIRST_FREE`, so `known(wk::SCALE, …)`
    /// could never match a stored row. The evaluator asks through
    /// `GraphStructure::scale`, and this is what answers it.
    pub const SCALE: Sym = 20;
    /// `value(term, v)` — what a term denotes. "The test suite takes 4 minutes"
    /// is `value(duration(test-suite), quantity(4, minutes))`.
    pub const VALUE: Sym = 21;
    /// `indeterminate(proposition)` — declared to have no sharp satisfaction
    /// condition. Sorites predicates: `heap` at forty-seven hundred grains.
    ///
    /// Sharpness is not a property of a bare predicate. It is a property of a
    /// predicate **under an interpretation**, often only over a region of the
    /// argument space, which is why this is keyed by the whole proposition and
    /// why it lives here — in the storage layer — rather than in the logic
    /// kernel. `artist-logic` knows only that it can *ask*; how the answer is
    /// recorded, argued with and retracted is the memory's business.
    pub const INDETERMINATE: Sym = 22;
    /// `total(proposition)` — established sharp, under this interpretation.
    /// Required for certified classical inference; presumption is not enough.
    pub const TOTAL: Sym = 23;
    pub const FIRST_INTERNED: Sym = 32;

    pub const NAMES: &[(Sym, &str)] = &[
        (TYPE, "type"),
        (HAS_RESOLVER, "has_resolver"),
        (SORT, "Sort"),
        (IS, "is"),
        (SCALE, "scale"),
        (VALUE, "value"),
        (INDETERMINATE, "indeterminate"),
        (TOTAL, "total"),
    ];
}

/// Propositions the store actually **affirms**.
///
/// The `assertion` relation is the belief layer; `object` is only storage.
/// Reading the second without the first is how a denied rule fired.
/// One row of the belief layer.
///
/// Keyed by the **assertion**, not by the proposition. The `assertion` relation
/// is deliberately keyed by its own derived id so that one proposition can
/// carry many rows — different agents, different worlds, different intervals,
/// opposite polarity — and reading it into a set of *proposition* ids discarded
/// exactly the multiplicity the storage layer exists to preserve. At most two
/// claims survived per proposition, with whichever interval the last row
/// happened to carry, so an affirmation over `[1000,2000)` and a denial over
/// `[3000,4000)` — which never co-exist — merged to `Conflicted` at 1500.
/// A fabricated conflict is worse than a missed one: it is the signal the agent
/// is told to go and read.
#[derive(Clone, Copy, Debug)]
struct BeliefRow {
    proposition: ObjectId,
    affirmed: bool,
    valid_from: Option<i64>,
    valid_to: Option<i64>,
    /// Who asserted it. The column has been in `assertion` since the schema was
    /// written and nothing read it, which is why a certificate over a stored
    /// claim could name no one.
    agent: Option<ObjectId>,
}

/// Every actual-world assertion row, one entry each.
async fn belief_rows(store: &MemoryStore) -> Result<Vec<BeliefRow>> {
    let rows = store
        .script(
            "?[prop_hi, prop_lo, affirmed, world_hi, valid_from, valid_to, agent_hi, agent_lo] := \
              *assertion{prop_hi, prop_lo, affirmed, world_hi, valid_from, valid_to, \
                         agent_hi, agent_lo}",
            BTreeMap::new(),
            false,
        )
        .await?;
    let mut out = Vec::new();
    for r in &rows.rows {
        // An assertion made in another world is not a fact about this one.
        // `known_in` answers `Unknown` by design, so a world-local claim reached
        // the actual world only through this path.
        if !matches!(r.get(3), Some(DataValue::Null) | None) {
            continue;
        }
        let valid_from = r.get(4).and_then(DataValue::get_int);
        let valid_to = r.get(5).and_then(DataValue::get_int);
        // An inverted interval covers no instant. Dropping it here says so once,
        // rather than leaving a claim that is stored, counted, and permanently
        // unanswerable for reasons nothing records.
        if let (Some(f), Some(t)) = (valid_from, valid_to)
            && f >= t
        {
            continue;
        }
        out.push(BeliefRow {
            proposition: ObjectId(
                ((int_at(r, 0)? as u64 as u128) << 64) | int_at(r, 1)? as u64 as u128,
            ),
            affirmed: matches!(r.get(2), Some(DataValue::Bool(true))),
            valid_from,
            valid_to,
            agent: match (
                r.get(6).and_then(DataValue::get_int),
                r.get(7).and_then(DataValue::get_int),
            ) {
                (Some(hi), Some(lo)) => {
                    Some(ObjectId(((hi as u64 as u128) << 64) | lo as u64 as u128))
                }
                _ => None,
            },
        });
    }
    Ok(out)
}

/// One assertion or retraction of a tuple: `(instant, is_assert, tuple)`.
/// `(instant, is_assert, statement id, tuple)`.
///
/// The statement id is what makes retraction *per statement*. Folding by tuple
/// alone could not tell "this statement was withdrawn" from "this fact no longer
/// holds", so two independent statements of one fact and a retraction of either
/// killed both — and over a closed predicate that is a confident `Fails` for a
/// row still live in the same snapshot. `schema.rs` states the opposite policy
/// for observations in as many words: collapsing independent sources destroys
/// the signal that makes agreement worth anything.
/// The trailing slot is the statement's **origin**, as an object: the session
/// that wrote it, or the named provenance in the `origin` column. Carried on the
/// event rather than looked up beside it because coreference rewrites tuples,
/// and a provenance map keyed by the pre-canonical tuple silently stops matching
/// the moment two names are merged.
type StatementEvent = (i64, bool, i64, Vec<ObjectId>, Option<ObjectId>);

/// An application of the reserved vocabulary: the operator and its arguments,
/// canonicalised through the positional extensionality mask.
type ReservedKey = (ObjectId, Vec<ObjectId>);

// A `in_force(&BeliefRow, t)` helper used to live here, and it is gone rather
// than left dead: the load-time interval filter it served was removed when rule
// selection became dated per query, and a predicate that still compiles but is
// called from nowhere is how the last round's `absorb` went unnoticed.
// `ReservedClaim::covers` and `rules_at` do this per query now.

/// One affirmed or denied application of the reserved vocabulary, with the
/// interval over which it holds.
#[derive(Clone, Copy, Debug)]
struct ReservedClaim {
    knowledge: Knowledge,
    valid_from: Option<i64>,
    valid_to: Option<i64>,
}

impl ReservedClaim {
    /// Half-open, so a claim retracted at `to` covers `[from, to)`.
    fn covers(&self, t: i64) -> bool {
        self.valid_from.is_none_or(|f| f <= t) && self.valid_to.is_none_or(|e| t < e)
    }
}

/// An immutable view of the store's relational layer.
#[derive(Clone, Debug, Default)]
pub struct RelationalView {
    by_name: BTreeMap<String, Sym>,
    by_sym: BTreeMap<Sym, String>,
    rels: BTreeMap<(Sym, usize), BTreeSet<Vec<ObjectId>>>,
    sorts: BTreeMap<Sym, Vec<ObjectId>>,
    closed: BTreeSet<Sym>,
    /// When each predicate *acquired* an authority. `has_resolver` is written as
    /// a valid-time statement precisely so a sort can gain one, and reading
    /// closure at the present and applying it to all of time made
    /// `(at 1 (green artist))` a confident `Refuted` at an instant before any
    /// data existed at all.
    closed_spans: BTreeMap<Sym, Vec<(i64, i64)>>,
    /// Predicates the view knowingly dropped rows for. A torn statement is
    /// skipped rather than evaluated at the wrong arity — and the predicate then
    /// stayed *closed*, so partial-write corruption was laundered into a
    /// confident refutation. Knowing you lost data is knowing you are not
    /// authoritative.
    torn: BTreeSet<Sym>,
    same: BTreeMap<ObjectId, ObjectId>,
    /// The change log behind `at`, `always`, `eventually` and `since`.
    history: BTreeMap<(Sym, usize), Vec<StatementEvent>>,
    instants: Vec<i64>,
    /// Stored `forall`-implications, indexed by the predicate they conclude.
    ///
    /// Rules live in the `object` table, not in `stmt`/`arg` — a formula is not
    /// a tuple — so this hook returned an empty default and **stored rules never
    /// fired against the real store**, however many were written. Everything
    /// §5.2 says about derivation was true of the test structures only.
    rules: BTreeMap<ObjectId, Vec<ObjectId>>,
    /// When each rule is in force. Kept apart from `rules` so selection can be
    /// dated without re-reading the belief layer on every query.
    rule_validity: BTreeMap<ObjectId, (Option<i64>, Option<i64>)>,
    /// What a term denotes.
    values: BTreeMap<ObjectId, ObjectId>,
    /// `unit → (factor, base)`, read off `scale` facts.
    scales: BTreeMap<ObjectId, (num_bigint::BigInt, ObjectId)>,
    /// Units the store holds contradictory conversions for. A contradiction is
    /// an absence of knowledge, never an exact answer picked by id order.
    conflicting_scales: BTreeSet<ObjectId>,
    /// Terms the store denotes two different ways. Same reasoning.
    conflicting_values: BTreeSet<ObjectId>,
    /// The graph the rule expressions were loaded into, so a caller can define
    /// them into its own.
    rule_graph: ObjectGraph,
    /// Statements, arguments, objects and belief rows loaded. Used as the
    /// universe version — see `snapshot`.
    stmt_count: u64,
    /// This view's present instant. Fixed at load, so every accessor agrees.
    now: i64,
    /// Affirmed and denied applications of the **reserved** vocabulary.
    ///
    /// `stmt`/`arg` cannot hold these. Its arguments are storage symbols, and
    /// a default, a norm or an attribution takes a *proposition* — so there is
    /// no row shape for `(usually P)`. And `desym` maps only the storage symbol
    /// space, every `wk::` id being below `FIRST_FREE`, so
    /// `known(wk::USUALLY, …)` could not have matched a row even if one
    /// existed. The consequence was that `same-as`, `prefer`, `usually`'s
    /// default check, all four norms, `before`'s stored-order fallback and the
    /// seven intensional relations were **permanently `Unknown`** against the
    /// real store: everything §4's norm paragraph, §5.4 and §7 claim was true
    /// of the in-memory test structure only.
    ///
    /// The belief layer is the right home for them, because that is what they
    /// are — claims *about* propositions, keyed by proposition id, which is
    /// exactly what the `assertion` relation stores.
    /// Each claim keeps its **validity interval**, so a dated query can be
    /// answered rather than filtered at load. `known_at` had no reserved branch
    /// at all — it went straight to `desym`, which is `None` for every `wk::`
    /// id — so the whole reserved vocabulary was `Unknown` under any instant,
    /// including the one it was recorded at. The belief layer carries
    /// `valid_from`/`valid_to` and the dated accessor could not reach them.
    reserved: BTreeMap<ReservedKey, Vec<ReservedClaim>>,
    /// Who vouches for each proposition, keyed by the proposition's node id.
    ///
    /// Every ingredient was already on disk — `assertion.agent_hi/lo`,
    /// `stmt.source_session`, `stmt.origin` — and nothing read any of it, so a
    /// certificate over a real store could name no source and fell back to
    /// naming the relation symbol. That is not a weaker attribution; it is a
    /// circular one, and it read like provenance to anyone checking the field
    /// was populated.
    attribution: BTreeMap<ObjectId, Vec<ObjectId>>,
    /// Names for the source atoms minted above, so [`Self::hydrate`] can define
    /// them into a caller's graph. Without this an attribution prints as a bare
    /// content id — accurate and useless.
    source_names: BTreeMap<ObjectId, String>,
}

impl RelationalView {
    /// Read every live statement into memory.
    pub async fn load(store: &MemoryStore) -> Result<Self> {
        let syms = store
            .script("?[sym_id, name] := *sym{sym_id, name}", BTreeMap::new(), false)
            .await?;
        // No `@` selector, so every version comes back — the whole history.
        let stmts = store
            .script(
                "?[stmt_id, at, pred, arity, source_session, origin] := \
                  *stmt{stmt_id, at, pred, arity, source_session, origin}",
                BTreeMap::new(),
                false,
            )
            .await?;
        let args = store
            .script(
                "?[stmt_id, pos, val] := *arg{stmt_id, pos, val}",
                BTreeMap::new(),
                false,
            )
            .await?;
        let mut view = Self::assemble(syms, stmts, args)?;
        // Rules are expressions, so they come from the object table — but
        // **storing an expression is not believing it**, which is the rule
        // `schema.rs` states over the `assertion` relation and which this
        // ignored. Every persisted node of the right shape became a live
        // inference rule: a quoted proposition, a residual, a query the agent
        // once asked, and — demonstrated against a real store — a rule the agent
        // had explicitly *denied*. Belief is a join, not an inference from
        // presence.
        let beliefs = belief_rows(store).await.unwrap_or_default();
        // The view's own present: the last instant it holds. `SystemTime::now()`
        // made a "present" query depend on the wall clock, so one immutable view
        // gave two answers under one `snapshot` — and `rules()` and `known()`
        // each took their own reading of "now".
        let now = view.instants.last().copied().unwrap_or(i64::MAX);
        view.now = now;
        // The answers depend on `object` and `assertion` too, so a version that
        // counts only `stmt` rows stamps two contradictory results with the same
        // number — exactly the composition `compose_snapshot` exists to refuse.
        // Affirming or retracting a rule changes every answer and moved nothing.
        let mut g = artist_logic::ObjectGraph::new();
        if let Ok(ids) = crate::graph_store::load_all(store, &mut g).await {
            // A rule both affirmed and denied is contested, and firing it
            // would be reasoning from a contradiction. A contested *claim* is
            // different: that is a signal to surface, not to drop.
            // A rule is reasoned with only while it is **in force** — which is
            // both bounds, not just the upper one. Checking `valid_to` alone let
            // a rule affirmed to start next century fire today.
            let mut affirmed: BTreeSet<ObjectId> = BTreeSet::new();
            let mut denied: BTreeSet<ObjectId> = BTreeSet::new();
            // **No interval filter here.** Which rules are in force depends on
            // the instant being reasoned about, which is not known at load —
            // `rules_at` decides it per query. Filtering at `now` meant a rule
            // believed only over a past window could never answer about that
            // window, and one believed only in the future derived about today.
            for b in &beliefs {
                if b.affirmed {
                    affirmed.insert(b.proposition);
                } else {
                    denied.insert(b.proposition);
                }
            }
            let believed: BTreeSet<ObjectId> =
                affirmed.difference(&denied).copied().collect();
            for b in &beliefs {
                view.rule_validity.insert(b.proposition, (b.valid_from, b.valid_to));
            }
            view.index_rules(&g, &ids, &believed);
            view.index_reserved(&g, &beliefs);
            view.stmt_count += ids.len() as u64 + beliefs.len() as u64;
            view.rule_graph = g;
        }
        // Outside the block above on purpose: statement provenance does not
        // depend on the object table loading, and burying it there would make a
        // failure to read expressions silently unattribute every ordinary fact.
        view.derive_attribution(&beliefs);
        Ok(view)
    }

    /// Define this view's rule expressions into `g`.
    ///
    /// `rules()` hands back ids, and an id the caller's graph does not define is
    /// one `open_binder` returns `None` for — so a stored rule fired only in the
    /// process that authored it, and the test that covered it reused the
    /// authoring graph and never noticed.
    pub fn hydrate(&self, g: &mut ObjectGraph) {
        // Provenance atoms first. A certificate naming a source the caller's
        // graph does not define prints a bare content id, which names the right
        // thing and tells the reader nothing — and the reader is the entire
        // point of naming an authority.
        for (id, name) in &self.source_names {
            if g.get(*id).is_none() {
                g.define(*id, artist_logic::object::CoreNode::Atom { name: Some(name.clone()) });
            }
        }
        for id in self.rule_graph.all_ids() {
            let Some(node) = self.rule_graph.get(id) else { continue };
            match g.get(id) {
                None => g.define(id, node.clone()),
                // An id the caller already defines **differently** is a
                // disagreement about what a content id means, and silently
                // trusting the caller's version let a partially-hydrated graph
                // hand `open_binder` an interior node that is not the one the
                // rule was written against. Overwriting is the safe direction:
                // these are blake3 content ids, so a mismatch means one of the
                // two is wrong and the stored rule is the one under evaluation.
                // **Only content ids.** The justification for overwriting was
                // that a mismatch means one of the two is wrong — true of a
                // blake3 content id, false of everything else here.
                // `all_ids()` also returns `oid(sym)` ids, which are
                // `IdSpace::WellKnown` and allocated by the store's own symbol
                // counter, so symbol 32 legitimately means different things in
                // different stores. Overwriting those clobbered a caller's
                // unrelated graph and changed answers it had already computed.
                Some(existing)
                    if existing != node && id.space() == artist_logic::object::IdSpace::Content =>
                {
                    g.define(id, node.clone())
                }
                Some(_) => {}
            }
        }
    }

    /// Intern a provenance name as an object.
    ///
    /// Atom ids are content-derived, so the id this produces is the same one any
    /// other graph would produce for the same name — which is what lets a
    /// certificate carrying it be re-checked in a process that never saw this
    /// view. The name is remembered so `hydrate` can define the node.
    fn source_atom(&mut self, name: &str) -> ObjectId {
        let mut scratch = ObjectGraph::new();
        let id = scratch.atom(name);
        self.source_names.insert(id, name.to_string());
        id
    }

    /// Build the proposition → sources index, after coreference has settled.
    ///
    /// Keyed on `(pred args)` as an application node, because that is what the
    /// evaluator hands to `attribution` — it is the same content id the store
    /// would compute, which is the whole reason node ids are content-derived.
    fn derive_attribution(&mut self, beliefs: &[BeliefRow]) {
        let mut g = ObjectGraph::new();
        let mut out: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
        let push = |out: &mut BTreeMap<ObjectId, Vec<ObjectId>>, prop, src| {
            let e: &mut Vec<ObjectId> = out.entry(prop).or_default();
            if !e.contains(&src) {
                e.push(src);
            }
        };
        for ((pred, _), events) in &self.history {
            let p = oid(*pred);
            for (_, is_assert, _, tuple, source) in events {
                // A retraction is not a claim about the world, so whoever wrote
                // it does not vouch for the tuple.
                let (true, Some(src)) = (*is_assert, *source) else { continue };
                let prop = g.apply(p, tuple.clone());
                push(&mut out, prop, src);
            }
        }
        for b in beliefs {
            if let Some(agent) = b.agent {
                push(&mut out, b.proposition, agent);
            }
        }
        self.attribution = out;
    }

    fn assemble(syms: NamedRows, stmts: NamedRows, args: NamedRows) -> Result<Self> {
        let mut view = RelationalView::default();
        for (s, n) in well_known::NAMES {
            view.by_name.insert((*n).to_string(), *s);
            view.by_sym.insert(*s, (*n).to_string());
        }
        for row in &syms.rows {
            let id = int_at(row, 0)? as Sym;
            let name = str_at(row, 1)?;
            view.by_name.insert(name.clone(), id);
            view.by_sym.insert(id, name);
        }

        // Bucket arguments by statement, keeping positional order.
        let mut by_stmt: BTreeMap<i64, BTreeMap<i64, i64>> = BTreeMap::new();
        for row in &args.rows {
            by_stmt
                .entry(int_at(row, 0)?)
                .or_default()
                .insert(int_at(row, 1)?, int_at(row, 2)?);
        }

        let mut instants: BTreeSet<i64> = BTreeSet::new();
        for row in &stmts.rows {
            let id = int_at(row, 0)?;
            let (ts, is_assert) = validity_at(row, 1)?;
            let pred = int_at(row, 2)? as Sym;
            let arity = int_at(row, 3)? as usize;
            let Some(slots) = by_stmt.get(&id) else { continue };
            if slots.len() != arity {
                // A torn statement is dropped rather than silently evaluated at
                // the wrong arity; the event log remains the system of record.
                // The predicate loses its authority with it.
                view.torn.insert(pred);
                continue;
            }
            let tuple: Vec<ObjectId> =
                slots.values().map(|v| oid(*v as Sym)).collect();
            instants.insert(ts);
            // Provenance, as an object rather than a string. `source_session`
            // names the run that wrote the row and `origin` names where it came
            // from; either is somebody a reader can go and interrogate, which is
            // exactly what a certificate's one trust point needs and has never
            // had. Prefer the session — it is the finer of the two.
            let session = str_at(row, 4).unwrap_or_default();
            let origin = str_at(row, 5).unwrap_or_default();
            let source = match (session.as_str(), origin.as_str()) {
                ("", "") | ("", "unknown") => None,
                ("", o) => Some(view.source_atom(o)),
                (s, _) => Some(view.source_atom(&format!("session:{s}"))),
            };
            view.history
                .entry((pred, arity))
                .or_default()
                .push((ts, is_assert, id, tuple, source));
        }
        view.instants = instants.into_iter().collect();
        view.stmt_count = stmts.rows.len() as u64 + args.rows.len() as u64;

        // **Order matters, and getting it wrong was three separate findings.**
        //
        // Identity has to be known before anything is derived from the log, and
        // the log has to be rewritten before the present state is folded out of
        // it. Folding first and canonicalising after meant a retraction of one
        // alias and an assertion of another were two tuples to the `rels` fold
        // and one tuple to the `known_at` fold — so `known` and `known_at(now)`
        // contradicted each other about the same instant, one of them with a
        // confident refutation. And deriving sorts, authority and units from the
        // raw symbol space left them keyed under names coreference had merged,
        // so re-keying afterwards silently dropped whichever collided.
        //
        // So: read `is` from the raw log, canonicalise the log, then derive
        // *everything else* from the canonical form. Nothing is re-keyed after
        // the fact because nothing was ever keyed wrongly.
        view.build_present();
        view.derive_identity();
        view.canonicalise_log();
        view.rels.clear();
        view.build_present();
        view.derive_rest();
        Ok(view)
    }

    /// Fold the change log into the present state.
    fn build_present(&mut self) {
        let now = self.instants.last().copied().unwrap_or(0);
        let keys: Vec<(Sym, usize)> = self.history.keys().cloned().collect();
        for (pred, arity) in keys {
            let live = self.state_at(pred, arity, now);
            if !live.is_empty() {
                self.rels.insert((pred, arity), live.into_iter().collect());
            }
        }
    }

    /// Sorts and closed-world status are *read off the facts*, never declared.
    /// A nominal sort that later acquires an authority becomes closed by writing
    /// one `has_resolver` statement, and every negation over it tightens.
    /// Read `is` off the raw log. Nothing else may be derived before this,
    /// because everything else is keyed by names it merges.
    fn derive_identity(&mut self) {
        // Union-find over asserted identities, so `is(skolem, entity)` actually
        // rewrites terms rather than sitting inert in the store.
        if let Some(pairs) = self.rels.get(&(well_known::IS, 2)).cloned() {
            // **A name can have several aliases.** `same` holds one out-edge
            // per node, so a second `is(a, ·)` row overwrote the first and the
            // two aliases were never unioned — leaving two names the store
            // declares identical with opposite answers, one of them a confident
            // refutation of a fact the store affirms under the other name. Which
            // one survived was decided by interning order.
            //
            // Union properly: merge the classes and point every member at the
            // least id, which is stable however the rows are ordered.
            let mut root: BTreeMap<ObjectId, ObjectId> = BTreeMap::new();
            let find = |root: &BTreeMap<ObjectId, ObjectId>, mut x: ObjectId| {
                for _ in 0..64 {
                    match root.get(&x) {
                        Some(r) if *r != x => x = *r,
                        _ => break,
                    }
                }
                x
            };
            for t in &pairs {
                let (ra, rb) = (find(&root, t[0]), find(&root, t[1]));
                if ra == rb {
                    continue;
                }
                let (rep, other) = (ra.min(rb), ra.max(rb));
                root.insert(other, rep);
            }
            let keys: Vec<ObjectId> = pairs.iter().flat_map(|t| [t[0], t[1]]).collect();
            for k in keys {
                let r = find(&root, k);
                if r != k {
                    self.same.insert(k, r);
                }
            }
            // Collapse chains so lookup is a single hop.
            // Collapse to a **fixed point**. The old loop stopped on
            // `next == cur` or a hop budget, and a cycle has neither — so with
            // `is(a,b), is(b,c), is(c,a)` each key landed wherever its budget
            // expired and `canonical(canonical(a)) != canonical(a)`. A stored
            // tuple then rewrote to one representative while a query for a
            // different name rewrote to another, and a closed predicate turned
            // that miss into a confident refutation of a fact still on file.
            //
            // A cycle means every name on it denotes one thing, so the whole
            // cycle collapses to its least id — stable no matter which edge is
            // walked first.
            let keys: Vec<ObjectId> = self.same.keys().cloned().collect();
            for k in keys {
                let mut seen = vec![k];
                let mut cur = k;
                while let Some(next) = self.same.get(&cur).copied() {
                    if let Some(pos) = seen.iter().position(|m| *m == next) {
                        let rep = *seen[pos..].iter().min().expect("non-empty");
                        for m in &seen[pos..] {
                            if *m == rep {
                                self.same.remove(m);
                            } else {
                                self.same.insert(*m, rep);
                            }
                        }
                        break;
                    }
                    seen.push(next);
                    cur = next;
                }
            }
            // Every remaining chain is acyclic; walk each to its root once.
            let keys: Vec<ObjectId> = self.same.keys().copied().collect();
            for k in keys {
                let mut cur = k;
                let mut guard = 0;
                while let Some(next) = self.same.get(&cur).copied() {
                    cur = next;
                    guard += 1;
                    if guard > pairs.len() + 1 {
                        break;
                    }
                }
                if cur != k {
                    self.same.insert(k, cur);
                } else {
                    self.same.remove(&k);
                }
            }
        }

    }

    fn derive_rest(&mut self) {
        if let Some(members) = self.rels.get(&(well_known::TYPE, 2)) {
            for t in members {
                let (Some(x), Some(sort)) = (desym(t[0]), desym(t[1])) else { continue };
                let entry = self.sorts.entry(sort).or_default();
                let member = oid(x);
                if !entry.contains(&member) {
                    entry.push(member);
                }
            }
        }
        if let Some(rs) = self.rels.get(&(well_known::HAS_RESOLVER, 1)) {
            for t in rs {
                if let Some(s) = desym(t[0])
                    && !self.torn.contains(&s)
                {
                    self.closed.insert(s);
                }
            }
        }
        // Closure is an **interval set**, not a start instant. A `min` over the
        // assert events ignored the retractions between them, so an
        // assert/retract/re-assert cycle backfilled authority across the hole
        // and licensed refutation at an instant when no resolver existed.
        if let Some(events) = self.history.get(&(well_known::HAS_RESOLVER, 1)).cloned() {
            let mut per: BTreeMap<Sym, Vec<(i64, bool)>> = BTreeMap::new();
            for (t, is_assert, _id, tuple, _) in events {
                if let Some(sym) = tuple.first().and_then(|x| desym(*x)) {
                    per.entry(sym).or_default().push((t, is_assert));
                }
            }
            for (sym, mut evs) in per {
                evs.sort_unstable();
                let mut spans: Vec<(i64, i64)> = Vec::new();
                let mut open: Option<i64> = None;
                for (t, is_assert) in evs {
                    match (is_assert, open) {
                        (true, None) => open = Some(t),
                        (false, Some(from)) => {
                            spans.push((from, t));
                            open = None;
                        }
                        _ => {}
                    }
                }
                if let Some(from) = open {
                    spans.push((from, i64::MAX));
                }
                self.closed_spans.insert(sym, spans);
            }
        }

        // Units, as data. A new unit costs one written fact — which was the
        // decision, and was not what the code did: no structure implemented
        // `GraphStructure::scale`, so every `(quantity n u)` comparison across
        // units stalled no matter what the store held.
        //
        // The factor is a *symbol* like every other stored argument, so its
        // numeric value is its name. A factor whose name is not a number is not
        // a conversion, and is skipped rather than guessed at.
        if let Some(rows) = self.rels.get(&(well_known::SCALE, 3)).cloned() {
            for t in &rows {
                let Some(factor_sym) = desym(t[1]) else { continue };
                let Some(name) = self.by_sym.get(&factor_sym) else { continue };
                let Ok(factor) = name.parse::<num_bigint::BigInt>() else { continue };
                // Zero makes every quantity in the unit provably equal to every
                // other; a negative factor is not a conversion either. Skipped
                // the way an unparseable name already is.
                if factor <= num_bigint::BigInt::from(0) {
                    continue;
                }
                // Two conflicting rows are a contradiction in the store, not a
                // race to be won by whichever argument symbol was interned
                // later. Neither is used.
                if let Some((existing, base)) = self.scales.get(&t[0])
                    && (*existing != factor || *base != t[2])
                {
                    self.conflicting_scales.insert(t[0]);
                    continue;
                }
                self.scales.insert(t[0], (factor, t[2]));
            }
        }

        // What a term denotes, so a *function* of an entity can have a value.
        if let Some(rows) = self.rels.get(&(well_known::VALUE, 2)).cloned() {
            for t in &rows {
                // Two rows denoting one term differently is a contradiction in
                // the store, not a race won by whichever argument symbol was
                // interned later. `scale` had this guard and `value` did not,
                // though the argument for it is identical.
                if let Some(existing) = self.values.get(&t[0])
                    && *existing != t[1]
                {
                    self.conflicting_values.insert(t[0]);
                    continue;
                }
                self.values.insert(t[0], t[1]);
            }
        }
    }

    /// Index affirmed applications of the reserved vocabulary.
    ///
    /// An affirmed `(usually P)` is a default on record; an affirmed
    /// `(obliged P)` is a norm on record; an affirmed `(same-as a b)` is a
    /// coreference claim. Denial is kept apart from affirmation rather than
    /// collapsed, so a proposition on file in both directions reaches
    /// `Knowledge::Conflicted` — which is the state §5.1 argues a multi-session
    /// store reaches constantly and which evaluation previously could not
    /// produce from data at all.
    /// Merge every reserved claim whose interval covers `at` — `None` meaning
    /// the present. Disagreement across sources is `Conflicted`, which is the
    /// whole reason both polarities are indexed rather than subtracted.
    fn reserved_lookup(&self, pred: ObjectId, args: &[ObjectId], at: Option<i64>) -> Knowledge {
        let key = (pred, self.canonical_args(pred, args));
        let Some(claims) = self.reserved.get(&key) else { return Knowledge::Unknown };
        // The view's own present, not the wall clock: a snapshot that answers
        // differently depending on when you ask is not a snapshot.
        let when = at.unwrap_or(self.now);
        claims
            .iter()
            .filter(|c| c.covers(when))
            .fold(Knowledge::Unknown, |acc, c| acc.merge(c.knowledge))
    }

    fn index_reserved(&mut self, g: &ObjectGraph, beliefs: &[BeliefRow]) {
        use artist_logic::object::CoreNode;
        for b in beliefs {
            let Some(CoreNode::Apply { operator, operands }) = g.get(b.proposition) else {
                continue;
            };
            if wk::name_of(*operator).is_none() {
                continue;
            }
            // The lookup canonicalises through the positional mask, so the index
            // must too — an affirmed claim about an entity with an alias on file
            // was otherwise unreachable under *either* name.
            let key = (*operator, self.canonical_args(*operator, operands));
            self.reserved.entry(key).or_default().push(ReservedClaim {
                knowledge: if b.affirmed { Knowledge::Holds } else { Knowledge::Denied },
                valid_from: b.valid_from,
                valid_to: b.valid_to,
            });
        }
    }

    /// Index stored `forall`-implications by the predicate they conclude.
    ///
    /// Mirrors `match_goal`: a conclusion may be wrapped in `usually` or `not`
    /// and still be *about* the goal predicate, so the wrappers are seen through
    /// here too. An index that missed them would leave exactly the defeasible
    /// rules — the shape most of this memory is made of — unreachable.
    fn index_rules(
        &mut self,
        g: &ObjectGraph,
        ids: &[ObjectId],
        affirmed: &BTreeSet<ObjectId>,
    ) {
        use artist_logic::object::CoreNode;
        for id in ids {
            // Presence in the object table is not belief.
            if !affirmed.contains(id) {
                continue;
            }
            let Some(CoreNode::Bind { binder, bodies, .. }) = g.get(*id) else { continue };
            if *binder != wk::FORALL {
                continue;
            }
            let Some(body) = bodies.first().copied() else { continue };
            let Some(CoreNode::Apply { operator, operands }) = g.get(body) else { continue };
            if *operator != wk::IMPLIES || operands.len() != 2 {
                continue;
            }
            let mut conclusion = operands[1];
            for _ in 0..8 {
                let Some(CoreNode::Apply { operator, operands }) = g.get(conclusion) else {
                    break;
                };
                if !matches!(*operator, wk::USUALLY | wk::NOT) {
                    break;
                }
                let Some(inner) = operands.last().copied() else { break };
                conclusion = inner;
            }
            if let Some(CoreNode::Apply { operator, .. }) = g.get(conclusion) {
                self.rules.entry(*operator).or_default().push(*id);
            }
        }
    }

    /// Tuples holding at `t`: for each tuple, the latest event at or before
    /// `t` decides it.
    fn state_at(&self, pred: Sym, arity: usize, t: i64) -> Vec<Vec<ObjectId>> {
        let Some(events) = self.history.get(&(pred, arity)) else {
            return Vec::new();
        };
        // Fold per *statement*, then a tuple holds if **any** statement asserting
        // it is still live.
        let mut per_stmt: BTreeMap<i64, (i64, bool, Vec<ObjectId>)> = BTreeMap::new();
        for (at, assert, id, tuple, _) in events {
            if *at > t {
                continue;
            }
            match per_stmt.get(id) {
                Some((prev, _, _)) if prev > at => {}
                _ => {
                    per_stmt.insert(*id, (*at, *assert, tuple.clone()));
                }
            }
        }
        let mut state: BTreeMap<Vec<ObjectId>, (i64, bool)> = BTreeMap::new();
        for (at, assert, tuple) in per_stmt.into_values() {
            let entry = state.entry(tuple).or_insert((at, assert));
            // One live assertion is enough; a retraction only wins if nothing
            // else still asserts the tuple.
            if assert && !entry.1 {
                *entry = (at, true);
            }
        }
        state
            .into_iter()
            .filter(|(_, (_, assert))| *assert)
            .map(|(tuple, _)| tuple)
            .collect()
    }

    pub fn sym(&self, name: &str) -> Option<Sym> {
        self.by_name.get(name).copied()
    }

    pub fn name(&self, s: Sym) -> Option<&str> {
        self.by_sym.get(&s).map(String::as_str)
    }

    /// Render a term for display, resolving interned symbols back to names.
    pub fn show(&self, t: ObjectId) -> String {
        desym(t)
            .and_then(|s| self.name(s))
            .map(str::to_string)
            .unwrap_or_else(|| format!("{t}"))
    }
}

/// Intern a name, returning its symbol. Well-known names resolve without a
/// write so their ids stay stable across stores.
pub async fn intern(store: &MemoryStore, name: &str) -> Result<Sym> {
    for (s, n) in well_known::NAMES {
        if *n == name {
            return Ok(*s);
        }
    }
    let mut params = BTreeMap::new();
    params.insert("n".to_string(), DataValue::from(name));
    let existing = store
        .script("?[sym_id] := *sym{sym_id, name}, name == $n", params.clone(), false)
        .await?;
    if let Some(row) = existing.rows.first() {
        return Ok(int_at(row, 0)? as Sym);
    }

    let max = store
        .script("?[max(sym_id)] := *sym{sym_id}", BTreeMap::new(), false)
        .await?;
    let next = max
        .rows
        .first()
        .and_then(|r| r.first())
        .and_then(DataValue::get_int)
        .map(|m| m + 1)
        .unwrap_or(well_known::FIRST_INTERNED as i64)
        .max(well_known::FIRST_INTERNED as i64);

    params.insert("id".to_string(), DataValue::from(next));
    store
        .script("?[sym_id, name] <- [[$id, $n]] :put sym {sym_id => name}", params, true)
        .await?;
    Ok(next as Sym)
}

/// Assert a statement. Arity is whatever `args` has — storage carries no bound
/// the logic does not have.
pub async fn assert_stmt(
    store: &MemoryStore,
    pred: Sym,
    args: &[Sym],
    origin: &str,
) -> Result<i64> {
    assert_stmt_at(store, pred, args, origin, None).await
}

/// Assert a statement as becoming true at a given instant (microseconds since
/// the epoch). `None` means now.
pub async fn assert_stmt_at(
    store: &MemoryStore,
    pred: Sym,
    args: &[Sym],
    origin: &str,
    at: Option<i64>,
) -> Result<i64> {
    let max = store
        .script("?[max(stmt_id)] := *stmt{stmt_id}", BTreeMap::new(), false)
        .await?;
    let id = max
        .rows
        .first()
        .and_then(|r| r.first())
        .and_then(DataValue::get_int)
        .map(|m| m + 1)
        .unwrap_or(1);

    let mut params = BTreeMap::new();
    params.insert("id".to_string(), DataValue::from(id));
    params.insert("ts".to_string(), DataValue::from(at.unwrap_or_else(now_micros)));
    params.insert("pred".to_string(), DataValue::from(pred as i64));
    params.insert("arity".to_string(), DataValue::from(args.len() as i64));
    params.insert("origin".to_string(), DataValue::from(origin));
    store
        .script(
            "?[stmt_id, at, pred, arity, origin] <- [[$id, [$ts, true], $pred, $arity, $origin]]
             :put stmt {stmt_id, at => pred, arity, origin}",
            params,
            true,
        )
        .await?;

    let rows: Vec<DataValue> = args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            DataValue::List(vec![
                DataValue::from(id),
                DataValue::from(i as i64),
                DataValue::from(*a as i64),
            ])
        })
        .collect();
    let mut params = BTreeMap::new();
    params.insert("rows".to_string(), DataValue::List(rows));
    store
        .script(
            "?[stmt_id, pos, val] <- $rows :put arg {stmt_id, pos => val}",
            params,
            true,
        )
        .await?;
    Ok(id)
}

/// Retract a statement as of an instant.
///
/// Not a delete and not a flag: a `[t, false]` validity row. The statement stays
/// in the history, so `At`, `always` and `eventually` can still see that it once
/// held — which is the whole point of a memory that can be asked what it used to
/// believe.
pub async fn retract_stmt(store: &MemoryStore, id: i64) -> Result<()> {
    retract_stmt_at(store, id, None).await
}

pub async fn retract_stmt_at(
    store: &MemoryStore,
    id: i64,
    at: Option<i64>,
) -> Result<()> {
    let mut params = BTreeMap::new();
    params.insert("id".to_string(), DataValue::from(id));
    store
        .script(
            "prev[pred, arity, origin] := *stmt{stmt_id: $id, pred, arity, origin}
             ?[stmt_id, at, pred, arity, origin] :=
                 prev[pred, arity, origin],
                 stmt_id = $id, at = [$ts, false]
             :put stmt {stmt_id, at => pred, arity, origin}",
            {
                let mut p = params.clone();
                p.insert(
                    "ts".to_string(),
                    DataValue::from(at.unwrap_or_else(now_micros)),
                );
                p
            },
            true,
        )
        .await?;
    let _ = params;
    Ok(())
}

impl GraphStructure for RelationalView {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        // The reserved vocabulary lives in the belief layer, not in
        // `stmt`/`arg`, so it is answered before the storage-symbol path.
        if wk::name_of(pred).is_some() {
            return self.reserved_lookup(pred, args, None);
        }
        // A predicate outside this view's symbol space is not something the
        // view can speak to at all — Unknown, never a refutation.
        let Some(p) = desym(pred) else { return Knowledge::Unknown };
        let canon = self.canonical_args(pred, args);
        let found = self.rels.get(&(p, args.len())).is_some_and(|t| t.contains(&canon));
        if found {
            return Knowledge::Holds;
        }
        // Absence is refutation only where this view is authoritative. The
        // view is a *snapshot loaded into memory*, so within it completeness is
        // exact — but a predicate not marked closed has no authority and must
        // stay Unknown rather than becoming a refutation.
        let now = self.instants.last().copied().unwrap_or(i64::MAX);
        if self.authoritative_at(p, args.len(), now) {
            Knowledge::Fails
        } else {
            Knowledge::Unknown
        }
    }

    fn known_at(&self, pred: ObjectId, args: &[ObjectId], t: i64) -> Knowledge {
        if wk::name_of(pred).is_some() {
            return self.reserved_lookup(pred, args, Some(t));
        }
        let Some(p) = desym(pred) else { return Knowledge::Unknown };
        let tuples = self.state_at(p, args.len(), t);
        let canon = self.canonical_args(pred, args);
        if tuples.contains(&canon) {
            return Knowledge::Holds;
        }
        if self.authoritative_at(p, args.len(), t) {
            Knowledge::Fails
        } else {
            Knowledge::Unknown
        }
    }

    /// Members as of `t`: sort membership read off the change log rather than
    /// the present state.
    fn extension_at(&self, domain: ObjectId, t: i64) -> Option<Extension> {
        let sort = desym(domain)?;
        let live = self.state_at(well_known::TYPE, 2, t);
        let members: Vec<ObjectId> = live
            .iter()
            .filter(|tuple| desym(tuple[1]) == Some(sort))
            .map(|tuple| self.canonical(tuple[0]))
            .collect();
        if members.is_empty() && !self.sorts.contains_key(&sort) {
            return None;
        }
        // Not `authoritative_at(sort, 2, t)`: that conjunct requires history for
        // *the sort symbol used as a binary predicate*, and `Path` is a sort, not
        // a 2-ary relation. Completeness therefore depended on an unrelated
        // `Path(x, y)` row existing, and the dated form could never agree with
        // the undated one.
        let authoritative = self.closed.contains(&sort)
            && !self.torn.contains(&well_known::TYPE)
            && self.authority_covers(sort, t);
        Some(if authoritative {
            Extension::complete(members)
        } else {
            Extension::partial(members)
        })
    }

    fn extension(&self, domain: ObjectId) -> Option<Extension> {
        // Loading the whole table establishes that the view holds everything
        // *it* has. It does not establish that those are all the members, and
        // the old comment here conflated the two: a sort with no authority
        // reported `complete`, so `(forall ((x Person)) (mortal x))` came back
        // `Supported/Exact` over whichever people happened to have been written
        // down — and adding one more `type` row, a pure addition, flipped it to
        // `Refuted`. Completeness is the same judgement `known` already makes
        // for refutation, and it needs the same evidence.
        let sort = desym(domain)?;
        let members = self.sorts.get(&sort).cloned()?;
        // `torn` guarded `known` and `is_closed` and not this. `sorts` is built
        // from `rels[(TYPE,2)]`, which silently excludes torn statements — so
        // corrupting one `arg` row shrank the enumeration while it still claimed
        // to be complete, and a universal flipped from `Refuted` to a confident
        // `Supported`. Knowing you lost rows is knowing you cannot enumerate.
        Some(if self.closed.contains(&sort) && !self.torn.contains(&well_known::TYPE) {
            Extension::complete(members)
        } else {
            Extension::partial(members)
        })
    }

    fn attribution(&self, proposition: ObjectId) -> Vec<ObjectId> {
        self.attribution.get(&proposition).cloned().unwrap_or_default()
    }

    fn is_closed(&self, pred: ObjectId) -> bool {
        desym(pred).is_some_and(|p| self.closed.contains(&p) && !self.torn.contains(&p))
    }

    /// Stored rules concluding `pred`. Empty until now, which made the whole
    /// derivation half of §5.2 inert against the real store.
    fn rules(&self, pred: ObjectId) -> Vec<ObjectId> {
        self.rules_at(pred, self.now)
    }

    /// Rules in force **at the instant being reasoned about**.
    ///
    /// Selecting at `now` was wrong in both directions: a rule affirmed to start
    /// next century derived about the past, and a rule in force at the queried
    /// instant but lapsed today could not answer about it. `reserved_lookup`
    /// and `authority_covers` already date their answers; this is the same
    /// judgement, and the rule index simply wasn't asking.
    fn rules_at(&self, pred: ObjectId, t: i64) -> Vec<ObjectId> {
        self.rules
            .get(&pred)
            .map(|rs| {
                rs.iter()
                    .filter(|r| {
                        self.rule_validity
                            .get(r)
                            .is_none_or(|(from, to)| {
                                from.is_none_or(|f| f <= t) && to.is_none_or(|e| t < e)
                            })
                    })
                    .copied()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One `scale` step, read off an ordinary stored fact.
    fn scale(&self, unit: ObjectId) -> Option<(num_bigint::BigInt, ObjectId)> {
        let u = self.canonical(unit);
        if self.conflicting_scales.contains(&u) {
            return None;
        }
        self.scales.get(&u).cloned()
    }

    /// Declared sharpness, read off ordinary stored claims.
    ///
    /// Both directions are consulted, and disagreement is resolved toward
    /// `Indeterminate` — a store that holds a proposition both sharp and
    /// borderline has not established sharpness, and certification must refuse
    /// it. Silence is `Unknown`: presumed total for queries, refused for proof.
    fn determinacy(&self, proposition: ObjectId) -> artist_logic::evidence::Determinacy {
        use artist_logic::evidence::Determinacy;
        // **The two id spaces are disjoint, so reading `stmt` rows alone made
        // this hook unreachable.** Every argument of every stored statement is
        // `oid(sym) = FIRST_FREE + sym`, in `IdSpace::WellKnown`; every
        // proposition the evaluator asks about is an `Apply`/`Bind` node tagged
        // `IdSpace::Content`. The bands cannot meet, so a declaration written as
        // a statement could never match — the same shape as "stored rules never
        // fired" and "the reserved vocabulary was permanently Unknown", now the
        // third time it has bitten.
        //
        // Propositions live in the object table, so a claim *about* one belongs
        // in the belief layer, where it also gets attribution, valid time,
        // retraction and conflict.
        let p = self.canonical(proposition);
        let holds = |sym: Sym, op: ObjectId| {
            self.rels.get(&(sym, 1)).is_some_and(|t| t.contains(&vec![p]))
                || self.reserved_lookup(op, &[p], None) == Knowledge::Holds
        };
        match (
            holds(well_known::INDETERMINATE, wk::INDETERMINATE),
            holds(well_known::TOTAL, wk::TOTAL),
        ) {
            (true, _) => Determinacy::Indeterminate,
            (false, true) => Determinacy::Total,
            (false, false) => Determinacy::Unknown,
        }
    }

    /// What a term denotes.
    fn value(&self, term: ObjectId) -> Option<ObjectId> {
        let t = self.canonical(term);
        if self.conflicting_values.contains(&t) {
            return None;
        }
        self.values.get(&t).copied()
    }

    /// **This view models no worlds**, so it can answer about none of them.
    ///
    /// The trait default forwards to `known`, which answers about the *actual*
    /// world — so `(in-world if-we-had-merged P)` came back with the truth about
    /// what really happened, labelled as the truth about a counterfactual. A
    /// store with no world column has to say `Unknown` and let the caller see
    /// the absence.
    fn known_in(&self, _pred: ObjectId, _args: &[ObjectId], _world: ObjectId) -> Knowledge {
        Knowledge::Unknown
    }

    fn known_at_in(
        &self,
        _pred: ObjectId,
        _args: &[ObjectId],
        _t: i64,
        _world: ObjectId,
    ) -> Knowledge {
        Knowledge::Unknown
    }

    fn instants(&self) -> Vec<i64> {
        self.instants.clone()
    }

    fn snapshot(&self) -> u64 {
        // **Not** the latest instant. That is *valid* time, and a backdated
        // write — an ordinary thing to do when recording something learned late
        // about an earlier moment — leaves it unmoved while the answers change
        // underneath it. Two writes in the same microsecond were likewise
        // indistinguishable. The count of loaded statements moves on every
        // write, including a retraction, which is what a version has to do.
        self.stmt_count
    }
}

impl RelationalView {
    /// Equivalence-class representative, so an asserted `is(a, b)` acts.
    /// Rewrite stored tuples, sort members and unit keys through the identity
    /// relation, so both sides of every lookup speak the same names.
    /// Rewrite the change log through the identity relation.
    ///
    /// **Only the log.** Everything else is derived *after* this runs, from the
    /// canonical form, so there is nothing left to re-key — and re-keying was
    /// itself a defect: collapsing `sorts` into a `BTreeMap` through `canonical`
    /// dropped whichever of two merged sort names collided, and then reported
    /// the truncated membership as `complete`. A universal came back `Supported`
    /// in a view that simultaneously affirmed the missing member's sort and
    /// refuted the property.
    fn canonicalise_log(&mut self) {
        if self.same.is_empty() {
            return;
        }
        let history = std::mem::take(&mut self.history);
        self.history = history
            .into_iter()
            .map(|((p, n), events)| {
                let pred = oid(p);
                let mapped = events
                    .into_iter()
                    .map(|(t, is_assert, id, tuple, source)| {
                        (t, is_assert, id, self.canonical_args(pred, &tuple), source)
                    })
                    .collect();
                ((p, n), mapped)
            })
            .collect();
    }

    /// May this view answer an *absence* about `pred/arity` as of `t`?
    ///
    /// Three things had to be true and only one was checked.
    ///
    /// * **The authority must already exist at `t`.** `has_resolver` is a
    ///   valid-time statement, and closure was read at the present and applied
    ///   backwards, so a predicate that acquired an authority yesterday refuted
    ///   claims about last year.
    /// * **The view must not have dropped rows for it.** A torn statement is
    ///   skipped, and the predicate stayed closed — laundering partial-write
    ///   corruption into a confident refutation.
    /// * **The arity must be one it has seen.** Relations are keyed
    ///   `(pred, arity)` everywhere else here; authority over `meets/2` says
    ///   nothing about `meets/3`, and answering `Fails` for an arity with no
    ///   rows at all is a claim the view cannot support. The test is against the
    ///   *change log*, not the present state — a predicate whose every row has
    ///   been retracted is still one this view has seen, and refuting there is
    ///   exactly what a retraction is for.
    fn authoritative_at(&self, pred: Sym, arity: usize, t: i64) -> bool {
        self.closed.contains(&pred)
            && !self.torn.contains(&pred)
            && self.authority_covers(pred, t)
            && self.history.contains_key(&(pred, arity))
    }

    /// Did an authority for `pred` exist at `t`? Half-open spans, so an
    /// authority retracted at `r` covers `[from, r)` and not `r` itself.
    fn authority_covers(&self, pred: Sym, t: i64) -> bool {
        match self.closed_spans.get(&pred) {
            None => false,
            Some(spans) => spans.iter().any(|(from, to)| *from <= t && t < *to),
        }
    }

    /// Canonicalise the argument positions coreference is allowed to touch.
    ///
    /// Substitution is per *position*, and this layer used to ignore that
    /// entirely: it mapped `canonical` over every argument before the lookup, so
    /// the intensional barrier the evaluator carefully routed around was undone
    /// one call deeper. With `is(superman, clark)` on file,
    /// `believes(lois, superman)` was answered as `believes(lois, clark)` — the
    /// exact substitution §7 names as the thing that must never happen.
    fn canonical_args(&self, pred: ObjectId, args: &[ObjectId]) -> Vec<ObjectId> {
        args.iter()
            .enumerate()
            .map(|(i, a)| {
                if wk::is_extensional_at(pred, i) { self.canonical(*a) } else { *a }
            })
            .collect()
    }

    fn canonical(&self, t: ObjectId) -> ObjectId {
        self.same.get(&t).copied().unwrap_or(t)
    }
}

fn validity_at(row: &[DataValue], i: usize) -> Result<(i64, bool)> {
    match row.get(i) {
        Some(DataValue::Validity(v)) => Ok((v.timestamp.0.0, v.is_assert.0)),
        other => Err(anyhow!("expected validity at column {i}, got {other:?}")),
    }
}

/// Microseconds since the epoch — the unit a `Validity` timestamp uses.
pub fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

fn int_at(row: &[DataValue], i: usize) -> Result<i64> {
    row.get(i)
        .and_then(DataValue::get_int)
        .ok_or_else(|| anyhow!("expected int at column {i}"))
}

fn str_at(row: &[DataValue], i: usize) -> Result<String> {
    row.get(i)
        .and_then(DataValue::get_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("expected string at column {i}"))
}
