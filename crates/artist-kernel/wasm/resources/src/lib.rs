//! Host-side adapter for component-owned URI resources.

use std::sync::Arc;

use artist_kernel::provider::{ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode};
use artist_kernel::{Kernel, NodeKind, ResourceUri};
use artist_wasm::{GenerationHandle, RuntimeStore};
use async_trait::async_trait;
use wasmtime::component::Linker;

pub mod bindings;

pub use bindings::artist::resources::types;
pub use bindings::exports::artist::resources::provider::Guest as ResourceGuest;

#[async_trait]
pub trait ResourceGuestAdapter: Send + Sync {
    async fn matches(&self, uri: &str) -> bool;
    async fn attrs(&self, uri: &str) -> Result<types::Attrs, types::Error>;
    async fn readdir(&self, uri: &str) -> Result<Vec<types::Entry>, types::Error>;
    async fn read(&self, uri: &str, offset: u64, size: u32) -> Result<Vec<u8>, types::Error>;
    async fn write(&self, uri: &str, offset: u64, data: Vec<u8>) -> Result<u32, types::Error>;
    async fn move_resource(&self, source: &str, destination: &str) -> Result<(), types::Error>;
    async fn delete(&self, uri: &str) -> Result<(), types::Error>;
}

/// Concrete adapter for a live resource-provider component.
///
/// The generation is pinned for each operation and a fresh component instance
/// is created for that operation. A replacement can therefore retire the old
/// generation without invalidating an in-flight request.
pub struct WasmResource {
    generation: GenerationHandle,
}

impl WasmResource {
    pub fn new(generation: GenerationHandle) -> Self {
        Self { generation }
    }

    async fn call<R, F>(&self, f: F) -> anyhow::Result<R>
    where
        F: for<'a> FnOnce(
                &'a bindings::exports::artist::resources::provider::Guest,
                &'a mut wasmtime::Store<RuntimeStore>,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = wasmtime::Result<R>> + Send + 'a>,
            > + Send,
        R: Send + 'static,
    {
        let lease = self
            .generation
            .pin()
            .ok_or_else(|| anyhow::anyhow!("resource generation is retiring"))?;
        let mut store = lease.store()?;
        let linker = Linker::new(lease.component().engine());
        let pre = linker.instantiate_pre(lease.component())?;
        let indices = bindings::exports::artist::resources::provider::GuestIndices::new(&pre)?;
        let instance = pre.instantiate_async(&mut store).await?;
        let guest = indices.load(&mut store, &instance)?;
        Ok(f(&guest, &mut store).await?)
    }
}

#[async_trait]
impl ResourceGuestAdapter for WasmResource {
    async fn matches(&self, uri: &str) -> bool {
        let uri = uri.to_owned();
        self.call(move |guest, store| {
            Box::pin(async move { guest.call_matches(store, &uri).await })
        })
        .await
        .unwrap_or(false)
    }

    async fn attrs(&self, uri: &str) -> Result<types::Attrs, types::Error> {
        let uri = uri.to_owned();
        self.call(move |guest, store| {
            Box::pin(async move { guest.call_get_attrs(store, &uri).await })
        })
        .await
        .map_err(|_| types::Error::Io)?
    }

    async fn readdir(&self, uri: &str) -> Result<Vec<types::Entry>, types::Error> {
        let uri = uri.to_owned();
        self.call(move |guest, store| {
            Box::pin(async move { guest.call_readdir(store, &uri).await })
        })
        .await
        .map_err(|_| types::Error::Io)?
    }

    async fn read(&self, uri: &str, offset: u64, size: u32) -> Result<Vec<u8>, types::Error> {
        let uri = uri.to_owned();
        self.call(move |guest, store| {
            Box::pin(async move { guest.call_read(store, &uri, offset, size).await })
        })
        .await
        .map_err(|_| types::Error::Io)?
    }

    async fn write(&self, uri: &str, offset: u64, data: Vec<u8>) -> Result<u32, types::Error> {
        let uri = uri.to_owned();
        self.call(move |guest, store| {
            Box::pin(async move { guest.call_write(store, &uri, offset, &data).await })
        })
        .await
        .map_err(|_| types::Error::Io)?
    }

    async fn move_resource(&self, source: &str, destination: &str) -> Result<(), types::Error> {
        let source = source.to_owned();
        let destination = destination.to_owned();
        self.call(move |guest, store| {
            Box::pin(async move { guest.call_move_resource(store, &source, &destination).await })
        })
        .await
        .map_err(|_| types::Error::Io)?
    }

    async fn delete(&self, uri: &str) -> Result<(), types::Error> {
        let uri = uri.to_owned();
        self.call(move |guest, store| Box::pin(async move { guest.call_delete(store, &uri).await }))
            .await
            .map_err(|_| types::Error::Io)?
    }
}

/// Adapts any component-defined resource implementation to the kernel's
/// temporary host bridge. The namespace component remains responsible for
/// deciding which claims are valid and how overlaps compose.
pub struct RoutedResource {
    name: String,
    guest: Arc<dyn ResourceGuestAdapter>,
}

impl RoutedResource {
    pub fn new(name: impl Into<String>, guest: Arc<dyn ResourceGuestAdapter>) -> Self {
        Self {
            name: name.into(),
            guest,
        }
    }
}

fn resource_error(uri: &ResourceUri, error: types::Error) -> ResourceError {
    let code = match error {
        types::Error::InvalidAddress => ResourceErrorCode::InvalidAddress,
        types::Error::NotFound => ResourceErrorCode::NotFound,
        types::Error::NotDir => ResourceErrorCode::NotDir,
        types::Error::IsDir => ResourceErrorCode::IsDir,
        types::Error::Io => ResourceErrorCode::Io,
        types::Error::PermissionDenied => ResourceErrorCode::PermissionDenied,
        types::Error::Unsupported => ResourceErrorCode::Unsupported,
        types::Error::Conflict => ResourceErrorCode::Conflict,
    };
    ResourceError::new(code, format!("resource component failure for {uri}"))
}

fn provider_attrs(attrs: types::Attrs) -> ProviderAttrs {
    let mtime =
        std::time::UNIX_EPOCH + std::time::Duration::new(attrs.mtime_secs, attrs.mtime_nsecs);
    ProviderAttrs {
        kind: match attrs.kind {
            types::Kind::Directory => NodeKind::Directory,
            types::Kind::File => NodeKind::File,
        },
        size: attrs.size,
        perm: 0o444,
        nlink: 1,
        uid: 0,
        gid: 0,
        atime: mtime,
        mtime,
        ctime: mtime,
    }
}

#[async_trait]
impl artist_kernel::ResourceProvider for RoutedResource {
    fn provider_name(&self) -> &str {
        &self.name
    }
    fn eligible(&self, _uri: &ResourceUri) -> bool {
        true
    }
    async fn matches(&self, uri: &ResourceUri) -> bool {
        self.guest.matches(&uri.to_string()).await
    }
    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        self.guest
            .attrs(&uri.to_string())
            .await
            .map(provider_attrs)
            .map_err(|error| resource_error(uri, error))
    }
    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        self.guest
            .readdir(&uri.to_string())
            .await
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| ProviderEntry {
                        name: entry.name,
                        attrs: ProviderAttrs {
                            kind: match entry.kind {
                                types::Kind::Directory => NodeKind::Directory,
                                types::Kind::File => NodeKind::File,
                            },
                            size: entry.size,
                            perm: 0o444,
                            nlink: 1,
                            uid: 0,
                            gid: 0,
                            atime: std::time::SystemTime::UNIX_EPOCH,
                            mtime: std::time::SystemTime::UNIX_EPOCH,
                            ctime: std::time::SystemTime::UNIX_EPOCH,
                        },
                    })
                    .collect()
            })
            .map_err(|error| resource_error(uri, error))
    }
    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        self.guest
            .read(&uri.to_string(), offset, size)
            .await
            .map_err(|error| resource_error(uri, error))
    }
    async fn write(
        &self,
        uri: &ResourceUri,
        offset: u64,
        data: &[u8],
    ) -> Result<u32, ResourceError> {
        self.guest
            .write(&uri.to_string(), offset, data.to_vec())
            .await
            .map_err(|error| resource_error(uri, error))
    }
    async fn move_resource(
        &self,
        source: &ResourceUri,
        destination: &ResourceUri,
    ) -> Result<(), ResourceError> {
        self.guest
            .move_resource(&source.to_string(), &destination.to_string())
            .await
            .map_err(|error| resource_error(source, error))
    }
    async fn delete(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        self.guest
            .delete(&uri.to_string())
            .await
            .map_err(|error| resource_error(uri, error))
    }
}

pub struct KernelResourceHost {
    kernel: Arc<Kernel>,
}

impl KernelResourceHost {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

fn convert_attrs(attrs: ProviderAttrs) -> types::Attrs {
    let duration = attrs
        .mtime
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    types::Attrs {
        kind: match attrs.kind {
            NodeKind::Directory => types::Kind::Directory,
            NodeKind::File => types::Kind::File,
        },
        size: attrs.size,
        mtime_secs: duration.as_secs(),
        mtime_nsecs: duration.subsec_nanos(),
    }
}

fn convert_entry(entry: ProviderEntry) -> types::Entry {
    types::Entry {
        name: entry.name,
        kind: match entry.attrs.kind {
            NodeKind::Directory => types::Kind::Directory,
            NodeKind::File => types::Kind::File,
        },
        size: entry.attrs.size,
    }
}

fn convert_error(error: ResourceError) -> types::Error {
    match error.code {
        ResourceErrorCode::InvalidAddress => types::Error::InvalidAddress,
        ResourceErrorCode::NotFound => types::Error::NotFound,
        ResourceErrorCode::NotDir => types::Error::NotDir,
        ResourceErrorCode::IsDir => types::Error::IsDir,
        ResourceErrorCode::PermissionDenied => types::Error::PermissionDenied,
        ResourceErrorCode::Conflict => types::Error::Conflict,
        ResourceErrorCode::Io
        | ResourceErrorCode::Unavailable
        | ResourceErrorCode::Component
        | ResourceErrorCode::Cancelled
        | ResourceErrorCode::Timeout => types::Error::Io,
        ResourceErrorCode::Unsupported => types::Error::Unsupported,
    }
}

#[async_trait]
impl ResourceGuestAdapter for KernelResourceHost {
    async fn matches(&self, uri: &str) -> bool {
        uri.parse::<ResourceUri>().is_ok()
    }
    async fn attrs(&self, uri: &str) -> Result<types::Attrs, types::Error> {
        let uri = uri.parse().map_err(|_| types::Error::InvalidAddress)?;
        self.kernel
            .attrs_uri(&uri)
            .await
            .map(convert_attrs)
            .map_err(convert_error)
    }
    async fn readdir(&self, uri: &str) -> Result<Vec<types::Entry>, types::Error> {
        let uri = uri.parse().map_err(|_| types::Error::InvalidAddress)?;
        self.kernel
            .readdir_uri(&uri)
            .await
            .map(|entries| entries.into_iter().map(convert_entry).collect())
            .map_err(convert_error)
    }
    async fn read(&self, uri: &str, offset: u64, size: u32) -> Result<Vec<u8>, types::Error> {
        let uri = uri.parse().map_err(|_| types::Error::InvalidAddress)?;
        self.kernel
            .read_uri(&uri, offset, size)
            .await
            .map_err(convert_error)
    }
    async fn write(&self, uri: &str, offset: u64, data: Vec<u8>) -> Result<u32, types::Error> {
        let uri = uri.parse().map_err(|_| types::Error::InvalidAddress)?;
        self.kernel
            .write_uri(&uri, offset, &data)
            .await
            .map_err(convert_error)
    }
    async fn move_resource(&self, source: &str, destination: &str) -> Result<(), types::Error> {
        let source = source.parse().map_err(|_| types::Error::InvalidAddress)?;
        let destination = destination
            .parse()
            .map_err(|_| types::Error::InvalidAddress)?;
        self.kernel
            .move_uri(&source, &destination)
            .await
            .map_err(convert_error)
    }
    async fn delete(&self, uri: &str) -> Result<(), types::Error> {
        let uri = uri.parse().map_err(|_| types::Error::InvalidAddress)?;
        self.kernel.delete_uri(&uri).await.map_err(convert_error)
    }
}
