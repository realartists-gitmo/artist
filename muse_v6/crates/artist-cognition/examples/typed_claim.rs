use std::collections::BTreeMap;

use artist_cognition::{CertifiedGraphClaim, ProofProductionRecord};
use artist_formal::{
    GraphBuilder, InterpretedGraph, ObjectInterpretation, ObjectMeaning, ObjectNode, Ontology,
    OntologyParameter, OntologySymbolDeclaration, OntologySymbolId, OntologyTypeDeclaration,
    OntologyTypeExpr, OntologyTypeId,
};
use artist_kernel::{
    Certificate, Context, ElaborationRole, OntologyCompiler, Provenance, Term, Theory,
    TheoryBuilder,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proposition = OntologyTypeId::from("Proposition");
    let person = OntologyTypeId::from("Person");
    let object = OntologyTypeId::from("Object");
    let alice = OntologySymbolId::from("alice");
    let car = OntologySymbolId::from("car-7");
    let owns = OntologySymbolId::from("owns");

    let ontology = Ontology {
        namespace: "example.ownership".into(),
        version: "1".into(),
        proposition_type: proposition.clone(),
        types: BTreeMap::from([
            (
                proposition.clone(),
                OntologyTypeDeclaration {
                    id: proposition.clone(),
                    universe: 1,
                    description: Some("propositions as kernel types".into()),
                },
            ),
            (
                person.clone(),
                OntologyTypeDeclaration {
                    id: person.clone(),
                    universe: 0,
                    description: None,
                },
            ),
            (
                object.clone(),
                OntologyTypeDeclaration {
                    id: object.clone(),
                    universe: 0,
                    description: None,
                },
            ),
        ]),
        symbols: BTreeMap::from([
            (
                alice.clone(),
                OntologySymbolDeclaration {
                    id: alice.clone(),
                    parameters: vec![],
                    result: OntologyTypeExpr::named(person.clone()),
                    description: None,
                },
            ),
            (
                car.clone(),
                OntologySymbolDeclaration {
                    id: car.clone(),
                    parameters: vec![],
                    result: OntologyTypeExpr::named(object.clone()),
                    description: None,
                },
            ),
            (
                owns.clone(),
                OntologySymbolDeclaration {
                    id: owns.clone(),
                    parameters: vec![
                        OntologyParameter {
                            role: "owner".into(),
                            ty: OntologyTypeExpr::named(person.clone()),
                        },
                        OntologyParameter {
                            role: "owned".into(),
                            ty: OntologyTypeExpr::named(object.clone()),
                        },
                    ],
                    result: OntologyTypeExpr::named(proposition.clone()),
                    description: None,
                },
            ),
        ]),
    };

    let mut graph = GraphBuilder::new();
    let owns_node = graph.insert("owns", ObjectNode::new());
    let alice_node = graph.insert("alice", ObjectNode::new());
    let car_node = graph.insert("car", ObjectNode::new());
    let claim = graph.insert(
        "claim",
        ObjectNode::new()
            .with_operator(owns_node.clone())
            .with_edge("owner", alice_node.clone())
            .with_edge("owned", car_node.clone()),
    );
    graph.root("main", claim.clone());

    let submission = InterpretedGraph {
        graph: graph.finish()?,
        ontology,
        interpretations: BTreeMap::from([
            (
                owns_node,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::function(
                        OntologyTypeExpr::named(person.clone()),
                        OntologyTypeExpr::function(
                            OntologyTypeExpr::named(object.clone()),
                            OntologyTypeExpr::named(proposition.clone()),
                        ),
                    ),
                    meaning: ObjectMeaning::Symbol { id: owns },
                },
            ),
            (
                alice_node,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(person),
                    meaning: ObjectMeaning::Symbol { id: alice },
                },
            ),
            (
                car_node,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(object),
                    meaning: ObjectMeaning::Symbol { id: car },
                },
            ),
            (
                claim.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(proposition),
                    meaning: ObjectMeaning::Application {
                        arguments: vec!["owner".into(), "owned".into()],
                    },
                },
            ),
        ]),
    };

    let compiled = OntologyCompiler::compile(Theory::empty("example", "1"), &submission)?;
    let elaboration =
        OntologyCompiler::elaborate(&submission, &compiled, &claim, ElaborationRole::Proposition)?;

    let mut proof_theory = TheoryBuilder::from_theory(compiled.theory.clone())?;
    proof_theory.axiom(
        "example.fact/alice-owns-car",
        elaboration.term.clone(),
        Provenance::new("example explicit fact"),
    )?;
    let proof_theory = proof_theory.finish();
    let certificate = Certificate::new(
        &proof_theory,
        Context::new(),
        elaboration.term.clone(),
        Term::constant("example.fact/alice-owns-car"),
    );
    let answer = CertifiedGraphClaim {
        claim,
        elaboration,
        dependencies: certificate.dependencies(&proof_theory),
        certificate,
        production: Some(ProofProductionRecord::External {
            identity: "example".into(),
            version: "1".into(),
        }),
    };
    answer.verify(&submission, &compiled, &proof_theory)?;
    Ok(())
}
