# Default prompt extension

This package owns the default `prompt://` composition policy. It receives
session inputs and returns the initial model-context snapshot through the
`artist:composition/extension` contract.

Rebuild with:

```text
cargo build -p artist-default-prompt --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/artist_default_prompt.wasm extension.wasm
```
