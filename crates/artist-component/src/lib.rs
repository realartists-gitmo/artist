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

macro_rules! typed_host_impl_dispatch {
    ($module:ident, $host:ident, $verb:literal, $method:ident, $request:ty, $response:ty, $target:expr, $args:expr) => {
        impl $module::artist::tool::$host::Host for HostState {
            fn $method(
                &mut self,
                requests: Vec<$request>,
            ) -> Vec<Result<$response, $module::artist::tool::types::Error>> {
                requests
                    .into_iter()
                    .map(|request| {
                        let target = ($target)(&request);
                        let args = ($args)(&request);
                        typed_host_invoke(self, $verb, target, args)
                    })
                    .collect()
            }
        }
        impl $module::artist::tool::types::Host for HostState {}
    };
}

typed_host_impl_dispatch!(
    read_bindings,
    host_read,
    "read",
    read,
    read_bindings::artist::tool::types::ReadRequest,
    read_bindings::artist::tool::types::ReadResult,
    |request: &read_bindings::artist::tool::types::ReadRequest| request.uri.clone(),
    |_request: &read_bindings::artist::tool::types::ReadRequest| serde_json::Value::Null
);
typed_host_impl_dispatch!(
    write_bindings,
    host_write,
    "write",
    write,
    write_bindings::artist::tool::types::WriteRequest,
    write_bindings::artist::tool::types::WriteResult,
    |request: &write_bindings::artist::tool::types::WriteRequest| request.uri.clone(),
    |request: &write_bindings::artist::tool::types::WriteRequest| serde_json::json!({"value": request.content})
);
typed_host_impl_dispatch!(
    edit_bindings,
    host_edit,
    "edit",
    edit,
    edit_bindings::artist::tool::types::EditRequest,
    edit_bindings::artist::tool::types::EditResult,
    |request: &edit_bindings::artist::tool::types::EditRequest| request.uri.clone(),
    |request: &edit_bindings::artist::tool::types::EditRequest| serde_json::to_value(
        &request.operations
    )
    .unwrap_or_default()
);
typed_host_impl_dispatch!(
    run_bindings,
    host_run,
    "run",
    run,
    run_bindings::artist::tool::types::RunRequest,
    String,
    |request: &run_bindings::artist::tool::types::RunRequest| request.uri.clone(),
    |request: &run_bindings::artist::tool::types::RunRequest| serde_json::json!({"args": request.args})
);
typed_host_impl_dispatch!(
    send_bindings,
    host_send,
    "send",
    send,
    send_bindings::artist::tool::types::SendRequest,
    String,
    |request: &send_bindings::artist::tool::types::SendRequest| request.uri.clone(),
    |request: &send_bindings::artist::tool::types::SendRequest| serde_json::json!({"content": request.content})
);

macro_rules! typed_host_single {
    ($module:ident, $host:ident, $verb:literal, $method:ident, $request:ty, $response:ty, $target:expr, $args:expr) => {
        impl $module::artist::tool::$host::Host for HostState {
            fn $method(
                &mut self,
                request: $request,
            ) -> Result<$response, $module::artist::tool::types::Error> {
                typed_host_invoke(self, $verb, ($target)(&request), ($args)(&request))
            }
        }
        impl $module::artist::tool::types::Host for HostState {}
    };
}

typed_host_single!(
    find_bindings,
    host_find,
    "find",
    find,
    find_bindings::artist::tool::types::FindRequest,
    Vec<String>,
    |request: &find_bindings::artist::tool::types::FindRequest| request
        .roots
        .first()
        .cloned()
        .unwrap_or_default(),
    |request: &find_bindings::artist::tool::types::FindRequest| serde_json::json!({"query": request.query})
);
typed_host_single!(
    grep_bindings,
    host_grep,
    "grep",
    grep,
    grep_bindings::artist::tool::types::GrepRequest,
    Vec<grep_bindings::artist::tool::types::AnchoredText>,
    |_request: &grep_bindings::artist::tool::types::GrepRequest| String::new(),
    |request: &grep_bindings::artist::tool::types::GrepRequest| serde_json::json!({"pattern": request.pattern})
);
typed_host_single!(
    poll_bindings,
    host_poll,
    "poll",
    poll,
    poll_bindings::artist::tool::types::PollRequest,
    poll_bindings::artist::tool::types::PollResult,
    |request: &poll_bindings::artist::tool::types::PollRequest| request
        .targets
        .first()
        .map(|target| target.uri.clone())
        .unwrap_or_default(),
    |request: &poll_bindings::artist::tool::types::PollRequest| serde_json::to_value(request)
        .unwrap_or_default()
);

impl abort_bindings::artist::tool::host_abort::Host for HostState {
    fn abort(
        &mut self,
        uris: Vec<String>,
    ) -> Vec<Result<String, abort_bindings::artist::tool::types::Error>> {
        uris.into_iter()
            .map(|uri| typed_host_invoke(self, "abort", uri.clone(), serde_json::Value::Null))
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
            .map(|uri| typed_host_invoke(self, "delete", uri.clone(), serde_json::Value::Null))
            .collect()
    }
}
impl delete_bindings::artist::tool::types::Host for HostState {}

fn typed_host_invoke<Response, ErrorType>(
    state: &mut HostState,
    verb: &str,
    target: String,
    args: serde_json::Value,
) -> Result<Response, ErrorType>
where
    Response: serde::de::DeserializeOwned,
    ErrorType: serde::de::DeserializeOwned,
{
    let capability = format!("resource.{verb}");
    let result = if !state.capabilities.contains(&capability) {
        Err(
            serde_json::json!({"code":"PermissionDenied","uri":target,"message":format!("capability denied: {capability}")}),
        )
    } else if let Some(kernel) = state.kernel.clone() {
        execute_typed_kernel_value(kernel, verb, target, args).map_err(|error| {
            serde_json::json!({
                "code": error.code,
                "uri": error.uri,
                "message": error.message,
            })
        })
    } else {
        Err(
            serde_json::json!({"code":"Internal","uri":target,"message":"typed tool host has no kernel"}),
        )
    };
    result
        .map_err(|error| serde_json::from_value(error).ok().unwrap())
        .and_then(|value| serde_json::from_value(value).map_err(|_| serde_json::from_value(serde_json::json!({"code":"Internal","uri":null,"message":"typed host response conversion failed"})).ok().unwrap()))
}

fn typed_error(
    code: tool_bindings::artist::tool::types::ErrorCode,
    message: String,
    uri: Option<String>,
) -> tool_bindings::artist::tool::types::Error {
    tool_bindings::artist::tool::types::Error { code, uri, message }
}

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
        KernelError::InvalidAnchor { message } => (
            tool_bindings::artist::tool::types::ErrorCode::InvalidAnchor,
            target,
            message,
        ),
        KernelError::NotFound { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::NotFound,
            Some(uri),
            "resource was not found".to_owned(),
        ),
        KernelError::AlreadyExists { uri } => (
            tool_bindings::artist::tool::types::ErrorCode::Conflict,
            Some(uri),
            "resource already exists".to_owned(),
        ),
        KernelError::InvalidState { message } | KernelError::Handler { message } => (
            tool_bindings::artist::tool::types::ErrorCode::Internal,
            target,
            message,
        ),
    };
    typed_error(code, message, uri)
}

fn execute_typed_kernel_value(
    kernel: KernelHandle,
    verb_name: &str,
    target: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, tool_bindings::artist::tool::types::Error> {
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
            return Err(typed_error(
                tool_bindings::artist::tool::types::ErrorCode::InvalidInput,
                format!("unknown universal verb: {verb_name}"),
                Some(target),
            ));
        }
    };
    let target_address = if target.contains("://") {
        artist_kernel::ResourceUri::parse(&target)
            .map(artist_kernel::ResourceAddress::uri)
            .map_err(|error| {
                typed_error(
                    tool_bindings::artist::tool::types::ErrorCode::InvalidUri,
                    error.to_string(),
                    Some(target.clone()),
                )
            })?
    } else {
        artist_kernel::ResourceAddress::path(target.clone())
    };
    let result = std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
        Ok::<_, String>(runtime.block_on(kernel.execute(KernelRequest::new(
            verb,
            target_address,
            args,
        ))))
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
    if !result.ok {
        return Err(kernel_error_to_typed(
            result
                .error
                .unwrap_or_else(|| artist_kernel::KernelError::InvalidState {
                    message: "tool operation failed without an error".to_owned(),
                }),
            Some(target),
        ));
    }
    let output = typed_kernel_output_value(
        verb_name,
        &target,
        result.value.unwrap_or(serde_json::Value::Null),
    )
    .map_err(|message| {
        typed_error(
            tool_bindings::artist::tool::types::ErrorCode::Internal,
            message,
            Some(target),
        )
    })?;
    Ok(output)
}

fn typed_kernel_output_value(
    verb: &str,
    target: &str,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    match verb {
        "read" => {
            let content = value
                .get("value")
                .and_then(|value| value.get("content"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Ok(serde_json::json!({
                "Text": {"uri": target, "lines": content.lines().enumerate().map(|(i, line)| serde_json::json!({"anchor": format!("line:{i}"), "text": line, "ending": "Lf"})).collect::<Vec<_>>()}
            }))
        }
        "write" => Ok(serde_json::json!({
            "Text": {"uri": target, "lines": []}
        })),
        "find" => Ok(value
            .get("paths")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new()))),
        "grep" => {
            let paths = value
                .get("paths")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            Ok(serde_json::json!(
                paths
                    .into_iter()
                    .map(|uri| serde_json::json!({"uri": uri, "lines": []}))
                    .collect::<Vec<_>>()
            ))
        }
        "run" | "send" | "abort" | "delete" => Ok(serde_json::json!([target])),
        "edit" => Ok(serde_json::json!({
            "uri": target, "changed": [], "diff": {"uri": target, "hunks": []}
        })),
        "poll" => Ok(serde_json::json!({"text": [], "satisfied": []})),
        _ => Ok(value),
    }
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
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config)
            .map_err(|e| ComponentError::Load(anyhow::anyhow!(e.to_string())))?;
        let component = wasmtime::component::Component::from_binary(&engine, bytes)
            .map_err(|e| ComponentError::Load(anyhow::anyhow!(format!("{e:#}"))))?;
        Ok(Self {
            engine,
            component,
            capabilities: capabilities.into_iter().collect(),
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
        let mut store = wasmtime::Store::new(
            &self.engine,
            HostState::with_kernel(self.capabilities.iter().cloned(), kernel),
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
        if package.contract.is_some() {
            let contract = package.contract.as_ref().unwrap();
            if let Some(verb) = contract.verb {
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
            } else {
                ComponentHost::new_with_capabilities(&bytes, options.granted_capabilities.clone())
                    .map_err(|error| ComponentError::Build {
                        diagnostics: format!("extension component validation failed: {error}"),
                    })?;
            }
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
        info: ComponentInfo,
        host: Option<Arc<ComponentHost>>,
        typed_host: Option<Arc<TypedComponentHost>>,
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
            if let Some(host) = &self.typed_host {
                host.invoke_json(verb, input, kernel)
            } else {
                let response = self.invoke_with_kernel(
                    Invocation {
                        id: format!("tool:{verb}"),
                        operation: format!("tool.{verb}"),
                        target: String::new(),
                        input: input.to_owned(),
                        context: super::Context {
                            cancellation_token: String::new(),
                            deadline_ms: None,
                            correlation_id: String::new(),
                        },
                    },
                    kernel,
                )?;
                Ok(response.output)
            }
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
                info: self.info.clone(),
                host: self.host.as_ref().map(Arc::clone),
                typed_host: self.typed_host.as_ref().map(Arc::clone),
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
            let build = package.build(options)?;
            let bytes = fs::read(&build.artifact).map_err(|error| {
                ComponentError::Load(anyhow::anyhow!(
                    "could not read built artifact {}: {error}",
                    build.artifact.display()
                ))
            })?;
            let (host, typed_host, info) = if package
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
                (None, Some(typed_host), info)
            } else {
                let host = Arc::new(ComponentHost::new_with_capabilities(
                    &bytes,
                    options.granted_capabilities.clone(),
                )?);
                let info = host.info()?;
                (Some(host), None, info)
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
                info,
                host,
                typed_host,
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
    use artist_kernel::{ItemResult, KernelError, KernelHandle, ResourceAddress};
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

        pub fn invoke(
            &self,
            target: ResourceAddress,
            args: Value,
            kernel: KernelHandle,
        ) -> Result<Value, KernelError> {
            let request = match self.verb {
                Verb::Read => {
                    serde_json::json!({"uri": target.to_string(), "at": null, "before": null, "after": null})
                }
                Verb::Write => {
                    let content = args.get("value").cloned().unwrap_or_else(|| args.clone());
                    serde_json::json!({"uri": target.to_string(), "content": content})
                }
                Verb::Edit => serde_json::json!({"uri": target.to_string(), "operations": []}),
                Verb::Run | Verb::Send => args,
                Verb::Find => args,
                Verb::Grep => args,
                Verb::Poll => args,
                Verb::Abort | Verb::Delete => serde_json::Value::Null,
            };
            let input_value = match self.verb {
                Verb::Read | Verb::Write | Verb::Edit | Verb::Run | Verb::Send => {
                    serde_json::json!([request])
                }
                Verb::Abort | Verb::Delete => serde_json::json!([target.to_string()]),
                _ => request,
            };
            let input =
                serde_json::to_string(&input_value).map_err(|error| KernelError::Handler {
                    message: format!("could not encode component input: {error}"),
                })?;
            let output = self
                .component
                .invoke_tool_json(self.verb, &input, kernel)
                .map_err(component_error)?;
            serde_json::from_str(&output).map_err(|error| KernelError::Handler {
                message: format!("component returned invalid output JSON: {error}"),
            })
        }

        pub async fn invoke_async(
            &self,
            target: ResourceAddress,
            args: Value,
            kernel: KernelHandle,
        ) -> Result<Value, KernelError> {
            let this = self.clone();
            tokio::task::spawn_blocking(move || this.invoke(target, args, kernel))
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
        BoxFuture, FileHandler, Handler, HandlerDescriptor, KernelError, KernelHandle, Request,
        ResourceAddress, ResourceUri, ToolDefinition, ToolProvider, Verb as KernelVerb,
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
                    serde_json::json!({
                        "type": "object",
                        "additionalProperties": true
                    })
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
            let active = self
                .registry
                .reload(&package, &self.options)
                .map_err(component_error)?;
            self.dirty.lock().unwrap().remove(&key);
            Ok(active)
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
                let target = request
                    .args
                    .get("target")
                    .and_then(Value::as_str)
                    .ok_or_else(|| KernelError::InvalidRequest {
                        message: format!("invoking {} requires args.target", request.verb),
                    })?;
                let target = if target.contains("://") {
                    ResourceUri::parse(target).map(ResourceAddress::uri)?
                } else {
                    ResourceAddress::path(target)
                };
                return ComponentTool::new(active, contract_verb)
                    .invoke_async(target, request.args.clone(), host)
                    .await;
            }

            {
                if request.verb == KernelVerb::Write {
                    self.prepare_write(&relative)?;
                }
                let mut mapped = request.clone();
                mapped.target = ResourceAddress::path(&relative);
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
                let target = args.get("target").and_then(Value::as_str).ok_or_else(|| {
                    KernelError::InvalidRequest {
                        message: format!("invoking {name} requires a target argument"),
                    }
                })?;
                let target = if target.contains("://") {
                    ResourceUri::parse(target).map(ResourceAddress::uri)?
                } else {
                    ResourceAddress::path(target)
                };
                return ComponentTool::new(active, verb).invoke(target, args, host);
            }

            let response = active
                .invoke_with_kernel(
                    crate::Invocation {
                        id: format!("tool:{name}"),
                        operation: registration.contract.to_string(),
                        target: args
                            .get("target")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        input: serde_json::to_string(&args).map_err(|error| {
                            KernelError::Handler {
                                message: format!("could not encode tool input: {error}"),
                            }
                        })?,
                        context: crate::Context {
                            cancellation_token: String::new(),
                            deadline_ms: None,
                            correlation_id: String::new(),
                        },
                    },
                    host,
                )
                .map_err(component_error)?;
            serde_json::from_str(&response.output).map_err(|error| KernelError::Handler {
                message: format!("component returned invalid output JSON: {error}"),
            })
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
        let host = ComponentHost::new_with_capabilities(
            &fixture(),
            ["resource.write".to_owned(), "resource.read".to_owned()],
        )
        .unwrap();
        let target = "component-bridge.txt";
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
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let kernel = artist_kernel::Kernel::new();
        runtime.block_on(kernel.register(artist_kernel::FileHandler::new(root.path()).unwrap()));
        let result = host
            .invoke_read(
                vec![tool_bindings::artist::tool::types::ReadRequest {
                    uri: "typed.txt".to_owned(),
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
                r#"[{"uri":"typed.txt","at":null,"before":null,"after":null}]"#,
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

    #[test]
    fn exercises_all_typed_verb_components_without_memory_backend() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let kernel = artist_kernel::Kernel::new();
        runtime.block_on(kernel.register(MockConformanceHandler));

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
                "[{\"uri\":\"typed.txt\",\"args\":[],\"context\":{\"working_uri\":null,\"environment\":[],\"cancellation_token\":null,\"deadline_ms\":null,\"correlation_id\":null}}]",
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
