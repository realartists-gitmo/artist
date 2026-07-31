//! A universal expression language: one representation that stores, prints and
//! decides.
//!
//! The design inverts the usual arrangement. Normally an evaluator's limits
//! propagate backwards into the representation — a decidable fragment is
//! chosen, and anything outside it cannot be written down. Here
//! **representation is total and evaluation is what may be partial.**
//!
//! That was not achieved by making the enums bigger. It was achieved by
//! removing them. [`object`] is an open graph in which `forall`, `lambda`,
//! `letrec`, `at`, `count`, `quote`, the modal operators and every future
//! construct are ordinary **operator objects**. An operator with no semantics
//! still parses, prints, hashes, persists, round-trips and can be quantified
//! over; only evaluation reports that it cannot interpret it, and [`registry`]
//! lets semantics arrive later with no migration and no reparse.
//!
//! The evaluator's answer is an interval rather than a truth value:
//!
//! ```text
//! must  ⊆  truth  ⊆  may
//! ```
//!
//! Both sides are monotone, so halting at an arbitrary instant leaves a sound
//! result and undecidability condenses into a residual instead of blocking. The
//! classical impossibility results are untouched; they constrain *decide φ*, and
//! this crate computes *a sound two-sided approximation of φ within budget b,
//! plus what remains*.
//!
//! What that buys, concretely:
//!
//! - **Negation stays honest under interruption.** Absence is refutation only
//!   where knowledge is complete, so a truncated scan can never be misread as a
//!   refutation.
//! - **Continuations shrink.** A cut-short scan returns the *same* quantifier
//!   over the unexamined tail, so re-asking makes progress rather than
//!   restarting — and resuming is just evaluating it.
//! - **Infinite domains stay decidable in practice.** A counterexample refutes a
//!   universal over ℕ in finite time; failing that, an interval abstraction with
//!   widening can close the unbounded tail.
//! - **Evidence is four-valued.** Nothing known, contradictory, out of budget
//!   and no-semantics are four different answers. A memory that ingests claims
//!   from many sessions genuinely holds support for `P` and for `¬P` at once.
//! - **Self-reference is representable.** The liar is constructible, storable
//!   and printable. Evaluation gives it Kripke semantics: grounded sentences get
//!   classical values, ungrounded-but-stable stay open, and ungrounded
//!   oscillators report a contradiction — which is what distinguishes the liar
//!   from the truth-teller.
//!
//! The maximality claim, stated exactly: **every finite or cyclic expression
//! graph over arbitrary first-class objects, types, binders, operators and
//! references can be represented, persisted and printed; anything external is
//! denotable by an opaque stable reference; evaluation is total under a budget,
//! sound where semantics are provided, and explicitly incomplete elsewhere.**
//!
//! It does *not* claim that finite syntax internally names every extensional
//! subset of an infinite domain — there are uncountably many such subsets and
//! only countably many finite descriptions. Those are reached by quantifying
//! over them collectively, defining the describable ones structurally,
//! referencing externally supplied ones, and returning honest partial results.

pub mod evidence;
pub mod graph_eval;
pub mod object;
pub mod registry;
pub mod syntax;
pub mod typing;

pub use evidence::{
    Assertion, Bound, ComputeStatus, EvaluationResult, Evidence, EvidenceLedger, Evidential,
    Polarity, SourceSpan,
};
pub use graph_eval::{
    BoundRelation, EmptyStructure, GraphEnv, GraphEvaluator, GraphStructure, MapGraphStructure,
};
pub use object::{
    Binding as ObjBinding, CoreNode, ExternalRef, LiteralValue, ObjectGraph, ObjectId, wk,
};
pub use registry::{OpContext, OperatorRegistry, OperatorSemantics};
pub use typing::{Typing, type_of};
