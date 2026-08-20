//! Host implementation of `wasi:filesystem` over the kernel [`Vfs`].
//!
//! The host maps inodes to `descriptor` resources directly (no cap-std, no
//! real directory), resolves WASI paths with [`crate::descriptor::resolve`],
//! and projects the kernel VFS's read/write/mutation operations.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use artist_kernel::vfs::{NodeKind, Vfs};
use tokio::sync::oneshot;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Access, Accessor, FutureReader, HasData, Resource, ResourceTable, Source, StreamConsumer,
    StreamReader, StreamResult,
};

use crate::bindings::types::{
    Advice, DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry, ErrorCode, Filesize,
    MetadataHashValue, NewTimestamp, OpenFlags, PathFlags,
};
use crate::bindings::{preopens, types};
use crate::descriptor::{self, Descriptor};
use crate::error::{FilesystemError, vfs_error_to_code};
use crate::streams::{ReadDirProducer, ReadStreamProducer};

type FsResult<T> = Result<T, FilesystemError>;

struct WriteConsumer {
    vfs: Arc<dyn Vfs>,
    ino: artist_kernel::vfs::Ino,
    offset: u64,
    pending: Option<Pin<Box<dyn Future<Output = Result<u32, ErrorCode>> + Send + 'static>>>,
    pending_len: usize,
    result: Option<oneshot::Sender<Result<(), ErrorCode>>>,
}

impl WriteConsumer {
    fn new(
        vfs: Arc<dyn Vfs>,
        ino: artist_kernel::vfs::Ino,
        offset: u64,
        result: oneshot::Sender<Result<(), ErrorCode>>,
    ) -> Self {
        Self {
            vfs,
            ino,
            offset,
            pending: None,
            pending_len: 0,
            result: Some(result),
        }
    }

    fn close(&mut self, result: Result<(), ErrorCode>) {
        if let Some(tx) = self.result.take() {
            let _ = tx.send(result);
        }
    }
}

impl<D> StreamConsumer<D> for WriteConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let me = &mut *self;
        if me.pending.is_none() {
            let mut src = src.as_direct(store);
            let bytes = src.remaining().to_vec();
            if bytes.is_empty() {
                if finish {
                    me.close(Ok(()));
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                return Poll::Pending;
            }
            src.mark_read(bytes.len());
            me.pending_len = bytes.len();
            let vfs = Arc::clone(&me.vfs);
            let ino = me.ino;
            let offset = me.offset;
            me.pending = Some(Box::pin(async move {
                let offset = if offset == u64::MAX {
                    vfs.getattr(ino).await.map_err(vfs_error_to_code)?.size
                } else {
                    offset
                };
                vfs.write(ino, offset, &bytes)
                    .await
                    .map_err(vfs_error_to_code)
            }));
        }
        match Pin::new(me.pending.as_mut().unwrap()).poll(cx) {
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(written)) => {
                if written as usize != me.pending_len {
                    me.close(Err(ErrorCode::Io));
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                me.offset += written as u64;
                me.pending = None;
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Err(error)) => {
                me.close(Err(error));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
        }
    }
}

impl Drop for WriteConsumer {
    fn drop(&mut self) {
        self.close(Ok(()));
    }
}

/// Marker host type tying the bindgen `D` parameter to a [`FilesystemCtxView`].
///
/// This is the `D` in `types::add_to_linker::<T, D>`: it implements
/// `HostDescriptorWithStore<T>` (all the store-taking methods), while its
/// projected [`FilesystemCtxView`] implements `types::Host` +
/// `types::HostDescriptor` + `preopens::Host`.
pub struct FilesystemHost;

impl HasData for FilesystemHost {
    type Data<'a> = FilesystemCtxView<'a>;
}

/// The embedder-owned filesystem state: the kernel [`Vfs`] plus preopens.
pub struct FilesystemCtx {
    pub(crate) vfs: Arc<dyn Vfs>,
    pub(crate) preopens: Vec<(artist_kernel::vfs::Ino, String)>,
}

impl FilesystemCtx {
    pub fn new(vfs: Arc<dyn Vfs>) -> Self {
        Self {
            vfs,
            preopens: Vec::new(),
        }
    }

    /// Preopen `ino` (a directory in `vfs`) at the WASI path `path`.
    pub fn preopen(&mut self, ino: artist_kernel::vfs::Ino, path: impl Into<String>) {
        self.preopens.push((ino, path.into()));
    }

    /// The currently preopened directories, as `(inode, guest path)` pairs.
    pub fn preopens(&self) -> &[(artist_kernel::vfs::Ino, String)] {
        &self.preopens
    }

    fn vfs(&self) -> Arc<dyn Vfs> {
        Arc::clone(&self.vfs)
    }
}

/// A scoped view of [`FilesystemCtx`] plus the [`ResourceTable`].
///
/// This is what the embedder hands the linker (see [`FilesystemView`]); it is
/// also `D::Data` for [`FilesystemHost`].
pub struct FilesystemCtxView<'a> {
    pub ctx: &'a mut FilesystemCtx,
    pub table: &'a mut ResourceTable,
}

/// Implemented by the embedder's store data to project a [`FilesystemCtxView`].
pub trait FilesystemView: Send {
    fn filesystem(&mut self) -> FilesystemCtxView<'_>;
}

/// Clone a descriptor (and its [`Vfs`]) out of the table for use across `await`.
fn extract<T>(
    store: &Accessor<T, FilesystemHost>,
    fd: &Resource<Descriptor>,
) -> FsResult<(Arc<dyn Vfs>, Descriptor)> {
    store.with(|mut s| {
        let view = s.get();
        let desc = view.table.get(fd).cloned().map_err(FilesystemError::trap)?;
        Ok((view.ctx.vfs(), desc))
    })
}

impl types::Host for FilesystemCtxView<'_> {
    fn convert_error_code(&mut self, error: FilesystemError) -> wasmtime::Result<ErrorCode> {
        error.downcast()
    }
}

impl types::HostDescriptor for FilesystemCtxView<'_> {
    fn drop(&mut self, rep: Resource<Descriptor>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl preopens::Host for FilesystemCtxView<'_> {
    fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        let mut out = Vec::with_capacity(self.ctx.preopens.len());
        for (ino, path) in &self.ctx.preopens {
            let rep = self.table.push(Descriptor::new(
                *ino,
                DescriptorFlags::READ | DescriptorFlags::WRITE | DescriptorFlags::MUTATE_DIRECTORY,
            ))?;
            out.push((rep, path.clone()));
        }
        Ok(out)
    }
}

impl<T> types::HostDescriptorWithStore<T> for FilesystemHost {
    fn read_via_stream(
        mut store: Access<T, Self>,
        fd: Resource<Descriptor>,
        offset: Filesize,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), ErrorCode>>)> {
        let (vfs, desc) = {
            let view = store.get();
            let desc = view
                .table
                .get(&fd)
                .cloned()
                .map_err(FilesystemError::trap)?;
            (view.ctx.vfs(), desc)
        };

        let (tx, rx) = oneshot::channel();
        let stream = StreamReader::new(
            &mut store,
            ReadStreamProducer::new(vfs, desc.ino, offset, tx),
        )?;
        let future = FutureReader::new(&mut store, rx)?;
        Ok((stream, future))
    }

    fn write_via_stream(
        mut store: Access<T, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
        offset: Filesize,
    ) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>> {
        let (vfs, desc) = {
            let view = store.get();
            let desc = view
                .table
                .get(&fd)
                .cloned()
                .map_err(FilesystemError::trap)?;
            (view.ctx.vfs(), desc)
        };
        let (tx, rx) = oneshot::channel();
        data.pipe(&mut store, WriteConsumer::new(vfs, desc.ino, offset, tx))?;
        FutureReader::new(&mut store, rx)
    }

    fn append_via_stream(
        mut store: Access<T, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>> {
        let (vfs, desc) = {
            let view = store.get();
            let desc = view
                .table
                .get(&fd)
                .cloned()
                .map_err(FilesystemError::trap)?;
            (view.ctx.vfs(), desc)
        };
        let (tx, rx) = oneshot::channel();
        data.pipe(&mut store, WriteConsumer::new(vfs, desc.ino, u64::MAX, tx))?;
        FutureReader::new(&mut store, rx)
    }

    async fn advise(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        _offset: Filesize,
        _length: Filesize,
        _advice: Advice,
    ) -> FsResult<()> {
        // No-op: the kernel Vfs has no advisory information to convey.
        let _ = extract(store, &fd)?;
        Ok(())
    }

    async fn sync_data(store: &Accessor<T, Self>, fd: Resource<Descriptor>) -> FsResult<()> {
        // No-op: the kernel Vfs is in-memory.
        let _ = extract(store, &fd)?;
        Ok(())
    }

    async fn get_flags(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
    ) -> FsResult<DescriptorFlags> {
        let (_, desc) = extract(store, &fd)?;
        Ok(desc.flags)
    }

    async fn get_type(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
    ) -> FsResult<DescriptorType> {
        let (vfs, desc) = extract(store, &fd)?;
        descriptor::get_type(vfs.as_ref(), desc.ino)
            .await
            .map_err(Into::into)
    }

    async fn set_size(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        _size: Filesize,
    ) -> FsResult<()> {
        let (vfs, desc) = extract(store, &fd)?;
        vfs.set_size(desc.ino, _size)
            .await
            .map_err(vfs_error_to_code)
            .map_err(Into::into)
    }

    async fn set_times(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        _data_access_timestamp: NewTimestamp,
        _data_modification_timestamp: NewTimestamp,
    ) -> FsResult<()> {
        let _ = extract(store, &fd)?;
        Err(ErrorCode::Unsupported.into())
    }

    fn read_directory(
        mut store: Access<T, Self>,
        fd: Resource<Descriptor>,
    ) -> wasmtime::Result<(
        StreamReader<DirectoryEntry>,
        FutureReader<Result<(), ErrorCode>>,
    )> {
        let (vfs, desc) = {
            let view = store.get();
            let desc = view
                .table
                .get(&fd)
                .cloned()
                .map_err(FilesystemError::trap)?;
            (view.ctx.vfs(), desc)
        };

        let (tx, rx) = oneshot::channel();
        let stream = StreamReader::new(&mut store, ReadDirProducer::new(vfs, desc.ino, tx))?;
        let future = FutureReader::new(&mut store, rx)?;
        Ok((stream, future))
    }

    async fn sync(store: &Accessor<T, Self>, fd: Resource<Descriptor>) -> FsResult<()> {
        let _ = extract(store, &fd)?;
        Ok(())
    }

    async fn create_directory_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FsResult<()> {
        let (vfs, base) = extract(store, &fd)?;
        let (parent, name) = descriptor::resolve_parent(vfs.as_ref(), base.ino, &path).await?;
        vfs.create_directory(parent, std::ffi::OsStr::new(&name))
            .await
            .map(|_| ())
            .map_err(vfs_error_to_code)
            .map_err(Into::into)
    }

    async fn stat(store: &Accessor<T, Self>, fd: Resource<Descriptor>) -> FsResult<DescriptorStat> {
        let (vfs, desc) = extract(store, &fd)?;
        descriptor::stat(vfs.as_ref(), desc.ino)
            .await
            .map_err(Into::into)
    }

    async fn stat_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> FsResult<DescriptorStat> {
        let (vfs, base) = extract(store, &fd)?;
        let ino = descriptor::resolve(
            vfs.as_ref(),
            base.ino,
            &path,
            path_flags.contains(PathFlags::SYMLINK_FOLLOW),
        )
        .await?;
        descriptor::stat(vfs.as_ref(), ino)
            .await
            .map_err(Into::into)
    }

    async fn set_times_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        _path_flags: PathFlags,
        _path: String,
        _data_access_timestamp: NewTimestamp,
        _data_modification_timestamp: NewTimestamp,
    ) -> FsResult<()> {
        let _ = extract(store, &fd)?;
        Err(ErrorCode::ReadOnly.into())
    }

    async fn link_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        _old_path_flags: PathFlags,
        _old_path: String,
        _new_fd: Resource<Descriptor>,
        _new_path: String,
    ) -> FsResult<()> {
        let _ = extract(store, &fd)?;
        Err(ErrorCode::Unsupported.into())
    }

    async fn open_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> FsResult<Resource<Descriptor>> {
        let (vfs, base) = extract(store, &fd)?;

        let mutating = flags.contains(DescriptorFlags::WRITE)
            || flags.contains(DescriptorFlags::MUTATE_DIRECTORY)
            || open_flags.contains(OpenFlags::CREATE)
            || open_flags.contains(OpenFlags::TRUNCATE);
        if mutating && !base.flags.contains(DescriptorFlags::MUTATE_DIRECTORY) {
            return Err(ErrorCode::ReadOnly.into());
        }

        let ino = match descriptor::resolve(
            vfs.as_ref(),
            base.ino,
            &path,
            path_flags.contains(PathFlags::SYMLINK_FOLLOW),
        )
        .await
        {
            Ok(ino) => ino,
            Err(ErrorCode::NoEntry) if open_flags.contains(OpenFlags::CREATE) => {
                let (parent, name) =
                    descriptor::resolve_parent(vfs.as_ref(), base.ino, &path).await?;
                vfs.create_file(parent, std::ffi::OsStr::new(&name))
                    .await
                    .map_err(vfs_error_to_code)?
                    .ino
            }
            Err(error) => return Err(error.into()),
        };

        // The path resolved, so the object exists: `exclusive` therefore fails.
        if open_flags.contains(OpenFlags::EXCLUSIVE) && open_flags.contains(OpenFlags::CREATE) {
            return Err(ErrorCode::Exist.into());
        }

        let attrs = vfs.getattr(ino).await.map_err(vfs_error_to_code)?;
        if open_flags.contains(OpenFlags::DIRECTORY) && attrs.kind != NodeKind::Directory {
            return Err(ErrorCode::NotDirectory.into());
        }
        if open_flags.contains(OpenFlags::TRUNCATE) {
            vfs.set_size(ino, 0).await.map_err(vfs_error_to_code)?;
        }

        let desc = Descriptor::new(ino, flags);
        store
            .with(|mut s| s.get().table.push(desc))
            .map_err(FilesystemError::trap)
    }

    async fn readlink_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        _path: String,
    ) -> FsResult<String> {
        let _ = extract(store, &fd)?;
        Err(ErrorCode::Unsupported.into())
    }

    async fn remove_directory_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FsResult<()> {
        let (vfs, base) = extract(store, &fd)?;
        let (parent, name) = descriptor::resolve_parent(vfs.as_ref(), base.ino, &path).await?;
        vfs.unlink(parent, std::ffi::OsStr::new(&name), true)
            .await
            .map_err(vfs_error_to_code)
            .map_err(Into::into)
    }

    async fn rename_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FsResult<()> {
        let (vfs, base) = extract(store, &fd)?;
        let new_base = store.with(|mut s| {
            s.get()
                .table
                .get(&new_fd)
                .cloned()
                .map_err(FilesystemError::trap)
        })?;
        let (old_parent, old_name) =
            descriptor::resolve_parent(vfs.as_ref(), base.ino, &old_path).await?;
        let (new_parent, new_name) =
            descriptor::resolve_parent(vfs.as_ref(), new_base.ino, &new_path).await?;
        vfs.rename(
            old_parent,
            std::ffi::OsStr::new(&old_name),
            new_parent,
            std::ffi::OsStr::new(&new_name),
        )
        .await
        .map_err(vfs_error_to_code)
        .map_err(Into::into)
    }

    async fn symlink_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        _old_path: String,
        _new_path: String,
    ) -> FsResult<()> {
        let _ = extract(store, &fd)?;
        Err(ErrorCode::Unsupported.into())
    }

    async fn unlink_file_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FsResult<()> {
        let (vfs, base) = extract(store, &fd)?;
        let (parent, name) = descriptor::resolve_parent(vfs.as_ref(), base.ino, &path).await?;
        vfs.unlink(parent, std::ffi::OsStr::new(&name), false)
            .await
            .map_err(vfs_error_to_code)
            .map_err(Into::into)
    }

    async fn is_same_object(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> wasmtime::Result<bool> {
        let (_, a) = extract(store, &fd)?;
        let b = store.with(|mut s| {
            s.get()
                .table
                .get(&other)
                .cloned()
                .map_err(FilesystemError::trap)
        })?;
        Ok(a.ino == b.ino)
    }

    async fn metadata_hash(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
    ) -> FsResult<MetadataHashValue> {
        let (vfs, desc) = extract(store, &fd)?;
        let attrs = vfs.getattr(desc.ino).await.map_err(vfs_error_to_code)?;
        Ok(descriptor::metadata_hash(desc.ino, &attrs))
    }

    async fn metadata_hash_at(
        store: &Accessor<T, Self>,
        fd: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> FsResult<MetadataHashValue> {
        let (vfs, base) = extract(store, &fd)?;
        let ino = descriptor::resolve(
            vfs.as_ref(),
            base.ino,
            &path,
            path_flags.contains(PathFlags::SYMLINK_FOLLOW),
        )
        .await?;
        let attrs = vfs.getattr(ino).await.map_err(vfs_error_to_code)?;
        Ok(descriptor::metadata_hash(ino, &attrs))
    }
}
