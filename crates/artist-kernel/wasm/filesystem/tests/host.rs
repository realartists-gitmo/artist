//! Host-only tests: drive the filesystem host's descriptor model, path
//! resolution, conversions, and read logic against an in-memory Rust fake of
//! the kernel [`Vfs`], plus a smoke test that the linker wiring type-checks.
//!
//! No guest wasm component is compiled or executed here.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use artist_kernel::vfs::{Attrs, DirEntry, Ino, NodeKind, Vfs, VfsError};
use artist_wasm_filesystem::descriptor;
use artist_wasm_filesystem::error::vfs_error_to_code;
use artist_wasm_filesystem::{FilesystemCtx, FilesystemCtxView, FilesystemView};
use async_trait::async_trait;
use wasmtime::component::{Linker, ResourceTable};
use wasmtime::{Config, Engine};

use artist_wasm_filesystem::bindings::types::{DescriptorType, ErrorCode};

// ---------------------------------------------------------------------------
// Fake Vfs
// ---------------------------------------------------------------------------

struct FakeNode {
    attrs: Attrs,
    children: HashMap<String, Ino>,
    data: Vec<u8>,
    parent: Option<Ino>,
}

#[derive(Default)]
struct FakeVfs {
    nodes: HashMap<Ino, FakeNode>,
}

fn attrs(ino: Ino, kind: NodeKind, size: u64) -> Attrs {
    let now = SystemTime::now();
    Attrs {
        ino,
        kind,
        size,
        perm: 0o555,
        nlink: 1,
        uid: 0,
        gid: 0,
        atime: now,
        mtime: now,
        ctime: now,
    }
}

impl FakeVfs {
    fn dir(&mut self, ino: Ino, parent: Option<Ino>) {
        self.nodes.insert(
            ino,
            FakeNode {
                attrs: attrs(ino, NodeKind::Directory, 0),
                children: HashMap::new(),
                data: Vec::new(),
                parent,
            },
        );
    }

    fn file(&mut self, ino: Ino, parent: Option<Ino>, data: impl Into<Vec<u8>>) {
        let data = data.into();
        let size = data.len() as u64;
        self.nodes.insert(
            ino,
            FakeNode {
                attrs: attrs(ino, NodeKind::File, size),
                children: HashMap::new(),
                data,
                parent,
            },
        );
    }

    fn link(&mut self, parent: Ino, name: &str, child: Ino) {
        self.nodes
            .get_mut(&parent)
            .unwrap()
            .children
            .insert(name.to_string(), child);
    }

    /// A small tree:
    ///
    /// ```text
    /// / (1)
    ///   ├── hello.txt (2)     "hello world\n"
    ///   └── sub (3)
    ///        └── nested.txt (4) "nested"
    /// ```
    fn sample() -> Self {
        let mut fs = FakeVfs::default();
        fs.dir(Ino(1), None);
        fs.file(Ino(2), Some(Ino(1)), "hello world\n");
        fs.dir(Ino(3), Some(Ino(1)));
        fs.file(Ino(4), Some(Ino(3)), "nested");
        fs.link(Ino(1), "hello.txt", Ino(2));
        fs.link(Ino(1), "sub", Ino(3));
        fs.link(Ino(3), "nested.txt", Ino(4));
        fs
    }
}

#[async_trait]
impl Vfs for FakeVfs {
    async fn lookup(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        let node = self.nodes.get(&parent).ok_or(VfsError::NotFound)?;
        let name = name.to_str().ok_or(VfsError::NotFound)?;
        let child = node.children.get(name).ok_or(VfsError::NotFound)?;
        Ok(self.nodes.get(child).unwrap().attrs.clone())
    }

    async fn getattr(&self, ino: Ino) -> Result<Attrs, VfsError> {
        self.nodes
            .get(&ino)
            .map(|n| n.attrs.clone())
            .ok_or(VfsError::NotFound)
    }

    async fn readdir(&self, ino: Ino) -> Result<Vec<DirEntry>, VfsError> {
        let node = self.nodes.get(&ino).ok_or(VfsError::NotFound)?;
        Ok(node
            .children
            .iter()
            .map(|(name, &cino)| {
                let child = self.nodes.get(&cino).unwrap();
                DirEntry {
                    ino: cino,
                    kind: child.attrs.kind,
                    name: OsString::from(name),
                }
            })
            .collect())
    }

    async fn read(&self, ino: Ino, offset: u64, size: u32) -> Result<Vec<u8>, VfsError> {
        let node = self.nodes.get(&ino).ok_or(VfsError::NotFound)?;
        let start = offset as usize;
        if start >= node.data.len() {
            return Ok(Vec::new());
        }
        let end = (start + size as usize).min(node.data.len());
        Ok(node.data[start..end].to_vec())
    }

    async fn parent(&self, ino: Ino) -> Option<Ino> {
        self.nodes.get(&ino).and_then(|n| n.parent)
    }
}

// ---------------------------------------------------------------------------
// Descriptor model
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_relative_paths() {
    let fs = FakeVfs::sample();
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "sub/nested.txt", false).await,
        Ok(Ino(4))
    );
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "hello.txt", false).await,
        Ok(Ino(2))
    );
    // Trailing slash and doubled separators are tolerated.
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "sub/", false).await,
        Ok(Ino(3))
    );
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "sub//nested.txt", false).await,
        Ok(Ino(4))
    );
}

#[tokio::test]
async fn resolve_dot_and_dotdot() {
    let fs = FakeVfs::sample();
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "sub/../hello.txt", false).await,
        Ok(Ino(2))
    );
    assert_eq!(
        descriptor::resolve(&fs, Ino(3), "../sub/../hello.txt", false).await,
        Ok(Ino(2))
    );
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "./sub/./nested.txt", false).await,
        Ok(Ino(4))
    );
}

#[tokio::test]
async fn resolve_rejects_absolute_and_escape() {
    let fs = FakeVfs::sample();
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "/etc", false).await,
        Err(ErrorCode::NotPermitted)
    );
    // `..` above the root has no parent.
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "..", false).await,
        Err(ErrorCode::NotPermitted)
    );
}

#[tokio::test]
async fn resolve_missing_component() {
    let fs = FakeVfs::sample();
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "nope", false).await,
        Err(ErrorCode::NoEntry)
    );
    assert_eq!(
        descriptor::resolve(&fs, Ino(1), "sub/missing", false).await,
        Err(ErrorCode::NoEntry)
    );
}

#[tokio::test]
async fn stat_and_type_conversion() {
    let fs = FakeVfs::sample();
    let stat = descriptor::stat(&fs, Ino(2)).await.unwrap();
    assert_eq!(stat.type_, DescriptorType::RegularFile);
    assert_eq!(stat.size, 12); // "hello world\n"
    assert_eq!(stat.link_count, 1);
    assert!(stat.data_access_timestamp.is_some());
    assert!(stat.data_modification_timestamp.is_some());

    let ty = descriptor::get_type(&fs, Ino(3)).await.unwrap();
    assert_eq!(ty, DescriptorType::Directory);
}

#[tokio::test]
async fn read_chunk_honors_offset_and_eof() {
    let fs = FakeVfs::sample();
    assert_eq!(
        descriptor::read_chunk(&fs, Ino(2), 0, 5).await.unwrap(),
        b"hello"
    );
    assert_eq!(
        descriptor::read_chunk(&fs, Ino(2), 6, 32).await.unwrap(),
        b"world\n"
    );
    assert_eq!(
        descriptor::read_chunk(&fs, Ino(2), 12, 32).await.unwrap(),
        b""
    );
}

#[tokio::test]
async fn read_dir_entries_translates() {
    let fs = FakeVfs::sample();
    let entries = descriptor::read_dir_entries(&fs, Ino(1)).await.unwrap();
    let mut by_name: HashMap<String, DescriptorType> =
        entries.into_iter().map(|e| (e.name, e.type_)).collect();
    assert_eq!(
        by_name.remove("hello.txt"),
        Some(DescriptorType::RegularFile)
    );
    assert_eq!(by_name.remove("sub"), Some(DescriptorType::Directory));
    assert!(by_name.is_empty());
}

#[tokio::test]
async fn metadata_hash_is_stable_and_distinct() {
    let fs = FakeVfs::sample();
    let a = fs.getattr(Ino(2)).await.unwrap();
    let h1 = descriptor::metadata_hash(Ino(2), &a);
    let h2 = descriptor::metadata_hash(Ino(2), &a);
    assert_eq!(h1, h2);

    let b = fs.getattr(Ino(4)).await.unwrap();
    let h3 = descriptor::metadata_hash(Ino(4), &b);
    assert_ne!(h1, h3);
}

#[test]
fn error_code_mapping() {
    assert_eq!(vfs_error_to_code(VfsError::NotFound), ErrorCode::NoEntry);
    assert_eq!(vfs_error_to_code(VfsError::NotDir), ErrorCode::NotDirectory);
    assert_eq!(vfs_error_to_code(VfsError::IsDir), ErrorCode::IsDirectory);
    assert_eq!(vfs_error_to_code(VfsError::Io), ErrorCode::Io);
}

#[test]
fn system_time_conversion() {
    let t = UNIX_EPOCH + std::time::Duration::new(5, 42);
    let instant = descriptor::system_time_to_instant(t).unwrap();
    assert_eq!(instant.seconds, 5);
    assert_eq!(instant.nanoseconds, 42);

    // Before the epoch maps to `none`.
    assert!(
        descriptor::system_time_to_instant(UNIX_EPOCH - std::time::Duration::new(1, 0)).is_none()
    );
}

// ---------------------------------------------------------------------------
// Linker wiring
// ---------------------------------------------------------------------------

struct State {
    ctx: FilesystemCtx,
    table: ResourceTable,
}

impl FilesystemView for State {
    fn filesystem(&mut self) -> FilesystemCtxView<'_> {
        FilesystemCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

#[test]
fn linker_wiring_type_checks() {
    let mut config = Config::new();
    config.wasm_component_model_async(true);
    let engine = Engine::new(&config).unwrap();

    let mut linker = Linker::<State>::new(&engine);
    artist_wasm_filesystem::add_to_linker(&mut linker).expect("add_to_linker should succeed");
}

#[test]
fn ctx_builds_and_preopens() {
    let mut ctx = FilesystemCtx::new(Arc::new(FakeVfs::sample()));
    ctx.preopen(Ino(1), "/");
    ctx.preopen(Ino(3), "/sub");
    assert_eq!(ctx.preopens().len(), 2);
    assert_eq!(ctx.preopens()[1].1, "/sub");
}
