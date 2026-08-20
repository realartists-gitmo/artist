use std::ffi::{OsStr, OsString};
use std::time::SystemTime;

use async_trait::async_trait;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ino(pub u64);

impl Ino {
    pub const ROOT: Ino = Ino(1);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Directory,
    File,
}

#[derive(Clone, Debug)]
pub struct Attrs {
    pub ino: Ino,
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

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub ino: Ino,
    pub kind: NodeKind,
    pub name: OsString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfsError {
    NotFound,
    NotDir,
    IsDir,
    Exists,
    NotEmpty,
    PermissionDenied,
    Unsupported,
    Io,
}

#[async_trait]
pub trait Vfs: Send + Sync {
    async fn lookup(&self, parent: Ino, name: &std::ffi::OsStr) -> Result<Attrs, VfsError>;
    async fn getattr(&self, ino: Ino) -> Result<Attrs, VfsError>;
    async fn readdir(&self, ino: Ino) -> Result<Vec<DirEntry>, VfsError>;
    async fn read(&self, ino: Ino, offset: u64, size: u32) -> Result<Vec<u8>, VfsError>;

    /// Write bytes at an offset, extending the file as necessary.
    async fn write(&self, _ino: Ino, _offset: u64, _data: &[u8]) -> Result<u32, VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Resize a regular file. Implementations may reject this when unsupported.
    async fn set_size(&self, _ino: Ino, _size: u64) -> Result<(), VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Create a regular file below `parent` and return its inode.
    async fn create_file(&self, _parent: Ino, _name: &OsStr) -> Result<Attrs, VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Create a directory below `parent` and return its inode.
    async fn create_directory(&self, _parent: Ino, _name: &OsStr) -> Result<Attrs, VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Rename or move one directory entry. The destination may replace a file
    /// according to the backing filesystem's normal rules.
    async fn rename(
        &self,
        _old_parent: Ino,
        _old_name: &OsStr,
        _new_parent: Ino,
        _new_name: &OsStr,
    ) -> Result<(), VfsError> {
        Err(VfsError::Unsupported)
    }

    /// Remove a file or an empty directory entry.
    async fn unlink(&self, _parent: Ino, _name: &OsStr, _directory: bool) -> Result<(), VfsError> {
        Err(VfsError::Unsupported)
    }
    async fn parent(&self, ino: Ino) -> Option<Ino>;
}
