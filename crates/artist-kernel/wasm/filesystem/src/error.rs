//! Error plumbing for the kernel filesystem host.
//!
//! The bindgen `trappable_error_type` config makes every host function return a
//! [`FilesystemError`] (rather than the raw `wasi:filesystem/types.error-code`).
//! The wrapper is either a normal error-code (the common case) or a full-blown
//! trap, which lets us `?`-propagate the plain code while still being able to
//! trap on genuinely unrecoverable host failures (e.g. a full resource table).

use std::error::Error;
use std::fmt;
use std::marker;

use artist_kernel::VfsError;

use crate::bindings::types::ErrorCode;

/// A host error that is either a plain [`ErrorCode`] or a trap.
///
/// This mirrors the shape of wasmtime-wasi's `TrappableError`: it is an
/// `Error` itself so `?` works, but `downcast` recovers the underlying code for
/// the guest-visible `convert_error_code` path.
#[repr(transparent)]
pub struct TrappableError<T> {
    err: wasmtime::Error,
    _marker: marker::PhantomData<T>,
}

impl<T> TrappableError<T> {
    pub fn trap(err: impl Into<wasmtime::Error>) -> Self {
        Self {
            err: err.into(),
            _marker: marker::PhantomData,
        }
    }

    pub fn downcast(self) -> wasmtime::Result<T>
    where
        T: Error + Send + Sync + 'static,
    {
        self.err.downcast()
    }
}

impl<T> From<T> for TrappableError<T>
where
    T: Error + Send + Sync + 'static,
{
    fn from(error: T) -> Self {
        Self {
            err: error.into(),
            _marker: marker::PhantomData,
        }
    }
}

impl<T> fmt::Debug for TrappableError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.err.fmt(f)
    }
}

impl<T> fmt::Display for TrappableError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.err.fmt(f)
    }
}

impl<T> Error for TrappableError<T> {}

/// The error type used throughout the filesystem host.
pub type FilesystemError = TrappableError<ErrorCode>;

/// Map a kernel [`VfsError`] to the WASI filesystem error-code.
pub fn vfs_error_to_code(e: VfsError) -> ErrorCode {
    match e {
        VfsError::NotFound => ErrorCode::NoEntry,
        VfsError::NotDir => ErrorCode::NotDirectory,
        VfsError::IsDir => ErrorCode::IsDirectory,
        VfsError::Exists => ErrorCode::Exist,
        VfsError::NotEmpty => ErrorCode::NotEmpty,
        VfsError::PermissionDenied => ErrorCode::Access,
        VfsError::Unsupported => ErrorCode::Unsupported,
        VfsError::Io => ErrorCode::Io,
    }
}
