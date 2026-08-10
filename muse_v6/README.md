# Muse

Muse is a single Rust workspace combining the semantic foundation and the verified cognitive superstrate.

The workspace now contains both halves as sibling crates under one Cargo workspace. The cognitive crates retain their historical `artist-*` package names so the verified source is not gratuitously rewritten; those crates are part of Muse and do not imply a runtime dependency on the Artist harness.

## Architecture

```text
Muse
├── semantic foundation
│   ├── ontology / UFO packages
│   ├── provenance and registry
│   ├── classification and resolution
│   ├── interpretation and validation
│   └── canonical I/O
├── cognitive superstrate
│   ├── exact formal graph representation
│   ├── fixed dependent-type kernel
│   ├── theory packages and finite certificate checkers
│   ├── empirical / Bayesian contracts
│   └── certified cognition/query artifacts
├── training-critical integration
│   ├── software / computing / agent / harness / Artist ontology funnel
│   ├── shared perspective-neutral occurrence/proposition IR
│   ├── generic structured-tool formalization + pinned Artist adapter
│   ├── deterministic semantic-to-superstrate lowering + DTT check
│   └── prose-window, conformance, quarantine, and split tooling
└── post-training product work
    └── Mnestic migration / persistence integration
```

See `docs/MUSE_COMPLETION_AND_WEB_HANDOFF.md` for the ordered remaining work.

## Workspace crates

Semantic crates:

- `muse-core`
- `muse-provenance`
- `muse-ontology`
- `muse-lexicon`
- `muse-registry`
- `muse-classification`
- `muse-reasoning`
- `muse-resolution`
- `muse-interpretation`
- `muse-occurrence`
- `muse-tooling`
- `muse-artist-adapter`
- `muse-superstrate`
- `muse-training`
- `muse-validation`
- `muse-io`
- `muse`
- `muse-cli`

Cognitive crates:

- `artist-formal`
- `artist-kernel`
- `artist-empirical`
- `artist-theories`
- `artist-cognition`
- `artist-cog`

The `muse` facade exposes the cognitive crates behind the `formal`, `kernel`, `empirical`, `theories`, and `cognition` features. The deterministic occurrence-to-superstrate bridge is exposed by the `superstrate` feature; pre-label corpus/conformance tooling is exposed by `training`.

## Verification

Run:

```bash
./scripts/verify.sh
```

or in Fish:

```fish
fish scripts/verify.fish
```

The unified verification script covers the complete Cargo workspace plus the deterministic UFO package checks and structural audits for both semantic and cognitive halves.

This environment does not contain a Rust toolchain. Toolchain-independent semantic/cognitive/pre-label checks are run here; the complete locked Cargo/fmt/test/Clippy/rustdoc/MSRV gate remains intentionally deferred to the final pre-training verification boundary. See `BUILD_STATUS.md`.
