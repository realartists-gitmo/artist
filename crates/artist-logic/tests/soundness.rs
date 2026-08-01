//! §7.1's soundness inequality, over generated structures rather than chosen ones.
//!
//! > For every `M ⊨ Γ`: `must(n) ≤_k ⟦n⟧_M ≤_k may(n)`.
//!
//! Examples can only refute a universally quantified claim, never establish it —
//! which is why four audits kept finding a twenty-sixth defect after twenty-five
//! were fixed. This file does not establish it either. What it does is make the
//! claim *executable*: a reference implementation of `⟦·⟧_M` transcribed straight
//! from the semantics, a generator of structures and terms, and the inequality
//! checked on every one. A violation is a counterexample the next auditor does
//! not have to hand-build.
//!
//! It has already earned its keep twice. It refuted §4.1's monotonicity lemma on
//! its first run, and it found the evaluator certifying classical tautologies
//! under a Belnap semantics — both recorded below.

use artist_logic::ObjectGraph;
use artist_logic::evidence::{Bound, DeterminacyBasis, EvaluationResult};
use artist_logic::graph_eval::{GraphEvaluator, MapGraphStructure};
use artist_logic::object::{CoreNode, ObjectId, wk};

/// Belnap's FOUR (§3). There is no fifth value here: `⊥` is the *absence* of an
/// entry in a partial valuation, which is what §4.2 corrected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Four {
    N,
    T,
    F,
    B,
}

impl Four {
    const ALL: [Four; 4] = [Four::N, Four::T, Four::F, Four::B];

    /// `≤_k`: `N <_k T <_k B`, `N <_k F <_k B`.
    fn le_k(self, other: Four) -> bool {
        use Four::*;
        matches!(
            (self, other),
            (N, _) | (_, B) | (T, T) | (F, F)
        )
    }

    fn not(self) -> Four {
        match self {
            Four::T => Four::F,
            Four::F => Four::T,
            v => v,
        }
    }

    /// Meet in the truth order `F <_t N <_t T`, `F <_t B <_t T`. `N` and `B` are
    /// truth-incomparable and their meet is `F`.
    fn and(self, other: Four) -> Four {
        use Four::*;
        match (self, other) {
            (F, _) | (_, F) => F,
            (T, v) | (v, T) => v,
            (N, B) | (B, N) => F,
            (v, _) => v,
        }
    }

    fn or(self, other: Four) -> Four {
        use Four::*;
        match (self, other) {
            (T, _) | (_, T) => T,
            (F, v) | (v, F) => v,
            (N, B) | (B, N) => T,
            (v, _) => v,
        }
    }

    /// Designated: established. Validity quantifies over this, not over `T`.
    fn designated(self) -> bool {
        matches!(self, Four::T | Four::B)
    }

    /// Arieli–Avron's strong implication (§3). Material `¬a ∨ b` leaves `P → P`
    /// at `N` equal to `N`, emptying the tautology set of a logic that is
    /// supposed to carry rules.
    fn implies(self, other: Four) -> Four {
        if self.designated() { other } else { Four::T }
    }
}

/// A **total** valuation of the atoms, and the pure FOUR evaluation under it.
fn eval_total(g: &ObjectGraph, n: ObjectId, atoms: &[ObjectId], vals: &[Four], fuel: u32) -> Four {
    if fuel == 0 {
        return Four::N;
    }
    if n == wk::TOP {
        return Four::T;
    }
    if n == wk::BOT {
        return Four::F;
    }
    if let Some(i) = atoms.iter().position(|a| *a == n) {
        return vals[i];
    }
    match g.get(n) {
        Some(CoreNode::Apply { operator, operands }) => {
            let ops: Vec<Four> =
                operands.iter().map(|o| eval_total(g, *o, atoms, vals, fuel - 1)).collect();
            match (*operator, ops.as_slice()) {
                (wk::NOT, [a]) => a.not(),
                (wk::AND, xs) if !xs.is_empty() => xs.iter().copied().reduce(Four::and).unwrap(),
                (wk::OR, xs) if !xs.is_empty() => xs.iter().copied().reduce(Four::or).unwrap(),
                (wk::IMPLIES, [a, b]) => a.implies(*b),
                _ => Four::N,
            }
        }
        _ => Four::N,
    }
}

/// `Φ_M` at a node, per §4.2: defined with value `c` iff **every total extension
/// of the partial valuation** gives `c`.
///
/// Quantifying over extensions of the *whole* valuation, not each argument
/// separately, is what correlates repeated occurrences of one atom — and is
/// exactly why `P ∨ ¬P` comes out undefined rather than `T`.
fn denote(g: &ObjectGraph, n: ObjectId, atoms: &[ObjectId], partial: &[Option<Four>]) -> Option<Four> {
    denote_over(g, n, atoms, partial, false)
}

/// As above, but `bivalent` restricts the completions to `{T, F}` — the
/// structures a presumption of bivalence quantifies over.
fn denote_over(
    g: &ObjectGraph,
    n: ObjectId,
    atoms: &[ObjectId],
    partial: &[Option<Four>],
    bivalent: bool,
) -> Option<Four> {
    let choices: &[Four] = if bivalent { &[Four::T, Four::F] } else { &Four::ALL };
    let open: Vec<usize> = partial.iter().enumerate().filter(|(_, v)| v.is_none()).map(|(i, _)| i).collect();
    let mut seen: Option<Four> = None;
    for mask in 0..choices.len().pow(open.len() as u32) {
        let mut vals: Vec<Four> = partial.iter().map(|v| v.unwrap_or(Four::N)).collect();
        let mut m = mask;
        for i in &open {
            vals[*i] = choices[m % choices.len()];
            m /= choices.len();
        }
        let got = eval_total(g, n, atoms, &vals, 24);
        match seen {
            None => seen = Some(got),
            Some(prev) if prev == got => {}
            Some(_) => return None,
        }
    }
    seen
}

/// §7.2's nine cells. Only `Certain` constrains the structure — `support`
/// asserts `⟦n⟧ ∈ {T,B}` and `refutation` asserts `⟦n⟧ ∈ {F,B}`. `Partial`
/// reports the search and asserts nothing, which is what makes every `Partial`
/// cell trivially sound.
fn interval(r: &EvaluationResult) -> (Option<Four>, Vec<Four>) {
    match (r.support == Bound::Certain, r.refutation == Bound::Certain) {
        (true, true) => (Some(Four::B), vec![Four::B]),
        (true, false) => (Some(Four::T), vec![Four::T, Four::B]),
        (false, true) => (Some(Four::F), vec![Four::F, Four::B]),
        (false, false) => (None, Four::ALL.to_vec()),
    }
}

/// Deterministic, so a failure is reproducible. A property test with an
/// unreproducible failure has thrown away its whole value.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }
}

fn gen_term(g: &mut ObjectGraph, rng: &mut Lcg, atoms: &[ObjectId], depth: u32) -> ObjectId {
    if depth == 0 || rng.pick(3) == 0 {
        return atoms[rng.pick(atoms.len())];
    }
    let a = gen_term(g, rng, atoms, depth - 1);
    let b = gen_term(g, rng, atoms, depth - 1);
    match rng.pick(4) {
        0 => g.apply(wk::NOT, vec![a]),
        1 => g.apply(wk::AND, vec![a, b]),
        2 => g.apply(wk::OR, vec![a, b]),
        _ => g.apply(wk::IMPLIES, vec![a, b]),
    }
}

/// **§4.2's monotonicity lemma**, which is the one Knaster–Tarski is earned from.
///
/// The order is information extension on partial valuations — *how much is
/// known* — not a value ordering. Revision 2 of the spec used the latter and this
/// test refuted it: `or(⊥,T) = T` while `or(⊥,B) = ⊥`, and `T ≤_k B`, so a
/// value-ordering monotonicity would demand `T ≤_k ⊥`.
#[test]
fn phi_is_monotone_under_information_extension() {
    let mut g = ObjectGraph::new();
    let atoms: Vec<ObjectId> = ["p", "q"].iter().map(|n| g.atom(n)).collect();
    let mut rng = Lcg(0xf00d);
    let mut terms = Vec::new();
    for _ in 0..60 {
        terms.push(gen_term(&mut g, &mut rng, &atoms, 3));
    }

    // Every partial valuation, and every ⊑-extension of it.
    let states: Vec<Option<Four>> = std::iter::once(None).chain(Four::ALL.map(Some)).collect();
    for a0 in &states {
        for a1 in &states {
            let v = [*a0, *a1];
            for b0 in &states {
                for b1 in &states {
                    let w = [*b0, *b1];
                    // v ⊑ w: w agrees wherever v is defined.
                    let extends = v
                        .iter()
                        .zip(w.iter())
                        .all(|(x, y)| x.is_none() || x == y);
                    if !extends {
                        continue;
                    }
                    for t in &terms {
                        if let Some(c) = denote(&g, *t, &atoms, &v) {
                            assert_eq!(
                                denote(&g, *t, &atoms, &w),
                                Some(c),
                                "Φ lost a determined value when the valuation grew: \
                                 {v:?} ⊑ {w:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// **The soundness property.** The evaluator's interval must bracket the
/// denotation, over generated structures and generated terms.
#[test]
fn evaluation_brackets_the_denotation() {
    let mut rng = Lcg(0x5eed_1234);

    for case in 0..300 {
        let mut g = ObjectGraph::new();
        let args = [g.atom("a"), g.atom("b")];
        let preds: Vec<ObjectId> = ["p", "q"].iter().map(|n| g.atom(n)).collect();

        let mut s = MapGraphStructure::new();
        let mut atoms = Vec::new();
        let mut partial = Vec::new();
        // A closed predicate refutes what it does not list; an open one leaves it
        // `N`. That reaches `T`, `F` and `N`. `B` is not expressible through this
        // structure's API and so is not generated — a coverage gap, stated.
        for p in &preds {
            let closed = rng.pick(2) == 0;
            if closed {
                s = s.closed(*p);
            }
            for a in &args {
                let node = g.apply(*p, vec![*a]);
                atoms.push(node);
                if rng.pick(2) == 0 {
                    s = s.fact(*p, vec![*a]);
                    partial.push(Some(Four::T));
                } else if closed {
                    partial.push(Some(Four::F));
                } else {
                    partial.push(Some(Four::N));
                }
            }
        }

        let mut pool = atoms.clone();
        pool.push(wk::TOP);
        pool.push(wk::BOT);
        let term = gen_term(&mut g, &mut rng, &pool, 3);

        let r = GraphEvaluator::new().eval(&mut g, term, &s, 100_000);

        // **Γ can include bivalence, and §7.1 quantifies over `M ⊨ Γ`.** The
        // evaluator has a tier that reads the classical table; its answers hold
        // in structures where every atom is `T` or `F`, and it records that by
        // marking the determinacy basis `Presumed`. Checking such a result
        // against an `N`-valued reference would be checking it against a
        // structure it never claimed to cover.
        let bivalent = r.determinacy_basis == DeterminacyBasis::Presumed;
        let effective: Vec<Option<Four>> = if bivalent {
            // Under the presumption, an atom the store left open is *some*
            // classical value; the reference agrees only when both agree.
            partial.iter().map(|v| if *v == Some(Four::N) { None } else { *v }).collect()
        } else {
            partial.clone()
        };
        let truth = match denote_over(&g, term, &atoms, &effective, bivalent) {
            Some(t) => t,
            // Undetermined even under the presumption: the evaluator may say
            // anything an interval permits, so there is nothing to check.
            None => continue,
        };
        let (must, may) = interval(&r);

        if let Some(must) = must {
            assert!(
                must.le_k(truth),
                "case {case}: must={must:?} is not ≤_k the denotation {truth:?} \
                 (support={:?} refutation={:?})",
                r.support,
                r.refutation
            );
        }
        assert!(
            may.contains(&truth),
            "case {case}: denotation {truth:?} outside may={may:?} \
             (support={:?} refutation={:?})",
            r.support,
            r.refutation
        );
    }
}

/// The worked values §4.2 keeps, so the doc and the reference cannot drift.
#[test]
fn the_worked_values_hold() {
    let mut g = ObjectGraph::new();
    let p = g.atom("p");
    let atoms = [p];
    let or_top = g.apply(wk::OR, vec![p, wk::TOP]);
    let or_bot = g.apply(wk::OR, vec![p, wk::BOT]);
    let and_bot = g.apply(wk::AND, vec![p, wk::BOT]);
    let excluded = {
        let np = g.apply(wk::NOT, vec![p]);
        g.apply(wk::OR, vec![p, np])
    };

    assert_eq!(denote(&g, or_top, &atoms, &[None]), Some(Four::T), "⊥ ∨ T = T");
    assert_eq!(denote(&g, or_bot, &atoms, &[None]), None, "⊥ ∨ F is undefined");
    assert_eq!(denote(&g, and_bot, &atoms, &[None]), Some(Four::F), "a false conjunct decides");
    // The one that matters most: a *classical* tautology is not valid in FOUR.
    assert_eq!(
        denote(&g, excluded, &atoms, &[Some(Four::N)]),
        Some(Four::N),
        "P ∨ ¬P at N is N ∨ N = N — enumerating Boolean masks certifies a falsehood here"
    );
}
