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
}

struct ResourceFs {
    router: ResourceRouter,
    runtime: Handle,
    mount: PathBuf,
    inodes: Mutex<Inodes>,
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
        let directory = self
            .runtime
            .block_on(
                self.router
                    .handle(ResourceRequest::Children { uri: uri.clone() }),
            )
            .is_ok();
        let size = if directory {
            0
        } else {
            match self
                .runtime
                .block_on(self.router.handle(ResourceRequest::Read {
                    uri: uri.clone(),
                    start_line: None,
                    line_count: None,
                })) {
                Ok(ResourceReply::Text { text }) => text.len() as u64,
                _ => return None,
            }
        };
        let ino = self.inode(uri.to_string(), Node::Resource { uri, directory });
        Some((ino, directory, size))
    }
    fn child_uri(&self, parent: &Node, name: &str) -> Option<ResourceUri> {
        match parent {
            Node::Root => None,
            Node::Scheme(scheme) if scheme == "file" => {
                ResourceUri::resolve(&format!("file:///{name}"), Path::new("/")).ok()
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
            if !self
                .runtime
                .block_on(self.router.schemes())
                .iter()
                .any(|scheme| scheme == name)
            {
                reply.error(Errno::ENOENT);
                return;
            }
            let ino = self.inode(format!("scheme:{name}"), Node::Scheme(name.into()));
            reply.entry(&TTL, &Self::attr(ino, true, 0), Generation(0));
            return;
        }
        let Some(uri) = self.child_uri(&parent, name) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.classify(uri) {
            Some((ino, dir, size)) => reply.entry(&TTL, &Self::attr(ino, dir, size), Generation(0)),
            None => reply.error(Errno::ENOENT),
        }
    }

    fn getattr(&self, _: &Request, ino: INodeNo, _: Option<FileHandle>, reply: ReplyAttr) {
        match self.node(ino) {
            Some(Node::Root | Node::Scheme(_)) => reply.attr(&TTL, &Self::attr(ino, true, 0)),
            Some(Node::Resource { uri, directory }) => {
                let size = if directory {
                    0
                } else {
                    match self
                        .runtime
                        .block_on(self.router.handle(ResourceRequest::Read {
                            uri,
                            start_line: None,
                            line_count: None,
                        })) {
                        Ok(ResourceReply::Text { text }) => text.len() as u64,
                        _ => 0,
                    }
                };
                reply.attr(&TTL, &Self::attr(ino, directory, size))
            }
            None => reply.error(Errno::ENOENT),
        }
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
        _: Option<FileHandle>,
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

    fn open(&self, _: &Request, _: INodeNo, _: OpenFlags, reply: ReplyOpen) {
        reply.opened(FileHandle(0), FopenFlags::FOPEN_DIRECT_IO);
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
                        uri,
                        directory: false,
                    },
                );
                reply.created(
                    &TTL,
                    &Self::attr(ino, false, 0),
                    Generation(0),
                    FileHandle(0),
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
        _: FileHandle,
        offset: u64,
        size: u32,
        _: OpenFlags,
        _: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let Some(Node::Resource { uri, .. }) = self.node(ino) else {
            reply.error(Errno::EISDIR);
            return;
        };
        match self
            .runtime
            .block_on(self.router.handle(ResourceRequest::Read {
                uri,
                start_line: None,
                line_count: None,
            })) {
            Ok(ResourceReply::Text { text }) => {
                let bytes = text.as_bytes();
                let start = (offset as usize).min(bytes.len());
                let end = (start + size as usize).min(bytes.len());
                reply.data(&bytes[start..end]);
            }
            Ok(_) => reply.error(Errno::EIO),
            Err(e) => reply.error(Self::error(e)),
        }
    }

    fn write(
        &self,
        _: &Request,
        ino: INodeNo,
        _: FileHandle,
        offset: u64,
        data: &[u8],
        _: WriteFlags,
        _: OpenFlags,
        _: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let Some(Node::Resource { uri, .. }) = self.node(ino) else {
            reply.error(Errno::EISDIR);
            return;
        };
        let mut text = match self
            .runtime
            .block_on(self.router.handle(ResourceRequest::Read {
                uri: uri.clone(),
                start_line: None,
                line_count: None,
            })) {
            Ok(ResourceReply::Text { text }) => text.into_bytes(),
            _ => Vec::new(),
        };
        let offset = offset as usize;
        if text.len() < offset {
            text.resize(offset, 0);
        }
        if text.len() < offset + data.len() {
            text.resize(offset + data.len(), 0);
        }
        text[offset..offset + data.len()].copy_from_slice(data);
        let Ok(text) = String::from_utf8(text) else {
            reply.error(Errno::EINVAL);
            return;
        };
        match self
            .runtime
            .block_on(self.router.handle(ResourceRequest::Write { uri, text }))
        {
            Ok(_) => reply.written(data.len() as u32),
            Err(e) => reply.error(Self::error(e)),
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
                for scheme in self.runtime.block_on(self.router.schemes()) {
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
                                self.runtime.block_on(self.router.projection_roots(&child))
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
    use std::{process::Command, sync::Arc};

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
        Arc::new(FilesystemProvider::new("/"))
            .register(&router, "file")
            .await
            .unwrap();
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
