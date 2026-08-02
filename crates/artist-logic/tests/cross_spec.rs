//! Invariants that span `docs/semantics.md`, `docs/formalism.md` and the code.
//!
//! Every defect this suite exists for was a claim that was true in a
//! *neighbouring* logic and asserted in this one: Boolean tautologies under
//! Belnap, intuitionistic validity treated as unconditional, material
//! implication where the semantics says strong. Each survived because the three
//! artifacts drifted independently and nothing compared them.
//!
//! These are **direct** tests over the whole of FOUR rather than more random
//! sampling. `soundness.rs` generates structures and can only report that
//! *something* is wrong; a generator also cannot tell you which document is the
//! liar. These name the claim, the section, and the value that breaks it.

use artist_logic::ObjectGraph;
use artist_logic::certificate::{Certificate, Step};
use artist_logic::evidence::{Determinacy, DeterminacyBasis, Evidential};
use artist_logic::graph_eval::{EmptyStructure, GraphEvaluator, MapGraphStructure};
use artist_logic::object::{ObjectId, wk};

/// Build a structure and a proposition node for each of Belnap's four values.
///
/// Returns `(graph, [(value-name, node)], structure)`. `B` is reachable only
/// because `MapGraphStructure::conflicting` exists — it did not for most of this
/// suite's life, and the one soundness bug the property test missed lived there.
fn four_valued() -> (ObjectGraph, Vec<(&'static str, ObjectId)>, MapGraphStructure) {
    let mut g = ObjectGraph::new();
    let a = g.atom("a");
    let (t, f, n, b) = (g.atom("t"), g.atom("f"), g.atom("n"), g.atom("b"));
    let s = MapGraphStructure::new()
        .fact(t, vec![a])
        .closed(f)
        .conflicting(b, vec![a]);
    let nodes = vec![
        ("T", g.apply(t, vec![a])),
        ("F", g.apply(f, vec![a])),
        ("N", g.apply(n, vec![a])),
        ("B", g.apply(b, vec![a])),
    ];
    (g, nodes, s)
}

fn ev(g: &mut ObjectGraph, node: ObjectId, s: &MapGraphStructure) -> Evidential {
    GraphEvaluator::new().eval(g, node, s, 100_000).evidential()
}

/// The fixture reaches all four. If this fails, every test below is weaker than
/// it looks and the failure would otherwise be silent.
#[test]
fn the_fixture_reaches_every_belnap_value() {
    let (mut g, nodes, s) = four_valued();
    let got: Vec<Evidential> = nodes.iter().map(|(_, n)| ev(&mut g, *n, &s)).collect();
    assert_eq!(
        got,
        vec![
            Evidential::Supported,
            Evidential::Refuted,
            Evidential::Open,
            Evidential::Conflicted
        ],
        "T, F, N, B — a suite blind to one of these is not testing this logic"
    );
}

/// **`→` is the strong implication, over all sixteen operand pairs.**
///
/// semantics §3, formalism §5.2's transfer row. `a ⊃ b = b` when `a` is
/// designated, `T` otherwise — *never* `¬a ∨ b`. The two differ at `a ∈ {N, B}`,
/// and at `B` material is **unsound**: `¬B ∨ F = B` is designated while
/// `B ⊃ F = F` is not, so a refuted implication read as supported.
///
/// The evaluator cannot always *establish* the strong value — "A is not
/// designated" needs `⟦A⟧ ∈ {N,F}`, which two bounds cannot express — so what is
/// asserted here is the sound direction: **it never claims more than the truth.**
#[test]
fn implication_is_never_stronger_than_the_strong_reading() {
    let (mut g, nodes, s) = four_valued();
    for (an, a) in &nodes {
        for (bn, b) in &nodes {
            let imp = g.apply(wk::IMPLIES, vec![*a, *b]);
            let got = ev(&mut g, imp, &s);
            // ⟦a ⊃ b⟧ under §3.
            let truth = match (*an, *bn) {
                ("T" | "B", _) => *bn,
                _ => "T",
            };
            let designated = matches!(truth, "T" | "B");
            let anti = matches!(truth, "F" | "B");
            if got == Evidential::Supported || got == Evidential::Conflicted {
                assert!(designated, "({an} → {bn}) claimed support; truth is {truth}");
            }
            if got == Evidential::Refuted || got == Evidential::Conflicted {
                assert!(anti, "({an} → {bn}) claimed refutation; truth is {truth}");
            }
        }
    }
}

/// The material shortcut is gone in the one place it was unsound: a conflicted
/// antecedent with a refuted consequent. `¬B ∨ F = B` — designated — while
/// `B ⊃ F = F`. This is the case `MapGraphStructure` could not build.
#[test]
fn a_conflicted_antecedent_does_not_support_an_implication() {
    let (mut g, nodes, s) = four_valued();
    let b = nodes.iter().find(|(k, _)| *k == "B").unwrap().1;
    let f = nodes.iter().find(|(k, _)| *k == "F").unwrap().1;
    let imp = g.apply(wk::IMPLIES, vec![b, f]);
    assert_ne!(
        ev(&mut g, imp, &s),
        Evidential::Supported,
        "materially this was Supported, and materially it was wrong"
    );
}

/// **There is no intuitionistic tier.** formalism §5.2 lists two, not three.
///
/// `¬(P ∧ ¬P)` is provable in G4ip and is `N` at `N`, so certifying it
/// unconditionally was unsound — and the presumption the tier claimed ("no atom
/// is `both`") is satisfied by `N`, so it never licensed what it certified.
#[test]
fn non_contradiction_is_not_unconditionally_valid() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let npa = g.apply(wk::NOT, vec![pa]);
    let conj = g.apply(wk::AND, vec![pa, npa]);
    let nc = g.apply(wk::NOT, vec![conj]);

    let r = GraphEvaluator::new().eval(&mut g, nc, &EmptyStructure, 100_000);
    if r.evidential() == Evidential::Supported {
        assert_eq!(
            r.determinacy_basis,
            DeterminacyBasis::Presumed,
            "it needs bivalence, so it must say so"
        );
        assert_ne!(r.determinacy, Determinacy::Total, "and must not claim established totality");
    }
}

/// …while `P → P` *is* unconditional, and for the right reason: it is valid in
/// FOUR itself under the strong implication, presuming nothing. The two coming
/// apart is the whole content of deleting the tier.
#[test]
fn self_implication_is_unconditional() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let identity = g.apply(wk::IMPLIES, vec![pa, pa]);

    let r = GraphEvaluator::new().eval(&mut g, identity, &EmptyStructure, 100_000);
    assert_eq!(r.evidential(), Evidential::Supported);
    assert_eq!(r.determinacy_basis, DeterminacyBasis::Derived, "nothing presumed");
    assert_eq!(r.determinacy, Determinacy::Total);
}

/// Excluded middle stays invalid. Adopting the strong implication was not a way
/// back to classical logic, and this is the assertion that pins the difference.
#[test]
fn excluded_middle_is_still_not_valid() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    let npa = g.apply(wk::NOT, vec![pa]);
    let lem = g.apply(wk::OR, vec![pa, npa]);

    let r = GraphEvaluator::new().eval(&mut g, lem, &EmptyStructure, 100_000);
    if r.evidential() == Evidential::Supported {
        assert_eq!(r.determinacy_basis, DeterminacyBasis::Presumed, "`N ∨ N = N`");
    }
}

/// **`Γ = ∅` is a property of the derivation, not a privilege of `Axiom`.**
///
/// semantics §7.1 said "`Axiom` alone" for two revisions, in three places. §8.5
/// reads an empty `Γ` as the *claim* that a judgment holds in every structure,
/// so a rule wrongly excluded from producing one is a rule whose unconditional
/// conclusions get read as unattributed instead.
#[test]
fn unconditionality_is_a_property_of_gamma() {
    let mut g = ObjectGraph::new();
    let not_top = g.apply(wk::NOT, vec![wk::TOP]);
    let both = g.apply(wk::AND, vec![wk::TOP, wk::TOP]);

    for (what, steps) in [
        ("axiom", vec![Step::Axiom { node: wk::TOP, holds: true }]),
        (
            "negation over an axiom",
            vec![
                Step::Axiom { node: wk::TOP, holds: true },
                Step::Negation { node: not_top, premise: 0 },
            ],
        ),
        (
            "connective over axioms",
            vec![
                Step::Axiom { node: wk::TOP, holds: true },
                Step::Connective { node: both, premises: vec![(0, 0), (0, 1)], holds: true },
            ],
        ),
    ] {
        let c = Certificate { steps }.check(&g).expect("valid");
        assert!(c.hypotheses.is_empty(), "{what} consults no testimony");
        assert!(
            c.authorities.is_empty() && c.assumed.is_empty(),
            "{what}: an empty Γ partitions into two empty lists"
        );
    }
}

/// **`usually`, `unless` and `prefer` are evaluator behaviour without certified
/// semantics**, and the kernel must not pretend otherwise.
///
/// semantics §11.3: two fixpoint constructions for the combined
/// grounding/argumentation operator were proposed and both refuted, so `R` and
/// `≺` sit in `M` uninterpreted. A `Step` for a default shipped once on the
/// invalid proof and was retracted. This test fails the moment one comes back
/// without the theorem.
#[test]
fn no_kernel_rule_certifies_a_default() {
    let mut g = ObjectGraph::new();
    let (p, e, a, adam) = (g.atom("p"), g.atom("e"), g.atom("a"), g.atom("adam"));
    let (pa, ea) = (g.apply(p, vec![a]), g.apply(e, vec![a]));

    for node in [
        g.apply(wk::UNLESS, vec![ea, pa]),
        g.apply(wk::USUALLY, vec![pa]),
        g.apply(wk::PREFER, vec![pa, ea]),
    ] {
        // The most favourable shape available: the body proved outright, the
        // exception refuted outright. Still not certifiable.
        let attempt = Certificate {
            steps: vec![
                Step::Told { node: pa, holds: true, sources: vec![adam] },
                Step::Told { node: ea, holds: false, sources: vec![adam] },
                Step::Connective { node, premises: vec![(0, 1)], holds: true },
            ],
        };
        assert!(
            attempt.check(&g).is_err(),
            "a default has no denotation yet, so it has no rule"
        );
    }
}

/// The evaluator still *answers* about defaults — they are evaluable, just not
/// certifiable. Losing that would be a capability regression dressed as rigour.
#[test]
fn a_default_still_evaluates() {
    let mut g = ObjectGraph::new();
    let (p, e, a) = (g.atom("p"), g.atom("e"), g.atom("a"));
    let (pa, ea) = (g.apply(p, vec![a]), g.apply(e, vec![a]));
    let hedged = g.apply(wk::UNLESS, vec![ea, pa]);
    let s = MapGraphStructure::new().fact(p, vec![a]);

    let r = GraphEvaluator::new().eval(&mut g, hedged, &s, 100_000);
    assert_eq!(r.evidential(), Evidential::Supported, "the default holds, unrefuted");
    assert!(!r.defeated_by.is_empty(), "and it names what would overturn it");
}

// ---------------------------------------------------------------------------
// Outstanding soundness debt, from adversarial review of the nine rules.
//
// `⟦·⟧` assigns a node one *complete* FOUR value or none; the bounds reason
// about the two evidence bits independently. A support bit can be fixed across
// every completion while the complete value varies between `T` and `B` — the
// semantics says `⊥`, the certificate says `Certain`, and `Certain` excludes
// `⊥`. `Instance` and `Instantiate` are refuted by countermodel; `Connective`
// needs a side condition it does not have.
//
// **The repair chosen is semantics.md §7.1b: give `⟦·⟧` independently partial
// components.** So these tests assert what *that* repair requires — which is
// mostly that the evaluator's existing answers become correct rather than
// merely reported. An earlier draft of this block asserted the opposite,
// encoding the repair that was *not* chosen; that is recorded because writing a
// regression for the wrong branch of a decision is its own failure mode.
//
// They run under `cargo test -- --ignored` and fail until the judgment type can
// express a support bit that is *established zero* rather than merely unknown.
// ---------------------------------------------------------------------------

/// A truth-teller: `Q := (holds (quote Q))`, whose bits are both undefined.
fn truth_teller(g: &mut ObjectGraph) -> ObjectId {
    let q = g.alloc();
    let quoted = g.apply(wk::QUOTE, vec![q]);
    let holds = g.apply(wk::HOLDS, vec![quoted]);
    let body = g.get(holds).cloned().expect("built");
    g.define(q, body);
    q
}

/// **`Bound::None` conflates an established zero with an unknown.**
///
/// This is the whole repair, and the other items follow from it. All four FOUR
/// values are already distinguishable as pairs — `T` is `(Certain, None)`, `F` is
/// `(None, Certain)`, `N` is `(None, None)`, `B` is `(Certain, Certain)`. What is
/// *not* distinguishable is the support bit of `F`, which is **established 0**,
/// from the support bit of a truth-teller, which is **unknown**: both report
/// `None`.
///
/// It shows up wherever a clause reads `¬a⁺`. `(a ⊃ b)⁺ = ¬a⁺ ∨ b⁺`, so a
/// node whose support bit is a known zero supports any implication over it,
/// while an unknown one supports nothing — and the evaluator cannot tell them
/// apart, so it must treat both as unknown and loses the first.
///
/// It is also the precondition `defeasibility.md` §6 names: a default defeated
/// by a *conflicted* exception is exactly the case that separates `⟦E⟧ ∈ {N,F}`
/// from `⟦E⟧ ∈ {F,B}`, and it cannot be stated until this lands.
#[test]
fn an_established_zero_is_distinguishable_from_an_unknown() {
    let (mut g, nodes, s) = four_valued();
    let f = nodes.iter().find(|(k, _)| *k == "F").unwrap().1;
    let n = nodes.iter().find(|(k, _)| *k == "N").unwrap().1;
    let q = truth_teller(&mut g);

    // `F` has support bit 0; the truth-teller's is undefined. Under §7.1b the
    // implications over them must differ — `F ⊃ N = T`, while `Q ⊃ N` is
    // undetermined on both bits.
    let over_f = g.apply(wk::IMPLIES, vec![f, n]);
    let over_q = g.apply(wk::IMPLIES, vec![q, n]);
    assert_ne!(
        ev(&mut g, over_f, &s),
        ev(&mut g, over_q, &s),
        "a known-zero support bit and an unknown one cannot license the same answer"
    );
}

/// With an established zero available, the false-antecedent row returns.
///
/// `(a ⊃ b)⁺ = ¬a⁺ ∨ b⁺`, so `a⁺ = 0` gives support outright. That row was
/// sacrificed when the strong implication landed, precisely because
/// `refutation: Certain` admits `B`; the repair recovers it rather than trading
/// it away permanently.
#[test]
fn a_failing_antecedent_supports_an_implication_again() {
    let (mut g, nodes, s) = four_valued();
    let f = nodes.iter().find(|(k, _)| *k == "F").unwrap().1;
    let n = nodes.iter().find(|(k, _)| *k == "N").unwrap().1;
    let imp = g.apply(wk::IMPLIES, vec![f, n]);
    assert_eq!(
        ev(&mut g, imp, &s),
        Evidential::Supported,
        "F ⊃ N = T: the antecedent is established not-designated"
    );
}

/// **A settled bit alongside an ungrounded node is now consistent.**
///
/// `(or B Q)` reports `support: Certain` *and* `grounding: StableLoop`. Under the
/// old exact-value semantics those contradicted each other — `Certain` asserted
/// `⟦n⟧ ∈ {T,B}` and `StableLoop` asserted `⟦n⟧ = ⊥` — and the evaluator was
/// emitting the pair anyway.
///
/// Under §7.1b it is simply the truth: the support bit is `1` because `B ∨ x` is
/// designated for every `x`, and the refutation bit is undefined because
/// `1 ∧ f_Q` is. `Grounded` means *both* bits settled, so the node is not
/// grounded and the support bound is still exact. An earlier draft of this test
/// asserted the grounding must therefore change, which was the repair-1 reading:
/// there is nothing wrong with the grounding, and there never was.
#[test]
fn a_settled_bit_may_sit_on_an_ungrounded_node() {
    use artist_logic::evidence::{Bound, Grounding};
    let mut g = ObjectGraph::new();
    let (b, a) = (g.atom("b"), g.atom("a"));
    let ba = g.apply(b, vec![a]);
    let q = truth_teller(&mut g);
    let s = MapGraphStructure::new().conflicting(b, vec![a]);

    let disj = g.apply(wk::OR, vec![ba, q]);
    let r = GraphEvaluator::new().eval(&mut g, disj, &s, 100_000);
    assert_eq!(r.support, Bound::Certain, "B ∨ x is designated for every x");
    assert!(!r.refutation.is_settled(), "and the refutation bit is not settled");
    assert_eq!(r.grounding, Grounding::StableLoop, "so the node is not grounded");
}
