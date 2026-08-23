.PHONY: check fmt plugin fuse-test test

fmt:
	RUSTC_WRAPPER= cargo fmt --all --check
	RUSTC_WRAPPER= cargo fmt --manifest-path plugins/Cargo.toml --all --check

plugin:
	RUSTC_WRAPPER= cargo build --manifest-path plugins/Cargo.toml --workspace --target wasm32-wasip2
	RUSTC_WRAPPER= cargo run -p artist-plugin --example load -- target/wasm32-wasip2/debug/artist_default_plugin.wasm target/wasm32-wasip2/debug/artist_builtin_tools.wasm target/wasm32-wasip2/debug/artist_tool_fixture.wasm target/wasm32-wasip2/debug/artist_ast_fixture.wasm

fuse-test: plugin
	RUSTC_WRAPPER= cargo run -p artist-plugin --example fabric -- target/wasm32-wasip2/debug/artist_builtin_tools.wasm target/wasm32-wasip2/debug/artist_tool_fixture.wasm target/wasm32-wasip2/debug/artist_ast_fixture.wasm

check:
	RUSTC_WRAPPER= cargo check --workspace
	RUSTC_WRAPPER= cargo clippy --workspace --all-targets -- -D warnings

test: fmt plugin check
	RUSTC_WRAPPER= cargo test --workspace
