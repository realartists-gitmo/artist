# Canonical artifacts

`ArtifactEnvelope` stores:

- stable kind;
- kind-specific format version;
- JSON payload;
- BLAKE3 digest over kind, version, and canonical payload bytes.

Canonical JSON recursively sorts object keys, preserves array order, and emits deterministic scalar encodings. Decoding verifies digest, kind, and format before deserialization.

## Current format versions

| Artifact | Version |
|---|---:|
| `artist.formal.interpreted-graph` | 3 |
| `artist.kernel.compiled-ontology` | 3 |
| `artist.kernel.elaboration` | 3 |
| `artist.cognition.certified-claim` | 3 |
| `artist.cognition.query` | 4 |
| `artist.cognition.answer` | 4 |
| `artist.empirical.observation` | 2 |
| `artist.empirical.inference-artifact` | 2 |
| `artist.empirical.posterior` | 2 |
| `artist.cognition.discovery-continuation` | 3 |
| `artist.cognition.kernel-continuation` | 2 |
| Other currently registered artifact kinds | 1 |

Interpreted-graph and ontology compilation version 3 include the universal `QuotedObject` type and full interpreted-submission identity. Query and answer version 4 additionally require semantic references in empirical requests and results to pin the complete interpreted submission rather than only a raw graph. Empirical observation, inference-artifact, and posterior version 2 use the same exact reference contract. Inference-artifact version 2 also binds certification to a recomputable elaboration of the exact interpreted claim and its complete dependency closure. Discovery-continuation version 3 requires the concrete selected theory. Kernel-continuation version 2 adds the canonical query hash and complete interpreted-submission identity.

Persisted or transported values should use `ArtifactEnvelope`. Raw Serde JSON is suitable only inside a single controlled component. Unknown versions are rejected rather than inferred or silently migrated.
