use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;

use crate::uri::ResourceUri;
use crate::vfs::NodeKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderAttrs {
    pub kind: NodeKind,
    pub size: u64,
    pub perm: u16,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub atime: SystemTime,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
}

impl ProviderAttrs {
    pub fn directory() -> Self {
        let now = SystemTime::now();
        Self {
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

    pub fn file(size: u64, mtime: SystemTime) -> Self {
        Self {
            kind: NodeKind::File,
            size,
            perm: 0o444,
            nlink: 1,
            uid: 0,
            gid: 0,
            atime: mtime,
            mtime,
            ctime: mtime,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEntry {
    pub name: String,
    pub attrs: ProviderAttrs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceErrorCode {
    InvalidAddress,
    NotFound,
    NotDir,
    IsDir,
    Unsupported,
    Unavailable,
    Io,
    Component,
    Cancelled,
    Timeout,
    Conflict,
    PermissionDenied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceError {
    pub code: ResourceErrorCode,
    pub message: String,
}

impl ResourceError {
    pub fn new(code: ResourceErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn not_found(uri: &ResourceUri) -> Self {
        Self::new(
            ResourceErrorCode::NotFound,
            format!("resource not found: {uri}"),
        )
    }
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ResourceError {}

#[async_trait]
pub trait ResourceProvider: Send + Sync {
    /// Stable provider identity used for diagnostics and registration.
    fn provider_name(&self) -> &str;

    /// Cheap synchronous eligibility check used before dynamic routing.
    /// This is not a URI mount or ownership declaration.
    fn eligible(&self, uri: &ResourceUri) -> bool;

    /// Decide whether this provider handles this URI. Unlike [`eligible`], this
    /// decision may be dynamic and may inspect the complete URI, including
    /// path suffixes, queries, and fragments. This is the routing hook for
    /// derived/decorating noun providers.
    async fn matches(&self, uri: &ResourceUri) -> bool {
        self.eligible(uri)
    }

    /// Higher-priority matching providers win when several routes handle a URI.
    fn priority(&self) -> u32 {
        0
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError>;

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError>;

    /// Read current bytes at `offset`, returning at most `size` bytes.
    /// Query semantics are provider-owned. Providers must reject unsupported
    /// queries rather than silently treating them as part of the path.
    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError>;

    /// Providers may override this if their hierarchy is not represented by
    /// ordinary URI parenthood. The default is correct for file-shaped trees.
    async fn parent(&self, uri: &ResourceUri) -> Option<ResourceUri> {
        uri.parent()
    }

    /// Write bytes at an offset in an existing resource.
    async fn write(
        &self,
        _uri: &ResourceUri,
        _offset: u64,
        _data: &[u8],
    ) -> Result<u32, ResourceError> {
        Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            "provider does not support URI writes",
        ))
    }

    /// Resize an existing resource.
    async fn set_size(&self, _uri: &ResourceUri, _size: u64) -> Result<(), ResourceError> {
        Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            "provider does not support URI resizing",
        ))
    }

    /// Create an empty regular resource at the exact destination URI.
    async fn create_file(&self, _uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            "provider does not support URI file creation",
        ))
    }

    /// Create a directory at the exact destination URI.
    async fn create_directory(&self, _uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            "provider does not support URI directory creation",
        ))
    }

    /// Move `source` to `destination`. Providers apply destination semantics:
    /// an existing directory receives the source basename; an absent
    /// destination is the exact target URI.
    async fn move_resource(
        &self,
        _source: &ResourceUri,
        _destination: &ResourceUri,
    ) -> Result<(), ResourceError> {
        Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            "provider does not support URI moves",
        ))
    }

    /// Delete the addressed resource.
    async fn delete(&self, _uri: &ResourceUri) -> Result<(), ResourceError> {
        Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            "provider does not support URI deletion",
        ))
    }
}

/// A local-over-global view of one logical namespace.
///
/// The two providers must claim the same URI scheme. The overlay exposes one
/// URI tree: local resources shadow global resources, directory listings are
/// merged, and provenance is never visible to callers. Existing mutations go
/// to the layer supplying the visible resource; explicit creation goes local.
pub struct LayeredResourceProvider {
    name: String,
    local: Arc<dyn ResourceProvider>,
    global: Arc<dyn ResourceProvider>,
}

impl LayeredResourceProvider {
    pub fn new<L, G>(name: impl Into<String>, local: L, global: G) -> Self
    where
        L: ResourceProvider + 'static,
        G: ResourceProvider + 'static,
    {
        Self {
            name: name.into(),
            local: Arc::new(local),
            global: Arc::new(global),
        }
    }

    pub fn from_arcs(
        name: impl Into<String>,
        local: Arc<dyn ResourceProvider>,
        global: Arc<dyn ResourceProvider>,
    ) -> Self {
        Self {
            name: name.into(),
            local,
            global,
        }
    }

    async fn visible(&self, uri: &ResourceUri) -> Result<Layer, ResourceError> {
        match self.local.attrs(uri).await {
            Ok(_) => Ok(Layer::Local),
            Err(error) if error.code == ResourceErrorCode::NotFound => {
                self.global.attrs(uri).await.map(|_| Layer::Global)
            }
            Err(error) => Err(error),
        }
    }

    fn provider(&self, layer: Layer) -> &Arc<dyn ResourceProvider> {
        match layer {
            Layer::Local => &self.local,
            Layer::Global => &self.global,
        }
    }
}

#[derive(Clone, Copy)]
enum Layer {
    Local,
    Global,
}

#[async_trait]
impl ResourceProvider for LayeredResourceProvider {
    fn provider_name(&self) -> &str {
        &self.name
    }

    fn eligible(&self, uri: &ResourceUri) -> bool {
        self.local.eligible(uri) || self.global.eligible(uri)
    }

    fn priority(&self) -> u32 {
        100
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        match self.visible(uri).await? {
            Layer::Local => self.local.attrs(uri).await,
            Layer::Global => self.global.attrs(uri).await,
        }
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        let local = self.local.readdir(uri).await;
        let global = self.global.readdir(uri).await;
        match (local, global) {
            (Ok(local), Ok(global)) => {
                let mut merged = BTreeMap::new();
                for entry in global {
                    merged.insert(entry.name.clone(), entry);
                }
                for entry in local {
                    merged.insert(entry.name.clone(), entry);
                }
                Ok(merged.into_values().collect())
            }
            (Ok(entries), Err(error)) if error.code == ResourceErrorCode::NotFound => Ok(entries),
            (Err(error), Ok(entries)) if error.code == ResourceErrorCode::NotFound => Ok(entries),
            (Err(local), Err(_global)) => Err(local),
            (Ok(_), Err(error)) | (Err(error), Ok(_)) => Err(error),
        }
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        match self.visible(uri).await? {
            Layer::Local => self.local.read(uri, offset, size).await,
            Layer::Global => self.global.read(uri, offset, size).await,
        }
    }

    async fn parent(&self, uri: &ResourceUri) -> Option<ResourceUri> {
        match self.visible(uri).await.ok()? {
            Layer::Local => self.local.parent(uri).await,
            Layer::Global => self.global.parent(uri).await,
        }
    }

    async fn write(
        &self,
        uri: &ResourceUri,
        offset: u64,
        data: &[u8],
    ) -> Result<u32, ResourceError> {
        let layer = match self.visible(uri).await {
            Ok(layer) => layer,
            Err(error) if error.code == ResourceErrorCode::NotFound => Layer::Local,
            Err(error) => return Err(error),
        };
        self.provider(layer).write(uri, offset, data).await
    }

    async fn set_size(&self, uri: &ResourceUri, size: u64) -> Result<(), ResourceError> {
        let layer = self.visible(uri).await?;
        self.provider(layer).set_size(uri, size).await
    }

    async fn create_file(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        self.local.create_file(uri).await
    }

    async fn create_directory(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        self.local.create_directory(uri).await
    }

    async fn move_resource(
        &self,
        source: &ResourceUri,
        destination: &ResourceUri,
    ) -> Result<(), ResourceError> {
        let source_layer = self.visible(source).await?;
        if let Ok(destination_layer) = self.visible(destination).await {
            if std::mem::discriminant(&source_layer) != std::mem::discriminant(&destination_layer) {
                return Err(ResourceError::new(
                    ResourceErrorCode::Conflict,
                    "layered move crosses local and global sources",
                ));
            }
        }
        self.provider(source_layer)
            .move_resource(source, destination)
            .await
    }

    async fn delete(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        let layer = self.visible(uri).await?;
        self.provider(layer).delete(uri).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    struct MemoryFiles {
        files: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    impl MemoryFiles {
        fn new<I, S, B>(files: I) -> Self
        where
            I: IntoIterator<Item = (S, B)>,
            S: Into<String>,
            B: Into<Vec<u8>>,
        {
            Self {
                files: Mutex::new(
                    files
                        .into_iter()
                        .map(|(name, bytes)| (name.into(), bytes.into()))
                        .collect(),
                ),
            }
        }

        fn key(uri: &ResourceUri) -> String {
            uri.path().trim_start_matches('/').to_owned()
        }
    }

    #[async_trait]
    impl ResourceProvider for MemoryFiles {
        fn provider_name(&self) -> &str {
            "memory"
        }

        fn eligible(&self, uri: &ResourceUri) -> bool {
            uri.scheme() == "overlay" && uri.authority().is_empty()
        }

        async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
            if uri.is_root() {
                return Ok(ProviderAttrs::directory());
            }
            self.files
                .lock()
                .unwrap()
                .get(&Self::key(uri))
                .map(|bytes| ProviderAttrs::file(bytes.len() as u64, SystemTime::now()))
                .ok_or_else(|| ResourceError::not_found(uri))
        }

        async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
            if !uri.is_root() {
                return Err(ResourceError::not_found(uri));
            }
            Ok(self
                .files
                .lock()
                .unwrap()
                .keys()
                .map(|name| ProviderEntry {
                    name: name.clone(),
                    attrs: ProviderAttrs::file(0, SystemTime::now()),
                })
                .collect())
        }

        async fn read(
            &self,
            uri: &ResourceUri,
            offset: u64,
            size: u32,
        ) -> Result<Vec<u8>, ResourceError> {
            let bytes = self
                .files
                .lock()
                .unwrap()
                .get(&Self::key(uri))
                .cloned()
                .ok_or_else(|| ResourceError::not_found(uri))?;
            Ok(bytes
                .into_iter()
                .skip(offset as usize)
                .take(size as usize)
                .collect())
        }

        async fn write(
            &self,
            uri: &ResourceUri,
            offset: u64,
            data: &[u8],
        ) -> Result<u32, ResourceError> {
            let mut files = self.files.lock().unwrap();
            let bytes = files
                .get_mut(&Self::key(uri))
                .ok_or_else(|| ResourceError::not_found(uri))?;
            let offset = offset as usize;
            if bytes.len() < offset {
                bytes.resize(offset, 0);
            }
            if bytes.len() < offset + data.len() {
                bytes.resize(offset + data.len(), 0);
            }
            bytes[offset..offset + data.len()].copy_from_slice(data);
            Ok(data.len() as u32)
        }

        async fn create_file(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
            self.files
                .lock()
                .unwrap()
                .insert(Self::key(uri), Vec::new());
            Ok(ProviderAttrs::file(0, SystemTime::now()))
        }
    }

    #[tokio::test]
    async fn local_over_global_view_merges_and_hides_provenance() {
        let overlay = LayeredResourceProvider::new(
            "overlay",
            MemoryFiles::new([("local", &b"local"[..]), ("shared", &b"local shared"[..])]),
            MemoryFiles::new([
                ("global", &b"global"[..]),
                ("shared", &b"global shared"[..]),
            ]),
        );
        let root: ResourceUri = "overlay:///".parse().unwrap();
        let entries = overlay.readdir(&root).await.unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["global", "local", "shared"]
        );
        let shared: ResourceUri = "overlay:///shared".parse().unwrap();
        assert_eq!(overlay.read(&shared, 0, 64).await.unwrap(), b"local shared");
        let global: ResourceUri = "overlay:///global".parse().unwrap();
        assert_eq!(overlay.read(&global, 0, 64).await.unwrap(), b"global");
    }

    #[tokio::test]
    async fn layered_creation_targets_local_source() {
        let local = MemoryFiles::new(Vec::<(&str, &[u8])>::new());
        let overlay = LayeredResourceProvider::new(
            "overlay",
            local,
            MemoryFiles::new([("global", &b"global"[..])]),
        );
        let created: ResourceUri = "overlay:///new".parse().unwrap();
        overlay.create_file(&created).await.unwrap();
        overlay.write(&created, 0, b"local").await.unwrap();
        assert_eq!(overlay.read(&created, 0, 64).await.unwrap(), b"local");
    }
}
