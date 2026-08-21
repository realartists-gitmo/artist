use std::ffi::OsStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use async_trait::async_trait;

use crate::agents::{AgentProcess, AgentTranscript, AgentsProvider};
use crate::contracts::{ContractRegistry, ExtensionContract};
use crate::namespace::Namespace;
use crate::native::FilesNamespace;
use crate::provider::{
    LayeredResourceProvider, ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode,
    ResourceProvider,
};
use crate::resources::UrlNamespace;
use crate::uri::{ResourceUri, UriError};
use crate::vfs::{Attrs, DirEntry, Ino, NodeKind, Vfs, VfsError};

#[derive(Clone)]
enum ProviderSlot {
    Namespace(Arc<dyn Namespace>),
    Resource(Arc<dyn ResourceProvider>),
}

impl ProviderSlot {
    fn provider_name(&self) -> &str {
        match self {
            Self::Namespace(provider) => provider.name(),
            Self::Resource(provider) => provider.provider_name(),
        }
    }

    fn priority(&self) -> u32 {
        match self {
            Self::Namespace(provider) => provider.priority(),
            Self::Resource(provider) => provider.priority(),
        }
    }

    async fn matches(&self, uri: &ResourceUri) -> bool {
        match self {
            Self::Namespace(provider) => provider.matches(uri).await,
            Self::Resource(provider) => provider.matches(uri).await,
        }
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        match self {
            Self::Namespace(provider) => ResourceProvider::attrs(&**provider, uri).await,
            Self::Resource(provider) => provider.attrs(uri).await,
        }
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        match self {
            Self::Namespace(provider) => ResourceProvider::readdir(&**provider, uri).await,
            Self::Resource(provider) => provider.readdir(uri).await,
        }
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        match self {
            Self::Namespace(provider) => {
                ResourceProvider::read(&**provider, uri, offset, size).await
            }
            Self::Resource(provider) => provider.read(uri, offset, size).await,
        }
    }

    async fn parent(&self, uri: &ResourceUri) -> Option<ResourceUri> {
        match self {
            Self::Namespace(provider) => ResourceProvider::parent(&**provider, uri).await,
            Self::Resource(provider) => provider.parent(uri).await,
        }
    }

    async fn write(
        &self,
        uri: &ResourceUri,
        offset: u64,
        data: &[u8],
    ) -> Result<u32, ResourceError> {
        match self {
            Self::Namespace(provider) => {
                ResourceProvider::write(&**provider, uri, offset, data).await
            }
            Self::Resource(provider) => provider.write(uri, offset, data).await,
        }
    }

    async fn set_size(&self, uri: &ResourceUri, size: u64) -> Result<(), ResourceError> {
        match self {
            Self::Namespace(provider) => ResourceProvider::set_size(&**provider, uri, size).await,
            Self::Resource(provider) => provider.set_size(uri, size).await,
        }
    }

    async fn replace_all(&self, uri: &ResourceUri, data: &[u8]) -> Result<u64, ResourceError> {
        match self {
            Self::Namespace(provider) => {
                ResourceProvider::replace_all(&**provider, uri, data).await
            }
            Self::Resource(provider) => provider.replace_all(uri, data).await,
        }
    }

    async fn create_file(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        match self {
            Self::Namespace(provider) => ResourceProvider::create_file(&**provider, uri).await,
            Self::Resource(provider) => provider.create_file(uri).await,
        }
    }

    async fn create_directory(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        match self {
            Self::Namespace(provider) => ResourceProvider::create_directory(&**provider, uri).await,
            Self::Resource(provider) => provider.create_directory(uri).await,
        }
    }

    async fn move_resource(
        &self,
        source: &ResourceUri,
        destination: &ResourceUri,
    ) -> Result<(), ResourceError> {
        match self {
            Self::Namespace(provider) => {
                ResourceProvider::move_resource(&**provider, source, destination).await
            }
            Self::Resource(provider) => provider.move_resource(source, destination).await,
        }
    }

    async fn delete(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        match self {
            Self::Namespace(provider) => ResourceProvider::delete(&**provider, uri).await,
            Self::Resource(provider) => provider.delete(uri).await,
        }
    }
}

struct Registry {
    namespaces: Vec<Arc<dyn Namespace>>,
    providers: Vec<ProviderSlot>,
}

/// The resource kernel and VFS projection.
///
/// The registry is published through one `RwLock`, so a running host can
/// publish a provider without exposing a partially updated routing/VFS view.
pub struct Kernel {
    registry: RwLock<Registry>,
    next_ino: AtomicU64,
    agents: Arc<AgentsProvider>,
    contracts: Arc<ContractRegistry>,
}

impl Kernel {
    /// Construct the kernel bootstrap surface.
    ///
    /// `file://` mirrors the current working directory by default, excluding
    /// every `.artist` directory and its descendants. Embedders that need a
    /// different host root should use [`Self::with_files_root`].
    pub fn new() -> Self {
        Self::with_url()
    }

    /// Construct a kernel with only the kernel-owned `url://` namespace root.
    ///
    /// Filesystem, tools, events, and other model-facing surfaces are opt-in
    /// providers/extensions rather than kernel requirements.
    pub fn with_url() -> Self {
        let kernel = Self::empty();
        kernel.register(UrlNamespace::new());
        kernel
    }

    /// Explicitly opt a kernel into the agent transcript/process provider.
    /// Agent resources are not part of the resource bootstrap surface.
    pub fn with_agents() -> Self {
        let kernel = Self::with_url();
        let agents: Arc<dyn ResourceProvider> = kernel.agents.clone();
        kernel
            .registry
            .write()
            .unwrap()
            .providers
            .push(ProviderSlot::Resource(agents));
        kernel
    }

    pub fn with_files_root(root: impl Into<std::path::PathBuf>) -> Self {
        let kernel = Self::with_url();
        kernel.register(FilesNamespace::new(root));
        kernel
    }

    /// Construct a kernel with selected host paths hidden from `file://` in
    /// addition to the universal `.artist` exclusion.
    pub fn with_files_root_excluding(
        root: impl Into<std::path::PathBuf>,
        exclusions: impl IntoIterator<Item = std::path::PathBuf>,
    ) -> Result<Self, ResourceError> {
        let kernel = Self::with_url();
        let files = FilesNamespace::new(root);
        for exclusion in exclusions {
            files.exclude_path(exclusion)?;
        }
        kernel.register(files);
        Ok(kernel)
    }

    /// Construct a kernel without native registrations.
    pub fn empty() -> Self {
        let agents = Arc::new(AgentsProvider::new());
        let contracts = Arc::new(ContractRegistry::new());
        Self {
            registry: RwLock::new(Registry {
                namespaces: Vec::new(),
                providers: Vec::new(),
            }),
            next_ino: AtomicU64::new(Ino::ROOT.0 + 1),
            agents,
            contracts,
        }
    }

    /// Register one of the native/VFS namespaces. The returned inode is
    /// runtime-local and must never cross the URI/component boundary.
    pub fn register<N: Namespace + 'static>(&self, mut namespace: N) -> Ino {
        let ino = Ino(self.next_ino.fetch_add(1, Ordering::Relaxed));
        namespace.set_root_ino(ino);
        let namespace: Arc<dyn Namespace> = Arc::new(namespace);
        let mut registry = self.registry.write().unwrap();
        registry
            .providers
            .push(ProviderSlot::Namespace(Arc::clone(&namespace)));
        registry.namespaces.push(namespace);
        ino
    }

    /// Register a provider-owned URI subtree without adding a new native root
    /// to the VFS. This is the path used by WASM resource components.
    pub fn register_resource_provider<P: ResourceProvider + 'static>(&self, provider: P) {
        let provider: Arc<dyn ResourceProvider> = Arc::new(provider);
        let name = provider.provider_name().to_owned();
        let mut registry = self.registry.write().unwrap();
        // Provider identity is the replacement boundary. Keeping two live
        // registrations under one name makes routing depend on insertion
        // order and leaves stale resource generations reachable after a hot
        // replacement.
        registry.providers.retain(|existing| {
            !matches!(existing, ProviderSlot::Resource(existing) if existing.provider_name() == name)
        });
        registry.providers.push(ProviderSlot::Resource(provider));
    }

    /// Remove a dynamically registered resource provider by its stable name.
    /// Native namespaces are never removed through this API.
    pub fn unregister_resource_provider(&self, name: &str) -> bool {
        let mut registry = self.registry.write().unwrap();
        let before = registry.providers.len();
        registry.providers.retain(|provider| {
            !matches!(provider, ProviderSlot::Resource(_) if provider.provider_name() == name)
        });
        before != registry.providers.len()
    }

    /// Register one logical namespace backed by a local-over-global provider
    /// pair. The child providers are kept private behind the flattened view.
    pub fn register_layered_resource_provider<L, G>(
        &self,
        name: impl Into<String>,
        local: L,
        global: G,
    ) where
        L: ResourceProvider + 'static,
        G: ResourceProvider + 'static,
    {
        self.register_resource_provider(LayeredResourceProvider::new(name, local, global));
    }

    /// Publish an append-only transcript at
    /// `agent://<agent>/transcript`.
    pub fn register_agent_transcript<T: AgentTranscript + 'static>(
        &self,
        agent: impl Into<String>,
        transcript: T,
    ) -> Result<(), ResourceError> {
        self.agents.register(agent, transcript)
    }

    pub async fn append_agent_event(
        &self,
        uri: &ResourceUri,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), ResourceError> {
        self.agents.append(uri, event_type, payload).await
    }

    pub async fn close_agent_transcript(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        self.agents.close(uri).await
    }

    /// Publish the live process channels at `agent://<agent>/stdin` and
    /// `agent://<agent>/stdout`.
    pub fn register_agent_process<T: AgentProcess + 'static>(
        &self,
        agent: impl Into<String>,
        process: T,
    ) -> Result<(), ResourceError> {
        self.agents.register_process(agent, process)
    }

    pub async fn write_agent_stdin(
        &self,
        uri: &ResourceUri,
        data: &[u8],
    ) -> Result<u32, ResourceError> {
        self.agents.write_stdin(uri, data).await
    }

    /// Register an opaque extension-defined contract. The kernel does not
    /// interpret its operation names or payloads.
    pub fn register_extension_contract<C: ExtensionContract + 'static>(
        &self,
        name: impl Into<String>,
        version: impl Into<String>,
        contract: C,
    ) -> Result<(), ResourceError> {
        self.contracts.register(name, version, contract)
    }

    pub async fn invoke_extension_contract(
        &self,
        name: &str,
        version: &str,
        operation: &str,
        input: &[u8],
    ) -> Result<Vec<u8>, ResourceError> {
        self.contracts.invoke(name, version, operation, input).await
    }

    pub fn namespace_names(&self) -> Vec<String> {
        self.registry
            .read()
            .unwrap()
            .namespaces
            .iter()
            .map(|namespace| namespace.name().to_string())
            .collect()
    }

    fn find_ino(&self, ino: Ino) -> Option<Arc<dyn Namespace>> {
        self.registry
            .read()
            .unwrap()
            .namespaces
            .iter()
            .find(|namespace| namespace.owns(ino))
            .cloned()
    }

    async fn find_uri(&self, uri: &ResourceUri) -> Option<ProviderSlot> {
        let providers = self.registry.read().unwrap().providers.clone();
        let mut matches = Vec::new();
        for provider in providers {
            if provider.matches(uri).await {
                matches.push(provider);
            }
        }
        matches
            .into_iter()
            .max_by_key(|provider| provider.priority())
    }

    async fn provider_for_uri(&self, uri: &ResourceUri) -> Result<ProviderSlot, ResourceError> {
        self.find_uri(uri).await.ok_or_else(|| {
            ResourceError::new(
                ResourceErrorCode::NotFound,
                format!("no provider matches resource URI {uri}"),
            )
        })
    }

    pub async fn attrs_uri(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        let base = uri.without_fragment();
        self.provider_for_uri(&base).await?.attrs(&base).await
    }

    pub async fn readdir_uri(
        &self,
        uri: &ResourceUri,
    ) -> Result<Vec<ProviderEntry>, ResourceError> {
        let base = uri.without_fragment();
        self.provider_for_uri(&base).await?.readdir(&base).await
    }

    pub async fn read_uri(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        let base = uri.without_fragment();
        self.provider_for_uri(&base)
            .await?
            .read(&base, offset, size)
            .await
    }

    pub async fn parent_uri(&self, uri: &ResourceUri) -> Option<ResourceUri> {
        let base = uri.without_fragment();
        self.find_uri(&base).await?.parent(&base).await
    }

    pub async fn write_uri(
        &self,
        uri: &ResourceUri,
        offset: u64,
        data: &[u8],
    ) -> Result<u32, ResourceError> {
        let base = uri.without_fragment();
        self.provider_for_uri(&base)
            .await?
            .write(&base, offset, data)
            .await
    }

    pub async fn set_size_uri(&self, uri: &ResourceUri, size: u64) -> Result<(), ResourceError> {
        let base = uri.without_fragment();
        self.provider_for_uri(&base)
            .await?
            .set_size(&base, size)
            .await
    }

    pub async fn replace_uri(&self, uri: &ResourceUri, data: &[u8]) -> Result<u64, ResourceError> {
        let base = uri.without_fragment();
        self.provider_for_uri(&base)
            .await?
            .replace_all(&base, data)
            .await
    }

    pub async fn create_file_uri(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        let base = uri.without_fragment();
        self.provider_for_uri(&base).await?.create_file(&base).await
    }

    pub async fn create_directory_uri(
        &self,
        uri: &ResourceUri,
    ) -> Result<ProviderAttrs, ResourceError> {
        let base = uri.without_fragment();
        self.provider_for_uri(&base)
            .await?
            .create_directory(&base)
            .await
    }

    pub async fn move_uri(
        &self,
        source: &ResourceUri,
        destination: &ResourceUri,
    ) -> Result<(), ResourceError> {
        let source = source.without_fragment();
        let destination = destination.without_fragment();
        self.provider_for_uri(&source)
            .await?
            .move_resource(&source, &destination)
            .await
    }

    pub async fn delete_uri(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        let base = uri.without_fragment();
        self.provider_for_uri(&base).await?.delete(&base).await
    }

    pub async fn attrs_str(&self, uri: &str) -> Result<ProviderAttrs, ResourceError> {
        let uri: ResourceUri = uri.parse().map_err(|e: UriError| {
            ResourceError::new(ResourceErrorCode::InvalidAddress, e.to_string())
        })?;
        self.attrs_uri(&uri).await
    }

    /// Compatibility name for embedders that use “provider” for a VFS
    /// namespace. URI-only providers should use [`Self::register_resource_provider`].
    pub fn register_provider<N: Namespace + 'static>(&self, namespace: N) -> Ino {
        self.register(namespace)
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Vfs for Kernel {
    async fn lookup(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        if parent == Ino::ROOT {
            let name = name.to_str().ok_or(VfsError::NotFound)?;
            let namespace = self
                .registry
                .read()
                .unwrap()
                .namespaces
                .iter()
                .find(|namespace| namespace.name() == name)
                .cloned()
                .ok_or(VfsError::NotFound)?;
            return Namespace::getattr(&*namespace, namespace.root_ino()).await;
        }
        match self.find_ino(parent) {
            Some(namespace) => Namespace::lookup(&*namespace, parent, name).await,
            None => Err(VfsError::NotFound),
        }
    }

    async fn getattr(&self, ino: Ino) -> Result<Attrs, VfsError> {
        if ino == Ino::ROOT {
            return Ok(root_attrs());
        }
        match self.find_ino(ino) {
            Some(namespace) => Namespace::getattr(&*namespace, ino).await,
            None => Err(VfsError::NotFound),
        }
    }

    async fn readdir(&self, ino: Ino) -> Result<Vec<DirEntry>, VfsError> {
        if ino == Ino::ROOT {
            return Ok(self
                .registry
                .read()
                .unwrap()
                .namespaces
                .iter()
                .map(|namespace| DirEntry {
                    ino: namespace.root_ino(),
                    kind: NodeKind::Directory,
                    name: namespace.name().into(),
                })
                .collect());
        }
        match self.find_ino(ino) {
            Some(namespace) => Namespace::readdir(&*namespace, ino).await,
            None => Err(VfsError::NotFound),
        }
    }

    async fn read(&self, ino: Ino, offset: u64, size: u32) -> Result<Vec<u8>, VfsError> {
        if ino == Ino::ROOT {
            return Err(VfsError::IsDir);
        }
        match self.find_ino(ino) {
            Some(namespace) => Namespace::read(&*namespace, ino, offset, size).await,
            None => Err(VfsError::NotFound),
        }
    }

    async fn write(&self, ino: Ino, offset: u64, data: &[u8]) -> Result<u32, VfsError> {
        match self.find_ino(ino) {
            Some(namespace) => Namespace::write(&*namespace, ino, offset, data).await,
            None => Err(VfsError::NotFound),
        }
    }

    async fn set_size(&self, ino: Ino, size: u64) -> Result<(), VfsError> {
        match self.find_ino(ino) {
            Some(namespace) => Namespace::set_size(&*namespace, ino, size).await,
            None => Err(VfsError::NotFound),
        }
    }

    async fn create_file(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        match self.find_ino(parent) {
            Some(namespace) => Namespace::create_file(&*namespace, parent, name).await,
            None => Err(VfsError::NotFound),
        }
    }

    async fn create_directory(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        match self.find_ino(parent) {
            Some(namespace) => Namespace::create_directory(&*namespace, parent, name).await,
            None => Err(VfsError::NotFound),
        }
    }

    async fn rename(
        &self,
        old_parent: Ino,
        old_name: &OsStr,
        new_parent: Ino,
        new_name: &OsStr,
    ) -> Result<(), VfsError> {
        let namespace = self.find_ino(old_parent).ok_or(VfsError::NotFound)?;
        if self.find_ino(new_parent).as_ref().map(|n| n.name()) != Some(namespace.name()) {
            return Err(VfsError::Unsupported);
        }
        Namespace::rename(&*namespace, old_parent, old_name, new_parent, new_name).await
    }

    async fn unlink(&self, parent: Ino, name: &OsStr, directory: bool) -> Result<(), VfsError> {
        match self.find_ino(parent) {
            Some(namespace) => Namespace::unlink(&*namespace, parent, name, directory).await,
            None => Err(VfsError::NotFound),
        }
    }

    async fn parent(&self, ino: Ino) -> Option<Ino> {
        if ino == Ino::ROOT {
            return None;
        }
        match self.find_ino(ino) {
            Some(namespace) => Namespace::parent(&*namespace, ino).await,
            None => None,
        }
    }
}

pub fn dir_attrs(ino: Ino) -> Attrs {
    let now = SystemTime::now();
    Attrs {
        ino,
        kind: NodeKind::Directory,
        size: 0,
        perm: 0o555,
        nlink: 2,
        uid: 0,
        gid: 0,
        atime: now,
        mtime: now,
        ctime: now,
    }
}

fn root_attrs() -> Attrs {
    dir_attrs(Ino::ROOT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::SystemTime;

    struct VirtualProvider {
        prefix: ResourceUri,
        data: HashMap<String, Vec<u8>>,
    }

    #[async_trait]
    impl ResourceProvider for VirtualProvider {
        fn provider_name(&self) -> &str {
            "virtual"
        }
        fn eligible(&self, uri: &ResourceUri) -> bool {
            uri.starts_with(&self.prefix)
        }
        fn priority(&self) -> u32 {
            100
        }

        async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
            if uri.is_root() || uri.path() == self.prefix.path() {
                return Ok(ProviderAttrs::directory());
            }
            let key = uri.path().trim_start_matches('/');
            self.data
                .get(key)
                .map(|bytes| ProviderAttrs::file(bytes.len() as u64, SystemTime::UNIX_EPOCH))
                .ok_or_else(|| ResourceError::not_found(uri))
        }

        async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
            if uri.path() != self.prefix.path() {
                return Err(ResourceError::new(
                    ResourceErrorCode::NotDir,
                    "not a directory",
                ));
            }
            Ok(self
                .data
                .keys()
                .map(|name| ProviderEntry {
                    name: name.clone(),
                    attrs: ProviderAttrs::file(
                        self.data[name].len() as u64,
                        SystemTime::UNIX_EPOCH,
                    ),
                })
                .collect())
        }

        async fn read(
            &self,
            uri: &ResourceUri,
            offset: u64,
            size: u32,
        ) -> Result<Vec<u8>, ResourceError> {
            let key = uri.path().trim_start_matches('/');
            let bytes = self
                .data
                .get(key)
                .ok_or_else(|| ResourceError::not_found(uri))?;
            Ok(bytes
                .iter()
                .skip(offset as usize)
                .take(size as usize)
                .copied()
                .collect())
        }
    }

    #[tokio::test]
    async fn kernel_bootstrap_publishes_only_url_namespace() {
        let kernel = Kernel::new();
        assert_eq!(kernel.namespace_names(), vec!["url"]);
        let roots = kernel.readdir(Ino::ROOT).await.unwrap();
        assert_eq!(
            roots
                .iter()
                .map(|entry| entry.name.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["url"]
        );
    }

    #[tokio::test]
    async fn native_roots_are_published() {
        let kernel = Kernel::with_files_root(std::env::temp_dir());
        assert_eq!(kernel.namespace_names(), vec!["url", "file"]);
        let roots = kernel.readdir(Ino::ROOT).await.unwrap();
        assert_eq!(
            roots
                .iter()
                .map(|entry| entry.name.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["url", "file"]
        );
    }

    #[tokio::test]
    async fn files_uri_reads_current_bytes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("hello.txt"), b"hello").unwrap();
        let kernel = Kernel::with_files_root(temp.path());
        let uri: ResourceUri = "file:///hello.txt".parse().unwrap();
        let attrs = kernel.attrs_uri(&uri).await.unwrap();
        assert_eq!(attrs.kind, NodeKind::File);
        assert_eq!(kernel.read_uri(&uri, 1, 3).await.unwrap(), b"ell");
        std::fs::write(temp.path().join("hello.txt"), b"changed").unwrap();
        assert_eq!(kernel.read_uri(&uri, 0, 7).await.unwrap(), b"changed");
    }

    #[tokio::test]
    async fn files_uri_reads_kind_projection_without_using_selector_as_path() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("hello.txt"), b"hello").unwrap();
        let kernel = Kernel::with_files_root(temp.path());
        let uri: ResourceUri = "file:///hello.txt?kind#content".parse().unwrap();

        // The kernel strips the position fragment before provider I/O. The
        // provider still owns the meaning of the query projection.
        assert_eq!(kernel.read_uri(&uri, 0, 32).await.unwrap(), b"file");

        let kind: ResourceUri = "file:///hello.txt?kind".parse().unwrap();
        assert_eq!(kernel.read_uri(&kind, 0, 32).await.unwrap(), b"file");
    }

    #[tokio::test]
    async fn files_uri_mutations_use_resource_addresses() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("hello.txt"), b"hello").unwrap();
        std::fs::create_dir(temp.path().join("destination")).unwrap();
        let kernel = Kernel::with_files_root(temp.path());

        let hello: ResourceUri = "file:///hello.txt#position".parse().unwrap();
        assert_eq!(kernel.write_uri(&hello, 5, b"!").await.unwrap(), 1);
        kernel.set_size_uri(&hello, 4).await.unwrap();
        assert_eq!(kernel.read_uri(&hello, 0, 16).await.unwrap(), b"hell");

        let created: ResourceUri = "file:///created.txt#position".parse().unwrap();
        kernel.create_file_uri(&created).await.unwrap();
        assert_eq!(kernel.read_uri(&created, 0, 16).await.unwrap(), b"");

        let created_dir: ResourceUri = "file:///created-dir#position".parse().unwrap();
        assert_eq!(
            kernel
                .create_directory_uri(&created_dir)
                .await
                .unwrap()
                .kind,
            NodeKind::Directory
        );

        let destination: ResourceUri = "file:///destination".parse().unwrap();

        kernel.move_uri(&hello, &destination).await.unwrap();
        let moved: ResourceUri = "file:///destination/hello.txt".parse().unwrap();
        assert_eq!(kernel.read_uri(&moved, 0, 16).await.unwrap(), b"hell");

        kernel.delete_uri(&created).await.unwrap();
        assert_eq!(
            kernel.attrs_uri(&created).await.unwrap_err().code,
            ResourceErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn files_namespace_exclusions_hide_host_backing_paths() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("visible.txt"), b"visible").unwrap();
        std::fs::create_dir(temp.path().join("private")).unwrap();
        std::fs::write(temp.path().join("private/secret.txt"), b"secret").unwrap();
        let kernel = Kernel::with_files_root_excluding(
            temp.path(),
            vec![std::path::PathBuf::from("private")],
        )
        .unwrap();

        assert_eq!(
            kernel.attrs_str("visible.txt").await.unwrap().kind,
            NodeKind::File
        );
        assert_eq!(
            kernel
                .attrs_str("file:///private/secret.txt")
                .await
                .unwrap_err()
                .code,
            ResourceErrorCode::NotFound
        );
        let files = kernel.lookup(Ino::ROOT, OsStr::new("file")).await.unwrap();
        let entries = kernel.readdir(files.ino).await.unwrap();
        assert!(
            entries
                .iter()
                .all(|entry| entry.name != OsStr::new("private"))
        );
    }

    #[tokio::test]
    async fn files_namespace_hides_every_artist_directory_automatically() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("visible.txt"), b"visible").unwrap();
        std::fs::create_dir(temp.path().join(".artist")).unwrap();
        std::fs::write(temp.path().join(".artist/private.txt"), b"private").unwrap();
        std::fs::create_dir_all(temp.path().join("nested/.artist/deeper")).unwrap();
        std::fs::write(
            temp.path().join("nested/.artist/deeper/private.txt"),
            b"private",
        )
        .unwrap();
        let kernel = Kernel::with_files_root(temp.path());

        assert!(kernel.attrs_str("visible.txt").await.is_ok());
        for uri in [
            "file:///.artist/private.txt",
            "file:///nested/.artist/deeper/private.txt",
        ] {
            assert_eq!(
                kernel.attrs_str(uri).await.unwrap_err().code,
                ResourceErrorCode::NotFound
            );
        }
        let root = kernel.attrs_str("file:///").await.unwrap();
        assert_eq!(root.kind, NodeKind::Directory);
        let entries = kernel
            .readdir_uri(&"file:///".parse().unwrap())
            .await
            .unwrap();
        assert!(entries.iter().all(|entry| entry.name != ".artist"));
    }

    #[tokio::test]
    async fn files_vfs_supports_write_rename_and_unlink() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("old.txt"), b"old").unwrap();
        let kernel = Kernel::with_files_root(temp.path());
        let files = kernel.lookup(Ino::ROOT, OsStr::new("file")).await.unwrap();
        let old = kernel
            .lookup(files.ino, OsStr::new("old.txt"))
            .await
            .unwrap();

        assert_eq!(kernel.write(old.ino, 0, b"new").await.unwrap(), 3);
        assert_eq!(kernel.read(old.ino, 0, 3).await.unwrap(), b"new");
        let created = kernel
            .create_file(files.ino, OsStr::new("created.txt"))
            .await
            .unwrap();
        kernel.write(created.ino, 0, b"created").await.unwrap();
        kernel
            .rename(
                files.ino,
                OsStr::new("created.txt"),
                files.ino,
                OsStr::new("renamed.txt"),
            )
            .await
            .unwrap();
        assert!(
            kernel
                .lookup(files.ino, OsStr::new("renamed.txt"))
                .await
                .is_ok()
        );
        kernel
            .unlink(files.ino, OsStr::new("renamed.txt"), false)
            .await
            .unwrap();
        assert!(matches!(
            kernel.lookup(files.ino, OsStr::new("renamed.txt")).await,
            Err(VfsError::NotFound)
        ));
    }

    #[tokio::test]
    async fn virtual_provider_claims_a_uri_subtree() {
        let prefix: ResourceUri = "resource://demo".parse().unwrap();
        let mut data = HashMap::new();
        data.insert("answer".into(), b"42".to_vec());
        let kernel = Kernel::new();
        kernel.register_resource_provider(VirtualProvider {
            prefix: prefix.clone(),
            data,
        });
        let uri: ResourceUri = "resource://demo/answer".parse().unwrap();
        assert_eq!(kernel.read_uri(&uri, 0, 2).await.unwrap(), b"42");
        assert_eq!(kernel.parent_uri(&uri).await.unwrap(), prefix);
    }
}
