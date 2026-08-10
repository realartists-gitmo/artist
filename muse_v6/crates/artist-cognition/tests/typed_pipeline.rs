use std::collections::BTreeMap;

use artist_cognition::{
    ArtifactContract, ArtifactEnvelope, CertifiedGraphClaim, CertifiedQueryWitness,
    CognitiveAnswer, CognitiveQuery, CognitiveQueryError, Continuation, DiscoveryContinuation,
    KernelContinuation, QueryId, QueryPayload, QueryPolicy, QueryRequest, QueryStatus,
    QueryVariable, QueryWitness,
};
use artist_formal::{
    GraphBuilder, InterpretedGraph, ObjectId, ObjectInterpretation, ObjectMeaning, ObjectNode,
    Ontology, OntologyParameter, OntologySymbolDeclaration, OntologySymbolId,
    OntologyTypeDeclaration, OntologyTypeExpr, OntologyTypeId,
};
use artist_kernel::{
    Certificate, CompiledOntologySubmission, Context, ElaborationRole, Kernel, KernelRequest,
    OntologyCompiler, Provenance, Term, Theory, TheoryBuilder,
};

#[derive(Clone)]
struct OwnershipFixture {
    submission: InterpretedGraph,
    claim: ObjectId,
    person_condition: ObjectId,
    person_claim: ObjectId,
    alice: ObjectId,
    car: ObjectId,
    person_type: ObjectId,
}

fn ownership_fixture() -> OwnershipFixture {
    let proposition = OntologyTypeId::from("Proposition");
    let person = OntologyTypeId::from("Person");
    let object = OntologyTypeId::from("Object");
    let alice = OntologySymbolId::from("alice");
    let car = OntologySymbolId::from("car");
    let owns = OntologySymbolId::from("owns");
    let is_person = OntologySymbolId::from("is_person");

    let ontology = Ontology {
        namespace: "test.ownership".into(),
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
                    parameters: Vec::new(),
                    result: OntologyTypeExpr::named(person.clone()),
                    description: None,
                },
            ),
            (
                car.clone(),
                OntologySymbolDeclaration {
                    id: car.clone(),
                    parameters: Vec::new(),
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
            (
                is_person.clone(),
                OntologySymbolDeclaration {
                    id: is_person.clone(),
                    parameters: vec![OntologyParameter {
                        role: "value".into(),
                        ty: OntologyTypeExpr::named(person.clone()),
                    }],
                    result: OntologyTypeExpr::named(proposition.clone()),
                    description: None,
                },
            ),
        ]),
    };

    let mut graph = GraphBuilder::new();
    let owns_object = graph.insert("owns", ObjectNode::new());
    let person_condition = graph.insert("is-person", ObjectNode::new());
    let person_type = graph.insert("person-type", ObjectNode::new());
    let alice_object = graph.insert("alice", ObjectNode::new());
    let car_object = graph.insert("car", ObjectNode::new());
    let claim = graph.insert(
        "claim",
        ObjectNode::new()
            .with_operator(owns_object.clone())
            .with_edge("owner", alice_object.clone())
            .with_edge("owned", car_object.clone()),
    );
    let person_claim = graph.insert(
        "person-claim",
        ObjectNode::new()
            .with_operator(person_condition.clone())
            .with_edge("value", alice_object.clone()),
    );
    graph.root("main", claim.clone());

    let submission = InterpretedGraph {
        graph: graph.finish().unwrap(),
        ontology,
        interpretations: BTreeMap::from([
            (
                person_type.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::Universe { level: 0 },
                    meaning: ObjectMeaning::Type {
                        value: OntologyTypeExpr::named(person.clone()),
                    },
                },
            ),
            (
                owns_object,
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
                person_condition.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::function(
                        OntologyTypeExpr::named(person.clone()),
                        OntologyTypeExpr::named(proposition.clone()),
                    ),
                    meaning: ObjectMeaning::Symbol { id: is_person },
                },
            ),
            (
                alice_object.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(person),
                    meaning: ObjectMeaning::Symbol { id: alice },
                },
            ),
            (
                car_object.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(object),
                    meaning: ObjectMeaning::Symbol { id: car },
                },
            ),
            (
                claim.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(proposition.clone()),
                    meaning: ObjectMeaning::Application {
                        arguments: vec!["owner".into(), "owned".into()],
                    },
                },
            ),
            (
                person_claim.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(proposition),
                    meaning: ObjectMeaning::Application {
                        arguments: vec!["value".into()],
                    },
                },
            ),
        ]),
    };

    OwnershipFixture {
        submission,
        claim,
        person_condition,
        person_claim,
        alice: alice_object,
        car: car_object,
        person_type,
    }
}

fn proof_theory(
    fixture: &OwnershipFixture,
) -> (
    CompiledOntologySubmission,
    Theory,
    CertifiedGraphClaim,
    Certificate,
) {
    let compiled =
        OntologyCompiler::compile(Theory::empty("test", "1"), &fixture.submission).unwrap();
    let claim = OntologyCompiler::elaborate(
        &fixture.submission,
        &compiled,
        &fixture.claim,
        ElaborationRole::Proposition,
    )
    .unwrap();
    let person_claim = OntologyCompiler::elaborate(
        &fixture.submission,
        &compiled,
        &fixture.person_claim,
        ElaborationRole::Proposition,
    )
    .unwrap();

    let mut builder = TheoryBuilder::from_theory(compiled.theory.clone()).unwrap();
    builder
        .axiom(
            "test.fact.owns",
            claim.term.clone(),
            Provenance::new("test explicit ownership fact"),
        )
        .unwrap();
    builder
        .axiom(
            "test.fact.person",
            person_claim.term.clone(),
            Provenance::new("test explicit person fact"),
        )
        .unwrap();
    let theory = builder.finish();

    let claim_certificate = Certificate::new(
        &theory,
        Context::new(),
        claim.term.clone(),
        Term::constant("test.fact.owns"),
    );
    let certified_claim = CertifiedGraphClaim {
        claim: fixture.claim.clone(),
        elaboration: claim,
        dependencies: claim_certificate.dependencies(&theory),
        certificate: claim_certificate,
        production: None,
    };
    let witness_certificate = Certificate::new(
        &theory,
        Context::new(),
        person_claim.term,
        Term::constant("test.fact.person"),
    );
    (compiled, theory, certified_claim, witness_certificate)
}

fn find_person_query(fixture: &OwnershipFixture) -> CognitiveQuery {
    CognitiveQuery {
        id: QueryId("find-person".into()),
        submission: fixture.submission.clone(),
        request: QueryRequest::Find {
            variables: vec![QueryVariable {
                name: "person".into(),
                expected_type: fixture.person_type.clone(),
            }],
            condition: fixture.person_condition.clone(),
            exhaustion_claim: None,
        },
        theory: None,
        policy: QueryPolicy::default(),
    }
}

fn witness_answer(
    query: &CognitiveQuery,
    witness: QueryWitness,
    certificate: Certificate,
    theory: &Theory,
) -> CognitiveAnswer {
    CognitiveAnswer {
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        status: QueryStatus::Answered(QueryPayload::CertifiedWitnesses {
            answers: vec![CertifiedQueryWitness {
                bindings: BTreeMap::from([("person".into(), witness)]),
                dependencies: certificate.dependencies(theory),
                certificate,
                production: None,
            }],
        }),
        procedure: Some("fixture".into()),
    }
}

#[test]
fn exact_ontology_to_certificate_chain_verifies() {
    let fixture = ownership_fixture();
    let (compiled, theory, certified, _) = proof_theory(&fixture);
    OntologyCompiler::verify_compiled(&fixture.submission, &compiled).unwrap();
    certified
        .verify(&fixture.submission, &compiled, &theory)
        .unwrap();
}

#[test]
fn typed_artifact_roundtrip_preserves_full_submission() {
    let fixture = ownership_fixture();
    assert_eq!(<InterpretedGraph as ArtifactContract>::FORMAT_VERSION, 3);
    let artifact = ArtifactEnvelope::encode_typed(&fixture.submission).unwrap();
    artifact.verify().unwrap();
    let decoded: InterpretedGraph = artifact.decode_typed().unwrap();
    assert_eq!(decoded, fixture.submission);
    assert_eq!(
        decoded.canonical_hash(),
        fixture.submission.canonical_hash()
    );
    let reference = decoded.reference("main").unwrap();
    assert_eq!(reference.submission, decoded.canonical_hash());
    assert_eq!(reference.graph, decoded.graph.canonical_hash());
    assert_eq!(reference.root, fixture.claim);
}

#[test]
fn query_validation_uses_ontological_roles_and_full_content_identity() {
    let fixture = ownership_fixture();
    let query = CognitiveQuery {
        id: QueryId("q1".into()),
        submission: fixture.submission.clone(),
        request: QueryRequest::Prove {
            proposition: fixture.claim.clone(),
        },
        theory: None,
        policy: QueryPolicy::default(),
    };
    query.validate().unwrap();
    let continuation_theory =
        OntologyCompiler::compile(Theory::empty("test", "1"), &fixture.submission)
            .unwrap()
            .theory
            .id();
    let continuation = DiscoveryContinuation {
        format_version: 3,
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        submission: fixture.submission.canonical_hash(),
        graph: fixture.submission.graph.canonical_hash(),
        ontology: fixture.submission.ontology.canonical_hash(),
        theory: continuation_theory,
        procedure: "test-search".into(),
        procedure_version: "1".into(),
        state: vec![1, 2, 3],
        completed_steps: 4,
    };
    query
        .validate_discovery_continuation(&continuation)
        .unwrap();

    let invalid = CognitiveQuery {
        request: QueryRequest::Prove {
            proposition: fixture.alice.clone(),
        },
        ..query.clone()
    };
    assert!(matches!(
        invalid.validate(),
        Err(CognitiveQueryError::ExpectedProposition(_))
    ));

    let changed = CognitiveQuery {
        policy: QueryPolicy {
            step_limit: Some(10),
            ..QueryPolicy::default()
        },
        ..query
    };
    assert_eq!(
        changed.validate_discovery_continuation(&continuation),
        Err(CognitiveQueryError::ContinuationMismatch)
    );
}

#[test]
fn proof_queries_reject_witness_payloads_and_pin_kernel_continuations() {
    let fixture = ownership_fixture();
    let compiled =
        OntologyCompiler::compile(Theory::empty("test", "1"), &fixture.submission).unwrap();
    let query = CognitiveQuery {
        id: QueryId("proof-query".into()),
        submission: fixture.submission.clone(),
        request: QueryRequest::Prove {
            proposition: fixture.claim,
        },
        theory: Some(compiled.theory.id()),
        policy: QueryPolicy::default(),
    };
    let incompatible = CognitiveAnswer {
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        status: QueryStatus::Answered(QueryPayload::CertifiedWitnesses {
            answers: Vec::new(),
        }),
        procedure: None,
    };
    assert_eq!(
        query.validate_answer(&incompatible),
        Err(CognitiveQueryError::IncompatibleAnswer)
    );

    let session = Kernel::start(
        compiled.theory.clone(),
        KernelRequest::Infer {
            context: Context::new(),
            term: Term::universe(0),
        },
    );
    let continuation = KernelContinuation {
        format_version: 2,
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        submission: fixture.submission.canonical_hash(),
        graph: fixture.submission.graph.canonical_hash(),
        ontology: fixture.submission.ontology.canonical_hash(),
        theory: compiled.theory.id(),
        session,
    };
    let paused = CognitiveAnswer {
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        status: QueryStatus::Paused(Continuation::Kernel(continuation.clone())),
        procedure: None,
    };
    query.validate_answer(&paused).unwrap();

    let mut mismatched = continuation;
    mismatched.query_content.0.push('x');
    let invalid = CognitiveAnswer {
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        status: QueryStatus::Paused(Continuation::Kernel(mismatched)),
        procedure: None,
    };
    assert_eq!(
        query.validate_answer(&invalid),
        Err(CognitiveQueryError::ContinuationMismatch)
    );
}

#[test]
fn discovery_continuations_record_the_concrete_selected_theory() {
    let fixture = ownership_fixture();
    let (compiled, theory, _, _) = proof_theory(&fixture);
    let query = CognitiveQuery {
        id: QueryId("discovery-theory".into()),
        submission: fixture.submission.clone(),
        request: QueryRequest::Prove {
            proposition: fixture.claim,
        },
        theory: None,
        policy: QueryPolicy {
            procedure: Some("fixture-search".into()),
            ..QueryPolicy::default()
        },
    };
    let continuation = DiscoveryContinuation {
        format_version: 3,
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        submission: fixture.submission.canonical_hash(),
        graph: fixture.submission.graph.canonical_hash(),
        ontology: fixture.submission.ontology.canonical_hash(),
        theory: theory.id(),
        procedure: "fixture-search".into(),
        procedure_version: "1".into(),
        state: vec![4, 5, 6],
        completed_steps: 7,
    };
    let answer = CognitiveAnswer {
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        status: QueryStatus::Paused(Continuation::Discovery(continuation.clone())),
        procedure: Some("fixture-search".into()),
    };
    query.verify_answer(&answer, &compiled, &theory).unwrap();

    let missing_envelope_procedure = CognitiveAnswer {
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        status: QueryStatus::Paused(Continuation::Discovery(continuation.clone())),
        procedure: None,
    };
    assert_eq!(
        query.validate_answer(&missing_envelope_procedure),
        Err(CognitiveQueryError::ProcedureMismatch)
    );

    let mut wrong_procedure = continuation.clone();
    wrong_procedure.procedure = "other-search".into();
    assert_eq!(
        query.validate_discovery_continuation(&wrong_procedure),
        Err(CognitiveQueryError::ProcedureMismatch)
    );

    let mut wrong = continuation;
    wrong.theory = compiled.theory.id();
    let invalid = CognitiveAnswer {
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        status: QueryStatus::Paused(Continuation::Discovery(wrong)),
        procedure: Some("fixture-search".into()),
    };
    assert_eq!(
        query.verify_answer(&invalid, &compiled, &theory),
        Err(CognitiveQueryError::AnswerTheoryMismatch)
    );
}

#[test]
fn graph_and_formal_witnesses_require_proofs_of_the_exact_result_predicate() {
    let fixture = ownership_fixture();
    let (compiled, theory, _, witness_certificate) = proof_theory(&fixture);
    let query = find_person_query(&fixture);

    let graph_answer = witness_answer(
        &query,
        QueryWitness::Graph {
            object: fixture.alice.clone(),
        },
        witness_certificate.clone(),
        &theory,
    );
    query
        .verify_answer(&graph_answer, &compiled, &theory)
        .unwrap();

    let formal_answer = witness_answer(
        &query,
        QueryWitness::Formal {
            term: Term::constant(compiled.names.symbols[&OntologySymbolId::from("alice")].clone()),
        },
        witness_certificate.clone(),
        &theory,
    );
    query
        .verify_answer(&formal_answer, &compiled, &theory)
        .unwrap();

    let wrong = witness_answer(
        &query,
        QueryWitness::Graph {
            object: fixture.car,
        },
        witness_certificate,
        &theory,
    );
    assert!(matches!(
        query.validate_answer(&wrong),
        Err(CognitiveQueryError::WitnessTypeMismatch { .. })
    ));
}

#[test]
fn enumeration_rejects_duplicate_result_tuples() {
    let fixture = ownership_fixture();
    let (_, theory, _, certificate) = proof_theory(&fixture);
    let query = CognitiveQuery {
        id: QueryId("enumerate-people".into()),
        submission: fixture.submission.clone(),
        request: QueryRequest::Enumerate {
            variables: vec![QueryVariable {
                name: "person".into(),
                expected_type: fixture.person_type.clone(),
            }],
            condition: fixture.person_condition.clone(),
            exhaustion_claim: None,
            limit: None,
        },
        theory: None,
        policy: QueryPolicy::default(),
    };
    let one = CertifiedQueryWitness {
        bindings: BTreeMap::from([(
            "person".into(),
            QueryWitness::Graph {
                object: fixture.alice,
            },
        )]),
        dependencies: certificate.dependencies(&theory),
        certificate,
        production: None,
    };
    let answer = CognitiveAnswer {
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        status: QueryStatus::Answered(QueryPayload::CertifiedWitnesses {
            answers: vec![one.clone(), one],
        }),
        procedure: None,
    };
    assert_eq!(
        query.validate_answer(&answer),
        Err(CognitiveQueryError::DuplicateWitness)
    );
}

#[test]
fn under_assumptions_requires_the_exact_ordered_context() {
    let fixture = ownership_fixture();
    let compiled =
        OntologyCompiler::compile(Theory::empty("test", "1"), &fixture.submission).unwrap();
    let elaboration = OntologyCompiler::elaborate(
        &fixture.submission,
        &compiled,
        &fixture.claim,
        ElaborationRole::Proposition,
    )
    .unwrap();
    let certificate = Certificate::new(
        &compiled.theory,
        Context(vec![elaboration.term.clone()]),
        elaboration.term.clone(),
        Term::var(0),
    );
    let certified = CertifiedGraphClaim {
        claim: fixture.claim.clone(),
        elaboration,
        dependencies: certificate.dependencies(&compiled.theory),
        certificate,
        production: None,
    };
    let query = CognitiveQuery {
        id: QueryId("under-assumption".into()),
        submission: fixture.submission,
        request: QueryRequest::UnderAssumptions {
            goal: fixture.claim.clone(),
            assumptions: vec![fixture.claim],
        },
        theory: Some(compiled.theory.id()),
        policy: QueryPolicy::default(),
    };
    let answer = CognitiveAnswer {
        query: query.id.clone(),
        query_content: query.canonical_hash().unwrap(),
        status: QueryStatus::Proved(certified),
        procedure: None,
    };
    query
        .verify_answer(&answer, &compiled, &compiled.theory)
        .unwrap();
}

#[test]
fn partial_and_higher_order_applications_elaborate_deterministically() {
    let template = ownership_fixture();
    let person = OntologyTypeId::from("Person");
    let object = OntologyTypeId::from("Object");
    let proposition = OntologyTypeId::from("Proposition");
    let owns = OntologySymbolId::from("owns");
    let alice = OntologySymbolId::from("alice");
    let car = OntologySymbolId::from("car");

    let mut graph = GraphBuilder::new();
    let owns_object = graph.insert("owns", ObjectNode::new());
    let alice_object = graph.insert("alice", ObjectNode::new());
    let car_object = graph.insert("car", ObjectNode::new());
    let partial = graph.insert(
        "owns-alice",
        ObjectNode::new()
            .with_operator(owns_object.clone())
            .with_edge("owner", alice_object.clone()),
    );
    let claim = graph.insert(
        "claim",
        ObjectNode::new()
            .with_operator(partial.clone())
            .with_edge("owned", car_object.clone()),
    );
    graph.root("main", claim.clone());

    let submission = InterpretedGraph {
        graph: graph.finish().unwrap(),
        ontology: template.submission.ontology,
        interpretations: BTreeMap::from([
            (
                owns_object,
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
                alice_object,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(person),
                    meaning: ObjectMeaning::Symbol { id: alice },
                },
            ),
            (
                car_object,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(object.clone()),
                    meaning: ObjectMeaning::Symbol { id: car },
                },
            ),
            (
                partial.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::function(
                        OntologyTypeExpr::named(object),
                        OntologyTypeExpr::named(proposition.clone()),
                    ),
                    meaning: ObjectMeaning::Application {
                        arguments: vec!["owner".into()],
                    },
                },
            ),
            (
                claim.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(proposition),
                    meaning: ObjectMeaning::Application {
                        arguments: vec!["owned".into()],
                    },
                },
            ),
        ]),
    };

    submission.validate().unwrap();
    assert!(submission.typing_diagnostics().is_empty());
    let compiled = OntologyCompiler::compile(Theory::empty("test", "1"), &submission).unwrap();
    let elaboration =
        OntologyCompiler::elaborate(&submission, &compiled, &claim, ElaborationRole::Proposition)
            .unwrap();
    assert_eq!(elaboration.object_dependencies.len(), 5);
}

#[test]
fn quoted_objects_are_ordinary_exact_typed_terms() {
    let proposition = OntologyTypeId::from("Proposition");
    let mentions = OntologySymbolId::from("mentions");
    let ontology = Ontology {
        namespace: "test.quoted".into(),
        version: "1".into(),
        proposition_type: proposition.clone(),
        types: BTreeMap::from([(
            proposition.clone(),
            OntologyTypeDeclaration {
                id: proposition.clone(),
                universe: 1,
                description: None,
            },
        )]),
        symbols: BTreeMap::from([(
            mentions.clone(),
            OntologySymbolDeclaration {
                id: mentions.clone(),
                parameters: vec![OntologyParameter {
                    role: "quoted".into(),
                    ty: OntologyTypeExpr::QuotedObject,
                }],
                result: OntologyTypeExpr::named(proposition.clone()),
                description: None,
            },
        )]),
    };

    let mut graph = GraphBuilder::new();
    let mentions_object = graph.insert("mentions", ObjectNode::new());
    let quoted = graph.insert("quoted", ObjectNode::new());
    graph
        .node_mut(&quoted)
        .unwrap()
        .edges
        .entry("self".into())
        .or_default()
        .push(quoted.clone());
    let claim = graph.insert(
        "claim",
        ObjectNode::new()
            .with_operator(mentions_object.clone())
            .with_edge("quoted", quoted.clone()),
    );
    graph.root("main", claim.clone());

    let submission = InterpretedGraph {
        graph: graph.finish().unwrap(),
        ontology,
        interpretations: BTreeMap::from([
            (
                mentions_object,
                ObjectInterpretation {
                    ty: OntologyTypeExpr::function(
                        OntologyTypeExpr::QuotedObject,
                        OntologyTypeExpr::named(proposition.clone()),
                    ),
                    meaning: ObjectMeaning::Symbol { id: mentions },
                },
            ),
            (
                quoted.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::QuotedObject,
                    meaning: ObjectMeaning::Quoted,
                },
            ),
            (
                claim.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::named(proposition),
                    meaning: ObjectMeaning::Application {
                        arguments: vec!["quoted".into()],
                    },
                },
            ),
        ]),
    };

    submission.validate().unwrap();
    assert!(submission.typing_diagnostics().is_empty());
    let compiled = OntologyCompiler::compile(Theory::empty("test", "1"), &submission).unwrap();
    let semantic_quote =
        OntologyCompiler::elaborate(&submission, &compiled, &quoted, ElaborationRole::Term)
            .unwrap();
    let explicit_quote = OntologyCompiler::elaborate(
        &submission,
        &compiled,
        &quoted,
        ElaborationRole::QuotedObject,
    )
    .unwrap();
    assert_eq!(semantic_quote.term, explicit_quote.term);
    OntologyCompiler::elaborate(&submission, &compiled, &claim, ElaborationRole::Proposition)
        .unwrap();
}
