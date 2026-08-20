//! Host glue for the noun contract.
//!
//! The kernel speaks inos; the wasm contract speaks paths. [`WasmNamespace`]
//! implements the kernel [`artist_kernel::Namespace`] surface, translating inos
//! to canonical paths and delegating metadata to a [`NamespaceGuest`].
//!
//! The guest side is deliberately a trait rather than the concrete bindgen
//! types: the bindgen adapter (which wraps a real instantiated component) is
//! written in the vertical slice, while tests here drive [`WasmNamespace`]
//! against a Rust fake implementing [`NamespaceGuest`].

use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use artist_kernel::namespace::Namespace;
use artist_kernel::provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
use artist_kernel::uri::ResourceUri;
use artist_kernel::vfs::{Attrs, DirEntry, Ino, NodeKind, VfsError};
use async_trait::async_trait;

use crate::bindings::ns;

/// The guest side of the noun contract: path-based, store-free.
///
/// A real component implementing `artist:nouns/namespace` satisfies this via
/// the bindgen adapter. Tests implement it directly.
#[async_trait]
pub trait NamespaceGuest: Send + Sync {
    async fn getattr(&self, path: &[String]) -> Result<ns::Attrs, ns::Error>;
    async fn readdir(&self, path: &[String]) -> Result<Vec<ns::Entry>, ns::Error>;
    async fn read(&self, path: &[String], offset: u64, size: u32) -> Result<Vec<u8>, ns::Error>;
}

#[derive(Debug)]
struct Node {
    /// Canonical path components from the namespace root.
    path: Vec<String>,
    parent: Option<Ino>,
    name: Option<String>,
}

#[derive(Debug, Default)]
struct PathTable {
    by_ino: HashMap<Ino, Node>,
    next: u64,
}

impl PathTable {
    fn path_of(&self, ino: Ino) -> Option<&Vec<String>> {
        self.by_ino.get(&ino).map(|n| &n.path)
    }

    fn parent_of(&self, ino: Ino) -> Option<Ino> {
        self.by_ino.get(&ino).and_then(|n| n.parent)
    }

    fn contains(&self, ino: Ino) -> bool {
        self.by_ino.contains_key(&ino)
    }

    /// Find the ino of an existing child of `parent` named `name`.
    fn child(&self, parent: Ino, name: &str) -> Option<Ino> {
        self.by_ino.iter().find_map(|(ino, node)| {
            if node.parent == Some(parent) && node.name.as_deref() == Some(name) {
                Some(*ino)
            } else {
                None
            }
        })
    }

    /// Allocate an ino for a node, or return the existing one.
    fn allocate(&mut self, parent: Option<Ino>, name: String) -> Ino {
        if let Some(parent) = parent {
            if let Some(ino) = self.child(parent, &name) {
                return ino;
            }
            let path = {
                let parent_path = self.path_of(parent);
                let mut path = parent_path.cloned().unwrap_or_default();
                path.push(name.clone());
                path
            };
            let ino = Ino(self.next);
            self.next += 1;
            self.by_ino.insert(
                ino,
                Node {
                    path,
                    parent: Some(parent),
                    name: Some(name),
                },
            );
            ino
        } else {
            let ino = Ino(self.next);
            self.next += 1;
            self.by_ino.insert(
                ino,
                Node {
                    path: vec![name.clone()],
                    parent: None,
                    name: Some(name),
                },
            );
            ino
        }
    }
}

/// A kernel namespace backed by a wasm noun extension.
pub struct WasmNamespace {
    name: String,
    root_ino: Ino,
    table: Mutex<PathTable>,
    guest: Box<dyn NamespaceGuest>,
}

impl WasmNamespace {
    pub fn new(name: impl Into<String>, guest: Box<dyn NamespaceGuest>) -> Self {
        let table = PathTable {
            next: Ino::ROOT.0 + 1,
            by_ino: HashMap::from([(
                Ino::ROOT,
                Node {
                    path: Vec::new(),
                    parent: None,
                    name: None,
                },
            )]),
        };
        Self {
            name: name.into(),
            root_ino: Ino(0),
            table: Mutex::new(table),
            guest,
        }
    }

    fn root(&self) -> Ino {
        let table = self.table.lock().unwrap();
        table
            .by_ino
            .iter()
            .find(|(_, n)| n.parent.is_none())
            .map(|(ino, _)| *ino)
            .unwrap_or(Ino::ROOT)
    }
}

fn guest_attrs_to_kernel(ino: Ino, attrs: &ns::Attrs) -> Attrs {
    let mtime = UNIX_EPOCH + std::time::Duration::new(attrs.mtime_secs, attrs.mtime_nsecs);
    Attrs {
        ino,
        kind: match attrs.kind {
            ns::Kind::Directory => NodeKind::Directory,
            ns::Kind::File => NodeKind::File,
        },
        size: attrs.size,
        perm: 0o555,
        nlink: 1,
        uid: 0,
        gid: 0,
        atime: SystemTime::now(),
        mtime,
        ctime: mtime,
    }
}

fn guest_error_to_kernel(err: ns::Error) -> VfsError {
    match err {
        ns::Error::NotFound => VfsError::NotFound,
        ns::Error::NotDir => VfsError::NotDir,
        ns::Error::IsDir => VfsError::IsDir,
        ns::Error::Io => VfsError::Io,
    }
}

fn guest_error_to_resource(err: ns::Error) -> ResourceError {
    let (code, message) = match err {
        ns::Error::NotFound => (ResourceErrorCode::NotFound, "resource not found"),
        ns::Error::NotDir => (ResourceErrorCode::NotDir, "resource is not a directory"),
        ns::Error::IsDir => (ResourceErrorCode::IsDir, "resource is a directory"),
        ns::Error::Io => (ResourceErrorCode::Io, "guest I/O failure"),
    };
    ResourceError::new(code, message)
}

fn guest_attrs_to_provider(attrs: &ns::Attrs) -> ProviderAttrs {
    let mtime = UNIX_EPOCH + std::time::Duration::new(attrs.mtime_secs, attrs.mtime_nsecs);
    let mut result = match attrs.kind {
        ns::Kind::Directory => ProviderAttrs::directory(),
        ns::Kind::File => ProviderAttrs::file(attrs.size, mtime),
    };
    result.mtime = mtime;
    result.ctime = mtime;
    result.atime = mtime;
    result
}

fn uri_path(uri: &ResourceUri) -> Vec<String> {
    uri.segments().map(str::to_owned).collect()
}

#[async_trait]
impl ResourceProvider for WasmNamespace {
    fn provider_name(&self) -> &str {
        &self.name
    }

    fn claims(&self, uri: &ResourceUri) -> bool {
        uri.scheme() == self.name && uri.authority().is_empty()
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        if !self.claims(uri) {
            return Err(ResourceError::not_found(uri));
        }
        self.guest
            .getattr(&uri_path(uri))
            .await
            .map(|attrs| guest_attrs_to_provider(&attrs))
            .map_err(guest_error_to_resource)
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        if !self.claims(uri) {
            return Err(ResourceError::not_found(uri));
        }
        self.guest
            .readdir(&uri_path(uri))
            .await
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| ProviderEntry {
                        name: entry.name,
                        attrs: guest_attrs_to_provider(&ns::Attrs {
                            kind: entry.kind,
                            size: entry.size,
                            mtime_secs: 0,
                            mtime_nsecs: 0,
                        }),
                    })
                    .collect()
            })
            .map_err(guest_error_to_resource)
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        if !self.claims(uri) {
            return Err(ResourceError::not_found(uri));
        }
        self.guest
            .read(&uri_path(uri), offset, size)
            .await
            .map_err(guest_error_to_resource)
    }
}

#[async_trait]
impl Namespace for WasmNamespace {
    fn name(&self) -> &str {
        &self.name
    }

    fn set_root_ino(&mut self, ino: Ino) {
        self.root_ino = ino;
        let mut table = self.table.lock().unwrap();
        table.next = ino.0 + 1;
        if let Some(node) = table.by_ino.remove(&Ino::ROOT) {
            table.by_ino.insert(ino, node);
        }
    }

    fn root_ino(&self) -> Ino {
        if self.root_ino.0 != 0 {
            self.root_ino
        } else {
            self.root()
        }
    }

    fn owns(&self, ino: Ino) -> bool {
        ino == self.root_ino() || self.table.lock().unwrap().contains(ino)
    }

    async fn lookup(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        let name = name.to_str().ok_or(VfsError::NotFound)?;
        let path = {
            let table = self.table.lock().unwrap();
            table.path_of(parent).cloned().ok_or(VfsError::NotFound)?
        };
        let child_path = {
            let mut p = path.clone();
            p.push(name.to_string());
            p
        };
        let attrs = self
            .guest
            .getattr(&child_path)
            .await
            .map_err(guest_error_to_kernel)?;
        let ino = {
            let mut table = self.table.lock().unwrap();
            table.allocate(Some(parent), name.to_string())
        };
        Ok(guest_attrs_to_kernel(ino, &attrs))
    }

    async fn getattr(&self, ino: Ino) -> Result<Attrs, VfsError> {
        let path = {
            let table = self.table.lock().unwrap();
            table.path_of(ino).cloned().ok_or(VfsError::NotFound)?
        };
        let attrs = self
            .guest
            .getattr(&path)
            .await
            .map_err(guest_error_to_kernel)?;
        Ok(guest_attrs_to_kernel(ino, &attrs))
    }

    async fn readdir(&self, ino: Ino) -> Result<Vec<DirEntry>, VfsError> {
        let path = {
            let table = self.table.lock().unwrap();
            table.path_of(ino).cloned().ok_or(VfsError::NotFound)?
        };
        let entries = self
            .guest
            .readdir(&path)
            .await
            .map_err(guest_error_to_kernel)?;
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let kind = match entry.kind {
                ns::Kind::Directory => NodeKind::Directory,
                ns::Kind::File => NodeKind::File,
            };
            let child_ino = {
                let mut table = self.table.lock().unwrap();
                table.allocate(Some(ino), entry.name.clone())
            };
            out.push(DirEntry {
                ino: child_ino,
                kind,
                name: entry.name.into(),
            });
        }
        Ok(out)
    }

    async fn read(&self, ino: Ino, offset: u64, size: u32) -> Result<Vec<u8>, VfsError> {
        let path = {
            let table = self.table.lock().unwrap();
            table.path_of(ino).cloned().ok_or(VfsError::NotFound)?
        };
        self.guest
            .read(&path, offset, size)
            .await
            .map_err(guest_error_to_kernel)
    }

    async fn parent(&self, ino: Ino) -> Option<Ino> {
        if ino == self.root_ino() {
            return Some(Ino::ROOT);
        }
        let table = self.table.lock().unwrap();
        table.parent_of(ino)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_kernel::vfs::Vfs;

    struct FakeGuest {
        attrs: HashMap<Vec<String>, ns::Attrs>,
        entries: HashMap<Vec<String>, Vec<ns::Entry>>,
        contents: HashMap<Vec<String>, Vec<u8>>,
    }

    #[async_trait]
    impl NamespaceGuest for FakeGuest {
        async fn getattr(&self, path: &[String]) -> Result<ns::Attrs, ns::Error> {
            self.attrs.get(path).cloned().ok_or(ns::Error::NotFound)
        }

        async fn readdir(&self, path: &[String]) -> Result<Vec<ns::Entry>, ns::Error> {
            self.entries.get(path).cloned().ok_or(ns::Error::NotFound)
        }

        async fn read(
            &self,
            path: &[String],
            _offset: u64,
            _size: u32,
        ) -> Result<Vec<u8>, ns::Error> {
            self.contents.get(path).cloned().ok_or(ns::Error::NotFound)
        }
    }

    fn p(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn ns_attrs(kind: ns::Kind, size: u64) -> ns::Attrs {
        ns::Attrs {
            kind,
            size,
            mtime_secs: 0,
            mtime_nsecs: 0,
        }
    }

    #[tokio::test]
    async fn mount_lookup_readdir_read() {
        let mut attrs = HashMap::new();
        attrs.insert(p(&[]), ns_attrs(ns::Kind::Directory, 0));
        attrs.insert(p(&["version"]), ns_attrs(ns::Kind::File, 7));
        attrs.insert(p(&["sub"]), ns_attrs(ns::Kind::Directory, 0));

        let mut entries = HashMap::new();
        entries.insert(
            p(&[]),
            vec![
                ns::Entry {
                    name: "version".into(),
                    kind: ns::Kind::File,
                    size: 7,
                },
                ns::Entry {
                    name: "sub".into(),
                    kind: ns::Kind::Directory,
                    size: 0,
                },
            ],
        );
        entries.insert(p(&["sub"]), vec![]);

        let mut contents = HashMap::new();
        contents.insert(p(&["version"]), b"0.1.0\n".to_vec());

        let guest = Box::new(FakeGuest {
            attrs,
            entries,
            contents,
        });
        let ns = WasmNamespace::new("demo", guest);

        let kernel = artist_kernel::Kernel::new();
        let root = kernel.register(ns);

        let v = kernel.getattr(root).await.unwrap();
        assert_eq!(v.kind, NodeKind::Directory);

        let children = kernel.readdir(root).await.unwrap();
        assert_eq!(children.len(), 2);

        let version = kernel.lookup(root, OsStr::new("version")).await.unwrap();
        assert_eq!(version.kind, NodeKind::File);
        assert_eq!(version.size, 7);

        let data = kernel.read(version.ino, 0, 8).await.unwrap();
        assert_eq!(data, b"0.1.0\n");

        assert_eq!(kernel.parent(version.ino).await, Some(root));
    }

    #[tokio::test]
    async fn missing_lookup_is_not_found() {
        let mut attrs = HashMap::new();
        attrs.insert(p(&[]), ns_attrs(ns::Kind::Directory, 0));
        let guest = Box::new(FakeGuest {
            attrs,
            entries: HashMap::new(),
            contents: HashMap::new(),
        });
        let ns = WasmNamespace::new("demo", guest);
        let kernel = artist_kernel::Kernel::new();
        let root = kernel.register(ns);

        let err = kernel.lookup(root, OsStr::new("nope")).await.unwrap_err();
        assert_eq!(err, VfsError::NotFound);
    }
}
