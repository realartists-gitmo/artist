use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File, ReadDir};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use async_trait::async_trait;

use crate::namespace::Namespace;
use crate::provider::{
    ProviderAttrs, ProviderEntry, ResourceError, ResourceErrorCode, ResourceProvider,
};
use crate::uri::ResourceUri;
use crate::vfs::{Attrs, DirEntry, Ino, NodeKind, VfsError};

static NEXT_REPLACEMENT_ID: AtomicU64 = AtomicU64::new(1);
const MAX_NATIVE_READ: u32 = 64 * 1024 * 1024;

fn to_vfs(ino: Ino, attrs: ProviderAttrs) -> Attrs {
    Attrs {
        ino,
        kind: attrs.kind,
        size: attrs.size,
        perm: attrs.perm,
        nlink: attrs.nlink,
        uid: attrs.uid,
        gid: attrs.gid,
        atime: attrs.atime,
        mtime: attrs.mtime,
        ctime: attrs.ctime,
    }
}

fn to_vfs_error(error: ResourceError) -> VfsError {
    match error.code {
        ResourceErrorCode::NotFound => VfsError::NotFound,
        ResourceErrorCode::NotDir => VfsError::NotDir,
        ResourceErrorCode::IsDir => VfsError::IsDir,
        ResourceErrorCode::Conflict => VfsError::Exists,
        ResourceErrorCode::PermissionDenied => VfsError::PermissionDenied,
        _ => VfsError::Io,
    }
}

/// An empty native root. Its children are supplied by separately registered
/// providers or future kernel-owned resources.
pub struct EmptyNamespace {
    name: String,
    root_ino: Ino,
}

impl EmptyNamespace {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            root_ino: Ino(0),
        }
    }
}

#[async_trait]
impl ResourceProvider for EmptyNamespace {
    fn provider_name(&self) -> &str {
        &self.name
    }

    fn eligible(&self, uri: &ResourceUri) -> bool {
        uri.scheme() == self.name && uri.is_root() && uri.authority().is_empty()
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
impl Namespace for EmptyNamespace {
    fn name(&self) -> &str {
        &self.name
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
            Ok(to_vfs(ino, ProviderAttrs::directory()))
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

/// The native `file://` provider. The URI root maps to `root` on the host.
pub struct FilesNamespace {
    root: PathBuf,
    exclusions: Arc<RwLock<Vec<PathBuf>>>,
    root_ino: Ino,
    nodes: Mutex<HashMap<Ino, PathBuf>>,
    reverse: Mutex<HashMap<PathBuf, Ino>>,
    next_ino: Mutex<u64>,
}

impl FilesNamespace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = fs::canonicalize(&root).unwrap_or(root);
        Self {
            root,
            exclusions: Arc::new(RwLock::new(Vec::new())),
            root_ino: Ino(0),
            nodes: Mutex::new(HashMap::new()),
            reverse: Mutex::new(HashMap::new()),
            next_ino: Mutex::new(Ino::ROOT.0 + 1),
        }
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    /// Exclude a host path and its descendants from the `file://` namespace.
    /// Relative paths are relative to this namespace's configured root.
    /// Providers that own a resource backed by this path can therefore keep
    /// their backing files private from the ordinary filesystem view.
    pub fn exclude_path(&self, path: impl AsRef<Path>) -> Result<(), ResourceError> {
        let path = path.as_ref();
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        if !path.starts_with(&self.root) {
            return Err(ResourceError::new(
                ResourceErrorCode::InvalidAddress,
                format!(
                    "files exclusion is outside namespace root: {}",
                    path.display()
                ),
            ));
        }
        let mut exclusions = self.exclusions.write().unwrap();
        if !exclusions.iter().any(|existing| existing == &path) {
            exclusions.push(path);
        }
        Ok(())
    }

    pub fn excluded_paths(&self) -> Vec<PathBuf> {
        self.exclusions.read().unwrap().clone()
    }

    fn is_excluded(&self, path: &Path) -> bool {
        let is_artist_private = path.strip_prefix(&self.root).ok().is_some_and(|relative| {
            relative.components().any(|component| {
                    matches!(component, Component::Normal(name) if name == OsStr::new(".artist"))
                })
        });
        is_artist_private
            || self
                .exclusions
                .read()
                .unwrap()
                .iter()
                .any(|excluded| path == excluded || path.starts_with(excluded))
    }

    fn host_path(&self, uri: &ResourceUri) -> Result<PathBuf, ResourceError> {
        if uri.scheme() != "file" || !uri.authority().is_empty() {
            return Err(ResourceError::new(
                ResourceErrorCode::InvalidAddress,
                format!("not a files URI: {uri}"),
            ));
        }
        let mut path = self.root.clone();
        for segment in uri.decoded_segments().map_err(|_| {
            ResourceError::new(
                ResourceErrorCode::InvalidAddress,
                format!("invalid URI: {uri}"),
            )
        })? {
            path.push(segment);
        }
        if self.is_excluded(&path) {
            return Err(ResourceError::not_found(uri));
        }
        Ok(path)
    }

    fn attrs_for_path(path: &Path) -> Result<ProviderAttrs, ResourceError> {
        let metadata = fs::metadata(path).map_err(|e| io_error(e, path))?;
        let kind = if metadata.is_dir() {
            NodeKind::Directory
        } else {
            NodeKind::File
        };
        let mtime = metadata.modified().unwrap_or_else(|_| SystemTime::now());
        let mut attrs = if kind == NodeKind::Directory {
            ProviderAttrs::directory()
        } else {
            ProviderAttrs::file(metadata.len(), mtime)
        };
        attrs.mtime = mtime;
        attrs.ctime = mtime;
        attrs.atime = mtime;
        attrs.perm = if kind == NodeKind::Directory {
            0o555
        } else {
            0o444
        };
        Ok(attrs)
    }

    fn allocate(&self, path: PathBuf) -> Ino {
        if let Some(ino) = self.reverse.lock().unwrap().get(&path).copied() {
            return ino;
        }
        let mut next = self.next_ino.lock().unwrap();
        let ino = Ino(*next);
        *next += 1;
        self.nodes.lock().unwrap().insert(ino, path.clone());
        self.reverse.lock().unwrap().insert(path, ino);
        ino
    }

    fn path_for_ino(&self, ino: Ino) -> Option<PathBuf> {
        let path = self.nodes.lock().unwrap().get(&ino).cloned()?;
        (!self.is_excluded(&path)).then_some(path)
    }

    fn set_root(&mut self, ino: Ino) {
        self.root_ino = ino;
        self.nodes.get_mut().unwrap().insert(ino, self.root.clone());
        self.reverse
            .get_mut()
            .unwrap()
            .insert(self.root.clone(), ino);
        *self.next_ino.get_mut().unwrap() = ino.0 + 1;
    }

    fn update_cached_paths(&self, old_path: &Path, new_path: &Path) -> Result<(), VfsError> {
        let mut nodes = self.nodes.lock().unwrap();
        let mut reverse = self.reverse.lock().unwrap();
        let moved: Vec<(Ino, PathBuf)> = nodes
            .iter()
            .filter(|(_, path)| *path == old_path || path.starts_with(old_path))
            .map(|(ino, path)| (*ino, path.clone()))
            .collect();
        for (ino, path) in moved {
            let suffix = path.strip_prefix(old_path).map_err(|_| VfsError::Io)?;
            let replacement = new_path.join(suffix);
            nodes.insert(ino, replacement.clone());
            reverse.remove(&path);
            reverse.insert(replacement, ino);
        }
        Ok(())
    }
}

fn io_error(error: std::io::Error, path: &Path) -> ResourceError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => ResourceErrorCode::NotFound,
        std::io::ErrorKind::PermissionDenied => ResourceErrorCode::PermissionDenied,
        std::io::ErrorKind::DirectoryNotEmpty => ResourceErrorCode::Conflict,
        std::io::ErrorKind::NotADirectory => ResourceErrorCode::NotDir,
        std::io::ErrorKind::AlreadyExists => ResourceErrorCode::Conflict,
        _ => ResourceErrorCode::Io,
    };
    ResourceError::new(code, format!("{}: {error}", path.display()))
}

fn validate_child_name(name: &OsStr) -> Result<&str, VfsError> {
    let name = name.to_str().ok_or(VfsError::NotFound)?;
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(VfsError::NotFound);
    }
    Ok(name)
}

#[async_trait]
impl ResourceProvider for FilesNamespace {
    fn provider_name(&self) -> &str {
        "file"
    }
    fn eligible(&self, uri: &ResourceUri) -> bool {
        uri.scheme() == "file" && uri.authority().is_empty() && self.host_path(uri).is_ok()
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        Self::attrs_for_path(&self.host_path(uri)?)
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        let path = self.host_path(uri)?;
        let metadata = fs::metadata(&path).map_err(|e| io_error(e, &path))?;
        if !metadata.is_dir() {
            return Err(ResourceError::new(
                ResourceErrorCode::NotDir,
                "resource is not a directory",
            ));
        }
        let mut out = Vec::new();
        for item in fs::read_dir(&path).map_err(|e| io_error(e, &path))? {
            let item = item.map_err(|e| io_error(e, &path))?;
            let item_path = item.path();
            if self.is_excluded(&item_path) {
                continue;
            }
            out.push(ProviderEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                attrs: Self::attrs_for_path(&item_path)?,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        if matches!(uri.query(), Some(query) if query != "kind") {
            return Err(ResourceError::new(
                ResourceErrorCode::Unsupported,
                format!("files provider does not support URI selector: {uri}"),
            ));
        }
        let path = self.host_path(uri)?;
        if uri.query() == Some("kind") {
            let kind = Self::attrs_for_path(&path)?.kind;
            let value = match kind {
                NodeKind::Directory => "directory",
                NodeKind::File => "file",
            };
            let bytes = value.as_bytes();
            return Ok(bytes
                .iter()
                .skip(offset as usize)
                .take(size as usize)
                .copied()
                .collect());
        }
        let mut file = File::open(&path).map_err(|e| io_error(e, &path))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| io_error(e, &path))?;
        // A resource read is a bounded slice, not permission to allocate an
        // arbitrary `u32`-sized buffer on behalf of a caller.
        let mut buffer = vec![0; size.min(MAX_NATIVE_READ) as usize];
        let count = file.read(&mut buffer).map_err(|e| io_error(e, &path))?;
        buffer.truncate(count);
        Ok(buffer)
    }

    async fn write(
        &self,
        uri: &ResourceUri,
        offset: u64,
        data: &[u8],
    ) -> Result<u32, ResourceError> {
        reject_mutation_query(uri)?;
        let path = self.host_path(uri)?;
        let metadata = fs::metadata(&path).map_err(|e| io_error(e, &path))?;
        if metadata.is_dir() {
            return Err(ResourceError::new(
                ResourceErrorCode::IsDir,
                "cannot write a directory",
            ));
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| io_error(e, &path))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| io_error(e, &path))?;
        file.write_all(data).map_err(|e| io_error(e, &path))?;
        u32::try_from(data.len()).map_err(|_| {
            ResourceError::new(
                ResourceErrorCode::Io,
                "write length exceeds the URI write result capacity",
            )
        })
    }

    async fn set_size(&self, uri: &ResourceUri, size: u64) -> Result<(), ResourceError> {
        reject_mutation_query(uri)?;
        let path = self.host_path(uri)?;
        let metadata = fs::metadata(&path).map_err(|e| io_error(e, &path))?;
        if metadata.is_dir() {
            return Err(ResourceError::new(
                ResourceErrorCode::IsDir,
                "cannot resize a directory",
            ));
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| io_error(e, &path))?;
        file.set_len(size).map_err(|e| io_error(e, &path))
    }

    async fn replace_all(&self, uri: &ResourceUri, data: &[u8]) -> Result<u64, ResourceError> {
        reject_mutation_query(uri)?;
        let path = self.host_path(uri)?;
        let metadata = fs::metadata(&path).map_err(|e| io_error(e, &path))?;
        if metadata.is_dir() {
            return Err(ResourceError::new(
                ResourceErrorCode::IsDir,
                "cannot replace a directory",
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            ResourceError::new(
                ResourceErrorCode::InvalidAddress,
                "resource has no replaceable parent directory",
            )
        })?;
        let file_name = path.file_name().ok_or_else(|| {
            ResourceError::new(
                ResourceErrorCode::InvalidAddress,
                "resource has no file name",
            )
        })?;
        let temporary = parent.join(format!(
            ".{}.artist-replace-{}",
            file_name.to_string_lossy(),
            NEXT_REPLACEMENT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| io_error(e, &temporary))?;
            file.write_all(data).map_err(|e| io_error(e, &temporary))?;
            file.sync_all().map_err(|e| io_error(e, &temporary))?;
            drop(file);
            atomic_replace_path(&temporary, &path).map_err(|e| io_error(e, &path))?;
            if let Ok(directory) = fs::File::open(parent) {
                let _ = directory.sync_all();
            }
            Ok::<u64, ResourceError>(data.len() as u64)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    async fn create_file(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        reject_mutation_query(uri)?;
        let path = self.host_path(uri)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| io_error(e, &path))?;
        drop(file);
        Self::attrs_for_path(&path)
    }

    async fn create_directory(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        reject_mutation_query(uri)?;
        let path = self.host_path(uri)?;
        fs::create_dir(&path).map_err(|e| io_error(e, &path))?;
        Self::attrs_for_path(&path)
    }

    async fn move_resource(
        &self,
        source: &ResourceUri,
        destination: &ResourceUri,
    ) -> Result<(), ResourceError> {
        reject_mutation_query(source)?;
        reject_mutation_query(destination)?;
        let source_path = self.host_path(source)?;
        let destination_path = self.host_path(destination)?;
        let target = match fs::metadata(&destination_path) {
            Ok(metadata) if metadata.is_dir() => {
                destination_path.join(source_path.file_name().ok_or_else(|| {
                    ResourceError::new(
                        ResourceErrorCode::InvalidAddress,
                        "cannot move the files namespace root",
                    )
                })?)
            }
            Ok(_) => destination_path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => destination_path,
            Err(error) => return Err(io_error(error, &destination_path)),
        };
        if self.is_excluded(&target) {
            return Err(ResourceError::not_found(destination));
        }
        fs::rename(&source_path, &target).map_err(|e| io_error(e, &target))?;
        self.update_cached_paths(&source_path, &target)
            .map_err(|_| {
                ResourceError::new(
                    ResourceErrorCode::Io,
                    "move succeeded but inode cache update failed",
                )
            })
    }

    async fn delete(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        reject_mutation_query(uri)?;
        let path = self.host_path(uri)?;
        let metadata = fs::metadata(&path).map_err(|e| io_error(e, &path))?;
        if metadata.is_dir() {
            fs::remove_dir(&path).map_err(|e| io_error(e, &path))
        } else {
            fs::remove_file(&path).map_err(|e| io_error(e, &path))
        }
    }
}

#[cfg(not(windows))]
fn atomic_replace_path(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn atomic_replace_path(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    let backup = temporary.with_extension("artist-backup");
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    if destination.exists() {
        fs::rename(destination, &backup)?;
    }
    match fs::rename(temporary, destination) {
        Ok(()) => {
            let _ = fs::remove_file(&backup);
            Ok(())
        }
        Err(error) => {
            if backup.exists() && !destination.exists() {
                let _ = fs::rename(&backup, destination);
            }
            Err(error)
        }
    }
}

fn reject_mutation_query(uri: &ResourceUri) -> Result<(), ResourceError> {
    if uri.query().is_some() {
        return Err(ResourceError::new(
            ResourceErrorCode::Unsupported,
            format!("files mutations do not support URI query selectors: {uri}"),
        ));
    }
    Ok(())
}

#[async_trait]
impl Namespace for FilesNamespace {
    fn name(&self) -> &str {
        "file"
    }
    fn set_root_ino(&mut self, ino: Ino) {
        self.set_root(ino);
    }
    fn root_ino(&self) -> Ino {
        self.root_ino
    }
    fn owns(&self, ino: Ino) -> bool {
        self.nodes.lock().unwrap().contains_key(&ino)
    }

    async fn lookup(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        let parent_path = self.path_for_ino(parent).ok_or(VfsError::NotFound)?;
        let name = name.to_str().ok_or(VfsError::NotFound)?;
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(VfsError::NotFound);
        }
        let path = parent_path.join(name);
        if self.is_excluded(&path) {
            return Err(VfsError::NotFound);
        }
        let attrs = Self::attrs_for_path(&path).map_err(to_vfs_error)?;
        let ino = self.allocate(path);
        Ok(to_vfs(ino, attrs))
    }

    async fn getattr(&self, ino: Ino) -> Result<Attrs, VfsError> {
        let path = self.path_for_ino(ino).ok_or(VfsError::NotFound)?;
        Self::attrs_for_path(&path)
            .map(|attrs| to_vfs(ino, attrs))
            .map_err(to_vfs_error)
    }

    async fn readdir(&self, ino: Ino) -> Result<Vec<DirEntry>, VfsError> {
        let path = self.path_for_ino(ino).ok_or(VfsError::NotFound)?;
        let metadata = fs::metadata(&path).map_err(|e| to_vfs_error(io_error(e, &path)))?;
        if !metadata.is_dir() {
            return Err(VfsError::NotDir);
        }
        let mut out = Vec::new();
        for item in fs::read_dir(&path).map_err(|e| to_vfs_error(io_error(e, &path)))? {
            let item = item.map_err(|e| to_vfs_error(io_error(e, &path)))?;
            let child_path = item.path();
            if self.is_excluded(&child_path) {
                continue;
            }
            let child_ino = self.allocate(child_path.clone());
            let attrs = Self::attrs_for_path(&child_path).map_err(to_vfs_error)?;
            out.push(DirEntry {
                ino: child_ino,
                kind: attrs.kind,
                name: item.file_name(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn read(&self, ino: Ino, offset: u64, size: u32) -> Result<Vec<u8>, VfsError> {
        let path = self.path_for_ino(ino).ok_or(VfsError::NotFound)?;
        let metadata = fs::metadata(&path).map_err(|e| to_vfs_error(io_error(e, &path)))?;
        if metadata.is_dir() {
            return Err(VfsError::IsDir);
        }
        let uri = ResourceUri::root("file").map_err(|_| VfsError::Io)?;
        let mut current = uri;
        let relative = path.strip_prefix(&self.root).map_err(|_| VfsError::Io)?;
        for component in relative.components() {
            let name = component.as_os_str().to_str().ok_or(VfsError::Io)?;
            current = current.child(name).map_err(|_| VfsError::Io)?;
        }
        ResourceProvider::read(self, &current, offset, size)
            .await
            .map_err(to_vfs_error)
    }

    async fn write(&self, ino: Ino, offset: u64, data: &[u8]) -> Result<u32, VfsError> {
        let path = self.path_for_ino(ino).ok_or(VfsError::NotFound)?;
        let metadata = fs::metadata(&path).map_err(|e| to_vfs_error(io_error(e, &path)))?;
        if metadata.is_dir() {
            return Err(VfsError::IsDir);
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| to_vfs_error(io_error(e, &path)))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| to_vfs_error(io_error(e, &path)))?;
        file.write_all(data)
            .map_err(|e| to_vfs_error(io_error(e, &path)))?;
        Ok(data.len() as u32)
    }

    async fn set_size(&self, ino: Ino, size: u64) -> Result<(), VfsError> {
        let path = self.path_for_ino(ino).ok_or(VfsError::NotFound)?;
        let metadata = fs::metadata(&path).map_err(|e| to_vfs_error(io_error(e, &path)))?;
        if metadata.is_dir() {
            return Err(VfsError::IsDir);
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| to_vfs_error(io_error(e, &path)))?;
        file.set_len(size)
            .map_err(|e| to_vfs_error(io_error(e, &path)))
    }

    async fn create_file(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        let parent_path = self.path_for_ino(parent).ok_or(VfsError::NotFound)?;
        let name = validate_child_name(name)?;
        let path = parent_path.join(name);
        if self.is_excluded(&path) {
            return Err(VfsError::NotFound);
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| to_vfs_error(io_error(e, &path)))?;
        drop(file);
        let ino = self.allocate(path.clone());
        Self::attrs_for_path(&path)
            .map(|attrs| to_vfs(ino, attrs))
            .map_err(to_vfs_error)
    }

    async fn create_directory(&self, parent: Ino, name: &OsStr) -> Result<Attrs, VfsError> {
        let parent_path = self.path_for_ino(parent).ok_or(VfsError::NotFound)?;
        let name = validate_child_name(name)?;
        let path = parent_path.join(name);
        if self.is_excluded(&path) {
            return Err(VfsError::NotFound);
        }
        fs::create_dir(&path).map_err(|e| to_vfs_error(io_error(e, &path)))?;
        let ino = self.allocate(path.clone());
        Self::attrs_for_path(&path)
            .map(|attrs| to_vfs(ino, attrs))
            .map_err(to_vfs_error)
    }

    async fn rename(
        &self,
        old_parent: Ino,
        old_name: &OsStr,
        new_parent: Ino,
        new_name: &OsStr,
    ) -> Result<(), VfsError> {
        let old_parent = self.path_for_ino(old_parent).ok_or(VfsError::NotFound)?;
        let new_parent = self.path_for_ino(new_parent).ok_or(VfsError::NotFound)?;
        let old_name = validate_child_name(old_name)?;
        let new_name = validate_child_name(new_name)?;
        let old_path = old_parent.join(old_name);
        let new_path = new_parent.join(new_name);
        if self.is_excluded(&old_path) || self.is_excluded(&new_path) {
            return Err(VfsError::NotFound);
        }
        fs::rename(&old_path, &new_path).map_err(|e| to_vfs_error(io_error(e, &new_path)))?;

        // Inodes are runtime-local. Update cached paths so open descriptors and
        // subsequent lookups continue to address the moved subtree.
        self.update_cached_paths(&old_path, &new_path)
    }

    async fn unlink(&self, parent: Ino, name: &OsStr, directory: bool) -> Result<(), VfsError> {
        let parent_path = self.path_for_ino(parent).ok_or(VfsError::NotFound)?;
        let name = validate_child_name(name)?;
        let path = parent_path.join(name);
        if self.is_excluded(&path) {
            return Err(VfsError::NotFound);
        }
        let metadata = fs::metadata(&path).map_err(|e| to_vfs_error(io_error(e, &path)))?;
        if metadata.is_dir() != directory {
            return if metadata.is_dir() {
                Err(VfsError::IsDir)
            } else {
                Err(VfsError::NotDir)
            };
        }
        if directory {
            fs::remove_dir(&path)
        } else {
            fs::remove_file(&path)
        }
        .map_err(|e| to_vfs_error(io_error(e, &path)))?;
        self.nodes
            .lock()
            .unwrap()
            .retain(|_, p| p != &path && !p.starts_with(&path));
        self.reverse
            .lock()
            .unwrap()
            .retain(|p, _| p != &path && !p.starts_with(&path));
        Ok(())
    }

    async fn parent(&self, ino: Ino) -> Option<Ino> {
        let path = self.path_for_ino(ino)?;
        if ino == self.root_ino {
            return Some(Ino::ROOT);
        }
        let parent = path.parent()?.to_path_buf();
        self.reverse
            .lock()
            .unwrap()
            .get(&parent)
            .copied()
            .or(Some(self.root_ino))
    }
}

impl FilesNamespace {
    pub fn configured_root(&self) -> &Path {
        &self.root
    }
}

#[allow(dead_code)]
fn _keep_read_dir_type(_: Option<ReadDir>) {}
