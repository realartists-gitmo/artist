//! Open-ended typed verb definitions and their active generations.
//!
//! This registry is intentionally independent of the installed ten-verb
//! compatibility layer. It is the seam through which verb packages become
//! discoverable and hot-swappable without changing kernel code.

use crate::{KernelError, VerbId};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerbDefinition {
    pub identity: VerbId,
    pub function: String,
    pub model_name: String,
    pub description: String,
    pub docs: Vec<String>,
    pub source: Option<PathBuf>,
    pub artifact: Option<PathBuf>,
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
        }
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
