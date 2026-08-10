# Harness and storage integration

## Harness contract

The harness submits `InterpretedGraph` values. It guarantees exact ontology identities and types for every object, noun, verb, relation, predicate, and function. The superstrate validates completeness and deterministic application typing; it does not infer semantic identities.

Quoted graph syntax uses `OntologyTypeExpr::QuotedObject` and `ObjectMeaning::Quoted`.

## Storage contract

Mnestic storage is external. Persist these values directly or inside `ArtifactEnvelope`:

- interpreted submissions and their hashes;
- compiled submissions and exact theories;
- elaboration records;
- certificates and dependency reports;
- observations and empirical artifacts;
- universal runs;
- queries, answers, and both continuation variants.

Storage may index immutable dependencies and supersession relations. It must not rewrite content-addressed artifacts in place.

## End-to-end deductive flow

1. Validate `InterpretedGraph`.
2. Inspect `typing_diagnostics`. Malformed applications remain exact data but cannot be semantically elaborated until corrected or explicitly quoted.
3. Compile the complete submission into a checked base theory.
4. Elaborate the selected object in its exact requested role.
5. Extend the compiled theory append-only with domain facts, definitions, axioms, or theorems.
6. Produce a candidate proof using any procedure.
7. Construct a certificate for the exact elaborated proposition and proof theory.
8. Verify `CertifiedGraphClaim` and its exact dependency report.

## End-to-end result query flow

1. Declare each result variable by an exact graph object denoting its ontology type.
2. Supply an exact condition/specification predicate with the required curried type.
3. Produce graph or formal witnesses.
4. Produce a proof of the fully instantiated predicate.
5. Return `CertifiedQueryWitness` with the certificate and dependency report.
6. Verify the complete `CognitiveAnswer` against the exact query hash, compiled submission, and proof theory.

See `crates/artist-cognition/examples/typed_claim.rs` and `crates/artist-cognition/tests/typed_pipeline.rs`.

## End-to-end empirical certification flow

1. Store every model, dataset, procedure, and claim reference as an `InterpretedGraphRef`, which pins the complete interpreted submission as well as the graph root.
2. Compile the exact claim submission and elaborate the claim root as a proposition.
3. Extend the compiled theory append-only with any explicit premises or proved facts.
4. Attach an `InferenceCertification` containing that elaboration, a certificate for the exact elaborated proposition, and its dependency report.
5. Call `InferenceArtifact::verify`. Only that verified result yields `Exact`, `VerifiedEnclosure`, `CertifiedCoverage`, `Asymptotic`, or `CertifiedOther`; absence of certification yields `Heuristic`.

The CLI exposes `observation-validate`, `posterior-validate`, and `inference-verify` for these contracts.
