//! The two-sided anytime evaluator, over the universal object graph.
//!
//! This closes the loop the object kernel left open. Representation lived in
//! [`crate::object`] while evaluation still ran on a closed `Formula` enum, so
//! the graph could say everything and decide nothing — two representations,
//! with the *weaker* one doing the work.
//!
//! Everything the old evaluator earned carries over unchanged, because those
//! were the right parts:
//!
//! * truth is two-sided, so halting at an arbitrary instant is sound;
//! * absence is refutation only where knowledge is complete;
//! * a cut-short scan yields a **residual**, kept quantified rather than
//!   unrolled;
//! * cost is a budget, never a refusal.
//!
//! What is new is that an operator with no semantics is no longer a
//! contradiction in terms. It evaluates to [`ComputeStatus::Unsupported`] with
//! its expression handed back — distinct from "no evidence" and from "out of
//! budget" — and registering semantics later needs no migration and no reparse.

use crate::certificate::{Certificate, Step};
use crate::evidence::{
    Bound, ComputeStatus, Credence, Derivation, Determinacy, DeterminacyBasis,
    EvaluationResult, Evidential, Grounding, Mode,
};
use crate::object::{CoreNode, LiteralValue, ObjectGraph, ObjectId, wk};
use crate::registry::OperatorRegistry;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use std::collections::BTreeMap;

/// What the evaluator asks of the world. The object-graph analogue of
/// [`crate::structure::Structure`].
/// What a resolver can say about one query.
///
/// The third case is the point. `Fails` and `Unknown` were previously both
/// spelled `None`, with a separate `is_closed(pred)` deciding which one it
/// meant — a relation-level flag standing in for a *query-level* fact.
///
/// That is not sound. A resolver may be authoritative for a relation in general
/// and still not be authoritative for a particular call: it timed out, the
/// index covers only part of the argument space, the caller lacks permission to
/// see some rows, the pattern of bound and free arguments is one it cannot
/// answer, or the snapshot it read is stale. Each of those must stay `Unknown`.
///
/// The failure mode is the worst of any in this file, because it does not stay
/// local: a fabricated `Fails` becomes `Refuted`, and a `Refuted` under a
/// negation becomes a confident `Supported` for the negation — a claim the
/// store never had grounds for, propagating upward with no trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knowledge {
    /// The relation holds for these arguments.
    Holds,
    /// It does not hold, **and the resolver was authoritative for this query**.
    /// Only this licenses `Refuted` from an absence.
    Fails,
    /// Someone recorded that it does **not** hold.
    ///
    /// Distinct from `Fails`, and the distinction is the difference between a
    /// memory that can record a negative result and one that cannot. `Fails`
    /// says *"I looked everywhere and it is not there"* — it needs completeness,
    /// and completeness is exactly what a coding agent's store almost never has.
    /// `Denied` says *"there is a claim on file that this is false"*, which
    /// needs no completeness at all because it is positive evidence.
    ///
    /// Without it, the most common thing a coding agent learns in a day —
    /// *"`-C target-cpu=native` did not fix the segfault"* — had nowhere to
    /// live. The only route to `Refuted` was `Fails`, and declaring the relation
    /// closed to obtain it also supported `(not (fixes flag-lto segfault))`,
    /// a claim nobody had investigated. Omniscience as a side effect of wanting
    /// to write down one negative.
    Denied,
    /// The store holds claims in **both** directions.
    ///
    /// [`crate::evidence`] opens by arguing that a store ingesting claims from
    /// many sessions will hold support for `P` and for `¬P` at once, and that
    /// collapsing that into "no idea" destroys the only signal that says *look
    /// here*. Evaluation could not produce that state: `Knowledge` had three
    /// cases and none of them was "both", so the only route to
    /// `Evidential::Conflicted` in the entire evaluator was the liar's parity
    /// flip. The motivating paragraph described something the code could not do.
    Conflicted,
    /// No answer. Covers absence of data and every kind of incompleteness.
    Unknown,
}

impl Knowledge {
    /// Lift a plain optional answer, treating absence as *not known*.
    ///
    /// The conservative direction on purpose: a resolver that has not been
    /// taught to distinguish "absent" from "absent and I would know" gets the
    /// reading that cannot manufacture a refutation.
    pub fn from_option(v: Option<bool>) -> Knowledge {
        match v {
            Some(true) => Knowledge::Holds,
            Some(false) => Knowledge::Fails,
            None => Knowledge::Unknown,
        }
    }

    /// The evidence this answer licenses.
    pub fn verdict(self) -> Option<EvaluationResult> {
        match self {
            Knowledge::Holds => Some(EvaluationResult::certain(true)),
            Knowledge::Fails | Knowledge::Denied => Some(EvaluationResult::certain(false)),
            Knowledge::Conflicted => Some(EvaluationResult::conflicted()),
            Knowledge::Unknown => None,
        }
    }

    /// Merge two independent answers about the same query. Disagreement is
    /// `Conflicted` rather than a winner, which is the whole point.
    pub fn merge(self, other: Knowledge) -> Knowledge {
        use Knowledge::*;
        match (self, other) {
            (Unknown, x) | (x, Unknown) => x,
            (Conflicted, _) | (_, Conflicted) => Conflicted,
            (Holds, Holds) => Holds,
            (Fails, Fails) | (Denied, Denied) | (Fails, Denied) | (Denied, Fails) => Denied,
            // One source affirms, another denies. Both are on file.
            _ => Conflicted,
        }
    }
}

/// An enumerated domain, and whether the enumeration is exhaustive.
///
/// The same distinction [`Knowledge`] draws for predicates, for domains. A
/// resolver may be able to list *some* members without being able to promise
/// there are no others — a partial index, a paged result, a permission filter.
///
/// This matters asymmetrically. One counterexample refutes a universal
/// regardless of completeness, but *supporting* one requires knowing the list
/// was exhaustive. Without this flag every `Some(vec)` was read as complete, so
/// a partial index made `(forall [(f Files)] (tested f))` come back
/// `Supported/Exact` on a store that had never seen most of the files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extension {
    pub members: Vec<ObjectId>,
    /// True only when these are *all* the members.
    pub complete: bool,
}

impl Extension {
    /// An exhaustive enumeration.
    pub fn complete(members: Vec<ObjectId>) -> Extension {
        Extension { members, complete: true }
    }

    /// Some members, possibly not all.
    pub fn partial(members: Vec<ObjectId>) -> Extension {
        Extension { members, complete: false }
    }
}

pub trait GraphStructure {
    /// What is known about `pred(args)`.
    ///
    /// Returning [`Knowledge::Fails`] is an assertion that this resolver was
    /// complete *for this query* — not merely that it found nothing.
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge;

    /// Members of a domain expression, or `None` when it cannot be enumerated
    /// at all. `Some` carries its own completeness — see [`Extension`].
    fn extension(&self, _domain: ObjectId) -> Option<Extension> {
        None
    }

    /// Members of a domain **as of** `t`.
    ///
    /// Defaults to `None`, not to [`Self::extension`], for the same reason
    /// `value_at` does: present-day membership answered under `at` makes a
    /// universal range over members the sort did not yet have, and
    /// `(at 1500 (forall [(x Path)] (compiles x)))` came back `Refuted` because
    /// of a file created at t = 9000. A completeness claim cannot be honoured
    /// across time by a hook that has no time in it.
    fn extension_at(&self, _domain: ObjectId, _t: i64) -> Option<Extension> {
        None
    }

    /// Graded belief about `proposition`, in log-odds milli-units.
    ///
    /// `None` by default and `None` for anything outside the vocabulary — which
    /// is *not* the same as a wide interval, and the difference is the one thing
    /// the unawareness literature agrees on. A store that grades returns an
    /// interval; a store asked about something it cannot express returns
    /// nothing, and the caller can tell.
    ///
    /// The evaluator never *combines* credences across a growing vocabulary,
    /// which is the operation no Bayesian account gets right: Mahtani's pair of
    /// formally isomorphic awareness-growth episodes demand opposite invariants,
    /// so no constraint on prior credences can fix the posterior. What the
    /// evaluator does instead is *re-derive* against the same origin-indexed
    /// ledger over the enlarged vocabulary. Being derivation-based rather than
    /// credence-propagating is what makes that well-defined.
    /// Who vouches for `proposition` — the agents, sessions or documents on
    /// whose word an answer about it rests.
    ///
    /// This is the provenance channel a certificate's `Told` step needs, and its
    /// absence is why that step named the *relation symbol* as its own
    /// authority: `(prefers adam tabs)` was certified on the authority of
    /// `prefers`, which is not an authority and cannot be doubted, consulted or
    /// retracted. §5.6 says in bold that naming the authority is what makes the
    /// reader's judgement possible; a name that means nothing does not.
    ///
    /// Empty by default, and empty is honest: it says the structure vouches for
    /// the answer itself and offers nobody to go and ask. The kernel records
    /// that under `assumed` rather than inventing an attribution to fill the
    /// field.
    fn attribution(&self, _proposition: ObjectId) -> Vec<ObjectId> {
        Vec::new()
    }

    fn credence(&self, _proposition: ObjectId) -> Option<Credence> {
        None
    }

    /// Does `proposition` have a sharp satisfaction condition?
    ///
    /// Defaults to `Unknown` — *not* `Total`. The evaluator presumes totality
    /// for ordinary queries and records that it presumed, so certification can
    /// refuse what a query accepted. Answering `Total` is a claim the store is
    /// making and must be able to defend.
    ///
    /// Sharpness is not a property of a bare predicate. It is a property of a
    /// predicate **under an interpretation**, often only over a region of the
    /// argument space, which is why this takes the whole proposition and why
    /// the storage convention for answering it lives in the memory layer rather
    /// than here.
    fn determinacy(&self, _proposition: ObjectId) -> Determinacy {
        Determinacy::Unknown
    }

    /// Whether knowledge of `pred` is complete. This is what licenses reading
    /// absence as refutation, and it is data rather than a compiled-in rule.
    fn is_closed(&self, _pred: ObjectId) -> bool {
        false
    }

    /// Whether `pred(args)` held **as of** instant `t`. A memory system needs
    /// *valid* time — when the belief held — not transaction time.
    fn known_at(&self, pred: ObjectId, args: &[ObjectId], _t: i64) -> Knowledge {
        self.known(pred, args)
    }

    /// Instants at which anything changed. Empty for a timeless structure,
    /// which makes temporal queries vacuous rather than wrong.
    fn instants(&self) -> Vec<i64> {
        Vec::new()
    }

    /// What a term *denotes*, as an object in the same universe.
    ///
    /// The formalism had exactly one denotation hook — [`Self::denote_instant`],
    /// returning an `i64` — so no term could be given a value of any other
    /// kind. `(= (duration test-suite) (quantity 4 minutes))` was `Open/Stalled`
    /// and stayed that way however much the store knew, because `duration` is a
    /// *function* and nothing could evaluate one.
    ///
    /// That left only encodings §9 forbids. `(duration-minutes test-suite 4)`
    /// smuggles the unit into the predicate name, which makes minutes and
    /// seconds unrelated words — precisely the failure `quantity` and `scale`
    /// exist to kill. `(duration test-suite 4 minutes)` leaves the magnitude and
    /// the unit as unrelated arguments. "The test suite takes 4 minutes" is not
    /// an exotic thing for a coding agent to remember, and none of the three
    /// available spellings both evaluated and survived §9.
    ///
    /// `denote_instant` is the special case of this for integers, and stays
    /// because temporal indexing needs an `i64` rather than an object.
    fn value(&self, _term: ObjectId) -> Option<ObjectId> {
        None
    }

    /// What a term denoted **as of** `t`.
    ///
    /// Defaults to `None`, **not** to [`Self::value`], and the difference is a
    /// soundness one. Every value a coding agent tracks changes — line counts,
    /// durations, versions, coverage — so a structure that answered a dated
    /// question with today's value would make `(at t1 (= (line-count f) 40))`
    /// come back `Refuted/Exact` for a true claim, and `Supported/Exact` for
    /// today's number under a past instant. Failing closed costs precision;
    /// forwarding costs correctness.
    fn value_at(&self, _term: ObjectId, _t: i64) -> Option<ObjectId> {
        None
    }

    /// What a term denotes in `world`. Same reasoning as [`Self::value_at`].
    fn value_in(&self, _term: ObjectId, _world: ObjectId) -> Option<ObjectId> {
        None
    }

    /// What is known about `pred(args)` at instant `t` **in** `world`.
    ///
    /// The two indices compose — a claim can be about a past state of a
    /// counterfactual world — and the dispatch used to drop the world whenever
    /// an instant was set. Defaults to ignoring the world, which is what every
    /// structure did implicitly before this existed.
    fn known_at_in(
        &self,
        pred: ObjectId,
        args: &[ObjectId],
        t: i64,
        _world: ObjectId,
    ) -> Knowledge {
        self.known_at(pred, args, t)
    }

    /// The instant a symbolic term denotes, if any.
    ///
    /// Without this, `at` accepted only an integer literal — so `(at t3 …)`,
    /// the exemplar in the spec's own §9, did not evaluate, and a memory had to
    /// write raw epoch microseconds into every temporal claim. Naming an
    /// instant is the normal case: "the start of session 7", "the commit that
    /// broke the build".
    fn denote_instant(&self, _term: ObjectId) -> Option<i64> {
        None
    }

    /// Stored rules whose *conclusion* is about `pred`.
    ///
    /// Each is a `(forall [(x D) …] (implies Antecedent Consequent))`. Indexing
    /// by the concluded predicate is what keeps rule selection from scanning
    /// the whole store on every atom.
    ///
    /// Without this the evaluator was a *checker* and never a *deriver*: you
    /// could store "every commit touching the parser needs review" and the fact
    /// that a commit touches the parser, ask whether it needs review, and get
    /// `Open`. Most of what this system remembers is rules, so most of what it
    /// remembered did nothing.
    fn rules(&self, _pred: ObjectId) -> Vec<ObjectId> {
        Vec::new()
    }

    /// Stored rules concluding `pred` that are in force **at** `t`.
    ///
    /// A rule is believed over an interval like any other claim, so selecting
    /// them at the present and then reasoning about the past uses rules the
    /// store records as not yet believed — and refuses rules it records as
    /// believed then. Defaults to the undated set, which is what a structure
    /// with no belief layer should say.
    fn rules_at(&self, pred: ObjectId, _t: i64) -> Vec<ObjectId> {
        self.rules(pred)
    }

    /// One `(scale unit factor base)` fact, if the store holds one for `unit`.
    ///
    /// This is the whole of unit support: the evaluator knows how to normalise
    /// and compare, and the store knows what the units *are*.
    fn scale(&self, _unit: ObjectId) -> Option<(BigInt, ObjectId)> {
        None
    }

    /// Worlds this structure models. Empty means "no modal structure", which
    /// makes a modal claim unanswerable rather than false.
    fn worlds(&self) -> Vec<ObjectId> {
        Vec::new()
    }

    /// Is `to` accessible from `from`?
    ///
    /// This is the accessibility relation, and supplying it is what gives
    /// `necessarily` and `possibly` any content at all. Without one they used
    /// to be looked up in the fact table as binary predicates over node ids —
    /// modal operators with no frame behind them.
    fn accessible(&self, _from: ObjectId, _to: ObjectId) -> bool {
        true
    }

    /// What is known about `pred(args)` **in** `world`.
    fn known_in(&self, pred: ObjectId, args: &[ObjectId], _world: ObjectId) -> Knowledge {
        self.known(pred, args)
    }

    /// The worlds most similar to `from` in which `condition` holds.
    ///
    /// Counterfactuals need a similarity ordering over worlds, and there is no
    /// agreed one — every treatment picks differently, and picking is a
    /// modelling decision rather than an implementation detail. So the
    /// *structure* supplies it. A structure that does not returns `None`, and
    /// `if-counterfactually` is honestly `Unsupported` rather than quietly
    /// wrong.
    fn closest_worlds(&self, _from: ObjectId, _condition: ObjectId) -> Option<Vec<ObjectId>> {
        None
    }

    /// Version of the universe these answers came from. Bounds computed against
    /// different versions must never be composed.
    fn snapshot(&self) -> u64 {
        0
    }
}

/// An empty world: nothing known, nothing closed.
pub struct EmptyStructure;
impl GraphStructure for EmptyStructure {
    fn known(&self, _pred: ObjectId, _args: &[ObjectId]) -> Knowledge {
        Knowledge::Unknown
    }
}

/// An exact rational.
///
/// Arithmetic has to be exact and ordered, and `BigInt` alone is not enough:
/// `Decimal { mantissa, scale }` denotes `mantissa × 10⁻ˢᶜᵃˡᵉ`, which is
/// rational, and integer division truncates. `40d1 = 4` used to come back
/// **Refuted** — the decimal simply was not a number as far as the evaluator
/// was concerned, so `=` fell through to comparing object ids.
///
/// Hand-rolled rather than pulling in `num-rational` for one type. No floats
/// anywhere: a literal has to stay hashable and exactly ordered to key an
/// evidence ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Num {
    num: BigInt,
    den: BigInt,
}

/// Largest decimal exponent that is a number rather than a denial of service.
/// `10^4096` is a 4097-digit integer; anything past it is being asked for by
/// mistake or on purpose.
pub const MAX_DECIMAL_SCALE: u32 = 4096;

impl Num {
    fn new(num: BigInt, den: BigInt) -> Option<Num> {
        if den.to_i64() == Some(0) {
            return None;
        }
        let (num, den) = if den < BigInt::from(0) { (-num, -den) } else { (num, den) };
        let g = gcd(num.clone(), den.clone());
        Some(Num { num: num / &g, den: den / &g })
    }

    fn int(n: BigInt) -> Num {
        Num { num: n, den: BigInt::from(1) }
    }

    /// `mantissa × 10⁻ˢᶜᵃˡᵉ`, exactly.
    ///
    /// The magnitude of the scale is bounded, because `10^scale` is not.
    /// `1d-2147483648` parses, prints and round-trips, and then `-scale`
    /// overflowed `i32` and **panicked** — so a literal could crash the process
    /// before the budget was ever consulted, and "evaluation is total under a
    /// budget" was false. A scale past this is not a number this system needs to
    /// represent; it is a way to ask for a gigabyte of digits.
    fn decimal(mantissa: BigInt, scale: i32) -> Option<Num> {
        if scale.unsigned_abs() > MAX_DECIMAL_SCALE {
            return None;
        }
        let ten = BigInt::from(10);
        if scale >= 0 {
            Num::new(mantissa, ten.pow(scale.unsigned_abs()))
        } else {
            Num::new(mantissa * ten.pow(scale.unsigned_abs()), BigInt::from(1))
        }
    }

    fn add(&self, o: &Num) -> Option<Num> {
        Num::new(&self.num * &o.den + &o.num * &self.den, &self.den * &o.den)
    }
    fn sub(&self, o: &Num) -> Option<Num> {
        Num::new(&self.num * &o.den - &o.num * &self.den, &self.den * &o.den)
    }
    fn mul(&self, o: &Num) -> Option<Num> {
        Num::new(&self.num * &o.num, &self.den * &o.den)
    }
    fn div(&self, o: &Num) -> Option<Num> {
        Num::new(&self.num * &o.den, &self.den * &o.num)
    }
    fn neg(&self) -> Num {
        Num { num: -self.num.clone(), den: self.den.clone() }
    }

    /// Whole-number value, when there is one.
    fn to_int(&self) -> Option<BigInt> {
        (self.den == BigInt::from(1)).then(|| self.num.clone())
    }
}

impl PartialOrd for Num {
    fn partial_cmp(&self, o: &Num) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Num {
    fn cmp(&self, o: &Num) -> std::cmp::Ordering {
        // Denominators are positive by construction, so cross-multiplying
        // preserves the ordering.
        (&self.num * &o.den).cmp(&(&o.num * &self.den))
    }
}

fn gcd(a: BigInt, b: BigInt) -> BigInt {
    let zero = BigInt::from(0);
    let (mut a, mut b) = (if a < zero { -a } else { a }, if b < zero { -b } else { b });
    while b != BigInt::from(0) {
        let t = a % &b;
        a = b;
        b = t;
    }
    if a == BigInt::from(0) { BigInt::from(1) } else { a }
}

/// What an aggregate is known to be: a closed interval, or a lower bound when
/// the scan could not be exhaustive.
///
/// **Two independent reasons an aggregate can be inexact, and they have
/// different consequences.** If the *members* were fully enumerated but the
/// predicate is undecided for some of them, `hi` is still a real upper bound —
/// there are no other members to contribute. If the *enumeration itself* was
/// partial, `hi` totals only what was seen and bounds nothing.
///
/// Collapsing them into one flag costs correctness in one direction or
/// capability in the other: reading `hi` regardless let a partial index decide
/// `(<= (count …) 3)`, and discarding it whenever `!exact` broke
/// `(<= (count …) 6)` over five known members with two undecided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AggregateBounds {
    pub lo: Num,
    /// `None` when unbounded above.
    pub hi: Option<Num>,
    /// The member enumeration was exhaustive, so `hi` really is a bound.
    pub complete: bool,
    /// Complete *and* every member decided: the aggregate is a value.
    pub exact: bool,
}

/// A relation bound to a predicate variable, plus whether it is the *whole*
/// relation.
///
/// The flag keeps recursion sound under a budget: a fixpoint cut short is an
/// under-approximation, so a tuple present really is a member but a tuple
/// absent might still be derived by a later round.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoundRelation {
    pub tuples: std::collections::BTreeSet<Vec<ObjectId>>,
    pub complete: bool,
}

/// Belnap's FOUR, for deciding validity in the logic this system actually has.
///
/// Kept here beside [`Skeleton`] rather than in `evidence.rs` because it is the
/// *semantic* four-valued algebra — `Evidential` is the evaluator's report of it,
/// and conflating the two is how the connective tables drifted from the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Four {
    N,
    T,
    F,
    B,
}

impl Four {
    fn from_digit(d: u64) -> Four {
        match d {
            1 => Four::T,
            2 => Four::F,
            3 => Four::B,
            _ => Four::N,
        }
    }

    fn not(self) -> Four {
        match self {
            Four::T => Four::F,
            Four::F => Four::T,
            v => v,
        }
    }

    /// Meet in the truth order `F <_t N <_t T`, `F <_t B <_t T`. `N` and `B` are
    /// truth-incomparable, so their meet is `F`.
    fn and(self, other: Four) -> Four {
        use Four::*;
        match (self, other) {
            (F, _) | (_, F) => F,
            (T, v) | (v, T) => v,
            (N, B) | (B, N) => F,
            (v, _) => v,
        }
    }

    /// Join in the truth order.
    fn or(self, other: Four) -> Four {
        use Four::*;
        match (self, other) {
            (T, _) | (_, T) => T,
            (F, v) | (v, F) => v,
            (N, B) | (B, N) => T,
            (v, _) => v,
        }
    }

    /// **Designated** — established, in the sense validity quantifies over.
    ///
    /// `T` is told-true and `B` is told-both; both are cases where the store has
    /// asserted the proposition. Validity means *always designated*, not always
    /// `T`, and that distinction is what makes the tautology set non-empty in a
    /// logic that admits `B` at all.
    fn designated(self) -> bool {
        matches!(self, Four::T | Four::B)
    }

    /// Anti-designated: the mirror, and what a `refutation: Certain` bound
    /// asserts (semantics §7.2).
    fn anti(self) -> bool {
        matches!(self, Four::F | Four::B)
    }

    /// **Arieli–Avron's strong implication.** `a ⊃ b = b` when `a` is designated,
    /// and `T` otherwise.
    ///
    /// Material implication — `¬a ∨ b` — leaves `P → P` at `N` equal to `N`, so
    /// FOUR has essentially no valid formulas and the entire "logical truths need
    /// no evidence" capability disappears. That is a real loss, not a tidy-up:
    /// a rule language whose implication has no deduction theorem cannot express
    /// rules.
    ///
    /// The strong reading fixes it without weakening anything that matters.
    /// `P ⊃ P` is designated everywhere. Modus ponens stays sound: if `a ⊃ b` and
    /// `a` are both designated then `a ⊃ b` *is* `b`, so `b` is designated.
    /// Excluded middle stays **invalid** — `P ∨ ¬P` at `N` is still `N` — which is
    /// exactly the property the vague-predicate tests exist to protect, and the
    /// reason this is not simply "going back to classical".
    ///
    /// Material implication remains expressible as `(or (not a) b)`, so nothing
    /// is lost by making the primitive the useful one.
    fn implies(self, other: Four) -> Four {
        if self.designated() { other } else { Four::T }
    }
}

/// A propositional skeleton over opaque atoms, indexed by position.
#[derive(Clone, Debug)]
pub(crate) enum Skeleton {
    Const(bool),
    Atom(usize),
    Not(Box<Skeleton>),
    And(Vec<Skeleton>),
    Or(Vec<Skeleton>),
    /// Kept as itself rather than desugared to `¬A ∨ B`: that rewrite is
    /// *classically* sound and constructively wrong, and it would have hidden
    /// the very distinction this is here to draw.
    Imp(Box<Skeleton>, Box<Skeleton>),
}

impl Skeleton {
    pub(crate) fn eval4(&self, digits: u64) -> Four {
        match self {
            Skeleton::Const(b) => {
                if *b {
                    Four::T
                } else {
                    Four::F
                }
            }
            Skeleton::Atom(i) => Four::from_digit((digits >> (2 * *i as u64)) & 0b11),
            Skeleton::Not(x) => x.eval4(digits).not(),
            Skeleton::And(xs) => {
                xs.iter().map(|x| x.eval4(digits)).fold(Four::T, Four::and)
            }
            Skeleton::Or(xs) => xs.iter().map(|x| x.eval4(digits)).fold(Four::F, Four::or),
            Skeleton::Imp(a, b) => a.eval4(digits).implies(b.eval4(digits)),
        }
    }

    pub(crate) fn eval(&self, mask: u32) -> bool {
        match self {
            Skeleton::Const(b) => *b,
            Skeleton::Atom(i) => mask & (1 << i) != 0,
            Skeleton::Not(x) => !x.eval(mask),
            // Empty connectives keep their identities, as they do everywhere
            // else here.
            Skeleton::And(xs) => xs.iter().all(|x| x.eval(mask)),
            Skeleton::Or(xs) => xs.iter().any(|x| x.eval(mask)),
            Skeleton::Imp(a, b) => !a.eval(mask) || b.eval(mask),
        }
    }
}



/// What a body does across an unbounded region of an integer domain.
///
/// Four values rather than two, because a universal and an existential need
/// different questions answered. "Does it hold everywhere?" refutes a universal
/// when the answer is no; refuting an *existential* needs "does it hold
/// nowhere?", and `no` to the first does not imply `yes` to the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tail {
    /// Holds at every point in the region.
    AllHold,
    /// Holds at no point in it.
    NoneHold,
    /// Holds somewhere and fails somewhere — **both regions non-empty**. This
    /// is the load-bearing case: it settles a universal *and* an existential,
    /// in opposite directions, so it may never be reported on a guess.
    Mixed,
    /// Outside the interpreted fragment.
    Unknown,
}

impl Tail {
    fn negate(self) -> Tail {
        match self {
            Tail::AllHold => Tail::NoneHold,
            Tail::NoneHold => Tail::AllHold,
            other => other,
        }
    }
}

/// Conjunction over tail verdicts.
///
/// `Mixed ∧ AllHold` is exactly the mixed conjunct's own split, so it stays
/// `Mixed`. Two mixed conjuncts are not: their holding regions may be disjoint,
/// making the conjunction `NoneHold`, and reporting `Mixed` would fabricate a
/// witness — which is precisely the error this whole rewrite exists to remove.
fn conjoin_tails(parts: &[Tail]) -> Tail {
    // One conjunct that never holds decides it, whatever the others do.
    if parts.contains(&Tail::NoneHold) {
        return Tail::NoneHold;
    }
    if parts.contains(&Tail::Unknown) {
        return Tail::Unknown;
    }
    match parts.iter().filter(|t| **t == Tail::Mixed).count() {
        0 => Tail::AllHold,
        1 => Tail::Mixed,
        _ => Tail::Unknown,
    }
}

/// Floor division, which `BigInt`'s `/` is not — it truncates toward zero.
fn div_floor(a: &BigInt, b: &BigInt) -> BigInt {
    let (q, r) = (a / b, a % b);
    let zero = BigInt::from(0);
    if r != zero && (r < zero) != (*b < zero) { q - 1 } else { q }
}

fn div_ceil(a: &BigInt, b: &BigInt) -> BigInt {
    let (q, r) = (a / b, a % b);
    let zero = BigInt::from(0);
    if r != zero && (r < zero) == (*b < zero) { q + 1 } else { q }
}

/// Provenance accumulated beneath the node currently being evaluated.
///
/// **This is the structural answer to the defect that recurred in every audit
/// round.** Four times running, the finding was a value computed and then not
/// carried: `lo`, `worst`, `self.negated`, `exact`, `complete`, and finally
/// `absorb` itself — a helper written to fold these very axes and called from
/// nowhere. Fixing the sites does not fix the class, because the class is
/// *"a verdict can be constructed without consulting what it was derived
/// from"*.
///
/// So it cannot be. Every result returned by [`State::check`] folds into this,
/// and every verdict a dispatch arm produces has it applied on the way out —
/// in `dispatch`, above every arm, where no operator can bypass it. An arm that
/// forgets is no longer possible to write; the worst it can do is be
/// *imprecise*.
///
/// It is deliberately conservative. If a conjunction short-circuits on a
/// certainly-false operand after a default was examined, the composite is still
/// marked `Default` — the defeasible step did not contribute, but proving that
/// requires exactly the per-arm reasoning that kept going wrong. Losing
/// precision is sound; losing an axis is not.
#[derive(Clone, Debug, Default)]
struct Axes {
    grounding: Grounding,
    derivation: Derivation,
    determinacy: Determinacy,
    defeated_by: Vec<ObjectId>,
}

impl Axes {
    fn absorb(&mut self, r: &EvaluationResult) {
        self.grounding = self.grounding.merge(r.grounding);
        self.derivation = self.derivation.merge(r.derivation);
        self.determinacy = self.determinacy.merge(r.determinacy);
        for d in &r.defeated_by {
            if !self.defeated_by.contains(d) {
                self.defeated_by.push(*d);
            }
        }
    }

    fn merge(&mut self, other: &Axes) {
        self.grounding = self.grounding.merge(other.grounding);
        self.derivation = self.derivation.merge(other.derivation);
        self.determinacy = self.determinacy.merge(other.determinacy);
        for d in &other.defeated_by {
            if !self.defeated_by.contains(d) {
                self.defeated_by.push(*d);
            }
        }
    }

    /// Stamp a verdict with everything it was derived from.
    fn stamp(&self, r: &mut EvaluationResult) {
        r.grounding = r.grounding.merge(self.grounding);
        r.derivation = r.derivation.merge(self.derivation);
        r.determinacy = r.determinacy.merge(self.determinacy);
        for d in &self.defeated_by {
            if !r.defeated_by.contains(d) {
                r.defeated_by.push(*d);
            }
        }
    }
}

/// Evidence accumulated while scanning a collection of cases.
///
/// Eight places scanned something — the operands of `and`, the members of a
/// domain, the accessible worlds, the instants, the closest worlds, the subsets
/// of a relation — and each rewrote the same loop. Only `and` wrote it
/// correctly. The rest classified a case by
/// `(support.is_certain(), refutation.is_certain())` and, when neither held,
/// recorded nothing but the compute status.
///
/// That was invisible to the middle of the strength lattice. `usually` used to
/// return `Partial / Exact`, so an undecided case looked exactly like a decided one by
/// the time the loop ended and the scan concluded `certain(universal)`. In the
/// existential direction it was worse: the evaluator refuted a proposition
/// while supporting its only instance.
///
/// So the accumulation happens once, here, and it is §5.2's own table applied
/// to scans rather than only to connectives:
///
/// ```text
/// universal:   support = ⊓ᵢ support(Pᵢ)   (needs completeness)
///              refutation = ⊔ᵢ refutation(Pᵢ)
/// existential: support = ⊔ᵢ support(Pᵢ)
///              refutation = ⊓ᵢ refutation(Pᵢ)   (needs completeness)
/// ```
///
/// One side needs the enumeration to have been exhaustive and the other does
/// not — a single counterexample refutes a universal however partial the index
/// was, but supporting one means knowing there were no others.
struct Scan {
    universal: bool,
    /// The meet side: what every case must agree on. Publishable only when the
    /// enumeration was complete and every case was computed exactly.
    gated: Bound,
    /// The join side: what any one case is enough to establish.
    free: Bound,
    worst: ComputeStatus,
    residuals: Vec<ObjectId>,
    grounding: Grounding,
    derivation: Derivation,
    determinacy: Determinacy,
    defeated_by: Vec<ObjectId>,
    /// Graded belief, folded over the cases that carry it.
    ///
    /// `None` until some case is graded, and it stays `None` when none is. Cases
    /// that are *not* graded are skipped rather than treated as vacuous, because
    /// the bounds taken here — a conjunction is no likelier than its least
    /// likely part, a disjunction no less likely than its likeliest — hold over
    /// any subset of the cases. Ignoring an ungraded case weakens the interval
    /// and cannot falsify it.
    credence: Option<Credence>,
}

impl Scan {
    fn new(universal: bool) -> Scan {
        Scan {
            universal,
            gated: Bound::Certain,
            free: Bound::None,
            worst: ComputeStatus::Exact,
            residuals: Vec::new(),
            grounding: Grounding::Grounded,
            derivation: Derivation::Observed,
            determinacy: Determinacy::Total,
            defeated_by: Vec::new(),
            credence: None,
        }
    }

    /// Fold every axis one case carries. Called from both branches of `step`,
    /// because a scan that short-circuits inherits its provenance too.
    fn absorb(&mut self, r: &EvaluationResult) {
        self.grounding = self.grounding.merge(r.grounding);
        self.derivation = self.derivation.merge(r.derivation);
        self.determinacy = self.determinacy.merge(r.determinacy);
        for d in &r.defeated_by {
            if !self.defeated_by.contains(d) {
                self.defeated_by.push(*d);
            }
        }
        if let Some(c) = r.credence {
            // Seeded from `VACUOUS`, **not** from the first graded case. Seeding
            // from the case publishes that case's floor as the conjunction's
            // floor, which is a lie the moment a second conjunct is ungraded:
            // one 95%-likely conjunct says nothing about a conjunction whose
            // other half nobody has graded. The cost is that a one-operand
            // `and` loses a bound it could have kept — a weakening, which is
            // the direction that cannot be wrong.
            let acc = self.credence.unwrap_or(Credence::VACUOUS);
            self.credence =
                Some(if self.universal { acc.conjoin(c) } else { acc.disjoin(c) });
        }
    }

    /// Fold one case in. Returns `true` when it decides the whole scan — a
    /// counterexample for a universal, a witness for an existential — which is
    /// the only situation where stopping early is sound.
    fn step(&mut self, r: &EvaluationResult, snapshot: u64) -> bool {
        // Bounds computed against different versions of the universe describe
        // different worlds, and a conclusion assembled from both was never true
        // at any single moment. `0` is version-free and composes with anything.
        if r.snapshot != 0 && snapshot != 0 && r.snapshot != snapshot {
            self.worst = downgrade(self.worst, ComputeStatus::Stalled);
            return false;
        }
        let (mine, deciding) = if self.universal {
            (r.support, r.refutation)
        } else {
            (r.refutation, r.support)
        };
        if deciding.is_certain() && !mine.is_certain() {
            // Even a decision inherits its provenance. Returning a bare
            // `certain(…)` at the call site dropped grounding, derivation and
            // defeaters — so a `possibly` over a default short-circuited on the
            // default's own now-`Certain` support and reported it as an observed
            // fact, which is the conflation this rework exists to remove.
            self.absorb(r);
            return true;
        }
        // A case with **both** sides certain is a contradiction, not a
        // decision. Short-circuiting on the deciding side would pick whichever
        // half the operator happens to look at and throw the other away — so
        // `(and Conflicted #true)` came back a definite `Refuted`, and its
        // negation a definite `Supported`: a verdict manufactured from a
        // contradiction, and a step *down* the information order that §5.1 says
        // evaluation never takes. Folding it in gives §5.2's answer instead,
        // where `⊓` over the supports and `⊔` over the refutations both reach
        // `Certain` and the conflict survives.
        self.absorb(r);
        self.gated = self.gated.meet(mine);
        self.free = self.free.join(deciding);
        if !mine.is_certain() {
            self.worst = downgrade(self.worst, r.compute_status);
            if let Some(x) = r.residual {
                self.residuals.push(x);
            }
        }
        false
    }

    /// A verdict reached by short-circuit, carrying the axes the deciding case
    /// brought with it.
    fn decided(&self, value: bool, snapshot: u64) -> EvaluationResult {
        let mut r = EvaluationResult::certain(value);
        r.grounding = self.grounding;
        r.derivation = self.derivation;
        r.determinacy = self.determinacy;
        r.defeated_by = self.defeated_by.clone();
        r.credence = self.credence;
        r.snapshot = snapshot;
        r
    }

    /// `(support, refutation, status)` for the scan as a whole.
    fn finish(&self, complete: bool) -> (Bound, Bound, ComputeStatus) {
        let status =
            if complete { self.worst } else { downgrade(self.worst, ComputeStatus::Stalled) };
        // The meet side speaks for cases that were never examined, so it may
        // only be published when there were none.
        let gated =
            if complete && self.worst == ComputeStatus::Exact { self.gated } else { Bound::None };
        if self.universal { (gated, self.free, status) } else { (self.free, gated, status) }
    }
}

/// Bindings during evaluation: first-order values and second-order relations.
#[derive(Clone, Debug, Default)]
pub struct GraphEnv {
    pub vars: BTreeMap<ObjectId, ObjectId>,
    pub rels: BTreeMap<ObjectId, BoundRelation>,
}

impl GraphEnv {
    pub fn new() -> Self {
        Self::default()
    }
    fn get(&self, k: &ObjectId) -> Option<&ObjectId> {
        self.vars.get(k)
    }
    fn with(&self, k: ObjectId, v: ObjectId) -> Self {
        let mut e = self.clone();
        e.vars.insert(k, v);
        e
    }
    fn with_rel(&self, k: ObjectId, r: BoundRelation) -> Self {
        let mut e = self.clone();
        e.rels.insert(k, r);
        e
    }

    /// Is `id` bound as a *relation* rather than an individual?
    ///
    /// A second-order variable is bound into `rels` only, so in an **argument**
    /// position `substitute` found nothing and handed the store the variable's
    /// raw skolem id. A closed resolver then answered `Fails` about an entity it
    /// was never asked about, and `(exists [(R (Rel M))] (and (deprecated R)
    /// (R a)))` came back `Refuted/Exact` on a store whose first-order facts say
    /// the opposite — under `implies`, a confident `Supported`. That is the
    /// fabricated-`Fails` failure §5.2 calls the worst in the system, reached by
    /// a route the `Knowledge` discipline does not cover, because the resolver
    /// was asked a question about a name that denotes nothing to it.
    fn is_relation_var(&self, id: &ObjectId) -> bool {
        self.rels.contains_key(id)
    }
}

pub struct GraphEvaluator {
    pub registry: OperatorRegistry,
}

impl Default for GraphEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphEvaluator {
    pub fn new() -> Self {
        GraphEvaluator { registry: OperatorRegistry::new() }
    }

    /// Evaluate `root`. The graph is mutable because a continuation is an
    /// ordinary object in the same universe — item 17 is only meaningful if the
    /// evaluator can *create* one.
    /// Evaluate for an ordinary query: totality is presumed, and the result
    /// records that it was presumed.
    pub fn eval(
        &self,
        g: &mut ObjectGraph,
        root: ObjectId,
        s: &dyn GraphStructure,
        budget: u64,
    ) -> EvaluationResult {
        self.eval_in(Mode::Query, g, root, s, budget)
    }

    /// Evaluate for **certification**: a classical verdict requires totality to
    /// have been *established*, not presumed.
    ///
    /// The two modes exist because one default cannot serve both. Presuming
    /// totality is the only workable default for answering questions — almost
    /// nothing is ever certified sharp, and requiring certification first would
    /// halt the system on day one. But that same presumption, inside a trusted
    /// kernel, certifies `heap(x) ∨ ¬heap(x)` for a predicate nobody established
    /// as bivalent. Optimistic for queries, strict for proof.
    pub fn certify(
        &self,
        g: &mut ObjectGraph,
        root: ObjectId,
        s: &dyn GraphStructure,
        budget: u64,
    ) -> EvaluationResult {
        self.eval_in(Mode::Certify, g, root, s, budget)
    }

    /// Evaluate, and hand back a **checkable derivation** alongside the answer.
    ///
    /// The certificate is validated by [`Certificate::check`], which knows
    /// nothing about resolvers, budgets, rules or scans — so a reader who
    /// distrusts this evaluator can still establish what the answer rests on,
    /// at a fraction of the cost of recomputing it and without trusting the
    /// thing under suspicion.
    ///
    /// Where a path emits no step the certificate proves *less* than the
    /// evaluator claimed, which is honest. It must never prove more.
    pub fn eval_traced(
        &self,
        g: &mut ObjectGraph,
        root: ObjectId,
        s: &dyn GraphStructure,
        budget: u64,
    ) -> (EvaluationResult, Certificate) {
        let mut st = self.state(Mode::Query, g, s, budget);
        st.trace = Some(Certificate::default());
        let mut r = st.check(root, &GraphEnv::new());
        r.spent = st.spent;
        r.snapshot = st.snapshot;
        let mut cert = st.trace.take().unwrap_or_default();
        // A derivation must conclude with the thing that was asked. Where the
        // last step is about something else — an operator that emits nothing —
        // say so rather than handing back a certificate for a sub-question.
        // A derivation that does not conclude the root establishes nothing about
        // the root, so it is discarded rather than capped with a placeholder.
        // The old `Unestablished` step made the certificate *look* total — it
        // checked, and returned a `Checked` with empty `authorities` and empty
        // `assumed`, which is the shape of a proof resting on nothing at all.
        // Every dated query went out that way, silently dropping the sources its
        // own inner steps had recorded.
        if cert.conclusion() != Some(root) {
            cert.steps.clear();
        }
        (r, cert)
    }

    pub fn eval_in(
        &self,
        mode: Mode,
        g: &mut ObjectGraph,
        root: ObjectId,
        s: &dyn GraphStructure,
        budget: u64,
    ) -> EvaluationResult {
        let mut st = self.state(mode, g, s, budget);
        let mut r = st.check(root, &GraphEnv::new());
        r.spent = st.spent;
        r.snapshot = st.snapshot;
        r
    }

    fn state<'a>(
        &'a self,
        mode: Mode,
        g: &'a mut ObjectGraph,
        s: &'a dyn GraphStructure,
        budget: u64,
    ) -> State<'a> {
        State {
            g,
            s,
            budget,
            spent: 0,
            snapshot: s.snapshot(),
            registry: &self.registry,
            depth: 0,
            path: Vec::new(),
            negated: false,
            instant: None,
            world: None,
            rule_depth: 0,
            mode,
            trace: None,
            axes: Axes::default(),
            value_depth: 0,
            read_depth: 0,
        }
    }
}

struct State<'a> {
    g: &'a mut ObjectGraph,
    s: &'a dyn GraphStructure,
    budget: u64,
    spent: u64,
    snapshot: u64,
    registry: &'a OperatorRegistry,
    depth: u32,
    /// Nodes currently being evaluated, with the parity of `not` above each.
    ///
    /// Re-entering a node already on the path is ungroundedness. Whether it
    /// *oscillates* is exactly whether the parity differs between the two
    /// visits, which is what separates the liar from the truth-teller.
    path: Vec<(ObjectId, bool)>,
    negated: bool,
    /// The instant currently being evaluated at. `None` is the present.
    instant: Option<i64>,
    /// Which world is being evaluated in, if the structure models any.
    world: Option<ObjectId>,
    /// How many rule applications deep. Selection is a search, so it needs a
    /// floor as well as the ordinary budget.
    rule_depth: u32,
    /// Query or certification. See [`GraphEvaluator::certify`].
    mode: Mode,
    /// Steps emitted so far, when the caller asked for a derivation.
    ///
    /// `None` by default: building a certificate costs allocation on every
    /// verdict, and most queries do not want one. The evaluator is unchanged
    /// when it is off, which is the property that keeps the certificate honest —
    /// it records what evaluation did rather than steering it.
    trace: Option<Certificate>,
    /// Provenance of everything evaluated beneath the current node. See
    /// [`Axes`] — this is what makes losing an axis unexpressible.
    axes: Axes,
    /// How many `value` links have been chased. A store may hold a denotation
    /// cycle, and nothing outside this counter would stop it.
    value_depth: u32,
    /// Depth of the term readers — `numeric`, `text`, `sequence`. They walk
    /// operands without consulting the budget, and cyclic terms are legal, so
    /// `#1=(+ 1 #1#)` recursed until the stack gave out. A crash is not one of
    /// the four compute statuses.
    read_depth: u32,
}

/// Longest chain of rule applications. Selection is a search, and this is the
/// dial that bounds it independently of the step budget.
const MAX_RULE_DEPTH: u32 = 8;

/// Guards against a cyclic expression driving the evaluator into the stack.
/// Hitting it is `Stalled`, never a verdict — the liar is representable, and
/// evaluating it simply does not terminate in a truth value.
///
/// (These two paragraphs were merged onto the wrong constant: the stack-guard
/// text sat on `MAX_RULE_DEPTH`, which is not what it describes.)
///
/// **Bounded by the stack, not by taste.** At 512 a hundred nested `not`s
/// aborted the process before this guard was reached — depth was never the
/// binding constraint, frame size was, and `apply`'s match is a large frame.
/// `cargo test` is a debug build, so the guard as tuned protected only the
/// profile it was never exercised in. A crash is not one of the four compute
/// statuses, so this is set low enough that the guard fires first.
const MAX_DEPTH: u32 = 48;

impl State<'_> {
    /// Record a step, when a derivation was asked for. Returns its index so a
    /// later step can cite it.
    fn emit(&mut self, step: Step) -> Option<usize> {
        self.trace.as_mut().map(|c| c.push(step))
    }

    /// The index of the most recent step concluding `node`, if any — how a
    /// composite step finds the premises it rests on.
    fn cite(&self, node: ObjectId) -> Option<usize> {
        let c = self.trace.as_ref()?;
        c.steps.iter().rposition(|st| st.node() == node)
    }

    fn charge(&mut self, n: u64) -> bool {
        self.spent = self.spent.saturating_add(n);
        self.spent < self.budget
    }

    fn exhausted(&self) -> bool {
        self.spent >= self.budget
    }

    fn out_of_budget(&self, node: ObjectId) -> EvaluationResult {
        let mut r = EvaluationResult::new(ComputeStatus::BudgetExhausted);
        r.residual = Some(node);
        r.snapshot = self.snapshot;
        r
    }

    fn unsupported(&self, node: ObjectId) -> EvaluationResult {
        let mut r = EvaluationResult::new(ComputeStatus::Unsupported);
        r.residual = Some(node);
        r.snapshot = self.snapshot;
        r
    }

    fn stalled(&self, node: ObjectId) -> EvaluationResult {
        let mut r = EvaluationResult::new(ComputeStatus::Stalled);
        r.residual = Some(node);
        r.snapshot = self.snapshot;
        r
    }

    /// Fold one sub-result's snapshot into this evaluation's.
    ///
    /// The field existed so that bounds computed against different versions of
    /// the universe would never be composed, and nothing anywhere compared it —
    /// written in nine places, read in zero. That is a comment wearing a safety
    /// mechanism's clothes.
    ///
    /// `0` means *version-free*: a result that does not depend on the store at
    /// all, which is what `EvaluationResult::certain` produces for `#true`,
    /// arithmetic, and every internal short-circuit. Those compose with
    /// anything. Two results that both name a version and disagree do not, and
    /// composing them is a bug in the caller rather than a fact about the
    /// world — so the composite degrades to `Stalled` and says so, instead of
    /// reporting a conclusion that was never true at any single moment.
    ///
    /// Enforcing it now costs nothing, because nothing caches results yet and
    /// the check always passes. That is the argument for doing it now: the
    /// alternative is retrofitting an invariant across every cached path after
    /// a cache exists, under pressure, because something already broke.
    fn compose_snapshot(&self, r: &EvaluationResult) -> bool {
        r.snapshot == 0 || self.snapshot == 0 || r.snapshot == self.snapshot
    }

    /// Rewrap a sub-result's continuation so it still answers **this** node's
    /// question.
    ///
    /// A continuation propagated upward unchanged answers the sub-question it
    /// was suspended in, not the one that was asked. `(not (exists …))`
    /// suspended inside the existential and handed back the *existential's*
    /// continuation — so resuming it returned `Supported` for a query whose
    /// answer was `Refuted`. §5.5's "resuming is just evaluating it" was false
    /// for every suspension below any operator.
    fn wrap_continuation(
        &mut self,
        mut r: EvaluationResult,
        op: ObjectId,
        prefix: &[ObjectId],
    ) -> EvaluationResult {
        if let Some(k) = r.continuation {
            let mut args = prefix.to_vec();
            args.push(k);
            r.continuation = Some(self.g.apply(op, args));
        }
        r
    }

    /// Drop a continuation that cannot be resumed standalone.
    ///
    /// A continuation is a term, and a term evaluated outside the environment
    /// that produced it means something else. Where the bindings cannot be
    /// reconstructed — a lambda's parameters, a `letrec`'s relation — no
    /// continuation is the honest answer, and losing the resume optimisation is
    /// the price.
    fn seal(&self, mut r: EvaluationResult) -> EvaluationResult {
        r.continuation = None;
        r
    }

    fn check(&mut self, node: ObjectId, env: &GraphEnv) -> EvaluationResult {
        if !self.charge(1) {
            return self.out_of_budget(node);
        }
        if self.depth >= MAX_DEPTH {
            return self.stalled(node);
        }
        let resolved = env.get(&node).copied().unwrap_or(node);

        // A sentence that depends on its own truth never becomes grounded, so
        // no revision sequence assigns it a stable value. Report that directly
        // instead of spinning to the depth guard.
        //
        // Deliberately *not* a monotone least fixed point, whatever the older
        // comment here said: the liar's value oscillates between rounds rather
        // than converging, and detecting the oscillation is the whole mechanism
        // that separates it from the truth-teller.
        if let Some((_, was_negated)) = self.path.iter().find(|(n, _)| *n == resolved) {
            // Ungroundedness is its own axis. Reporting the liar as
            // `Conflicted` made one value mean two unrelated things: "the store
            // holds claims in both directions, go read them" and "this sentence
            // has no stable value, stop asking". §5.3 says in bold that the
            // liar's treatment is revision-theoretic and not ordinary two-sided
            // evidence, and then reused the value for ordinary two-sided
            // evidence anyway.
            let mut r = EvaluationResult::new(ComputeStatus::Exact);
            r.grounding = if *was_negated != self.negated {
                // Parity flipped around the loop: the value would oscillate
                // between revision rounds. The liar.
                Grounding::Oscillatory
            } else {
                // Stable but ungrounded: the truth-teller.
                Grounding::StableLoop
            };
            r.residual = Some(resolved);
            r.snapshot = self.snapshot;
            // Emitted only when the kernel can *verify* ungroundedness, which it
            // can do exactly for loops through the single-operand reference
            // operators. Anywhere else — a cycle running through a branching
            // connective — the kernel refuses, because a reachable cycle is not
            // ungroundedness: `P ↔ P ∨ ⊤` loops and grounds to true in one step
            // of the fixpoint. The evaluator may still *report* the axis; what it
            // may not do is claim a proof of it.
            if crate::certificate::loop_parity(self.g, resolved).is_some() {
                self.emit(Step::Ungrounded { node: resolved });
            }
            // Through the same channel as every other result: an early return
            // is exactly where an axis gets lost.
            self.axes.absorb(&r);
            return r;
        }
        self.path.push((resolved, self.negated));
        let out = self.dispatch(resolved, env);
        self.path.pop();
        self.axes.absorb(&out);
        out
    }

    /// `(implies A C)` under **Arieli–Avron's strong implication**, which is what
    /// semantics §3 commits to: `A ⊃ C = C` when `A` is designated, `T`
    /// otherwise.
    ///
    /// This used to run through the ordinary connective scan with operand 0
    /// negated — i.e. materially, as `¬A ∨ C`. The two differ, and one direction
    /// is **unsound**: with `A` conflicted, `¬B ∨ F = B ∨ F = B` is designated
    /// while `B ⊃ F = F` is not, so a refuted implication was reported as
    /// supported. A store that holds both directions of a claim is ordinary here,
    /// so this was reachable, and the soundness property test could not see it
    /// because `MapGraphStructure` cannot express `Conflicted`.
    ///
    /// What the two bounds can establish, and nothing more:
    ///
    /// * **Support** — from the consequent alone. If `C` is designated then
    ///   `A ⊃ C` is designated either way: it is `C` when `A` is designated, and
    ///   `T` when it is not.
    /// * **Refutation** — needs `A` designated *and* `C` anti-designated, which
    ///   is exactly `support`/`refutation` being `Certain` on the two operands.
    ///
    /// The material shortcut "a refuted antecedent supports the implication" is
    /// **not** available: `refutation: Certain` asserts `⟦A⟧ ∈ {F,B}`, and `B` is
    /// designated. Losing it costs completeness on `(implies #false X)`, which
    /// now comes back `Open` — sound, and weaker than before.
    fn implication(
        &mut self,
        node: ObjectId,
        ante: ObjectId,
        conseq: ObjectId,
        env: &GraphEnv,
    ) -> EvaluationResult {
        if self.exhausted() {
            return self.out_of_budget(node);
        }
        // **Operands in source order**, which is not a stylistic choice. Budget
        // is consumed as evaluation proceeds, so whichever operand runs first
        // takes a share that grows with the *total* budget and starves the
        // other. Evaluating the consequent first made `(implies A (forall …))`
        // refute at budget 32 and go open at 128 — a property test caught the
        // retraction. Left to right keeps each operand's share monotone.
        let a = self.check(ante, env);
        let a = self.seal(a);
        let c = self.check(conseq, env);
        let c = self.seal(c);

        let mut scan = Scan::new(false);
        scan.absorb(&a);
        scan.absorb(&c);

        // A designated consequent decides on its own: `A ⊃ C` is `C` when `A` is
        // designated and `T` when it is not, so either way it is designated.
        // Refutation needs `A` designated *and* `C` anti-designated.
        //
        // The material shortcut — a refuted antecedent supporting the
        // implication — is **not** available, because `refutation: Certain`
        // asserts `⟦A⟧ ∈ {F,B}` and `B` is designated. That costs
        // `(implies #false X)`, which is now `Open`.
        let decided = if c.support.is_certain() {
            Some(true)
        } else if a.support.is_certain() && c.refutation.is_certain() {
            Some(false)
        } else {
            None
        };
        match decided {
            Some(holds) => {
                let (cited, slot) = if holds { (conseq, 1) } else { (ante, 0) };
                if let Some(k) = self.cite(cited) {
                    self.emit(Step::Connective { node, premises: vec![(k, slot)], holds });
                }
                scan.decided(holds, self.snapshot)
            }
            // Undecided: neither bound is established, and both operands were
            // visited, so what is missing is information rather than traversal.
            //
            // Built explicitly rather than through `finish_scan`, because that
            // reads bounds a `Scan` accumulated through `step` — and this scan
            // was only fed axes. An empty join-scan finishes at `or`'s identity,
            // *refuted*, which the enclosing `not` then turned into `Supported`
            // for a truth-teller.
            None => {
                let worst = downgrade(
                    downgrade(ComputeStatus::Exact, a.compute_status),
                    c.compute_status,
                );
                let mut out = EvaluationResult::new(worst);
                out.grounding = scan.grounding;
                out.derivation = scan.derivation;
                out.determinacy = scan.determinacy;
                out.defeated_by = scan.defeated_by.clone();
                out.credence = scan.credence;
                out.snapshot = self.snapshot;
                out
            }
        }
    }

    /// Is this true — or false — **whatever the store says**?
    ///
    /// The evaluator could not recognise a single logical truth. `P → P` came
    /// back `Open/Stalled`: it went looking for facts about `P`, found none, and
    /// reported an absence. That is honest and useless. Validity is the case
    /// where the answer does not depend on the model at all, and a system that
    /// has to ask the world whether `P → P` holds is not doing deduction.
    ///
    /// Only the **propositional skeleton** is decided here. Full higher-order
    /// validity is undecidable; the boolean structure over opaque atoms is
    /// decidable, cheap at these sizes, and catches every classical tautology
    /// anyone writes down. Quantified validity keeps going to the budget.
    ///
    /// **This is gated on determinacy, and that gating is the whole reason it
    /// waited.** `P ∨ ¬P` is a tautology *given bivalence*, and bivalence is
    /// exactly what a vague predicate lacks. Recognising it optimistically would
    /// have certified excluded middle for `heap`, turning a deduction feature
    /// into a bivalence launderer. So: in `Query` mode the skeleton is decided
    /// and the result is marked `Unknown` determinacy — presumed, not
    /// established. In `Certify` mode a classical tautology is only returned
    /// when every atom under it is established `Total`.
    fn valid(&mut self, node: ObjectId, env: &GraphEnv) -> Option<EvaluationResult> {
        let mut atoms: Vec<ObjectId> = Vec::new();
        let outer = self.read_depth;
        self.read_depth = 0;
        let shape = self.skeleton(node, env, &mut atoms, 0);
        self.read_depth = outer;
        let shape = shape?;
        // Two atoms of boolean structure is where tautologies live; beyond a
        // handful the truth table is not worth walking, and the ordinary
        // evaluator will do better with real facts anyway.
        if atoms.is_empty() || atoms.len() > 8 {
            return None;
        }
        // **Validity in FOUR, not in the Booleans.** This enumerated `2^n`
        // Boolean masks, which decides validity in a *different logic* from the
        // one §3 commits to. A soundness property test caught it in both
        // polarities: `P ∨ ¬P` at `N` is `N ∨ N = N` and `P ∧ ¬P` at `N` is `N`,
        // so the Boolean reading certified a tautology as `Supported` and a
        // contradiction as `Refuted` about a sentence the model leaves `Open`.
        // That is the manufacture-a-verdict-from-absence defect arriving through
        // the one path that never consults the store.
        //
        // The honest consequence is that this shortcut now fires rarely —
        // essentially only for structure over `⊤`/`⊥` — because a logic whose
        // store can hold both and neither has almost no valid formulas. What used
        // to be "recognised" now goes to ordinary evaluation against real facts,
        // which is where an answer that depends on the facts belongs.
        if atoms.len() > 8 {
            return None;
        }
        // Validity is **always designated**, and refutation is always
        // anti-designated — which is exactly what §7.2 says a `Certain` bound
        // asserts (`support ⟹ ⟦n⟧ ∈ {T,B}`, `refutation ⟹ ⟦n⟧ ∈ {F,B}`). Reading
        // validity as "always `T`" instead is what emptied the tautology set.
        // **Two tiers, and the difference between them is what is presumed.**
        //
        // FOUR-validity is unconditional: true in every structure this semantics
        // admits, including ones that hold *both* or *neither*. Classical
        // validity is true in every **bivalent** structure, which is a strictly
        // stronger assumption and one the store may not satisfy.
        //
        // Collapsing the two was the bug a property test caught: reading only the
        // Boolean table certified `P ∨ ¬P` as `Supported` and `P ∧ ¬P` as
        // `Refuted` with nothing recording that bivalence had been assumed.
        // Deleting the classical tier outright was the over-correction — it is
        // sound *relative to a recorded presumption*, which is precisely the
        // conditional judgment §7.1 is built around, and the determinacy gate
        // below is what records it.
        let mut all_true = true;
        let mut all_false = true;
        for digits in 0u64..4u64.pow(atoms.len() as u32) {
            let v = shape.eval4(digits);
            all_true &= v.designated();
            all_false &= v.anti();
            if !all_true && !all_false {
                break;
            }
        }
        // Unconditional: no presumption to record, so no gate to pass, and the
        // totality is *derived* rather than declared or presumed.
        if all_true || all_false {
            let mut r = EvaluationResult::certain(all_true);
            r.determinacy = Determinacy::Total;
            r.determinacy_basis = DeterminacyBasis::Derived;
            r.snapshot = self.snapshot;
            return Some(r);
        }
        // Otherwise fall back to the classical table, whose answers are
        // conditional on bivalence and gated accordingly.
        let mut all_true = true;
        let mut all_false = true;
        for mask in 0u32..(1u32 << atoms.len()) {
            if shape.eval(mask) {
                all_false = false;
            } else {
                all_true = false;
            }
            if !all_true && !all_false {
                return None;
            }
        }
        // **There is no constructive tier, and there was no sound way to have
        // one.** It claimed G4ip validity presumes only that no atom is `both`.
        // That presumption does not validate it: `¬(P ∧ ¬P)` is G4ip-provable and
        // at `N` evaluates to `¬(N ∧ N) = ¬N = N`, which is undesignated, and `N`
        // is not `both`. Excluding gaps *and* gluts is bivalence — the classical
        // tier's own presumption — under which G4ip proves a strict subset of
        // what the truth table proves. So the tier was unsound as stated and
        // redundant once corrected.
        //
        // What it was protecting is protected better: `P → P` needs no
        // presumption at all, and the FOUR tier above certifies it
        // unconditionally, because `⊃` is the strong implication.
        // **Not `merge`.** Merging asks "what do these claims come to", and
        // `Unknown` is superseded by anything said. This asks "is every atom
        // *established* total", where silence is the whole objection — so an
        // undeclared atom must block, and folding with `merge` would let it
        // through.
        let declared = atoms.iter().map(|a| self.s.determinacy(*a));
        let worst = if declared.clone().all(|d| d == Determinacy::Total) {
            Determinacy::Total
        } else {
            declared.fold(Determinacy::Unknown, |a, b| a.max(b))
        };
        // Everything reaching here presumes bivalence, so in `Certify` mode it
        // is gated on totality being established — no exceptions, since there is
        // no longer a class of formula that earns its way past.
        if self.mode == Mode::Certify && !worst.licenses_classical() {
            return None;
        }
        let mut r = EvaluationResult::certain(all_true);
        // **Constructive is not unconditional here.** This branch used to set
        // `Total`/`Derived` for anything G4ip proves, on the reasoning that a
        // constructive validity is true whatever the atoms mean. That holds in
        // an intuitionistic setting and fails in this one: `¬(P ∧ ¬P)` is
        // intuitionistically provable, and in FOUR at `N` it is `N ∧ N = N`,
        // whose negation is `N` — not designated. A soundness property test
        // found exactly that formula.
        //
        // So only the FOUR tier above is unconditional. Everything reaching here
        // presumes something — bivalence for the classical table, and at minimum
        // that no atom is `both` for the constructive one — and says so.
        // `constructive` survives solely to pick which presumption the `Certify`
        // gate below demands, never to claim there is none.
        r.determinacy = worst;
        {
            // **Bivalence was presumed, and the presumption is now on the
            // record.** This tier reads the *classical* table, so its answers
            // hold only in structures where every atom is `T` or `F` — which a
            // Belnap store need not be. Leaving the basis at its default made a
            // conditional judgment indistinguishable from an unconditional one,
            // and a soundness property test read it as the latter and found the
            // contradiction. Marking it `Presumed` is what lets a reader — or a
            // test — apply the condition the answer actually carries.
            r.determinacy_basis = DeterminacyBasis::Presumed;
        }
        r.snapshot = self.snapshot;
        Some(r)
    }

    /// The boolean skeleton over opaque atoms, or `None` outside the fragment.
    fn skeleton(
        &mut self,
        node: ObjectId,
        env: &GraphEnv,
        atoms: &mut Vec<ObjectId>,
        depth: u32,
    ) -> Option<Skeleton> {
        // **Bound the *expansion*, not the depth.** The graph is a DAG and this
        // returns an unshared tree, so `xᵢ₊₁ = (and xᵢ xᵢ)` — twenty-five nodes
        // — expanded to 2²⁴ and took half a minute inside a query whose budget
        // was ten thousand steps. A depth cap does not bound a doubling.
        if depth > 32 || self.read_depth > 4096 {
            return None;
        }
        self.read_depth += 1;
        let r = self.substitute(node, env, 0);
        if r == wk::TOP {
            return Some(Skeleton::Const(true));
        }
        if r == wk::BOT {
            return Some(Skeleton::Const(false));
        }
        if let Some(CoreNode::Apply { operator, operands }) = self.g.get(r).cloned() {
            let parts = |me: &mut Self, atoms: &mut Vec<ObjectId>| -> Option<Vec<Skeleton>> {
                operands.iter().map(|o| me.skeleton(*o, env, atoms, depth + 1)).collect()
            };
            match operator {
                wk::NOT if operands.len() == 1 => {
                    return Some(Skeleton::Not(Box::new(parts(self, atoms)?.remove(0))));
                }
                wk::AND => return Some(Skeleton::And(parts(self, atoms)?)),
                wk::OR => return Some(Skeleton::Or(parts(self, atoms)?)),
                wk::IMPLIES if operands.len() == 2 => {
                    let mut p = parts(self, atoms)?;
                    let conseq = p.remove(1);
                    let ante = p.remove(0);
                    return Some(Skeleton::Imp(Box::new(ante), Box::new(conseq)));
                }
                _ => {}
            }
        }
        // Anything else is an opaque atom. Two occurrences of the *same* node
        // are the same atom — which is what makes `P → P` a tautology and
        // `P → Q` not.
        let idx = match atoms.iter().position(|a| *a == r) {
            Some(i) => i,
            None => {
                atoms.push(r);
                atoms.len() - 1
            }
        };
        Some(Skeleton::Atom(idx))
    }

    fn dispatch(&mut self, resolved: ObjectId, env: &GraphEnv) -> EvaluationResult {

        match self.g.get(resolved) {
            None => self.unsupported(resolved),
            Some(CoreNode::Atom { .. }) if resolved == wk::TOP => {
                self.emit(Step::Axiom { node: resolved, holds: true });
                EvaluationResult::certain(true)
            }
            Some(CoreNode::Atom { .. }) if resolved == wk::BOT => {
                self.emit(Step::Axiom { node: resolved, holds: false });
                EvaluationResult::certain(false)
            }
            // A bare atom or literal is not a proposition; it denotes.
            Some(CoreNode::Atom { .. })
            | Some(CoreNode::Literal(_))
            | Some(CoreNode::External(_))
            | Some(CoreNode::Opaque { .. }) => self.unsupported(resolved),
            Some(CoreNode::Apply { operator, operands }) => {
                let (op, args) = (*operator, operands.clone());
                // Truth that does not depend on the store is decided before the
                // store is consulted.
                if matches!(op, wk::NOT | wk::AND | wk::OR | wk::IMPLIES)
                    && let Some(r) = self.valid(resolved, env)
                {
                    // **No step is emitted.** `valid` decides on the *classical*
                    // skeleton, and this logic is Belnap's: `P ∨ ¬P` at `N` is
                    // `N`, and even `P → P` at `N` is `N` (semantics §3), so a
                    // classical validity is not a validity here. The evaluator
                    // may still answer — its own semantics for `valid` is a
                    // separate question — but there is no kernel rule that
                    // licenses it, and emitting one would be certifying a
                    // formula the model does not validate.
                    return r;
                }
                let outer = std::mem::take(&mut self.axes);
                self.depth += 1;
                let mut r = self.apply(resolved, op, &args, env);
                self.depth -= 1;
                // Everything evaluated beneath this node — including through a
                // *reader* like `numeric` or `aggregate_bounds`, which never
                // hands a result back to its caller — reaches the verdict here.
                self.axes.stamp(&mut r);
                let mine = std::mem::replace(&mut self.axes, outer);
                self.axes.merge(&mine);
                r
            }
            Some(CoreNode::Bind { .. }) => {
                // Stored binders are de Bruijn; open them into the nominal
                // form the rest of the evaluator is written against. Fresh
                // variables per opening make capture impossible by
                // construction rather than by discipline.
                let Some((b, vs, bs)) = self.g.open_binder(resolved) else {
                    return self.unsupported(resolved);
                };
                let outer = std::mem::take(&mut self.axes);
                self.depth += 1;
                let mut r = self.bind(resolved, b, &vs, &bs, env);
                self.depth -= 1;
                self.axes.stamp(&mut r);
                let mine = std::mem::replace(&mut self.axes, outer);
                self.axes.merge(&mine);
                r
            }
        }
    }

    fn apply(
        &mut self,
        node: ObjectId,
        op: ObjectId,
        args: &[ObjectId],
        env: &GraphEnv,
    ) -> EvaluationResult {
        match op {
            wk::NOT => {
                // Arity-guarded like `at` and `=` already are. `args.first()`
                // alone silently discarded the rest, so `(not P Q)` evaluated as
                // `(not P)` — a *different* proposition, answered confidently.
                let [a] = args else { return self.unsupported(node) };
                self.negated = !self.negated;
                let r = self.check(*a, env);
                self.negated = !self.negated;
                let mut r = r.negate();
                // A witness for `P` is a counterexample to `¬P` — the same
                // member, playing the opposite role. Rather than track the
                // flip, drop it: a binding is a justification, and the honest
                // default for one is silence.
                r.bindings.clear();
                self.wrap_continuation(r, wk::NOT, &[])
            }
            wk::AND | wk::OR | wk::IMPLIES => {
                if op == wk::IMPLIES {
                    let [ante, conseq] = args else { return self.unsupported(node) };
                    return self.implication(node, *ante, *conseq, env);
                }
                // An operand list is exhaustive by construction, so a
                // connective is a scan whose enumeration is always complete.
                let mut scan = Scan::new(op == wk::AND);
                for (i, a) in args.iter().enumerate() {
                    if self.exhausted() {
                        return self.out_of_budget(node);
                    }
                    // `implies` negates its first operand — and it must do so
                    // *before* checking it, not after. Negating the result left
                    // `self.negated` at the outer parity, so the cycle detector
                    // saw a spurious flip around a stable loop and returned the
                    // oscillation verdict. `(not (implies B #false))` came back
                    // `Refuted/Exact` while the classically identical
                    // `(not (not B))` was correctly `Open/Exact`: one sentence,
                    // two answers, decided by spelling.
                    let flip = op == wk::IMPLIES && i == 0;
                    if flip {
                        self.negated = !self.negated;
                    }
                    let mut r = self.check(*a, env);
                    if flip {
                        self.negated = !self.negated;
                        r = r.negate();
                    }
                    // A sub-continuation cannot be resumed as this connective's
                    // continuation; the scan hands back a residual instead.
                    r = self.seal(r);
                    if scan.step(&r, self.snapshot) {
                        let holds = op != wk::AND;
                        if let Some(k) = self.cite(*a) {
                            // The operand *slot*, not just the node: under
                            // content addressing `(implies P P)` repeats an id,
                            // and a premise identified by node alone can be
                            // read into the wrong position.
                            self.emit(Step::Connective { node, premises: vec![(k, i)], holds });
                        }
                        return scan.decided(holds, self.snapshot);
                    }
                }
                self.finish_scan(node, &scan, true)
            }
            wk::EQ => {
                let [a, b] = args else { return self.unsupported(node) };
                // Reflexivity needs no denotation: one id is one thing however
                // uncomputable it is. `(= #1=(build artist) #1#)` — literally
                // the same node twice — came back `Open` because `denote`
                // refuses every compound before ever comparing.
                if self.key(*a, env) == self.key(*b, env) {
                    return EvaluationResult::certain(true);
                }
                // Numeric first: an aggregate denotes a *number*, so comparing
                // its object id would compare the expression rather than its
                // value and report a confident false.
                if let (Some(x), Some(y)) = (self.numeric(*a, env), self.numeric(*b, env)) {
                    return EvaluationResult::certain(x == y);
                }
                // Quantities compare after normalising to a common base, so
                // `(quantity 4 minutes)` and `(quantity 240 seconds)` are equal
                // given only `(scale minutes 60 seconds)`.
                if let (Some((x, ux)), Some((y, uy))) =
                    (self.quantity(*a, env), self.quantity(*b, env))
                {
                    return if ux == uy {
                        EvaluationResult::certain(x == y)
                    } else {
                        // Different bases: no conversion is known, so nothing
                        // is decided rather than declaring them unequal.
                        self.stalled(node)
                    };
                }
                if let (Some(x), Some(y)) = (self.text(*a, env), self.text(*b, env)) {
                    return EvaluationResult::certain(x == y);
                }
                match (self.denote(*a, env), self.denote(*b, env)) {
                    (Some(x), Some(y)) => EvaluationResult::certain(x == y),
                    _ => self.stalled(node),
                }
            }
            wk::LEQ => {
                let [a, b] = args else { return self.unsupported(node) };
                if let (Some(x), Some(y)) = (self.numeric(*a, env), self.numeric(*b, env)) {
                    return EvaluationResult::certain(x <= y);
                }
                if let (Some((x, ux)), Some((y, uy))) =
                    (self.quantity(*a, env), self.quantity(*b, env))
                {
                    return if ux == uy {
                        EvaluationResult::certain(x <= y)
                    } else {
                        self.stalled(node)
                    };
                }
                // A partial aggregate is still a bound, and a bound can decide
                // a comparison long before the scan finishes. `(<= 3 (count …))`
                // is settled the moment three members are established, however
                // many remain unexamined.
                //
                // §5.2 has always claimed this. It was not true: `number()`
                // returned `None` for anything inexact, so every comparison
                // over a partial aggregate stalled, and "at least three of the
                // five tests fail" was unanswerable unless the resolver was
                // authoritative about every test.
                let (lo_a, hi_a) = self.interval(*a, env);
                let (lo_b, hi_b) = self.interval(*b, env);
                // a ≤ b holds when a's maximum is under b's minimum.
                if let (Some(ha), Some(lb)) = (&hi_a, &lo_b)
                    && ha <= lb
                {
                    return EvaluationResult::certain(true);
                }
                // …and fails when a's minimum already exceeds b's maximum.
                if let (Some(la), Some(hb)) = (&lo_a, &hi_b)
                    && la > hb
                {
                    return EvaluationResult::certain(false);
                }
                self.stalled(node)
            }
            wk::CONTAINS | wk::STARTS_WITH | wk::ENDS_WITH => {
                let [a, b] = args else { return self.unsupported(node) };
                match (self.text(*a, env), self.text(*b, env)) {
                    (Some(x), Some(y)) => EvaluationResult::certain(match op {
                        wk::CONTAINS => x.contains(&y),
                        wk::STARTS_WITH => x.starts_with(&y),
                        _ => x.ends_with(&y),
                    }),
                    _ => self.stalled(node),
                }
            }
            wk::SAME_AS => {
                // Coreference, *not* value equality — the two are different
                // relations and §7 turns on keeping them apart. `=` is decided
                // by computation and nothing revises it; `same-as` is an
                // asserted claim that two names denote one thing, and it can be
                // wrong, retracted or contradicted.
                //
                // Reflexive and symmetric here; transitivity and the actual
                // rewriting live in the structure, which is also where a
                // retraction can undo them. Substitution deliberately does not
                // happen inside `quote`, `believes` or `asserted-by`: those
                // mention their arguments rather than using them, and rewriting
                // through one is how "Lois believes Superman flies" becomes
                // "Lois believes Clark Kent flies".
                let [a, b] = args else { return self.unsupported(node) };
                let (x, y) = (self.key(*a, env), self.key(*b, env));
                if x == y {
                    return EvaluationResult::certain(true);
                }
                // Symmetric, so the two directions are two readings of one
                // question and `merge` is the right combiner. §7's fourth rule —
                // "conflicted identity yields conflicted conclusions" — is this
                // line: the old `(Holds, _) | (_, Holds)` arm let an affirmation
                // win before the disagreement was ever visible.
                // Symmetry, not corroboration. `merge` treats the two
                // directions as independent sources, so a closed resolver that
                // held `(same-as a b)` and had simply never written the mirror
                // row answered `Holds` one way and `Fails` the other, and the
                // merge manufactured a `Conflicted` identity out of nothing —
                // which §7 then propagates into every conclusion drawn through
                // it. An authoritative *absence* on the mirror is not evidence
                // against an identity the store affirms.
                let (fwd, rev) = (self.lookup(op, &[x, y]), self.lookup(op, &[y, x]));
                // A contested answer in either direction contests the identity.
                // The table below reads only `Holds` and `Denied`, so a
                // `Conflicted` row was either flattened by the other direction
                // or discarded entirely — §5.2's "nothing downstream may flatten
                // it" and §7's fourth rule, broken in opposite directions by the
                // same omission.
                if fwd == Knowledge::Conflicted || rev == Knowledge::Conflicted {
                    return EvaluationResult::conflicted();
                }
                let affirms = fwd == Knowledge::Holds || rev == Knowledge::Holds;
                let denies = fwd == Knowledge::Denied || rev == Knowledge::Denied;
                let both = match (affirms, denies) {
                    (true, true) => Knowledge::Conflicted,
                    (true, false) => Knowledge::Holds,
                    (false, true) => Knowledge::Denied,
                    (false, false) => {
                        if fwd == Knowledge::Fails && rev == Knowledge::Fails {
                            Knowledge::Fails
                        } else {
                            Knowledge::Unknown
                        }
                    }
                };
                let out = match both.verdict() {
                    Some(r) => r,
                    None => self.derive(node, op, &[x, y], env),
                };
                self.qualified(node, out)
            }
            wk::USUALLY => {
                // A default: holds absent counter-evidence.
                //
                // A default is *certain that the default applies*; what makes
                // it defeasible is the defeater, not a weaker bound. That
                // distinction lives on `Derivation`, so nothing downstream can
                // promote a default into a *settled reading* — which is the
                // property that matters, and is not the same as keeping it out
                // of `must`.
                let [inner] = args else { return self.unsupported(node) };
                let r = self.check(*inner, env);
                // Both sides are tested together. Testing refutation first made
                // `(usually Conflicted)` a definite `Refuted` — the operator
                // picking whichever half it happened to look at, which is the
                // pathology `Scan::step`'s own comment forbids.
                if r.support.is_certain() && r.refutation.is_certain() {
                    return EvaluationResult::conflicted();
                }
                if r.refutation.is_certain() {
                    // Explicit counter-evidence defeats the default outright.
                    return EvaluationResult::certain(false);
                }
                if r.support.is_certain() {
                    return EvaluationResult::certain(true);
                }
                // A default needs a *stored* default. Without that check,
                // `(usually P)` returned `Supported/Exact` on an **empty**
                // structure — so hedging was a free upgrade from `Open`, and the
                // evaluator claimed to have finished thinking about a
                // proposition it had never heard of. Wrapping a claim cannot
                // manufacture evidence for it.
                let target = self.key(*inner, env);
                let held = self.lookup(wk::USUALLY, &[target]) == Knowledge::Holds;
                // A default for the opposite claim is not a tie to be broken by
                // whichever was written first — it is exactly the state the
                // four-valued design exists to surface. Reporting weak support
                // and moving on is the one reading a memory must not give, since
                // `Conflicted` is the signal that says *look here*.
                let negated = self.g.apply(wk::NOT, vec![target]);
                let against = self.lookup(wk::USUALLY, &[negated]) == Knowledge::Holds;
                if held || against {
                    // A default on record is *certain* that the default
                    // applies — what makes it defeasible is the defeater, not a
                    // weaker bound. Encoding it as `Partial` conflated "holds by
                    // default" with "some evidence, not conclusive", which are
                    // different claims, and left graded strength with no
                    // representation at all.
                    let mut out = EvaluationResult::new(ComputeStatus::Exact);
                    if held {
                        out.support = Bound::Certain;
                    }
                    if against {
                        out.refutation = Bound::Certain;
                    }
                    out.snapshot = self.snapshot;
                    return out.defeasible(*inner);
                }
                // No default on record: `usually` adds nothing, so it reports
                // what its argument reported.
                self.wrap_continuation(r, wk::USUALLY, &[])
            }
            wk::UNLESS => {
                // `(unless Exception P)` — P, defeated when the exception
                // holds. The asymmetry matters: an exception that is merely
                // *possible* does not defeat, or no default would ever survive.
                let [exception, inner] = args else { return self.unsupported(node) };
                let e = self.check(*exception, env);
                if e.support.is_certain() {
                    return self.stalled(node);
                }
                let r = self.check(*inner, env);
                // `unless` exists to name a defeater, and named none: learning
                // `E` flips a *definite* Supported to Open, which is the file's
                // own definition of a default. `usually` marks and names; this
                // did neither.
                let r = r.defeasible(*exception);
                self.wrap_continuation(r, wk::UNLESS, &[*exception])
            }
            wk::PREFER => {
                // A strict order over options: irreflexive, and asymmetric
                // where the store is authoritative. Transitivity is left to
                // stored rules rather than baked in, so a cycle in the store is
                // visible as a conflict instead of being silently closed over.
                let [a, b] = args else { return self.unsupported(node) };
                let (x, y) = (self.key(*a, env), self.key(*b, env));
                if x == y {
                    return EvaluationResult::certain(false);
                }
                // The gate was per-arm, so four reserved relations reached a
                // verdict without ever meeting it — and certified
                // negation-as-failure over a proposition the store had
                // *declared* to have no sharp condition.
                let out = match self.lookup(op, &[x, y]) {
                    Knowledge::Unknown => match self.lookup(op, &[y, x]) {
                        // The reverse preference refutes this one.
                        Knowledge::Holds => EvaluationResult::certain(false),
                        _ => self.derive(node, op, &[x, y], env),
                    },
                    other => other.verdict().unwrap_or_else(|| self.stalled(node)),
                };
                self.qualified(node, out)
            }
            wk::NECESSARILY | wk::POSSIBLY => {
                // Quantify over accessible worlds. Without a frame these were
                // ordinary binary predicates over node ids — "necessarily P"
                // was true iff a fact table happened to hold a row for it.
                let [inner] = args else { return self.unsupported(node) };
                let here = self.world;
                let reachable: Vec<ObjectId> = self
                    .s
                    .worlds()
                    .into_iter()
                    .filter(|w| here.is_none_or(|h| self.s.accessible(h, *w)))
                    .collect();
                if reachable.is_empty() {
                    // No frame: unanswerable, not false.
                    return self.unsupported(node);
                }
                let universal = op == wk::NECESSARILY;
                let mut scan = Scan::new(universal);
                for w in reachable {
                    if self.exhausted() {
                        return self.out_of_budget(node);
                    }
                    let prev = self.world;
                    self.world = Some(w);
                    let r = self.check(*inner, env);
                    self.world = prev;
                    if scan.step(&r, self.snapshot) {
                        return scan.decided(!universal, self.snapshot);
                    }
                }
                // `worlds()` is the frame in full, so the enumeration is
                // complete by construction.
                self.finish_scan(node, &scan, true)
            }
            wk::IN_WORLD => {
                let [w, inner] = args else { return self.unsupported(node) };
                let target = self.key(*w, env);
                let prev = self.world;
                self.world = Some(target);
                let r = self.check(*inner, env);
                self.world = prev;
                self.wrap_continuation(r, wk::IN_WORLD, &[*w])
            }
            wk::COUNTERFACTUAL => {
                // `(if-counterfactually P Q)` — Q in the closest P-worlds.
                //
                // The similarity ordering is the structure's to supply, because
                // there is no agreed one: Lewis, Stalnaker and every successor
                // pick differently, and the choice is a modelling decision. A
                // structure without one gets `Unsupported`, which is the honest
                // answer rather than a quietly invented ordering.
                let [cond, conseq] = args else { return self.unsupported(node) };
                let Some(here) = self.world.or_else(|| self.s.worlds().first().copied()) else {
                    return self.unsupported(node);
                };
                let Some(closest) = self.s.closest_worlds(here, *cond) else {
                    return self.unsupported(node);
                };
                if closest.is_empty() {
                    // Vacuously true: no world satisfies the antecedent.
                    return EvaluationResult::certain(true);
                }
                // Q must hold in *every* closest P-world, so this is a
                // universal scan and carries a universal's bounds.
                let mut scan = Scan::new(true);
                for w in closest {
                    if self.exhausted() {
                        return self.out_of_budget(node);
                    }
                    let prev = self.world;
                    self.world = Some(w);
                    let r = self.check(*conseq, env);
                    self.world = prev;
                    if scan.step(&r, self.snapshot) {
                        return scan.decided(false, self.snapshot);
                    }
                }
                self.finish_scan(node, &scan, true)
            }
            wk::EVAL => {
                // `(eval (quote X))` evaluates X. The inverse of `quote`, and
                // the reason the pair is what makes the language reflective.
                let [a] = args else { return self.unsupported(node) };
                let target = env.get(a).copied().unwrap_or(*a);
                match self.g.get(target).cloned() {
                    Some(CoreNode::Apply { operator, operands })
                        if operator == wk::QUOTE && !operands.is_empty() =>
                    {
                        self.check(operands[0], env)
                    }
                    // Evaluating something that is not a quotation is just
                    // evaluating it — `eval` is idempotent on ordinary terms.
                    _ => self.check(target, env),
                }
            }
            wk::PROVABLE => {
                // Provability *within the current budget*, which is the only
                // reading an evaluator can honestly offer: a failure to derive
                // is not a proof of underivability, so this never refutes.
                let [a] = args else { return self.unsupported(node) };
                let r = self.about(*a, env);
                if r.support.is_certain() {
                    EvaluationResult::certain(true)
                } else {
                    self.stalled(node)
                }
            }
            wk::GROUNDED | wk::DEFEASIBLE | wk::DETERMINATE | wk::PRESUMED => {
                // The four axes, asked about rather than merely carried.
                //
                // Each reads a *finished* evaluation. An axis off a truncated
                // one is a guess: a defeasible step the budget never reached
                // makes `(defeasible P)` answer false about a conclusion that is
                // defeasible, which is the same manufacture-a-verdict-from-
                // absence defect one level up in the language.
                //
                // **Truncation blocks; absence does not.** `BudgetExhausted`
                // stopped early and `Unsupported` never descended, so either may
                // be hiding a loop or a defeater that was never visited. A
                // `Stalled` evaluation visited everything and found no evidence
                // — the axes it computed on the way are complete. Blocking on it
                // too was this same defect *again*, inside the fix for it: it
                // turned "nothing is known about P" into "nothing is known about
                // whether P is grounded", and an unheard-of atom is perfectly
                // grounded. Where absence really does leave nothing to report,
                // the arms below say so themselves — `Determinacy::Unknown`
                // stalls, and `defeasible` stalls on an `Open` verdict.
                let [a] = args else { return self.unsupported(node) };
                let r = self.about(*a, env);
                match r.compute_status {
                    ComputeStatus::Exact | ComputeStatus::Stalled => {}
                    ComputeStatus::BudgetExhausted => return self.out_of_budget(node),
                    ComputeStatus::Unsupported => return self.unsupported(node),
                }
                match op {
                    // Grounding and determinacy are properties of the
                    // *sentence*, settled whether or not evidence bears on it —
                    // an unheard-of atom is perfectly grounded.
                    wk::GROUNDED => {
                        EvaluationResult::certain(r.grounding == Grounding::Grounded)
                    }
                    wk::DETERMINATE => match r.determinacy {
                        Determinacy::Total => EvaluationResult::certain(true),
                        Determinacy::Indeterminate => EvaluationResult::certain(false),
                        // A sharp condition nobody wrote down. Real, and not
                        // established — which is exactly what the middle of the
                        // strength lattice is for, and the first producer of it.
                        Determinacy::Underspecified => {
                            let mut out = EvaluationResult::new(ComputeStatus::Exact);
                            out.support = Bound::Partial;
                            out.determinacy_basis = r.determinacy_basis.clone();
                            out
                        }
                        // Nobody has said. Not "no".
                        Determinacy::Unknown => self.stalled(node),
                    },
                    wk::PRESUMED => EvaluationResult::certain(
                        r.determinacy_basis == DeterminacyBasis::Presumed,
                    ),
                    // Derivation classifies a *conclusion*, so with no
                    // conclusion there is nothing to classify. `Observed` is the
                    // struct default, and reading it off an `Open` result would
                    // report "not defeasible" about a question never answered.
                    _ if r.evidential() == Evidential::Open => self.stalled(node),
                    _ => EvaluationResult::certain(r.derivation != Derivation::Observed),
                }
            }
            wk::DEFEATED_BY => {
                // `(defeated-by P D)` — is D among the things that would
                // overturn P? The evaluator has computed this set on every query
                // since the axes landed and there was no way to ask for it, so
                // *"what would change my mind about X"* was unaskable about a
                // value the answer was already sitting in.
                let [p, d] = args else { return self.unsupported(node) };
                let defeater = env.get(d).copied().unwrap_or(*d);
                let r = self.about(*p, env);
                // Same rule as the axis operators: truncation blocks, absence
                // does not. A stalled evaluation still visited every branch, so
                // the defeater set it accumulated is complete. The `Open` guard
                // just below is what actually withholds an answer here — with no
                // conclusion, there is nothing for a defeater to overturn.
                match r.compute_status {
                    ComputeStatus::Exact | ComputeStatus::Stalled => {}
                    ComputeStatus::BudgetExhausted => return self.out_of_budget(node),
                    ComputeStatus::Unsupported => return self.unsupported(node),
                }
                if r.evidential() == Evidential::Open {
                    return self.stalled(node);
                }
                // Refuting is a completeness claim, and it is earned: an exact
                // evaluation visited every step its conclusion rests on, and a
                // defeater of a branch that was short-circuited away is not a
                // defeater of the conclusion.
                EvaluationResult::certain(r.defeated_by.contains(&defeater))
            }
            wk::LIKELY => {
                // Reads the credence interval. `None` there means *not graded or
                // not expressible*, which is emphatically not "even odds" — the
                // one thing the unawareness literature agrees on — so it stalls
                // rather than answering `Open` and letting the two states look
                // alike to a caller who only checks the evidential axis.
                let [a] = args else { return self.unsupported(node) };
                let r = self.about(*a, env);
                match r.compute_status {
                    ComputeStatus::BudgetExhausted => return self.out_of_budget(node),
                    // No semantics for what is inside is not "no grade for it".
                    ComputeStatus::Unsupported => return self.unsupported(node),
                    _ => {}
                }
                let Some(c) = r.credence else { return self.stalled(node) };
                match c.decided() {
                    Some(v) => EvaluationResult::certain(v),
                    // Expressible, graded, and the interval straddles even
                    // odds. A genuine `Open/Exact`, and distinguishable from
                    // the ungraded case above.
                    None => {
                        let mut out = EvaluationResult::new(ComputeStatus::Exact);
                        out.credence = Some(c);
                        out
                    }
                }
            }
            wk::ASSERTED_BY | wk::BELIEVES | wk::SUPPORTS | wk::ATTACKS
            | wk::DERIVED_FROM | wk::SOURCE | wk::DENOTES => {
                // Intensional relations: they are *about* their arguments, so
                // the arguments are node ids to be looked up verbatim. No
                // coreference substitution happens here even when the store
                // holds `same-as` — that is precisely the barrier §7 requires,
                // and it is why these cannot share the ordinary predicate path.
                // Bound variables are substituted here as everywhere else.
                // Blocking that is not what §7 asks for: rewriting one *name*
                // into another is coreference, and binding a quantified
                // variable is not — conflating them made "for every x, someone
                // believes something about x" unaskable.
                let tuple: Vec<ObjectId> =
                    args.iter().map(|a| self.key(*a, env)).collect();
                self.resolve_or_derive(node, op, &tuple, env)
            }
            wk::ALWAYS | wk::EVENTUALLY => {
                // Quantify the inner proposition over the structure's
                // instants. `instants()` was implemented by `RelationalView` and
                // never called, so `(eventually P)` fell through to the fact
                // table as a binary predicate over node ids.
                let [inner] = args else { return self.unsupported(node) };
                let ts = self.s.instants();
                if ts.is_empty() {
                    // A timeless structure makes a temporal claim
                    // *unanswerable* rather than false — there is no instant to
                    // witness it either way. Not "vacuous", which the old
                    // comment said: a vacuous universal is true, and this is
                    // Open.
                    return self.stalled(node);
                }
                let universal = op == wk::ALWAYS;
                let mut scan = Scan::new(universal);
                for t in ts {
                    if self.exhausted() {
                        return self.out_of_budget(node);
                    }
                    let prev = self.instant;
                    self.instant = Some(t);
                    let r = self.check(*inner, env);
                    self.instant = prev;
                    if scan.step(&r, self.snapshot) {
                        return scan.decided(!universal, self.snapshot);
                    }
                }
                // `instants()` reports every instant at which anything changed,
                // so the enumeration is complete.
                self.finish_scan(node, &scan, true)
            }
            wk::DURING => {
                // `(during t (interval a b))` — half-open, so adjacent
                // intervals tile the line without overlapping.
                let [t, iv] = args else { return self.unsupported(node) };
                let Some(when) = self.instant_of(*t, env) else { return self.stalled(node) };
                let target = env.get(iv).copied().unwrap_or(*iv);
                let Some(CoreNode::Apply { operator, operands }) = self.g.get(target).cloned()
                else {
                    return self.stalled(node);
                };
                if operator != wk::INTERVAL {
                    return self.unsupported(node);
                }
                let [from, to] = operands.as_slice() else { return self.unsupported(node) };
                match (self.instant_of(*from, env), self.instant_of(*to, env)) {
                    (Some(a), Some(b)) => EvaluationResult::certain(a <= when && when < b),
                    _ => self.stalled(node),
                }
            }
            wk::BEFORE => {
                // Temporal order over instants. Symbolic instants resolve
                // through the structure, so `(before (session-start s7) now)`
                // is answerable without writing raw epoch integers.
                let [a, b] = args else { return self.unsupported(node) };
                if let (Some(x), Some(y)) = (self.instant_of(*a, env), self.instant_of(*b, env))
                {
                    return EvaluationResult::certain(x < y);
                }
                // Not everything ordered is a *time*. "Run `cargo fmt` before
                // committing" orders two actions, and requiring both sides to
                // denote instants made the normal case for a process fact
                // inexpressible — as it did for dependency order, review order,
                // and every other precedence a repository has.
                let (x, y) = (self.key(*a, env), self.key(*b, env));
                self.resolve_or_derive(node, op, &[x, y], env)
            }
            wk::SINCE => {
                // `(since P Q)` — P has held at every instant from the most
                // recent one at which Q held.
                //
                // Declared in `wk::` since the beginning, matched nowhere, and
                // covered by §5.5's claim that the temporal operators converge
                // over a finite instant set. `always`/`eventually` cannot stand
                // in for it: they range over *every* instant with no way to
                // restrict to a window, which is the entire content of `since`.
                let [p, q] = args else { return self.unsupported(node) };
                let ts = self.s.instants();
                if ts.is_empty() {
                    return self.stalled(node);
                }
                let mut from = None;
                for t in &ts {
                    let prev = self.instant;
                    self.instant = Some(*t);
                    let r = self.check(*q, env);
                    self.instant = prev;
                    if r.support.is_certain() {
                        // The *latest* onset, not whichever the iteration
                        // reached last: `instants()` carries no ordering
                        // guarantee in the trait, and `since` was its only
                        // order-sensitive consumer.
                        from = Some(from.map_or(*t, |f: i64| f.max(*t)));
                    }
                }
                // No witnessed onset: the window is undefined, which is not the
                // same as the claim being false.
                let Some(from) = from else { return self.stalled(node) };
                let mut scan = Scan::new(true);
                for t in ts.into_iter().filter(|t| *t >= from) {
                    if self.exhausted() {
                        return self.out_of_budget(node);
                    }
                    let prev = self.instant;
                    self.instant = Some(t);
                    let r = self.check(*p, env);
                    self.instant = prev;
                    if scan.step(&r, self.snapshot) {
                        return scan.decided(false, self.snapshot);
                    }
                }
                self.finish_scan(node, &scan, true)
            }
            wk::OBLIGED | wk::PERMITTED | wk::FORBIDDEN => {
                // A norm is a *stored* fact about what ought to be, and it is
                // deliberately **not** evaluated by checking whether it holds.
                // Reading `(obliged P)` as "P in every accessible world" makes a
                // rule false exactly when it is broken, which destroys the only
                // reason to record one.
                let [inner] = args else { return self.unsupported(node) };
                let target = self.key(*inner, env);
                let direct = self.lookup(op, &[target]);
                if let Some(r) = direct.verdict() {
                    return self.qualified(node, r);
                }
                // Two entailments, and no more: an obligation implies a
                // permission, and forbidding something is obliging its negation.
                let derived = match op {
                    // Via `check`, not a bare lookup, so the two declared
                    // entailments chain: `forbidden(P)` gives `obliged(¬P)`
                    // gives `permitted(¬P)`.
                    wk::PERMITTED => {
                        let obliged = self.g.apply(wk::OBLIGED, vec![target]);
                        let r = self.check(obliged, env);
                        if r.support.is_certain() {
                            Knowledge::Holds
                        } else {
                            Knowledge::Unknown
                        }
                    }
                    wk::FORBIDDEN => {
                        let negated = self.g.apply(wk::NOT, vec![target]);
                        self.lookup(wk::OBLIGED, &[negated])
                    }
                    // …and the same identity read the other way. §4 states
                    // "forbidding **is** obliging a negation", and an identity
                    // that holds in one direction only is not one.
                    wk::OBLIGED => match self.g.get(target).cloned() {
                        Some(CoreNode::Apply { operator, operands })
                            if operator == wk::NOT && operands.len() == 1 =>
                        {
                            self.lookup(wk::FORBIDDEN, &[operands[0]])
                        }
                        _ => Knowledge::Unknown,
                    },
                    _ => Knowledge::Unknown,
                };
                let out = match derived {
                    Knowledge::Holds => EvaluationResult::certain(true),
                    _ => self.derive(node, op, &[target], env),
                };
                self.qualified(node, out)
            }
            wk::VIOLATED => {
                // The bridge from norms back to truth, and the reason keeping
                // them apart costs nothing: a norm on file, plus the world
                // failing to match it.
                let [inner] = args else { return self.unsupported(node) };
                let target = self.key(*inner, env);
                let obliged = self.g.apply(wk::OBLIGED, vec![target]);
                let holds = self.check(target, env);
                let norm = self.check(obliged, env);
                // A contested norm, or a contested fact, makes the violation
                // contested — not whichever branch is written first.
                if (norm.support.is_certain() && norm.refutation.is_certain())
                    || (holds.support.is_certain() && holds.refutation.is_certain())
                {
                    return EvaluationResult::conflicted();
                }
                if norm.support.is_certain() && holds.refutation.is_certain() {
                    return EvaluationResult::certain(true);
                }
                if norm.refutation.is_certain() || holds.support.is_certain() {
                    return EvaluationResult::certain(false);
                }
                // …and a violation may itself be concluded by a rule. This was
                // the one reserved arm `resolve_or_derive` did not reach.
                self.derive(node, op, &[target], env)
            }
            wk::TYPE_OF => {
                // `(type-of x T)` is a *relation*, and it was declared in `wk::`
                // and matched nowhere — so a reserved name returned
                // `Unsupported` and never fell through to the store, making
                // "this field is a `usize`" permanently unretrievable if you
                // chose the reserved spelling to record it.
                let [x, t] = args else { return self.unsupported(node) };
                let tuple = [self.key(*x, env), self.key(*t, env)];
                self.resolve_or_derive(node, op, &tuple, env)
            }
            // A sequence is a value, not a proposition — `(seq …)` denotes and
            // `nth` projects. Both are read through `denote`.
            wk::SEQ | wk::NTH => self.unsupported(node),
            wk::AT => {
                // Evaluate against the structure as it was at an instant.
                // `always` and `eventually` derive from it by quantifying over
                // every instant, and `since` by quantifying over a window.
                let [t, inner] = args else { return self.unsupported(node) };
                let Some(when) = self.instant_of(*t, env) else { return self.stalled(node) };
                let prev = self.instant;
                self.instant = Some(when);
                let r = self.check(*inner, env);
                self.instant = prev;
                self.wrap_continuation(r, wk::AT, &[*t])
            }
            wk::HOLDS => {
                // Descend into a quotation — deliberately, never automatically.
                let [a] = args else { return self.unsupported(node) };
                let target = env.get(a).copied().unwrap_or(*a);
                match self.g.get(target) {
                    Some(CoreNode::Apply { operator, operands })
                        if *operator == wk::QUOTE && !operands.is_empty() =>
                    {
                        let inner = operands[0];
                        self.check(inner, env)
                    }
                    _ => self.stalled(node),
                }
            }
            // Quoting is opaque by design: `⟨P⟩` is a name for P, not a claim.
            wk::QUOTE => self.unsupported(node),
            _ if env.rels.contains_key(&op) => {
                let rel = &env.rels[&op];
                let ground: Option<Vec<ObjectId>> =
                    args.iter().map(|a| env.get(a).copied().or(Some(*a))).collect();
                let Some(tuple) = ground else { return self.stalled(node) };
                if rel.tuples.contains(&tuple) {
                    EvaluationResult::certain(true)
                } else if rel.complete {
                    EvaluationResult::certain(false)
                } else {
                    // Mid-fixpoint: absence is provisional, not refutation.
                    self.stalled(node)
                }
            }
            _ if self.is_lambda(op) => {
                // Applying an inline definition: bind the parameters and check
                // the body. Intensional, where a bound relation is extensional.
                let Some(CoreNode::Bind { vars, bodies, .. }) = self.g.get(op).cloned() else {
                    return self.unsupported(node);
                };
                if vars.len() != args.len() || bodies.is_empty() {
                    return self.unsupported(node);
                }
                let Some((_, vars, bodies)) = self.g.open_binder(op) else {
                    return self.unsupported(node);
                };
                let mut scoped = env.clone();
                for (b, a) in vars.iter().zip(args) {
                    scoped.vars.insert(b.var, env.get(a).copied().unwrap_or(*a));
                }
                let r = self.check(bodies[0], &scoped);
                self.seal(r)
            }
            _ => {
                // Not a built-in. Ask the registry; absence is its own answer.
                if self.registry.has(op) {
                    let mut budget = self.budget.saturating_sub(self.spent);
                    let r = self.registry.evaluate(
                        self.g,
                        node,
                        op,
                        args,
                        &mut budget,
                        self.snapshot,
                    );
                    self.spent = self.spent.saturating_add(1);
                    // A registered operator is the one place a result crosses
                    // into the evaluator from code it does not control, so it is
                    // where the version invariant is actually load-bearing: an
                    // answer computed against a different snapshot of the
                    // universe cannot be folded into this one.
                    if !self.compose_snapshot(&r) {
                        return self.stalled(node);
                    }
                    return r;
                }
                // A **well-known** operator with no semantics is unsupported,
                // not a predicate. Falling through to the structure made the
                // reserved vocabulary indistinguishable from user predicates:
                // `(necessarily P)` became true or false according to whether a
                // fact table happened to hold a row for the pair of node ids,
                // with no accessibility relation anywhere in sight — and a
                // resolver marked closed turned "I have no modal semantics"
                // into a confident refutation.
                //
                // `Unsupported` is also the answer the crate docs already
                // promise: distinct from "no evidence" and from "out of
                // budget", so registering semantics later is a pure addition.
                if wk::name_of(op).is_some() {
                    return self.unsupported(node);
                }

                // An uninterpreted *user* predicate: consult the structure.
                // A relation variable used as an *argument* names a relation
                // this scan invented, not an entity the store has ever seen.
                // Asking about it invites an authoritative answer to a question
                // nobody asked.
                if args.iter().any(|a| env.is_relation_var(a)) {
                    return self.stalled(node);
                }
                let tuple: Vec<ObjectId> = args.iter().map(|a| self.key(*a, env)).collect();
                // `at` and `in-world` compose: a claim can be about a past
                // state *of a counterfactual world*, and the old match dropped
                // the world whenever an instant was set — so
                // `(at t3 (in-world w (green artist)))` was silently answered
                // against the wrong index.
                let answer = match (self.instant, self.world) {
                    (Some(t), Some(w)) => self.s.known_at_in(op, &tuple, t, w),
                    (Some(t), None) => self.s.known_at(op, &tuple, t),
                    (None, Some(w)) => self.s.known_in(op, &tuple, w),
                    (None, None) => self.lookup(op, &tuple),
                };
                // Only the resolver may license a refutation from absence.
                // `is_closed` is a relation-level flag and cannot speak for a
                // query that timed out, hit a partial index, or was refused.
                let out = match answer.verdict() {
                    Some(r) => {
                        if self.trace.is_some() {
                            // About the **instantiated** proposition. Under a
                            // scan the body node is the same for every member —
                            // the variable is bound in the environment, not
                            // substituted into the id — so naming it here made
                            // every member's step indistinguishable, and an
                            // `Instance` had nothing to be checked against.
                            let inst = self.g.apply(op, tuple.clone());
                            // Who said so. Naming `op` here — the relation
                            // symbol — made every fact its own authority, so a
                            // certificate's one declared trust point pointed at
                            // itself and the asserting agent sitting in the
                            // store went unasked-for.
                            let sources = self.s.attribution(inst);
                            // A conflict is `aff ≠ ∅ ∧ den ≠ ∅` under semantics
                            // §8.2 — two pieces of testimony, not a primitive.
                            // The old `Conflict` step minted `Certain` on *both*
                            // sides for an arbitrary id with no lookup at all,
                            // making it a second undeclared trust point and a
                            // valid premise for every other rule. There is no
                            // rule for it now, so nothing is emitted and the
                            // certificate simply does not conclude this node.
                            if answer != Knowledge::Conflicted {
                                self.emit(Step::Told {
                                    node: inst,
                                    holds: r.support.is_certain(),
                                    sources,
                                });
                            }
                        }
                        r
                    }
                    None => self.derive(node, op, &tuple, env),
                };
                self.qualified(node, out)
            }
        }
    }

    /// A reserved relation the store cannot answer directly, tried as a goal.
    ///
    /// `derive` was called from exactly one site — the uninterpreted
    /// user-predicate arm — so every reserved relation ended in
    /// `lookup(…).verdict()` else `stalled`. A conditional norm ("on CI the
    /// `--release` flag is forbidden"), a derived type fact ("every field named
    /// `*_id` is a `u64`"), derived coreference, a derived preference and
    /// derived provenance were all storable, indexable, satisfiable and
    /// **inert** — the same defect §5.4 identified and fixed for `usually`,
    /// still present across the rest of the vocabulary. A norm that can only
    /// ever be a ground term cannot be stated about a class of situations,
    /// which is most of what a norm is for.
    /// Attach the proposition's determinacy to a verdict about it, and — in
    /// certification mode — refuse a classical verdict that totality does not
    /// license.
    ///
    /// The evidence survives either way. A borderline claim with real evidence
    /// behind it reports `Supported / Indeterminate`, because the grounds are
    /// genuine even where there is no sharp fact to be right about. What
    /// certification withholds is the *classical* reading, not the evidence.
    ///
    /// Applied whatever the outcome, including an absence: *"there is no sharp
    /// fact here"* is information about the proposition, and it is true whether
    /// or not the store also happens to have evidence.
    /// Evaluate the proposition `a` denotes, **without letting its axes reach
    /// the enclosing verdict**.
    ///
    /// A claim about a sentence is not the sentence. `(grounded L)` for the liar
    /// must be a perfectly grounded `Refuted`; `(defeasible P)` for a default P
    /// is an observed fact about P, not another default. Every other compound
    /// wants the opposite — `dispatch` stamps a node with everything beneath it,
    /// which is what makes axis loss unexpressible — so reflection is the one
    /// place that has to opt out, deliberately and in one function rather than
    /// per operator.
    ///
    /// `provable` did not opt out, and inherited its argument's grounding for as
    /// long as it has existed.
    ///
    /// Unwraps one `quote`, so `(grounded (quote P))` and `(grounded P)` agree.
    /// The quoted form is what a stored claim looks like; the bare form is what
    /// a query looks like, and answering them differently would be a distinction
    /// drawn by spelling.
    fn about(&mut self, a: ObjectId, env: &GraphEnv) -> EvaluationResult {
        let target = env.get(&a).copied().unwrap_or(a);
        let inner = match self.g.get(target).cloned() {
            Some(CoreNode::Apply { operator, operands })
                if operator == wk::QUOTE && !operands.is_empty() =>
            {
                operands[0]
            }
            _ => target,
        };
        let outer = std::mem::take(&mut self.axes);
        let r = self.check(inner, env);
        self.axes = outer;
        r
    }

    fn qualified(&mut self, node: ObjectId, mut r: EvaluationResult) -> EvaluationResult {
        let d = self.s.determinacy(node);
        r.determinacy = r.determinacy.merge(d);
        // Anything the store actually answered was declared by it; silence is
        // what leaves the verdict presumed.
        if d != Determinacy::Unknown {
            r.determinacy_basis = DeterminacyBasis::Declared(vec![node]);
        }
        // `or`, not assignment: a composite that folded credences from its parts
        // must not have them cleared by a store that grades nothing at this node.
        r.credence = self.s.credence(node).or(r.credence);
        if self.mode == Mode::Certify && !d.licenses_classical() {
            let mut out = self.stalled(node);
            out.determinacy = r.determinacy;
            out.grounding = r.grounding;
            out.derivation = r.derivation;
            return out;
        }
        r
    }

    fn resolve_or_derive(
        &mut self,
        node: ObjectId,
        op: ObjectId,
        tuple: &[ObjectId],
        env: &GraphEnv,
    ) -> EvaluationResult {
        if tuple.iter().any(|a| env.is_relation_var(a)) {
            return self.stalled(node);
        }
        let out = match self.lookup(op, tuple).verdict() {
            Some(r) => r,
            None => self.derive(node, op, tuple, env),
        };
        self.qualified(node, out)
    }

    /// Backward chaining: try each stored rule that concludes `pred`.
    ///
    /// A rule is `(forall [(x …)] (implies A C))`. Match the goal against `C`;
    /// where that binds the rule's variables, evaluate `A` under the binding.
    /// A supported antecedent supports the goal.
    ///
    /// Recursion is bounded twice over. `rule_depth` caps chain length, and the
    /// ordinary path/parity machinery already refuses to re-enter a goal that
    /// is being derived — so a rule whose antecedent leads back to its own
    /// conclusion yields `Open` rather than spinning.
    fn derive(
        &mut self,
        node: ObjectId,
        pred: ObjectId,
        goal_args: &[ObjectId],
        env: &GraphEnv,
    ) -> EvaluationResult {
        if self.rule_depth >= MAX_RULE_DEPTH {
            return self.stalled(node);
        }
        let candidates = match self.instant {
            Some(t) => self.s.rules_at(pred, t),
            None => self.s.rules(pred),
        };
        if candidates.is_empty() {
            return self.stalled(node);
        }

        let mut worst = ComputeStatus::Exact;
        for rule in candidates {
            if self.exhausted() {
                return self.out_of_budget(node);
            }
            let Some((binder, vars, bodies)) = self.g.open_binder(rule) else { continue };
            if binder != wk::FORALL {
                continue;
            }
            let Some(imp) = bodies.first().copied() else { continue };
            let Some(CoreNode::Apply { operator, operands }) = self.g.get(imp).cloned() else {
                continue;
            };
            if operator != wk::IMPLIES || operands.len() != 2 {
                continue;
            }
            let (antecedent, consequent) = (operands[0], operands[1]);

            let Some(m) = self.match_goal(consequent, pred, goal_args, &vars, 0) else {
                continue;
            };
            let mut scoped = env.clone();
            for (v, val) in &m.bindings {
                scoped.vars.insert(*v, *val);
            }

            self.rule_depth += 1;
            let r = self.check(antecedent, &scoped);
            self.rule_depth -= 1;

            if r.support.is_certain() {
                // **No step is emitted, and this is a known gap rather than a
                // decision.** `ModusPonens` needs a premise establishing the
                // *instantiated* implication `A → C`, and reaching one from the
                // stored `∀x⃗. A → C` requires universal instantiation — the
                // converse direction of `Step::Instance`, which goes instance to
                // binder. That rule is sound in FOUR (a meet over the extension
                // is `≤_t` each instance) and cheap, but it is not among the
                // eight, so emitting anything here would be certifying with a
                // rule the spec does not contain. Recorded in semantics §12.
                //
                // Defeasible firings are a separate matter and are not
                // certifiable at all until §12's fixpoint theorem exists.
                return m.conclude(self.snapshot, node);
            }
            // A refuted antecedent says nothing: the rule simply does not
            // apply, and another might. Only an inconclusive one is worth
            // reporting, since it means more budget or more facts could help.
            if !r.refutation.is_certain() {
                worst = downgrade(worst, r.compute_status);
            }
        }
        // …and that report was then thrown away — `let _ = worst;` — so a
        // derivation whose antecedent ran out of budget came back `Stalled`,
        // documented as "re-asking will stall the same way". Re-asking with more
        // budget is exactly what would have helped, and the resumable work went
        // with it.
        if worst != ComputeStatus::Exact {
            return self.combine(node, Bound::None, Bound::None, worst, Vec::new());
        }
        self.stalled(node)
    }

}

/// A rule's conclusion matched against a goal.
///
/// The conclusion is not always the bare goal atom. `∀x. under-tests(x) →
/// usually (needs-review x)` concludes something *about* `needs-review`, and the
/// wrapper is part of what the rule says — so it has to travel with the
/// bindings and be reapplied to the derived answer. Firing that rule and
/// returning `Certain` would turn every default into a fact the moment it was
/// written as a rule instead of a ground term.
struct GoalMatch {
    bindings: Vec<(ObjectId, ObjectId)>,
    negated: bool,
    defeasible: bool,
}

impl GoalMatch {
    fn conclude(&self, snapshot: u64, goal: ObjectId) -> EvaluationResult {
        let mut out = EvaluationResult::certain(!self.negated);
        out.snapshot = snapshot;
        if self.defeasible {
            return out.defeasible(goal);
        }
        out.derivation = Derivation::Derived;
        out
    }
}

impl State<'_> {
    /// Match a rule's conclusion against a goal, binding the rule's variables.
    ///
    /// Deliberately first-order and structural: same operator, same arity, and
    /// each conclusion argument is either one of the rule's variables (bind it)
    /// or a constant that must match. No unification of nested terms — that is
    /// a different and much larger machine, and the rules a memory holds do not
    /// need it.
    fn match_goal(
        &mut self,
        consequent: ObjectId,
        pred: ObjectId,
        goal_args: &[ObjectId],
        vars: &[crate::object::Binding],
        depth: u32,
    ) -> Option<GoalMatch> {
        let Some(CoreNode::Apply { operator, operands }) = self.g.get(consequent).cloned() else {
            return None;
        };
        // A rule may conclude something *about* the goal predicate rather than
        // the bare atom. Requiring the conclusion's operator to **be** the goal
        // predicate meant a rule could only ever conclude a positive ground
        // atom — so "usually, a file under `tests/` does not need review", which
        // is the shape almost every default takes, was storable, indexable,
        // satisfiable, and inert. Defaults existed only as ground facts, which
        // is not the form a default comes in.
        // Bounded, like every other recursion in this file. `match_term` thirty
        // lines below carries a depth of 32 and every reader carries a guard;
        // this walk carried none, so a rule whose conclusion is the cyclic node
        // `C = (not C)` — parseable as `#1=(not #1#)` — recursed until the stack
        // died. A crash is not one of the four compute statuses.
        if depth > 32 {
            return None;
        }
        if matches!(operator, wk::USUALLY | wk::NOT)
            && operands.len() == 1
            && let Some(inner) = operands.last().copied()
        {
            let mut m = self.match_goal(inner, pred, goal_args, vars, depth + 1)?;
            if operator == wk::NOT {
                // `¬usually(P)` says the *default* does not hold. That is not
                // evidence against `P`, and treating the two wrappers as
                // commutative made it indistinguishable from `usually(¬P)`,
                // which is.
                if m.defeasible {
                    return None;
                }
                m.negated = !m.negated;
            } else {
                m.defeasible = true;
            }
            return Some(m);
        }
        if operator != pred || operands.len() != goal_args.len() {
            return None;
        }
        let names: Vec<ObjectId> = vars.iter().map(|b| b.var).collect();
        let mut out: Vec<(ObjectId, ObjectId)> = Vec::new();
        for (c, g) in operands.iter().zip(goal_args) {
            if !self.match_term(*c, *g, &names, &mut out, 0) {
                return None;
            }
        }
        Some(GoalMatch { bindings: out, negated: false, defeasible: false })
    }

    /// Match one conclusion argument against one goal argument, structurally.
    ///
    /// This used to compare only at the top level: an argument was either a bare
    /// rule variable or a constant that had to be identical. So a rule could
    /// only ever be written about arguments that are *whole* — and the moment a
    /// conclusion said something about a compound, as
    /// `∀x. on-ci(x) → forbidden(uses(x, --release))` does, nothing matched.
    /// Conditional norms take that shape almost by definition, and it is the
    /// shape §9 asks for over composed predicate names.
    ///
    /// Still first-order and still not unification: the goal side is ground, so
    /// this is one-way matching with a consistency check, and no occurs-check or
    /// substitution composition is needed.
    fn match_term(
        &mut self,
        pattern: ObjectId,
        goal: ObjectId,
        names: &[ObjectId],
        out: &mut Vec<(ObjectId, ObjectId)>,
        depth: u32,
    ) -> bool {
        if depth > 32 {
            return false;
        }
        if names.contains(&pattern) {
            // A variable already bound must bind consistently.
            return match out.iter().find(|(v, _)| *v == pattern) {
                Some((_, prev)) => *prev == goal,
                None => {
                    out.push((pattern, goal));
                    true
                }
            };
        }
        if pattern == goal {
            return true;
        }
        match (self.g.get(pattern).cloned(), self.g.get(goal).cloned()) {
            (
                Some(CoreNode::Apply { operator: po, operands: pa }),
                Some(CoreNode::Apply { operator: go, operands: ga }),
            ) => {
                if po != go || pa.len() != ga.len() {
                    return false;
                }
                pa.iter()
                    .zip(ga.iter())
                    .all(|(x, y)| self.match_term(*x, *y, names, out, depth + 1))
            }
            // A conclusion whose content is itself general — "every module must
            // have all its files formatted" — has a binder in it, and bailing on
            // anything but `Apply` meant such a rule never matched. The slots are
            // de Bruijn on both sides, so they compare structurally.
            (
                Some(CoreNode::Bind { binder: pb, vars: pv, bodies: pbod }),
                Some(CoreNode::Bind { binder: gb, vars: gv, bodies: gbod }),
            ) => {
                if pb != gb || pv != gv || pbod.len() != gbod.len() {
                    return false;
                }
                pbod.iter()
                    .zip(gbod.iter())
                    .all(|(x, y)| self.match_term(*x, *y, names, out, depth + 1))
            }
            _ => false,
        }
    }

    fn bind(
        &mut self,
        node: ObjectId,
        binder: ObjectId,
        vars: &[crate::object::Binding],
        bodies: &[ObjectId],
        env: &GraphEnv,
    ) -> EvaluationResult {
        if binder == wk::LETREC {
            return self.letrec(node, vars, bodies, env);
        }
        let universal = match binder {
            wk::FORALL => true,
            wk::EXISTS => false,
            // A lambda is a value, not a proposition; aggregates denote numbers
            // and are read through `number_bounds`.
            _ => return self.unsupported(node),
        };
        // **More than one slot is more than one quantifier.** Taking
        // `vars.first()` and dropping the rest left slots 1..n as the free
        // skolems `open_binder` minted, so the body was evaluated with a name
        // the store has never seen — and a closed resolver answered `Fails`
        // about it, which the scan turned into `Refuted/Exact` for a
        // *true* claim. `(exists [(x M) (y M)] (calls x y))` is documented
        // surface syntax, round-trips through the printer, and came back
        // confidently false.
        //
        // Re-associating into nested single-slot binders is exact rather than
        // approximate: the slots are already opened to distinct nominals, so
        // abstracting them back one at a time reconstructs the same meaning,
        // and telescoping domains keep working because an inner domain may
        // mention an outer variable.
        if vars.len() > 1
            && let Some(body) = bodies.first().copied()
        {
            let mut nested = body;
            for v in vars.iter().rev() {
                nested = self.g.quantify(binder, v.var, v.domain, nested);
            }
            return self.check(nested, env);
        }
        // Second-order: a variable whose domain is a relation type ranges over
        // relations, not individuals.
        if let Some(v) = vars.first()
            && let Some(d) = v.domain
            && self.is_relation_type(d)
        {
            return self.second_order(node, universal, v, bodies, env, d);
        }
        let (Some(v), Some(body)) = (vars.first(), bodies.first()) else {
            return self.unsupported(node);
        };
        let Some(domain) = v.domain else { return self.unsupported(node) };
        let Some(members) = self.members_of(domain, env) else {
            // Unenumerable. Two very different reasons, and conflating them was
            // unsound: the domain may be *known* to be the integers — where
            // probing and interval widening are the right move, and infinity
            // costs precision rather than termination — or it may simply be a
            // domain this evaluator does not recognise.
            //
            // Treating the second as the first meant an unrecognised domain
            // silently became "all integers". `(forall [(x (Refine Int λn. 0≤n))]
            // (0 ≤ x))` was **Refuted**, because the prober walked to n = −1 and
            // called it a counterexample over a domain that excludes it.
            if self.is_integer_domain(domain) {
                return self.infinite(node, universal, v, *body, domain, env);
            }
            return self.stalled(node);
        };

        let Extension { members, complete } = members;
        let mut scan = Scan::new(universal);
        for (i, m) in members.iter().enumerate() {
            if self.exhausted() {
                // Keep the question, narrowed to what is left. The continuation
                // is an ordinary expression — the *same* quantifier over the
                // unexamined tail — so resuming is just evaluating it, and each
                // pass strictly shrinks the domain rather than restarting.
                let mut r = self.out_of_budget(node);
                // A **contested** member has both bounds certain, so it looks
                // decided to the gated side and clean to the status — the
                // deciding side is the only thing that shows it. Without that
                // conjunct a `Conflicted` prefix resumed to a definite verdict.
                //
                // **What the prefix already established travels with the
                // continuation, or the continuation is a different question.**
                // Rebuilding over the tail alone restarted the meet side at
                // `Certain` and kept `complete: true`, so a universal whose
                // examined members left it honestly `Open` resumed to
                // `Supported/Exact` — and a `Conflicted` prefix resumed to a
                // definite verdict in the opposite direction. Which answer you
                // got depended on where the budget ran out.
                let clean = scan.gated == Bound::Certain
                        && scan.free == Bound::None
                        && scan.worst == ComputeStatus::Exact;
                r.continuation =
                    Some(self.narrow(binder, v, &members[i..], *body, complete && clean));
                r.dependencies = members[i..].to_vec();
                return r;
            }
            let scoped = env.with(v.var, *m);
            let r = self.check(*body, &scoped);
            // **A member cut short mid-body belongs in the tail, not the
            // prefix.** The exhaustion test at the top of the loop leaves it
            // examined-but-undecided and excludes it from the continuation, so
            // its undecidedness is folded into the scan and then lost — which
            // forces the whole prefix to be marked unclean and makes the
            // continuation unable to conclude anything the original could.
            // Handing it back unexamined keeps the prefix honest *and* the
            // continuation answerable.
            if r.compute_status == ComputeStatus::BudgetExhausted {
                let mut out = self.out_of_budget(node);
                let clean =
                    scan.gated == Bound::Certain
                        && scan.free == Bound::None
                        && scan.worst == ComputeStatus::Exact;
                out.continuation =
                    Some(self.narrow(binder, v, &members[i..], *body, complete && clean));
                out.dependencies = members[i..].to_vec();
                return out;
            }
            // A counterexample decides a universal; a witness decides an
            // existential. Both are definite, both stop the scan, and **both are
            // the answer to a question somebody asked** — so the member travels
            // with the verdict instead of being discarded at the point it was
            // found.
            if scan.step(&r, self.snapshot) {
                if self.trace.is_some() {
                    let inst = self.substitute(*body, &scoped, 0);
                    // Emit only what the kernel can actually verify. Under a
                    // re-associated multi-slot binder the body still mentions an
                    // *outer* variable, so `instance` is not this binder's body
                    // under this value and no honest step can be written — the
                    // certificate then proves less, which is documented and
                    // fine, rather than carrying a step that cannot check.
                    // Against the **stored** body, which is de Bruijn — the
                    // opened one carries nominals and would never match the
                    // form the kernel reads back out of the graph.
                    let stored = match self.g.get(node) {
                        Some(CoreNode::Bind { bodies, .. }) => bodies.first().copied(),
                        _ => None,
                    };
                    let verifiable = stored.is_some_and(|b| {
                        crate::certificate::instantiates(self.g, b, *m, inst, 0)
                    });
                    if verifiable
                        && let Some(k) = self.cite(inst)
                    {
                        self.emit(Step::Instance {
                            node,
                            premise: k,
                            value: *m,
                            instance: inst,
                            holds: !universal,
                        });
                    }
                }
                return scan.decided(!universal, self.snapshot).witnessed(0, *m);
            }
        }
        // Having examined every member we *could see* concludes nothing unless
        // those were all of them. A universal needs the absence of a
        // counterexample to be an absence in the world, not in the index; an
        // existential needs the same of its witness. Both directions of this
        // conclusion require completeness, which is why one guard covers them —
        // and why it lives in `Scan::finish` rather than being re-derived here.
        self.finish_scan(node, &scan, complete)
    }

    /// Quantification across an infinite domain.
    ///
    /// Two phases. Concrete probing finds a witness or a counterexample in
    /// finite time when one is near; failing that, the linear fragment decides
    /// what the body does over the whole unexamined tail. The analysis is
    /// finite even though the domain is not, and when neither phase lands the
    /// answer is an honest stall.
    ///
    /// The probed region has to be accounted for separately, which the earlier
    /// version did not do. A probe that came back `Open` establishes nothing
    /// about that point — so "no witness in the tail" only refutes an
    /// existential if every probe was *certainly refuted*, and "holds
    /// throughout the tail" only supports a universal if every probe was
    /// certainly supported. `Mixed` needs neither, because the witness it
    /// reports is in the tail by construction.
    fn infinite(
        &mut self,
        node: ObjectId,
        universal: bool,
        v: &crate::object::Binding,
        body: ObjectId,
        domain: ObjectId,
        env: &GraphEnv,
    ) -> EvaluationResult {
        let signed = !self.is_nat(domain);
        let reserve = 64u64.min(self.budget.saturating_sub(self.spent) / 2);
        let mut i: i64 = 0;
        let mut every_probe_supported = true;
        let mut every_probe_refuted = true;
        // Phase 1 — concrete probing, depth taken from the budget rather than a
        // constant, so more budget genuinely buys more search.
        while self.budget.saturating_sub(self.spent) > reserve && i < 1_000_000 {
            let n = if signed {
                if i % 2 == 0 { i / 2 } else { -(i / 2) - 1 }
            } else {
                i
            };
            let lit = self.g.int(n);
            let scoped = env.with(v.var, lit);
            let r = self.check(body, &scoped);
            match (universal, r.support.is_certain(), r.refutation.is_certain()) {
                (true, _, true) => return EvaluationResult::certain(false),
                (false, true, _) => return EvaluationResult::certain(true),
                _ => {}
            }
            every_probe_supported &= r.support.is_certain();
            every_probe_refuted &= r.refutation.is_certain();
            i += 1;
        }
        if self.exhausted() {
            return self.out_of_budget(node);
        }
        // Phase 2 — decide the tail. For a signed domain the probes alternate
        // outward, so the unexamined region is two tails rather than one and
        // `None` — every integer — is the honest description of it.
        let lo = if signed { None } else { Some(i) };
        match self.tail_verdict(body, v.var, lo, env, 0) {
            Tail::AllHold if every_probe_supported => EvaluationResult::certain(true),
            Tail::NoneHold if every_probe_refuted => EvaluationResult::certain(false),
            // A witness or counterexample inside the tail settles it on its
            // own, whatever the probes did.
            Tail::Mixed => EvaluationResult::certain(!universal),
            _ => self.stalled(node),
        }
    }

    fn is_nat(&mut self, d: ObjectId) -> bool {
        d == wk::NAT_TYPE
            || matches!(self.g.get(d), Some(CoreNode::Apply { operator, .. })
                        if *operator == wk::NAT_TYPE)
    }

    /// Is `d` a domain whose members are integers, so probing and interval
    /// widening are meaningful?
    ///
    /// This must be a *positive* test. Using "not enumerable" as the trigger
    /// made every unrecognised domain — a refinement type, an unmodelled sort,
    /// a typo — behave as the integers, which turns a probe at `n = −1` into a
    /// counterexample for a domain that may not contain it.
    fn is_integer_domain(&mut self, d: ObjectId) -> bool {
        if d == wk::INT_TYPE || self.is_nat(d) {
            return true;
        }
        // `(Refine Int p)` still ranges over integers, so probing is sound —
        // the refinement narrows which ones, and a probe outside it is not a
        // counterexample. Until the predicate is consulted, stay honest.
        false
    }

    /// What `body` does across `[lo, ∞)` — or across every integer when `lo` is
    /// `None`.
    ///
    /// Only the linear fragment is interpreted: a comparison is normalised to
    /// `(a − b) ≤ 0` first so correlated occurrences of the variable cancel —
    /// without that, plain intervals cannot even prove `n ≤ n + 1`.
    ///
    /// This used to answer `Option<bool>`, meaning *"does it hold everywhere?"*,
    /// and the caller read `Some(false)` as *"it holds nowhere"*. Those are
    /// different questions, and the gap between them is where the worst finding
    /// of the audit lived: `(exists [(n Nat)] (and (<= 100 n) (<= n 200)))` —
    /// true, witness 150 — came back `Refuted/Exact` at one budget and
    /// `Supported/Exact` at another. An answer that changes when you think
    /// longer is not an answer, and both were stamped exact.
    ///
    /// Four values close it, and `lo` is finally used: it is the difference
    /// between "fails eventually" and "fails throughout".
    fn tail_verdict(
        &mut self,
        body: ObjectId,
        var: ObjectId,
        lo: Option<i64>,
        env: &GraphEnv,
        depth: u32,
    ) -> Tail {
        if depth > 64 {
            return Tail::Unknown;
        }
        let Some(node) = self.g.get(body).cloned() else { return Tail::Unknown };
        let zero = BigInt::from(0);
        match node {
            CoreNode::Apply { operator, operands } if operator == wk::LEQ => {
                let [a, b] = operands.as_slice() else { return Tail::Unknown };
                let (Some(mut diff), Some(rhs)) =
                    (self.lin_form(*a, var, env, 0), self.lin_form(*b, var, env, 0))
                else {
                    return Tail::Unknown;
                };
                for (k, c) in rhs.0 {
                    *diff.0.entry(k).or_insert_with(|| BigInt::from(0)) -= c;
                }
                diff.1 -= rhs.1;
                let coeff = diff.0.get(&var).cloned().unwrap_or_else(|| zero.clone());
                if !diff.0.iter().all(|(k, c)| *k == var || *c == zero) {
                    return Tail::Unknown;
                }
                // The claim, normalised: `coeff·n + k ≤ 0`.
                let k = diff.1;
                if coeff == zero {
                    // The variable cancelled, so it is a constant fact.
                    return if k <= zero { Tail::AllHold } else { Tail::NoneHold };
                }
                let Some(lo) = lo else {
                    // Every integer, and a non-zero coefficient makes
                    // `coeff·n + k` change sign — so both regions are non-empty
                    // and neither is the whole domain.
                    return Tail::Mixed;
                };
                let lo = BigInt::from(lo);
                if coeff > zero {
                    // Increasing in n: holds for n ≤ ⌊−k ÷ coeff⌋.
                    if div_floor(&-k, &coeff) < lo { Tail::NoneHold } else { Tail::Mixed }
                } else {
                    // Decreasing in n: holds for n ≥ ⌈−k ÷ coeff⌉.
                    if div_ceil(&-k, &coeff) <= lo { Tail::AllHold } else { Tail::Mixed }
                }
            }
            CoreNode::Apply { operator, operands }
                if operator == wk::AND || operator == wk::OR =>
            {
                let parts: Vec<Tail> = operands
                    .iter()
                    .map(|o| self.tail_verdict(*o, var, lo, env, depth + 1))
                    .collect();
                if operator == wk::AND {
                    conjoin_tails(&parts)
                } else {
                    conjoin_tails(&parts.iter().map(|t| t.negate()).collect::<Vec<_>>()).negate()
                }
            }
            CoreNode::Apply { operator, operands } if operator == wk::NOT => {
                let [inner] = operands.as_slice() else { return Tail::Unknown };
                self.tail_verdict(*inner, var, lo, env, depth + 1).negate()
            }
            _ => Tail::Unknown,
        }
    }

    /// `Σ cᵢ·vᵢ + k`, or `None` outside the linear fragment.
    #[allow(clippy::type_complexity)]
    fn lin_form(
        &mut self,
        t: ObjectId,
        var: ObjectId,
        env: &GraphEnv,
        depth: u32,
    ) -> Option<(BTreeMap<ObjectId, BigInt>, BigInt)> {
        // Cycles are legal expressions, and this walks operands without ever
        // consulting the budget — so `(+ x #1=(+ 1 #1#))` recursed until the
        // stack gave out. A crash is not one of the four compute statuses.
        if depth > 64 {
            return None;
        }
        let r = env.get(&t).copied().unwrap_or(t);
        if r == var {
            let mut m = BTreeMap::new();
            m.insert(var, BigInt::from(1));
            return Some((m, BigInt::from(0)));
        }
        match self.g.get(r).cloned() {
            Some(CoreNode::Literal(LiteralValue::Int(n))) => {
                Some((BTreeMap::new(), n))
            }
            Some(CoreNode::Apply { operator, operands }) => match (operator, operands.as_slice()) {
                (wk::ADD, [a, b]) => {
                    let (mut x, xk) = self.lin_form(*a, var, env, depth + 1)?;
                    let (y, yk) = self.lin_form(*b, var, env, depth + 1)?;
                    for (k, c) in y {
                        *x.entry(k).or_insert_with(|| BigInt::from(0)) += c;
                    }
                    Some((x, xk + yk))
                }
                (wk::NEG, [a]) => {
                    let (x, k) = self.lin_form(*a, var, env, depth + 1)?;
                    Some((x.into_iter().map(|(a, b)| (a, -b)).collect(), -k))
                }
                (wk::MUL, [a, b]) => {
                    let (x, xk) = self.lin_form(*a, var, env, depth + 1)?;
                    let (y, yk) = self.lin_form(*b, var, env, depth + 1)?;
                    // Linear only when one side is constant.
                    if x.is_empty() {
                        Some((y.into_iter().map(|(k, c)| (k, c * &xk)).collect(), xk * yk))
                    } else if y.is_empty() {
                        Some((x.into_iter().map(|(k, c)| (k, c * &yk)).collect(), xk * yk))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn is_relation_type(&mut self, d: ObjectId) -> bool {
        matches!(self.g.get(d), Some(CoreNode::Apply { operator, .. })
                 if *operator == wk::RELATION_TYPE)
    }

    /// Least fixpoint. Monotone definitions converge and are marked complete; a
    /// negative occurrence makes the operator non-monotone, so rather than
    /// compute a wrong fixpoint the relation stays incomplete and everything
    /// downstream degrades honestly.
    fn letrec(
        &mut self,
        node: ObjectId,
        vars: &[crate::object::Binding],
        bodies: &[ObjectId],
        env: &GraphEnv,
    ) -> EvaluationResult {
        let (Some(pred), Some(def), Some(scope)) =
            (vars.first(), bodies.first(), bodies.last())
        else {
            return self.unsupported(node);
        };
        let params: Vec<&crate::object::Binding> = vars[1..].iter().collect();
        if params.is_empty() {
            return self.unsupported(node);
        }
        let mut domains = Vec::new();
        for p in &params {
            let Some(d) = p.domain else { return self.unsupported(node) };
            let Some(m) = self.members_of(d, env) else { return self.stalled(node) };
            // A partial parameter domain means the fixpoint ranges over a
            // subset, so it can never be declared complete.
            if !m.complete {
                return self.stalled(node);
            }
            domains.push((p.var, m.members));
        }
        let candidates = product(&domains);

        let mut rel = BoundRelation { tuples: Default::default(), complete: false };
        let monotone = !self.occurs_negatively(*def, pred.var, false, 0);
        loop {
            if self.exhausted() {
                break;
            }
            let mut next = rel.tuples.clone();
            // A candidate the definition neither established nor refuted is a
            // tuple whose membership is *undetermined*, not one that is out. A
            // `Partial` body — a default — hits exactly this, and publishing the
            // relation `complete` afterwards turned that undetermined tuple's
            // absence into a refutation.
            let mut undetermined = false;
            for cand in &candidates {
                if self.exhausted() {
                    break;
                }
                let mut scoped = env.clone();
                for (v, t) in cand {
                    scoped.vars.insert(*v, *t);
                }
                scoped.rels.insert(
                    pred.var,
                    BoundRelation { tuples: rel.tuples.clone(), complete: monotone },
                );
                let r = self.check(*def, &scoped);
                let contested = r.support.is_certain() && r.refutation.is_certain();
                if r.support.is_certain() && !contested {
                    next.insert(params.iter().map(|p| cand[&p.var]).collect());
                } else if contested || !r.refutation.is_certain() {
                    // A contested tuple is undetermined, not a member — and a
                    // relation with an undetermined tuple cannot be published
                    // complete, or its absence becomes a refutation.
                    undetermined = true;
                }
            }
            if next == rel.tuples {
                rel.complete = monotone && !undetermined;
                break;
            }
            rel.tuples = next;
        }
        let scoped = env.with_rel(pred.var, rel);
        let r = self.check(*scope, &scoped);
        self.seal(r)
    }

    /// Does the recursive relation `p` occur in a negative position of `f`?
    ///
    /// Recognising only `not` was unsound. The antecedent of `implies` is a
    /// negative position too, so `p(a) ↔ ¬p(a)` written as
    /// `(implies (p x) #false)` was judged **monotone**, the relation was
    /// published `complete`, absence became refutation, and a definition with no
    /// fixpoint at all returned `Supported/Exact`. Spelled with `not`, the same
    /// definition was correctly `Open/Stalled`.
    ///
    /// Operators whose monotonicity is not established are treated as negative
    /// whenever `p` occurs beneath them. That loses completeness — some of them
    /// are monotone — and losing completeness is sound, where guessing is not.
    fn occurs_negatively(&mut self, f: ObjectId, p: ObjectId, neg: bool, depth: u32) -> bool {
        if depth > 64 {
            return true;
        }
        match self.g.get(f).cloned() {
            Some(CoreNode::Apply { operator, operands }) => {
                if operator == p {
                    return neg;
                }
                // Contexts through which polarity is known to propagate
                // unchanged.
                let monotone = matches!(
                    operator,
                    wk::AND
                        | wk::OR
                        | wk::AT
                        | wk::IN_WORLD
                        | wk::ALWAYS
                        | wk::EVENTUALLY
                        | wk::NECESSARILY
                        | wk::POSSIBLY
                        | wk::USUALLY
                        | wk::HOLDS
                );
                if operator != wk::NOT && operator != wk::IMPLIES && !monotone {
                    return operands.iter().any(|o| self.mentions(*o, p, depth + 1));
                }
                operands.iter().enumerate().any(|(i, o)| {
                    // `not` flips everything under it; `implies` flips only its
                    // antecedent.
                    let flip = operator == wk::NOT || (operator == wk::IMPLIES && i == 0);
                    self.occurs_negatively(*o, p, neg ^ flip, depth + 1)
                })
            }
            Some(CoreNode::Bind { bodies, .. }) => {
                bodies.iter().any(|b| self.occurs_negatively(*b, p, neg, depth + 1))
            }
            _ => false,
        }
    }

    /// Does `p` appear anywhere beneath `f`? Used where polarity cannot be
    /// tracked, so any occurrence at all has to count against monotonicity.
    fn mentions(&mut self, f: ObjectId, p: ObjectId, depth: u32) -> bool {
        if depth > 64 {
            return true;
        }
        match self.g.get(f).cloned() {
            Some(CoreNode::Apply { operator, operands }) => {
                operator == p || operands.iter().any(|o| self.mentions(*o, p, depth + 1))
            }
            Some(CoreNode::Bind { bodies, .. }) => {
                bodies.iter().any(|b| self.mentions(*b, p, depth + 1))
            }
            _ => false,
        }
    }

    /// Quantification over relations, with the soundness discipline that cost a
    /// confident wrong answer once already: exhausting the candidate space is a
    /// verdict **only** when that space really was the whole powerset.
    fn second_order(
        &mut self,
        node: ObjectId,
        universal: bool,
        v: &crate::object::Binding,
        bodies: &[ObjectId],
        env: &GraphEnv,
        rel_ty: ObjectId,
    ) -> EvaluationResult {
        let Some(body) = bodies.first() else { return self.unsupported(node) };
        let Some(CoreNode::Apply { operands, .. }) = self.g.get(rel_ty).cloned() else {
            return self.unsupported(node);
        };
        let mut per_position = Vec::new();
        let mut exhaustive = true;
        for d in &operands {
            match self.members_of(*d, env) {
                Some(m) => {
                    // A partial position is the same situation as an infinite
                    // one for exhaustion purposes: absence of a witness among
                    // the members we saw proves nothing about the rest.
                    exhaustive &= m.complete;
                    per_position.push(m.members);
                }
                None => {
                    // Infinite position: the powerset is uncountable, so
                    // exhaustion is unreachable and cannot yield a verdict.
                    exhaustive = false;
                    per_position.push(self.ground_points(*body));
                }
            }
        }
        let tuples = tuples_of(&per_position);
        if tuples.len() > 20 {
            // Too wide to walk within any sane budget.
            return self.stalled(node);
        }
        let mut mask = vec![false; tuples.len()];
        let mut scan = Scan::new(universal);
        loop {
            if self.exhausted() {
                return self.out_of_budget(node);
            }
            let rel = BoundRelation {
                tuples: (0..tuples.len())
                    .filter(|i| mask[*i])
                    .map(|i| tuples[i].clone())
                    .collect(),
                complete: true,
            };
            let scoped = env.with_rel(v.var, rel);
            let r = self.check(*body, &scoped);
            if scan.step(&r, self.snapshot) {
                return scan.decided(!universal, self.snapshot);
            }
            if !increment(&mut mask) {
                break;
            }
        }
        self.finish_scan(node, &scan, exhaustive)
    }

    /// Ground points a body names directly — the only members of an infinite
    /// domain a finite formula can distinguish.
    fn ground_points(&mut self, body: ObjectId) -> Vec<ObjectId> {
        let mut out = Vec::new();
        for id in self.g.reachable(body) {
            if matches!(
                self.g.get(id),
                Some(CoreNode::Literal(_)) | Some(CoreNode::Atom { name: Some(_) })
            ) && !out.contains(&id)
            {
                out.push(id);
            }
        }
        out.truncate(4);
        out
    }

    /// Turn an accumulated [`Scan`] into a result.
    ///
    /// A scan that came out one-sided and exact is the ordinary definite
    /// answer. Anything else keeps both bounds, which is the whole point of
    /// accumulating them.
    fn finish_scan(&self, node: ObjectId, scan: &Scan, complete: bool) -> EvaluationResult {
        let (support, refutation, status) = scan.finish(complete);
        let carry = |mut r: EvaluationResult| {
            r.grounding = scan.grounding;
            r.derivation = scan.derivation;
            r.determinacy = scan.determinacy;
            r.defeated_by = scan.defeated_by.clone();
            r.credence = scan.credence;
            r
        };
        // Collapse to a definite answer only when the *other* side is empty.
        // Testing `is_certain() != is_certain()` reads `(Partial, Certain)` as
        // one-sided and republishes it as `(None, Certain)` — throwing away
        // defeasible support and manufacturing a definite `Refuted`, which is
        // the round-two `(and Conflicted #true)` failure reachable again the
        // moment one operand is a default instead of a truth atom. `Scan` folds
        // correctly; this line was undoing it.
        if status == ComputeStatus::Exact {
            if support.is_certain() && refutation == Bound::None {
                return carry(EvaluationResult::certain(true));
            }
            if refutation.is_certain() && support == Bound::None {
                return carry(EvaluationResult::certain(false));
            }
        }
        carry(self.combine(node, support, refutation, status, scan.residuals.clone()))
    }

    fn combine(
        &self,
        node: ObjectId,
        support: Bound,
        refutation: Bound,
        status: ComputeStatus,
        residuals: Vec<ObjectId>,
    ) -> EvaluationResult {
        let mut r = EvaluationResult::new(status);
        r.support = support;
        r.refutation = refutation;
        r.residual = Some(node);
        r.dependencies = residuals;
        r.snapshot = self.snapshot;
        r
    }

    fn is_lambda(&mut self, id: ObjectId) -> bool {
        matches!(self.g.get(id), Some(CoreNode::Bind { binder, .. }) if *binder == wk::LAMBDA)
    }

    /// Members of a domain expression.
    ///
    /// `(set a b c)` enumerates literally and `(where D p)` refines by a
    /// predicate, so union, difference and carved-out subsets are ordinary
    /// expressions rather than enum variants. Anything else is asked of the
    /// structure.
    fn members_of(&mut self, domain: ObjectId, env: &GraphEnv) -> Option<Extension> {
        // `(where D f)` recurses on its base, so `D = (where D f)` never
        // returned.
        if self.read_depth > 64 {
            return None;
        }
        self.read_depth += 1;
        let out = self.members_of_guarded(domain, env);
        self.read_depth -= 1;
        out
    }

    fn members_of_guarded(&mut self, domain: ObjectId, env: &GraphEnv) -> Option<Extension> {
        // A domain can be a *term*: `(tests-of m)` under a quantifier over
        // modules. The raw id was handed to `extension`, so the store was asked
        // to enumerate `(tests-of <skolem>)` and honestly knew nothing —
        // a silent stall on the most natural encoding of a per-group query.
        let domain = self.substitute(domain, env, 0);
        match self.g.get(domain).cloned() {
            // A literal enumeration is exhaustive by construction.
            Some(CoreNode::Apply { operator, operands }) if operator == wk::SET_DOMAIN => {
                Some(Extension::complete(operands))
            }
            // …and the same enumeration without that promise, which is how a
            // suspended scan over a partial index says what it still is.
            Some(CoreNode::Apply { operator, operands }) if operator == wk::SET_PARTIAL => {
                Some(Extension::partial(operands))
            }
            // `(defeaters P)` — what would overturn P, as a domain. The binary
            // `defeated-by` can only test a candidate already in hand; this is
            // what makes the set enumerable and countable, so
            // `(count (defeaters P))` and "is any defeater one I believe" are
            // ordinary questions rather than a Rust field nobody can reach.
            //
            // Complete exactly when the evaluation finished. A defeater set read
            // off a truncated run is a floor, not a list, and publishing it as
            // complete would let a universal over it be *supported* by the
            // defeaters the budget happened to reach.
            Some(CoreNode::Apply { operator, operands }) if operator == wk::DEFEATERS_DOMAIN => {
                let [p] = operands.as_slice() else { return None };
                let r = self.about(*p, env);
                // Truncation blocks, absence does not — the same rule the axis
                // operators use, and stated once so the three sites cannot
                // drift. Only a run that stopped early can be hiding a defeater
                // it never reached; a stalled one visited every branch. The
                // `Open` test does the real work: with no conclusion there is
                // nothing to defeat, so the set is not merely empty but
                // undefined, and calling that a complete enumeration of zero
                // would license "nothing would change my mind".
                let truncated = matches!(
                    r.compute_status,
                    ComputeStatus::BudgetExhausted | ComputeStatus::Unsupported
                );
                let complete = !truncated && r.evidential() != Evidential::Open;
                Some(Extension { members: r.defeated_by, complete })
            }
            // `(sort S)` is the members of a sort. Without this arm the whole
            // node — the `(sort S)` application, not `S` — was handed to
            // `extension()`, which keys on the bare sort, so a sort wrapped in
            // its own reserved constructor enumerated nowhere.
            Some(CoreNode::Apply { operator, operands }) if operator == wk::SORT_DOMAIN => {
                let [sort] = operands.as_slice() else { return None };
                // `(sort Prop)` is `Prop`, and the never-complete rule below
                // must not be escapable by spelling. A store's propositions are
                // never all of them.
                let ext = self.s.extension(*sort)?;
                Some(if *sort == wk::PROP_TYPE {
                    Extension::partial(ext.members)
                } else {
                    ext
                })
            }
            Some(CoreNode::Apply { operator, operands }) if operator == wk::WHERE_DOMAIN => {
                let [base, filter] = operands.as_slice() else { return None };
                let base_ext = self.members_of(*base, env)?;
                let mut out = Vec::new();
                // The refinement is exact only when the base was exhaustive
                // *and* every member was decided either way. A member merely
                // not-refuted is kept — over-approximating rather than dropping
                // it — and that guess must not be laundered into certainty.
                let mut exact = base_ext.complete;
                for m in base_ext.members {
                    if self.exhausted() {
                        return Some(Extension::partial(out));
                    }
                    let mut scoped = env.clone();
                    let opened = self.g.open_binder(*filter);
                    let filter_body = match &opened {
                        Some((_, vars, bodies)) if !bodies.is_empty() => {
                            if let Some(v) = vars.first() {
                                scoped.vars.insert(v.var, m);
                            }
                            bodies[0]
                        }
                        _ => *filter,
                    };
                    let r = self.check(filter_body, &scoped);
                    // A **contested** member has both bounds certain. Testing
                    // refutation alone silently excluded it *and* left the
                    // enumeration marked exhaustive, so one spelling of a
                    // question was vacuously and definitely true while the other
                    // was honestly `Conflicted`.
                    let contested = r.support.is_certain() && r.refutation.is_certain();
                    if contested {
                        out.push(m);
                        exact = false;
                    } else if !r.refutation.is_certain() {
                        out.push(m);
                        // Kept without being established: the set is a guess.
                        if !r.support.is_certain() {
                            exact = false;
                        }
                    }
                }
                Some(Extension { members: out, complete: exact })
            }
            // The structure's own instant set, as a domain. `instants()` was
            // reachable only through `always`/`eventually`/`since`, so the
            // reserved `instants` domain — which §4 lists — enumerated nothing
            // and a bounded temporal universal could not be written.
            // Quantifying over propositions, so that partial awareness —
            // "there is some property distinguishing these cases and I cannot
            // name it" — is a term rather than an aspiration. The candidates are
            // the propositions the *store* holds; a quantifier over every
            // proposition expressible in the language is not something any
            // structure can enumerate, and claiming otherwise would be the
            // fabricated-completeness bug in a new coat.
            _ if domain == wk::PROP_TYPE => {
                self.s.extension(wk::PROP_TYPE).map(|e| Extension {
                    members: e.members,
                    // Never complete: the store's propositions are not all of
                    // them, and an existential over an incomplete extension is
                    // exactly what `Extension::partial` is for.
                    complete: false,
                })
            }
            _ if domain == wk::INSTANTS_DOMAIN => {
                let ts = self.s.instants();
                if ts.is_empty() {
                    // A timeless structure has no instant to witness anything,
                    // which makes a temporal claim *unanswerable* — the same
                    // reading `always`/`eventually` already take. Reporting an
                    // empty *complete* domain instead made a universal vacuously
                    // true and, worse, refuted an existential: absence read as
                    // refutation, arriving through a domain rather than a
                    // resolver.
                    return None;
                }
                let members = ts.into_iter().map(|t| self.g.int(t)).collect();
                Some(Extension::complete(members))
            }
            _ => match self.instant {
                Some(t) => self.s.extension_at(domain, t),
                None => self.s.extension(domain),
            },
        }
    }

    /// The same quantifier over an explicit remaining set.
    ///
    /// Kept *quantified* rather than unrolled into a conjunction, so the
    /// continuation is no larger than the question that produced it and each
    /// pass strictly shrinks what is left. Unrolling would make re-asking
    /// slower than starting over, and the measure would climb instead of fall.
    /// The completeness of the source domain travels with it. Rebuilding a
    /// partial enumeration as a `set` — which `members_of` reads as exhaustive
    /// by construction — made the continuation a *stronger* question than the
    /// one it suspended, so suspending and resuming was a soundness upgrade.
    fn narrow(
        &mut self,
        binder: ObjectId,
        v: &crate::object::Binding,
        rest: &[ObjectId],
        body: ObjectId,
        complete: bool,
    ) -> ObjectId {
        let op = if complete { wk::SET_DOMAIN } else { wk::SET_PARTIAL };
        let remaining = self.g.apply(op, rest.to_vec());
        self.g.quantify(binder, v.var, Some(remaining), body)
    }

    /// Resolve `id` to a term whose **object identity is its meaning**.
    ///
    /// Only atoms, literals and external references qualify. For those,
    /// distinct ids really do denote distinct things, so comparing ids decides
    /// equality and `=` may refute — which is what makes `where`-based
    /// exception sets ("every file except `mod.rs`") work.
    ///
    /// A compound does not qualify. `(build-minutes artist)` and `4` are
    /// different ids and may still denote the same thing; comparing the ids
    /// compares the *expressions*. This match previously had identical arms —
    /// `_ => Some(r)` — so it restricted nothing, and every uncomputable term
    /// under `=` came back `Refuted/Exact`, inverting to a confident
    /// `Supported` under a negation. That is the same fabricated-verdict
    /// failure `Knowledge::Fails` was introduced to stop at resolvers, arriving
    /// by another route.
    ///
    /// See [`State::key`] for the *other* question this used to answer, and
    /// should not have.
    fn denote(&mut self, id: ObjectId, env: &GraphEnv) -> Option<ObjectId> {
        // Guarded here rather than only in `project`, because the recursion is
        // `denote → project → denote`: `project` increments and decrements
        // within one hop, so the counter returned to zero every time round and
        // bounded nothing. A guard on a callee does not bound a cycle that
        // passes through its caller.
        if self.read_depth > 64 {
            return None;
        }
        self.read_depth += 1;
        let out = self.denote_guarded(id, env);
        self.read_depth -= 1;
        out
    }

    fn denote_guarded(&mut self, id: ObjectId, env: &GraphEnv) -> Option<ObjectId> {
        let r = self.substitute(id, env, 0);
        // A projection out of a literal sequence *is* computable, so it denotes
        // in exactly the way `(+ 2 2)` does — the compound rule below is about
        // terms whose value the evaluator cannot reach, not about every
        // compound.
        if let Some(v) = self.project(r, env) {
            return self.denote(v, env);
        }
        match self.g.get(r) {
            // A *composed* expression is not ground *by itself* — but the
            // structure may say what it denotes, and then it is. Without this,
            // `value` was wired into `numeric`, `quantity` and `sequence` only,
            // so a functional term could name a number, a duration or a
            // sequence and never an entity: "the author of this commit is Adam"
            // stalled while "the line count is 40" did not.
            Some(CoreNode::Apply { .. }) | Some(CoreNode::Bind { .. }) => {
                if self.value_depth >= 4 {
                    return None;
                }
                let v = self.denotation(r).filter(|v| *v != r)?;
                self.value_depth += 1;
                let out = self.denote(v, env);
                self.value_depth -= 1;
                out
            }
            // Everything else is its own meaning — atoms, literals, externals,
            // opaque nodes, and an id **absent from this graph**, which is how a
            // lifted entity from the relational store arrives.
            //
            // Unless the structure says otherwise. Comparing ids without asking
            // made `(= a b)` a definite `Refuted` while the store held
            // `value(a) = b`, which is a fabricated verdict about an identity
            // the store had actually recorded.
            _ => {
                if self.value_depth < 4
                    && let Some(v) = self.denotation(r).filter(|v| *v != r)
                {
                    self.value_depth += 1;
                    let out = self.denote(v, env);
                    self.value_depth -= 1;
                    return out;
                }
                Some(r)
            }
        }
    }

    /// Resolve `id` to a **lookup key**.
    ///
    /// This is a different question from [`State::denote`], and collapsing the
    /// two into one function cost the formalism most of its expressiveness.
    /// Denotational grounding asks *"is this term's id its meaning?"* — the
    /// right question for `=` and arithmetic, and the reason a compound must be
    /// refused there. A relational lookup asks *"does the store hold this exact
    /// tuple?"*, and for that a compound's content id is a perfectly good key.
    ///
    /// Applying the strict answer to both meant **no user predicate could take
    /// a compound argument at all**:
    ///
    /// ```text
    /// (causes (quote (omits rocksdb-slice.h cstdint))
    ///         (quote (build-fails mnestic-rocks-0.1.10)))   →  Open / Stalled
    /// ```
    ///
    /// — while the seven intensional operators, which bypassed `ground`
    /// entirely, answered the same shape on the same store. Recording *why*
    /// something happened is not an exotic requirement for a coding agent's
    /// memory; it is most of what it learns.
    ///
    /// Lookup on a compound argument is **intensional**: the key is the
    /// expression, so `same-as` does not rewrite through it. That is the same
    /// barrier §7 already requires of `believes` and `asserted-by`, and it is
    /// what makes keying by id sound here.
    fn key(&mut self, id: ObjectId, env: &GraphEnv) -> ObjectId {
        self.substitute(id, env, 0)
    }

    /// Replace bound variables **throughout** a term, not just at its root.
    ///
    /// `key` was `env.get(&id).unwrap_or(id)` — a single lookup at the argument
    /// boundary. A quantifier binds `v0 → cstdint` in the environment, but the
    /// argument node is `(omits rocksdb-slice.h v0)`, whose *own* id is not an
    /// environment key, so the store was asked for a tuple still containing the
    /// variable and matched nothing. Compound arguments therefore worked only
    /// while they were ground: the moment one was generalised — which is what
    /// quantifiers, rules and aggregates are *for* — it stopped evaluating.
    ///
    /// This is variable substitution, not coreference rewriting, so it happens
    /// inside quotations too. §7's barrier is about `same-as` rewriting one name
    /// into another; binding a quantified variable is a different operation and
    /// blocking it made "for every x, someone believes something about x"
    /// unaskable for no reason.
    fn substitute(&mut self, id: ObjectId, env: &GraphEnv, depth: u32) -> ObjectId {
        if let Some(v) = env.get(&id) {
            return *v;
        }
        if depth > 64 || env.vars.is_empty() {
            return id;
        }
        match self.g.get(id).cloned() {
            Some(CoreNode::Apply { operator, operands }) => {
                let op = self.substitute(operator, env, depth + 1);
                let args: Vec<ObjectId> =
                    operands.iter().map(|o| self.substitute(*o, env, depth + 1)).collect();
                if op == operator && args == operands {
                    return id;
                }
                self.g.apply(op, args)
            }
            // A binder's own slots are de Bruijn and shadow nothing in `env`,
            // but its *body* can still mention an outer nominal variable — so
            // returning it untouched left the whole family "a norm, belief or
            // attribution whose content is itself general, stated per entity"
            // storable, indexable and inert. Rebuilding is safe precisely
            // because the slots carry no names to capture.
            Some(CoreNode::Bind { binder, vars, bodies }) => {
                let new_bodies: Vec<ObjectId> =
                    bodies.iter().map(|b| self.substitute(*b, env, depth + 1)).collect();
                if new_bodies == bodies {
                    return id;
                }
                self.g.intern(CoreNode::Bind { binder, vars, bodies: new_bodies })
            }
            _ => id,
        }
    }

    /// A term's denotation, indexed by whichever axes are in force.
    fn denotation(&mut self, r: ObjectId) -> Option<ObjectId> {
        match (self.instant, self.world) {
            (Some(t), _) => self.s.value_at(r, t),
            (None, Some(w)) => self.s.value_in(r, w),
            (None, None) => self.s.value(r),
        }
    }

    /// What the structure knows, indexed by whichever axes are in force.
    ///
    /// Only the user-predicate arm consulted `known_at`/`known_in`; every
    /// reserved relation — `same-as`, `prefer`, `usually`, the norms, the seven
    /// intensional relations, `before`'s stored-order fallback — called `known`
    /// directly and discarded the instant and the world. So no preference, norm,
    /// default, coreference or attribution could be dated or made
    /// context-local: "Adam preferred spaces until March", "on CI `--release` is
    /// forbidden" had no encoding, while §5.2 said `(at t P)` evaluates `P`
    /// against the structure as of `t`.
    fn lookup(&self, op: ObjectId, tuple: &[ObjectId]) -> Knowledge {
        match (self.instant, self.world) {
            (Some(t), Some(w)) => self.s.known_at_in(op, tuple, t, w),
            (Some(t), None) => self.s.known_at(op, tuple, t),
            (None, Some(w)) => self.s.known_in(op, tuple, w),
            (None, None) => self.s.known(op, tuple),
        }
    }

    /// `(nth (seq a b c) 1)` → `b`. `None` when this is not a projection, or
    /// when the index is out of range or not computable.
    fn project(&mut self, id: ObjectId, env: &GraphEnv) -> Option<ObjectId> {
        // `project` and `denote` are mutually recursive, so `S = (seq N)`,
        // `N = (nth S 0)` walked them until the stack gave out.
        if self.read_depth > 64 {
            return None;
        }
        self.read_depth += 1;
        let out = self.project_guarded(id, env);
        self.read_depth -= 1;
        out
    }

    fn project_guarded(&mut self, id: ObjectId, env: &GraphEnv) -> Option<ObjectId> {
        let Some(CoreNode::Apply { operator, operands }) = self.g.get(id).cloned() else {
            return None;
        };
        if operator != wk::NTH {
            return None;
        }
        let [s, i] = operands.as_slice() else { return None };
        let seq = self.sequence(*s, env)?;
        let idx = self.numeric(*i, env)?.to_int()?.to_usize()?;
        seq.get(idx).copied()
    }

    /// The elements of a literal sequence.
    fn sequence(&mut self, id: ObjectId, env: &GraphEnv) -> Option<Vec<ObjectId>> {
        let r = self.substitute(id, env, 0);
        match self.g.get(r).cloned() {
            Some(CoreNode::Apply { operator, operands }) if operator == wk::SEQ => Some(operands),
            _ => {
                // …or a term the structure denotes as one.
                if self.value_depth >= 4 {
                    return None;
                }
                let v = self.denotation(r).filter(|v| *v != r)?;
                self.value_depth += 1;
                let out = self.sequence(v, env);
                self.value_depth -= 1;
                out
            }
        }
    }

    /// Reduce `(quantity n unit)` to `(magnitude, base-unit)` by chasing
    /// `scale` facts to a common base.
    ///
    /// The unit vocabulary lives entirely in the store: `(scale minutes 60
    /// seconds)` is an ordinary fact, so `(quantity 4 furlongs)` starts working
    /// the moment someone writes `(scale furlongs 201168 mm)`. Nothing here
    /// knows what a minute is.
    ///
    /// Without this a duration could be stored and never compared. The three
    /// available encodings were all lossy: `(build-minutes artist 4)` smuggles
    /// the unit into the predicate name, which §9 forbids and which makes
    /// minutes and seconds unrelated words; `(duration build 4 minutes)` leaves
    /// the magnitude and the unit as unrelated arguments; and the honest
    /// `(= (duration build) (quantity 4 minutes))` used to be *Refuted*.
    fn quantity(&mut self, id: ObjectId, env: &GraphEnv) -> Option<(Num, ObjectId)> {
        // `(+ q1 q2)` recurses on operands, so a cyclic term walked it until the
        // stack gave out — and the *operand order* decided whether it did, since
        // `collect::<Option<_>>` short-circuits on the first `None`. `(+ 1 T)`
        // returned early and looked safe; `(+ T 1)` aborted the process. A test
        // written with the safe order is a test that proves nothing.
        if self.read_depth > 64 {
            return None;
        }
        self.read_depth += 1;
        let out = self.quantity_guarded(id, env);
        self.read_depth -= 1;
        out
    }

    fn quantity_guarded(&mut self, id: ObjectId, env: &GraphEnv) -> Option<(Num, ObjectId)> {
        let r = self.substitute(id, env, 0);
        let denoted = match self.g.get(r).cloned() {
            Some(CoreNode::Apply { operator, .. }) if operator == wk::QUANTITY => None,
            // A term the structure denotes as a quantity — "the duration of the
            // test suite" — reads as one.
            _ if self.value_depth < 4 => self.denotation(r).filter(|v| *v != r),
            _ => None,
        };
        if let Some(v) = denoted {
            self.value_depth += 1;
            let out = self.quantity(v, env);
            self.value_depth -= 1;
            return out;
        }
        let Some(CoreNode::Apply { operator, operands }) = self.g.get(r).cloned() else {
            return None;
        };
        if operator == wk::ADD || operator == wk::NEG {
            // Quantities could be *compared* and never combined, so no total
            // over durations, sizes or costs was reachable — and
            // `aggregate_bounds` reads its `sum` values through `numeric`, so a
            // `sum` of quantities was out too. Normalise each operand to its
            // base and combine there.
            let parts: Option<Vec<(Num, ObjectId)>> =
                operands.iter().map(|o| self.quantity(*o, env)).collect();
            let parts = parts?;
            let (first, base) = parts.first().cloned()?;
            if !parts.iter().all(|(_, b)| *b == base) {
                // No conversion known between the bases: nothing is decided.
                return None;
            }
            return match (operator, parts.as_slice()) {
                (wk::NEG, [(a, _)]) => Some((a.neg(), base)),
                (wk::NEG, [(a, _), (b, _)]) => Some((a.sub(b)?, base)),
                (wk::ADD, rest) => {
                    let mut acc = first;
                    for (v, _) in rest.iter().skip(1) {
                        acc = acc.add(v)?;
                    }
                    Some((acc, base))
                }
                _ => None,
            };
        }
        if operator != wk::QUANTITY {
            return None;
        }
        let [amount, unit] = operands.as_slice() else { return None };
        let mut value = self.numeric(*amount, env)?;
        let mut unit = self.denote(*unit, env)?;

        // Walk to the base unit. Bounded so a cyclic `scale` chain — which a
        // store can perfectly well contain — cannot spin forever.
        for _ in 0..32 {
            let Some(next) = self.scale_of(unit) else { break };
            let (factor, base) = next;
            value = value.mul(&factor)?;
            if base == unit {
                break;
            }
            unit = base;
        }
        Some((value, unit))
    }

    /// One `(scale unit factor base)` step, if the store holds it.
    ///
    /// The decision was that units are **data**: `(scale minutes 60 seconds)` is
    /// an ordinary stored fact, so a new unit costs a write and never a
    /// migration. It was not implemented that way. `wk::SCALE` was matched
    /// nowhere in the evaluator and this called a bespoke Rust trait method that
    /// no production structure implemented — so a new unit cost a *recompile*,
    /// and the doc promising `(quantity 4 furlongs)` would start working "the
    /// moment someone writes `(scale furlongs 201168 mm)`" was false as written.
    ///
    /// The evaluator side was never the problem — this hook is exactly right,
    /// and `(scale u n b)` needs a partial-tuple lookup that `known` cannot
    /// express, so a dedicated hook is the honest interface. What was missing is
    /// that **no structure implemented it**: `MapGraphStructure` and
    /// `RelationalView` both inherited the empty default, so writing the fact
    /// changed nothing. One generic implementation over the fact table, in each,
    /// is what makes a new unit cost a write.
    fn scale_of(&mut self, unit: ObjectId) -> Option<(Num, ObjectId)> {
        if let Some((factor, base)) = self.s.scale(unit) {
            return Some((Num::int(factor), base));
        }
        None
    }

    /// Resolve `id` to an instant: an integer literal, or a symbolic term the
    /// structure can denote.
    fn instant_of(&mut self, id: ObjectId, env: &GraphEnv) -> Option<i64> {
        if let Some(n) = self.number(id, env) {
            return n.to_i64();
        }
        let r = self.substitute(id, env, 0);
        if let Some(t) = self.s.denote_instant(r) {
            return Some(t);
        }
        // …or the general denotation hook may resolve it to something numeric.
        if self.value_depth >= 4 {
            return None;
        }
        let v = self.denotation(r).filter(|v| *v != r)?;
        self.value_depth += 1;
        let out = self.instant_of(v, env);
        self.value_depth -= 1;
        out
    }

    /// Numeric bounds on `id`: `(lower, upper)`, either side `None` when
    /// unbounded in that direction.
    ///
    /// An exact value is the degenerate interval `[n, n]`. An aggregate over a
    /// partially known domain has a real lower bound and often no upper one,
    /// and that is enough to decide many comparisons.
    ///
    /// **The upper bound is only a bound when the scan was exhaustive.** Over an
    /// incomplete domain, `hi` totals the members that were *seen*; unseen ones
    /// may contribute more, so it bounds nothing. `number()` has always checked
    /// `exact`, and this forwarded `hi` without it — so the laundering the `=`
    /// path was repaired for simply arrived through `<=` instead, and
    /// `(<= (count …) 3)` came back `Supported/Exact` over a domain the resolver
    /// never claimed to have enumerated.
    fn interval(&mut self, id: ObjectId, env: &GraphEnv) -> (Option<Num>, Option<Num>) {
        if let Some(n) = self.numeric(id, env) {
            return (Some(n.clone()), Some(n));
        }
        let r = self.substitute(id, env, 0);
        if let Some(CoreNode::Bind { binder, .. }) = self.g.get(r).cloned()
            && (binder == wk::COUNT || binder == wk::SUM)
            && let Some((_, vars, bodies)) = self.g.open_binder(r)
            && let Some(b) = self.aggregate_bounds(binder, &vars, &bodies, env)
        {
            // `hi` needs a complete enumeration because unseen members can add
            // to it. `lo` needs one too whenever a contribution can be
            // *negative*, which is `sum` and not `count` — every `count`
            // contribution is `+1`, so what was seen really is a floor. Without
            // the distinction, `(<= 0 (sum …))` over a partial enumeration came
            // back `Supported/Exact` and completing the enumeration flipped it.
            let lo_bounded = b.complete || binder == wk::COUNT;
            return (
                if lo_bounded { Some(b.lo) } else { None },
                if b.complete { b.hi } else { None },
            );
        }
        (None, None)
    }

    /// Exact numeric value of `id`, over the rationals.
    ///
    /// This is where the arithmetic gaps closed. `+` and `*` were binary-only
    /// despite unbounded arity, `-` existed only as unary negation, and `mod`,
    /// `len` and `substr` were declared in `wk::` but uninterpreted — so each
    /// of them came back **Refuted** under `=` rather than stalling.
    fn numeric(&mut self, id: ObjectId, env: &GraphEnv) -> Option<Num> {
        if self.read_depth > 64 {
            return None;
        }
        self.read_depth += 1;
        let out = self.numeric_guarded(id, env);
        self.read_depth -= 1;
        out
    }

    fn numeric_guarded(&mut self, id: ObjectId, env: &GraphEnv) -> Option<Num> {
        let r = self.substitute(id, env, 0);
        self.numeric_direct(r, env).or_else(|| {
            // Not readable as it stands — but the structure may *denote* it.
            // This is what makes `(= (duration test-suite) (quantity 4 minutes))`
            // an answerable question rather than a permanently stalled one.
            // Bounded, because a store can perfectly well hold a denotation
            // cycle and nothing stops it.
            if self.value_depth >= 4 {
                return None;
            }
            let v = self.denotation(r)?;
            if v == r {
                return None;
            }
            self.value_depth += 1;
            let out = self.numeric(v, env);
            self.value_depth -= 1;
            out
        })
    }

    fn numeric_direct(&mut self, r: ObjectId, env: &GraphEnv) -> Option<Num> {
        match self.g.get(r).cloned() {
            Some(CoreNode::Literal(LiteralValue::Int(n))) => Some(Num::int(n)),
            Some(CoreNode::Literal(LiteralValue::Decimal { mantissa, scale })) => {
                Num::decimal(mantissa, scale)
            }
            Some(CoreNode::Apply { operator, operands }) => {
                // `len` takes text, not numbers, so it is handled before the
                // operands are evaluated numerically.
                if operator == wk::LEN {
                    let [t] = operands.as_slice() else { return None };
                    // Length of a sequence or of a text, whichever it is.
                    if let Some(items) = self.sequence(*t, env) {
                        return Some(Num::int(BigInt::from(items.len())));
                    }
                    let s = self.text(*t, env)?;
                    return Some(Num::int(BigInt::from(s.chars().count())));
                }
                // A projection reduces before anything else looks at it.
                if operator == wk::NTH
                    && let Some(v) = self.project(r, env)
                {
                    return self.numeric(v, env);
                }
                let vals: Option<Vec<Num>> =
                    operands.iter().map(|o| self.numeric(*o, env)).collect();
                let v = vals?;
                match (operator, v.as_slice()) {
                    // n-ary, matching the unbounded arity the kernel advertises.
                    (wk::ADD, []) => Some(Num::int(BigInt::from(0))),
                    (wk::ADD, rest) => {
                        rest.iter().try_fold(Num::int(BigInt::from(0)), |a, b| a.add(b))
                    }
                    (wk::MUL, []) => Some(Num::int(BigInt::from(1))),
                    (wk::MUL, rest) => {
                        rest.iter().try_fold(Num::int(BigInt::from(1)), |a, b| a.mul(b))
                    }
                    // `-` is negation with one operand and subtraction with two,
                    // which is what every reader of the printed form expects.
                    (wk::NEG, [a]) => Some(a.neg()),
                    (wk::NEG, [a, b]) => a.sub(b),
                    (wk::DIV, [a, b]) => a.div(b),
                    (wk::MOD, [a, b]) => {
                        let (x, y) = (a.to_int()?, b.to_int()?);
                        (y != BigInt::from(0)).then(|| Num::int(x % y))
                    }
                    _ => None,
                }
            }
            _ => self.number(r, env).map(Num::int),
        }
    }

    fn number(&mut self, id: ObjectId, env: &GraphEnv) -> Option<BigInt> {
        // The integer reader recurses on operands too, and `instant_of` reaches
        // it — so `(before T sym)` over a cyclic `T` aborted the process even
        // after `numeric` was guarded. Guarding four of the five readers is
        // guarding none of them.
        if self.read_depth > 64 {
            return None;
        }
        self.read_depth += 1;
        let out = self.number_guarded(id, env);
        self.read_depth -= 1;
        out
    }

    fn number_guarded(&mut self, id: ObjectId, env: &GraphEnv) -> Option<BigInt> {
        let r = self.substitute(id, env, 0);
        match self.g.get(r).cloned() {
            Some(CoreNode::Literal(LiteralValue::Int(n))) => Some(n.clone()),
            Some(CoreNode::Apply { operator, operands }) => {
                let vals: Option<Vec<BigInt>> =
                    operands.iter().map(|o| self.number(*o, env)).collect();
                let v = vals?;
                match (operator, v.as_slice()) {
                    (wk::ADD, [a, b]) => Some(a + b),
                    (wk::MUL, [a, b]) => Some(a * b),
                    (wk::NEG, [a]) => Some(-a),
                    (wk::DIV, [a, b]) if b.to_i64() != Some(0) => Some(a / b),
                    _ => None,
                }
            }
            Some(CoreNode::Bind { binder, .. }) if binder == wk::COUNT || binder == wk::SUM => {
                let (_, vars, bodies) = self.g.open_binder(id)?;
                let b = self.aggregate_bounds(binder, &vars, &bodies, env)?;
                // Only an exact aggregate is a *value*. A partial one is still
                // a usable bound, which `LEQ` now genuinely reads — the old
                // comment said so while `number()` returned `None` and every
                // comparison over a partial aggregate stalled.
                match (b.exact, b.hi) {
                    (true, Some(hi)) if b.lo == hi => b.lo.to_int(),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Two-sided bounds for `count` / `sum`: `lo` counts what is proven, `hi`
    /// adds what is not yet ruled out. When they meet the aggregate is exact,
    /// which is what lets a comparison decide before the scan finishes.
    fn aggregate_bounds(
        &mut self,
        binder: ObjectId,
        vars: &[crate::object::Binding],
        bodies: &[ObjectId],
        env: &GraphEnv,
    ) -> Option<AggregateBounds> {
        let v = vars.first()?;
        let body = *bodies.first()?;
        let value = bodies.get(1).copied();
        // An aggregate over several slots ranges over the product, and reading
        // only the first left the others free — so `(count [(x M) (y M)] …)`
        // counted zero over a store with four pairs. Refuse rather than
        // undercount: an aggregate is a *value*, so there is no honest partial
        // answer to give, and `None` here stalls the comparison that wanted it.
        if vars.len() > 1 {
            return None;
        }
        let ext = self.members_of(v.domain?, env)?;
        // An incomplete *enumeration* bounds the total from below only: unseen
        // members may contribute more. Reporting `hi` as if the scan were
        // exhaustive is how "exactly 3 untested files" was `Supported/Exact` on
        // a store that knew nothing about testedness.
        let complete = ext.complete;
        let mut all_decided = true;
        let zero = Num::int(BigInt::from(0));
        let (mut lo, mut hi) = (zero.clone(), zero.clone());
        for m in ext.members {
            if self.exhausted() {
                return Some(AggregateBounds { lo, hi: None, complete: false, exact: false });
            }
            let scoped = env.with(v.var, m);
            // `numeric`, not `number`: an aggregate whose contributions are
            // integers is a special case, not the definition. Totalling
            // durations, coverage percentages or sizes was out of reach for no
            // reason other than the extractor.
            let contribution = match (binder == wk::SUM, value) {
                (true, Some(t)) => self.numeric(t, &scoped)?,
                _ => Num::int(BigInt::from(1)),
            };
            let r = self.check(body, &scoped);
            let contested = r.support.is_certain() && r.refutation.is_certain();
            if r.support.is_certain() && !contested {
                lo = lo.add(&contribution)?;
                hi = hi.add(&contribution)?;
            } else if contested || !r.refutation.is_certain() {
                // Undecided: it may or may not contribute. Which *end* of the
                // interval that widens depends on the sign — a negative
                // contribution to a `sum` lowers the floor rather than raising
                // the ceiling, and adding it to `hi` would have produced an
                // upper bound smaller than the true total.
                if contribution < zero {
                    lo = lo.add(&contribution)?;
                } else {
                    hi = hi.add(&contribution)?;
                }
                all_decided = false;
            }
        }
        Some(AggregateBounds {
            lo,
            hi: Some(hi),
            complete,
            exact: complete && all_decided,
        })
    }

    fn text(&mut self, id: ObjectId, env: &GraphEnv) -> Option<String> {
        if self.read_depth > 64 {
            return None;
        }
        self.read_depth += 1;
        let out = self.text_guarded(id, env);
        self.read_depth -= 1;
        out
    }

    fn text_guarded(&mut self, id: ObjectId, env: &GraphEnv) -> Option<String> {
        let r = self.substitute(id, env, 0);
        match self.g.get(r).cloned() {
            Some(CoreNode::Literal(LiteralValue::Text(s))) => Some(s.clone()),
            Some(CoreNode::Apply { operator, operands }) if operator == wk::CONCAT => {
                let parts: Option<Vec<String>> =
                    operands.iter().map(|o| self.text(*o, env)).collect();
                Some(parts?.concat())
            }
            Some(CoreNode::Apply { operator, operands }) if operator == wk::SUBSTR => {
                // `(substr s start len)` — character offsets, not bytes, so a
                // multi-byte character cannot be split in half.
                let [src, start, len] = operands.as_slice() else { return None };
                let s = self.text(*src, env)?;
                let from = self.number(*start, env)?.to_usize()?;
                let count = self.number(*len, env)?.to_usize()?;
                Some(s.chars().skip(from).take(count).collect())
            }
            // …or a term the structure denotes as text. Every other reader fell
            // through to `denotation` and this one did not, so `=` saw a denoted
            // string and the six text operators did not — paths, branch names,
            // error messages and commit messages are the coding agent's
            // principal text values, and the only text it could compute over was
            // a literal it already held.
            _ => {
                if self.value_depth >= 4 {
                    return None;
                }
                let v = self.denotation(r).filter(|v| *v != r)?;
                self.value_depth += 1;
                let out = self.text(v, env);
                self.value_depth -= 1;
                out
            }
        }
    }
}

fn increment(mask: &mut [bool]) -> bool {
    for bit in mask.iter_mut() {
        if *bit {
            *bit = false;
        } else {
            *bit = true;
            return true;
        }
    }
    false
}

fn tuples_of(per_position: &[Vec<ObjectId>]) -> Vec<Vec<ObjectId>> {
    let mut acc: Vec<Vec<ObjectId>> = vec![Vec::new()];
    for points in per_position {
        let mut next = Vec::new();
        for prefix in &acc {
            for p in points {
                let mut q = prefix.clone();
                q.push(*p);
                next.push(q);
            }
        }
        acc = next;
    }
    acc
}

fn product(domains: &[(ObjectId, Vec<ObjectId>)]) -> Vec<BTreeMap<ObjectId, ObjectId>> {
    let mut acc: Vec<BTreeMap<ObjectId, ObjectId>> = vec![BTreeMap::new()];
    for (v, members) in domains {
        let mut next = Vec::new();
        for prefix in &acc {
            for m in members {
                let mut b = prefix.clone();
                b.insert(*v, *m);
                next.push(b);
            }
        }
        acc = next;
    }
    acc
}

/// Worst-case merge of computation statuses. `Unsupported` dominates, because a
/// missing operator is not fixed by more budget.
fn downgrade(a: ComputeStatus, b: ComputeStatus) -> ComputeStatus {
    use ComputeStatus::*;
    match (a, b) {
        (Unsupported, _) | (_, Unsupported) => Unsupported,
        (BudgetExhausted, _) | (_, BudgetExhausted) => BudgetExhausted,
        (Stalled, _) | (_, Stalled) => Stalled,
        _ => Exact,
    }
}

/// A structure backed by an in-memory table, for tests and small views.
#[derive(Default)]
pub struct MapGraphStructure {
    facts: std::collections::BTreeSet<(ObjectId, Vec<ObjectId>)>,
    /// Tuples the store was told **both** ways — Belnap's `B` (semantics §8.2:
    /// `aff ≠ ∅ ∧ den ≠ ∅`).
    conflicts: std::collections::BTreeSet<(ObjectId, Vec<ObjectId>)>,
    closed: std::collections::BTreeSet<ObjectId>,
    domains: BTreeMap<ObjectId, Vec<ObjectId>>,
    /// Integer values of literal nodes, so `scale` can read its factor back out
    /// of a fact without needing the graph.
    ints: BTreeMap<ObjectId, BigInt>,
    rules: BTreeMap<ObjectId, Vec<ObjectId>>,
    values: BTreeMap<ObjectId, ObjectId>,
    worlds: Vec<ObjectId>,
    instants: Vec<i64>,
    /// Who vouches for a proposition, and how strongly it is believed.
    attribution: BTreeMap<ObjectId, Vec<ObjectId>>,
    credences: BTreeMap<ObjectId, Credence>,
    version: u64,
}

impl MapGraphStructure {
    pub fn new() -> Self {
        Self::default()
    }
    /// The store was told **both** directions — Belnap's `B`.
    ///
    /// Added because its absence was a hole in the *verification*, not in the
    /// fixture. `tests/soundness.rs` could not generate a conflicted atom, so
    /// its structures only ever reached `T`, `F` and `N` — and the one soundness
    /// bug it failed to catch (material implication reporting a refuted
    /// implication as supported) lives exactly at `B`. A property test that
    /// cannot express a quarter of the truth values is not checking the logic it
    /// claims to.
    pub fn conflicting(mut self, pred: ObjectId, args: Vec<ObjectId>) -> Self {
        self.conflicts.insert((pred, args));
        self
    }

    pub fn fact(mut self, pred: ObjectId, args: Vec<ObjectId>) -> Self {
        self.facts.insert((pred, args));
        self.version += 1;
        self
    }
    /// `(scale unit factor base)` as an ordinary fact.
    pub fn scale_fact(mut self, unit: ObjectId, factor: ObjectId, n: i64, base: ObjectId) -> Self {
        self.ints.insert(factor, BigInt::from(n));
        self.facts.insert((wk::SCALE, vec![unit, factor, base]));
        self.version += 1;
        self
    }
    /// What a term denotes.
    pub fn denotes(mut self, term: ObjectId, value: ObjectId) -> Self {
        self.values.insert(term, value);
        self.version += 1;
        self
    }
    pub fn rule(mut self, concludes: ObjectId, rule: ObjectId) -> Self {
        self.rules.entry(concludes).or_default().push(rule);
        self.version += 1;
        self
    }
    pub fn world(mut self, w: ObjectId) -> Self {
        self.worlds.push(w);
        self
    }
    pub fn instant(mut self, t: i64) -> Self {
        self.instants.push(t);
        self.instants.sort_unstable();
        self
    }
    pub fn closed(mut self, pred: ObjectId) -> Self {
        self.closed.insert(pred);
        self
    }
    pub fn domain(mut self, d: ObjectId, members: Vec<ObjectId>) -> Self {
        self.domains.insert(d, members);
        self
    }
    pub fn at_version(mut self, v: u64) -> Self {
        self.version = v;
        self
    }
    /// Who said so.
    pub fn attributed(mut self, proposition: ObjectId, sources: Vec<ObjectId>) -> Self {
        self.attribution.insert(proposition, sources);
        self
    }
    /// Graded belief, in log-odds milli-units.
    pub fn graded(mut self, proposition: ObjectId, lo: i64, hi: i64) -> Self {
        self.credences.insert(proposition, Credence { lo, hi });
        self.version += 1;
        self
    }
}

impl GraphStructure for MapGraphStructure {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        // Told both. Checked first, because a conflicted tuple is also in
        // `facts` and `Holds` would mask it.
        if self.conflicts.contains(&(pred, args.to_vec())) {
            return Knowledge::Conflicted;
        }
        if self.facts.contains(&(pred, args.to_vec())) {
            return Knowledge::Holds;
        }
        // An in-memory map genuinely *is* complete for a closed predicate:
        // there is no index to be partial, no timeout, nothing to be refused.
        // A resolver over a real store is not, which is why the judgement lives
        // here rather than in the evaluator.
        if self.closed.contains(&pred) {
            Knowledge::Fails
        } else {
            Knowledge::Unknown
        }
    }
    fn extension(&self, domain: ObjectId) -> Option<Extension> {
        // An in-memory map really is exhaustive for what it holds — there is no
        // index to be partial and nothing to time out. A resolver over a real
        // store must decide this per query.
        self.domains.get(&domain).cloned().map(Extension::complete)
    }
    fn is_closed(&self, pred: ObjectId) -> bool {
        self.closed.contains(&pred)
    }
    fn attribution(&self, proposition: ObjectId) -> Vec<ObjectId> {
        self.attribution.get(&proposition).cloned().unwrap_or_default()
    }
    fn credence(&self, proposition: ObjectId) -> Option<Credence> {
        self.credences.get(&proposition).copied()
    }
    /// Units come out of the ordinary fact table, which is the whole content of
    /// "units are data". No structure implemented this hook, so
    /// `(scale minutes 60 seconds)` could be written and never acted on.
    fn scale(&self, unit: ObjectId) -> Option<(BigInt, ObjectId)> {
        self.facts.iter().find_map(|(pred, args)| match args.as_slice() {
            [u, factor, base] if *pred == wk::SCALE && *u == unit => {
                Some((self.ints.get(factor).cloned()?, *base))
            }
            _ => None,
        })
    }
    fn rules(&self, pred: ObjectId) -> Vec<ObjectId> {
        self.rules.get(&pred).cloned().unwrap_or_default()
    }
    fn value(&self, term: ObjectId) -> Option<ObjectId> {
        self.values.get(&term).copied()
    }
    fn worlds(&self) -> Vec<ObjectId> {
        self.worlds.clone()
    }
    fn instants(&self) -> Vec<i64> {
        self.instants.clone()
    }
    fn snapshot(&self) -> u64 {
        self.version
    }
}
