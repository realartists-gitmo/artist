# Trusted computing base

## Trusted for deductive graph claims

- `artist-kernel::term`, `theory`, and `machine`;
- `artist-kernel::ontology` for deterministic ontology-to-DTT translation;
- certificate and theory deserialization when accepted from storage;
- the Rust compiler, standard library, Serde, BLAKE3 implementation, and platform;
- explicit axioms and oracles, only for dependency chains that use them.

## Not trusted

- natural-language or harness preprocessing beyond its explicit ontology assertions;
- proof search, tactics, external provers, and universal trace producers;
- reflected proof, rewrite, program, modal, temporal, and coinductive libraries as sources of DTT truth;
- Bayesian engines and observation authorities;
- Mnestic storage policy, UI, and orchestration.

`CertifiedGraphClaim` prevents these components from bypassing the kernel: the exact graph elaboration is recomputed and the final DTT certificate must independently pass.
