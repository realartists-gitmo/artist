//! Host adapter for the URI-aware noun provider contract.
//!
//! A noun is not mounted at a URI prefix. The guest receives the complete
//! canonical URI and decides whether it handles that URI. This permits routes
//! such as `file:///nuke.rs/symbols`, derived resources, decorators, and
//! overlays.

use std::sync::Arc;
use std::time::UNIX_EPOCH;

use artist_kernel::provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
use artist_kernel::uri::ResourceUri;
use artist_kernel::{Kernel, NodeKind};
use artist_wasm::{GenerationHandle, RuntimeStore};
use async_trait::async_trait;
use wasmtime::component::{HasData, Linker};

use crate::bindings::{resource_host, types};

/// The guest-side noun behavior. A real component adapter implements this
/// trait around the generated Wasmtime bindings; tests and host-native nouns
/// can implement it directly.
#[async_trait]
pub trait RoutedNounGuest: Send + Sync {
    async fn matches(&self, uri: &str) -> bool;
    async fn getattr(&self, uri: &str) -> Result<types::Attrs, types::Error>;
    async fn readdir(&self, uri: &str) -> Result<Vec<types::Entry>, types::Error>;
    async fn read(&self, uri: &str, offset: u64, size: u32) -> Result<Vec<u8>, types::Error>;
}

/// Host-side resource capability exposed to a noun so it can derive or
/// decorate another provider's resource.
#[async_trait]
pub trait ResourceHost: Send + Sync {
    async fn get_attrs(&self, uri: &str) -> Result<types::Attrs, types::Error>;
    async fn readdir(&self, uri: &str) -> Result<Vec<types::Entry>, types::Error>;
    async fn read(&self, uri: &str, offset: u64, size: u32) -> Result<Vec<u8>, types::Error>;
}

/// The standard host implementation. It delegates through the kernel's URI
/// router, so a noun can derive from any currently visible resource without
/// learning which provider supplies that resource.
pub struct KernelResourceHost {
    kernel: Arc<Kernel>,
}

impl KernelResourceHost {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

fn system_time_parts(time: std::time::SystemTime) -> (u64, u32) {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or((0, 0))
}

fn kernel_error(error: ResourceError) -> types::Error {
    match error.code {
        ResourceErrorCode::NotFound => types::Error::NotFound,
        ResourceErrorCode::NotDir => types::Error::NotDir,
        ResourceErrorCode::IsDir => types::Error::IsDir,
        ResourceErrorCode::PermissionDenied => types::Error::PermissionDenied,
        ResourceErrorCode::Unsupported => types::Error::Unsupported,
        ResourceErrorCode::InvalidAddress
        | ResourceErrorCode::Unavailable
        | ResourceErrorCode::Io
        | ResourceErrorCode::Component
        | ResourceErrorCode::Cancelled
        | ResourceErrorCode::Timeout
        | ResourceErrorCode::Conflict => types::Error::Io,
    }
}

fn kernel_attrs(attrs: ProviderAttrs) -> types::Attrs {
    let (mtime_secs, mtime_nsecs) = system_time_parts(attrs.mtime);
    types::Attrs {
        kind: match attrs.kind {
            NodeKind::Directory => types::Kind::Directory,
            NodeKind::File => types::Kind::File,
        },
        size: attrs.size,
        mtime_secs,
        mtime_nsecs,
    }
}

fn kernel_entry(entry: ProviderEntry) -> types::Entry {
    types::Entry {
        name: entry.name,
        kind: match entry.attrs.kind {
            NodeKind::Directory => types::Kind::Directory,
            NodeKind::File => types::Kind::File,
        },
        size: entry.attrs.size,
    }
}

#[async_trait]
impl ResourceHost for KernelResourceHost {
    async fn get_attrs(&self, uri: &str) -> Result<types::Attrs, types::Error> {
        let uri: ResourceUri = uri.parse().map_err(|_| types::Error::Io)?;
        self.kernel
            .attrs_uri(&uri)
            .await
            .map(kernel_attrs)
            .map_err(kernel_error)
    }

    async fn readdir(&self, uri: &str) -> Result<Vec<types::Entry>, types::Error> {
        let uri: ResourceUri = uri.parse().map_err(|_| types::Error::Io)?;
        self.kernel
            .readdir_uri(&uri)
            .await
            .map(|entries| entries.into_iter().map(kernel_entry).collect())
            .map_err(kernel_error)
    }

    async fn read(&self, uri: &str, offset: u64, size: u32) -> Result<Vec<u8>, types::Error> {
        let uri: ResourceUri = uri.parse().map_err(|_| types::Error::Io)?;
        self.kernel
            .read_uri(&uri, offset, size)
            .await
            .map_err(kernel_error)
    }
}

pub struct ResourceHostContext<'a> {
    pub resource_host: Arc<dyn ResourceHost>,
    pub _borrow: std::marker::PhantomData<&'a mut ()>,
}

impl<'a> ResourceHostContext<'a> {
    pub fn new(resource_host: Arc<dyn ResourceHost>) -> Self {
        Self {
            resource_host,
            _borrow: std::marker::PhantomData,
        }
    }
}

pub trait ResourceHostView: Send {
    fn resource_host(&mut self) -> ResourceHostContext<'_>;
}

pub struct ResourceHostMarker;

impl HasData for ResourceHostMarker {
    type Data<'a> = ResourceHostContext<'a>;
}

impl resource_host::Host for ResourceHostContext<'_> {
    async fn get_attrs(
        &mut self,
        uri: String,
    ) -> wasmtime::Result<Result<types::Attrs, types::Error>> {
        Ok(self.resource_host.get_attrs(&uri).await)
    }

    async fn readdir(
        &mut self,
        uri: String,
    ) -> wasmtime::Result<Result<Vec<types::Entry>, types::Error>> {
        Ok(self.resource_host.readdir(&uri).await)
    }

    async fn read(
        &mut self,
        uri: String,
        offset: u64,
        size: u32,
    ) -> wasmtime::Result<Result<Vec<u8>, types::Error>> {
        Ok(self.resource_host.read(&uri, offset, size).await)
    }
}

pub fn add_resource_host_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>
where
    T: ResourceHostView + 'static,
{
    resource_host::add_to_linker::<_, ResourceHostMarker>(linker, T::resource_host)
}

impl ResourceHostView for RuntimeStore {
    fn resource_host(&mut self) -> ResourceHostContext<'_> {
        ResourceHostContext::new(Arc::new(KernelResourceHost::new(self.host.kernel())))
    }
}

/// Adapter for a live noun extension generation.
///
/// Each operation pins the generation, creates a fresh component instance,
/// and releases the pin when the call finishes. Replacing an extension can
/// therefore retire old instances without racing an in-flight kernel request.
pub struct WasmRoutedNoun {
    generation: GenerationHandle,
}

impl WasmRoutedNoun {
    pub fn new(generation: GenerationHandle) -> Self {
        Self { generation }
    }

    async fn call<R, F>(&self, f: F) -> anyhow::Result<R>
    where
        F: for<'a> FnOnce(
                &'a crate::bindings::ns::Guest,
                &'a mut wasmtime::Store<RuntimeStore>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = wasmtime::Result<R>> + Send + 'a>>
            + Send,
        R: Send + 'static,
    {
        let lease = self
            .generation
            .pin()
            .ok_or_else(|| anyhow::anyhow!("noun generation is retiring"))?;
        let mut store = lease.store()?;
        let mut linker = Linker::new(lease.component().engine());
        add_resource_host_to_linker(&mut linker)?;
        let pre = linker.instantiate_pre(lease.component())?;
        let indices = crate::bindings::ns::GuestIndices::new(&pre)?;
        let instance = pre.instantiate_async(&mut store).await?;
        let guest = indices.load(&mut store, &instance)?;
        Ok(f(&guest, &mut store).await?)
    }
}

#[async_trait]
impl RoutedNounGuest for WasmRoutedNoun {
    async fn matches(&self, uri: &str) -> bool {
        let uri = uri.to_owned();
        self.call(move |guest, store| Box::pin(async move { guest.call_matches(store, &uri).await }))
            .await
            .unwrap_or(false)
    }

    async fn getattr(&self, uri: &str) -> Result<types::Attrs, types::Error> {
        let uri = uri.to_owned();
        self.call(move |guest, store| Box::pin(async move { guest.call_getattr(store, &uri).await }))
            .await
            .map_err(|_| types::Error::Io)?
    }

    async fn readdir(&self, uri: &str) -> Result<Vec<types::Entry>, types::Error> {
        let uri = uri.to_owned();
        self.call(move |guest, store| Box::pin(async move { guest.call_readdir(store, &uri).await }))
            .await
            .map_err(|_| types::Error::Io)?
    }

    async fn read(
        &self,
        uri: &str,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, types::Error> {
        let uri = uri.to_owned();
        self.call(move |guest, store| Box::pin(async move {
            guest.call_read(store, &uri, offset, size).await
        }))
            .await
            .map_err(|_| types::Error::Io)?
    }
}

/// A URI-aware noun provider. `eligible` is intentionally broad only because
/// the guest's `matches` method is the actual routing authority.
pub struct RoutedNoun {
    name: String,
    guest: Arc<dyn RoutedNounGuest>,
}

impl RoutedNoun {
    pub fn new(name: impl Into<String>, guest: Arc<dyn RoutedNounGuest>) -> Self {
        Self {
            name: name.into(),
            guest,
        }
    }
}

fn attrs_to_provider(attrs: &types::Attrs) -> ProviderAttrs {
    let mtime = UNIX_EPOCH + std::time::Duration::new(attrs.mtime_secs, attrs.mtime_nsecs);
    match attrs.kind {
        types::Kind::Directory => ProviderAttrs::directory(),
        types::Kind::File => ProviderAttrs::file(attrs.size, mtime),
    }
}

fn error_to_resource(uri: &ResourceUri, error: types::Error) -> ResourceError {
    let (code, message) = match error {
        types::Error::NotFound => (ResourceErrorCode::NotFound, "noun route not found"),
        types::Error::NotDir => (ResourceErrorCode::NotDir, "noun route is not a directory"),
        types::Error::IsDir => (ResourceErrorCode::IsDir, "noun route is a directory"),
        types::Error::Io => (ResourceErrorCode::Io, "noun route I/O failure"),
        types::Error::PermissionDenied => (
            ResourceErrorCode::PermissionDenied,
            "noun route access denied",
        ),
        types::Error::Unsupported => (ResourceErrorCode::Unsupported, "noun route unsupported"),
    };
    ResourceError::new(code, format!("{message}: {uri}"))
}

#[async_trait]
impl ResourceProvider for RoutedNoun {
    fn provider_name(&self) -> &str {
        &self.name
    }

    /// This is only a cheap eligibility gate. The actual route decision is
    /// made by `matches`, which receives the complete URI asynchronously.
    fn eligible(&self, _uri: &ResourceUri) -> bool {
        true
    }

    fn priority(&self) -> u32 {
        1_000
    }

    async fn matches(&self, uri: &ResourceUri) -> bool {
        self.guest.matches(&uri.to_string()).await
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        self.guest
            .getattr(&uri.to_string())
            .await
            .map(|attrs| attrs_to_provider(&attrs))
            .map_err(|error| error_to_resource(uri, error))
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
                        attrs: attrs_to_provider(&types::Attrs {
                            kind: entry.kind,
                            size: entry.size,
                            mtime_secs: 0,
                            mtime_nsecs: 0,
                        }),
                    })
                    .collect()
            })
            .map_err(|error| error_to_resource(uri, error))
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
            .map_err(|error| error_to_resource(uri, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_kernel::Kernel;

    struct Symbols;

    #[async_trait]
    impl RoutedNounGuest for Symbols {
        async fn matches(&self, uri: &str) -> bool {
            uri.ends_with("/nuke.rs/symbols")
        }

        async fn getattr(&self, _uri: &str) -> Result<types::Attrs, types::Error> {
            Ok(types::Attrs {
                kind: types::Kind::File,
                size: 7,
                mtime_secs: 0,
                mtime_nsecs: 0,
            })
        }

        async fn readdir(&self, _uri: &str) -> Result<Vec<types::Entry>, types::Error> {
            Err(types::Error::NotDir)
        }

        async fn read(&self, _uri: &str, offset: u64, size: u32) -> Result<Vec<u8>, types::Error> {
            Ok(b"Symbol"[offset as usize..]
                .iter()
                .copied()
                .take(size as usize)
                .collect())
        }
    }

    #[tokio::test]
    async fn routes_a_file_suffix_without_treating_it_as_a_prefix_mount() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("nuke.rs"), b"fn nuke() {}").unwrap();
        let kernel = Kernel::with_files_root(temp.path());
        kernel.register_resource_provider(RoutedNoun::new("ast-symbols", Arc::new(Symbols)));

        let base: ResourceUri = "file:///nuke.rs".parse().unwrap();
        assert_eq!(
            kernel.read_uri(&base, 0, 64).await.unwrap(),
            b"fn nuke() {}"
        );

        let derived: ResourceUri = "file:///nuke.rs/symbols".parse().unwrap();
        assert_eq!(kernel.read_uri(&derived, 0, 64).await.unwrap(), b"Symbol");
    }
}
