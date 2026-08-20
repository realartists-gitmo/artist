//! artist-vfs-windows — bridges the platform-neutral [`artist_kernel::Vfs`] out
//! to WinFsp on windows hosts.
//!
//! The driver is built a priori: it is cfg-gated to `windows` and cannot be
//! compiled or tested on this (Linux) host. The non-windows build provides a
//! stub `mount` so the workspace stays green everywhere.

use std::path::Path;
use std::sync::Arc;

use artist_kernel::Vfs;

/// Mount `vfs` at `mountpoint`, marking the volume read-only when `readonly`.
///
/// Returns a guard that keeps the volume mounted; dropping it stops the WinFsp
/// dispatcher and unmounts.
#[cfg(not(windows))]
pub fn mount(
    _vfs: Arc<dyn Vfs>,
    _mountpoint: &Path,
    _readonly: bool,
) -> std::io::Result<WinFspMount> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "WinFsp mounts are only supported on windows",
    ))
}

/// Opaque guard returned by [`mount`] on non-windows hosts.
#[cfg(not(windows))]
pub struct WinFspMount(());

#[cfg(windows)]
mod driver;
#[cfg(windows)]
pub use driver::WinFspMount;
#[cfg(windows)]
pub use driver::mount;
