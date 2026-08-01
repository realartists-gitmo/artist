//! Differential properties over generated terms.
//!
//! This file exists because `conformance.rs` was not enough, and the reason is
//! worth stating precisely rather than filed as a lesson.
//!
//! `conformance.rs` was written to stop the spec drifting from the code, and it
//! works by walking §5.2's table one row at a time. Row tests check the cases
//! somebody thought of. Every finding in the three audits that followed was
//! found by **differential probing** instead: the same term at two budgets, a
//! query against its own continuation, one sentence in two spellings. None of
//! those is a row in any table, and all of them walked straight past a suite
//! that was green.
//!
//! So these are properties, not cases, and they are checked over generated
//! terms rather than hand-picked ones:
//!
//! 1. **Budget monotonicity** — more budget never moves *down* the information
//!    order. This is the assertion that catches a verdict flipping when you
//!    think longer, which is what the tail abstraction did.
//!
//!    That claim used to say it caught three findings at once, and
//!    instrumentation said otherwise: across 2400 evaluations the generator
//!    entered `infinite()` **zero** times (every quantifier domain was one the
//!    fixture enumerates), reached `derive`'s status accumulation **zero** times
//!    (the fixture had no rules), and produced defeasible results **zero**
//!    times (the fixture held no defaults). A property test over terms that cannot
//!    reach the code proves nothing while looking rigorous. The generator now
//!    quantifies over `Nat`/`Int`, the fixture carries a rule and a default, and
//!    continuation laundering is property 2's job rather than this one's.
//! 2. **Continuation soundness** — resuming is never *stronger* than the
//!    question that was suspended.
//! 3. **Spelling invariance** — classically identical spellings agree.
//! 4. **Round-trip preserves evaluation** — over generated atom names, not the
//!    seven that happened to get written down.
//!
//! Generation is deterministic. `Math::random` has no place in a test that has
//! to be reproducible from its own failure message, so the terms come from a
//! fixed LCG and every failure prints the seed that produced it.
//!
//! **Budgets are deliberately small.** Every property here needs budgets that
//! *vary*, not budgets that are large — and a generated term over `Nat` or
//! `Int` sends the evaluator into `infinite()`, which probes concrete integers
//! until the budget runs down. At 200k and 1M that was tens of thousands of
//! probe iterations per term and about 95% of the whole suite's wall clock,
//! buying nothing: not one assertion here distinguishes a budget of 5,000 from
//! one of 1,000,000.

use artist_logic::evidence::{ComputeStatus, EvaluationResult, Evidential};
use artist_logic::graph_eval::{GraphEvaluator, GraphStructure, Knowledge, MapGraphStructure};
use artist_logic::object::{Binding, LiteralValue, wk};
use artist_logic::syntax::{parse, print};
use artist_logic::{ObjectGraph, ObjectId};
use num_bigint::BigInt;

// ------------------------------------------------------------- generation

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // Numerical Recipes LCG. Reproducible is the only requirement.
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 11
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len() as u64) as usize]
    }
}

/// A structure with a bit of everything, so generated terms actually reach the
/// interesting paths rather than stalling immediately.
struct Mixed {
    holds: Vec<(ObjectId, Vec<ObjectId>)>,
    /// Rules to derive through, so `derive`'s status accumulation is actually
    /// entered rather than short-circuiting on an empty candidate list.
    rules: Vec<(ObjectId, ObjectId)>,
    /// Defaults on record, so `Derivation::Default` appears in generated terms
    /// at all — without one, `usually` reports its argument and defeasibility is
    /// never exercised here. (`Bound::Partial` has no producer and cannot appear;
    /// this comment claimed otherwise for a round.)
    defaults: Vec<ObjectId>,
    denied: Vec<(ObjectId, Vec<ObjectId>)>,
    domain: ObjectId,
    members: Vec<ObjectId>,
    complete: bool,
    world: ObjectId,
}

impl GraphStructure for Mixed {
    fn known(&self, pred: ObjectId, args: &[ObjectId]) -> Knowledge {
        if self.holds.iter().any(|(p, a)| *p == pred && a == args) {
            return Knowledge::Holds;
        }
        if self.denied.iter().any(|(p, a)| *p == pred && a == args) {
            return Knowledge::Denied;
        }
        if pred == wk::USUALLY && args.len() == 1 && self.defaults.contains(&args[0]) {
            return Knowledge::Holds;
        }
        Knowledge::Unknown
    }
    fn rules(&self, pred: ObjectId) -> Vec<ObjectId> {
        self.rules.iter().filter(|(p, _)| *p == pred).map(|(_, r)| *r).collect()
    }
    fn extension(&self, domain: ObjectId) -> Option<artist_logic::graph_eval::Extension> {
        (domain == self.domain).then(|| artist_logic::graph_eval::Extension {
            members: self.members.clone(),
            complete: self.complete,
        })
    }
    fn worlds(&self) -> Vec<ObjectId> {
        vec![self.world]
    }
    fn instants(&self) -> Vec<i64> {
        vec![1, 2, 3]
    }
}

struct Gen {
    preds: Vec<ObjectId>,
    consts: Vec<ObjectId>,
    domain: ObjectId,
}

impl Gen {
    /// A proposition of bounded depth. Every shape the evaluator dispatches on
    /// gets a chance to appear.
    fn prop(&self, g: &mut ObjectGraph, r: &mut Rng, depth: u32) -> ObjectId {
        if depth == 0 {
            return self.atom(g, r);
        }
        match r.below(13) {
            0 => {
                let inner = self.prop(g, r, depth - 1);
                g.apply(wk::NOT, vec![inner])
            }
            1 | 2 => {
                let (a, b) = (self.prop(g, r, depth - 1), self.prop(g, r, depth - 1));
                g.apply(wk::AND, vec![a, b])
            }
            3 | 4 => {
                let (a, b) = (self.prop(g, r, depth - 1), self.prop(g, r, depth - 1));
                g.apply(wk::OR, vec![a, b])
            }
            5 => {
                let (a, b) = (self.prop(g, r, depth - 1), self.prop(g, r, depth - 1));
                g.apply(wk::IMPLIES, vec![a, b])
            }
            6 => {
                let v = g.fresh();
                let body = self.prop(g, r, depth - 1);
                let binder = if r.below(2) == 0 { wk::FORALL } else { wk::EXISTS };
                // One case in four quantifies over an *integer* domain, which is
                // the only route into `infinite()` and the tail abstraction. The
                // generator used to pass `Some(self.domain)` every time, and
                // `Mixed::extension` answers `Some` for it — so the header's
                // claim to catch the tail-abstraction flip was reached zero
                // times in 2400 evaluations.
                let dom = match r.below(4) {
                    0 => wk::NAT_TYPE,
                    1 => wk::INT_TYPE,
                    _ => self.domain,
                };
                g.quantify(binder, v, Some(dom), body)
            }
            11 => {
                // A comparison over the bound variable, so the linear fragment
                // has something to decide when the domain is unbounded.
                let v = g.fresh();
                let k = g.int(r.below(200) as i64);
                let body = if r.below(2) == 0 {
                    g.apply(wk::LEQ, vec![v, k])
                } else {
                    g.apply(wk::LEQ, vec![k, v])
                };
                let binder = if r.below(2) == 0 { wk::FORALL } else { wk::EXISTS };
                let dom = if r.below(2) == 0 { wk::NAT_TYPE } else { wk::INT_TYPE };
                g.quantify(binder, v, Some(dom), body)
            }
            7 => {
                let inner = self.prop(g, r, depth - 1);
                g.apply(wk::USUALLY, vec![inner])
            }
            8 => {
                let v = g.fresh();
                let body = self.prop(g, r, depth - 1);
                let counted =
                    g.bind(wk::COUNT, vec![Binding { var: v, domain: Some(self.domain) }], vec![body]);
                let n = g.int(r.below(4) as i64);
                if r.below(2) == 0 {
                    g.apply(wk::LEQ, vec![counted, n])
                } else {
                    g.apply(wk::LEQ, vec![n, counted])
                }
            }
            9 => {
                let inner = self.prop(g, r, depth - 1);
                let op = r.pick(&[wk::NECESSARILY, wk::POSSIBLY, wk::ALWAYS, wk::EVENTUALLY]);
                g.apply(op, vec![inner])
            }
            10 => {
                let inner = self.prop(g, r, depth - 1);
                let t = g.int(r.below(4) as i64);
                g.apply(wk::AT, vec![t, inner])
            }
            _ => self.atom(g, r),
        }
    }

    fn atom(&self, g: &mut ObjectGraph, r: &mut Rng) -> ObjectId {
        match r.below(8) {
            0 => wk::TOP,
            1 => wk::BOT,
            _ => {
                let p = r.pick(&self.preds);
                let a = r.pick(&self.consts);
                g.apply(p, vec![a])
            }
        }
    }
}

fn fixture(seed: u64) -> (ObjectGraph, Gen, Mixed, Rng) {
    let mut g = ObjectGraph::new();
    let mut r = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(seed));
    let preds: Vec<ObjectId> = ["p", "q", "s"].iter().map(|n| g.atom(n)).collect();
    let consts: Vec<ObjectId> = ["a", "b", "c"].iter().map(|n| g.atom(n)).collect();
    let domain = g.atom("D");
    let world = g.atom("w");

    // A scattering of facts, denials and members — different per seed, so the
    // properties are checked against many shapes of partial knowledge.
    let mut holds = Vec::new();
    let mut denied = Vec::new();
    for p in &preds {
        for c in &consts {
            match r.below(4) {
                0 => holds.push((*p, vec![*c])),
                1 => denied.push((*p, vec![*c])),
                _ => {}
            }
        }
    }
    let members = consts[..1 + r.below(3) as usize].to_vec();
    let complete = r.below(2) == 0;

    // A default on record for one atom, and a rule concluding one predicate, so
    // `Derivation::Default` and `derive`'s status accumulation are both
    // reachable. Measured: 2368 entries to `infinite()`, 117399 to `derive`'s
    // accumulation, 17 defeasible results across 2400 evaluations. These are
    // load-bearing rhetoric — the paragraph exists because the previous figures
    // were fabricated zeros — so they get re-measured when the evaluator moves.
    let defaults = vec![g.apply(preds[0], vec![consts[0]])];
    let x = g.fresh();
    let antecedent = g.apply(preds[1], vec![x]);
    let consequent = g.apply(preds[2], vec![x]);
    let imp = g.apply(wk::IMPLIES, vec![antecedent, consequent]);
    let rule = g.quantify(wk::FORALL, x, None, imp);
    let rules = vec![(preds[2], rule)];

    let s = Mixed { holds, denied, rules, defaults, domain, members, complete, world };
    (g, Gen { preds, consts, domain }, s, r)
}

/// The information order: `must` grows and `may` shrinks. `Certain` on a side
/// may never become less than `Certain`, and a side that was `Partial` may not
/// drop to `None`.
fn no_information_lost(before: &EvaluationResult, after: &EvaluationResult) -> bool {
    after.support >= before.support && after.refutation >= before.refutation
}

// -------------------------------------------------------------- properties

/// **More budget never moves the answer down the information order.**
///
/// An answer that flips from `Refuted/Exact` to `Supported/Exact` when you
/// think longer is not an answer, and nothing in the row-by-row suite was
/// positioned to notice.
#[test]
fn more_budget_never_retracts_a_conclusion() {
    for seed in 0..400u64 {
        let (mut g, shapes, s, mut r) = fixture(seed);
        let term = shapes.prop(&mut g, &mut r, 3);
        let text = print(&g, term);

        let ev = GraphEvaluator::new();
        let mut prev: Option<EvaluationResult> = None;
        for budget in [8u64, 32, 128, 512, 1_500, 5_000] {
            let now = ev.eval(&mut g, term, &s, budget);
            if let Some(before) = &prev {
                assert!(
                    no_information_lost(before, &now),
                    "seed {seed}, budget {budget}: information went backwards on {text}\n  \
                     before: {:?}/{:?}  after: {:?}/{:?}",
                    before.support,
                    before.refutation,
                    now.support,
                    now.refutation
                );
                // The stronger half: a definite answer is never revised.
                if before.is_definite() {
                    assert_eq!(
                        before.evidential(),
                        now.evidential(),
                        "seed {seed}: a definite verdict changed with budget on {text}"
                    );
                }
            }
            prev = Some(now);
        }
    }
}

/// **A continuation is never stronger than the question that produced it.**
///
/// `narrow` rebuilt the unexamined members as a `set`, which is exhaustive by
/// construction — so suspending and resuming was a soundness upgrade, and a
/// universal that was honestly `Open` came back `Supported/Exact` from its own
/// continuation.
#[test]
fn resuming_never_concludes_more_than_the_whole_query() {
    let ev = GraphEvaluator::new();
    for seed in 0..400u64 {
        let (mut g, shapes, s, mut r) = fixture(seed);
        let term = shapes.prop(&mut g, &mut r, 3);
        let text = print(&g, term);

        let whole = ev.eval(&mut g, term, &s, 5_000);
        for budget in [4u64, 12, 40, 100] {
            let cut = ev.eval(&mut g, term, &s, budget);
            let Some(k) = cut.continuation else { continue };
            let resumed = ev.eval(&mut g, k, &s, 5_000);
            assert!(
                no_information_lost(&resumed, &whole),
                "seed {seed}, budget {budget}: the continuation of {text} concluded more \
                 than the query itself\n  whole: {:?}/{:?}  resumed: {:?}/{:?}",
                whole.support,
                whole.refutation,
                resumed.support,
                resumed.refutation
            );
        }
    }
}

/// **Classically identical spellings agree.**
///
/// `implies` negated its *result* while leaving the cycle detector's parity at
/// the outer value, so `(not (implies P #false))` and `(not (not P))` — the same
/// sentence — could get different answers. Checked over generated `P`, and
/// against the cyclic case that actually broke.
#[test]
fn a_sentence_does_not_depend_on_its_spelling() {
    let ev = GraphEvaluator::new();
    for seed in 0..300u64 {
        let (mut g, shapes, s, mut r) = fixture(seed);
        let p = shapes.prop(&mut g, &mut r, 2);

        // ¬¬P, twice.
        let double_not = {
            let inner = g.apply(wk::NOT, vec![p]);
            g.apply(wk::NOT, vec![inner])
        };
        let via_implies = {
            let inner = g.apply(wk::IMPLIES, vec![p, wk::BOT]);
            g.apply(wk::NOT, vec![inner])
        };
        let a = ev.eval(&mut g, double_not, &s, 5_000);
        let b = ev.eval(&mut g, via_implies, &s, 5_000);
        assert_eq!(
            (a.support, a.refutation),
            (b.support, b.refutation),
            "seed {seed}: ¬¬P disagreed with ¬(P → ⊥) on {}",
            print(&g, p)
        );

        // …and `P ∨ Q` against `¬P → Q`.
        let q = shapes.prop(&mut g, &mut r, 2);
        let disj = g.apply(wk::OR, vec![p, q]);
        let implication = {
            let np = g.apply(wk::NOT, vec![p]);
            g.apply(wk::IMPLIES, vec![np, q])
        };
        let a = ev.eval(&mut g, disj, &s, 5_000);
        let b = ev.eval(&mut g, implication, &s, 5_000);
        assert_eq!(
            (a.support, a.refutation),
            (b.support, b.refutation),
            "seed {seed}: P ∨ Q disagreed with ¬P → Q"
        );
    }
}

/// **Round-tripping preserves what an expression *does*, over generated atom
/// names rather than seven hand-picked ones.**
///
/// `escape_atom` bars an atom only when it parses as a `BigInt`, so an atom
/// named `1d2` printed bare and reparsed as a `Decimal`, and `0xff` as `Bytes`.
/// Text equality still held — which is exactly what §3 says text equality is too
/// weak to catch, in the section describing this bug as fixed.
#[test]
fn round_tripping_generated_atoms_preserves_evaluation() {
    let ev = GraphEvaluator::new();
    let alphabet = ['a', 'z', '0', '1', '9', 'd', 'x', 'e', '-', '+', '.', '_', 'f'];
    for seed in 0..600u64 {
        let mut r = Rng(seed.wrapping_mul(0x2545F4914F6CDD1D).wrapping_add(7));
        let len = 1 + r.below(4) as usize;
        let name: String = (0..len).map(|_| r.pick(&alphabet)).collect();

        let mut g = ObjectGraph::new();
        let a = g.atom(&name);
        let p = g.atom("p");
        let term = g.apply(p, vec![a]);
        let s = MapGraphStructure::new().fact(p, vec![a]);

        let text = print(&g, term);
        let before = ev.eval(&mut g, term, &s, 10_000);

        let mut fresh = ObjectGraph::new();
        let back = parse(&mut fresh, &text)
            .unwrap_or_else(|e| panic!("atom {name:?} printed as {text} and would not reparse: {e}"));
        // The reparsed graph must still contain an *atom* with that name — not a
        // literal that merely prints the same way.
        let kids = fresh.children(back);
        let arg = kids.get(1).copied().unwrap_or(back);
        assert!(
            matches!(
                fresh.get(arg),
                Some(artist_logic::object::CoreNode::Atom { name: Some(n) }) if *n == name
            ),
            "atom {name:?} printed as {text} and came back as {:?}",
            fresh.get(arg)
        );

        let after = ev.eval(&mut fresh, back, &s, 10_000);
        assert_eq!(
            (before.evidential(), before.compute_status),
            (after.evidential(), after.compute_status),
            "round trip changed what {text} means"
        );
    }
}

/// **Nothing claims `Exact` while carrying an unresolved residual it could act
/// on**, and nothing reports a definite verdict on both sides at once — the two
/// invariants `is_definite` exists to protect.
#[test]
fn a_definite_answer_is_one_sided_and_exact() {
    let ev = GraphEvaluator::new();
    for seed in 0..500u64 {
        let (mut g, shapes, s, mut r) = fixture(seed);
        let term = shapes.prop(&mut g, &mut r, 3);
        let out = ev.eval(&mut g, term, &s, 5_000);
        if out.is_definite() {
            assert_eq!(out.compute_status, ComputeStatus::Exact, "seed {seed}");
            assert_ne!(
                out.evidential(),
                Evidential::Conflicted,
                "seed {seed}: a contradiction is not a definite reading"
            );
            assert_ne!(out.evidential(), Evidential::Open, "seed {seed}");
        }
        // `Unsupported` means no more budget will help, so it must never come
        // with a definite verdict attached.
        if out.compute_status == ComputeStatus::Unsupported {
            assert!(!out.is_definite(), "seed {seed}: unsupported yet definite");
        }
    }
}

/// **Negating twice is the identity on both axes.** `negate` swaps the bounds,
/// and a swap that lost the compute status would launder an absence.
#[test]
fn double_negation_is_the_identity_on_both_axes() {
    let ev = GraphEvaluator::new();
    for seed in 0..300u64 {
        let (mut g, shapes, s, mut r) = fixture(seed);
        let term = shapes.prop(&mut g, &mut r, 3);
        let neg = g.apply(wk::NOT, vec![term]);
        let neg2 = g.apply(wk::NOT, vec![neg]);

        let a = ev.eval(&mut g, term, &s, 5_000);
        let b = ev.eval(&mut g, neg2, &s, 5_000);
        assert_eq!(
            (a.support, a.refutation, a.compute_status),
            (b.support, b.refutation, b.compute_status),
            "seed {seed}: ¬¬ changed {}",
            print(&g, term)
        );
    }
}

/// The literal forms that must not be swallowed by the atom printer, called out
/// by name because they are the ones that were.
#[test]
fn the_literal_shaped_atom_names_survive() {
    let ev = GraphEvaluator::new();
    for name in ["1d2", "0xff", "12d3", "-4d1", "0x", "1d", "42", "-7", "0b1"] {
        let mut g = ObjectGraph::new();
        let a = g.atom(name);
        let p = g.atom("p");
        let term = g.apply(p, vec![a]);
        let s = MapGraphStructure::new().fact(p, vec![a]);

        let text = print(&g, term);
        let mut fresh = ObjectGraph::new();
        let back = parse(&mut fresh, &text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let after = ev.eval(&mut fresh, back, &s, 10_000);
        assert_eq!(
            after.evidential(),
            Evidential::Supported,
            "an atom named {name:?} printed as {text} and stopped being that atom"
        );
    }
}

/// A `Decimal` literal is still a decimal, and a `Bytes` literal still bytes —
/// the escaping must not go so wide that it breaks the real literals.
#[test]
fn quoting_atoms_does_not_break_real_literals() {
    let mut g = ObjectGraph::new();
    let holder = g.atom("h");
    let d = g.lit(LiteralValue::Decimal { mantissa: BigInt::from(-12345), scale: 3 });
    let b = g.lit(LiteralValue::Bytes(vec![0, 255]));
    let i = g.lit(LiteralValue::Int(BigInt::from(-7)));
    let term = g.apply(holder, vec![d, b, i]);

    let text = print(&g, term);
    let mut fresh = ObjectGraph::new();
    let back = parse(&mut fresh, &text).expect("literals reparse");
    assert_eq!(print(&fresh, back), text);

    let kids = fresh.children(back);
    assert!(matches!(
        fresh.get(kids[1]),
        Some(artist_logic::object::CoreNode::Literal(LiteralValue::Decimal { scale: 3, .. }))
    ));
    assert!(matches!(
        fresh.get(kids[2]),
        Some(artist_logic::object::CoreNode::Literal(LiteralValue::Bytes(_)))
    ));
}

/// Defeasibility is carried, not rounded off, through every connective.
#[test]
fn defeasibility_survives_every_connective() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let hedged = g.apply(wk::USUALLY, vec![claim]);
    let s = MapGraphStructure::new().fact(wk::USUALLY, vec![claim]);
    let ev = GraphEvaluator::new();

    let base = ev.eval(&mut g, hedged, &s, 20_000);
    assert_eq!(base.derivation, artist_logic::evidence::Derivation::Default);

    for wrap in [wk::AND, wk::OR, wk::NOT, wk::USUALLY] {
        let node = g.apply(wrap, vec![hedged]);
        let r = ev.eval(&mut g, node, &s, 20_000);
        assert_eq!(
            r.derivation,
            artist_logic::evidence::Derivation::Default,
            "a one-element {wrap:?} lost its operand's defeasibility"
        );
    }
}
