//! `artist-wasm-filesystem` — the kernel's `wasi:filesystem` host.
//!
//! Per `kernel-wasm-scaffold.md`, the kernel does not use wasmtime-wasi's
//! cap-std filesystem host (which is real-directory based). Instead it binds the
//! `wasi:filesystem` WIT directly and implements the host over the kernel's
//! virtual [`artist_kernel::Vfs`], so extensions can access capability-scoped
//! namespaces through ordinary WASI file APIs.
//!
//! The embedder stores a [`FilesystemCtx`] plus a
//! [`wasmtime::component::ResourceTable`] in its [`Store`](wasmtime::Store)
//! data, implements [`FilesystemView`] to project a [`FilesystemCtxView`], and
//! calls [`add_to_linker`].

pub mod bindings;
pub mod descriptor;
pub mod error;
pub mod host;
pub mod streams;

use wasmtime::component::Linker;

pub use host::{FilesystemCtx, FilesystemCtxView, FilesystemHost, FilesystemView};

/// Add the `wasi:filesystem/types` and `wasi:filesystem/preopens` imports to
/// `linker`, backed by the kernel [`artist_kernel::Vfs`].
///
/// `T` is the embedder's store data; it must implement [`FilesystemView`].
pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>
where
    T: FilesystemView + 'static,
{
    bindings::types::add_to_linker::<_, FilesystemHost>(linker, T::filesystem)?;
    bindings::preopens::add_to_linker::<_, FilesystemHost>(linker, T::filesystem)?;
    Ok(())
}
