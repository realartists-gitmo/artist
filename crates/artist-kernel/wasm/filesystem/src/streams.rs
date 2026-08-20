//! Host-side stream producers for `read-via-stream` and `read-directory`.
//!
//! The kernel [`Vfs`] is async, but [`StreamProducer::poll_produce`] is
//! synchronous. Each producer therefore holds a boxed, pollable future for the
//! in-flight [`Vfs`] call (no extra threads, no blocking): [`poll_produce`]
//! starts a read/readdir, polls it, and on completion hands the buffer to the
//! destination, closing the associated `future<result<_, error-code>>` on
//! EOF/error via a [`oneshot`] channel.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use artist_kernel::vfs::{Ino, Vfs};
use tokio::sync::oneshot;
use wasmtime::StoreContextMut;
use wasmtime::component::{Destination, StreamProducer, StreamResult, VecBuffer};

use crate::bindings::types::{DirectoryEntry, ErrorCode};
use crate::descriptor;

/// Default chunk size for byte reads. Mirrors wasmtime-wasi's own default.
const DEFAULT_BUFFER_CAPACITY: usize = 8192;

/// Produces the byte stream returned by `descriptor.read-via-stream`.
///
/// Reads [`DEFAULT_BUFFER_CAPACITY`] bytes at a time from a fixed starting
/// offset, advancing the offset across successive reads.
pub struct ReadStreamProducer {
    vfs: Arc<dyn Vfs>,
    ino: Ino,
    offset: u64,
    pending: Option<BoxedRead>,
    result: Option<oneshot::Sender<Result<(), ErrorCode>>>,
}

type BoxedRead = Pin<Box<dyn Future<Output = Result<Vec<u8>, ErrorCode>> + Send + 'static>>;

impl ReadStreamProducer {
    pub fn new(
        vfs: Arc<dyn Vfs>,
        ino: Ino,
        offset: u64,
        result: oneshot::Sender<Result<(), ErrorCode>>,
    ) -> Self {
        Self {
            vfs,
            ino,
            offset,
            pending: None,
            result: Some(result),
        }
    }

    fn close(&mut self, res: Result<(), ErrorCode>) {
        if let Some(tx) = self.result.take() {
            let _ = tx.send(res);
        }
    }
}

impl<D> StreamProducer<D> for ReadStreamProducer {
    type Item = u8;
    type Buffer = VecBuffer<u8>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        // A zero-length read is a readiness probe: report ready without
        // consuming anything.
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let me = &mut *self;

        if me.pending.is_none() {
            let vfs = Arc::clone(&me.vfs);
            let ino = me.ino;
            let offset = me.offset;
            me.pending = Some(Box::pin(async move {
                descriptor::read_chunk(vfs.as_ref(), ino, offset, DEFAULT_BUFFER_CAPACITY as u32)
                    .await
            }));
        }

        match Pin::new(me.pending.as_mut().unwrap()).poll(cx) {
            Poll::Pending => {
                if finish {
                    // Cancel the in-flight read; the next poll starts fresh.
                    me.pending = None;
                    return Poll::Ready(Ok(StreamResult::Cancelled));
                }
                Poll::Pending
            }
            Poll::Ready(Ok(buf)) => {
                me.pending = None;
                if buf.is_empty() {
                    me.close(Ok(()));
                    Poll::Ready(Ok(StreamResult::Dropped))
                } else {
                    me.offset += buf.len() as u64;
                    dst.set_buffer(buf.into());
                    Poll::Ready(Ok(StreamResult::Completed))
                }
            }
            Poll::Ready(Err(e)) => {
                me.pending = None;
                me.close(Err(e));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
        }
    }
}

impl Drop for ReadStreamProducer {
    fn drop(&mut self) {
        self.close(Ok(()));
    }
}

/// Produces the directory-entry stream returned by `descriptor.read-directory`.
///
/// The kernel [`Vfs::readdir`] returns a directory's entries in one call, so a
/// single poll yields the full entry set (the stream machinery retains surplus
/// items in memory for subsequent guest reads).
pub struct ReadDirProducer {
    vfs: Arc<dyn Vfs>,
    ino: Ino,
    pending: Option<BoxedReaddir>,
    result: Option<oneshot::Sender<Result<(), ErrorCode>>>,
}

type BoxedReaddir =
    Pin<Box<dyn Future<Output = Result<Vec<DirectoryEntry>, ErrorCode>> + Send + 'static>>;

impl ReadDirProducer {
    pub fn new(
        vfs: Arc<dyn Vfs>,
        ino: Ino,
        result: oneshot::Sender<Result<(), ErrorCode>>,
    ) -> Self {
        Self {
            vfs,
            ino,
            pending: None,
            result: Some(result),
        }
    }

    fn close(&mut self, res: Result<(), ErrorCode>) {
        if let Some(tx) = self.result.take() {
            let _ = tx.send(res);
        }
    }
}

impl<D> StreamProducer<D> for ReadDirProducer {
    type Item = DirectoryEntry;
    type Buffer = VecBuffer<DirectoryEntry>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let me = &mut *self;

        if me.pending.is_none() {
            let vfs = Arc::clone(&me.vfs);
            let ino = me.ino;
            me.pending = Some(Box::pin(async move {
                descriptor::read_dir_entries(vfs.as_ref(), ino).await
            }));
        }

        match Pin::new(me.pending.as_mut().unwrap()).poll(cx) {
            Poll::Pending => {
                if finish {
                    me.pending = None;
                    return Poll::Ready(Ok(StreamResult::Cancelled));
                }
                Poll::Pending
            }
            Poll::Ready(Ok(entries)) => {
                me.pending = None;
                me.close(Ok(()));
                dst.set_buffer(entries.into());
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Err(e)) => {
                me.pending = None;
                me.close(Err(e));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
        }
    }
}

impl Drop for ReadDirProducer {
    fn drop(&mut self) {
        self.close(Ok(()));
    }
}
