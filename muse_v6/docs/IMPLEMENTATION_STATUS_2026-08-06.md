# Muse implementation status — 2026-08-06

This file records the current training-critical implementation state. It is not a release-verification claim.

## Completed in the current tranche

- Repaired workspace Rust-version inheritance and made verification scripts non-mutating.
- Built and verified the six-package semantic funnel: provenance, software, computing, agent, generic coding harness, Artist.
- Pinned Artist `Gortnite` to commit `656383b4906a796727b09a249ef5da7e60f51b81` and audited the frozen session envelope/event schema plus exhaustive 31-tool registry.
- Added `muse-occurrence`, the perspective-neutral shared occurrence/proposition IR.
- Added `muse-tooling`, the protocol-neutral deterministic structured-tool normalization layer.
- Added `muse-artist-adapter`, which joins Artist model-turn/tool-result/change records without treating reported success as observed external effects and extracts non-tool prose separately for SLM corpus construction.
- Added semantic training canonicalization for occurrence documents. It removes producer-local ID/alpha/commutative variance while preserving perspective and source identity; invalid scope and recursive occurrence structures are rejected.

## Intentionally not claimed yet

- Rust compilation, tests, Clippy, rustdoc, MSRV, release builds, or locked dependency verification. The current environment has no Rust toolchain, and the full Rust gate belongs immediately before training-contract freeze.
- Complete Artist non-tool event specialization.
- Semantic-to-fixed-superstrate lowering.
- Frozen prose-label protocol.
- Corpus ingestion/labeling/evaluation set construction.
- H200 training or model evaluation.

## Immediate dependency order

1. Complete the deterministic semantic-to-formal bridge and kernel-check path.
2. Complete Artist structured event specialization where it contributes semantics beyond generic tool normalization.
3. Freeze the prose-label protocol against the canonical occurrence target and formal lowering contract.
4. Build/import the non-tool coding-agent prose corpus, label it, quarantine nonconforming cases, and audit leakage/coverage.
5. Run the full Rust/release verification gate.
6. Freeze dataset/ontology/schema/lowering identities and train/evaluate the SLM.
