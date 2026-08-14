//! Open-ended typed verb definitions and their active generations.
//!
//! This registry is intentionally independent of the installed ten-verb
//! compatibility layer. It is the seam through which verb packages become
//! discoverable and hot-swappable without changing kernel code.

use crate::{DynamicType, KernelError, VerbId};
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
    #[serde(default = "default_manifest_file")]
    pub component: String,
}

fn default_manifest_file() -> String {
    "component.wasm".to_owned()
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
            identity,
            &self.function,
            &self.model_name,
            &self.description,
        );
        definition.docs = self.docs.clone();
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

#[derive(Clone, Debug)]
pub struct ActiveVerb {
    pub definition: VerbDefinition,
    pub generation: u64,
}

#[derive(Clone, Default)]
pub struct VerbRegistry {
    entries: Arc<std::sync::RwLock<BTreeMap<VerbId, Arc<ActiveVerb>>>>,
}

impl VerbRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn activate(&self, definition: VerbDefinition) -> Result<u64, KernelError> {
        let mut entries = self.entries.write().map_err(|_| KernelError::Handler {
            message: "verb registry lock poisoned".to_owned(),
        })?;
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
        Ok(generation)
    }

    pub fn deactivate(&self, identity: &VerbId) -> Result<Option<Arc<ActiveVerb>>, KernelError> {
        self.entries
            .write()
            .map_err(|_| KernelError::Handler {
                message: "verb registry lock poisoned".to_owned(),
            })
            .map(|mut entries| entries.remove(identity))
    }

    pub fn current(&self, identity: &VerbId) -> Result<Option<Arc<ActiveVerb>>, KernelError> {
        self.entries
            .read()
            .map_err(|_| KernelError::Handler {
                message: "verb registry lock poisoned".to_owned(),
            })
            .map(|entries| entries.get(identity).cloned())
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

    fn definition(name: &str) -> VerbDefinition {
        VerbDefinition::new(
            VerbId::new(format!("example:{name}/{name}@1.0.0")).unwrap(),
            name,
            name,
            format!("{name} description"),
        )
    }

    #[test]
    fn manifest_discovery_is_name_agnostic_and_preserves_artifact_path() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("uppercase-package");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(
            package.join("verb.toml"),
            "identity = 'example:text/uppercase@1.0.0'\nfunction = 'uppercase'\nmodel_name = 'uppercase'\ndescription = 'Uppercase text'\ndocs = ['tool.md']\ncomponent = 'uppercase.wasm'\n",
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
    fn dynamic_verbs_activate_replace_and_deactivate() {
        let registry = VerbRegistry::new();
        let identity = definition("uppercase").identity.clone();
        assert_eq!(registry.activate(definition("uppercase")).unwrap(), 1);
        assert_eq!(registry.activate(definition("uppercase")).unwrap(), 2);
        assert_eq!(registry.current(&identity).unwrap().unwrap().generation, 2);
        assert!(registry.deactivate(&identity).unwrap().is_some());
        assert!(registry.current(&identity).unwrap().is_none());
    }
}
