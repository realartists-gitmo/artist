# Artist default tools

This package contains the default Rust-to-WASM tool component. The component
is registered by the bootstrap under the model-facing names `read`, `write`,
`edit`, `move`, `find`, and `grep`; each call carries its selected tool name in the
host-to-component envelope.

The source of truth is `crates/artist-default-tools/src/lib.rs`. Rebuild the
artifact with:

```text
cargo build -p artist-default-tools --target wasm32-wasip2 --release
```

Then replace `extension.wasm` with the resulting component artifact.
