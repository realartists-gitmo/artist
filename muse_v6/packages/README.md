# Bundled packages

`packages/ufo/` is the executable Muse UFO-family baseline.

| File | Scope |
|---|---|
| `ufo-provenance.muse.json` | 12 primary-source and evidence records |
| `ufo-a.muse.json` | Endurants, types, moments, relators, qualities, dependence and mereology |
| `ufo-b.muse.json` | Events, participation, situations, time, manifestation, change and causation |
| `ufo-c.muse.json` | Agents, intentions, actions, commitments, claims and social relators |
| `ufo-mlt.muse.json` | Type orders, categorization, characterization, powertypes and partitions |
| `ufo-ab.muse.json` | Branching histories and the temporal/modal A/B bridge |
| `ufo-services.muse.json` | Offering, negotiation, agreement, delivery, result and service roles |
| `ufo-legal.muse.json` | Norms, legal acts/relators, Hohfeldian conduct and competence positions |
| `ufo-en-lexicon.muse.json` | English entries, senses, attestations, and relation predicate frames |

The generated packages are immutable and content-pinned. Their generator is
`scripts/generate_ufo_packages.py`; bootstrap files under `packages/sources/bootstrap/` are
retained only as reproducible source inputs and are not loaded as released packages.

This profile is complete for its declared release scope, not for the unbounded future Muse
ontology/lexicon. Artist harness and computing ontologies remain separate authoritative imports.
