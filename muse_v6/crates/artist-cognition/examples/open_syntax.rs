use std::collections::BTreeMap;

use artist_cognition::formal::{
    GraphBuilder, InterpretedGraph, Literal, ObjectInterpretation, ObjectMeaning, ObjectNode,
    Ontology, OntologyTypeDeclaration, OntologyTypeExpr, OntologyTypeId,
};
use artist_cognition::kernel::{ElaborationRole, OntologyCompiler, Theory};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proposition = OntologyTypeId::from("Proposition");
    let ontology = Ontology {
        namespace: "example.open-syntax".into(),
        version: "1".into(),
        proposition_type: proposition.clone(),
        types: BTreeMap::from([(
            proposition.clone(),
            OntologyTypeDeclaration {
                id: proposition,
                universe: 1,
                description: None,
            },
        )]),
        symbols: BTreeMap::new(),
    };

    let mut graph = GraphBuilder::new();
    let self_reference = graph.insert("self", ObjectNode::new());
    let unknown_operator = graph.insert(
        "operator",
        ObjectNode::new().with_property("name", Literal::Text("future/truth".into())),
    );
    *graph.node_mut(&self_reference).expect("inserted node") = ObjectNode::new()
        .with_operator(unknown_operator.clone())
        .with_edge("quoted", self_reference.clone());
    graph.root("main", self_reference.clone());

    let submission = InterpretedGraph {
        graph: graph.finish()?,
        ontology,
        interpretations: BTreeMap::from([
            (
                self_reference.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::QuotedObject,
                    meaning: ObjectMeaning::Quoted,
                },
            ),
            (
                unknown_operator,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::QuotedObject,
                    meaning: ObjectMeaning::Quoted,
                },
            ),
        ]),
    };

    submission.validate()?;
    let compiled = OntologyCompiler::compile(Theory::empty("example", "1"), &submission)?;
    let quoted = OntologyCompiler::elaborate(
        &submission,
        &compiled,
        &self_reference,
        ElaborationRole::QuotedObject,
    )?;
    println!("submission: {}", submission.canonical_hash());
    println!("quoted term: {}", quoted.term);
    Ok(())
}
