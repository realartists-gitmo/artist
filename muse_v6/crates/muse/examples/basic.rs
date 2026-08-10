#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use muse::{
    io::load_registry_directory,
    resolution::{
        LexicalResolver, ReferenceMode, RegistryResolver, ResolutionPolicy, ResolutionRequest,
    },
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = load_registry_directory("../../packages")?;
    let snapshot = registry.snapshot_all()?;
    let request = ResolutionRequest {
        surface: "event".to_owned(),
        language: muse::core::LanguageTag::from("en"),
        snapshot,
        domain_hints: BTreeSet::from(["foundational ontology".to_owned()]),
        context_concepts: BTreeSet::new(),
        reference_mode: ReferenceMode::Use,
        policy: ResolutionPolicy::default(),
    };
    println!("{:#?}", RegistryResolver::new(&registry).resolve(&request)?);
    Ok(())
}
