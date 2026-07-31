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
use artist_logic::{GraphStructure, ObjectId, wk};

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
    pub const FIRST_INTERNED: Sym = 32;

    pub const NAMES: &[(Sym, &str)] = &[
        (TYPE, "type"),
        (HAS_RESOLVER, "has_resolver"),
        (SORT, "Sort"),
        (IS, "is"),
    ];
}

/// An immutable view of the store's relational layer.
#[derive(Clone, Debug, Default)]
pub struct RelationalView {
    by_name: BTreeMap<String, Sym>,
    by_sym: BTreeMap<Sym, String>,
    rels: BTreeMap<(Sym, usize), BTreeSet<Vec<ObjectId>>>,
    sorts: BTreeMap<Sym, Vec<ObjectId>>,
    closed: BTreeSet<Sym>,
    same: BTreeMap<ObjectId, ObjectId>,
    /// (instant, is_assert, tuple) per (pred, arity) — the change log behind
    /// `At`, `always` and `eventually`.
    history: BTreeMap<(Sym, usize), Vec<(i64, bool, Vec<ObjectId>)>>,
    instants: Vec<i64>,
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
                "?[stmt_id, at, pred, arity] := *stmt{stmt_id, at, pred, arity}",
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
        Self::assemble(syms, stmts, args)
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
                continue;
            }
            let tuple: Vec<ObjectId> =
                slots.values().map(|v| oid(*v as Sym)).collect();
            instants.insert(ts);
            view.history
                .entry((pred, arity))
                .or_default()
                .push((ts, is_assert, tuple));
        }
        view.instants = instants.into_iter().collect();

        // The present is the history read at its last instant.
        let now = view.instants.last().copied().unwrap_or(0);
        let keys: Vec<(Sym, usize)> = view.history.keys().cloned().collect();
        for (pred, arity) in keys {
            let live = view.state_at(pred, arity, now);
            if !live.is_empty() {
                view.rels.insert((pred, arity), live.into_iter().collect());
            }
        }

        view.derive();
        Ok(view)
    }

    /// Sorts and closed-world status are *read off the facts*, never declared.
    /// A nominal sort that later acquires an authority becomes closed by writing
    /// one `has_resolver` statement, and every negation over it tightens.
    fn derive(&mut self) {
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
                if let Some(s) = desym(t[0]) {
                    self.closed.insert(s);
                }
            }
        }

        // Union-find over asserted identities, so `is(skolem, entity)` actually
        // rewrites terms rather than sitting inert in the store.
        if let Some(pairs) = self.rels.get(&(well_known::IS, 2)).cloned() {
            for t in &pairs {
                let (a, b) = (t[0].clone(), t[1].clone());
                let rep = self.same.get(&b).cloned().unwrap_or(b);
                self.same.insert(a, rep);
            }
            // Collapse chains so lookup is a single hop.
            let keys: Vec<ObjectId> = self.same.keys().cloned().collect();
            for k in keys {
                let mut cur = self.same.get(&k).cloned().unwrap_or_else(|| k.clone());
                let mut hops = 0;
                while let Some(next) = self.same.get(&cur).cloned() {
                    if next == cur || hops > pairs.len() {
                        break;
                    }
                    cur = next;
                    hops += 1;
                }
                self.same.insert(k, cur);
            }
        }
    }

    /// Tuples holding at `t`: for each tuple, the latest event at or before
    /// `t` decides it.
    fn state_at(&self, pred: Sym, arity: usize, t: i64) -> Vec<Vec<ObjectId>> {
        let Some(events) = self.history.get(&(pred, arity)) else {
            return Vec::new();
        };
        let mut state: BTreeMap<Vec<ObjectId>, (i64, bool)> = BTreeMap::new();
        for (at, assert, tuple) in events {
            if *at > t {
                continue;
            }
            match state.get(tuple) {
                Some((prev, _)) if prev > at => {}
                _ => {
                    state.insert(tuple.clone(), (*at, *assert));
                }
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
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Option<bool> {
        let p = desym(pred)?;
        let canon: Vec<ObjectId> = args.iter().map(|a| self.canonical(*a)).collect();
        let found = self.rels.get(&(p, args.len())).is_some_and(|t| t.contains(&canon));
        if found {
            return Some(true);
        }
        // Absence is refutation only where knowledge is complete.
        self.closed.contains(&p).then_some(false)
    }

    fn known_at(&self, pred: ObjectId, args: &[ObjectId], t: i64) -> Option<bool> {
        let p = desym(pred)?;
        let tuples = self.state_at(p, args.len(), t);
        let canon: Vec<ObjectId> = args.iter().map(|a| self.canonical(*a)).collect();
        if tuples.contains(&canon) {
            return Some(true);
        }
        self.closed.contains(&p).then_some(false)
    }

    fn extension(&self, domain: ObjectId) -> Option<Vec<ObjectId>> {
        self.sorts.get(&desym(domain)?).cloned()
    }

    fn is_closed(&self, pred: ObjectId) -> bool {
        desym(pred).is_some_and(|p| self.closed.contains(&p))
    }

    fn instants(&self) -> Vec<i64> {
        self.instants.clone()
    }

    fn snapshot(&self) -> u64 {
        // The latest instant doubles as the universe version: any write moves
        // it, so results computed before and after are distinguishable.
        self.instants.last().copied().unwrap_or(0) as u64
    }
}

impl RelationalView {
    /// Equivalence-class representative, so an asserted `is(a, b)` acts.
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
