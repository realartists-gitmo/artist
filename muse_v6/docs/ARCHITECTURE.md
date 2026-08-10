# Architecture

```text
source evidence
      |
      v
muse-provenance
      |
      +-------------------+
      |                   |
      v                   v
muse-ontology       muse-lexicon
      |                   |
      +---------+---------+
                v
          muse-registry
          /      |      \
         v       v       v
classification reasoning resolution
         \       |       /
          \      v      /
          muse-interpretation
                  |
                  v
            muse-validation
                  |
                  v
               muse-io
```

The dependency graph is one-way. Ontology declarations do not depend on lexical entries. Lexical
senses point to concepts or relations. The registry is immutable per `(package ID, version,
digest)` and produces exact content-bound snapshots.

`muse-reasoning` works on neutral ontology facts and does not parse language. It consumes an
`OntologyIndex` and a `KnowledgeBase`, computes deterministic closure, and reports conformance
violations. `muse-validation` can project an `InterpretationBundle` into that neutral fact model.

A future superstrate adapter depends on Muse and the superstrate. Neither clean half depends on
the adapter.
