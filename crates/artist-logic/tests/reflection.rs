//! The four axes, as questions the language can ask.
//!
//! The evaluator has computed grounding, derivation, determinacy and the
//! defeater set on every query since those axes landed, and no term could ask
//! for any of them. A memory that works out its own epistemic status and cannot
//! write any of it down has the status for its own benefit — which is precisely
//! backwards for a system whose job is to be interrogated later.
//!
//! The subtle half of this is that a claim *about* a sentence is not the
//! sentence. `(grounded L)` for the liar must come back a perfectly grounded
//! `Refuted`; if reflection inherited its argument's axes the answer would be
//! ungrounded, and asking whether something is broken would break in the same
//! way. `provable` did inherit them, for as long as it has existed.

use artist_logic::evidence::{Bound, ComputeStatus, Determinacy, Evidential, Grounding};
use artist_logic::graph_eval::{
    EmptyStructure, GraphEvaluator, GraphStructure, Knowledge, MapGraphStructure,
};
use artist_logic::object::{wk, Binding};
use artist_logic::ObjectGraph;

fn ask(g: &mut ObjectGraph, node: artist_logic::ObjectId, s: &dyn GraphStructure) -> artist_logic::evidence::EvaluationResult {
    GraphEvaluator::new().eval(g, node, s, 100_000)
}

/// The liar has no stable value. The *report* that it has no stable value is an
/// ordinary, perfectly grounded fact — and it is now sayable.
#[test]
fn a_report_about_an_ungrounded_sentence_is_grounded() {
    let mut g = ObjectGraph::new();
    let liar = g.alloc();
    let q = g.apply(wk::QUOTE, vec![liar]);
    let h = g.apply(wk::HOLDS, vec![q]);
    let n = g.apply(wk::NOT, vec![h]);
    let body = g.get(n).cloned().expect("built");
    g.define(liar, body);

    let direct = ask(&mut g, liar, &EmptyStructure);
    assert_ne!(direct.grounding, Grounding::Grounded, "the liar itself is ungrounded");

    let q = g.apply(wk::GROUNDED, vec![liar]);
    let r = ask(&mut g, q, &EmptyStructure);
    assert_eq!(r.evidential(), Evidential::Refuted, "…and that is a fact about it");
    assert_eq!(
        r.grounding,
        Grounding::Grounded,
        "the report does not catch what it reports on"
    );
}

/// An ordinary atom nobody has an opinion about is *grounded* — grounding is
/// about reaching a value, not about having one recorded. Conflating the two
/// would make every unknown fact look like a liar sentence.
#[test]
fn absence_of_evidence_is_not_ungroundedness() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let q = g.apply(wk::GROUNDED, vec![claim]);
    assert_eq!(ask(&mut g, q, &EmptyStructure).evidential(), Evidential::Supported);
}

/// A conclusion reached through a defeater is defeasible and *names* what would
/// defeat it; the observed fact underneath it is neither. Both are now askable,
/// and — the part that matters — so is the defeater itself.
#[test]
fn defeasibility_and_its_defeaters_are_askable() {
    let mut g = ObjectGraph::new();
    let (p, a, except) = (g.atom("p"), g.atom("a"), g.atom("except"));
    let claim = g.apply(p, vec![a]);
    let exception = g.apply(except, vec![a]);
    let hedged = g.apply(wk::UNLESS, vec![exception, claim]);
    let s = MapGraphStructure::new().fact(p, vec![a]);

    let q = g.apply(wk::DEFEASIBLE, vec![hedged]);
    assert_eq!(
        ask(&mut g, q, &s).evidential(),
        Evidential::Supported,
        "it holds only until the exception does"
    );

    let q = g.apply(wk::DEFEASIBLE, vec![claim]);
    assert_eq!(
        ask(&mut g, q, &s).evidential(),
        Evidential::Refuted,
        "the fact underneath it is not a default"
    );

    // **"What would change my mind about this?"** — computed on every query
    // since the axes landed, and until now unaskable.
    let q = g.apply(wk::DEFEATED_BY, vec![hedged, exception]);
    assert_eq!(ask(&mut g, q, &s).evidential(), Evidential::Supported);

    let irrelevant = g.apply(except, vec![p]);
    let q = g.apply(wk::DEFEATED_BY, vec![hedged, irrelevant]);
    assert_eq!(
        ask(&mut g, q, &s).evidential(),
        Evidential::Refuted,
        "and something that would not change it is answered too"
    );
}

/// **Derivation classifies a conclusion, so with no conclusion there is nothing
/// to classify.** `Derivation::Observed` is the struct default; reading it off
/// an `Open` result would answer "not defeasible" about a question that was
/// never answered — the manufacture-a-verdict-from-absence defect, one level up.
#[test]
fn defeasibility_of_an_unanswered_question_is_not_no() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let q = g.apply(wk::DEFEASIBLE, vec![claim]);
    let r = ask(&mut g, q, &EmptyStructure);
    assert_eq!(r.evidential(), Evidential::Open);
    assert_eq!(r.compute_status, ComputeStatus::Stalled);
}

/// `(defeaters P)` as a domain: the set is enumerable and countable, not a Rust
/// field. This is what makes *"what would change my mind about X"* a question
/// rather than an aspiration.
#[test]
fn the_defeater_set_is_a_domain() {
    let mut g = ObjectGraph::new();
    let (p, a, except) = (g.atom("p"), g.atom("a"), g.atom("except"));
    let claim = g.apply(p, vec![a]);
    let exception = g.apply(except, vec![a]);
    let hedged = g.apply(wk::UNLESS, vec![exception, claim]);
    let s = MapGraphStructure::new().fact(p, vec![a]);

    // `count` is a binder, not an application — the defeaters of `hedged` are a
    // domain to range over, which is the whole claim being made here.
    let d = g.apply(wk::DEFEATERS_DOMAIN, vec![hedged]);
    let v = g.fresh();
    let n = g.bind(wk::COUNT, vec![Binding { var: v, domain: Some(d) }], vec![wk::TOP]);
    let one = g.int(1);
    let q = g.apply(wk::EQ, vec![n, one]);
    assert_eq!(
        ask(&mut g, q, &s).evidential(),
        Evidential::Supported,
        "exactly one thing would overturn it, and the store can count it"
    );
}

/// Totality declared, totality denied, and totality nobody has spoken to — three
/// different answers, and the third is not "no".
#[test]
fn determinacy_distinguishes_denied_from_unspoken() {
    struct Declaring {
        sharp: artist_logic::ObjectId,
        vague: artist_logic::ObjectId,
    }
    impl GraphStructure for Declaring {
        fn known(&self, _: artist_logic::ObjectId, _: &[artist_logic::ObjectId]) -> Knowledge {
            Knowledge::Holds
        }
        fn determinacy(&self, proposition: artist_logic::ObjectId) -> Determinacy {
            if proposition == self.sharp {
                Determinacy::Total
            } else if proposition == self.vague {
                Determinacy::Indeterminate
            } else {
                Determinacy::Unknown
            }
        }
    }

    let mut g = ObjectGraph::new();
    let (heap, tall, plain) = (g.atom("heap"), g.atom("tall"), g.atom("plain"));
    let n = g.int(10_000);
    let sharp = g.apply(heap, vec![n]);
    let vague = g.apply(tall, vec![n]);
    let quiet = g.apply(plain, vec![n]);
    let s = Declaring { sharp, vague };

    let q = g.apply(wk::DETERMINATE, vec![sharp]);
    assert_eq!(ask(&mut g, q, &s).evidential(), Evidential::Supported);

    let q = g.apply(wk::DETERMINATE, vec![vague]);
    assert_eq!(ask(&mut g, q, &s).evidential(), Evidential::Refuted);

    let q = g.apply(wk::DETERMINATE, vec![quiet]);
    let r = ask(&mut g, q, &s);
    assert_eq!(r.evidential(), Evidential::Open, "nobody has said, which is not a no");
    assert_eq!(r.compute_status, ComputeStatus::Stalled);

    // And *whether we are taking totality on faith* is itself askable — the one
    // question that distinguishes an ordinary query from a certification.
    let q = g.apply(wk::PRESUMED, vec![quiet]);
    assert_eq!(ask(&mut g, q, &s).evidential(), Evidential::Supported);
    let q = g.apply(wk::PRESUMED, vec![sharp]);
    assert_eq!(ask(&mut g, q, &s).evidential(), Evidential::Refuted);
}

/// `Bound::Partial` finally has a producer. A sharp condition that exists and is
/// simply unrecorded is neither a yes nor a no.
#[test]
fn an_underspecified_condition_is_partial() {
    struct Underspecified(artist_logic::ObjectId);
    impl GraphStructure for Underspecified {
        fn known(&self, _: artist_logic::ObjectId, _: &[artist_logic::ObjectId]) -> Knowledge {
            Knowledge::Holds
        }
        fn determinacy(&self, p: artist_logic::ObjectId) -> Determinacy {
            if p == self.0 { Determinacy::Underspecified } else { Determinacy::Unknown }
        }
    }
    let mut g = ObjectGraph::new();
    let (complex, m) = (g.atom("too-complex"), g.atom("mod1"));
    let claim = g.apply(complex, vec![m]);
    let q = g.apply(wk::DETERMINATE, vec![claim]);
    let r = ask(&mut g, q, &Underspecified(claim));
    assert_eq!(r.support, Bound::Partial);
    assert_eq!(r.refutation, Bound::None);
}

/// Quoted and bare agree. A stored claim looks like the first and a query looks
/// like the second, and answering them differently would be a distinction drawn
/// by spelling.
#[test]
fn reflection_sees_through_a_quotation() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let claim = g.apply(p, vec![a]);
    let quoted = g.apply(wk::QUOTE, vec![claim]);
    let bare = g.apply(wk::GROUNDED, vec![claim]);
    let wrapped = g.apply(wk::GROUNDED, vec![quoted]);
    let s = MapGraphStructure::new().fact(p, vec![a]);
    assert_eq!(ask(&mut g, bare, &s).evidential(), ask(&mut g, wrapped, &s).evidential());
}
