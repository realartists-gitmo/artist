# Muse semantic-label specification

Status: normative for `muse-semantic-label-6`.

Semantic-v6 is the only active labeling contract. Earlier prose-v4 and semantic-v5 material is historical regression evidence, not normative label syntax.

## 1. Learned task

The SLM maps one model-visible source window to `LearnedSemanticTarget` from `muse-training::semantic_v6`.

The learned target contains only independent semantic facts the model must infer:

- exact local source spans, inlined directly in anchors (no learned span IDs);
- singular ontology-typed referents, occurrences, and variables;
- truth-valued propositions;
- non-truth-valued semantic content such as questions/directives/quotations;
- exact grounded scalar/composite values;
- explicit ambiguity alternatives;
- semantic roots.

The SLM does **not** predict source IDs, run/message IDs, record IDs, speaker/actor/recorder identities supplied by the window, ontology snapshots, package namespaces, ontology ancestors, derivation metadata, evidence records, schema versions, canonical IDs, formal closure, or any other deterministic metadata.

The harness compiles the learned target into the rich occurrence/provenance IR and then into the formal/kernel representation. Deterministic tool parsing is an independent safety/recovery path, not an exclusion rule for training data.

## 2. Universal ontology typing

Every learned referent, occurrence, and variable has exactly one `sort`: the narrowest globally canonical ontology concept justified by the source and pinned lexical/ontology evidence.

No ancestor duplication is emitted. If `FileDelete <: Event`, emit `FileDelete`, not both. If no narrower classification is justified, back off to the narrowest foundational concept justified by the construction.

Open vocabulary is preserved with exact lexical anchors. An unknown noun or verb may remain lexically open while still receiving a foundational ontology sort and source-grounded semantic relations.

`TypeAssertion` is source content asserting a type. It is not a second channel for intrinsic object classification. A type assertion redundant with the object's structural sort/ontology ancestry is rejected.

### Namespaceless learned vocabulary

The SLM emits globally unique canonical symbols such as `Agent`, `File`, `ToolInvocation`, `messageReceiver`, or `agentParticipant`. Package-qualified IDs remain internal registry identities.

The registry hard-fails on any new bare-name collision until an explicit canonical rename is assigned. Automatic namespace fallback or automatic name mangling is forbidden.

## 3. Source windows

### Prose

A prose window is one model-visible discourse block from one source/run/message/speaker/channel. It does not cross speaker, message, run, channel, or structured-record boundaries. Oversized prose is split UTF-8-safely at discourse-preferred boundaries.

Source-established speaker/addressee identities are compiler-owned context terms. The SLM may refer to `@speaker`/`@addressee`; it never predicts those identities back to the harness.

### Structured tool traffic

Model-visible tool calls, source-visible invocation updates, and tool results are learned windows by default.

A normalized structured window is governed by an exact versioned `NormalizationContract` that pins:

- source adapter and adapter version;
- positive model-visibility assertion;
- alias rules;
- opaque binary fields and their closed encoding types;
- semantic context dependencies used when a large record is partitioned.

The contract itself is part of the training contract and its digest is re-derived. A repeated digest without the rules is insufficient.

Structured JSON is parsed exactly. Duplicate object keys are rejected. Exact number lexemes survive parsing; learned numeric values are canonical mathematical values and are checked for equality with the original source lexeme.

JSON encoded inside an ordinary string remains a string unless a pinned reversible normalization rule says otherwise.

Large strings/objects are split into bounded parts without losing exact RFC-6901 source coordinates. Contract-declared context is repeated read-only so fragments remain interpretable. String byte ranges are absolute within the original decoded field and are translated correctly for fragments.

Only schema-proven opaque binary data may be externalized. For Base64, the source value must actually decode as Base64. Key-name/size heuristics may not hide natural language, code, JSON, reports, or arbitrary strings.

Unknown source visibility is `BOUNCE: SOURCE`, never guessed.

## 4. Structured record semantics

`ToolCall`, `ToolCallUpdate`, and `ToolResult` are distinct source record families.

A tool call establishes an invocation record and its supplied/requested content. It does **not** establish that a requested external effect occurred.

A lifecycle update is information about an existing invocation, not another invocation.

A tool result is an information artifact linked to the exact invocation when source identity licenses the join. Execution outcome is separate from payload semantics. A failed/timed-out result can still contain valid observations emitted before failure.

Generic key names such as `status`, `content`, `message`, or `result` have no universal semantic meaning. Their interpretation comes from the pinned source/tool normalization contract and ontology.

Inter-agent messages/prompts remain scoped semantic content. Assertions/questions/directives inside a message do not leak into top-level truth merely because the message appears inside a tool argument/result.

Message creation/requested delivery and successful delivery are distinct semantics.

## 5. Semantic object model

### Singular objects

`LearnedReferent`, `LearnedOccurrence`, and `LearnedVariable` contain one ontology sort and exact source grounding. No multi-type set exists in the learned schema.

### One ontology relation channel

Ontology relations are represented only as `Relation` propositions. Relations are binary under the pinned ontology schema. There is no parallel participant-role/attribute relation channel.

For open predicates, semantic participant relations such as `agentParticipant`, `patientParticipant`, `themeParticipant`, etc. preserve argument structure. `surfaceSubjectParticipant` / `surfaceObjectParticipant` are lowest-precision fallbacks only when the source establishes grammatical argument structure but not a stronger semantic role.

### Proposition vs semantic content

`LearnedPropositionExpr` contains only truth-valued structures.

Questions, directives, commissives, and quotations are `LearnedContent` with an ontology content sort. Questions therefore cannot accidentally be asserted as propositions. Presenter/addressee relations for source-established discourse context are compiler-owned.

## 6. Finite logical grammar

The finite Rust grammar is reserved for genuinely logical/compositional shapes whose semantics are not simply ontology classification:

- equality;
- ordered/dimensional comparison;
- negation;
- conjunction/disjunction;
- implication;
- counterfactual;
- `unless`;
- existential/universal binding;
- exact/inequality/approximate cardinality;
- vague plural-scale cardinality;
- presupposed-vs-asserted content.

Lexical semantic force is ontology-backed rather than duplicated in Rust enums.

`ScopedOperator` covers source-grounded propositional operators whose ontology sort determines the force, including:

- epistemic possibility/probability/necessity/expectation/qualification/certainty;
- deontic permission/obligation/prohibition/advisability;
- dynamic capability;
- generic/habitual force;
- perfect/progressive/already/still/again aspectual force;
- optimization such as maximization/minimization.

Participant-relative operators may use ontology relations such as `operatorBearer`. Optimization uses `optimizationTarget` when the source expresses what is maximized/minimized.

Generalized quantifiers (`MostQuantifier`, `ManyQuantifier`, `FewQuantifier`, `SeveralQuantifier`) and focus operators (`ExclusiveFocusOperator`, `AdditiveFocusOperator`, `ScalarFocusOperator`) are ontology-typed lexical operators, not approximate cardinality aliases.

## 7. Numeric/value semantics

The learned value grammar distinguishes:

- exact integer/decimal/boolean/string/null;
- ratio;
- percentage;
- numeric approximation;
- numeric interval;
- vague plural scale;
- source-explicit arithmetic `+`, `-`, `*`;
- measurement with unit.

Numeric `/` is canonically a ratio, not a competing arithmetic division representation.

`~93.2%` has one canonical shape: approximation outside a percentage value. `20-60 s` is an interval-valued measurement. `1000s`/`thousands` is plural-scale cardinality, not `Approximately(1000)`. `5 or so` is approximate cardinality whose exact count and approximation cue are separately grounded.

The SLM does not execute arithmetic merely to replace a source expression with its result.

## 8. Focus is not cardinality

Focus-sensitive `only`, `also/too`, and `even` are not flattened into cardinality.

For example, `only 500` may contain an exact `500` cardinality proposition plus an `ExclusiveFocusOperator` scoped over the relevant content. The exclusivity is independent semantic information.

A focus operator points to the exact focal source constituent; it does not duplicate the denotation merely to identify what is focused.

## 9. Ambiguity

A genuine unresolved ambiguity is represented by complete competing readings under an `Ambiguity` root. None of the alternatives is independently asserted.

All alternatives must be individually well-typed and must cover the same source semantic footprint. An ambiguity annotation cannot be used to assert one branch while merely mentioning another.

Do not invent an `exhaustive` claim. The source/labeler may know only the plausible readings presently identified.

## 10. Grounding and coverage

Grounding is semantic evidence, not truth-conditional meaning. Learned anchors contain exact coordinates directly; rich `SourceSpanId`s are deterministically interned by the compiler and are never SLM outputs.

Every learned source span must be reachable from a semantic root. Every learned semantic object must be reachable from a root. Decorative/unrooted junk is rejected.

Grounded literals use exactly one exact source span and must agree with the source value.

Every model-visible alphanumeric source token in prose and every model-visible lexical byte in structured string fragments must be claimed by rooted semantics. Structured scalar/container fields must be claimed at their exact source path. Opaque fields are exempt only under their validated normalization rule.

Operator cues may ground actual operator material (`not`, `if`, `or so`, etc.) but may not overlap scoped semantic material to fake source coverage. Operators with their own grounded lexical referent/focus/scale do not get a duplicate generic operator anchor.

## 11. Formal lowering

Semantic-v6 roots lower to actual formal objects/operators/relations. Truth-conditionally relevant meaning may not survive merely as serialized JSON metadata.

Occurrences are not independently graph-rooted because they exist. Hypothetical/requested/embedded events remain scoped by the propositions/content that mention them.

Quotation preserves typed quoted content/source anchoring without asserting the quotation. Questions remain typed semantic-content objects, not fake propositions.

Ontology relation domain/range checks use the pinned package declarations. Multiple declared domain/range entries are alternative licensed classes unless the ontology schema explicitly says otherwise.

## 12. Canonicality rule

One independent semantic fact has one canonical learned representation.

The SLM must not emit facts deterministically recoverable from:

- ontology ancestry;
- another learned field;
- source-window metadata;
- normalization metadata;
- logical closure;
- canonicalization.

If two schema forms express the same semantic fact, the schema is defective and must be fixed before mass labeling.

## 13. Bounce policy

A labeller may:

- ACCEPT;
- ACCEPT with explicit ambiguity;
- BOUNCE: SOURCE;
- BOUNCE: WINDOW;
- BOUNCE: ONTOLOGY;
- BOUNCE: FORMALISM.

Ambiguity alone is not a bounce.

Repeated ontology/formalism/window bounces are defects in the system, not excuses to improvise labels. That failure class blocks mass labeling until fixed.

## 14. Active adversarial gate

`audit/semantic-v6/` pins real transcript cases designed to force false precision or scope leakage: modality, plural scale, optimization, genericity, capability, approximate count, focus, measurement/range, questions/options, lifecycle updates, requested tool effects, delegation prompts, task status, partial-failure results, message aliases, provider lifecycle status, and encoded structured results.

The old prose-v4 and semantic-v5 audits are retained only as historical regression evidence.
