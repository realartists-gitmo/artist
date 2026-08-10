# Runtime feature completeness

The runtime architecture is complete for the repository's scope. New domain concepts, logics, proof producers, storage adapters, or empirical engines can be added without changing the trusted calculus.

| Capability | Implemented contract |
|---|---|
| Finite cyclic/self-referential representation | `ObjectGraph` |
| Exact ontology-bearing writes | `InterpretedGraph` |
| Application type diagnostics without semantic loss | `typing_diagnostics` |
| Stable ontology identity | `Ontology::canonical_hash` |
| Deterministic ontology-to-DTT submission compilation | `OntologyCompiler::compile` |
| Deterministic term/proposition/quote elaboration | `OntologyCompiler::elaborate` |
| Recomputable elaboration records | `verify_record` |
| Fixed DTT kernel | `artist-kernel` |
| Append-only theory reuse | `Theory::is_extension_of` |
| Exact proof/dependency chain | `CertifiedGraphClaim` |
| Arbitrary finitary proof rules | `ReflectedProofSystem` |
| Universal checker/computation traces | `UniversalProgram`, `UniversalRun` |
| Rewrites and program traces | `check_rewrite_trace`, `check_execution` |
| Safety/liveness/modal/temporal/coinductive certificates | `artist-theories` |
| Complete theory-package schema and import policy | `TheoryPackage`, `PackageExtensions` |
| Proof-carrying query/status/continuation schema | `CertifiedQueryWitness`, `CognitiveQuery`, `QueryStatus`, `Continuation` |
| Immutable observations and uncertain clock mapping | `artist-empirical` |
| Exact empirical claim references and claim-bound DTT certification | `InferenceArtifact`, `InferenceCertification` |
| Explicit Bayesian/model-comparison contracts | `BayesianModel`, `PosteriorClaim`, `ModelComparison` |
| Canonical versioned persistence envelope | `ArtifactEnvelope` |
| JSON CLI coverage | `artist-cog` |

## Deliberately external components

The following are implementations against published interfaces, not missing superstrate design:

- proof search and synthesis algorithms;
- natural-language parsing and harness ontology construction;
- domain-specific theory packages;
- Bayesian inference algorithms;
- Mnestic storage and indexing;
- optional external mechanized metatheory.

Their outputs remain explicit data and cannot extend kernel trust implicitly.
