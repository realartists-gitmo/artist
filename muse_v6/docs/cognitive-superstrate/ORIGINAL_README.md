# Artist Cognitive Superstrate

A Rust workspace for exact ontology-bearing cognition and proof-carrying formal reasoning.

The harness write boundary is `artist_formal::InterpretedGraph`. Every submitted graph object already has an exact ontological type and denotation. The superstrate does not guess noun or verb meanings. It mechanically embeds that ontology into one fixed dependent type theory, then checks explicit proof certificates under exact theory identities.

A finite graph may describe infinite subject matter: quantified domains, infinite sets, streams, nonterminating programs, ordinal/cardinal claims, and infinite behavior represented by finite definitions or certificates. The representation itself remains finite and content-addressed.

## Workspace

| Crate | Responsibility |
|---|---|
| `artist-formal` | Finite cyclic graph representation, exact ontology declarations, total object interpretation, open queries |
| `artist-kernel` | Fixed dependent-type calculus, checked theories, deterministic resumable checking, ontology-to-DTT compilation |
| `artist-theories` | Universal finite-trace verifier, reflected calculi, rewriting, program traces, temporal/modal/coinductive checkers, theory packages |
| `artist-empirical` | Immutable observations, clock relations, explicit Bayesian/model-comparison contracts, certified procedure artifacts |
| `artist-cognition` | End-to-end certified graph claims, proof-carrying query/result contracts, canonical artifact envelopes |
| `artist-cog` | JSON command-line interface |

## Exact pipeline

```text
harness-supplied InterpretedGraph
    -> deterministic ontology-to-DTT compilation
    -> deterministic graph-object elaboration
    -> append-only domain theory/facts
    -> candidate proof production
    -> kernel certificate check
    -> exact dependency report
```

Structural interpretation is total. A graph may still be ill-typed as an application, cyclic as a semantic term, unproved, undecidable, or unavailable in a requested formal role. Those are explicit diagnostics; none turns the graph into “uninterpreted” data.

## Verification

```bash
./scripts/verify.sh
python3 scripts/static_verify.py
```

The first command runs formatting, the complete Rust test suite, and Clippy with warnings denied. The second performs toolchain-independent structural checks. This package contains no permanent Lean workspace; external proof assistants may be used separately for assurance work without becoming runtime dependencies.

See `docs/ARCHITECTURE.md`, `docs/ONTOLOGY.md`, `docs/KERNEL.md`, and `docs/ASSURANCE.md`.
