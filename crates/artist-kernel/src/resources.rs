use std::ffi::OsStr;

use async_trait::async_trait;

use crate::kernel::dir_attrs;
use crate::namespace::Namespace;
use crate::provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
use crate::uri::ResourceUri;
use crate::vfs::{Attrs, DirEntry, Ino, VfsError};

/// The kernel-owned namespace-registration root at `url://`.
pub struct UrlNamespace {
    root_ino: Ino,
}

impl UrlNamespace {
    pub fn new() -> Self {
        Self { root_ino: Ino(0) }
    }
}

impl Default for UrlNamespace {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ResourceProvider for UrlNamespace {
    fn provider_name(&self) -> &str {
        "url"
    }

    fn eligible(&self, uri: &ResourceUri) -> bool {
        uri.scheme() == "url" && uri.authority().is_empty() && uri.is_root()
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        if self.eligible(uri) {
            Ok(ProviderAttrs::directory())
        } else {
            Err(ResourceError::not_found(uri))
        }
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        if self.eligible(uri) {
            Ok(Vec::new())
        } else {
            Err(ResourceError::not_found(uri))
        }
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        _offset: u64,
        _size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        if self.eligible(uri) {
            Err(ResourceError::new(
                ResourceErrorCode::IsDir,
                "cannot read a namespace directory",
            ))
        } else {
            Err(ResourceError::not_found(uri))
        }
    }
}

#[async_trait]
impl Namespace for UrlNamespace {
    fn name(&self) -> &str {
        "url"
    }
    fn set_root_ino(&mut self, ino: Ino) {
        self.root_ino = ino;
    }
    fn root_ino(&self) -> Ino {
        self.root_ino
    }
    fn owns(&self, ino: Ino) -> bool {
        ino == self.root_ino
    }

    async fn lookup(&self, _parent: Ino, _name: &OsStr) -> Result<Attrs, VfsError> {
        Err(VfsError::NotFound)
    }

    async fn getattr(&self, ino: Ino) -> Result<Attrs, VfsError> {
        if ino == self.root_ino {
            Ok(dir_attrs(self.root_ino))
        } else {
            Err(VfsError::NotFound)
        }
    }

    async fn readdir(&self, ino: Ino) -> Result<Vec<DirEntry>, VfsError> {
        if ino == self.root_ino {
            Ok(Vec::new())
        } else {
            Err(VfsError::NotFound)
        }
    }

    async fn read(&self, _ino: Ino, _offset: u64, _size: u32) -> Result<Vec<u8>, VfsError> {
        Err(VfsError::IsDir)
    }

    async fn parent(&self, ino: Ino) -> Option<Ino> {
        (ino == self.root_ino).then_some(Ino::ROOT)
    }
}
