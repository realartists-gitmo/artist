//! WinFsp adapter for the platform-independent resource mount model.

use std::{
    ffi::c_void,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::runtime::Handle;
use winfsp::{
    FspError, U16CStr,
    filesystem::{
        DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, OpenFileInfo, VolumeInfo,
        WideNameInfo,
    },
    host::{FileSystemHost, VolumeParams},
};

use crate::{
    ResourceReply, ResourceRequest, ResourceRouter,
    mount::{MountNode, ResourceMountModel},
};

const DIRECTORY: u32 = 0x10;
const ARCHIVE: u32 = 0x20;
const DIRECTORY_FILE: u32 = 0x1;
const END_OF_FILE: i32 = 0xC000_0011_u32 as i32;
const INVALID_PARAMETER: i32 = 0xC000_000D_u32 as i32;
const NOT_A_DIRECTORY: i32 = 0xC000_0103_u32 as i32;
const NOT_FOUND: i32 = 0xC000_0034_u32 as i32;

pub struct WinFspMount {
    path: PathBuf,
    _temp: tempfile::TempDir,
    host: FileSystemHost<ResourceWinFsp>,
    _init: winfsp::FspInit,
}

impl WinFspMount {
    pub fn mount(router: ResourceRouter, runtime: Handle) -> Result<Self, std::io::Error> {
        let init = winfsp::winfsp_init().map_err(io_error)?;
        let temp = tempfile::Builder::new().prefix("artist-").tempdir()?;
        let path = temp.path().to_owned();
        let context = ResourceWinFsp {
            model: ResourceMountModel::new(router, runtime, path.clone()),
        };
        let mut params = VolumeParams::new();
        params
            .filesystem_name("Artist")
            .sector_size(512)
            .sectors_per_allocation_unit(1)
            .case_sensitive_search(true)
            .case_preserved_names(true)
            .unicode_on_disk(true)
            .persistent_acls(false)
            .post_cleanup_when_modified_only(true)
            .read_only_volume(false);
        let mut host: FileSystemHost<ResourceWinFsp> =
            FileSystemHost::new(params, context).map_err(io_error)?;
        host.mount(&path).map_err(io_error)?;
        host.start().map_err(io_error)?;
        Ok(Self {
            path,
            _temp: temp,
            host,
            _init: init,
        })
    }

    pub fn root(&self) -> &Path {
        &self.path
    }
}

impl Drop for WinFspMount {
    fn drop(&mut self) {
        self.host.stop();
        self.host.unmount();
    }
}

fn io_error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

struct ResourceWinFsp {
    model: ResourceMountModel,
}

struct ResourceHandle {
    node: Mutex<MountNode>,
    bytes: Mutex<Vec<u8>>,
    dirty: AtomicBool,
    deleted: AtomicBool,
}

impl ResourceWinFsp {
    fn error(status: i32) -> FspError {
        FspError::NTSTATUS(status)
    }

    fn resolve(&self, file_name: &U16CStr) -> Option<MountNode> {
        let mut node = MountNode::Root;
        for name in file_name
            .to_string_lossy()
            .split(['\\', '/'])
            .filter(|part| !part.is_empty())
        {
            node = match &node {
                MountNode::Root => self
                    .model
                    .schemes()
                    .into_iter()
                    .find(|scheme| scheme == name)
                    .map(MountNode::Scheme)?,
                _ => {
                    self.model
                        .classify(self.model.child_uri(&node, name)?)?
                        .node
                }
            };
        }
        Some(node)
    }

    fn resolve_parent(&self, file_name: &U16CStr) -> Option<(MountNode, String)> {
        let text = file_name.to_string_lossy();
        let mut parts = text
            .split(['\\', '/'])
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let name = parts.pop()?.to_owned();
        let parent = if parts.is_empty() {
            MountNode::Root
        } else {
            let parent = winfsp::U16CString::from_str(format!("\\{}", parts.join("\\"))).ok()?;
            self.resolve(&parent)?
        };
        Some((parent, name))
    }

    fn bytes(&self, node: &MountNode) -> winfsp::Result<Vec<u8>> {
        match node {
            MountNode::Resource {
                uri,
                directory: false,
            } => match self.model.handle(ResourceRequest::Read {
                uri: uri.clone(),
                start_line: None,
                line_count: None,
            }) {
                Ok(ResourceReply::Text { text }) => Ok(text.into_bytes()),
                _ => Err(Self::error(NOT_FOUND)),
            },
            _ => Ok(Vec::new()),
        }
    }

    fn info(node: &MountNode, size: u64) -> FileInfo {
        let directory = matches!(node, MountNode::Root | MountNode::Scheme(_))
            || matches!(
                node,
                MountNode::Resource {
                    directory: true,
                    ..
                }
            );
        FileInfo {
            file_attributes: if directory { DIRECTORY } else { ARCHIVE },
            allocation_size: size.div_ceil(512) * 512,
            file_size: size,
            ..FileInfo::default()
        }
    }

    fn commit(&self, context: &ResourceHandle) -> winfsp::Result<()> {
        if !context.dirty.load(Ordering::Acquire) {
            return Ok(());
        }
        let MountNode::Resource {
            uri,
            directory: false,
        } = context.node.lock().unwrap().clone()
        else {
            return Err(Self::error(INVALID_PARAMETER));
        };
        let text = String::from_utf8(context.bytes.lock().unwrap().clone())
            .map_err(|_| Self::error(INVALID_PARAMETER))?;
        self.model
            .handle(ResourceRequest::Write { uri, text })
            .map_err(|_| Self::error(INVALID_PARAMETER))?;
        context.dirty.store(false, Ordering::Release);
        Ok(())
    }
}

impl FileSystemContext for ResourceWinFsp {
    type FileContext = ResourceHandle;

    fn get_security_by_name(
        &self,
        file_name: &U16CStr,
        _security_descriptor: Option<&mut [c_void]>,
        _reparse_point_resolver: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
    ) -> winfsp::Result<FileSecurity> {
        let node = self
            .resolve(file_name)
            .ok_or_else(|| Self::error(NOT_FOUND))?;
        Ok(FileSecurity {
            reparse: false,
            sz_security_descriptor: 0,
            attributes: Self::info(&node, 0).file_attributes,
        })
    }

    fn open(
        &self,
        file_name: &U16CStr,
        _create_options: u32,
        _granted_access: u32,
        file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        let node = self
            .resolve(file_name)
            .ok_or_else(|| Self::error(NOT_FOUND))?;
        let bytes = self.bytes(&node)?;
        *file_info.as_mut() = Self::info(&node, bytes.len() as u64);
        Ok(ResourceHandle::new(node, bytes))
    }

    fn create(
        &self,
        file_name: &U16CStr,
        create_options: u32,
        _granted_access: u32,
        _file_attributes: u32,
        _security_descriptor: Option<&[c_void]>,
        _allocation_size: u64,
        _extra_buffer: Option<&[u8]>,
        _extra_buffer_is_reparse_point: bool,
        file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        if create_options & DIRECTORY_FILE != 0 {
            return Err(Self::error(INVALID_PARAMETER));
        }
        let (parent, name) = self
            .resolve_parent(file_name)
            .ok_or_else(|| Self::error(NOT_FOUND))?;
        let uri = self
            .model
            .child_uri(&parent, &name)
            .ok_or_else(|| Self::error(INVALID_PARAMETER))?;
        self.model
            .handle(ResourceRequest::Write {
                uri: uri.clone(),
                text: String::new(),
            })
            .map_err(|_| Self::error(INVALID_PARAMETER))?;
        let node = MountNode::Resource {
            uri,
            directory: false,
        };
        *file_info.as_mut() = Self::info(&node, 0);
        Ok(ResourceHandle::new(node, Vec::new()))
    }

    fn close(&self, context: Self::FileContext) {
        let _ = self.commit(&context);
    }

    fn cleanup(&self, context: &Self::FileContext, _file_name: Option<&U16CStr>, _flags: u32) {
        let _ = self.commit(context);
        if context.deleted.load(Ordering::Acquire) {
            if let MountNode::Resource { uri, .. } = context.node.lock().unwrap().clone() {
                let _ = self.model.handle(ResourceRequest::Move {
                    from: uri,
                    to: None,
                });
            }
        }
    }

    fn flush(
        &self,
        context: Option<&Self::FileContext>,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        if let Some(context) = context {
            self.commit(context)?;
            *file_info = context.info();
        }
        Ok(())
    }

    fn get_file_info(
        &self,
        context: &Self::FileContext,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        *file_info = context.info();
        Ok(())
    }

    fn overwrite(
        &self,
        context: &Self::FileContext,
        _file_attributes: u32,
        _replace_file_attributes: bool,
        allocation_size: u64,
        _extra_buffer: Option<&[u8]>,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        let mut bytes = context.bytes.lock().unwrap();
        bytes.clear();
        bytes.reserve(allocation_size as usize);
        context.dirty.store(true, Ordering::Release);
        *file_info = Self::info(&context.node.lock().unwrap(), 0);
        Ok(())
    }

    fn read_directory(
        &self,
        context: &Self::FileContext,
        _pattern: Option<&U16CStr>,
        marker: DirMarker,
        buffer: &mut [u8],
    ) -> winfsp::Result<u32> {
        let node = context.node.lock().unwrap().clone();
        if !matches!(node, MountNode::Root | MountNode::Scheme(_))
            && !matches!(
                node,
                MountNode::Resource {
                    directory: true,
                    ..
                }
            )
        {
            return Err(Self::error(NOT_A_DIRECTORY));
        }
        let marker = marker
            .inner_as_cstr()
            .map(U16CStr::to_string_lossy)
            .unwrap_or_default();
        let mut cursor = 0;
        for entry in self.model.entries(&node) {
            if !marker.is_empty() && entry.name <= marker {
                continue;
            }
            let mut info = DirInfo::<255>::new();
            *info.file_info_mut() = Self::info(&entry.node, entry.size);
            info.set_name(&entry.name)?;
            if !info.append_to_buffer(buffer, &mut cursor) {
                break;
            }
        }
        Ok(cursor)
    }

    fn read(
        &self,
        context: &Self::FileContext,
        buffer: &mut [u8],
        offset: u64,
    ) -> winfsp::Result<u32> {
        let bytes = context.bytes.lock().unwrap();
        if offset >= bytes.len() as u64 {
            return Err(Self::error(END_OF_FILE));
        }
        let start = offset as usize;
        let len = buffer.len().min(bytes.len() - start);
        buffer[..len].copy_from_slice(&bytes[start..start + len]);
        Ok(len as u32)
    }

    fn write(
        &self,
        context: &Self::FileContext,
        buffer: &[u8],
        offset: u64,
        write_to_eof: bool,
        constrained_io: bool,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<u32> {
        let mut bytes = context.bytes.lock().unwrap();
        let start = if write_to_eof {
            bytes.len()
        } else {
            offset as usize
        };
        if constrained_io && start >= bytes.len() {
            return Ok(0);
        }
        let len = if constrained_io {
            buffer.len().min(bytes.len() - start)
        } else {
            buffer.len()
        };
        if bytes.len() < start {
            bytes.resize(start, 0);
        }
        if bytes.len() < start + len {
            bytes.resize(start + len, 0);
        }
        bytes[start..start + len].copy_from_slice(&buffer[..len]);
        context.dirty.store(true, Ordering::Release);
        *file_info = Self::info(&context.node.lock().unwrap(), bytes.len() as u64);
        Ok(len as u32)
    }

    fn set_file_size(
        &self,
        context: &Self::FileContext,
        new_size: u64,
        set_allocation_size: bool,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        let mut bytes = context.bytes.lock().unwrap();
        if set_allocation_size {
            if new_size < bytes.len() as u64 {
                bytes.truncate(new_size as usize);
                context.dirty.store(true, Ordering::Release);
            } else {
                let additional = new_size as usize - bytes.len();
                bytes.reserve(additional);
            }
        } else {
            bytes.resize(new_size as usize, 0);
            context.dirty.store(true, Ordering::Release);
        }
        drop(bytes);
        *file_info = context.info();
        Ok(())
    }

    fn get_volume_info(&self, volume_info: &mut VolumeInfo) -> winfsp::Result<()> {
        volume_info.total_size = 1_u64 << 40;
        volume_info.free_size = 1_u64 << 39;
        volume_info.set_volume_label("Artist");
        Ok(())
    }

    fn set_delete(
        &self,
        context: &Self::FileContext,
        _file_name: &U16CStr,
        delete_file: bool,
    ) -> winfsp::Result<()> {
        context.deleted.store(delete_file, Ordering::Release);
        Ok(())
    }

    fn rename(
        &self,
        context: &Self::FileContext,
        _file_name: &U16CStr,
        new_file_name: &U16CStr,
        _replace_if_exists: bool,
    ) -> winfsp::Result<()> {
        self.commit(context)?;
        let (parent, name) = self
            .resolve_parent(new_file_name)
            .ok_or_else(|| Self::error(NOT_FOUND))?;
        let to = self
            .model
            .child_uri(&parent, &name)
            .ok_or_else(|| Self::error(INVALID_PARAMETER))?;
        let mut node = context.node.lock().unwrap();
        let MountNode::Resource {
            uri: from,
            directory,
        } = node.clone()
        else {
            return Err(Self::error(INVALID_PARAMETER));
        };
        self.model
            .handle(ResourceRequest::Move {
                from,
                to: Some(to.clone()),
            })
            .map_err(|_| Self::error(INVALID_PARAMETER))?;
        *node = MountNode::Resource { uri: to, directory };
        Ok(())
    }
}

impl ResourceHandle {
    fn new(node: MountNode, bytes: Vec<u8>) -> Self {
        Self {
            node: Mutex::new(node),
            bytes: Mutex::new(bytes),
            dirty: AtomicBool::new(false),
            deleted: AtomicBool::new(false),
        }
    }

    fn info(&self) -> FileInfo {
        ResourceWinFsp::info(
            &self.node.lock().unwrap(),
            self.bytes.lock().unwrap().len() as u64,
        )
    }
}
