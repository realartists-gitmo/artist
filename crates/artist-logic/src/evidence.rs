//! Four-valued evidence, and evaluation results that keep the reasons apart.
//!
//! Three-valued truth was wrong for a *memory*. `Unknown` was doing four jobs
//! at once: nothing is known, the evaluator ran out of budget, the operator has
//! no semantics, and — the one with no home at all — **both sides have
//! evidence**. A store that ingests claims from many sessions will hold support
//! for `P` and for `¬P` simultaneously, and collapsing that into the same value
//! as "no idea" destroys the only signal that says *look here*.
//!
//! So support and refutation are tracked independently, and computational
//! status is a separate axis again. "I have no evidence", "I have contradictory
//! evidence", "I ran out of budget" and "I cannot interpret this operator" are
//! four different answers and the caller can tell them apart.

use crate::object::{IdSpace, ObjectId};
use std::collections::{BTreeMap, BTreeSet};

/// One side of the evidence, as a monotone bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Bound {
    /// **This side's bit is established *zero*.**
    ///
    /// Declared first so the derived `Ord` makes [`Self::meet`] and
    /// [`Self::join`] the Kleene operations on `{0, u, 1}`: a meet is `Excluded`
    /// if any operand is, `Certain` only if all are, and unknown otherwise.
    ///
    /// `None` used to carry this meaning as well as its own, and the conflation
    /// is what made two rules unsound. `F`'s support bit is a known zero and a
    /// truth-teller's is undefined; both reported `None`, so every clause reading
    /// `¬a⁺` had to treat a settled fact as unknown. It is also why
    /// `⟦E⟧ ∈ {N,F}` — *E fails* — could not be said at all, only
    /// `⟦E⟧ ∈ {F,B}` — *E is refuted* — which admits the designated `B`.
    Excluded,
    /// Nothing establishes this side, and nothing rules it out either.
    #[default]
    None,
    /// Some evidence, not conclusive.
    ///
    /// It used to be produced by `usually`, and that was a conflation: a default
    /// is not weak evidence, it is evidence with a defeat condition, and
    /// defeasibility now lives on [`Derivation`] where it belongs. The level
    /// then had **no producer at all** for a while, which this comment said
    /// outright rather than pressing a third meaning into the gap.
    ///
    /// It has one now, and it is the case the level was always for: `(determinate
    /// P)` over a proposition whose condition is [`Determinacy::Underspecified`]
    /// — sharp, real, and simply unrecorded. Neither a yes nor a no, and
    /// collapsing it to either would be a claim about the records masquerading
    /// as a claim about the world.
    ///
    /// Graded *strength* — "three of five sources agree" — stays on
    /// [`Credence`], where the arithmetic is right, rather than being flattened
    /// onto this three-element lattice.
    Partial,
    /// Established.
    Certain,
}

impl Bound {
    pub fn meet(self, other: Bound) -> Bound {
        self.min(other)
    }
    pub fn join(self, other: Bound) -> Bound {
        self.max(other)
    }
    pub fn is_certain(self) -> bool {
        self == Bound::Certain
    }
    /// Established *not* to hold — the bit is zero.
    pub fn is_excluded(self) -> bool {
        self == Bound::Excluded
    }
    /// Does this side say anything about the structure at all?
    ///
    /// `None` and `Partial` do not: `Partial` reports the search, and neither
    /// constrains `⟦n⟧`. Only the two settled states are claims.
    pub fn is_settled(self) -> bool {
        matches!(self, Bound::Excluded | Bound::Certain)
    }
}

/// The four evidential states, independent of how computation went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidential {
    /// Neither supported nor refuted.
    Open,
    Supported,
    Refuted,
    /// Both — a genuine contradiction in the store, not an absence.
    Conflicted,
}

/// Whether a sentence is *grounded* — a third axis, and not a kind of evidence.
///
/// The liar was reported `Conflicted`, which made one value mean two unrelated
/// things. `Conflicted` is supposed to mean the store holds claims in both
/// directions — the signal that says *look here*, with two sources to go read.
/// The liar has no sources and no claims; it has no stable value at all, which
/// §5.3 states in bold and then contradicted by reusing the same value for it.
///
/// A caller that finds `Conflicted` should go look at the evidence. A caller
/// that finds `Oscillatory` should stop asking. Those are different
/// instructions, and one value could not carry both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Grounding {
    /// Reaches a value through the ordinary evaluation of its parts.
    #[default]
    Grounded,
    /// Depends on its own truth, stably — the truth-teller. Never gets a value,
    /// and no revision changes that.
    StableLoop,
    /// Depends on its own truth with reversed polarity — the liar. Its value
    /// would oscillate between revision rounds rather than converge.
    Oscillatory,
}

impl Grounding {
    /// A composite is no better grounded than its worst part.
    pub fn merge(self, other: Grounding) -> Grounding {
        self.max(other)
    }
}

/// A **credence interval** in log-odds milli-units, or nothing.
///
/// Interval rather than point, for a proved reason. Dekel–Lipman–Rustichini
/// showed that no single-space, two-valued representation can host an agent
/// unaware of some propositions, and every construction that escapes it makes
/// *expressibility* an index separate from truth — which is what the two-sided
/// bound already does. A point credence would give that back up.
///
/// Log-odds because independent evidence *adds* there, and because the store
/// already keeps `llr_milli` deduplicated by origin, so corroboration compounds
/// while hearing the same thing twice does not.
///
/// **`None` is not `[0,1]`.** An unknown credence over a proposition you can
/// state is a wide interval; a proposition your vocabulary cannot state has no
/// interval at all. Heifetz–Meier–Schipper make this exact — belief with
/// probability ≥ 0 coincides with *awareness* — and Piermont shows the two are
/// behaviourally distinct: refusing to commit to any plan cannot be produced by
/// any amount of uncertainty under full awareness. Collapsing them would lose
/// the distinction that the whole architecture is built to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Credence {
    /// Lower bound, log-odds × 1000. `i64::MIN / 4` reads as "certainly not".
    pub lo: i64,
    /// Upper bound.
    pub hi: i64,
}

impl Credence {
    /// The widest honest interval: expressible, and nothing known.
    pub const VACUOUS: Credence = Credence { lo: -20_000, hi: 20_000 };

    pub fn point(llr: i64) -> Credence {
        Credence { lo: llr, hi: llr }
    }

    /// Independent evidence adds in log-odds — which is the whole reason the
    /// ledger stores them that way.
    pub fn combine(self, other: Credence) -> Credence {
        Credence {
            lo: self.lo.saturating_add(other.lo).clamp(-20_000, 20_000),
            hi: self.hi.saturating_add(other.hi).clamp(-20_000, 20_000),
        }
    }

    /// The credence of a **conjunction**, from the credences of its parts.
    ///
    /// Not `combine`. Log-odds add for independent evidence bearing on *one*
    /// proposition; they do not add across a conjunction of *different* ones,
    /// and treating the two as the same operation is how a system ends up
    /// confident that six 90%-likely things are all true at once.
    ///
    /// What is stated here is the half of the Fréchet bound that needs no
    /// independence assumption at all: a conjunction is no more likely than its
    /// least likely conjunct. The lower bound genuinely is vacuous without one —
    /// two individually near-certain claims can be jointly impossible — so it is
    /// reported as vacuous rather than assumed away. That asymmetry is not a
    /// gap in the implementation; it is the actual state of knowledge, and the
    /// interval representation exists to be able to say it.
    pub fn conjoin(self, other: Credence) -> Credence {
        Credence { lo: Credence::VACUOUS.lo, hi: self.hi.min(other.hi) }
    }

    /// The credence of a **disjunction** — the mirror. A disjunction is at least
    /// as likely as its likeliest disjunct, and no independence assumption
    /// bounds it above.
    pub fn disjoin(self, other: Credence) -> Credence {
        Credence { lo: self.lo.max(other.lo), hi: Credence::VACUOUS.hi }
    }

    /// What both sources agree on. Disagreement *widens*: an interval that
    /// narrowed under conflict would be claiming precision the sources do not
    /// jointly support.
    pub fn merge(self, other: Credence) -> Credence {
        Credence { lo: self.lo.min(other.lo), hi: self.hi.max(other.hi) }
    }

    pub fn negate(self) -> Credence {
        // Saturating, like `combine`. A structure returning `i64::MIN` panicked
        // on any negation above it — one factor outside the documented
        // `i64::MIN / 4` convention, which is not far enough to rely on.
        Credence { lo: self.hi.saturating_neg(), hi: self.lo.saturating_neg() }
    }

    /// Is the whole interval on one side of even odds?
    pub fn decided(self) -> Option<bool> {
        if self.lo > 0 {
            Some(true)
        } else if self.hi < 0 {
            Some(false)
        } else {
            None
        }
    }
}

/// Whether a proposition has a **stable, determinate satisfaction condition** —
/// and therefore whether classical operations on it are licensed.
///
/// This is a fifth axis and it is deliberately *not* part of [`Grounding`],
/// though folding it there is the obvious-looking move. The test for separate
/// axes is whether all four combinations are inhabited, and they are: the liar
/// is ungrounded and semantically precise; a borderline `(heap 4783)` is
/// perfectly grounded and semantically indeterminate; "this sentence is
/// heapish" is both. Combining them would recreate exactly the conflation that
/// splitting `Conflicted` and `Partial` was meant to remove.
///
/// Sorites is the reason this exists. Most natural-language predicates are not
/// sharp, so *totality cannot be assumed for every stored expression* — it is a
/// checked or declared property of a proposition **under an interpretation**,
/// and often only over a region of the argument space. `heap` is sharp at ten
/// thousand grains and has no answer at forty-seven hundred.
///
/// Crucially this does **not** override the evidence axis. There is real
/// evidence that someone is tall, and the honest report is
/// `Supported` + `Indeterminate`: strong grounds, no sharp fact. Determinacy
/// gates classical *operations*; it does not force evidence back to `Open`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Determinacy {
    /// Established total: a sharp condition, and the store says so.
    Total,
    /// A sharp condition exists but has not been recorded. "Too complex" where
    /// the threshold is real and simply unwritten — a gap in the *records*, not
    /// in the world.
    Underspecified,
    /// Not established either way. The default, and the honest one: ordinary
    /// evaluation presumes totality, and this records that it was **presumed
    /// rather than established** so certification can refuse it.
    #[default]
    Unknown,
    /// Declared to have no sharp condition here. Borderline by nature.
    Indeterminate,
}

/// **How** a determinacy verdict was arrived at.
///
/// Without this, a `Total` that somebody wrote down and a `Total` the evaluator
/// worked out are the same value, and a reader who wants to retract the first
/// has nothing to retract. Same reasoning as `defeated_by`: a claim's grounds
/// have to travel with it or they cannot be argued with. `Presumed` is the
/// default and is what makes ordinary evaluation distinguishable from
/// certification even when both report the same value.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum DeterminacyBasis {
    /// Nobody said; ordinary evaluation proceeds anyway and records that it did.
    #[default]
    Presumed,
    /// Stated outright, by these assertions.
    Declared(Vec<ObjectId>),
    /// Worked out — a constructive validity, or a rule over declarations.
    Derived,
}

impl Determinacy {
    /// A composite is no more determinate than its least determinate part —
    /// but **`Unknown` is not a floor.** The default is `Unknown`, so a plain
    /// `max` over `Total < Underspecified < Unknown < Indeterminate` made
    /// `merge(Unknown, Total) == Unknown`, and a declared `Total` could never
    /// reach a result: the hook's entire positive answer was unreachable and
    /// `licenses_classical` was unsatisfiable outside the tautology path.
    ///
    /// `Unknown` means *nobody has said*, which anything said supersedes.
    /// `Indeterminate` and `Underspecified` are claims, and they still dominate.
    pub fn merge(self, other: Determinacy) -> Determinacy {
        use Determinacy::*;
        match (self, other) {
            (Unknown, x) | (x, Unknown) => x,
            (a, b) => a.max(b),
        }
    }

    /// May a *classical* operation — excluded middle, bivalent case analysis —
    /// be applied? Only against an established sharp condition.
    pub fn licenses_classical(self) -> bool {
        self == Determinacy::Total
    }
}

/// What an evaluation is *for*.
///
/// One mode was one too few. If ordinary queries and trusted inference share a
/// default, then presuming totality — which is the only practical default, since
/// almost nothing is ever certified sharp — silently certifies
/// `heap(x) ∨ ¬heap(x)` for a predicate whose interpretation nobody established
/// as total. Excluded middle is *precisely* the tautology that needs bivalence,
/// which needs totality, so a validity recogniser with an optimistic default is
/// a bivalence-laundering machine that looks like a feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// Answer the question. Presume totality, and record that you presumed.
    #[default]
    Query,
    /// Certify the answer. Totality must be established, not presumed.
    Certify,
}

/// How a conclusion was reached. Orthogonal to how *strong* it is.
///
/// Defeasibility used to be encoded as weakness: `usually` produced
/// `Bound::Partial`, so "holds by default" and "some evidence, not conclusive"
/// were the same value. They are not the same thing. A default can be
/// overwhelmingly well attested and still be defeasible — "the test suite
/// passes" is both — and a claim can be weakly evidenced without being
/// defeasible at all.
///
/// Collapsing them cost in both directions: `Partial` had to stand in for
/// defeasibility, which left graded strength with no representation, and every
/// consumer that reasoned about strength was silently reasoning about defeat
/// conditions instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Derivation {
    /// Read off the structure, or computed.
    #[default]
    Observed,
    /// Concluded through a stored rule.
    Derived,
    /// Holds absent a defeater. Whatever its strength, it is revisable by
    /// evidence that does not touch the reasons for it.
    Default,
}

impl Derivation {
    /// A conclusion is as defeasible as the most defeasible step in it.
    pub fn merge(self, other: Derivation) -> Derivation {
        self.max(other)
    }
}

/// Why evaluation is in the state it is in. Orthogonal to the evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComputeStatus {
    /// Ran to completion.
    Exact,
    /// Budget ran out; more budget would do more.
    BudgetExhausted,
    /// An operator here has no registered semantics. More budget will not help;
    /// registering semantics will.
    Unsupported,
    /// Ran to completion and made no progress — the abstraction is too weak or
    /// the question is undecidable. Re-asking will stall the same way.
    Stalled,
}

/// The result of evaluating an expression.
#[derive(Clone, Debug)]
pub struct EvaluationResult {
    pub support: Bound,
    pub refutation: Bound,
    pub compute_status: ComputeStatus,
    /// Whether this sentence reaches a value at all.
    pub grounding: Grounding,
    /// How it was reached, and therefore what could overturn it.
    pub derivation: Derivation,
    /// Whether the proposition has a sharp condition to be right or wrong about.
    pub determinacy: Determinacy,
    /// …and on what grounds. See [`DeterminacyBasis`].
    pub determinacy_basis: DeterminacyBasis,
    /// Graded belief, when the store grades. `None` means *not expressible or
    /// not graded* — see [`Credence`] on why that is not the same as a wide
    /// interval.
    pub credence: Option<Credence>,
    /// Conditions that would defeat this conclusion — the defeaters. Empty for
    /// an observed one.
    pub defeated_by: Vec<ObjectId>,
    /// **What settled it**: the member that witnessed an existential, or the
    /// counterexample that refuted a universal, as `(slot, value)`.
    ///
    /// Keyed by **slot position**, not by variable id. A stored binder is de
    /// Bruijn and `open_binder` mints a fresh nominal on every opening, so the
    /// variable the scan bound is an internal artifact the caller has no handle
    /// on — reporting it would name something the asker cannot match against
    /// their own question. The slot is what they wrote.
    ///
    /// The evaluator could prove existence without exhibiting a witness — the
    /// scan had the member in hand at the moment it short-circuited and threw it
    /// away. That made every *wh*-question unaskable: "is `mod1` the module with
    /// the most failures" was checkable, "which module has the most failures"
    /// was not, so the store could grade an answer and never produce one.
    ///
    /// Unlike the other axes, bindings **do not propagate**. A defeasible step
    /// anywhere makes a whole conclusion defeasible, so those must travel up; a
    /// witness justifies exactly the claim it witnessed and says nothing about
    /// whatever wraps it. So compositions drop it unless they are transparent
    /// pass-throughs, which is conservative in the direction that cannot be
    /// wrong.
    pub bindings: Vec<(usize, ObjectId)>,
    /// What is left to do, as an object in the same universe.
    pub residual: Option<ObjectId>,
    /// Serialized evaluator state, so work resumes rather than restarting.
    pub continuation: Option<ObjectId>,
    /// Objects this result depends on — the invalidation set.
    pub dependencies: Vec<ObjectId>,
    /// Which version of the universe this was computed against. Bounds from
    /// different versions must not be composed.
    pub snapshot: u64,
    pub spent: u64,
}

impl EvaluationResult {
    pub fn new(status: ComputeStatus) -> Self {
        EvaluationResult {
            support: Bound::None,
            refutation: Bound::None,
            compute_status: status,
            grounding: Grounding::Grounded,
            derivation: Derivation::Observed,
            determinacy: Determinacy::Unknown,
            determinacy_basis: DeterminacyBasis::Presumed,
            credence: None,
            defeated_by: Vec::new(),
            bindings: Vec::new(),
            residual: None,
            continuation: None,
            dependencies: Vec::new(),
            snapshot: 0,
            spent: 0,
        }
    }

    /// A definite classical answer: `T` when `v`, `F` otherwise.
    ///
    /// **Both bits are settled**, which is the point of `Bound::Excluded`
    /// existing. `T` is `⟨1,0⟩`, not `⟨1,unknown⟩`: a structure that says a fact
    /// *holds* has told you its refutation bit is zero, and leaving that as
    /// `None` threw the information away — which is why "the antecedent fails"
    /// could not be expressed and a whole row of the implication table was lost.
    ///
    /// `Conflicted` is the case where both bits are one, and it has its own
    /// constructor rather than being reachable from here.
    pub fn certain(v: bool) -> Self {
        let mut r = EvaluationResult::new(ComputeStatus::Exact);
        if v {
            r.support = Bound::Certain;
            r.refutation = Bound::Excluded;
        } else {
            r.support = Bound::Excluded;
            r.refutation = Bound::Certain;
        }
        r
    }

    /// Both sides established: the store holds claims in both directions.
    ///
    /// This is what the four-valued design was *for*, and until `Knowledge`
    /// gained a case for it the only expression in the whole language that
    /// could produce it was the liar. A memory that ingests claims from many
    /// sessions reaches this state constantly, and reaching it is the signal
    /// that says *look here* — which is why it must not be a tie broken by
    /// whichever source was read first.
    pub fn conflicted() -> Self {
        let mut r = EvaluationResult::new(ComputeStatus::Exact);
        r.support = Bound::Certain;
        r.refutation = Bound::Certain;
        r
    }

    /// The four-valued reading, from the two bits.
    ///
    /// **Keyed on what each side *asserts*, not on whether it is non-`None`.**
    /// This matched `(_, Bound::None)` for `Supported`, which was right while
    /// `None` was the only way to say "nothing on that side" — and became wrong
    /// the moment `Excluded` existed, because a definite `T` is `⟨Certain,
    /// Excluded⟩` and the old reading called it `Conflicted`. `Conflicted` means
    /// **both bits are one**, which is a fact about the store, and `Excluded` is
    /// the opposite of evidence rather than a weak form of it.
    pub fn evidential(&self) -> Evidential {
        match (self.support.is_certain(), self.refutation.is_certain()) {
            (true, true) => Evidential::Conflicted,
            (true, false) => Evidential::Supported,
            (false, true) => Evidential::Refuted,
            (false, false) => Evidential::Open,
        }
    }

    /// Only an exact, uncontradicted, **certain** result licenses a definite
    /// reading.
    ///
    /// `Supported` merely means support exceeds `None`, so testing the
    /// evidential state alone promoted "usually true, nothing against it" into a
    /// definite truth. Defeasibility now lives on [`Derivation`], and the test
    /// below consults it directly.
    /// One side established and the other **empty**, grounded, and not
    /// defeasible.
    ///
    /// Testing `is_certain() != is_certain()` called `(Partial, Certain)`
    /// definite — a claim with partial support *and* conclusive evidence
    /// against it, which is contested and is the one state that must never read
    /// as settled. And a *default*, however strongly attested, is not a
    /// definite reading either: something that does not touch the reasons for it
    /// can still overturn it.
    pub fn is_definite(&self) -> bool {
        self.compute_status == ComputeStatus::Exact
            && self.grounding == Grounding::Grounded
            && self.derivation != Derivation::Default
            // **One side certain and the other not certain.** This read
            // `== Bound::None` on the opposite side, which was the only way to
            // say "nothing there" before `Excluded` existed — and became wrong
            // immediately after, because a definite `T` is now
            // `⟨Certain, Excluded⟩` and the strongest possible answer stopped
            // counting as definite. What must be excluded is the *other side
            // also being certain*, which is `Conflicted`, not the other side
            // being settled.
            && (self.support.is_certain() != self.refutation.is_certain())
    }

    /// Record what settled this, when a scan decided it on one member.
    pub fn witnessed(mut self, slot: usize, value: ObjectId) -> Self {
        self.bindings.push((slot, value));
        self
    }

    /// Mark this conclusion defeasible, naming what would defeat it.
    pub fn defeasible(mut self, by: ObjectId) -> Self {
        self.derivation = self.derivation.merge(Derivation::Default);
        if !self.defeated_by.contains(&by) {
            self.defeated_by.push(by);
        }
        self
    }

    pub fn negate(mut self) -> Self {
        std::mem::swap(&mut self.support, &mut self.refutation);
        self.credence = self.credence.map(Credence::negate);
        self
    }
}

/// Polarity of an assertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Polarity {
    Affirm,
    Deny,
}

/// A proposition asserted *by someone, somewhere, at some time, under some
/// modality*.
///
/// The proposition is not the record. `prefers(adam, tabs)` is timeless and
/// agent-free; what a memory actually holds is that a particular agent asserted
/// it in a particular world over a particular interval. A `live` flag can exist
/// as a search-index projection, but it cannot be the semantic record — that was
/// exactly the conflation that made "what did I used to believe" unaskable.
#[derive(Clone, Debug)]
pub struct Assertion {
    pub id: ObjectId,
    pub proposition: ObjectId,
    pub polarity: Polarity,
    pub asserting_agent: Option<ObjectId>,
    pub world: Option<ObjectId>,
    /// Valid time: when the claim holds, in microseconds.
    pub valid_from: Option<i64>,
    pub valid_to: Option<i64>,
    /// Transaction time: when we learned it.
    pub recorded_at: i64,
    pub modality: Option<ObjectId>,
    pub interpretation: Option<ObjectId>,
    pub scope: Option<ObjectId>,
}

impl Assertion {
    /// Derive this assertion's identity from **all** of its fields.
    ///
    /// An assertion cannot be keyed by its proposition. The same proposition is
    /// routinely asserted by different agents, in different worlds, over
    /// different intervals, with opposite polarity — those are distinct records
    /// and keying on the proposition would let one silently overwrite another.
    ///
    /// Deriving rather than requiring a caller-supplied id also makes the
    /// idempotence right: re-recording the *same* claim by the same agent at the
    /// same time is one record, while changing any field is a new one.
    ///
    /// Domain-separated from expression content ids, and tagged `Content`,
    /// because an assertion is identified by its structure just as an
    /// expression is.
    pub fn derive_id(&self) -> ObjectId {
        fn opt(h: &mut blake3::Hasher, tag: u8, v: Option<ObjectId>) {
            h.update(&[tag]);
            h.update(&v.map(|x| x.0).unwrap_or(0).to_le_bytes());
        }
        let mut h = blake3::Hasher::new();
        h.update(b"artist-logic/assertion\0");
        h.update(&self.proposition.0.to_le_bytes());
        h.update(&[matches!(self.polarity, Polarity::Affirm) as u8]);
        opt(&mut h, 1, self.asserting_agent);
        opt(&mut h, 2, self.world);
        opt(&mut h, 3, self.modality);
        opt(&mut h, 4, self.interpretation);
        opt(&mut h, 5, self.scope);
        h.update(&self.valid_from.unwrap_or(i64::MIN).to_le_bytes());
        h.update(&self.valid_to.unwrap_or(i64::MAX).to_le_bytes());
        h.update(&self.recorded_at.to_le_bytes());
        let bytes = h.finalize();
        let mut buf = [0u8; 16];
        buf.copy_from_slice(&bytes.as_bytes()[..16]);
        ObjectId::tagged(IdSpace::Content, u128::from_le_bytes(buf))
    }

    /// Build an assertion with its identity already derived.
    pub fn sealed(mut self) -> Self {
        self.id = self.derive_id();
        self
    }

    /// An affirmation with everything optional left out.
    pub fn affirm(proposition: ObjectId, recorded_at: i64) -> Self {
        Assertion {
            id: ObjectId(0),
            proposition,
            polarity: Polarity::Affirm,
            asserting_agent: None,
            world: None,
            valid_from: None,
            valid_to: None,
            recorded_at,
            modality: None,
            interpretation: None,
            scope: None,
        }
        .sealed()
    }
}

/// Evidence bearing on an assertion, stored separately from the proposition so
/// that retraction removes a *relation* and never the proposition itself.
#[derive(Clone, Debug)]
pub struct Evidence {
    pub id: ObjectId,
    pub target: ObjectId,
    pub polarity: Polarity,
    pub source: Option<ObjectId>,
    pub source_span: Option<SourceSpan>,
    pub transformation: Option<ObjectId>,
    pub derived_from: Vec<ObjectId>,
    /// Log-likelihood ratio, scaled by 1000 so it stays exact and hashable.
    pub llr_milli: Option<i64>,
    pub created_by: Option<ObjectId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    pub locator: String,
    pub start: u64,
    pub end: u64,
}

/// A ledger over assertions and evidence.
#[derive(Clone, Debug, Default)]
pub struct EvidenceLedger {
    assertions: BTreeMap<ObjectId, Assertion>,
    evidence: Vec<Evidence>,
}

impl EvidenceLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn assert(&mut self, a: Assertion) {
        self.assertions.insert(a.id, a);
    }

    pub fn record(&mut self, e: Evidence) {
        self.evidence.push(e);
    }

    pub fn assertion(&self, id: ObjectId) -> Option<&Assertion> {
        self.assertions.get(&id)
    }

    /// Evidence bearing on a proposition, in both directions.
    pub fn bearing_on(&self, proposition: ObjectId) -> (Vec<&Evidence>, Vec<&Evidence>) {
        let relevant: Vec<&Evidence> = self
            .evidence
            .iter()
            .filter(|e| {
                e.target == proposition
                    || self
                        .assertions
                        .get(&e.target)
                        .is_some_and(|a| a.proposition == proposition)
            })
            .collect();
        let (mut pro, mut con) = (Vec::new(), Vec::new());
        for e in relevant {
            match e.polarity {
                Polarity::Affirm => pro.push(e),
                Polarity::Deny => con.push(e),
            }
        }
        (pro, con)
    }

    /// The evidential state of a proposition.
    ///
    /// Deduplicated by **origin**, not content: independent corroboration
    /// compounds, hearing the same thing twice does not.
    pub fn state_of(&self, proposition: ObjectId) -> Evidential {
        let (pro, con) = self.bearing_on(proposition);
        let distinct = |v: &[&Evidence]| -> usize {
            let mut seen = std::collections::BTreeSet::new();
            for e in v {
                seen.insert(e.source.map(|s| s.0).unwrap_or(e.id.0));
            }
            seen.len()
        };
        match (distinct(&pro) > 0, distinct(&con) > 0) {
            (false, false) => Evidential::Open,
            (true, false) => Evidential::Supported,
            (false, true) => Evidential::Refuted,
            (true, true) => Evidential::Conflicted,
        }
    }

    /// Net weight in log-odds milli-units, origin-deduplicated.
    pub fn weight_of(&self, proposition: ObjectId) -> i64 {
        let (pro, con) = self.bearing_on(proposition);
        let sum = |v: &[&Evidence]| -> i64 {
            // **Disjoint ancestry, not distinct origin.** Deduplicating by
            // `source` alone handles two independent *reports* and misses two
            // *derivations* that share a premise: both get their own evidence
            // record, both name a different immediate source, and log-odds are
            // additive — so one observation is counted twice and confidence
            // grows out of nothing.
            //
            // This is the defect PLN never closed. Their own book calls the
            // mechanism mandatory — "iterated inference involves multiple
            // evidence-counting, and so the resulting truth values become
            // meaningless" — the shipping engine never implemented it, and
            // NARS-style evidential stamps only arrived in the 2026 rewrite.
            // The data was already here in `derived_from`; nothing read it.
            //
            // Conservative on purpose: sharing a remote ancestor drops the
            // second contribution even where the two really are independent.
            // Understating confidence is survivable; manufacturing it is not.
            let mut taken: BTreeSet<u128> = BTreeSet::new();
            let mut total = 0i64;
            for e in v {
                let stamp = self.ancestry(e);
                if stamp.iter().any(|a| taken.contains(a)) {
                    continue;
                }
                taken.extend(stamp);
                total = total.saturating_add(e.llr_milli.unwrap_or(0));
            }
            total
        };
        sum(&pro) - sum(&con)
    }

    /// Graded belief about a proposition, as an interval, or nothing when no
    /// evidence bears on it at all.
    ///
    /// The `None` here is the one the unawareness literature insists on:
    /// *nothing bears on this*, which is not the same as *everything bearing on
    /// it cancels out*. The latter is a wide interval and this returns one.
    ///
    /// Evidence with no recorded strength still moves the interval — in one
    /// direction only. An affirmation of unknown weight cannot lower the
    /// credence, so it widens upward and leaves the floor alone; that is what an
    /// interval is for, and scoring it as zero would have silently treated
    /// "somebody vouched for this, strength unrecorded" as no evidence at all.
    ///
    /// Deduplicated by the same disjoint-ancestry rule as [`Self::weight_of`],
    /// for the same reason: two derivations sharing a premise are one piece of
    /// evidence wearing two hats.
    pub fn credence_of(&self, proposition: ObjectId) -> Option<Credence> {
        let (pro, con) = self.bearing_on(proposition);
        if pro.is_empty() && con.is_empty() {
            return None;
        }
        let mut taken: BTreeSet<u128> = BTreeSet::new();
        let mut acc = Credence::point(0);
        for (e, affirming) in
            pro.iter().map(|e| (*e, true)).chain(con.iter().map(|e| (*e, false)))
        {
            let stamp = self.ancestry(e);
            if stamp.iter().any(|a| taken.contains(a)) {
                continue;
            }
            taken.extend(stamp);
            let c = match (e.llr_milli, affirming) {
                (Some(w), true) => Credence::point(w),
                (Some(w), false) => Credence::point(-w),
                // Strength unrecorded: bounded on the side it cannot move.
                (None, true) => Credence { lo: 0, hi: Credence::VACUOUS.hi },
                (None, false) => Credence { lo: Credence::VACUOUS.lo, hi: 0 },
            };
            acc = acc.combine(c);
        }
        Some(acc)
    }

    /// Who vouches for a proposition: the origin of every piece of evidence
    /// bearing on it, and the agent of every assertion of it.
    ///
    /// This is what a certificate's `Told` step needs and had no way to get. A
    /// step that names no source is not a lesser certificate — it is an honest
    /// one, and the checker records it as taken on trust rather than pretending
    /// the relation symbol vouched for itself.
    pub fn attribution(&self, proposition: ObjectId) -> Vec<ObjectId> {
        let mut out: Vec<ObjectId> = Vec::new();
        let mut push = |id: ObjectId| {
            if !out.contains(&id) {
                out.push(id);
            }
        };
        for a in self.assertions.values() {
            if a.proposition == proposition && let Some(agent) = a.asserting_agent {
                push(agent);
            }
        }
        let (pro, con) = self.bearing_on(proposition);
        for e in pro.iter().chain(con.iter()) {
            if let Some(s) = e.source {
                push(s);
            } else if let Some(c) = e.created_by {
                push(c);
            }
        }
        out
    }

    /// Everything this evidence rests on: its origin, and the transitive
    /// closure of what it was derived from.
    ///
    /// Two pieces of evidence may be summed only if their stamps are disjoint.
    fn ancestry(&self, e: &Evidence) -> BTreeSet<u128> {
        let mut out = BTreeSet::new();
        let mut frontier = vec![e.id];
        out.insert(e.source.map(|s| s.0).unwrap_or(e.id.0));
        let mut guard = 0;
        while let Some(cur) = frontier.pop() {
            guard += 1;
            if guard > 1024 {
                break;
            }
            for src in self.evidence.iter().filter(|x| x.id == cur) {
                for d in &src.derived_from {
                    if out.insert(d.0) {
                        frontier.push(*d);
                    }
                }
                if let Some(s) = src.source {
                    out.insert(s.0);
                }
            }
        }
        out
    }

    pub fn evidence_count(&self) -> usize {
        self.evidence.len()
    }

    /// Retract an assertion. The proposition object is untouched — only the
    /// claim that someone asserted it stops holding.
    pub fn retract(&mut self, assertion: ObjectId, at: i64) {
        if let Some(a) = self.assertions.get_mut(&assertion) {
            a.valid_to = Some(at);
        }
    }
}
