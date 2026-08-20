//! artist-vfs — bridges the platform-neutral [`artist_kernel::Vfs`] out to FUSE
//! on unix hosts via fuser's experimental async API.

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use artist_kernel::{Attrs, DirEntry, Ino, NodeKind, Vfs, VfsError};
use async_trait::async_trait;
use fuser::experimental::{
    AsyncFilesystem, DirEntListBuilder, GetAttrResponse, LookupResponse, RequestContext,
    TokioAdapter,
};
use fuser::{
    Config, Errno, FileAttr, FileHandle, FileType, Generation, INodeNo, LockOwner, MountOption,
};

const TTL: Duration = Duration::from_secs(1);

pub struct FuseBridge {
    vfs: Arc<dyn Vfs>,
}

impl FuseBridge {
    pub fn new(vfs: Arc<dyn Vfs>) -> Self {
        Self { vfs }
    }
}

fn map_err(e: &VfsError) -> Errno {
    match e {
        VfsError::NotFound => Errno::ENOENT,
        VfsError::NotDir => Errno::ENOTDIR,
        VfsError::IsDir => Errno::EISDIR,
        VfsError::Exists => Errno::EEXIST,
        VfsError::NotEmpty => Errno::ENOTEMPTY,
        VfsError::PermissionDenied => Errno::EACCES,
        VfsError::Unsupported => Errno::ENOSYS,
        VfsError::Io => Errno::EIO,
    }
}

fn to_file_attr(a: &Attrs) -> FileAttr {
    FileAttr {
        ino: INodeNo(a.ino.0),
        size: a.size,
        blocks: a.size.div_ceil(512),
        atime: a.atime,
        mtime: a.mtime,
        ctime: a.ctime,
        crtime: a.ctime,
        kind: match a.kind {
            NodeKind::Directory => FileType::Directory,
            NodeKind::File => FileType::RegularFile,
        },
        perm: a.perm,
        nlink: a.nlink,
        uid: a.uid,
        gid: a.gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn to_file_type(kind: &NodeKind) -> FileType {
    match kind {
        NodeKind::Directory => FileType::Directory,
        NodeKind::File => FileType::RegularFile,
    }
}

#[async_trait]
impl AsyncFilesystem for FuseBridge {
    async fn lookup(
        &self,
        _context: &RequestContext,
        parent: INodeNo,
        name: &OsStr,
    ) -> Result<LookupResponse, Errno> {
        let attrs = self
            .vfs
            .lookup(Ino(parent.0), name)
            .await
            .map_err(|e| map_err(&e))?;
        Ok(LookupResponse::new(
            TTL,
            to_file_attr(&attrs),
            Generation(0),
        ))
    }

    async fn getattr(
        &self,
        _context: &RequestContext,
        ino: INodeNo,
        _file_handle: Option<FileHandle>,
    ) -> Result<GetAttrResponse, Errno> {
        let attrs = self
            .vfs
            .getattr(Ino(ino.0))
            .await
            .map_err(|e| map_err(&e))?;
        Ok(GetAttrResponse::new(TTL, to_file_attr(&attrs)))
    }

    async fn read(
        &self,
        _context: &RequestContext,
        ino: INodeNo,
        _file_handle: FileHandle,
        offset: u64,
        size: u32,
        _flags: fuser::OpenFlags,
        _lock: Option<LockOwner>,
        out_data: &mut Vec<u8>,
    ) -> Result<(), Errno> {
        let data = self
            .vfs
            .read(Ino(ino.0), offset, size)
            .await
            .map_err(|e| map_err(&e))?;
        out_data.extend_from_slice(&data);
        Ok(())
    }

    async fn readdir(
        &self,
        _context: &RequestContext,
        ino: INodeNo,
        _file_handle: FileHandle,
        offset: u64,
        builder: DirEntListBuilder<'_>,
    ) -> Result<(), Errno> {
        let mut entries: Vec<DirEntry> = Vec::new();
        let ino = Ino(ino.0);
        entries.push(DirEntry {
            ino,
            kind: NodeKind::Directory,
            name: ".".into(),
        });
        if let Some(parent) = self.vfs.parent(ino).await {
            entries.push(DirEntry {
                ino: parent,
                kind: NodeKind::Directory,
                name: "..".into(),
            });
        }
        entries.extend(self.vfs.readdir(ino).await.map_err(|e| map_err(&e))?);

        let mut builder = builder;
        for (i, entry) in entries.iter().enumerate().skip(offset as usize) {
            // i + 1 means the index of the next entry
            if builder.add(
                INodeNo(entry.ino.0),
                i as u64 + 1,
                to_file_type(&entry.kind),
                &entry.name,
            ) {
                break;
            }
        }
        Ok(())
    }
}

/// Mount `vfs` at `mountpoint`. The returned [`fuser::BackgroundSession`] lives
/// for the duration of the mount; dropping it unmounts the filesystem.
pub fn mount(
    vfs: Arc<dyn Vfs>,
    mountpoint: &Path,
    readonly: bool,
) -> io::Result<fuser::BackgroundSession> {
    let mut config = Config::default();
    config
        .mount_options
        .push(MountOption::FSName("artist".to_string()));
    config
        .mount_options
        .push(MountOption::Subtype("artist".to_string()));
    if readonly {
        config.mount_options.push(MountOption::RO);
    }

    let adapter = TokioAdapter::new(FuseBridge::new(vfs));
    let session = fuser::Session::new(adapter, mountpoint, &config)?;
    session.spawn()
}
