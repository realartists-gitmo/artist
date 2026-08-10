#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

python3 scripts/verify_generated_packages.py
python3 scripts/verify_foundation_packages.py
python3 scripts/prelabel_verify.py
cargo +1.97.1 fmt --all -- --check
cargo +1.97.1 metadata --format-version 1 --locked >/dev/null
cargo +1.97.1 check --workspace --all-targets --locked
cargo +1.97.1 test --workspace --all-targets --locked
cargo +1.97.1 test --workspace --doc --locked
cargo +1.97.1 clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo +1.97.1 doc --workspace --no-deps --locked
cargo +1.97.1 build --workspace --release --locked
cargo +1.85.0 check --workspace --all-targets --locked
python3 scripts/static_verify.py
sha256sum -c MANIFEST.sha256
echo "Unified Muse verification passed."
