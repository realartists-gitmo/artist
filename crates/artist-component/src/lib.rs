//! Host-side implementation of the Artist WebAssembly Component ABI.

/// Version-one tool contracts. These deliberately import the shared
/// `artist:resource` types.
pub mod tool_v1_bindings {
    wasmtime::component::bindgen!({
        path: "wit/tool-surface-v1",
        world: "read-world",
        additional_derives: [serde::Serialize, serde::Deserialize],
    });
}

macro_rules! tool_v1_world_bindings {
    ($module:ident, $world:literal) => {
        pub mod $module {
            wasmtime::component::bindgen!({
                path: "wit/tool-surface-v1",
                world: $world,
                additional_derives: [serde::Serialize, serde::Deserialize],
            });
        }
    };
}

tool_v1_world_bindings!(tool_v1_write_bindings, "write-world");
tool_v1_world_bindings!(tool_v1_edit_bindings, "edit-world");
tool_v1_world_bindings!(tool_v1_find_bindings, "find-world");
tool_v1_world_bindings!(tool_v1_grep_bindings, "grep-world");
tool_v1_world_bindings!(tool_v1_run_bindings, "run-world");
tool_v1_world_bindings!(tool_v1_send_bindings, "send-world");
tool_v1_world_bindings!(tool_v1_abort_bindings, "abort-world");
tool_v1_world_bindings!(tool_v1_delete_bindings, "delete-world");
tool_v1_world_bindings!(tool_v1_poll_bindings, "poll-world");

macro_rules! tool_v1_async_world_bindings {
    ($module:ident, $world:literal) => {
        pub mod $module {
            wasmtime::component::bindgen!({
                path: "wit/tool-surface-v1",
                world: $world,
                imports: { default: async },
                exports: { default: async },
                additional_derives: [serde::Serialize, serde::Deserialize],
            });
        }
    };
}

tool_v1_async_world_bindings!(tool_v1_async_tool_v1_bindings, "read-world");
tool_v1_async_world_bindings!(tool_v1_async_tool_v1_write_bindings, "write-world");
tool_v1_async_world_bindings!(tool_v1_async_tool_v1_edit_bindings, "edit-world");
tool_v1_async_world_bindings!(tool_v1_async_tool_v1_find_bindings, "find-world");
tool_v1_async_world_bindings!(tool_v1_async_tool_v1_grep_bindings, "grep-world");
tool_v1_async_world_bindings!(tool_v1_async_tool_v1_run_bindings, "run-world");
tool_v1_async_world_bindings!(tool_v1_async_tool_v1_send_bindings, "send-world");
tool_v1_async_world_bindings!(tool_v1_async_tool_v1_abort_bindings, "abort-world");
tool_v1_async_world_bindings!(tool_v1_async_tool_v1_delete_bindings, "delete-world");
tool_v1_async_world_bindings!(tool_v1_async_tool_v1_poll_bindings, "poll-world");

// The generated v1 worlds are the sole universal component ABI. These local
// aliases are only used by the typed adapter below while its per-verb match is
// collapsed to the v1 worlds.

/// Canonical shared resource contract. Resource extensions and model-facing
/// tools are required to converge on these types at their component boundary.
pub mod resource_bindings {
    wasmtime::component::bindgen!({
        path: "wit/resource-surface",
        world: "host-world",
        additional_derives: [serde::Serialize, serde::Deserialize],
    });
}

macro_rules! resource_world_bindings {
    ($module:ident, $world:literal) => {
        pub mod $module {
            wasmtime::component::bindgen!({
                path: "wit/resource-surface",
                world: $world,
                additional_derives: [serde::Serialize, serde::Deserialize],
            });
        }
    };
}

resource_world_bindings!(resource_tool_v1_bindings, "resource-read-world");
resource_world_bindings!(resource_tool_v1_write_bindings, "resource-write-world");
resource_world_bindings!(resource_tool_v1_edit_bindings, "resource-edit-world");
resource_world_bindings!(resource_tool_v1_find_bindings, "resource-find-world");
resource_world_bindings!(resource_tool_v1_grep_bindings, "resource-grep-world");
resource_world_bindings!(resource_tool_v1_run_bindings, "resource-run-world");
resource_world_bindings!(resource_tool_v1_send_bindings, "resource-send-world");
resource_world_bindings!(resource_tool_v1_abort_bindings, "resource-abort-world");
resource_world_bindings!(resource_tool_v1_delete_bindings, "resource-delete-world");
resource_world_bindings!(resource_tool_v1_poll_bindings, "resource-poll-world");
resource_world_bindings!(resource_extension_bindings, "extension-world");

pub mod resource_async_tool_v1_bindings {
    wasmtime::component::bindgen!({
        path: "wit/resource-surface",
        world: "resource-read-world",
        imports: { default: async },
        exports: { default: async },
        additional_derives: [serde::Serialize, serde::Deserialize],
    });
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

/// Async host bindings used by every component path that can make nested
/// resource calls. Keeping this separate from the synchronous native adapter
/// prevents component-to-kernel calls from blocking an executor.
pub mod resource_async_host_bindings {
    wasmtime::component::bindgen!({
        path: "wit/resource-surface",
        world: "host-world",
        imports: { default: async },
        exports: { default: async },
        additional_derives: [serde::Serialize, serde::Deserialize],
    });
}

macro_rules! resource_async_world_bindings {
    ($module:ident, $world:literal) => {
        pub mod $module {
            wasmtime::component::bindgen!({
                path: "wit/resource-surface",
                world: $world,
                imports: { default: async },
                exports: { default: async },
                additional_derives: [serde::Serialize, serde::Deserialize],
            });
        }
    };
}

resource_async_world_bindings!(
    resource_async_tool_v1_write_bindings,
    "resource-write-world"
);
resource_async_world_bindings!(resource_async_tool_v1_edit_bindings, "resource-edit-world");
resource_async_world_bindings!(resource_async_tool_v1_find_bindings, "resource-find-world");
resource_async_world_bindings!(resource_async_tool_v1_grep_bindings, "resource-grep-world");
resource_async_world_bindings!(resource_async_tool_v1_run_bindings, "resource-run-world");
resource_async_world_bindings!(resource_async_tool_v1_send_bindings, "resource-send-world");
resource_async_world_bindings!(
    resource_async_tool_v1_abort_bindings,
    "resource-abort-world"
);
resource_async_world_bindings!(
    resource_async_tool_v1_delete_bindings,
    "resource-delete-world"
);
resource_async_world_bindings!(resource_async_tool_v1_poll_bindings, "resource-poll-world");

/// Stable identities for the universal typed tool contracts.
pub mod contracts {
    use serde::{Deserialize, Serialize};
    use std::{fmt, str::FromStr};

    #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
    #[serde(rename_all = "lowercase")]
    pub enum Verb {
        Read,
        Write,
        Edit,
        Poll,
        Send,
        Run,
        Abort,
        Delete,
        Find,
        Grep,
    }

    impl Verb {
        pub const ALL: [Self; 10] = [
            Self::Read,
            Self::Write,
            Self::Edit,
            Self::Poll,
            Self::Send,
            Self::Run,
            Self::Abort,
            Self::Delete,
            Self::Find,
            Self::Grep,
        ];

        pub const fn interface(self) -> &'static str {
            match self {
                Self::Read => "read",
                Self::Write => "write",
                Self::Edit => "edit",
                Self::Poll => "poll",
                Self::Send => "send",
                Self::Run => "run",
                Self::Abort => "abort",
                Self::Delete => "delete",
                Self::Find => "find",
                Self::Grep => "grep",
            }
        }
    }

    impl fmt::Display for Verb {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.interface())
        }
    }

    #[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
    pub struct ContractId {
        pub namespace: String,
        /// Universal verbs have a typed adapter; extension contracts may use
        /// any interface name and are retained for dynamic registration.
        pub interface: String,
        pub verb: Option<Verb>,
        pub major: u16,
    }

    impl ContractId {
        pub const NAMESPACE: &'static str = "artist:tool";
        pub const MAJOR: u16 = 1;

        pub fn universal(verb: Verb) -> Self {
            Self {
                namespace: Self::NAMESPACE.to_owned(),
                interface: verb.interface().to_owned(),
                verb: Some(verb),
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
            let universal = Verb::ALL
                .into_iter()
                .find(|candidate| candidate.interface() == verb);
            Ok(Self {
                namespace: namespace.to_owned(),
                interface: verb.to_owned(),
                verb: universal,
                major,
            })
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
        pub fn universal(verb: Verb) -> Self {
            Self {
                id: ContractId::universal(verb),
                interface: verb.interface().to_owned(),
                imports: Vec::new(),
            }
        }
    }

    pub fn universal_contracts() -> Vec<ContractDescriptor> {
        Verb::ALL
            .into_iter()
            .map(ContractDescriptor::universal)
            .collect()
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum PollAtom {
        Changed { target: usize },
        Regex { target: usize, pattern: String },
        Terminated { target: usize },
        Timeout { milliseconds: u64 },
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum PollCondition {
        Atom(PollAtom),
        All(Vec<PollCondition>),
        Any(Vec<PollCondition>),
    }

    pub fn validate_poll_condition(
        condition: &PollCondition,
        target_count: usize,
        termination_supported: &[bool],
    ) -> Result<(), String> {
        if termination_supported.len() != target_count {
            return Err("termination capability table does not match targets".to_owned());
        }
        fn walk(
            condition: &PollCondition,
            target_count: usize,
            termination_supported: &[bool],
        ) -> Result<(), String> {
            match condition {
                PollCondition::Atom(PollAtom::Changed { target })
                | PollCondition::Atom(PollAtom::Regex { target, .. }) => {
                    if *target >= target_count {
                        return Err(format!("poll target {target} is out of range"));
                    }
                }
                PollCondition::Atom(PollAtom::Terminated { target }) => {
                    if *target >= target_count {
                        return Err(format!("poll target {target} is out of range"));
                    }
                    if !termination_supported[*target] {
                        return Err(format!("poll target {target} does not support termination"));
                    }
                }
                PollCondition::Atom(PollAtom::Timeout { .. }) => {}
                PollCondition::All(children) | PollCondition::Any(children) => {
                    if children.is_empty() {
                        return Err("poll boolean conditions cannot be empty".to_owned());
                    }
                    for child in children {
                        walk(child, target_count, termination_supported)?;
                    }
                }
            }
            Ok(())
        }
        walk(condition, target_count, termination_supported)
    }
}

use artist_kernel::KernelHandle;
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

/// Inspect a validated component without binding it to a closed world. The
/// `implements` annotation is the authoritative versioned contract identity;
/// callers can compare it with a discovered `VerbId` before publication.
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

impl resource_bindings::artist::resource::read::Host for HostState {
    fn read(
        &mut self,
        requests: Vec<resource_bindings::artist::resource::types::ReadRequest>,
    ) -> Vec<
        Result<
            resource_bindings::artist::resource::types::ReadResult,
            resource_bindings::artist::resource::types::Error,
        >,
    > {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<resource_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                let at = request.at.map(|position| match position {
                    resource_bindings::artist::resource::types::Position::Top => {
                        artist_kernel::Position::Top
                    }
                    resource_bindings::artist::resource::types::Position::Bottom => {
                        artist_kernel::Position::Bottom
                    }
                    resource_bindings::artist::resource::types::Position::At(anchor) => {
                        artist_kernel::Position::At(anchor_from_string(&anchor))
                    }
                });
                typed_host_invoke_operation(
                    self,
                    "read",
                    Some(target),
                    artist_kernel::Operation::Read(vec![artist_kernel::ReadRequest {
                        uri,
                        at,
                        before: request.before,
                        after: request.after,
                    }]),
                )
            })
            .collect()
    }
}
impl resource_bindings::artist::resource::write::Host for HostState {
    fn write(
        &mut self,
        requests: Vec<resource_bindings::artist::resource::types::WriteRequest>,
    ) -> Vec<
        Result<
            resource_bindings::artist::resource::types::WriteResult,
            resource_bindings::artist::resource::types::Error,
        >,
    > {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<resource_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                typed_host_invoke_operation(
                    self,
                    "write",
                    Some(target),
                    artist_kernel::Operation::Write(vec![artist_kernel::WriteRequest {
                        uri,
                        content: request.content,
                    }]),
                )
            })
            .collect()
    }
}
impl resource_bindings::artist::resource::edit::Host for HostState {
    fn edit(
        &mut self,
        requests: Vec<resource_bindings::artist::resource::types::EditRequest>,
    ) -> Vec<
        Result<
            resource_bindings::artist::resource::types::EditResult,
            resource_bindings::artist::resource::types::Error,
        >,
    > {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<resource_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                let operations = request
                    .operations
                    .into_iter()
                    .map(|operation| match operation {
                        resource_bindings::artist::resource::types::EditOperation::Replace(replace) => {
                            Ok(artist_kernel::EditOperation::Replace(
                                artist_kernel::ReplaceOperation {
                                    start: anchor_from_string(&replace.start),
                                    end: replace.end.as_deref().map(anchor_from_string),
                                    content: replace.content,
                                },
                            ))
                        }
                        resource_bindings::artist::resource::types::EditOperation::Insert(insert) => Ok(
                            artist_kernel::EditOperation::Insert(artist_kernel::InsertOperation {
                                at: match insert.at {
                                    resource_bindings::artist::resource::types::InsertionPoint::Top => {
                                        artist_kernel::InsertionPoint::Top
                                    }
                                    resource_bindings::artist::resource::types::InsertionPoint::Bottom => {
                                        artist_kernel::InsertionPoint::Bottom
                                    }
                                    resource_bindings::artist::resource::types::InsertionPoint::Before(
                                        anchor,
                                    ) => artist_kernel::InsertionPoint::Before(anchor_from_string(
                                        &anchor,
                                    )),
                                    resource_bindings::artist::resource::types::InsertionPoint::After(
                                        anchor,
                                    ) => artist_kernel::InsertionPoint::After(anchor_from_string(
                                        &anchor,
                                    )),
                                },
                                content: insert.content,
                            }),
                        ),
                    })
                    .collect::<Result<Vec<_>, resource_bindings::artist::resource::types::Error>>()?;
                typed_host_invoke_operation(
                    self,
                    "edit",
                    Some(target),
                    artist_kernel::Operation::Edit(vec![artist_kernel::EditRequest {
                        uri,
                        operations,
                    }]),
                )
            })
            .collect()
    }
}
impl resource_bindings::artist::resource::run::Host for HostState {
    fn run(
        &mut self,
        requests: Vec<resource_bindings::artist::resource::types::RunRequest>,
    ) -> Vec<Result<String, resource_bindings::artist::resource::types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<resource_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                typed_host_invoke_operation(
                    self,
                    "run",
                    Some(target),
                    artist_kernel::Operation::Run(vec![artist_kernel::RunRequest {
                        uri,
                        args: request.args,
                    }]),
                )
            })
            .collect()
    }
}

impl resource_bindings::artist::resource::send::Host for HostState {
    fn send(
        &mut self,
        requests: Vec<resource_bindings::artist::resource::types::SendRequest>,
    ) -> Vec<Result<String, resource_bindings::artist::resource::types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<resource_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                typed_host_invoke_operation(
                    self,
                    "send",
                    Some(target),
                    artist_kernel::Operation::Send(vec![artist_kernel::SendRequest {
                        uri,
                        content: request.content,
                    }]),
                )
            })
            .collect()
    }
}

impl resource_bindings::artist::resource::find::Host for HostState {
    fn find(
        &mut self,
        request: resource_bindings::artist::resource::types::FindRequest,
    ) -> Result<Vec<String>, resource_bindings::artist::resource::types::Error> {
        let roots = request
            .roots
            .iter()
            .map(|root| {
                artist_kernel::ResourceUri::parse(root).map_err(|error| {
                    typed_error_for::<resource_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(root.clone()),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        typed_host_invoke_operation(
            self,
            "find",
            roots.first().map(ToString::to_string),
            artist_kernel::Operation::Find(artist_kernel::FindRequest {
                roots,
                query: request.query,
            }),
        )
    }
}
impl resource_bindings::artist::resource::grep::Host for HostState {
    fn grep(
        &mut self,
        request: resource_bindings::artist::resource::types::GrepRequest,
    ) -> Result<
        Vec<resource_bindings::artist::resource::types::AnchoredText>,
        resource_bindings::artist::resource::types::Error,
    > {
        let source = match request.source {
            resource_bindings::artist::resource::types::GrepSource::Resources(uris) => {
                let uris = uris.iter().map(|uri| artist_kernel::ResourceUri::parse(uri).map_err(|error| typed_error_for::<resource_bindings::artist::resource::types::Error>(tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri, error.to_string(), Some(uri.clone())))).collect::<Result<Vec<_>, _>>()?;
                artist_kernel::GrepSource::Resources(uris)
            }
            resource_bindings::artist::resource::types::GrepSource::Text(texts) => artist_kernel::GrepSource::Text(texts.into_iter().map(|text| -> Result<artist_kernel::AnchoredText, resource_bindings::artist::resource::types::Error> { Ok(artist_kernel::AnchoredText {
                uri: artist_kernel::ResourceUri::parse(&text.uri).map_err(|error| typed_error_for::<resource_bindings::artist::resource::types::Error>(tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri, error.to_string(), Some(text.uri.clone())))?,
                lines: text.lines.into_iter().map(|line| artist_kernel::AnchoredLine {
                    anchor: anchor_from_string(&line.anchor),
                    text: line.text,
                    ending: match line.ending {
                        resource_bindings::artist::resource::types::LineEnding::None => artist_kernel::LineEnding::None,
                        resource_bindings::artist::resource::types::LineEnding::Lf => artist_kernel::LineEnding::Lf,
                        resource_bindings::artist::resource::types::LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                        resource_bindings::artist::resource::types::LineEnding::Cr => artist_kernel::LineEnding::Cr,
                    },
                }).collect(),
            }) }).collect::<Result<Vec<_>, _>>()?),
        };
        let target = match &source {
            artist_kernel::GrepSource::Resources(uris) => uris.first().map(ToString::to_string),
            artist_kernel::GrepSource::Text(texts) => {
                texts.first().map(|text| text.uri.to_string())
            }
        };
        typed_host_invoke_operation(
            self,
            "grep",
            target,
            artist_kernel::Operation::Grep(artist_kernel::GrepRequest {
                pattern: request.pattern,
                source,
            }),
        )
    }
}
impl resource_bindings::artist::resource::poll::Host for HostState {
    fn poll(
        &mut self,
        request: resource_bindings::artist::resource::types::PollRequest,
    ) -> Result<
        resource_bindings::artist::resource::types::PollResult,
        resource_bindings::artist::resource::types::Error,
    > {
        let targets = request
            .targets
            .iter()
            .map(|target| {
                let uri = artist_kernel::ResourceUri::parse(&target.uri).map_err(|error| {
                    typed_error_for::<resource_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.uri.clone()),
                    )
                })?;
                let from_position = target.from_position.clone().map(|position| match position {
                    resource_bindings::artist::resource::types::Position::Top => {
                        artist_kernel::Position::Top
                    }
                    resource_bindings::artist::resource::types::Position::Bottom => {
                        artist_kernel::Position::Bottom
                    }
                    resource_bindings::artist::resource::types::Position::At(anchor) => {
                        artist_kernel::Position::At(anchor_from_string(&anchor))
                    }
                });
                Ok(artist_kernel::PollTarget { uri, from_position })
            })
            .collect::<Result<Vec<_>, resource_bindings::artist::resource::types::Error>>()?;
        let until = request.until.map(lower_poll_wire).transpose()?;
        let target = targets.first().map(|target| target.uri.to_string());
        typed_host_invoke_operation(
            self,
            "poll",
            target,
            artist_kernel::Operation::Poll(artist_kernel::PollRequest { targets, until }),
        )
    }
}

fn lower_poll_wire(
    wire: resource_bindings::artist::resource::types::PollConditionWire,
) -> Result<artist_kernel::PollCondition, resource_bindings::artist::resource::types::Error> {
    fn lower(
        index: usize,
        nodes: &[resource_bindings::artist::resource::types::PollNode],
    ) -> Result<artist_kernel::PollCondition, resource_bindings::artist::resource::types::Error>
    {
        let node = nodes.get(index).ok_or_else(|| {
            typed_error_for::<resource_bindings::artist::resource::types::Error>(
                tool_v1_bindings::artist::resource::types::ErrorCode::InvalidInput,
                format!("poll node {index} is out of range"),
                None,
            )
        })?;
        Ok(match node {
            resource_bindings::artist::resource::types::PollNode::Atom(atom) => {
                artist_kernel::PollCondition::Atom(match atom {
                    resource_bindings::artist::resource::types::PollAtom::Changed(lines) => {
                        artist_kernel::PollAtom::Changed(*lines)
                    }
                    resource_bindings::artist::resource::types::PollAtom::Regex(regex) => {
                        artist_kernel::PollAtom::Regex(artist_kernel::RegexAtom {
                            target: regex.target,
                            pattern: regex.pattern.clone(),
                        })
                    }
                    resource_bindings::artist::resource::types::PollAtom::Terminated(target) => {
                        artist_kernel::PollAtom::Terminated(*target)
                    }
                    resource_bindings::artist::resource::types::PollAtom::Timeout(milliseconds) => {
                        artist_kernel::PollAtom::Timeout(*milliseconds)
                    }
                })
            }
            resource_bindings::artist::resource::types::PollNode::All(children) => {
                artist_kernel::PollCondition::All(
                    children
                        .iter()
                        .map(|child| lower(*child as usize, nodes))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            resource_bindings::artist::resource::types::PollNode::Any(children) => {
                artist_kernel::PollCondition::Any(
                    children
                        .iter()
                        .map(|child| lower(*child as usize, nodes))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
        })
    }
    lower(wire.root as usize, &wire.nodes)
}

impl resource_bindings::artist::resource::abort::Host for HostState {
    fn abort(
        &mut self,
        uris: Vec<String>,
    ) -> Vec<Result<String, resource_bindings::artist::resource::types::Error>> {
        uris.into_iter()
            .map(|uri| {
                let target = uri.clone();
                let parsed = artist_kernel::ResourceUri::parse(&uri).map_err(|error| {
                    typed_error_for::<resource_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                typed_host_invoke_operation(
                    self,
                    "abort",
                    Some(target),
                    artist_kernel::Operation::Abort(vec![parsed]),
                )
            })
            .collect()
    }
}

impl resource_bindings::artist::resource::delete::Host for HostState {
    fn delete(
        &mut self,
        uris: Vec<String>,
    ) -> Vec<Result<String, resource_bindings::artist::resource::types::Error>> {
        uris.into_iter()
            .map(|uri| {
                let target = uri.clone();
                let parsed = artist_kernel::ResourceUri::parse(&uri).map_err(|error| {
                    typed_error_for::<resource_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                typed_host_invoke_operation(
                    self,
                    "delete",
                    Some(target),
                    artist_kernel::Operation::Delete(vec![parsed]),
                )
            })
            .collect()
    }
}

impl resource_bindings::artist::resource::types::Host for HostState {}

impl resource_async_host_bindings::artist::resource::read::Host for HostState {
    async fn read(
        &mut self,
        requests: Vec<resource_async_host_bindings::artist::resource::types::ReadRequest>,
    ) -> Vec<
        Result<
            resource_async_host_bindings::artist::resource::types::ReadResult,
            resource_async_host_bindings::artist::resource::types::Error,
        >,
    > {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let target = request.uri.clone();
            let result = (|| {
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<resource_async_host_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                let at = request.at.map(|position| match position {
                    resource_async_host_bindings::artist::resource::types::Position::Top => {
                        artist_kernel::Position::Top
                    }
                    resource_async_host_bindings::artist::resource::types::Position::Bottom => {
                        artist_kernel::Position::Bottom
                    }
                    resource_async_host_bindings::artist::resource::types::Position::At(anchor) => {
                        artist_kernel::Position::At(anchor_from_string(&anchor))
                    }
                });
                Ok((uri, at, request.before, request.after))
            })();
            let result = match result {
                Ok((uri, at, before, after)) => {
                    typed_host_invoke_operation_async(
                        self,
                        "read",
                        Some(target),
                        artist_kernel::Operation::Read(vec![artist_kernel::ReadRequest {
                            uri,
                            at,
                            before,
                            after,
                        }]),
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            results.push(result);
        }
        results
    }
}

impl resource_async_host_bindings::artist::resource::write::Host for HostState {
    async fn write(
        &mut self,
        requests: Vec<resource_async_host_bindings::artist::resource::types::WriteRequest>,
    ) -> Vec<
        Result<
            resource_async_host_bindings::artist::resource::types::WriteResult,
            resource_async_host_bindings::artist::resource::types::Error,
        >,
    > {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let target = request.uri.clone();
            let result = match artist_kernel::ResourceUri::parse(&target) {
                Ok(uri) => {
                    typed_host_invoke_operation_async(
                        self,
                        "write",
                        Some(target),
                        artist_kernel::Operation::Write(vec![artist_kernel::WriteRequest {
                            uri,
                            content: request.content,
                        }]),
                    )
                    .await
                }
                Err(error) => Err(typed_error_for::<
                    resource_async_host_bindings::artist::resource::types::Error,
                >(
                    tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                    error.to_string(),
                    Some(target),
                )),
            };
            results.push(result);
        }
        results
    }
}

impl resource_async_host_bindings::artist::resource::run::Host for HostState {
    async fn run(
        &mut self,
        requests: Vec<resource_async_host_bindings::artist::resource::types::RunRequest>,
    ) -> Vec<Result<String, resource_async_host_bindings::artist::resource::types::Error>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let target = request.uri.clone();
            let result = match artist_kernel::ResourceUri::parse(&target) {
                Ok(uri) => {
                    typed_host_invoke_operation_async(
                        self,
                        "run",
                        Some(target),
                        artist_kernel::Operation::Run(vec![artist_kernel::RunRequest {
                            uri,
                            args: request.args,
                        }]),
                    )
                    .await
                }
                Err(error) => Err(typed_error_for::<
                    resource_async_host_bindings::artist::resource::types::Error,
                >(
                    tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                    error.to_string(),
                    Some(target),
                )),
            };
            results.push(result);
        }
        results
    }
}

impl resource_async_host_bindings::artist::resource::edit::Host for HostState {
    async fn edit(
        &mut self,
        requests: Vec<resource_async_host_bindings::artist::resource::types::EditRequest>,
    ) -> Vec<
        Result<
            resource_async_host_bindings::artist::resource::types::EditResult,
            resource_async_host_bindings::artist::resource::types::Error,
        >,
    > {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let target = request.uri.clone();
            let parsed = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                typed_error_for::<resource_async_host_bindings::artist::resource::types::Error>(
                    tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                    error.to_string(),
                    Some(target.clone()),
                )
            });
            let result = match parsed {
                Ok(uri) => {
                    let operations = request
                        .operations
                        .into_iter()
                        .map(|operation| match operation {
                            resource_async_host_bindings::artist::resource::types::EditOperation::Replace(replace) => {
                                artist_kernel::EditOperation::Replace(artist_kernel::ReplaceOperation {
                                    start: anchor_from_string(&replace.start),
                                    end: replace.end.as_deref().map(anchor_from_string),
                                    content: replace.content,
                                })
                            }
                            resource_async_host_bindings::artist::resource::types::EditOperation::Insert(insert) => {
                                artist_kernel::EditOperation::Insert(artist_kernel::InsertOperation {
                                    at: match insert.at {
                                        resource_async_host_bindings::artist::resource::types::InsertionPoint::Top => artist_kernel::InsertionPoint::Top,
                                        resource_async_host_bindings::artist::resource::types::InsertionPoint::Bottom => artist_kernel::InsertionPoint::Bottom,
                                        resource_async_host_bindings::artist::resource::types::InsertionPoint::Before(anchor) => artist_kernel::InsertionPoint::Before(anchor_from_string(&anchor)),
                                        resource_async_host_bindings::artist::resource::types::InsertionPoint::After(anchor) => artist_kernel::InsertionPoint::After(anchor_from_string(&anchor)),
                                    },
                                    content: insert.content,
                                })
                            }
                        })
                        .collect();
                    typed_host_invoke_operation_async(
                        self,
                        "edit",
                        Some(target),
                        artist_kernel::Operation::Edit(vec![artist_kernel::EditRequest {
                            uri,
                            operations,
                        }]),
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            results.push(result);
        }
        results
    }
}

impl resource_async_host_bindings::artist::resource::send::Host for HostState {
    async fn send(
        &mut self,
        requests: Vec<resource_async_host_bindings::artist::resource::types::SendRequest>,
    ) -> Vec<Result<String, resource_async_host_bindings::artist::resource::types::Error>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let target = request.uri.clone();
            let result = match artist_kernel::ResourceUri::parse(&target) {
                Ok(uri) => {
                    typed_host_invoke_operation_async(
                        self,
                        "send",
                        Some(target),
                        artist_kernel::Operation::Send(vec![artist_kernel::SendRequest {
                            uri,
                            content: request.content,
                        }]),
                    )
                    .await
                }
                Err(error) => Err(typed_error_for::<
                    resource_async_host_bindings::artist::resource::types::Error,
                >(
                    tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                    error.to_string(),
                    Some(target),
                )),
            };
            results.push(result);
        }
        results
    }
}

impl resource_async_host_bindings::artist::resource::find::Host for HostState {
    async fn find(
        &mut self,
        request: resource_async_host_bindings::artist::resource::types::FindRequest,
    ) -> Result<Vec<String>, resource_async_host_bindings::artist::resource::types::Error> {
        let target = request.roots.first().cloned();
        let roots = request
            .roots
            .iter()
            .map(|root| {
                artist_kernel::ResourceUri::parse(root).map_err(|error| {
                    typed_error_for::<resource_async_host_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(root.clone()),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        typed_host_invoke_operation_async(
            self,
            "find",
            target,
            artist_kernel::Operation::Find(artist_kernel::FindRequest {
                roots,
                query: request.query,
            }),
        )
        .await
    }
}

impl resource_async_host_bindings::artist::resource::grep::Host for HostState {
    async fn grep(
        &mut self,
        request: resource_async_host_bindings::artist::resource::types::GrepRequest,
    ) -> Result<
        Vec<resource_async_host_bindings::artist::resource::types::AnchoredText>,
        resource_async_host_bindings::artist::resource::types::Error,
    > {
        let source = match request.source {
            resource_async_host_bindings::artist::resource::types::GrepSource::Resources(uris) => {
                let parsed = uris
                    .iter()
                    .map(|uri| {
                        artist_kernel::ResourceUri::parse(uri).map_err(|error| {
                            typed_error_for::<
                                resource_async_host_bindings::artist::resource::types::Error,
                            >(
                                tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                                error.to_string(),
                                Some(uri.clone()),
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                artist_kernel::GrepSource::Resources(parsed)
            }
            resource_async_host_bindings::artist::resource::types::GrepSource::Text(texts) => {
                let parsed = texts
                    .into_iter()
                    .map(|text| {
                        let uri = artist_kernel::ResourceUri::parse(&text.uri).map_err(|error| {
                            typed_error_for::<resource_async_host_bindings::artist::resource::types::Error>(
                                tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                                error.to_string(),
                                Some(text.uri.clone()),
                            )
                        })?;
                        let lines = text
                            .lines
                            .into_iter()
                            .map(|line| artist_kernel::AnchoredLine {
                                anchor: anchor_from_string(&line.anchor),
                                text: line.text,
                                ending: match line.ending {
                                    resource_async_host_bindings::artist::resource::types::LineEnding::None => artist_kernel::LineEnding::None,
                                    resource_async_host_bindings::artist::resource::types::LineEnding::Lf => artist_kernel::LineEnding::Lf,
                                    resource_async_host_bindings::artist::resource::types::LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                                    resource_async_host_bindings::artist::resource::types::LineEnding::Cr => artist_kernel::LineEnding::Cr,
                                },
                            })
                            .collect();
                        Ok(artist_kernel::AnchoredText { uri, lines })
                    })
                    .collect::<Result<Vec<_>, resource_async_host_bindings::artist::resource::types::Error>>()?;
                artist_kernel::GrepSource::Text(parsed)
            }
        };
        let target = match &source {
            artist_kernel::GrepSource::Resources(uris) => uris.first().map(ToString::to_string),
            artist_kernel::GrepSource::Text(texts) => {
                texts.first().map(|text| text.uri.to_string())
            }
        };
        typed_host_invoke_operation_async(
            self,
            "grep",
            target,
            artist_kernel::Operation::Grep(artist_kernel::GrepRequest {
                pattern: request.pattern,
                source,
            }),
        )
        .await
    }
}

impl resource_async_host_bindings::artist::resource::abort::Host for HostState {
    async fn abort(
        &mut self,
        uris: Vec<String>,
    ) -> Vec<Result<String, resource_async_host_bindings::artist::resource::types::Error>> {
        let mut results = Vec::with_capacity(uris.len());
        for target in uris {
            let result = match artist_kernel::ResourceUri::parse(&target) {
                Ok(uri) => {
                    typed_host_invoke_operation_async(
                        self,
                        "abort",
                        Some(target),
                        artist_kernel::Operation::Abort(vec![uri]),
                    )
                    .await
                }
                Err(error) => Err(typed_error_for::<
                    resource_async_host_bindings::artist::resource::types::Error,
                >(
                    tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                    error.to_string(),
                    Some(target),
                )),
            };
            results.push(result);
        }
        results
    }
}

fn lower_poll_wire_async(
    wire: resource_async_host_bindings::artist::resource::types::PollConditionWire,
) -> Result<
    artist_kernel::PollCondition,
    resource_async_host_bindings::artist::resource::types::Error,
> {
    fn lower(
        index: usize,
        nodes: &[resource_async_host_bindings::artist::resource::types::PollNode],
    ) -> Result<
        artist_kernel::PollCondition,
        resource_async_host_bindings::artist::resource::types::Error,
    > {
        let node = nodes.get(index).ok_or_else(|| {
            typed_error_for::<resource_async_host_bindings::artist::resource::types::Error>(
                tool_v1_bindings::artist::resource::types::ErrorCode::InvalidInput,
                format!("poll node {index} is out of range"),
                None,
            )
        })?;
        Ok(match node {
            resource_async_host_bindings::artist::resource::types::PollNode::Atom(atom) => {
                artist_kernel::PollCondition::Atom(match atom {
                    resource_async_host_bindings::artist::resource::types::PollAtom::Changed(
                        lines,
                    ) => artist_kernel::PollAtom::Changed(*lines),
                    resource_async_host_bindings::artist::resource::types::PollAtom::Regex(
                        regex,
                    ) => artist_kernel::PollAtom::Regex(artist_kernel::RegexAtom {
                        target: regex.target,
                        pattern: regex.pattern.clone(),
                    }),
                    resource_async_host_bindings::artist::resource::types::PollAtom::Terminated(
                        target,
                    ) => artist_kernel::PollAtom::Terminated(*target),
                    resource_async_host_bindings::artist::resource::types::PollAtom::Timeout(
                        milliseconds,
                    ) => artist_kernel::PollAtom::Timeout(*milliseconds),
                })
            }
            resource_async_host_bindings::artist::resource::types::PollNode::All(children) => {
                artist_kernel::PollCondition::All(
                    children
                        .iter()
                        .map(|child| lower(*child as usize, nodes))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            resource_async_host_bindings::artist::resource::types::PollNode::Any(children) => {
                artist_kernel::PollCondition::Any(
                    children
                        .iter()
                        .map(|child| lower(*child as usize, nodes))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
        })
    }
    lower(wire.root as usize, &wire.nodes)
}

impl resource_async_host_bindings::artist::resource::poll::Host for HostState {
    async fn poll(
        &mut self,
        request: resource_async_host_bindings::artist::resource::types::PollRequest,
    ) -> Result<
        resource_async_host_bindings::artist::resource::types::PollResult,
        resource_async_host_bindings::artist::resource::types::Error,
    > {
        let target = request.targets.first().map(|target| target.uri.clone());
        let targets = request
            .targets
            .into_iter()
            .map(|target| {
                let uri = artist_kernel::ResourceUri::parse(&target.uri).map_err(|error| {
                    typed_error_for::<resource_async_host_bindings::artist::resource::types::Error>(
                        tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.uri.clone()),
                    )
                })?;
                let from_position = target.from_position.map(|position| match position {
                    resource_async_host_bindings::artist::resource::types::Position::Top => artist_kernel::Position::Top,
                    resource_async_host_bindings::artist::resource::types::Position::Bottom => artist_kernel::Position::Bottom,
                    resource_async_host_bindings::artist::resource::types::Position::At(anchor) => artist_kernel::Position::At(anchor_from_string(&anchor)),
                });
                Ok(artist_kernel::PollTarget { uri, from_position })
            })
            .collect::<Result<Vec<_>, resource_async_host_bindings::artist::resource::types::Error>>()?;
        let until = request.until.map(lower_poll_wire_async).transpose()?;
        typed_host_invoke_operation_async(
            self,
            "poll",
            target,
            artist_kernel::Operation::Poll(artist_kernel::PollRequest { targets, until }),
        )
        .await
    }
}

impl resource_async_host_bindings::artist::resource::types::Host for HostState {}

impl resource_async_host_bindings::artist::resource::delete::Host for HostState {
    async fn delete(
        &mut self,
        uris: Vec<String>,
    ) -> Vec<Result<String, resource_async_host_bindings::artist::resource::types::Error>> {
        let mut results = Vec::with_capacity(uris.len());
        for target in uris {
            let result = match artist_kernel::ResourceUri::parse(&target) {
                Ok(uri) => {
                    typed_host_invoke_operation_async(
                        self,
                        "delete",
                        Some(target),
                        artist_kernel::Operation::Delete(vec![uri]),
                    )
                    .await
                }
                Err(error) => Err(typed_error_for::<
                    resource_async_host_bindings::artist::resource::types::Error,
                >(
                    tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
                    error.to_string(),
                    Some(target),
                )),
            };
            results.push(result);
        }
        results
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

fn resource_error_to_kernel(
    error: resource_async_tool_v1_bindings::artist::resource::types::Error,
) -> artist_kernel::KernelError {
    use artist_kernel::KernelError;
    let code = format!("{:?}", error.code);
    let uri = error.uri.unwrap_or_default();
    match code.as_str() {
        "InvalidUri" => KernelError::InvalidUri {
            message: error.message,
        },
        "InvalidInput" => KernelError::InvalidRequest {
            message: error.message,
        },
        "InvalidPattern" => KernelError::InvalidPattern {
            message: error.message,
        },
        "NotFound" => KernelError::NotFound { uri },
        "WrongKind" => KernelError::WrongKind {
            message: error.message,
        },
        "InvalidAnchor" => KernelError::InvalidAnchor {
            message: error.message,
        },
        "StaleAnchor" => KernelError::StaleAnchor {
            message: error.message,
        },
        "Immutable" => KernelError::Immutable { uri },
        "Unsupported" => KernelError::UnsupportedVerb {
            verb: "read".to_owned(),
            uri,
        },
        "PermissionDenied" => KernelError::PermissionDenied { uri },
        "Conflict" => KernelError::Conflict { uri },
        "NotEmpty" => KernelError::NotEmpty { uri },
        "Aborted" => KernelError::Aborted {
            message: error.message,
        },
        _ => KernelError::Handler {
            message: error.message,
        },
    }
}

fn resource_read_result_to_kernel(
    result: resource_async_tool_v1_bindings::artist::resource::types::ReadResult,
) -> Result<artist_kernel::ReadResult, artist_kernel::KernelError> {
    use resource_async_tool_v1_bindings::artist::resource::types::{LineEnding, ReadResult};
    match result {
        ReadResult::Text(text) => Ok(artist_kernel::ReadResult::Text(
            artist_kernel::AnchoredText {
                uri: artist_kernel::ResourceUri::parse(&text.uri).map_err(|error| {
                    artist_kernel::KernelError::InvalidUri {
                        message: error.to_string(),
                    }
                })?,
                lines: text
                    .lines
                    .into_iter()
                    .map(|line| artist_kernel::AnchoredLine {
                        anchor: anchor_from_string(&line.anchor),
                        text: line.text,
                        ending: match line.ending {
                            LineEnding::None => artist_kernel::LineEnding::None,
                            LineEnding::Lf => artist_kernel::LineEnding::Lf,
                            LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                            LineEnding::Cr => artist_kernel::LineEnding::Cr,
                        },
                    })
                    .collect(),
            },
        )),
        ReadResult::Directory(directory) => Ok(artist_kernel::ReadResult::Directory {
            uri: artist_kernel::ResourceUri::parse(&directory.uri).map_err(|error| {
                artist_kernel::KernelError::InvalidUri {
                    message: error.to_string(),
                }
            })?,
            entries: directory
                .entries
                .into_iter()
                .map(|uri| {
                    artist_kernel::ResourceUri::parse(&uri).map_err(|error| {
                        artist_kernel::KernelError::InvalidUri {
                            message: error.to_string(),
                        }
                    })
                })
                .collect::<Result<_, _>>()?,
        }),
    }
}

fn resource_write_result_to_kernel(
    result: resource_async_tool_v1_write_bindings::artist::resource::types::WriteResult,
) -> Result<artist_kernel::WriteResult, artist_kernel::KernelError> {
    Ok(artist_kernel::WriteResult {
        text: artist_kernel::AnchoredText {
            uri: artist_kernel::ResourceUri::parse(&result.text.uri).map_err(|error| {
                artist_kernel::KernelError::InvalidUri { message: error.to_string() }
            })?,
            lines: result
                .text
                .lines
                .into_iter()
                .map(|line| artist_kernel::AnchoredLine {
                    anchor: anchor_from_string(&line.anchor),
                    text: line.text,
                    ending: match line.ending {
                        resource_async_tool_v1_write_bindings::artist::resource::types::LineEnding::None => artist_kernel::LineEnding::None,
                        resource_async_tool_v1_write_bindings::artist::resource::types::LineEnding::Lf => artist_kernel::LineEnding::Lf,
                        resource_async_tool_v1_write_bindings::artist::resource::types::LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                        resource_async_tool_v1_write_bindings::artist::resource::types::LineEnding::Cr => artist_kernel::LineEnding::Cr,
                    },
                })
                .collect(),
        },
    })
}

fn resource_write_error_to_kernel(
    error: resource_async_tool_v1_write_bindings::artist::resource::types::Error,
) -> artist_kernel::KernelError {
    let code = format!("{:?}", error.code);
    let uri = error.uri.unwrap_or_default();
    match code.as_str() {
        "InvalidUri" => artist_kernel::KernelError::InvalidUri {
            message: error.message,
        },
        "InvalidInput" => artist_kernel::KernelError::InvalidRequest {
            message: error.message,
        },
        "NotFound" => artist_kernel::KernelError::NotFound { uri },
        "PermissionDenied" => artist_kernel::KernelError::PermissionDenied { uri },
        "Conflict" => artist_kernel::KernelError::Conflict { uri },
        "WrongKind" => artist_kernel::KernelError::WrongKind {
            message: error.message,
        },
        _ => artist_kernel::KernelError::Handler {
            message: error.message,
        },
    }
}

fn resource_error_to_kernel_for(
    error: resource_async_tool_v1_edit_bindings::artist::resource::types::Error,
    verb: &str,
) -> artist_kernel::KernelError {
    let code = format!("{:?}", error.code);
    let uri = error.uri.unwrap_or_default();
    match code.as_str() {
        "InvalidUri" => artist_kernel::KernelError::InvalidUri {
            message: error.message,
        },
        "InvalidInput" => artist_kernel::KernelError::InvalidRequest {
            message: error.message,
        },
        "InvalidPattern" => artist_kernel::KernelError::InvalidPattern {
            message: error.message,
        },
        "NotFound" => artist_kernel::KernelError::NotFound { uri },
        "WrongKind" => artist_kernel::KernelError::WrongKind {
            message: error.message,
        },
        "InvalidAnchor" => artist_kernel::KernelError::InvalidAnchor {
            message: error.message,
        },
        "StaleAnchor" => artist_kernel::KernelError::StaleAnchor {
            message: error.message,
        },
        "Immutable" => artist_kernel::KernelError::Immutable { uri },
        "Unsupported" => artist_kernel::KernelError::UnsupportedVerb {
            verb: verb.to_owned(),
            uri,
        },
        "PermissionDenied" => artist_kernel::KernelError::PermissionDenied { uri },
        "Conflict" => artist_kernel::KernelError::Conflict { uri },
        "NotEmpty" => artist_kernel::KernelError::NotEmpty { uri },
        "Aborted" => artist_kernel::KernelError::Aborted {
            message: error.message,
        },
        _ => artist_kernel::KernelError::Handler {
            message: error.message,
        },
    }
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

fn resource_edit_requests_from_kernel(
    requests: Vec<artist_kernel::EditRequest>,
) -> Result<
    Vec<resource_async_tool_v1_edit_bindings::artist::resource::types::EditRequest>,
    artist_kernel::KernelError,
> {
    use resource_async_tool_v1_edit_bindings::artist::resource::types::{
        EditOperation, EditRequest, InsertOperation, InsertionPoint, ReplaceOperation,
    };
    requests
        .into_iter()
        .map(|request| {
            Ok(EditRequest {
                uri: request.uri.to_string(),
                operations: request
                    .operations
                    .into_iter()
                    .map(|operation| match operation {
                        artist_kernel::EditOperation::Replace(operation) => {
                            EditOperation::Replace(ReplaceOperation {
                                start: operation.start.to_string(),
                                end: operation.end.map(|anchor| anchor.to_string()),
                                content: operation.content,
                            })
                        }
                        artist_kernel::EditOperation::Insert(operation) => {
                            EditOperation::Insert(InsertOperation {
                                at: match operation.at {
                                    artist_kernel::InsertionPoint::Top => InsertionPoint::Top,
                                    artist_kernel::InsertionPoint::Bottom => InsertionPoint::Bottom,
                                    artist_kernel::InsertionPoint::Before(anchor) => {
                                        InsertionPoint::Before(anchor.to_string())
                                    }
                                    artist_kernel::InsertionPoint::After(anchor) => {
                                        InsertionPoint::After(anchor.to_string())
                                    }
                                },
                                content: operation.content,
                            })
                        }
                    })
                    .collect(),
            })
        })
        .collect()
}

fn resource_edit_result_to_kernel(
    result: resource_async_tool_v1_edit_bindings::artist::resource::types::EditResult,
) -> Result<artist_kernel::EditResult, artist_kernel::KernelError> {
    use resource_async_tool_v1_edit_bindings::artist::resource::types::LineEnding;
    type KernelError = artist_kernel::KernelError;
    let text = result.text;
    let anchored_text =
        |text: resource_async_tool_v1_edit_bindings::artist::resource::types::AnchoredText| {
            Ok::<_, KernelError>(artist_kernel::AnchoredText {
                uri: artist_kernel::ResourceUri::parse(&text.uri).map_err(|error| {
                    KernelError::InvalidUri {
                        message: error.to_string(),
                    }
                })?,
                lines: text
                    .lines
                    .into_iter()
                    .map(|line| artist_kernel::AnchoredLine {
                        anchor: anchor_from_string(&line.anchor),
                        text: line.text,
                        ending: match line.ending {
                            LineEnding::None => artist_kernel::LineEnding::None,
                            LineEnding::Lf => artist_kernel::LineEnding::Lf,
                            LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                            LineEnding::Cr => artist_kernel::LineEnding::Cr,
                        },
                    })
                    .collect(),
            })
        };
    let text = anchored_text(text)?;
    let hunks = result
        .diff
        .hunks
        .into_iter()
        .map(|hunk| {
            Ok::<_, KernelError>(artist_kernel::DiffHunk {
                old: hunk
                    .old
                    .into_iter()
                    .map(|line| artist_kernel::AnchoredLine {
                        anchor: anchor_from_string(&line.anchor),
                        text: line.text,
                        ending: match line.ending {
                            LineEnding::None => artist_kernel::LineEnding::None,
                            LineEnding::Lf => artist_kernel::LineEnding::Lf,
                            LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                            LineEnding::Cr => artist_kernel::LineEnding::Cr,
                        },
                    })
                    .collect(),
                new: hunk
                    .new
                    .into_iter()
                    .map(|line| artist_kernel::AnchoredLine {
                        anchor: anchor_from_string(&line.anchor),
                        text: line.text,
                        ending: match line.ending {
                            LineEnding::None => artist_kernel::LineEnding::None,
                            LineEnding::Lf => artist_kernel::LineEnding::Lf,
                            LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                            LineEnding::Cr => artist_kernel::LineEnding::Cr,
                        },
                    })
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>, KernelError>>()?;
    Ok(artist_kernel::EditResult {
        text,
        diff: artist_kernel::AnchoredDiff {
            uri: artist_kernel::ResourceUri::parse(&result.diff.uri).map_err(|error| {
                KernelError::InvalidUri {
                    message: error.to_string(),
                }
            })?,
            hunks,
        },
    })
}

fn resource_run_requests_from_kernel(
    requests: Vec<artist_kernel::RunRequest>,
) -> Vec<resource_async_tool_v1_run_bindings::artist::resource::types::RunRequest> {
    requests
        .into_iter()
        .map(
            |request| resource_async_tool_v1_run_bindings::artist::resource::types::RunRequest {
                uri: request.uri.to_string(),
                args: request.args,
            },
        )
        .collect()
}

fn resource_send_requests_from_kernel(
    requests: Vec<artist_kernel::SendRequest>,
) -> Vec<resource_async_tool_v1_send_bindings::artist::resource::types::SendRequest> {
    requests
        .into_iter()
        .map(
            |request| resource_async_tool_v1_send_bindings::artist::resource::types::SendRequest {
                uri: request.uri.to_string(),
                content: request.content,
            },
        )
        .collect()
}

fn resource_uri_results_to_kernel(
    results: Vec<
        Result<String, resource_async_tool_v1_run_bindings::artist::resource::types::Error>,
    >,
    verb: &str,
) -> Result<
    Vec<Result<artist_kernel::ResourceUri, artist_kernel::KernelError>>,
    artist_kernel::KernelError,
> {
    results
        .into_iter()
        .map(|result| match result {
            Ok(uri) => artist_kernel::ResourceUri::parse(&uri)
                .map(Ok)
                .map_err(|error| artist_kernel::KernelError::InvalidUri {
                    message: error.to_string(),
                }),
            Err(error) => Ok(Err(resource_error_parts_to_kernel(
                format!("{:?}", error.code),
                error.uri,
                error.message,
                verb,
            ))),
        })
        .collect()
}

fn resource_send_results_to_kernel(
    results: Vec<
        Result<String, resource_async_tool_v1_send_bindings::artist::resource::types::Error>,
    >,
) -> Result<
    Vec<Result<artist_kernel::ResourceUri, artist_kernel::KernelError>>,
    artist_kernel::KernelError,
> {
    results
        .into_iter()
        .map(|result| match result {
            Ok(uri) => artist_kernel::ResourceUri::parse(&uri)
                .map(Ok)
                .map_err(|error| artist_kernel::KernelError::InvalidUri {
                    message: error.to_string(),
                }),
            Err(error) => Ok(Err(resource_error_parts_to_kernel(
                format!("{:?}", error.code),
                error.uri,
                error.message,
                "send",
            ))),
        })
        .collect()
}

fn resource_abort_results_to_kernel(
    results: Vec<
        Result<String, resource_async_tool_v1_abort_bindings::artist::resource::types::Error>,
    >,
) -> Result<
    Vec<Result<artist_kernel::ResourceUri, artist_kernel::KernelError>>,
    artist_kernel::KernelError,
> {
    results
        .into_iter()
        .map(|result| match result {
            Ok(uri) => artist_kernel::ResourceUri::parse(&uri)
                .map(Ok)
                .map_err(|error| artist_kernel::KernelError::InvalidUri {
                    message: error.to_string(),
                }),
            Err(error) => Ok(Err(resource_error_parts_to_kernel(
                format!("{:?}", error.code),
                error.uri,
                error.message,
                "abort",
            ))),
        })
        .collect()
}

fn resource_delete_results_to_kernel(
    results: Vec<
        Result<String, resource_async_tool_v1_delete_bindings::artist::resource::types::Error>,
    >,
) -> Result<
    Vec<Result<artist_kernel::ResourceUri, artist_kernel::KernelError>>,
    artist_kernel::KernelError,
> {
    results
        .into_iter()
        .map(|result| match result {
            Ok(uri) => artist_kernel::ResourceUri::parse(&uri)
                .map(Ok)
                .map_err(|error| artist_kernel::KernelError::InvalidUri {
                    message: error.to_string(),
                }),
            Err(error) => Ok(Err(resource_error_parts_to_kernel(
                format!("{:?}", error.code),
                error.uri,
                error.message,
                "delete",
            ))),
        })
        .collect()
}

fn resource_find_request_from_kernel(
    request: artist_kernel::FindRequest,
) -> resource_async_tool_v1_find_bindings::artist::resource::types::FindRequest {
    resource_async_tool_v1_find_bindings::artist::resource::types::FindRequest {
        roots: request
            .roots
            .into_iter()
            .map(|uri| uri.to_string())
            .collect(),
        query: request.query,
    }
}

fn resource_find_result_to_kernel(
    result: Result<
        Vec<String>,
        resource_async_tool_v1_find_bindings::artist::resource::types::Error,
    >,
) -> Result<Vec<artist_kernel::ResourceUri>, artist_kernel::KernelError> {
    match result {
        Ok(uris) => uris
            .into_iter()
            .map(|uri| {
                artist_kernel::ResourceUri::parse(&uri).map_err(|error| {
                    artist_kernel::KernelError::InvalidUri {
                        message: error.to_string(),
                    }
                })
            })
            .collect(),
        Err(error) => Err(resource_error_parts_to_kernel(
            format!("{:?}", error.code),
            error.uri,
            error.message,
            "find",
        )),
    }
}

fn resource_grep_request_from_kernel(
    request: artist_kernel::GrepRequest,
) -> resource_async_tool_v1_grep_bindings::artist::resource::types::GrepRequest {
    use resource_async_tool_v1_grep_bindings::artist::resource::types::{
        AnchoredLine, AnchoredText, GrepSource,
    };
    let source = match request.source {
        artist_kernel::GrepSource::Resources(uris) => {
            GrepSource::Resources(uris.into_iter().map(|uri| uri.to_string()).collect())
        }
        artist_kernel::GrepSource::Text(texts) => GrepSource::Text(
            texts
                .into_iter()
                .map(|text| AnchoredText {
                    uri: text.uri.to_string(),
                    lines: text
                        .lines
                        .into_iter()
                        .map(|line| AnchoredLine {
                            anchor: line.anchor.to_string(),
                            text: line.text,
                            ending: match line.ending {
                                artist_kernel::LineEnding::None => {
                                    resource_async_tool_v1_grep_bindings::artist::resource::types::LineEnding::None
                                }
                                artist_kernel::LineEnding::Lf => {
                                    resource_async_tool_v1_grep_bindings::artist::resource::types::LineEnding::Lf
                                }
                                artist_kernel::LineEnding::Crlf => {
                                    resource_async_tool_v1_grep_bindings::artist::resource::types::LineEnding::Crlf
                                }
                                artist_kernel::LineEnding::Cr => {
                                    resource_async_tool_v1_grep_bindings::artist::resource::types::LineEnding::Cr
                                }
                            },
                        })
                        .collect(),
                })
                .collect(),
        ),
    };
    resource_async_tool_v1_grep_bindings::artist::resource::types::GrepRequest {
        pattern: request.pattern,
        source,
    }
}

fn resource_grep_results_to_kernel(
    result: Result<
        Vec<resource_async_tool_v1_grep_bindings::artist::resource::types::AnchoredText>,
        resource_async_tool_v1_grep_bindings::artist::resource::types::Error,
    >,
) -> Result<Vec<artist_kernel::AnchoredText>, artist_kernel::KernelError> {
    match result {
        Ok(texts) => texts
            .into_iter()
            .map(|text| {
                Ok(artist_kernel::AnchoredText {
                    uri: artist_kernel::ResourceUri::parse(&text.uri).map_err(|error| {
                        artist_kernel::KernelError::InvalidUri {
                            message: error.to_string(),
                        }
                    })?,
                    lines: text
                        .lines
                        .into_iter()
                        .map(|line| artist_kernel::AnchoredLine {
                            anchor: anchor_from_string(&line.anchor),
                            text: line.text,
                            ending: match line.ending {
                                resource_async_tool_v1_grep_bindings::artist::resource::types::LineEnding::None => artist_kernel::LineEnding::None,
                                resource_async_tool_v1_grep_bindings::artist::resource::types::LineEnding::Lf => artist_kernel::LineEnding::Lf,
                                resource_async_tool_v1_grep_bindings::artist::resource::types::LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                                resource_async_tool_v1_grep_bindings::artist::resource::types::LineEnding::Cr => artist_kernel::LineEnding::Cr,
                            },
                        })
                        .collect(),
                })
            })
            .collect(),
        Err(error) => Err(resource_error_parts_to_kernel(
            format!("{:?}", error.code),
            error.uri,
            error.message,
            "grep",
        )),
    }
}

fn resource_poll_request_from_kernel(
    request: artist_kernel::PollRequest,
) -> resource_async_tool_v1_poll_bindings::artist::resource::types::PollRequest {
    use resource_async_tool_v1_poll_bindings::artist::resource::types::{PollTarget, Position};
    let targets = request
        .targets
        .into_iter()
        .map(|target| PollTarget {
            uri: target.uri.to_string(),
            from_position: target.from_position.map(|position| match position {
                artist_kernel::Position::Top => Position::Top,
                artist_kernel::Position::Bottom => Position::Bottom,
                artist_kernel::Position::At(anchor) => Position::At(anchor.to_string()),
            }),
        })
        .collect();
    let until = request.until.map(|condition| {
        let mut nodes = Vec::new();
        fn lower(
            condition: artist_kernel::PollCondition,
            nodes: &mut Vec<
                resource_async_tool_v1_poll_bindings::artist::resource::types::PollNode,
            >,
        ) -> u32 {
            use resource_async_tool_v1_poll_bindings::artist::resource::types::{
                PollAtom, PollNode, RegexAtom,
            };
            let node = match condition {
                artist_kernel::PollCondition::Atom(atom) => PollNode::Atom(match atom {
                    artist_kernel::PollAtom::Changed(target) => PollAtom::Changed(target),
                    artist_kernel::PollAtom::Regex(regex) => PollAtom::Regex(RegexAtom {
                        target: regex.target,
                        pattern: regex.pattern,
                    }),
                    artist_kernel::PollAtom::Terminated(target) => PollAtom::Terminated(target),
                    artist_kernel::PollAtom::Timeout(milliseconds) => {
                        PollAtom::Timeout(milliseconds)
                    }
                }),
                artist_kernel::PollCondition::All(children) => PollNode::All(
                    children
                        .into_iter()
                        .map(|child| lower(child, nodes))
                        .collect(),
                ),
                artist_kernel::PollCondition::Any(children) => PollNode::Any(
                    children
                        .into_iter()
                        .map(|child| lower(child, nodes))
                        .collect(),
                ),
            };
            let index = nodes.len() as u32;
            nodes.push(node);
            index
        }
        let root = lower(condition, &mut nodes);
        resource_async_tool_v1_poll_bindings::artist::resource::types::PollConditionWire {
            root,
            nodes,
        }
    });
    resource_async_tool_v1_poll_bindings::artist::resource::types::PollRequest { targets, until }
}

fn resource_poll_result_to_kernel(
    result: Result<
        resource_async_tool_v1_poll_bindings::artist::resource::types::PollResult,
        resource_async_tool_v1_poll_bindings::artist::resource::types::Error,
    >,
) -> Result<artist_kernel::PollResult, artist_kernel::KernelError> {
    use resource_async_tool_v1_poll_bindings::artist::resource::types::LineEnding;
    match result {
        Ok(result) => Ok(artist_kernel::PollResult {
            text: result
                .text
                .into_iter()
                .map(|text| {
                    Ok(artist_kernel::AnchoredText {
                        uri: artist_kernel::ResourceUri::parse(&text.uri).map_err(|error| {
                            artist_kernel::KernelError::InvalidUri {
                                message: error.to_string(),
                            }
                        })?,
                        lines: text
                            .lines
                            .into_iter()
                            .map(|line| artist_kernel::AnchoredLine {
                                anchor: anchor_from_string(&line.anchor),
                                text: line.text,
                                ending: match line.ending {
                                    LineEnding::None => artist_kernel::LineEnding::None,
                                    LineEnding::Lf => artist_kernel::LineEnding::Lf,
                                    LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                                    LineEnding::Cr => artist_kernel::LineEnding::Cr,
                                },
                            })
                            .collect(),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            satisfied: result
                .satisfied
                .into_iter()
                .map(|atom| match atom {
                    resource_async_tool_v1_poll_bindings::artist::resource::types::PollAtom::Changed(
                        target,
                    ) => artist_kernel::PollAtom::Changed(target),
                    resource_async_tool_v1_poll_bindings::artist::resource::types::PollAtom::Regex(
                        regex,
                    ) => artist_kernel::PollAtom::Regex(artist_kernel::RegexAtom {
                        target: regex.target,
                        pattern: regex.pattern,
                    }),
                    resource_async_tool_v1_poll_bindings::artist::resource::types::PollAtom::Terminated(
                        target,
                    ) => artist_kernel::PollAtom::Terminated(target),
                    resource_async_tool_v1_poll_bindings::artist::resource::types::PollAtom::Timeout(
                        milliseconds,
                    ) => artist_kernel::PollAtom::Timeout(milliseconds),
                })
                .collect(),
        }),
        Err(error) => Err(resource_error_parts_to_kernel(
            format!("{:?}", error.code),
            error.uri,
            error.message,
            "poll",
        )),
    }
}

fn typed_error_for<E>(
    code: tool_v1_bindings::artist::resource::types::ErrorCode,
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

fn async_typed_error(
    code: tool_v1_bindings::artist::resource::types::ErrorCode,
    message: String,
    uri: Option<String>,
) -> resource_async_host_bindings::artist::resource::types::Error {
    use resource_async_host_bindings::artist::resource::types::ErrorCode as AsyncCode;
    let code = match format!("{code:?}").as_str() {
        "InvalidUri" => AsyncCode::InvalidUri,
        "InvalidInput" => AsyncCode::InvalidInput,
        "InvalidPattern" => AsyncCode::InvalidPattern,
        "InvalidAnchor" => AsyncCode::InvalidAnchor,
        "StaleAnchor" => AsyncCode::StaleAnchor,
        "WrongKind" => AsyncCode::WrongKind,
        "NotFound" => AsyncCode::NotFound,
        "Conflict" => AsyncCode::Conflict,
        "Immutable" => AsyncCode::Immutable,
        "PermissionDenied" => AsyncCode::PermissionDenied,
        "NotEmpty" => AsyncCode::NotEmpty,
        "Aborted" => AsyncCode::Aborted,
        "Internal" => AsyncCode::Internal,
        _ => AsyncCode::Unsupported,
    };
    resource_async_host_bindings::artist::resource::types::Error { code, uri, message }
}

fn async_error_from_kernel(
    error: artist_kernel::KernelError,
    target: Option<String>,
) -> resource_async_host_bindings::artist::resource::types::Error {
    let error = kernel_error_to_typed(error, target);
    async_typed_error(error.code, error.message, error.uri)
}

/// Execute a contract-shaped kernel operation. JSON is used only to adapt the
/// already-computed typed result to the generated WIT record; it is not used
/// to construct, route, or execute the kernel operation.
fn typed_host_invoke_operation<Response, ErrorType>(
    state: &mut HostState,
    verb: &str,
    target: Option<String>,
    operation: artist_kernel::Operation,
) -> Result<Response, ErrorType>
where
    Response: serde::de::DeserializeOwned,
    ErrorType: serde::de::DeserializeOwned,
{
    let capability = format!("resource.{verb}");
    if !state.capabilities.contains(&capability) {
        return Err(typed_error_for(
            tool_v1_bindings::artist::resource::types::ErrorCode::PermissionDenied,
            format!("capability denied: {capability}"),
            target,
        ));
    }
    let Some(kernel) = state.kernel.clone() else {
        return Err(typed_error_for(
            tool_v1_bindings::artist::resource::types::ErrorCode::Internal,
            "typed tool host has no kernel".to_owned(),
            target,
        ));
    };
    let scope = state.scope.clone();
    let result = futures::executor::block_on(kernel.execute_operation_with_scope(operation, scope))
        .map_err(|error| typed_error_from_kernel(error, target.clone()))?;
    let value = operation_primary_value(result)
        .map_err(|error| typed_error_from_kernel(error, target.clone()))?;
    let mut value = serde_json::to_value(value).map_err(|error| {
        typed_error_for(
            tool_v1_bindings::artist::resource::types::ErrorCode::Internal,
            error.to_string(),
            None,
        )
    })?;
    normalize_typed_value(&mut value);
    serde_json::from_value(value).map_err(|_| {
        typed_error_for(
            tool_v1_bindings::artist::resource::types::ErrorCode::Internal,
            "typed host response conversion failed".to_owned(),
            None,
        )
    })
}

/// Async counterpart for the canonical component host bridge. Component
/// invocations must never block an executor while routing a nested operation
/// through the kernel.
async fn typed_host_invoke_operation_async<Response>(
    state: &mut HostState,
    verb: &str,
    target: Option<String>,
    operation: artist_kernel::Operation,
) -> Result<Response, resource_async_host_bindings::artist::resource::types::Error>
where
    Response: AsyncOperationResponse,
{
    let capability = format!("resource.{verb}");
    if !state.capabilities.contains(&capability) {
        return Err(async_typed_error(
            tool_v1_bindings::artist::resource::types::ErrorCode::PermissionDenied,
            format!("capability denied: {capability}"),
            target,
        ));
    }
    let Some(kernel) = state.kernel.clone() else {
        return Err(async_typed_error(
            tool_v1_bindings::artist::resource::types::ErrorCode::Internal,
            "typed tool host has no kernel".to_owned(),
            target,
        ));
    };
    let result = kernel
        .execute_operation_with_scope(operation, state.scope.clone())
        .await
        .map_err(|error| async_error_from_kernel(error, target.clone()))?;
    Response::from_operation(verb, result).map_err(|error| async_error_from_kernel(error, target))
}

trait AsyncOperationResponse: Sized {
    fn from_operation(
        verb: &str,
        result: artist_kernel::OperationResult,
    ) -> Result<Self, artist_kernel::KernelError>;
}

fn async_anchored_text(
    text: artist_kernel::AnchoredText,
) -> resource_async_host_bindings::artist::resource::types::AnchoredText {
    use resource_async_host_bindings::artist::resource::types::{AnchoredLine, LineEnding};
    resource_async_host_bindings::artist::resource::types::AnchoredText {
        uri: text.uri.to_string(),
        lines: text
            .lines
            .into_iter()
            .map(|line| AnchoredLine {
                anchor: line.anchor.to_string(),
                text: line.text,
                ending: match line.ending {
                    artist_kernel::LineEnding::None => LineEnding::None,
                    artist_kernel::LineEnding::Lf => LineEnding::Lf,
                    artist_kernel::LineEnding::Crlf => LineEnding::Crlf,
                    artist_kernel::LineEnding::Cr => LineEnding::Cr,
                },
            })
            .collect(),
    }
}

impl AsyncOperationResponse for resource_async_host_bindings::artist::resource::types::ReadResult {
    fn from_operation(
        _verb: &str,
        result: artist_kernel::OperationResult,
    ) -> Result<Self, artist_kernel::KernelError> {
        let artist_kernel::OperationResult::Read(mut values) = result else {
            return Err(artist_kernel::KernelError::Handler {
                message: "read response kind mismatch".into(),
            });
        };
        match values.remove(0)? {
            artist_kernel::ReadResult::Text(text) => Ok(Self::Text(async_anchored_text(text))),
            artist_kernel::ReadResult::Directory { uri, entries } => Ok(Self::Directory(
                resource_async_host_bindings::artist::resource::types::DirectoryResult {
                    uri: uri.to_string(),
                    entries: entries.into_iter().map(|entry| entry.to_string()).collect(),
                },
            )),
        }
    }
}

impl AsyncOperationResponse for resource_async_host_bindings::artist::resource::types::WriteResult {
    fn from_operation(
        _verb: &str,
        result: artist_kernel::OperationResult,
    ) -> Result<Self, artist_kernel::KernelError> {
        let artist_kernel::OperationResult::Write(mut values) = result else {
            return Err(artist_kernel::KernelError::Handler {
                message: "write response kind mismatch".into(),
            });
        };
        Ok(Self {
            text: async_anchored_text(values.remove(0)?.text),
        })
    }
}

impl AsyncOperationResponse for resource_async_host_bindings::artist::resource::types::EditResult {
    fn from_operation(
        _verb: &str,
        result: artist_kernel::OperationResult,
    ) -> Result<Self, artist_kernel::KernelError> {
        let artist_kernel::OperationResult::Edit(mut values) = result else {
            return Err(artist_kernel::KernelError::Handler {
                message: "edit response kind mismatch".into(),
            });
        };
        let value = values.remove(0)?;
        let diff_uri = value.diff.uri.clone();
        let hunks = value
            .diff
            .hunks
            .into_iter()
            .map(
                |hunk| resource_async_host_bindings::artist::resource::types::DiffHunk {
                    old: hunk.old.into_iter().map(async_anchored_line).collect(),
                    new: hunk.new.into_iter().map(async_anchored_line).collect(),
                },
            )
            .collect();
        Ok(Self {
            text: async_anchored_text(value.text),
            diff: resource_async_host_bindings::artist::resource::types::AnchoredDiff {
                uri: diff_uri.to_string(),
                hunks,
            },
        })
    }
}

impl AsyncOperationResponse for Vec<String> {
    fn from_operation(
        _verb: &str,
        result: artist_kernel::OperationResult,
    ) -> Result<Self, artist_kernel::KernelError> {
        let artist_kernel::OperationResult::Find(result) = result else {
            return Err(artist_kernel::KernelError::Handler {
                message: "find response kind mismatch".into(),
            });
        };
        Ok(result?.into_iter().map(|uri| uri.to_string()).collect())
    }
}

impl AsyncOperationResponse
    for Vec<resource_async_host_bindings::artist::resource::types::AnchoredText>
{
    fn from_operation(
        _verb: &str,
        result: artist_kernel::OperationResult,
    ) -> Result<Self, artist_kernel::KernelError> {
        let artist_kernel::OperationResult::Grep(result) = result else {
            return Err(artist_kernel::KernelError::Handler {
                message: "grep response kind mismatch".into(),
            });
        };
        Ok(result?.into_iter().map(async_anchored_text).collect())
    }
}

impl AsyncOperationResponse for resource_async_host_bindings::artist::resource::types::PollResult {
    fn from_operation(
        _verb: &str,
        result: artist_kernel::OperationResult,
    ) -> Result<Self, artist_kernel::KernelError> {
        let artist_kernel::OperationResult::Poll(result) = result else {
            return Err(artist_kernel::KernelError::Handler {
                message: "poll response kind mismatch".into(),
            });
        };
        let result = result?;
        Ok(Self {
            text: result.text.into_iter().map(async_anchored_text).collect(),
            satisfied: result
                .satisfied
                .into_iter()
                .map(|atom| match atom {
                    artist_kernel::PollAtom::Changed(target) => {
                        resource_async_host_bindings::artist::resource::types::PollAtom::Changed(
                            target,
                        )
                    }
                    artist_kernel::PollAtom::Regex(regex) => {
                        resource_async_host_bindings::artist::resource::types::PollAtom::Regex(
                            resource_async_host_bindings::artist::resource::types::RegexAtom {
                                target: regex.target,
                                pattern: regex.pattern,
                            },
                        )
                    }
                    artist_kernel::PollAtom::Terminated(target) => {
                        resource_async_host_bindings::artist::resource::types::PollAtom::Terminated(
                            target,
                        )
                    }
                    artist_kernel::PollAtom::Timeout(milliseconds) => {
                        resource_async_host_bindings::artist::resource::types::PollAtom::Timeout(
                            milliseconds,
                        )
                    }
                })
                .collect(),
        })
    }
}

impl AsyncOperationResponse for String {
    fn from_operation(
        verb: &str,
        result: artist_kernel::OperationResult,
    ) -> Result<Self, artist_kernel::KernelError> {
        let (mut values, expected) = match result {
            artist_kernel::OperationResult::Run(values) => (values, "run"),
            artist_kernel::OperationResult::Send(values) => (values, "send"),
            artist_kernel::OperationResult::Abort(values) => (values, "abort"),
            artist_kernel::OperationResult::Delete(values) => (values, "delete"),
            _ => {
                return Err(artist_kernel::KernelError::Handler {
                    message: format!("{verb} response kind mismatch"),
                });
            }
        };
        if verb != expected {
            return Err(artist_kernel::KernelError::Handler {
                message: format!("{verb} response kind mismatch"),
            });
        }
        Ok(values.remove(0)?.to_string())
    }
}

fn async_anchored_line(
    line: artist_kernel::AnchoredLine,
) -> resource_async_host_bindings::artist::resource::types::AnchoredLine {
    use resource_async_host_bindings::artist::resource::types::LineEnding;
    resource_async_host_bindings::artist::resource::types::AnchoredLine {
        anchor: line.anchor.to_string(),
        text: line.text,
        ending: match line.ending {
            artist_kernel::LineEnding::None => LineEnding::None,
            artist_kernel::LineEnding::Lf => LineEnding::Lf,
            artist_kernel::LineEnding::Crlf => LineEnding::Crlf,
            artist_kernel::LineEnding::Cr => LineEnding::Cr,
        },
    }
}

fn operation_primary_value(
    result: artist_kernel::OperationResult,
) -> Result<serde_json::Value, artist_kernel::KernelError> {
    use artist_kernel::OperationResult;
    match result {
        OperationResult::Read(mut values) => values.remove(0).and_then(|value| {
            serde_json::to_value(value).map_err(|error| artist_kernel::KernelError::Handler {
                message: error.to_string(),
            })
        }),
        OperationResult::Write(mut values) => values.remove(0).and_then(|value| {
            serde_json::to_value(value).map_err(|error| artist_kernel::KernelError::Handler {
                message: error.to_string(),
            })
        }),
        OperationResult::Edit(mut values) => values.remove(0).and_then(|value| {
            serde_json::to_value(value).map_err(|error| artist_kernel::KernelError::Handler {
                message: error.to_string(),
            })
        }),
        OperationResult::Run(mut values)
        | OperationResult::Send(mut values)
        | OperationResult::Abort(mut values)
        | OperationResult::Delete(mut values) => values
            .remove(0)
            .map(|uri| serde_json::json!(uri.to_string())),
        OperationResult::Find(value) => value.map(|uris| {
            serde_json::json!(
                uris.into_iter()
                    .map(|uri| uri.to_string())
                    .collect::<Vec<_>>()
            )
        }),
        OperationResult::Grep(value) => {
            value.map(|texts| serde_json::to_value(texts).unwrap_or(serde_json::Value::Null))
        }
        OperationResult::Poll(value) => {
            value.map(|value| serde_json::to_value(value).unwrap_or(serde_json::Value::Null))
        }
    }
}

/// Adapt only representation details between the kernel's Rust value types
/// and WIT's scalar aliases. This remains at the component adapter boundary;
/// no JSON enters kernel dispatch.
fn normalize_typed_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(anchor) = object.get("anchor").cloned()
                && let Some(tokens) = anchor.get("tokens").and_then(serde_json::Value::as_array)
            {
                let joined = tokens
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(".");
                object.insert(
                    "anchor".to_owned(),
                    serde_json::Value::String(format!("#{joined}")),
                );
            }
            for child in object.values_mut() {
                normalize_typed_value(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                normalize_typed_value(child);
            }
        }
        _ => {}
    }
}

fn typed_error(
    code: tool_v1_bindings::artist::resource::types::ErrorCode,
    message: String,
    uri: Option<String>,
) -> tool_v1_bindings::artist::resource::types::Error {
    tool_v1_bindings::artist::resource::types::Error { code, uri, message }
}

// Kept behind the provider adapter boundary for older external callers; the
// universal component worlds never construct kernel operations from this path.
fn kernel_error_to_typed(
    error: artist_kernel::KernelError,
    target: Option<String>,
) -> tool_v1_bindings::artist::resource::types::Error {
    use artist_kernel::KernelError;
    let (code, uri, message) = match error {
        KernelError::InvalidUri { message } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::InvalidUri,
            target,
            message,
        ),
        KernelError::UnsupportedUri { uri } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::Unsupported,
            Some(uri),
            "URI scheme is not supported".to_owned(),
        ),
        KernelError::NoHandler { uri } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::Unsupported,
            Some(uri),
            "no handler is registered for this URI".to_owned(),
        ),
        KernelError::UnsupportedVerb { verb, uri } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::Unsupported,
            Some(uri),
            format!("verb {verb} is not supported for this resource"),
        ),
        KernelError::InvalidRequest { message } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::InvalidInput,
            target,
            message,
        ),
        KernelError::InvalidPattern { message } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::InvalidPattern,
            target,
            message,
        ),
        KernelError::InvalidAnchor { message } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::InvalidAnchor,
            target,
            message,
        ),
        KernelError::StaleAnchor { message } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::StaleAnchor,
            target,
            message,
        ),
        KernelError::WrongKind { message } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::WrongKind,
            target,
            message,
        ),
        KernelError::NotFound { uri } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::NotFound,
            Some(uri),
            "resource was not found".to_owned(),
        ),
        KernelError::AlreadyExists { uri } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::Conflict,
            Some(uri),
            "resource conflict".to_owned(),
        ),
        KernelError::Immutable { uri } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::Immutable,
            Some(uri),
            "resource is immutable".to_owned(),
        ),
        KernelError::PermissionDenied { uri } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::PermissionDenied,
            Some(uri),
            "permission denied".to_owned(),
        ),
        KernelError::Conflict { uri } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::Conflict,
            Some(uri),
            "resource conflict".to_owned(),
        ),
        KernelError::NotEmpty { uri } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::NotEmpty,
            Some(uri),
            "resource is not empty".to_owned(),
        ),
        KernelError::Aborted { message } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::Aborted,
            target,
            message,
        ),
        KernelError::Handler { message } => (
            tool_v1_bindings::artist::resource::types::ErrorCode::Internal,
            target,
            message,
        ),
    };
    typed_error(code, message, uri)
}

/// Host for components that implement the typed `artist:tool` worlds.
pub struct TypedComponentHost {
    engine: wasmtime::Engine,
    component: wasmtime::component::Component,
    capabilities: HashSet<String>,
    dependencies: Vec<Arc<DynamicDependency>>,
    canonical_resource_imports: bool,
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

fn validate_typed_contract_names(
    verb: contracts::Verb,
    exports: &[String],
    tool_imports: &[String],
) -> Result<(), String> {
    let v1_export = format!("artist:tool/{}@1.0.0", verb.interface());
    if exports == &[v1_export.clone()] && tool_imports.iter().any(|name| name.contains("resource/"))
    {
        return Ok(());
    }
    Err(format!(
        "typed contract {} requires export {} and direct artist:resource imports (exports={exports:?}, imports={tool_imports:?})",
        contracts::ContractId::universal(verb),
        v1_export
    ))
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
        let canonical_resource_imports = component
            .component_type()
            .imports(&engine)
            .any(|(name, _)| name.contains("resource/"));
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
            canonical_resource_imports,
        })
    }

    /// Validate that the component has exactly the world selected by its
    /// contract. This is intentionally performed against the compiled
    /// component type, before instantiation or invocation.
    pub fn validate_contract(&self, verb: contracts::Verb) -> Result<(), ComponentError> {
        let component_type = self.component.component_type();
        let exports = component_type
            .exports(&self.engine)
            .map(|(name, _)| name.to_owned())
            .collect::<Vec<_>>();
        let imports = component_type
            .imports(&self.engine)
            .map(|(name, _)| name.to_owned())
            .collect::<Vec<_>>();
        let tool_imports = imports
            .iter()
            .filter(|name| name.contains("artist:tool/") || name.contains("resource/"))
            .cloned()
            .collect::<Vec<_>>();
        validate_typed_contract_names(verb, &exports, &tool_imports)
            .map_err(|message| ComponentError::Load(anyhow::anyhow!(message)))
    }

    fn linker_for(
        &self,
        verb: contracts::Verb,
    ) -> Result<wasmtime::component::Linker<HostState>, ComponentError> {
        let mut linker = wasmtime::component::Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| ComponentError::Load(anyhow::anyhow!(e.to_string())))?;
        macro_rules! add_resource_import {
            ($instance:literal, $interface:ident) => {{
                let mut instance = linker
                    .instance($instance)
                    .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
                resource_bindings::artist::resource::$interface::add_to_linker_instance::<
                    _,
                    wasmtime::component::HasSelf<_>,
                >(&mut instance, |state: &mut HostState| state)
                .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
            }};
        }
        macro_rules! add_resource {
            ($interface:ident) => {
                resource_bindings::artist::resource::$interface::add_to_linker::<
                    _,
                    wasmtime::component::HasSelf<_>,
                >(&mut linker, |state: &mut HostState| state)
                .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
            };
        }
        match verb {
            contracts::Verb::Read => {
                add_resource!(read);
                add_resource_import!("resource-read", read);
            }
            contracts::Verb::Write => {
                add_resource!(write);
                add_resource_import!("resource-write", write);
            }
            contracts::Verb::Edit => {
                add_resource!(edit);
                add_resource_import!("resource-edit", edit);
            }
            contracts::Verb::Find => {
                add_resource!(find);
                add_resource_import!("resource-find", find);
            }
            contracts::Verb::Grep => {
                add_resource!(grep);
                add_resource_import!("resource-grep", grep);
            }
            contracts::Verb::Run => {
                add_resource!(run);
                add_resource_import!("resource-run", run);
            }
            contracts::Verb::Send => {
                add_resource!(send);
                add_resource_import!("resource-send", send);
            }
            contracts::Verb::Abort => {
                add_resource!(abort);
                add_resource_import!("resource-abort", abort);
            }
            contracts::Verb::Delete => {
                add_resource!(delete);
                add_resource_import!("resource-delete", delete);
            }
            contracts::Verb::Poll => {
                add_resource!(poll);
                add_resource_import!("resource-poll", poll);
            }
        }
        Ok(linker)
    }

    fn dynamic_linker(&self) -> Result<wasmtime::component::Linker<HostState>, ComponentError> {
        dynamic_linker_for(
            &self.engine,
            &self.component,
            &self.capabilities,
            &self.dependencies,
        )
    }

    fn async_linker_for(
        &self,
        verb: contracts::Verb,
    ) -> Result<wasmtime::component::Linker<HostState>, ComponentError> {
        let mut linker = wasmtime::component::Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
        macro_rules! add_resource_import {
            ($instance:literal, $interface:ident) => {{
                let mut instance = linker
                    .instance($instance)
                    .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
                resource_async_host_bindings::artist::resource::$interface::add_to_linker_instance::<
                    _,
                    wasmtime::component::HasSelf<_>,
                >(&mut instance, |state: &mut HostState| state)
                .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
            }};
        }
        macro_rules! add_resource {
            ($interface:ident) => {
                resource_async_host_bindings::artist::resource::$interface::add_to_linker::<
                    _,
                    wasmtime::component::HasSelf<_>,
                >(&mut linker, |state: &mut HostState| state)
                .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
            };
        }
        match verb {
            contracts::Verb::Read => {
                add_resource!(read);
                add_resource_import!("resource-read", read);
            }
            contracts::Verb::Write => {
                add_resource!(write);
                add_resource_import!("resource-write", write);
            }
            contracts::Verb::Edit => {
                add_resource!(edit);
                add_resource_import!("resource-edit", edit);
            }
            contracts::Verb::Find => {
                add_resource!(find);
                add_resource_import!("resource-find", find);
            }
            contracts::Verb::Grep => {
                add_resource!(grep);
                add_resource_import!("resource-grep", grep);
            }
            contracts::Verb::Run => {
                add_resource!(run);
                add_resource_import!("resource-run", run);
            }
            contracts::Verb::Send => {
                add_resource!(send);
                add_resource_import!("resource-send", send);
            }
            contracts::Verb::Abort => {
                add_resource!(abort);
                add_resource_import!("resource-abort", abort);
            }
            contracts::Verb::Delete => {
                add_resource!(delete);
                add_resource_import!("resource-delete", delete);
            }
            contracts::Verb::Poll => {
                add_resource!(poll);
                add_resource_import!("resource-poll", poll);
            }
        }
        Ok(linker)
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
            .get_func(&mut store, "invoke")
            .or_else(|| instance.get_func(&mut store, export_name))
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
                    .or_else(|| (params.len() == 1).then_some(input))
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
            .get_func(&mut store, "invoke")
            .or_else(|| instance.get_func(&mut store, export_name))
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
                    .or_else(|| (params.len() == 1).then_some(input))
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
        let mut store = new_store(
            &self.engine,
            HostState::with_capabilities(self.capabilities.iter().cloned()),
        );
        let linker = self.dynamic_linker()?;
        let instance = linker
            .instantiate(&mut store, &self.component)
            .map_err(|error| ComponentError::Build {
                diagnostics: error.to_string(),
            })?;
        if instance.get_func(&mut store, "invoke").is_none()
            && instance.get_func(&mut store, export_name).is_none()
        {
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
        if !exports
            .iter()
            .any(|name| *name == "invoke" || *name == export_name)
        {
            return Err(ComponentError::Build {
                diagnostics: format!(
                    "package-local WIT component exports neither invoke nor {export_name}"
                ),
            });
        }
        Ok(())
    }

    pub fn invoke_read(
        &self,
        requests: Vec<tool_v1_bindings::artist::resource::types::ReadRequest>,
        kernel: KernelHandle,
    ) -> Result<
        Vec<
            Result<
                tool_v1_bindings::artist::resource::types::ReadResult,
                tool_v1_bindings::artist::resource::types::Error,
            >,
        >,
        ComponentError,
    > {
        let input = serde_json::to_string(&requests)
            .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?;
        let output = self.invoke_json(contracts::Verb::Read, &input, kernel)?;
        serde_json::from_str(&output)
            .map_err(|_| ComponentError::MalformedResponse { field: "output" })
    }

    /// Dispatch one typed verb export using its WIT record JSON representation.
    /// This is the provider/runtime adapter; the component itself still
    /// receives and returns typed WIT values.
    pub fn invoke_json(
        &self,
        verb: contracts::Verb,
        input: &str,
        kernel: KernelHandle,
    ) -> Result<String, ComponentError> {
        self.invoke_json_with_context(
            verb,
            input,
            kernel,
            artist_kernel::InvocationContext::default(),
        )
    }

    pub fn invoke_json_with_context(
        &self,
        verb: contracts::Verb,
        input: &str,
        kernel: KernelHandle,
        context: artist_kernel::InvocationContext,
    ) -> Result<String, ComponentError> {
        let mut store = new_store(
            &self.engine,
            HostState::with_kernel_context(self.capabilities.iter().cloned(), kernel, context),
        );
        let linker = self.linker_for(verb)?;
        macro_rules! invoke {
            ($module:ident, $world:ident, $ty:ty, $instance:ident, $method:ident, $request:expr) => {{
                let instance =
                    $module::$world::instantiate(&mut store, &self.component, &linker)
                        .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?;
                let request: $ty = serde_json::from_str($request)
                    .map_err(|_| ComponentError::MalformedResponse { field: "input" })?;
                let result = instance
                    .$instance()
                    .$method(&mut store, &request)
                    .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?;
                serde_json::to_string(&result)
                    .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))
            }};
        }
        match verb {
            contracts::Verb::Read => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_bindings,
                        ReadWorld,
                        Vec<tool_v1_bindings::artist::resource::types::ReadRequest>,
                        artist_tool_read,
                        call_read,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_bindings,
                        ReadWorld,
                        Vec<tool_v1_bindings::artist::resource::types::ReadRequest>,
                        artist_tool_read,
                        call_read,
                        input
                    )
                }
            }
            contracts::Verb::Write => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_write_bindings,
                        WriteWorld,
                        Vec<tool_v1_write_bindings::artist::resource::types::WriteRequest>,
                        artist_tool_write,
                        call_write,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_write_bindings,
                        WriteWorld,
                        Vec<tool_v1_write_bindings::artist::resource::types::WriteRequest>,
                        artist_tool_write,
                        call_write,
                        input
                    )
                }
            }
            contracts::Verb::Edit => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_edit_bindings,
                        EditWorld,
                        Vec<tool_v1_edit_bindings::artist::resource::types::EditRequest>,
                        artist_tool_edit,
                        call_edit,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_edit_bindings,
                        EditWorld,
                        Vec<tool_v1_edit_bindings::artist::resource::types::EditRequest>,
                        artist_tool_edit,
                        call_edit,
                        input
                    )
                }
            }
            contracts::Verb::Find => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_find_bindings,
                        FindWorld,
                        tool_v1_find_bindings::artist::resource::types::FindRequest,
                        artist_tool_find,
                        call_find,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_find_bindings,
                        FindWorld,
                        tool_v1_find_bindings::artist::resource::types::FindRequest,
                        artist_tool_find,
                        call_find,
                        input
                    )
                }
            }
            contracts::Verb::Grep => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_grep_bindings,
                        GrepWorld,
                        tool_v1_grep_bindings::artist::resource::types::GrepRequest,
                        artist_tool_grep,
                        call_grep,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_grep_bindings,
                        GrepWorld,
                        tool_v1_grep_bindings::artist::resource::types::GrepRequest,
                        artist_tool_grep,
                        call_grep,
                        input
                    )
                }
            }
            contracts::Verb::Run => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_run_bindings,
                        RunWorld,
                        Vec<tool_v1_run_bindings::artist::resource::types::RunRequest>,
                        artist_tool_run,
                        call_run,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_run_bindings,
                        RunWorld,
                        Vec<tool_v1_run_bindings::artist::resource::types::RunRequest>,
                        artist_tool_run,
                        call_run,
                        input
                    )
                }
            }
            contracts::Verb::Send => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_send_bindings,
                        SendWorld,
                        Vec<tool_v1_send_bindings::artist::resource::types::SendRequest>,
                        artist_tool_send,
                        call_send,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_send_bindings,
                        SendWorld,
                        Vec<tool_v1_send_bindings::artist::resource::types::SendRequest>,
                        artist_tool_send,
                        call_send,
                        input
                    )
                }
            }
            contracts::Verb::Abort => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_abort_bindings,
                        AbortWorld,
                        Vec<String>,
                        artist_tool_abort,
                        call_abort,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_abort_bindings,
                        AbortWorld,
                        Vec<String>,
                        artist_tool_abort,
                        call_abort,
                        input
                    )
                }
            }
            contracts::Verb::Delete => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_delete_bindings,
                        DeleteWorld,
                        Vec<String>,
                        artist_tool_delete,
                        call_delete,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_delete_bindings,
                        DeleteWorld,
                        Vec<String>,
                        artist_tool_delete,
                        call_delete,
                        input
                    )
                }
            }
            contracts::Verb::Poll => {
                if self.canonical_resource_imports {
                    invoke!(
                        tool_v1_poll_bindings,
                        PollWorld,
                        tool_v1_poll_bindings::artist::resource::types::PollRequest,
                        artist_tool_poll,
                        call_poll,
                        input
                    )
                } else {
                    invoke!(
                        tool_v1_poll_bindings,
                        PollWorld,
                        tool_v1_poll_bindings::artist::resource::types::PollRequest,
                        artist_tool_poll,
                        call_poll,
                        input
                    )
                }
            }
        }
    }

    /// Async counterpart used by the canonical v1 universal tool path. The
    /// JSON here is only the provider adapter representation; the component
    /// call itself is generated WIT and uses the shared resource types.
    pub async fn invoke_json_async_with_context(
        &self,
        verb: contracts::Verb,
        input: &str,
        kernel: KernelHandle,
        context: artist_kernel::InvocationContext,
    ) -> Result<String, ComponentError> {
        self.invoke_json_async_with_scope(
            verb,
            input,
            kernel,
            artist_kernel::InvocationScope::new(context),
        )
        .await
    }

    pub async fn invoke_json_async_with_scope(
        &self,
        verb: contracts::Verb,
        input: &str,
        kernel: KernelHandle,
        scope: artist_kernel::InvocationScope,
    ) -> Result<String, ComponentError> {
        let mut store = new_store(
            &self.engine,
            HostState::with_kernel_scope(self.capabilities.iter().cloned(), kernel, scope),
        );
        let linker = self.async_linker_for(verb)?;
        macro_rules! invoke {
            ($module:ident, $world:ident, $ty:ty, $instance:ident, $method:ident) => {{
                let instance =
                    $module::$world::instantiate_async(&mut store, &self.component, &linker)
                        .await
                        .map_err(|error| {
                            ComponentError::Invoke(anyhow::anyhow!(error.to_string()))
                        })?;
                let request: $ty = serde_json::from_str(input)
                    .map_err(|_| ComponentError::MalformedResponse { field: "input" })?;
                let result = instance
                    .$instance()
                    .$method(&mut store, &request)
                    .await
                    .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))?;
                serde_json::to_string(&result)
                    .map_err(|error| ComponentError::Invoke(anyhow::anyhow!(error.to_string())))
            }};
        }
        match verb {
            contracts::Verb::Read => invoke!(
                tool_v1_async_tool_v1_bindings,
                ReadWorld,
                Vec<tool_v1_async_tool_v1_bindings::artist::resource::types::ReadRequest>,
                artist_tool_read,
                call_read
            ),
            contracts::Verb::Write => invoke!(
                tool_v1_async_tool_v1_write_bindings,
                WriteWorld,
                Vec<tool_v1_async_tool_v1_write_bindings::artist::resource::types::WriteRequest>,
                artist_tool_write,
                call_write
            ),
            contracts::Verb::Edit => invoke!(
                tool_v1_async_tool_v1_edit_bindings,
                EditWorld,
                Vec<tool_v1_async_tool_v1_edit_bindings::artist::resource::types::EditRequest>,
                artist_tool_edit,
                call_edit
            ),
            contracts::Verb::Find => invoke!(
                tool_v1_async_tool_v1_find_bindings,
                FindWorld,
                tool_v1_async_tool_v1_find_bindings::artist::resource::types::FindRequest,
                artist_tool_find,
                call_find
            ),
            contracts::Verb::Grep => invoke!(
                tool_v1_async_tool_v1_grep_bindings,
                GrepWorld,
                tool_v1_async_tool_v1_grep_bindings::artist::resource::types::GrepRequest,
                artist_tool_grep,
                call_grep
            ),
            contracts::Verb::Run => invoke!(
                tool_v1_async_tool_v1_run_bindings,
                RunWorld,
                Vec<tool_v1_async_tool_v1_run_bindings::artist::resource::types::RunRequest>,
                artist_tool_run,
                call_run
            ),
            contracts::Verb::Send => invoke!(
                tool_v1_async_tool_v1_send_bindings,
                SendWorld,
                Vec<tool_v1_async_tool_v1_send_bindings::artist::resource::types::SendRequest>,
                artist_tool_send,
                call_send
            ),
            contracts::Verb::Abort => invoke!(
                tool_v1_async_tool_v1_abort_bindings,
                AbortWorld,
                Vec<String>,
                artist_tool_abort,
                call_abort
            ),
            contracts::Verb::Delete => invoke!(
                tool_v1_async_tool_v1_delete_bindings,
                DeleteWorld,
                Vec<String>,
                artist_tool_delete,
                call_delete
            ),
            contracts::Verb::Poll => invoke!(
                tool_v1_async_tool_v1_poll_bindings,
                PollWorld,
                tool_v1_async_tool_v1_poll_bindings::artist::resource::types::PollRequest,
                artist_tool_poll,
                call_poll
            ),
        }
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
    macro_rules! add_resource {
        ($interface:ident) => {
            resource_bindings::artist::resource::$interface::add_to_linker::<
                _,
                wasmtime::component::HasSelf<_>,
            >(&mut linker, |state: &mut HostState| state)
            .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
        };
    }
    add_resource!(read);
    add_resource!(write);
    add_resource!(edit);
    add_resource!(find);
    add_resource!(grep);
    add_resource!(run);
    add_resource!(send);
    add_resource!(abort);
    add_resource!(delete);
    add_resource!(poll);
    for (import_name, _) in component.component_type().imports(engine) {
        if import_name.starts_with("wasi:") || import_name.starts_with("artist:tool/") {
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
    macro_rules! add_resource {
        ($interface:ident) => {
            resource_async_host_bindings::artist::resource::$interface::add_to_linker::<
                HostState,
                wasmtime::component::HasSelf<HostState>,
            >(&mut linker, |state: &mut HostState| state)
            .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
        };
    }
    add_resource!(read);
    add_resource!(write);
    add_resource!(edit);
    add_resource!(find);
    add_resource!(grep);
    add_resource!(run);
    add_resource!(send);
    add_resource!(abort);
    add_resource!(delete);
    add_resource!(poll);
    for (import_name, _) in component.component_type().imports(engine) {
        if import_name.starts_with("wasi:") || import_name.starts_with("artist:tool/") {
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
                        object
                            .get(field.name)
                            .or_else(|| object.get(&field.name.replace('-', "_")))
                            .ok_or_else(|| format!("missing record field {}", field.name))
                            .and_then(|value| {
                                json_to_component_val(value, &field.ty)
                                    .map(|value| (field.name.to_owned(), value))
                            })
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
        if !rustflags.contains("target-cpu") {
            if !rustflags.is_empty() {
                rustflags.push(' ');
            }
            rustflags.push_str("-C target-cpu=native");
        }
        let mut build = escargot::CargoBuild::new()
            .manifest_path(manifest)
            .target_dir(root.join("target"))
            .target(&options.target)
            .arg("--message-format")
            .arg("json")
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
                                .find(|path| path.is_file() && path.file_name().is_some_and(|name| name == filename.as_str()))
                        })
                })
            })
            .ok_or_else(|| {
                format!(
                    "cargo succeeded but emitted no WASM component artifact (target={target_name}, expected={filename})"
                )
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
            &package.root.join("Cargo.toml"),
            &package.root.join("Cargo.lock"),
        ] {
            super::package::hash_path(&mut hasher, path)
                .map_err(|diagnostics| ComponentError::Build { diagnostics })?;
        }
        super::package::hash_path(
            &mut hasher,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("wit/tool-surface-v1"),
        )
        .map_err(|diagnostics| ComponentError::Build { diagnostics })?;
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
        if package
            .contract
            .as_ref()
            .is_some_and(|contract| contract.verb.is_none())
            && package.wit.is_none()
        {
            return Err(ComponentError::Build {
                diagnostics: format!(
                    "extension contract {} requires a package-local tool.wit",
                    package.contract.as_ref().expect("checked above")
                ),
            });
        }
        if package
            .contract
            .as_ref()
            .and_then(|contract| contract.verb)
            .is_some()
        {
            let contract = package.contract.as_ref().unwrap();
            let verb = contract.verb.expect("checked above");
            let expected_capability = format!("resource.{verb}");
            if package.manifest.capabilities != vec![expected_capability.clone()] {
                return Err(ComponentError::Build {
                    diagnostics: format!(
                        "contract {} must declare exactly capability {}",
                        contract, expected_capability
                    ),
                });
            }
            let host = TypedComponentHost::new_with_capabilities(
                &bytes,
                options.granted_capabilities.clone(),
            )?;
            host.validate_contract(verb)
                .map_err(|error| ComponentError::Build {
                    diagnostics: format!("typed component validation failed: {error}"),
                })?;
        } else if package.wit.is_some() {
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
        } else {
            return Err(ComponentError::Build {
                diagnostics:
                    "component package must declare a typed universal contract or package-local WIT"
                        .to_owned(),
            });
        }
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
        collections::HashMap,
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
        collections::HashMap,
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
        typed_host: Option<Arc<TypedComponentHost>>,
        dynamic_host: Option<Arc<TypedComponentHost>>,
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
            verb: super::contracts::Verb,
            input: &str,
            kernel: artist_kernel::KernelHandle,
        ) -> Result<String, ComponentError> {
            self.invoke_tool_json_with_context(
                verb,
                input,
                kernel,
                artist_kernel::InvocationContext::default(),
            )
        }

        pub fn invoke_tool_json_with_context(
            &self,
            verb: super::contracts::Verb,
            input: &str,
            kernel: artist_kernel::KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> Result<String, ComponentError> {
            self.typed_host
                .as_ref()
                .ok_or_else(|| {
                    ComponentError::Invoke(anyhow::anyhow!(
                        "named invocation requires a typed v1 component"
                    ))
                })?
                .invoke_json_with_context(verb, input, kernel, context)
        }

        pub async fn invoke_tool_json_async_with_context(
            &self,
            verb: super::contracts::Verb,
            input: &str,
            kernel: artist_kernel::KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> Result<String, ComponentError> {
            let host = self.typed_host.as_ref().ok_or_else(|| {
                ComponentError::Invoke(anyhow::anyhow!(
                    "async named invocation requires a typed v1 component"
                ))
            })?;
            host.invoke_json_async_with_context(verb, input, kernel, context)
                .await
        }

        pub async fn invoke_tool_json_async_with_scope(
            &self,
            verb: super::contracts::Verb,
            input: &str,
            kernel: artist_kernel::KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<String, ComponentError> {
            let host = self.typed_host.as_ref().ok_or_else(|| {
                ComponentError::Invoke(anyhow::anyhow!(
                    "async named invocation requires a typed v1 component"
                ))
            })?;
            host.invoke_json_async_with_scope(verb, input, kernel, scope)
                .await
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
                .as_ref()
                .ok_or_else(|| {
                    ComponentError::Invoke(anyhow::anyhow!(
                        "component has no package-local WIT host"
                    ))
                })?
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
                .as_ref()
                .ok_or_else(|| {
                    ComponentError::Invoke(anyhow::anyhow!(
                        "component has no package-local WIT host"
                    ))
                })?
                .invoke_dynamic_json_async_with_scope(input, export_name, kernel, scope)
                .await
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
                typed_host: self.typed_host.as_ref().map(Arc::clone),
                dynamic_host: self.dynamic_host.as_ref().map(Arc::clone),
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
            let (typed_host, dynamic_host, info) = if package
                .contract
                .as_ref()
                .and_then(|contract| contract.verb)
                .is_some()
            {
                let typed_host = Arc::new(TypedComponentHost::new_with_capabilities(
                    &bytes,
                    options.granted_capabilities.clone(),
                )?);
                let info = ComponentMetadata {
                    name: package.manifest.name.clone(),
                    version: package.manifest.version.clone(),
                    abi_version: super::ABI_VERSION.to_owned(),
                    interfaces: vec![
                        package
                            .contract
                            .as_ref()
                            .expect("typed contract was checked above")
                            .to_string(),
                    ],
                    required_capabilities: package.manifest.capabilities.clone(),
                };
                (Some(typed_host), None, info)
            } else if package.wit.is_some() {
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
                (None, Some(dynamic_host), info)
            } else {
                return Err(ComponentError::Load(anyhow::anyhow!(
                    "component package must declare a typed universal contract or package-local WIT"
                )));
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
                    typed_host,
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
    use super::{ComponentError, contracts::Verb, runtime::ActiveVersion};
    use artist_kernel::{ItemResult, KernelError, KernelHandle};
    use serde_json::Value;
    use std::sync::Arc;

    #[derive(Clone)]
    pub struct ComponentTool {
        component: Arc<ActiveVersion>,
        verb: Verb,
    }

    impl ComponentTool {
        pub fn new(component: ActiveVersion, verb: Verb) -> Self {
            Self {
                component: Arc::new(component),
                verb,
            }
        }

        pub fn verb(&self) -> Verb {
            self.verb
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
            // The model adapter owns JSON shape conversion, but it must retain
            // the complete per-verb request. In particular, it must never
            // synthesize a target, discard read windows, or replace edit
            // operations with an empty list.
            let input_value = match self.verb {
                Verb::Read | Verb::Write | Verb::Edit | Verb::Run | Verb::Send => {
                    let mut requests = args.get("requests").cloned().unwrap_or_else(|| {
                        if args.is_array() {
                            args.clone()
                        } else {
                            serde_json::json!([args])
                        }
                    });
                    normalize_model_requests(self.verb, &mut requests)?;
                    requests
                }
                Verb::Abort | Verb::Delete => args.get("uris").cloned().unwrap_or_else(|| {
                    if args.is_array() {
                        args.clone()
                    } else if let Some(uri) = args.get("uri").or_else(|| args.get("target")) {
                        serde_json::json!([uri])
                    } else {
                        args.clone()
                    }
                }),
                Verb::Find | Verb::Grep | Verb::Poll => args,
            };
            let input =
                serde_json::to_string(&input_value).map_err(|error| KernelError::Handler {
                    message: format!("could not encode component input: {error}"),
                })?;
            let output = self
                .component
                .invoke_tool_json_with_context(self.verb, &input, kernel, context)
                .map_err(component_error)?;
            serde_json::from_str(&output).map_err(|error| KernelError::Handler {
                message: format!("component returned invalid output JSON: {error}"),
            })
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
            let input_value = match self.verb {
                Verb::Read | Verb::Write | Verb::Edit | Verb::Run | Verb::Send => {
                    let mut requests = args.get("requests").cloned().unwrap_or_else(|| {
                        if args.is_array() {
                            args.clone()
                        } else {
                            serde_json::json!([args])
                        }
                    });
                    normalize_model_requests(self.verb, &mut requests)?;
                    requests
                }
                Verb::Abort | Verb::Delete => args.get("uris").cloned().unwrap_or_else(|| {
                    if args.is_array() {
                        args.clone()
                    } else if let Some(uri) = args.get("uri").or_else(|| args.get("target")) {
                        serde_json::json!([uri])
                    } else {
                        args.clone()
                    }
                }),
                Verb::Find | Verb::Grep | Verb::Poll => args,
            };
            let input =
                serde_json::to_string(&input_value).map_err(|error| KernelError::Handler {
                    message: format!("could not encode component input: {error}"),
                })?;
            let output = self
                .component
                .invoke_tool_json_async_with_context(self.verb, &input, kernel, context)
                .await
                .map_err(component_error)?;
            serde_json::from_str(&output).map_err(|error| KernelError::Handler {
                message: format!("component returned invalid output JSON: {error}"),
            })
        }

        pub async fn invoke_async_with_scope(
            &self,
            args: Value,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Value, KernelError> {
            let input_value = match self.verb {
                Verb::Read | Verb::Write | Verb::Edit | Verb::Run | Verb::Send => {
                    let mut requests = args.get("requests").cloned().unwrap_or_else(|| {
                        if args.is_array() {
                            args.clone()
                        } else {
                            serde_json::json!([args])
                        }
                    });
                    normalize_model_requests(self.verb, &mut requests)?;
                    requests
                }
                Verb::Abort | Verb::Delete => args.get("uris").cloned().unwrap_or_else(|| {
                    if args.is_array() {
                        args.clone()
                    } else if let Some(uri) = args.get("uri").or_else(|| args.get("target")) {
                        serde_json::json!([uri])
                    } else {
                        args.clone()
                    }
                }),
                Verb::Find | Verb::Grep | Verb::Poll => args,
            };
            let input =
                serde_json::to_string(&input_value).map_err(|error| KernelError::Handler {
                    message: format!("could not encode component input: {error}"),
                })?;
            let output = self
                .component
                .invoke_tool_json_async_with_scope(self.verb, &input, kernel, scope)
                .await
                .map_err(component_error)?;
            serde_json::from_str(&output).map_err(|error| KernelError::Handler {
                message: format!("component returned invalid output JSON: {error}"),
            })
        }
    }

    fn component_error(error: ComponentError) -> KernelError {
        KernelError::Handler {
            message: error.to_string(),
        }
    }

    fn normalize_model_requests(verb: Verb, requests: &mut Value) -> Result<(), KernelError> {
        let Some(items) = requests.as_array_mut() else {
            return Err(KernelError::InvalidRequest {
                message: format!("{verb} expects a request list"),
            });
        };
        for item in items {
            let Some(object) = item.as_object_mut() else {
                return Err(KernelError::InvalidRequest {
                    message: format!("{verb} request must be an object"),
                });
            };
            // `target` is accepted only as outer model-adapter sugar. The
            // typed request sent to the component always uses `uri`.
            if !object.contains_key("uri") {
                if let Some(target) = object.remove("target") {
                    object.insert("uri".to_owned(), target);
                }
            }
            match verb {
                Verb::Read => {
                    object.entry("at").or_insert(Value::Null);
                    object.entry("before").or_insert(Value::Null);
                    object.entry("after").or_insert(Value::Null);
                }
                Verb::Write if !object.contains_key("content") => {
                    if let Some(value) = object.remove("value") {
                        object.insert("content".to_owned(), value);
                    }
                }
                Verb::Edit if !object.contains_key("operations") => {
                    return Err(KernelError::InvalidRequest {
                        message: "edit requires operations".to_owned(),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Invoke an already-produced kernel result through the normal component
    /// adapter shape. Useful to test the bridge without registering a second
    /// recursive kernel handler.
    pub fn result_value(result: ItemResult) -> Result<Value, KernelError> {
        if result.ok {
            Ok(result.value.unwrap_or(Value::Null))
        } else {
            Err(result.error.unwrap_or_else(|| KernelError::Handler {
                message: "component tool failed without an error".to_owned(),
            }))
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
        AnchoredDiff, AnchoredText, BoxFuture, EditResult, FileHandler, GrepSource, Handler,
        HandlerDescriptor, KernelError, KernelHandle, Operation, OperationResult, ReadResult,
        Request, ResourceAddress, ResourceUri, ToolDefinition, ToolProvider, TypedHandler,
        Verb as KernelVerb, WriteResult,
    };
    use serde_json::Value;
    use std::{
        collections::HashMap,
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
        files: FileHandler,
        root: PathBuf,
        registry: ComponentRegistry,
        options: BuildOptions,
        dirty: Arc<Mutex<std::collections::HashSet<PathBuf>>>,
        known_packages: Arc<Mutex<HashMap<String, (PathBuf, ToolRegistration)>>>,
    }

    impl Clone for ToolsHandler {
        fn clone(&self) -> Self {
            Self {
                files: FileHandler::new(&self.root)
                    .expect("a cloned tools handler must retain its filesystem root"),
                root: self.root.clone(),
                registry: self.registry.clone(),
                options: self.options.clone(),
                dirty: Arc::clone(&self.dirty),
                known_packages: Arc::clone(&self.known_packages),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    pub struct ToolRegistration {
        pub package: String,
        pub contract: super::contracts::ContractId,
        pub description: String,
        pub version: String,
        pub input_schema: Option<serde_yaml::Value>,
        pub output_schema: Option<serde_yaml::Value>,
        pub capabilities: Vec<String>,
    }

    impl ToolRegistration {
        pub fn tool_name(&self) -> String {
            self.contract
                .verb
                .map(|verb| verb.to_string())
                .unwrap_or_else(|| self.contract.interface.clone())
        }

        fn parameters(&self) -> Value {
            self.input_schema
                .as_ref()
                .and_then(|schema| serde_json::to_value(schema).ok())
                .unwrap_or_else(|| {
                    match self.contract.verb {
                        Some(super::contracts::Verb::Read) => serde_json::json!({"type":"object","properties":{"requests":{"type":"array"},"uri":{"type":"string"},"at":{},"before":{"type":"integer","minimum":0},"after":{"type":"integer","minimum":0}},"oneOf":[{"required":["requests"]},{"required":["uri"]}]}),
                        Some(super::contracts::Verb::Write) => serde_json::json!({"type":"object","properties":{"requests":{"type":"array"},"uri":{"type":"string"},"content":{"type":"string"}},"oneOf":[{"required":["requests"]},{"required":["uri","content"]}]}),
                        Some(super::contracts::Verb::Edit) => serde_json::json!({"type":"object","properties":{"requests":{"type":"array"},"uri":{"type":"string"},"operations":{"type":"array"}},"oneOf":[{"required":["requests"]},{"required":["uri","operations"]}]}),
                        Some(super::contracts::Verb::Find) => serde_json::json!({"type":"object","required":["roots","query"],"properties":{"roots":{"type":"array","items":{"type":"string"}},"query":{"type":"string"}}}),
                        Some(super::contracts::Verb::Grep) => serde_json::json!({"type":"object","required":["pattern","source"],"properties":{"pattern":{"type":"string"},"source":{}}}),
                        Some(super::contracts::Verb::Poll) => serde_json::json!({"type":"object","required":["targets"],"properties":{"targets":{"type":"array"},"until":{}}}),
                        Some(super::contracts::Verb::Run) | Some(super::contracts::Verb::Send) => serde_json::json!({"type":"object","required":["requests"],"properties":{"requests":{"type":"array"}}}),
                        Some(super::contracts::Verb::Abort) | Some(super::contracts::Verb::Delete) => serde_json::json!({"type":"object","required":["uris"],"properties":{"uris":{"type":"array","items":{"type":"string"}}}}),
                        None => serde_json::json!({"type":"object","additionalProperties":true}),
                    }
                })
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
            Ok(Self {
                files: FileHandler::new(&root)?,
                root,
                registry: ComponentRegistry::new(),
                options: BuildOptions {
                    granted_capabilities: granted_capabilities.into_iter().collect(),
                    ..BuildOptions::default()
                },
                dirty: watcher.map_or_else(
                    || Arc::new(Mutex::new(std::collections::HashSet::new())),
                    super::watcher::SharedWatcher::dirty_set,
                ),
                known_packages: Arc::new(Mutex::new(HashMap::new())),
            })
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
                    contract,
                    description: package.manifest.description,
                    version: package.manifest.version,
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

        fn relative_path(&self, target: &ResourceAddress) -> Result<PathBuf, KernelError> {
            let uri = target.as_uri().ok_or_else(|| KernelError::InvalidUri {
                message: format!("tools handler requires a tools:// URI, got {target}"),
            })?;
            if uri.scheme() != "tools" {
                return Err(KernelError::UnsupportedUri {
                    uri: target.to_string(),
                });
            }
            let host = uri.as_ref().host_str().unwrap_or_default();
            let path = format!("{host}{}", uri.path())
                .trim_start_matches('/')
                .to_owned();
            if path.is_empty() {
                return Err(KernelError::InvalidRequest {
                    message: "tools:// target must name a package or package file".to_owned(),
                });
            }
            Ok(PathBuf::from(path))
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

        fn map_typed_operation(&self, operation: Operation) -> Result<Operation, KernelError> {
            let map = |uri: ResourceUri| self.map_typed_uri(&uri);
            Ok(match operation {
                Operation::Read(requests) => Operation::Read(
                    requests
                        .into_iter()
                        .map(|mut request| {
                            request.uri = map(request.uri)?;
                            Ok(request)
                        })
                        .collect::<Result<_, KernelError>>()?,
                ),
                Operation::Write(requests) => Operation::Write(
                    requests
                        .into_iter()
                        .map(|mut request| {
                            request.uri = map(request.uri)?;
                            Ok(request)
                        })
                        .collect::<Result<_, KernelError>>()?,
                ),
                Operation::Edit(requests) => Operation::Edit(
                    requests
                        .into_iter()
                        .map(|mut request| {
                            request.uri = map(request.uri)?;
                            Ok(request)
                        })
                        .collect::<Result<_, KernelError>>()?,
                ),
                Operation::Find(mut request) => {
                    request.roots = request
                        .roots
                        .into_iter()
                        .map(map)
                        .collect::<Result<_, KernelError>>()?;
                    Operation::Find(request)
                }
                Operation::Grep(mut request) => {
                    if let GrepSource::Resources(uris) = request.source {
                        request.source =
                            GrepSource::Resources(uris.into_iter().map(map).collect::<Result<
                                _,
                                KernelError,
                            >>(
                            )?);
                    }
                    Operation::Grep(request)
                }
                other => {
                    return Err(KernelError::UnsupportedVerb {
                        verb: format!("typed tools operation {other:?}"),
                        uri: "tools://".to_owned(),
                    });
                }
            })
        }

        fn unmap_text(&self, mut text: AnchoredText) -> Result<AnchoredText, KernelError> {
            text.uri = self.unmap_typed_uri(text.uri)?;
            Ok(text)
        }

        fn unmap_result(&self, result: OperationResult) -> Result<OperationResult, KernelError> {
            Ok(match result {
                OperationResult::Read(results) => OperationResult::Read(
                    results
                        .into_iter()
                        .map(|result| {
                            result.and_then(|value| match value {
                                ReadResult::Text(text) => {
                                    self.unmap_text(text).map(ReadResult::Text)
                                }
                                ReadResult::Directory { uri, entries } => {
                                    Ok(ReadResult::Directory {
                                        uri: self.unmap_typed_uri(uri)?,
                                        entries: entries
                                            .into_iter()
                                            .map(|uri| self.unmap_typed_uri(uri))
                                            .collect::<Result<_, _>>()?,
                                    })
                                }
                            })
                        })
                        .collect(),
                ),
                OperationResult::Write(results) => OperationResult::Write(
                    results
                        .into_iter()
                        .map(|result| {
                            result.and_then(|WriteResult { text }| {
                                self.unmap_text(text).map(|text| WriteResult { text })
                            })
                        })
                        .collect(),
                ),
                OperationResult::Edit(results) => OperationResult::Edit(
                    results
                        .into_iter()
                        .map(|result| {
                            result.and_then(|mut value| {
                                value.text = self.unmap_text(value.text)?;
                                value.diff = AnchoredDiff {
                                    uri: self.unmap_typed_uri(value.diff.uri)?,
                                    ..value.diff
                                };
                                Ok::<EditResult, KernelError>(value)
                            })
                        })
                        .collect(),
                ),
                OperationResult::Find(result) => OperationResult::Find(match result {
                    Ok(uris) => Ok(uris
                        .into_iter()
                        .map(|uri| self.unmap_typed_uri(uri))
                        .collect::<Result<_, _>>()?),
                    Err(error) => Err(error),
                }),
                OperationResult::Grep(result) => OperationResult::Grep(match result {
                    Ok(texts) => Ok(texts
                        .into_iter()
                        .map(|text| self.unmap_text(text))
                        .collect::<Result<_, _>>()?),
                    Err(error) => Err(error),
                }),
                other => other,
            })
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
                            return Ok(active);
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
                    .is_some_and(|contract| contract.verb.is_none())
            {
                match self.custom_dependencies(&package) {
                    Ok(dependencies) => dependencies,
                    Err(error) => return current.ok_or(error),
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
                Err(error) => return current.ok_or_else(|| component_error(error)),
            };
            self.dirty.lock().unwrap().remove(&key);
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
                if contract.verb.is_none()
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

        fn mark_typed_operation_dirty(&self, operation: &Operation) {
            let mark = |uri: &ResourceUri| {
                if let Ok(relative) = self.relative_uri_path(uri) {
                    self.mark_dirty(&relative);
                }
            };
            match operation {
                Operation::Write(requests) => {
                    requests.iter().for_each(|request| mark(&request.uri))
                }
                Operation::Edit(requests) => requests.iter().for_each(|request| mark(&request.uri)),
                _ => {}
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

        async fn execute_inner(
            &self,
            request: Request,
            host: KernelHandle,
        ) -> Result<Value, KernelError> {
            let relative = self.relative_path(&request.target)?;
            let package_root = self.package_root(&relative).ok();
            let is_package_root = relative.components().count() == 1
                && package_root
                    .as_ref()
                    .is_some_and(|root| root.join("tool.md").is_file());
            if is_package_root {
                let package_root = package_root.expect("package root checked above");
                let package = ToolPackage::discover(&package_root).map_err(component_error)?;
                let contract = package
                    .contract
                    .ok_or_else(|| KernelError::InvalidRequest {
                        message: format!(
                            "tool package {} has no registered contract",
                            package.manifest.name
                        ),
                    })?;
                let Some(contract_verb) = contract.verb else {
                    return Err(KernelError::UnsupportedVerb {
                        verb: request.verb.to_string(),
                        uri: request.target.to_string(),
                    });
                };
                if contract_verb.to_string() != request.verb.to_string() {
                    return Err(KernelError::UnsupportedVerb {
                        verb: request.verb.to_string(),
                        uri: request.target.to_string(),
                    });
                }
                let active = self.activate(&package_root)?;
                return ComponentTool::new(active, contract_verb)
                    .invoke_async(request.args.clone(), host)
                    .await;
            }

            {
                if request.verb == KernelVerb::Write {
                    self.prepare_write(&relative)?;
                }
                let mut mapped = request.clone();
                mapped.target =
                    ResourceUri::parse(&self.root.join(&relative).display().to_string())
                        .map(ResourceAddress::uri)?;
                let result = self.files.execute(mapped, host).await?;
                if matches!(request.verb, KernelVerb::Write | KernelVerb::Edit) {
                    self.mark_dirty(&relative);
                }
                return Ok(result);
            }
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
            let package_root = self.package_path_for_name(&registration.package)?;
            let active = self.activate(&package_root)?;
            if let Some(verb) = registration.contract.verb {
                return ComponentTool::new(active, verb)
                    .invoke_async_with_scope(args, host, scope.clone())
                    .await;
            }

            // Custom package-local contracts remain on the quarantined
            // reflective adapter until their direct generated WIT invocation
            // is migrated. Universal named tools never take this branch.
            active
                .invoke_dynamic_json_async_with_scope(
                    &args,
                    &registration.contract.interface,
                    host,
                    scope,
                )
                .await
                .map_err(component_error)
        }
    }

    impl Handler for ToolsHandler {
        fn descriptor(&self) -> HandlerDescriptor {
            HandlerDescriptor {
                name: "tools".to_owned(),
                schemes: vec!["tools".to_owned()],
                verbs: vec![
                    KernelVerb::Read,
                    KernelVerb::Write,
                    KernelVerb::Edit,
                    KernelVerb::Find,
                    KernelVerb::Grep,
                    KernelVerb::Run,
                ],
            }
        }

        fn execute<'a>(
            &'a self,
            request: Request,
            host: KernelHandle,
        ) -> BoxFuture<'a, Result<Value, KernelError>> {
            Box::pin(self.execute_inner(request, host))
        }
    }

    impl TypedHandler for ToolsHandler {
        fn descriptor(&self) -> HandlerDescriptor {
            HandlerDescriptor {
                name: "tools-typed".to_owned(),
                schemes: vec!["tools".to_owned()],
                verbs: vec![
                    KernelVerb::Read,
                    KernelVerb::Write,
                    KernelVerb::Edit,
                    KernelVerb::Find,
                    KernelVerb::Grep,
                ],
            }
        }

        fn claims_operation(&self, operation: &Operation) -> bool {
            match operation {
                Operation::Read(requests) => {
                    !requests.is_empty()
                        && requests
                            .iter()
                            .all(|request| request.uri.scheme() == "tools")
                }
                Operation::Write(requests) => {
                    !requests.is_empty()
                        && requests
                            .iter()
                            .all(|request| request.uri.scheme() == "tools")
                }
                Operation::Edit(requests) => {
                    !requests.is_empty()
                        && requests
                            .iter()
                            .all(|request| request.uri.scheme() == "tools")
                }
                Operation::Find(request) => {
                    !request.roots.is_empty()
                        && request.roots.iter().all(|uri| uri.scheme() == "tools")
                }
                Operation::Grep(request) => matches!(
                    &request.source,
                    GrepSource::Resources(uris)
                        if !uris.is_empty() && uris.iter().all(|uri| uri.scheme() == "tools")
                ),
                _ => false,
            }
        }

        fn execute_typed<'a>(
            &'a self,
            operation: Operation,
            host: KernelHandle,
            _context: artist_kernel::InvocationContext,
        ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
            Box::pin(async move {
                self.mark_typed_operation_dirty(&operation);
                let mapped = self.map_typed_operation(operation)?;
                let result = self.files.execute_typed(mapped, host, _context).await?;
                self.unmap_result(result)
            })
        }
    }

    impl ToolProvider for ToolsHandler {
        fn tool_definitions(&self) -> Vec<ToolDefinition> {
            self.registrations()
                .unwrap_or_default()
                .into_iter()
                .map(|registration| ToolDefinition {
                    name: registration.tool_name(),
                    description: registration.description.clone(),
                    parameters: registration.parameters(),
                })
                .collect()
        }

        fn execute_tool<'a>(
            &'a self,
            name: &'a str,
            args: Value,
            host: KernelHandle,
        ) -> BoxFuture<'a, Result<Value, KernelError>> {
            Box::pin(async move {
                self.execute_named_inner_async(
                    name,
                    args,
                    host,
                    artist_kernel::InvocationContext::default(),
                )
                .await
            })
        }

        fn execute_tool_with_context<'a>(
            &'a self,
            name: &'a str,
            args: Value,
            host: KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> BoxFuture<'a, Result<Value, KernelError>> {
            Box::pin(async move {
                self.execute_named_inner_async(name, args, host, context)
                    .await
            })
        }

        fn execute_tool_with_scope<'a>(
            &'a self,
            name: &'a str,
            args: Value,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> BoxFuture<'a, Result<Value, KernelError>> {
            Box::pin(async move {
                self.execute_named_inner_async_scope(name, args, host, scope)
                    .await
            })
        }
    }

    fn component_error(error: super::ComponentError) -> KernelError {
        KernelError::Handler {
            message: error.to_string(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use artist_kernel::{Kernel, Verb as KernelVerb, WriteRequest};
        use tempfile::tempdir;

        #[derive(Clone, Default)]
        struct NamedFakeNamespace {
            resources: Arc<Mutex<std::collections::HashMap<String, String>>>,
        }

        impl TypedHandler for NamedFakeNamespace {
            fn descriptor(&self) -> HandlerDescriptor {
                HandlerDescriptor {
                    name: "named-fake".to_owned(),
                    schemes: vec!["fake".to_owned()],
                    verbs: vec![KernelVerb::Run, KernelVerb::Send],
                }
            }

            fn claims_operation(&self, operation: &Operation) -> bool {
                let uris = match operation {
                    Operation::Run(requests) => requests
                        .iter()
                        .map(|request| &request.uri)
                        .collect::<Vec<_>>(),
                    Operation::Send(requests) => requests
                        .iter()
                        .map(|request| &request.uri)
                        .collect::<Vec<_>>(),
                    _ => return false,
                };
                !uris.is_empty() && uris.iter().all(|uri| uri.scheme() == "fake")
            }

            fn execute_typed<'a>(
                &'a self,
                operation: Operation,
                _host: KernelHandle,
                _context: artist_kernel::InvocationContext,
            ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
                let resources = Arc::clone(&self.resources);
                Box::pin(async move {
                    let mut resources = resources.lock().unwrap();
                    match operation {
                        Operation::Run(mut requests) => {
                            let request = requests.pop().unwrap();
                            let uri = request.uri.to_string();
                            if resources.contains_key(&uri) {
                                return Err(KernelError::Conflict { uri });
                            }
                            resources.insert(uri.clone(), String::new());
                            Ok(OperationResult::Run(vec![Ok(request.uri)]))
                        }
                        Operation::Send(requests) => Ok(OperationResult::Send(
                            requests
                                .into_iter()
                                .map(|request| {
                                    let uri = request.uri.to_string();
                                    resources.entry(uri).or_default().push_str(&request.content);
                                    Ok(request.uri)
                                })
                                .collect(),
                        )),
                        _ => unreachable!(),
                    }
                })
            }
        }

        #[tokio::test]
        async fn exposes_tool_packages_as_virtual_files() {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs");
            let handler = ToolsHandler::new(&root, Vec::<String>::new()).unwrap();
            let kernel = Kernel::new();
            let result = handler
                .execute(
                    Request::new(
                        KernelVerb::Read,
                        ResourceUri::parse("tools://read/tool.md").unwrap(),
                        Value::Null,
                    ),
                    kernel.handle(),
                )
                .await
                .unwrap();
            assert_eq!(result["value"]["type"], "text");
            assert!(
                result["value"]["content"]
                    .as_str()
                    .unwrap()
                    .contains("contract: artist:tool:read@1")
            );
        }

        #[tokio::test]
        async fn writes_new_tool_package_files_under_the_virtual_mount() {
            let root = tempdir().unwrap();
            let handler = ToolsHandler::new(root.path(), Vec::<String>::new()).unwrap();
            let kernel = Kernel::new();
            let result = handler
                .execute(
                    Request::new(
                        KernelVerb::Write,
                        ResourceUri::parse("tools://new-tool/tool.md").unwrap(),
                        serde_json::json!({"value":"---\nname: new-tool\ndescription: test\nversion: 0.1.0\n---\n"}),
                    ),
                    kernel.handle(),
                )
                .await
                .unwrap();
            assert_eq!(result["written"], true);
            assert!(root.path().join("new-tool/tool.md").is_file());
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
            let files_root = tempdir().unwrap();
            let source = files_root.path().join("note.txt");
            std::fs::write(&source, "tools namespace\n").unwrap();

            let kernel = Kernel::new();
            kernel
                .register(artist_kernel::FileHandler::new(files_root.path()).unwrap())
                .await;
            kernel
                .register_typed(artist_kernel::FileHandler::new(files_root.path()).unwrap())
                .await;
            kernel
                .register(ToolsHandler::new(&source_root, ["resource.read".to_owned()]).unwrap())
                .await;

            let result = kernel
                .execute(Request::new(
                    KernelVerb::Read,
                    ResourceUri::parse("tools://read").unwrap(),
                    serde_json::json!({"target": source.to_string_lossy()}),
                ))
                .await;
            assert!(result.ok, "tool invocation failed: {result:?}");
            assert!(
                result
                    .value
                    .unwrap()
                    .to_string()
                    .contains("tools namespace")
            );
        }

        #[tokio::test]
        async fn typed_tools_search_maps_virtual_uris_without_leaking_file_uris() {
            let root = tempdir().unwrap();
            std::fs::create_dir_all(root.path().join("pkg")).unwrap();
            std::fs::write(root.path().join("pkg/note.txt"), "virtual needle\n").unwrap();
            let handler = ToolsHandler::new(root.path(), Vec::<String>::new()).unwrap();
            let kernel = Kernel::new();
            kernel.register_typed(handler).await;

            let found = kernel
                .execute_operation(artist_kernel::Operation::Find(artist_kernel::FindRequest {
                    roots: vec![ResourceUri::parse("tools:///").unwrap()],
                    query: "note".to_owned(),
                }))
                .await
                .unwrap();
            let artist_kernel::OperationResult::Find(Ok(paths)) = found else {
                panic!("unexpected typed find result: {found:?}");
            };
            assert_eq!(
                paths,
                vec![ResourceUri::parse("tools:///pkg/note.txt").unwrap()]
            );

            let grep = kernel
                .execute_operation(artist_kernel::Operation::Grep(artist_kernel::GrepRequest {
                    pattern: "needle".to_owned(),
                    source: artist_kernel::GrepSource::Resources(vec![
                        ResourceUri::parse("tools:///pkg/note.txt").unwrap(),
                    ]),
                }))
                .await
                .unwrap();
            let artist_kernel::OperationResult::Grep(Ok(matches)) = grep else {
                panic!("unexpected typed grep result: {grep:?}");
            };
            assert_eq!(
                matches[0].uri,
                ResourceUri::parse("tools:///pkg/note.txt").unwrap()
            );
            assert_eq!(matches[0].lines[0].text, "virtual needle");
        }

        #[tokio::test]
        async fn exposes_named_tools_and_dispatches_against_an_ordinary_file() {
            let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs");
            let files_root = tempdir().unwrap();
            let source = files_root.path().join("named.txt");
            std::fs::write(&source, "named tool dispatch\n").unwrap();

            let kernel = Kernel::new();
            kernel
                .register(artist_kernel::FileHandler::new(files_root.path()).unwrap())
                .await;
            kernel
                .register_typed(artist_kernel::FileHandler::new(files_root.path()).unwrap())
                .await;
            kernel
                .register_tool_handler(
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
                    serde_json::json!({"target": source.to_string_lossy()}),
                )
                .await
                .unwrap();
            assert!(result.to_string().contains("named tool dispatch"));
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

            let files_root = tempdir().unwrap();
            let target = files_root.path().join("target.txt");
            std::fs::write(&target, "generation one\n").unwrap();
            let kernel = Kernel::new();
            kernel
                .register_typed(artist_kernel::FileHandler::new(files_root.path()).unwrap())
                .await;
            let tools = ToolsHandler::new(tools_root.path(), ["resource.read".to_owned()]).unwrap();
            let observed_tools = tools.clone();
            kernel.register_typed_tool_handler(tools).await;

            let input = serde_json::json!({"target": target.to_string_lossy()});
            let first = kernel.execute_tool("read", input.clone()).await.unwrap();
            assert!(first.to_string().contains("generation one"));
            let first_generation = observed_tools
                .registry
                .current_generation("artist-tool-read")
                .expect("first generation active");

            let edit = kernel
                .execute_operation(Operation::Write(vec![WriteRequest {
                    uri: ResourceUri::parse("tools://read/tool.md").unwrap(),
                    content: "not valid frontmatter".to_owned(),
                }]))
                .await
                .unwrap();
            assert!(matches!(
                edit,
                OperationResult::Write(values)
                    if matches!(values.as_slice(), [Ok(WriteResult { .. })])
            ));
            assert_eq!(
                std::fs::read_to_string(package_root.join("tool.md")).unwrap(),
                "not valid frontmatter"
            );
            let second = kernel.execute_tool("read", input).await.unwrap();
            assert!(second.to_string().contains("generation one"));

            // A later valid self-edit must use the same handler allocation and
            // activate a new generation on the named-tool path.  Checking the
            // catalog as well as executing the tool catches the split-brain
            // failure where typed writes dirty one ToolsHandler while named
            // execution consults another.
            let repaired = "---\nname: artist-tool-read\ndescription: generation two\nversion: 0.1.1\ncontract: artist:tool:read@1\ncapabilities:\n  - resource.read\n---\n";
            let repair = kernel
                .execute_operation(Operation::Write(vec![WriteRequest {
                    uri: ResourceUri::parse("tools://read/tool.md").unwrap(),
                    content: repaired.to_owned(),
                }]))
                .await
                .unwrap();
            assert!(matches!(
                repair,
                OperationResult::Write(values)
                    if matches!(values.as_slice(), [Ok(WriteResult { .. })])
            ));
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
                    serde_json::json!({"target": target.to_string_lossy()}),
                )
                .await
                .unwrap();
            assert!(third.to_string().contains("generation one"));
            let second_generation = observed_tools
                .registry
                .current_generation("artist-tool-read")
                .expect("repaired generation active");
            assert!(second_generation > first_generation);
        }

        #[tokio::test]
        async fn named_wasm_run_and_send_reach_a_scheme_claiming_namespace() {
            let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs");
            let namespace = NamedFakeNamespace::default();
            let kernel = Kernel::new();
            kernel.register_typed(namespace.clone()).await;
            kernel
                .register_tool_handler(
                    ToolsHandler::new(
                        &source_root,
                        ["resource.run".to_owned(), "resource.send".to_owned()],
                    )
                    .unwrap(),
                )
                .await;

            let uri = "fake://named-shell";
            let run = kernel
                .execute_tool(
                    "run",
                    serde_json::json!({"requests":[{"uri":uri,"args":[]}]}),
                )
                .await
                .unwrap();
            assert!(run.to_string().contains(uri));
            let send = kernel
                .execute_tool(
                    "send",
                    serde_json::json!({"requests":[{"uri":uri,"content":"cargo test\n"}]}),
                )
                .await
                .unwrap();
            assert!(send.to_string().contains(uri));
            assert_eq!(
                namespace.resources.lock().unwrap().get(uri).unwrap(),
                "cargo test\n"
            );
        }
    }
}

/// Bootstrap support for the self-describing `resources://` package namespace.
/// Package discovery/cataloging is deliberately independent of claim routing;
/// this keeps documentation and package-file inspection available before any
/// resource component is activated.
pub mod resources {
    use artist_kernel::{
        BoxFuture, FileHandler, Handler, HandlerDescriptor, KernelError, KernelHandle, Operation,
        OperationResult, Request, ResourceAddress, ResourceCatalogDoc, ResourceCatalogEntry,
        ResourceCatalogProvider, ResourceUri, TypedHandler, Verb,
    };
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use std::{
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

    macro_rules! async_resource_uri_list_call {
        (
            $method:ident,
            $module:ident,
            $world:ident,
            $interface:ident,
            $call:ident
        ) => {
            async fn $method(
                &self,
                uris: Vec<String>,
                kernel: KernelHandle,
                scope: artist_kernel::InvocationScope,
            ) -> Result<
                Vec<Result<String, super::$module::artist::resource::types::Error>>,
                KernelError,
            > {
                let failure_scope = scope.clone();
                let mut store = super::new_store(
                    &self.engine,
                    super::HostState::with_kernel_scope(self.capabilities.clone(), kernel, scope),
                );
                let mut linker = wasmtime::component::Linker::new(&self.engine);
                wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                    KernelError::Handler {
                        message: format!(
                            "link resource {} WASI imports: {error}",
                            stringify!($method)
                        ),
                    }
                })?;
                Self::link_nested_standard_imports(&mut linker)?;
                self.link_custom_imports(&mut linker)?;
                let instance =
                    super::$module::$world::instantiate_async(&mut store, &self.component, &linker)
                        .await
                        .map_err(|error| {
                            wasm_invocation_failure(
                                &failure_scope,
                                format!("resource {} instantiation failed", stringify!($method)),
                                error,
                            )
                        })?;
                instance
                    .$interface()
                    .$call(&mut store, &uris)
                    .await
                    .map_err(|error| {
                        wasm_invocation_failure(
                            &failure_scope,
                            format!("resource {} failed", stringify!($method)),
                            error,
                        )
                    })
            }
        };
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

        fn link_nested_standard_imports(
            linker: &mut wasmtime::component::Linker<super::HostState>,
        ) -> Result<(), KernelError> {
            let mut filesystem =
                linker
                    .instance("artist:resource/filesystem")
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
            macro_rules! link {
                ($interface:ident) => {
                    super::resource_async_host_bindings::artist::resource::$interface::add_to_linker::<
                        super::HostState,
                        wasmtime::component::HasSelf<super::HostState>,
                    >(linker, |state: &mut super::HostState| state)
                    .map_err(|error| KernelError::Handler {
                        message: format!(
                            "link nested resource {}: {error}",
                            stringify!($interface)
                        ),
                    })?;
                };
            }
            link!(read);
            link!(write);
            link!(edit);
            link!(find);
            link!(grep);
            link!(run);
            link!(send);
            link!(abort);
            link!(delete);
            link!(poll);
            Ok(())
        }

        async fn claim(
            &self,
            verb: Verb,
            uri: &ResourceUri,
        ) -> Result<artist_kernel::ClaimDecision, KernelError> {
            let claim_context = artist_kernel::InvocationContext {
                deadline_ms: Some(50),
                ..Default::default()
            };
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_scope(
                    std::iter::empty::<String>(),
                    artist_kernel::InvocationScope::new(claim_context),
                ),
            );
            store
                .set_fuel(1_000_000)
                .map_err(|error| KernelError::Handler {
                    message: format!("configure resource claim fuel: {error}"),
                })?;
            // Claim is deliberately a capability-free phase. The linker only
            // supplies empty plumbing for imports retained by the concrete
            // component; the claim state has no kernel or useful authority.
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            // WASI is linked with an empty capability context for Rust
            // component plumbing (the claim store still has no filesystem,
            // process, network, or kernel authority). No useful WASI handle
            // is available during claim.
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link claim WASI imports: {error}"),
                }
            })?;
            // Concrete resource worlds may import standard interfaces even
            // though claim itself is forbidden from using them. The host
            // implementation sees a claim-phase state with no kernel, so any
            // attempted nested call is denied rather than executed.
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance =
                super::resource_async_extension_bindings::ExtensionWorld::instantiate_async(
                    &mut store,
                    &self.component,
                    &linker,
                )
                .await
                .map_err(|error| KernelError::Handler {
                    message: format!("resource claim instantiation failed: {error}"),
                })?;
            let decision = instance
                    .artist_resource_extension()
                    .call_claim(
                        &mut store,
                        &super::resource_async_extension_bindings::artist::resource::types::ClaimRequest {
                            verb: match verb {
                                Verb::Read => super::resource_async_extension_bindings::artist::resource::types::Verb::Read,
                                Verb::Write => super::resource_async_extension_bindings::artist::resource::types::Verb::Write,
                                Verb::Edit => super::resource_async_extension_bindings::artist::resource::types::Verb::Edit,
                                Verb::Poll => super::resource_async_extension_bindings::artist::resource::types::Verb::Poll,
                                Verb::Send => super::resource_async_extension_bindings::artist::resource::types::Verb::Send,
                                Verb::Run => super::resource_async_extension_bindings::artist::resource::types::Verb::Run,
                                Verb::Abort => super::resource_async_extension_bindings::artist::resource::types::Verb::Abort,
                                Verb::Delete => super::resource_async_extension_bindings::artist::resource::types::Verb::Delete,
                                Verb::Find => super::resource_async_extension_bindings::artist::resource::types::Verb::Find,
                                Verb::Grep => super::resource_async_extension_bindings::artist::resource::types::Verb::Grep,
                            },
                            uri: uri.to_string(),
                        },
                    )
                    .await
                    .map_err(|error| KernelError::Handler {
                        message: format!("resource claim failed: {error}"),
                    })?;
            Ok(match decision {
                    super::resource_async_extension_bindings::artist::resource::types::ClaimDecision::Pass => artist_kernel::ClaimDecision::Pass,
                    super::resource_async_extension_bindings::artist::resource::types::ClaimDecision::Handle => artist_kernel::ClaimDecision::Handle,
                    super::resource_async_extension_bindings::artist::resource::types::ClaimDecision::Reserve => artist_kernel::ClaimDecision::Reserve,
            })
        }

        async fn invoke_read_typed(
            &self,
            requests: Vec<artist_kernel::ReadRequest>,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<Result<artist_kernel::ReadResult, KernelError>>, KernelError> {
            let failure_scope = scope.clone();
            let wire_requests = requests
                .iter()
                .map(|request| {
                    super::resource_async_tool_v1_bindings::artist::resource::types::ReadRequest {
                        uri: request.uri.to_string(),
                        at: request.at.as_ref().map(|position| match position {
                            artist_kernel::Position::Top => super::resource_async_tool_v1_bindings::artist::resource::types::Position::Top,
                            artist_kernel::Position::Bottom => super::resource_async_tool_v1_bindings::artist::resource::types::Position::Bottom,
                            artist_kernel::Position::At(anchor) => super::resource_async_tool_v1_bindings::artist::resource::types::Position::At(anchor.to_string()),
                        }),
                        before: request.before,
                        after: request.after,
                    }
                })
                .collect::<Vec<_>>();
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_kernel_scope(self.capabilities.clone(), kernel, scope),
            );
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link resource read WASI imports: {error}"),
                }
            })?;
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance =
                super::resource_async_tool_v1_bindings::ResourceReadWorld::instantiate_async(
                    &mut store,
                    &self.component,
                    &linker,
                )
                .await
                .map_err(|error| {
                    wasm_invocation_failure(
                        &failure_scope,
                        "resource read instantiation failed",
                        error,
                    )
                })?;
            instance
                .artist_resource_read()
                .call_read(&mut store, &wire_requests)
                .await
                .map_err(|error| {
                    wasm_invocation_failure(&failure_scope, "resource read failed", error)
                })?
                .into_iter()
                .map(|result| {
                    result
                        .map(super::resource_read_result_to_kernel)
                        .map_err(super::resource_error_to_kernel)
                })
                .collect()
        }

        async fn invoke_write_typed(
            &self,
            requests: Vec<artist_kernel::WriteRequest>,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<Result<artist_kernel::WriteResult, KernelError>>, KernelError> {
            let failure_scope = scope.clone();
            let wire_requests = requests
                .iter()
                .map(|request| {
                    super::resource_async_tool_v1_write_bindings::artist::resource::types::WriteRequest {
                        uri: request.uri.to_string(),
                        content: request.content.clone(),
                    }
                })
                .collect::<Vec<_>>();
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_kernel_scope(self.capabilities.clone(), kernel, scope),
            );
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link resource write WASI imports: {error}"),
                }
            })?;
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance =
                super::resource_async_tool_v1_write_bindings::ResourceWriteWorld::instantiate_async(
                    &mut store,
                    &self.component,
                    &linker,
                )
                .await
                .map_err(|error| {
                    wasm_invocation_failure(
                        &failure_scope,
                        "resource write instantiation failed",
                        error,
                    )
                })?;
            instance
                .artist_resource_write()
                .call_write(&mut store, &wire_requests)
                .await
                .map_err(|error| {
                    wasm_invocation_failure(&failure_scope, "resource write failed", error)
                })?
                .into_iter()
                .map(|result| {
                    result
                        .map(|value| super::resource_write_result_to_kernel(value))
                        .map_err(super::resource_write_error_to_kernel)
                })
                .collect()
        }

        async fn invoke_edit_typed(
            &self,
            requests: Vec<artist_kernel::EditRequest>,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<Result<artist_kernel::EditResult, KernelError>>, KernelError> {
            let failure_scope = scope.clone();
            let wire_requests = super::resource_edit_requests_from_kernel(requests)?;
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_kernel_scope(self.capabilities.clone(), kernel, scope),
            );
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link resource edit WASI imports: {error}"),
                }
            })?;
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance =
                super::resource_async_tool_v1_edit_bindings::ResourceEditWorld::instantiate_async(
                    &mut store,
                    &self.component,
                    &linker,
                )
                .await
                .map_err(|error| {
                    wasm_invocation_failure(
                        &failure_scope,
                        "resource edit instantiation failed",
                        error,
                    )
                })?;
            instance
                .artist_resource_edit()
                .call_edit(&mut store, &wire_requests)
                .await
                .map_err(|error| {
                    wasm_invocation_failure(&failure_scope, "resource edit failed", error)
                })?
                .into_iter()
                .map(|result| match result {
                    Ok(value) => Ok(Ok(super::resource_edit_result_to_kernel(value)?)),
                    Err(error) => Ok(Err(super::resource_error_to_kernel_for(error, "edit"))),
                })
                .collect()
        }

        async fn invoke_run_typed(
            &self,
            requests: Vec<artist_kernel::RunRequest>,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<Result<artist_kernel::ResourceUri, KernelError>>, KernelError> {
            let failure_scope = scope.clone();
            let wire_requests = super::resource_run_requests_from_kernel(requests);
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_kernel_scope(self.capabilities.clone(), kernel, scope),
            );
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link resource run WASI imports: {error}"),
                }
            })?;
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance =
                super::resource_async_tool_v1_run_bindings::ResourceRunWorld::instantiate_async(
                    &mut store,
                    &self.component,
                    &linker,
                )
                .await
                .map_err(|error| {
                    wasm_invocation_failure(
                        &failure_scope,
                        "resource run instantiation failed",
                        error,
                    )
                })?;
            let result = instance
                .artist_resource_run()
                .call_run(&mut store, &wire_requests)
                .await
                .map_err(|error| {
                    wasm_invocation_failure(&failure_scope, "resource run failed", error)
                })?;
            super::resource_uri_results_to_kernel(result, "run")
        }

        async fn invoke_send_typed(
            &self,
            requests: Vec<artist_kernel::SendRequest>,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<Result<artist_kernel::ResourceUri, KernelError>>, KernelError> {
            let failure_scope = scope.clone();
            let wire_requests = super::resource_send_requests_from_kernel(requests);
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_kernel_scope(self.capabilities.clone(), kernel, scope),
            );
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link resource send WASI imports: {error}"),
                }
            })?;
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance =
                super::resource_async_tool_v1_send_bindings::ResourceSendWorld::instantiate_async(
                    &mut store,
                    &self.component,
                    &linker,
                )
                .await
                .map_err(|error| {
                    wasm_invocation_failure(
                        &failure_scope,
                        "resource send instantiation failed",
                        error,
                    )
                })?;
            let result = instance
                .artist_resource_send()
                .call_send(&mut store, &wire_requests)
                .await
                .map_err(|error| {
                    wasm_invocation_failure(&failure_scope, "resource send failed", error)
                })?;
            super::resource_send_results_to_kernel(result)
        }

        async fn invoke_find_typed(
            &self,
            request: artist_kernel::FindRequest,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<artist_kernel::ResourceUri>, KernelError> {
            let failure_scope = scope.clone();
            let wire_request = super::resource_find_request_from_kernel(request);
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_kernel_scope(self.capabilities.clone(), kernel, scope),
            );
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link resource find WASI imports: {error}"),
                }
            })?;
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance =
                super::resource_async_tool_v1_find_bindings::ResourceFindWorld::instantiate_async(
                    &mut store,
                    &self.component,
                    &linker,
                )
                .await
                .map_err(|error| {
                    wasm_invocation_failure(
                        &failure_scope,
                        "resource find instantiation failed",
                        error,
                    )
                })?;
            let result = instance
                .artist_resource_find()
                .call_find(&mut store, &wire_request)
                .await
                .map_err(|error| {
                    wasm_invocation_failure(&failure_scope, "resource find failed", error)
                })?;
            super::resource_find_result_to_kernel(result)
        }

        async fn invoke_grep_typed(
            &self,
            request: artist_kernel::GrepRequest,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<Vec<artist_kernel::AnchoredText>, KernelError> {
            let failure_scope = scope.clone();
            let wire_request = super::resource_grep_request_from_kernel(request);
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_kernel_scope(self.capabilities.clone(), kernel, scope),
            );
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link resource grep WASI imports: {error}"),
                }
            })?;
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance =
                super::resource_async_tool_v1_grep_bindings::ResourceGrepWorld::instantiate_async(
                    &mut store,
                    &self.component,
                    &linker,
                )
                .await
                .map_err(|error| {
                    wasm_invocation_failure(
                        &failure_scope,
                        "resource grep instantiation failed",
                        error,
                    )
                })?;
            let result = instance
                .artist_resource_grep()
                .call_grep(&mut store, &wire_request)
                .await
                .map_err(|error| {
                    wasm_invocation_failure(&failure_scope, "resource grep failed", error)
                })?;
            super::resource_grep_results_to_kernel(result)
        }

        async fn invoke_poll_typed(
            &self,
            request: artist_kernel::PollRequest,
            kernel: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> Result<artist_kernel::PollResult, KernelError> {
            let failure_scope = scope.clone();
            let wire_request = super::resource_poll_request_from_kernel(request);
            let mut store = super::new_store(
                &self.engine,
                super::HostState::with_kernel_scope(self.capabilities.clone(), kernel, scope),
            );
            let mut linker = wasmtime::component::Linker::new(&self.engine);
            wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|error| {
                KernelError::Handler {
                    message: format!("link resource poll WASI imports: {error}"),
                }
            })?;
            Self::link_nested_standard_imports(&mut linker)?;
            self.link_custom_imports(&mut linker)?;
            let instance =
                super::resource_async_tool_v1_poll_bindings::ResourcePollWorld::instantiate_async(
                    &mut store,
                    &self.component,
                    &linker,
                )
                .await
                .map_err(|error| {
                    wasm_invocation_failure(
                        &failure_scope,
                        "resource poll instantiation failed",
                        error,
                    )
                })?;
            let result = instance
                .artist_resource_poll()
                .call_poll(&mut store, &wire_request)
                .await
                .map_err(|error| {
                    wasm_invocation_failure(&failure_scope, "resource poll failed", error)
                })?;
            super::resource_poll_result_to_kernel(result)
        }

        async_resource_uri_list_call!(
            invoke_abort_typed,
            resource_async_tool_v1_abort_bindings,
            ResourceAbortWorld,
            artist_resource_abort,
            call_abort
        );
        async_resource_uri_list_call!(
            invoke_delete_typed,
            resource_async_tool_v1_delete_bindings,
            ResourceDeleteWorld,
            artist_resource_delete,
            call_delete
        );
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
                Self::validate_resource_wit(&wit, &parsed.frontmatter.capabilities)?;
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
            let canonical = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit/resource-surface");
            let mut resolve = wit_parser::Resolve::default();
            resolve
                .push_dir(canonical)
                .map_err(|error| KernelError::InvalidRequest {
                    message: format!("parse canonical resource WIT: {error}"),
                })?;
            let package = resolve
                .push_file(path)
                .map_err(|error| KernelError::InvalidRequest {
                    message: format!("parse resource.wit {}: {error}", path.display()),
                })?;
            let world = resolve.select_world(&[package], None).map_err(|error| {
                KernelError::InvalidRequest {
                    message: format!("resource.wit has no resolvable world: {error}"),
                }
            })?;
            let mut imports = std::collections::HashSet::new();
            for item in resolve.worlds[world].imports.values() {
                let wit_parser::WorldItem::Interface { id, .. } = item else {
                    continue;
                };
                let interface = &resolve.interfaces[*id];
                if interface.package.is_some_and(|package| {
                    let name = &resolve.packages[package].name;
                    name.namespace == "artist" && name.name == "resource"
                }) && interface.name.as_deref().is_some_and(|name| {
                    super::contracts::Verb::ALL
                        .iter()
                        .any(|verb| verb.to_string() == name)
                }) {
                    imports.insert(interface.name.clone().unwrap());
                }
            }
            Ok(imports)
        }

        fn validate_resource_wit(path: &Path, capabilities: &[String]) -> Result<(), KernelError> {
            let canonical = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit/resource-surface");
            let mut resolve = wit_parser::Resolve::default();
            resolve
                .push_dir(canonical)
                .map_err(|error| KernelError::InvalidRequest {
                    message: format!("parse canonical resource WIT: {error}"),
                })?;
            let package = resolve
                .push_file(path)
                .map_err(|error| KernelError::InvalidRequest {
                    message: format!("parse resource.wit {}: {error}", path.display()),
                })?;
            let world = resolve.select_world(&[package], None).map_err(|error| {
                KernelError::InvalidRequest {
                    message: format!("resource.wit has no resolvable world: {error}"),
                }
            })?;
            for item in resolve.worlds[world].imports.values() {
                let wit_parser::WorldItem::Interface {
                    id: interface_id, ..
                } = item
                else {
                    continue;
                };
                let interface = &resolve.interfaces[*interface_id];
                let Some(package_id) = interface.package else {
                    continue;
                };
                let package_name = &resolve.packages[package_id].name;
                if package_name.namespace != "artist" || package_name.name != "resource" {
                    continue;
                }
                let Some(interface_name) = interface.name.as_deref() else {
                    continue;
                };
                // `artist:resource/types` is shared data, not an executable
                // nested operation and therefore does not require authority.
                if !super::contracts::Verb::ALL
                    .iter()
                    .any(|verb| verb.to_string() == interface_name)
                {
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
            if let Some(artifact) = &self.wasm {
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
        for verb in &manifest.exports {
            if !super::contracts::Verb::ALL
                .iter()
                .any(|candidate| candidate.interface() == verb)
                || !seen.insert(verb)
            {
                return Err(KernelError::InvalidRequest {
                    message: format!("invalid or duplicate resource export {verb}"),
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
            &package.root.join("../wit/resource-surface/world.wit"),
            &package.root.join("../../wit/resource-surface/world.wit"),
            &package.root.join("../../../wit/resource-surface/world.wit"),
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
        for verb in super::contracts::Verb::ALL {
            let actual = exports
                .iter()
                .any(|name| name.contains(&format!("artist:resource/{verb}")));
            let declared = package
                .manifest
                .exports
                .iter()
                .any(|export| export == verb.to_string().as_str());
            if actual != declared {
                return Err(KernelError::InvalidRequest {
                    message: format!(
                        "resource component export mismatch for {verb}: declared={declared}, actual={actual}"
                    ),
                });
            }
        }
        // The component type is authoritative: a package cannot hide a
        // standard nested operation import behind stale or missing WIT.
        let actual_imports = component
            .component_type()
            .imports(&engine)
            .map(|(name, _)| name.to_owned())
            .collect::<Vec<_>>();
        for import in &actual_imports {
            for verb in super::contracts::Verb::ALL {
                if import.contains(&format!("resource/{verb}"))
                    && !package
                        .manifest
                        .capabilities
                        .iter()
                        .any(|capability| capability == &format!("resource.{verb}"))
                {
                    return Err(KernelError::PermissionDenied {
                        uri: format!("resource.{verb}"),
                    });
                }
            }
        }
        if let Some(wit) = &package.wit {
            let declared_imports = ResourcePackage::resource_wit_imports(wit)?;
            for verb in super::contracts::Verb::ALL {
                let actual = actual_imports
                    .iter()
                    .any(|import| import.contains(&format!("resource/{verb}")));
                let declared = declared_imports.contains(&verb.to_string());
                if actual != declared {
                    return Err(KernelError::InvalidRequest {
                        message: format!(
                            "resource.wit/component import mismatch for {verb}: declared={declared}, actual={actual}"
                        ),
                    });
                }
            }
        }
        let _ = options;
        Ok(())
    }

    pub struct ResourcesHandler {
        root: PathBuf,
        files: FileHandler,
        active: super::generations::GenerationStore<ActiveResource>,
        dirty: Arc<std::sync::Mutex<std::collections::HashSet<PathBuf>>>,
        activation_locks:
            Arc<std::sync::Mutex<std::collections::HashMap<PathBuf, Arc<std::sync::Mutex<()>>>>>,
        disabled_file_package: Option<String>,
        publication_lock: Arc<std::sync::Mutex<()>>,
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
            Ok(Self {
                files: FileHandler::new(&root)?,
                root,
                active: super::generations::GenerationStore::new(),
                dirty: watcher.map_or_else(
                    || Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
                    super::watcher::SharedWatcher::dirty_set,
                ),
                activation_locks: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
                disabled_file_package: None,
                publication_lock: Arc::new(std::sync::Mutex::new(())),
            })
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
                        return Ok(active.generation);
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
                    if let Some(active) = self.active.read().unwrap().get(&package.manifest.name) {
                        return Ok(active.generation);
                    }
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
            verb: Verb,
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
                    })
                    .or_else(|| {
                        self.activate(&package.root).ok();
                        self.active
                            .read()
                            .unwrap()
                            .get(&package.manifest.name)
                            .cloned()
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

        fn operation_candidates(
            &self,
            operation: &Operation,
            scope: &artist_kernel::InvocationScope,
        ) -> Vec<Arc<ActiveResource>> {
            let (verb, uris): (Verb, Vec<&ResourceUri>) = match operation {
                Operation::Read(items) => {
                    (Verb::Read, items.iter().map(|item| &item.uri).collect())
                }
                Operation::Write(items) => {
                    (Verb::Write, items.iter().map(|item| &item.uri).collect())
                }
                Operation::Edit(items) => {
                    (Verb::Edit, items.iter().map(|item| &item.uri).collect())
                }
                Operation::Find(request) => (Verb::Find, request.roots.iter().collect()),
                Operation::Grep(request) => match &request.source {
                    artist_kernel::GrepSource::Resources(uris) => {
                        (Verb::Grep, uris.iter().collect())
                    }
                    artist_kernel::GrepSource::Text(text) => {
                        let _ = text;
                        (Verb::Grep, Vec::new())
                    }
                },
                Operation::Run(items) => (Verb::Run, items.iter().map(|item| &item.uri).collect()),
                Operation::Send(items) => {
                    (Verb::Send, items.iter().map(|item| &item.uri).collect())
                }
                Operation::Abort(uris) => (Verb::Abort, uris.iter().collect()),
                Operation::Delete(uris) => (Verb::Delete, uris.iter().collect()),
                Operation::Poll(request) => (
                    Verb::Poll,
                    request.targets.iter().map(|target| &target.uri).collect(),
                ),
            };
            let mut candidates = Vec::new();
            for uri in uris {
                for candidate in self.active_candidates(uri, verb, scope) {
                    if !candidates
                        .iter()
                        .any(|current: &Arc<ActiveResource>| Arc::ptr_eq(current, &candidate))
                    {
                        candidates.push(candidate);
                    }
                }
            }
            candidates
        }

        fn bootstrap_operation(operation: &Operation) -> bool {
            let all_resources = |uris: &[ResourceUri]| {
                !uris.is_empty() && uris.iter().all(|uri| uri.scheme() == "resources")
            };
            match operation {
                Operation::Read(items) => all_resources(
                    &items
                        .iter()
                        .map(|item| item.uri.clone())
                        .collect::<Vec<_>>(),
                ),
                Operation::Write(items) => all_resources(
                    &items
                        .iter()
                        .map(|item| item.uri.clone())
                        .collect::<Vec<_>>(),
                ),
                Operation::Edit(items) => all_resources(
                    &items
                        .iter()
                        .map(|item| item.uri.clone())
                        .collect::<Vec<_>>(),
                ),
                Operation::Find(request) => all_resources(&request.roots),
                Operation::Grep(request) => matches!(
                    &request.source,
                    artist_kernel::GrepSource::Resources(uris) if all_resources(uris)
                ),
                Operation::Abort(uris) | Operation::Delete(uris) => all_resources(uris),
                _ => false,
            }
        }

        fn mark_bootstrap_dirty(&self, operation: &Operation) {
            let mut packages = std::collections::HashSet::new();
            let mut collect = |uri: &ResourceUri| {
                if uri.scheme() == "resources" {
                    if let Some(package) = uri.as_ref().host_str() {
                        packages.insert(package.to_owned());
                    }
                }
            };
            match operation {
                Operation::Write(items) => items.iter().for_each(|item| collect(&item.uri)),
                Operation::Edit(items) => items.iter().for_each(|item| collect(&item.uri)),
                Operation::Delete(uris) => uris.iter().for_each(&mut collect),
                _ => return,
            }
            if !packages.is_empty() {
                let mut dirty = self.dirty.lock().unwrap();
                for package in packages {
                    dirty.insert(self.root.join(package));
                }
            }
        }

        fn map_bootstrap_operation(&self, operation: Operation) -> Result<Operation, KernelError> {
            let map = |uri: ResourceUri| self.map_uri(&ResourceAddress::uri(uri));
            Ok(match operation {
                Operation::Read(items) => Operation::Read(
                    items
                        .into_iter()
                        .map(|mut item| {
                            item.uri = map(item.uri)?;
                            Ok(item)
                        })
                        .collect::<Result<_, KernelError>>()?,
                ),
                Operation::Write(items) => Operation::Write(
                    items
                        .into_iter()
                        .map(|mut item| {
                            item.uri = map(item.uri)?;
                            Ok(item)
                        })
                        .collect::<Result<_, KernelError>>()?,
                ),
                Operation::Edit(items) => Operation::Edit(
                    items
                        .into_iter()
                        .map(|mut item| {
                            item.uri = map(item.uri)?;
                            Ok(item)
                        })
                        .collect::<Result<_, KernelError>>()?,
                ),
                Operation::Find(mut request) => {
                    request.roots = request
                        .roots
                        .into_iter()
                        .map(map)
                        .collect::<Result<_, KernelError>>()?;
                    Operation::Find(request)
                }
                Operation::Grep(mut request) => {
                    if let artist_kernel::GrepSource::Resources(uris) = request.source {
                        request.source = artist_kernel::GrepSource::Resources(
                            uris.into_iter()
                                .map(map)
                                .collect::<Result<_, KernelError>>()?,
                        );
                    }
                    Operation::Grep(request)
                }
                Operation::Abort(uris) => Operation::Abort(
                    uris.into_iter()
                        .map(map)
                        .collect::<Result<_, KernelError>>()?,
                ),
                Operation::Delete(uris) => Operation::Delete(
                    uris.into_iter()
                        .map(map)
                        .collect::<Result<_, KernelError>>()?,
                ),
                other => {
                    return Err(KernelError::UnsupportedVerb {
                        verb: operation_verb(&other).to_owned(),
                        uri: operation_uri(&other),
                    });
                }
            })
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

        fn unmap_bootstrap_result(
            &self,
            result: OperationResult,
        ) -> Result<OperationResult, KernelError> {
            let uri = |uri: ResourceUri| self.unmap_bootstrap_uri(uri);
            Ok(match result {
                OperationResult::Read(values) => OperationResult::Read(
                    values
                        .into_iter()
                        .map(|value| {
                            value.and_then(|value| match value {
                                artist_kernel::ReadResult::Text(mut text) => {
                                    text.uri = uri(text.uri)?;
                                    Ok(artist_kernel::ReadResult::Text(text))
                                }
                                artist_kernel::ReadResult::Directory { uri: root, entries } => {
                                    Ok(artist_kernel::ReadResult::Directory {
                                        uri: uri(root)?,
                                        entries: entries
                                            .into_iter()
                                            .map(uri)
                                            .collect::<Result<_, _>>()?,
                                    })
                                }
                            })
                        })
                        .collect(),
                ),
                OperationResult::Write(values) => OperationResult::Write(
                    values
                        .into_iter()
                        .map(|value| {
                            value.and_then(|mut result| {
                                result.text.uri = uri(result.text.uri)?;
                                Ok(result)
                            })
                        })
                        .collect(),
                ),
                OperationResult::Edit(values) => OperationResult::Edit(
                    values
                        .into_iter()
                        .map(|value| {
                            value.and_then(|mut result| {
                                result.text.uri = uri(result.text.uri)?;
                                result.diff.uri = uri(result.diff.uri)?;
                                Ok(result)
                            })
                        })
                        .collect(),
                ),
                OperationResult::Find(value) => OperationResult::Find(
                    value.map(|uris| uris.into_iter().map(uri).collect::<Result<_, _>>())?,
                ),
                OperationResult::Grep(value) => OperationResult::Grep(value.map(|texts| {
                    texts
                        .into_iter()
                        .map(|mut text| {
                            text.uri = uri(text.uri)?;
                            Ok(text)
                        })
                        .collect::<Result<_, KernelError>>()
                })?),
                OperationResult::Abort(values) => OperationResult::Abort(
                    values
                        .into_iter()
                        .map(|value| value.and_then(uri))
                        .collect(),
                ),
                OperationResult::Delete(values) => OperationResult::Delete(
                    values
                        .into_iter()
                        .map(|value| value.and_then(uri))
                        .collect(),
                ),
                other => other,
            })
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

        async fn select_candidate(
            &self,
            verb: Verb,
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
                        .any(|export| export == verb.to_string().as_str())
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

    impl ResourceCatalogProvider for ResourcesHandler {
        fn resource_catalog(&self) -> Vec<ResourceCatalogEntry> {
            self.catalog()
        }
    }

    impl Handler for ResourcesHandler {
        fn descriptor(&self) -> HandlerDescriptor {
            HandlerDescriptor {
                name: "resources-bootstrap".to_owned(),
                schemes: vec!["resources".to_owned()],
                verbs: Verb::ALL.to_vec(),
            }
        }

        fn execute<'a>(
            &'a self,
            mut request: Request,
            host: KernelHandle,
        ) -> BoxFuture<'a, Result<serde_json::Value, KernelError>> {
            let mapped = self.map_uri(&request.target);
            Box::pin(async move {
                request.target = ResourceAddress::uri(mapped?);
                // The bootstrap mount exposes package files through the same
                // ordinary file semantics as tools:// package inspection.
                self.files.execute(request, host).await
            })
        }
    }

    impl TypedHandler for ResourcesHandler {
        fn descriptor(&self) -> HandlerDescriptor {
            HandlerDescriptor {
                name: "resource-extensions".to_owned(),
                schemes: Vec::new(),
                verbs: Verb::ALL.to_vec(),
            }
        }

        fn claims_operation(&self, operation: &Operation) -> bool {
            Self::bootstrap_operation(operation)
                || !self
                    .operation_candidates(
                        operation,
                        &artist_kernel::InvocationScope::new(
                            artist_kernel::InvocationContext::default(),
                        ),
                    )
                    .is_empty()
        }

        fn claim_operation<'a>(
            &'a self,
            operation: &'a Operation,
        ) -> BoxFuture<'a, Result<artist_kernel::ClaimDecision, KernelError>> {
            self.claim_operation_with_scope(
                operation,
                artist_kernel::InvocationScope::new(artist_kernel::InvocationContext::default()),
            )
        }

        fn claim_operation_with_scope<'a>(
            &'a self,
            operation: &'a Operation,
            scope: artist_kernel::InvocationScope,
        ) -> BoxFuture<'a, Result<artist_kernel::ClaimDecision, KernelError>> {
            Box::pin(async move {
                if Self::bootstrap_operation(operation) {
                    return Ok(artist_kernel::ClaimDecision::Handle);
                }
                let candidates = self.operation_candidates(operation, &scope);
                if candidates.is_empty() {
                    return Ok(artist_kernel::ClaimDecision::Pass);
                }
                let uri = operation_uri(operation);
                let verb = operation_verb(operation);
                let uri = match ResourceUri::parse(&uri) {
                    Ok(uri) => uri,
                    Err(error) => {
                        return Err(KernelError::Handler {
                            message: format!("resource claim failed: {error}"),
                        });
                    }
                };
                let verb = Verb::ALL
                    .iter()
                    .copied()
                    .find(|candidate| candidate.to_string() == verb)
                    .unwrap_or(Verb::Read);
                let claim_key =
                    format!("resource-extensions:{}:{}", verb, operation_uri(&operation));
                let (candidate, decision) = if let Some(decision) = scope.pinned_claim(&claim_key) {
                    (candidates.into_iter().next(), decision)
                } else {
                    self.select_candidate(verb, &uri, candidates).await?
                };
                if let Some(candidate) = candidate {
                    let pin_key = generation_pin_key(verb, &uri);
                    scope.pin_generation(
                        pin_key.clone(),
                        candidate.package.manifest.name.clone(),
                        candidate.generation,
                    );
                    scope.pin_generation_handle(pin_key, Arc::clone(&candidate));
                    scope.pin_generation_handle(
                        resource_package_pin_key(&candidate.package.manifest.name),
                        Arc::clone(&candidate),
                    );
                }
                Ok(decision)
            })
        }

        fn execute_typed<'a>(
            &'a self,
            operation: Operation,
            host: KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
            self.execute_typed_with_scope(
                operation,
                host,
                artist_kernel::InvocationScope::new(context),
            )
        }

        fn execute_typed_with_scope<'a>(
            &'a self,
            operation: Operation,
            host: KernelHandle,
            scope: artist_kernel::InvocationScope,
        ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
            Box::pin(async move {
                if Self::bootstrap_operation(&operation) {
                    self.mark_bootstrap_dirty(&operation);
                    let mapped = self.map_bootstrap_operation(operation)?;
                    let result = self
                        .files
                        .execute_typed(mapped, host, scope.context.clone())
                        .await?;
                    return self.unmap_bootstrap_result(result);
                }
                let mut candidates = self.operation_candidates(&operation, &scope);
                if candidates.is_empty() {
                    return Err(KernelError::NoHandler {
                        uri: operation_uri(&operation),
                    });
                }
                let uri = ResourceUri::parse(&operation_uri(&operation)).map_err(|error| {
                    KernelError::InvalidUri {
                        message: error.to_string(),
                    }
                })?;
                let verb = Verb::ALL
                    .iter()
                    .copied()
                    .find(|candidate| candidate.to_string() == operation_verb(&operation))
                    .unwrap_or(Verb::Read);
                if let Some((package, generation)) =
                    scope.pinned_generation(&generation_pin_key(verb, &uri))
                {
                    candidates.retain(|candidate| {
                        candidate.package.manifest.name == package
                            && candidate.generation == generation
                    });
                }
                // Claiming and invoking are one routing transaction. The
                // kernel records the decision against this operation before
                // entering the invoke phase; reuse that decision and the
                // generation lease instead of asking a hot-reloadable
                // component to claim the operation a second time.
                let claim_key =
                    format!("resource-extensions:{}:{}", verb, operation_uri(&operation));
                let (candidate, decision) = if let Some(decision) = scope.pinned_claim(&claim_key) {
                    (candidates.into_iter().next(), decision)
                } else {
                    self.select_candidate(verb, &uri, candidates).await?
                };
                if decision == artist_kernel::ClaimDecision::Reserve {
                    return Err(KernelError::UnsupportedVerb {
                        verb: verb.to_string(),
                        uri: uri.to_string(),
                    });
                }
                let Some(candidate) = candidate else {
                    return Err(KernelError::NoHandler {
                        uri: operation_uri(&operation),
                    });
                };
                let frame_key = format!(
                    "{}@{}:{}:{}",
                    candidate.package.manifest.name,
                    candidate.generation,
                    verb,
                    uri.without_fragment()
                );
                let _routing_frame =
                    scope
                        .enter_routing_frame(frame_key)
                        .map_err(|cycle| KernelError::Handler {
                            message: format!("resource routing cycle: {cycle}"),
                        })?;
                let verb = operation_verb(&operation);
                if let Operation::Write(requests) = &operation {
                    let output = candidate
                        .host
                        .invoke_write_typed(requests.clone(), host.clone(), scope.clone())
                        .await?;
                    return Ok(OperationResult::Write(output));
                }
                if let Operation::Read(requests) = &operation {
                    let output = candidate
                        .host
                        .invoke_read_typed(requests.clone(), host.clone(), scope.clone())
                        .await?;
                    return Ok(OperationResult::Read(output));
                }
                if let Operation::Edit(requests) = &operation {
                    let output = candidate
                        .host
                        .invoke_edit_typed(requests.clone(), host.clone(), scope.clone())
                        .await?;
                    return Ok(OperationResult::Edit(output));
                }
                if let Operation::Run(requests) = &operation {
                    let output = candidate
                        .host
                        .invoke_run_typed(requests.clone(), host.clone(), scope.clone())
                        .await?;
                    return Ok(OperationResult::Run(output));
                }
                if let Operation::Send(requests) = &operation {
                    let output = candidate
                        .host
                        .invoke_send_typed(requests.clone(), host.clone(), scope.clone())
                        .await?;
                    return Ok(OperationResult::Send(output));
                }
                if let Operation::Find(request) = &operation {
                    let output = candidate
                        .host
                        .invoke_find_typed(request.clone(), host.clone(), scope.clone())
                        .await?;
                    return Ok(OperationResult::Find(Ok(output)));
                }
                if let Operation::Grep(request) = &operation {
                    let output = candidate
                        .host
                        .invoke_grep_typed(request.clone(), host.clone(), scope.clone())
                        .await?;
                    return Ok(OperationResult::Grep(Ok(output)));
                }
                if let Operation::Poll(request) = &operation {
                    let output = candidate
                        .host
                        .invoke_poll_typed(request.clone(), host.clone(), scope.clone())
                        .await?;
                    return Ok(OperationResult::Poll(Ok(output)));
                }
                if let Operation::Abort(uris) = &operation {
                    let output = candidate
                        .host
                        .invoke_abort_typed(
                            uris.iter().map(ToString::to_string).collect(),
                            host.clone(),
                            scope.clone(),
                        )
                        .await?;
                    return Ok(OperationResult::Abort(
                        super::resource_abort_results_to_kernel(output)?,
                    ));
                }
                if let Operation::Delete(uris) = &operation {
                    let output = candidate
                        .host
                        .invoke_delete_typed(
                            uris.iter().map(ToString::to_string).collect(),
                            host.clone(),
                            scope.clone(),
                        )
                        .await?;
                    return Ok(OperationResult::Delete(
                        super::resource_delete_results_to_kernel(output)?,
                    ));
                }
                Err(KernelError::Handler {
                    message: format!(
                        "resource verb {verb} was not dispatched through its typed WIT interface"
                    ),
                })
            })
        }
    }

    fn operation_uri(operation: &Operation) -> String {
        match operation {
            Operation::Read(items) => items
                .first()
                .map(|item| item.uri.to_string())
                .unwrap_or_default(),
            Operation::Write(items) => items
                .first()
                .map(|item| item.uri.to_string())
                .unwrap_or_default(),
            Operation::Edit(items) => items
                .first()
                .map(|item| item.uri.to_string())
                .unwrap_or_default(),
            Operation::Find(request) => request
                .roots
                .first()
                .map(ToString::to_string)
                .unwrap_or_default(),
            Operation::Grep(request) => match &request.source {
                artist_kernel::GrepSource::Resources(uris) => {
                    uris.first().map(ToString::to_string).unwrap_or_default()
                }
                artist_kernel::GrepSource::Text(text) => text
                    .first()
                    .map(|text| text.uri.to_string())
                    .unwrap_or_default(),
            },
            Operation::Run(items) => items
                .first()
                .map(|item| item.uri.to_string())
                .unwrap_or_default(),
            Operation::Send(items) => items
                .first()
                .map(|item| item.uri.to_string())
                .unwrap_or_default(),
            Operation::Abort(uris) | Operation::Delete(uris) => {
                uris.first().map(ToString::to_string).unwrap_or_default()
            }
            Operation::Poll(request) => request
                .targets
                .first()
                .map(|target| target.uri.to_string())
                .unwrap_or_default(),
        }
    }

    fn operation_verb(operation: &Operation) -> &'static str {
        match operation {
            Operation::Read(_) => "read",
            Operation::Write(_) => "write",
            Operation::Edit(_) => "edit",
            Operation::Find(_) => "find",
            Operation::Grep(_) => "grep",
            Operation::Run(_) => "run",
            Operation::Send(_) => "send",
            Operation::Abort(_) => "abort",
            Operation::Delete(_) => "delete",
            Operation::Poll(_) => "poll",
        }
    }

    fn generation_pin_key(verb: Verb, uri: &ResourceUri) -> String {
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
        let bytes = std::fs::read(build.artifact).unwrap();
        let host = TypedComponentHost::new_with_capabilities(&bytes, ["resource.read".to_owned()])
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("typed.txt"), "typed bridge\n").unwrap();
        let typed_uri = root.path().join("typed.txt").display().to_string();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let kernel = artist_kernel::Kernel::new();
        runtime.block_on(kernel.register(artist_kernel::FileHandler::new(root.path()).unwrap()));
        runtime
            .block_on(kernel.register_typed(artist_kernel::FileHandler::new(root.path()).unwrap()));
        let result = host
            .invoke_read(
                vec![tool_v1_bindings::artist::resource::types::ReadRequest {
                    uri: typed_uri.clone(),
                    at: None,
                    before: None,
                    after: None,
                }],
                kernel.handle(),
            )
            .unwrap();
        assert!(result[0].is_ok(), "typed read failed: {result:?}");

        let registry = runtime::ComponentRegistry::new();
        let active = registry.reload(&package, &options).unwrap();
        assert_eq!(active.info().interfaces, vec!["artist:tool:read@1"]);
        let routed = active
            .invoke_tool_json(
                contracts::Verb::Read,
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

    struct MockConformanceHandler;

    impl artist_kernel::Handler for MockConformanceHandler {
        fn descriptor(&self) -> artist_kernel::HandlerDescriptor {
            artist_kernel::HandlerDescriptor {
                name: "typed-conformance-mock".to_owned(),
                schemes: Vec::new(),
                verbs: artist_kernel::Verb::ALL.to_vec(),
            }
        }

        fn claims(&self, _address: &artist_kernel::ResourceAddress) -> bool {
            true
        }

        fn execute<'a>(
            &'a self,
            request: artist_kernel::Request,
            _host: artist_kernel::KernelHandle,
        ) -> artist_kernel::BoxFuture<'a, Result<serde_json::Value, artist_kernel::KernelError>>
        {
            Box::pin(async move {
                Ok(match request.verb {
                    artist_kernel::Verb::Read => serde_json::json!({
                        "path": request.target.to_string(),
                        "value": {"content": "mock text\n"}
                    }),
                    artist_kernel::Verb::Find | artist_kernel::Verb::Grep => {
                        serde_json::json!({"paths": [request.target.to_string()]})
                    }
                    artist_kernel::Verb::Write => serde_json::json!({"written": true}),
                    artist_kernel::Verb::Edit => serde_json::json!({"edited": true}),
                    artist_kernel::Verb::Delete => serde_json::json!({"deleted": true}),
                    _ => serde_json::json!({"ok": true}),
                })
            })
        }
    }

    impl artist_kernel::TypedHandler for MockConformanceHandler {
        fn descriptor(&self) -> artist_kernel::HandlerDescriptor {
            artist_kernel::HandlerDescriptor {
                name: "typed-conformance-mock".to_owned(),
                schemes: Vec::new(),
                verbs: artist_kernel::Verb::ALL.to_vec(),
            }
        }

        fn claims_operation(&self, _operation: &artist_kernel::Operation) -> bool {
            true
        }

        fn execute_typed<'a>(
            &'a self,
            operation: artist_kernel::Operation,
            _host: artist_kernel::KernelHandle,
            _context: artist_kernel::InvocationContext,
        ) -> artist_kernel::BoxFuture<
            'a,
            Result<artist_kernel::OperationResult, artist_kernel::KernelError>,
        > {
            Box::pin(async move {
                use artist_kernel::*;
                let line = |uri: ResourceUri| AnchoredText {
                    uri,
                    lines: vec![AnchoredLine {
                        anchor: Anchor::from_tokens(vec!["1".to_owned()]),
                        text: "mock text".to_owned(),
                        ending: LineEnding::Lf,
                    }],
                };
                Ok(match operation {
                    Operation::Read(requests) => OperationResult::Read(
                        requests
                            .into_iter()
                            .map(|request| Ok(ReadResult::Text(line(request.uri))))
                            .collect(),
                    ),
                    Operation::Write(requests) => OperationResult::Write(
                        requests
                            .into_iter()
                            .map(|request| {
                                Ok(WriteResult {
                                    text: line(request.uri),
                                })
                            })
                            .collect(),
                    ),
                    Operation::Edit(requests) => OperationResult::Edit(
                        requests
                            .into_iter()
                            .map(|request| {
                                Ok(EditResult {
                                    text: line(request.uri.clone()),
                                    diff: AnchoredDiff {
                                        uri: request.uri,
                                        hunks: Vec::new(),
                                    },
                                })
                            })
                            .collect(),
                    ),
                    Operation::Find(request) => OperationResult::Find(Ok(request.roots)),
                    Operation::Grep(request) => OperationResult::Grep(Ok(match request.source {
                        GrepSource::Resources(uris) => uris.into_iter().map(line).collect(),
                        GrepSource::Text(text) => text,
                    })),
                    Operation::Run(requests) => OperationResult::Run(
                        requests
                            .into_iter()
                            .map(|request| Ok(request.uri))
                            .collect(),
                    ),
                    Operation::Send(requests) => OperationResult::Send(
                        requests
                            .into_iter()
                            .map(|request| Ok(request.uri))
                            .collect(),
                    ),
                    Operation::Abort(uris) => {
                        OperationResult::Abort(uris.into_iter().map(Ok).collect())
                    }
                    Operation::Delete(uris) => {
                        OperationResult::Delete(uris.into_iter().map(Ok).collect())
                    }
                    Operation::Poll(request) => OperationResult::Poll(Ok(PollResult {
                        text: request
                            .targets
                            .into_iter()
                            .map(|target| line(target.uri))
                            .collect(),
                        satisfied: Vec::new(),
                    })),
                })
            })
        }
    }

    #[test]
    fn exercises_all_typed_verb_components_without_memory_backend() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let kernel = artist_kernel::Kernel::new();
        runtime.block_on(kernel.register(MockConformanceHandler));
        runtime.block_on(kernel.register_typed(MockConformanceHandler));

        let cases = [
            (
                contracts::Verb::Read,
                "[{\"uri\":\"typed.txt\",\"at\":null,\"before\":null,\"after\":null}]",
            ),
            (
                contracts::Verb::Write,
                "[{\"uri\":\"typed-write.txt\",\"content\":\"hello\\n\"}]",
            ),
            (
                contracts::Verb::Edit,
                "[{\"uri\":\"typed.txt\",\"operations\":[]}]",
            ),
            (
                contracts::Verb::Find,
                "{\"roots\":[\"typed.txt\"],\"query\":\"typed\"}",
            ),
            (
                contracts::Verb::Grep,
                "{\"pattern\":\"typed\",\"source\":{\"Resources\":[\"typed.txt\"]}}",
            ),
            (
                contracts::Verb::Run,
                "[{\"uri\":\"typed.txt\",\"args\":[]}]",
            ),
            (
                contracts::Verb::Send,
                "[{\"uri\":\"typed.txt\",\"content\":\"hello\"}]",
            ),
            (contracts::Verb::Abort, "[\"missing-abort\"]"),
            (contracts::Verb::Delete, "[\"missing-delete\"]"),
            (
                contracts::Verb::Poll,
                "{\"targets\":[{\"uri\":\"typed.txt\",\"from_position\":null}],\"until\":null}",
            ),
        ];

        for (verb, input) in cases {
            let package_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("conformance/verbs")
                .join(verb.interface());
            let package = package::ToolPackage::discover(&package_root).unwrap();
            let mut options = package::BuildOptions::default();
            options.force = true;
            options.granted_capabilities = package.manifest.capabilities.clone();
            let build = package.build(&options).unwrap();
            let bytes = std::fs::read(build.artifact).unwrap();
            let host = TypedComponentHost::new_with_capabilities(
                &bytes,
                options.granted_capabilities.clone(),
            )
            .unwrap();
            let output = host.invoke_json(verb, input, kernel.handle()).unwrap();
            serde_json::from_str::<serde_json::Value>(&output).unwrap();
        }

        let package_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs/read");
        let package = package::ToolPackage::discover(&package_root).unwrap();
        let mut options = package::BuildOptions::default();
        options.force = true;
        options.granted_capabilities = package.manifest.capabilities.clone();
        let build = package.build(&options).unwrap();
        let host = TypedComponentHost::new_with_capabilities(
            &std::fs::read(build.artifact).unwrap(),
            std::iter::empty::<String>(),
        )
        .unwrap();
        assert!(host.validate_contract(contracts::Verb::Read).is_ok());
        assert!(host.validate_contract(contracts::Verb::Write).is_err());
        let denied = host
            .invoke_json(
                contracts::Verb::Read,
                "[{\"uri\":\"mock.txt\",\"at\":null,\"before\":null,\"after\":null}]",
                kernel.handle(),
            )
            .unwrap();
        assert!(denied.contains("PermissionDenied"));
    }

    #[test]
    fn rejects_extra_typed_exports_and_imports() {
        let read = contracts::Verb::Read;
        let exports = vec![
            "artist:tool/read@0.1.0".to_owned(),
            "extra/export".to_owned(),
        ];
        let imports = vec![
            "artist:tool/types@0.1.0".to_owned(),
            "artist:tool/old-helper@0.1.0".to_owned(),
            "artist:tool/extra@0.1.0".to_owned(),
        ];
        assert!(validate_typed_contract_names(read, &exports, &imports).is_err());
        assert!(
            validate_typed_contract_names(
                read,
                &["artist:tool/read@0.1.0".to_owned()],
                &imports[..2]
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_typed_manifest_capability_mismatch() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/verbs/read");
        let package = package::ToolPackage::discover(&root).unwrap();
        let mut options = package::BuildOptions::default();
        options.force = true;
        options.granted_capabilities = package.manifest.capabilities.clone();
        let build = package.build(&options).unwrap();

        let mut mismatched = package.clone();
        mismatched.manifest.capabilities = vec!["resource.write".to_owned()];
        let error = package::validate_artifact(&build.artifact, &mismatched, &options).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("must declare exactly capability resource.read")
        );
    }

    #[test]
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
            "package example:aliased@1.0.0;\nworld aliased { import source-read: artist:%resource/read@1.0.0; export artist:%resource/extension@1.0.0; export artist:%resource/read@1.0.0; }\n",
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
            Some(contracts::ContractId::universal(contracts::Verb::Read))
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
        kernel
            .register_typed(artist_kernel::FileHandler::new(project.path()).unwrap())
            .await;
        kernel
            .register_typed_handler(resources::ResourcesHandler::new(&resource_root).unwrap())
            .await;

        let source_uri = artist_kernel::ResourceUri::parse(&source.display().to_string()).unwrap();
        let projection =
            artist_kernel::ResourceUri::parse(&format!("{source_uri}/symbols/")).unwrap();
        let result = kernel
            .execute_operation(artist_kernel::Operation::Read(vec![
                artist_kernel::ReadRequest {
                    uri: projection,
                    at: None,
                    before: None,
                    after: None,
                },
            ]))
            .await
            .unwrap();
        let artist_kernel::OperationResult::Read(values) = result else {
            panic!("AST resource returned the wrong result variant");
        };
        let Ok(artist_kernel::ReadResult::Text(text)) = &values[0] else {
            panic!("AST resource did not return anchored text: {values:?}");
        };
        assert_eq!(text.lines.len(), 2);
        assert!(text.lines.iter().any(|line| line.text.contains("fn main")));

        // The bootstrap mount is also a typed file namespace: package
        // metadata remains inspectable through the same typed resource path.
        let package_file =
            artist_kernel::ResourceUri::parse("resources://ast/resource.md").unwrap();
        let package_result = kernel
            .execute_operation(artist_kernel::Operation::Read(vec![
                artist_kernel::ReadRequest {
                    uri: package_file.clone(),
                    at: None,
                    before: None,
                    after: None,
                },
            ]))
            .await
            .unwrap();
        let artist_kernel::OperationResult::Read(package_values) = package_result else {
            panic!("resource package read returned the wrong result variant");
        };
        let Ok(artist_kernel::ReadResult::Text(package_text)) = &package_values[0] else {
            panic!("resource package read did not return text: {package_values:?}");
        };
        assert_eq!(package_text.uri, package_file);
        assert!(
            package_text
                .lines
                .iter()
                .any(|line| line.text.contains("artist-ast"))
        );
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
        kernel
            .register_typed(artist_kernel::FileHandler::new(project.path()).unwrap())
            .await;
        kernel
            .register_typed_handler(resources::ResourcesHandler::new(&resource_root).unwrap())
            .await;

        let file = artist_kernel::ResourceUri::parse(&source.display().to_string()).unwrap();
        let projection =
            artist_kernel::ResourceUri::parse(&format!("{file}/symbols/main/callers?limit=1"))
                .unwrap();
        let result = kernel
            .execute_operation(artist_kernel::Operation::Read(vec![
                artist_kernel::ReadRequest {
                    uri: projection,
                    at: None,
                    before: None,
                    after: None,
                },
            ]))
            .await
            .unwrap();
        let artist_kernel::OperationResult::Read(values) = result else {
            panic!("AST child returned the wrong result variant");
        };
        let Ok(artist_kernel::ReadResult::Text(text)) = &values[0] else {
            panic!("AST child did not return anchored text: {values:?}");
        };
        assert_eq!(text.uri.query(), Some("limit=1"));
        assert!(text.lines.iter().any(|line| line.text.contains("main()")));
    }

    #[tokio::test]
    async fn ast_resource_reserves_projection_mutations() {
        let resource_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/resources");
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("main.rs");
        std::fs::write(&source, "fn main() {}\n").unwrap();
        let kernel = artist_kernel::Kernel::new();
        kernel
            .register_typed(artist_kernel::FileHandler::new(project.path()).unwrap())
            .await;
        kernel
            .register_typed_handler(resources::ResourcesHandler::new(&resource_root).unwrap())
            .await;
        let projection = artist_kernel::ResourceUri::parse(&format!(
            "{}/symbols/",
            artist_kernel::ResourceUri::parse(&source.display().to_string()).unwrap()
        ))
        .unwrap();
        let result = kernel
            .execute_operation(artist_kernel::Operation::Edit(vec![
                artist_kernel::EditRequest {
                    uri: projection,
                    operations: Vec::new(),
                },
            ]))
            .await;
        assert!(
            matches!(
                result,
            Ok(artist_kernel::OperationResult::Edit(ref values))
                    if matches!(values.as_slice(), [Err(artist_kernel::KernelError::UnsupportedVerb { .. })])
            ),
            "unexpected projection mutation result: {result:?}"
        );
    }

    #[test]
    fn exposes_ten_stable_universal_contract_ids() {
        use std::str::FromStr;

        let contracts = contracts::universal_contracts();
        assert_eq!(contracts.len(), 10);
        let read = contracts::ContractId::from_str("artist:tool:read@1").unwrap();
        assert_eq!(
            read,
            contracts::ContractId::universal(contracts::Verb::Read)
        );
        assert_eq!(read.to_string(), "artist:tool:read@1");

        let mut registry = contract_registry::ContractRegistry::default();
        registry
            .register(contracts::ContractDescriptor::universal(
                contracts::Verb::Read,
            ))
            .unwrap();
        assert!(registry.resolve(&read).is_some());
        assert!(
            registry
                .register(contracts::ContractDescriptor::universal(
                    contracts::Verb::Read
                ))
                .is_err()
        );
    }

    #[test]
    fn accepts_and_registers_extension_contracts_beyond_universal_verbs() {
        use std::str::FromStr;

        let contract = contracts::ContractId::from_str("acme:format@2").unwrap();
        assert_eq!(contract.interface, "format");
        assert_eq!(contract.verb, None);
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
    fn validates_typed_poll_conditions_and_run_context() {
        let condition = contracts::PollCondition::Any(vec![
            contracts::PollCondition::Atom(contracts::PollAtom::Regex {
                target: 0,
                pattern: "FAILED".to_owned(),
            }),
            contracts::PollCondition::Atom(contracts::PollAtom::Terminated { target: 1 }),
        ]);
        assert!(contracts::validate_poll_condition(&condition, 2, &[false, true]).is_ok());
        assert!(contracts::validate_poll_condition(&condition, 2, &[false, false]).is_err());
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
