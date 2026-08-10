#![forbid(unsafe_code)]

use std::{collections::BTreeSet, path::Path};

use muse_core::LanguageTag;
use muse_io::load_registry_directory;
use muse_resolution::{
    LexicalResolver, ReferenceMode, RegistryResolver, ResolutionPolicy, ResolutionRequest,
    ResolutionResult,
};
use muse_validation::{ValidationOptions, validate_registry};

fn packages() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages")
}

#[test]
fn bundled_packages_load_and_validate() {
    let registry = load_registry_directory(packages()).unwrap();
    let snapshot = registry.snapshot_all().unwrap();
    let options = ValidationOptions::default();
    let report = validate_registry(&registry, &snapshot, &options).unwrap();
    assert!(report.passed(&options), "{:#?}", report.diagnostics);
    assert!(report.stats.concepts > 50);
    assert!(report.stats.lexical_senses > 50);
}

#[test]
fn event_resolves_to_the_ufo_event_concept() {
    let registry = load_registry_directory(packages()).unwrap();
    let request = ResolutionRequest {
        surface: "event".to_owned(),
        language: LanguageTag::from("en"),
        snapshot: registry.snapshot_all().unwrap(),
        domain_hints: BTreeSet::from(["foundational ontology".to_owned()]),
        context_concepts: BTreeSet::new(),
        reference_mode: ReferenceMode::Use,
        policy: ResolutionPolicy::default(),
    };
    let result = RegistryResolver::new(&registry).resolve(&request).unwrap();
    assert!(matches!(result, ResolutionResult::Resolved { .. }));
}
