use std::ffi::OsStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use async_trait::async_trait;

use crate::namespace::Namespace;
use crate::native::{EmptyNamespace, FilesNamespace};
use crate::provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
use crate::resources::Resources;
use crate::uri::{ResourceUri, UriError};
use crate::vfs::{Attrs, DirEntry, Ino, NodeKind, Vfs, VfsError};

#[derive(Clone)]
enum ProviderSlot {
    Namespace(Arc<dyn Namespace>),
    Resource(Arc<dyn ResourceProvider>),
}

impl ProviderSlot {
    fn claims(&self, uri: &ResourceUri) -> bool {
        match self {
            Self::Namespace(provider) => provider.claims(uri),
            Self::Resource(provider) => provider.claims(uri),
        }
    }

    fn priority(&self) -> u32 {
        match self {
            Self::Namespace(provider) => provider.priority(),
            Self::Resource(provider) => provider.priority(),
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
}

impl Kernel {
    /// Construct the complete native bootstrap surface.
    ///
    /// `files://` mirrors the current working directory by default. Embedders
    /// that need a different host root should use [`Self::with_files_root`].
    pub fn new() -> Self {
        let root = std::env::current_dir().unwrap_or_else(|_| ".".into());
        Self::with_files_root(root)
    }

    pub fn with_files_root(root: impl Into<std::path::PathBuf>) -> Self {
        let kernel = Self::empty();
        kernel.register(Resources::new());
        kernel.register(EmptyNamespace::new("tools"));
        kernel.register(EmptyNamespace::new("events"));
        kernel.register(FilesNamespace::new(root));
        kernel
    }

    /// Construct a kernel without native registrations.
    pub fn empty() -> Self {
        Self {
            registry: RwLock::new(Registry {
                namespaces: Vec::new(),
                providers: Vec::new(),
            }),
            next_ino: AtomicU64::new(Ino::ROOT.0 + 1),
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
        self.registry
            .write()
            .unwrap()
            .providers
            .push(ProviderSlot::Resource(Arc::new(provider)));
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

    fn find_uri(&self, uri: &ResourceUri) -> Option<ProviderSlot> {
        self.registry
            .read()
            .unwrap()
            .providers
            .iter()
            .filter(|provider| provider.claims(uri))
            .max_by_key(|provider| provider.priority())
            .cloned()
    }

    fn provider_for_uri(&self, uri: &ResourceUri) -> Result<ProviderSlot, ResourceError> {
        self.find_uri(uri).ok_or_else(|| {
            ResourceError::new(
                ResourceErrorCode::NotFound,
                format!("no provider claims resource URI {uri}"),
            )
        })
    }

    pub async fn attrs_uri(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        self.provider_for_uri(uri)?.attrs(uri).await
    }

    pub async fn readdir_uri(
        &self,
        uri: &ResourceUri,
    ) -> Result<Vec<ProviderEntry>, ResourceError> {
        self.provider_for_uri(uri)?.readdir(uri).await
    }

    pub async fn read_uri(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        self.provider_for_uri(uri)?.read(uri, offset, size).await
    }

    pub async fn parent_uri(&self, uri: &ResourceUri) -> Option<ResourceUri> {
        self.find_uri(uri)?.parent(uri).await
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
        fn claims(&self, uri: &ResourceUri) -> bool {
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
    async fn native_roots_are_published() {
        let kernel = Kernel::with_files_root(std::env::temp_dir());
        assert_eq!(
            kernel.namespace_names(),
            vec!["resources", "tools", "events", "files"]
        );
        let roots = kernel.readdir(Ino::ROOT).await.unwrap();
        assert_eq!(
            roots
                .iter()
                .map(|entry| entry.name.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["resources", "tools", "events", "files"]
        );
    }

    #[tokio::test]
    async fn files_uri_reads_current_bytes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("hello.txt"), b"hello").unwrap();
        let kernel = Kernel::with_files_root(temp.path());
        let uri: ResourceUri = "files:///hello.txt".parse().unwrap();
        let attrs = kernel.attrs_uri(&uri).await.unwrap();
        assert_eq!(attrs.kind, NodeKind::File);
        assert_eq!(kernel.read_uri(&uri, 1, 3).await.unwrap(), b"ell");
        std::fs::write(temp.path().join("hello.txt"), b"changed").unwrap();
        assert_eq!(kernel.read_uri(&uri, 0, 7).await.unwrap(), b"changed");
    }

    #[tokio::test]
    async fn files_vfs_supports_write_rename_and_unlink() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("old.txt"), b"old").unwrap();
        let kernel = Kernel::with_files_root(temp.path());
        let files = kernel.lookup(Ino::ROOT, OsStr::new("files")).await.unwrap();
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
        let prefix: ResourceUri = "resources://demo".parse().unwrap();
        let mut data = HashMap::new();
        data.insert("answer".into(), b"42".to_vec());
        let kernel = Kernel::new();
        kernel.register_resource_provider(VirtualProvider {
            prefix: prefix.clone(),
            data,
        });
        let uri: ResourceUri = "resources://demo/answer".parse().unwrap();
        assert_eq!(kernel.read_uri(&uri, 0, 2).await.unwrap(), b"42");
        assert_eq!(kernel.parent_uri(&uri).await.unwrap(), prefix);
    }
}
