# Muse pre-label contract

Status: **SEMANTIC-V6 GREEN UNDER ALL AVAILABLE TOOLCHAIN-INDEPENDENT GATES. RUST TOOLCHAIN GATE REMAINS REQUIRED AND UNCLAIMED.**

The active learned protocol is `muse-semantic-label-6` with corpus schema `muse-corpus-5`, occurrence compatibility schema `muse-occurrence-7`, training canonicalization `muse-occurrence-training-canonical-7`, and superstrate lowering `muse-superstrate-lowering-10`.

Semantic-v6 uses the separate minimal `LearnedSemanticTarget`; the SLM no longer predicts the rich occurrence/provenance document. It emits singular namespaceless ontology sorts, one ontology-relation channel, exact source grounding, propositions, typed semantic content, grounded values, scoped ontology operators, and explicit ambiguity branches. Deterministic source/context metadata and formal closure are compiler-owned.

Model-visible tool calls, lifecycle updates, and results are learned. Their exact source visibility/alias/opaque/context behavior is pinned by a versioned normalization contract. Requested effects are intensional and cannot become observed effects without evidence. Tool-result execution outcome is separate from result payload semantics.

A label is admitted only after semantic-v6 validation, exact source coverage, reachability/cycle checks, ontology symbol resolution and domain/range checking, deterministic compilation to the rich IR, the semantic-v6 compatibility firewall, canonicalization, formal lowering, ontology compilation, and kernel checking. Conformance does not repair labels.

The normative adversarial gate is `audit/semantic-v6/`. `audit/prose-v4/` and `audit/semantic-v5/` are historical regression evidence and must not define current semantics.

Mass labeling must not begin if the semantic-v6 adversarial/source/ontology/static gates fail. The complete Rust gate (`cargo check`, tests, Clippy, rustdoc, formatting/MSRV as applicable) remains mandatory before final training freeze. It is not marked passed unless executed with the pinned Rust toolchain.
