//! Persisting *claims about* expressions, as distinct from the expressions.
//!
//! `graph_store` persists the object graph. Nothing about a node being present
//! says whether it is believed: the same graph serves as a queried proposition,
//! a quoted one inside `(believes sarah (quote P))`, a residual left by a
//! budget-exhausted evaluation, and an asserted belief. Collapsing those was the
//! mention/use confusion — "the stored belief and the question about it are the
//! same object" is true of the *expression* and false of the *record*.
//!
//! So an assertion is its own row, with its own derived identity. **It is not
//! keyed by proposition.** The same proposition is routinely asserted by
//! different agents, in different worlds, over different intervals, with
//! opposite polarity; keying on the proposition would let one of those
//! overwrite another, which is data loss disguised as idempotence.

use anyhow::{Result, anyhow};
use artist_logic::evidence::{Assertion, Polarity};
use artist_logic::ObjectId;
use cozo::DataValue;
use std::collections::BTreeMap;

use crate::store::MemoryStore;

fn split(id: ObjectId) -> (i64, i64) {
    ((id.0 >> 64) as i64, (id.0 & u64::MAX as u128) as i64)
}

fn join(hi: i64, lo: i64) -> ObjectId {
    ObjectId(((hi as u64 as u128) << 64) | (lo as u64 as u128))
}

fn opt_pair(v: Option<ObjectId>) -> (DataValue, DataValue) {
    match v {
        Some(id) => {
            let (h, l) = split(id);
            (DataValue::from(h), DataValue::from(l))
        }
        None => (DataValue::Null, DataValue::Null),
    }
}

fn opt_int(v: Option<i64>) -> DataValue {
    v.map(DataValue::from).unwrap_or(DataValue::Null)
}

fn read_pair(row: &[DataValue], hi: usize, lo: usize) -> Option<ObjectId> {
    match (row.get(hi), row.get(lo)) {
        (Some(DataValue::Null), _) | (None, _) => None,
        (Some(a), Some(b)) => Some(join(a.get_int()?, b.get_int()?)),
        _ => None,
    }
}

/// Record an assertion. Idempotent on its derived id, so re-recording the same
/// claim by the same agent at the same instant writes one row, while changing
/// any field writes a different one.
pub async fn record_assertion(store: &MemoryStore, a: &Assertion) -> Result<ObjectId> {
    let id = a.derive_id();
    let (hi, lo) = split(id);
    let (ph, pl) = split(a.proposition);
    let (agh, agl) = opt_pair(a.asserting_agent);
    let (wh, wl) = opt_pair(a.world);
    let (mh, ml) = opt_pair(a.modality);
    let (sh, sl) = opt_pair(a.scope);

    let row = DataValue::List(vec![
        DataValue::from(hi),
        DataValue::from(lo),
        DataValue::from(ph),
        DataValue::from(pl),
        DataValue::from(matches!(a.polarity, Polarity::Affirm)),
        agh,
        agl,
        wh,
        wl,
        opt_int(a.valid_from),
        opt_int(a.valid_to),
        DataValue::from(a.recorded_at),
        mh,
        ml,
        sh,
        sl,
    ]);

    let mut params = BTreeMap::new();
    params.insert("rows".to_string(), DataValue::List(vec![row]));
    store
        .script_owned(
            "?[hi, lo, prop_hi, prop_lo, affirmed, agent_hi, agent_lo, world_hi, world_lo, \
              valid_from, valid_to, recorded_at, modality_hi, modality_lo, scope_hi, scope_lo] \
             <- $rows \
             :put assertion {hi, lo => prop_hi, prop_lo, affirmed, agent_hi, agent_lo, \
              world_hi, world_lo, valid_from, valid_to, recorded_at, modality_hi, modality_lo, \
              scope_hi, scope_lo}"
                .to_string(),
            params,
            true,
        )
        .await?;
    Ok(id)
}

/// Every assertion *about* `proposition`.
///
/// Plural by construction: this is the query the keying decision exists to
/// make answerable. Two agents disagreeing about one proposition are two rows,
/// and both come back.
pub async fn assertions_about(
    store: &MemoryStore,
    proposition: ObjectId,
) -> Result<Vec<Assertion>> {
    let (ph, pl) = split(proposition);
    let mut params = BTreeMap::new();
    params.insert("ph".to_string(), DataValue::from(ph));
    params.insert("pl".to_string(), DataValue::from(pl));

    let rows = store
        .script_owned(
            "?[hi, lo, prop_hi, prop_lo, affirmed, agent_hi, agent_lo, world_hi, world_lo, \
              valid_from, valid_to, recorded_at, modality_hi, modality_lo, scope_hi, scope_lo] := \
              *assertion{hi, lo, prop_hi, prop_lo, affirmed, agent_hi, agent_lo, world_hi, \
               world_lo, valid_from, valid_to, recorded_at, modality_hi, modality_lo, \
               scope_hi, scope_lo}, prop_hi = $ph, prop_lo = $pl"
                .to_string(),
            params,
            false,
        )
        .await?;

    rows.rows
        .iter()
        .map(|r| {
            let get = |i: usize| -> Result<i64> {
                r.get(i)
                    .and_then(|v| v.get_int())
                    .ok_or_else(|| anyhow!("assertion column {i}"))
            };
            Ok(Assertion {
                id: join(get(0)?, get(1)?),
                proposition: join(get(2)?, get(3)?),
                polarity: match r.get(4) {
                    Some(DataValue::Bool(true)) => Polarity::Affirm,
                    _ => Polarity::Deny,
                },
                asserting_agent: read_pair(&r[..], 5, 6),
                world: read_pair(&r[..], 7, 8),
                valid_from: r.get(9).and_then(|v| v.get_int()),
                valid_to: r.get(10).and_then(|v| v.get_int()),
                recorded_at: get(11)?,
                modality: read_pair(&r[..], 12, 13),
                interpretation: None,
                scope: read_pair(&r[..], 14, 15),
            })
        })
        .collect()
}
