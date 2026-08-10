# Conformance

## Required commands

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/static_verify.py
```

`./scripts/verify.sh` runs the first three commands.

## Required positive behavior

A conforming build must support exact interpreted graphs, ontology hashing, ontology compilation, proposition and quote elaboration, append-only proof theories, certified graph claims, universal accepting traces, arbitrary computation traces, proof-carrying query answers and complete statuses, immutable observation ledgers, Bayesian contracts, package validation, canonical artifacts, and resumable kernel checking.

## Required rejection behavior

A conforming build rejects incomplete ontology coverage, unknown symbol identities, mismatched direct-symbol types, malformed ontology applications during semantic compilation, generated-name conflicts, cyclic semantic-term unfolding, altered elaboration records, theory/certificate mismatches, incomplete dependency reports, invalid universal transitions, wrong outputs, invalid package imports/extensions, corrupted artifacts, malformed clocks/observations, and mismatched answers or continuations.

Cycles, quotation, contradiction, uncertainty, and lack of a proof are not themselves representation failures.

Empirical validation rejects semantic references that mismatch the complete interpreted submission and certificates that do not prove the deterministic elaboration of the exact reported claim.
