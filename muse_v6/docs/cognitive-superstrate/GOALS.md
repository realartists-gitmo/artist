# Normative goals

## Exact representation and interpretation

**G1 — Universal finite representation.** Every finitely encoded graph, including descriptions of infinite objects or behavior, can be retained without changing the graph schema.

**G2 — Open vocabulary.** New nouns, verbs, predicates, relations, functions, modalities, and domain types are ontology/theory data, not Rust enum additions.

**G3 — Total structural interpretation.** Every harness write is an `InterpretedGraph`: every graph object has one exact ontological type and denotation. Remaining ambiguity, when intentionally present, is represented explicitly as content.

**G4 — Exact ontological typing.** The harness identifies every submitted noun and verb and supplies exact operation roles and result types. The superstrate does not perform semantic guessing.

**G5 — Separate formal well-typedness.** An exactly interpreted graph may still contain an ontologically ill-typed application, a semantic cycle, or content unavailable in a requested DTT role. These are diagnostics, not failures of interpretation.

**G6 — Infinity by finite description.** Quantification, recursive definitions, coinductive behavior, infinite domains, and nontermination claims are represented intensionally by finite objects and checked through finite certificates.

## Fixed proof calculus

**G7 — One trusted calculus.** Deductive acceptance uses one fixed predicative dependent type theory with universes, Pi, Sigma, identity/J, strictly positive inductive families, and terminating definitional computation.

**G8 — Mechanical ontology embedding.** Ontological types become DTT types; nouns become constants; verbs become curried functions or predicates; the distinguished proposition type maps to `Type 0` under propositions-as-types.

**G9 — Theory-specific content, not theory-specific meaning.** Domain theories add declarations, definitions, facts, axioms, oracles, and metatheorems. They do not reinterpret the harness ontology.

**G10 — Exact acceptance meaning.** Acceptance establishes exactly `T ; Γ ⊢ proof : proposition` for the recorded theory, context, proposition, and proof.

**G11 — Dependency disclosure.** Every accepted result exposes its exact theory, local assumptions, transitive declaration dependencies, declaration kinds, and provenance.

**G12 — Append-only extension.** Existing checked declarations never change. Facts and theorems extend the exact compiled submission theory without changing ontology-derived meanings or deterministic elaborations.

**G13 — Theory isolation.** Cross-theory use requires a checked translation or explicit bridge theorem.

## Universal certificate and computation support

**G14 — Finite universal traces.** A deterministic register/stack machine represents arbitrary finite proof checkers and computations. A certificate contains the full initial state and every claimed successor state.

**G15 — No verifier bypass.** Universal trace acceptance is proof-production evidence. A deductive graph claim is accepted only when a matching DTT certificate passes the kernel.

**G16 — Honest nontermination.** Proof search and programs may run indefinitely. Paused, cancelled, resource-limited, exhausted, and unknown outcomes remain distinct from disproof.

**G17 — Separate continuations.** Kernel-checker continuations and discovery/computation continuations have distinct exact schemas and identity pins.

## Empirical cognition

**G18 — Observation/content separation.** An observation records that a source emitted content at a time; it does not prove that content.

**G19 — Immutable revision.** Supersession creates a new event and preserves prior records and historical dependencies.

**G20 — Explicit Bayesian conditions.** Posterior and model-comparison claims retain the model, prior, likelihood, dependence assumptions, latent/causal structure, exact dataset, procedure, approximation class, diagnostics, and error or convergence claims.

**G21 — No probability-to-proof collapse.** Probabilistic output becomes a deductive premise only through an explicit decision or bridge rule.

## Persistence and reproducibility

**G22 — Canonical artifacts.** Versioned envelopes use sorted canonical JSON and BLAKE3 integrity digests. Unknown future formats are rejected rather than guessed.

**G23 — Determinism.** Identical checked theory, request, and continuation state produce identical transitions and results.

**G24 — No hidden authority.** Axioms, oracles, observations, external procedures, approximations, and proof producers are always explicit.

## Non-goals

- Semantic omniscience or automatic discovery of every true claim.
- Extensional storage of actually infinite collections.
- Guaranteed termination of proof search, synthesis, or arbitrary programs.
- Unrestricted internal truth predicates or cyclic kernel proof terms.
- Silent trust in the harness, solvers, universal verifier library, Bayesian engines, storage, or user interfaces.
