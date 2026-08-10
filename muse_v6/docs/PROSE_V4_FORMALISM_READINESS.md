# Historical Muse prose-v4 readiness record

Status: **SUPERSEDED by `muse-semantic-label-6`.** This document records an earlier prose-only gate. It is not current pre-label readiness. The 36-window fixture is historical regression evidence; current readiness is defined by `docs/SEMANTIC_LABEL_SPEC.md`, `docs/PRELABEL_CONTRACT.md`, and `audit/semantic-v6/ADVERSARIAL_DRAW.md`.

V4 supersedes the invalid v3 readiness decision. The v3 audit accidentally made the ontology funnel largely ceremonial for learned prose: occurrences/referents could be left untyped, semantic roles were forbidden, `Situation` was overused as a fallback, exact cardinality was source-anchored instead of structural, and the formal lowerer serialized semantic objects as quoted JSON instead of preserving their ontology types. Those rules contradicted the superstrate integration contract.

V4 re-applies the corrected contract to the same fixed 36-window Claude/Grok/Codex adversarial sample. The sample was not replaced.

## What v4 legislates

- Lexical openness and ontological typing are independent. Unknown words remain exact-source-anchored while their semantic objects remain ontology-typed.
- Every learned-prose referent and occurrence carries exactly one narrowest justified pinned ontology concept. Ancestor closure is downstream.
- Every bound quantifier/interrogative variable carries an ontology domain; `Entity` is the final conservative fallback.
- Specialized ontology concepts/relations and pinned lexicon/frame evidence are used when justified. UFO supplies foundational types when no narrower concept is warranted.
- Semantic participant roles are permitted and preferred when source plus pinned semantics justify them; grammatical roles remain valid fallback structure.
- Occurrences carry only their narrowest justified ontology type. The redundant/non-operational learned occurrence `kind` field is absent; event/situation ancestry is derived downstream from the ontology type.
- Open predicate lexical identity remains exact source spans. An ontology type does not replace the source lexeme, and the source lexeme does not replace the ontology type.
- Counterfactual conditionals have a finite structural operator distinct from material implication.
- Exact numeric cardinality has `Exactly`, `AtLeast`, `AtMost`, `MoreThan`, and `FewerThan` operators over exact integer literals. Measurement/data values remain separate semantic cases.
- Structural operators remain source-grounded; ontology classification is intrinsic typing of the represented semantic object, not an invented source proposition.
- Formal lowering preserves referent, occurrence, variable, and literal ontology types in the `InterpretedGraph`. They are no longer demoted to untyped quoted objects. Ontology participant/attribute relation identities are also carried in the formal operation identity rather than surviving only as quoted metadata; ontology participant relations are validated against their declared domain/range.
- Deterministic structured adapters and learned prose converge on the same typed occurrence/proposition IR.

## Fixed regression result

The exact fixture contains 36 windows from 29 sessions: 16 Claude, 11 Grok and 9 Codex; 11 user windows, 20 visible agent windows and 5 agent-reasoning windows.

`python3 audit/prose-v4/scripts/verify_regression.py` reports:

- 36/36 windows pass deterministic rebuild, scope, structure, ontology, and source-coverage checks;
- 611 traversed semantic nodes;
- 332/332 open occurrences ontologically typed;
- 360/360 referent inventory entries ontologically typed;
- 28/28 bound variables have ontology domains;
- 149 occurrences use a concept narrower than the audit's bare `Event`/`Situation` fallback;
- 29 referents use a concept narrower than `Entity`/`Object`/`Agent`;
- 3 exact numeric-cardinality structures;
- 32 grammatical operator profiles;
- 3 explicitly licensed local ellipses;
- 2 retained source ambiguities;
- source-span semantic coverage minimum 82.2% (sample 4), mean 95.0%.

The v4 verifier loads the pinned ontology packages, derives the globally unique namespaceless learned vocabulary, and checks every emitted concept/relation symbol against it. The few source-local collisions are explicitly renamed; any new unresolved collision fails the registry/pre-label gate. Authoritative package-qualified IDs remain internal and are restored deterministically before ontology conformance and formal lowering. Coverage remains a backstop, not a semantic proof.

## Retained ambiguities

Only two unresolved readings remain in the fixed sample:

1. Sample 3: the referent of `that` can plausibly be the pref composite, the anticipated test-league data/output, or the E-adjustment context.
2. Sample 29: `You can even take screenshots...` does not force capability versus permission/option.

The protocol retains these explicitly rather than forcing an annotator choice.

## Source-static gate

`python3 scripts/prelabel_verify.py` passes and includes the v4 source-shape verifier plus the fixed v4 regression.

This readiness level deliberately does not move the repository's separately defined final Rust/release gate. The current execution environment used for this correction does not provide a Rust toolchain, so Cargo/rustfmt/Clippy/rustdoc/MSRV compilation was not rerun here. That gate remains mandatory immediately before training-contract freeze/GPU use; any semantic code change required by that gate invalidates this readiness verdict and requires rerunning the same v4 regression.

The future dataset-specific transcript adapter is only a parser into the frozen generic `TranscriptRecord`/window contract. It may not change label semantics.
