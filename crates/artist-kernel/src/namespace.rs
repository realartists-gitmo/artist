use std::ffi::OsStr;

use async_trait::async_trait;

use crate::provider::ResourceProvider;
use crate::vfs::{Attrs, DirEntry, Ino, VfsError};

/// A provider that can also be projected through the inode-based VFS adapters.
///
/// The URI-facing methods live on [`ResourceProvider`]. These methods are only
/// the host projection and may use runtime-local inode numbers.
#[async_trait]
pub trait Namespace: ResourceProvider + Send + Sync {
    fn name(&self) -> &str;
    fn set_root_ino(&mut self, ino: Ino);
    fn root_ino(&self) -> Ino;
    fn owns(&self, ino: Ino) -> bool;
    async fn lookup(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError>;
    async fn getattr(&self, ino: Ino) -> Result<Attrs, VfsError>;
    async fn readdir(&self, ino: Ino) -> Result<Vec<DirEntry>, VfsError>;
    async fn read(&self, ino: Ino, offset: u64, size: u32) -> Result<Vec<u8>, VfsError>;
    async fn write(&self, ino: Ino, offset: u64, data: &[u8]) -> Result<u32, VfsError> {
        let _ = (ino, offset, data);
        Err(VfsError::Unsupported)
    }
    async fn set_size(&self, ino: Ino, size: u64) -> Result<(), VfsError> {
        let _ = (ino, size);
        Err(VfsError::Unsupported)
    }
    async fn create_file(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        let _ = (parent, name);
        Err(VfsError::Unsupported)
    }
    async fn create_directory(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        let _ = (parent, name);
        Err(VfsError::Unsupported)
    }
    async fn rename(
        &self,
        old_parent: Ino,
        old_name: &OsStr,
        new_parent: Ino,
        new_name: &OsStr,
    ) -> Result<(), VfsError> {
        let _ = (old_parent, old_name, new_parent, new_name);
        Err(VfsError::Unsupported)
    }
    async fn unlink(&self, parent: Ino, name: &OsStr, directory: bool) -> Result<(), VfsError> {
        let _ = (parent, name, directory);
        Err(VfsError::Unsupported)
    }
    async fn parent(&self, ino: Ino) -> Option<Ino>;
}
