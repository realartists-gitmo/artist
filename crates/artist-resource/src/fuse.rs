//! Linux FUSE view of the logical resource tree.

use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, UNIX_EPOCH},
};

use fuser::{
    BackgroundSession, BsdFileFlags, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, LockOwner, MountOption, OpenFlags, RenameFlags, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
    WriteFlags,
};
use tokio::runtime::Handle;

use crate::{ResourceReply, ResourceRequest, ResourceRouter, ResourceUri, mount_path_to_uri};

const TTL: Duration = Duration::from_millis(100);

pub struct FuseMount {
    path: PathBuf,
    _temp: tempfile::TempDir,
    session: Option<BackgroundSession>,
}

impl FuseMount {
    pub fn mount(router: ResourceRouter, runtime: Handle) -> Result<Self, std::io::Error> {
        let temp = tempfile::Builder::new().prefix("artist-").tempdir()?;
        let path = temp.path().to_owned();
        let filesystem = ResourceFs::new(router, runtime, path.clone());
        let mut config = fuser::Config::default();
        config.mount_options = vec![MountOption::FSName("artist".into())];
        let session = fuser::spawn_mount2(filesystem, &path, &config)?;
        Ok(Self {
            path,
            _temp: temp,
            session: Some(session),
        })
    }
    pub fn root(&self) -> &Path {
        &self.path
    }
    pub fn artist_root_env(&self) -> (&'static str, &Path) {
        ("ARTIST_ROOT", &self.path)
    }
}

impl Drop for FuseMount {
    fn drop(&mut self) {
        self.session.take();
    }
}

#[derive(Clone)]
enum Node {
    Root,
    Scheme(String),
    Resource { uri: ResourceUri, directory: bool },
}

struct Inodes {
    next: u64,
    nodes: HashMap<u64, Node>,
    keys: HashMap<String, u64>,
    lookups: HashMap<u64, u64>,
    opens: HashMap<u64, u64>,
}

struct OpenFile {
    ino: u64,
    uri: ResourceUri,
    bytes: Vec<u8>,
    dirty: bool,
}

struct OpenFiles {
    next: u64,
    files: HashMap<u64, OpenFile>,
}

struct ResourceFs {
    router: ResourceRouter,
    runtime: Handle,
    mount: PathBuf,
    inodes: Mutex<Inodes>,
    open_files: Mutex<OpenFiles>,
}

impl ResourceFs {
    fn new(router: ResourceRouter, runtime: Handle, mount: PathBuf) -> Self {
        Self {
            router,
            runtime,
            mount,
            inodes: Mutex::new(Inodes {
                next: 2,
                nodes: HashMap::from([(1, Node::Root)]),
                keys: HashMap::from([("/".into(), 1)]),
                lookups: HashMap::new(),
                opens: HashMap::new(),
            }),
            open_files: Mutex::new(OpenFiles {
                next: 1,
                files: HashMap::new(),
            }),
        }
    }
    fn node(&self, ino: INodeNo) -> Option<Node> {
        self.inodes.lock().ok()?.nodes.get(&u64::from(ino)).cloned()
    }
    fn inode(&self, key: String, node: Node) -> INodeNo {
        let mut table = self.inodes.lock().unwrap();
        if let Some(ino) = table.keys.get(&key) {
            return INodeNo(*ino);
        }
        let ino = table.next;
        table.next += 1;
        table.keys.insert(key, ino);
        table.nodes.insert(ino, node);
        INodeNo(ino)
    }
    fn classify(&self, uri: ResourceUri) -> Option<(INodeNo, bool, u64)> {
        let (directory, size) = match self.runtime.block_on(
            self.router
                .handle(ResourceRequest::Children { uri: uri.clone() }),
        ) {
            Ok(ResourceReply::Children { .. }) => (true, 0),
            _ => match self
                .runtime
                .block_on(self.router.handle(ResourceRequest::Read {
                    uri: uri.clone(),
                    start_line: None,
                    line_count: None,
                })) {
                Ok(ResourceReply::Text { text }) => (false, text.len() as u64),
                _ => return None,
            },
        };
        let ino = self.inode(uri.to_string(), Node::Resource { uri, directory });
        Some((ino, directory, size))
    }

    fn acquire_lookup(&self, ino: INodeNo) {
        *self
            .inodes
            .lock()
            .unwrap()
            .lookups
            .entry(u64::from(ino))
            .or_default() += 1;
    }

    fn forget_inode(&self, ino: INodeNo, count: u64) {
        let ino = u64::from(ino);
        if ino == 1 {
            return;
        }
        let mut table = self.inodes.lock().unwrap();
        let remaining = table.lookups.entry(ino).or_default();
        *remaining = remaining.saturating_sub(count);
        Self::reclaim_inode(&mut table, ino);
    }

    fn reclaim_inode(table: &mut Inodes, ino: u64) {
        if table.lookups.get(&ino).copied().unwrap_or(0) != 0
            || table.opens.get(&ino).copied().unwrap_or(0) != 0
        {
            return;
        }
        table.lookups.remove(&ino);
        table.opens.remove(&ino);
        table.nodes.remove(&ino);
        table.keys.retain(|_, value| *value != ino);
    }

    fn reclaim_unreferenced(&self) {
        let mut table = self.inodes.lock().unwrap();
        let candidates = table
            .nodes
            .keys()
            .copied()
            .filter(|ino| *ino != 1)
            .collect::<Vec<_>>();
        for ino in candidates {
            Self::reclaim_inode(&mut table, ino);
        }
    }

    fn open_file(&self, ino: INodeNo, uri: ResourceUri, bytes: Vec<u8>) -> FileHandle {
        let ino = u64::from(ino);
        *self.inodes.lock().unwrap().opens.entry(ino).or_default() += 1;
        let mut files = self.open_files.lock().unwrap();
        let handle = files.next;
        files.next += 1;
        files.files.insert(
            handle,
            OpenFile {
                ino,
                uri,
                bytes,
                dirty: false,
            },
        );
        FileHandle(handle)
    }

    fn commit_file(&self, handle: FileHandle) -> Result<(), Errno> {
        let mut files = self.open_files.lock().map_err(Self::error)?;
        let Some(file) = files.files.get_mut(&u64::from(handle)) else {
            return Err(Errno::EBADF);
        };
        if !file.dirty {
            return Ok(());
        }
        let text = String::from_utf8(file.bytes.clone()).map_err(|_| Errno::EINVAL)?;
        self.runtime
            .block_on(self.router.handle(ResourceRequest::Write {
                uri: file.uri.clone(),
                text,
            }))
            .map_err(Self::error)?;
        file.dirty = false;
        Ok(())
    }

    fn apply_write(file: &mut OpenFile, offset: usize, data: &[u8]) {
        if file.bytes.len() < offset {
            file.bytes.resize(offset, 0);
        }
        if file.bytes.len() < offset + data.len() {
            file.bytes.resize(offset + data.len(), 0);
        }
        file.bytes[offset..offset + data.len()].copy_from_slice(data);
        file.dirty = true;
    }
    fn child_uri(&self, parent: &Node, name: &str) -> Option<ResourceUri> {
        match parent {
            Node::Root => None,
            Node::Scheme(scheme) if matches!(scheme.as_str(), "file" | "profiles" | "plugins") => {
                ResourceUri::resolve(&format!("{scheme}:///{name}"), Path::new("/")).ok()
            }
            Node::Scheme(scheme) => {
                ResourceUri::resolve(&format!("{scheme}://{name}/"), Path::new("/")).ok()
            }
            Node::Resource { uri, .. } if name.starts_with('?') => {
                uri.descend_projection(&name[1..]).ok()
            }
            Node::Resource { uri, .. } if !uri.projection_segments().is_empty() => {
                uri.descend_projection(name).ok()
            }
            Node::Resource { uri, .. } if name.contains('?') => {
                let (base, projection) = name.split_once('?')?;
                let child = if uri.as_url().scheme() == "file" {
                    let path = uri.file_path()?.join(base);
                    ResourceUri::resolve(&path.to_string_lossy(), Path::new("/")).ok()?
                } else {
                    let mut url = uri.as_url().clone();
                    url.path_segments_mut().ok()?.push(base);
                    ResourceUri::from_url(url).ok()?
                };
                child.descend_projection(projection).ok()
            }
            Node::Resource { uri, .. } if uri.as_url().scheme() == "file" => {
                let path = uri.file_path()?.join(name);
                ResourceUri::resolve(&path.to_string_lossy(), Path::new("/")).ok()
            }
            Node::Resource { uri, .. } => {
                let mut url = uri.as_url().clone();
                url.path_segments_mut().ok()?.push(name);
                ResourceUri::from_url(url).ok()
            }
        }
    }
    fn attr(ino: INodeNo, directory: bool, size: u64) -> FileAttr {
        FileAttr {
            ino,
            size,
            blocks: size.div_ceil(512),
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: if directory {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            perm: if directory { 0o755 } else { 0o644 },
            nlink: if directory { 2 } else { 1 },
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            flags: 0,
            blksize: 512,
        }
    }
    fn error(_: impl std::fmt::Display) -> Errno {
        Errno::EIO
    }
}

impl Filesystem for ResourceFs {
    fn lookup(&self, _: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let Some(parent) = self.node(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let Some(name) = name.to_str() else {
            reply.error(Errno::EINVAL);
            return;
        };
        if matches!(parent, Node::Root) {
            if !self.router.schemes().iter().any(|scheme| scheme == name) {
                reply.error(Errno::ENOENT);
                return;
            }
            let ino = self.inode(format!("scheme:{name}"), Node::Scheme(name.into()));
            self.acquire_lookup(ino);
            reply.entry(&TTL, &Self::attr(ino, true, 0), Generation(0));
            return;
        }
        let Some(uri) = self.child_uri(&parent, name) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.classify(uri) {
            Some((ino, dir, size)) => {
                self.acquire_lookup(ino);
                reply.entry(&TTL, &Self::attr(ino, dir, size), Generation(0));
            }
            None => reply.error(Errno::ENOENT),
        }
    }

    fn getattr(&self, _: &Request, ino: INodeNo, _: Option<FileHandle>, reply: ReplyAttr) {
        match self.node(ino) {
            Some(Node::Root | Node::Scheme(_)) => reply.attr(&TTL, &Self::attr(ino, true, 0)),
            Some(Node::Resource { uri, directory }) => {
                if directory {
                    reply.attr(&TTL, &Self::attr(ino, true, 0));
                    return;
                }
                match self
                    .runtime
                    .block_on(self.router.handle(ResourceRequest::Read {
                        uri,
                        start_line: None,
                        line_count: None,
                    })) {
                    Ok(ResourceReply::Text { text }) => {
                        reply.attr(&TTL, &Self::attr(ino, false, text.len() as u64))
                    }
                    _ => reply.error(Errno::ENOENT),
                }
            }
            None => reply.error(Errno::ENOENT),
        }
    }

    fn forget(&self, _: &Request, ino: INodeNo, nlookup: u64) {
        self.forget_inode(ino, nlookup);
    }

    fn setattr(
        &self,
        _: &Request,
        ino: INodeNo,
        _: Option<u32>,
        _: Option<u32>,
        _: Option<u32>,
        size: Option<u64>,
        _: Option<TimeOrNow>,
        _: Option<TimeOrNow>,
        _: Option<std::time::SystemTime>,
        fh: Option<FileHandle>,
        _: Option<std::time::SystemTime>,
        _: Option<std::time::SystemTime>,
        _: Option<std::time::SystemTime>,
        _: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let Some(Node::Resource {
            uri,
            directory: false,
        }) = self.node(ino)
        else {
            reply.error(Errno::EISDIR);
            return;
        };
        let Some(size) = size else {
            reply.attr(&TTL, &Self::attr(ino, false, 0));
            return;
        };
        if let Some(handle) = fh {
            let mut files = self.open_files.lock().unwrap();
            let Some(file) = files.files.get_mut(&u64::from(handle)) else {
                reply.error(Errno::EBADF);
                return;
            };
            file.bytes.resize(size as usize, 0);
            file.dirty = true;
            reply.attr(&TTL, &Self::attr(ino, false, size));
            return;
        }
        let mut bytes = match self
            .runtime
            .block_on(self.router.handle(ResourceRequest::Read {
                uri: uri.clone(),
                start_line: None,
                line_count: None,
            })) {
            Ok(ResourceReply::Text { text }) => text.into_bytes(),
            _ => Vec::new(),
        };
        bytes.resize(size as usize, 0);
        let Ok(text) = String::from_utf8(bytes) else {
            reply.error(Errno::EINVAL);
            return;
        };
        match self
            .runtime
            .block_on(self.router.handle(ResourceRequest::Write { uri, text }))
        {
            Ok(_) => reply.attr(&TTL, &Self::attr(ino, false, size)),
            Err(error) => reply.error(Self::error(error)),
        }
    }

    fn open(&self, _: &Request, ino: INodeNo, _: OpenFlags, reply: ReplyOpen) {
        let Some(Node::Resource {
            uri,
            directory: false,
        }) = self.node(ino)
        else {
            reply.error(Errno::EISDIR);
            return;
        };
        let bytes = match self
            .runtime
            .block_on(self.router.handle(ResourceRequest::Read {
                uri: uri.clone(),
                start_line: None,
                line_count: None,
            })) {
            Ok(ResourceReply::Text { text }) => text.into_bytes(),
            Ok(_) => {
                reply.error(Errno::EIO);
                return;
            }
            Err(error) => {
                reply.error(Self::error(error));
                return;
            }
        };
        let handle = self.open_file(ino, uri, bytes);
        reply.opened(handle, FopenFlags::FOPEN_DIRECT_IO);
    }

    fn create(
        &self,
        _: &Request,
        parent: INodeNo,
        name: &OsStr,
        _: u32,
        _: u32,
        _: i32,
        reply: ReplyCreate,
    ) {
        let (Some(parent), Some(name)) = (self.node(parent), name.to_str()) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let Some(uri) = self.child_uri(&parent, name) else {
            reply.error(Errno::EINVAL);
            return;
        };
        match self
            .runtime
            .block_on(self.router.handle(ResourceRequest::Write {
                uri: uri.clone(),
                text: String::new(),
            })) {
            Ok(_) => {
                let ino = self.inode(
                    uri.to_string(),
                    Node::Resource {
                        uri: uri.clone(),
                        directory: false,
                    },
                );
                self.acquire_lookup(ino);
                let handle = self.open_file(ino, uri, Vec::new());
                reply.created(
                    &TTL,
                    &Self::attr(ino, false, 0),
                    Generation(0),
                    handle,
                    FopenFlags::FOPEN_DIRECT_IO,
                );
            }
            Err(error) => reply.error(Self::error(error)),
        }
    }

    fn read(
        &self,
        _: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _: OpenFlags,
        _: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let files = self.open_files.lock().unwrap();
        let Some(file) = files
            .files
            .get(&u64::from(fh))
            .filter(|file| file.ino == u64::from(ino))
        else {
            reply.error(Errno::EBADF);
            return;
        };
        let start = (offset as usize).min(file.bytes.len());
        let end = (start + size as usize).min(file.bytes.len());
        reply.data(&file.bytes[start..end]);
    }

    fn write(
        &self,
        _: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _: WriteFlags,
        _: OpenFlags,
        _: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let mut files = self.open_files.lock().unwrap();
        let Some(file) = files
            .files
            .get_mut(&u64::from(fh))
            .filter(|file| file.ino == u64::from(ino))
        else {
            reply.error(Errno::EBADF);
            return;
        };
        Self::apply_write(file, offset as usize, data);
        reply.written(data.len() as u32);
    }

    fn flush(&self, _: &Request, _: INodeNo, fh: FileHandle, _: LockOwner, reply: ReplyEmpty) {
        match self.commit_file(fh) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }

    fn fsync(&self, _: &Request, _: INodeNo, fh: FileHandle, _: bool, reply: ReplyEmpty) {
        match self.commit_file(fh) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }

    fn release(
        &self,
        _: &Request,
        _: INodeNo,
        fh: FileHandle,
        _: OpenFlags,
        _: Option<LockOwner>,
        _: bool,
        reply: ReplyEmpty,
    ) {
        let commit = self.commit_file(fh);
        let file = self.open_files.lock().unwrap().files.remove(&u64::from(fh));
        if let Some(file) = file {
            let mut table = self.inodes.lock().unwrap();
            let opens = table.opens.entry(file.ino).or_default();
            *opens = opens.saturating_sub(1);
            Self::reclaim_inode(&mut table, file.ino);
        }
        match commit {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }

    fn readdir(
        &self,
        _: &Request,
        ino: INodeNo,
        _: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let Some(node) = self.node(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let mut entries = vec![
            (ino, FileType::Directory, ".".to_owned()),
            (INodeNo::ROOT, FileType::Directory, "..".to_owned()),
        ];
        match node {
            Node::Root => {
                for scheme in self.router.schemes() {
                    let child =
                        self.inode(format!("scheme:{scheme}"), Node::Scheme(scheme.clone()));
                    entries.push((child, FileType::Directory, scheme));
                }
            }
            Node::Scheme(scheme) => {
                if let Ok(root) = ResourceUri::resolve(&format!("{scheme}:///"), Path::new("/"))
                    && let Ok(ResourceReply::Children { children }) = self
                        .runtime
                        .block_on(self.router.handle(ResourceRequest::Children { uri: root }))
                {
                    for child in children {
                        if child.file_path().as_deref() == Some(self.mount.as_path()) {
                            continue;
                        }
                        if let Some((child_ino, directory, _)) = self.classify(child.clone()) {
                            let name = child
                                .as_url()
                                .host_str()
                                .filter(|host| !host.is_empty())
                                .or_else(|| {
                                    child
                                        .as_url()
                                        .path_segments()?
                                        .rfind(|segment| !segment.is_empty())
                                })
                                .unwrap_or_default()
                                .to_owned();
                            entries.push((
                                child_ino,
                                if directory {
                                    FileType::Directory
                                } else {
                                    FileType::RegularFile
                                },
                                name,
                            ));
                        }
                    }
                }
            }
            Node::Resource { uri, .. } => {
                if let Ok(ResourceReply::Children { children }) = self.runtime.block_on(
                    self.router
                        .handle(ResourceRequest::Children { uri: uri.clone() }),
                ) {
                    for child in children {
                        if let Some((child_ino, directory, _)) = self.classify(child.clone()) {
                            let base_name = child
                                .projection_segments()
                                .last()
                                .cloned()
                                .unwrap_or_else(|| {
                                    child
                                        .as_url()
                                        .path_segments()
                                        .and_then(|mut segments| {
                                            segments.rfind(|segment| !segment.is_empty())
                                        })
                                        .unwrap_or_default()
                                        .to_owned()
                                });
                            entries.push((
                                child_ino,
                                if directory {
                                    FileType::Directory
                                } else {
                                    FileType::RegularFile
                                },
                                base_name.clone(),
                            ));
                            for projection in if child.projection_segments().is_empty() {
                                self.router.projection_roots(&child)
                            } else {
                                Vec::new()
                            } {
                                if let Ok(projected) = child.descend_projection(&projection)
                                    && let Some((projected_ino, projected_directory, _)) =
                                        self.classify(projected)
                                {
                                    entries.push((
                                        projected_ino,
                                        if projected_directory {
                                            FileType::Directory
                                        } else {
                                            FileType::RegularFile
                                        },
                                        format!("{base_name}?{projection}"),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        for (index, (ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(ino, (index + 1) as u64, kind, name) {
                break;
            }
        }
        reply.ok();
        // Plain readdir does not acquire kernel lookup references. Drop any
        // directory-enumeration-only inode now; a later lookup recreates it
        // deterministically, while looked-up or open nodes remain pinned.
        self.reclaim_unreferenced();
    }

    fn unlink(&self, _: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.remove(parent, name, reply)
    }
    fn rmdir(&self, _: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.remove(parent, name, reply)
    }
    fn rename(
        &self,
        _: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        if !flags.is_empty() {
            reply.error(Errno::EINVAL);
            return;
        }
        let (Some(parent), Some(newparent), Some(name), Some(newname)) = (
            self.node(parent),
            self.node(newparent),
            name.to_str(),
            newname.to_str(),
        ) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let (Some(from), Some(to)) = (
            self.child_uri(&parent, name),
            self.child_uri(&newparent, newname),
        ) else {
            reply.error(Errno::EINVAL);
            return;
        };
        match self.runtime.block_on(
            self.router
                .handle(ResourceRequest::Move { from, to: Some(to) }),
        ) {
            Ok(_) => reply.ok(),
            Err(e) => reply.error(Self::error(e)),
        }
    }
}

impl ResourceFs {
    fn remove(&self, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let (Some(parent), Some(name)) = (self.node(parent), name.to_str()) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let Some(from) = self.child_uri(&parent, name) else {
            reply.error(Errno::EINVAL);
            return;
        };
        match self
            .runtime
            .block_on(self.router.handle(ResourceRequest::Move { from, to: None }))
        {
            Ok(_) => reply.ok(),
            Err(e) => reply.error(Self::error(e)),
        }
    }
}

pub fn fuse_path_to_uri(mount: &Path, path: &Path) -> Result<ResourceUri, String> {
    mount_path_to_uri(mount, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FilesystemProvider, ResourceError, ResourceOperation, ResourceProvider, ResourceRoute,
        uri_to_mount_path,
    };
    use async_trait::async_trait;
    use std::{
        process::Command,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[tokio::test]
    async fn inode_reclamation_respects_lookup_and_open_references() {
        let mount = tempfile::tempdir().unwrap();
        let fs = ResourceFs::new(
            ResourceRouter::new(),
            Handle::current(),
            mount.path().to_owned(),
        );
        let uri = ResourceUri::resolve("file:///dynamic", Path::new("/")).unwrap();
        let ino = fs.inode(
            uri.to_string(),
            Node::Resource {
                uri: uri.clone(),
                directory: false,
            },
        );
        fs.acquire_lookup(ino);
        let handle = fs.open_file(ino, uri, Vec::new());
        fs.forget_inode(ino, 1);
        assert!(fs.node(ino).is_some(), "open handle must pin the inode");

        let file = fs
            .open_files
            .lock()
            .unwrap()
            .files
            .remove(&u64::from(handle))
            .unwrap();
        let mut table = fs.inodes.lock().unwrap();
        *table.opens.get_mut(&file.ino).unwrap() -= 1;
        ResourceFs::reclaim_inode(&mut table, file.ino);
        assert!(!table.nodes.contains_key(&file.ino));
    }

    #[test]
    fn per_open_buffer_handles_overwrite_sparse_extension_and_utf8_validation() {
        let uri = ResourceUri::resolve("file:///buffer", Path::new("/")).unwrap();
        let mut file = OpenFile {
            ino: 2,
            uri,
            bytes: b"hello".to_vec(),
            dirty: false,
        };
        ResourceFs::apply_write(&mut file, 1, b"a");
        ResourceFs::apply_write(&mut file, 7, b"z");
        assert_eq!(file.bytes, b"hallo\0\0z");
        assert!(file.dirty);
        file.bytes = vec![0xff];
        assert!(String::from_utf8(file.bytes).is_err());
    }

    struct CountingWrites {
        writes: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ResourceProvider for CountingWrites {
        async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
            match request {
                ResourceRequest::Write { .. } => {
                    self.writes.fetch_add(1, Ordering::AcqRel);
                    Ok(ResourceReply::Written)
                }
                other => Err(ResourceError::Unsupported {
                    uri: other.uri().clone(),
                    operation: other.operation(),
                }),
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn repeated_flushes_commit_each_dirty_buffer_only_once() {
        let writes = Arc::new(AtomicUsize::new(0));
        let router = ResourceRouter::new();
        router
            .register(
                "writes",
                ResourceRoute::new("file:///**", None::<String>, [ResourceOperation::Write]),
                Arc::new(CountingWrites {
                    writes: writes.clone(),
                }),
            )
            .await
            .unwrap();
        let mount = tempfile::tempdir().unwrap();
        let fs = Arc::new(ResourceFs::new(
            router,
            Handle::current(),
            mount.path().to_owned(),
        ));
        let uri = ResourceUri::resolve("file:///buffer", Path::new("/")).unwrap();
        let ino = fs.inode(
            uri.to_string(),
            Node::Resource {
                uri: uri.clone(),
                directory: false,
            },
        );
        let handle = fs.open_file(ino, uri, b"old".to_vec());
        {
            let mut files = fs.open_files.lock().unwrap();
            ResourceFs::apply_write(files.files.get_mut(&u64::from(handle)).unwrap(), 0, b"new");
        }
        tokio::task::spawn_blocking(move || {
            fs.commit_file(handle).unwrap();
            fs.commit_file(handle).unwrap();
        })
        .await
        .unwrap();
        assert_eq!(writes.load(Ordering::Acquire), 1);
    }

    struct Symbols;
    #[async_trait]
    impl ResourceProvider for Symbols {
        async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
            match request {
                ResourceRequest::Children { uri } if uri.projection_segments().len() == 1 => {
                    Ok(ResourceReply::Children {
                        children: vec![uri.descend_projection("foo").unwrap()],
                    })
                }
                ResourceRequest::Read { uri, .. } => Ok(ResourceReply::Text {
                    text: format!("symbol at {uri}\n"),
                }),
                other => Err(ResourceError::Unsupported {
                    uri: other.uri().clone(),
                    operation: other.operation(),
                }),
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires a usable /dev/fuse"]
    async fn ordinary_io_and_query_projection_are_visible_to_unix() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("rust.rs");
        std::fs::write(&source, "fn foo() {}\n").unwrap();
        let router = ResourceRouter::new();
        let filesystem: Arc<dyn ResourceProvider> = Arc::new(FilesystemProvider::new("/"));
        for operation in [
            ResourceOperation::Read,
            ResourceOperation::Children,
            ResourceOperation::Write,
            ResourceOperation::Edit,
            ResourceOperation::Move,
        ] {
            router
                .register(
                    format!("file.{operation}"),
                    ResourceRoute::new("file:///**", None::<String>, [operation]),
                    filesystem.clone(),
                )
                .await
                .unwrap();
        }
        router
            .register(
                "symbols",
                ResourceRoute::new(
                    "file:///**/*.rs",
                    Some("symbols/**"),
                    [ResourceOperation::Read, ResourceOperation::Children],
                ),
                Arc::new(Symbols),
            )
            .await
            .unwrap();
        let mount = FuseMount::mount(router, Handle::current()).unwrap();
        let uri = ResourceUri::resolve(&source.to_string_lossy(), Path::new("/")).unwrap();
        let projected_cwd = uri_to_mount_path(
            mount.root(),
            &ResourceUri::resolve(&temp.path().to_string_lossy(), Path::new("/")).unwrap(),
        )
        .unwrap();
        let shell = Command::new("sh")
            .args(["-c", "printf '%s\n%s' \"$PWD\" \"$ARTIST_ROOT\""])
            .current_dir(&projected_cwd)
            .env("ARTIST_ROOT", mount.root())
            .output()
            .unwrap();
        assert!(shell.status.success());
        assert_eq!(
            String::from_utf8(shell.stdout).unwrap(),
            format!("{}\n{}", projected_cwd.display(), mount.root().display())
        );
        assert_eq!(
            std::fs::read_to_string(uri_to_mount_path(mount.root(), &uri).unwrap()).unwrap(),
            "fn foo() {}\n"
        );
        let projected = uri.descend_projection("symbols").unwrap();
        let children = std::fs::read_dir(uri_to_mount_path(mount.root(), &projected).unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(children, ["foo"]);

        let created = source.with_file_name("created.txt");
        let created_uri = ResourceUri::resolve(&created.to_string_lossy(), Path::new("/")).unwrap();
        let created_mount = uri_to_mount_path(mount.root(), &created_uri).unwrap();
        std::fs::write(&created_mount, "written through fuse\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&created).unwrap(),
            "written through fuse\n"
        );
        let moved = source.with_file_name("moved.txt");
        let moved_uri = ResourceUri::resolve(&moved.to_string_lossy(), Path::new("/")).unwrap();
        let moved_mount = uri_to_mount_path(mount.root(), &moved_uri).unwrap();
        std::fs::rename(&created_mount, &moved_mount).unwrap();
        assert!(moved.exists());
        std::fs::remove_file(moved_mount).unwrap();
        assert!(!moved.exists());
    }
}
