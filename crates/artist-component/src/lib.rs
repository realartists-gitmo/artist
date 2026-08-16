//! Host-side implementation of the Artist WebAssembly Component ABI.

/// Canonical shared resource contract. Resource extensions and model-facing
/// tools are required to converge on these types at their component boundary.
pub mod resource_bindings {
    wasmtime::component::bindgen!({
        path: "wit/resource-surface",
        world: "types-world",
        additional_derives: [serde::Serialize, serde::Deserialize],
    });
    pub use self::exports::artist;
}
pub mod resource_async_extension_bindings {
    wasmtime::component::bindgen!({
        path: "wit/resource-surface",
        world: "extension-world",
        imports: { default: async },
        exports: { default: async },
        additional_derives: [serde::Serialize, serde::Deserialize],
    });
}

/// Open contract identities. Package discovery, not this host module, defines
/// which tool interfaces are installed.
pub mod contracts {
    use serde::{Deserialize, Serialize};
    use std::{fmt, str::FromStr};

    #[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
    pub struct ContractId {
        pub namespace: String,
        pub interface: String,
        pub major: u16,
    }

    impl ContractId {
        pub const NAMESPACE: &'static str = "artist:tool";
        pub const MAJOR: u16 = 1;

        pub fn universal(interface: impl Into<String>) -> Self {
            Self {
                namespace: Self::NAMESPACE.to_owned(),
                interface: interface.into(),
                major: Self::MAJOR,
            }
        }
    }

    impl fmt::Display for ContractId {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "{}:{}@{}",
                self.namespace, self.interface, self.major
            )
        }
    }

    impl FromStr for ContractId {
        type Err = String;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            let (name, version) = value
                .rsplit_once('@')
                .ok_or_else(|| "contract ID must end with @<major>".to_owned())?;
            let major = version
                .parse::<u16>()
                .map_err(|_| "contract major version is not a u16".to_owned())?;
            let (namespace, verb) = name
                .rsplit_once(':')
                .ok_or_else(|| "contract ID must be namespace:verb@major".to_owned())?;
            let handler = Self {
                namespace: namespace.to_owned(),
                interface: verb.to_owned(),
                major,
            };
            Ok(handler)
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct ContractDescriptor {
        pub id: ContractId,
        pub interface: String,
        pub imports: Vec<ContractId>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ExecutionContext {
        pub working_uri: Option<String>,
        pub environment: Vec<(String, String)>,
        pub cancellation_token: Option<String>,
        pub deadline_ms: Option<u64>,
        pub correlation_id: Option<String>,
    }

    impl ExecutionContext {
        pub fn inherited(
            working_uri: Option<String>,
            cancellation_token: Option<String>,
            deadline_ms: Option<u64>,
            correlation_id: Option<String>,
        ) -> Self {
            Self {
                working_uri,
                environment: Vec::new(),
                cancellation_token,
                deadline_ms,
                correlation_id,
            }
        }

        pub fn with_environment(
            mut self,
            name: impl Into<String>,
            value: impl Into<String>,
        ) -> Self {
            self.environment.push((name.into(), value.into()));
            self
        }
    }

    impl ContractDescriptor {
        pub fn universal(interface: impl Into<String>) -> Self {
            let interface = interface.into();
            Self {
                id: ContractId::universal(interface.clone()),
                interface,
                imports: Vec::new(),
            }
        }
    }
}

use artist_kernel::{
    DynamicType, DynamicValue, DynamicVerbCall, KernelError, KernelHandle, ResourceUri, VerbId,
};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// Validate a verb/resource artifact before it is published into an active
/// package generation. This deliberately uses Wasmtime's component parser,
/// not a magic-byte check, so core modules and malformed binaries are rejected.
pub fn validate_component_artifact(path: impl AsRef<Path>) -> Result<(), String> {
    inspect_component_exports(path).map(|_| ())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentExportDescriptor {
    pub name: String,
    pub implements: Option<String>,
    pub kind: String,
}

pub fn dynamic_value_to_component_val(
    value: &DynamicValue,
) -> Result<wasmtime::component::Val, String> {
    use wasmtime::component::Val;
    Ok(match value {
        DynamicValue::Bool(value) => Val::Bool(*value),
        DynamicValue::S8(value) => Val::S8(*value),
        DynamicValue::S16(value) => Val::S16(*value),
        DynamicValue::S32(value) => Val::S32(*value),
        DynamicValue::S64(value) => Val::S64(*value),
        DynamicValue::U8(value) => Val::U8(*value),
        DynamicValue::U16(value) => Val::U16(*value),
        DynamicValue::U32(value) => Val::U32(*value),
        DynamicValue::U64(value) => Val::U64(*value),
        DynamicValue::F32(value) => Val::Float32(*value),
        DynamicValue::F64(value) => Val::Float64(*value),
        DynamicValue::Char(value) => Val::Char(*value),
        DynamicValue::String(value) => Val::String(value.clone()),
        DynamicValue::ResourceUri(value) => Val::String(value.to_string()),
        DynamicValue::List(values) => Val::List(
            values
                .iter()
                .map(dynamic_value_to_component_val)
                .collect::<Result<_, _>>()?,
        ),
        DynamicValue::Tuple(values) => Val::Tuple(
            values
                .iter()
                .map(dynamic_value_to_component_val)
                .collect::<Result<_, _>>()?,
        ),
        DynamicValue::Record(values) => {
            let preferred = [
                "uri",
                "lines",
                "anchor",
                "text",
                "ending",
                "entries",
                "at",
                "before",
                "after",
                "code",
                "message",
                "reason",
                "content",
                "changed",
                "diff",
                "old",
                "new",
                "hunks",
                "query",
                "pattern",
                "args",
                "cwd",
                "environment",
            ];
            let mut fields = values.iter().collect::<Vec<_>>();
            fields.sort_by_key(|(name, _)| {
                preferred
                    .iter()
                    .position(|candidate| candidate == name)
                    .unwrap_or(preferred.len())
            });
            Val::Record(
                fields
                    .into_iter()
                    .map(|(name, value)| Ok((name.clone(), dynamic_value_to_component_val(value)?)))
                    .collect::<Result<_, String>>()?,
            )
        }
        DynamicValue::Option(value) => Val::Option(
            value
                .as_deref()
                .map(dynamic_value_to_component_val)
                .transpose()?
                .map(Box::new),
        ),
        DynamicValue::Result(Ok(value)) => {
            Val::Result(Ok(Some(Box::new(dynamic_value_to_component_val(value)?))))
        }
        DynamicValue::Result(Err(value)) => {
            Val::Result(Err(Some(Box::new(dynamic_value_to_component_val(value)?))))
        }
        DynamicValue::Enum(value) => Val::Enum(value.clone()),
        DynamicValue::Variant(name, value) => Val::Variant(
            name.clone(),
            value
                .as_deref()
                .map(dynamic_value_to_component_val)
                .transpose()?
                .map(Box::new),
        ),
        DynamicValue::Flags(values) => Val::Flags(values.clone()),
    })
}

/// Lower a dynamic value using the discovered Component Model type. Unlike
/// the shape-only lowerer above, this preserves WIT record field order and
/// therefore works for reflective calls whose record layout is not sorted by
/// field name.
fn dynamic_value_to_component_val_with_type(
    value: &DynamicValue,
    ty: &wasmtime::component::Type,
) -> Result<wasmtime::component::Val, String> {
    use wasmtime::component::{Type, Val};
    match (value, ty) {
        (DynamicValue::Bool(value), Type::Bool) => Ok(Val::Bool(*value)),
        (DynamicValue::S8(value), Type::S8) => Ok(Val::S8(*value)),
        (DynamicValue::U8(value), Type::U8) => Ok(Val::U8(*value)),
        (DynamicValue::S16(value), Type::S16) => Ok(Val::S16(*value)),
        (DynamicValue::U16(value), Type::U16) => Ok(Val::U16(*value)),
        (DynamicValue::S32(value), Type::S32) => Ok(Val::S32(*value)),
        (DynamicValue::U32(value), Type::U32) => Ok(Val::U32(*value)),
        (DynamicValue::S64(value), Type::S64) => Ok(Val::S64(*value)),
        (DynamicValue::U64(value), Type::U64) => Ok(Val::U64(*value)),
        (DynamicValue::F32(value), Type::Float32) => Ok(Val::Float32(*value)),
        (DynamicValue::F64(value), Type::Float64) => Ok(Val::Float64(*value)),
        (DynamicValue::Char(value), Type::Char) => Ok(Val::Char(*value)),
        (DynamicValue::String(value), Type::String) => Ok(Val::String(value.clone())),
        (DynamicValue::ResourceUri(value), Type::String) => Ok(Val::String(value.to_string())),
        (DynamicValue::List(values), Type::List(list)) => values
            .iter()
            .map(|value| dynamic_value_to_component_val_with_type(value, &list.ty()))
            .collect::<Result<Vec<_>, _>>()
            .map(Val::List),
        (DynamicValue::Tuple(values), Type::Tuple(tuple)) => tuple
            .types()
            .zip(values)
            .map(|(ty, value)| dynamic_value_to_component_val_with_type(value, &ty))
            .collect::<Result<Vec<_>, _>>()
            .map(Val::Tuple),
        (DynamicValue::Record(values), Type::Record(record)) => record
            .fields()
            .map(|field| {
                let value = values
                    .get(field.name)
                    .ok_or_else(|| format!("missing record field {}", field.name))?;
                Ok((
                    field.name.to_owned(),
                    dynamic_value_to_component_val_with_type(value, &field.ty)
                        .map_err(|error| format!("record field {}: {}", field.name, error))?,
                ))
            })
            .collect::<Result<Vec<_>, String>>()
            .map(Val::Record),
        (DynamicValue::Option(None), Type::Option(_)) => Ok(Val::Option(None)),
        (DynamicValue::Option(Some(value)), Type::Option(option)) => {
            Ok(Val::Option(Some(Box::new(
                dynamic_value_to_component_val_with_type(value, &option.ty())?,
            ))))
        }
        (DynamicValue::Enum(value), Type::Enum(_)) => Ok(Val::Enum(value.clone())),
        (DynamicValue::Variant(name, value), Type::Variant(variant)) => {
            let case = variant
                .cases()
                .find(|case| case.name == name)
                .ok_or_else(|| format!("unknown variant case {name}"))?;
            let payload = match (&case.ty, value) {
                (None, None) => None,
                (Some(ty), Some(value)) => Some(Box::new(
                    dynamic_value_to_component_val_with_type(value, ty)?,
                )),
                _ => return Err(format!("invalid payload for variant case {name}")),
            };
            Ok(Val::Variant(name.clone(), payload))
        }
        (DynamicValue::Result(Ok(value)), Type::Result(result)) => {
            let value = dynamic_value_to_component_val_with_type(
                value,
                &result
                    .ok()
                    .ok_or_else(|| "result has no ok type".to_owned())?,
            )?;
            Ok(Val::Result(Ok(Some(Box::new(value)))))
        }
        (DynamicValue::Result(Err(value)), Type::Result(result)) => {
            let value = dynamic_value_to_component_val_with_type(
                value,
                &result
                    .err()
                    .ok_or_else(|| "result has no err type".to_owned())?,
            )?;
            Ok(Val::Result(Err(Some(Box::new(value)))))
        }
        _ => Err(format!(
            "dynamic value does not match Component Model type {ty:?}"
        )),
    }
}

pub fn component_val_to_dynamic_value(
    value: &wasmtime::component::Val,
) -> Result<DynamicValue, String> {
    use wasmtime::component::Val;
    Ok(match value {
        Val::Bool(value) => DynamicValue::Bool(*value),
        Val::S8(value) => DynamicValue::S8(*value),
        Val::S16(value) => DynamicValue::S16(*value),
        Val::S32(value) => DynamicValue::S32(*value),
        Val::S64(value) => DynamicValue::S64(*value),
        Val::U8(value) => DynamicValue::U8(*value),
        Val::U16(value) => DynamicValue::U16(*value),
        Val::U32(value) => DynamicValue::U32(*value),
        Val::U64(value) => DynamicValue::U64(*value),
        Val::Float32(value) => DynamicValue::F32(*value),
        Val::Float64(value) => DynamicValue::F64(*value),
        Val::Char(value) => DynamicValue::Char(*value),
        Val::String(value) => DynamicValue::String(value.clone()),
        Val::List(values) => DynamicValue::List(
            values
                .iter()
                .map(component_val_to_dynamic_value)
                .collect::<Result<_, _>>()?,
        ),
        Val::Tuple(values) => DynamicValue::Tuple(
            values
                .iter()
                .map(component_val_to_dynamic_value)
                .collect::<Result<_, _>>()?,
        ),
        Val::Record(values) => DynamicValue::Record(
            values
                .iter()
                .map(|(name, value)| Ok((name.clone(), component_val_to_dynamic_value(value)?)))
                .collect::<Result<_, String>>()?,
        ),
        Val::Option(value) => DynamicValue::Option(
            value
                .as_deref()
                .map(component_val_to_dynamic_value)
                .transpose()?
                .map(Box::new),
        ),
        Val::Result(Ok(Some(value))) => {
            DynamicValue::Result(Ok(Box::new(component_val_to_dynamic_value(value)?)))
        }
        Val::Result(Err(Some(value))) => {
            DynamicValue::Result(Err(Box::new(component_val_to_dynamic_value(value)?)))
        }
        Val::Result(Ok(None)) | Val::Result(Err(None)) => {
            return Err("unit result payloads need an explicit unit dynamic type".to_owned());
        }
        Val::Enum(value) => DynamicValue::Enum(value.clone()),
        Val::Variant(name, value) => DynamicValue::Variant(
            name.clone(),
            value
                .as_deref()
                .map(component_val_to_dynamic_value)
                .transpose()?
                .map(Box::new),
        ),
        Val::Flags(values) => DynamicValue::Flags(values.clone()),
        Val::Map(_) | Val::Resource(_) | Val::Future(_) | Val::Stream(_) | Val::ErrorContext(_) => {
            return Err("unsupported Component Model value kind".to_owned());
        }
    })
}

/// Lift a Component Model value using the registered contract type. A plain
/// lift cannot distinguish Artist-owned URI aliases from ordinary WIT
/// strings; the contract-directed form preserves that identity recursively.
pub fn component_val_to_dynamic_value_with_type(
    value: &wasmtime::component::Val,
    ty: &DynamicType,
) -> Result<DynamicValue, String> {
    use wasmtime::component::Val;
    match (value, ty) {
        (Val::String(value), DynamicType::ResourceUri) => Ok(DynamicValue::ResourceUri(
            artist_kernel::ResourceUri::parse(value).map_err(|error| error.to_string())?,
        )),
        (Val::List(values), DynamicType::List(element)) => Ok(DynamicValue::List(
            values
                .iter()
                .map(|value| component_val_to_dynamic_value_with_type(value, element))
                .collect::<Result<_, _>>()?,
        )),
        (Val::Tuple(values), DynamicType::Tuple(types)) => {
            if values.len() != types.len() {
                return Err("tuple value does not match its registered arity".to_owned());
            }
            Ok(DynamicValue::Tuple(
                values
                    .iter()
                    .zip(types)
                    .map(|(value, ty)| component_val_to_dynamic_value_with_type(value, ty))
                    .collect::<Result<_, _>>()?,
            ))
        }
        (Val::Record(values), DynamicType::Record(types)) => {
            if values.len() != types.len() {
                return Err("record value does not match its registered fields".to_owned());
            }
            let mut output = std::collections::BTreeMap::new();
            for (name, value) in values {
                let ty = types
                    .get(name)
                    .ok_or_else(|| format!("record value contains unknown field {name}"))?;
                output.insert(
                    name.clone(),
                    component_val_to_dynamic_value_with_type(value, ty)?,
                );
            }
            Ok(DynamicValue::Record(output))
        }
        (Val::Option(value), DynamicType::Option(inner)) => Ok(DynamicValue::Option(
            value
                .as_deref()
                .map(|value| component_val_to_dynamic_value_with_type(value, inner))
                .transpose()?
                .map(Box::new),
        )),
        (Val::Result(Ok(Some(value))), DynamicType::Result { ok: Some(ty), .. }) => {
            Ok(DynamicValue::Result(Ok(Box::new(
                component_val_to_dynamic_value_with_type(value, ty)?,
            ))))
        }
        (Val::Result(Err(Some(value))), DynamicType::Result { err: Some(ty), .. }) => {
            Ok(DynamicValue::Result(Err(Box::new(
                component_val_to_dynamic_value_with_type(value, ty)?,
            ))))
        }
        (Val::Result(Ok(None)), DynamicType::Result { ok: None, .. }) => {
            Err("unit result payloads need an explicit unit dynamic type".to_owned())
        }
        (Val::Result(Err(None)), DynamicType::Result { err: None, .. }) => {
            Err("unit result payloads need an explicit unit dynamic type".to_owned())
        }
        (Val::Variant(name, Some(value)), DynamicType::Variant(cases)) => {
            let Some(Some(ty)) = cases.get(name) else {
                return Err(format!(
                    "variant case {name} has no registered payload type"
                ));
            };
            Ok(DynamicValue::Variant(
                name.clone(),
                Some(Box::new(component_val_to_dynamic_value_with_type(
                    value, ty,
                )?)),
            ))
        }
        (Val::Variant(name, None), DynamicType::Variant(cases)) => {
            if !matches!(cases.get(name), Some(None)) {
                return Err(format!("variant case {name} has an unexpected payload"));
            }
            Ok(DynamicValue::Variant(name.clone(), None))
        }
        (Val::Enum(name), DynamicType::Enum(cases)) if cases.iter().any(|case| case == name) => {
            Ok(DynamicValue::Enum(name.clone()))
        }
        (Val::Flags(values), DynamicType::Flags(flags))
            if values
                .iter()
                .all(|value| flags.iter().any(|flag| flag == value)) =>
        {
            Ok(DynamicValue::Flags(values.clone()))
        }
        _ => component_val_to_dynamic_value(value),
    }
}

/// Invoke a validated component export using Wasmtime's dynamic Component
/// Model values. The caller supplies result slots because result types belong
/// to the discovered WIT contract, not to this host crate.
pub fn invoke_component_function(
    path: impl AsRef<Path>,
    verb: &VerbId,
    params: &[wasmtime::component::Val],
    results: &mut [wasmtime::component::Val],
) -> Result<(), String> {
    let path = path.as_ref();
    validate_verb_export(path, verb)?;
    let engine = wasmtime::Engine::default();
    let component = wasmtime::component::Component::from_file(&engine, path)
        .map_err(|error| format!("invalid WebAssembly component {}: {error}", path.display()))?;
    let linker = wasmtime::component::Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker
        .instantiate(&mut store, &component)
        .map_err(|error| format!("could not instantiate {}: {error}", path.display()))?;
    let function = instance
        .get_func(&mut store, verb.function())
        .ok_or_else(|| format!("component function {} disappeared after validation", verb))?;
    let function_type = function.ty(&store);
    if function_type.params().len() != params.len() {
        return Err(format!(
            "component function {} expects {} parameters, received {}",
            verb,
            function_type.params().len(),
            params.len()
        ));
    }
    if function_type.results().len() != results.len() {
        return Err(format!(
            "component function {} returns {} values, received {} result slots",
            verb,
            function_type.results().len(),
            results.len()
        ));
    }
    function
        .call(&mut store, params, results)
        .map_err(|error| format!("component function {} failed: {error}", verb))
}

/// Inspect a validated component without binding it to a closed world. The
/// `implements` annotation is the authoritative versioned contract identity;
/// callers can compare it with a discovered `VerbId` before publication.
pub fn validate_verb_export(path: impl AsRef<Path>, verb: &VerbId) -> Result<(), String> {
    let exports = inspect_component_exports(path)?;
    let export = exports
        .iter()
        .find(|export| export.name == verb.function())
        .ok_or_else(|| {
            format!(
                "component does not export function {} for {}",
                verb.function(),
                verb
            )
        })?;
    let implements_match = export
        .implements
        .as_deref()
        .is_some_and(|identity| identity == verb.as_str() || identity == verb.contract_versioned());
    if !implements_match {
        return Err(format!(
            "export {} has incompatible contract annotation {:?}; expected {} or {}",
            export.name,
            export.implements,
            verb,
            verb.contract_versioned()
        ));
    }
    if export.kind != "function" {
        return Err(format!(
            "export {} is not a component function",
            export.name
        ));
    }
    Ok(())
}

pub fn inspect_component_exports(
    path: impl AsRef<Path>,
) -> Result<Vec<ComponentExportDescriptor>, String> {
    let path = path.as_ref();
    let engine = wasmtime::Engine::default();
    let component = wasmtime::component::Component::from_file(&engine, path)
        .map_err(|error| format!("invalid WebAssembly component {}: {error}", path.display()))?;
    Ok(component
        .component_type()
        .exports(&engine)
        .map(|(name, export)| ComponentExportDescriptor {
            name: name.to_owned(),
            implements: export.implements.map(str::to_owned),
            kind: match export.ty {
                wasmtime::component::types::ComponentItem::ComponentFunc(_) => "function",
                wasmtime::component::types::ComponentItem::CoreFunc(_) => "core-function",
                wasmtime::component::types::ComponentItem::Module(_) => "module",
                wasmtime::component::types::ComponentItem::Component(_) => "component",
                wasmtime::component::types::ComponentItem::ComponentInstance(_) => "instance",
                wasmtime::component::types::ComponentItem::Type(_) => "type",
                wasmtime::component::types::ComponentItem::Resource(_) => "resource",
            }
            .to_owned(),
        })
        .collect())
}

/// Errors raised while loading or invoking a component.
#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("component load failed: {0}")]
    Load(#[source] anyhow::Error),
    #[error("component build failed: {diagnostics}")]
    Build { diagnostics: String },
    #[error("component invocation failed: {0}")]
    Invoke(#[source] anyhow::Error),
    #[error("component returned malformed {field} JSON")]
    MalformedResponse { field: &'static str },
    #[error("component invocation cancelled")]
    Cancelled,
    #[error("component capability denied: {0}")]
    CapabilityDenied(String),
    #[error("invalid resource handle: {0}")]
    InvalidHandle(String),
    #[error("no active component for package: {0}")]
    NotActive(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentMetadata {
    pub name: String,
    pub version: String,
    pub abi_version: String,
    pub interfaces: Vec<String>,
    pub required_capabilities: Vec<String>,
}

pub const ABI_VERSION: &str = "1.0";

/// Minimal host state for the first ABI slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComponentPhase {
    Claim,
    Invoke,
}

pub struct HostState {
    wasi: wasmtime_wasi::WasiCtx,
    table: wasmtime::component::ResourceTable,
    capabilities: HashSet<String>,
    kernel: Option<KernelHandle>,
    context: artist_kernel::InvocationContext,
    scope: artist_kernel::InvocationScope,
    phase: ComponentPhase,
    limits: wasmtime::StoreLimits,
}

impl Default for HostState {
    fn default() -> Self {
        Self::with_capabilities(std::iter::empty::<String>())
    }
}

impl HostState {
    fn with_capabilities<I>(capabilities: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        Self {
            wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
            table: wasmtime::component::ResourceTable::new(),
            capabilities: capabilities.into_iter().collect(),
            kernel: None,
            context: artist_kernel::InvocationContext::default(),
            scope: artist_kernel::InvocationScope::new(artist_kernel::InvocationContext::default()),
            phase: ComponentPhase::Invoke,
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(package::WasmExecutionLimits::default().max_memory_bytes)
                .table_elements(package::WasmExecutionLimits::default().max_table_elements)
                .instances(package::WasmExecutionLimits::default().max_instances)
                .memories(package::WasmExecutionLimits::default().max_memories)
                .tables(package::WasmExecutionLimits::default().max_tables)
                .build(),
        }
    }

    fn with_kernel<I>(capabilities: I, kernel: KernelHandle) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let mut state = Self::with_capabilities(capabilities);
        state.kernel = Some(kernel);
        state
    }

    fn with_kernel_context<I>(
        capabilities: I,
        kernel: KernelHandle,
        context: artist_kernel::InvocationContext,
    ) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        Self::with_kernel_scope(
            capabilities,
            kernel,
            artist_kernel::InvocationScope::new(context),
        )
    }

    fn with_kernel_scope<I>(
        capabilities: I,
        kernel: KernelHandle,
        scope: artist_kernel::InvocationScope,
    ) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let mut state = Self::with_kernel(capabilities, kernel);
        state.wasi = wasmtime_wasi::WasiCtxBuilder::new()
            .preopened_dir(
                "/",
                "/",
                wasmtime_wasi::DirPerms::all(),
                wasmtime_wasi::FilePerms::all(),
            )
            .expect("preopen host root for component invocation")
            .build();
        state.context = scope.context.clone();
        state.scope = scope;
        state
    }

    fn with_scope<I>(capabilities: I, scope: artist_kernel::InvocationScope) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let mut state = Self::with_capabilities(capabilities);
        // Claiming is intentionally filesystem-capable: extension authors may
        // inspect arbitrary host paths when deciding ownership. The caller
        // accepts the resulting I/O cost and any policy implications.
        state.wasi = wasmtime_wasi::WasiCtxBuilder::new()
            .preopened_dir(
                "/",
                "/",
                wasmtime_wasi::DirPerms::all(),
                wasmtime_wasi::FilePerms::all(),
            )
            .expect("preopen host root for resource claim")
            .build();
        state.context = scope.context.clone();
        state.scope = scope;
        state.phase = ComponentPhase::Claim;
        state
    }
}

fn new_store(engine: &wasmtime::Engine, state: HostState) -> wasmtime::Store<HostState> {
    let cancellation = state.scope.cancellation.clone();
    let deadline = state.scope.remaining_deadline();
    let started = Instant::now();
    let mut store = wasmtime::Store::new(engine, state);
    store.limiter(|state| &mut state.limits);
    // A single runtime-owned ticker advances every component engine. The
    // callback keeps ordinary calls alive, but turns cancellation/deadline
    // expiry into a Wasmtime interruption even when the guest is CPU-bound.
    store.set_epoch_deadline(1);
    store.epoch_deadline_callback(move |_store| {
        if cancellation.is_cancelled()
            || deadline.is_some_and(|deadline| started.elapsed() >= deadline)
        {
            return Ok(wasmtime::UpdateDeadline::Interrupt);
        }
        Ok(wasmtime::UpdateDeadline::Continue(1))
    });
    // Fuel is a hard ceiling for pathological guests. Claim calls can use a
    // smaller budget later; ordinary invocations get the shared default.
    let _ = store.set_fuel(10_000_000);
    store
}

impl wasmtime_wasi::WasiView for HostState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn anchor_from_string(value: &str) -> artist_kernel::Anchor {
    artist_kernel::Anchor::from_tokens(
        value
            .trim_start_matches('#')
            .split('.')
            .map(str::to_owned)
            .collect(),
    )
}

#[cfg(test)]
fn dynamic_read_result_to_kernel(
    result: &DynamicValue,
) -> Result<artist_kernel::ReadResult, artist_kernel::KernelError> {
    use artist_kernel::KernelError;
    let DynamicValue::Result(result) = result else {
        return Err(KernelError::Handler {
            message: "resource read returned an invalid result value".to_owned(),
        });
    };
    match result {
        Ok(value) => {
            let DynamicValue::Variant(name, Some(value)) = value.as_ref() else {
                return Err(KernelError::Handler {
                    message: "resource read returned an invalid success variant".to_owned(),
                });
            };
            let DynamicValue::Record(fields) = value.as_ref() else {
                return Err(KernelError::Handler {
                    message: "resource read result payload is not a record".to_owned(),
                });
            };
            match name.as_str() {
                "text" => {
                    let uri = dynamic_wit_uri(fields, "uri")?;
                    let lines = dynamic_wit_lines(fields.get("lines"))?;
                    Ok(artist_kernel::ReadResult::Text(
                        artist_kernel::AnchoredText { uri, lines },
                    ))
                }
                "directory" => {
                    let uri = dynamic_wit_uri(fields, "uri")?;
                    let entries = match fields.get("entries") {
                        Some(DynamicValue::List(entries)) => entries
                            .iter()
                            .map(dynamic_wit_uri_value)
                            .collect::<Result<Vec<_>, _>>()?,
                        _ => {
                            return Err(KernelError::Handler {
                                message: "resource directory result has no entries".to_owned(),
                            });
                        }
                    };
                    Ok(artist_kernel::ReadResult::Directory { uri, entries })
                }
                _ => Err(KernelError::Handler {
                    message: format!("resource read returned unknown result variant {name}"),
                }),
            }
        }
        Err(error) => Err(dynamic_resource_error_to_kernel(error, "read")?),
    }
}

fn dynamic_resource_error_to_kernel(
    value: &DynamicValue,
    verb: &str,
) -> Result<artist_kernel::KernelError, artist_kernel::KernelError> {
    use artist_kernel::KernelError;
    let DynamicValue::Record(fields) = value else {
        return Err(KernelError::Handler {
            message: "resource error is not a record".to_owned(),
        });
    };
    let code = match fields.get("code") {
        Some(DynamicValue::Enum(code)) => code.clone(),
        _ => {
            return Err(KernelError::Handler {
                message: "resource error has no code".to_owned(),
            });
        }
    };
    let uri = match fields.get("uri") {
        Some(DynamicValue::Option(Some(uri))) => Some(dynamic_wit_string_value(uri)?),
        Some(DynamicValue::String(uri)) => Some(uri.clone()),
        _ => None,
    };
    let message = match fields.get("message") {
        Some(DynamicValue::String(message)) => message.clone(),
        _ => "resource operation failed".to_owned(),
    };
    Ok(resource_error_parts_to_kernel(code, uri, message, verb))
}

fn dynamic_wit_string_value(value: &DynamicValue) -> Result<String, artist_kernel::KernelError> {
    use artist_kernel::KernelError;
    match value {
        DynamicValue::String(value) => Ok(value.clone()),
        DynamicValue::ResourceUri(value) => Ok(value.to_string()),
        DynamicValue::Enum(value) => Ok(value.clone()),
        _ => Err(KernelError::Handler {
            message: "resource value is not a string".to_owned(),
        }),
    }
}

fn dynamic_wit_uri_value(
    value: &DynamicValue,
) -> Result<artist_kernel::ResourceUri, artist_kernel::KernelError> {
    use artist_kernel::KernelError;
    artist_kernel::ResourceUri::parse(&dynamic_wit_string_value(value)?).map_err(|error| {
        KernelError::InvalidUri {
            message: error.to_string(),
        }
    })
}

fn dynamic_wit_uri(
    fields: &std::collections::BTreeMap<String, DynamicValue>,
    name: &str,
) -> Result<artist_kernel::ResourceUri, artist_kernel::KernelError> {
    use artist_kernel::KernelError;
    fields
        .get(name)
        .ok_or_else(|| KernelError::Handler {
            message: format!("resource result has no {name}"),
        })
        .and_then(dynamic_wit_uri_value)
}

fn dynamic_wit_lines(
    value: Option<&DynamicValue>,
) -> Result<Vec<artist_kernel::AnchoredLine>, artist_kernel::KernelError> {
    use artist_kernel::KernelError;
    let Some(DynamicValue::List(lines)) = value else {
        return Err(KernelError::Handler {
            message: "resource text result has no lines".to_owned(),
        });
    };
    lines
        .iter()
        .map(|line| {
            let DynamicValue::Record(fields) = line else {
                return Err(KernelError::Handler {
                    message: "resource text line is not a record".to_owned(),
                });
            };
            let anchor_value = fields.get("anchor").ok_or_else(|| KernelError::Handler {
                message: "resource text line has no anchor".to_owned(),
            })?;
            let anchor = match anchor_value {
                DynamicValue::String(value) => value.clone(),
                DynamicValue::List(values) => {
                    let tokens = values
                        .iter()
                        .map(dynamic_wit_string_value)
                        .collect::<Result<Vec<_>, _>>()?;
                    format!("#{}", tokens.join("."))
                }
                _ => {
                    return Err(KernelError::Handler {
                        message: "resource text line anchor is not a string or token list"
                            .to_owned(),
                    });
                }
            };
            let text = dynamic_wit_string_value(fields.get("text").ok_or_else(|| {
                KernelError::Handler {
                    message: "resource text line has no text".to_owned(),
                }
            })?)?;
            let ending = match dynamic_wit_string_value(fields.get("ending").ok_or_else(|| {
                KernelError::Handler {
                    message: "resource text line has no ending".to_owned(),
                }
            })?)?
            .as_str()
            {
                "none" => artist_kernel::LineEnding::None,
                "lf" => artist_kernel::LineEnding::Lf,
                "crlf" => artist_kernel::LineEnding::Crlf,
                "cr" => artist_kernel::LineEnding::Cr,
                value => {
                    return Err(KernelError::Handler {
                        message: format!("unknown resource line ending {value}"),
                    });
                }
            };
            Ok(artist_kernel::AnchoredLine {
                anchor: anchor_from_string(&anchor),
                text,
                ending,
            })
        })
        .collect()
}

fn dynamic_wit_text(
    value: Option<&DynamicValue>,
) -> Result<artist_kernel::AnchoredText, artist_kernel::KernelError> {
    let DynamicValue::Record(fields) =
        value.ok_or_else(|| artist_kernel::KernelError::Handler {
            message: "resource text result has no text".to_owned(),
        })?
    else {
        return Err(artist_kernel::KernelError::Handler {
            message: "resource text result is not a record".to_owned(),
        });
    };
    Ok(artist_kernel::AnchoredText {
        uri: dynamic_wit_uri(fields, "uri")?,
        lines: dynamic_wit_lines(fields.get("lines"))?,
    })
}

fn dynamic_wit_diff(
    value: Option<&DynamicValue>,
) -> Result<artist_kernel::AnchoredDiff, artist_kernel::KernelError> {
    let DynamicValue::Record(fields) =
        value.ok_or_else(|| artist_kernel::KernelError::Handler {
            message: "resource edit result has no diff".to_owned(),
        })?
    else {
        return Err(artist_kernel::KernelError::Handler {
            message: "resource edit diff is not a record".to_owned(),
        });
    };
    let uri = dynamic_wit_uri(fields, "uri")?;
    let Some(DynamicValue::List(hunks)) = fields.get("hunks") else {
        return Err(artist_kernel::KernelError::Handler {
            message: "resource edit diff has no hunks".to_owned(),
        });
    };
    let hunks = hunks
        .iter()
        .map(|hunk| {
            let DynamicValue::Record(fields) = hunk else {
                return Err(artist_kernel::KernelError::Handler {
                    message: "resource edit hunk is not a record".to_owned(),
                });
            };
            Ok(artist_kernel::DiffHunk {
                old: dynamic_wit_lines(fields.get("old"))?,
                new: dynamic_wit_lines(fields.get("new"))?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(artist_kernel::AnchoredDiff { uri, hunks })
}

fn resource_error_parts_to_kernel(
    code: String,
    uri: Option<String>,
    message: String,
    verb: &str,
) -> artist_kernel::KernelError {
    let uri = uri.unwrap_or_default();
    match code.as_str() {
        "InvalidUri" => artist_kernel::KernelError::InvalidUri { message },
        "InvalidInput" => artist_kernel::KernelError::InvalidRequest { message },
        "InvalidPattern" => artist_kernel::KernelError::InvalidPattern { message },
        "NotFound" => artist_kernel::KernelError::NotFound { uri },
        "WrongKind" => artist_kernel::KernelError::WrongKind { message },
        "InvalidAnchor" => artist_kernel::KernelError::InvalidAnchor { message },
        "StaleAnchor" => artist_kernel::KernelError::StaleAnchor { message },
        "Immutable" => artist_kernel::KernelError::Immutable { uri },
        "Unsupported" => artist_kernel::KernelError::UnsupportedVerb {
            verb: verb.to_owned(),
            uri,
        },
        "PermissionDenied" => artist_kernel::KernelError::PermissionDenied { uri },
        "Conflict" => artist_kernel::KernelError::Conflict { uri },
        "NotEmpty" => artist_kernel::KernelError::NotEmpty { uri },
        "Aborted" => artist_kernel::KernelError::Aborted { message },
        _ => artist_kernel::KernelError::Handler { message },
    }
}

fn kernel_error_to_typed(
    error: artist_kernel::KernelError,
    target: Option<String>,
) -> resource_bindings::artist::resource::types::Error {
    use artist_kernel::KernelError;
    let (code, uri, message) = match error {
        KernelError::InvalidUri { message } => (
            resource_bindings::artist::resource::types::ErrorCode::InvalidUri,
            target,
            message,
        ),
        KernelError::UnsupportedUri { uri } => (
            resource_bindings::artist::resource::types::ErrorCode::Unsupported,
            Some(uri),
            "URI scheme is not supported".to_owned(),
        ),
        KernelError::NoHandler { uri } => (
            resource_bindings::artist::resource::types::ErrorCode::Unsupported,
            Some(uri),
            "no handler is registered for this URI".to_owned(),
        ),
        KernelError::UnsupportedVerb { verb, uri } => (
            resource_bindings::artist::resource::types::ErrorCode::Unsupported,
            Some(uri),
            format!("verb {verb} is not supported for this resource"),
        ),
        KernelError::InvalidRequest { message } => (
            resource_bindings::artist::resource::types::ErrorCode::InvalidInput,
            target,
            message,
        ),
        KernelError::InvalidPattern { message } => (
            resource_bindings::artist::resource::types::ErrorCode::InvalidPattern,
            target,
            message,
        ),
        KernelError::InvalidAnchor { message } => (
            resource_bindings::artist::resource::types::ErrorCode::InvalidAnchor,
            target,
            message,
        ),
        KernelError::StaleAnchor { message } => (
            resource_bindings::artist::resource::types::ErrorCode::StaleAnchor,
            target,
            message,
        ),
        KernelError::WrongKind { message } => (
            resource_bindings::artist::resource::types::ErrorCode::WrongKind,
            target,
            message,
        ),
        KernelError::NotFound { uri } => (
            resource_bindings::artist::resource::types::ErrorCode::NotFound,
            Some(uri),
            "resource was not found".to_owned(),
        ),
        KernelError::AlreadyExists { uri } => (
            resource_bindings::artist::resource::types::ErrorCode::Conflict,
            Some(uri),
            "resource conflict".to_owned(),
        ),
        KernelError::Immutable { uri } => (
            resource_bindings::artist::resource::types::ErrorCode::Immutable,
            Some(uri),
            "resource is immutable".to_owned(),
        ),
        KernelError::PermissionDenied { uri } => (
            resource_bindings::artist::resource::types::ErrorCode::PermissionDenied,
            Some(uri),
            "permission denied".to_owned(),
        ),
        KernelError::Conflict { uri } => (
            resource_bindings::artist::resource::types::ErrorCode::Conflict,
            Some(uri),
            "resource conflict".to_owned(),
        ),
        KernelError::NotEmpty { uri } => (
            resource_bindings::artist::resource::types::ErrorCode::NotEmpty,
            Some(uri),
            "resource is not empty".to_owned(),
        ),
        KernelError::Aborted { message } => (
            resource_bindings::artist::resource::types::ErrorCode::Aborted,
            target,
            message,
        ),
        KernelError::Handler { message } => (
            resource_bindings::artist::resource::types::ErrorCode::Internal,
            target,
            message,
        ),
    };
    typed_error(code, message, uri)
}

fn typed_error_for<E>(
    code: resource_bindings::artist::resource::types::ErrorCode,
    message: String,
    uri: Option<String>,
) -> E
where
    E: serde::de::DeserializeOwned,
{
    serde_json::from_value(
        serde_json::to_value(typed_error(code, message, uri)).expect("typed error is serializable"),
    )
    .expect("WIT error types share the canonical error representation")
}

fn typed_error_from_kernel<E>(error: artist_kernel::KernelError, target: Option<String>) -> E
where
    E: serde::de::DeserializeOwned,
{
    let error = kernel_error_to_typed(error, target);
    serde_json::from_value(serde_json::to_value(error).expect("typed error is serializable"))
        .expect("WIT error types share the canonical error representation")
}

fn typed_error(
    code: resource_bindings::exports::artist::resource::types::ErrorCode,
    message: String,
    uri: Option<String>,
) -> resource_bindings::exports::artist::resource::types::Error {
    resource_bindings::exports::artist::resource::types::Error { code, uri, message }
}

/// Reflective host for components implementing package-owned WIT contracts.
pub struct TypedComponentHost {
    engine: wasmtime::Engine,
    component: wasmtime::component::Component,
    capabilities: HashSet<String>,
    dependencies: Vec<Arc<DynamicDependency>>,
}

#[derive(Clone)]
struct DynamicDependency {
    contract: String,
    engine: wasmtime::Engine,
    component: wasmtime::component::Component,
    capabilities: HashSet<String>,
    dependencies: Vec<Arc<DynamicDependency>>,
}

#[derive(Clone)]
struct DependencySpec {
    contract: String,
    bytes: Vec<u8>,
    dependencies: Vec<DependencySpec>,
}

impl TypedComponentHost {
    pub fn new_with_capabilities<I>(bytes: &[u8], capabilities: I) -> Result<Self, ComponentError>
    where
        I: IntoIterator<Item = String>,
    {
        Self::new_with_dependencies(bytes, capabilities, Vec::new())
    }

    pub fn new_with_dependencies<I>(
        bytes: &[u8],
        capabilities: I,
        dependencies: Vec<(String, Vec<u8>)>,
    ) -> Result<Self, ComponentError>
    where
        I: IntoIterator<Item = String>,
    {
        let dependencies = dependencies
            .into_iter()
            .map(|(contract, bytes)| DependencySpec {
                contract,
                bytes,
                dependencies: Vec::new(),
            })
            .collect();
        Self::new_with_dependency_specs(bytes, capabilities, dependencies)
    }

    fn new_with_dependency_specs<I>(
        bytes: &[u8],
        capabilities: I,
        dependencies: Vec<DependencySpec>,
    ) -> Result<Self, ComponentError>
    where
        I: IntoIterator<Item = String>,
    {
        let engine = package::component_engine()?;
        let component = wasmtime::component::Component::from_binary(&engine, bytes)
            .map_err(|e| ComponentError::Load(anyhow::anyhow!(format!("{e:#}"))))?;
        let capabilities = capabilities.into_iter().collect::<HashSet<_>>();
        fn load_dependencies(
            engine: &wasmtime::Engine,
            capabilities: &HashSet<String>,
            specs: Vec<DependencySpec>,
        ) -> Result<Vec<Arc<DynamicDependency>>, ComponentError> {
            specs
                .into_iter()
                .map(|spec| {
                    let component =
                        wasmtime::component::Component::from_binary(engine, &spec.bytes).map_err(
                            |error| ComponentError::Load(anyhow::anyhow!(error.to_string())),
                        )?;
                    let dependencies = load_dependencies(engine, capabilities, spec.dependencies)?;
                    Ok(Arc::new(DynamicDependency {
                        contract: spec.contract,
                        engine: engine.clone(),
                        component,
                        capabilities: capabilities.clone(),
                        dependencies,
                    }))
                })
                .collect()
        }
        let dependencies = load_dependencies(&engine, &capabilities, dependencies)?;
        Ok(Self {
            engine,
            component,
            capabilities,
            dependencies,
        })
    }

    /// Validate that the component has exactly the world selected by its
    /// contract. This is intentionally performed against the compiled
    /// component type, before instantiation or invocation.

    fn dynamic_linker(&self) -> Result<wasmtime::component::Linker<HostState>, ComponentError> {
        dynamic_linker_for(
            &self.engine,
            &self.component,
            &self.capabilities,
            &self.dependencies,
        )
    }

    /// Invoke a package-local export through its registered WIT contract.
    /// Contract shape comes from the active package definition, never from a
    /// central list of semantic verbs.
    pub fn invoke_dynamic_values(
        &self,
        function_name: &str,
        input: &DynamicValue,
        input_type: &DynamicType,
        output_type: &DynamicType,
        kernel: KernelHandle,
    ) -> Result<DynamicValue, ComponentError> {
        input
            .validate(input_type)
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
        let mut store = new_store(
            &self.engine,
            HostState::with_kernel_context(
                self.capabilities.iter().cloned(),
                kernel,
                artist_kernel::InvocationContext::default(),
            ),
        );
        let linker = self.dynamic_linker()?;
        let instance = linker
            .instantiate(&mut store, &self.component)
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
        let function = instance
            .get_func(&mut store, "invoke")
            .or_else(|| instance.get_func(&mut store, function_name))
            .ok_or_else(|| {
                ComponentError::Invoke(anyhow::anyhow!(format!(
                    "component exports neither invoke nor {function_name}"
                )))
            })?;
        let function_type = function.ty(&store);
        if function_type.params().len() != 1 || function_type.results().len() != 1 {
            return Err(ComponentError::Invoke(anyhow::anyhow!(format!(
                "dynamic function {function_name} must have one parameter and one result"
            ))));
        }
        let parameter = dynamic_value_to_component_val(input)
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error)))?;
        let mut results = function_type
            .results()
            .map(|_| wasmtime::component::Val::Bool(false))
            .collect::<Vec<_>>();
        function
            .call(&mut store, &[parameter], &mut results)
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
        let output = component_val_to_dynamic_value_with_type(&results[0], output_type)
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error)))?;
        output
            .validate(output_type)
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
        Ok(output)
    }

    /// Resolve a dynamic function's name and contract from the kernel's
    /// active `VerbId` lease before linking or invoking the component. This
    /// keeps package replacement and component invocation on the same
    /// generation-aware contract registry.
    pub fn invoke_dynamic_verb(
        &self,
        call: &DynamicVerbCall,
        kernel: &artist_kernel::Kernel,
        host: KernelHandle,
    ) -> Result<DynamicValue, ComponentError> {
        let lease = kernel
            .verb_registry()
            .acquire(&call.verb)
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
        let definition = lease.definition();
        if call.function != definition.function {
            return Err(ComponentError::Invoke(anyhow::anyhow!(format!(
                "dynamic call function {} does not match active package function {}",
                call.function, definition.function
            ))));
        }
        let input_type = definition.input_type.as_ref().ok_or_else(|| {
            ComponentError::Invoke(anyhow::anyhow!(format!(
                "verb {} has no registered input contract",
                call.verb
            )))
        })?;
        let output_type = definition.output_type.as_ref().ok_or_else(|| {
            ComponentError::Invoke(anyhow::anyhow!(format!(
                "verb {} has no registered output contract",
                call.verb
            )))
        })?;
        self.invoke_dynamic_values(
            &definition.function,
            &call.input,
            input_type,
            output_type,
            host,
        )
    }

    /// Invoke an arbitrary package-local WIT contract through the Component
    /// Model's reflective typed-value API. The package must export a root
    /// `invoke` function (or the supplied interface name) and may import the
    /// universal typed host interfaces.
    pub fn invoke_dynamic_json(
        &self,
        input: &serde_json::Value,
        export_name: &str,
        kernel: artist_kernel::KernelHandle,
    ) -> Result<serde_json::Value, ComponentError> {
        self.invoke_dynamic_json_with_context(
            input,
            export_name,
            kernel,
            artist_kernel::InvocationContext::default(),
        )
    }

    pub fn invoke_dynamic_json_with_context(
        &self,
        input: &serde_json::Value,
        export_name: &str,
        kernel: artist_kernel::KernelHandle,
        context: artist_kernel::InvocationContext,
    ) -> Result<serde_json::Value, ComponentError> {
        let mut store = new_store(
            &self.engine,
            HostState::with_kernel_context(self.capabilities.iter().cloned(), kernel, context),
        );
        let linker = self.dynamic_linker()?;
        let instance = linker
            .instantiate(&mut store, &self.component)
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
        let func = instance
            .get_func(&mut store, export_name)
            .or_else(|| {
                let interface = format!("artist:tool/{export_name}@1.0.0");
                let interface_index = instance
                    .get_export_index(&mut store, None, &interface)
                    .or_else(|| {
                        instance.get_export_index(
                            &mut store,
                            None,
                            format!("artist:tool/{export_name}").as_str(),
                        )
                    })?;
                let function_index =
                    instance.get_export_index(&mut store, Some(&interface_index), export_name)?;
                instance.get_func(&mut store, &function_index)
            })
            .or_else(|| instance.get_func(&mut store, "invoke"))
            .ok_or_else(|| {
                ComponentError::Invoke(anyhow::anyhow!(format!(
                    "dynamic tool exports neither invoke nor {export_name}"
                )))
            })?;
        let function_type = func.ty(&store);
        let params = function_type.params().collect::<Vec<_>>();
        let arguments = params
            .iter()
            .map(|(name, ty)| {
                let value = input
                    .get(*name)
                    .or_else(|| (params.len() == 1).then(|| input.get("requests").unwrap_or(input)))
                    .ok_or_else(|| {
                        ComponentError::Invoke(anyhow::anyhow!(format!(
                            "dynamic tool input is missing parameter {name}"
                        )))
                    })?;
                json_to_component_val(value, ty)
                    .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut results = func
            .ty(&store)
            .results()
            .map(|_| wasmtime::component::Val::Bool(false))
            .collect::<Vec<_>>();
        func.call(&mut store, &arguments, &mut results)
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
        let values = results
            .iter()
            .map(component_val_to_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(match values.as_slice() {
            [] => serde_json::Value::Null,
            [value] => value.clone(),
            values => serde_json::Value::Array(values.to_vec()),
        })
    }

    pub async fn invoke_dynamic_json_async_with_scope(
        &self,
        input: &serde_json::Value,
        export_name: &str,
        kernel: artist_kernel::KernelHandle,
        scope: artist_kernel::InvocationScope,
    ) -> Result<serde_json::Value, ComponentError> {
        let mut store = new_store(
            &self.engine,
            HostState::with_kernel_scope(self.capabilities.iter().cloned(), kernel, scope),
        );
        let linker = dynamic_linker_for_async(
            &self.engine,
            &self.component,
            &self.capabilities,
            &self.dependencies,
        )?;
        let instance = linker
            .instantiate_async(&mut store, &self.component)
            .await
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
        let func = instance
            .get_func(&mut store, export_name)
            .or_else(|| {
                let interface = format!("artist:tool/{export_name}@1.0.0");
                let interface_index = instance
                    .get_export_index(&mut store, None, &interface)
                    .or_else(|| {
                        instance.get_export_index(
                            &mut store,
                            None,
                            format!("artist:tool/{export_name}").as_str(),
                        )
                    })?;
                let function_index =
                    instance.get_export_index(&mut store, Some(&interface_index), export_name)?;
                instance.get_func(&mut store, &function_index)
            })
            .or_else(|| instance.get_func(&mut store, "invoke"))
            .ok_or_else(|| {
                ComponentError::Invoke(anyhow::anyhow!(format!(
                    "dynamic tool exports neither invoke nor {export_name}"
                )))
            })?;
        let function_type = func.ty(&store);
        let params = function_type.params().collect::<Vec<_>>();
        let arguments = params
            .iter()
            .map(|(name, ty)| {
                let value = input
                    .get(*name)
                    .or_else(|| (params.len() == 1).then(|| input.get("requests").unwrap_or(input)))
                    .ok_or_else(|| {
                        ComponentError::Invoke(anyhow::anyhow!(format!(
                            "dynamic tool input is missing parameter {name}"
                        )))
                    })?;
                json_to_component_val(value, ty)
                    .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut results = func
            .ty(&store)
            .results()
            .map(|_| wasmtime::component::Val::Bool(false))
            .collect::<Vec<_>>();
        func.call_async(&mut store, &arguments, &mut results)
            .await
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
        let values = results
            .iter()
            .map(component_val_to_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(match values.as_slice() {
            [] => serde_json::Value::Null,
            [value] => value.clone(),
            values => serde_json::Value::Array(values.to_vec()),
        })
    }

    pub fn validate_dynamic_export(&self, export_name: &str) -> Result<(), ComponentError> {
        let component_type = self.component.component_type();
        let exports = component_type
            .exports(&self.engine)
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        let short_export_name = export_name.rsplit(':').next().unwrap_or(export_name);
        if !exports.iter().any(|name| {
            *name == "invoke"
                || *name == export_name
                || *name == short_export_name
                || name.ends_with(&format!("/{export_name}"))
                || name.contains(&format!("/{export_name}@"))
                || name.ends_with(&format!("/{short_export_name}"))
                || name.contains(&format!("/{short_export_name}@"))
        }) {
            return Err(ComponentError::Build {
                diagnostics: format!(
                    "package-local WIT component exports neither invoke nor {export_name}"
                ),
            });
        }
        Ok(())
    }

    pub fn validate_dynamic_export_shape(&self, export_name: &str) -> Result<(), ComponentError> {
        let component_type = self.component.component_type();
        let exports = component_type
            .exports(&self.engine)
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        let short_export_name = export_name.rsplit(':').next().unwrap_or(export_name);
        if !exports.iter().any(|name| {
            *name == "invoke"
                || *name == export_name
                || *name == short_export_name
                || name.ends_with(&format!("/{export_name}"))
                || name.contains(&format!("/{export_name}@"))
                || name.ends_with(&format!("/{short_export_name}"))
                || name.contains(&format!("/{short_export_name}@"))
        }) {
            return Err(ComponentError::Build {
                diagnostics: format!(
                    "package-local WIT component exports neither invoke nor {export_name}"
                ),
            });
        }
        Ok(())
    }
}

fn dynamic_linker_for(
    engine: &wasmtime::Engine,
    component: &wasmtime::component::Component,
    _capabilities: &HashSet<String>,
    dependencies: &[Arc<DynamicDependency>],
) -> Result<wasmtime::component::Linker<HostState>, ComponentError> {
    let mut linker = wasmtime::component::Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
        .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
    for (import_name, _) in component.component_type().imports(engine) {
        if import_name.starts_with("wasi:")
            || (import_name.contains("artist:%resource/")
                || import_name.contains("artist:resource/"))
                && !import_name.contains("/host@")
        {
            continue;
        }
        if import_name.contains("artist:resource/host@") {
            define_resource_host(&mut linker, import_name)?;
            continue;
        }
        let dependency = dependencies.iter().find(|dependency| {
            contract_matches_import(&dependency.contract, import_name)
                || dependency
                    .component
                    .component_type()
                    .exports(&dependency.engine)
                    .any(|(_, export)| export.is_implements(import_name))
        });
        let Some(dependency) = dependency else {
            return Err(ComponentError::Load(anyhow::anyhow!(format!(
                "no active custom tool satisfies component import {import_name}"
            ))));
        };
        let mut instance = linker
            .instance(import_name)
            .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
        define_dependency_exports(&mut instance, Arc::clone(dependency))?;
    }
    Ok(linker)
}

/// Async counterpart of the package-local linker. Dependency exports are
/// ordinary typed component functions, but their implementation may itself
/// import another component. Keep that recursion inside Wasmtime's async
/// linker so custom A -> B -> C calls never block a runtime worker.
fn dynamic_linker_for_async(
    engine: &wasmtime::Engine,
    component: &wasmtime::component::Component,
    _capabilities: &HashSet<String>,
    dependencies: &[Arc<DynamicDependency>],
) -> Result<wasmtime::component::Linker<HostState>, ComponentError> {
    let mut linker = wasmtime::component::Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)
        .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
    for (import_name, _) in component.component_type().imports(engine) {
        if import_name.starts_with("wasi:")
            || (import_name.contains("artist:%resource/")
                || import_name.contains("artist:resource/"))
                && !import_name.contains("/host@")
        {
            continue;
        }
        if import_name.contains("artist:resource/host@") {
            define_resource_host_async(&mut linker, import_name)?;
            continue;
        }
        let dependency = dependencies.iter().find(|dependency| {
            contract_matches_import(&dependency.contract, import_name)
                || dependency
                    .component
                    .component_type()
                    .exports(&dependency.engine)
                    .any(|(_, export)| export.is_implements(import_name))
        });
        let Some(dependency) = dependency else {
            return Err(ComponentError::Load(anyhow::anyhow!(format!(
                "no active custom tool satisfies component import {import_name}"
            ))));
        };
        let mut instance = linker
            .instance(import_name)
            .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
        define_dependency_exports_async(&mut instance, Arc::clone(dependency))?;
    }
    Ok(linker)
}

fn define_resource_host(
    linker: &mut wasmtime::component::Linker<HostState>,
    import_name: &str,
) -> Result<(), ComponentError> {
    let mut instance = linker
        .instance(import_name)
        .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
    instance
        .func_wrap(
            "invoke",
            |mut caller: wasmtime::StoreContextMut<'_, HostState>,
             (verb, uri, input): (String, String, String)| {
                let kernel = caller
                    .data()
                    .kernel
                    .clone()
                    .ok_or_else(|| wasmtime::Error::msg("resource host has no kernel"))?;
                let scope = caller.data().scope.clone();
                futures::executor::block_on(invoke_resource_host(kernel, scope, verb, uri, input))
                    .map(|output| (output,))
                    .map_err(|error| wasmtime::Error::msg(error.to_string()))
            },
        )
        .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
    instance
        .func_wrap(
            "invoke-batch",
            |caller: wasmtime::StoreContextMut<'_, HostState>,
             (verb, uris, inputs): (String, Vec<String>, Vec<String>)| {
                let kernel = caller
                    .data()
                    .kernel
                    .clone()
                    .ok_or_else(|| wasmtime::Error::msg("resource host has no kernel"))?;
                let scope = caller.data().scope.clone();
                if uris.len() != inputs.len() {
                    return Err(wasmtime::Error::msg("resource host batch lengths differ"));
                }
                futures::executor::block_on(invoke_resource_host_batch(
                    kernel, scope, verb, uris, inputs,
                ))
                .map(|outputs| (outputs,))
                .map_err(|error| wasmtime::Error::msg(error.to_string()))
            },
        )
        .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
    Ok(())
}

fn define_resource_host_async(
    linker: &mut wasmtime::component::Linker<HostState>,
    import_name: &str,
) -> Result<(), ComponentError> {
    let mut instance = linker
        .instance(import_name)
        .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
    instance
        .func_new_async("invoke", |mut caller, _function, params, results| {
            Box::new(async move {
                let strings = params
                    .iter()
                    .map(|value| match value {
                        wasmtime::component::Val::String(value) => Ok(value.clone()),
                        _ => Err(wasmtime::Error::msg("resource host expects strings")),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if strings.len() != 3 || results.len() != 1 {
                    return Err(wasmtime::Error::msg("invalid resource host arity"));
                }
                let kernel = caller
                    .data()
                    .kernel
                    .clone()
                    .ok_or_else(|| wasmtime::Error::msg("resource host has no kernel"))?;
                let scope = caller.data().scope.clone();
                let output = invoke_resource_host(
                    kernel,
                    scope,
                    strings[0].clone(),
                    strings[1].clone(),
                    strings[2].clone(),
                )
                .await
                .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                results[0] = wasmtime::component::Val::String(output);
                Ok(())
            })
        })
        .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
    instance
        .func_new_async("invoke-batch", |mut caller, _function, params, results| {
            Box::new(async move {
                if params.len() != 3 || results.len() != 1 {
                    return Err(wasmtime::Error::msg("invalid resource host batch arity"));
                }
                let verb = match &params[0] {
                    wasmtime::component::Val::String(value) => value.clone(),
                    _ => return Err(wasmtime::Error::msg("invalid batch verb")),
                };
                let uris = match &params[1] {
                    wasmtime::component::Val::List(values) => values
                        .iter()
                        .map(|value| match value {
                            wasmtime::component::Val::String(value) => Ok(value.clone()),
                            _ => Err(wasmtime::Error::msg("invalid batch uri")),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    _ => return Err(wasmtime::Error::msg("invalid batch uris")),
                };
                let inputs = match &params[2] {
                    wasmtime::component::Val::List(values) => values
                        .iter()
                        .map(|value| match value {
                            wasmtime::component::Val::String(value) => Ok(value.clone()),
                            _ => Err(wasmtime::Error::msg("invalid batch input")),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    _ => return Err(wasmtime::Error::msg("invalid batch inputs")),
                };
                let kernel = caller
                    .data()
                    .kernel
                    .clone()
                    .ok_or_else(|| wasmtime::Error::msg("resource host has no kernel"))?;
                let scope = caller.data().scope.clone();
                let outputs = invoke_resource_host_batch(kernel, scope, verb, uris, inputs)
                    .await
                    .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                results[0] = wasmtime::component::Val::List(
                    outputs
                        .into_iter()
                        .map(wasmtime::component::Val::String)
                        .collect(),
                );
                Ok(())
            })
        })
        .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
    Ok(())
}

async fn invoke_resource_host(
    kernel: KernelHandle,
    scope: artist_kernel::InvocationScope,
    verb_name: String,
    uri: String,
    input: String,
) -> Result<String, KernelError> {
    let mut input_value: serde_json::Value =
        serde_json::from_str(&input).map_err(|error| KernelError::InvalidRequest {
            message: format!("invalid resource host input: {error}"),
        })?;
    let resource_uri = match ResourceUri::parse(&uri) {
        Ok(uri) => uri,
        Err(error) => {
            return Ok(serde_json::json!({
                "err": tools::kernel_error_to_json_host(error, &uri)
            })
            .to_string());
        }
    };
    let input_type = match kernel.universal_input_type(verb_name.clone(), resource_uri) {
        Ok(input_type) => input_type,
        Err(error) => {
            return Ok(serde_json::json!({
                "err": tools::kernel_error_to_json_host(error, &uri)
            })
            .to_string());
        }
    };
    if input_type
        .as_ref()
        .is_none_or(|ty| matches!(ty, DynamicType::Record(fields) if fields.contains_key("uri")))
    {
        if let serde_json::Value::Object(fields) = &mut input_value {
            fields.insert("uri".to_owned(), serde_json::Value::String(uri.clone()));
        }
    }
    let dynamic = match input_type {
        Some(input_type) => match tools::json_to_dynamic_typed(&input_value, &input_type) {
            Ok(value) => value,
            Err(error) => {
                return Ok(serde_json::json!({
                    "err": tools::kernel_error_to_json_host(error, &uri)
                })
                .to_string());
            }
        },
        None => match tools::json_to_dynamic_host(input_value, &uri) {
            Ok(value) => value,
            Err(error) => {
                return Ok(serde_json::json!({
                    "err": tools::kernel_error_to_json_host(error, &uri)
                })
                .to_string());
            }
        },
    };
    let result = kernel
        .execute_universal_with_scope(verb_name, dynamic, scope.clone())
        .await;
    let output = match result {
        Ok(mut values) => match values.pop() {
            Some(value) => {
                let stdout = value.result.output.clone();
                serde_json::json!({"ok": tools::dynamic_to_json_host(stdout)})
            }
            None => {
                let error = KernelError::Handler {
                    message: "resource host returned no result".to_owned(),
                };
                serde_json::json!({"err": {"code": "internal", "uri": uri, "message": error.to_string()}})
            }
        },
        Err(error) => {
            serde_json::json!({"err": tools::kernel_error_to_json_host(error, &uri)})
        }
    };
    serde_json::to_string(&output).map_err(|error| KernelError::Handler {
        message: format!("could not encode resource host output: {error}"),
    })
}

async fn invoke_resource_host_batch(
    kernel: KernelHandle,
    scope: artist_kernel::InvocationScope,
    verb: String,
    uris: Vec<String>,
    inputs: Vec<String>,
) -> Result<Vec<String>, KernelError> {
    if scope.mutation_transaction().is_some() {
        return futures::future::try_join_all(uris.into_iter().zip(inputs).map(|(uri, input)| {
            invoke_resource_host(kernel.clone(), scope.child(), verb.clone(), uri, input)
        }))
        .await;
    }
    let mut parsed = Vec::with_capacity(uris.len());
    let mut outputs: Vec<Option<Result<String, KernelError>>> =
        (0..uris.len()).map(|_| None).collect();
    for (index, (uri, input)) in uris.iter().zip(inputs).enumerate() {
        let resource_uri = match ResourceUri::parse(uri) {
            Ok(uri) => uri,
            Err(error) => {
                outputs[index] = Some(Ok(serde_json::json!({
                    "err": tools::kernel_error_to_json_host(error, uri)
                })
                .to_string()));
                continue;
            }
        };
        let mut value: serde_json::Value = match serde_json::from_str(&input) {
            Ok(value) => value,
            Err(error) => {
                outputs[index] = Some(Ok(serde_json::json!({
                    "err": tools::kernel_error_to_json_host(
                        KernelError::InvalidRequest {
                            message: format!("invalid resource host input: {error}"),
                        },
                        uri,
                    )
                })
                .to_string()));
                continue;
            }
        };
        let input_type = match kernel.universal_input_type(verb.clone(), resource_uri.clone()) {
            Ok(input_type) => input_type,
            Err(error) => {
                outputs[index] = Some(Ok(serde_json::json!({
                    "err": tools::kernel_error_to_json_host(error, uri)
                })
                .to_string()));
                continue;
            }
        };
        if input_type.as_ref().is_none_or(
            |ty| matches!(ty, DynamicType::Record(fields) if fields.contains_key("uri")),
        ) {
            if let serde_json::Value::Object(fields) = &mut value {
                fields.insert("uri".to_owned(), serde_json::Value::String(uri.clone()));
            }
        }
        let dynamic = match input_type {
            Some(input_type) => match tools::json_to_dynamic_typed(&value, &input_type) {
                Ok(value) => value,
                Err(error) => {
                    outputs[index] = Some(Ok(serde_json::json!({
                        "err": tools::kernel_error_to_json_host(error, uri)
                    })
                    .to_string()));
                    continue;
                }
            },
            None => match tools::json_to_dynamic_host(value, uri) {
                Ok(value) => value,
                Err(error) => {
                    outputs[index] = Some(Ok(serde_json::json!({
                        "err": tools::kernel_error_to_json_host(error, uri)
                    })
                    .to_string()));
                    continue;
                }
            },
        };
        parsed.push((index, resource_uri, dynamic));
    }
    let values = kernel
        .execute_universal_batch_with_scope(
            verb,
            parsed
                .iter()
                .map(|(_, uri, value)| (uri.clone(), value.clone()))
                .collect(),
            scope,
        )
        .await;
    for ((index, _, _), result) in parsed.into_iter().zip(values) {
        let uri = &uris[index];
        let output = match result {
            Ok(value) => {
                serde_json::json!({"ok": tools::dynamic_to_json_host(value.result.output)})
            }
            Err(error) => {
                serde_json::json!({"err": tools::kernel_error_to_json_host(error, &uri)})
            }
        };
        outputs[index] =
            Some(
                serde_json::to_string(&output).map_err(|error| KernelError::Handler {
                    message: format!("could not encode resource host output: {error}"),
                }),
            );
    }
    Ok(outputs
        .into_iter()
        .map(|output| {
            output.unwrap_or_else(|| {
                Err(KernelError::Handler {
                    message: "resource host produced no batch slot".to_owned(),
                })
            })
        })
        .collect::<Result<Vec<_>, _>>()?)
}

fn define_dependency_exports_async(
    instance: &mut wasmtime::component::LinkerInstance<'_, HostState>,
    dependency: Arc<DynamicDependency>,
) -> Result<(), ComponentError> {
    for (name, export) in dependency
        .component
        .component_type()
        .exports(&dependency.engine)
    {
        match &export.ty {
            wasmtime::component::types::ComponentItem::ComponentFunc(_) => {
                let dependency = Arc::clone(&dependency);
                let export_name = name.to_owned();
                instance
                    .func_new_async(name, move |mut store, _ty, params, results| {
                        let dependency = Arc::clone(&dependency);
                        let export_name = export_name.clone();
                        Box::new(async move {
                            let linker = dynamic_linker_for_async(
                                &dependency.engine,
                                &dependency.component,
                                &dependency.capabilities,
                                &dependency.dependencies,
                            )
                            .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                            let imported = linker
                                .instantiate_async(&mut store, &dependency.component)
                                .await
                                .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                            let function =
                                imported.get_func(&mut store, &export_name).ok_or_else(|| {
                                    wasmtime::Error::msg(format!(
                                        "dependency export disappeared: {export_name}"
                                    ))
                                })?;
                            function
                                .call_async(&mut store, params, results)
                                .await
                                .map_err(|error| wasmtime::Error::msg(error.to_string()))
                        })
                    })
                    .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
            }
            wasmtime::component::types::ComponentItem::ComponentInstance(component_instance) => {
                let mut nested = instance
                    .instance(name)
                    .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
                define_dependency_instance_exports_async(
                    &mut nested,
                    Arc::clone(&dependency),
                    component_instance,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn define_dependency_instance_exports_async(
    instance: &mut wasmtime::component::LinkerInstance<'_, HostState>,
    dependency: Arc<DynamicDependency>,
    component_instance: &wasmtime::component::types::ComponentInstance,
) -> Result<(), ComponentError> {
    for (name, export) in component_instance.exports(&dependency.engine) {
        if matches!(
            export.ty,
            wasmtime::component::types::ComponentItem::ComponentFunc(_)
        ) {
            let dependency = Arc::clone(&dependency);
            let export_name = name.to_owned();
            instance
                .func_new_async(name, move |mut store, _ty, params, results| {
                    let dependency = Arc::clone(&dependency);
                    let export_name = export_name.clone();
                    Box::new(async move {
                        let linker = dynamic_linker_for_async(
                            &dependency.engine,
                            &dependency.component,
                            &dependency.capabilities,
                            &dependency.dependencies,
                        )
                        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                        let imported = linker
                            .instantiate_async(&mut store, &dependency.component)
                            .await
                            .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                        let function =
                            imported.get_func(&mut store, &export_name).ok_or_else(|| {
                                wasmtime::Error::msg(format!(
                                    "dependency export disappeared: {export_name}"
                                ))
                            })?;
                        function
                            .call_async(&mut store, params, results)
                            .await
                            .map_err(|error| wasmtime::Error::msg(error.to_string()))
                    })
                })
                .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
        }
    }
    Ok(())
}

fn contract_matches_import(contract: &str, import_name: &str) -> bool {
    if contract == import_name {
        return true;
    }
    let Some((identity, major)) = contract.rsplit_once('@') else {
        return false;
    };
    let Some((namespace, interface)) = identity.rsplit_once(':') else {
        return false;
    };
    let prefix = format!("{namespace}:{interface}/");
    import_name
        .strip_prefix(&prefix)
        .and_then(|value| value.rsplit_once('@'))
        .is_some_and(|(_, version)| version.split('.').next() == Some(major))
}

fn define_dependency_exports(
    instance: &mut wasmtime::component::LinkerInstance<'_, HostState>,
    dependency: Arc<DynamicDependency>,
) -> Result<(), ComponentError> {
    for (name, export) in dependency
        .component
        .component_type()
        .exports(&dependency.engine)
    {
        match &export.ty {
            wasmtime::component::types::ComponentItem::ComponentFunc(_) => {
                let dependency = Arc::clone(&dependency);
                let name = name.to_owned();
                let export_name = name.clone();
                instance
                    .func_new(&name, move |mut store, _ty, params, results| {
                        let linker = dynamic_linker_for(
                            &dependency.engine,
                            &dependency.component,
                            &dependency.capabilities,
                            &dependency.dependencies,
                        )
                        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                        let imported = linker
                            .instantiate(&mut store, &dependency.component)
                            .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                        let function =
                            imported.get_func(&mut store, &export_name).ok_or_else(|| {
                                wasmtime::Error::msg(format!(
                                    "dependency export disappeared: {export_name}"
                                ))
                            })?;
                        function
                            .call(&mut store, params, results)
                            .map_err(|error| wasmtime::Error::msg(error.to_string()))
                    })
                    .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
            }
            wasmtime::component::types::ComponentItem::ComponentInstance(component_instance) => {
                let mut nested = instance
                    .instance(name)
                    .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
                define_dependency_instance_exports(
                    &mut nested,
                    Arc::clone(&dependency),
                    component_instance,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn define_dependency_instance_exports(
    instance: &mut wasmtime::component::LinkerInstance<'_, HostState>,
    dependency: Arc<DynamicDependency>,
    component_instance: &wasmtime::component::types::ComponentInstance,
) -> Result<(), ComponentError> {
    for (name, export) in component_instance.exports(&dependency.engine) {
        if matches!(
            export.ty,
            wasmtime::component::types::ComponentItem::ComponentFunc(_)
        ) {
            let dependency = Arc::clone(&dependency);
            let name = name.to_owned();
            let export_name = name.clone();
            instance
                .func_new(&name, move |mut store, _ty, params, results| {
                    let linker = dynamic_linker_for(
                        &dependency.engine,
                        &dependency.component,
                        &dependency.capabilities,
                        &dependency.dependencies,
                    )
                    .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                    let imported = linker
                        .instantiate(&mut store, &dependency.component)
                        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                    let function =
                        imported.get_func(&mut store, &export_name).ok_or_else(|| {
                            wasmtime::Error::msg(format!(
                                "dependency export disappeared: {export_name}"
                            ))
                        })?;
                    function
                        .call(&mut store, params, results)
                        .map_err(|error| wasmtime::Error::msg(error.to_string()))
                })
                .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
        }
    }
    Ok(())
}

fn json_to_component_val(
    value: &serde_json::Value,
    ty: &wasmtime::component::Type,
) -> Result<wasmtime::component::Val, String> {
    use wasmtime::component::{Type, Val};
    match ty {
        Type::Bool => value
            .as_bool()
            .map(Val::Bool)
            .ok_or_else(|| "expected boolean".to_owned()),
        Type::S8 => value
            .as_i64()
            .map(|value| Val::S8(value as i8))
            .ok_or_else(|| "expected signed integer".to_owned()),
        Type::U8 => value
            .as_u64()
            .map(|value| Val::U8(value as u8))
            .ok_or_else(|| "expected unsigned integer".to_owned()),
        Type::S16 => value
            .as_i64()
            .map(|value| Val::S16(value as i16))
            .ok_or_else(|| "expected signed integer".to_owned()),
        Type::U16 => value
            .as_u64()
            .map(|value| Val::U16(value as u16))
            .ok_or_else(|| "expected unsigned integer".to_owned()),
        Type::S32 => value
            .as_i64()
            .map(|value| Val::S32(value as i32))
            .ok_or_else(|| "expected signed integer".to_owned()),
        Type::U32 => value
            .as_u64()
            .map(|value| Val::U32(value as u32))
            .ok_or_else(|| "expected unsigned integer".to_owned()),
        Type::S64 => value
            .as_i64()
            .map(Val::S64)
            .ok_or_else(|| "expected signed integer".to_owned()),
        Type::U64 => value
            .as_u64()
            .map(Val::U64)
            .ok_or_else(|| "expected unsigned integer".to_owned()),
        Type::Float32 => value
            .as_f64()
            .map(|value| Val::Float32(value as f32))
            .ok_or_else(|| "expected number".to_owned()),
        Type::Float64 => value
            .as_f64()
            .map(Val::Float64)
            .ok_or_else(|| "expected number".to_owned()),
        Type::Char => value
            .as_str()
            .and_then(|value| value.chars().next())
            .map(Val::Char)
            .ok_or_else(|| "expected character".to_owned()),
        Type::String => value
            .as_str()
            .map(|value| Val::String(value.to_owned()))
            .ok_or_else(|| "expected string".to_owned()),
        Type::List(list) => value
            .as_array()
            .ok_or_else(|| "expected list".to_owned())
            .and_then(|values| {
                values
                    .iter()
                    .map(|value| json_to_component_val(value, &list.ty()))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Val::List)
            }),
        Type::Map(map) => value
            .as_object()
            .ok_or_else(|| "expected map".to_owned())
            .and_then(|object| {
                object
                    .iter()
                    .map(|(key, value)| {
                        Ok((
                            Val::String(key.clone()),
                            json_to_component_val(value, &map.value())?,
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()
                    .map(Val::Map)
            }),
        Type::Record(record) => value
            .as_object()
            .ok_or_else(|| "expected record".to_owned())
            .and_then(|object| {
                record
                    .fields()
                    .map(|field| {
                        let value = object
                            .get(field.name)
                            .or_else(|| object.get(&field.name.replace('-', "_")))
                            .cloned()
                            .or_else(|| {
                                matches!(&field.ty, Type::Option(_))
                                    .then_some(serde_json::Value::Null)
                            });
                        let value = value
                            .as_ref()
                            .ok_or_else(|| format!("missing record field {}", field.name))?;
                        json_to_component_val(value, &field.ty)
                            .map(|value| (field.name.to_owned(), value))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Val::Record)
            }),
        Type::Tuple(tuple) => value
            .as_array()
            .ok_or_else(|| "expected tuple".to_owned())
            .and_then(|values| {
                tuple
                    .types()
                    .zip(values)
                    .map(|(ty, value)| json_to_component_val(value, &ty))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Val::Tuple)
            }),
        Type::Enum(enumeration) => value
            .as_str()
            .map(|value| Val::Enum(value.to_owned()))
            .ok_or_else(|| {
                format!(
                    "expected enum; valid values: {:?}",
                    enumeration.names().collect::<Vec<_>>()
                )
            }),
        Type::Flags(_flags) => value
            .as_array()
            .ok_or_else(|| "expected flags".to_owned())
            .and_then(|values| {
                values
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "flag must be a string".to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Val::Flags)
            }),
        Type::Option(option) => {
            if value.is_null() {
                Ok(Val::Option(None))
            } else {
                json_to_component_val(value, &option.ty())
                    .map(|value| Val::Option(Some(Box::new(value))))
            }
        }
        Type::Variant(variant) => {
            let object = value
                .as_object()
                .ok_or_else(|| "expected variant object".to_owned())?;
            let (name, value) = object
                .iter()
                .next()
                .ok_or_else(|| "variant object is empty".to_owned())?;
            let case = variant
                .cases()
                .find(|case| case.name == name)
                .ok_or_else(|| format!("unknown variant case {name}"))?;
            let payload = match (&case.ty, value) {
                (None, serde_json::Value::Null) => None,
                (Some(ty), value) => Some(Box::new(json_to_component_val(value, ty)?)),
                (None, _) => return Err(format!("variant case {name} has no payload")),
            };
            Ok(Val::Variant(name.clone(), payload))
        }
        Type::Result(result) => {
            let object = value
                .as_object()
                .ok_or_else(|| "expected result object".to_owned())?;
            if let Some(value) = object.get("ok") {
                Ok(Val::Result(Ok(Some(Box::new(json_to_component_val(
                    value,
                    &result.ok().ok_or_else(|| "result has no ok type")?,
                )?)))))
            } else if let Some(value) = object.get("err") {
                Ok(Val::Result(Err(Some(Box::new(json_to_component_val(
                    value,
                    &result.err().ok_or_else(|| "result has no err type")?,
                )?)))))
            } else {
                Err("result requires ok or err".to_owned())
            }
        }
        unsupported => Err(format!("unsupported dynamic WIT type: {unsupported:?}")),
    }
}

fn component_val_to_json(
    value: &wasmtime::component::Val,
) -> Result<serde_json::Value, ComponentError> {
    use wasmtime::component::Val;
    Ok(match value {
        Val::Bool(value) => serde_json::json!(value),
        Val::S8(value) => serde_json::json!(value),
        Val::U8(value) => serde_json::json!(value),
        Val::S16(value) => serde_json::json!(value),
        Val::U16(value) => serde_json::json!(value),
        Val::S32(value) => serde_json::json!(value),
        Val::U32(value) => serde_json::json!(value),
        Val::S64(value) => serde_json::json!(value),
        Val::U64(value) => serde_json::json!(value),
        Val::Float32(value) => serde_json::json!(value),
        Val::Float64(value) => serde_json::json!(value),
        Val::Char(value) => serde_json::json!(value.to_string()),
        Val::String(value) => serde_json::json!(value),
        Val::List(values) | Val::Tuple(values) => serde_json::Value::Array(
            values
                .iter()
                .map(component_val_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Val::Map(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    Ok((
                        component_val_to_json(key)?
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        component_val_to_json(value)?,
                    ))
                })
                .collect::<Result<serde_json::Map<_, _>, ComponentError>>()?,
        ),
        Val::Record(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(name, value)| Ok((name.clone(), component_val_to_json(value)?)))
                .collect::<Result<serde_json::Map<_, _>, ComponentError>>()?,
        ),
        Val::Variant(name, value) => {
            serde_json::json!({name: value.as_deref().map(component_val_to_json).transpose()?})
        }
        Val::Enum(name) => serde_json::json!(name),
        Val::Option(value) => value
            .as_deref()
            .map(component_val_to_json)
            .transpose()?
            .unwrap_or(serde_json::Value::Null),
        Val::Result(Ok(value)) => {
            serde_json::json!({"ok": value.as_deref().map(component_val_to_json).transpose()?})
        }
        Val::Result(Err(value)) => {
            serde_json::json!({"err": value.as_deref().map(component_val_to_json).transpose()?})
        }
        Val::Flags(values) => serde_json::json!(values),
        unsupported => {
            return Err(ComponentError::Invoke(anyhow::anyhow!(format!(
                "unsupported dynamic WIT result: {unsupported:?}"
            ))));
        }
    })
}

pub mod package {
    use super::{ABI_VERSION, ComponentError, TypedComponentHost};
    use gray_matter::{Matter, engine::YAML};
    use serde::{Deserialize, Serialize, de::DeserializeOwned};
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        sync::{Mutex, OnceLock},
        time::Duration,
    };
    use walkdir::WalkDir;

    /// Per-invocation Wasmtime resource ceilings. These are deliberately runtime
    /// policy rather than part of any package or WIT contract.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct WasmExecutionLimits {
        pub max_memory_bytes: usize,
        pub max_table_elements: usize,
        pub max_instances: usize,
        pub max_memories: usize,
        pub max_tables: usize,
    }

    impl Default for WasmExecutionLimits {
        fn default() -> Self {
            Self {
                max_memory_bytes: 512 * 1024 * 1024,
                max_table_elements: 100_000,
                max_instances: 128,
                max_memories: 32,
                max_tables: 32,
            }
        }
    }

    static COMPONENT_ENGINE: OnceLock<wasmtime::Engine> = OnceLock::new();

    #[allow(deprecated)]
    pub(crate) fn component_engine() -> Result<wasmtime::Engine, ComponentError> {
        let engine = COMPONENT_ENGINE.get_or_init(|| {
            let mut config = wasmtime::Config::new();
            config.wasm_component_model(true);
            config.wasm_component_model_implements(true);
            config.async_support(true);
            config.wasm_component_model_async(true);
            config.epoch_interruption(true);
            config.consume_fuel(true);
            wasmtime::Engine::new(&config).expect("artist WASM engine must initialize")
        });
        register_epoch_engine();
        Ok(engine.clone())
    }

    static EPOCH_TICKER: OnceLock<()> = OnceLock::new();
    static CARGO_BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    pub(crate) fn cargo_build_lock() -> std::sync::MutexGuard<'static, ()> {
        CARGO_BUILD_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap()
    }

    pub(crate) struct CargoComponentTarget {
        pub package_name: String,
        pub package_version: String,
        pub target_name: String,
        pub target_directory: PathBuf,
    }

    pub(crate) fn resolve_component_target(
        manifest: &Path,
    ) -> Result<CargoComponentTarget, String> {
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(manifest)
            .no_deps()
            .other_options(vec!["--offline".to_owned()])
            .exec()
            .map_err(|error| format!("cargo metadata failed: {error}"))?;
        let package = metadata
            .packages
            .iter()
            .find(|package| package.manifest_path.as_str() == manifest.to_string_lossy())
            .or_else(|| metadata.packages.first())
            .ok_or_else(|| "Cargo metadata contained no package".to_owned())?;
        let target = package
            .targets
            .iter()
            .find(|target| {
                target
                    .kind
                    .iter()
                    .any(|kind| matches!(kind, cargo_metadata::TargetKind::CDyLib))
            })
            .or_else(|| {
                package.targets.iter().find(|target| {
                    target
                        .kind
                        .iter()
                        .any(|kind| matches!(kind, cargo_metadata::TargetKind::Bin))
                })
            })
            .ok_or_else(|| "package must declare a cdylib or bin target".to_owned())?;
        Ok(CargoComponentTarget {
            package_name: package.name.clone(),
            package_version: package.version.to_string(),
            target_name: target.name.clone(),
            target_directory: metadata.target_directory.clone().into_std_path_buf(),
        })
    }

    /// Build one authored package target and recover the emitted component.
    /// Tool and resource packages deliberately share this Cargo boundary; the
    /// package-specific layers own only manifest validation, fingerprints,
    /// caching, and component-link validation.
    pub(crate) fn build_wasm_artifact(
        root: &Path,
        manifest: &Path,
        target_name: &str,
        options: &BuildOptions,
    ) -> Result<PathBuf, String> {
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(manifest)
            .no_deps()
            .other_options(vec!["--offline".to_owned()])
            .exec()
            .map_err(|error| format!("cargo metadata failed: {error}"))?;
        let target_directory = metadata.target_directory.clone().into_std_path_buf();

        let mut rustflags = std::env::var("RUSTFLAGS").unwrap_or_default();
        if !options.target.starts_with("wasm") && !rustflags.contains("target-cpu") {
            if !rustflags.is_empty() {
                rustflags.push(' ');
            }
            rustflags.push_str("-C target-cpu=native");
        }
        let mut build = escargot::CargoBuild::new()
            .manifest_path(manifest)
            .target_dir(root.join("target"))
            .target(&options.target)
            .arg("--lib")
            .arg("--offline")
            .env("RUSTFLAGS", rustflags);
        build = match options.profile {
            BuildProfile::Debug => build,
            BuildProfile::Release => build.release(),
            BuildProfile::Product => build.arg("--profile").arg("product"),
        };
        let _build_guard = cargo_build_lock();
        let mut command = build.into_command();
        command.current_dir(root);
        if let Ok(path) = std::env::var("PATH") {
            command.env("PATH", format!("/usr/bin:/bin:{path}"));
        }
        let messages = escargot::CommandMessages::with_command(command)
            .map_err(|error| format!("cargo build failed: {error}"))?;
        let filename = format!("{}.wasm", target_name.replace('-', "_"));
        let artifact = messages
            .filter_map(|message| message.ok())
            .find_map(|message| {
                let escargot::format::Message::CompilerArtifact(artifact) =
                    message.decode().ok()?
                else {
                    return None;
                };
                (artifact.target.name == target_name).then(|| {
                    artifact
                        .filenames
                        .into_iter()
                        .map(|path| path.into_owned())
                        .find(|path| path.extension().is_some_and(|extension| extension == "wasm"))
                })?
            })
            .or_else(|| {
                let expected = target_directory
                    .join(&options.target)
                    .join(options.profile.directory())
                    .join(&filename);
                expected.is_file().then_some(expected).or_else(|| {
                    [target_directory.clone(), root.join("target")]
                        .into_iter()
                        .find_map(|search_root| {
                        WalkDir::new(search_root)
                            .into_iter()
                            .filter_map(Result::ok)
                            .map(|entry| entry.into_path())
                            .find(|path| {
                                path.is_file()
                                    && path.extension().is_some_and(|extension| extension == "wasm")
                                    && path.file_stem().is_some_and(|stem| {
                                        stem.to_string_lossy().replace('-', "_")
                                            == target_name.replace('-', "_")
                                    })
                            })
                        })
                })
            })
            .ok_or_else(|| {
                format!("cargo succeeded but emitted no WASM component artifact (target={target_name}, expected={filename})")
            })?;
        if !artifact.is_file() {
            return Err(format!(
                "cargo produced a non-file artifact {}",
                artifact.display()
            ));
        }
        Ok(artifact)
    }

    /// Package-independent cache helpers shared by tool and resource
    /// adapters. Their manifests and component validation differ, but the
    /// Cargo/fingerprint/provenance boundary should remain one implementation.
    pub(crate) fn hash_path(hasher: &mut Sha256, path: &Path) -> Result<(), String> {
        if path.is_file() {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(
                fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?,
            );
            return Ok(());
        }
        if path.is_dir() {
            let mut files = WalkDir::new(path)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .map(|entry| entry.path().to_owned())
                .collect::<Vec<_>>();
            files.sort();
            for file in files {
                hash_path(hasher, &file)?;
            }
        }
        Ok(())
    }

    pub(crate) fn authored_inputs_newer_than_artifact(root: &Path, artifact: &Path) -> bool {
        let Ok(artifact_time) = fs::metadata(artifact).and_then(|metadata| metadata.modified())
        else {
            return true;
        };
        WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| {
                entry.path() != artifact
                    && entry
                        .path()
                        .extension()
                        .is_none_or(|extension| extension != "json" && extension != "wasm")
            })
            .any(|entry| {
                fs::metadata(entry.path())
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| modified > artifact_time)
            })
    }

    pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
        let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub(crate) fn provenance_matches(path: &Path, fingerprint: &str, artifact: &Path) -> bool {
        let Ok(bytes) = fs::read(path) else {
            return false;
        };
        let Ok(provenance) = serde_json::from_slice::<BuildProvenance>(&bytes) else {
            return false;
        };
        provenance.fingerprint == fingerprint
            && sha256_file(artifact).is_ok_and(|hash| hash == provenance.artifact_sha256)
    }

    pub(crate) fn find_cached_artifact(
        root: &Path,
        target_name: &str,
        target: &str,
        profile: BuildProfile,
        fingerprint: &str,
    ) -> Result<Option<(PathBuf, PathBuf)>, String> {
        let filename = format!("{}.wasm", target_name.replace('-', "_"));
        let mut directory = Some(root);
        while let Some(current) = directory {
            let artifact = current
                .join("target")
                .join(target)
                .join(profile.directory())
                .join(&filename);
            let provenance = artifact.with_extension("wasm.artist.json");
            if artifact.is_file() {
                let valid = fs::read(&provenance)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<BuildProvenance>(&bytes).ok())
                    .is_some_and(|value| {
                        value.fingerprint == fingerprint
                            && sha256_file(&artifact)
                                .is_ok_and(|hash| hash == value.artifact_sha256)
                    });
                if valid {
                    return Ok(Some((artifact, provenance)));
                }
            }
            directory = current.parent();
        }
        Ok(None)
    }

    fn register_epoch_engine() {
        EPOCH_TICKER.get_or_init(|| {
            std::thread::Builder::new()
                .name("artist-wasm-epoch".to_owned())
                .spawn(|| {
                    loop {
                        std::thread::sleep(Duration::from_millis(10));
                        // All components share the one engine, so the ticker has
                        // no reload-sized registry to retain forever.
                        if let Some(engine) = COMPONENT_ENGINE.get() {
                            engine.increment_epoch();
                        }
                    }
                })
                .expect("artist WASM epoch ticker must start");
        });
    }

    #[derive(Clone, Debug, Deserialize, PartialEq)]
    pub struct ToolFrontmatter {
        pub name: String,
        pub description: String,
        pub version: String,
        #[serde(default)]
        pub contract: Option<String>,
        pub input_schema: Option<serde_yaml::Value>,
        pub output_schema: Option<serde_yaml::Value>,
        #[serde(default)]
        pub capabilities: Vec<String>,
    }

    #[derive(Clone, Debug, PartialEq)]
    pub struct ToolPackage {
        pub root: PathBuf,
        pub manifest: ToolFrontmatter,
        pub prose: String,
        pub wasm: Option<PathBuf>,
        pub source: Option<PathBuf>,
        pub build_manifest: Option<PathBuf>,
        pub wit: Option<PathBuf>,
        pub contract: Option<crate::contracts::ContractId>,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum BuildProfile {
        #[default]
        Product,
        Debug,
        Release,
    }

    impl BuildProfile {
        pub(crate) fn directory(self) -> &'static str {
            match self {
                Self::Debug => "debug",
                Self::Release => "release",
                Self::Product => "product",
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct BuildOptions {
        pub target: String,
        pub profile: BuildProfile,
        pub granted_capabilities: Vec<String>,
        pub force: bool,
    }

    impl Default for BuildOptions {
        fn default() -> Self {
            Self {
                target: "wasm32-wasip2".to_owned(),
                profile: BuildProfile::Product,
                granted_capabilities: Vec::new(),
                force: false,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct BuildResult {
        pub artifact: PathBuf,
        pub provenance: PathBuf,
        pub fingerprint: String,
        pub cached: bool,
    }

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub struct BuildProvenance {
        pub package_name: String,
        pub package_version: String,
        pub contract: Option<String>,
        pub abi_version: String,
        pub target: String,
        pub profile: String,
        pub fingerprint: String,
        pub artifact_sha256: String,
        pub cargo_version: String,
        pub rustc_version: String,
    }

    pub struct ParsedManifest<T> {
        pub frontmatter: T,
        pub prose: String,
    }

    pub fn parse_frontmatter<T: DeserializeOwned>(
        markdown: &str,
    ) -> Result<ParsedManifest<T>, String> {
        let parsed = Matter::<YAML>::new()
            .parse_with_struct::<T>(markdown)
            .ok_or_else(|| "Markdown must contain valid YAML frontmatter".to_owned())?;
        Ok(ParsedManifest {
            frontmatter: parsed.data,
            prose: parsed.content.trim().to_owned(),
        })
    }

    impl ToolPackage {
        pub fn discover(root: impl AsRef<Path>) -> Result<Self, ComponentError> {
            let root = root.as_ref().to_owned();
            let markdown = fs::read_to_string(root.join("tool.md"))
                .map_err(|e| ComponentError::Load(anyhow::anyhow!(e)))?;
            let parsed = parse_frontmatter::<ToolFrontmatter>(&markdown).map_err(|error| {
                ComponentError::Load(anyhow::anyhow!(format!(
                    "tool.md must contain valid YAML frontmatter: {error}"
                )))
            })?;
            let manifest = parsed.frontmatter;
            let contract = manifest
                .contract
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|error: String| ComponentError::Load(anyhow::anyhow!(error)))?;
            let wasm_path = root.join("tool.wasm");
            let wasm = wasm_path.is_file().then_some(wasm_path);
            let source_path = if root.join("src").is_dir() {
                Some(root.join("src"))
            } else if root.join("source").is_dir() {
                Some(root.join("source"))
            } else {
                None
            };
            if wasm.is_none() && source_path.is_none() {
                return Err(ComponentError::Load(anyhow::anyhow!(
                    "tool package needs tool.wasm or a src/ or source/ directory"
                )));
            }
            let cargo_manifest = root.join("Cargo.toml");
            let wit_path = root.join("tool.wit");
            if contract.is_none() && !wit_path.is_file() {
                return Err(ComponentError::Load(anyhow::anyhow!(
                    "tool package must declare a typed contract or tool.wit"
                )));
            }
            if let Some(path) = wit_path.as_path().is_file().then_some(wit_path.as_path()) {
                wit_parser::UnresolvedPackageGroup::parse_dir(path.parent().unwrap()).map_err(
                    |error| {
                        ComponentError::Load(anyhow::anyhow!(format!(
                            "parse package-local WIT {}: {error}",
                            path.display()
                        )))
                    },
                )?;
            }
            Ok(Self {
                root,
                manifest,
                prose: parsed.prose,
                wasm,
                source: source_path,
                build_manifest: cargo_manifest.is_file().then_some(cargo_manifest),
                wit: wit_path.is_file().then_some(wit_path),
                contract,
            })
        }

        /// Build the authored package into a validated WASM component.
        ///
        /// This method is deliberately synchronous: callers that need async
        /// orchestration can run it on a blocking worker. It never replaces a
        /// previously valid artifact when compilation or validation fails.
        pub fn build(&self, options: &BuildOptions) -> Result<BuildResult, ComponentError> {
            if let Some(artifact) = &self.wasm {
                let newer =
                    super::package::authored_inputs_newer_than_artifact(&self.root, artifact);
                let validation = validate_artifact(artifact, self, options);
                if !newer {
                    validation?;
                    let mut hasher = Sha256::new();
                    hasher.update(fs::read(artifact).map_err(|error| ComponentError::Build {
                        diagnostics: format!("could not read {}: {error}", artifact.display()),
                    })?);
                    return Ok(BuildResult {
                        provenance: artifact.with_extension("wasm.artist.json"),
                        artifact: artifact.clone(),
                        fingerprint: format!("artifact:{:x}", hasher.finalize()),
                        cached: true,
                    });
                }
            }
            if self.build_manifest.is_none() {
                let artifact = self.wasm.clone().ok_or_else(|| ComponentError::Build {
                    diagnostics: "tool package has neither Cargo.toml nor tool.wasm".to_owned(),
                })?;
                validate_artifact(&artifact, self, options)?;
                let mut hasher = Sha256::new();
                hasher.update(fs::read(&artifact).map_err(|error| ComponentError::Build {
                    diagnostics: format!("could not read {}: {error}", artifact.display()),
                })?);
                return Ok(BuildResult {
                    provenance: artifact.with_extension("wasm.artist.json"),
                    artifact,
                    fingerprint: format!("{:x}", hasher.finalize()),
                    cached: true,
                });
            }
            let manifest = self
                .build_manifest
                .as_ref()
                .ok_or_else(|| ComponentError::Build {
                    diagnostics: "source package has no Cargo.toml".to_owned(),
                })?;
            let target = super::package::resolve_component_target(manifest)
                .map_err(|diagnostics| ComponentError::Build { diagnostics })?;

            let fingerprint = fingerprint(self, options, &target.package_version)?;
            if !options.force {
                if let Some(artifact) = &self.wasm {
                    let provenance = artifact.with_extension("wasm.artist.json");
                    if super::package::provenance_matches(&provenance, &fingerprint, artifact)
                        && validate_artifact(artifact, self, options).is_ok()
                    {
                        return Ok(BuildResult {
                            artifact: artifact.clone(),
                            provenance,
                            fingerprint,
                            cached: true,
                        });
                    }
                }
            }
            if !options.force {
                if let Some((artifact, provenance)) = super::package::find_cached_artifact(
                    &self.root,
                    &target.target_name,
                    &options.target,
                    options.profile,
                    &fingerprint,
                )
                .map_err(|diagnostics| ComponentError::Build { diagnostics })?
                {
                    if validate_artifact(&artifact, self, options).is_ok() {
                        return Ok(BuildResult {
                            artifact,
                            provenance,
                            fingerprint,
                            cached: true,
                        });
                    }
                }
            }

            let artifact = build_wasm_artifact(&self.root, manifest, &target.target_name, options)
                .map_err(|diagnostics| ComponentError::Build { diagnostics })?;
            let provenance = artifact.with_extension("wasm.artist.json");

            validate_artifact(&artifact, self, options)?;
            let provenance_value = BuildProvenance {
                package_name: target.package_name,
                package_version: target.package_version,
                contract: self.contract.as_ref().map(ToString::to_string),
                abi_version: ABI_VERSION.to_owned(),
                target: options.target.clone(),
                profile: options.profile.directory().to_owned(),
                fingerprint: fingerprint.clone(),
                artifact_sha256: super::package::sha256_file(&artifact)
                    .map_err(|diagnostics| ComponentError::Build { diagnostics })?,
                cargo_version: tool_version("cargo"),
                rustc_version: tool_version("rustc"),
            };
            fs::write(
                &provenance,
                serde_json::to_vec_pretty(&provenance_value).unwrap(),
            )
            .map_err(|error| ComponentError::Build {
                diagnostics: format!("could not write provenance: {error}"),
            })?;
            Ok(BuildResult {
                artifact,
                provenance,
                fingerprint,
                cached: false,
            })
        }
    }

    fn fingerprint(
        package: &ToolPackage,
        options: &BuildOptions,
        package_version: &str,
    ) -> Result<String, ComponentError> {
        let mut hasher = Sha256::new();
        hasher.update(b"artist-component-build-v1\0");
        hasher.update(ABI_VERSION.as_bytes());
        hasher.update(options.target.as_bytes());
        hasher.update(options.profile.directory().as_bytes());
        hasher.update(package.manifest.name.as_bytes());
        hasher.update(package_version.as_bytes());
        if let Some(contract) = &package.contract {
            hasher.update(contract.to_string().as_bytes());
        }
        for capability in &package.manifest.capabilities {
            hasher.update(capability.as_bytes());
        }
        for path in [
            &package.root.join("tool.md"),
            &package.root.join("tool.wit"),
            &package.root.join("deps"),
            &package.root.join("Cargo.toml"),
            &package.root.join("Cargo.lock"),
        ] {
            super::package::hash_path(&mut hasher, path)
                .map_err(|diagnostics| ComponentError::Build { diagnostics })?;
        }
        super::package::hash_path(
            &mut hasher,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/typed-guest/src"),
        )
        .map_err(|diagnostics| ComponentError::Build { diagnostics })?;
        if let Some(source) = &package.source {
            super::package::hash_path(&mut hasher, source)
                .map_err(|diagnostics| ComponentError::Build { diagnostics })?;
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    pub(crate) fn validate_artifact(
        artifact: &Path,
        package: &ToolPackage,
        options: &BuildOptions,
    ) -> Result<(), ComponentError> {
        let bytes = fs::read(artifact).map_err(|error| ComponentError::Build {
            diagnostics: format!("could not read {}: {error}", artifact.display()),
        })?;
        let host = TypedComponentHost::new_with_capabilities(
            &bytes,
            options.granted_capabilities.clone(),
        )?;
        host.validate_dynamic_export_shape(
            &package
                .contract
                .as_ref()
                .map(|contract| contract.interface.as_str())
                .unwrap_or("invoke"),
        )?;
        if package.manifest.name.is_empty() {
            return Err(ComponentError::Build {
                diagnostics: "tool name cannot be empty".to_owned(),
            });
        }
        Ok(())
    }

    pub(crate) fn tool_version(tool: &str) -> String {
        Command::new(tool)
            .arg("--version")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_else(|| "unknown".to_owned())
    }
}

/// Shared active-generation storage used by both tool and resource package
/// adapters. Activation validates candidates before inserting immutable
/// `Arc` generations, so readers never observe a partially swapped package.
pub mod generations {
    use std::{
        collections::{BTreeMap, HashMap},
        ops::Deref,
        sync::{Arc, RwLock},
    };

    #[derive(Clone)]
    pub struct GenerationStore<T>(Arc<RwLock<HashMap<String, Arc<T>>>>);

    impl<T> Default for GenerationStore<T> {
        fn default() -> Self {
            Self(Arc::new(RwLock::new(HashMap::new())))
        }
    }

    impl<T> Deref for GenerationStore<T> {
        type Target = RwLock<HashMap<String, Arc<T>>>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl<T> GenerationStore<T> {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn next_generation(&self, name: &str, current: impl Fn(&T) -> u64) -> u64 {
            self.read()
                .unwrap()
                .get(name)
                .map(|active| current(active.as_ref()) + 1)
                .unwrap_or(1)
        }

        pub fn insert(&self, name: String, generation: T) -> Arc<T> {
            let generation = Arc::new(generation);
            self.write().unwrap().insert(name, Arc::clone(&generation));
            generation
        }

        pub fn current(&self, name: &str) -> Option<Arc<T>> {
            self.read().unwrap().get(name).cloned()
        }

        pub fn values(&self) -> Vec<Arc<T>> {
            self.read().unwrap().values().cloned().collect()
        }

        pub fn remove_where(&self, mut predicate: impl FnMut(&T) -> bool) {
            self.write()
                .unwrap()
                .retain(|_, value| !predicate(value.as_ref()));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::GenerationStore;

        #[test]
        fn replacement_keeps_old_generation_usable_by_existing_leases() {
            let store = GenerationStore::new();
            let old = store.insert("demo".to_owned(), 1_u64);
            assert_eq!(store.next_generation("demo", |generation| *generation), 2);
            let new = store.insert("demo".to_owned(), 2_u64);

            assert_eq!(*old, 1);
            assert_eq!(*new, 2);
            assert_eq!(*store.current("demo").unwrap(), 2);
        }
    }
}

/// Versioned component activation and hot-reload coordination.
pub mod runtime {
    use super::{
        ComponentError, ComponentMetadata, TypedComponentHost,
        package::{BuildOptions, ToolPackage},
    };
    use std::{
        collections::{BTreeMap, HashMap},
        fs,
        path::PathBuf,
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    /// An immutable activated component version. Cloning this value pins that
    /// version until the clone is dropped, even after a reload is committed.
    pub struct ActiveVersion {
        package: String,
        generation: u64,
        fingerprint: String,
        provenance: PathBuf,
        artifact: Arc<Vec<u8>>,
        info: ComponentMetadata,
        dynamic_host: Arc<TypedComponentHost>,
    }

    impl ActiveVersion {
        pub fn package(&self) -> &str {
            &self.package
        }

        pub fn generation(&self) -> u64 {
            self.generation
        }

        pub fn fingerprint(&self) -> &str {
            &self.fingerprint
        }

        pub fn provenance(&self) -> &PathBuf {
            &self.provenance
        }

        pub(crate) fn artifact_bytes(&self) -> Arc<Vec<u8>> {
            Arc::clone(&self.artifact)
        }

        pub fn info(&self) -> &ComponentMetadata {
            &self.info
        }

        pub fn invoke_tool_json(
            &self,
            verb: impl Into<String>,
            input: &str,
            kernel: artist_kernel::KernelHandle,
        ) -> Result<String, ComponentError> {
            let verb = verb.into();
            self.invoke_tool_json_with_context(
                verb,
                input,
                kernel,
                artist_kernel::InvocationContext::default(),
            )
        }

        pub fn invoke_tool_json_with_context(
            &self,
            verb: impl Into<String>,
            input: &str,
            kernel: artist_kernel::KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> Result<String, ComponentError> {
            let verb = verb.into();
            let mut value: serde_json::Value = serde_json::from_str(input)
                .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
            if let Some(object) = value.as_object_mut() {
                if !object.contains_key("uri") {
                    if let Some(target) = object.remove("target") {
                        object.insert("uri".to_owned(), target);
                    }
                }
            }
            value = match value {
                serde_json::Value::Object(object) if object.contains_key("requests") => {
                    serde_json::Value::Object(object)
                }
                serde_json::Value::Array(requests) => serde_json::json!({ "requests": requests }),
                value => serde_json::json!({ "requests": [value] }),
            };
            serde_json::to_string(
                &self
                    .dynamic_host
                    .invoke_dynamic_json_with_context(&value, &verb, kernel, context)?,
            )
            .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))
        }

        pub async fn invoke_tool_json_async_with_context(
            &self,
            verb: impl Into<String>,
            input: &str,
            kernel: artist_kernel::KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> Result<String, ComponentError> {
            let verb = verb.into();
            let mut value: serde_json::Value = serde_json::from_str(input)
                .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
            if let Some(object) = value.as_object_mut() {
                if !object.contains_key("uri") {
                    if let Some(target) = object.remove("target") {
                        object.insert("uri".to_owned(), target);
                    }
                }
            }
            value = match value {
                serde_json::Value::Object(object) if object.contains_key("requests") => {
                    serde_json::Value::Object(object)
                }
                serde_json::Value::Array(requests) => serde_json::json!({ "requests": requests }),
                value => serde_json::json!({ "requests": [value] }),
            };
            let output = self
                .dynamic_host
                .invoke_dynamic_json_async_with_scope(
                    &value,
                    &verb,
                    kernel.clone(),
                    artist_kernel::InvocationScope::new(context),
                )
                .await?;
            serde_json::to_string(&output)
                .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))
        }

        pub async fn invoke_tool_json_async_with_scope(
            &self,
            verb: impl Into<String>,
            input: &str,
            kernel: artist_kernel::KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<String, ComponentError> {
            let verb = verb.into();
            let mut value: serde_json::Value = serde_json::from_str(input)
                .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
            if let Some(object) = value.as_object_mut() {
                if !object.contains_key("uri") {
                    if let Some(target) = object.remove("target") {
                        object.insert("uri".to_owned(), target);
                    }
                }
            }
            value = match value {
                serde_json::Value::Object(object) if object.contains_key("requests") => {
                    serde_json::Value::Object(object)
                }
                serde_json::Value::Array(requests) => serde_json::json!({ "requests": requests }),
                value => serde_json::json!({ "requests": [value] }),
            };
            let output = self
                .dynamic_host
                .invoke_dynamic_json_async_with_scope(&value, &verb, kernel, scope)
                .await?;
            serde_json::to_string(&output)
                .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))
        }

        pub fn invoke_dynamic_json(
            &self,
            input: &serde_json::Value,
            export_name: &str,
            kernel: artist_kernel::KernelHandle,
        ) -> Result<serde_json::Value, ComponentError> {
            self.invoke_dynamic_json_with_context(
                input,
                export_name,
                kernel,
                artist_kernel::InvocationContext::default(),
            )
        }

        pub fn invoke_dynamic_json_with_context(
            &self,
            input: &serde_json::Value,
            export_name: &str,
            kernel: artist_kernel::KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> Result<serde_json::Value, ComponentError> {
            self.dynamic_host
                .invoke_dynamic_json_with_context(input, export_name, kernel, context)
        }

        pub async fn invoke_dynamic_json_async_with_scope(
            &self,
            input: &serde_json::Value,
            export_name: &str,
            kernel: artist_kernel::KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<serde_json::Value, ComponentError> {
            self.dynamic_host
                .invoke_dynamic_json_async_with_scope(input, export_name, kernel, scope)
                .await
        }

        pub async fn observe_json_async_with_scope(
            &self,
            response: &serde_json::Value,
            kernel: artist_kernel::KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<String, ComponentError> {
            let value = self
                .dynamic_host
                .invoke_dynamic_json_async_with_scope(response, "observe", kernel, scope)
                .await?;
            value.as_str().map(str::to_owned).ok_or_else(|| {
                ComponentError::Invoke(anyhow::anyhow!(
                    "component observer returned a non-string value"
                ))
            })
        }
    }

    impl Clone for ActiveVersion {
        fn clone(&self) -> Self {
            Self {
                package: self.package.clone(),
                generation: self.generation,
                fingerprint: self.fingerprint.clone(),
                provenance: self.provenance.clone(),
                artifact: Arc::clone(&self.artifact),
                info: self.info.clone(),
                dynamic_host: Arc::clone(&self.dynamic_host),
            }
        }
    }

    /// Registry of the currently active component version for each package.
    /// Building and validation happen before the write-side swap, so a failed
    /// reload cannot disturb the active version.
    #[derive(Clone, Default)]
    pub struct ComponentRegistry {
        active: super::generations::GenerationStore<ActiveVersion>,
        debounce: Arc<Mutex<HashMap<String, Arc<Mutex<u64>>>>>,
    }

    impl ComponentRegistry {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn reload(
            &self,
            package: &ToolPackage,
            options: &BuildOptions,
        ) -> Result<ActiveVersion, ComponentError> {
            self.reload_with_dependencies(package, options, Vec::new())
        }

        pub fn reload_with_dependencies(
            &self,
            package: &ToolPackage,
            options: &BuildOptions,
            dependencies: Vec<(String, Vec<u8>)>,
        ) -> Result<ActiveVersion, ComponentError> {
            let dependencies = dependencies
                .into_iter()
                .map(|(contract, bytes)| super::DependencySpec {
                    contract,
                    bytes,
                    dependencies: Vec::new(),
                })
                .collect();
            self.reload_with_dependency_specs(package, options, dependencies)
        }

        pub(crate) fn reload_with_dependency_specs(
            &self,
            package: &ToolPackage,
            options: &BuildOptions,
            dependencies: Vec<super::DependencySpec>,
        ) -> Result<ActiveVersion, ComponentError> {
            let build = package.build(options)?;
            let bytes = fs::read(&build.artifact).map_err(|error| {
                ComponentError::Load(anyhow::anyhow!(
                    "could not read built artifact {}: {error}",
                    build.artifact.display()
                ))
            })?;
            let dynamic_host = Arc::new(TypedComponentHost::new_with_dependency_specs(
                &bytes,
                options.granted_capabilities.clone(),
                dependencies,
            )?);
            dynamic_host.validate_dynamic_export(
                &package
                    .contract
                    .as_ref()
                    .map(|contract| contract.interface.as_str())
                    .unwrap_or("invoke"),
            )?;
            let info = ComponentMetadata {
                name: package.manifest.name.clone(),
                version: package.manifest.version.clone(),
                abi_version: super::ABI_VERSION.to_owned(),
                interfaces: vec![
                    package
                        .contract
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "package-local-wit".to_owned()),
                ],
                required_capabilities: package.manifest.capabilities.clone(),
            };
            let package_name = package.manifest.name.clone();

            let generation = self
                .active
                .next_generation(&package_name, |current| current.generation);
            let version = self.active.insert(
                package_name.clone(),
                ActiveVersion {
                    package: package_name.clone(),
                    generation,
                    fingerprint: build.fingerprint,
                    provenance: build.provenance,
                    artifact: Arc::new(bytes),
                    info,
                    dynamic_host,
                },
            );
            Ok((*version).clone())
        }

        pub fn current(&self, package: &str) -> Result<ActiveVersion, ComponentError> {
            self.active
                .read()
                .unwrap()
                .get(package)
                .map(|version| (**version).clone())
                .ok_or_else(|| ComponentError::NotActive(package.to_owned()))
        }

        pub fn current_generation(&self, package: &str) -> Option<u64> {
            self.active
                .current(package)
                .map(|version| version.generation)
        }

        /// Schedule an explicit reload after a quiet period. Repeated source
        /// edits supersede earlier requests; the transactional reload remains
        /// the only operation that can activate a candidate.
        pub fn debounce_reload(
            &self,
            package: ToolPackage,
            options: BuildOptions,
            quiet_period: Duration,
        ) -> ReloadTicket {
            let package_name = package.manifest.name.clone();
            let sequence = {
                let mut pending = self.debounce.lock().unwrap();
                pending
                    .entry(package_name)
                    .or_insert_with(|| Arc::new(Mutex::new(0)))
                    .clone()
            };
            let registry = self.clone();
            let current = {
                let mut value = sequence.lock().unwrap();
                *value += 1;
                *value
            };
            let ticket = ReloadTicket {
                sequence: Arc::clone(&sequence),
                generation: current,
            };
            thread::spawn(move || {
                thread::sleep(quiet_period);
                if *sequence.lock().unwrap() == current {
                    let _ = registry.reload(&package, &options);
                }
            });
            ticket
        }
    }

    #[derive(Clone)]
    pub struct ReloadTicket {
        sequence: Arc<Mutex<u64>>,
        generation: u64,
    }

    impl ReloadTicket {
        pub fn cancel(&self) {
            let mut sequence = self.sequence.lock().unwrap();
            if *sequence == self.generation {
                *sequence += 1;
            }
        }
    }
}

/// Registry metadata for typed tool contracts. It deliberately stores
/// contract identity separately from loaded implementation versions: an
/// implementation may reload without changing the contract it satisfies.
pub mod contract_registry {
    use super::contracts::{ContractDescriptor, ContractId};
    use std::collections::HashMap;

    #[derive(Clone, Debug, Default)]
    pub struct ContractRegistry {
        contracts: HashMap<ContractId, ContractDescriptor>,
    }

    impl ContractRegistry {
        pub fn register(&mut self, descriptor: ContractDescriptor) -> Result<(), String> {
            if descriptor.id.namespace.is_empty() || descriptor.id.interface.is_empty() {
                return Err("contract namespace and interface cannot be empty".to_owned());
            }
            if descriptor.interface != descriptor.id.interface {
                return Err(format!(
                    "contract interface {} does not match {}",
                    descriptor.interface, descriptor.id
                ));
            }
            if self
                .contracts
                .insert(descriptor.id.clone(), descriptor)
                .is_some()
            {
                return Err("contract is already registered".to_owned());
            }
            Ok(())
        }

        pub fn resolve(&self, id: &ContractId) -> Option<&ContractDescriptor> {
            self.contracts.get(id)
        }

        pub fn accepts(&self, expected: &ContractId, actual: &ContractId) -> bool {
            expected.namespace == actual.namespace
                && expected.interface == actual.interface
                && expected.major == actual.major
        }

        pub fn all(&self) -> impl Iterator<Item = &ContractDescriptor> {
            self.contracts.values()
        }
    }
}

/// Adapter that exposes one activated WASM verb component as a callable tool.
/// It is deliberately not a kernel Handler: installing it as a universal
/// fallback would recurse when the component uses its kernel capability.
pub mod tool_adapter {
    use super::{ComponentError, runtime::ActiveVersion};
    use artist_kernel::{KernelError, KernelHandle};
    use serde_json::Value;
    use std::sync::Arc;

    #[derive(Clone)]
    pub struct ComponentTool {
        component: Arc<ActiveVersion>,
        verb: String,
    }

    impl ComponentTool {
        pub fn new(component: ActiveVersion, verb: impl Into<String>) -> Self {
            Self {
                component: Arc::new(component),
                verb: verb.into(),
            }
        }

        pub fn verb(&self) -> &str {
            &self.verb
        }

        pub fn invoke(&self, args: Value, kernel: KernelHandle) -> Result<Value, KernelError> {
            self.invoke_with_context(args, kernel, artist_kernel::InvocationContext::default())
        }

        pub fn invoke_with_context(
            &self,
            args: Value,
            kernel: KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> Result<Value, KernelError> {
            let input_value = normalize_batch_input(args)?;
            let input =
                serde_json::to_string(&input_value).map_err(|error| KernelError::Handler {
                    message: format!("could not encode component input: {error}"),
                })?;
            let output = self
                .component
                .invoke_tool_json_with_context(&self.verb, &input, kernel, context)
                .map_err(component_error)?;
            single_batch_result(serde_json::from_str(&output).map_err(|error| {
                KernelError::Handler {
                    message: format!("component returned invalid output JSON: {error}"),
                }
            })?)
        }

        pub async fn invoke_async(
            &self,
            args: Value,
            kernel: KernelHandle,
        ) -> Result<Value, KernelError> {
            self.invoke_async_with_context(
                args,
                kernel,
                artist_kernel::InvocationContext::default(),
            )
            .await
        }

        pub async fn invoke_async_with_context(
            &self,
            args: Value,
            kernel: KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> Result<Value, KernelError> {
            let input_value = normalize_batch_input(args)?;
            let input =
                serde_json::to_string(&input_value).map_err(|error| KernelError::Handler {
                    message: format!("could not encode component input: {error}"),
                })?;
            let output = self
                .component
                .invoke_tool_json_async_with_context(&self.verb, &input, kernel, context)
                .await
                .map_err(component_error)?;
            single_batch_result(serde_json::from_str(&output).map_err(|error| {
                KernelError::Handler {
                    message: format!("component returned invalid output JSON: {error}"),
                }
            })?)
        }

        pub async fn invoke_async_with_scope(
            &self,
            args: Value,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Value, KernelError> {
            let input_value = normalize_batch_input(args)?;
            let input =
                serde_json::to_string(&input_value).map_err(|error| KernelError::Handler {
                    message: format!("could not encode component input: {error}"),
                })?;
            let output = self
                .component
                .invoke_tool_json_async_with_scope(&self.verb, &input, kernel, scope)
                .await
                .map_err(component_error)?;
            single_batch_result(serde_json::from_str(&output).map_err(|error| {
                KernelError::Handler {
                    message: format!("component returned invalid output JSON: {error}"),
                }
            })?)
        }

        pub async fn invoke_batch_async_with_scope(
            &self,
            args: Vec<Value>,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<Value>, KernelError> {
            let input = serde_json::json!({ "requests": args });
            let output = self
                .component
                .invoke_tool_json_async_with_scope(
                    &self.verb,
                    &serde_json::to_string(&input).map_err(|error| KernelError::Handler {
                        message: format!("could not encode component batch: {error}"),
                    })?,
                    kernel.clone(),
                    scope.clone(),
                )
                .await
                .map_err(component_error)?;
            Ok(self
                .invoke_batch_with_observations_from_output(&output, kernel, scope)
                .await?
                .into_iter()
                .map(|(value, _)| value)
                .collect())
        }

        pub async fn invoke_batch_with_observations_async_with_scope(
            &self,
            args: Vec<Value>,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<(Value, String)>, KernelError> {
            let input = serde_json::json!({ "requests": args });
            let output = self
                .component
                .invoke_tool_json_async_with_scope(
                    &self.verb,
                    &serde_json::to_string(&input).map_err(|error| KernelError::Handler {
                        message: format!("could not encode component batch: {error}"),
                    })?,
                    kernel.clone(),
                    scope.clone(),
                )
                .await
                .map_err(component_error)?;
            self.invoke_batch_with_observations_from_output(&output, kernel, scope)
                .await
        }

        async fn invoke_batch_with_observations_from_output(
            &self,
            output: &str,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<(Value, String)>, KernelError> {
            let values: Vec<Value> =
                match serde_json::from_str(output).map_err(|error| KernelError::Handler {
                    message: format!("component returned invalid batch JSON: {error}"),
                })? {
                    Value::Array(values) => values,
                    other => {
                        return Err(KernelError::Handler {
                            message: format!("component returned non-batch output: {other}"),
                        });
                    }
                };
            let mut observed = Vec::with_capacity(values.len());
            for value in values {
                let stdobs = self
                    .component
                    .observe_json_async_with_scope(&value, kernel.clone(), scope.child())
                    .await
                    .map_err(component_error)?;
                observed.push((value, stdobs));
            }
            Ok(observed)
        }
    }

    fn component_error(error: ComponentError) -> KernelError {
        KernelError::Handler {
            message: error.to_string(),
        }
    }

    fn single_batch_result(value: Value) -> Result<Value, KernelError> {
        match value {
            Value::Array(mut values) if values.len() == 1 => Ok(values.remove(0)),
            Value::Array(values) => Err(KernelError::Handler {
                message: format!(
                    "component returned {} results for a one-item call",
                    values.len()
                ),
            }),
            other => Err(KernelError::Handler {
                message: format!("component returned non-batch result: {other}"),
            }),
        }
    }

    fn normalize_model_input(mut input: Value) -> Result<Value, KernelError> {
        // `target` is the only adapter sugar retained at this boundary. The
        // active WIT contract owns every other field and its required shape.
        if let Some(object) = input.as_object_mut() {
            if !object.contains_key("uri") {
                if let Some(target) = object.remove("target") {
                    object.insert("uri".to_owned(), target);
                }
            }
        }
        Ok(input)
    }

    fn normalize_batch_input(input: Value) -> Result<Value, KernelError> {
        let input = normalize_model_input(input)?;
        match input {
            Value::Object(object) if object.get("requests").is_some_and(Value::is_array) => {
                Ok(Value::Object(object))
            }
            Value::Array(requests) => Ok(serde_json::json!({ "requests": requests })),
            input => Ok(serde_json::json!({ "requests": [input] })),
        }
    }
}

/// Kernel handler for the self-modifying `tools://` namespace.
pub mod tools {
    use super::{
        package::{BuildOptions, ToolPackage},
        runtime::ComponentRegistry,
        tool_adapter::ComponentTool,
    };
    use artist_kernel::{
        AnchoredText, BoxFuture, ClaimDecision, DynamicClaimProvider, DynamicResourceProvider,
        DynamicType, DynamicValue, DynamicVerbResult, FileHandler, FileResourceProvider,
        FileVerbBindings, KernelError, KernelHandle, ResourceFuture, ResourceUri, ToolDefinition,
        ToolProvider, VerbId,
    };
    use serde_json::Value;
    use std::{
        collections::{BTreeMap, HashMap},
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };

    /// A virtual mount over tool package files and their activated components.
    ///
    /// Package files use ordinary filesystem semantics through `FileHandler`.
    /// A verb request on a package root activates the package selected by the
    /// URI path when the verb matches its declared typed contract. A
    /// successful write/edit is visible to the next invocation; a failed rebuild leaves the previously active component
    /// untouched.
    pub struct ToolsHandler {
        root: PathBuf,
        registry: ComponentRegistry,
        options: BuildOptions,
        dirty: Arc<Mutex<std::collections::HashSet<PathBuf>>>,
        known_packages: Arc<Mutex<HashMap<String, (PathBuf, ToolRegistration)>>>,
        published_definitions: Arc<Mutex<HashMap<String, artist_kernel::VerbDefinition>>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ToolsVerbBindings {
        pub read: VerbId,
        pub write: VerbId,
        pub edit: VerbId,
        pub insert: VerbId,
        pub delete: VerbId,
        pub find: VerbId,
        pub grep: VerbId,
    }

    pub struct DynamicToolsProvider {
        handler: Arc<ToolsHandler>,
        bindings: ToolsVerbBindings,
    }

    impl DynamicToolsProvider {
        pub fn new(handler: Arc<ToolsHandler>, bindings: ToolsVerbBindings) -> Self {
            Self { handler, bindings }
        }
    }

    impl DynamicClaimProvider for DynamicToolsProvider {
        fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision {
            if uri.scheme() != "tools" {
                return ClaimDecision::Pass;
            }
            if [
                &self.bindings.read,
                &self.bindings.write,
                &self.bindings.edit,
                &self.bindings.insert,
                &self.bindings.delete,
                &self.bindings.find,
                &self.bindings.grep,
            ]
            .contains(&verb)
            {
                ClaimDecision::Handle
            } else {
                ClaimDecision::Pass
            }
        }
    }

    impl DynamicResourceProvider for DynamicToolsProvider {
        fn verb_definitions(&self) -> Vec<artist_kernel::VerbDefinition> {
            if self.handler.dynamic_verb_definitions().is_err() {
                return Vec::new();
            }
            [
                (&self.bindings.read, "read"),
                (&self.bindings.write, "write"),
                (&self.bindings.edit, "edit"),
                (&self.bindings.insert, "insert"),
                (&self.bindings.delete, "delete"),
                (&self.bindings.find, "find"),
                (&self.bindings.grep, "grep"),
            ]
            .into_iter()
            .map(|(identity, function)| {
                artist_kernel::VerbDefinition::new(
                    identity.clone(),
                    function,
                    function,
                    format!("Tools {function} provider"),
                )
            })
            .collect()
        }

        fn invoke<'a>(
            &'a self,
            verb: &'a VerbId,
            uri: &'a ResourceUri,
            input: DynamicValue,
        ) -> ResourceFuture<'a> {
            Box::pin(async move {
                let mapped_uri = self.handler.map_typed_uri(uri)?;
                let identity = [
                    (&self.bindings.read, "read"),
                    (&self.bindings.write, "write"),
                    (&self.bindings.edit, "edit"),
                    (&self.bindings.insert, "insert"),
                    (&self.bindings.delete, "delete"),
                    (&self.bindings.find, "find"),
                    (&self.bindings.grep, "grep"),
                ]
                .into_iter()
                .find_map(|(candidate, function)| (candidate == verb).then_some(function))
                .ok_or_else(|| KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: uri.to_string(),
                })?;
                let mapped_input =
                    remap_tools_dynamic(input, &|value| self.handler.map_typed_uri(&value))?;
                let provider = FileResourceProvider::new(
                    Arc::new(FileHandler::new(self.handler.root())?),
                    FileVerbBindings {
                        read: self.bindings.read.clone(),
                        write: self.bindings.write.clone(),
                        edit: self.bindings.edit.clone(),
                        insert: self.bindings.insert.clone(),
                        delete: self.bindings.delete.clone(),
                        find: self.bindings.find.clone(),
                        grep: self.bindings.grep.clone(),
                    },
                );
                let result = provider.invoke(verb, &mapped_uri, mapped_input).await?;
                if verb == &self.bindings.write
                    || verb == &self.bindings.edit
                    || verb == &self.bindings.insert
                {
                    if let Ok(relative) = self.handler.relative_uri_path(uri) {
                        self.handler.mark_dirty(&relative);
                    }
                }
                Ok(DynamicVerbResult {
                    verb: verb.clone(),
                    function: identity.to_owned(),
                    output: remap_tools_dynamic(result.output, &|value| {
                        self.handler.unmap_typed_uri(value)
                    })?,
                })
            })
        }
    }

    fn remap_tools_dynamic(
        value: DynamicValue,
        map_uri: &dyn Fn(ResourceUri) -> Result<ResourceUri, KernelError>,
    ) -> Result<DynamicValue, KernelError> {
        Ok(match value {
            DynamicValue::ResourceUri(uri) => DynamicValue::ResourceUri(map_uri(uri)?),
            DynamicValue::List(values) => DynamicValue::List(
                values
                    .into_iter()
                    .map(|value| remap_tools_dynamic(value, map_uri))
                    .collect::<Result<_, _>>()?,
            ),
            DynamicValue::Tuple(values) => DynamicValue::Tuple(
                values
                    .into_iter()
                    .map(|value| remap_tools_dynamic(value, map_uri))
                    .collect::<Result<_, _>>()?,
            ),
            DynamicValue::Record(fields) => DynamicValue::Record(
                fields
                    .into_iter()
                    .map(|(name, value)| Ok((name, remap_tools_dynamic(value, map_uri)?)))
                    .collect::<Result<_, KernelError>>()?,
            ),
            DynamicValue::Option(value) => DynamicValue::Option(
                value
                    .map(|value| remap_tools_dynamic(*value, map_uri).map(Box::new))
                    .transpose()?,
            ),
            DynamicValue::Result(result) => DynamicValue::Result(match result {
                Ok(value) => Ok(Box::new(remap_tools_dynamic(*value, map_uri)?)),
                Err(value) => Err(Box::new(remap_tools_dynamic(*value, map_uri)?)),
            }),
            DynamicValue::Variant(name, value) => DynamicValue::Variant(
                name,
                value
                    .map(|value| remap_tools_dynamic(*value, map_uri).map(Box::new))
                    .transpose()?,
            ),
            other => other,
        })
    }

    impl Clone for ToolsHandler {
        fn clone(&self) -> Self {
            Self {
                root: self.root.clone(),
                registry: self.registry.clone(),
                options: self.options.clone(),
                dirty: Arc::clone(&self.dirty),
                known_packages: Arc::clone(&self.known_packages),
                published_definitions: Arc::clone(&self.published_definitions),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    pub struct ToolRegistration {
        pub package: String,
        pub root: PathBuf,
        pub contract: super::contracts::ContractId,
        pub description: String,
        pub version: String,
        pub wit: Option<PathBuf>,
        pub artifact: Option<PathBuf>,
        pub input_schema: Option<serde_yaml::Value>,
        pub output_schema: Option<serde_yaml::Value>,
        pub capabilities: Vec<String>,
    }

    impl ToolRegistration {
        pub fn tool_name(&self) -> String {
            self.contract.interface.clone()
        }

        /// Convert a discovered tool package into the kernel's open-ended
        /// verb definition. The WIT source, not the legacy contract enum,
        /// supplies the typed input/output shape.
        pub fn dynamic_definition(&self) -> Result<artist_kernel::VerbDefinition, KernelError> {
            let wit = self
                .wit
                .as_ref()
                .ok_or_else(|| KernelError::InvalidRequest {
                    message: format!("tool package {} has no WIT contract", self.package),
                })?;
            let function = self.tool_name();
            let (input, output) =
                artist_kernel::dynamic_contract_from_wit(wit, &self.contract.interface, &function)?;
            let (package, interface) =
                self.contract.namespace.split_once(':').ok_or_else(|| {
                    KernelError::InvalidRequest {
                        message: format!(
                            "invalid tool contract namespace {}",
                            self.contract.namespace
                        ),
                    }
                })?;
            let identity = artist_kernel::VerbId::new(format!(
                "{package}:{interface}/{}@{}.0.0",
                function, self.contract.major
            ))
            .map_err(|message| KernelError::InvalidRequest { message })?;
            Ok(artist_kernel::VerbDefinition::new(
                identity,
                function.clone(),
                function,
                &self.description,
            )
            .with_contract(input, output)
            .with_extractor("tool-wit")
            .with_schema_adapter("tool-frontmatter")
            .with_source(self.root.clone(), self.artifact.clone()))
        }

        fn parameters_from_wit(&self) -> Value {
            self.input_schema
                .as_ref()
                .and_then(|schema| serde_json::to_value(schema).ok())
                .or_else(|| {
                    self.dynamic_definition().ok().and_then(|definition| {
                        definition.input_type.as_ref().map(dynamic_type_schema)
                    })
                })
                .unwrap_or_else(|| serde_json::json!({"type":"object","additionalProperties":true}))
        }
    }

    fn dynamic_type_schema(ty: &DynamicType) -> Value {
        match ty {
            DynamicType::Bool => serde_json::json!({"type":"boolean"}),
            DynamicType::S8
            | DynamicType::S16
            | DynamicType::S32
            | DynamicType::S64
            | DynamicType::U8
            | DynamicType::U16
            | DynamicType::U32
            | DynamicType::U64 => serde_json::json!({"type":"integer"}),
            DynamicType::F32 | DynamicType::F64 => serde_json::json!({"type":"number"}),
            DynamicType::Char | DynamicType::String | DynamicType::ResourceUri => {
                serde_json::json!({"type":"string"})
            }
            DynamicType::List(inner) => {
                serde_json::json!({"type":"array","items":dynamic_type_schema(inner)})
            }
            DynamicType::Tuple(items) => serde_json::json!({
                "type":"array",
                "prefixItems":items.iter().map(dynamic_type_schema).collect::<Vec<_>>(),
                "minItems":items.len(),
                "maxItems":items.len()
            }),
            DynamicType::Record(fields) => serde_json::json!({
                "type":"object",
                "properties":fields.iter().map(|(name, ty)| (name.clone(), dynamic_type_schema(ty))).collect::<serde_json::Map<_, _>>(),
                "required":fields.iter().filter_map(|(name, ty)| (!matches!(ty, DynamicType::Option(_))).then_some(name.clone())).collect::<Vec<_>>(),
                "additionalProperties":false
            }),
            DynamicType::Option(inner) => {
                serde_json::json!({"anyOf":[dynamic_type_schema(inner),{"type":"null"}]})
            }
            DynamicType::Result { ok, err } => {
                let mut variants = Vec::new();
                if let Some(ok) = ok {
                    variants.push(dynamic_type_schema(ok));
                }
                if let Some(err) = err {
                    variants.push(dynamic_type_schema(err));
                }
                serde_json::json!({"anyOf":variants})
            }
            DynamicType::Enum(values) => serde_json::json!({"type":"string","enum":values}),
            DynamicType::Variant(values)
                if values.len() == 3
                    && values.contains_key("top")
                    && values.contains_key("bottom")
                    && values.contains_key("at") =>
            {
                serde_json::json!({
                    "type":"string",
                    "anyOf":[{"enum":["top","bottom"]},{"type":"string","pattern":"^#.+$"}]
                })
            }
            DynamicType::Variant(values) => serde_json::json!({
                "type":"object",
                "oneOf":values.iter().map(|(name, payload)| {
                    let value = payload.as_ref().map(dynamic_type_schema).unwrap_or_else(|| serde_json::json!({"type":"null"}));
                    serde_json::json!({"properties":{name:value},"required":[name],"additionalProperties":false})
                }).collect::<Vec<_>>()
            }),
            DynamicType::Flags(values) => {
                serde_json::json!({"type":"array","items":{"type":"string","enum":values}})
            }
        }
    }

    impl ToolsHandler {
        pub fn new(
            root: impl AsRef<Path>,
            granted_capabilities: impl IntoIterator<Item = String>,
        ) -> Result<Self, KernelError> {
            Self::new_with_watcher(root, granted_capabilities, None)
        }

        pub fn new_with_watcher(
            root: impl AsRef<Path>,
            granted_capabilities: impl IntoIterator<Item = String>,
            watcher: Option<&super::watcher::SharedWatcher>,
        ) -> Result<Self, KernelError> {
            let root =
                std::fs::canonicalize(root.as_ref()).map_err(|error| KernelError::Handler {
                    message: format!(
                        "canonicalize tools root {}: {error}",
                        root.as_ref().display()
                    ),
                })?;
            let mut granted_capabilities = granted_capabilities.into_iter().collect::<Vec<_>>();
            if granted_capabilities.is_empty() {
                let entries = std::fs::read_dir(&root).map_err(|error| KernelError::Handler {
                    message: format!("read tools root {}: {error}", root.display()),
                })?;
                for entry in entries {
                    let entry = entry.map_err(|error| KernelError::Handler {
                        message: format!("read tools directory entry: {error}"),
                    })?;
                    if !entry
                        .file_type()
                        .map_err(|error| KernelError::Handler {
                            message: format!("inspect tools directory entry: {error}"),
                        })?
                        .is_dir()
                    {
                        continue;
                    }
                    if let Ok(package) = ToolPackage::discover(entry.path()) {
                        granted_capabilities.extend(package.manifest.capabilities);
                    }
                }
                granted_capabilities.sort();
                granted_capabilities.dedup();
            }
            let handler = Self {
                root,
                registry: ComponentRegistry::new(),
                options: BuildOptions {
                    granted_capabilities,
                    ..BuildOptions::default()
                },
                dirty: watcher.map_or_else(
                    || Arc::new(Mutex::new(std::collections::HashSet::new())),
                    super::watcher::SharedWatcher::dirty_set,
                ),
                known_packages: Arc::new(Mutex::new(HashMap::new())),
                published_definitions: Arc::new(Mutex::new(HashMap::new())),
            };
            let registrations = handler.registrations()?;
            handler.ensure_activated(&registrations);
            Ok(handler)
        }

        pub fn root(&self) -> &Path {
            &self.root
        }

        /// Discover the current tool configuration without activating code.
        /// Universal and extension contracts are returned alike; provider
        /// adapters can use this as the dynamic model-tool registry.
        pub fn registrations(&self) -> Result<Vec<ToolRegistration>, KernelError> {
            let mut registrations = Vec::new();
            let mut live_package_paths = std::collections::HashSet::new();
            let mut valid_package_paths = std::collections::HashSet::new();
            let mut observed_package_names = std::collections::HashSet::new();
            for entry in std::fs::read_dir(&self.root).map_err(|error| KernelError::Handler {
                message: format!("read tools root {}: {error}", self.root.display()),
            })? {
                let entry = entry.map_err(|error| KernelError::Handler {
                    message: format!("read tools directory entry: {error}"),
                })?;
                if !entry
                    .file_type()
                    .map_err(|error| KernelError::Handler {
                        message: format!("inspect tools directory entry: {error}"),
                    })?
                    .is_dir()
                {
                    continue;
                }
                if !entry.path().join("tool.md").is_file() {
                    continue;
                }
                live_package_paths.insert(entry.path());
                let package = match ToolPackage::discover(entry.path()) {
                    Ok(package) => package,
                    Err(_) => continue,
                };
                let Some(contract) = package.contract else {
                    continue;
                };
                let registration = ToolRegistration {
                    package: package.manifest.name,
                    root: package.root,
                    contract,
                    description: package.manifest.description,
                    version: package.manifest.version,
                    wit: package.wit,
                    artifact: package.wasm,
                    input_schema: package.manifest.input_schema,
                    output_schema: package.manifest.output_schema,
                    capabilities: package.manifest.capabilities,
                };
                valid_package_paths.insert(entry.path());
                observed_package_names.insert(registration.package.clone());
                self.known_packages.lock().unwrap().insert(
                    registration.package.clone(),
                    (entry.path(), registration.clone()),
                );
            }
            self.known_packages
                .lock()
                .unwrap()
                .retain(|name, (path, _)| {
                    live_package_paths.contains(path)
                        && (!valid_package_paths.contains(path)
                            || observed_package_names.contains(name))
                });
            registrations.extend(
                self.known_packages
                    .lock()
                    .unwrap()
                    .values()
                    .map(|(_, registration)| registration.clone()),
            );
            registrations.sort_by(|left, right| left.package.cmp(&right.package));
            registrations.dedup_by(|left, right| left.package == right.package);
            registrations
                .sort_by(|left, right| left.contract.to_string().cmp(&right.contract.to_string()));
            Ok(registrations)
        }

        /// Publish the discovered tool catalog in the kernel's open-ended
        /// verb-definition shape. Activation remains a separate kernel
        /// transaction so a malformed package cannot partially replace the
        /// active set.
        pub fn dynamic_verb_definitions(
            &self,
        ) -> Result<Vec<artist_kernel::VerbDefinition>, KernelError> {
            let registrations = self.registrations()?;
            self.ensure_activated(&registrations);
            Ok(self
                .published_definitions
                .lock()
                .map_err(|_| KernelError::Handler {
                    message: "published tool definition lock poisoned".to_owned(),
                })?
                .values()
                .cloned()
                .collect())
        }

        /// Activation is part of publication, not a lazy first-call side
        /// effect. The model catalog and the resource catalog therefore never
        /// advertise a generation that has not already passed component
        /// loading and ABI validation.
        fn ensure_activated(&self, registrations: &[ToolRegistration]) {
            let granted = &self.options.granted_capabilities;
            let live = registrations
                .iter()
                .map(|registration| registration.package.as_str())
                .collect::<std::collections::HashSet<_>>();
            if let Ok(mut published) = self.published_definitions.lock() {
                published.retain(|package, _| live.contains(package.as_str()));
            }
            let candidates = registrations
                .iter()
                .filter(|registration| {
                    registration
                        .capabilities
                        .iter()
                        .all(|capability| granted.iter().any(|granted| granted == capability))
                })
                .filter_map(|registration| {
                    let package_root = self.package_path_for_name(&registration.package).ok()?;
                    if !package_root.join("tool.wasm").is_file()
                        && !package_root.join("Cargo.toml").is_file()
                    {
                        return None;
                    }
                    Some((registration, package_root))
                })
                .collect::<Vec<_>>();
            std::thread::scope(|thread_scope| {
                let activations = candidates
                    .into_iter()
                    .map(|(registration, package_root)| {
                        thread_scope.spawn(move || (registration, self.activate(&package_root)))
                    })
                    .collect::<Vec<_>>();
                for activation in activations {
                    let Ok((registration, result)) = activation.join() else {
                        continue;
                    };
                    if result.is_ok() {
                        if let Ok(definition) = registration.dynamic_definition() {
                            if let Ok(mut published) = self.published_definitions.lock() {
                                published.insert(registration.package.clone(), definition);
                            }
                        }
                    }
                }
            });
        }

        fn relative_uri_path(&self, uri: &ResourceUri) -> Result<PathBuf, KernelError> {
            if uri.scheme() != "tools" {
                return Err(KernelError::UnsupportedUri {
                    uri: uri.to_string(),
                });
            }
            let host = uri.as_ref().host_str().unwrap_or_default();
            let path = format!("{host}{}", uri.path())
                .trim_start_matches('/')
                .to_owned();
            let relative = PathBuf::from(path);
            if relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::RootDir
                )
            }) {
                return Err(KernelError::InvalidRequest {
                    message: "tools URI escapes the tools root".to_owned(),
                });
            }
            Ok(relative)
        }

        fn map_typed_uri(&self, uri: &ResourceUri) -> Result<ResourceUri, KernelError> {
            let relative = self.relative_uri_path(uri)?;
            let mut mapped = ResourceUri::parse(&self.root.join(relative).display().to_string())?;
            if let Some(query) = uri.query() {
                mapped = mapped.with_query(query);
            }
            if let Some(fragment) = uri.fragment() {
                mapped = mapped.with_fragment(fragment);
            }
            Ok(mapped)
        }

        fn unmap_typed_uri(&self, uri: ResourceUri) -> Result<ResourceUri, KernelError> {
            let path = uri
                .as_ref()
                .to_file_path()
                .map_err(|_| KernelError::InvalidUri {
                    message: uri.to_string(),
                })?;
            let relative = path
                .strip_prefix(&self.root)
                .map_err(|_| KernelError::InvalidUri {
                    message: uri.to_string(),
                })?;
            let relative = relative
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            let mut mapped = ResourceUri::parse(&format!("tools:///{relative}"))?;
            if let Some(query) = uri.query() {
                mapped = mapped.with_query(query);
            }
            if let Some(fragment) = uri.fragment() {
                mapped = mapped.with_fragment(fragment);
            }
            Ok(mapped)
        }

        fn unmap_text(&self, mut text: AnchoredText) -> Result<AnchoredText, KernelError> {
            text.uri = self.unmap_typed_uri(text.uri)?;
            Ok(text)
        }

        fn package_root(&self, relative: &Path) -> Result<PathBuf, KernelError> {
            let package = relative
                .components()
                .next()
                .and_then(|component| match component {
                    std::path::Component::Normal(package) => Some(package),
                    _ => None,
                })
                .ok_or_else(|| KernelError::InvalidRequest {
                    message: "tools:// target must name a package".to_owned(),
                })?;
            let root = self.root.join(package);
            if !root.is_dir() {
                return Err(KernelError::NotFound {
                    uri: root.display().to_string(),
                });
            }
            Ok(root)
        }

        fn activate(
            &self,
            package_root: &Path,
        ) -> Result<super::runtime::ActiveVersion, KernelError> {
            let package = match ToolPackage::discover(package_root) {
                Ok(package) => package,
                Err(error) => {
                    let known = self
                        .known_packages
                        .lock()
                        .unwrap()
                        .values()
                        .find(|(path, _)| path == package_root)
                        .map(|(_, registration)| registration.package.clone());
                    if let Some(package_name) = known {
                        if let Ok(active) = self.registry.current(&package_name) {
                            return Err(KernelError::Handler {
                                message: format!(
                                    "candidate package {} is invalid; generation {} remains active",
                                    package_name,
                                    active.generation()
                                ),
                            });
                        }
                    }
                    return Err(component_error(error));
                }
            };
            let key = package_root.to_owned();
            let current = self.registry.current(&package.manifest.name).ok();
            let dirty = self.dirty.lock().unwrap().contains(&key);
            if let Some(active) = current.clone().filter(|_| !dirty) {
                return Ok(active);
            }
            let dependencies = if package.wit.is_some()
                && package
                    .contract
                    .as_ref()
                    .is_some_and(|contract| contract.namespace != "artist:tool")
            {
                match self.custom_dependencies(&package) {
                    Ok(dependencies) => dependencies,
                    Err(error) => return Err(error),
                }
            } else {
                Vec::new()
            };
            let active = match self.registry.reload_with_dependency_specs(
                &package,
                &self.options,
                dependencies,
            ) {
                Ok(active) => active,
                Err(error) => return Err(component_error(error)),
            };
            self.dirty.lock().unwrap().remove(&key);
            Ok(active)
        }

        fn active_for_scope(
            &self,
            package: &str,
            scope: &artist_kernel::InvocationScope,
        ) -> Result<super::runtime::ActiveVersion, KernelError> {
            let key = format!("tool-generation:{package}");
            if let Some(active) = scope.generation_handle::<super::runtime::ActiveVersion>(&key) {
                return Ok((*active).clone());
            }
            let active = self
                .registry
                .current(package)
                .map_err(|error| KernelError::Handler {
                    message: error.to_string(),
                })?;
            let generation = scope.snapshot_generation(package, active.generation());
            if generation != active.generation() {
                return Err(KernelError::Conflict {
                    uri: format!("tool generation changed: {package}"),
                });
            }
            scope.pin_generation_handle(key, Arc::new(active.clone()));
            Ok(active)
        }

        fn custom_dependencies(
            &self,
            current: &ToolPackage,
        ) -> Result<Vec<super::DependencySpec>, KernelError> {
            let engine =
                super::package::component_engine().map_err(|error| KernelError::Handler {
                    message: error.to_string(),
                })?;
            fn make_spec(
                contract: &str,
                packages: &[(String, ToolPackage)],
                registry: &ComponentRegistry,
                options: &BuildOptions,
                engine: &wasmtime::Engine,
                visiting: &mut std::collections::HashSet<String>,
            ) -> Result<super::DependencySpec, KernelError> {
                let package = packages
                    .iter()
                    .find(|(id, _)| id == contract)
                    .map(|(_, package)| package)
                    .ok_or_else(|| KernelError::NotFound {
                        uri: contract.to_owned(),
                    })?;
                let bytes = if let Ok(active) = registry.current(&package.manifest.name) {
                    (*active.artifact_bytes()).clone()
                } else {
                    // Dependency imports bind to a registered active
                    // generation, never to an unregistered artifact that was
                    // merely built while resolving another package.
                    let active = registry.reload(package, options).map_err(component_error)?;
                    (*active.artifact_bytes()).clone()
                };
                if !visiting.insert(contract.to_owned()) {
                    return Err(KernelError::InvalidRequest {
                        message: format!("custom tool dependency cycle at {contract}"),
                    });
                }
                let component = wasmtime::component::Component::from_binary(engine, &bytes)
                    .map_err(|error| KernelError::Handler {
                        message: format!("load dependency {contract}: {error}"),
                    })?;
                let mut dependencies = Vec::new();
                for (import, _) in component.component_type().imports(engine) {
                    let children = packages
                        .iter()
                        .filter(|(candidate, _)| super::contract_matches_import(candidate, import))
                        .collect::<Vec<_>>();
                    if children.len() > 1 {
                        return Err(KernelError::Conflict {
                            uri: import.to_owned(),
                        });
                    }
                    if let Some((child, _)) = children.first() {
                        dependencies.push(make_spec(
                            child, packages, registry, options, engine, visiting,
                        )?);
                    }
                }
                visiting.remove(contract);
                Ok(super::DependencySpec {
                    contract: contract.to_owned(),
                    bytes,
                    dependencies,
                })
            }
            let mut packages = Vec::new();
            for entry in std::fs::read_dir(&self.root).map_err(|error| KernelError::Handler {
                message: error.to_string(),
            })? {
                let entry = entry.map_err(|error| KernelError::Handler {
                    message: error.to_string(),
                })?;
                if !entry
                    .file_type()
                    .map_err(|error| KernelError::Handler {
                        message: error.to_string(),
                    })?
                    .is_dir()
                {
                    continue;
                }
                let Ok(package) = ToolPackage::discover(entry.path()) else {
                    continue;
                };
                let Some(contract) = package.contract.as_ref() else {
                    continue;
                };
                if contract.namespace != "artist:tool"
                    && package.wit.is_some()
                    && package.manifest.name != current.manifest.name
                {
                    packages.push((contract.to_string(), package));
                }
            }
            let current_build = current.build(&self.options).map_err(component_error)?;
            let current_bytes =
                std::fs::read(current_build.artifact).map_err(|error| KernelError::Handler {
                    message: error.to_string(),
                })?;
            let component = wasmtime::component::Component::from_binary(&engine, &current_bytes)
                .map_err(|error| KernelError::Handler {
                    message: error.to_string(),
                })?;
            let mut output = Vec::new();
            for (import, _) in component.component_type().imports(&engine) {
                let matches = packages
                    .iter()
                    .filter(|(id, _)| super::contract_matches_import(id, import))
                    .collect::<Vec<_>>();
                if matches.len() > 1 {
                    return Err(KernelError::Conflict {
                        uri: import.to_owned(),
                    });
                }
                if let Some((contract, _)) = matches.first() {
                    output.push(make_spec(
                        contract,
                        &packages,
                        &self.registry,
                        &self.options,
                        &engine,
                        &mut std::collections::HashSet::new(),
                    )?);
                }
            }
            Ok(output)
        }

        fn package_path_for_name(&self, name: &str) -> Result<PathBuf, KernelError> {
            if let Some((path, _)) = self.known_packages.lock().unwrap().get(name) {
                return Ok(path.clone());
            }
            for entry in std::fs::read_dir(&self.root).map_err(|error| KernelError::Handler {
                message: format!("read tools root {}: {error}", self.root.display()),
            })? {
                let entry = entry.map_err(|error| KernelError::Handler {
                    message: format!("read tools directory entry: {error}"),
                })?;
                if !entry
                    .file_type()
                    .map_err(|error| KernelError::Handler {
                        message: format!("inspect tools directory entry: {error}"),
                    })?
                    .is_dir()
                {
                    continue;
                }
                if !entry.path().join("tool.md").is_file() {
                    continue;
                }
                let package = match ToolPackage::discover(entry.path()) {
                    Ok(package) => package,
                    Err(_) => continue,
                };
                if package.manifest.name == name {
                    return Ok(entry.path());
                }
            }
            Err(KernelError::NotFound {
                uri: name.to_owned(),
            })
        }

        fn mark_dirty(&self, relative: &Path) {
            if let Ok(package_root) = self.package_root(relative) {
                self.dirty.lock().unwrap().insert(package_root);
            }
        }

        fn prepare_write(&self, relative: &Path) -> Result<(), KernelError> {
            for component in relative.components() {
                if !matches!(component, std::path::Component::Normal(_)) {
                    return Err(KernelError::InvalidRequest {
                        message: "tools:// write paths must remain within the tools root"
                            .to_owned(),
                    });
                }
            }
            let parent = relative.parent().unwrap_or_else(|| Path::new(""));
            std::fs::create_dir_all(self.root.join(parent)).map_err(|error| KernelError::Handler {
                message: format!("create tools package directories: {error}"),
            })
        }

        async fn execute_named_inner_async(
            &self,
            name: &str,
            args: Value,
            host: KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> Result<Value, KernelError> {
            self.execute_named_inner_async_scope(
                name,
                args,
                host,
                artist_kernel::InvocationScope::new(context),
            )
            .await
        }

        async fn execute_named_inner_async_scope(
            &self,
            name: &str,
            args: Value,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Value, KernelError> {
            let registration = self
                .registrations()?
                .into_iter()
                .find(|registration| registration.tool_name() == name)
                .ok_or_else(|| KernelError::Handler {
                    message: format!("no named tool registered: {name}"),
                })?;
            let active = self.active_for_scope(&registration.package, &scope)?;
            ComponentTool::new(active, registration.contract.interface)
                .invoke_async_with_scope(args, host, scope)
                .await
        }
    }

    impl ToolProvider for ToolsHandler {
        fn tool_definitions(&self) -> Vec<ToolDefinition> {
            let registrations = match self.registrations() {
                Ok(registrations) => registrations,
                Err(_) => return Vec::new(),
            };
            let _ = self.dynamic_verb_definitions();
            let published = match self.published_definitions.lock() {
                Ok(published) => published.clone(),
                Err(_) => return Vec::new(),
            };
            registrations
                .into_iter()
                .filter_map(|registration| {
                    let definition = published.get(&registration.package)?;
                    let input_type = definition.input_type.clone();
                    Some(ToolDefinition {
                        name: definition.function.clone(),
                        description: definition.description.clone(),
                        parameters: input_type
                            .as_ref()
                            .map(dynamic_type_schema)
                            .and_then(|schema| json_to_dynamic(schema).ok())
                            .unwrap_or_else(|| DynamicValue::Record(Default::default())),
                        input_type,
                    })
                })
                .collect()
        }

        fn execute_tool<'a>(
            &'a self,
            name: &'a str,
            args: DynamicValue,
            host: KernelHandle,
        ) -> BoxFuture<'a, Result<DynamicValue, KernelError>> {
            Box::pin(async move {
                let registration = self
                    .registrations()?
                    .into_iter()
                    .find(|registration| registration.tool_name() == name)
                    .ok_or_else(|| KernelError::Handler {
                        message: format!("no named tool registered: {name}"),
                    })?;
                let output = self
                    .execute_named_inner_async(
                        name,
                        dynamic_to_json(&args),
                        host,
                        artist_kernel::InvocationContext::default(),
                    )
                    .await?;
                typed_tool_output(&output, &registration)
            })
        }

        fn execute_tool_with_context<'a>(
            &'a self,
            name: &'a str,
            args: DynamicValue,
            host: KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> BoxFuture<'a, Result<DynamicValue, KernelError>> {
            Box::pin(async move {
                let registration = self
                    .registrations()?
                    .into_iter()
                    .find(|registration| registration.tool_name() == name)
                    .ok_or_else(|| KernelError::Handler {
                        message: format!("no named tool registered: {name}"),
                    })?;
                let output = self
                    .execute_named_inner_async(name, dynamic_to_json(&args), host, context)
                    .await?;
                typed_tool_output(&output, &registration)
            })
        }

        fn execute_tool_with_scope<'a>(
            &'a self,
            name: &'a str,
            args: DynamicValue,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> BoxFuture<'a, Result<DynamicValue, KernelError>> {
            Box::pin(async move {
                let registration = self
                    .registrations()?
                    .into_iter()
                    .find(|registration| registration.tool_name() == name)
                    .ok_or_else(|| KernelError::Handler {
                        message: format!("no named tool registered: {name}"),
                    })?;
                let output = self
                    .execute_named_inner_async_scope(name, dynamic_to_json(&args), host, scope)
                    .await?;
                typed_tool_output(&output, &registration)
            })
        }

        fn execute_tool_for_model<'a>(
            &'a self,
            name: &'a str,
            args: DynamicValue,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> BoxFuture<'a, Result<DynamicValue, KernelError>> {
            Box::pin(async move {
                let registration = self
                    .registrations()?
                    .into_iter()
                    .find(|registration| registration.tool_name() == name)
                    .ok_or_else(|| KernelError::Handler {
                        message: format!("no named tool registered: {name}"),
                    })?;
                let active = self.active_for_scope(&registration.package, &scope)?;
                let tool = ComponentTool::new(active, registration.contract.interface.clone());
                let values = tool
                    .invoke_batch_with_observations_async_with_scope(
                        vec![dynamic_to_json(&args)],
                        host,
                        scope,
                    )
                    .await?;
                let (value, _stdobs) =
                    values
                        .into_iter()
                        .next()
                        .ok_or_else(|| KernelError::Handler {
                            message: "component returned no tool result".to_owned(),
                        })?;
                json_to_dynamic(value)
            })
        }

        fn execute_tool_for_model_result<'a>(
            &'a self,
            name: &'a str,
            args: DynamicValue,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> BoxFuture<'a, Result<artist_kernel::ToolModelResult, KernelError>> {
            Box::pin(async move {
                let registration = self
                    .registrations()?
                    .into_iter()
                    .find(|registration| registration.tool_name() == name)
                    .ok_or_else(|| KernelError::Handler {
                        message: format!("no named tool registered: {name}"),
                    })?;
                let active = self.active_for_scope(&registration.package, &scope)?;
                let generation = active.generation();
                let verb = registration.dynamic_definition()?.identity;
                let tool = ComponentTool::new(active, registration.contract.interface.clone());
                let (value, stdobs) = tool
                    .invoke_batch_with_observations_async_with_scope(
                        vec![dynamic_to_json(&args)],
                        host,
                        scope,
                    )
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| KernelError::Handler {
                        message: "component returned no tool result".to_owned(),
                    })?;
                let stdout = typed_tool_output(&value, &registration)?;
                Ok(artist_kernel::ToolModelResult {
                    stdobs,
                    stdout: Ok(stdout),
                    verb,
                    generation,
                })
            })
        }

        fn execute_tools_for_model<'a>(
            &'a self,
            name: &'a str,
            args: Vec<DynamicValue>,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> BoxFuture<'a, Vec<Result<DynamicValue, KernelError>>> {
            Box::pin(async move {
                let registration = match self
                    .registrations()
                    .ok()
                    .and_then(|items| items.into_iter().find(|item| item.tool_name() == name))
                {
                    Some(registration) => registration,
                    None => {
                        return args
                            .into_iter()
                            .map(|_| {
                                Err(KernelError::Handler {
                                    message: format!("no named tool registered: {name}"),
                                })
                            })
                            .collect();
                    }
                };
                let active = match self.active_for_scope(&registration.package, &scope) {
                    Ok(active) => active,
                    Err(error) => return args.into_iter().map(|_| Err(error.clone())).collect(),
                };
                let tool = ComponentTool::new(active, registration.contract.interface.clone());
                let values = match tool
                    .invoke_batch_with_observations_async_with_scope(
                        args.iter().map(dynamic_to_json).collect(),
                        host,
                        scope,
                    )
                    .await
                {
                    Ok(values) => values,
                    Err(error) => return args.into_iter().map(|_| Err(error.clone())).collect(),
                };
                values
                    .into_iter()
                    .map(|(value, _stdobs)| json_to_dynamic(value).map_err(|error| error))
                    .collect()
            })
        }

        fn execute_tools_for_model_results<'a>(
            &'a self,
            name: &'a str,
            args: Vec<DynamicValue>,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> BoxFuture<'a, Vec<Result<artist_kernel::ToolModelResult, KernelError>>> {
            Box::pin(async move {
                let registration = match self
                    .registrations()
                    .ok()
                    .and_then(|items| items.into_iter().find(|item| item.tool_name() == name))
                {
                    Some(registration) => registration,
                    None => {
                        return args
                            .into_iter()
                            .map(|_| {
                                Err(KernelError::Handler {
                                    message: format!("no named tool registered: {name}"),
                                })
                            })
                            .collect();
                    }
                };
                let active = match self.active_for_scope(&registration.package, &scope) {
                    Ok(active) => active,
                    Err(error) => return args.into_iter().map(|_| Err(error.clone())).collect(),
                };
                let generation = active.generation();
                let verb = match registration.dynamic_definition() {
                    Ok(definition) => definition.identity,
                    Err(error) => return args.into_iter().map(|_| Err(error.clone())).collect(),
                };
                let tool = ComponentTool::new(active, registration.contract.interface.clone());
                let values = match tool
                    .invoke_batch_with_observations_async_with_scope(
                        args.iter().map(dynamic_to_json).collect(),
                        host,
                        scope,
                    )
                    .await
                {
                    Ok(values) => values,
                    Err(error) => return args.into_iter().map(|_| Err(error.clone())).collect(),
                };
                values
                    .into_iter()
                    .map(|(value, stdobs)| {
                        let stdout = typed_tool_output(&value, &registration)?;
                        Ok(artist_kernel::ToolModelResult {
                            stdobs,
                            stdout: Ok(stdout),
                            verb: verb.clone(),
                            generation,
                        })
                    })
                    .collect()
            })
        }

        fn execute_tool_batch_with_scope<'a>(
            &'a self,
            name: &'a str,
            args: Vec<DynamicValue>,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> BoxFuture<'a, Vec<Result<DynamicVerbResult, KernelError>>> {
            Box::pin(async move {
                let registration = match self.registrations().ok().and_then(|registrations| {
                    registrations
                        .into_iter()
                        .find(|registration| registration.tool_name() == name)
                }) {
                    Some(registration) => registration,
                    None => {
                        return args
                            .into_iter()
                            .map(|_| {
                                Err(KernelError::Handler {
                                    message: format!("no named tool registered: {name}"),
                                })
                            })
                            .collect();
                    }
                };
                let active = match self.active_for_scope(&registration.package, &scope) {
                    Ok(active) => active,
                    Err(error) => {
                        let message = error.to_string();
                        return args
                            .into_iter()
                            .map(|_| {
                                Err(KernelError::Handler {
                                    message: message.clone(),
                                })
                            })
                            .collect();
                    }
                };
                let tool = ComponentTool::new(active, registration.contract.interface.clone());
                let values = match tool
                    .invoke_batch_async_with_scope(
                        args.iter().map(dynamic_to_json).collect(),
                        host,
                        scope,
                    )
                    .await
                {
                    Ok(values) => values,
                    Err(error) => {
                        let message = error.to_string();
                        return args
                            .into_iter()
                            .map(|_| {
                                Err(KernelError::Handler {
                                    message: message.clone(),
                                })
                            })
                            .collect();
                    }
                };
                let identity = registration
                    .dynamic_definition()
                    .map(|definition| definition.identity);
                values
                    .into_iter()
                    .map(|value| {
                        match (identity.clone(), typed_tool_output(&value, &registration)) {
                            (Ok(verb), Ok(output)) => Ok(DynamicVerbResult {
                                verb,
                                function: name.to_owned(),
                                output,
                            }),
                            (Err(error), _) => Err(error),
                            (_, Err(error)) => Err(error),
                        }
                    })
                    .collect()
            })
        }
    }

    fn component_error(error: super::ComponentError) -> KernelError {
        KernelError::Handler {
            message: error.to_string(),
        }
    }

    pub(crate) fn json_to_dynamic(value: Value) -> Result<DynamicValue, KernelError> {
        Ok(match value {
            Value::Null => DynamicValue::Option(None),
            Value::Bool(value) => DynamicValue::Bool(value),
            Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    DynamicValue::S64(value)
                } else if let Some(value) = value.as_u64() {
                    DynamicValue::U64(value)
                } else {
                    DynamicValue::F64(value.as_f64().ok_or_else(|| {
                        KernelError::InvalidRequest {
                            message: "model number is not representable as a dynamic value"
                                .to_owned(),
                        }
                    })?)
                }
            }
            Value::String(value) => DynamicValue::String(value),
            Value::Array(values) => DynamicValue::List(
                values
                    .into_iter()
                    .map(json_to_dynamic)
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(fields) => DynamicValue::Record(
                fields
                    .into_iter()
                    .map(|(name, value)| Ok((name, json_to_dynamic(value)?)))
                    .collect::<Result<_, KernelError>>()?,
            ),
        })
    }

    pub(crate) fn json_to_dynamic_host(
        value: Value,
        uri: &str,
    ) -> Result<DynamicValue, KernelError> {
        fn promote(value: DynamicValue, uri: &str) -> Result<DynamicValue, KernelError> {
            Ok(match value {
                DynamicValue::Record(fields) => DynamicValue::Record(
                    fields
                        .into_iter()
                        .map(|(name, value)| {
                            let value = if name == "uri" || name == "root" {
                                DynamicValue::ResourceUri(ResourceUri::parse(match value {
                                    DynamicValue::String(ref value) => value,
                                    _ => uri,
                                })?)
                            } else if matches!(name.as_str(), "before" | "after") {
                                match value {
                                    DynamicValue::S64(value) => {
                                        DynamicValue::U32(u32::try_from(value).map_err(|_| {
                                            KernelError::InvalidRequest {
                                                message: format!("{name} is outside u32 range"),
                                            }
                                        })?)
                                    }
                                    DynamicValue::U64(value) => {
                                        DynamicValue::U32(u32::try_from(value).map_err(|_| {
                                            KernelError::InvalidRequest {
                                                message: format!("{name} is outside u32 range"),
                                            }
                                        })?)
                                    }
                                    value => promote(value, uri)?,
                                }
                            } else if name == "at" {
                                match value {
                                    DynamicValue::Record(mut fields) if fields.len() == 1 => {
                                        if let Some(value) = fields.remove("top") {
                                            if matches!(value, DynamicValue::Option(None)) {
                                                DynamicValue::Variant("top".to_owned(), None)
                                            } else {
                                                DynamicValue::Record(fields)
                                            }
                                        } else if let Some(value) = fields.remove("bottom") {
                                            if matches!(value, DynamicValue::Option(None)) {
                                                DynamicValue::Variant("bottom".to_owned(), None)
                                            } else {
                                                DynamicValue::Record(fields)
                                            }
                                        } else if let Some(value) = fields.remove("at") {
                                            DynamicValue::Variant(
                                                "at".to_owned(),
                                                Some(Box::new(promote(value, uri)?)),
                                            )
                                        } else {
                                            DynamicValue::Record(fields)
                                        }
                                    }
                                    value => promote(value, uri)?,
                                }
                            } else {
                                promote(value, uri)?
                            };
                            Ok((name, value))
                        })
                        .collect::<Result<_, KernelError>>()?,
                ),
                DynamicValue::List(values) => DynamicValue::List(
                    values
                        .into_iter()
                        .map(|value| promote(value, uri))
                        .collect::<Result<_, _>>()?,
                ),
                value => value,
            })
        }
        promote(json_to_dynamic(value)?, uri)
    }

    pub(crate) fn json_to_dynamic_typed(
        value: &Value,
        ty: &DynamicType,
    ) -> Result<DynamicValue, KernelError> {
        let invalid = || KernelError::InvalidRequest {
            message: format!("component output does not satisfy typed contract {ty:?}"),
        };
        match ty {
            DynamicType::Bool => value.as_bool().map(DynamicValue::Bool).ok_or_else(invalid),
            DynamicType::S8 => value
                .as_i64()
                .and_then(|v| i8::try_from(v).ok())
                .map(DynamicValue::S8)
                .ok_or_else(invalid),
            DynamicType::S16 => value
                .as_i64()
                .and_then(|v| i16::try_from(v).ok())
                .map(DynamicValue::S16)
                .ok_or_else(invalid),
            DynamicType::S32 => value
                .as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .map(DynamicValue::S32)
                .ok_or_else(invalid),
            DynamicType::S64 => value.as_i64().map(DynamicValue::S64).ok_or_else(invalid),
            DynamicType::U8 => value
                .as_u64()
                .and_then(|v| u8::try_from(v).ok())
                .map(DynamicValue::U8)
                .ok_or_else(invalid),
            DynamicType::U16 => value
                .as_u64()
                .and_then(|v| u16::try_from(v).ok())
                .map(DynamicValue::U16)
                .ok_or_else(invalid),
            DynamicType::U32 => value
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .map(DynamicValue::U32)
                .ok_or_else(invalid),
            DynamicType::U64 => value.as_u64().map(DynamicValue::U64).ok_or_else(invalid),
            DynamicType::F32 => value
                .as_f64()
                .map(|v| DynamicValue::F32(v as f32))
                .ok_or_else(invalid),
            DynamicType::F64 => value.as_f64().map(DynamicValue::F64).ok_or_else(invalid),
            DynamicType::Char => value
                .as_str()
                .and_then(|v| {
                    let mut chars = v.chars();
                    let c = chars.next()?;
                    chars.next().is_none().then_some(c)
                })
                .map(DynamicValue::Char)
                .ok_or_else(invalid),
            DynamicType::String => value
                .as_str()
                .map(|v| DynamicValue::String(v.to_owned()))
                .ok_or_else(invalid),
            DynamicType::ResourceUri => value
                .as_str()
                .ok_or_else(invalid)
                .and_then(|v| ResourceUri::parse(v).map(DynamicValue::ResourceUri)),
            DynamicType::List(inner) => value
                .as_array()
                .ok_or_else(invalid)?
                .iter()
                .map(|v| json_to_dynamic_typed(v, inner))
                .collect::<Result<_, _>>()
                .map(DynamicValue::List),
            DynamicType::Tuple(types) => {
                let values = value.as_array().ok_or_else(invalid)?;
                if values.len() != types.len() {
                    return Err(invalid());
                }
                values
                    .iter()
                    .zip(types)
                    .map(|(v, ty)| json_to_dynamic_typed(v, ty))
                    .collect::<Result<_, _>>()
                    .map(DynamicValue::Tuple)
            }
            DynamicType::Record(types) => {
                let fields = value.as_object().ok_or_else(invalid)?;
                if fields.keys().any(|name| !types.contains_key(name)) {
                    return Err(invalid());
                }
                types
                    .iter()
                    .map(|(name, ty)| match fields.get(name) {
                        Some(field) => Ok((name.clone(), json_to_dynamic_typed(field, ty)?)),
                        None if matches!(ty, DynamicType::Option(_)) => {
                            Ok((name.clone(), DynamicValue::Option(None)))
                        }
                        None => Err(invalid()),
                    })
                    .collect::<Result<BTreeMap<_, _>, KernelError>>()
                    .map(DynamicValue::Record)
            }
            DynamicType::Option(inner) => {
                if value.is_null() {
                    Ok(DynamicValue::Option(None))
                } else {
                    Ok(DynamicValue::Option(Some(Box::new(json_to_dynamic_typed(
                        value, inner,
                    )?))))
                }
            }
            DynamicType::Result { ok, err } => {
                let fields = value.as_object().ok_or_else(invalid)?;
                if let Some(value) = fields.get("ok") {
                    let ty = ok.as_deref().ok_or_else(invalid)?;
                    Ok(DynamicValue::Result(Ok(Box::new(json_to_dynamic_typed(
                        value, ty,
                    )?))))
                } else if let Some(value) = fields.get("err") {
                    let ty = err.as_deref().ok_or_else(invalid)?;
                    Ok(DynamicValue::Result(Err(Box::new(json_to_dynamic_typed(
                        value, ty,
                    )?))))
                } else {
                    Err(invalid())
                }
            }
            DynamicType::Enum(values) => value
                .as_str()
                .filter(|v| values.iter().any(|candidate| candidate == v))
                .map(|v| DynamicValue::Enum(v.to_owned()))
                .ok_or_else(invalid),
            DynamicType::Variant(cases) => {
                if let Some(name) = value.as_str() {
                    if !cases.contains_key(name) && cases.contains_key("at") {
                        let payload_type = cases
                            .get("at")
                            .and_then(Option::as_ref)
                            .ok_or_else(invalid)?;
                        return Ok(DynamicValue::Variant(
                            "at".to_owned(),
                            Some(Box::new(json_to_dynamic_typed(
                                &Value::String(name.to_owned()),
                                payload_type,
                            )?)),
                        ));
                    }
                    if let Some(payload) = cases.get(name) {
                        return match payload {
                            None => Ok(DynamicValue::Variant(name.to_owned(), None)),
                            Some(payload_type) => Ok(DynamicValue::Variant(
                                name.to_owned(),
                                Some(Box::new(json_to_dynamic_typed(
                                    &Value::String(name.to_owned()),
                                    payload_type,
                                )?)),
                            )),
                        };
                    }
                }
                let fields = value.as_object().ok_or_else(invalid)?;
                let (name, value) = fields.iter().next().ok_or_else(invalid)?;
                let case = cases.get(name).ok_or_else(invalid)?;
                let payload = match (case, value.is_null()) {
                    (None, true) => None,
                    (Some(ty), _) => Some(Box::new(json_to_dynamic_typed(value, ty)?)),
                    _ => return Err(invalid()),
                };
                Ok(DynamicValue::Variant(name.clone(), payload))
            }
            DynamicType::Flags(flags) => {
                let values = value
                    .as_array()
                    .ok_or_else(invalid)?
                    .iter()
                    .map(|v| v.as_str().map(str::to_owned).ok_or_else(invalid))
                    .collect::<Result<Vec<_>, _>>()?;
                if values
                    .iter()
                    .all(|v| flags.iter().any(|candidate| candidate == v))
                {
                    Ok(DynamicValue::Flags(values))
                } else {
                    Err(invalid())
                }
            }
        }
    }

    fn typed_tool_output(
        value: &Value,
        registration: &ToolRegistration,
    ) -> Result<DynamicValue, KernelError> {
        let definition = registration.dynamic_definition()?;
        let output_type =
            definition
                .output_type
                .as_ref()
                .ok_or_else(|| KernelError::InvalidRequest {
                    message: "tool has no output type".into(),
                })?;
        json_to_dynamic_typed(value, output_type)
    }

    pub(crate) fn dynamic_to_json(value: &DynamicValue) -> Value {
        match value {
            DynamicValue::Bool(value) => Value::Bool(*value),
            DynamicValue::S8(value) => serde_json::json!(*value),
            DynamicValue::S16(value) => serde_json::json!(*value),
            DynamicValue::S32(value) => serde_json::json!(*value),
            DynamicValue::S64(value) => serde_json::json!(*value),
            DynamicValue::U8(value) => serde_json::json!(*value),
            DynamicValue::U16(value) => serde_json::json!(*value),
            DynamicValue::U32(value) => serde_json::json!(*value),
            DynamicValue::U64(value) => serde_json::json!(*value),
            DynamicValue::F32(value) => serde_json::json!(*value),
            DynamicValue::F64(value) => serde_json::json!(*value),
            DynamicValue::Char(value) => Value::String(value.to_string()),
            DynamicValue::String(value) | DynamicValue::Enum(value) => Value::String(value.clone()),
            DynamicValue::ResourceUri(value) => Value::String(value.to_string()),
            DynamicValue::List(values) | DynamicValue::Tuple(values) => {
                Value::Array(values.iter().map(dynamic_to_json).collect())
            }
            DynamicValue::Record(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), dynamic_to_json(value)))
                    .collect(),
            ),
            DynamicValue::Option(None) => Value::Null,
            DynamicValue::Option(Some(value)) => dynamic_to_json(value),
            DynamicValue::Result(Ok(value)) | DynamicValue::Result(Err(value)) => {
                dynamic_to_json(value)
            }
            DynamicValue::Variant(name, value) => {
                let mut object = serde_json::Map::new();
                object.insert(
                    name.clone(),
                    value.as_deref().map(dynamic_to_json).unwrap_or(Value::Null),
                );
                Value::Object(object)
            }
            DynamicValue::Flags(values) => {
                Value::Array(values.iter().cloned().map(Value::String).collect())
            }
        }
    }

    pub(crate) fn dynamic_to_json_host(value: DynamicValue) -> Value {
        fn lower(value: DynamicValue, field: Option<&str>) -> Value {
            if field == Some("anchor") {
                if let DynamicValue::List(tokens) = value {
                    let tokens = tokens
                        .into_iter()
                        .filter_map(|token| match token {
                            DynamicValue::String(token) => Some(token),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    return Value::String(format!("#{}", tokens.join(".")));
                }
            }
            match value {
                DynamicValue::Record(fields) => Value::Object(
                    fields
                        .into_iter()
                        .map(|(name, value)| (name.clone(), lower(value, Some(&name))))
                        .collect(),
                ),
                DynamicValue::List(values) | DynamicValue::Tuple(values) => {
                    Value::Array(values.into_iter().map(|value| lower(value, None)).collect())
                }
                DynamicValue::Option(None) => Value::Null,
                DynamicValue::Option(Some(value)) => lower(*value, field),
                DynamicValue::Result(Ok(value)) => serde_json::json!({"ok": lower(*value, field)}),
                DynamicValue::Result(Err(value)) => {
                    serde_json::json!({"err": lower(*value, field)})
                }
                DynamicValue::Variant(name, value) => {
                    let mut object = serde_json::Map::new();
                    object.insert(
                        name,
                        value
                            .map(|value| lower(*value, None))
                            .unwrap_or(Value::Null),
                    );
                    Value::Object(object)
                }
                value => dynamic_to_json(&value),
            }
        }
        lower(value, None)
    }

    pub(crate) fn kernel_error_to_json_host(error: KernelError, uri: &str) -> Value {
        let code = match error {
            KernelError::InvalidUri { .. } => "invalid-uri",
            KernelError::UnsupportedUri { .. }
            | KernelError::NoHandler { .. }
            | KernelError::UnsupportedVerb { .. } => "unsupported",
            KernelError::InvalidRequest { .. } => "invalid-input",
            KernelError::InvalidPattern { .. } => "invalid-pattern",
            KernelError::InvalidAnchor { .. } => "invalid-anchor",
            KernelError::StaleAnchor { .. } => "stale-anchor",
            KernelError::WrongKind { .. } => "wrong-kind",
            KernelError::Immutable { .. } => "immutable",
            KernelError::PermissionDenied { .. } => "permission-denied",
            KernelError::Conflict { .. } => "conflict",
            KernelError::NotEmpty { .. } => "not-empty",
            KernelError::Aborted { .. } => "aborted",
            KernelError::NotFound { .. } => "not-found",
            KernelError::AlreadyExists { .. } => "conflict",
            KernelError::Handler { .. } => "internal",
        };
        serde_json::json!({
            "code": code,
            "uri": uri,
            "message": error.to_string(),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use artist_kernel::{
            FileResourceProvider, FileVerbBindings, Kernel, ReadInput, VerbDefinition, VerbId,
            WriteInput,
        };
        use tempfile::tempdir;

        fn register_file_provider(kernel: &Kernel, root: &std::path::Path) {
            let identity = |function: &str| {
                VerbId::new(format!("artist:filesystem/{function}@1.0.0")).unwrap()
            };
            kernel
                .register_dynamic_resource_provider(Arc::new(FileResourceProvider::new(
                    Arc::new(artist_kernel::FileHandler::new(root).unwrap()),
                    FileVerbBindings {
                        read: identity("read"),
                        write: identity("write"),
                        edit: identity("edit"),
                        insert: identity("insert"),
                        delete: identity("delete"),
                        find: identity("find"),
                        grep: identity("grep"),
                    },
                )))
                .unwrap();
        }

        fn register_tools_provider(kernel: &Kernel, handler: ToolsHandler) {
            let identity =
                |function: &str| VerbId::new(format!("artist:tools/{function}@1.0.0")).unwrap();
            kernel
                .register_dynamic_resource_provider(Arc::new(DynamicToolsProvider::new(
                    Arc::new(handler),
                    ToolsVerbBindings {
                        read: identity("read"),
                        write: identity("write"),
                        edit: identity("edit"),
                        insert: identity("insert"),
                        delete: identity("delete"),
                        find: identity("find"),
                        grep: identity("grep"),
                    },
                )))
                .unwrap();
        }

        #[derive(Clone, Default)]
        struct NamedFakeNamespace {
            resources: Arc<Mutex<std::collections::HashMap<String, String>>>,
        }

        impl DynamicClaimProvider for NamedFakeNamespace {
            fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision {
                if uri.scheme() == "fake"
                    && ["run", "write"].iter().any(|function| {
                        verb == &VerbId::new(format!("artist:filesystem/{function}@1.0.0")).unwrap()
                    })
                {
                    ClaimDecision::Handle
                } else {
                    ClaimDecision::Pass
                }
            }
        }

        impl DynamicResourceProvider for NamedFakeNamespace {
            fn invoke<'a>(
                &'a self,
                verb: &'a VerbId,
                uri: &'a ResourceUri,
                input: DynamicValue,
            ) -> ResourceFuture<'a> {
                let resources = Arc::clone(&self.resources);
                Box::pin(async move {
                    let mut resources = resources.lock().unwrap();
                    let function = verb.function();
                    if function == "run" {
                        let key = uri.to_string();
                        if resources.contains_key(&key) {
                            return Err(KernelError::Conflict { uri: key });
                        }
                        resources.insert(key, String::new());
                    } else if function == "write" {
                        let content = match input {
                            DynamicValue::Record(fields) => match fields.get("content") {
                                Some(DynamicValue::String(content)) => content.clone(),
                                _ => String::new(),
                            },
                            _ => String::new(),
                        };
                        resources
                            .entry(uri.to_string())
                            .or_default()
                            .push_str(&content);
                    }
                    Ok(DynamicVerbResult {
                        verb: verb.clone(),
                        function: function.to_owned(),
                        output: DynamicValue::ResourceUri(uri.clone()),
                    })
                })
            }
        }

        #[tokio::test]
        async fn registers_extension_contracts_in_the_dynamic_tool_catalog() {
            let root = tempdir().unwrap();
            let package = root.path().join("formatter");
            std::fs::create_dir_all(package.join("src")).unwrap();
            std::fs::write(
                package.join("tool.md"),
                "---\nname: formatter\ndescription: Format source\nversion: 0.1.0\ncontract: acme:format@2\ncapabilities: []\n---\n",
            )
            .unwrap();
            let handler = ToolsHandler::new(root.path(), Vec::<String>::new()).unwrap();
            let registrations = handler.registrations().unwrap();
            assert_eq!(registrations.len(), 1);
            assert_eq!(registrations[0].contract.to_string(), "acme:format@2");
            assert_eq!(registrations[0].input_schema, None);
        }

        #[tokio::test]
        async fn dispatches_a_typed_verb_directly_from_the_tools_namespace() {
            let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs");
            let kernel = Kernel::new();
            register_tools_provider(
                &kernel,
                ToolsHandler::new(&source_root, ["resource.read".to_owned()]).unwrap(),
            );
            let result = kernel
                .invoke_dynamic_resource(
                    VerbId::new("artist:tools/read@1.0.0").unwrap(),
                    ResourceUri::parse("tools://read/tool.md").unwrap(),
                    DynamicValue::Record(std::collections::BTreeMap::new()),
                )
                .await
                .unwrap();
            assert!(format!("{:?}", result.output).contains("contract: artist:tool:read@1"));
        }

        #[tokio::test]
        async fn typed_tools_search_maps_virtual_uris_without_leaking_file_uris() {
            let root = tempdir().unwrap();
            std::fs::create_dir_all(root.path().join("pkg")).unwrap();
            std::fs::write(root.path().join("pkg/note.txt"), "virtual needle\n").unwrap();
            let handler = ToolsHandler::new(root.path(), Vec::<String>::new()).unwrap();
            let kernel = Kernel::new();
            register_tools_provider(&kernel, handler);

            let found = kernel
                .invoke_dynamic_resource(
                    VerbId::new("artist:tools/find@1.0.0").unwrap(),
                    ResourceUri::parse("tools:///").unwrap(),
                    DynamicValue::Record(std::collections::BTreeMap::from([(
                        "query".to_owned(),
                        DynamicValue::String("note".to_owned()),
                    )])),
                )
                .await
                .unwrap();
            let DynamicValue::Record(fields) = found.output else {
                panic!("unexpected dynamic find result: {found:?}");
            };
            let Some(DynamicValue::List(paths)) = fields.get("uris") else {
                panic!("dynamic find result has no uris: {fields:?}");
            };
            assert_eq!(
                *paths,
                vec![DynamicValue::ResourceUri(
                    ResourceUri::parse("tools:///pkg/note.txt").unwrap(),
                )]
            );

            let grep = kernel
                .invoke_dynamic_resource(
                    VerbId::new("artist:tools/grep@1.0.0").unwrap(),
                    ResourceUri::parse("tools:///pkg/note.txt").unwrap(),
                    DynamicValue::Record(std::collections::BTreeMap::from([(
                        "pattern".to_owned(),
                        DynamicValue::String("needle".to_owned()),
                    )])),
                )
                .await
                .unwrap();
            let DynamicValue::Record(fields) = grep.output else {
                panic!("unexpected dynamic grep result: {grep:?}");
            };
            let Some(DynamicValue::List(matches)) = fields.get("matches") else {
                panic!("dynamic grep result has no matches: {fields:?}");
            };
            assert!(format!("{:?}", matches[0]).contains("virtual needle"));
        }

        #[tokio::test]
        async fn exposes_named_tools_and_dispatches_against_an_ordinary_file() {
            let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs");
            let files_root = tempdir().unwrap();
            let source = files_root.path().join("named.txt");
            std::fs::write(&source, "named tool dispatch\n").unwrap();

            let kernel = Kernel::new();
            register_file_provider(&kernel, files_root.path());
            kernel
                .register_tool_provider(
                    ToolsHandler::new(&source_root, ["resource.read".to_owned()]).unwrap(),
                )
                .await;

            let definitions = kernel.tool_definitions().await;
            let read = definitions
                .iter()
                .find(|definition| definition.name == "read")
                .unwrap();
            assert_eq!(read.description, "WASM read verb component");

            let result = kernel
                .execute_tool(
                    "read",
                    json_to_dynamic(serde_json::json!({
                        "uri": format!("file://{}", source.display()),
                        "at": null,
                        "before": null,
                        "after": null,
                    }))
                    .unwrap(),
                )
                .await
                .unwrap();
            assert!(
                dynamic_to_json(&result)
                    .to_string()
                    .contains("named tool dispatch")
            );
        }

        #[tokio::test]
        async fn malformed_edit_keeps_active_named_generation_executable() {
            let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs/read");
            let authored = ToolPackage::discover(&source).unwrap();
            let mut build_options = BuildOptions::default();
            build_options.granted_capabilities = vec!["resource.read".to_owned()];
            let artifact = authored.build(&build_options).unwrap().artifact;

            let tools_root = tempdir().unwrap();
            let package_root = tools_root.path().join("read");
            std::fs::create_dir_all(&package_root).unwrap();
            std::fs::copy(&artifact, package_root.join("tool.wasm")).unwrap();
            std::fs::copy(source.join("tool.md"), package_root.join("tool.md")).unwrap();
            std::fs::copy(source.join("tool.wit"), package_root.join("tool.wit")).unwrap();
            std::fs::create_dir_all(package_root.join("deps/resource")).unwrap();
            std::fs::copy(
                source.join("deps/resource/world.wit"),
                package_root.join("deps/resource/world.wit"),
            )
            .unwrap();

            let files_root = tempdir().unwrap();
            let target = files_root.path().join("target.txt");
            std::fs::write(&target, "generation one\n").unwrap();
            let kernel = Kernel::new();
            register_file_provider(&kernel, files_root.path());
            let tools = ToolsHandler::new(tools_root.path(), ["resource.read".to_owned()]).unwrap();
            let observed_tools = tools.clone();
            kernel.register_tool_provider(tools).await;

            let input = json_to_dynamic(serde_json::json!({
                "uri": format!("file://{}", target.display()),
                "at": null,
                "before": null,
                "after": null,
            }))
            .unwrap();
            let first = kernel.execute_tool("read", input.clone()).await.unwrap();
            assert!(
                dynamic_to_json(&first)
                    .to_string()
                    .contains("generation one")
            );
            let bindings = ToolsVerbBindings {
                read: VerbId::new("artist:tools/read@1.0.0").unwrap(),
                write: VerbId::new("artist:tools/write@1.0.0").unwrap(),
                edit: VerbId::new("artist:tools/edit@1.0.0").unwrap(),
                insert: VerbId::new("artist:tools/insert@1.0.0").unwrap(),
                delete: VerbId::new("artist:tools/delete@1.0.0").unwrap(),
                find: VerbId::new("artist:tools/find@1.0.0").unwrap(),
                grep: VerbId::new("artist:tools/grep@1.0.0").unwrap(),
            };
            DynamicToolsProvider::new(Arc::new(observed_tools.clone()), bindings.clone())
                .invoke(
                    &bindings.write,
                    &ResourceUri::parse("tools://read/tool.md").unwrap(),
                    DynamicValue::Record(std::collections::BTreeMap::from([(
                        "content".to_owned(),
                        DynamicValue::String("not valid frontmatter".to_owned()),
                    )])),
                )
                .await
                .unwrap();
            assert_eq!(
                std::fs::read_to_string(package_root.join("tool.md")).unwrap(),
                "not valid frontmatter"
            );
            let second = kernel.execute_tool("read", input).await.unwrap();
            assert!(
                dynamic_to_json(&second)
                    .to_string()
                    .contains("generation one")
            );

            // A later valid self-edit must use the same handler allocation and
            // activate a new generation on the named-tool path.  Checking the
            // catalog as well as executing the tool catches the split-brain
            // failure where typed writes dirty one ToolsHandler while named
            // execution consults another.
            let repaired = "---\nname: artist-tool-read\ndescription: generation two\nversion: 0.1.1\ncontract: artist:tool:read@1\ncapabilities:\n  - resource.read\n---\n";
            DynamicToolsProvider::new(Arc::new(observed_tools.clone()), bindings)
                .invoke(
                    &VerbId::new("artist:tools/write@1.0.0").unwrap(),
                    &ResourceUri::parse("tools://read/tool.md").unwrap(),
                    DynamicValue::Record(std::collections::BTreeMap::from([(
                        "content".to_owned(),
                        DynamicValue::String(repaired.to_owned()),
                    )])),
                )
                .await
                .unwrap();
            let definitions = kernel.tool_definitions().await;
            assert!(
                definitions
                    .iter()
                    .any(|definition| definition.name == "read"
                        && definition.description == "generation two")
            );
            let third = kernel
                .execute_tool(
                    "read",
                    json_to_dynamic(serde_json::json!({
                        "uri": format!("file://{}", target.display()),
                        "at": null,
                        "before": null,
                        "after": null,
                    }))
                    .unwrap(),
                )
                .await
                .unwrap();
            assert!(
                dynamic_to_json(&third)
                    .to_string()
                    .contains("generation one")
            );
        }

        #[tokio::test]
        async fn named_wasm_run_and_write_reach_a_scheme_claiming_namespace() {
            let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs");
            let namespace = NamedFakeNamespace::default();
            let kernel = Kernel::new();
            kernel
                .register_dynamic_resource_provider(Arc::new(namespace.clone()))
                .unwrap();
            kernel
                .register_tool_provider(
                    ToolsHandler::new(
                        &source_root,
                        ["resource.run".to_owned(), "resource.write".to_owned()],
                    )
                    .unwrap(),
                )
                .await;

            let uri = "fake://named-shell";
            let run = kernel
                .execute_tool(
                    "run",
                    json_to_dynamic(serde_json::json!({"requests":[{"uri":uri,"args":[]}]}))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(dynamic_to_json(&run).to_string().contains(uri));
            let write = kernel
                .execute_tool(
                    "write",
                    json_to_dynamic(serde_json::json!({"uri":uri,"content":"cargo test\n"}))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(dynamic_to_json(&write).to_string().contains(uri));
            assert!(namespace.resources.lock().unwrap().get(uri).is_none());
        }
    }
}

/// Bootstrap support for the self-describing `resources://` package namespace.
/// Package discovery/cataloging is deliberately independent of claim routing;
/// this keeps documentation and package-file inspection available before any
/// resource component is activated.
pub mod resources {
    use artist_kernel::{
        BoxFuture, ClaimDecision, DynamicClaimProvider, DynamicResourceProvider, DynamicValue,
        DynamicVerbResult, FileHandler, FileResourceProvider, FileVerbBindings, KernelError,
        KernelHandle, ResourceAddress, ResourceCatalogDoc, ResourceCatalogEntry,
        ResourceCatalogProvider, ResourceFuture, ResourceUri, VerbId,
    };
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };
    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub struct ResourceRoute {
        pub schemes: Vec<String>,
    }

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub struct QueryDoc {
        pub name: String,
        pub summary: String,
    }

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub struct ResourceDoc {
        pub uri: String,
        pub summary: String,
        #[serde(default)]
        pub verbs: Vec<String>,
        #[serde(default)]
        pub query: Vec<QueryDoc>,
    }

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
    pub struct ResourceFrontmatter {
        pub name: String,
        pub description: String,
        pub version: String,
        pub contract: String,
        #[serde(default)]
        pub routes: Vec<ResourceRoute>,
        #[serde(default)]
        pub exports: Vec<String>,
        #[serde(default)]
        pub capabilities: Vec<String>,
        #[serde(default)]
        pub docs: Vec<ResourceDoc>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ResourcePackage {
        pub root: PathBuf,
        pub manifest: ResourceFrontmatter,
        pub prose: String,
        pub wasm: Option<PathBuf>,
        pub source: Option<PathBuf>,
        pub build_manifest: Option<PathBuf>,
        pub wit: Option<PathBuf>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ResourceBuildResult {
        pub artifact: PathBuf,
        pub provenance: PathBuf,
        pub fingerprint: String,
        pub cached: bool,
    }

    struct ResourceComponentHost {
        engine: wasmtime::Engine,
        component: wasmtime::component::Component,
        capabilities: Vec<String>,
        dependencies: Vec<Arc<super::DynamicDependency>>,
    }

    fn wasm_invocation_failure(
        scope: &artist_kernel::InvocationScope,
        operation: impl std::fmt::Display,
        error: impl std::fmt::Display,
    ) -> KernelError {
        if scope.cancellation.is_cancelled() {
            return KernelError::Aborted {
                message: operation.to_string(),
            };
        }
        KernelError::Handler {
            message: format!("{operation}: {error}"),
        }
    }

    impl ResourceComponentHost {
        fn new(
            bytes: &[u8],
            capabilities: Vec<String>,
            dependencies: Vec<super::DependencySpec>,
        ) -> Result<Self, KernelError> {
            let typed = super::TypedComponentHost::new_with_dependency_specs(
                bytes,
                capabilities.clone(),
                dependencies,
            )
            .map_err(|error| KernelError::Handler {
                message: format!("load resource component: {error}"),
            })?;
            Ok(Self {
                engine: typed.engine,
                component: typed.component,
                capabilities,
                dependencies: typed.dependencies,
            })
        }

        fn link_custom_imports(
            &self,
            linker: &mut wasmtime::component::Linker<super::HostState>,
        ) -> Result<(), KernelError> {
            for (import_name, _) in self.component.component_type().imports(&self.engine) {
                if import_name.starts_with("wasi:")
                    || import_name.contains("artist:%resource/")
                    || import_name.contains("artist:resource/")
                {
                    continue;
                }
                let dependency = self.dependencies.iter().find(|dependency| {
                    super::contract_matches_import(&dependency.contract, import_name)
                        || dependency
                            .component
                            .component_type()
                            .exports(&dependency.engine)
                            .any(|(_, export)| export.is_implements(import_name))
                });
                let Some(dependency) = dependency else {
                    return Err(KernelError::InvalidRequest {
                        message: format!(
                            "no active custom resource dependency satisfies {import_name}"
                        ),
                    });
                };
                let mut instance =
                    linker
                        .instance(import_name)
                        .map_err(|error| KernelError::Handler {
                            message: format!(
                                "create resource dependency instance {import_name}: {error}"
                            ),
                        })?;
                super::define_dependency_exports_async(&mut instance, Arc::clone(dependency))
                    .map_err(|error| KernelError::Handler {
                        message: format!("link resource dependency {import_name}: {error}"),
                    })?;
            }
            Ok(())
        }

        /// Invoke an exported resource interface through the Component Model
        /// dynamically. Resource packages are identified by their WIT
        /// interface/function contract; no generated Rust world or installed
        /// verb enum is needed at this boundary.
        async fn invoke_contract(
            &self,
            verb: &VerbId,
            function: &str,
            input: DynamicValue,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
            capabilities: &[String],
        ) -> Result<DynamicValue, KernelError> {
            let failure_scope = scope.clone();
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_kernel_scope(capabilities.to_vec(), kernel, scope),
            );
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link dynamic resource WASI imports: {error}"),
                }
            })?;
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance = wasmtime::component::Linker::instantiate_async(
                &linker,
                &mut store,
                &self.component,
            )
            .await
            .map_err(|error| {
                wasm_invocation_failure(
                    &failure_scope,
                    format!("dynamic resource {verb} instantiation failed"),
                    error,
                )
            })?;

            let extension_contract = verb.as_str().starts_with("artist:resource/extension@");
            let interface = if extension_contract {
                "extension"
            } else {
                verb.function()
            };
            let interface_name = if extension_contract {
                format!("artist:resource/extension@{}", verb.version())
            } else {
                // Resource packages expose executable projections through the
                // canonical nested resource interface; the routing verb may
                // use the resource provider's own contract namespace.
                format!("artist:resource/{}@{}", interface, verb.version())
            };
            let function = if extension_contract {
                "claim"
            } else {
                function
            };
            let interface_index = instance
                .get_export_index(&mut store, None, interface_name.as_str())
                .or_else(|| {
                    instance.get_export_index(
                        &mut store,
                        None,
                        interface_name
                            .split('@')
                            .next()
                            .unwrap_or(interface_name.as_str()),
                    )
                })
                .or_else(|| instance.get_export_index(&mut store, None, interface))
                .or_else(|| {
                    let escaped = interface_name.replace("artist:resource/", "artist:%resource/");
                    instance
                        .get_export_index(&mut store, None, escaped.as_str())
                        .or_else(|| {
                            instance.get_export_index(
                                &mut store,
                                None,
                                escaped.split('@').next().unwrap_or(escaped.as_str()),
                            )
                        })
                })
                .or_else(|| instance.get_export_index(&mut store, None, interface))
                .ok_or_else(|| KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: "<resource-component>".to_owned(),
                })?;
            let function_index = instance
                .get_export_index(&mut store, Some(&interface_index), function)
                .ok_or_else(|| KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: format!("<resource-function:{interface_name}/{function}>").to_owned(),
                })?;
            let export_function =
                instance
                    .get_func(&mut store, &function_index)
                    .ok_or_else(|| KernelError::Handler {
                        message: format!(
                            "resource export {interface}/{function} is not a function"
                        ),
                    })?;
            let function_type = export_function.ty(&store);
            if function_type.params().len() != 1 || function_type.results().len() != 1 {
                return Err(KernelError::InvalidRequest {
                    message: format!(
                        "resource export {interface}/{function} must have one parameter and one result"
                    ),
                });
            }
            let parameter = super::dynamic_value_to_component_val_with_type(
                &input,
                &function_type
                    .params()
                    .next()
                    .expect("checked parameter arity")
                    .1,
            )
            .map_err(|error| KernelError::InvalidRequest {
                message: format!("lower resource {interface}/{function} input: {error}"),
            })?;
            let mut results = function_type
                .results()
                .map(|_| wasmtime::component::Val::Bool(false))
                .collect::<Vec<_>>();
            export_function
                .call_async(&mut store, &[parameter], &mut results)
                .await
                .map_err(|error| {
                    wasm_invocation_failure(
                        &failure_scope,
                        format!("resource export {interface}/{function} failed"),
                        error,
                    )
                })?;
            super::component_val_to_dynamic_value(&results[0]).map_err(|error| {
                KernelError::Handler {
                    message: format!("lift resource {interface}/{function} result: {error}"),
                }
            })
        }

        async fn invoke_dynamic(
            &self,
            verb: &VerbId,
            input: DynamicValue,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
            capabilities: &[String],
        ) -> Result<DynamicValue, KernelError> {
            self.invoke_contract(verb, verb.function(), input, kernel, scope, capabilities)
                .await
        }

        fn link_nested_standard_imports(
            linker: &mut wasmtime::component::Linker<super::HostState>,
        ) -> Result<(), KernelError> {
            let mut filesystem = linker
                .instance("artist:resource/filesystem@1.0.0")
                .map_err(|error| KernelError::Handler {
                    message: format!("create filesystem host instance: {error}"),
                })?;
            filesystem
                .func_wrap(
                    "is-file",
                    |_caller: wasmtime::StoreContextMut<'_, super::HostState>,
                     (uri,): (String,)| {
                        Ok::<_, wasmtime::Error>((artist_kernel::ResourceUri::parse(&uri)
                            .ok()
                            .and_then(|uri| uri.as_ref().to_file_path().ok())
                            .is_some_and(|path| {
                                std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
                            }),))
                    },
                )
                .map_err(|error| KernelError::Handler {
                    message: format!("link filesystem host instance: {error}"),
                })?;

            let mut read = linker
                .instance("artist:resource/read@1.0.0")
                .map_err(|error| KernelError::Handler {
                    message: format!("create nested read host instance: {error}"),
                })?;
            read.func_new_async("read", |caller, _func, params, results| {
                let input = params
                    .first()
                    .ok_or_else(|| wasmtime::Error::msg("nested read received no request list"))
                    .and_then(|value| {
                        super::component_val_to_dynamic_value(&value).map_err(wasmtime::Error::msg)
                    });
                let (kernel, scope, input) = match input {
                    Ok(input) => {
                        let state = caller.data();
                        let Some(kernel) = state.kernel.clone() else {
                            return Box::new(async {
                                Err(wasmtime::Error::msg("nested read has no kernel host"))
                            });
                        };
                        (kernel, state.scope.clone(), input)
                    }
                    Err(error) => return Box::new(async move { Err(wasmtime::Error::msg(error)) }),
                };
                Box::new(async move {
                    let output = forward_nested_read(kernel, scope, input)
                        .await
                        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                    let value =
                        super::dynamic_value_to_component_val(&nested_component_value(output))
                            .map_err(wasmtime::Error::msg)?;
                    let result = results
                        .first_mut()
                        .ok_or_else(|| wasmtime::Error::msg("nested read has no result slot"))?;
                    *result = value;
                    Ok(())
                })
            })
            .map_err(|error| KernelError::Handler {
                message: format!("link nested read host instance: {error}"),
            })?;
            let mut write = linker
                .instance("artist:resource/write@1.0.0")
                .map_err(|error| KernelError::Handler {
                    message: format!("create nested write host instance: {error}"),
                })?;
            write
                .func_new_async("write", |caller, _func, params, results| {
                    let input = params
                        .first()
                        .ok_or_else(|| {
                            wasmtime::Error::msg("nested write received no request list")
                        })
                        .and_then(|value| {
                            super::component_val_to_dynamic_value(&value)
                                .map_err(wasmtime::Error::msg)
                        });
                    let (kernel, scope, input) = match input {
                        Ok(input) => {
                            let state = caller.data();
                            let Some(kernel) = state.kernel.clone() else {
                                return Box::new(async {
                                    Err(wasmtime::Error::msg("nested write has no kernel host"))
                                });
                            };
                            (kernel, state.scope.clone(), input)
                        }
                        Err(error) => {
                            return Box::new(async move { Err(wasmtime::Error::msg(error)) });
                        }
                    };
                    Box::new(async move {
                        let output = forward_nested_write(kernel, scope, input)
                            .await
                            .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                        let value =
                            super::dynamic_value_to_component_val(&nested_component_value(output))
                                .map_err(wasmtime::Error::msg)?;
                        let result = results.first_mut().ok_or_else(|| {
                            wasmtime::Error::msg("nested write has no result slot")
                        })?;
                        *result = value;
                        Ok(())
                    })
                })
                .map_err(|error| KernelError::Handler {
                    message: format!("link nested write host instance: {error}"),
                })?;

            let mut poll = linker
                .instance("artist:resource/poll@1.0.0")
                .map_err(|error| KernelError::Handler {
                    message: format!("create nested poll host instance: {error}"),
                })?;
            poll.func_new_async("poll", |caller, _func, params, results| {
                let input = params
                    .first()
                    .ok_or_else(|| wasmtime::Error::msg("nested poll received no request list"))
                    .and_then(|value| {
                        super::component_val_to_dynamic_value(&value).map_err(wasmtime::Error::msg)
                    });
                let (kernel, scope, input) = match input {
                    Ok(input) => {
                        let state = caller.data();
                        let Some(kernel) = state.kernel.clone() else {
                            return Box::new(async {
                                Err(wasmtime::Error::msg("nested poll has no kernel host"))
                            });
                        };
                        (kernel, state.scope.clone(), input)
                    }
                    Err(error) => return Box::new(async move { Err(wasmtime::Error::msg(error)) }),
                };
                Box::new(async move {
                    let output = forward_nested_poll(kernel, scope, input)
                        .await
                        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                    let value =
                        super::dynamic_value_to_component_val(&nested_component_value(output))
                            .map_err(wasmtime::Error::msg)?;
                    let result = results
                        .first_mut()
                        .ok_or_else(|| wasmtime::Error::msg("nested poll has no result slot"))?;
                    *result = value;
                    Ok(())
                })
            })
            .map_err(|error| KernelError::Handler {
                message: format!("link nested poll host instance: {error}"),
            })?;
            Ok(())
        }
    }

    async fn forward_nested_read(
        kernel: KernelHandle,
        scope: artist_kernel::InvocationScope,
        input: DynamicValue,
    ) -> Result<DynamicValue, KernelError> {
        let DynamicValue::List(requests) = input else {
            return Err(KernelError::InvalidRequest {
                message: "nested read expects a request list".to_owned(),
            });
        };
        let verb = VerbId::new("artist:filesystem/read@1.0.0")
            .map_err(|message| KernelError::InvalidRequest { message })?;
        let mut results = vec![None; requests.len()];
        let mut valid = Vec::new();
        for (index, request) in requests.into_iter().enumerate() {
            let DynamicValue::Record(fields) = &request else {
                results[index] = Some(DynamicValue::Result(Err(Box::new(nested_resource_error(
                    "nested read request is not a record",
                    None,
                )))));
                continue;
            };
            let Some(DynamicValue::String(uri)) = fields.get("uri") else {
                results[index] = Some(DynamicValue::Result(Err(Box::new(nested_resource_error(
                    "nested read request has no uri",
                    None,
                )))));
                continue;
            };
            let parsed = match ResourceUri::parse(uri) {
                Ok(uri) => uri,
                Err(_) => {
                    results[index] = Some(DynamicValue::Result(Err(Box::new(
                        nested_resource_error("nested read request has an invalid uri", Some(uri)),
                    ))));
                    continue;
                }
            };
            valid.push((
                index,
                uri.to_owned(),
                artist_kernel::ResourceRequest {
                    uri: parsed,
                    input: request,
                },
            ));
        }
        let batch = kernel
            .invoke_dynamic_resource_batch_with_scope(
                verb,
                valid
                    .iter()
                    .map(|(_, _, request)| request.clone())
                    .collect(),
                scope,
            )
            .await;
        for ((index, uri, _), result) in valid.into_iter().zip(batch) {
            results[index] = Some(match result {
                Ok(result) => {
                    let response = match result.output {
                        DynamicValue::Record(fields) if fields.contains_key("lines") => {
                            DynamicValue::Variant(
                                "text".to_owned(),
                                Some(Box::new(DynamicValue::Record(fields))),
                            )
                        }
                        other => other,
                    };
                    DynamicValue::Result(Ok(Box::new(response)))
                }
                Err(error) => DynamicValue::Result(Err(Box::new(nested_resource_error(
                    &error.to_string(),
                    Some(&uri),
                )))),
            });
        }
        Ok(DynamicValue::List(
            results.into_iter().map(Option::unwrap).collect(),
        ))
    }

    fn nested_component_value(value: DynamicValue) -> DynamicValue {
        match value {
            DynamicValue::ResourceUri(uri) => DynamicValue::String(uri.to_string()),
            DynamicValue::List(values) => {
                DynamicValue::List(values.into_iter().map(nested_component_value).collect())
            }
            DynamicValue::Tuple(values) => {
                DynamicValue::Tuple(values.into_iter().map(nested_component_value).collect())
            }
            DynamicValue::Record(fields) => DynamicValue::Record(
                fields
                    .into_iter()
                    .map(|(name, value)| {
                        let value = match (name.as_str(), value) {
                            ("anchor", DynamicValue::List(tokens)) => {
                                let tokens = tokens
                                    .into_iter()
                                    .filter_map(|token| match token {
                                        DynamicValue::String(token) => Some(token),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>();
                                DynamicValue::String(format!("#{}", tokens.join(".")))
                            }
                            ("ending", DynamicValue::String(ending)) => DynamicValue::Enum(ending),
                            (_, value) => nested_component_value(value),
                        };
                        (name, value)
                    })
                    .collect(),
            ),
            DynamicValue::Option(value) => {
                DynamicValue::Option(value.map(|value| Box::new(nested_component_value(*value))))
            }
            DynamicValue::Variant(name, value) => DynamicValue::Variant(
                name,
                value.map(|value| Box::new(nested_component_value(*value))),
            ),
            DynamicValue::Result(Ok(value)) => {
                DynamicValue::Result(Ok(Box::new(nested_component_value(*value))))
            }
            DynamicValue::Result(Err(value)) => {
                DynamicValue::Result(Err(Box::new(nested_component_value(*value))))
            }
            other => other,
        }
    }

    async fn forward_nested_write(
        kernel: KernelHandle,
        scope: artist_kernel::InvocationScope,
        input: DynamicValue,
    ) -> Result<DynamicValue, KernelError> {
        let DynamicValue::List(requests) = input else {
            return Err(KernelError::InvalidRequest {
                message: "nested write expects a request list".to_owned(),
            });
        };
        let verb = VerbId::new("artist:filesystem/write@1.0.0")
            .map_err(|message| KernelError::InvalidRequest { message })?;
        let mut results = vec![None; requests.len()];
        let mut valid = Vec::new();
        for (index, request) in requests.into_iter().enumerate() {
            let fields = match &request {
                DynamicValue::Record(fields) => fields,
                _ => {
                    results[index] = Some(DynamicValue::Result(Err(Box::new(
                        nested_resource_error("nested write request is not a record", None),
                    ))));
                    continue;
                }
            };
            let Some(DynamicValue::String(uri)) = fields.get("uri") else {
                results[index] = Some(DynamicValue::Result(Err(Box::new(nested_resource_error(
                    "nested write request has no uri",
                    None,
                )))));
                continue;
            };
            let parsed = match ResourceUri::parse(uri) {
                Ok(uri) => uri,
                Err(_) => {
                    results[index] = Some(DynamicValue::Result(Err(Box::new(
                        nested_resource_error("nested write request has an invalid uri", Some(uri)),
                    ))));
                    continue;
                }
            };
            valid.push((
                index,
                uri.to_owned(),
                artist_kernel::ResourceRequest {
                    uri: parsed,
                    input: request,
                },
            ));
        }
        let batch = kernel
            .invoke_dynamic_resource_batch_with_scope(
                verb,
                valid
                    .iter()
                    .map(|(_, _, request)| request.clone())
                    .collect(),
                scope,
            )
            .await;
        for ((index, uri, _), result) in valid.into_iter().zip(batch) {
            results[index] = Some(match result {
                Ok(result) => DynamicValue::Result(Ok(Box::new(result.output))),
                Err(error) => DynamicValue::Result(Err(Box::new(nested_resource_error(
                    &error.to_string(),
                    Some(&uri),
                )))),
            });
        }
        Ok(DynamicValue::List(
            results.into_iter().map(Option::unwrap).collect(),
        ))
    }

    async fn forward_nested_poll(
        kernel: KernelHandle,
        scope: artist_kernel::InvocationScope,
        input: DynamicValue,
    ) -> Result<DynamicValue, KernelError> {
        let DynamicValue::List(requests) = input else {
            return Err(KernelError::InvalidRequest {
                message: "nested poll expects a request list".to_owned(),
            });
        };
        let process_verb = VerbId::new("artist:process/poll@1.0.0")
            .map_err(|message| KernelError::InvalidRequest { message })?;
        let session_verb = VerbId::new("artist:session/poll@1.0.0")
            .map_err(|message| KernelError::InvalidRequest { message })?;
        let mut results = vec![None; requests.len()];
        let mut valid = Vec::new();
        for (index, request) in requests.into_iter().enumerate() {
            let Some(DynamicValue::String(uri)) = (match &request {
                DynamicValue::Record(fields) => fields.get("uri"),
                _ => None,
            }) else {
                results[index] = Some(DynamicValue::Result(Err(Box::new(nested_resource_error(
                    "nested poll request has no uri",
                    None,
                )))));
                continue;
            };
            let parsed = match ResourceUri::parse(uri) {
                Ok(uri) => uri,
                Err(_) => {
                    results[index] = Some(DynamicValue::Result(Err(Box::new(
                        nested_resource_error("nested poll request has an invalid uri", Some(uri)),
                    ))));
                    continue;
                }
            };
            valid.push((
                index,
                uri.to_owned(),
                artist_kernel::ResourceRequest {
                    uri: parsed,
                    input: request,
                },
            ));
        }

        let process_results = kernel
            .invoke_dynamic_resource_batch_with_scope(
                process_verb,
                valid
                    .iter()
                    .map(|(_, _, request)| request.clone())
                    .collect(),
                scope.clone(),
            )
            .await;
        let mut fallback = Vec::new();
        for position in 0..valid.len() {
            let result = process_results.get(position).cloned().unwrap_or_else(|| {
                Err(KernelError::Handler {
                    message: "nested poll provider returned the wrong batch length".to_owned(),
                })
            });
            if result.is_err() {
                fallback.push((position, valid[position].2.clone()));
            } else {
                let (index, uri, _) = &valid[position];
                results[*index] = Some(match result {
                    Ok(result) => DynamicValue::Result(Ok(Box::new(
                        normalize_nested_poll_response(result.output, uri),
                    ))),
                    Err(_) => unreachable!("checked above"),
                });
            }
        }

        let session_results = kernel
            .invoke_dynamic_resource_batch_with_scope(
                session_verb,
                fallback
                    .iter()
                    .map(|(_, request)| request.clone())
                    .collect(),
                scope,
            )
            .await;
        for ((position, _), result) in fallback.into_iter().zip(session_results) {
            let (index, uri, _) = &valid[position];
            results[*index] = Some(match result {
                Ok(result) => DynamicValue::Result(Ok(Box::new(normalize_nested_poll_response(
                    result.output,
                    uri,
                )))),
                Err(error) => DynamicValue::Result(Err(Box::new(nested_resource_error(
                    &error.to_string(),
                    Some(uri),
                )))),
            });
        }
        Ok(DynamicValue::List(
            results.into_iter().map(Option::unwrap).collect(),
        ))
    }

    fn normalize_nested_poll_response(value: DynamicValue, uri: &str) -> DynamicValue {
        match value {
            DynamicValue::Record(mut fields) if fields.contains_key("text") => {
                fields
                    .entry("uri".to_owned())
                    .or_insert_with(|| DynamicValue::String(uri.to_owned()));
                fields
                    .entry("reason".to_owned())
                    .or_insert_with(|| DynamicValue::Enum("changed".to_owned()));
                DynamicValue::Record(fields)
            }
            DynamicValue::Variant(_, _) => DynamicValue::Record(BTreeMap::from([
                ("uri".to_owned(), DynamicValue::String(uri.to_owned())),
                (
                    "text".to_owned(),
                    DynamicValue::Record(BTreeMap::from([
                        ("uri".to_owned(), DynamicValue::String(uri.to_owned())),
                        ("lines".to_owned(), DynamicValue::List(Vec::new())),
                    ])),
                ),
                (
                    "reason".to_owned(),
                    DynamicValue::Enum("changed".to_owned()),
                ),
            ])),
            _ => value,
        }
    }

    fn nested_resource_error(message: &str, uri: Option<&str>) -> DynamicValue {
        DynamicValue::Record(BTreeMap::from([
            ("code".to_owned(), DynamicValue::Enum("internal".to_owned())),
            (
                "uri".to_owned(),
                DynamicValue::Option(
                    uri.map(|value| Box::new(DynamicValue::String(value.to_owned()))),
                ),
            ),
            (
                "message".to_owned(),
                DynamicValue::String(message.to_owned()),
            ),
        ]))
    }

    impl ResourceComponentHost {
        async fn claim(
            &self,
            verb: &VerbId,
            uri: &ResourceUri,
        ) -> Result<artist_kernel::ClaimDecision, KernelError> {
            let claim_context = artist_kernel::InvocationContext {
                deadline_ms: Some(50),
                ..Default::default()
            };
            let input = DynamicValue::Record(BTreeMap::from([
                // Older package-local extension WITs model the verb identity
                // as an enum; its stable case is the interface name.
                (
                    "verb".to_owned(),
                    DynamicValue::Enum(verb.function().to_owned()),
                ),
                ("uri".to_owned(), DynamicValue::String(uri.to_string())),
            ]));
            let decision = self
                .invoke_contract(
                    &VerbId::new("artist:resource/extension@1.0.0")
                        .map_err(|message| KernelError::InvalidRequest { message })?,
                    "claim",
                    input,
                    KernelHandle::detached(),
                    artist_kernel::InvocationScope::new(claim_context),
                    &[],
                )
                .await?;
            match decision {
                DynamicValue::Enum(name) | DynamicValue::Variant(name, None) => {
                    match name.as_str() {
                        "pass" => Ok(artist_kernel::ClaimDecision::Pass),
                        "handle" => Ok(artist_kernel::ClaimDecision::Handle),
                        "reserve" => Ok(artist_kernel::ClaimDecision::Reserve),
                        _ => Err(KernelError::InvalidRequest {
                            message: format!("resource claim returned unknown decision {name}"),
                        }),
                    }
                }
                other => Err(KernelError::InvalidRequest {
                    message: format!("resource claim returned invalid decision {other:?}"),
                }),
            }
        }
    }

    struct ActiveResource {
        package: ResourcePackage,
        generation: u64,
        artifact: Arc<Vec<u8>>,
        host: Arc<ResourceComponentHost>,
        fingerprint: String,
    }

    impl ResourcePackage {
        pub fn discover(root: impl AsRef<Path>) -> Result<Self, KernelError> {
            let root = root.as_ref().to_owned();
            let markdown = fs::read_to_string(root.join("resource.md")).map_err(|error| {
                KernelError::Handler {
                    message: format!("read resource.md: {error}"),
                }
            })?;
            let parsed = super::package::parse_frontmatter::<ResourceFrontmatter>(&markdown)
                .map_err(|error| KernelError::InvalidRequest {
                    message: format!(
                        "invalid resource.md frontmatter in {}: {error}",
                        root.display()
                    ),
                })?;
            let wasm_path = root.join("resource.wasm");
            let source = if root.join("src").is_dir() {
                Some(root.join("src"))
            } else if root.join("source").is_dir() {
                Some(root.join("source"))
            } else {
                None
            };
            if !wasm_path.is_file() && source.is_none() {
                return Err(KernelError::InvalidRequest {
                    message: format!(
                        "resource package {} has no resource.wasm or source",
                        root.display()
                    ),
                });
            }
            if let Some(wit) = root
                .join("resource.wit")
                .is_file()
                .then(|| root.join("resource.wit"))
            {
                let _ = wit;
            }
            Ok(Self {
                root: root.clone(),
                manifest: parsed.frontmatter,
                prose: parsed.prose,
                wasm: wasm_path.is_file().then_some(wasm_path),
                source,
                build_manifest: root
                    .join("Cargo.toml")
                    .is_file()
                    .then_some(root.join("Cargo.toml")),
                wit: root
                    .join("resource.wit")
                    .is_file()
                    .then_some(root.join("resource.wit")),
            })
        }

        fn resource_wit_imports(
            path: &Path,
        ) -> Result<std::collections::HashSet<String>, KernelError> {
            let mut resolve = wit_parser::Resolve::default();
            resolve
                .push_dir(path.parent().unwrap_or_else(|| Path::new(".")))
                .map_err(|error| KernelError::InvalidRequest {
                    message: format!("parse canonical resource WIT: {error}"),
                })?;
            let package = resolve
                .packages
                .iter()
                .find(|(_, package)| package.name.namespace != "artist")
                .map(|(id, _)| id)
                .ok_or_else(|| KernelError::InvalidRequest {
                    message: "resource.wit package was not resolved".to_owned(),
                })?;
            let world = resolve
                .select_world(std::slice::from_ref(&package), None)
                .map_err(|error| KernelError::InvalidRequest {
                    message: format!("resource.wit has no resolvable world: {error}"),
                })?;
            let mut imports = std::collections::HashSet::new();
            for item in resolve.worlds[world].imports.values() {
                let wit_parser::WorldItem::Interface { id, .. } = item else {
                    continue;
                };
                let interface = &resolve.interfaces[*id];
                if interface
                    .name
                    .as_deref()
                    .is_some_and(|name| !matches!(name, "types" | "filesystem"))
                {
                    imports.insert(interface.name.clone().unwrap());
                }
            }
            Ok(imports)
        }

        fn validate_resource_wit(path: &Path, capabilities: &[String]) -> Result<(), KernelError> {
            let mut resolve = wit_parser::Resolve::default();
            let packages = resolve
                .push_dir(path.parent().unwrap_or_else(|| Path::new(".")))
                .map_err(|error| KernelError::InvalidRequest {
                    message: format!("parse canonical resource WIT: {error}"),
                })?;
            let world = resolve
                .select_world(std::slice::from_ref(&packages.0), None)
                .map_err(|error| KernelError::InvalidRequest {
                    message: format!("resource.wit has no resolvable world: {error}"),
                })?;
            for item in resolve.worlds[world].imports.values() {
                let wit_parser::WorldItem::Interface {
                    id: interface_id, ..
                } = item
                else {
                    continue;
                };
                let interface = &resolve.interfaces[*interface_id];
                let Some(interface_name) = interface.name.as_deref() else {
                    continue;
                };
                // `artist:resource/types` is shared data, not an executable
                // nested operation and therefore does not require authority.
                if !matches!(interface_name, "read") {
                    continue;
                }
                let capability = format!("resource.{interface_name}");
                if !capabilities.iter().any(|value| value == &capability) {
                    return Err(KernelError::PermissionDenied { uri: capability });
                }
            }
            Ok(())
        }

        pub fn advertised_schemes(&self) -> impl Iterator<Item = &str> {
            self.manifest
                .routes
                .iter()
                .flat_map(|route| route.schemes.iter().map(String::as_str))
        }

        fn wit_identity(&self) -> Option<String> {
            let path = self.wit.as_ref()?;
            let source = fs::read_to_string(path).ok()?;
            source.lines().find_map(|line| {
                let value = line.trim().strip_prefix("package ")?.trim_end_matches(';');
                let (identity, version) = value.rsplit_once('@')?;
                let major = version.split('.').next()?;
                Some(format!("{identity}@{major}"))
            })
        }

        pub fn catalog_entry(&self) -> ResourceCatalogEntry {
            ResourceCatalogEntry {
                name: self.manifest.name.clone(),
                description: self.manifest.description.clone(),
                docs: self
                    .manifest
                    .docs
                    .iter()
                    .map(|doc| ResourceCatalogDoc {
                        uri: doc.uri.clone(),
                        summary: doc.summary.clone(),
                        verbs: doc.verbs.clone(),
                        query: doc.query.iter().map(|query| query.name.clone()).collect(),
                    })
                    .collect(),
            }
        }

        /// Build or load the resource component without publishing it. The
        /// caller owns generation swap/claim registration; a failed candidate
        /// therefore cannot disturb an already-active package.
        pub fn build(
            &self,
            options: &super::package::BuildOptions,
        ) -> Result<ResourceBuildResult, KernelError> {
            validate_manifest(&self.manifest)?;
            let fingerprint = fingerprint(self, options)?;
            if let Some(artifact) = &self.wasm
                && !super::package::authored_inputs_newer_than_artifact(&self.root, artifact)
            {
                validate_artifact(artifact, self, options)?;
                return Ok(ResourceBuildResult {
                    provenance: artifact.with_extension("wasm.artist.json"),
                    artifact: artifact.clone(),
                    fingerprint,
                    cached: true,
                });
            }
            let manifest =
                self.build_manifest
                    .as_ref()
                    .ok_or_else(|| KernelError::InvalidRequest {
                        message: "resource source package has no Cargo.toml".to_owned(),
                    })?;
            let target = super::package::resolve_component_target(manifest)
                .map_err(|message| KernelError::Handler { message })?;
            let profile = super::package::BuildProfile::directory(options.profile);
            let artifact_path = target
                .target_directory
                .join(&options.target)
                .join(profile)
                .join(format!("{}.wasm", target.target_name.replace('-', "_")));
            let provenance = artifact_path.with_extension("wasm.artist.json");
            if !options.force
                && artifact_path.is_file()
                && super::package::provenance_matches(&provenance, &fingerprint, &artifact_path)
            {
                validate_artifact(&artifact_path, self, options)?;
                return Ok(ResourceBuildResult {
                    artifact: artifact_path,
                    provenance,
                    fingerprint,
                    cached: true,
                });
            }
            let artifact = super::package::build_wasm_artifact(
                &self.root,
                manifest,
                &target.target_name,
                options,
            )
            .map_err(|message| KernelError::Handler { message })?;
            validate_artifact(&artifact, self, options)?;
            let provenance_value = super::package::BuildProvenance {
                package_name: target.package_name,
                package_version: target.package_version,
                contract: Some(self.manifest.contract.clone()),
                abi_version: super::ABI_VERSION.to_owned(),
                target: options.target.clone(),
                profile: profile.to_owned(),
                fingerprint: fingerprint.clone(),
                artifact_sha256: super::package::sha256_file(&artifact)
                    .map_err(|message| KernelError::Handler { message })?,
                cargo_version: super::package::tool_version("cargo"),
                rustc_version: super::package::tool_version("rustc"),
            };
            let provenance = artifact.with_extension("wasm.artist.json");
            fs::write(
                &provenance,
                serde_json::to_vec_pretty(&provenance_value).unwrap(),
            )
            .map_err(|error| KernelError::Handler {
                message: format!("write resource provenance: {error}"),
            })?;
            Ok(ResourceBuildResult {
                artifact,
                provenance,
                fingerprint,
                cached: false,
            })
        }
    }

    fn validate_manifest(manifest: &ResourceFrontmatter) -> Result<(), KernelError> {
        if manifest.contract != "artist:resource:extension@1" {
            return Err(KernelError::InvalidRequest {
                message: format!("unsupported resource contract {}", manifest.contract),
            });
        }
        if manifest.routes.is_empty() {
            return Err(KernelError::InvalidRequest {
                message: "resource package must declare at least one route".to_owned(),
            });
        }
        let mut seen = std::collections::HashSet::new();
        for export in &manifest.exports {
            if export.is_empty()
                || !export.chars().enumerate().all(|(index, ch)| {
                    if index == 0 {
                        ch.is_ascii_alphabetic() || ch == '_'
                    } else {
                        ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
                    }
                })
                || !seen.insert(export)
            {
                return Err(KernelError::InvalidRequest {
                    message: format!("invalid or duplicate resource export {export}"),
                });
            }
        }
        for route in &manifest.routes {
            for scheme in &route.schemes {
                if scheme.is_empty()
                    || !scheme.chars().enumerate().all(|(index, ch)| {
                        ch.is_ascii_alphanumeric() && (index > 0 || ch.is_ascii_alphabetic())
                            || index > 0 && matches!(ch, '+' | '-' | '.')
                    })
                {
                    return Err(KernelError::InvalidUri {
                        message: format!("invalid resource route scheme {scheme}"),
                    });
                }
            }
        }
        Ok(())
    }

    fn fingerprint(
        package: &ResourcePackage,
        options: &super::package::BuildOptions,
    ) -> Result<String, KernelError> {
        let mut hasher = Sha256::new();
        hasher.update(b"artist-resource-component-v1\0");
        hasher.update(super::ABI_VERSION.as_bytes());
        hasher.update(options.target.as_bytes());
        hasher.update(super::package::BuildProfile::directory(options.profile).as_bytes());
        hasher.update(package.manifest.name.as_bytes());
        hasher.update(package.manifest.version.as_bytes());
        hasher.update(package.manifest.contract.as_bytes());
        for capability in &package.manifest.capabilities {
            hasher.update(capability.as_bytes());
        }
        for path in [
            &package.root.join("resource.md"),
            &package.root.join("resource.wit"),
            &package.root.join("Cargo.toml"),
            &package.root.join("Cargo.lock"),
            &package.root.join("deps"),
        ] {
            super::package::hash_path(&mut hasher, path)
                .map_err(|message| KernelError::Handler { message })?;
        }
        if let Some(source) = &package.source {
            super::package::hash_path(&mut hasher, source)
                .map_err(|message| KernelError::Handler { message })?;
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn validate_artifact(
        artifact: &Path,
        package: &ResourcePackage,
        options: &super::package::BuildOptions,
    ) -> Result<(), KernelError> {
        let bytes = fs::read(artifact).map_err(|error| KernelError::Handler {
            message: format!("read {}: {error}", artifact.display()),
        })?;
        let engine = super::package::component_engine().map_err(|error| KernelError::Handler {
            message: error.to_string(),
        })?;
        let component =
            wasmtime::component::Component::from_binary(&engine, &bytes).map_err(|error| {
                KernelError::Handler {
                    message: format!("resource component validation failed: {error}"),
                }
            })?;
        let exports = component
            .component_type()
            .exports(&engine)
            .map(|(name, _)| name.to_owned())
            .collect::<Vec<_>>();
        if !exports
            .iter()
            .any(|name| name.contains("artist:resource/extension"))
        {
            return Err(KernelError::InvalidRequest {
                message: "resource component must export artist:resource/extension@1.0.0"
                    .to_owned(),
            });
        }
        let actual_exports = resource_interface_names(&exports);
        let declared_exports = package
            .manifest
            .exports
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        if actual_exports != declared_exports {
            return Err(KernelError::InvalidRequest {
                message: format!(
                    "resource component export mismatch: declared={declared_exports:?}, actual={actual_exports:?}"
                ),
            });
        }
        // The component type is authoritative: a package cannot hide a
        // standard nested operation import behind stale or missing WIT.
        let actual_imports = component
            .component_type()
            .imports(&engine)
            .map(|(name, _)| name.to_owned())
            .collect::<Vec<_>>();
        let actual_imports = resource_interface_names(&actual_imports);
        for interface in &actual_imports {
            if !package
                .manifest
                .capabilities
                .iter()
                .any(|capability| capability == &format!("resource.{interface}"))
            {
                return Err(KernelError::PermissionDenied {
                    uri: format!("resource.{interface}"),
                });
            }
        }
        if let Some(wit) = &package.wit {
            let declared_imports = ResourcePackage::resource_wit_imports(wit)?;
            if actual_imports != declared_imports {
                return Err(KernelError::InvalidRequest {
                    message: format!(
                        "resource.wit/component import mismatch: declared={declared_imports:?}, actual={actual_imports:?}"
                    ),
                });
            }
        }
        let _ = options;
        Ok(())
    }

    fn resource_interface_names(names: &[String]) -> std::collections::HashSet<String> {
        names
            .iter()
            .filter(|name| {
                name.starts_with("artist:%resource/") || name.starts_with("artist:resource/")
            })
            .filter_map(|name| name.rsplit_once('/').map(|(_, value)| value))
            .filter_map(|value| value.split('@').next())
            .filter(|name| !matches!(*name, "extension" | "types" | "filesystem"))
            .map(str::to_owned)
            .collect()
    }

    pub struct ResourcesHandler {
        root: PathBuf,
        active: super::generations::GenerationStore<ActiveResource>,
        dirty: Arc<std::sync::Mutex<std::collections::HashSet<PathBuf>>>,
        activation_locks:
            Arc<std::sync::Mutex<std::collections::HashMap<PathBuf, Arc<std::sync::Mutex<()>>>>>,
        disabled_file_package: Option<String>,
        publication_lock: Arc<std::sync::Mutex<()>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ResourceVerbBindings {
        pub read: VerbId,
        pub write: VerbId,
        pub edit: VerbId,
        pub insert: VerbId,
        pub poll: VerbId,
        pub run: VerbId,
        pub abort: VerbId,
        pub delete: VerbId,
        pub find: VerbId,
        pub grep: VerbId,
    }

    pub struct DynamicResourcesProvider {
        handler: Arc<ResourcesHandler>,
        bindings: ResourceVerbBindings,
    }

    impl ResourcesHandler {
        async fn invoke_bootstrap_dynamic(
            &self,
            verb: &VerbId,
            uri: ResourceUri,
            input: DynamicValue,
            bindings: &ResourceVerbBindings,
        ) -> Result<DynamicValue, KernelError> {
            let mapped_uri = self.map_uri(&ResourceAddress::uri(uri.clone()))?;
            let input = match verb.function() {
                "find" => DynamicValue::Record(BTreeMap::from([(
                    "query".to_owned(),
                    DynamicValue::String(dynamic_resource_string(&input, "query")?),
                )])),
                "grep" => DynamicValue::Record(BTreeMap::from([(
                    "pattern".to_owned(),
                    DynamicValue::String(dynamic_resource_string(&input, "pattern")?),
                )])),
                _ => remap_bootstrap_dynamic(input, &|value| {
                    self.map_uri(&ResourceAddress::uri(value))
                })?,
            };
            let identity = [
                &bindings.read,
                &bindings.write,
                &bindings.edit,
                &bindings.insert,
                &bindings.delete,
                &bindings.find,
                &bindings.grep,
            ]
            .into_iter()
            // Universal WASM adapters arrive with the open `artist:tool/*`
            // identity.  The selected provider owns the concrete identity;
            // select its binding by function rather than requiring the
            // provider namespace to leak into universal routing.
            .find(|identity| identity.function() == verb.function())
            .cloned()
            .ok_or_else(|| KernelError::UnsupportedVerb {
                verb: verb.to_string(),
                uri: uri.to_string(),
            })?;
            let provider = FileResourceProvider::new(
                Arc::new(FileHandler::new(&self.root)?),
                FileVerbBindings {
                    read: bindings.read.clone(),
                    write: bindings.write.clone(),
                    edit: bindings.edit.clone(),
                    insert: bindings.insert.clone(),
                    delete: bindings.delete.clone(),
                    find: bindings.find.clone(),
                    grep: bindings.grep.clone(),
                },
            );
            let result = provider.invoke(&identity, &mapped_uri, input).await?;
            remap_bootstrap_dynamic(result.output, &|value| self.unmap_bootstrap_uri(value))
        }
    }

    impl DynamicResourcesProvider {
        pub fn new(handler: Arc<ResourcesHandler>, bindings: ResourceVerbBindings) -> Self {
            Self { handler, bindings }
        }
    }

    impl ResourcesHandler {
        pub fn new(root: impl AsRef<Path>) -> Result<Self, KernelError> {
            Self::new_with_watcher(root, None)
        }

        pub fn new_with_watcher(
            root: impl AsRef<Path>,
            watcher: Option<&super::watcher::SharedWatcher>,
        ) -> Result<Self, KernelError> {
            let root = fs::canonicalize(root.as_ref()).map_err(|error| KernelError::Handler {
                message: format!("canonicalize resources root: {error}"),
            })?;
            let handler = Self {
                root,
                active: super::generations::GenerationStore::new(),
                dirty: watcher.map_or_else(
                    || Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
                    super::watcher::SharedWatcher::dirty_set,
                ),
                activation_locks: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
                disabled_file_package: None,
                publication_lock: Arc::new(std::sync::Mutex::new(())),
            };
            Ok(handler)
        }

        pub fn without_file_package(mut self, name: impl Into<String>) -> Self {
            self.disabled_file_package = Some(name.into());
            self
        }

        pub fn activate(&self, package_root: impl AsRef<Path>) -> Result<u64, KernelError> {
            self.activate_with_visiting(
                package_root.as_ref(),
                &mut std::collections::HashSet::new(),
            )
        }

        pub fn ensure_activated(&self) -> Result<(), KernelError> {
            for package in self.discover() {
                self.activate(&package.root)?;
            }
            Ok(())
        }

        fn activate_with_visiting(
            &self,
            package_root: &Path,
            visiting: &mut std::collections::HashSet<String>,
        ) -> Result<u64, KernelError> {
            let package = match ResourcePackage::discover(package_root) {
                Ok(package) => package,
                Err(error) => {
                    if !package_root.is_dir() {
                        self.active
                            .remove_where(|active| active.package.root == package_root);
                        self.dirty.lock().unwrap().remove(package_root);
                        return Err(error);
                    }
                    // Discovery is part of candidate activation. A malformed
                    // edit to an already-active package must leave the
                    // complete prior generation usable, just like a failed
                    // component build does.
                    if let Some(active) = self
                        .active
                        .read()
                        .unwrap()
                        .values()
                        .find(|active| active.package.root == package_root)
                    {
                        return Err(KernelError::Handler {
                            message: format!(
                                "candidate resource package is invalid; generation {} remains active",
                                active.generation
                            ),
                        });
                    }
                    return Err(error);
                }
            };
            if let Some(active) = self.active.current(&package.manifest.name) {
                if active.package.root != package.root {
                    return Err(KernelError::Conflict {
                        uri: package.manifest.name.clone(),
                    });
                }
            }
            let options = super::package::BuildOptions {
                granted_capabilities: package.manifest.capabilities.clone(),
                ..Default::default()
            };
            let candidate_fingerprint = fingerprint(&package, &options)?;
            if let Some(active) = self.active.read().unwrap().get(&package.manifest.name)
                && active.package.root == package.root
                && active.fingerprint == candidate_fingerprint
            {
                self.dirty.lock().unwrap().remove(package_root);
                return Ok(active.generation);
            }
            let build = match package.build(&options) {
                Ok(build) => build,
                Err(error) => {
                    return Err(error);
                }
            };
            let bytes = fs::read(&build.artifact).map_err(|error| KernelError::Handler {
                message: format!("read resource artifact: {error}"),
            })?;
            let dependencies = self.resource_dependencies(&package, &bytes, visiting)?;
            let host = Arc::new(ResourceComponentHost::new(
                &bytes,
                package.manifest.capabilities.clone(),
                dependencies,
            )?);
            let package_root = package.root.clone();
            // A successful rename replaces every prior name associated with
            // this package root, preventing ghost catalog/routing entries.
            let lock = {
                let mut locks = self.activation_locks.lock().unwrap();
                Arc::clone(
                    locks
                        .entry(package_root.to_owned())
                        .or_insert_with(|| Arc::new(std::sync::Mutex::new(()))),
                )
            };
            let _activation = lock.lock().unwrap();
            let _publication = self.publication_lock.lock().unwrap();
            let current_package = ResourcePackage::discover(&package_root)?;
            let current_fingerprint = fingerprint(&current_package, &options)?;
            if current_fingerprint != candidate_fingerprint {
                return Err(KernelError::Conflict {
                    uri: package_root.display().to_string(),
                });
            }
            let mut active = self.active.write().unwrap();
            if let Some(existing) = active.get(&package.manifest.name) {
                if existing.package.root != package_root {
                    return Err(KernelError::Conflict {
                        uri: package.manifest.name.clone(),
                    });
                }
                if existing.fingerprint == candidate_fingerprint {
                    self.dirty.lock().unwrap().remove(&package_root);
                    return Ok(existing.generation);
                }
            }
            let generation = active
                .get(&package.manifest.name)
                .map(|current| current.generation + 1)
                .unwrap_or(1);
            active.retain(|_, value| value.package.root != package_root);
            active.insert(
                package.manifest.name.clone(),
                Arc::new(ActiveResource {
                    package,
                    generation,
                    artifact: Arc::new(bytes),
                    host,
                    fingerprint: candidate_fingerprint,
                }),
            );
            self.dirty.lock().unwrap().remove(&package_root);
            Ok(generation)
        }

        fn resource_dependencies(
            &self,
            current: &ResourcePackage,
            bytes: &[u8],
            visiting: &mut std::collections::HashSet<String>,
        ) -> Result<Vec<super::DependencySpec>, KernelError> {
            let current_identity = current.wit_identity();
            if let Some(identity) = &current_identity {
                if !visiting.insert(identity.clone()) {
                    return Err(KernelError::InvalidRequest {
                        message: format!("resource dependency cycle at {identity}"),
                    });
                }
            }
            let engine =
                super::package::component_engine().map_err(|error| KernelError::Handler {
                    message: format!("load resource component engine: {error}"),
                })?;
            let component =
                wasmtime::component::Component::from_binary(&engine, bytes).map_err(|error| {
                    KernelError::InvalidRequest {
                        message: format!("inspect resource dependencies: {error}"),
                    }
                })?;
            let candidates = self
                .discover()
                .into_iter()
                .filter(|package| package.manifest.name != current.manifest.name)
                .filter_map(|package| package.wit_identity().map(|identity| (identity, package)))
                .collect::<Vec<_>>();
            let mut dependencies = Vec::new();
            for (import, _) in component.component_type().imports(&engine) {
                if import.starts_with("wasi:")
                    || import.contains("artist:%resource/")
                    || import.contains("artist:resource/")
                {
                    continue;
                }
                let matches = candidates
                    .iter()
                    .filter(|(identity, _)| super::contract_matches_import(identity, import))
                    .collect::<Vec<_>>();
                if matches.len() > 1 {
                    return Err(KernelError::Conflict {
                        uri: import.to_owned(),
                    });
                }
                let Some((identity, package)) = matches.first() else {
                    return Err(KernelError::NotFound {
                        uri: import.to_owned(),
                    });
                };
                if visiting.contains(identity) {
                    return Err(KernelError::InvalidRequest {
                        message: format!("resource dependency cycle at {identity}"),
                    });
                }
                let active = if self.dirty.lock().unwrap().contains(&package.root) {
                    // A dependency edit follows the same candidate path as a
                    // top-level edit. A failed rebuild leaves the prior
                    // generation available, while a successful one becomes
                    // the generation pinned into this activation.
                    self.activate_with_visiting(&package.root, visiting)?;
                    self.active
                        .read()
                        .unwrap()
                        .get(&package.manifest.name)
                        .cloned()
                } else {
                    self.active
                        .read()
                        .unwrap()
                        .get(&package.manifest.name)
                        .cloned()
                };
                let (dependency_bytes, dependency_package) = if let Some(active) = active {
                    (active.artifact.clone(), active.package.clone())
                } else {
                    self.activate_with_visiting(&package.root, visiting)?;
                    let active = self
                        .active
                        .read()
                        .unwrap()
                        .get(&package.manifest.name)
                        .cloned()
                        .ok_or_else(|| KernelError::NotFound {
                            uri: package.manifest.name.clone(),
                        })?;
                    (active.artifact.clone(), active.package.clone())
                };
                let children = self.resource_dependencies(
                    &dependency_package,
                    dependency_bytes.as_slice(),
                    visiting,
                )?;
                dependencies.push(super::DependencySpec {
                    contract: identity.clone(),
                    bytes: dependency_bytes.as_ref().clone(),
                    dependencies: children,
                });
            }
            if let Some(identity) = current_identity {
                visiting.remove(&identity);
            }
            Ok(dependencies)
        }

        pub fn active_generation(&self, name: &str) -> Option<u64> {
            self.active
                .read()
                .unwrap()
                .get(name)
                .map(|active| active.generation)
        }

        pub fn discover(&self) -> Vec<ResourcePackage> {
            let Ok(entries) = fs::read_dir(&self.root) else {
                return Vec::new();
            };
            let mut packages = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
                .filter_map(|entry| ResourcePackage::discover(entry.path()).ok())
                .collect::<Vec<_>>();
            packages.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
            packages
        }

        pub fn catalog(&self) -> Vec<ResourceCatalogEntry> {
            self.active
                .remove_where(|active| !active.package.root.is_dir());
            // The model catalog is a view of published generations, not a
            // second discovery registry. Activate valid candidates first;
            // failed activation leaves an existing generation untouched and
            // never publishes a never-active or malformed package.
            for package in self.discover() {
                let _ = self.activate(&package.root);
            }
            let mut packages = self
                .active
                .read()
                .unwrap()
                .values()
                .filter(|active| {
                    active.package.root.is_dir()
                        && active.package.root.join("resource.md").is_file()
                        && self
                            .disabled_file_package
                            .as_ref()
                            .is_none_or(|name| active.package.manifest.name != *name)
                })
                .map(|active| active.package.clone())
                .collect::<Vec<_>>();
            packages.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
            packages
                .into_iter()
                .map(|package| package.catalog_entry())
                .collect()
        }

        fn active_candidates(
            &self,
            uri: &ResourceUri,
            verb: &VerbId,
            scope: &artist_kernel::InvocationScope,
        ) -> Vec<Arc<ActiveResource>> {
            self.active
                .remove_where(|active| !active.package.root.is_dir());
            // A claim phase may already have selected an active generation.
            // Reuse that leased object even if the live registry has swapped
            // to a newer generation before a nested call arrives.
            if let Some(pinned) =
                scope.generation_handle::<ActiveResource>(&generation_pin_key(verb, uri))
            {
                return vec![pinned];
            }
            let mut candidates = Vec::new();
            for package in self.discover() {
                if uri.scheme() == "file"
                    && self
                        .disabled_file_package
                        .as_ref()
                        .is_some_and(|name| name == &package.manifest.name)
                {
                    continue;
                }
                if !package
                    .advertised_schemes()
                    .any(|scheme| scheme == uri.scheme())
                {
                    continue;
                }
                let snapshotted = scope.snapshotted_generation(&package.manifest.name);
                let package_dirty = self.dirty.lock().unwrap().contains(&package.root);
                let active = scope
                    .generation_handle::<ActiveResource>(&resource_package_pin_key(
                        &package.manifest.name,
                    ))
                    .or_else(|| {
                        snapshotted.and_then(|generation| {
                            self.active
                                .read()
                                .unwrap()
                                .get(&package.manifest.name)
                                .filter(|active| active.generation == generation)
                                .cloned()
                        })
                    })
                    .or_else(|| {
                        if !package_dirty {
                            self.active
                                .read()
                                .unwrap()
                                .get(&package.manifest.name)
                                .cloned()
                        } else {
                            None
                        }
                    });
                if let Some(active) = active {
                    let generation =
                        scope.snapshot_generation(&active.package.manifest.name, active.generation);
                    if generation != active.generation {
                        continue;
                    }
                    scope.pin_generation_handle(
                        resource_package_pin_key(&active.package.manifest.name),
                        Arc::clone(&active),
                    );
                    candidates.push(active);
                }
            }
            // Discovery is intentionally fallible and may omit a package while
            // it is being edited. Keep the last active generation eligible in
            // that interval; activation will retry the candidate and preserve
            // this generation if the edit is malformed.
            for active in self.active.read().unwrap().values() {
                // A removed package is a real deactivation. Keep an active
                // generation through malformed edits at its existing root,
                // but never let a deleted package become a permanent routing
                // ghost.
                if !active.package.root.is_dir()
                    || !active.package.root.join("resource.md").is_file()
                {
                    continue;
                }
                if let Some(generation) =
                    scope.snapshotted_generation(&active.package.manifest.name)
                    && generation != active.generation
                {
                    continue;
                }
                if !(uri.scheme() == "file"
                    && self
                        .disabled_file_package
                        .as_ref()
                        .is_some_and(|name| name == &active.package.manifest.name))
                    && active
                        .package
                        .advertised_schemes()
                        .any(|scheme| scheme == uri.scheme())
                    && !candidates
                        .iter()
                        .any(|current: &Arc<ActiveResource>| Arc::ptr_eq(current, active))
                {
                    candidates.push(Arc::clone(active));
                }
            }
            // HashMap iteration is not a routing policy. Keep claim order
            // deterministic so diagnostics and conflict arbitration are
            // stable across processes.
            candidates.sort_by(|left, right| {
                left.package
                    .manifest
                    .name
                    .cmp(&right.package.manifest.name)
                    .then(left.generation.cmp(&right.generation))
            });
            candidates
        }

        fn unmap_bootstrap_uri(&self, uri: ResourceUri) -> Result<ResourceUri, KernelError> {
            let path = uri
                .as_ref()
                .to_file_path()
                .map_err(|_| KernelError::InvalidUri {
                    message: uri.to_string(),
                })?;
            let relative = path
                .strip_prefix(&self.root)
                .map_err(|_| KernelError::InvalidUri {
                    message: uri.to_string(),
                })?;
            let mut components = relative.components();
            let package = components
                .next()
                .and_then(|component| match component {
                    std::path::Component::Normal(value) => value.to_str(),
                    _ => None,
                })
                .ok_or_else(|| KernelError::InvalidUri {
                    message: uri.to_string(),
                })?;
            let rest = components
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let mut output = ResourceUri::parse(&if rest.is_empty() {
                format!("resources://{package}/")
            } else {
                format!("resources://{package}/{rest}")
            })?;
            if let Some(query) = uri.query() {
                output = output.with_query(query);
            }
            if let Some(fragment) = uri.fragment() {
                output = output.with_fragment(fragment);
            }
            Ok(output)
        }

        fn map_uri(&self, target: &ResourceAddress) -> Result<ResourceUri, KernelError> {
            let uri = target.as_uri().ok_or_else(|| KernelError::InvalidUri {
                message: "resources handler requires URI".to_owned(),
            })?;
            if uri.scheme() != "resources" {
                return Err(KernelError::UnsupportedUri {
                    uri: uri.to_string(),
                });
            }
            let package = uri.as_ref().host_str().unwrap_or_default();
            if package.is_empty() {
                return Err(KernelError::InvalidUri {
                    message: "resources URI must name a package".to_owned(),
                });
            }
            let relative = uri.path().trim_start_matches('/');
            if std::path::Path::new(relative)
                .components()
                .any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir | std::path::Component::RootDir
                    )
                })
            {
                return Err(KernelError::InvalidRequest {
                    message: "resources URI escapes the resources root".to_owned(),
                });
            }
            let path = self.root.join(package).join(relative);
            let mut mapped = ResourceUri::parse(&path.display().to_string())?;
            if let Some(query) = uri.query() {
                mapped = mapped.with_query(query);
            }
            if let Some(fragment) = uri.fragment() {
                mapped = mapped.with_fragment(fragment);
            }
            Ok(mapped)
        }

        async fn invoke_dynamic_provider(
            &self,
            verb: &VerbId,
            uri: &ResourceUri,
            input: DynamicValue,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<DynamicValue, KernelError> {
            let candidates = self.active_candidates(uri, verb, &scope);
            if candidates.is_empty() {
                return Err(KernelError::NoHandler {
                    uri: uri.to_string(),
                });
            }
            let (candidate, decision) = self.select_candidate(verb, uri, candidates).await?;
            if decision == artist_kernel::ClaimDecision::Reserve {
                return Err(KernelError::UnsupportedVerb {
                    verb: verb.to_string(),
                    uri: uri.to_string(),
                });
            }
            let Some(candidate) = candidate else {
                return Err(KernelError::NoHandler {
                    uri: uri.to_string(),
                });
            };
            // Resource packages own their exact WIT input shape. The kernel
            // supplies the leased typed value unchanged; no central verb
            // vocabulary may reshape it here.
            let input = if verb.function() == "read" {
                let requests = match input {
                    DynamicValue::Record(mut fields) => match fields.remove("requests") {
                        Some(DynamicValue::List(requests)) => requests,
                        Some(other) => {
                            return Err(KernelError::InvalidRequest {
                                message: format!(
                                    "resource read requests expects a list, got {other:?}"
                                ),
                            });
                        }
                        None => vec![DynamicValue::Record(fields)],
                    },
                    DynamicValue::List(requests) => requests,
                    other => {
                        return Err(KernelError::InvalidRequest {
                            message: format!(
                                "resource read expects a request record or list, got {other:?}"
                            ),
                        });
                    }
                };
                DynamicValue::List(
                    requests
                        .into_iter()
                        .map(|request| match request {
                            DynamicValue::Record(mut fields) => {
                                fields.insert(
                                    "uri".to_owned(),
                                    DynamicValue::String(uri.to_string()),
                                );
                                Ok(DynamicValue::Record(fields))
                            }
                            other => Err(KernelError::InvalidRequest {
                                message: format!(
                                    "resource read request expects a record, got {other:?}"
                                ),
                            }),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                )
            } else {
                input
            };
            candidate
                .host
                .invoke_dynamic(verb, input, host, scope, &candidate.host.capabilities)
                .await
        }

        async fn select_candidate(
            &self,
            verb: &VerbId,
            uri: &ResourceUri,
            candidates: Vec<Arc<ActiveResource>>,
        ) -> Result<(Option<Arc<ActiveResource>>, artist_kernel::ClaimDecision), KernelError>
        {
            let mut owners = Vec::new();
            for candidate in candidates {
                let decision = candidate.host.claim(verb, uri).await.map_err(|error| {
                    KernelError::Handler {
                        message: format!(
                            "resource claim failed for {} ({}) in {}: {error}",
                            verb, uri, candidate.package.manifest.name
                        ),
                    }
                })?;
                if matches!(decision, artist_kernel::ClaimDecision::Handle)
                    && !candidate
                        .package
                        .manifest
                        .exports
                        .iter()
                        .any(|export| export == verb.function())
                {
                    return Err(KernelError::Handler {
                        message: format!(
                            "resource package {} claimed undeclared verb {verb}",
                            candidate.package.manifest.name
                        ),
                    });
                }
                if decision != artist_kernel::ClaimDecision::Pass {
                    owners.push((candidate, decision));
                }
            }
            if owners.len() > 1 {
                return Err(KernelError::Conflict {
                    uri: format!(
                        "{uri} (competing resource packages: {})",
                        owners
                            .iter()
                            .map(|(candidate, decision)| {
                                format!("{}:{decision:?}", candidate.package.manifest.name)
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
            Ok(owners
                .pop()
                .map(|(candidate, decision)| (Some(candidate), decision))
                .unwrap_or((None, artist_kernel::ClaimDecision::Pass)))
        }
    }

    impl DynamicClaimProvider for DynamicResourcesProvider {
        fn claim(&self, verb: &VerbId, uri: &ResourceUri) -> ClaimDecision {
            if uri.scheme() == "resources" {
                let owns = [
                    &self.bindings.read,
                    &self.bindings.write,
                    &self.bindings.edit,
                    &self.bindings.insert,
                    &self.bindings.poll,
                    &self.bindings.run,
                    &self.bindings.abort,
                    &self.bindings.delete,
                    &self.bindings.find,
                    &self.bindings.grep,
                ]
                .contains(&verb);
                return if owns {
                    ClaimDecision::Handle
                } else {
                    ClaimDecision::Pass
                };
            }
            let scope =
                artist_kernel::InvocationScope::new(artist_kernel::InvocationContext::default());
            if self.handler.active_candidates(uri, verb, &scope).is_empty() {
                ClaimDecision::Pass
            } else {
                ClaimDecision::Handle
            }
        }
    }

    impl DynamicResourceProvider for DynamicResourcesProvider {
        fn verb_definitions(&self) -> Vec<artist_kernel::VerbDefinition> {
            if self.handler.ensure_activated().is_err() {
                return Vec::new();
            }
            [
                (&self.bindings.read, "read"),
                (&self.bindings.write, "write"),
                (&self.bindings.edit, "edit"),
                (&self.bindings.insert, "insert"),
                (&self.bindings.poll, "poll"),
                (&self.bindings.run, "run"),
                (&self.bindings.abort, "abort"),
                (&self.bindings.delete, "delete"),
                (&self.bindings.find, "find"),
                (&self.bindings.grep, "grep"),
            ]
            .into_iter()
            .map(|(identity, function)| {
                artist_kernel::VerbDefinition::new(
                    identity.clone(),
                    function,
                    function,
                    format!("Resources {function} provider"),
                )
            })
            .collect()
        }

        fn invoke<'a>(
            &'a self,
            verb: &'a VerbId,
            uri: &'a ResourceUri,
            input: DynamicValue,
        ) -> ResourceFuture<'a> {
            Box::pin(async move {
                // The package-file bootstrap mount is still a compatibility
                // path: it exposes ordinary filesystem files before a
                // resource component is activated. Component-backed calls
                // below use the direct dynamic WIT path.
                if uri.scheme() == "resources" {
                    let result = self
                        .handler
                        .invoke_bootstrap_dynamic(verb, uri.clone(), input, &self.bindings)
                        .await?;
                    return Ok(DynamicVerbResult {
                        verb: verb.clone(),
                        function: verb.function().to_owned(),
                        output: result,
                    });
                }
                let result = self
                    .handler
                    .invoke_dynamic_provider(
                        verb,
                        uri,
                        input,
                        KernelHandle::detached(),
                        artist_kernel::InvocationScope::new(
                            artist_kernel::InvocationContext::default(),
                        ),
                    )
                    .await?;
                Ok(DynamicVerbResult {
                    verb: verb.clone(),
                    function: verb.function().to_owned(),
                    output: result,
                })
            })
        }

        fn invoke_with_host<'a>(
            &'a self,
            verb: &'a VerbId,
            uri: &'a ResourceUri,
            input: DynamicValue,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> ResourceFuture<'a> {
            Box::pin(async move {
                if uri.scheme() == "resources" {
                    let result = self
                        .handler
                        .invoke_bootstrap_dynamic(verb, uri.clone(), input, &self.bindings)
                        .await?;
                    return Ok(DynamicVerbResult {
                        verb: verb.clone(),
                        function: verb.function().to_owned(),
                        output: result,
                    });
                }
                let result = self
                    .handler
                    .invoke_dynamic_provider(verb, uri, input, host, scope)
                    .await?;
                Ok(DynamicVerbResult {
                    verb: verb.clone(),
                    function: verb.function().to_owned(),
                    output: result,
                })
            })
        }
    }

    fn remap_bootstrap_dynamic(
        value: DynamicValue,
        map_uri: &dyn Fn(ResourceUri) -> Result<ResourceUri, KernelError>,
    ) -> Result<DynamicValue, KernelError> {
        Ok(match value {
            DynamicValue::ResourceUri(uri) => DynamicValue::ResourceUri(map_uri(uri)?),
            DynamicValue::String(value) if value.starts_with("resources://") => {
                DynamicValue::String(map_uri(ResourceUri::parse(&value)?)?.to_string())
            }
            DynamicValue::List(values) => DynamicValue::List(
                values
                    .into_iter()
                    .map(|value| remap_bootstrap_dynamic(value, map_uri))
                    .collect::<Result<_, _>>()?,
            ),
            DynamicValue::Tuple(values) => DynamicValue::Tuple(
                values
                    .into_iter()
                    .map(|value| remap_bootstrap_dynamic(value, map_uri))
                    .collect::<Result<_, _>>()?,
            ),
            DynamicValue::Record(fields) => DynamicValue::Record(
                fields
                    .into_iter()
                    .map(|(name, value)| Ok((name, remap_bootstrap_dynamic(value, map_uri)?)))
                    .collect::<Result<_, KernelError>>()?,
            ),
            DynamicValue::Option(value) => DynamicValue::Option(
                value
                    .map(|value| remap_bootstrap_dynamic(*value, map_uri).map(Box::new))
                    .transpose()?,
            ),
            DynamicValue::Result(result) => DynamicValue::Result(match result {
                Ok(value) => Ok(Box::new(remap_bootstrap_dynamic(*value, map_uri)?)),
                Err(value) => Err(Box::new(remap_bootstrap_dynamic(*value, map_uri)?)),
            }),
            DynamicValue::Variant(name, value) => DynamicValue::Variant(
                name,
                value
                    .map(|value| remap_bootstrap_dynamic(*value, map_uri).map(Box::new))
                    .transpose()?,
            ),
            other => other,
        })
    }

    fn dynamic_resource_record<'a>(
        input: &'a DynamicValue,
    ) -> Result<&'a BTreeMap<String, DynamicValue>, KernelError> {
        match input {
            DynamicValue::Record(fields) => Ok(fields),
            _ => Err(KernelError::InvalidRequest {
                message: "resource dynamic input must be a record".to_owned(),
            }),
        }
    }

    fn dynamic_resource_string(input: &DynamicValue, name: &str) -> Result<String, KernelError> {
        match dynamic_resource_record(input)?.get(name) {
            Some(DynamicValue::String(value)) => Ok(value.clone()),
            _ => Err(KernelError::InvalidRequest {
                message: format!("resource dynamic field {name} must be a string"),
            }),
        }
    }

    fn dynamic_resource_line(line: artist_kernel::AnchoredLine) -> DynamicValue {
        DynamicValue::Record(BTreeMap::from([
            (
                "anchor".to_owned(),
                DynamicValue::List(
                    line.anchor
                        .tokens()
                        .iter()
                        .cloned()
                        .map(DynamicValue::String)
                        .collect(),
                ),
            ),
            ("text".to_owned(), DynamicValue::String(line.text)),
            (
                "ending".to_owned(),
                DynamicValue::String(format!("{:?}", line.ending).to_lowercase()),
            ),
        ]))
    }

    fn dynamic_resource_text(text: artist_kernel::AnchoredText) -> DynamicValue {
        DynamicValue::Record(BTreeMap::from([
            ("uri".to_owned(), DynamicValue::ResourceUri(text.uri)),
            (
                "lines".to_owned(),
                DynamicValue::List(text.lines.into_iter().map(dynamic_resource_line).collect()),
            ),
        ]))
    }

    impl ResourceCatalogProvider for ResourcesHandler {
        fn resource_catalog(&self) -> Vec<ResourceCatalogEntry> {
            self.catalog()
        }
    }

    fn generation_pin_key(verb: &VerbId, uri: &ResourceUri) -> String {
        format!("{}:{}", verb, uri.without_fragment())
    }

    fn resource_package_pin_key(package: &str) -> String {
        format!("resource-package:{package}")
    }
}

/// Shared debounced watcher for editable tool/resource package trees. The
/// caller owns activation policy; this service only identifies the affected
/// package, so one broken edit cannot poison unrelated packages.
pub mod watcher {
    use artist_kernel::KernelError;
    use notify_debouncer_mini::{
        Debouncer, new_debouncer,
        notify::{RecommendedWatcher, RecursiveMode},
    };
    use std::{
        path::{Path, PathBuf},
        sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
        sync::{Arc, Mutex},
        time::Duration,
    };

    pub type PackageWatcher = Debouncer<RecommendedWatcher>;

    pub struct WatcherHandle {
        stop: Option<Sender<()>>,
        join: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for WatcherHandle {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    /// Shared dirty-package state for the project-level tools and resources
    /// roots. A single debouncer feeds both namespaces so an external edit
    /// cannot leave their activation paths observing different file-change
    /// timelines.
    #[derive(Clone, Default)]
    pub struct SharedWatcher {
        dirty: Arc<Mutex<std::collections::HashSet<PathBuf>>>,
    }

    impl SharedWatcher {
        pub fn new() -> Self {
            Self::default()
        }

        pub(crate) fn dirty_set(&self) -> Arc<Mutex<std::collections::HashSet<PathBuf>>> {
            Arc::clone(&self.dirty)
        }

        pub fn start(
            &self,
            roots: impl IntoIterator<Item = impl AsRef<Path>>,
        ) -> Result<WatcherHandle, KernelError> {
            let (debouncer, events) = watch_roots(roots).map_err(|error| KernelError::Handler {
                message: format!("start shared package watcher: {error}"),
            })?;
            let dirty = Arc::clone(&self.dirty);
            let (stop_tx, stop_rx) = mpsc::channel();
            let join = std::thread::spawn(move || {
                let _debouncer = debouncer;
                loop {
                    match stop_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    while let Ok(package) = events.try_recv() {
                        dirty.lock().unwrap().insert(package);
                    }
                }
            });
            Ok(WatcherHandle {
                stop: Some(stop_tx),
                join: Some(join),
            })
        }
    }

    pub fn watch_roots(
        roots: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> Result<(PackageWatcher, Receiver<PathBuf>), notify_debouncer_mini::notify::Error> {
        let roots = roots
            .into_iter()
            .map(|root| root.as_ref().to_owned())
            .collect::<Vec<_>>();
        let watched_roots = roots.clone();
        let (events_tx, events_rx) = mpsc::channel();
        let mut debouncer = new_debouncer(
            Duration::from_millis(200),
            move |result: notify_debouncer_mini::DebounceEventResult| {
                if let Ok(events) = result {
                    for event in events {
                        if let Some(package) = package_root_for_event(&event.path, &watched_roots) {
                            let _ = events_tx.send(package);
                        }
                    }
                }
            },
        )?;
        for root in roots {
            debouncer.watcher().watch(&root, RecursiveMode::Recursive)?;
        }
        Ok((debouncer, events_rx))
    }

    fn package_root(path: &Path) -> Option<PathBuf> {
        if path
            .components()
            .any(|component| matches!(component.as_os_str().to_str(), Some("target" | ".git")))
        {
            return None;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with('.')
                    || name.ends_with('~')
                    || name.ends_with(".wasm.artist.json")
                    || name.contains(".artist-tmp-")
            })
        {
            return None;
        }
        path.ancestors()
            .find(|candidate| {
                candidate.join("tool.md").is_file() || candidate.join("resource.md").is_file()
            })
            .map(Path::to_owned)
    }

    fn package_root_for_event(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
        if let Some(package) = package_root(path) {
            return Some(package);
        }
        // A deletion event may point at the package manifest itself. Once the
        // file is gone, marker-based ancestor discovery cannot identify the
        // package, so recover the first directory below the watched root.
        roots.iter().find_map(|root| {
            let relative = path.strip_prefix(root).ok()?;
            let package = relative.components().next()?.as_os_str();
            let package = root.join(package);
            (package != *root).then_some(package)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register_file_provider(kernel: &artist_kernel::Kernel, root: &std::path::Path) {
        let identity = |function: &str| {
            artist_kernel::VerbId::new(format!("artist:filesystem/{function}@1.0.0")).unwrap()
        };
        kernel
            .register_dynamic_resource_provider(std::sync::Arc::new(
                artist_kernel::FileResourceProvider::new(
                    std::sync::Arc::new(artist_kernel::FileHandler::new(root).unwrap()),
                    artist_kernel::FileVerbBindings {
                        read: identity("read"),
                        write: identity("write"),
                        edit: identity("edit"),
                        insert: identity("insert"),
                        delete: identity("delete"),
                        find: identity("find"),
                        grep: identity("grep"),
                    },
                ),
            ))
            .unwrap();
    }

    fn register_resource_provider(
        kernel: &artist_kernel::Kernel,
        handler: resources::ResourcesHandler,
    ) {
        let identity = |function: &str| {
            artist_kernel::VerbId::new(format!("artist:resources/{function}@1.0.0")).unwrap()
        };
        kernel
            .register_dynamic_resource_provider(std::sync::Arc::new(
                resources::DynamicResourcesProvider::new(
                    std::sync::Arc::new(handler),
                    resources::ResourceVerbBindings {
                        read: identity("read"),
                        write: identity("write"),
                        edit: identity("edit"),
                        insert: identity("insert"),
                        poll: identity("poll"),
                        run: identity("run"),
                        abort: identity("abort"),
                        delete: identity("delete"),
                        find: identity("find"),
                        grep: identity("grep"),
                    },
                ),
            ))
            .unwrap();
    }

    #[test]
    fn dynamic_values_round_trip_through_component_values() {
        let value = DynamicValue::Record(std::collections::BTreeMap::from([
            ("name".into(), DynamicValue::String("transform".into())),
            ("enabled".into(), DynamicValue::Bool(true)),
            (
                "items".into(),
                DynamicValue::List(vec![DynamicValue::S32(7), DynamicValue::S32(9)]),
            ),
        ]));
        let component = dynamic_value_to_component_val(&value).unwrap();
        assert_eq!(component_val_to_dynamic_value(&component).unwrap(), value);
    }

    #[test]
    fn typed_lifting_preserves_uri_aliases_inside_nested_values() {
        let uri = artist_kernel::ResourceUri::parse("osproc://42").unwrap();
        let value = DynamicValue::Record(std::collections::BTreeMap::from([(
            "targets".into(),
            DynamicValue::List(vec![DynamicValue::ResourceUri(uri.clone())]),
        )]));
        let ty = DynamicType::Record(std::collections::BTreeMap::from([(
            "targets".into(),
            DynamicType::List(Box::new(DynamicType::ResourceUri)),
        )]));
        let component = dynamic_value_to_component_val(&value).unwrap();
        assert_eq!(
            component_val_to_dynamic_value_with_type(&component, &ty).unwrap(),
            value
        );
    }

    #[test]
    fn component_artifact_validation_rejects_core_or_invalid_wasm() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("invalid.wasm");
        std::fs::write(&path, b"not a component").unwrap();
        let error = validate_component_artifact(&path).unwrap_err();
        assert!(error.contains("invalid WebAssembly component"));
    }
    use crate::tools::ToolsHandler;
    use std::path::PathBuf;

    #[test]
    fn typed_component_invokes_a_real_filesystem_resource() {
        let package_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs/read");
        let package = package::ToolPackage::discover(&package_root).unwrap();
        let mut options = package::BuildOptions::default();
        options.force = true;
        options.granted_capabilities = vec!["resource.read".to_owned()];
        let build = package.build(&options).unwrap();
        let _bytes = std::fs::read(build.artifact).unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("typed.txt"), "typed bridge\n").unwrap();
        let typed_uri = root.path().join("typed.txt").display().to_string();
        let kernel = artist_kernel::Kernel::new();
        register_file_provider(&kernel, root.path());

        let registry = runtime::ComponentRegistry::new();
        let active = registry.reload(&package, &options).unwrap();
        assert_eq!(active.info().interfaces, vec!["artist:tool:read@1"]);
        let routed = active
            .invoke_tool_json(
                "read",
                &format!(
                    r#"[{{"uri":"{}","at":null,"before":null,"after":null}}]"#,
                    typed_uri
                ),
                kernel.handle(),
            )
            .unwrap();
        assert!(
            routed.contains("typed bridge"),
            "typed runtime route failed: {routed}"
        );
    }
    fn validates_named_wit_imports_for_nested_capability_authority() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(
            root.path().join("resource.md"),
            "---\nname: aliased\ndescription: Aliased\nversion: 0.1.0\ncontract: artist:resource:extension@1\nroutes: [{schemes: [file]}]\nexports: [read]\ncapabilities: []\n---\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("resource.wit"),
            "package example:aliased@1.0.0;\nworld aliased { import source-read: artist:tool/read@1.0.0; export artist:%resource/extension@1.0.0; export artist:tool/read@1.0.0; }\n",
        )
        .unwrap();
        let error = resources::ResourcePackage::discover(root.path()).unwrap_err();
        assert!(
            matches!(error, artist_kernel::KernelError::PermissionDenied { ref uri } if uri == "resource.read")
        );
    }

    #[test]
    fn discovers_tool_markdown_packages() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tool.md"),
            "---\nname: echo\ndescription: Echo input\nversion: 0.1.0\ncontract: \"artist:tool:read@1\"\ncapabilities:\n  - resource.read\n---\n\nEcho prose.\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn run() {}\n").unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"echo\"\n",
        )
        .unwrap();
        let package = package::ToolPackage::discover(dir.path()).unwrap();
        assert_eq!(package.manifest.name, "echo");
        assert_eq!(package.prose, "Echo prose.");
        assert!(package.wasm.is_none());
        assert!(package.source.is_some());
        assert!(package.build_manifest.is_some());
        assert!(package.wit.is_none());
        assert_eq!(
            package.contract.as_ref().map(ToString::to_string),
            Some("artist:tool:read@1".to_owned())
        );
    }

    #[test]
    fn discovers_a_typed_tool_contract_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tool.md"),
            "---\nname: read\ndescription: Read tool\nversion: 0.1.0\ncontract: artist:tool:read@1\n---\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let package = package::ToolPackage::discover(dir.path()).unwrap();
        assert_eq!(
            package.contract,
            Some(contracts::ContractId::universal("read"))
        );
    }

    #[test]
    fn malformed_tool_packages_are_isolated_from_valid_and_cached_tools() {
        let root = tempfile::tempdir().unwrap();
        let valid = root.path().join("valid");
        std::fs::create_dir(&valid).unwrap();
        std::fs::write(
            valid.join("tool.md"),
            "---\nname: valid-tool\ndescription: Valid tool\nversion: 0.1.0\ncontract: artist:tool:read@1\n---\n",
        )
        .unwrap();
        std::fs::write(
            valid.join("tool.wasm"),
            b"not activated in this discovery test",
        )
        .unwrap();

        let malformed = root.path().join("malformed");
        std::fs::create_dir(&malformed).unwrap();
        std::fs::write(malformed.join("tool.md"), "not frontmatter").unwrap();

        let handler = ToolsHandler::new(root.path(), []).unwrap();
        let registrations = handler.registrations().unwrap();
        assert_eq!(
            registrations
                .iter()
                .map(|registration| registration.package.as_str())
                .collect::<Vec<_>>(),
            vec!["valid-tool"]
        );

        // A half-edited package must not erase its last known registration or
        // prevent lookup of an unrelated valid package.
        std::fs::write(valid.join("tool.md"), "not frontmatter either").unwrap();
        let registrations = handler.registrations().unwrap();
        assert_eq!(registrations.len(), 1);
        assert_eq!(registrations[0].package, "valid-tool");
    }

    #[test]
    fn removed_or_renamed_tool_packages_do_not_leave_ghost_registrations() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("package");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(
            package.join("tool.md"),
            "---\nname: cached-tool\ndescription: Cached tool\nversion: 0.1.0\ncontract: artist:tool:read@1\n---\n",
        )
        .unwrap();
        std::fs::write(package.join("tool.wasm"), b"not activated in this test").unwrap();

        let handler = ToolsHandler::new(root.path(), []).unwrap();
        assert_eq!(handler.registrations().unwrap().len(), 1);

        // A package rename is a manifest identity change, not merely moving
        // its directory. The old registration must disappear on the next
        // catalog observation.
        std::fs::write(
            package.join("tool.md"),
            "---\nname: renamed-tool\ndescription: Renamed tool\nversion: 0.1.0\ncontract: artist:tool:read@1\n---\n",
        )
        .unwrap();
        let registrations = handler.registrations().unwrap();
        assert_eq!(registrations.len(), 1);
        assert_eq!(registrations[0].package, "renamed-tool");

        std::fs::remove_dir_all(&package).unwrap();
        assert!(handler.registrations().unwrap().is_empty());
    }

    #[test]
    fn discovers_resource_packages_and_builds_compact_catalog_entries() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("ast");
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::write(
            package.join("resource.md"),
            "---\nname: artist-ast\ndescription: AST projections\nversion: 0.1.0\ncontract: artist:resource:extension@1\nroutes:\n  - schemes: [file]\nexports: [read]\ncapabilities: [resource.read]\ndocs:\n  - uri: file://<path>/symbols/\n    summary: Lists symbols\n    verbs: [read]\n    query:\n      - name: kind\n        summary: Symbol kind\n---\n\nFull AST documentation.\n",
        )
        .unwrap();
        let handler = resources::ResourcesHandler::new(root.path()).unwrap();
        let packages = handler.discover();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].manifest.routes[0].schemes, vec!["file"]);
        assert!(packages[0].prose.contains("Full AST documentation"));
        let catalog = handler.catalog();
        assert!(
            catalog.is_empty(),
            "inactive packages must not seed the catalog"
        );

        let authored_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/resources");
        let active_handler = resources::ResourcesHandler::new(&authored_root).unwrap();
        active_handler.activate(authored_root.join("ast")).unwrap();
        let generation = active_handler.active_generation("artist-ast");
        let catalog = active_handler.catalog();
        assert_eq!(active_handler.active_generation("artist-ast"), generation);
        assert_eq!(catalog.len(), 1);
        assert_eq!(
            catalog[0].description,
            "AST projections over ordinary file resources"
        );
        assert!(
            catalog[0]
                .docs
                .iter()
                .any(|doc| doc.uri == "file://<path>/symbols/")
        );
    }

    #[test]
    fn builds_the_ast_resource_component_with_a_nested_read_import() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/resources/ast");
        let package = resources::ResourcePackage::discover(&root).unwrap();
        assert_eq!(package.manifest.name, "artist-ast");
        assert!(package.wit.is_some());
        let build = package
            .build(&package::BuildOptions {
                force: true,
                granted_capabilities: package.manifest.capabilities.clone(),
                ..Default::default()
            })
            .unwrap();
        assert!(build.artifact.is_file());
        let engine = package::component_engine().unwrap();
        let component = wasmtime::component::Component::from_binary(
            &engine,
            &std::fs::read(build.artifact).unwrap(),
        )
        .unwrap();
        let imports = component
            .component_type()
            .imports(&engine)
            .map(|(name, _)| name.to_owned())
            .collect::<Vec<_>>();
        assert!(imports.iter().any(|name| name.contains("resource/read")));
    }

    #[tokio::test]
    async fn ast_resource_reads_a_file_through_the_typed_nested_resource_path() {
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("hello world.rs");
        std::fs::write(&source, "fn main() {}\nstruct Marker;\n").unwrap();
        let authored_root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/resources/ast");
        let authored = resources::ResourcePackage::discover(&authored_root).unwrap();
        let built = authored
            .build(&package::BuildOptions {
                force: true,
                granted_capabilities: authored.manifest.capabilities.clone(),
                ..Default::default()
            })
            .unwrap();
        let resource_root = tempfile::tempdir().unwrap();
        let installed = resource_root.path().join("ast");
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::copy(
            authored_root.join("resource.md"),
            installed.join("resource.md"),
        )
        .unwrap();
        std::fs::copy(built.artifact, installed.join("resource.wasm")).unwrap();

        let kernel = artist_kernel::Kernel::new();
        register_file_provider(&kernel, project.path());
        register_resource_provider(
            &kernel,
            resources::ResourcesHandler::new(&resource_root).unwrap(),
        );

        let source_uri = artist_kernel::ResourceUri::parse(&source.display().to_string()).unwrap();
        let projection =
            artist_kernel::ResourceUri::parse(&format!("{source_uri}/symbols/")).unwrap();
        let result = kernel
            .invoke_dynamic_resource(
                artist_kernel::VerbId::new("artist:resources/read@1.0.0").unwrap(),
                projection,
                artist_kernel::DynamicValue::Record(std::collections::BTreeMap::from([
                    ("at".to_owned(), artist_kernel::DynamicValue::Option(None)),
                    (
                        "before".to_owned(),
                        artist_kernel::DynamicValue::Option(None),
                    ),
                    (
                        "after".to_owned(),
                        artist_kernel::DynamicValue::Option(None),
                    ),
                ])),
            )
            .await
            .unwrap();
        assert!(format!("{:?}", result.output).contains("fn main"));

        // The bootstrap mount is also a typed file namespace: package
        // metadata remains inspectable through the same typed resource path.
        let package_file =
            artist_kernel::ResourceUri::parse("resources://ast/resource.md").unwrap();
        let package_result = kernel
            .invoke_dynamic_resource(
                artist_kernel::VerbId::new("artist:resources/read@1.0.0").unwrap(),
                package_file.clone(),
                artist_kernel::DynamicValue::Record(std::collections::BTreeMap::from([
                    ("at".to_owned(), artist_kernel::DynamicValue::Option(None)),
                    (
                        "before".to_owned(),
                        artist_kernel::DynamicValue::Option(None),
                    ),
                    (
                        "after".to_owned(),
                        artist_kernel::DynamicValue::Option(None),
                    ),
                ])),
            )
            .await
            .unwrap();
        assert!(format!("{:?}", package_result.output).contains("artist-ast"));
    }

    #[tokio::test]
    async fn ast_resource_exposes_deeper_children_and_preserves_query_identity() {
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("main.rs");
        std::fs::write(&source, "fn main() {}\nfn caller() { main(); }\n").unwrap();
        let authored_root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/resources/ast");
        let authored = resources::ResourcePackage::discover(&authored_root).unwrap();
        let built = authored
            .build(&package::BuildOptions {
                force: true,
                granted_capabilities: authored.manifest.capabilities.clone(),
                ..Default::default()
            })
            .unwrap();
        let resource_root = tempfile::tempdir().unwrap();
        let installed = resource_root.path().join("ast");
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::copy(
            authored_root.join("resource.md"),
            installed.join("resource.md"),
        )
        .unwrap();
        std::fs::copy(built.artifact, installed.join("resource.wasm")).unwrap();

        let kernel = artist_kernel::Kernel::new();
        register_file_provider(&kernel, project.path());
        register_resource_provider(
            &kernel,
            resources::ResourcesHandler::new(&resource_root).unwrap(),
        );

        let file = artist_kernel::ResourceUri::parse(&source.display().to_string()).unwrap();
        let projection =
            artist_kernel::ResourceUri::parse(&format!("{file}/symbols/main/callers?limit=1"))
                .unwrap();
        let result = kernel
            .invoke_dynamic_resource(
                artist_kernel::VerbId::new("artist:resources/read@1.0.0").unwrap(),
                projection,
                artist_kernel::DynamicValue::Record(std::collections::BTreeMap::from([
                    ("at".to_owned(), artist_kernel::DynamicValue::Option(None)),
                    (
                        "before".to_owned(),
                        artist_kernel::DynamicValue::Option(None),
                    ),
                    (
                        "after".to_owned(),
                        artist_kernel::DynamicValue::Option(None),
                    ),
                ])),
            )
            .await
            .unwrap();
        let output = format!("{:?}", result.output);
        assert!(output.contains("limit=1"));
        assert!(output.contains("main()"));
    }

    #[tokio::test]
    async fn ast_resource_reserves_projection_mutations() {
        let resource_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/resources");
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("main.rs");
        std::fs::write(&source, "fn main() {}\n").unwrap();
        let kernel = artist_kernel::Kernel::new();
        register_file_provider(&kernel, project.path());
        register_resource_provider(
            &kernel,
            resources::ResourcesHandler::new(&resource_root).unwrap(),
        );
        let projection = artist_kernel::ResourceUri::parse(&format!(
            "{}/symbols/",
            artist_kernel::ResourceUri::parse(&source.display().to_string()).unwrap()
        ))
        .unwrap();
        let result = kernel
            .invoke_dynamic_resource(
                artist_kernel::VerbId::new("artist:resources/edit@1.0.0").unwrap(),
                projection,
                artist_kernel::DynamicValue::Record(std::collections::BTreeMap::from([
                    (
                        "start".to_owned(),
                        artist_kernel::DynamicValue::String("missing".to_owned()),
                    ),
                    ("end".to_owned(), artist_kernel::DynamicValue::Option(None)),
                    (
                        "content".to_owned(),
                        artist_kernel::DynamicValue::String(String::new()),
                    ),
                ])),
            )
            .await;
        assert!(matches!(
            result,
            Err(artist_kernel::KernelError::UnsupportedVerb { .. })
        ));
    }

    #[test]
    fn contract_ids_are_open_and_registry_discovery_is_not_a_closed_enum() {
        use std::str::FromStr;

        let read = contracts::ContractId::from_str("artist:tool:read@1").unwrap();
        assert_eq!(read, contracts::ContractId::universal("read"));
        assert_eq!(read.to_string(), "artist:tool:read@1");

        let mut registry = contract_registry::ContractRegistry::default();
        registry
            .register(contracts::ContractDescriptor::universal("read"))
            .unwrap();
        assert!(registry.resolve(&read).is_some());
        assert!(
            registry
                .register(contracts::ContractDescriptor::universal("read"))
                .is_err()
        );
    }

    #[test]
    fn accepts_and_registers_extension_contracts_beyond_universal_verbs() {
        use std::str::FromStr;

        let contract = contracts::ContractId::from_str("acme:format@2").unwrap();
        assert_eq!(contract.interface, "format");
        assert_eq!(contract.to_string(), "acme:format@2");

        let mut registry = contract_registry::ContractRegistry::default();
        registry
            .register(contracts::ContractDescriptor {
                id: contract.clone(),
                interface: "format".to_owned(),
                imports: Vec::new(),
            })
            .unwrap();
        assert!(registry.resolve(&contract).is_some());
    }

    #[test]
    fn custom_contract_versions_match_component_import_names() {
        assert!(contract_matches_import(
            "acme:format@2",
            "acme:format/format@2.0.0"
        ));
        assert!(!contract_matches_import(
            "acme:format@2",
            "acme:format/format@3.0.0"
        ));
    }

    #[test]
    fn preserves_inherited_execution_context() {
        let context = contracts::ExecutionContext::inherited(
            Some("file:///workspace".to_owned()),
            Some("cancel-1".to_owned()),
            Some(1000),
            Some("trace-1".to_owned()),
        )
        .with_environment("MODE", "test");
        assert_eq!(
            context.environment,
            vec![("MODE".to_owned(), "test".to_owned())]
        );
    }
}
