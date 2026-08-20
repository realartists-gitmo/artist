# Trusted system components

Artifacts in this directory are bootstrapped by the native host and are not
part of the model-visible URL composition.

Rebuild the trusted root from source with:

```sh
cargo build -p artist-root --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/artist_root.wasm extensions/system/root.wasm
```
