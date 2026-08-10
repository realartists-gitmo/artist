#!/usr/bin/env fish

function run_step
    echo "==> $argv"
    command $argv
end

function main
    set root (realpath (dirname (status filename))/..)
    cd $root; or return 1

    run_step python3 scripts/verify_generated_packages.py; or return 1
    run_step python3 scripts/verify_foundation_packages.py; or return 1
    run_step python3 scripts/prelabel_verify.py; or return 1
    run_step cargo +1.97.1 fmt --all -- --check; or return 1
    run_step cargo +1.97.1 metadata --format-version 1 --locked; or return 1
    run_step cargo +1.97.1 check --workspace --all-targets --locked; or return 1
    run_step cargo +1.97.1 test --workspace --all-targets --locked; or return 1
    run_step cargo +1.97.1 test --workspace --doc --locked; or return 1
    run_step cargo +1.97.1 clippy --workspace --all-targets --locked -- -D warnings; or return 1
    env RUSTDOCFLAGS="-D warnings" cargo +1.97.1 doc --workspace --no-deps --locked; or return 1
    run_step cargo +1.97.1 build --workspace --release --locked; or return 1
    run_step cargo +1.85.0 check --workspace --all-targets --locked; or return 1
    run_step python3 scripts/static_verify.py; or return 1
    run_step sha256sum -c MANIFEST.sha256; or return 1

    echo "Unified Muse verification passed."
end

main
