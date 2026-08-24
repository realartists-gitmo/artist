.PHONY: check fmt plugin fuse-test test

fmt:
	RUSTC_WRAPPER= cargo fmt --all --check
	RUSTC_WRAPPER= cargo fmt --manifest-path plugins/Cargo.toml --all --check

plugin:
	RUSTC_WRAPPER= env -C plugins cargo build --workspace --target wasm32-wasip2
	RUSTC_WRAPPER= cargo run -p artist-plugin --example load

fuse-test:
	RUSTC_WRAPPER= cargo test -p artist-resource -- --ignored

check:
	RUSTC_WRAPPER= cargo check --workspace
	RUSTC_WRAPPER= cargo clippy --workspace --all-targets -- -D warnings
	RUSTC_WRAPPER= cargo clippy --manifest-path plugins/Cargo.toml --workspace --all-targets -- -D warnings

test: fmt plugin check
	RUSTC_WRAPPER= cargo test --workspace
	RUSTC_WRAPPER= cargo test --manifest-path plugins/Cargo.toml --workspace
