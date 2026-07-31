//! The universal object graph.
//!
//! One identity space, four node shapes, and no closed list of constructs.
//!
//! The recurring defect this replaces: extend a closed Rust enum, declare the
//! ceiling removed, then discover the *next* surrounding enum silently
//! reinstated it. That happened to `Domain`, to second-order arity, to
//! `Literal`, to a powerset bit width, and to `Sym`. The fix is not another
//! variant — it is to stop letting Rust's type system decide what the language
//! can say.
//!
//! Here `Forall`, `Lambda`, `LetRec`, `At`, `Count`, `Quote`, arithmetic, the
//! modal operators and every future construct are **operator objects**, not
//! variants. An unknown operator still loads, prints, hashes, round-trips and
//! can be quantified over; only *evaluation* may report that it has no
//! semantics. Representation is total; evaluation is what is allowed to be
//! partial.
//!
//! Three properties fall out of the shape rather than being enforced:
//!
//! * **No variable capture.** A variable is an ordinary object with its own
//!   identity, so two distinct binders cannot collide however they are nested,
//!   substituted or serialized.
//! * **Cycles are representable.** [`ObjectGraph::alloc`] hands out an id before
//!   the body exists, so an object may refer to itself. Self-referential
//!   propositions are representable even where no classical evaluator can assign
//!   them a consistent truth value.
//! * **Sharing is free.** Operands are ids, so a subterm referenced a thousand
//!   times is stored once.

use num_bigint::BigInt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// 128-bit identity, shared by *everything*: entities, predicates, functions,
/// types, formulas, queries, variables, worlds, times, sources, operators,
/// proofs, evidence and continuations.
///
/// Deliberately not `Sym = u32`. A 32-bit symbol space is a ceiling, and it is
/// also the wrong *model* — it forced entities and formulas into different
/// namespaces, which is why a formula could not be stored beside a fact.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub u128);

impl std::fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{:032x}", self.0)
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{:x}", self.0)
    }
}

/// Values that carry no further structure. Integers are arbitrary-precision:
/// `i64` was a silent magnitude ceiling that degraded to `Unknown` at the
/// boundary instead of saying so.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LiteralValue {
    Int(BigInt),
    /// `mantissa × 10^-scale`. Exact, ordered, hashable — no floats, so a
    /// literal can key an evidence ledger.
    Decimal { mantissa: BigInt, scale: i32 },
    Text(String),
    Bool(bool),
    Bytes(Vec<u8>),
}

/// A direct denotation of something outside the language.
///
/// A file, a process, a person, a physical observation, a remote object, a
/// dataset, an oracle, an externally supplied mathematical object. The point is
/// that the language can *name* these without first reducing them to internal
/// syntax — and an unresolved reference is still a perfectly good expression.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ExternalRef {
    pub namespace: String,
    pub locator: Vec<u8>,
    pub version: Option<Vec<u8>>,
    pub digest: Option<[u8; 32]>,
}

/// One variable bound by a binder, with an optional domain expression.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Binding {
    pub var: ObjectId,
    pub domain: Option<ObjectId>,
}

/// The kernel. Everything the language can say is one of these.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CoreNode {
    /// A primitive: an entity, a predicate, an operator, a sort, a variable, a
    /// world, an instant. The name is for humans; identity is the id.
    Atom { name: Option<String> },
    Literal(LiteralValue),
    /// Operator application. The operator is itself an object, so new
    /// constructs need no enum variant and no storage migration.
    Apply { operator: ObjectId, operands: Vec<ObjectId> },
    /// Variable binding. `bodies` is a vector because binders differ in arity —
    /// `Forall` takes one, `LetRec` takes a definition *and* a scope.
    Bind { binder: ObjectId, vars: Vec<Binding>, bodies: Vec<ObjectId> },
    External(ExternalRef),
    /// A node from a producer this build does not understand, preserved
    /// verbatim so a round trip through an older binary is lossless.
    Opaque { tag: String, payload: Vec<u8> },
}

/// Well-known object ids.
///
/// Low, fixed, and content-address-free so they are stable across stores. Every
/// one of these is an ordinary object — the evaluator privileges them, the
/// *representation* does not.
pub mod wk {
    use super::ObjectId;

    macro_rules! wk {
        ($($name:ident = $v:expr, $label:expr;)*) => {
            $(pub const $name: ObjectId = ObjectId($v);)*
            /// Human-readable names for the reserved ids.
            pub const NAMES: &[(ObjectId, &str)] = &[$(($name, $label)),*];
        };
    }

    wk! {
        // --- logical connectives ---
        NOT = 1, "not"; AND = 2, "and"; OR = 3, "or"; IMPLIES = 4, "implies";
        EQ = 5, "="; LEQ = 6, "<="; TOP = 7, "true"; BOT = 8, "false";
        // --- binders ---
        FORALL = 16, "forall"; EXISTS = 17, "exists"; LAMBDA = 18, "lambda";
        LETREC = 19, "letrec"; COUNT = 20, "count"; SUM = 21, "sum";
        // --- arithmetic ---
        ADD = 32, "+"; MUL = 33, "*"; NEG = 34, "-"; DIV = 35, "/"; MOD = 36, "mod";
        // --- structured literals ---
        CONCAT = 48, "concat"; LEN = 49, "len"; SUBSTR = 50, "substr";
        CONTAINS = 51, "contains"; STARTS_WITH = 52, "starts-with";
        ENDS_WITH = 53, "ends-with";
        // --- quotation, denotation, evaluation (item 9) ---
        QUOTE = 64, "quote"; DENOTES = 65, "denotes"; HOLDS = 66, "holds";
        ASSERTED_BY = 67, "asserted-by"; BELIEVES = 68, "believes";
        EVAL = 69, "eval"; PROVABLE = 70, "provable";
        // --- time, worlds, modality (item 12) ---
        AT = 80, "at"; IN_WORLD = 81, "in-world"; NECESSARILY = 82, "necessarily";
        POSSIBLY = 83, "possibly"; COUNTERFACTUAL = 84, "if-counterfactually";
        BEFORE = 85, "before"; DURING = 86, "during"; ALWAYS = 87, "always";
        EVENTUALLY = 88, "eventually"; SINCE = 89, "since";
        // --- types (item 6) ---
        UNIVERSE = 96, "Universe"; FUNCTION_TYPE = 97, "->";
        RELATION_TYPE = 98, "Rel"; PROP_TYPE = 99, "Prop";
        EXPR_TYPE = 100, "Expr"; WORLD_TYPE = 101, "World";
        CONTEXT_TYPE = 102, "Context"; REFINEMENT_TYPE = 103, "Refine";
        PRODUCT_TYPE = 104, "Product"; SUM_TYPE = 105, "Sum";
        TYPE_OF = 106, "type-of"; INT_TYPE = 107, "Int"; NAT_TYPE = 108, "Nat";
        // --- domains as expressions ---
        SORT_DOMAIN = 112, "sort"; SET_DOMAIN = 113, "set";
        WHERE_DOMAIN = 114, "where"; INSTANTS_DOMAIN = 115, "instants";
        // --- evidence and provenance (item 13) ---
        SUPPORTS = 128, "supports"; ATTACKS = 129, "attacks";
        DERIVED_FROM = 130, "derived-from"; SOURCE = 131, "source";
        SAME_AS = 132, "same-as";
    }

    /// First id available to callers. Everything below is reserved, so the
    /// well-known operators keep stable ids across every store.
    pub const FIRST_FREE: ObjectId = ObjectId(4096);

    pub fn name_of(id: ObjectId) -> Option<&'static str> {
        NAMES.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
    }
}

/// A content-addressable, cycle-tolerant object store.
#[derive(Clone, Debug, Default)]
pub struct ObjectGraph {
    nodes: BTreeMap<ObjectId, CoreNode>,
    names: BTreeMap<String, ObjectId>,
    next_alloc: u128,
}

impl ObjectGraph {
    pub fn new() -> Self {
        let mut g = ObjectGraph {
            nodes: BTreeMap::new(),
            names: BTreeMap::new(),
            next_alloc: wk::FIRST_FREE.0,
        };
        for (id, label) in wk::NAMES {
            if label.is_empty() {
                continue;
            }
            g.nodes.insert(*id, CoreNode::Atom { name: Some((*label).to_string()) });
            g.names.insert((*label).to_string(), *id);
        }
        g
    }

    /// Intern a node by content. Structurally identical expressions share an id,
    /// so equality of pure structure is pointer equality.
    pub fn intern(&mut self, node: CoreNode) -> ObjectId {
        let id = content_id(&node);
        self.nodes.entry(id).or_insert(node);
        id
    }

    /// Reserve an id whose body is not yet known.
    ///
    /// This is what makes cycles, recursive definitions, streams and
    /// self-referential propositions representable: the id exists before the
    /// body, so the body may mention it.
    pub fn alloc(&mut self) -> ObjectId {
        let id = ObjectId(self.next_alloc);
        self.next_alloc += 1;
        id
    }

    /// Fill in a previously [`alloc`](Self::alloc)ated id.
    pub fn define(&mut self, id: ObjectId, node: CoreNode) {
        self.nodes.insert(id, node);
    }

    pub fn get(&self, id: ObjectId) -> Option<&CoreNode> {
        self.nodes.get(&id)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.nodes.keys().copied()
    }

    // ---- constructors ---------------------------------------------------

    /// A named atom, interned by name so the same name is the same object.
    pub fn atom(&mut self, name: &str) -> ObjectId {
        if let Some(id) = self.names.get(name) {
            return *id;
        }
        let id = self.intern(CoreNode::Atom { name: Some(name.to_string()) });
        self.names.insert(name.to_string(), id);
        id
    }

    /// A fresh anonymous atom. Variables use this, which is why capture is
    /// impossible: every binder's variable is a distinct object.
    pub fn fresh(&mut self) -> ObjectId {
        let id = self.alloc();
        self.define(id, CoreNode::Atom { name: None });
        id
    }

    pub fn lit(&mut self, v: LiteralValue) -> ObjectId {
        self.intern(CoreNode::Literal(v))
    }

    pub fn int(&mut self, n: i64) -> ObjectId {
        self.lit(LiteralValue::Int(BigInt::from(n)))
    }

    pub fn text(&mut self, s: &str) -> ObjectId {
        self.lit(LiteralValue::Text(s.to_string()))
    }

    pub fn boolean(&mut self, b: bool) -> ObjectId {
        self.lit(LiteralValue::Bool(b))
    }

    pub fn apply(&mut self, operator: ObjectId, operands: Vec<ObjectId>) -> ObjectId {
        self.intern(CoreNode::Apply { operator, operands })
    }

    pub fn bind(
        &mut self,
        binder: ObjectId,
        vars: Vec<Binding>,
        bodies: Vec<ObjectId>,
    ) -> ObjectId {
        self.intern(CoreNode::Bind { binder, vars, bodies })
    }

    pub fn external(&mut self, r: ExternalRef) -> ObjectId {
        self.intern(CoreNode::External(r))
    }

    /// Quantify one variable over a domain.
    pub fn quantify(
        &mut self,
        binder: ObjectId,
        var: ObjectId,
        domain: Option<ObjectId>,
        body: ObjectId,
    ) -> ObjectId {
        self.bind(binder, vec![Binding { var, domain }], vec![body])
    }

    // ---- traversal ------------------------------------------------------

    /// Direct children of a node.
    pub fn children(&self, id: ObjectId) -> Vec<ObjectId> {
        match self.get(id) {
            Some(CoreNode::Apply { operator, operands }) => {
                let mut v = vec![*operator];
                v.extend(operands.iter().copied());
                v
            }
            Some(CoreNode::Bind { binder, vars, bodies }) => {
                let mut v = vec![*binder];
                for b in vars {
                    v.push(b.var);
                    if let Some(d) = b.domain {
                        v.push(d);
                    }
                }
                v.extend(bodies.iter().copied());
                v
            }
            _ => Vec::new(),
        }
    }

    /// Every object reachable from `root`, cycle-safe.
    pub fn reachable(&self, root: ObjectId) -> BTreeSet<ObjectId> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            stack.extend(self.children(id));
        }
        seen
    }

    /// Whether `root` reaches itself — a genuinely cyclic expression.
    pub fn is_cyclic(&self, root: ObjectId) -> bool {
        self.children(root)
            .into_iter()
            .any(|c| c == root || self.reachable(c).contains(&root))
    }

    /// Copy `root` and everything under it into `dst`, preserving cycles and
    /// sharing.
    pub fn transplant(&self, root: ObjectId, dst: &mut ObjectGraph) -> ObjectId {
        let mut map: BTreeMap<ObjectId, ObjectId> = BTreeMap::new();
        for id in self.reachable(root) {
            // Reserve first so a cyclic body can refer to its own new id.
            let new = if id.0 < wk::FIRST_FREE.0 { id } else { dst.alloc() };
            map.insert(id, new);
        }
        for (old, new) in &map {
            if let Some(node) = self.get(*old) {
                let remapped = remap(node, &map);
                dst.define(*new, remapped);
            }
        }
        map.get(&root).copied().unwrap_or(root)
    }
}

fn remap(node: &CoreNode, map: &BTreeMap<ObjectId, ObjectId>) -> CoreNode {
    let m = |x: &ObjectId| map.get(x).copied().unwrap_or(*x);
    match node {
        CoreNode::Apply { operator, operands } => CoreNode::Apply {
            operator: m(operator),
            operands: operands.iter().map(m).collect(),
        },
        CoreNode::Bind { binder, vars, bodies } => CoreNode::Bind {
            binder: m(binder),
            vars: vars
                .iter()
                .map(|b| Binding { var: m(&b.var), domain: b.domain.as_ref().map(m) })
                .collect(),
            bodies: bodies.iter().map(m).collect(),
        },
        other => other.clone(),
    }
}

/// Content address of a node: blake3 over a canonical encoding, truncated to
/// 128 bits.
///
/// Ids at or below [`wk::FIRST_FREE`] are never produced here, so reserved
/// operators keep their stable small ids.
pub fn content_id(node: &CoreNode) -> ObjectId {
    let mut h = blake3::Hasher::new();
    encode(node, &mut h);
    let bytes = h.finalize();
    let mut buf = [0u8; 16];
    buf.copy_from_slice(&bytes.as_bytes()[..16]);
    let raw = u128::from_le_bytes(buf);
    ObjectId(raw.saturating_add(wk::FIRST_FREE.0))
}

fn encode(node: &CoreNode, h: &mut blake3::Hasher) {
    match node {
        CoreNode::Atom { name } => {
            h.update(b"atom\0");
            h.update(name.as_deref().unwrap_or("").as_bytes());
        }
        CoreNode::Literal(v) => {
            h.update(b"lit\0");
            match v {
                LiteralValue::Int(n) => {
                    h.update(b"i");
                    h.update(n.to_signed_bytes_le().as_slice());
                }
                LiteralValue::Decimal { mantissa, scale } => {
                    h.update(b"d");
                    h.update(mantissa.to_signed_bytes_le().as_slice());
                    h.update(&scale.to_le_bytes());
                }
                LiteralValue::Text(s) => {
                    h.update(b"t");
                    h.update(s.as_bytes());
                }
                LiteralValue::Bool(b) => {
                    h.update(b"b");
                    h.update(&[*b as u8]);
                }
                LiteralValue::Bytes(v) => {
                    h.update(b"y");
                    h.update(v);
                }
            }
        }
        CoreNode::Apply { operator, operands } => {
            h.update(b"app\0");
            h.update(&operator.0.to_le_bytes());
            for o in operands {
                h.update(&o.0.to_le_bytes());
            }
        }
        CoreNode::Bind { binder, vars, bodies } => {
            h.update(b"bind\0");
            h.update(&binder.0.to_le_bytes());
            for b in vars {
                h.update(&b.var.0.to_le_bytes());
                h.update(&b.domain.map(|d| d.0).unwrap_or(0).to_le_bytes());
            }
            for b in bodies {
                h.update(&b.0.to_le_bytes());
            }
        }
        CoreNode::External(r) => {
            h.update(b"ext\0");
            h.update(r.namespace.as_bytes());
            h.update(&r.locator);
            if let Some(v) = &r.version {
                h.update(v);
            }
            if let Some(d) = &r.digest {
                h.update(d);
            }
        }
        CoreNode::Opaque { tag, payload } => {
            h.update(b"opaque\0");
            h.update(tag.as_bytes());
            h.update(payload);
        }
    }
}
