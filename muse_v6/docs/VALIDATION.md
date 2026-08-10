# Validation

Validation occurs in independent layers:

1. Package-local identity and structural validation.
2. Canonical envelope and package-content digest verification.
3. Exact import closure and immutable registry conflict validation.
4. Ontology reference, hierarchy, inverse, cardinality, partition, restriction, and rule checks.
5. Lexicon target, predicate-frame, attestation, and total coverage checks.
6. Provenance and evidence-reference validation.
7. Deterministic reasoning and interpretation conformance.
8. Competency, negative, and regression tests across all imported UFO domains.
9. Static workspace, source-generator, package-count, and repository-manifest verification.
10. Cargo formatting, unit/integration/doc tests, strict Clippy, docs, release build, and MSRV.

`WorldAssumption::Open` reports contradictions and upper-bound violations without treating missing
facts as false. `WorldAssumption::Closed` additionally enforces minimum cardinalities, existential
restrictions, and complete-partition coverage for release/test datasets that claim completeness.
