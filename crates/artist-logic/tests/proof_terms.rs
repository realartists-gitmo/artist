//! A derivation is an object in the same universe as what it derives.
//!
//! §5.6 says the answer to *"why do you believe this"* becomes "an object the
//! memory can store, transmit, and re-check". `Certificate` was a Rust struct
//! with no term encoding and no serialisation, so that held only for a caller
//! inside the same process: restart, and the reason was gone while the
//! conclusion remained.
//!
//! Encoded, it is queryable, which is the part that pays. *"Which conclusions
//! rest on the assertion I am about to retract"* is a question about a graph,
//! and a certificate that is not in the graph cannot be asked it.

use artist_logic::certificate::{Certificate, Step};
use artist_logic::graph_eval::{GraphEvaluator, MapGraphStructure};
use artist_logic::object::wk;
use artist_logic::{ObjectGraph, ObjectId};

/// Round trip, on a certificate the evaluator actually produced.
#[test]
fn an_emitted_certificate_survives_the_round_trip() {
    let mut g = ObjectGraph::new();
    let (p, q, a, adam) = (g.atom("p"), g.atom("q"), g.atom("a"), g.atom("adam"));
    let (pa, qa) = (g.apply(p, vec![a]), g.apply(q, vec![a]));
    let both = g.apply(wk::AND, vec![pa, qa]);
    let s = MapGraphStructure::new()
        .fact(p, vec![a])
        .fact(q, vec![a])
        .attributed(pa, vec![adam])
        .attributed(qa, vec![adam]);

    let (r, cert) = GraphEvaluator::new().eval_traced(&mut g, both, &s, 100_000);
    assert!(!cert.steps.is_empty());

    let term = cert.to_term(&mut g);
    let back = Certificate::from_term(&g, term).expect("decodes");
    assert_eq!(back, cert, "every field, including the ones the checker rejects on");

    // And the decoded one still convinces the kernel of the same thing.
    let a1 = cert.check(&g).expect("valid");
    let a2 = back.check(&g).expect("still valid");
    assert_eq!(a1, a2);
    assert!(back.justifies(&g, both, &r));
}

/// Every step shape survives, not only the ones an easy query happens to emit.
/// A shape that decoded wrongly would produce a *valid-looking* certificate for
/// a different claim, which is worse than failing to decode.
#[test]
fn every_step_shape_round_trips() {
    let mut g = ObjectGraph::new();
    let (p, a, rule, src) = (g.atom("p"), g.atom("a"), g.atom("r"), g.atom("s"));
    let pa = g.apply(p, vec![a]);
    let cert = Certificate {
        steps: vec![
            Step::Told { node: pa, holds: true, sources: vec![src] },
            Step::Told { node: pa, holds: false, sources: Vec::new() },
            Step::Axiom { node: wk::TOP, holds: true },
            Step::Tautology { node: pa, holds: true, constructive: false },
            Step::Negation { node: pa, premise: 0 },
            Step::Connective { node: pa, premises: vec![0, 1], holds: true },
            Step::Instance { node: pa, premise: 2, value: a, instance: pa, holds: false },
            Step::Exhaustive {
                node: pa,
                premises: vec![(0, a, pa), (1, p, pa)],
                holds: true,
                complete: true,
            },
            Step::Rule { node: pa, rule, premises: vec![3, 4], defeasible: true },
            Step::Conflict { node: pa, sources: vec![src, a] },
            Step::Ungrounded { node: pa, oscillating: true },
            Step::Unestablished { node: pa },
        ],
    };
    let term = cert.to_term(&mut g);
    assert_eq!(Certificate::from_term(&g, term).as_ref(), Some(&cert));
}

/// A term that is not a proof decodes to nothing — **never to a partial
/// certificate**, which would be one the kernel rejects for the wrong reason.
#[test]
fn a_non_proof_term_decodes_to_nothing() {
    let mut g = ObjectGraph::new();
    let (p, a) = (g.atom("p"), g.atom("a"));
    let pa = g.apply(p, vec![a]);
    assert!(Certificate::from_term(&g, pa).is_none());

    // A proof whose *steps* are malformed is rejected whole.
    let bogus = g.apply(wk::STEP, vec![wk::BY_AXIOM]);
    let proof = g.apply(wk::PROOF, vec![bogus]);
    assert!(Certificate::from_term(&g, proof).is_none());

    // An unknown step kind is not silently skipped either.
    let kind = g.atom("by-vibes");
    let step = g.apply(wk::STEP, vec![kind, pa]);
    let proof = g.apply(wk::PROOF, vec![step]);
    assert!(Certificate::from_term(&g, proof).is_none());
}

/// The encoding is content-addressed like everything else, so the same
/// derivation in two graphs is the same object — which is what lets a
/// certificate be transmitted rather than merely printed.
#[test]
fn the_same_derivation_is_the_same_object_in_any_graph() {
    let build = |g: &mut ObjectGraph| {
        let (p, a) = (g.atom("p"), g.atom("a"));
        let pa = g.apply(p, vec![a]);
        let cert = Certificate {
            steps: vec![Step::Told { node: pa, holds: true, sources: vec![ObjectId(7)] }],
        };
        cert.to_term(g)
    };
    let (mut g1, mut g2) = (ObjectGraph::new(), ObjectGraph::new());
    assert_eq!(build(&mut g1), build(&mut g2));
}
