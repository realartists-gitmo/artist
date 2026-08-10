use std::collections::BTreeMap;

use artist_cognition::empirical::{
    CertificationClass, EXACT, InferenceArtifact, InferenceCertification, InferenceError,
    ProcedureRun, RunId,
};
use artist_cognition::formal::{
    GraphBuilder, InterpretedGraph, ObjectInterpretation, ObjectMeaning, ObjectNode, Ontology,
    OntologyParameter, OntologySymbolDeclaration, OntologySymbolId, OntologyTypeDeclaration,
    OntologyTypeExpr, OntologyTypeId,
};
use artist_cognition::kernel::{
    Certificate, CheckSession, Context, CoreGraphCodec, ElaborationRole, Kernel, KernelRequest,
    KernelResult, OntologyCompiler, Provenance, SessionStatus, Term, Theory, TheoryBuilder,
};

#[test]
fn complete_deductive_path_roundtrips_and_resumes() {
    let mut builder = TheoryBuilder::new("integration", "1");
    builder
        .axiom("Nat", Term::universe(0), Provenance::new("fixture"))
        .unwrap();
    builder
        .axiom("zero", Term::constant("Nat"), Provenance::new("fixture"))
        .unwrap();
    builder
        .define(
            "identity",
            Term::pi(Term::constant("Nat"), Term::constant("Nat")),
            Term::lam(Term::constant("Nat"), Term::var(0)),
            Provenance::new("fixture"),
        )
        .unwrap();
    let theory = builder.finish();

    let proof = Term::app(Term::constant("identity"), Term::constant("zero"));
    let graph = CoreGraphCodec::encode(&proof).unwrap();
    assert_eq!(CoreGraphCodec::decode(&graph).unwrap(), proof);

    let certificate = Certificate::new(&theory, Context::new(), Term::constant("Nat"), proof);
    let mut session = Kernel::start(
        theory.clone(),
        KernelRequest::Certificate {
            certificate: certificate.clone(),
        },
    );
    assert!(matches!(
        session.run_slice(1),
        SessionStatus::Running { .. }
    ));
    let serialized = serde_json::to_vec(&session).unwrap();
    let mut resumed: CheckSession = serde_json::from_slice(&serialized).unwrap();
    let terminal = loop {
        match resumed.run_slice(1) {
            SessionStatus::Running { .. } => {}
            terminal => break terminal,
        }
    };
    assert!(matches!(
        terminal,
        SessionStatus::Accepted {
            result: KernelResult::Certified,
            ..
        }
    ));
    assert!(
        certificate
            .dependencies(&theory)
            .dependencies
            .iter()
            .any(|dependency| dependency.name.as_str() == "identity")
    );
}

fn exact_claim_submission() -> InterpretedGraph {
    let proposition = OntologyTypeId::from("Proposition");
    let quantity = OntologyTypeId::from("Quantity");
    let value = OntologyTypeId::from("Value");
    let exact = OntologySymbolId::from(EXACT);
    let quantity_symbol = OntologySymbolId::from("fixture.quantity");
    let value_symbol = OntologySymbolId::from("fixture.value");

    let ontology = Ontology {
        namespace: "test.empirical".into(),
        version: "1".into(),
        proposition_type: proposition.clone(),
        types: BTreeMap::from([
            (
                proposition.clone(),
                OntologyTypeDeclaration {
                    id: proposition.clone(),
                    universe: 1,
                    description: None,
                },
            ),
            (
                quantity.clone(),
                OntologyTypeDeclaration {
                    id: quantity.clone(),
                    universe: 0,
                    description: None,
                },
            ),
            (
                value.clone(),
                OntologyTypeDeclaration {
                    id: value.clone(),
                    universe: 0,
                    description: None,
                },
            ),
        ]),
        symbols: BTreeMap::from([
            (
                exact.clone(),
                OntologySymbolDeclaration {
                    id: exact.clone(),
                    parameters: vec![
                        OntologyParameter {
                            role: "quantity".into(),
                            ty: OntologyTypeExpr::named(quantity.clone()),
                        },
                        OntologyParameter {
                            role: "value".into(),
                            ty: OntologyTypeExpr::named(value.clone()),
                        },
                    ],
                    result: OntologyTypeExpr::named(proposition.clone()),
                    description: None,
                },
            ),
            (
                quantity_symbol.clone(),
                OntologySymbolDeclaration {
                    id: quantity_symbol.clone(),
                    parameters: Vec::new(),
                    result: OntologyTypeExpr::named(quantity.clone()),
                    description: None,
                },
            ),
            (
                value_symbol.clone(),
                OntologySymbolDeclaration {
                    id: value_symbol.clone(),
                    parameters: Vec::new(),
                    result: OntologyTypeExpr::named(value.clone()),
                    description: None,
                },
            ),
        ]),
    };

    let mut graph = GraphBuilder::new();
    let exact_object = graph.insert("exact", ObjectNode::new());
    let quantity_object = graph.insert("quantity", ObjectNode::new());
    let value_object = graph.insert("value", ObjectNode::new());
    let claim = graph.insert(
        "claim",
        ObjectNode::new()
            .with_operator(exact_object.clone())
            .with_edge("quantity", quantity_object.clone())
            .with_edge("value", value_object.clone()),
    );
    graph.root("main", claim.clone());

    InterpretedGraph {
        graph: graph.finish().unwrap(),
        ontology,
        interpretations: BTreeMap::from([
            (
                exact_object,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::function(
                        OntologyTypeExpr::named(quantity.clone()),
                        OntologyTypeExpr::function(
                            OntologyTypeExpr::named(value.clone()),
                            OntologyTypeExpr::named(proposition.clone()),
                        ),
                    ),
                    meaning: ObjectMeaning::Symbol { id: exact },
                },
            ),
            (
                quantity_object,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(quantity),
                    meaning: ObjectMeaning::Symbol {
                        id: quantity_symbol,
                    },
                },
            ),
            (
                value_object,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(value),
                    meaning: ObjectMeaning::Symbol { id: value_symbol },
                },
            ),
            (
                claim,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(proposition),
                    meaning: ObjectMeaning::Application {
                        arguments: vec!["quantity".into(), "value".into()],
                    },
                },
            ),
        ]),
    }
}

#[test]
fn empirical_status_requires_a_certificate_for_the_exact_interpreted_claim() {
    let submission = exact_claim_submission();
    submission.validate().unwrap();
    let compiled =
        OntologyCompiler::compile(Theory::empty("empirical-integration", "1"), &submission)
            .unwrap();
    let reference = submission.reference("main").unwrap();
    let claim_root = reference.root.clone();
    let elaboration = OntologyCompiler::elaborate(
        &submission,
        &compiled,
        &claim_root,
        ElaborationRole::Proposition,
    )
    .unwrap();

    let mut builder = TheoryBuilder::from_theory(compiled.theory.clone()).unwrap();
    builder
        .axiom(
            "fixture.exact-proof",
            elaboration.term.clone(),
            Provenance::new("fixture exact empirical fact"),
        )
        .unwrap();
    let theory = builder.finish();
    let certificate = Certificate::new(
        &theory,
        Context::new(),
        elaboration.term.clone(),
        Term::constant("fixture.exact-proof"),
    );

    let procedure = ProcedureRun {
        id: RunId("run".into()),
        procedure: reference.clone(),
        configuration: reference.clone(),
        seed: None,
        limits: reference.clone(),
        diagnostics: None,
    };
    let heuristic = InferenceArtifact {
        claim: reference.clone(),
        procedure: procedure.clone(),
        certification: None,
    };
    assert_eq!(
        heuristic
            .verify(&submission, &compiled, &compiled.theory)
            .unwrap(),
        CertificationClass::Heuristic
    );

    let certified = InferenceArtifact {
        claim: reference,
        procedure,
        certification: Some(InferenceCertification {
            dependencies: certificate.dependencies(&theory),
            elaboration,
            certificate,
        }),
    };
    assert_eq!(
        certified.verify(&submission, &compiled, &theory).unwrap(),
        CertificationClass::Exact
    );

    let mut unrelated = certified;
    unrelated
        .certification
        .as_mut()
        .unwrap()
        .certificate
        .proposition = Term::universe(0);
    assert_eq!(
        unrelated.verify(&submission, &compiled, &theory),
        Err(InferenceError::CertificateMismatch)
    );
}

#[test]
fn theory_identity_blocks_cross_version_certificates() {
    let mut first = TheoryBuilder::new("same-name", "1");
    first
        .axiom("P", Term::universe(0), Provenance::new("first"))
        .unwrap();
    first
        .axiom("p", Term::constant("P"), Provenance::new("first"))
        .unwrap();
    let first = first.finish();

    let mut second = TheoryBuilder::new("same-name", "1");
    second
        .axiom("P", Term::universe(0), Provenance::new("second"))
        .unwrap();
    second
        .axiom("p", Term::constant("P"), Provenance::new("second"))
        .unwrap();
    let second = second.finish();

    let certificate = Certificate::new(
        &first,
        Context::new(),
        Term::constant("P"),
        Term::constant("p"),
    );
    assert!(Kernel::run_to_completion(second, KernelRequest::Certificate { certificate }).is_err());
}
