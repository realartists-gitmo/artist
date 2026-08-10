# UFO-family coverage

## Release profile

Muse 0.2.0 packages the requested UFO family as seven executable ontology modules plus provenance
and English lexicon packages.

| Module | Concepts | Relations | Axioms | Functional coverage |
|---|---:|---:|---:|---|
| UFO-A | 66 | 28 | 23 | endurants, moments, relators, quality, type structure, dependence, mereology |
| UFO-B | 20 | 36 | 11 | events, participations, situations, temporal relations, manifestation, causation |
| UFO-C | 56 | 26 | 17 | agents, intentions, actions, commitments, claims, social relators and norms |
| UFO-MLT | 15 | 15 | 11 | type orders, categorization, characterization, powertypes and partitions |
| UFO-AB | 14 | 19 | 7 | histories, temporal branching, modal support, causal successor bridge |
| UFO-S | 35 | 30 | 16 | offering, negotiation, agreement, delivery, capability, result and failure |
| UFO-L | 60 | 40 | 25 | legal things, norms, acts, relators, conduct/competence positions and cases |
| **Total ontology** | **266** | **194** | **110** | |

The English lexicon adds 460 entries and senses, 194 predicate frames, and 460 terminological
attestations. Provenance contains 12 source/evidence records.

## Axiomatic behavior

The release represents and executes:

- complete/disjoint partitions;
- parent and relation hierarchies;
- domain/range typing and reciprocal inverses;
- relation characteristics;
- qualified minimum/maximum cardinalities;
- existential and universal restrictions;
- causal, participation, social, service, and legal grounding rules;
- closed-world completeness checks where requested.

## Scope boundary

“Complete” means complete for the declared Muse 0.2.0 executable profile and its tests. It does not
mean that the scholarly UFO program is closed, that all possible domain ontologies are bundled, or
that natural language has a finite completed lexicon. New papers, corrected analyses,
jurisdictional law, service-sector specializations, and new lexical evidence must be released as
new source-pinned packages.
