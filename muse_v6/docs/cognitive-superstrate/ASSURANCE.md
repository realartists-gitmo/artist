# Assurance boundary

## Runtime guarantees implemented in Rust

- exact total ontology coverage for harness writes;
- deterministic ontology-to-DTT compilation and elaboration records;
- checked append-only theories and content identities;
- explicit DTT certificate checking and dependency closure;
- deterministic serializable checker continuations;
- deterministic finite universal traces;
- canonical artifact integrity checks.

## Trusted computing base

Deductive acceptance trusts the Rust kernel implementation, ontology compiler for the graph-to-DTT mapping, serialization used for accepted artifacts, compiler/runtime platform, and any explicitly listed axioms or oracles.

The harness is trusted only for the ontology identity/type assertions it supplies. Those assertions are retained and auditable; they are not inferred by the kernel.

Proof search, universal trace producers, reflected checkers, solvers, Bayesian engines, observation sources, storage, and UI are not trusted for deductive acceptance.

## External mechanized assurance

A proof assistant may separately establish metatheorems or executable refinement. Such work is valuable but is not a runtime dependency and is not permanently bundled in this project. Any future assurance artifact should target the exact released Rust calculus and identify the source revision it proves.

Open assurance work does not imply an open runtime design fork. It changes confidence in the implementation, not the implemented semantics.
