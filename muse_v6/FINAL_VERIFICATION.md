# Semantic-v6 final source verification

Date: 2026-08-08

Active contract: `muse-semantic-label-6` / `muse-corpus-5` / `muse-occurrence-7` / canonicalization 7 / lowering 10.

## Verified in this continuation

The final source tree was checked without modifying the semantic contract.

Passed:

- deterministic generated UFO package verification;
- foundation package verification: 7 packages, 226 concepts, 183 relations;
- semantic-v6 source-contract verification;
- semantic static verification: 25 Rust files, 9 sealed packages, 266 concepts, 194 relations, 110 axioms;
- combined static verification: 24 workspace crates;
- fixed prose-v4 regression deterministic rebuild and structural checks;
- prose-v4 source coverage: 36 windows, minimum 82.2%, mean 95.0%;
- semantic-v6 adversarial audit against the original transcript archive: 12/12 pinned cases, including exact source path, line, raw byte length, SHA-256, and source needle;
- Python bytecode compilation for `scripts/` and `audit/`;
- Bash syntax checks for every `scripts/*.sh` entry point;
- root `MANIFEST.sha256` verification after final manifest regeneration.

Exact adversarial source archive used:

`agentic-sessions-big-37(2).zip`

SHA-256:

`949f9996b2287e521a7348d1e6cf6e0d08ca965172b51936ce20dbabb1232b88`

## Rust execution gate

This execution environment does not contain `rustc`, `cargo`, `rustup`, `rustfmt`, or Clippy. Network restrictions also prevent installing the pinned toolchains here. Therefore the executable Rust gate remains deliberately unclaimed:

- Rust 1.97.1 formatting;
- locked metadata;
- workspace `check`;
- workspace tests and doctests;
- Clippy with warnings denied;
- rustdoc with warnings denied;
- release build;
- Rust 1.85.0 MSRV check.

Run `scripts/verify.sh` in an environment with the pinned Rust toolchains before the training-contract freeze. No archive claiming that executable gate passed is produced here.
