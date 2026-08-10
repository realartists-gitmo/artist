# Build status

## Assembly inputs

- `muse-semantic-foundation-ufo-complete.zip` SHA-256: `18b2af0e327b2faa2e800b5e89ae33f3949e6668b69f57ef167626ece6b1ac67`
- `artist-cognitive-superstrate-verified.zip` SHA-256: `72eb93039a076090ff29b68dd1632d4352526cb61f281777cd02611cee2de2de`

The `MANIFEST.sha256` from each extracted input was verified successfully before merging. Copies are retained under `docs/provenance/`.

## Unified-workspace status

The semantic and cognitive crates are members of one top-level Cargo workspace. The training-critical integration layer is now present: ontology funnel, shared occurrence IR, protocol-neutral tool model, pinned Artist adapter, deterministic superstrate lowering, and pre-label corpus/conformance tooling.

The assembly environment had no Rust toolchain, so no claim is made here that the newly merged workspace was compiled, tested, linted, documented, or checked at the MSRV. Run `scripts/verify.sh` or `scripts/verify.fish` in the target environment.

Toolchain-independent combined static verification is expected to be run before packaging, and `MANIFEST.sha256` is regenerated over the final combined tree.

## 2026-08-06 web-session continuation

Baseline hardening completed before semantic work:

- all historical `artist-*` crates now inherit `rust-version.workspace = true`, matching the workspace MSRV declaration (`1.85`);
- `scripts/verify.sh` and `scripts/verify.fish` are non-formatting and no longer regenerate `MANIFEST.sha256`;
- explicit formatting maintenance entry points are `scripts/format.sh` and `scripts/format.fish`;
- generated UFO verification now renders into a temporary directory and compares bytes/hashes without rewriting `packages/ufo/`;
- the required external ontology/interoperability audit is recorded in `docs/ONTOLOGY_SOURCE_AUDIT.md`.

This environment still has no Rust toolchain. Per the accelerated training plan, the complete Cargo/rustfmt/Clippy/rustdoc/MSRV verification gate remains the final pre-training gate rather than a blocker for semantic implementation. Toolchain-independent verification covers the 24-member workspace structure, deterministic generated packages, ontology references, the exact Artist Gortnite pin, all 37 pinned event kinds, all 31 pinned built-in tools, and the pre-label contract. No claim of Rust compilation is made here.

## Semantic-v6 labeling readiness

The active pre-label contract is `muse-semantic-label-6` / `muse-corpus-5`. The SLM-facing `LearnedSemanticTarget` is separate from the rich provenance IR, singularly ontology-typed, namespaceless, tool-aware, exact-source-grounded, and gated by `audit/semantic-v6/`. Requested/observed effects, tool lifecycle/result identity, semantic content vs propositions, ontology-backed scope operators, exact numeric/value semantics, ambiguity branches, and source completeness are enforced structurally. All available toolchain-independent gates pass; the complete pinned Rust toolchain gate remains mandatory and unclaimed because this environment has no Rust toolchain.
