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
    /// Symbolic name of the package-owned typed routing extractor.
    #[serde(default)]
    pub extractor: Option<String>,
    /// External model-schema adapter metadata. The kernel stores the
    /// identity; conversion remains outside the semantic execution path.
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default = "default_manifest_file")]
    pub component: String,
}

fn default_manifest_file() -> String {
    "component.wasm".to_owned()
}

fn wit_type_matches(
    resolve: &wit_parser::Resolve,
    wit_type: wit_parser::Type,
    dynamic_type: &DynamicType,
) -> bool {
    use wit_parser::{Type, TypeDefKind};
    match (wit_type, dynamic_type) {
        (Type::Bool, DynamicType::Bool)
        | (Type::S8, DynamicType::S8)
        | (Type::S16, DynamicType::S16)
        | (Type::U32, DynamicType::U32)
        | (Type::U64, DynamicType::U64)
        | (Type::S32, DynamicType::S32)
        | (Type::S64, DynamicType::S64)
        | (Type::U8, DynamicType::U8)
        | (Type::U16, DynamicType::U16)
        | (Type::F32, DynamicType::F32)
        | (Type::F64, DynamicType::F64)
        | (Type::Char, DynamicType::Char)
        | (Type::String, DynamicType::String | DynamicType::ResourceUri) => true,
        (Type::Id(id), dynamic_type) => match &resolve.types[id].kind {
            TypeDefKind::Type(inner) => wit_type_matches(resolve, *inner, dynamic_type),
            TypeDefKind::List(inner) => {
                matches!(dynamic_type, DynamicType::List(value) if wit_type_matches(resolve, *inner, value))
            }
            TypeDefKind::Option(inner) => {
                matches!(dynamic_type, DynamicType::Option(value) if wit_type_matches(resolve, *inner, value))
            }
            TypeDefKind::Tuple(tuple) => matches!(dynamic_type, DynamicType::Tuple(values)
                if values.len() == tuple.types.len()
                    && values.iter().zip(&tuple.types).all(|(value, ty)| wit_type_matches(resolve, *ty, value))),
            TypeDefKind::Record(record) => matches!(dynamic_type, DynamicType::Record(values)
                if values.len() == record.fields.len()
                    && record.fields.iter().all(|field| values.get(&field.name).is_some_and(|value| wit_type_matches(resolve, field.ty, value)))),
            TypeDefKind::Result(result) => matches!(dynamic_type, DynamicType::Result { ok, err }
                if result.ok.map(|ty| ok.as_deref().is_some_and(|value| wit_type_matches(resolve, ty, value))).unwrap_or(ok.is_none())
                    && result.err.map(|ty| err.as_deref().is_some_and(|value| wit_type_matches(resolve, ty, value))).unwrap_or(err.is_none())),
            TypeDefKind::Enum(enumeration) => matches!(dynamic_type, DynamicType::Enum(values)
                if values == &enumeration.cases.iter().map(|case| case.name.clone()).collect::<Vec<_>>()),
            TypeDefKind::Variant(variant) => matches!(dynamic_type, DynamicType::Variant(cases)
            if cases.len() == variant.cases.len()
                && variant.cases.iter().all(|case| cases.get(&case.name).is_some_and(|value| match (case.ty, value) {
                    (None, None) => true,
                    (Some(ty), Some(value)) => wit_type_matches(resolve, ty, value),
                    _ => false,
                }))),
            TypeDefKind::Flags(flags) => matches!(dynamic_type, DynamicType::Flags(values)
                if values.iter().all(|value| flags.flags.iter().any(|flag| flag.name == *value))
                    && values.len() == values.iter().collect::<std::collections::BTreeSet<_>>().len()),
            _ => false,
        },
        _ => false,
    }
}

/// Convert a parsed WIT type into the Artist-owned dynamic contract type.
///
/// This is deliberately structural: aliases are followed through the WIT
/// resolver, while unsupported resource/future/stream handles are rejected
/// before a package can be published.
pub fn dynamic_type_from_wit(
    resolve: &wit_parser::Resolve,
    wit_type: wit_parser::Type,
) -> Result<DynamicType, KernelError> {
    use wit_parser::{Type, TypeDefKind};
    match wit_type {
        Type::Bool => Ok(DynamicType::Bool),
        Type::S8 => Ok(DynamicType::S8),
        Type::S16 => Ok(DynamicType::S16),
        Type::S32 => Ok(DynamicType::S32),
        Type::S64 => Ok(DynamicType::S64),
        Type::U8 => Ok(DynamicType::U8),
        Type::U16 => Ok(DynamicType::U16),
        Type::U32 => Ok(DynamicType::U32),
        Type::U64 => Ok(DynamicType::U64),
        Type::F32 => Ok(DynamicType::F32),
        Type::F64 => Ok(DynamicType::F64),
        Type::String => Ok(DynamicType::String),
        Type::Char => Ok(DynamicType::Char),
        Type::Id(id) => {
            let definition = &resolve.types[id];
            if definition
                .name
                .as_deref()
                .is_some_and(|name| matches!(name, "uri" | "resource-uri"))
            {
                return Ok(DynamicType::ResourceUri);
            }
            match &definition.kind {
                TypeDefKind::Type(inner) => dynamic_type_from_wit(resolve, *inner),
                TypeDefKind::List(inner) => Ok(DynamicType::List(Box::new(dynamic_type_from_wit(
                    resolve, *inner,
                )?))),
                TypeDefKind::Option(inner) => Ok(DynamicType::Option(Box::new(
                    dynamic_type_from_wit(resolve, *inner)?,
                ))),
                TypeDefKind::Tuple(tuple) => Ok(DynamicType::Tuple(
                    tuple
                        .types
                        .iter()
                        .copied()
                        .map(|ty| dynamic_type_from_wit(resolve, ty))
                        .collect::<Result<_, _>>()?,
                )),
                TypeDefKind::Record(record) => Ok(DynamicType::Record(
                    record
                        .fields
                        .iter()
                        .map(|field| {
                            Ok((
                                field.name.clone(),
                                dynamic_type_from_wit(resolve, field.ty)?,
                            ))
                        })
                        .collect::<Result<BTreeMap<_, _>, KernelError>>()?,
                )),
                TypeDefKind::Result(result) => Ok(DynamicType::Result {
                    ok: result
                        .ok
                        .map(|ty| dynamic_type_from_wit(resolve, ty).map(Box::new))
                        .transpose()?,
                    err: result
                        .err
                        .map(|ty| dynamic_type_from_wit(resolve, ty).map(Box::new))
                        .transpose()?,
                }),
                TypeDefKind::Enum(enumeration) => Ok(DynamicType::Enum(
                    enumeration
                        .cases
                        .iter()
                        .map(|case| case.name.clone())
                        .collect(),
                )),
                TypeDefKind::Variant(variant) => Ok(DynamicType::Variant(
                    variant
                        .cases
                        .iter()
                        .map(|case| {
                            Ok((
                                case.name.clone(),
                                case.ty
                                    .map(|ty| dynamic_type_from_wit(resolve, ty))
                                    .transpose()?,
                            ))
                        })
                        .collect::<Result<BTreeMap<_, _>, KernelError>>()?,
                )),
                TypeDefKind::Flags(flags) => Ok(DynamicType::Flags(
                    flags.flags.iter().map(|flag| flag.name.clone()).collect(),
                )),
                kind => Err(KernelError::InvalidRequest {
                    message: format!("unsupported WIT type in dynamic contract: {kind:?}"),
                }),
            }
        }
        kind => Err(KernelError::InvalidRequest {
            message: format!("unsupported WIT primitive in dynamic contract: {kind:?}"),
        }),
    }
}

/// Parse a WIT source file and derive the scalar item contract for a canonical
/// batch function. The outer list/result shape is validated here and is not
/// exposed as the model-facing input shape.
pub fn dynamic_contract_from_wit(
    path: &Path,
    interface_name: &str,
    function_name: &str,
) -> Result<(DynamicType, DynamicType), KernelError> {
    let mut resolve = wit_parser::Resolve::default();
    let (package_id, _) = resolve
        .push_path(path.parent().unwrap_or(path))
        .map_err(|error| KernelError::InvalidRequest {
            message: format!("invalid WIT contract {}: {error}", path.display()),
        })?;
    let interface_id = resolve.packages[package_id]
        .interfaces
        .get(interface_name)
        .copied()
        .ok_or_else(|| KernelError::InvalidRequest {
            message: format!(
                "WIT interface {interface_name} is absent from {}",
                path.display()
            ),
        })?;
    let function = resolve.interfaces[interface_id]
        .functions
        .get(function_name)
        .ok_or_else(|| KernelError::InvalidRequest {
            message: format!(
                "WIT function {function_name} is absent from {}",
                path.display()
            ),
        })?;
    if function.params.len() != 1 {
        return Err(KernelError::InvalidRequest {
            message: format!("WIT function {function_name} must have exactly one parameter"),
        });
    }
    let output = function.result.ok_or_else(|| KernelError::InvalidRequest {
        message: format!("WIT function {function_name} must return one result"),
    })?;
    let batch_input = dynamic_type_from_wit(&resolve, function.params[0].ty)?;
    let scalar_input = match batch_input {
        DynamicType::List(inner) => *inner,
        _ => {
            return Err(KernelError::InvalidRequest {
                message: format!("WIT function {function_name} must accept list<request>"),
            });
        }
    };
    let batch_output = dynamic_type_from_wit(&resolve, output)?;
    let (scalar_output, error_output) = match batch_output {
        DynamicType::List(inner) => match *inner {
            DynamicType::Result {
                ok: Some(ok),
                err: Some(err),
            } => (*ok, *err),
            _ => {
                return Err(KernelError::InvalidRequest {
                    message: format!(
                        "WIT function {function_name} must return list<result<response,error>>"
                    ),
                });
            }
        },
        _ => {
            return Err(KernelError::InvalidRequest {
                message: format!("WIT function {function_name} must return a result list"),
            });
        }
    };
    let observer = resolve.interfaces[interface_id]
        .functions
        .get("observe")
        .ok_or_else(|| KernelError::InvalidRequest {
            message: format!(
                "WIT interface {interface_name} must export observe(result<Response, Error>) -> string"
            ),
        })?;
    let observer_input = observer
        .params
        .first()
        .and_then(|param| dynamic_type_from_wit(&resolve, param.ty).ok());
    let observer_output = observer
        .result
        .and_then(|result| dynamic_type_from_wit(&resolve, result).ok());
    let expected_observer_input = DynamicType::Result {
        ok: Some(Box::new(scalar_output.clone())),
        err: Some(Box::new(error_output)),
    };
    if observer.params.len() != 1
        || observer_input != Some(expected_observer_input)
        || observer_output != Some(DynamicType::String)
    {
        return Err(KernelError::InvalidRequest {
            message: format!(
                "WIT interface {interface_name} observer must accept one result and return string"
            ),
        });
    }
    Ok((scalar_input, scalar_output))
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
        definition.extractor = self.extractor.clone();
        definition.schema = self.schema.clone();
        definition.dependencies = self
            .dependencies
            .iter()
            .map(|dependency| {
                VerbId::new(dependency).map_err(|message| KernelError::InvalidRequest { message })
            })
            .collect::<Result<_, _>>()?;
        let mut wit_contract = None;
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
            let wit_function = interface.functions.get(&self.function).ok_or_else(|| {
                KernelError::InvalidRequest {
                    message: format!(
                        "WIT function {} is absent from {}",
                        self.function,
                        wit_path.display()
                    ),
                }
            })?;
            let (derived_input, derived_output) =
                dynamic_contract_from_wit(&wit_path, identity.interface(), &self.function)?;
            wit_contract = Some((derived_input.clone(), derived_output.clone()));
            let input_type = self
                .input_type
                .as_deref()
                .map(DynamicType::named)
                .transpose()?;
            let output_type = self
                .output_type
                .as_deref()
                .map(DynamicType::named)
                .transpose()?;
            let input_matches = input_type.as_ref().is_none_or(|ty| ty == &derived_input);
            let output_matches = output_type.as_ref().is_none_or(|ty| ty == &derived_output);
            if (input_type.is_some() && !input_matches) || !output_matches {
                return Err(KernelError::InvalidRequest {
                    message: format!(
                        "WIT function {} does not match the declared dynamic contract",
                        self.function
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
        } else if let Some((input, output)) = wit_contract {
            definition.input_type = Some(input);
            definition.output_type = Some(output);
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
    pub extractor: Option<String>,
    pub schema: Option<String>,
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
            extractor: None,
            schema: None,
            input_type: None,
            output_type: None,
        }
    }

    pub fn with_contract(mut self, input: DynamicType, output: DynamicType) -> Self {
        self.input_type = Some(input);
        self.output_type = Some(output);
        self
    }

    pub fn with_extractor(mut self, extractor: impl Into<String>) -> Self {
        self.extractor = Some(extractor.into());
        self
    }

    pub fn with_schema_adapter(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    pub fn with_source(mut self, source: impl Into<PathBuf>, artifact: Option<PathBuf>) -> Self {
        self.source = Some(source.into());
        self.artifact = artifact;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerbToolDescriptor {
    pub identity: VerbId,
    pub name: String,
    pub description: String,
    pub docs: Vec<String>,
    pub input_type: Option<DynamicType>,
    pub output_type: Option<DynamicType>,
    pub schema: Option<String>,
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
        let _ = active;
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
        if definitions.is_empty() {
            return Ok(Vec::new());
        }
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
        self.validate_result_for_definition(call, result, &active.definition)
    }

    pub fn validate_result_for_lease(
        &self,
        call: &DynamicVerbCall,
        result: &DynamicVerbResult,
        lease: &VerbLease,
    ) -> Result<(), KernelError> {
        self.validate_result_for_definition(call, result, &lease.active.definition)
    }

    fn validate_result_for_definition(
        &self,
        call: &DynamicVerbCall,
        result: &DynamicVerbResult,
        definition: &VerbDefinition,
    ) -> Result<(), KernelError> {
        let output_type =
            definition
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
                input_type: active.definition.input_type.clone(),
                output_type: active.definition.output_type.clone(),
                schema: active.definition.schema.clone(),
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
            "package example:text@1.0.0; interface text { transform: func(input: string) -> string }",
        )
        .unwrap();
        let manifest = VerbPackageManifest::from_toml(
            "identity = 'example:text/transform@1.0.0'\nfunction = 'missing'\nmodel_name = 'transform'\ndescription = 'Transform'\nwit = 'contract.wit'\n",
        )
        .unwrap();
        assert!(matches!(
            manifest.definition(root.path()),
            Err(KernelError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn manifest_wit_contract_accepts_nested_list_and_option_shapes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("contract.wit"),
            "package example:text@1.0.0; interface text { type error = string; transform: func(input: list<list<string>>) -> list<result<option<string>, error>>; observe: func(response: result<option<string>, error>) -> string; }",
        )
        .unwrap();
        let manifest = VerbPackageManifest::from_toml(
            "identity = 'example:text/transform@1.0.0'\nfunction = 'transform'\nmodel_name = 'transform'\ndescription = 'Transform'\nwit = 'contract.wit'\ninput_type = 'list<string>'\noutput_type = 'option<string>'\n",
        )
        .unwrap();
        let definition = manifest.definition(root.path()).unwrap();
        assert_eq!(
            definition.input_type,
            Some(DynamicType::List(Box::new(DynamicType::String)))
        );
        assert_eq!(
            definition.output_type,
            Some(DynamicType::Option(Box::new(DynamicType::String)))
        );
    }

    #[test]
    fn derives_dynamic_contract_types_from_wit_records_and_results() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("contract.wit");
        std::fs::write(
            &path,
            "package example:text@1.0.0; interface text { type uri = string; record payload { uri: uri, name: string, tags: list<string> } enum mode { fast, slow } transform: func(input: list<payload>) -> list<result<list<string>, mode>>; observe: func(response: result<list<string>, mode>) -> string; }",
        )
        .unwrap();
        let mut resolve = wit_parser::Resolve::default();
        let (package_id, _) = resolve.push_path(&path).unwrap();
        let interface_id = resolve.packages[package_id].interfaces["text"];
        let function = &resolve.interfaces[interface_id].functions["transform"];
        let input = match dynamic_type_from_wit(&resolve, function.params[0].ty).unwrap() {
            DynamicType::List(inner) => *inner,
            other => panic!("expected batch input list, got {other:?}"),
        };
        let output = match dynamic_type_from_wit(&resolve, function.result.unwrap()).unwrap() {
            DynamicType::List(inner) => match *inner {
                DynamicType::Result { ok, err } => {
                    assert_eq!(
                        err,
                        Some(Box::new(DynamicType::Enum(vec![
                            "fast".into(),
                            "slow".into(),
                        ])))
                    );
                    *ok.expect("result ok type")
                }
                other => panic!("expected result item, got {other:?}"),
            },
            other => panic!("expected batch output list, got {other:?}"),
        };
        assert_eq!(
            input,
            DynamicType::Record(BTreeMap::from([
                ("uri".into(), DynamicType::ResourceUri),
                ("name".into(), DynamicType::String),
                (
                    "tags".into(),
                    DynamicType::List(Box::new(DynamicType::String)),
                ),
            ]))
        );
        assert_eq!(output, DynamicType::List(Box::new(DynamicType::String)));

        let manifest = VerbPackageManifest::from_toml(
            "identity = 'example:text/transform@1.0.0'\nfunction = 'transform'\nmodel_name = 'transform'\ndescription = 'Transform'\nwit = 'contract.wit'\n",
        )
        .unwrap();
        let definition = manifest.definition(root.path()).unwrap();
        assert_eq!(definition.input_type, Some(input));
        assert_eq!(definition.output_type, Some(output));
    }

    #[test]
    fn manifest_discovery_is_name_agnostic_and_preserves_artifact_path() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("transform-package");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("transform.wasm"), b"component-artifact").unwrap();
        std::fs::write(
            package.join("verb.toml"),
            "identity = 'example:text/transform@1.0.0'\nfunction = 'transform'\nmodel_name = 'transform'\ndescription = 'Transform text'\ndocs = ['tool.md']\ninput_type = 'string'\noutput_type = 'string'\ncomponent = 'transform.wasm'\n",
        )
        .unwrap();
        let definitions = discover_verb_packages(root.path()).unwrap();
        assert_eq!(definitions.len(), 1);
        assert_eq!(
            definitions[0].identity.to_string(),
            "example:text/transform@1.0.0"
        );
        assert_eq!(
            definitions[0].artifact,
            Some(package.join("transform.wasm"))
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
        let identity = VerbId::new("example:text/transform@1.0.0").unwrap();
        let definition =
            VerbDefinition::new(identity.clone(), "transform", "transform", "Transform")
                .with_contract(DynamicType::String, DynamicType::String);
        registry.activate(definition).unwrap();
        let call = DynamicVerbCall {
            verb: identity.clone(),
            function: "transform".into(),
            input: crate::DynamicValue::String("hello".into()),
        };
        let active = registry.validate_call(&call).unwrap();
        assert_eq!(active.generation, 1);
        let result = DynamicVerbResult {
            verb: identity,
            function: "transform".into(),
            output: crate::DynamicValue::String("HELLO".into()),
        };
        registry.validate_result(&call, &result).unwrap();
    }

    #[test]
    fn active_tool_descriptors_are_derived_from_dynamic_packages() {
        let registry = VerbRegistry::new();
        let mut definition = definition("transform");
        definition.docs.push("transform.md".into());
        definition = definition
            .with_contract(DynamicType::String, DynamicType::String)
            .with_extractor("uri-record")
            .with_schema_adapter("json-schema-v1");
        registry.activate(definition).unwrap();
        let tools = registry.tool_descriptors().unwrap();
        assert_eq!(tools[0].name, "transform");
        assert_eq!(tools[0].docs, vec!["transform.md"]);
        assert_eq!(tools[0].input_type, Some(DynamicType::String));
        assert_eq!(tools[0].output_type, Some(DynamicType::String));
        assert_eq!(tools[0].schema.as_deref(), Some("json-schema-v1"));
        assert_eq!(
            tools[0].identity.to_string(),
            "example:transform/transform@1.0.0"
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

    struct TransformExecutor;

    impl DynamicVerbExecutor for TransformExecutor {
        fn invoke(&self, call: &DynamicVerbCall) -> Result<DynamicVerbResult, KernelError> {
            let DynamicValue::String(value) = &call.input else {
                return Err(KernelError::InvalidRequest {
                    message: "transform expects a string".into(),
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
        let identity = VerbId::new("example:text/transform@1.0.0").unwrap();
        registry
            .activate(
                VerbDefinition::new(identity.clone(), "transform", "transform", "Transform")
                    .with_contract(DynamicType::String, DynamicType::String),
            )
            .unwrap();
        registry
            .register_executor(identity.clone(), Arc::new(TransformExecutor))
            .unwrap();
        let result = registry
            .execute(&DynamicVerbCall {
                verb: identity,
                function: "transform".into(),
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
        let identity = definition("transform").identity.clone();
        assert_eq!(registry.activate(definition("transform")).unwrap(), 1);
        assert_eq!(registry.activate(definition("transform")).unwrap(), 1);
        assert_eq!(registry.activate(definition("transform")).unwrap(), 1);
        assert_eq!(registry.current(&identity).unwrap().unwrap().generation, 1);
        assert!(registry.deactivate(&identity).unwrap().is_some());
        assert!(registry.current(&identity).unwrap().is_none());
    }

    #[test]
    fn contract_change_creates_generation_and_old_lease_stays_pinned() {
        let registry = VerbRegistry::new();
        let identity = VerbId::new("example:text/transform@1.0.0").unwrap();
        registry
            .activate(
                VerbDefinition::new(identity.clone(), "transform", "transform", "Transform")
                    .with_contract(DynamicType::String, DynamicType::String),
            )
            .unwrap();
        let old_lease = registry.acquire(&identity).unwrap();
        let old_call = DynamicVerbCall {
            verb: identity.clone(),
            function: "transform".into(),
            input: DynamicValue::String("old".into()),
        };

        let next_generation = registry
            .activate(
                VerbDefinition::new(identity.clone(), "transform", "transform", "Transform")
                    .with_contract(DynamicType::U32, DynamicType::U32),
            )
            .unwrap();
        assert_eq!(old_lease.generation(), 1);
        assert_eq!(next_generation, 2);
        assert!(registry.validate_call(&old_call).is_err());

        let old_result = DynamicVerbResult {
            verb: identity,
            function: "transform".into(),
            output: DynamicValue::String("old result".into()),
        };
        assert!(
            registry
                .validate_result_for_lease(&old_call, &old_result, &old_lease)
                .is_ok()
        );
    }
}
