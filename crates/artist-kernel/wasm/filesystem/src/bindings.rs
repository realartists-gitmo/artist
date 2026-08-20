//! Generated bindings for the kernel's `wasi:filesystem` host (`artist:kernelfs`).
//!
//! The world `kernel-filesystem` imports `wasi:filesystem/types@0.3.0` and
//! `wasi:filesystem/preopens@0.3.0`. On the host side bindgen produces the
//! `Host` / `HostDescriptor` / `HostDescriptorWithStore` / `preopens::Host`
//! traits that [`crate::host`] implements over the kernel [`artist_kernel::Vfs`].
//!
//! The `descriptor` resource is mapped to our own [`crate::descriptor::Descriptor`]
//! (an ino + open flags), so the wasmtime `ResourceTable` stores host data
//! directly rather than an opaque handle.

mod generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "kernel-filesystem",
        imports: {
            "wasi:filesystem/types.[method]descriptor.read-via-stream": store | trappable,
            "wasi:filesystem/types.[method]descriptor.write-via-stream": store | trappable,
            "wasi:filesystem/types.[method]descriptor.append-via-stream": store | trappable,
            "wasi:filesystem/types.[method]descriptor.read-directory": store | trappable,
            default: trappable,
        },
        with: {
            "wasi:filesystem/types.descriptor": crate::descriptor::Descriptor,
        },
        trappable_error_type: {
            "wasi:filesystem/types.error-code" => crate::error::FilesystemError,
        },
        additional_derives: [PartialEq, Eq],
    });
}

pub use generated::wasi::clocks::system_clock;
pub use generated::wasi::filesystem::preopens;
pub use generated::wasi::filesystem::types;
pub use generated::wasi::filesystem::types::Host;
pub use generated::wasi::filesystem::types::HostDescriptor;
pub use generated::wasi::filesystem::types::HostDescriptorWithStore;
