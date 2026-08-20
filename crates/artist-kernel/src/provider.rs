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

    /// Whether this provider owns the addressed resource.
    fn claims(&self, uri: &ResourceUri) -> bool;

    /// More-specific claims win over less-specific claims.
    fn priority(&self) -> u32 {
        0
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError>;

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError>;

    /// Read current bytes at `offset`, returning at most `size` bytes.
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
}
