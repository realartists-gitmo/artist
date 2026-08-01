//! The universal object graph.
//!
//! One identity space, six node shapes, and no closed list of constructs.
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
/// **The space is partitioned by its top two bits**, so the three ways an id
/// can come into existence cannot collide with each other:
///
/// ```text
/// 00…  well-known   fixed, hand-assigned, below FIRST_FREE
/// 01…  content      blake3 over canonical structure
/// 10…  nominal      collision-resistant random, for things with no structure
/// 11…  reserved
/// ```
///
/// Without this, a random nominal id could land on a content-derived one and a
/// skolem would silently *become* a stored expression. Partitioning makes that
/// impossible rather than improbable.
///
/// Note "collision-resistant", not "unique". Randomness gives no guarantee of
/// distributed uniqueness; it gives a probability, and the honest claim is the
/// same one content addressing already rests on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub u128);

/// Bit position of the two-bit namespace tag.
const TAG_SHIFT: u32 = 126;
/// Everything below the tag.
const PAYLOAD_MASK: u128 = (1u128 << TAG_SHIFT) - 1;

/// Which namespace an id belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdSpace {
    /// Reserved, hand-assigned, stable across every store.
    WellKnown,
    /// Derived from structure; the same expression gets this id everywhere.
    Content,
    /// Random; names something with no structure to be identified by — a
    /// variable, a skolem, or a node whose body does not exist yet.
    Nominal,
    /// Unassigned.
    Reserved,
}

impl ObjectId {
    pub const TAG_WELL_KNOWN: u128 = 0 << TAG_SHIFT;
    pub const TAG_CONTENT: u128 = 1 << TAG_SHIFT;
    pub const TAG_NOMINAL: u128 = 2 << TAG_SHIFT;

    /// Build an id in `space` from `payload`, discarding any high bits the tag
    /// occupies.
    pub fn tagged(space: IdSpace, payload: u128) -> ObjectId {
        let tag = match space {
            IdSpace::WellKnown => Self::TAG_WELL_KNOWN,
            IdSpace::Content => Self::TAG_CONTENT,
            IdSpace::Nominal => Self::TAG_NOMINAL,
            IdSpace::Reserved => 3u128 << TAG_SHIFT,
        };
        ObjectId(tag | (payload & PAYLOAD_MASK))
    }

    pub fn space(self) -> IdSpace {
        match self.0 >> TAG_SHIFT {
            0 => IdSpace::WellKnown,
            1 => IdSpace::Content,
            2 => IdSpace::Nominal,
            _ => IdSpace::Reserved,
        }
    }
}

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

/// One bound slot **as stored**: a domain, and nothing else.
///
/// There is deliberately no variable id here. A stored binder is in de Bruijn
/// form — occurrences inside the body are `(bvar k)`, counting slots outward
/// from the occurrence — so `(forall ((f Path)) (p f))` and
/// `(forall ((x Path)) (p x))` are *the same stored structure* and content-address
/// to one id.
///
/// Keeping a variable id here, or a display name excluded from the hash, would
/// break content addressing outright: two alpha-equivalent nodes would share an
/// id while differing in bytes, and `intern`'s first-writer-wins would pick
/// between them by accident of ordering. A caller who built with `#A` could
/// then read back a node mentioning `#B`, leaving its own references dangling.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Binder {
    pub domain: Option<ObjectId>,
}

/// One variable bound by a binder, **as written by a caller**.
///
/// This is the builder-side view: you name a variable, build a body that
/// mentions it, and [`ObjectGraph::bind`] abstracts the name away. It is not
/// what gets stored — see [`Binder`] — and the round trip back to this form is
/// [`ObjectGraph::open_binder`].
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
    Bind { binder: ObjectId, vars: Vec<Binder>, bodies: Vec<ObjectId> },
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
        // A bound occurrence: `(bvar k)`, k counting slots outward.
        BVAR = 22, "bvar";
        // --- arithmetic ---
        ADD = 32, "+"; MUL = 33, "*"; NEG = 34, "-"; DIV = 35, "/"; MOD = 36, "mod";
        // --- structured literals ---
        CONCAT = 48, "concat"; LEN = 49, "len"; SUBSTR = 50, "substr";
        CONTAINS = 51, "contains"; STARTS_WITH = 52, "starts-with";
        ENDS_WITH = 53, "ends-with";
        // Ordered data. `set` is a *domain* and cannot be an argument; there was
        // no term-level sequence at all, so an argv, the frames of a stack
        // trace, or the steps of a release process had to become either a text
        // literal (structure lost) or `(arg cmd 0 "cargo")` triples.
        SEQ = 54, "seq"; NTH = 55, "nth";
        // --- quotation, denotation, evaluation (item 9) ---
        QUOTE = 64, "quote"; DENOTES = 65, "denotes"; HOLDS = 66, "holds";
        ASSERTED_BY = 67, "asserted-by"; BELIEVES = 68, "believes";
        EVAL = 69, "eval"; PROVABLE = 70, "provable";
        // --- the axes, as questions ---
        //
        // The evaluator computes grounding, derivation, determinacy and the
        // defeater set on **every** query and had a term for none of them, so a
        // memory that works out its own epistemic status could not write any of
        // it down. `provable` was the only reflective operator and it reads one
        // bit of one axis.
        //
        // Each is a claim *about* a proposition, so each is intensional in its
        // argument and — this is the part that is easy to get wrong — each is
        // itself an ordinary grounded, observed claim. `(grounded L)` for the
        // liar is **Refuted**, not ungrounded: a report about a sentence with no
        // stable value is not itself a sentence with no stable value.
        GROUNDED = 71, "grounded"; DEFEASIBLE = 72, "defeasible";
        DEFEATED_BY = 73, "defeated-by"; DETERMINATE = 74, "determinate";
        PRESUMED = 75, "presumed"; LIKELY = 76, "likely";
        // `(defeaters P)` as a *domain*: what would change my mind about P,
        // enumerable and countable like any other collection. The binary
        // `defeated-by` can only check a candidate you already have in hand.
        DEFEATERS_DOMAIN = 77, "defeaters";
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
        // `set` is exhaustive by construction; `set-partial` is the same
        // enumeration *without* that promise. Suspending a scan over a partial
        // index used to rebuild the remaining members as a `set`, so resuming
        // asked a strictly stronger question than the one that was suspended —
        // a universal that was honestly `Open` before the budget ran out came
        // back `Supported/Exact` from its own continuation.
        SET_PARTIAL = 116, "set-partial";
        // --- evidence and provenance (item 13) ---
        SUPPORTS = 128, "supports"; ATTACKS = 129, "attacks";
        DERIVED_FROM = 130, "derived-from"; SOURCE = 131, "source";
        SAME_AS = 132, "same-as";
        // --- quantities and intervals ---
        //
        // The *vocabulary* of units is deliberately not here. `minutes`, `mb`
        // and `commits` are ordinary atoms, and conversions are ordinary stored
        // facts — `(scale minutes 60 seconds)`. Only the two structural forms
        // are reserved, so a new unit costs a write and never a migration.
        QUANTITY = 144, "quantity"; SCALE = 145, "scale";
        // --- defeasibility ---
        //
        // A coding agent's memory is mostly defaults and preferences, and a
        // default that cannot be overridden is just a fact. `usually` holds
        // absent counter-evidence, `unless` names an exception that defeats it,
        // and `prefer` orders options.
        USUALLY = 147, "usually"; UNLESS = 148, "unless"; PREFER = 149, "prefer";
        INTERVAL = 146, "interval";
        // --- norms ---
        //
        // A norm is **not** truth in all accessible worlds, and modelling it
        // that way is worse than not modelling it: smuggling an obligation into
        // `necessarily` reports the rule as *false* the moment somebody breaks
        // it, which is the one thing a norm must not do in a memory whose job is
        // recording what actually happened. "Run `cargo fmt` before committing"
        // stays true on the day it is skipped; what changes is that `violated`
        // starts holding.
        OBLIGED = 150, "obliged"; PERMITTED = 151, "permitted";
        FORBIDDEN = 152, "forbidden"; VIOLATED = 153, "violated";
        // --- admissibility ---
        //
        // Claims *about a proposition*: that it has no sharp satisfaction
        // condition here, or that it has an established one. Reserved because
        // they must reach the belief layer — a declaration is an assertion like
        // any other, so it wants attribution, valid time, retraction and
        // conflict, and only reserved operators are indexed there.
        //
        // Their arguments are intensional: `(indeterminate P)` is about the
        // proposition, and rewriting inside it through `same-as` would change
        // which claim is on file.
        INDETERMINATE = 154, "indeterminate"; TOTAL = 155, "total";
        // --- certificates as terms ---
        //
        // A derivation was a Rust struct, so §5.6's claim that the answer to
        // *"why do you believe this"* is "an object the memory can store,
        // transmit, and re-check" held only for a caller inside the same
        // process. Encoded here it is an ordinary expression: it persists
        // through the object table, prints through the same printer, and — the
        // point — is **queryable**. "Which conclusions rest on the assertion I
        // am about to retract" is a question about a graph, and a certificate
        // that is not in the graph cannot answer it.
        //
        // `(proof step …)`, each step `(step <kind> …)`. The kinds are reserved
        // rather than ordinary atoms so that a certificate written by one store
        // means the same thing in another; a content-addressed atom named
        // "by-told" would collide with a user's atom of the same name and, worse,
        // would be *absent* from a store that had never minted it.
        PROOF = 160, "proof"; STEP = 161, "step";
        BY_TOLD = 162, "by-told"; BY_AXIOM = 163, "by-axiom";
        BY_TAUTOLOGY = 164, "by-tautology"; BY_NEGATION = 165, "by-negation";
        BY_CONNECTIVE = 166, "by-connective"; BY_INSTANCE = 167, "by-instance";
        BY_EXHAUSTIVE = 168, "by-exhaustive"; BY_RULE = 169, "by-rule";
        BY_CONFLICT = 170, "by-conflict"; BY_UNGROUNDED = 171, "by-ungrounded";
        BY_UNESTABLISHED = 172, "by-unestablished";
        BY_DEFEASIBLE = 173, "by-defeasible";
    }

    /// First id available to callers. Everything below is reserved, so the
    /// well-known operators keep stable ids across every store.
    pub const FIRST_FREE: ObjectId = ObjectId(4096);

    pub fn name_of(id: ObjectId) -> Option<&'static str> {
        NAMES.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
    }

    /// Which argument positions of `op` are **extensional** — that is, which
    /// ones coreference may rewrite through.
    ///
    /// `None` means "every position", which is the answer for ordinary user
    /// predicates and for the logical operators.
    ///
    /// The barrier is per *position*, not per operator, and getting that wrong
    /// costs in both directions. `believes(agent, proposition)` is extensional
    /// in the agent and intensional in the proposition: block both and you lose
    /// coreference on agent names for no reason; block neither and "Lois
    /// believes Superman flies" silently becomes "Lois believes Clark Kent
    /// flies". A hand-written list of *operators* — which is what §7 used to
    /// describe — cannot express either half of that.
    ///
    /// Declaring it here rather than at each consumer is the point. The
    /// evaluator honoured the barrier by routing intensional operators around
    /// coreference entirely, and then `RelationalView::known` canonicalised
    /// every argument through the `is` union-find before looking the tuple up —
    /// so the rule held in one file and was undone in the next. Any layer that
    /// substitutes now asks the same function.
    ///
    /// Positions past the end of the mask are intensional, so a longer
    /// application than the declaration anticipated fails closed.
    pub fn extensional_positions(op: ObjectId) -> Option<&'static [bool]> {
        match op {
            // The proposition is mentioned; the agent, source or world is used.
            BELIEVES => Some(&[true, false]),
            ASSERTED_BY | SOURCE => Some(&[false, true]),
            // Both arguments are propositions being talked about.
            SUPPORTS | ATTACKS | DERIVED_FROM => Some(&[false, false]),
            // `(denotes expr value)` — the expression is named, the value is
            // an ordinary object.
            DENOTES => Some(&[false, true]),
            // A quotation is opaque in full, and so is what a norm is about:
            // rewriting inside `(obliged P)` would change which rule is on file.
            QUOTE | HOLDS | EVAL | PROVABLE | USUALLY => Some(&[false]),
            INDETERMINATE | TOTAL => Some(&[false]),
            // Reflection on the axes is *about* the proposition, exactly as
            // `provable` is. Rewriting inside `(defeasible P)` through
            // coreference would change which claim's derivation is being
            // reported.
            GROUNDED | DEFEASIBLE | DETERMINATE | PRESUMED | LIKELY => Some(&[false]),
            DEFEATERS_DOMAIN => Some(&[false]),
            // Both a proposition and a thing that would defeat it — and a
            // defeater is itself a proposition.
            DEFEATED_BY => Some(&[false, false]),
            OBLIGED | PERMITTED | FORBIDDEN | VIOLATED => Some(&[false]),
            NECESSARILY | POSSIBLY => Some(&[false]),
            // `(in-world w P)` and `(if-counterfactually P Q)` — the world is an
            // ordinary object, the propositions are not.
            IN_WORLD => Some(&[true, false]),
            COUNTERFACTUAL => Some(&[false, false]),
            _ => None,
        }
    }

    /// Whether coreference may rewrite argument `pos` of `op`.
    pub fn is_extensional_at(op: ObjectId, pos: usize) -> bool {
        match extensional_positions(op) {
            None => true,
            Some(mask) => mask.get(pos).copied().unwrap_or(false),
        }
    }
}

/// A content-addressable, cycle-tolerant object store.
///
/// `Clone` and `Default` are hand-written rather than derived, and both reseed
/// the nominal allocator. A derived `Clone` would hand the copy the same seed
/// and counter, so parent and copy would mint identical ids for unrelated
/// entities; a derived `Default` would produce a graph with a zero seed *and*
/// no well-known atoms, which is not a usable graph at all.
#[derive(Debug)]
pub struct ObjectGraph {
    nodes: BTreeMap<ObjectId, CoreNode>,
    names: BTreeMap<String, ObjectId>,
    /// Per-graph random seed for nominal ids. See [`ObjectGraph::alloc`].
    alloc_seed: [u8; 32],
    alloc_counter: u64,
}

impl Clone for ObjectGraph {
    fn clone(&self) -> Self {
        Self {
            nodes: self.nodes.clone(),
            names: self.names.clone(),
            alloc_seed: fresh_seed(),
            alloc_counter: 0,
        }
    }
}

impl Default for ObjectGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// 32 bytes from the OS. Falls back to a time-and-address mix only if the
/// syscall fails, which on Linux means the entropy pool is unavailable — rare,
/// and still better than a constant.
fn fresh_seed() -> [u8; 32] {
    let mut seed = [0u8; 32];
    if getrandom::fill(&mut seed).is_err() {
        let mut h = blake3::Hasher::new();
        h.update(b"artist-logic/seed-fallback\0");
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            h.update(&d.as_nanos().to_le_bytes());
        }
        h.update(&(&seed as *const _ as usize).to_le_bytes());
        seed.copy_from_slice(h.finalize().as_bytes());
    }
    seed
}

impl ObjectGraph {
    pub fn new() -> Self {
        let mut g = ObjectGraph {
            nodes: BTreeMap::new(),
            names: BTreeMap::new(),
            alloc_seed: fresh_seed(),
            alloc_counter: 0,
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
    ///
    /// **Globally unique, not sequential.** This used to be a counter starting
    /// at `FIRST_FREE`, so every freshly opened graph minted 4096, 4097, … —
    /// and two stores that each did so held different entities under identical
    /// ids. `Memory::search` already queries `global.rocks` and
    /// `project.rocks` together, so that was a live collision, not a
    /// hypothetical merge problem.
    ///
    /// The id is `blake3(domain ‖ seed ‖ counter)`: one entropy draw per graph
    /// rather than a syscall per allocation, expanded by a PRF we already
    /// depend on. Domain-separated from [`content_id`] so the nominal and
    /// content spaces are independent — a nominal id colliding with a content
    /// id is then exactly as unlikely as two content ids colliding, which is
    /// the assumption content addressing already rests on.
    pub fn alloc(&mut self) -> ObjectId {
        let mut h = blake3::Hasher::new();
        h.update(b"artist-logic/nominal\0");
        h.update(&self.alloc_seed);
        h.update(&self.alloc_counter.to_le_bytes());
        self.alloc_counter += 1;
        let bytes = h.finalize();
        let mut buf = [0u8; 16];
        buf.copy_from_slice(&bytes.as_bytes()[..16]);
        ObjectId::tagged(IdSpace::Nominal, u128::from_le_bytes(buf))
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

    /// Bind `vars` over `bodies`, abstracting the variable names away.
    ///
    /// Callers write the nominal style — mint a variable, build a body that
    /// mentions it, hand both here — and what gets stored is de Bruijn. The
    /// signature is unchanged from when variables *were* stored, which is what
    /// keeps this from rippling through every construction site.
    ///
    /// **Slot numbering.** Indices count slots outward from the occurrence, so
    /// `(bvar 0)` is the last slot of the innermost enclosing binder. Entering
    /// a binder with `n` slots shifts outer references by `n`. A flat index is
    /// used rather than a (depth, slot) pair because `vars` is a vector: "k
    /// binders up" cannot say *which* of that binder's variables is meant.
    ///
    /// **Scope.** Domains telescope — the domain of slot `j` sees slots
    /// `0..j`, so `(forall ((n Nat) (xs (vec-of n))) …)` is expressible. Every
    /// body sees every slot, which is what makes `letrec` recursive.
    pub fn bind(
        &mut self,
        binder: ObjectId,
        vars: Vec<Binding>,
        bodies: Vec<ObjectId>,
    ) -> ObjectId {
        let names: Vec<ObjectId> = vars.iter().map(|b| b.var).collect();
        let mut slots = Vec::with_capacity(vars.len());
        for (j, b) in vars.iter().enumerate() {
            // Telescoping: slot j's domain may mention slots 0..j.
            let domain = b.domain.map(|d| self.abstract_over(d, &names[..j], 0));
            slots.push(Binder { domain });
        }
        let bodies: Vec<ObjectId> = bodies
            .into_iter()
            .map(|b| self.abstract_over(b, &names, 0))
            .collect();
        self.intern(CoreNode::Bind { binder, vars: slots, bodies })
    }

    /// A bound occurrence: `(bvar k)`.
    pub fn bvar(&mut self, index: usize) -> ObjectId {
        let k = self.lit(LiteralValue::Int(BigInt::from(index)));
        self.apply(wk::BVAR, vec![k])
    }

    /// If `id` is `(bvar k)`, its index.
    pub fn as_bvar(&self, id: ObjectId) -> Option<usize> {
        let CoreNode::Apply { operator, operands } = self.get(id)? else {
            return None;
        };
        if *operator != wk::BVAR || operands.len() != 1 {
            return None;
        }
        match self.get(operands[0])? {
            CoreNode::Literal(LiteralValue::Int(n)) => usize::try_from(n.clone()).ok(),
            _ => None,
        }
    }

    /// Replace occurrences of `names` with `(bvar …)`, `depth` binders in.
    fn abstract_over(&mut self, id: ObjectId, names: &[ObjectId], depth: usize) -> ObjectId {
        if names.is_empty() {
            return id;
        }
        if let Some(k) = names.iter().position(|n| *n == id) {
            // Last-declared is index 0, so a later slot shadows an earlier one.
            return self.bvar(depth + (names.len() - 1 - k));
        }
        self.map_children(id, depth, &mut |g, child, d| g.abstract_over(child, names, d))
    }

    /// Replace `(bvar …)` with `names`, `depth` binders in. Inverse of
    /// [`Self::abstract_over`].
    fn instantiate(&mut self, id: ObjectId, names: &[ObjectId], depth: usize) -> ObjectId {
        if names.is_empty() {
            return id;
        }
        if let Some(k) = self.as_bvar(id) {
            if k >= depth && k - depth < names.len() {
                return names[names.len() - 1 - (k - depth)];
            }
            return id;
        }
        self.map_children(id, depth, &mut |g, child, d| g.instantiate(child, names, d))
    }

    /// Rebuild `id` with `f` applied to each child, tracking binder depth.
    ///
    /// Cyclic nodes are returned untouched: a cycle has no finite de Bruijn
    /// rewriting, and a self-referential proposition that also captures an
    /// enclosing variable is not something the language needs to abstract over.
    /// This is the explicit exception to "structurally identical expressions
    /// share an id" — see the note on `alloc`.
    fn map_children(
        &mut self,
        id: ObjectId,
        depth: usize,
        f: &mut impl FnMut(&mut Self, ObjectId, usize) -> ObjectId,
    ) -> ObjectId {
        if self.is_cyclic(id) {
            return id;
        }
        match self.get(id).cloned() {
            Some(CoreNode::Apply { operator, operands }) => {
                let operator = f(self, operator, depth);
                let operands = operands.into_iter().map(|o| f(self, o, depth)).collect();
                self.intern(CoreNode::Apply { operator, operands })
            }
            Some(CoreNode::Bind { binder, vars, bodies }) => {
                let inner = vars.len();
                let mut slots = Vec::with_capacity(inner);
                for (j, slot) in vars.iter().enumerate() {
                    // Slot j's domain sits under j of this binder's own slots.
                    let domain = slot.domain.map(|d| f(self, d, depth + j));
                    slots.push(Binder { domain });
                }
                let bodies = bodies
                    .into_iter()
                    .map(|b| f(self, b, depth + inner))
                    .collect();
                self.intern(CoreNode::Bind { binder, vars: slots, bodies })
            }
            _ => id,
        }
    }

    /// Open a stored binder into the nominal form callers and the evaluator
    /// work in: fresh variables substituted for `(bvar …)`.
    ///
    /// This is the other half of locally-nameless. Storage is canonical so that
    /// alpha-equivalent facts share an identity; traversal is named so that
    /// nothing downstream has to do index arithmetic.
    pub fn open_binder(
        &mut self,
        id: ObjectId,
    ) -> Option<(ObjectId, Vec<Binding>, Vec<ObjectId>)> {
        let Some(CoreNode::Bind { binder, vars, bodies }) = self.get(id).cloned() else {
            return None;
        };
        let fresh: Vec<ObjectId> = (0..vars.len()).map(|_| self.fresh()).collect();
        let mut out = Vec::with_capacity(vars.len());
        for (j, slot) in vars.iter().enumerate() {
            let domain = slot.domain.map(|d| self.instantiate(d, &fresh[..j], 0));
            out.push(Binding { var: fresh[j], domain });
        }
        let bodies = bodies
            .into_iter()
            .map(|b| self.instantiate(b, &fresh, 0))
            .collect();
        Some((binder, out, bodies))
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
    /// Every id this graph defines. Used to copy a whole loaded graph into
    /// another one, where "whole" is the point and there is no single root.
    pub fn all_ids(&self) -> Vec<ObjectId> {
        self.nodes.keys().copied().collect()
    }

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
                .map(|b| Binder { domain: b.domain.as_ref().map(m) })
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
    ObjectId::tagged(IdSpace::Content, u128::from_le_bytes(buf))
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
            // No variable id is hashed: a stored binder has none. That is
            // precisely what makes alpha-equivalent expressions one object.
            for b in vars {
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
