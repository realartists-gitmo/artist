//! Host adapter for the generic opaque tool component ABI.

use artist_wasm::GenerationHandle;
use wasmtime::component::Linker;

use crate::bindings::tool;
use crate::bindings::resource;
use artist_kernel::{ResourceErrorCode, ResourceUri};
use artist_wasm::{HostEnvironment, RuntimeStore};

#[derive(Clone)]
pub struct WasmTool {
    name: String,
    generation: GenerationHandle,
}

impl WasmTool {
    pub fn new(name: impl Into<String>, generation: GenerationHandle) -> Self {
        Self { name: name.into(), generation }
    }

    pub fn name(&self) -> &str { &self.name }

    pub async fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, ToolError> {
        let request_value: serde_json::Value = toon_format::decode_default(
            std::str::from_utf8(request).map_err(|_| ToolError::InvalidArgument)?,
        ).map_err(|_| ToolError::InvalidArgument)?;
        let envelope = serde_json::json!({ "tool": self.name, "request": request_value });
        let envelope = toon_format::encode_default(&envelope)
            .map_err(|_| ToolError::Internal)?;
        let lease = self.generation.pin().ok_or(ToolError::Unavailable)?;
        let mut store = lease.store().map_err(|_| ToolError::Internal)?;
        let mut linker = Linker::new(lease.component().engine());
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|_| ToolError::Internal)?;
        resource::add_to_linker::<_, ResourceHostMarker>(&mut linker, resource_context).map_err(|_| ToolError::Internal)?;
        let pre = linker.instantiate_pre(lease.component()).map_err(|_| ToolError::Internal)?;
        let indices = tool::GuestIndices::new(&pre).map_err(|_| ToolError::Internal)?;
        let instance = pre.instantiate_async(&mut store).await.map_err(|_| ToolError::Internal)?;
        let guest = indices.load(&mut store, &instance).map_err(|_| ToolError::Internal)?;
        guest.call_invoke(&mut store, envelope.as_bytes()).await.map_err(|_| ToolError::Internal)?.map_err(ToolError::from)
    }
}

struct ResourceHostMarker;

struct ResourceContext {
    host: std::sync::Arc<dyn HostEnvironment>,
}

impl wasmtime::component::HasData for ResourceHostMarker {
    type Data<'a> = ResourceContext;
}

fn resource_context(store: &mut RuntimeStore) -> ResourceContext {
    ResourceContext { host: std::sync::Arc::clone(&store.host) }
}

impl resource::Host for ResourceContext {
    async fn read(&mut self, uri: String, offset: u64, size: u32) -> wasmtime::Result<Result<Vec<u8>, resource::Error>> {
        let uri: ResourceUri = uri.parse().map_err(|_| resource::Error::InvalidArgument)?;
        Ok(self.host.read(&uri, offset, size).await.map_err(map_resource_error))
    }
    async fn write(&mut self, uri: String, offset: u64, data: Vec<u8>) -> wasmtime::Result<Result<u32, resource::Error>> {
        let uri: ResourceUri = uri.parse().map_err(|_| resource::Error::InvalidArgument)?;
        let kernel = self.host.kernel();
        let result = kernel.write_uri(&uri, offset, &data).await;
        if matches!(result, Err(ref error) if error.code == ResourceErrorCode::NotFound) && offset == 0 {
            kernel.create_file_uri(&uri).await.map_err(map_resource_error)?;
            return Ok(kernel.write_uri(&uri, offset, &data).await.map_err(map_resource_error));
        }
        Ok(result.map_err(map_resource_error))
    }
    async fn truncate(&mut self, uri: String, size: u64) -> wasmtime::Result<Result<(), resource::Error>> {
        let uri: ResourceUri = uri.parse().map_err(|_| resource::Error::InvalidArgument)?;
        Ok(self.host.kernel().set_size_uri(&uri, size).await.map_err(map_resource_error))
    }
    async fn readdir(&mut self, uri: String) -> wasmtime::Result<Result<Vec<resource::Entry>, resource::Error>> {
        let uri: ResourceUri = uri.parse().map_err(|_| resource::Error::InvalidArgument)?;
        let entries = self.host.kernel().readdir_uri(&uri).await.map_err(map_resource_error)?;
        Ok(Ok(entries.into_iter().map(|entry| resource::Entry {
            name: entry.name,
            kind: match entry.attrs.kind {
                artist_kernel::NodeKind::File => resource::Kind::File,
                artist_kernel::NodeKind::Directory => resource::Kind::Directory,
            },
            size: entry.attrs.size,
        }).collect()))
    }
    async fn move_resource(&mut self, source: String, destination: String) -> wasmtime::Result<Result<(), resource::Error>> {
        let source: ResourceUri = source.parse().map_err(|_| resource::Error::InvalidArgument)?;
        let destination: ResourceUri = destination.parse().map_err(|_| resource::Error::InvalidArgument)?;
        Ok(self.host.kernel().move_uri(&source, &destination).await.map_err(map_resource_error))
    }
    async fn delete(&mut self, uri: String) -> wasmtime::Result<Result<(), resource::Error>> {
        let uri: ResourceUri = uri.parse().map_err(|_| resource::Error::InvalidArgument)?;
        Ok(self.host.kernel().delete_uri(&uri).await.map_err(map_resource_error))
    }
}

fn map_resource_error(error: artist_kernel::ResourceError) -> resource::Error {
    match error.code {
        ResourceErrorCode::InvalidAddress => resource::Error::InvalidArgument,
        ResourceErrorCode::NotFound => resource::Error::NotFound,
        ResourceErrorCode::PermissionDenied => resource::Error::PermissionDenied,
        ResourceErrorCode::Unsupported => resource::Error::Unsupported,
        ResourceErrorCode::Conflict => resource::Error::Conflict,
        _ => resource::Error::Io,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolError {
    InvalidArgument,
    NotFound,
    Unsupported,
    PermissionDenied,
    Conflict,
    Aborted,
    Internal,
    Unavailable,
}

impl From<tool::Error> for ToolError {
    fn from(error: tool::Error) -> Self {
        match error {
            tool::Error::InvalidArgument => Self::InvalidArgument,
            tool::Error::NotFound => Self::NotFound,
            tool::Error::Unsupported => Self::Unsupported,
            tool::Error::PermissionDenied => Self::PermissionDenied,
            tool::Error::Conflict => Self::Conflict,
            tool::Error::Aborted => Self::Aborted,
            tool::Error::Internal => Self::Internal,
        }
    }
}
