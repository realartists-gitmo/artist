//! Host-side implementation of the Artist WebAssembly Component ABI.

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "artist-world",
    });
}

/// Generated bindings for the typed universal tool contracts. Individual
/// verb components will eventually select the exported interface they
/// implement; the aggregate world exists here to validate the shared package
/// as one coherent WIT surface.
pub mod tool_bindings {
    wasmtime::component::bindgen!({
        path: "wit/tool-surface",
        world: "tool-world",
        additional_derives: [serde::Serialize, serde::Deserialize],
    });
}

macro_rules! typed_world_bindings {
    ($module:ident, $world:literal) => {
        pub mod $module {
            wasmtime::component::bindgen!({
                path: "wit/tool-surface",
                world: $world,
                additional_derives: [serde::Serialize, serde::Deserialize],
            });
        }
    };
}

typed_world_bindings!(read_bindings, "read-world");
typed_world_bindings!(write_bindings, "write-world");
typed_world_bindings!(edit_bindings, "edit-world");
typed_world_bindings!(find_bindings, "find-world");
typed_world_bindings!(grep_bindings, "grep-world");
typed_world_bindings!(run_bindings, "run-world");
typed_world_bindings!(send_bindings, "send-world");
typed_world_bindings!(abort_bindings, "abort-world");
typed_world_bindings!(delete_bindings, "delete-world");
typed_world_bindings!(poll_bindings, "poll-world");

pub use bindings::artist::component::types::{
    ComponentInfo, Context, Error, ErrorKind, HandleOwnership, HostRequest, HostResponse,
    Invocation, ResourceHandle, Response, StreamChunk,
};

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

use artist_kernel::{KernelHandle, Request as KernelRequest, Verb as KernelVerb};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Errors raised while loading or invoking a component.
#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("component load failed: {0}")]
    Load(#[source] anyhow::Error),
    #[error("component build failed: {diagnostics}")]
    Build { diagnostics: String },
    #[error("component invocation failed: {0}")]
    Invoke(#[source] anyhow::Error),
    #[error("component returned an ABI error: {0:?}")]
    Abi(Error),
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

pub const ABI_VERSION: &str = "0.1";

pub fn validate_component_info(info: &ComponentInfo) -> Result<(), ComponentError> {
    if info.abi_version != ABI_VERSION {
        return Err(ComponentError::Abi(Error {
            kind: ErrorKind::Incompatible,
            message: format!(
                "component ABI {} is incompatible with {}",
                info.abi_version, ABI_VERSION
            ),
            details: None,
        }));
    }
    Ok(())
}

pub fn validate_capabilities(
    info: &ComponentInfo,
    granted: &HashSet<String>,
) -> Result<(), ComponentError> {
    if let Some(missing) = info
        .required_capabilities
        .iter()
        .find(|capability| !granted.contains(*capability))
    {
        return Err(ComponentError::CapabilityDenied(missing.clone()));
    }
    Ok(())
}

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Minimal host state for the first ABI slice.
pub struct HostState {
    wasi: wasmtime_wasi::WasiCtx,
    table: wasmtime::component::ResourceTable,
    capabilities: HashSet<String>,
    handles: Arc<Mutex<HashMap<String, String>>>,
    kernel: Option<KernelHandle>,
    context: artist_kernel::InvocationContext,
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
            handles: Arc::new(Mutex::new(HashMap::new())),
            kernel: None,
            context: artist_kernel::InvocationContext::default(),
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
        let mut state = Self::with_kernel(capabilities, kernel);
        state.context = context;
        state
    }
}

impl wasmtime_wasi::WasiView for HostState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl bindings::artist::component::host::Host for HostState {
    fn invoke(&mut self, request: HostRequest) -> Result<HostResponse, Error> {
        // This is the quarantined legacy component ABI. Universal verbs are
        // required to use the typed `artist:tool` worlds below; this bridge is
        // retained only for pre-existing extension components while arbitrary
        // package-local WIT contracts are being established.
        if let Some(capability) = request.capability.as_deref()
            && !self.capabilities.contains(capability)
        {
            return Err(Error {
                kind: ErrorKind::HostFailure,
                message: format!("capability denied: {capability}"),
                details: None,
            });
        }
        if let Some(handle) = request.handle {
            let handles = self.handles.lock().unwrap();
            if !handles.contains_key(&handle.id) {
                return Err(Error {
                    kind: ErrorKind::HostFailure,
                    message: "invalid resource handle".to_owned(),
                    details: None,
                });
            }
        }
        if let (Some(kernel), Some(verb_name)) =
            (self.kernel.clone(), request.operation.strip_prefix("tool."))
        {
            let verb = match verb_name {
                "read" => KernelVerb::Read,
                "write" => KernelVerb::Write,
                "edit" => KernelVerb::Edit,
                "poll" => KernelVerb::Poll,
                "send" => KernelVerb::Send,
                "run" => KernelVerb::Run,
                "abort" => KernelVerb::Abort,
                "delete" => KernelVerb::Delete,
                "find" => KernelVerb::Find,
                "grep" => KernelVerb::Grep,
                _ => {
                    return Err(Error {
                        kind: ErrorKind::InvalidRequest,
                        message: format!("unknown tool verb: {verb_name}"),
                        details: None,
                    });
                }
            };
            let target = request.target.clone();
            let input = request.input.clone();
            let result = std::thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
                let target = if target.contains("://") {
                    artist_kernel::ResourceUri::parse(&target)
                        .map(artist_kernel::ResourceAddress::uri)
                        .map_err(|error| error.to_string())?
                } else {
                    artist_kernel::ResourceAddress::path(target)
                };
                let args = serde_json::from_str(&input).map_err(|error| error.to_string())?;
                Ok::<_, String>(
                    runtime.block_on(kernel.execute(KernelRequest::new(verb, target, args))),
                )
            })
            .join()
            .map_err(|_| Error {
                kind: ErrorKind::HostFailure,
                message: "tool host bridge panicked".to_owned(),
                details: None,
            })?
            .map_err(|message| Error {
                kind: ErrorKind::HostFailure,
                message,
                details: None,
            })?;
            if result.ok {
                return Ok(HostResponse {
                    output: serde_json::to_string(
                        result.value.as_ref().unwrap_or(&serde_json::Value::Null),
                    )
                    .unwrap(),
                    metadata: "{\"bridge\":\"kernel\"}".to_owned(),
                });
            }
            return Err(Error {
                kind: ErrorKind::HostFailure,
                message: result.error.map_or_else(
                    || "tool operation failed".to_owned(),
                    |error| error.to_string(),
                ),
                details: None,
            });
        }
        Ok(HostResponse {
            output: request.input,
            metadata: format!("{{\"operation\":\"{}\"}}", request.operation),
        })
    }

    fn acquire(&mut self, target: String) -> Result<ResourceHandle, Error> {
        let id = format!("resource-{}", self.handles.lock().unwrap().len() + 1);
        self.handles.lock().unwrap().insert(id.clone(), target);
        Ok(ResourceHandle {
            id,
            ownership: HandleOwnership::Owned,
        })
    }

    fn release(&mut self, handle: ResourceHandle) -> Result<(), Error> {
        if handle.ownership != HandleOwnership::Owned {
            return Err(Error {
                kind: ErrorKind::InvalidRequest,
                message: "only owned handles may be released".to_owned(),
                details: None,
            });
        }
        self.handles
            .lock()
            .unwrap()
            .remove(&handle.id)
            .map(|_| ())
            .ok_or_else(|| Error {
                kind: ErrorKind::InvalidRequest,
                message: "resource handle already released".to_owned(),
                details: None,
            })
    }

    fn borrow_handle(&mut self, handle: ResourceHandle) -> Result<ResourceHandle, Error> {
        if !self.handles.lock().unwrap().contains_key(&handle.id) {
            return Err(Error {
                kind: ErrorKind::InvalidRequest,
                message: "invalid resource handle".to_owned(),
                details: None,
            });
        }
        Ok(ResourceHandle {
            id: handle.id,
            ownership: HandleOwnership::Borrowed,
        })
    }
}

impl bindings::artist::component::types::Host for HostState {}

impl read_bindings::artist::tool::host_read::Host for HostState {
    fn read(
        &mut self,
        requests: Vec<read_bindings::artist::tool::types::ReadRequest>,
    ) -> Vec<
        Result<
            read_bindings::artist::tool::types::ReadResult,
            read_bindings::artist::tool::types::Error,
        >,
    > {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<read_bindings::artist::tool::types::Error>(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                let at = request.at.map(|position| match position {
                    read_bindings::artist::tool::types::Position::Top => {
                        artist_kernel::Position::Top
                    }
                    read_bindings::artist::tool::types::Position::Bottom => {
                        artist_kernel::Position::Bottom
                    }
                    read_bindings::artist::tool::types::Position::At(anchor) => {
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
impl read_bindings::artist::tool::types::Host for HostState {}
impl write_bindings::artist::tool::host_write::Host for HostState {
    fn write(
        &mut self,
        requests: Vec<write_bindings::artist::tool::types::WriteRequest>,
    ) -> Vec<
        Result<
            write_bindings::artist::tool::types::WriteResult,
            write_bindings::artist::tool::types::Error,
        >,
    > {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<write_bindings::artist::tool::types::Error>(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
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
impl write_bindings::artist::tool::types::Host for HostState {}
impl edit_bindings::artist::tool::host_edit::Host for HostState {
    fn edit(
        &mut self,
        requests: Vec<edit_bindings::artist::tool::types::EditRequest>,
    ) -> Vec<
        Result<
            edit_bindings::artist::tool::types::EditResult,
            edit_bindings::artist::tool::types::Error,
        >,
    > {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<edit_bindings::artist::tool::types::Error>(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                let operations = request
                    .operations
                    .into_iter()
                    .map(|operation| match operation {
                        edit_bindings::artist::tool::types::EditOperation::Replace(replace) => {
                            Ok(artist_kernel::EditOperation::Replace(
                                artist_kernel::ReplaceOperation {
                                    start: anchor_from_string(&replace.start),
                                    end: replace.end.as_deref().map(anchor_from_string),
                                    content: replace.content,
                                },
                            ))
                        }
                        edit_bindings::artist::tool::types::EditOperation::Insert(insert) => Ok(
                            artist_kernel::EditOperation::Insert(artist_kernel::InsertOperation {
                                at: match insert.at {
                                    edit_bindings::artist::tool::types::InsertionPoint::Top => {
                                        artist_kernel::InsertionPoint::Top
                                    }
                                    edit_bindings::artist::tool::types::InsertionPoint::Bottom => {
                                        artist_kernel::InsertionPoint::Bottom
                                    }
                                    edit_bindings::artist::tool::types::InsertionPoint::Before(
                                        anchor,
                                    ) => artist_kernel::InsertionPoint::Before(anchor_from_string(
                                        &anchor,
                                    )),
                                    edit_bindings::artist::tool::types::InsertionPoint::After(
                                        anchor,
                                    ) => artist_kernel::InsertionPoint::After(anchor_from_string(
                                        &anchor,
                                    )),
                                },
                                content: insert.content,
                            }),
                        ),
                    })
                    .collect::<Result<Vec<_>, edit_bindings::artist::tool::types::Error>>()?;
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
impl edit_bindings::artist::tool::types::Host for HostState {}
impl run_bindings::artist::tool::host_run::Host for HostState {
    fn run(
        &mut self,
        requests: Vec<run_bindings::artist::tool::types::RunRequest>,
    ) -> Vec<Result<String, run_bindings::artist::tool::types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<run_bindings::artist::tool::types::Error>(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?;
                let working_uri = request
                    .working_uri
                    .map(|value| artist_kernel::ResourceUri::parse(&value))
                    .transpose()
                    .map_err(|error| {
                        typed_error_for::<run_bindings::artist::tool::types::Error>(
                            tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
                            error.to_string(),
                            Some(target.clone()),
                        )
                    })?;
                let environment = request
                    .environment
                    .into_iter()
                    .map(|entry| artist_kernel::EnvironmentEntry {
                        name: entry.name,
                        value: entry.value,
                    })
                    .collect();
                typed_host_invoke_operation(
                    self,
                    "run",
                    Some(target),
                    artist_kernel::Operation::Run(vec![artist_kernel::RunRequest {
                        uri,
                        args: request.args,
                        working_uri,
                        environment,
                    }]),
                )
            })
            .collect()
    }
}
impl run_bindings::artist::tool::types::Host for HostState {}

impl send_bindings::artist::tool::host_send::Host for HostState {
    fn send(
        &mut self,
        requests: Vec<send_bindings::artist::tool::types::SendRequest>,
    ) -> Vec<Result<String, send_bindings::artist::tool::types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let uri = artist_kernel::ResourceUri::parse(&target).map_err(|error| {
                    typed_error_for::<send_bindings::artist::tool::types::Error>(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
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
impl send_bindings::artist::tool::types::Host for HostState {}

impl find_bindings::artist::tool::host_find::Host for HostState {
    fn find(
        &mut self,
        request: find_bindings::artist::tool::types::FindRequest,
    ) -> Result<Vec<String>, find_bindings::artist::tool::types::Error> {
        let roots = request
            .roots
            .iter()
            .map(|root| {
                artist_kernel::ResourceUri::parse(root).map_err(|error| {
                    typed_error_for::<find_bindings::artist::tool::types::Error>(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
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
impl find_bindings::artist::tool::types::Host for HostState {}
impl grep_bindings::artist::tool::host_grep::Host for HostState {
    fn grep(
        &mut self,
        request: grep_bindings::artist::tool::types::GrepRequest,
    ) -> Result<
        Vec<grep_bindings::artist::tool::types::AnchoredText>,
        grep_bindings::artist::tool::types::Error,
    > {
        let source = match request.source {
            grep_bindings::artist::tool::types::GrepSource::Resources(uris) => {
                let uris = uris.iter().map(|uri| artist_kernel::ResourceUri::parse(uri).map_err(|error| typed_error_for::<grep_bindings::artist::tool::types::Error>(tool_bindings::artist::tool::types::ErrorCode::InvalidUri, error.to_string(), Some(uri.clone())))).collect::<Result<Vec<_>, _>>()?;
                artist_kernel::GrepSource::Resources(uris)
            }
            grep_bindings::artist::tool::types::GrepSource::Text(texts) => artist_kernel::GrepSource::Text(texts.into_iter().map(|text| -> Result<artist_kernel::AnchoredText, grep_bindings::artist::tool::types::Error> { Ok(artist_kernel::AnchoredText {
                uri: artist_kernel::ResourceUri::parse(&text.uri).map_err(|error| typed_error_for::<grep_bindings::artist::tool::types::Error>(tool_bindings::artist::tool::types::ErrorCode::InvalidUri, error.to_string(), Some(text.uri.clone())))?,
                lines: text.lines.into_iter().map(|line| artist_kernel::AnchoredLine {
                    anchor: anchor_from_string(&line.anchor),
                    text: line.text,
                    ending: match line.ending {
                        grep_bindings::artist::tool::types::LineEnding::None => artist_kernel::LineEnding::None,
                        grep_bindings::artist::tool::types::LineEnding::Lf => artist_kernel::LineEnding::Lf,
                        grep_bindings::artist::tool::types::LineEnding::Crlf => artist_kernel::LineEnding::Crlf,
                        grep_bindings::artist::tool::types::LineEnding::Cr => artist_kernel::LineEnding::Cr,
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
impl grep_bindings::artist::tool::types::Host for HostState {}
impl poll_bindings::artist::tool::host_poll::Host for HostState {
    fn poll(
        &mut self,
        request: poll_bindings::artist::tool::types::PollRequest,
    ) -> Result<
        poll_bindings::artist::tool::types::PollResult,
        poll_bindings::artist::tool::types::Error,
    > {
        let targets = request
            .targets
            .iter()
            .map(|target| {
                let uri = artist_kernel::ResourceUri::parse(&target.uri).map_err(|error| {
                    typed_error_for::<poll_bindings::artist::tool::types::Error>(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.uri.clone()),
                    )
                })?;
                let from_position = target.from_position.clone().map(|position| match position {
                    poll_bindings::artist::tool::types::Position::Top => {
                        artist_kernel::Position::Top
                    }
                    poll_bindings::artist::tool::types::Position::Bottom => {
                        artist_kernel::Position::Bottom
                    }
                    poll_bindings::artist::tool::types::Position::At(anchor) => {
                        artist_kernel::Position::At(anchor_from_string(&anchor))
                    }
                });
                Ok(artist_kernel::PollTarget { uri, from_position })
            })
            .collect::<Result<Vec<_>, poll_bindings::artist::tool::types::Error>>()?;
        let until = request.until.map(lower_poll_wire).transpose()?;
        let target = targets.first().map(|target| target.uri.to_string());
        typed_host_invoke_operation(
            self,
            "poll",
            target,
            artist_kernel::Operation::Poll(artist_kernel::PollRequest {
                targets,
                until,
                before: request.before,
                after: request.after,
            }),
        )
    }
}
impl poll_bindings::artist::tool::types::Host for HostState {}

fn lower_poll_wire(
    wire: poll_bindings::artist::tool::types::PollConditionWire,
) -> Result<artist_kernel::PollCondition, poll_bindings::artist::tool::types::Error> {
    fn lower(
        index: usize,
        nodes: &[poll_bindings::artist::tool::types::PollNode],
    ) -> Result<artist_kernel::PollCondition, poll_bindings::artist::tool::types::Error> {
        let node = nodes.get(index).ok_or_else(|| {
            typed_error_for::<poll_bindings::artist::tool::types::Error>(
                tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                format!("poll node {index} is out of range"),
                None,
            )
        })?;
        Ok(match node {
            poll_bindings::artist::tool::types::PollNode::Atom(atom) => {
                artist_kernel::PollCondition::Atom(match atom {
                    poll_bindings::artist::tool::types::PollAtom::Changed(lines) => {
                        artist_kernel::PollAtom::Changed(*lines)
                    }
                    poll_bindings::artist::tool::types::PollAtom::Regex(regex) => {
                        artist_kernel::PollAtom::Regex(artist_kernel::RegexAtom {
                            target: regex.target,
                            pattern: regex.pattern.clone(),
                        })
                    }
                    poll_bindings::artist::tool::types::PollAtom::Terminated(target) => {
                        artist_kernel::PollAtom::Terminated(*target)
                    }
                    poll_bindings::artist::tool::types::PollAtom::Timeout(milliseconds) => {
                        artist_kernel::PollAtom::Timeout(*milliseconds)
                    }
                })
            }
            poll_bindings::artist::tool::types::PollNode::All(children) => {
                artist_kernel::PollCondition::All(
                    children
                        .iter()
                        .map(|child| lower(*child as usize, nodes))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            poll_bindings::artist::tool::types::PollNode::Any(children) => {
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

impl abort_bindings::artist::tool::host_abort::Host for HostState {
    fn abort(
        &mut self,
        uris: Vec<String>,
    ) -> Vec<Result<String, abort_bindings::artist::tool::types::Error>> {
        uris.into_iter()
            .map(|uri| {
                let target = uri.clone();
                let parsed = artist_kernel::ResourceUri::parse(&uri).map_err(|error| {
                    typed_error_for::<abort_bindings::artist::tool::types::Error>(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
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
impl abort_bindings::artist::tool::types::Host for HostState {}

impl delete_bindings::artist::tool::host_delete::Host for HostState {
    fn delete(
        &mut self,
        uris: Vec<String>,
    ) -> Vec<Result<String, delete_bindings::artist::tool::types::Error>> {
        uris.into_iter()
            .map(|uri| {
                let target = uri.clone();
                let parsed = artist_kernel::ResourceUri::parse(&uri).map_err(|error| {
                    typed_error_for::<delete_bindings::artist::tool::types::Error>(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
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
impl delete_bindings::artist::tool::types::Host for HostState {}

fn anchor_from_string(value: &str) -> artist_kernel::Anchor {
    artist_kernel::Anchor::from_tokens(
        value
            .trim_start_matches('#')
            .split('.')
            .map(str::to_owned)
            .collect(),
    )
}

fn typed_error_for<E>(
    code: tool_bindings::artist::tool::types::ErrorCode,
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
            tool_bindings::artist::tool::types::ErrorCode::PermissionDenied,
            format!("capability denied: {capability}"),
            target,
        ));
    }
    let Some(kernel) = state.kernel.clone() else {
        return Err(typed_error_for(
            tool_bindings::artist::tool::types::ErrorCode::Internal,
            "typed tool host has no kernel".to_owned(),
            target,
        ));
    };
    let context = state.context.clone();
    let result = std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().map_err(|error| {
            artist_kernel::KernelError::Handler {
                message: error.to_string(),
            }
        })?;
        runtime.block_on(kernel.execute_operation_with_context(operation, context))
    })
    .join()
    .map_err(|_| {
        typed_error_for(
            tool_bindings::artist::tool::types::ErrorCode::Internal,
            "typed kernel bridge panicked".to_owned(),
            target.clone(),
        )
    })?
    .map_err(|error| typed_error_from_kernel(error, target.clone()))?;
    let value = operation_primary_value(result)
        .map_err(|error| typed_error_from_kernel(error, target.clone()))?;
    let mut value = serde_json::to_value(value).map_err(|error| {
        typed_error_for(
            tool_bindings::artist::tool::types::ErrorCode::Internal,
            error.to_string(),
            None,
        )
    })?;
    normalize_typed_value(&mut value);
    serde_json::from_value(value).map_err(|_| {
        typed_error_for(
            tool_bindings::artist::tool::types::ErrorCode::Internal,
            "typed host response conversion failed".to_owned(),
            None,
        )
    })
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
    code: tool_bindings::artist::tool::types::ErrorCode,
    message: String,
    uri: Option<String>,
) -> tool_bindings::artist::tool::types::Error {
    tool_bindings::artist::tool::types::Error { code, uri, message }
}

// Retained only as source archaeology while downstream legacy components are
// removed. Universal typed worlds never call this JSON bridge.
fn kernel_error_to_typed(
    error: artist_kernel::KernelError,
    target: Option<String>,
) -> tool_bindings::artist::tool::types::Error {
    use artist_kernel::KernelError;
    let (code, uri, message) = match error {
        KernelError::InvalidUri { message } => (
            tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
            target,
            message,
        ),
        KernelError::UnsupportedUri { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::Unsupported,
            Some(uri),
            "URI scheme is not supported".to_owned(),
        ),
        KernelError::NoHandler { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::Unsupported,
            Some(uri),
            "no handler is registered for this URI".to_owned(),
        ),
        KernelError::UnsupportedVerb { verb, uri } => (
            tool_bindings::artist::tool::types::ErrorCode::Unsupported,
            Some(uri),
            format!("verb {verb} is not supported for this resource"),
        ),
        KernelError::InvalidRequest { message } => (
            tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
            target,
            message,
        ),
        KernelError::InvalidPattern { message } => (
            tool_bindings::artist::tool::types::ErrorCode::InvalidPattern,
            target,
            message,
        ),
        KernelError::InvalidAnchor { message } => (
            tool_bindings::artist::tool::types::ErrorCode::InvalidAnchor,
            target,
            message,
        ),
        KernelError::StaleAnchor { message } => (
            tool_bindings::artist::tool::types::ErrorCode::StaleAnchor,
            target,
            message,
        ),
        KernelError::WrongKind { message } => (
            tool_bindings::artist::tool::types::ErrorCode::WrongKind,
            target,
            message,
        ),
        KernelError::NotFound { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::NotFound,
            Some(uri),
            "resource was not found".to_owned(),
        ),
        KernelError::AlreadyExists { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::AlreadyExists,
            Some(uri),
            "resource already exists".to_owned(),
        ),
        KernelError::Immutable { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::Immutable,
            Some(uri),
            "resource is immutable".to_owned(),
        ),
        KernelError::PermissionDenied { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::PermissionDenied,
            Some(uri),
            "permission denied".to_owned(),
        ),
        KernelError::Conflict { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::Conflict,
            Some(uri),
            "resource conflict".to_owned(),
        ),
        KernelError::NotEmpty { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::NotEmpty,
            Some(uri),
            "resource is not empty".to_owned(),
        ),
        KernelError::Aborted { message } => (
            tool_bindings::artist::tool::types::ErrorCode::Aborted,
            target,
            message,
        ),
        KernelError::InvalidState { message } => (
            tool_bindings::artist::tool::types::ErrorCode::InvalidState,
            target,
            message,
        ),
        KernelError::Handler { message } => (
            tool_bindings::artist::tool::types::ErrorCode::Internal,
            target,
            message,
        ),
    };
    typed_error(code, message, uri)
}

#[cfg(any())]
fn execute_typed_kernel_value(
    kernel: KernelHandle,
    verb_name: &str,
    target: String,
    args: serde_json::Value,
    context: artist_kernel::InvocationContext,
) -> Result<serde_json::Value, tool_bindings::artist::tool::types::Error> {
    let uri = (!target.is_empty())
        .then(|| artist_kernel::ResourceUri::parse(&target))
        .transpose()
        .map_err(|error| {
            typed_error(
                tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
                error.to_string(),
                (!target.is_empty()).then_some(target.clone()),
            )
        })?;
    let operation = match verb_name {
        "read" => artist_kernel::Operation::Read(vec![artist_kernel::ReadRequest {
            uri: uri.clone().ok_or_else(|| {
                typed_error(
                    tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                    "read requires a URI".to_owned(),
                    Some(target.clone()),
                )
            })?,
            at: None,
            before: None,
            after: None,
        }]),
        "write" => artist_kernel::Operation::Write(vec![artist_kernel::WriteRequest {
            uri: uri.clone().ok_or_else(|| {
                typed_error(
                    tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                    "write requires a URI".to_owned(),
                    Some(target.clone()),
                )
            })?,
            content: args
                .get("value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }]),
        "find" => artist_kernel::Operation::Find(artist_kernel::FindRequest {
            roots: args
                .get("roots")
                .and_then(serde_json::Value::as_array)
                .map(|roots| {
                    roots
                        .iter()
                        .filter_map(|root| root.as_str())
                        .map(artist_kernel::ResourceUri::parse)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()
                .map_err(|error| {
                    typed_error(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        None,
                    )
                })?
                .unwrap_or_else(|| uri.clone().into_iter().collect()),
            query: args
                .get("query")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        "grep" => artist_kernel::Operation::Grep(artist_kernel::GrepRequest {
            pattern: args
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            source: args
                .get("source")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| {
                    typed_error(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                        error.to_string(),
                        None,
                    )
                })?
                .unwrap_or_else(|| {
                    artist_kernel::GrepSource::Resources(uri.clone().into_iter().collect())
                }),
        }),
        "abort" => artist_kernel::Operation::Abort(vec![uri.clone().ok_or_else(|| {
            typed_error(
                tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                "abort requires a URI".to_owned(),
                Some(target.clone()),
            )
        })?]),
        "delete" => artist_kernel::Operation::Delete(vec![uri.clone().ok_or_else(|| {
            typed_error(
                tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                "delete requires a URI".to_owned(),
                Some(target.clone()),
            )
        })?]),
        "send" => artist_kernel::Operation::Send(vec![artist_kernel::SendRequest {
            uri: uri.clone().ok_or_else(|| {
                typed_error(
                    tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                    "send requires a URI".to_owned(),
                    Some(target.clone()),
                )
            })?,
            content: args
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }]),
        "run" => artist_kernel::Operation::Run(vec![artist_kernel::RunRequest {
            uri: uri.clone().ok_or_else(|| {
                typed_error(
                    tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                    "run requires a URI".to_owned(),
                    Some(target.clone()),
                )
            })?,
            args: args
                .get("args")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            working_uri: args
                .get("working_uri")
                .cloned()
                .filter(|value| !value.is_null())
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| {
                    typed_error(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })?,
            environment: serde_json::from_value(
                args.get("environment")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
            )
            .map_err(|error| {
                typed_error(
                    tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                    error.to_string(),
                    Some(target.clone()),
                )
            })?,
        }]),
        "edit" => {
            let mut operations = if args.is_array() {
                args
            } else {
                args.get("operations")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([]))
            };
            let operations = serde_json::from_value(operations).map_err(|error| {
                typed_error(
                    tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                    format!("invalid edit operations: {error}"),
                    Some(target.clone()),
                )
            })?;
            artist_kernel::Operation::Edit(vec![artist_kernel::EditRequest {
                uri: uri.ok_or_else(|| {
                    typed_error(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                        "edit requires a URI".to_owned(),
                        Some(target.clone()),
                    )
                })?,
                operations,
            }])
        }
        "poll" => {
            let targets = serde_json::from_value(
                args.get("targets")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
            )
            .map_err(|error| {
                typed_error(
                    tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                    format!("invalid poll targets: {error}"),
                    Some(target.clone()),
                )
            })?;
            let until = if let Some(condition) = args.get("until") {
                Some(lower_poll_condition(condition).map_err(|message| {
                    typed_error(
                        tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                        message,
                        Some(target.clone()),
                    )
                })?)
            } else {
                None
            };
            let mut request = artist_kernel::PollRequest {
                targets,
                until,
                before: args
                    .get("before")
                    .and_then(serde_json::Value::as_u64)
                    .map(|v| v as u32),
                after: args
                    .get("after")
                    .and_then(serde_json::Value::as_u64)
                    .map(|v| v as u32),
            };
            if request.targets.is_empty() {
                request.targets.push(artist_kernel::PollTarget {
                    uri: uri.ok_or_else(|| {
                        typed_error(
                            tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                            "poll requires a URI".to_owned(),
                            Some(target.clone()),
                        )
                    })?,
                    from_position: None,
                });
            }
            artist_kernel::Operation::Poll(request)
        }
        _ => {
            return Err(typed_error(
                tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                format!("typed operation {verb_name} is not yet representable"),
                Some(target),
            ));
        }
    };
    let result = std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
        runtime
            .block_on(kernel.execute_operation_with_context(operation, context))
            .map_err(|error| error.to_string())
    })
    .join()
    .map_err(|_| {
        typed_error(
            tool_bindings::artist::tool::types::ErrorCode::Internal,
            "typed kernel bridge panicked".to_owned(),
            Some(target.clone()),
        )
    })?
    .map_err(|message| {
        typed_error(
            tool_bindings::artist::tool::types::ErrorCode::Internal,
            message,
            Some(target.clone()),
        )
    })?;
    let value = match result {
        artist_kernel::OperationResult::Read(mut values) => values
            .remove(0)
            .map_err(|error| kernel_error_to_typed(error, Some(target.clone())))
            .and_then(|value| {
                serde_json::to_value(value).map_err(|error| {
                    typed_error(
                        tool_bindings::artist::tool::types::ErrorCode::Internal,
                        error.to_string(),
                        Some(target.clone()),
                    )
                })
            })?,
        artist_kernel::OperationResult::Find(value) => serde_json::to_value(
            value.map_err(|error| kernel_error_to_typed(error, Some(target.clone())))?,
        )
        .unwrap(),
        artist_kernel::OperationResult::Grep(value) => serde_json::to_value(
            value.map_err(|error| kernel_error_to_typed(error, Some(target.clone())))?,
        )
        .unwrap(),
        artist_kernel::OperationResult::Write(mut values) => serde_json::to_value(
            values
                .remove(0)
                .map_err(|error| kernel_error_to_typed(error, Some(target.clone())))?,
        )
        .unwrap(),
        artist_kernel::OperationResult::Edit(mut values) => serde_json::to_value(
            values
                .remove(0)
                .map_err(|error| kernel_error_to_typed(error, Some(target.clone())))?,
        )
        .unwrap(),
        artist_kernel::OperationResult::Poll(value) => serde_json::to_value(
            value.map_err(|error| kernel_error_to_typed(error, Some(target.clone())))?,
        )
        .unwrap(),
        artist_kernel::OperationResult::Send(mut values)
        | artist_kernel::OperationResult::Run(mut values)
        | artist_kernel::OperationResult::Abort(mut values)
        | artist_kernel::OperationResult::Delete(mut values) => serde_json::to_value(
            values
                .remove(0)
                .map_err(|error| kernel_error_to_typed(error, Some(target.clone())))?,
        )
        .unwrap(),
    };
    Ok(value)
}

/// Lower the component-facing indexed poll condition into the recursive
/// kernel semantic value. The indexed form exists only because WIT records
/// cannot recursively refer to themselves in all host binding modes.
#[cfg(any())]
fn lower_poll_condition(value: &serde_json::Value) -> Result<artist_kernel::PollCondition, String> {
    let nodes = value
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "poll condition requires nodes".to_owned())?;
    let root = value
        .get("root")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "poll condition requires root".to_owned())? as usize;
    fn lower(
        index: usize,
        nodes: &[serde_json::Value],
    ) -> Result<artist_kernel::PollCondition, String> {
        let node = nodes
            .get(index)
            .ok_or_else(|| format!("poll condition node {index} is out of range"))?;
        if let Some(atom) = node.get("atom") {
            let value = if let Some(object) = atom.as_object() {
                if let Some(lines) = object.get("changed").and_then(serde_json::Value::as_u64) {
                    artist_kernel::PollAtom::Changed(lines as u32)
                } else if let Some(regex) = object.get("regex") {
                    artist_kernel::PollAtom::Regex(artist_kernel::RegexAtom {
                        target: regex
                            .get("target")
                            .and_then(serde_json::Value::as_u64)
                            .ok_or_else(|| "regex atom requires target".to_owned())?
                            as u32,
                        pattern: regex
                            .get("pattern")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| "regex atom requires pattern".to_owned())?
                            .to_owned(),
                    })
                } else if let Some(target) =
                    object.get("terminated").and_then(serde_json::Value::as_u64)
                {
                    artist_kernel::PollAtom::Terminated(target as u32)
                } else if let Some(milliseconds) =
                    object.get("timeout").and_then(serde_json::Value::as_u64)
                {
                    artist_kernel::PollAtom::Timeout(milliseconds)
                } else {
                    return Err("unknown poll atom".to_owned());
                }
            } else if let Some(lines) = atom.as_u64() {
                artist_kernel::PollAtom::Changed(lines as u32)
            } else {
                return Err("invalid poll atom".to_owned());
            };
            return Ok(artist_kernel::PollCondition::Atom(value));
        }
        for (key, constructor) in [("all", true), ("any", false)] {
            if let Some(children) = node.get(key).and_then(serde_json::Value::as_array) {
                let children = children
                    .iter()
                    .map(|child| {
                        child
                            .as_u64()
                            .ok_or_else(|| "poll boolean child must be an index".to_owned())
                            .and_then(|index| lower(index as usize, nodes))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(if constructor {
                    artist_kernel::PollCondition::All(children)
                } else {
                    artist_kernel::PollCondition::Any(children)
                });
            }
        }
        Err(format!("unknown poll node {index}"))
    }
    lower(root, nodes)
}

/// A loaded, validated component instance.
pub struct ComponentHost {
    engine: wasmtime::Engine,
    component: wasmtime::component::Component,
    capabilities: HashSet<String>,
    bytes: Arc<Vec<u8>>,
}

/// Host for components that implement the typed `artist:tool` worlds.
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

fn validate_typed_contract_names(
    verb: contracts::Verb,
    exports: &[String],
    tool_imports: &[String],
) -> Result<(), String> {
    let expected_export = format!("artist:tool/{}@0.1.0", verb.interface());
    if exports != &[expected_export.clone()] {
        return Err(format!(
            "typed contract {} requires exactly export {}, found {:?}",
            contracts::ContractId::universal(verb),
            expected_export,
            exports
        ));
    }
    let expected_import = format!("artist:tool/host-{}@0.1.0", verb.interface());
    let expected_imports = vec![
        "artist:tool/types@0.1.0".to_owned(),
        expected_import.clone(),
    ];
    if tool_imports != expected_imports {
        return Err(format!(
            "typed contract {} requires type definitions and exactly import {}, found {:?}",
            contracts::ContractId::universal(verb),
            expected_import,
            tool_imports
        ));
    }
    Ok(())
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
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config)
            .map_err(|e| ComponentError::Load(anyhow::anyhow!(e.to_string())))?;
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
            .filter(|name| name.starts_with("artist:tool/"))
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
        macro_rules! add {
            ($world:ty) => {
                <$world>::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
                    &mut linker,
                    |state: &mut HostState| state,
                )
                .map_err(|e| ComponentError::Load(anyhow::anyhow!(e.to_string())))?;
            };
        }
        match verb {
            contracts::Verb::Read => {
                add!(read_bindings::ReadWorld);
            }
            contracts::Verb::Write => {
                add!(write_bindings::WriteWorld);
            }
            contracts::Verb::Edit => {
                add!(edit_bindings::EditWorld);
            }
            contracts::Verb::Find => {
                add!(find_bindings::FindWorld);
            }
            contracts::Verb::Grep => {
                add!(grep_bindings::GrepWorld);
            }
            contracts::Verb::Run => {
                add!(run_bindings::RunWorld);
            }
            contracts::Verb::Send => {
                add!(send_bindings::SendWorld);
            }
            contracts::Verb::Abort => {
                add!(abort_bindings::AbortWorld);
            }
            contracts::Verb::Delete => {
                add!(delete_bindings::DeleteWorld);
            }
            contracts::Verb::Poll => {
                add!(poll_bindings::PollWorld);
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
        let mut store = wasmtime::Store::new(
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

    pub fn validate_dynamic_export(&self, export_name: &str) -> Result<(), ComponentError> {
        let mut store = wasmtime::Store::new(
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
        requests: Vec<tool_bindings::artist::tool::types::ReadRequest>,
        kernel: KernelHandle,
    ) -> Result<
        Vec<
            Result<
                tool_bindings::artist::tool::types::ReadResult,
                tool_bindings::artist::tool::types::Error,
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
        let mut store = wasmtime::Store::new(
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
                invoke!(
                    read_bindings,
                    ReadWorld,
                    Vec<read_bindings::artist::tool::types::ReadRequest>,
                    artist_tool_read,
                    call_read,
                    input
                )
            }
            contracts::Verb::Write => {
                invoke!(
                    write_bindings,
                    WriteWorld,
                    Vec<write_bindings::artist::tool::types::WriteRequest>,
                    artist_tool_write,
                    call_write,
                    input
                )
            }
            contracts::Verb::Edit => {
                invoke!(
                    edit_bindings,
                    EditWorld,
                    Vec<edit_bindings::artist::tool::types::EditRequest>,
                    artist_tool_edit,
                    call_edit,
                    input
                )
            }
            contracts::Verb::Find => {
                invoke!(
                    find_bindings,
                    FindWorld,
                    find_bindings::artist::tool::types::FindRequest,
                    artist_tool_find,
                    call_find,
                    input
                )
            }
            contracts::Verb::Grep => {
                invoke!(
                    grep_bindings,
                    GrepWorld,
                    grep_bindings::artist::tool::types::GrepRequest,
                    artist_tool_grep,
                    call_grep,
                    input
                )
            }
            contracts::Verb::Run => {
                invoke!(
                    run_bindings,
                    RunWorld,
                    Vec<run_bindings::artist::tool::types::RunRequest>,
                    artist_tool_run,
                    call_run,
                    input
                )
            }
            contracts::Verb::Send => {
                invoke!(
                    send_bindings,
                    SendWorld,
                    Vec<send_bindings::artist::tool::types::SendRequest>,
                    artist_tool_send,
                    call_send,
                    input
                )
            }
            contracts::Verb::Abort => {
                invoke!(
                    abort_bindings,
                    AbortWorld,
                    Vec<String>,
                    artist_tool_abort,
                    call_abort,
                    input
                )
            }
            contracts::Verb::Delete => {
                invoke!(
                    delete_bindings,
                    DeleteWorld,
                    Vec<String>,
                    artist_tool_delete,
                    call_delete,
                    input
                )
            }
            contracts::Verb::Poll => {
                invoke!(
                    poll_bindings,
                    PollWorld,
                    poll_bindings::artist::tool::types::PollRequest,
                    artist_tool_poll,
                    call_poll,
                    input
                )
            }
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
    macro_rules! add {
        ($world:ty) => {
            <$world>::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
                &mut linker,
                |state: &mut HostState| state,
            )
            .map_err(|error| ComponentError::Load(anyhow::anyhow!(error.to_string())))?;
        };
    }
    add!(read_bindings::ReadWorld);
    add!(write_bindings::WriteWorld);
    add!(edit_bindings::EditWorld);
    add!(find_bindings::FindWorld);
    add!(grep_bindings::GrepWorld);
    add!(run_bindings::RunWorld);
    add!(send_bindings::SendWorld);
    add!(abort_bindings::AbortWorld);
    add!(delete_bindings::DeleteWorld);
    add!(poll_bindings::PollWorld);

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
    use super::{ABI_VERSION, ComponentError, ComponentHost, TypedComponentHost};
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
    };
    use walkdir::WalkDir;

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
        fn directory(self) -> &'static str {
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

    #[derive(Debug, Deserialize)]
    struct CargoMetadata {
        packages: Vec<CargoPackage>,
    }

    #[derive(Debug, Deserialize)]
    struct CargoPackage {
        name: String,
        version: String,
        manifest_path: PathBuf,
        targets: Vec<CargoTarget>,
    }

    #[derive(Debug, Deserialize)]
    struct CargoTarget {
        name: String,
        kind: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct CargoBuildMessage {
        reason: String,
        target: Option<CargoMessageTarget>,
        filenames: Option<Vec<PathBuf>>,
    }

    #[derive(Debug, Deserialize)]
    struct CargoMessageTarget {
        name: String,
    }

    impl ToolPackage {
        pub fn discover(root: impl AsRef<Path>) -> Result<Self, ComponentError> {
            let root = root.as_ref().to_owned();
            let markdown = fs::read_to_string(root.join("tool.md"))
                .map_err(|e| ComponentError::Load(anyhow::anyhow!(e)))?;
            let (frontmatter, prose) = split_frontmatter(&markdown)?;
            let manifest: ToolFrontmatter = serde_yaml::from_str(frontmatter)
                .map_err(|e| ComponentError::Load(anyhow::anyhow!(e)))?;
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
                prose: prose.trim().to_owned(),
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
            let metadata = cargo_metadata(manifest)?;
            let package = metadata
                .packages
                .iter()
                .find(|package| package.manifest_path == *manifest)
                .or_else(|| metadata.packages.first())
                .ok_or_else(|| ComponentError::Build {
                    diagnostics: "Cargo metadata contained no package".to_owned(),
                })?;
            let target = package
                .targets
                .iter()
                .find(|target| target.kind.iter().any(|kind| kind == "cdylib"))
                .or_else(|| {
                    package
                        .targets
                        .iter()
                        .find(|target| target.kind.iter().any(|kind| kind == "bin"))
                })
                .ok_or_else(|| ComponentError::Build {
                    diagnostics: "package must declare a cdylib or bin target".to_owned(),
                })?;

            let fingerprint = fingerprint(self, options, &package.version)?;
            if !options.force {
                if let Some((artifact, provenance)) = find_cached_artifact(
                    &self.root,
                    &target.name,
                    &options.target,
                    options.profile,
                    &fingerprint,
                )? {
                    validate_artifact(&artifact, self, options)?;
                    return Ok(BuildResult {
                        artifact,
                        provenance,
                        fingerprint,
                        cached: true,
                    });
                }
            }

            let mut command = Command::new("cargo");
            command
                .arg("build")
                .arg("--manifest-path")
                .arg(manifest)
                .arg("--target")
                .arg(&options.target)
                .arg("--message-format")
                .arg("json-render-diagnostics")
                .arg("--offline");
            let mut rustflags = std::env::var("RUSTFLAGS").unwrap_or_default();
            if !rustflags.contains("target-cpu") {
                if !rustflags.is_empty() {
                    rustflags.push(' ');
                }
                rustflags.push_str("-C target-cpu=native");
            }
            command.env("RUSTFLAGS", rustflags);
            match options.profile {
                BuildProfile::Debug => {}
                BuildProfile::Release => {
                    command.arg("--release");
                }
                BuildProfile::Product => {
                    command.args(["--profile", "product"]);
                }
            }
            let output = command.output().map_err(|error| ComponentError::Build {
                diagnostics: format!("could not execute cargo: {error}"),
            })?;
            if !output.status.success() {
                return Err(ComponentError::Build {
                    diagnostics: command_diagnostics(&output.stdout, &output.stderr),
                });
            }
            let artifact = cargo_artifact(
                &output.stdout,
                &target.name,
                &options.target,
                options.profile,
            )
            .ok_or_else(|| ComponentError::Build {
                diagnostics: "cargo succeeded but emitted no WASM component artifact".to_owned(),
            })?;
            if !artifact.is_file() {
                return Err(ComponentError::Build {
                    diagnostics: format!(
                        "cargo succeeded but did not produce expected component {}",
                        artifact.display()
                    ),
                });
            }
            let provenance = artifact.with_extension("wasm.artist.json");

            validate_artifact(&artifact, self, options)?;
            let provenance_value = BuildProvenance {
                package_name: package.name.clone(),
                package_version: package.version.clone(),
                contract: self.contract.as_ref().map(ToString::to_string),
                abi_version: ABI_VERSION.to_owned(),
                target: options.target.clone(),
                profile: options.profile.directory().to_owned(),
                fingerprint: fingerprint.clone(),
                artifact_sha256: sha256_file(&artifact)?,
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

    fn cargo_metadata(manifest: &Path) -> Result<CargoMetadata, ComponentError> {
        let output = Command::new("cargo")
            .args([
                "metadata",
                "--no-deps",
                "--format-version",
                "1",
                "--offline",
            ])
            .arg("--manifest-path")
            .arg(manifest)
            .output()
            .map_err(|error| ComponentError::Build {
                diagnostics: format!("could not execute cargo metadata: {error}"),
            })?;
        if !output.status.success() {
            return Err(ComponentError::Build {
                diagnostics: command_diagnostics(&output.stdout, &output.stderr),
            });
        }
        serde_json::from_slice(&output.stdout).map_err(|error| ComponentError::Build {
            diagnostics: format!("invalid cargo metadata: {error}"),
        })
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
            hash_path(&mut hasher, path)?;
        }
        hash_path(
            &mut hasher,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("wit/tool-surface/world.wit"),
        )?;
        hash_path(
            &mut hasher,
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/typed-guest/src"),
        )?;
        if let Some(source) = &package.source {
            hash_path(&mut hasher, source)?;
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn hash_path(hasher: &mut Sha256, path: &Path) -> Result<(), ComponentError> {
        if path.is_file() {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(fs::read(path).map_err(|error| ComponentError::Build {
                diagnostics: format!("could not read {}: {error}", path.display()),
            })?);
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
            ComponentHost::new_with_capabilities(&bytes, options.granted_capabilities.clone())
                .map_err(|error| ComponentError::Build {
                    diagnostics: format!("component validation failed: {error}"),
                })?;
        }
        if package.manifest.name.is_empty() {
            return Err(ComponentError::Build {
                diagnostics: "tool name cannot be empty".to_owned(),
            });
        }
        Ok(())
    }

    fn read_provenance(path: &Path) -> Result<BuildProvenance, ComponentError> {
        let bytes = fs::read(path).map_err(|error| ComponentError::Build {
            diagnostics: error.to_string(),
        })?;
        serde_json::from_slice(&bytes).map_err(|error| ComponentError::Build {
            diagnostics: error.to_string(),
        })
    }

    fn find_cached_artifact(
        root: &Path,
        target_name: &str,
        target: &str,
        profile: BuildProfile,
        fingerprint: &str,
    ) -> Result<Option<(PathBuf, PathBuf)>, ComponentError> {
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
                if let Ok(value) = read_provenance(&provenance)
                    && value.fingerprint == fingerprint
                    && sha256_file(&artifact).is_ok_and(|hash| hash == value.artifact_sha256)
                {
                    return Ok(Some((artifact, provenance)));
                }
            }
            directory = current.parent();
        }
        Ok(None)
    }

    fn cargo_artifact(
        stdout: &[u8],
        target_name: &str,
        target: &str,
        profile: BuildProfile,
    ) -> Option<PathBuf> {
        stdout
            .split(|byte| *byte == b'\n')
            .filter_map(|line| serde_json::from_slice::<CargoBuildMessage>(line).ok())
            .filter(|message| message.reason == "compiler-artifact")
            .filter(|message| {
                message
                    .target
                    .as_ref()
                    .is_some_and(|value| value.name == target_name)
            })
            .flat_map(|message| message.filenames.unwrap_or_default())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "wasm")
                    && path
                        .components()
                        .any(|component| component.as_os_str() == target)
                    && path
                        .components()
                        .any(|component| component.as_os_str() == profile.directory())
            })
    }

    fn sha256_file(path: &Path) -> Result<String, ComponentError> {
        let bytes = fs::read(path).map_err(|error| ComponentError::Build {
            diagnostics: error.to_string(),
        })?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    fn tool_version(tool: &str) -> String {
        Command::new(tool)
            .arg("--version")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_else(|| "unknown".to_owned())
    }

    fn command_diagnostics(stdout: &[u8], stderr: &[u8]) -> String {
        let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
        if !stderr.is_empty() {
            stderr
        } else {
            String::from_utf8_lossy(stdout).trim().to_owned()
        }
    }

    fn split_frontmatter(markdown: &str) -> Result<(&str, &str), ComponentError> {
        let mut lines = markdown.lines();
        if lines.next() != Some("---") {
            return Err(ComponentError::Load(anyhow::anyhow!(
                "tool.md must begin with YAML frontmatter"
            )));
        }
        let Some(end) = markdown[4..].find("\n---") else {
            return Err(ComponentError::Load(anyhow::anyhow!(
                "tool.md frontmatter is not closed"
            )));
        };
        let end = end + 4;
        Ok((&markdown[4..end], &markdown[end + 4..]))
    }
}

impl ComponentHost {
    fn linker(&self) -> Result<wasmtime::component::Linker<HostState>, ComponentError> {
        let mut linker = wasmtime::component::Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| ComponentError::Load(anyhow::anyhow!(e.to_string())))?;
        bindings::ArtistWorld::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
            &mut linker,
            |state: &mut HostState| state,
        )
        .map_err(|e| ComponentError::Load(anyhow::anyhow!(e.to_string())))?;
        Ok(linker)
    }

    pub fn new(bytes: &[u8]) -> Result<Self, ComponentError> {
        Self::new_with_capabilities(bytes, std::iter::empty::<String>())
    }

    pub fn new_with_capabilities<I>(bytes: &[u8], capabilities: I) -> Result<Self, ComponentError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config)
            .map_err(|e| ComponentError::Load(anyhow::anyhow!(e.to_string())))?;
        let component = wasmtime::component::Component::from_binary(&engine, bytes)
            .map_err(|e| ComponentError::Load(anyhow::anyhow!(e.to_string())))?;
        let host = Self {
            engine,
            component,
            capabilities: capabilities.into_iter().collect(),
            bytes: Arc::new(bytes.to_vec()),
        };
        host.validate()?;
        Ok(host)
    }

    pub fn validate(&self) -> Result<ComponentInfo, ComponentError> {
        let info = self.info()?;
        validate_component_info(&info)?;
        validate_capabilities(&info, &self.capabilities)?;
        Ok(info)
    }

    pub fn info(&self) -> Result<ComponentInfo, ComponentError> {
        let mut store = wasmtime::Store::new(
            &self.engine,
            HostState::with_capabilities(self.capabilities.iter().cloned()),
        );
        let linker = self.linker()?;
        let instance = bindings::ArtistWorld::instantiate(&mut store, &self.component, &linker)
            .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?;
        instance
            .artist_component_component()
            .call_info(&mut store)
            .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))
    }

    pub fn invoke(&self, request: Invocation) -> Result<Response, ComponentError> {
        serde_json::from_str::<serde_json::Value>(&request.input)
            .map_err(|_| ComponentError::MalformedResponse { field: "input" })?;
        let mut store = wasmtime::Store::new(
            &self.engine,
            HostState::with_capabilities(self.capabilities.iter().cloned()),
        );
        let linker = self.linker()?;
        let instance = bindings::ArtistWorld::instantiate(&mut store, &self.component, &linker)
            .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?;
        let response = instance
            .artist_component_component()
            .call_invoke(&mut store, &request)
            .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?
            .map_err(ComponentError::Abi)?;
        serde_json::from_str::<serde_json::Value>(&response.output)
            .map_err(|_| ComponentError::MalformedResponse { field: "output" })?;
        serde_json::from_str::<serde_json::Value>(&response.metadata)
            .map_err(|_| ComponentError::MalformedResponse { field: "metadata" })?;
        Ok(response)
    }

    /// Invoke a component with the kernel capability bridge installed. The
    /// component remains the execution boundary; resource effects occur only
    /// through the explicitly supplied kernel handle.
    pub fn invoke_with_kernel(
        &self,
        request: Invocation,
        kernel: KernelHandle,
    ) -> Result<Response, ComponentError> {
        serde_json::from_str::<serde_json::Value>(&request.input)
            .map_err(|_| ComponentError::MalformedResponse { field: "input" })?;
        let mut store = wasmtime::Store::new(
            &self.engine,
            HostState::with_kernel(self.capabilities.iter().cloned(), kernel),
        );
        let linker = self.linker()?;
        let instance = bindings::ArtistWorld::instantiate(&mut store, &self.component, &linker)
            .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?;
        let response = instance
            .artist_component_component()
            .call_invoke(&mut store, &request)
            .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?
            .map_err(ComponentError::Abi)?;
        serde_json::from_str::<serde_json::Value>(&response.output)
            .map_err(|_| ComponentError::MalformedResponse { field: "output" })?;
        serde_json::from_str::<serde_json::Value>(&response.metadata)
            .map_err(|_| ComponentError::MalformedResponse { field: "metadata" })?;
        Ok(response)
    }

    pub fn invoke_stream(&self, request: Invocation) -> Result<Vec<StreamChunk>, ComponentError> {
        let mut store = wasmtime::Store::new(
            &self.engine,
            HostState::with_capabilities(self.capabilities.iter().cloned()),
        );
        let linker = self.linker()?;
        let instance = bindings::ArtistWorld::instantiate(&mut store, &self.component, &linker)
            .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?;
        instance
            .artist_component_component()
            .call_invoke_stream(&mut store, &request)
            .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?
            .map_err(ComponentError::Abi)
    }

    pub async fn invoke_async(&self, request: Invocation) -> Result<Response, ComponentError> {
        let bytes = self.bytes.clone();
        let capabilities = self.capabilities.clone();
        tokio::task::spawn_blocking(move || {
            Self::new_with_capabilities(&bytes, capabilities)?.invoke(request)
        })
        .await
        .map_err(|e| ComponentError::Invoke(anyhow::anyhow!(e.to_string())))?
    }

    pub fn invoke_with_cancellation(
        &self,
        request: Invocation,
        cancellation: &Cancellation,
    ) -> Result<Response, ComponentError> {
        if cancellation.is_cancelled() {
            return Err(ComponentError::Cancelled);
        }
        let result = self.invoke(request);
        if cancellation.is_cancelled() {
            return Err(ComponentError::Cancelled);
        }
        result
    }
}

/// Versioned component activation and hot-reload coordination.
pub mod runtime {
    use super::{
        ComponentError, ComponentHost, ComponentInfo, Invocation, Response, StreamChunk,
        TypedComponentHost,
        package::{BuildOptions, ToolPackage},
    };
    use std::{
        collections::HashMap,
        fs,
        path::PathBuf,
        sync::{Arc, Mutex, RwLock},
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
        info: ComponentInfo,
        host: Option<Arc<ComponentHost>>,
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

        pub fn info(&self) -> &ComponentInfo {
            &self.info
        }

        pub fn invoke(&self, request: Invocation) -> Result<Response, ComponentError> {
            self.host
                .as_ref()
                .ok_or_else(|| {
                    ComponentError::Invoke(anyhow::anyhow!(
                        "typed component requires a verb contract"
                    ))
                })?
                .invoke(request)
        }

        pub fn invoke_with_kernel(
            &self,
            request: Invocation,
            kernel: artist_kernel::KernelHandle,
        ) -> Result<Response, ComponentError> {
            self.host
                .as_ref()
                .ok_or_else(|| {
                    ComponentError::Invoke(anyhow::anyhow!(
                        "typed component requires a verb contract"
                    ))
                })?
                .invoke_with_kernel(request, kernel)
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
            if let Some(host) = &self.typed_host {
                host.invoke_json_with_context(verb, input, kernel, context)
            } else {
                let response = self.invoke_with_kernel(
                    Invocation {
                        id: format!("tool:{verb}"),
                        operation: format!("tool.{verb}"),
                        target: String::new(),
                        input: input.to_owned(),
                        context: super::Context {
                            cancellation_token: context.cancellation_token.unwrap_or_default(),
                            deadline_ms: context.deadline_ms,
                            correlation_id: context.correlation_id.unwrap_or_default(),
                        },
                    },
                    kernel,
                )?;
                Ok(response.output)
            }
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

        pub fn invoke_stream(
            &self,
            request: Invocation,
        ) -> Result<Vec<StreamChunk>, ComponentError> {
            self.host
                .as_ref()
                .ok_or_else(|| {
                    ComponentError::Invoke(anyhow::anyhow!(
                        "typed component has no generic stream entrypoint"
                    ))
                })?
                .invoke_stream(request)
        }

        pub async fn invoke_async(&self, request: Invocation) -> Result<Response, ComponentError> {
            self.host
                .as_ref()
                .ok_or_else(|| {
                    ComponentError::Invoke(anyhow::anyhow!(
                        "typed component requires typed dispatch"
                    ))
                })?
                .invoke_async(request)
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
                host: self.host.as_ref().map(Arc::clone),
                typed_host: self.typed_host.as_ref().map(Arc::clone),
                dynamic_host: self.dynamic_host.as_ref().map(Arc::clone),
            }
        }
    }

    #[derive(Clone, Debug)]
    pub struct InvocationResult {
        pub generation: u64,
        pub response: Response,
    }

    /// Registry of the currently active component version for each package.
    /// Building and validation happen before the write-side swap, so a failed
    /// reload cannot disturb the active version.
    #[derive(Clone, Default)]
    pub struct ComponentRegistry {
        active: Arc<RwLock<HashMap<String, Arc<ActiveVersion>>>>,
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
            let (host, typed_host, dynamic_host, info) = if package
                .contract
                .as_ref()
                .and_then(|contract| contract.verb)
                .is_some()
            {
                let typed_host = Arc::new(TypedComponentHost::new_with_capabilities(
                    &bytes,
                    options.granted_capabilities.clone(),
                )?);
                let info = ComponentInfo {
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
                (None, Some(typed_host), None, info)
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
                let info = ComponentInfo {
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
                (None, None, Some(dynamic_host), info)
            } else {
                let host = Arc::new(ComponentHost::new_with_capabilities(
                    &bytes,
                    options.granted_capabilities.clone(),
                )?);
                let info = host.info()?;
                (Some(host), None, None, info)
            };
            let package_name = package.manifest.name.clone();

            let mut active = self.active.write().unwrap();
            let generation = active
                .get(&package_name)
                .map_or(1, |current| current.generation + 1);
            let version = Arc::new(ActiveVersion {
                package: package_name.clone(),
                generation,
                fingerprint: build.fingerprint,
                provenance: build.provenance,
                artifact: Arc::new(bytes),
                info,
                host,
                typed_host,
                dynamic_host,
            });
            active.insert(package_name, Arc::clone(&version));
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

        pub fn invoke(
            &self,
            package: &str,
            request: Invocation,
        ) -> Result<InvocationResult, ComponentError> {
            let version = self.current(package)?;
            let generation = version.generation;
            let response = version.invoke(request)?;
            Ok(InvocationResult {
                generation,
                response,
            })
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
            let this = self.clone();
            tokio::task::spawn_blocking(move || this.invoke(args, kernel))
                .await
                .map_err(|error| KernelError::Handler {
                    message: format!("component tool task failed: {error}"),
                })?
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
                        Some(super::contracts::Verb::Poll) => serde_json::json!({"type":"object","required":["targets"],"properties":{"targets":{"type":"array"},"until":{},"before":{"type":"integer"},"after":{"type":"integer"}}}),
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
                dirty: Arc::new(Mutex::new(std::collections::HashSet::new())),
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
                let package = ToolPackage::discover(entry.path()).map_err(component_error)?;
                let Some(contract) = package.contract else {
                    continue;
                };
                registrations.push(ToolRegistration {
                    package: package.manifest.name,
                    contract,
                    description: package.manifest.description,
                    version: package.manifest.version,
                    input_schema: package.manifest.input_schema,
                    output_schema: package.manifest.output_schema,
                    capabilities: package.manifest.capabilities,
                });
            }
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
            ResourceUri::parse(&self.root.join(relative).display().to_string())
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
            ResourceUri::parse(&format!("tools:///{relative}"))
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
                                value.uri = self.unmap_typed_uri(value.uri)?;
                                value.changed = value
                                    .changed
                                    .into_iter()
                                    .map(|text| self.unmap_text(text))
                                    .collect::<Result<_, _>>()?;
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
            let package = ToolPackage::discover(package_root).map_err(component_error)?;
            let key = package_root.to_owned();
            let current = self.registry.current(&package.manifest.name).ok();
            let dirty = self.dirty.lock().unwrap().contains(&key);
            if current.is_some() && !dirty {
                return Ok(current.unwrap());
            }
            let dependencies = if package.wit.is_some()
                && package
                    .contract
                    .as_ref()
                    .is_some_and(|contract| contract.verb.is_none())
            {
                self.custom_dependencies(&package)?
            } else {
                Vec::new()
            };
            let active = self
                .registry
                .reload_with_dependency_specs(&package, &self.options, dependencies)
                .map_err(component_error)?;
            self.dirty.lock().unwrap().remove(&key);
            Ok(active)
        }

        fn custom_dependencies(
            &self,
            current: &ToolPackage,
        ) -> Result<Vec<super::DependencySpec>, KernelError> {
            let mut config = wasmtime::Config::new();
            config.wasm_component_model(true);
            let engine = wasmtime::Engine::new(&config).map_err(|error| KernelError::Handler {
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
                    let build = package.build(options).map_err(component_error)?;
                    std::fs::read(build.artifact).map_err(|error| KernelError::Handler {
                        message: error.to_string(),
                    })?
                };
                if !visiting.insert(contract.to_owned()) {
                    return Err(KernelError::InvalidState {
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
                let package = ToolPackage::discover(entry.path()).map_err(component_error)?;
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

        fn execute_named_inner(
            &self,
            name: &str,
            args: Value,
            host: KernelHandle,
        ) -> Result<Value, KernelError> {
            self.execute_named_inner_with_context(
                name,
                args,
                host,
                artist_kernel::InvocationContext::default(),
            )
        }

        fn execute_named_inner_with_context(
            &self,
            name: &str,
            args: Value,
            host: KernelHandle,
            context: artist_kernel::InvocationContext,
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
                return ComponentTool::new(active, verb).invoke_with_context(args, host, context);
            }

            active
                .invoke_dynamic_json_with_context(
                    &args,
                    &registration.contract.interface,
                    host,
                    context,
                )
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
            Box::pin(async move { self.execute_named_inner(name, args, host) })
        }

        fn execute_tool_with_context<'a>(
            &'a self,
            name: &'a str,
            args: Value,
            host: KernelHandle,
            context: artist_kernel::InvocationContext,
        ) -> BoxFuture<'a, Result<Value, KernelError>> {
            Box::pin(
                async move { self.execute_named_inner_with_context(name, args, host, context) },
            )
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
        use artist_kernel::{Kernel, Verb as KernelVerb};
        use tempfile::tempdir;

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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, process::Command};

    fn fixture() -> Vec<u8> {
        let manifest =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/echo/Cargo.toml");
        let status = Command::new("cargo")
            .args([
                "build",
                "--manifest-path",
                manifest.to_str().unwrap(),
                "--target",
                "wasm32-wasip2",
                "--offline",
            ])
            .status()
            .expect("build conformance component");
        assert!(status.success());
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("target/wasm32-wasip2/debug/artist_component_echo.wasm");
        std::fs::read(root).expect("read conformance component")
    }

    #[test]
    fn loads_describes_and_invokes_conformance_component() {
        let host = ComponentHost::new(&fixture()).unwrap();
        let info = host.validate().unwrap();
        assert_eq!(info.name, "conformance-echo");
        let response = host
            .invoke(Invocation {
                id: "1".to_owned(),
                operation: "echo".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{\"ok\":true}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-1".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-1".to_owned(),
                },
            })
            .unwrap();
        assert_eq!(response.output, "{\"ok\":true}");
    }

    #[test]
    fn preserves_declared_component_errors() {
        let host = ComponentHost::new(&fixture()).unwrap();
        let error = host
            .invoke(Invocation {
                id: "2".to_owned(),
                operation: "error".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-2".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-2".to_owned(),
                },
            })
            .unwrap_err();
        assert!(matches!(
            error,
            ComponentError::Abi(Error {
                kind: ErrorKind::InvalidRequest,
                ..
            })
        ));
    }

    #[test]
    fn rejects_incompatible_abi_and_cancelled_invocations() {
        let info = ComponentInfo {
            name: "bad".to_owned(),
            version: "0.1.0".to_owned(),
            abi_version: "9.0".to_owned(),
            interfaces: Vec::new(),
            required_capabilities: Vec::new(),
        };
        assert!(matches!(
            validate_component_info(&info),
            Err(ComponentError::Abi(Error {
                kind: ErrorKind::Incompatible,
                ..
            }))
        ));

        let host = ComponentHost::new(&fixture()).unwrap();
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let error = host.invoke_with_cancellation(
            Invocation {
                id: "3".to_owned(),
                operation: "echo".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-3".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-3".to_owned(),
                },
            },
            &cancellation,
        );
        assert!(matches!(error, Err(ComponentError::Cancelled)));
    }

    #[test]
    fn rejects_malformed_structured_response() {
        let host = ComponentHost::new(&fixture()).unwrap();
        let error = host
            .invoke(Invocation {
                id: "4".to_owned(),
                operation: "malformed".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-4".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-4".to_owned(),
                },
            })
            .unwrap_err();
        assert!(matches!(
            error,
            ComponentError::MalformedResponse { field: "output" }
        ));
    }

    #[test]
    fn exercises_the_component_to_kernel_host_boundary() {
        let host = ComponentHost::new(&fixture()).unwrap();
        let error = host
            .invoke(Invocation {
                id: "5".to_owned(),
                operation: "host".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-5".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-5".to_owned(),
                },
            })
            .unwrap_err();
        assert!(matches!(
            error,
            ComponentError::Abi(Error {
                kind: ErrorKind::HostFailure,
                ..
            })
        ));

        let host =
            ComponentHost::new_with_capabilities(&fixture(), ["resource.read".to_owned()]).unwrap();
        let response = host
            .invoke(Invocation {
                id: "6".to_owned(),
                operation: "host".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-6".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-6".to_owned(),
                },
            })
            .unwrap();
        assert_eq!(response.output, "{}");
    }

    #[test]
    fn handles_are_host_issued_and_released_explicitly() {
        let host = ComponentHost::new(&fixture()).unwrap();
        let response = host
            .invoke(Invocation {
                id: "7".to_owned(),
                operation: "handle".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{\"handle\":true}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-7".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-7".to_owned(),
                },
            })
            .unwrap();
        assert_eq!(response.output, "{\"handle\":true}");
    }

    #[test]
    fn executes_real_kernel_operations_through_a_component() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        let kernel = artist_kernel::Kernel::new();
        runtime.block_on(kernel.register(artist_kernel::FileHandler::new(root.path()).unwrap()));
        runtime
            .block_on(kernel.register_typed(artist_kernel::FileHandler::new(root.path()).unwrap()));
        let host = ComponentHost::new_with_capabilities(
            &fixture(),
            ["resource.write".to_owned(), "resource.read".to_owned()],
        )
        .unwrap();
        let target = root
            .path()
            .join("component-bridge.txt")
            .display()
            .to_string();
        let write = host
            .invoke_with_kernel(
                Invocation {
                    id: "bridge-write".to_owned(),
                    operation: "tool.write".to_owned(),
                    target: target.to_owned(),
                    input: "{\"value\":\"hello\"}".to_owned(),
                    context: Context {
                        cancellation_token: String::new(),
                        deadline_ms: None,
                        correlation_id: "bridge".to_owned(),
                    },
                },
                kernel.handle(),
            )
            .unwrap();
        assert!(write.output.contains("written"));
        let read = host
            .invoke_with_kernel(
                Invocation {
                    id: "bridge-read".to_owned(),
                    operation: "tool.read".to_owned(),
                    target: target.to_owned(),
                    input: "{}".to_owned(),
                    context: Context {
                        cancellation_token: String::new(),
                        deadline_ms: None,
                        correlation_id: "bridge".to_owned(),
                    },
                },
                kernel.handle(),
            )
            .unwrap();
        assert!(read.output.contains("hello"));
    }

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
                vec![tool_bindings::artist::tool::types::ReadRequest {
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
                                    uri: request.uri.clone(),
                                    changed: Vec::new(),
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
                "[{\"uri\":\"typed.txt\",\"args\":[],\"working_uri\":null,\"environment\":[]}]",
            ),
            (
                contracts::Verb::Send,
                "[{\"uri\":\"typed.txt\",\"content\":\"hello\"}]",
            ),
            (contracts::Verb::Abort, "[\"missing-abort\"]"),
            (contracts::Verb::Delete, "[\"missing-delete\"]"),
            (
                contracts::Verb::Poll,
                "{\"targets\":[{\"uri\":\"typed.txt\",\"from_position\":null}],\"until\":null,\"before\":null,\"after\":null}",
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
            "artist:tool/host-read@0.1.0".to_owned(),
            "artist:tool/extra@0.1.0".to_owned(),
        ];
        assert!(validate_typed_contract_names(read, &exports, &imports).is_err());
        assert!(
            validate_typed_contract_names(
                read,
                &["artist:tool/read@0.1.0".to_owned()],
                &imports[..2]
            )
            .is_ok()
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
    fn streams_chunks_and_async_invokes() {
        let host = ComponentHost::new(&fixture()).unwrap();
        let chunks = host
            .invoke_stream(Invocation {
                id: "8".to_owned(),
                operation: "stream".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{\"stream\":true}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-8".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-8".to_owned(),
                },
            })
            .unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(chunks[1].final_);

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let response = runtime
            .block_on(host.invoke_async(Invocation {
                id: "9".to_owned(),
                operation: "echo".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-9".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-9".to_owned(),
                },
            }))
            .unwrap();
        assert_eq!(response.output, "{}");

        let authorized_host =
            ComponentHost::new_with_capabilities(&fixture(), ["resource.read".to_owned()]).unwrap();
        let response = runtime
            .block_on(authorized_host.invoke_async(Invocation {
                id: "10".to_owned(),
                operation: "host".to_owned(),
                target: "repo://project/example".to_owned(),
                input: "{}".to_owned(),
                context: Context {
                    cancellation_token: "cancel-10".to_owned(),
                    deadline_ms: None,
                    correlation_id: "test-10".to_owned(),
                },
            }))
            .unwrap();
        assert_eq!(response.output, "{}");
    }

    #[test]
    fn discovers_tool_markdown_packages() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tool.md"),
            "---\nname: echo\ndescription: Echo input\nversion: 0.1.0\ncapabilities:\n  - resource.read\n---\n\nEcho prose.\n",
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
        assert!(package.contract.is_none());
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
    fn builds_validates_and_reuses_a_source_package() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/echo");
        let package = package::ToolPackage::discover(&root).unwrap();
        let options = package::BuildOptions {
            force: true,
            ..Default::default()
        };
        let first = package.build(&options).unwrap();
        assert!(!first.cached);
        assert!(first.artifact.is_file());
        assert!(first.provenance.is_file());

        let cached = package
            .build(&package::BuildOptions {
                force: false,
                ..options
            })
            .unwrap();
        assert!(cached.cached);
        assert_eq!(cached.fingerprint, first.fingerprint);

        let provenance: package::BuildProvenance =
            serde_json::from_slice(&std::fs::read(first.provenance).unwrap()).unwrap();
        assert_eq!(provenance.abi_version, ABI_VERSION);
        assert_eq!(provenance.target, "wasm32-wasip2");
    }

    #[test]
    fn rejects_malformed_component_bytes() {
        assert!(matches!(
            ComponentHost::new(b"not a component"),
            Err(ComponentError::Load(_))
        ));
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

    #[test]
    fn registry_swaps_versions_and_pins_existing_invocations() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/echo");
        let package = package::ToolPackage::discover(&root).unwrap();
        let registry = runtime::ComponentRegistry::new();
        let first = registry
            .reload(
                &package,
                &package::BuildOptions {
                    force: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let pinned = first.clone();
        let second = registry
            .reload(&package, &package::BuildOptions::default())
            .unwrap();

        assert_eq!(first.generation(), 1);
        assert_eq!(pinned.generation(), 1);
        assert_eq!(second.generation(), 2);
        assert_eq!(
            registry.current("conformance-echo").unwrap().generation(),
            2
        );

        let request = |id: &str| Invocation {
            id: id.to_owned(),
            operation: "echo".to_owned(),
            target: "repo://project/example".to_owned(),
            input: "{}".to_owned(),
            context: Context {
                cancellation_token: format!("cancel-{id}"),
                deadline_ms: None,
                correlation_id: format!("correlation-{id}"),
            },
        };
        assert_eq!(pinned.invoke(request("old")).unwrap().output, "{}");
        let result = registry.invoke("conformance-echo", request("new")).unwrap();
        assert_eq!(result.generation, 2);
        assert_eq!(result.response.output, "{}");
    }

    #[test]
    fn failed_reload_preserves_the_active_version() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/echo");
        let package = package::ToolPackage::discover(&root).unwrap();
        let registry = runtime::ComponentRegistry::new();
        let active = registry
            .reload(&package, &package::BuildOptions::default())
            .unwrap();

        let broken = tempfile::tempdir().unwrap();
        std::fs::write(
            broken.path().join("tool.md"),
            "---\nname: conformance-echo\ndescription: broken\nversion: 0.1.0\n---\n",
        )
        .unwrap();
        std::fs::create_dir(broken.path().join("src")).unwrap();
        std::fs::write(broken.path().join("src/lib.rs"), "this is not Rust\n").unwrap();
        std::fs::write(broken.path().join("Cargo.toml"), "not valid cargo\n").unwrap();
        let broken = package::ToolPackage::discover(broken.path()).unwrap();

        assert!(
            registry
                .reload(&broken, &package::BuildOptions::default())
                .is_err()
        );
        let still_active = registry.current("conformance-echo").unwrap();
        assert_eq!(still_active.generation(), active.generation());
        assert_eq!(still_active.fingerprint(), active.fingerprint());
    }
}
