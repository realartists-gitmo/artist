# Muse SLM training critical path

Status: authoritative accelerated-training contract. The active generic pre-label implementation is `muse-semantic-label-6` / `muse-corpus-5` with the minimal ontology-first learned target and the real-transcript semantic-v6 adversarial gate. The historical prose-v4 fixture remains a regression subset only. All available toolchain-independent gates are green; the complete pinned Rust gate remains mandatory immediately before final training-contract freeze and GPU training.

## Objective

Reach a production-quality, version-frozen Muse prose-to-formal training contract without reducing semantic or formal scope. Elapsed time is compressed by dependency-aware parallel work, not by omitting required design, ontology, validation, or evaluation work.

## Governing decisions

1. Existing Muse components may be redesigned when there is a concrete correctness, semantic, formal, interoperability, or training-quality reason. Compatibility is preserved by explicit versioning, not by retaining a subpar design.
2. The ontology funnel is completed in order: software engineering -> computing -> AI/software agents -> generic coding harness -> Artist-specific adapter vocabulary.
3. The SLM predicts the minimal semantic-v6 `LearnedSemanticTarget`; deterministic compilation maps it into the shared rich occurrence/proposition IR used by deterministic adapters and the formal superstrate. The model is never trained to reproduce rich provenance/debug metadata.
4. Artist-specific source authority is `realartists-gitmo/artist`, branch `Gortnite`. `main` is not authoritative for this work.
5. Tool-call formalization has a generic protocol-neutral form. Artist mappings specialize that form. Future harnesses add adapters without changing the shared semantic language.
6. Model-visible structured tool calls/results/updates are SLM learning targets by default. Deterministic decoding remains an independent safety/recovery path and may recover any semantics that can be established mechanically.
7. SLM training targets both prose and normalized model-visible structured records -> the same semantic-v6 learned language. Large coding-agent transcript corpora remain suitable sources; source-specific adapters must pin model visibility and normalization exactly.
8. Lexicon/resolution machinery is optional lexicalization support only. Open nouns and predicates are preserved by exact source anchors on singularly ontology-typed referents/occurrences, so unknown words never require untyped structures. It must not impose a finite-lexicon bottleneck.
9. Mnestic/RocksDB migration is required for final Muse product completion but is not a blocker for SLM training.
10. Final Muse facade/crate naming/release-surface cleanup is required for final product completion but is not a blocker for SLM training.
11. The complete Rust/release verification suite is the final pre-training gate, immediately before the training contract is frozen and H200 budget is spent. Toolchain-independent checks run continuously during development.

## Perspective-neutral semantic rule

Muse never needs a privileged metaphysical narrator. Proposition content is represented independently from the occurrence or stance that presents it.

Examples:

- User prose `I thought X` -> a thought/attitude presentation whose holder is the user and whose content is proposition X.
- Agent prose `I thought X` -> a thought/attitude presentation whose holder is the agent and whose content is proposition X.
- `Maybe X` -> a hypothesis/possibility presentation of proposition X, not an assertion that X is true.
- `X happened` -> an assertion/report whose content presents an event occurrence.
- A structured tool result reporting success -> a tool-result occurrence with a reported success status. It does not independently assert every requested external world effect occurred unless the record provides evidence for that effect.

Truth, proof, belief, hypothesis, command, question, desire, intention, quotation, observation, and report status attach to an actor/source or formal judgment. They are never silently collapsed into an unqualified global fact.

## Pre-training dependency order

1. Source-pin ontology/interoperability references.
2. Complete ontology funnel through generic coding harness; add only necessary Artist-specific vocabulary from `Gortnite`.
3. Complete shared perspective-neutral occurrence/proposition IR and canonicalization.
4. Specify generic deterministic tool-event formalization.
5. Audit Artist `Gortnite` schemas and implement the Artist adapter plus exhaustive fixtures.
6. Implement semantic -> fixed-superstrate lowering.
7. Freeze prose-window annotation semantics against the same occurrence language.
8. Build transcript ingestion, boundary filtering, label import/revision, canonicalization, conformance QA, quarantine, hashes, provenance, and leakage-safe split tooling. The source-specific adapter for the selected open transcript database is added once that database/schema is named; the generic normalized transcript contract is already fixed.
9. Build and audit training/evaluation corpora from open coding-agent prose plus targeted hand-authored/adversarial fixtures.
10. Run complete Rust/release/end-to-end verification.
11. Freeze ontology snapshots + occurrence schema + tool rules + label rules + lowering + serialization as one training contract version.
12. Train/evaluate on H200; reject any checkpoint/artifact that fails semantic parse, ontology validation, deterministic lowering, kernel checking, or held-out semantic evaluation.

## Post-training required completion

- Mnestic/RocksDB migration and migrations/round-trips.
- Final Muse facade/product surface, compatibility naming, and release policy.
- Full product archive and final verification evidence after those pieces land.
