> **2026-08-08 semantic-v6 pre-label status.** The active contract is `muse-semantic-label-6` / `muse-corpus-5`: a separate minimal ontology-first learned target with singular namespaceless sorts, exact source grounding, learned model-visible tool traffic, typed semantic content distinct from propositions, requested-vs-observed effect scope, pinned exact normalization, ontology-backed scoped operators, and the real-transcript `audit/semantic-v6/` adversarial gate. All available toolchain-independent gates pass. The complete pinned Rust/release suite remains mandatory and unclaimed in this environment.

# Muse Completion and Web-Session Handoff Specification

## Goal

Complete Muse as the standalone semantic/formal cognition system used by Artist, with one shared formal occurrence language for deterministic structured events and SLM-produced prose formalizations.

This scope does **not** require an exhaustive ontology of every possible domain or an exhaustive natural-language lexicon. Muse must provide a broad safe ontology foundation and deep, precise ontology where Artist/coding-harness activity requires it.

## Fixed architectural constraints

1. **The SLM is only a prose-to-formalism transducer.** It does not prove theorems, perform proof search, do Bayesian inference, choose priors, compute likelihoods/posteriors, synthesize programs, or discover consequences.
2. **Structured tool calls are formalized deterministically.** The deterministic tool formalizer and the prose SLM must emit the same occurrence vocabulary and proposition structures.
3. **Tool-call formalization is specified before prose labeling.** The structured event source is the highest-information source and should establish ergonomic canonical representations that prose labels reuse.
4. **Windows contain contiguous non-tool-call prose only and never span run boundaries.**
5. **Labels represent what occurs/is expressed in the source, not logical closure or inferred world knowledge.**
6. **Nested propositions are structured formal objects, never opaque strings.** Belief, desire, intention, command, question, quotation, negation, modality, and temporal structure must preserve embedded formal content.
7. **Ontology contributes vocabulary, types, constraints, canonical identities, and validation.** It does not independently enrich labels with unstated facts.
8. **Use the narrowest ontology type justified by the source.** Otherwise fall upward to the narrowest guaranteed type. Preserve ambiguity/underspecification explicitly rather than guessing.
9. **Ontology funnel:** broad reusable foundations at the top; increasingly precise computing/agent/harness/Artist packages below.
10. **The fixed DTT kernel remains the trusted checker.** Ordinary higher-order/classical object logic is represented by theory packages/terms checked by the DTT kernel. Do not make the kernel depend on domain ontologies.
11. **Bayesian, proof-search, computation, and discovery machinery are downstream consumers/producers.** They are not SLM label content except when the source prose itself discusses such things.
12. **Mnestic persistence does not determine semantics.** Stored artifacts retain their explicit ontology/package/provenance identities; reads must not silently reinterpret old data under new ontology releases.
13. **Lexical infrastructure is supporting metadata, not the open-ended prose interpreter.** Keep canonical labels, aliases, technical terms, usages, attestations, and optional lexical mappings; do not make a finite lexicon the critical path for unrestricted prose formalization.

---

# Ordered remaining work

## 1. Preserve and improve the unified source baseline

The newest Muse semantic foundation and the verified cognitive superstrate have now been assembled into one top-level Muse Cargo workspace. Treat this unified tree as the sole non-Artist baseline.

The baseline is not design-frozen. Redesign any component when there is a concrete semantic, formal, correctness, interoperability, or training-quality reason to do so. Do not preserve a weaker design merely to avoid churn. The fixed DTT kernel remains the trusted checker unless an independently justified kernel change is explicitly required; integration convenience alone is not such a reason.

Run toolchain-independent checks continuously while implementing. The complete Rust/release verification suite is the final pre-training gate in section 14, immediately before the training contract is frozen and GPU budget is spent.

Preserve exact package identities, canonical hashing, provenance, deterministic serialization, and theory/certificate behavior when their semantics remain unchanged. When semantics must change, version them explicitly rather than silently reinterpreting old artifacts.

## 2. Research and source-pin the missing ontology funnel

Before authoring concepts, audit authoritative or well-established sources that can be reused or aligned. Source-pin every adopted definition or mapping.

At minimum investigate:

- UFO-derived software-engineering ontologies, especially SEON and its relevant component ontologies.
- W3C PROV/PROV-O for provenance alignment where Muse's existing provenance model benefits from explicit crosswalks.
- CodeMeta and SPDX for software artifact/build/package metadata and interoperability.
- Established agent/action/message models such as FIPA where still semantically useful.
- Current agent/tool telemetry/specification conventions only as interoperability evidence, not as ontology authority.
- Tool/RPC invocation specifications where useful for generic invocation/result/error distinctions.

Do not import an ontology wholesale merely because it exists. Reuse concepts only when their identity conditions and constraints fit Muse's UFO-grounded model.

## 3. Build the missing ontology packages in funnel order

### 3.1 Software engineering ontology

Ground software-specific concepts in UFO and reuse/source-align existing software-engineering ontology work where appropriate.

Required coverage includes at least:

- software system
- software product
- program
- source code
- source-code artifact
- software artifact
- configuration
- dependency
- build artifact
- test artifact
- requirement/specification
- software-development activity
- coding activity
- build activity
- compilation activity
- testing activity
- configuration-management activity
- defect/diagnostic where ontologically justified

Maintain distinctions between information objects, physical/digital bearers, executable artifacts, processes/events, and states/situations.

### 3.2 Computing ontology

Add execution-level concepts needed for actual machine/tool logs, including at least:

- computer system
- operating system/environment
- process
- command as information artifact
- command invocation/execution as event
- executable/program invocation
- argument
- environment binding
- filesystem object
- file/directory/path distinctions
- file state/version
- read/write/create/delete/move/copy operations
- standard input/output/error or equivalent information artifacts
- exit status/result
- resource usage where available
- success/failure as outcome/status without conflating requested and observed effects

### 3.3 AI and software-agent ontology

Add framework-neutral agentic-computing concepts, including at least:

- artificial agent
- software agent
- model-backed agent
- language model
- model invocation
- agent run/session
- task/subtask
- message
- conversational turn
- context/context window
- tool
- tool specification
- tool invocation
- tool result
- delegation
- observation supplied to an agent
- generated response/content

Ground intentional/social concepts in UFO-C where applicable, but do not pretend software implementation structures are identical to human mental states without explicit modeling distinctions.

### 3.4 Agentic coding-harness ontology

Specialize the preceding layers for coding harnesses, including at least:

- coding harness
- coding agent
- run
- turn
- dialogue/event-log segment
- workspace
- repository
- source file
- tool call
- tool result
- shell invocation
- file read/write/edit
- patch application
- build execution
- compilation
- test execution
- diagnostic emission
- tool failure
- command failure
- retry/recovery
- generated artifact
- external tool/service invocation

This layer must be reusable by coding harnesses other than Artist.

### 3.5 Artist ontology

Inspect the actual current Artist source and event/tool schemas. Add only concepts that are genuinely Artist-specific, such as concrete protocol/event/tool kinds, Artist roles, Artist policy objects, or Artist-specific run semantics.

Do not place generic coding-harness concepts in the Artist package.

### Ontology completion criteria

For every new package:

- stable opaque IDs;
- explicit imports and exact version/content pins;
- human-readable canonical labels;
- provenance to source material or explicit Muse-authored rationale;
- taxonomic parents;
- domain/range or role constraints;
- identity/dependence constraints where applicable;
- disjointness/partitions where justified;
- conformance tests and competency cases;
- conservative fallback paths to broader ontology categories;
- package sealing and deterministic hashes;
- no identification of UFO-MLT type order with DTT universe levels.

## 4. Define the canonical shared occurrence model

Create one canonical occurrence/proposition IR used by both deterministic structured-event formalization and prose labels.

It must represent at minimum:

- referents/entities and identity/coreference;
- events, states, situations, and transitions;
- event participants/semantic roles;
- relations and qualities;
- propositions as structured objects;
- nested propositions;
- beliefs, desires, intentions, commands, questions, assertions, and other attitude/speech-act content;
- negation;
- conjunction/disjunction/implication as needed by expressed content;
- quantification;
- modality;
- tense/temporal anchoring and ordering;
- causal claims when expressed;
- quotation versus ordinary use;
- source provenance;
- source-span or structured-field alignment;
- explicit ambiguity/underspecification;
- cross-reference to ontology package snapshots.

The occurrence model must distinguish:

- a type from an instance;
- a command string/specification from an execution of the command;
- a requested action from an attempted/performed action;
- a tool returning success from an independently observed world effect;
- a proposition from its truth/proof status;
- an agent holding an attitude from the embedded proposition that is the attitude content;
- mention/quotation of a proposition from asserting that proposition.

Canonicalization rules must make logically/structurally equivalent representations converge where Muse requires one training target. The SLM should not be trained against arbitrary alpha/syntactic variants of the same Muse representation.

## 5. Specify tool-call formalization completely before prose labels

Define a protocol-neutral deterministic tool-event model first, then map the actual Artist tool/event schema into it. Artist is one adapter, not the ontology boundary. Use the current `Gortnite` branch of the authoritative Artist GitHub repository for Artist-specific schemas.

The specification must cover:

- invocation identity;
- run/turn identity;
- invoking agent;
- tool identity and tool specification;
- arguments and their typed values/references;
- temporal ordering;
- result/error;
- exit/status codes when applicable;
- stdout/stderr/diagnostics where applicable;
- artifacts read/used/generated/modified;
- requested effects;
- observed effects;
- success/failure/partial outcome;
- retry/continuation relationships;
- source structured-record provenance.

For each tool family, define exact canonical formalization rules. At minimum cover all tool families that actually occur in Artist, including shell/process execution, file operations, patch/edit operations, compilation/build/test operations, and any network/service/model calls exposed by the harness.

No SLM is used for fields that are deterministically available from structured records.

## 6. Implement and exhaustively test the deterministic tool-call formalizer

Implement the structured-event conversion against real Artist fixtures.

Requirements:

- total over every structurally valid known Artist event kind;
- explicit unknown/unsupported event representation instead of dropping data;
- deterministic output and canonical serialization;
- no inferred effects beyond what the event/result actually establishes;
- source-field alignment retained for auditability;
- exact ontology snapshot identity embedded/pinned;
- golden fixtures for success, failure, partial completion, malformed inputs, retries, nested calls, and multi-artifact effects;
- round-trip/canonical-hash tests.

The ergonomics and canonical structures established here become the reference structures for prose labels.

## 7. Implement the Muse semantic-to-superstrate bridge

Replace the documented future integration boundary with a real adapter from Muse semantic/occurrence structures to the finalized superstrate formal submission.

The bridge must bind:

- Muse semantic object IDs -> formal object IDs;
- Muse concept IDs -> formal ontology type IDs;
- Muse relation IDs -> formal symbols;
- nested occurrence/proposition structures -> exact formal terms;
- ontology/package snapshot identity -> exact formal submission identity;
- quotation/mention -> the superstrate's quoted-object mechanisms where appropriate;
- ambiguity/underspecification -> explicit representable structures without inventing a resolution.

The lowering must be deterministic, recomputable, and kernel-checkable.

Preserve the existing trusted DTT kernel and theory-package model. HOL/classical logic remains an object-level theory/fragment checked by DTT rather than a second trusted kernel.

## 8. Define the prose-window label specification from the now-stable occurrence language

Only after the tool formalizer and shared occurrence model are stable, freeze the SLM annotation/label rules.

Input units:

- contiguous non-tool-call prose;
- windows never cross run boundaries;
- tool calls/results split prose continuity;
- retain source run/turn/message/span identities.

Output:

- every occurrence/claim/attitude/event/state/relation materially expressed in the window;
- entities and coreference supported by the window/context policy;
- nested propositions represented structurally;
- ontology-linked types at the narrowest justified level;
- explicit ambiguity or broad guaranteed upper-bound classification when exact resolution is unsupported;
- source-span alignments;
- no downstream proof, theorem, Bayesian state, search result, or inferred closure.

Cross-source convergence requirement:

When prose describes an occurrence that is also represented by a structured tool event, both representations must use the same ontology concepts, relations, roles, and canonical occurrence patterns to the extent supported by each source. Prose may be less informative, but it must not use a separate semantic dialect.

## 9. Recast lexical machinery as supporting lexicalization metadata

Audit `muse-lexicon` and `muse-resolution` so public documentation/API does not imply they are the unrestricted prose-understanding engine.

Retain useful functionality:

- canonical names;
- aliases;
- abbreviations;
- technical multiword terms;
- language/register/domain metadata;
- attestations/evidence;
- optional candidate mappings;
- deterministic controlled-vocabulary resolution.

The SLM remains responsible for open-ended prose interpretation into the formal occurrence language.

## 10. Implement the corpus-labeling and QA pipeline

Build the tooling that turns generic coding-agent prose transcripts into training examples, with Artist runs supported as one source rather than required as the primary corpus.

Required behavior:

- ingest coding-agent transcript/run logs without spanning run boundaries;
- deterministically recognize and exclude structured tool-call/result payloads from SLM prose examples while preserving their boundaries;
- deterministically formalize structured tool events when an adapter is available;
- segment contiguous non-tool prose;
- create configured-length prose windows without crossing tool-call boundaries;
- attach source/run/turn/span metadata;
- attach ontology/package snapshot identity;
- ingest/generate labels using the frozen occurrence-label schema;
- validate labels through ontology conformance and formal lowering/kernel well-formedness;
- reject or quarantine invalid examples rather than silently repair them;
- canonicalize serialization;
- content-hash examples and datasets;
- create train/dev/test splits at the run level to prevent leakage across windows from the same run;
- preserve annotation/model provenance and revision history;
- support regression fixtures and deterministic rebuilds.

## 11. Move Mnestic persistence into Muse (post-training blocker only for final Muse product)

Migrate the RocksDB-backed persistence currently living in Artist into Muse as an independent persistence subsystem/adaptor.

It must store/retrieve without changing meaning:

- raw source/run/tool/prose records as required;
- formal occurrence graphs;
- ontology/package snapshots and pins;
- source alignments/provenance;
- observations;
- proof/certificate artifacts;
- empirical/inference artifacts;
- dataset/label identities where useful.

Storage schemas must be versioned and migrations explicit. Reading an old artifact must retain the ontology/theory identity under which it was created.

Mnestic is not a prerequisite for defining labels, but it is part of the final Muse product scope.

## 12. Finalize the already-unified Muse product surface (post-training blocker only for final Muse product)

The semantic and cognitive halves now occupy one top-level Cargo workspace. Finish the product-level integration while preserving clean internal dependency boundaries.

Target conceptual structure:

```text
Muse
+-- semantic foundation
|   +-- ontology
|   +-- provenance
|   +-- lexicalization metadata
|   +-- registry/classification/validation
|   `-- occurrence representation
+-- formal cognition
|   +-- formal graph
|   +-- fixed DTT kernel
|   +-- theory packages / HOL-classical layer
|   +-- empirical contracts
|   `-- certification/query machinery
+-- integration
|   +-- semantic -> formal lowering
|   +-- deterministic tool-call formalizer
|   +-- optional Artist integration
|   `-- dataset/label tooling
`-- persistence
    `-- Mnestic/RocksDB adapter
```

Remaining requirements:

- semantic crates remain usable without Artist;
- kernel/formal crates remain usable on already interpreted formal input;
- Artist-specific code lives behind an optional package/crate boundary;
- persistence remains replaceable behind explicit interfaces;
- finish the Muse facade around the semantic-to-formal bridge and persistence once those exist;
- decide whether historical `artist-*` cognitive crate names remain compatibility names or migrate to `muse-*` names without losing artifact compatibility;
- define an exact release/version policy across the integrated workspace.

## 13. End-to-end conformance and adversarial testing

Add tests that exercise the complete path:

```text
structured Artist event
    -> deterministic occurrence graph
    -> ontology validation
    -> formal lowering
    -> DTT checking
    -> serialization/persistence round-trip
```

and:

```text
prose fixture + gold label
    -> label validation
    -> ontology validation
    -> formal lowering
    -> DTT checking
```

Required adversarial cases include:

- ambiguous pronouns/coreference;
- nested beliefs/desires/commands/questions;
- negation scope;
- modality;
- quotation versus assertion;
- failed/partial tool calls;
- success status with absent/unverified external effect;
- retries and duplicate events;
- events referenced later in prose;
- broad fallback classification;
- ontology-version mismatch;
- malformed formal applications;
- exact package/theory identity mismatch;
- run-boundary and tool-boundary windowing errors.

## 14. Run the complete release verification and freeze the training contract

Before declaring Muse complete for this scope:

- regenerate and verify manifests;
- verify all generated/source-pinned ontology packages;
- `cargo fmt --check`;
- locked dependency resolution/build;
- all unit/integration/doc tests;
- strict Clippy with warnings denied;
- rustdoc with warnings denied;
- debug and release builds;
- declared MSRV verification;
- static structural verification;
- deterministic/canonical serialization tests;
- content-hash reproducibility tests;
- full end-to-end tool/prose occurrence fixtures;
- Mnestic persistence round-trips/migrations;
- archive extraction verification;
- final content manifest and SHA-256.

Freeze/version the following together before spending GPU training budget:

- ontology package snapshot set;
- shared occurrence IR/schema;
- tool-call formalization rules;
- prose-label schema/annotation rules;
- semantic-to-formal lowering version;
- canonical serialization format.

Changes after training must create new explicit versions rather than silently changing the meaning of existing labels.

---

# Work that is not a blocker for SLM labeling/training

The following do not need new implementation before training the prose formalizer unless another product requirement independently demands them. Mnestic migration and final facade/product-surface cleanup are also explicitly post-training work; they remain required for final Muse completion, not for the SLM training freeze:

- proof-search algorithms;
- theorem discovery;
- program synthesis;
- Bayesian inference algorithms;
- heuristic prior generation;
- likelihood computation engines;
- posterior computation;
- exhaustive natural-language lexicons;
- exhaustive world/domain ontologies;
- every possible future UFO specialization.

Muse's existing superstrate contracts for these downstream activities should remain available, but the SLM does not produce or execute them.

---

# Exact web-session source handoff

Place **one Muse project root** in the web session's local environment:

```text
work/
`-- muse/
```

Use the unified Muse workspace archive produced with this specification. It already contains both previously separate non-Artist halves:

```text
muse/
+-- crates/muse-*            # semantic foundation
+-- crates/artist-formal     # cognitive formal graph
+-- crates/artist-kernel     # fixed DTT kernel
+-- crates/artist-empirical
+-- crates/artist-theories
+-- crates/artist-cognition
+-- crates/artist-cog
+-- packages/ufo/
+-- docs/cognitive-superstrate/
+-- docs/MUSE_COMPLETION_AND_WEB_HANDOFF.md
+-- scripts/
+-- Cargo.toml
+-- Cargo.lock
+`-- MANIFEST.sha256
```

The historical `artist-*` crate names inside Muse are retained for source/artifact continuity; they are not a separate workspace and do not mean the Artist harness source is bundled.

The unified package was assembled from these exact inputs:

- `muse-semantic-foundation-ufo-complete.zip` — SHA-256 `18b2af0e327b2faa2e800b5e89ae33f3949e6668b69f57ef167626ece6b1ac67`
- `artist-cognitive-superstrate-verified.zip` — SHA-256 `72eb93039a076090ff29b68dd1632d4352526cb61f281777cd02611cee2de2de`

Both extracted input manifests were hash-verified before assembly. Their original manifests are retained under `docs/provenance/`.

The Artist harness repository does **not** need to be copied into this package. When the work reaches Artist-specific ontology, tool-call schemas, or Mnestic migration, use the current Artist GitHub repository through the available GitHub connection so the implementation is derived from the live authoritative code rather than a stale copied tree.

Do not separately copy old Muse milestone trees, old superstrate archives, or the UFO master graph. The current unified Muse tree contains the relevant semantic packages, retained source material, cognitive source, docs, tests, and verification scripts.

---

# Status of older local Muse copies

The newly unified Muse package supersedes both prior non-Artist working roots:

- the semantic-only `~/Downloads/muse` installed from `muse-semantic-foundation-ufo-complete.zip`;
- the separate verified `artist-cognitive-superstrate` tree.

For the web session, use the unified archive from this handoff rather than trying to reconstruct Muse from those two older roots.

---

# Expected return from the web session

Return one source archive containing the completed Muse workspace plus its verification evidence. Do not return only patches or generated binaries.

At minimum the returned archive must contain:

- complete Cargo workspace source;
- all ontology package source/evidence files;
- generated/sealed ontology packages;
- shared occurrence schema;
- tool-call formalization specification and implementation;
- semantic-to-superstrate bridge;
- prose-label schema and dataset tooling;
- Mnestic persistence implementation/migrations;
- complete docs;
- complete tests/fixtures;
- lockfile/toolchain configuration;
- verification scripts;
- content manifest;
- final verification log;
- archive SHA-256.

The web session must state explicitly anything it did not compile, test, verify, source-audit, or migrate. No component should be called complete merely because interfaces or placeholder packages exist.
