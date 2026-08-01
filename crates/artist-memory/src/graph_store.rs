//! Persistence for the universal object graph.
//!
//! This closes the defect that motivated the rewrite: `assert_stmt` took
//! `pred: Sym` and `args: &[Sym]`, so a *formula* could not be stored at all.
//! Lambdas, quantifiers, fixpoints, domains, quoted propositions, continuations
//! and — fatally — the residual of a partially evaluated query all had nowhere
//! to go. The shrinking-residual design depended on storing an object the store
//! could not represent.
//!
//! There is now exactly one representation. A fact is not a privileged row; it
//! is an ordinary expression node, `(prefers adam tabs)`, stored the same way a
//! query or a continuation is.

use anyhow::{Result, anyhow};
use artist_logic::object::{Binder, CoreNode, ExternalRef, LiteralValue, ObjectGraph, ObjectId};
use cozo::DataValue;
use num_bigint::BigInt;
use std::collections::BTreeMap;

use crate::store::MemoryStore;

/// Relations for the object graph. Applied alongside the rest of the schema.
pub const GRAPH_RELATIONS: &[(&str, &str)] = &[
    (
        "object",
        r#"
        :create object {
            hi: Int, lo: Int
            =>
            kind: Int,
            name: String,
            has_name: Bool,
            payload: Bytes,
            created_at: Float default now(),
        }
        "#,
    ),
    (
        "operand",
        r#":create operand { hi: Int, lo: Int, position: Int => value_hi: Int, value_lo: Int }"#,
    ),
    (
        "binder_var",
        r#"
        :create binder_var {
            hi: Int, lo: Int, position: Int
            =>
            dom_hi: Int? default null, dom_lo: Int? default null,
        }
        "#,
    ),
];

const KIND_ATOM: i64 = 1;
const KIND_LITERAL: i64 = 2;
const KIND_APPLY: i64 = 3;
const KIND_BIND: i64 = 4;
const KIND_EXTERNAL: i64 = 5;
const KIND_OPAQUE: i64 = 6;

/// `u128` does not fit a Cozo `Int`, so ids travel as a high/low pair. The
/// identity space stays 128-bit; only the encoding is split.
fn split(id: ObjectId) -> (i64, i64) {
    ((id.0 >> 64) as i64, (id.0 & u64::MAX as u128) as i64)
}

fn join(hi: i64, lo: i64) -> ObjectId {
    ObjectId(((hi as u64 as u128) << 64) | (lo as u64 as u128))
}

/// Persist everything reachable from `root`. Cycles and sharing survive because
/// nodes are stored by id, never inlined.
pub async fn store_expression(
    store: &MemoryStore,
    g: &ObjectGraph,
    root: ObjectId,
) -> Result<ObjectId> {
    let mut objects = Vec::new();
    let mut operands = Vec::new();
    let mut binder_vars = Vec::new();

    for id in g.reachable(root) {
        let Some(node) = g.get(id) else { continue };
        let (hi, lo) = split(id);
        let (kind, name, has_name, payload) = match node {
            CoreNode::Atom { name } => (
                KIND_ATOM,
                name.clone().unwrap_or_default(),
                name.is_some(),
                Vec::new(),
            ),
            CoreNode::Literal(v) => (KIND_LITERAL, String::new(), false, encode_literal(v)),
            CoreNode::Apply { operator, operands: ops } => {
                for (i, o) in ops.iter().enumerate() {
                    let (vh, vl) = split(*o);
                    operands.push(row5(hi, lo, i as i64, vh, vl));
                }
                // Position -1 holds the operator, so an application with no
                // operands still records its head.
                let (oh, ol) = split(*operator);
                operands.push(row5(hi, lo, -1, oh, ol));
                (KIND_APPLY, String::new(), false, Vec::new())
            }
            CoreNode::Bind { binder, vars, bodies } => {
                let (bh, bl) = split(*binder);
                operands.push(row5(hi, lo, -1, bh, bl));
                for (i, b) in bodies.iter().enumerate() {
                    let (vh, vl) = split(*b);
                    operands.push(row5(hi, lo, i as i64, vh, vl));
                }
                for (i, b) in vars.iter().enumerate() {
                    let (dh, dl) = match b.domain {
                        Some(d) => {
                            let (a, c) = split(d);
                            (DataValue::from(a), DataValue::from(c))
                        }
                        None => (DataValue::Null, DataValue::Null),
                    };
                    binder_vars.push(DataValue::List(vec![
                        DataValue::from(hi),
                        DataValue::from(lo),
                        DataValue::from(i as i64),
                        dh,
                        dl,
                    ]));
                }
                (KIND_BIND, String::new(), false, Vec::new())
            }
            CoreNode::External(r) => (KIND_EXTERNAL, r.namespace.clone(), true, encode_external(r)),
            CoreNode::Opaque { tag, payload } => {
                (KIND_OPAQUE, tag.clone(), true, payload.clone())
            }
        };
        objects.push(DataValue::List(vec![
            DataValue::from(hi),
            DataValue::from(lo),
            DataValue::from(kind),
            DataValue::from(name.as_str()),
            DataValue::Bool(has_name),
            DataValue::Bytes(payload),
        ]));
    }

    put(store, "?[hi, lo, kind, name, has_name, payload] <- $rows
         :put object {hi, lo => kind, name, has_name, payload}", objects).await?;
    put(store, "?[hi, lo, position, value_hi, value_lo] <- $rows
         :put operand {hi, lo, position => value_hi, value_lo}", operands).await?;
    put(store, "?[hi, lo, position, dom_hi, dom_lo] <- $rows
         :put binder_var {hi, lo, position => dom_hi, dom_lo}", binder_vars).await?;
    Ok(root)
}

/// Load `root` and everything under it. Ids are preserved, so a reloaded graph
/// is interchangeable with the one that was stored.
pub async fn load_expression(
    store: &MemoryStore,
    g: &mut ObjectGraph,
    root: ObjectId,
) -> Result<ObjectId> {
    load_all(store, g).await?;
    Ok(root)
}

/// Load **every** stored object into `g`, returning the ids.
///
/// `load_expression` always read the whole table anyway — it queried `*object`
/// unfiltered and then handed back the root it was given. Naming that shape is
/// what lets [`crate::relational::RelationalView`] index stored *rules*, which
/// live in this table and are not reachable from `stmt`/`arg` at all.
pub async fn load_all(store: &MemoryStore, g: &mut ObjectGraph) -> Result<Vec<ObjectId>> {
    let objects = store
        .script(
            "?[hi, lo, kind, name, has_name, payload] :=
                *object{hi, lo, kind, name, has_name, payload}",
            BTreeMap::new(),
            false,
        )
        .await?;
    let operands = store
        .script(
            "?[hi, lo, position, value_hi, value_lo] :=
                *operand{hi, lo, position, value_hi, value_lo}",
            BTreeMap::new(),
            false,
        )
        .await?;
    let binder_vars = store
        .script(
            "?[hi, lo, position, dom_hi, dom_lo] :=
                *binder_var{hi, lo, position, dom_hi, dom_lo}",
            BTreeMap::new(),
            false,
        )
        .await?;

    let mut loaded = Vec::new();
    // Bucket children by owner, keeping position order.
    let mut ops: BTreeMap<ObjectId, BTreeMap<i64, ObjectId>> = BTreeMap::new();
    for r in &operands.rows {
        let owner = join(int(r, 0)?, int(r, 1)?);
        ops.entry(owner)
            .or_default()
            .insert(int(r, 2)?, join(int(r, 3)?, int(r, 4)?));
    }
    let mut vars: BTreeMap<ObjectId, BTreeMap<i64, Binder>> = BTreeMap::new();
    for r in &binder_vars.rows {
        let owner = join(int(r, 0)?, int(r, 1)?);
        let domain = match (r.get(3), r.get(4)) {
            (Some(DataValue::Null), _) | (None, _) => None,
            (Some(a), Some(b)) => Some(join(
                a.get_int().ok_or_else(|| anyhow!("dom_hi"))?,
                b.get_int().ok_or_else(|| anyhow!("dom_lo"))?,
            )),
            _ => None,
        };
        vars.entry(owner).or_default().insert(int(r, 2)?, Binder { domain });
    }

    for r in &objects.rows {
        let id = join(int(r, 0)?, int(r, 1)?);
        let kind = int(r, 2)?;
        let name = r.get(3).and_then(DataValue::get_str).unwrap_or("").to_string();
        let has_name = matches!(r.get(4), Some(DataValue::Bool(true)));
        let payload = bytes(r, 5);

        let node = match kind {
            KIND_ATOM => CoreNode::Atom { name: has_name.then_some(name) },
            KIND_LITERAL => CoreNode::Literal(decode_literal(&payload)?),
            KIND_APPLY => {
                let children = ops.get(&id).cloned().unwrap_or_default();
                let operator = *children.get(&-1).ok_or_else(|| anyhow!("apply has no head"))?;
                CoreNode::Apply {
                    operator,
                    operands: children
                        .iter()
                        .filter(|(p, _)| **p >= 0)
                        .map(|(_, v)| *v)
                        .collect(),
                }
            }
            KIND_BIND => {
                let children = ops.get(&id).cloned().unwrap_or_default();
                let binder = *children.get(&-1).ok_or_else(|| anyhow!("bind has no binder"))?;
                CoreNode::Bind {
                    binder,
                    vars: vars
                        .get(&id)
                        .map(|m| m.values().cloned().collect())
                        .unwrap_or_default(),
                    bodies: children
                        .iter()
                        .filter(|(p, _)| **p >= 0)
                        .map(|(_, v)| *v)
                        .collect(),
                }
            }
            KIND_EXTERNAL => CoreNode::External(decode_external(&name, &payload)?),
            KIND_OPAQUE => CoreNode::Opaque { tag: name, payload },
            other => return Err(anyhow!("unknown object kind {other}")),
        };
        g.define(id, node);
        loaded.push(id);
    }
    Ok(loaded)
}

async fn put(store: &MemoryStore, script: &'static str, rows: Vec<DataValue>) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut params = BTreeMap::new();
    params.insert("rows".to_string(), DataValue::List(rows));
    store.script(script, params, true).await?;
    Ok(())
}

fn row5(hi: i64, lo: i64, pos: i64, vh: i64, vl: i64) -> DataValue {
    DataValue::List(vec![
        DataValue::from(hi),
        DataValue::from(lo),
        DataValue::from(pos),
        DataValue::from(vh),
        DataValue::from(vl),
    ])
}

fn int(row: &[DataValue], i: usize) -> Result<i64> {
    row.get(i)
        .and_then(DataValue::get_int)
        .ok_or_else(|| anyhow!("expected int at column {i}"))
}

fn bytes(row: &[DataValue], i: usize) -> Vec<u8> {
    match row.get(i) {
        Some(DataValue::Bytes(b)) => b.clone(),
        _ => Vec::new(),
    }
}

fn encode_literal(v: &LiteralValue) -> Vec<u8> {
    let mut out = Vec::new();
    match v {
        LiteralValue::Int(n) => {
            out.push(0);
            out.extend_from_slice(&n.to_signed_bytes_le());
        }
        LiteralValue::Decimal { mantissa, scale } => {
            out.push(1);
            out.extend_from_slice(&scale.to_le_bytes());
            out.extend_from_slice(&mantissa.to_signed_bytes_le());
        }
        LiteralValue::Text(s) => {
            out.push(2);
            out.extend_from_slice(s.as_bytes());
        }
        LiteralValue::Bool(b) => {
            out.push(3);
            out.push(*b as u8);
        }
        LiteralValue::Bytes(b) => {
            out.push(4);
            out.extend_from_slice(b);
        }
    }
    out
}

fn decode_literal(b: &[u8]) -> Result<LiteralValue> {
    let (tag, rest) = b.split_first().ok_or_else(|| anyhow!("empty literal"))?;
    Ok(match tag {
        0 => LiteralValue::Int(BigInt::from_signed_bytes_le(rest)),
        1 => {
            let (s, m) = rest.split_at(4);
            let mut sa = [0u8; 4];
            sa.copy_from_slice(s);
            LiteralValue::Decimal {
                mantissa: BigInt::from_signed_bytes_le(m),
                scale: i32::from_le_bytes(sa),
            }
        }
        2 => LiteralValue::Text(String::from_utf8(rest.to_vec())?),
        3 => LiteralValue::Bool(rest.first().copied().unwrap_or(0) == 1),
        4 => LiteralValue::Bytes(rest.to_vec()),
        other => return Err(anyhow!("unknown literal tag {other}")),
    })
}

fn encode_external(r: &ExternalRef) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(r.locator.len() as u32).to_le_bytes());
    out.extend_from_slice(&r.locator);
    match &r.version {
        Some(v) => {
            out.push(1);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            out.extend_from_slice(v);
        }
        None => out.push(0),
    }
    match &r.digest {
        Some(d) => {
            out.push(1);
            out.extend_from_slice(d);
        }
        None => out.push(0),
    }
    out
}

fn decode_external(namespace: &str, b: &[u8]) -> Result<ExternalRef> {
    let mut i = 0usize;
    let take = |i: &mut usize, n: usize| -> Result<Vec<u8>> {
        if *i + n > b.len() {
            return Err(anyhow!("truncated external ref"));
        }
        let v = b[*i..*i + n].to_vec();
        *i += n;
        Ok(v)
    };
    let len = u32::from_le_bytes(take(&mut i, 4)?.try_into().map_err(|_| anyhow!("len"))?) as usize;
    let locator = take(&mut i, len)?;
    let version = if take(&mut i, 1)?[0] == 1 {
        let n =
            u32::from_le_bytes(take(&mut i, 4)?.try_into().map_err(|_| anyhow!("len"))?) as usize;
        Some(take(&mut i, n)?)
    } else {
        None
    };
    let digest = if take(&mut i, 1)?[0] == 1 {
        let d = take(&mut i, 32)?;
        let mut a = [0u8; 32];
        a.copy_from_slice(&d);
        Some(a)
    } else {
        None
    };
    Ok(ExternalRef { namespace: namespace.to_string(), locator, version, digest })
}
