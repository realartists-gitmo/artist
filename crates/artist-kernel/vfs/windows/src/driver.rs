use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use artist_kernel::{Attrs, DirEntry, Ino, NodeKind, Vfs, VfsError};
use futures::executor::block_on;
use winfsp::Result;
use winfsp::U16CStr;
use winfsp::filesystem::{
    DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, OpenFileInfo, VolumeInfo,
    WideNameInfo,
};
use winfsp::host::{CoarseGuard, FileSystemHost, FileSystemParams, MountPoint, VolumeParams};

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// A WinFsp filesystem that bridges the kernel [`Vfs`] out to windows.
///
/// `FileContext` carries the resolved inode. Read-only: the volume is mounted
/// with `read_only_volume`, and every mutating callback returns an error.
pub struct WinFspFs {
    vfs: Arc<dyn Vfs>,
}

impl WinFspFs {
    pub fn new(vfs: Arc<dyn Vfs>) -> Self {
        Self { vfs }
    }

    fn resolve(&self, file_name: &U16CStr) -> Result<Ino> {
        // WinFsp hands us an absolute path rooted at `\`. Walk it down from the
        // kernel root via `lookup`.
        let mut ino = Ino::ROOT;
        for comp in file_name.as_slice().split(|&c| c == b'\\' as u16) {
            if comp.is_empty() {
                continue;
            }
            let name = String::from_utf16_lossy(comp);
            ino = block_on(self.vfs.lookup(ino, std::ffi::OsStr::new(&name))).map_err(map_err)?;
        }
        Ok(ino)
    }
}

fn map_err(e: VfsError) -> FspError {
    use std::io::ErrorKind;
    use winfsp::error::FspError;
    match e {
        VfsError::NotFound => FspError::IO(ErrorKind::NotFound),
        VfsError::NotDir => FspError::IO(ErrorKind::NotADirectory),
        VfsError::IsDir => FspError::IO(ErrorKind::IsADirectory),
        VfsError::Io => FspError::IO(ErrorKind::Other),
    }
}

fn filetime(t: SystemTime) -> u64 {
    // Windows FILETIME: 100ns intervals since 1601-01-01T00:00:00Z.
    const EPOCH_DIFF: u64 = 116444736000000000;
    let since_epoch = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    since_epoch.as_secs() * 10_000_000 + since_epoch.subsec_nanos() as u64 / 100 + EPOCH_DIFF
}

fn to_file_info(attrs: &Attrs, file_info: &mut FileInfo) {
    let is_dir = attrs.kind == NodeKind::Directory;
    file_info.file_attributes = if is_dir {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    file_info.file_size = attrs.size;
    file_info.allocation_size = attrs.size;
    file_info.creation_time = filetime(attrs.ctime);
    file_info.last_access_time = filetime(attrs.atime);
    file_info.last_write_time = filetime(attrs.mtime);
    file_info.change_time = filetime(attrs.ctime);
    file_info.index_number = attrs.ino.0;
    file_info.hard_links = attrs.nlink;
    file_info.ea_size = 0;
}

impl FileSystemContext for WinFspFs {
    type FileContext = u64;

    fn get_security_by_name(
        &self,
        file_name: &U16CStr,
        _security_descriptor: Option<&mut [c_void]>,
        _reparse_point_resolver: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
    ) -> Result<FileSecurity> {
        let ino = self.resolve(file_name)?;
        let attrs = block_on(self.vfs.getattr(ino)).map_err(map_err)?;
        Ok(FileSecurity {
            reparse: false,
            sz_security_descriptor: 0,
            attributes: if attrs.kind == NodeKind::Directory {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                FILE_ATTRIBUTE_NORMAL
            },
        })
    }

    fn open(
        &self,
        file_name: &U16CStr,
        _create_options: u32,
        _granted_access: winfsp_sys::FILE_ACCESS_RIGHTS,
        file_info: &mut OpenFileInfo,
    ) -> Result<Self::FileContext> {
        let ino = self.resolve(file_name)?;
        let attrs = block_on(self.vfs.getattr(ino)).map_err(map_err)?;
        to_file_info(&attrs, file_info.as_mut());
        Ok(ino.0)
    }

    fn close(&self, _context: Self::FileContext) {}

    fn get_file_info(&self, context: &Self::FileContext, file_info: &mut FileInfo) -> Result<()> {
        let attrs = block_on(self.vfs.getattr(Ino(*context))).map_err(map_err)?;
        to_file_info(&attrs, file_info);
        Ok(())
    }

    fn read(&self, context: &Self::FileContext, buffer: &mut [u8], offset: u64) -> Result<u32> {
        let size = buffer.len() as u32;
        let data = block_on(self.vfs.read(Ino(*context), offset, size)).map_err(map_err)?;
        let n = data.len().min(buffer.len());
        buffer[..n].copy_from_slice(&data[..n]);
        Ok(n as u32)
    }

    fn read_directory(
        &self,
        context: &Self::FileContext,
        _pattern: Option<&U16CStr>,
        _marker: DirMarker,
        buffer: &mut [u8],
    ) -> Result<u32> {
        let ino = Ino(*context);
        let mut entries: Vec<DirEntry> = Vec::new();
        entries.push(DirEntry {
            ino,
            kind: NodeKind::Directory,
            name: ".".into(),
        });
        if let Some(parent) = block_on(self.vfs.parent(ino)) {
            entries.push(DirEntry {
                ino: parent,
                kind: NodeKind::Directory,
                name: "..".into(),
            });
        }
        entries.extend(block_on(self.vfs.readdir(ino)).map_err(map_err)?);

        let mut cursor = 0u32;
        for entry in entries {
            let mut info: DirInfo = DirInfo::new();
            let attrs = block_on(self.vfs.getattr(entry.ino)).map_err(map_err)?;
            to_file_info(&attrs, info.file_info_mut());
            let name: Vec<u16> = entry.name.encode_wide().collect();
            info.set_name_raw(name.as_slice()).map_err(map_err)?;
            if !info.append_to_buffer(buffer, &mut cursor) {
                break;
            }
        }
        DirInfo::finalize_buffer(buffer, &mut cursor);
        Ok(cursor)
    }

    fn get_volume_info(&self, out_volume_info: &mut VolumeInfo) -> Result<()> {
        out_volume_info.total_size = 0;
        out_volume_info.free_size = 0;
        out_volume_info.set_volume_label("artist");
        Ok(())
    }
}

/// Guard that keeps a WinFsp volume mounted. Dropping it stops the dispatcher
/// and unmounts.
pub struct WinFspMount {
    host: Option<FileSystemHost<WinFspFs, CoarseGuard>>,
}

impl Drop for WinFspMount {
    fn drop(&mut self) {
        if let Some(host) = self.host.as_mut() {
            host.stop();
            host.unmount();
        }
    }
}

pub fn mount(vfs: Arc<dyn Vfs>, mountpoint: &Path, readonly: bool) -> std::io::Result<WinFspMount> {
    let mut volume_params = VolumeParams::new();
    volume_params
        .file_info_timeout(1000)
        .filesystem_name("artist")
        .case_preserved_names(true)
        .unicode_on_disk(true)
        .read_only_volume(readonly);

    let params = FileSystemParams::default_params(volume_params);
    let mut host =
        FileSystemHost::new_with_options(params, WinFspFs::new(vfs)).map_err(io_from_winfsp)?;
    host.mount(MountPoint::MountPoint(mountpoint.as_os_str()))
        .map_err(io_from_winfsp)?;
    host.start().map_err(io_from_winfsp)?;

    Ok(WinFspMount { host: Some(host) })
}

fn io_from_winfsp(e: FspError) -> std::io::Error {
    std::io::Error::other(e)
}
