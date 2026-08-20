//! The ino-based descriptor model bridging `wasi:filesystem` to kernel [`Vfs`].
//!
//! A [`Descriptor`] is the host representation of the `descriptor` resource: an
//! inode plus the open flags. Everything else — the node's type, size,
//! timestamps — is derived on demand from the [`Vfs`], so a descriptor is never
//! more than a cheap, `Clone`-able handle.

use std::ffi::OsStr;
use std::time::{SystemTime, UNIX_EPOCH};

use artist_kernel::vfs::{Attrs, Ino, NodeKind, Vfs};

use crate::bindings::system_clock;
use crate::bindings::types::{
    DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry, ErrorCode, MetadataHashValue,
};
use crate::error::vfs_error_to_code;

/// A `wasi:filesystem` descriptor: an inode plus the open flags.
///
/// This is stored directly in the wasmtime `ResourceTable` (via the bindgen
/// `with` mapping), so cloning is cheap and the host never needs a side table.
#[derive(Clone, Debug)]
pub struct Descriptor {
    pub ino: Ino,
    pub flags: DescriptorFlags,
}

impl Descriptor {
    pub fn new(ino: Ino, flags: DescriptorFlags) -> Self {
        Self { ino, flags }
    }
}

/// Resolve a WASI path (relative, `/`-separated) against `base` to an inode.
///
/// Absolute paths fail with `not-permitted`; `..` above the kernel root fails
/// with `not-permitted`; a missing or non-directory step maps through
/// [`vfs_error_to_code`]. The kernel [`Vfs`] has no symlinks, so
/// `symlink_follow` is accepted for API compatibility but has no effect.
pub async fn resolve(
    vfs: &dyn Vfs,
    base: Ino,
    path: &str,
    _symlink_follow: bool,
) -> Result<Ino, ErrorCode> {
    if path.starts_with('/') {
        return Err(ErrorCode::NotPermitted);
    }

    let mut cur = base;
    for component in path.split('/') {
        match component {
            // Tolerate empty components (e.g. trailing slash, doubled slash).
            "" | "." => {}
            ".." => {
                cur = vfs.parent(cur).await.ok_or(ErrorCode::NotPermitted)?;
            }
            name => {
                let attrs = vfs
                    .lookup(cur, OsStr::new(name))
                    .await
                    .map_err(vfs_error_to_code)?;
                cur = attrs.ino;
            }
        }
    }
    Ok(cur)
}

/// Resolve the parent directory and final child name of a relative path.
pub async fn resolve_parent(
    vfs: &dyn Vfs,
    base: Ino,
    path: &str,
) -> Result<(Ino, String), ErrorCode> {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(ErrorCode::Invalid);
    }
    let parent = resolve(vfs, base, parent, true).await?;
    let attrs = vfs.getattr(parent).await.map_err(vfs_error_to_code)?;
    if attrs.kind != NodeKind::Directory {
        return Err(ErrorCode::NotDirectory);
    }
    Ok((parent, name.to_string()))
}

/// Fetch attributes for an inode and translate to a WASI descriptor type.
pub async fn get_type(vfs: &dyn Vfs, ino: Ino) -> Result<DescriptorType, ErrorCode> {
    let attrs = vfs.getattr(ino).await.map_err(vfs_error_to_code)?;
    Ok(node_kind_to_descriptor_type(attrs.kind))
}

/// Fetch attributes for an inode and translate to a WASI descriptor stat.
pub async fn stat(vfs: &dyn Vfs, ino: Ino) -> Result<DescriptorStat, ErrorCode> {
    let attrs = vfs.getattr(ino).await.map_err(vfs_error_to_code)?;
    Ok(attrs_to_stat(&attrs))
}

/// Read one chunk of a file at `offset` (at most `size` bytes).
pub async fn read_chunk(
    vfs: &dyn Vfs,
    ino: Ino,
    offset: u64,
    size: u32,
) -> Result<Vec<u8>, ErrorCode> {
    vfs.read(ino, offset, size).await.map_err(vfs_error_to_code)
}

/// Read and translate a directory's entries into WASI directory entries.
pub async fn read_dir_entries(vfs: &dyn Vfs, ino: Ino) -> Result<Vec<DirectoryEntry>, ErrorCode> {
    let entries = vfs.readdir(ino).await.map_err(vfs_error_to_code)?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .name
            .into_string()
            .map_err(|_| ErrorCode::IllegalByteSequence)?;
        out.push(DirectoryEntry {
            type_: node_kind_to_descriptor_type(entry.kind),
            name,
        });
    }
    Ok(out)
}

/// A stable, deterministic metadata hash over the attributes the kernel Vfs
/// exposes (inode, size, modification time). Replaces the `st_dev`/`st_ino`
/// pair WASI deliberately omits.
pub fn metadata_hash(ino: Ino, attrs: &Attrs) -> MetadataHashValue {
    let (secs, nanos) = attrs
        .mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_secs(), d.subsec_nanos() as u64))
        .unwrap_or((0, 0));

    let mut lower = ino.0 ^ attrs.size ^ secs;
    let mut upper = nanos ^ (attrs.size >> 32) ^ (ino.0 << 1);
    lower = lower.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    upper = upper.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    MetadataHashValue {
        lower,
        upper: upper ^ (lower >> 32),
    }
}

/// Translate a kernel [`NodeKind`] to the WASI descriptor type.
pub fn node_kind_to_descriptor_type(kind: NodeKind) -> DescriptorType {
    match kind {
        NodeKind::Directory => DescriptorType::Directory,
        NodeKind::File => DescriptorType::RegularFile,
    }
}

/// Translate kernel [`Attrs`] to the WASI descriptor stat.
pub fn attrs_to_stat(attrs: &Attrs) -> DescriptorStat {
    DescriptorStat {
        type_: node_kind_to_descriptor_type(attrs.kind),
        link_count: attrs.nlink as u64,
        size: attrs.size,
        data_access_timestamp: system_time_to_instant(attrs.atime),
        data_modification_timestamp: system_time_to_instant(attrs.mtime),
        status_change_timestamp: system_time_to_instant(attrs.ctime),
    }
}

/// Translate a [`SystemTime`] to a `wasi:clocks/system-clock.instant`.
///
/// Returns `None` for times before the unix epoch, which the kernel never
/// produces for a live Vfs but which the type allows.
pub fn system_time_to_instant(t: SystemTime) -> Option<system_clock::Instant> {
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    Some(system_clock::Instant {
        seconds: d.as_secs() as i64,
        nanoseconds: d.subsec_nanos(),
    })
}
