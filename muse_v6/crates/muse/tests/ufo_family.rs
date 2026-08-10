#![forbid(unsafe_code)]

use std::{collections::BTreeSet, error::Error, path::PathBuf};

use muse::{
    core::{ConceptId, LanguageTag, RelationId, SemanticObjectId},
    io::load_registry_directory,
    reasoning::{
        Fact, ForwardReasoner, KnowledgeBase, ReasoningOptions, ViolationKind, WorldAssumption,
    },
    resolution::{
        LexicalResolver, ReferenceMode, RegistryResolver, ResolutionPolicy, ResolutionRequest,
        ResolutionResult,
    },
    validation::{ValidationOptions, validate_registry},
};

fn package_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/ufo")
}

#[test]
fn full_family_loads_and_passes_release_validation() -> Result<(), Box<dyn Error>> {
    let registry = load_registry_directory(package_dir())?;
    assert_eq!(registry.package_count(), 9);
    let snapshot = registry.snapshot_all()?;
    let report = validate_registry(&registry, &snapshot, &ValidationOptions::default())?;
    assert!(
        report.passed(&ValidationOptions::default()),
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(report.stats.packages, 9);
    assert_eq!(report.stats.concepts, 266);
    assert_eq!(report.stats.relations, 194);
    assert_eq!(report.stats.lexical_entries, 460);
    assert_eq!(report.stats.lexical_senses, 460);
    assert_eq!(report.stats.predicate_frames, 194);
    assert_eq!(report.stats.attestations, 460);
    assert_eq!(report.stats.evidence_records, 12);
    Ok(())
}

#[test]
fn cross_module_taxonomy_is_connected() -> Result<(), Box<dyn Error>> {
    let registry = load_registry_directory(package_dir())?;
    let snapshot = registry.snapshot_all()?;
    let index = registry.ontology_index(&snapshot)?;
    let cases = [
        ("ufo:Action", "ufo:Event"),
        ("ufo:Event", "ufo:Perdurant"),
        ("ufo:SocialCommitment", "ufo:Moment"),
        ("service:ServiceAgreement", "ufo:SocialRelator"),
        ("legal:ClaimRight", "legal:LegalPosition"),
        ("legal:LegalRelator", "ufo:SocialRelator"),
        ("mlt:Powertype", "mlt:HigherOrderType"),
        ("mlt:Powertype", "mlt:PartitioningType"),
        ("ab:CounterfactualHistory", "ab:History"),
    ];
    for (child, parent) in cases {
        assert!(
            index.is_subtype(&ConceptId::from(child), &ConceptId::from(parent)),
            "expected {child} <: {parent}"
        );
    }
    Ok(())
}

#[test]
fn b_and_ab_causal_rules_fire() -> Result<(), Box<dyn Error>> {
    let registry = load_registry_directory(package_dir())?;
    let snapshot = registry.snapshot_all()?;
    let index = registry.ontology_index(&snapshot)?;
    let e1 = SemanticObjectId::from("object:event-1");
    let e2 = SemanticObjectId::from("object:event-2");
    let situation = SemanticObjectId::from("object:situation");
    let kb = KnowledgeBase::new([
        Fact::InstanceOf {
            individual: e1.clone(),
            concept: ConceptId::from("ufo:Event"),
        },
        Fact::InstanceOf {
            individual: e2.clone(),
            concept: ConceptId::from("ufo:Event"),
        },
        Fact::InstanceOf {
            individual: situation.clone(),
            concept: ConceptId::from("ufo:Situation"),
        },
        Fact::Relation {
            subject: e1.clone(),
            relation: RelationId::from("ufo:bringsAbout"),
            object: situation.clone(),
        },
        Fact::Relation {
            subject: situation,
            relation: RelationId::from("ufo:triggers"),
            object: e2.clone(),
        },
    ]);
    let report = ForwardReasoner::new(&index).reason(&kb, &ReasoningOptions::default())?;
    for relation in ["ufo:directlyCauses", "ufo:causes", "ab:causalSuccessor"] {
        assert!(
            report.closure.facts.contains(&Fact::Relation {
                subject: e1.clone(),
                relation: RelationId::from(relation),
                object: e2.clone(),
            }),
            "missing derived relation {relation}"
        );
    }
    Ok(())
}

#[test]
fn legal_partition_rejects_incompatible_positions() -> Result<(), Box<dyn Error>> {
    let registry = load_registry_directory(package_dir())?;
    let snapshot = registry.snapshot_all()?;
    let index = registry.ontology_index(&snapshot)?;
    let position = SemanticObjectId::from("object:position");
    let kb = KnowledgeBase::new([
        Fact::InstanceOf {
            individual: position.clone(),
            concept: ConceptId::from("legal:ClaimRight"),
        },
        Fact::InstanceOf {
            individual: position,
            concept: ConceptId::from("legal:Duty"),
        },
    ]);
    let report = ForwardReasoner::new(&index).reason(&kb, &ReasoningOptions::default())?;
    assert!(
        report
            .violations
            .iter()
            .any(|violation| violation.kind == ViolationKind::DisjointTypes)
    );
    Ok(())
}

#[test]
fn closed_world_service_agreement_requires_parties_and_commitments() -> Result<(), Box<dyn Error>> {
    let registry = load_registry_directory(package_dir())?;
    let snapshot = registry.snapshot_all()?;
    let index = registry.ontology_index(&snapshot)?;
    let agreement = SemanticObjectId::from("object:agreement");
    let kb = KnowledgeBase::new([Fact::InstanceOf {
        individual: agreement,
        concept: ConceptId::from("service:ServiceAgreement"),
    }]);
    let report = ForwardReasoner::new(&index).reason(
        &kb,
        &ReasoningOptions {
            world_assumption: WorldAssumption::Closed,
            ..ReasoningOptions::default()
        },
    )?;
    let kinds: BTreeSet<ViolationKind> = report
        .violations
        .iter()
        .map(|violation| violation.kind)
        .collect();
    assert!(kinds.contains(&ViolationKind::MinimumCardinality));
    assert!(kinds.contains(&ViolationKind::ExistentialRestriction));
    Ok(())
}

#[test]
fn domain_range_inverse_and_superrelation_inference_work() -> Result<(), Box<dyn Error>> {
    let registry = load_registry_directory(package_dir())?;
    let snapshot = registry.snapshot_all()?;
    let index = registry.ontology_index(&snapshot)?;
    let component = SemanticObjectId::from("object:component");
    let whole = SemanticObjectId::from("object:whole");
    let kb = KnowledgeBase::new([Fact::Relation {
        subject: component.clone(),
        relation: RelationId::from("ufo:componentOf"),
        object: whole.clone(),
    }]);
    let report = ForwardReasoner::new(&index).reason(&kb, &ReasoningOptions::default())?;
    for individual in [&component, &whole] {
        assert!(report.closure.facts.contains(&Fact::InstanceOf {
            individual: individual.clone(),
            concept: ConceptId::from("ufo:Object"),
        }));
    }
    assert!(report.closure.facts.contains(&Fact::Relation {
        subject: component.clone(),
        relation: RelationId::from("ufo:properPartOf"),
        object: whole.clone(),
    }));
    assert!(report.closure.facts.contains(&Fact::Relation {
        subject: whole,
        relation: RelationId::from("ufo:hasComponent"),
        object: component,
    }));
    Ok(())
}

#[test]
fn representative_terms_resolve() -> Result<(), Box<dyn Error>> {
    let registry = load_registry_directory(package_dir())?;
    let snapshot = registry.snapshot_all()?;
    let resolver = RegistryResolver::new(&registry);
    for surface in [
        "service agreement",
        "claim-right",
        "powertype",
        "counterfactual history",
        "social commitment",
    ] {
        let result = resolver.resolve(&ResolutionRequest {
            surface: surface.to_owned(),
            language: LanguageTag::from("en"),
            snapshot: snapshot.clone(),
            domain_hints: BTreeSet::new(),
            context_concepts: BTreeSet::new(),
            reference_mode: ReferenceMode::Use,
            policy: ResolutionPolicy::default(),
        })?;
        assert!(
            matches!(&result, ResolutionResult::Resolved { .. }),
            "{surface}: {result:?}"
        );
    }
    Ok(())
}
