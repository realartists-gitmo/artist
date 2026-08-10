use artist_kernel::{
    CheckSession, ConstructorField, Context, Declaration, DeclarationKind, InductiveConstructor,
    InductiveDeclaration, Kernel, KernelError, KernelRequest, KernelResult, Provenance,
    SessionStatus, Term, Theory, TheoryBuilder, TheoryError, Transparency,
};

fn empty() -> Theory {
    Theory::empty("test", "1")
}

fn base_values() -> Theory {
    let mut builder = TheoryBuilder::new("values", "1");
    builder
        .axiom("A", Term::universe(0), Provenance::new("fixture"))
        .unwrap();
    builder
        .axiom("a", Term::constant("A"), Provenance::new("fixture"))
        .unwrap();
    builder
        .axiom("B", Term::universe(0), Provenance::new("fixture"))
        .unwrap();
    builder
        .axiom("b", Term::constant("B"), Provenance::new("fixture"))
        .unwrap();
    builder.finish()
}

fn natural_numbers(builder: &mut TheoryBuilder) {
    builder
        .inductive(InductiveDeclaration {
            name: "Nat".into(),
            parameters: vec![],
            indices: vec![],
            universe: 0,
            constructors: vec![
                InductiveConstructor {
                    name: "Nat.zero".into(),
                    fields: vec![],
                    result_indices: vec![],
                },
                InductiveConstructor {
                    name: "Nat.succ".into(),
                    fields: vec![ConstructorField::Recursive { indices: vec![] }],
                    result_indices: vec![],
                },
            ],
            provenance: Provenance::new("fixture"),
        })
        .unwrap();
}

fn nat_eliminator(scrutinee: Term) -> Term {
    Term::elim(
        "Nat",
        vec![],
        vec![],
        Term::lam(Term::constant("Nat"), Term::constant("Nat")),
        vec![
            Term::constant("Nat.zero"),
            Term::lam(
                Term::constant("Nat"),
                Term::lam(Term::constant("Nat"), Term::var(0)),
            ),
        ],
        scrutinee,
    )
}

#[test]
fn sigma_pairs_check_and_project_definitionally() {
    let mut builder = TheoryBuilder::from_theory(base_values()).unwrap();
    let pair_type = Term::sigma(Term::constant("A"), Term::constant("B"));
    builder
        .define(
            "p",
            pair_type,
            Term::pair(Term::constant("a"), Term::constant("b")),
            Provenance::new("fixture"),
        )
        .unwrap();
    let theory = builder.finish();

    assert_eq!(
        Kernel::run_to_completion(
            theory.clone(),
            KernelRequest::Infer {
                context: Context::new(),
                term: Term::fst(Term::constant("p")),
            },
        ),
        Ok(KernelResult::Inferred(Term::constant("A")))
    );
    assert_eq!(
        Kernel::run_to_completion(
            theory.clone(),
            KernelRequest::DefEq {
                context: Context::new(),
                left: Term::fst(Term::constant("p")),
                right: Term::constant("a"),
            },
        ),
        Ok(KernelResult::DefinitionallyEqual)
    );
    assert_eq!(
        Kernel::run_to_completion(
            theory,
            KernelRequest::DefEq {
                context: Context::new(),
                left: Term::snd(Term::constant("p")),
                right: Term::constant("b"),
            },
        ),
        Ok(KernelResult::DefinitionallyEqual)
    );
}

#[test]
fn identity_j_has_the_reflexivity_computation_rule() {
    let theory = base_values();
    let carrier = Term::constant("A");
    let value = Term::constant("a");
    let motive = Term::lam(
        carrier.clone(),
        Term::lam(
            carrier.clone(),
            Term::lam(
                Term::id(carrier.clone(), Term::var(1), Term::var(0)),
                carrier.clone(),
            ),
        ),
    );
    let eliminator = Term::j(
        carrier.clone(),
        motive,
        Term::lam(carrier.clone(), Term::var(0)),
        value.clone(),
        value.clone(),
        Term::refl(value.clone()),
    );
    assert_eq!(
        Kernel::run_to_completion(
            theory.clone(),
            KernelRequest::Infer {
                context: Context::new(),
                term: eliminator.clone(),
            },
        ),
        Ok(KernelResult::Inferred(carrier))
    );
    assert_eq!(
        Kernel::run_to_completion(
            theory,
            KernelRequest::DefEq {
                context: Context::new(),
                left: eliminator,
                right: value,
            },
        ),
        Ok(KernelResult::DefinitionallyEqual)
    );
}

#[test]
fn universes_are_cumulative_while_equality_remains_intensional() {
    assert_eq!(
        Kernel::run_to_completion(
            empty(),
            KernelRequest::Check {
                context: Context::new(),
                term: Term::universe(0),
                expected: Term::universe(2),
            },
        ),
        Ok(KernelResult::Checked)
    );
}

#[test]
fn empty_and_polynomial_inductive_families_are_native() {
    let mut builder = TheoryBuilder::new("inductives", "1");
    builder
        .inductive(InductiveDeclaration {
            name: "Empty".into(),
            parameters: vec![],
            indices: vec![],
            universe: 0,
            constructors: vec![],
            provenance: Provenance::new("fixture"),
        })
        .unwrap();
    natural_numbers(&mut builder);
    builder
        .inductive(InductiveDeclaration {
            name: "List".into(),
            parameters: vec![Term::universe(0)],
            indices: vec![],
            universe: 0,
            constructors: vec![
                InductiveConstructor {
                    name: "List.nil".into(),
                    fields: vec![],
                    result_indices: vec![],
                },
                InductiveConstructor {
                    name: "List.cons".into(),
                    fields: vec![
                        ConstructorField::Plain { ty: Term::var(0) },
                        ConstructorField::Recursive { indices: vec![] },
                    ],
                    result_indices: vec![],
                },
            ],
            provenance: Provenance::new("fixture"),
        })
        .unwrap();
    let theory = builder.finish();
    assert!(theory.inductive(&"Empty".into()).is_some());
    assert!(theory.inductive(&"Nat".into()).is_some());
    assert!(theory.inductive(&"List".into()).is_some());
}

#[test]
fn indexed_inductive_family_is_checked() {
    let mut builder = TheoryBuilder::new("vectors", "1");
    natural_numbers(&mut builder);
    builder
        .inductive(InductiveDeclaration {
            name: "Vec".into(),
            parameters: vec![Term::universe(0)],
            indices: vec![Term::constant("Nat")],
            universe: 0,
            constructors: vec![
                InductiveConstructor {
                    name: "Vec.nil".into(),
                    fields: vec![],
                    result_indices: vec![Term::constant("Nat.zero")],
                },
                InductiveConstructor {
                    name: "Vec.cons".into(),
                    fields: vec![
                        ConstructorField::Plain {
                            ty: Term::constant("Nat"),
                        },
                        ConstructorField::Plain { ty: Term::var(1) },
                        ConstructorField::Recursive {
                            indices: vec![Term::var(1)],
                        },
                    ],
                    result_indices: vec![Term::app(Term::constant("Nat.succ"), Term::var(2))],
                },
            ],
            provenance: Provenance::new("fixture"),
        })
        .unwrap();
    let theory = builder.finish();
    assert!(theory.inductive(&"Vec".into()).is_some());
}

#[test]
fn strict_positivity_rejects_negative_occurrence() {
    let mut builder = TheoryBuilder::new("bad", "1");
    let error = builder
        .inductive(InductiveDeclaration {
            name: "Bad".into(),
            parameters: vec![],
            indices: vec![],
            universe: 0,
            constructors: vec![InductiveConstructor {
                name: "Bad.mk".into(),
                fields: vec![ConstructorField::Plain {
                    ty: Term::pi(Term::constant("Bad"), Term::universe(0)),
                }],
                result_indices: vec![],
            }],
            provenance: Provenance::new("fixture"),
        })
        .unwrap_err();
    assert!(matches!(error, TheoryError::InvalidInductiveField { .. }));
}

#[test]
fn nat_iota_reduction_computes_through_recursive_hypothesis() {
    let mut builder = TheoryBuilder::new("nat", "1");
    natural_numbers(&mut builder);
    let theory = builder.finish();
    let one = Term::app(Term::constant("Nat.succ"), Term::constant("Nat.zero"));
    let eliminator = nat_eliminator(one);
    assert_eq!(
        Kernel::run_to_completion(
            theory.clone(),
            KernelRequest::Infer {
                context: Context::new(),
                term: eliminator.clone(),
            },
        ),
        Ok(KernelResult::Inferred(Term::constant("Nat")))
    );
    assert_eq!(
        Kernel::run_to_completion(
            theory,
            KernelRequest::DefEq {
                context: Context::new(),
                left: eliminator,
                right: Term::constant("Nat.zero"),
            },
        ),
        Ok(KernelResult::DefinitionallyEqual)
    );
}

#[test]
fn malformed_eliminator_is_rejected_not_treated_as_unknown() {
    let mut builder = TheoryBuilder::new("nat", "1");
    natural_numbers(&mut builder);
    let theory = builder.finish();
    let malformed = Term::elim(
        "Nat",
        vec![],
        vec![],
        Term::lam(Term::constant("Nat"), Term::universe(0)),
        vec![],
        Term::constant("Nat.zero"),
    );
    assert!(matches!(
        Kernel::run_to_completion(
            theory,
            KernelRequest::Infer {
                context: Context::new(),
                term: malformed,
            },
        ),
        Err(KernelError::InvalidInductiveEliminator { .. })
    ));
}

#[test]
fn inductive_checking_continuation_roundtrips_without_replay() {
    let mut builder = TheoryBuilder::new("nat", "1");
    natural_numbers(&mut builder);
    let theory = builder.finish();
    let mut session = Kernel::start(
        theory,
        KernelRequest::Infer {
            context: Context::new(),
            term: nat_eliminator(Term::constant("Nat.zero")),
        },
    );
    assert!(matches!(
        session.run_slice(1),
        SessionStatus::Running { steps: 1 }
    ));
    let serialized = serde_json::to_string(&session).unwrap();
    let mut resumed: CheckSession = serde_json::from_str(&serialized).unwrap();
    loop {
        match resumed.run_slice(1) {
            SessionStatus::Running { .. } => {}
            SessionStatus::Accepted { result, .. } => {
                assert_eq!(result, KernelResult::Inferred(Term::constant("Nat")));
                break;
            }
            SessionStatus::Rejected { error, .. } => panic!("unexpected rejection: {error}"),
        }
    }
}

#[test]
fn directly_constructed_invalid_theory_is_rejected_before_checking() {
    let mut theory = base_values();
    theory.declarations.push(Declaration {
        name: "bad".into(),
        ty: Term::constant("A"),
        body: Some(Term::constant("missing")),
        kind: DeclarationKind::Definition,
        transparency: Transparency::Transparent,
        provenance: Provenance::new("malformed fixture"),
    });
    assert!(matches!(
        Kernel::run_to_completion(
            theory,
            KernelRequest::Infer {
                context: Context::new(),
                term: Term::universe(0),
            },
        ),
        Err(KernelError::InvalidTheory(_))
    ));
}

#[test]
fn deserialized_continuation_revalidates_its_embedded_theory() {
    let session = Kernel::start(
        base_values(),
        KernelRequest::Infer {
            context: Context::new(),
            term: Term::constant("a"),
        },
    );
    let mut encoded = serde_json::to_value(session).unwrap();
    encoded["theory"]["declarations"][0]["body"] = serde_json::to_value(Term::universe(0)).unwrap();
    let mut restored: CheckSession = serde_json::from_value(encoded).unwrap();
    assert!(matches!(
        restored.run_slice(1),
        SessionStatus::Rejected {
            error: KernelError::InvalidTheory(_),
            ..
        }
    ));
}

#[test]
fn function_recursive_inductive_field_gets_a_functional_induction_hypothesis() {
    let mut builder = TheoryBuilder::from_theory(base_values()).unwrap();
    builder
        .inductive(InductiveDeclaration {
            name: "Tree".into(),
            parameters: vec![Term::universe(0)],
            indices: vec![],
            universe: 0,
            constructors: vec![
                InductiveConstructor {
                    name: "Tree.leaf".into(),
                    fields: vec![ConstructorField::Plain { ty: Term::var(0) }],
                    result_indices: vec![],
                },
                InductiveConstructor {
                    name: "Tree.node".into(),
                    fields: vec![ConstructorField::RecursiveFunction {
                        domain: Term::var(0),
                        indices: vec![],
                    }],
                    result_indices: vec![],
                },
            ],
            provenance: Provenance::new("fixture"),
        })
        .unwrap();
    let theory = builder.finish();
    let carrier = Term::constant("A");
    let leaf = Term::apply_many(
        Term::constant("Tree.leaf"),
        [carrier.clone(), Term::constant("a")],
    );
    let children = Term::lam(carrier.clone(), leaf);
    let node = Term::apply_many(Term::constant("Tree.node"), [carrier.clone(), children]);
    let motive = Term::lam(
        Term::app(Term::constant("Tree"), carrier.clone()),
        carrier.clone(),
    );
    let leaf_branch = Term::lam(carrier.clone(), Term::var(0));
    let child_function_type = Term::pi(
        carrier.clone(),
        Term::app(Term::constant("Tree"), carrier.clone()),
    );
    let induction_function_type = Term::pi(carrier.clone(), carrier.clone());
    let node_branch = Term::lam(
        child_function_type,
        Term::lam(induction_function_type, Term::constant("a")),
    );
    let eliminator = Term::elim(
        "Tree",
        vec![carrier.clone()],
        vec![],
        motive,
        vec![leaf_branch, node_branch],
        node,
    );
    assert_eq!(
        Kernel::run_to_completion(
            theory.clone(),
            KernelRequest::Infer {
                context: Context::new(),
                term: eliminator.clone(),
            },
        ),
        Ok(KernelResult::Inferred(carrier))
    );
    assert_eq!(
        Kernel::run_to_completion(
            theory,
            KernelRequest::DefEq {
                context: Context::new(),
                left: eliminator,
                right: Term::constant("a"),
            },
        ),
        Ok(KernelResult::DefinitionallyEqual)
    );
}

#[test]
fn malformed_j_does_not_compute_from_an_unrelated_reflexivity_witness() {
    let theory = base_values();
    let malformed = Term::j(
        Term::constant("A"),
        Term::lam(
            Term::constant("A"),
            Term::lam(
                Term::constant("A"),
                Term::lam(
                    Term::id(Term::constant("A"), Term::var(1), Term::var(0)),
                    Term::constant("A"),
                ),
            ),
        ),
        Term::lam(Term::constant("A"), Term::var(0)),
        Term::constant("a"),
        Term::constant("b"),
        Term::refl(Term::constant("a")),
    );
    assert!(matches!(
        Kernel::run_to_completion(
            theory,
            KernelRequest::DefEq {
                context: Context::new(),
                left: malformed,
                right: Term::constant("a"),
            },
        ),
        Err(KernelError::NotDefinitionallyEqual { .. })
    ));
}

#[test]
fn inductive_iota_checks_constructor_parameters_before_reducing() {
    let mut builder = TheoryBuilder::from_theory(base_values()).unwrap();
    builder
        .inductive(InductiveDeclaration {
            name: "Box".into(),
            parameters: vec![Term::universe(0)],
            indices: vec![],
            universe: 0,
            constructors: vec![InductiveConstructor {
                name: "Box.mk".into(),
                fields: vec![ConstructorField::Plain { ty: Term::var(0) }],
                result_indices: vec![],
            }],
            provenance: Provenance::new("fixture"),
        })
        .unwrap();
    let theory = builder.finish();
    let scrutinee = Term::apply_many(
        Term::constant("Box.mk"),
        [Term::constant("A"), Term::constant("a")],
    );
    let malformed = Term::elim(
        "Box",
        vec![Term::constant("B")],
        vec![],
        Term::lam(
            Term::app(Term::constant("Box"), Term::constant("B")),
            Term::constant("B"),
        ),
        vec![Term::lam(Term::constant("B"), Term::var(0))],
        scrutinee,
    );
    assert!(matches!(
        Kernel::run_to_completion(
            theory,
            KernelRequest::DefEq {
                context: Context::new(),
                left: malformed,
                right: Term::constant("a"),
            },
        ),
        Err(KernelError::NotDefinitionallyEqual { .. })
    ));
}

#[test]
fn serialized_checked_theory_reconstructs_exactly() {
    let mut builder = TheoryBuilder::new("roundtrip", "1");
    natural_numbers(&mut builder);
    let theory = builder.finish();
    let encoded = serde_json::to_vec(&theory).unwrap();
    let restored: Theory = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(restored, theory);
    assert_eq!(restored.validate(), Ok(()));
}
