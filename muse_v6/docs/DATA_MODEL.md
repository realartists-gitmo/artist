# Data model

## Identity

Every package is identified by `(PackageId, PackageVersion, ContentDigest)`. Concept, relation,
entry, sense, frame, evidence, source, and interpretation identifiers are opaque stable strings.
A label change does not require a new concept identifier; a denotation change does.

## Separation of layers

- Ontology concepts state what entities and relations are.
- Lexical entries state how expressions are formed.
- Lexical senses connect entries to semantic targets.
- Provenance states why a declaration or mapping is accepted.
- Interpretations bind a fixed package snapshot to a semantic graph.

No layer treats a preferred label as the identity of its concept.
