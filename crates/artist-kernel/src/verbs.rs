//! Open-ended typed verb definitions and their active generations.
//!
//! This registry is intentionally independent of the installed ten-verb
//! compatibility layer. It is the seam through which verb packages become
//! discoverable and hot-swappable without changing kernel code.

use crate::{DynamicType, DynamicVerbCall, DynamicVerbResult, KernelError, VerbId};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct VerbPackageManifest {
    pub identity: String,
    pub function: String,
    pub model_name: String,
    pub description: String,
    #[serde(default)]
    pub docs: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub input_type: Option<String>,
    pub output_type: Option<String>,
    #[serde(default)]
    pub wit: Option<String>,
    #[serde(default = "default_manifest_file")]
    pub component: String,
}

fn default_manifest_file() -> String {
    "component.wasm".to_owned()
}

pub trait DynamicVerbExecutor: Send + Sync {
    fn invoke(&self, call: &DynamicVerbCall) -> Result<DynamicVerbResult, KernelError>;
}

impl VerbPackageManifest {
    pub fn from_toml(text: &str) -> Result<Self, KernelError> {
        toml::from_str(text).map_err(|error| KernelError::InvalidRequest {
            message: format!("invalid verb package manifest: {error}"),
        })
    }

    pub fn definition(&self, package_dir: &Path) -> Result<VerbDefinition, KernelError> {
        let identity = VerbId::new(&self.identity)
            .map_err(|message| KernelError::InvalidRequest { message })?;
        let mut definition = VerbDefinition::new(
            identity.clone(),
            &self.function,
            &self.model_name,
            &self.description,
        );
        definition.docs = self.docs.clone();
        definition.dependencies = self
            .dependencies
            .iter()
            .map(|dependency| {
                VerbId::new(dependency).map_err(|message| KernelError::InvalidRequest { message })
            })
            .collect::<Result<_, _>>()?;
        if let Some(wit) = &self.wit {
            let wit_path = package_dir.join(wit);
            let mut resolve = wit_parser::Resolve::default();
            let (package_id, _) =
                resolve
                    .push_path(&wit_path)
                    .map_err(|error| KernelError::InvalidRequest {
                        message: format!("invalid WIT contract {}: {error}", wit_path.display()),
                    })?;
            let package = &resolve.packages[package_id];
            let interface_id = package
                .interfaces
                .get(identity.interface())
                .ok_or_else(|| KernelError::InvalidRequest {
                    message: format!(
                        "WIT interface {} is absent from {}",
                        identity.interface(),
                        wit_path.display()
                    ),
                })?;
            let interface = &resolve.interfaces[*interface_id];
            if !interface.functions.contains_key(&self.function) {
                return Err(KernelError::InvalidRequest {
                    message: format!(
                        "WIT function {} is absent from {}",
                        self.function,
                        wit_path.display()
                    ),
                });
            }
        }
        if let (Some(input), Some(output)) = (&self.input_type, &self.output_type) {
            definition.input_type = Some(DynamicType::named(input)?);
            definition.output_type = Some(DynamicType::named(output)?);
        } else if self.input_type.is_some() || self.output_type.is_some() {
            return Err(KernelError::InvalidRequest {
                message: "verb package must declare both input_type and output_type".into(),
            });
        }
        definition.source = Some(package_dir.to_owned());
        definition.artifact = Some(package_dir.join(&self.component));
        Ok(definition)
    }
}

/// Discover package metadata without knowing any verb names. Activation is a
/// separate operation so malformed packages never partially publish.
pub fn discover_verb_packages(root: &Path) -> Result<Vec<VerbDefinition>, KernelError> {
    let mut packages = Vec::new();
    let entries = fs::read_dir(root).map_err(|error| KernelError::Handler {
        message: format!("cannot scan verb package root {}: {error}", root.display()),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| KernelError::Handler {
            message: format!("cannot read verb package entry: {error}"),
        })?;
        if !entry
            .file_type()
            .map_err(|error| KernelError::Handler {
                message: format!("cannot inspect verb package entry: {error}"),
            })?
            .is_dir()
        {
            continue;
        }
        let directory = entry.path();
        let manifest_path = directory.join("verb.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&manifest_path).map_err(|error| KernelError::Handler {
            message: format!("cannot read {}: {error}", manifest_path.display()),
        })?;
        packages.push(VerbPackageManifest::from_toml(&text)?.definition(&directory)?);
    }
    packages.sort_by(|left, right| left.identity.cmp(&right.identity));
    Ok(packages)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerbDefinition {
    pub identity: VerbId,
    pub function: String,
    pub model_name: String,
    pub description: String,
    pub docs: Vec<String>,
    pub source: Option<PathBuf>,
    pub artifact: Option<PathBuf>,
    pub dependencies: Vec<VerbId>,
    /// The typed function contract. Optional only while compatibility packages
    /// are being migrated; dynamically invokable packages must provide both.
    pub input_type: Option<DynamicType>,
    pub output_type: Option<DynamicType>,
}

impl VerbDefinition {
    pub fn new(
        identity: VerbId,
        function: impl Into<String>,
        model_name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            identity,
            function: function.into(),
            model_name: model_name.into(),
            description: description.into(),
            docs: Vec::new(),
            source: None,
            artifact: None,
            dependencies: Vec::new(),
            input_type: None,
            output_type: None,
        }
    }

    pub fn with_contract(mut self, input: DynamicType, output: DynamicType) -> Self {
        self.input_type = Some(input);
        self.output_type = Some(output);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerbToolDescriptor {
    pub identity: VerbId,
    pub name: String,
    pub description: String,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ActiveVerb {
    pub definition: VerbDefinition,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct VerbLease {
    pub active: Arc<ActiveVerb>,
}

impl VerbLease {
    pub fn generation(&self) -> u64 {
        self.active.generation
    }

    pub fn definition(&self) -> &VerbDefinition {
        &self.active.definition
    }
}

#[derive(Clone, Default)]
pub struct VerbRegistry {
    entries: Arc<std::sync::RwLock<BTreeMap<VerbId, Arc<ActiveVerb>>>>,
    executors: Arc<std::sync::RwLock<BTreeMap<VerbId, Arc<dyn DynamicVerbExecutor>>>>,
}

impl VerbRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_executor(
        &self,
        identity: VerbId,
        executor: Arc<dyn DynamicVerbExecutor>,
    ) -> Result<(), KernelError> {
        self.executors
            .write()
            .map_err(|_| KernelError::Handler {
                message: "verb executor registry lock poisoned".to_owned(),
            })?
            .insert(identity, executor);
        Ok(())
    }

    pub fn execute(&self, call: &DynamicVerbCall) -> Result<DynamicVerbResult, KernelError> {
        let active = self.validate_call(call)?;
        let executor = self
            .executors
            .read()
            .map_err(|_| KernelError::Handler {
                message: "verb executor registry lock poisoned".to_owned(),
            })?
            .get(&call.verb)
            .cloned()
            .ok_or_else(|| KernelError::InvalidRequest {
                message: format!("verb {} has no active executor", call.verb),
            })?;
        let result = executor.invoke(call)?;
        self.validate_result(call, &result)?;
        if self.current(&call.verb)?.map(|value| value.generation) != Some(active.generation) {
            return Err(KernelError::InvalidRequest {
                message: format!("verb {} was replaced during invocation", call.verb),
            });
        }
        Ok(result)
    }

    pub fn activate(&self, definition: VerbDefinition) -> Result<u64, KernelError> {
        let mut generations = self.activate_packages(vec![definition])?;
        Ok(generations.remove(0))
    }

    /// Publish a package set as one transaction. All identities are checked
    /// before the lock is mutated, so a malformed or duplicate package cannot
    /// leave a partially updated active registry.
    pub fn activate_packages(
        &self,
        definitions: Vec<VerbDefinition>,
    ) -> Result<Vec<u64>, KernelError> {
        let mut seen = std::collections::BTreeSet::new();
        let desired = definitions
            .iter()
            .map(|definition| definition.identity.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let active = self.entries.read().map_err(|_| KernelError::Handler {
            message: "verb registry lock poisoned".to_owned(),
        })?;
        for definition in &definitions {
            if !seen.insert(definition.identity.clone()) {
                return Err(KernelError::Conflict {
                    uri: format!("verb://{}", definition.identity),
                });
            }
            if definition.function.is_empty()
                || definition.model_name.is_empty()
                || definition.description.is_empty()
            {
                return Err(KernelError::InvalidRequest {
                    message: format!(
                        "verb {} has incomplete package metadata",
                        definition.identity
                    ),
                });
            }
            for dependency in &definition.dependencies {
                if !desired.contains(dependency) && !active.contains_key(dependency) {
                    return Err(KernelError::NotFound {
                        uri: format!("verb://dependency/{dependency}"),
                    });
                }
            }
            if let Some(artifact) = &definition.artifact {
                if !artifact.is_file() {
                    return Err(KernelError::NotFound {
                        uri: artifact.display().to_string(),
                    });
                }
            }
        }
        drop(active);
        let by_identity = definitions
            .iter()
            .map(|definition| (&definition.identity, definition))
            .collect::<BTreeMap<_, _>>();
        fn visit(
            identity: &VerbId,
            by_identity: &BTreeMap<&VerbId, &VerbDefinition>,
            visiting: &mut std::collections::BTreeSet<VerbId>,
            visited: &mut std::collections::BTreeSet<VerbId>,
        ) -> Result<(), KernelError> {
            if visited.contains(identity) {
                return Ok(());
            }
            if !visiting.insert(identity.clone()) {
                return Err(KernelError::Conflict {
                    uri: format!("verb://dependency-cycle/{identity}"),
                });
            }
            if let Some(definition) = by_identity.get(identity) {
                for dependency in &definition.dependencies {
                    if by_identity.contains_key(dependency) {
                        visit(dependency, by_identity, visiting, visited)?;
                    }
                }
            }
            visiting.remove(identity);
            visited.insert(identity.clone());
            Ok(())
        }
        let mut visiting = std::collections::BTreeSet::new();
        let mut visited = std::collections::BTreeSet::new();
        for identity in &seen {
            visit(identity, &by_identity, &mut visiting, &mut visited)?;
        }

        let mut entries = self.entries.write().map_err(|_| KernelError::Handler {
            message: "verb registry lock poisoned".to_owned(),
        })?;
        let generations = definitions
            .into_iter()
            .map(|definition| {
                if let Some(active) = entries.get(&definition.identity) {
                    if active.definition == definition {
                        return active.generation;
                    }
                }
                let generation = entries
                    .get(&definition.identity)
                    .map(|active| active.generation + 1)
                    .unwrap_or(1);
                entries.insert(
                    definition.identity.clone(),
                    Arc::new(ActiveVerb {
                        definition,
                        generation,
                    }),
                );
                generation
            })
            .collect();
        Ok(generations)
    }

    pub fn activate_discovered(&self, root: &Path) -> Result<Vec<u64>, KernelError> {
        let definitions = discover_verb_packages(root)?;
        if let Some(definition) = definitions
            .iter()
            .find(|definition| definition.input_type.is_none() || definition.output_type.is_none())
        {
            return Err(KernelError::InvalidRequest {
                message: format!(
                    "verb package {} has no complete typed function contract",
                    definition.identity
                ),
            });
        }
        self.activate_packages(definitions)
    }

    pub fn deactivate(&self, identity: &VerbId) -> Result<Option<Arc<ActiveVerb>>, KernelError> {
        let removed = self
            .entries
            .write()
            .map_err(|_| KernelError::Handler {
                message: "verb registry lock poisoned".to_owned(),
            })
            .map(|mut entries| entries.remove(identity))?;
        self.executors
            .write()
            .map_err(|_| KernelError::Handler {
                message: "verb executor registry lock poisoned".to_owned(),
            })?
            .remove(identity);
        Ok(removed)
    }

    pub fn reconcile_packages(
        &self,
        definitions: Vec<VerbDefinition>,
    ) -> Result<Vec<VerbId>, KernelError> {
        self.activate_packages(definitions.clone())?;
        let desired = definitions
            .into_iter()
            .map(|definition| definition.identity)
            .collect::<std::collections::BTreeSet<_>>();
        let removed = self
            .entries
            .read()
            .map_err(|_| KernelError::Handler {
                message: "verb registry lock poisoned".to_owned(),
            })?
            .keys()
            .filter(|identity| !desired.contains(*identity))
            .cloned()
            .collect::<Vec<_>>();
        for identity in &removed {
            self.deactivate(identity)?;
        }
        Ok(removed)
    }

    pub fn acquire(&self, identity: &VerbId) -> Result<VerbLease, KernelError> {
        let active = self
            .current(identity)?
            .ok_or_else(|| KernelError::UnsupportedVerb {
                verb: identity.to_string(),
                uri: "<verb-lease>".into(),
            })?;
        Ok(VerbLease { active })
    }

    pub fn current(&self, identity: &VerbId) -> Result<Option<Arc<ActiveVerb>>, KernelError> {
        self.entries
            .read()
            .map_err(|_| KernelError::Handler {
                message: "verb registry lock poisoned".to_owned(),
            })
            .map(|entries| entries.get(identity).cloned())
    }

    pub fn validate_call(&self, call: &DynamicVerbCall) -> Result<Arc<ActiveVerb>, KernelError> {
        let active = self
            .current(&call.verb)?
            .ok_or_else(|| KernelError::UnsupportedVerb {
                verb: call.verb.to_string(),
                uri: "<dynamic-call>".to_owned(),
            })?;
        if active.definition.function != call.function {
            return Err(KernelError::InvalidRequest {
                message: format!(
                    "function {} is not exported by verb {}",
                    call.function, call.verb
                ),
            });
        }
        let input_type =
            active
                .definition
                .input_type
                .as_ref()
                .ok_or_else(|| KernelError::InvalidRequest {
                    message: format!("verb {} has no published input contract", call.verb),
                })?;
        call.input.validate(input_type)?;
        Ok(active)
    }

    pub fn validate_result(
        &self,
        call: &DynamicVerbCall,
        result: &DynamicVerbResult,
    ) -> Result<(), KernelError> {
        if result.verb != call.verb || result.function != call.function {
            return Err(KernelError::InvalidRequest {
                message: "dynamic result identity does not match its call".to_owned(),
            });
        }
        let active = self
            .current(&call.verb)?
            .ok_or_else(|| KernelError::UnsupportedVerb {
                verb: call.verb.to_string(),
                uri: "<dynamic-result>".to_owned(),
            })?;
        let output_type =
            active
                .definition
                .output_type
                .as_ref()
                .ok_or_else(|| KernelError::InvalidRequest {
                    message: format!("verb {} has no published output contract", call.verb),
                })?;
        result.output.validate(output_type)
    }

    pub fn tool_descriptors(&self) -> Result<Vec<VerbToolDescriptor>, KernelError> {
        Ok(self
            .definitions()?
            .into_iter()
            .map(|active| VerbToolDescriptor {
                identity: active.definition.identity.clone(),
                name: active.definition.model_name.clone(),
                description: active.definition.description.clone(),
                docs: active.definition.docs.clone(),
            })
            .collect())
    }

    pub fn definitions(&self) -> Result<Vec<Arc<ActiveVerb>>, KernelError> {
        self.entries
            .read()
            .map_err(|_| KernelError::Handler {
                message: "verb registry lock poisoned".to_owned(),
            })
            .map(|entries| entries.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DynamicValue;

    fn definition(name: &str) -> VerbDefinition {
        VerbDefinition::new(
            VerbId::new(format!("example:{name}/{name}@1.0.0")).unwrap(),
            name,
            name,
            format!("{name} description"),
        )
    }

    #[test]
    fn manifest_wit_contract_must_contain_declared_function() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("contract.wit"),
            "package example:text@1.0.0; interface text { uppercase: func(input: string) -> string }",
        )
        .unwrap();
        let manifest = VerbPackageManifest::from_toml(
            "identity = 'example:text/uppercase@1.0.0'\nfunction = 'missing'\nmodel_name = 'uppercase'\ndescription = 'Uppercase'\nwit = 'contract.wit'\n",
        )
        .unwrap();
        assert!(matches!(
            manifest.definition(root.path()),
            Err(KernelError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn manifest_discovery_is_name_agnostic_and_preserves_artifact_path() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("uppercase-package");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("uppercase.wasm"), b"component-artifact").unwrap();
        std::fs::write(
            package.join("verb.toml"),
            "identity = 'example:text/uppercase@1.0.0'\nfunction = 'uppercase'\nmodel_name = 'uppercase'\ndescription = 'Uppercase text'\ndocs = ['tool.md']\ninput_type = 'string'\noutput_type = 'string'\ncomponent = 'uppercase.wasm'\n",
        )
        .unwrap();
        let definitions = discover_verb_packages(root.path()).unwrap();
        assert_eq!(definitions.len(), 1);
        assert_eq!(
            definitions[0].identity.to_string(),
            "example:text/uppercase@1.0.0"
        );
        assert_eq!(
            definitions[0].artifact,
            Some(package.join("uppercase.wasm"))
        );
        let registry = VerbRegistry::new();
        assert_eq!(registry.activate_discovered(root.path()).unwrap(), vec![1]);
    }

    #[test]
    fn missing_component_artifact_is_rejected_before_publication() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("missing");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(
            package.join("verb.toml"),
            "identity = 'example:text/missing@1.0.0'\nfunction = 'missing'\nmodel_name = 'missing'\ndescription = 'Missing'\ninput_type = 'string'\noutput_type = 'string'", 
        )
        .unwrap();
        let registry = VerbRegistry::new();
        assert!(matches!(
            registry.activate_discovered(root.path()),
            Err(KernelError::NotFound { .. })
        ));
        assert!(registry.definitions().unwrap().is_empty());
    }

    #[test]
    fn invalid_manifest_does_not_produce_a_definition() {
        let error = VerbPackageManifest::from_toml(
            "identity = 'not-a-contract'\nfunction = 'x'\nmodel_name = 'x'\ndescription = 'x'",
        )
        .unwrap()
        .definition(Path::new("."))
        .unwrap_err();
        assert!(matches!(error, KernelError::InvalidRequest { .. }));
    }

    #[test]
    fn dynamic_calls_are_checked_against_the_active_contract() {
        let registry = VerbRegistry::new();
        let identity = VerbId::new("example:text/uppercase@1.0.0").unwrap();
        let definition =
            VerbDefinition::new(identity.clone(), "uppercase", "uppercase", "Uppercase")
                .with_contract(DynamicType::String, DynamicType::String);
        registry.activate(definition).unwrap();
        let call = DynamicVerbCall {
            verb: identity.clone(),
            function: "uppercase".into(),
            input: crate::DynamicValue::String("hello".into()),
        };
        let active = registry.validate_call(&call).unwrap();
        assert_eq!(active.generation, 1);
        let result = DynamicVerbResult {
            verb: identity,
            function: "uppercase".into(),
            output: crate::DynamicValue::String("HELLO".into()),
        };
        registry.validate_result(&call, &result).unwrap();
    }

    #[test]
    fn active_tool_descriptors_are_derived_from_dynamic_packages() {
        let registry = VerbRegistry::new();
        let mut definition = definition("uppercase");
        definition.docs.push("uppercase.md".into());
        registry.activate(definition).unwrap();
        let tools = registry.tool_descriptors().unwrap();
        assert_eq!(tools[0].name, "uppercase");
        assert_eq!(tools[0].docs, vec!["uppercase.md"]);
        assert_eq!(
            tools[0].identity.to_string(),
            "example:uppercase/uppercase@1.0.0"
        );
    }

    #[test]
    fn activation_rejects_dependency_cycles_before_publication() {
        let registry = VerbRegistry::new();
        let mut first = definition("first");
        let mut second = definition("second");
        first.dependencies.push(second.identity.clone());
        second.dependencies.push(first.identity.clone());
        assert!(matches!(
            registry.activate_packages(vec![first, second]),
            Err(KernelError::Conflict { .. })
        ));
        assert!(registry.definitions().unwrap().is_empty());
    }

    #[test]
    fn activation_rejects_missing_dynamic_dependencies() {
        let registry = VerbRegistry::new();
        let mut definition = definition("dependent");
        definition
            .dependencies
            .push(VerbId::new("example:missing/missing@1.0.0").unwrap());
        assert!(matches!(
            registry.activate(definition),
            Err(KernelError::NotFound { .. })
        ));
        assert!(registry.definitions().unwrap().is_empty());
    }

    #[test]
    fn package_batch_is_atomic_on_duplicate_identity() {
        let registry = VerbRegistry::new();
        let existing = definition("existing");
        registry.activate(existing.clone()).unwrap();
        let duplicate = definition("new");
        let error = registry
            .activate_packages(vec![duplicate.clone(), duplicate])
            .unwrap_err();
        assert!(matches!(error, KernelError::Conflict { .. }));
        assert!(registry.current(&existing.identity).unwrap().is_some());
        assert!(
            registry
                .current(&VerbId::new("example:new/new@1.0.0").unwrap())
                .unwrap()
                .is_none()
        );
    }

    struct UppercaseExecutor;

    impl DynamicVerbExecutor for UppercaseExecutor {
        fn invoke(&self, call: &DynamicVerbCall) -> Result<DynamicVerbResult, KernelError> {
            let DynamicValue::String(value) = &call.input else {
                return Err(KernelError::InvalidRequest {
                    message: "uppercase expects a string".into(),
                });
            };
            Ok(DynamicVerbResult {
                verb: call.verb.clone(),
                function: call.function.clone(),
                output: DynamicValue::String(value.to_uppercase()),
            })
        }
    }

    #[test]
    fn dynamic_executor_runs_without_kernel_verb_dispatch() {
        let registry = VerbRegistry::new();
        let identity = VerbId::new("example:text/uppercase@1.0.0").unwrap();
        registry
            .activate(
                VerbDefinition::new(identity.clone(), "uppercase", "uppercase", "Uppercase")
                    .with_contract(DynamicType::String, DynamicType::String),
            )
            .unwrap();
        registry
            .register_executor(identity.clone(), Arc::new(UppercaseExecutor))
            .unwrap();
        let result = registry
            .execute(&DynamicVerbCall {
                verb: identity,
                function: "uppercase".into(),
                input: DynamicValue::String("hello".into()),
            })
            .unwrap();
        assert_eq!(result.output, DynamicValue::String("HELLO".into()));
    }

    #[test]
    fn package_reconciliation_removes_deleted_dynamic_tools() {
        let registry = VerbRegistry::new();
        registry.activate(definition("old")).unwrap();
        let removed = registry
            .reconcile_packages(vec![definition("new")])
            .unwrap();
        assert_eq!(removed, vec![VerbId::new("example:old/old@1.0.0").unwrap()]);
        assert!(
            registry
                .current(&VerbId::new("example:old/old@1.0.0").unwrap())
                .unwrap()
                .is_none()
        );
        assert_eq!(registry.tool_descriptors().unwrap().len(), 1);
    }

    #[test]
    fn dynamic_verbs_activate_replace_and_deactivate() {
        let registry = VerbRegistry::new();
        let identity = definition("uppercase").identity.clone();
        assert_eq!(registry.activate(definition("uppercase")).unwrap(), 1);
        assert_eq!(registry.activate(definition("uppercase")).unwrap(), 1);
        assert_eq!(registry.activate(definition("uppercase")).unwrap(), 1);
        assert_eq!(registry.current(&identity).unwrap().unwrap().generation, 1);
        assert!(registry.deactivate(&identity).unwrap().is_some());
        assert!(registry.current(&identity).unwrap().is_none());
    }
}
