# Production-readiness foundation plan

Source: [`THINGSWENEEDTOFINISHUP.md`](THINGSWENEEDTOFINISHUP.md)

## Goal

Finish the ideal horizontal contracts that wide features (memory, TTSR, todos,
identities, MCP, computer use, multi-agent workflows, and GPUI) will depend on.
This plan does not implement those product features. It makes each one possible
without adding feature-specific knowledge to the kernel.

This is an ideal-architecture plan, not an incremental delivery plan. Phase
boundaries express dependency order and review boundaries only. Intermediate
compilation, temporary usability, compatibility scaffolding, and getting a
partial path "working" are explicitly irrelevant. Prefer the final coherent
abstraction even when several crates, the WIT ABI, fixtures, and documentation
must change atomically.

## Constraints

- [x] Keep Rig as the model-execution integration; do not create a second conversation log.
- [x] Keep the canonical transcript append-only and owned by `artist-kernel`/`artist-store`.
- [x] Keep scheduling, provider protocol mechanics, and plugin execution outside `artist-core`.
- [ ] Preserve deterministic plugin ordering by `(priority, plugin id)`.
- [x] Keep model-facing tools in plugins; the host owns only generic runtime/fabric mechanics.
- [x] Do not add permissions, sandboxing, isolation, frontend work, or feature-specific kernel events.
- [x] Treat this as a pre-production contract reset: bump the WIT ABI, record version, and file format directly where required; do not add compatibility adapters or migrations.
- [x] Keep ephemeral `provider-state` explicitly ephemeral; do not turn it into the product database.
- [x] Do not introduce transitional APIs, duplicate old/new execution paths, compatibility shims, or milestone-specific architecture merely to keep intermediate commits compiling.
- [ ] Judge each phase by the final cross-crate contract and conformance suite, not by standalone demos or temporary end-to-end usability.

## Completion gate

The foundation is ready for wide features when all of the following are true:

- [x] Every advertised lifecycle capability changes or observes a normal `SessionHandle` execution path in production code, not only in plugin-host examples.
- [x] A plugin can persist domain state through a durable resource without using `provider-state`.
- [x] A plugin can append a generic, durable, session-relevant event without adding a new kernel enum variant.
- [ ] Binary content can be stored once, referenced by digest, returned through resources/tools, and preserved in session history.
- [ ] Tool metadata, failures, progress, and next-action semantics survive WIT -> registry -> Rig -> stream-event round trips.
- [ ] A model provider can be installed and selected through the plugin/provider contract rather than by manually calling `ProfileModelRouter::register` in the application.
- [ ] Accounts, credentials, provider-private conversation state, and provider capabilities have durable, scoped homes outside the kernel transcript.
- [ ] An extension can create, drive, observe, cancel, and await a related Artist session through a host service.
- [x] Restart, cancellation, concurrent-call, malformed-plugin, and stale-format tests pass under `make test`.

---

## Phase 0 — Freeze cross-cutting contracts

These decisions precede implementation so later phases do not create mutually
incompatible envelopes and stores.

- [x] Resolve the questions in **Open decisions** below and record the answers here.
- [x] Draw the dependency boundary before adding types:
  - `artist-core`: durable, provider-neutral values only.
  - `artist-store`: canonical session records plus narrow persistence traits.
  - `artist-kernel`: single-session state machine, canonical ordering, and lineage validation.
  - `artist-resource`: resource/blob abstractions and tool contracts.
  - `artist-plugin`: WIT host, package activation, and adapters to kernel/resource traits.
  - `artist-rig`: Rig request/stream conversion only.
  - new `artist-runtime` if needed: session registry/scheduler, account/provider assembly, and child-session host service.
- [x] Specify one invocation scope used across hooks, tools, resources, plugin events, and child sessions: `session_id`, optional `run_id`, optional `call_id`, correlation ID, and parent correlation ID.
- [x] Define hard limits for plugin event payloads, metadata, hook rewrites, tool output, progress events, and inline binary data.
- [x] Enforce the resolved lifecycle failure policy: critical-path composition fails the pending operation atomically; post-commit observers consume durable events independently and can never rewrite the originating outcome.
- [ ] Document which calls may be retried. Never retry after visible tool activity or an externally committed provider operation.
- [x] Decide the ABI/record/file version bumps once, then update WIT bindings, SDK macros, fixtures, package validation, and stale-version tests together.

**Exit:** the approved contracts identify an owner, durability behavior, ordering,
limits, and failure semantics for every new value.

---

## Phase 1 — Put lifecycle plugins in the real execution path (first blocker)

### 1.1 Kernel-owned execution extension boundary

- [x] Add a small async, object-safe execution-extension trait in `artist-kernel`; `artist-kernel` must not depend on `artist-plugin`.
- [x] Make the no-plugin implementation an identity/no-op implementation.
- [x] Implement the trait for `PluginHost` in `artist-plugin` and remove unnecessary `&mut self` APIs so normal session execution can share it safely.
- [x] Pass the extension boundary through every `SessionHandle::create*`, `resume*`, and spawn path without multiplying constructor variants; use an options/dependencies struct.
- [x] Preserve plugin package snapshots for an in-flight invocation so activation does not change the middle of a composed pipeline.

### 1.2 Prompt composition

- [ ] Run `compose-prompt` before `SessionStore::create` and before the first profile/input is recorded.
- [ ] Freeze the resulting fragments in `InitialContext`; never recompute them on resume.
- [ ] Validate fragment role/source/content and reject oversized or malformed output before creating the session.
- [ ] Test zero, one, and multiple prompt plugins, deterministic ordering, failure atomicity, and resume behavior.

### 1.3 Context transformation

- [ ] Replace the current lossy WIT `message { role, content }` with a typed representation that preserves tool calls/results, images/attachments, reasoning/opaque parts, sequence, and notification semantics.
- [x] Invoke context transforms after canonical transcript projection and compaction selection, immediately before each provider request.
- [x] Keep transformed context request-local; it must not rewrite the canonical transcript or invalidate prior cache prefixes.
- [ ] Validate transformed tool-call pairing and content references before sending to Rig.
- [ ] Test memory injection, deletion/reordering rules, rich content, invalid transforms, steering delivery, compaction, and handoff epochs.

### 1.4 Hooks

- [x] Replace arbitrary hook strings with a versioned hook phase enum and typed payloads for at least: before model request, after model response, before tool execution, after tool result, run completed, run interrupted, and run failed.
- [ ] Define exactly what `rewrite` can rewrite at each phase; reject rewrites that mutate durable history or committed tool activity.
- [x] Apply hooks at the `SessionHandle`/`StreamingModel` boundary and around tool execution, not only inside `PluginHost` examples.
- [x] Preserve first-stop behavior and deterministic rewrite order.
- [x] Map a stop to a typed run outcome/failure rather than silently dropping the request.
- [ ] Test ordering, stop short-circuiting, legal/illegal rewrites, cancellation, and hook failure policy.

### 1.5 Model configuration and event observation

- [x] Apply model configuration after profile route selection and before constructing each provider request, including fallback attempts.
- [x] Validate transformed provider/model/parameters against the selected provider descriptor; do not permit an unrecorded route switch.
- [x] Deliver lifecycle observations from the canonical kernel event emission point so direct `RigModel` use and resumed sessions behave identically.
- [x] Give each post-commit observer a durable cursor, ordered retry, and explicit dead-letter diagnostics; observer failure must not alter the already-canonical session event or run outcome.
- [ ] Prevent event observers from recursively observing their own delivery diagnostics.
- [ ] Test event order against subscriber order, fallback attempts, partial output/reset, terminal controls, observer restart/replay, and dead-letter behavior.

### 1.6 Integration proof

- [ ] Add one fixture per lifecycle capability that makes an externally visible assertion.
- [ ] Add one normal `SessionHandle -> ProfileModelRouter -> RigModel` integration test using all lifecycle fixtures together.
- [ ] Remove or rename any advertised capability still acting as a placeholder.
- [ ] Update `README.md` and `PLUGINS.md` to describe actual call timing and failure semantics.

**Exit:** prompt, context, hooks, model configuration, and event observation are
exercised by normal sessions and covered by deterministic integration tests.

---

## Phase 2 — Generic durable plugin lifecycle facts

### 2.1 Canonical envelope

- [ ] Add a provider-neutral `PluginEvent` value to `artist-core` with:
  - emitting `plugin_id`;
  - namespaced event type and mandatory registered event schema/version;
  - invocation scope/correlation;
  - structured JSON payload;
  - optional opaque presentation metadata;
  - durability/session relevance declared by the emitter and accepted by the host.
- [x] Add one generic `TranscriptEntryKind::PluginEvent` and one generic `StreamEventKind::PluginEvent`; never add `MemoryWritten`, `TodoUpdated`, etc.
- [x] Require schemas for every plugin-emitted event, including runtime diagnostics; reject unregistered event types and schema-invalid payloads before publication or durable append.
- [x] Snapshot each event schema and its digest in a durable event-type catalog so replay never depends on the currently installed plugin revision.
- [x] Specify ID generation, ordering relative to tool results/run completion, maximum size/depth, and JSON validation.
- [x] Extend the record reducer so malformed IDs, foreign session/run/call scopes, duplicates, and events after invalid terminal boundaries are rejected.

### 2.2 Emission and observation

- [x] Add a `host-events.emit` WIT import for plugins; derive plugin identity from the activated instance rather than trusting a guest-supplied ID.
- [x] Route session-relevant emissions through the owning session actor so transcript append and stream publication retain one total order.
- [x] Return success only after a durable append; expose backpressure/failure to the emitter.
- [x] Keep runtime-only diagnostics out of the canonical transcript while giving them a separate observation path.
- [x] Deliver accepted plugin events to subscribers and event-observer plugins without recursive re-emission loops.
- [x] Define how presentation metadata is namespaced and ignored safely by clients that do not understand it.

### 2.3 Tests

- [ ] Round-trip plugin events through JSON, the record reducer, `MemoryStore`, and `FileStore`.
- [ ] Test concurrent emitters, append conflicts, restart/replay, cancellation races, payload limits, invalid scope, and observer recursion.
- [ ] Demonstrate two unknown domain event types projected by a generic client without kernel changes.

**Exit:** a future plugin can durably report any domain fact, and GPUI can replay
it from the canonical session record without kernel feature knowledge.

---

## Phase 3 — Durable resources, blobs, attachments, and rich content

### 3.1 Keep ephemeral and durable state separate

- [ ] Rename/document `provider-state` as process/provider-lifetime scratch state; clear behavior on restart must be explicit.
- [ ] Introduce a narrow durable resource-store trait with atomic create/read/update/delete/list and revision checks; its semantic contract must not assume a local filesystem or a distributed deployment.
- [ ] Scope durable keys by owner plus explicit domain scope (global, account, workspace/project, identity, or session); never infer scope from arbitrary string prefixes.
- [ ] Make resource projection a session/runtime view, not one process-global mount table. Compose each view explicitly from global, account, identity, workspace/project, profile, and session layers.
- [ ] Permit two simultaneous sessions on one machine to see different `ResourceFabric`/FUSE projections, including different project-local skills and plugin resources, while sharing content-addressed blobs where their explicit scopes overlap.
- [ ] Give every native mount instance an explicit projection/view identity; provider routing and FFF indexing must use that view rather than ambient process state.
- [x] Provide a file-backed implementation with atomic writes/recovery and an in-memory test implementation; these are implementations of the backend-neutral contract, not architecture-defining shortcuts.
- [x] Expose durable state as ordinary resource providers/URIs so todos, memory, rules, identities, relationships, and MCP caches own their schemas and operations.
- [x] Do not expose a generic plugin key/value database import.
- [ ] Test restart durability, optimistic conflicts, concurrent writers, corrupt records, scope separation, and package replacement.

### 3.2 Content-addressed blob and ordered multimodal-content contract

- [x] Add a `BlobRef` contract containing digest algorithm/value, byte length, media type, and optional logical name; the digest is computed by the host.
- [ ] Add streaming/size-bounded blob put, get/range, stat, and delete APIs with in-memory and atomic file-backed stores.
- [ ] Define one generic ordered content sequence for inputs, steering, assistant messages, tool results, resources, and provider requests. Parts may be text, structured data, blob-backed media/attachments, reasoning, or namespaced opaque content without adding media-specific kernel variants for every future modality.
- [ ] Replace string-only `Command::Input`, `Command::Steer`, `TranscriptEntryKind::Input`, `TranscriptEntryKind::SteeringQueued`, and corresponding WIT messages with the ordered content sequence; preserve exact interleaving such as `[text, image, text, image]` in the canonical transcript.
- [x] Define attachment/rich-content values that reference blobs and carry semantic role, media type, accessibility/alternate text, optional logical name, and namespaced provider metadata.
- [x] Store blob references—not base64 or provider upload handles—at their exact canonical transcript positions. Provider adapters resolve them to provider-native image/file parts only while constructing a request.
- [x] Make provider modality support discoverable. A vision-capable route receives ordered image parts; an unsupported route returns a typed capability mismatch or invokes an explicitly configured content transform rather than silently dropping/reordering media.
- [x] Preserve provider cache prefixes by making canonical content immutable. Media compaction/transformation appends a typed projection artifact and never edits the original input or prior transcript entries.
- [ ] Treat cache impact as part of the compaction decision: compute the earliest provider-visible content position changed by a transform, preserve the longest unchanged prefix, and require policy to accept the resulting cache boundary explicitly. Never invalidate more of the prefix than the provider serialization actually changes.
- [ ] Generalize compaction policies over ordered content parts so future plugins can implement image recompression, resizing, OCR/text replacement, thumbnail substitution, snapcompact-style image compaction, or arbitrary modality transforms.
- [x] Require every derived part to record source blob digest(s), transformation identity/version, output digest, provider serialization identity where relevant, cache-boundary impact, and projection scope. Repeated projection must choose the same immutable artifact until a new explicit compaction epoch is appended.
- [ ] Scope provider cache handles to the exact effective projection digest. A transformed projection creates an explicit successor cache state and may reuse any provider-supported unchanged-prefix handle; it must never overwrite or ambiguously alias the original cache state.
- [x] Ensure compaction can replace content only in the model projection covered by its durable compaction record; clients and future policies can still retrieve original blobs and reconstruct canonical history.
- [x] Keep provider-private uploaded-file handles outside canonical content, keyed by provider/account plus source digest, so uploads can be reused without contaminating or changing the cache-stable transcript.
- [x] Adopt hybrid roots + leases + tracing GC: canonical transcripts, durable resources, provider-private state, plugin packages, and account records are roots; temporary/in-flight blobs use leases; unreachable blobs enter quarantine before deletion.
- [ ] Require plugin-defined durable stores to expose typed blob references through `list-blob-refs`; arbitrary JSON scanning is not a valid tracing contract.
- [x] Extend `ResourceReply`, WIT, SDK conversion, tool output, `ContentPart`, transcript validation, and Rig conversion to preserve ordered rich content without base64-in-JSON.
- [ ] Keep textual reads and the native FUSE/WinFsp projection deterministic; define how non-text nodes appear through a filesystem mount without pretending bytes are UTF-8.
- [ ] Add vision and generic binary fixtures; verify exact transcript position, text/media interleaving, digest deduplication, range reads, media metadata, transform provenance, longest-prefix cache preservation, cache-stable repeated projection, projection-digest cache identity, GC rooting, transcript replay, and provider round trips.

**Exit:** screenshots, uploaded files, generated media, and computer observations use
the same content contract and require no screenshot-specific resource variant.

---

## Phase 4 — Restore the complete tool contract

### 4.1 Definitions and validation

- [x] Extend the canonical and WIT tool definition with output schema, category, and explicit annotations for read-only, destructive, idempotent, and open-world behavior.
- [x] Reconcile annotations with existing `ToolEffect`; define one source of truth for profile policy and reject contradictory definitions at activation.
- [x] Validate input/output schemas and metadata during candidate activation, before registry replacement.
- [ ] Preserve all metadata through plugin SDK -> `PluginHost` -> `ToolRegistry` -> Rig dynamic tool conversion.

### 4.2 Results, failures, and progress

- [x] Replace string-only plugin tool failures with a structured failure carrying stable code, human message, retryability, optional details, and optional field violations.
- [x] Add structured next-action hints to successful and failed results without turning hints into kernel control.
- [x] Add correlated progress events with sequence, optional fraction, message, and structured detail; stream them through generic tool/plugin events without writing every token-like update to the canonical transcript unless explicitly durable.
- [x] Validate tool outputs against output schema before exposing them to the model or transcript.
- [x] Preserve typed `yield`/`handoff` as terminal kernel controls; next-action hints must not duplicate those semantics.

### 4.3 Compatibility tests

- [x] Update every existing tool plugin and SDK macro to populate the expanded contract.
- [ ] Test malformed schemas, contradictory annotations, structured errors, progress ordering, output validation, cancellation, and hot activation.
- [x] Verify read-only/destructive/idempotent/open-world metadata is available to future UI/approval layers without implementing those layers now.

**Exit:** no Gortnite-era tool semantic listed in the source draft is lost at a
WIT, registry, Rig, transcript, or stream boundary.

---

## Phase 5 — Make model providers real plugins and add account infrastructure

### 5.1 Complete Rig capability inventory and contract derivation

- [x] Inventory every Rig 0.42 provider client against protocols, streaming events, tools, images/files, audio where relevant, reasoning, usage, request IDs, retry signals, model listing, and every authentication form it supports.
- [ ] Treat complete support for every Rig provider and capability as the baseline, not as a prioritized subset or spike outcome.
- [x] Inventory what Rig does **not** provide: multi-account selection, credential lifecycle, ChatGPT subscription auth, Copilot OAuth, account defaults, durable provider chains, and plugin discovery.
- [ ] Derive the provider-neutral contract from the complete matrix, then validate it with API-key and OAuth/subscription implementations; prototypes do not narrow the final contract.
- [ ] Implement and register the complete Rig provider catalog, either through shared generic adapters or provider-specific adapters as demanded by capability differences; every supported Rig provider must pass the common conformance suite plus provider-specific tests.
- [ ] Expose Rig model listing/capability discovery through the provider registry wherever Rig supplies it, without hard-coding provider names in the kernel.
- [ ] Choose where Rig is reused as an implementation library versus where Artist needs a provider-neutral host transport. Do not fork Rig behavior into `artist-kernel`.

### 5.2 Provider plugin contract

- [x] Split the current configuration-transform capability from the capability that actually supplies a streaming model implementation; name both unambiguously.
- [ ] Permit both installable host-native Rig-backed drivers and language-neutral WASM provider drivers behind the same provider registry and conformance contract.
- [ ] Design the WASM socket for arbitrary third-party providers, including generic host-mediated HTTP/SSE/WebSocket transport, streaming/backpressure, cancellation, and credential references; do not constrain it to Rig's provider catalog.
- [ ] Define provider descriptors: provider ID, supported model patterns, capabilities, auth kinds, transport/API variants, and configurable parameters.
- [ ] Define start/stream/cancel semantics that preserve every `ModelEvent`, backpressure, terminal error metadata, and abort propagation.
- [ ] Make `ProfileModelRouter` resolve providers from the activated provider registry; remove application-level manual registration as the production path.
- [ ] Snapshot provider/plugin revision per attempt so hot activation cannot change an in-flight stream.
- [x] Keep ordered profile fallback and sticky epoch behavior, but key route resolution by provider, account, API variant, and model.
- [x] Add conformance tests reusable by every provider plugin.

### 5.3 Accounts and credentials

- [x] Add durable account descriptors with account ID, provider ID, credential reference/type, API variant, default model, default reasoning mode, and non-secret metadata.
- [ ] Support every authentication flow exposed by Rig providers, plus extensible credential kinds for arbitrary WASM providers; no closed credential enum may require a kernel change for a new provider.
- [x] Put secret material behind a credential-store trait; never place tokens in profiles, plugin events, errors, transcripts, or package state.
- [ ] Support credential operations needed by API keys, OAuth access/refresh tokens, device/browser flows, and subscription session credentials.
- [ ] Define refresh serialization, expiry, revocation, redaction, and account deletion behavior.
- [ ] Add host-mediated auth imports/services so provider plugins request credentials by reference and cannot accidentally persist them in `provider-state`.
- [ ] Make account/model/reasoning selection deterministic and snapshot the effective non-secret selection in completion metadata.

### 5.4 Provider-private durable state and optimization

- [x] Define an opaque, provider-owned state record scoped by provider revision + account + session/profile epoch.
- [x] Persist conversation/response chain IDs, capability probes with expiry, provider-private context, uploaded-file handles, and cache handles outside the model-neutral transcript.
- [ ] Represent context window, reasoning controls, fast mode, prefix-cache policy, and API selection as typed capabilities/configuration rather than kernel branches.
- [ ] Commit provider state only at defined stream checkpoints; prevent a cancelled/failed attempt from advancing a conversation chain incorrectly.
- [ ] Keep overload/backoff state account/provider scoped and bounded; integrate retries with the no-repeat-after-tool-activity rule.
- [ ] Test restart/resume, token refresh races, account switching, fallback, cancellation, uploaded-file reuse, cache hit metadata, overload retry, and stale provider revisions.

**Exit:** production applications select installed provider plugins and accounts;
they do not construct and register native `StreamingModel` objects manually.

---

## Phase 6 — Host-managed extension session service

### 6.1 Durable identity and lineage

- [x] Add provider-neutral session metadata containing session ID, created time, creator/plugin identity, optional creation lineage (parent session and parent run/call correlation), relationship kind, and initial profile.
- [x] Store metadata and canonical events atomically enough that a returned session ID always resolves after restart.
- [x] Keep lineage validation in the kernel/store boundary; reject missing parents, self-parenting, and duplicate IDs.
- [x] Define terminal result as a typed completed/yielded/interrupted/failed outcome.

### 6.2 Runtime scheduler

- [x] Add a host/runtime session registry that owns live `SessionHandle`s and guarantees at most one active actor per session ID.
- [x] Implement create, resume-on-demand, send input, steer, inspect snapshot, subscribe/replay events, abort/stop, and await terminal outcome.
- [x] Separate kernel identity/logging from runtime scheduling; do not spawn processes/tasks from `artist-core`.
- [x] Require session creation to state attached/detached behavior and durable recovery policy explicitly; do not hide child cancellation or restart behavior behind a convenience default.
- [x] Define detached/attached child behavior, plugin-resolved recovery actions, subscriber lag/replay, cancellation propagation, and runtime shutdown.
- [ ] Make create idempotent under a caller-supplied request ID so plugin retries cannot duplicate child sessions.

### 6.3 Plugin host API

- [x] Add a `host-sessions` WIT import using opaque durable session handles/IDs rather than guest task handles.
- [x] Derive creator identity and parent correlation from invocation context; do not trust guest-supplied ownership fields.
- [ ] Keep this as a general plugin host API with structural handle/lineage validation only. Do not add authorization, permissions, or a security boundary.
- [x] Do not register a model-facing subagent/modal tool as part of this plan; orchestration plugins decide when and how to expose or use session creation.
- [x] Stream child events with cursor-based replay and bounded polling/subscription semantics compatible with WASM components.
- [x] Ensure plugin cancellation drops waits/subscriptions without implicitly killing a detached child.

### 6.4 Tests

- [ ] Test nested creation, concurrent create idempotency, parent/child replay, child yield, explicit stop, attached and detached cancellation, host restart under every recovery policy, plugin-resolved recovery, subscriber lag, and malformed handles.
- [ ] Test that child plugin events and attachments remain replayable without loading the originating plugin.

**Exit:** a handoff/orchestration plugin can manage related sessions entirely
through the host boundary while the runtime, not the kernel, schedules them.

---

## Phase 7 — Production hardening and release gate

- [ ] Add shared conformance suites for lifecycle providers, model providers, resource providers, blob stores, durable resource stores, and session runtimes.
- [ ] Add deterministic fault injection at every durable append/rename/checkpoint and verify restart recovery.
- [ ] Add cancellation and bounded-backpressure tests for model streams, plugin events, progress, blobs, and child-session subscriptions.
- [ ] Add malformed/oversized WIT payload tests and prove limits are enforced before expensive allocation or durable append.
- [ ] Add package hot-activation tests proving in-flight calls retain old revisions while new calls use the new revision.
- [ ] Add redaction tests for credentials and provider-private data across errors, debug output, observations, transcripts, and plugin events.
- [ ] Add multi-process append/conflict tests for every file-backed durable store, matching `FileStore` guarantees.
- [ ] Run `cargo fmt --check`, workspace Clippy with warnings denied, all native tests, every WASM build/validation test, cross-platform compile checks, and `make test` in CI.
- [ ] Update `README.md`, `PLUGINS.md`, `PROFILES.md`, `TRANSCRIPT_V1.md`, and the resource-fabric contract with final ownership and replay semantics.
- [ ] Remove obsolete placeholders, duplicate adapters, stringly typed legacy paths, and manual production registration paths.
- [x] Review every new core enum/field: if it names a product feature or provider, move it out of the kernel contract.
- [ ] Mark this plan complete only after the completion gate at the top is demonstrably green.

---

## Decisions and rationale

### Resolved from owner input

- [x] **Model provider forms:** both host-native Rig-backed provider plugins and WASM provider plugins are permitted behind one provider-neutral registry and conformance contract.
- [x] **Provider breadth:** support every provider, authentication form, and relevant capability exposed by Rig. Also provide a WASM extensibility socket capable of implementing arbitrary providers outside Rig.
- [x] **Resource views:** persistence location and resource visibility are separate concerns. Storage contracts remain backend-neutral, while each session gets an explicitly composed resource/FUSE view. Sessions on the same machine may have different project-local skills, resources, profiles, and indexes.
- [x] **Pagination:** no generic tool pagination contract will be designed or implemented in this plan. Existing operation-specific cursors remain operation-specific.
- [x] **Architectural standard:** there is no staged-delivery framing. Intermediate compilation and temporary usability are irrelevant; only the coherent final architecture matters.

### Lifecycle failure semantics

- [x] Critical-path composition fails the pending operation atomically. Post-commit observers consume the durable event log independently with durable cursors, retry, and explicit dead-letter diagnostics. An observer failure never rewrites the originating session outcome.

### Credential storage boundary

- [x] Credential storage is an extensible host service with OS-keychain, external-vault, and caller-supplied implementations conforming to one contract. Accounts store only opaque credential references; neither the kernel nor provider plugins own secret persistence.

### Blob ownership and retention scorecard

All options assume immutable, digest-addressed blobs. Scores are 1 (poor) to 5
(strong). `Safety` means avoiding deletion of live data; `Reclaim` means removing
unreachable data; `Concurrency` means behavior under crashes and concurrent
writers; `Simplicity` includes implementation and reasoning cost.

| Strategy | Safety | Reclaim | Concurrency | Simplicity | Runtime cost | Principal trade-off |
|---|---:|---:|---:|---:|---|---|
| Retain forever | 5 | 1 | 5 | 5 | Lowest CPU; unbounded disk | Perfectly safe and trivial, but storage grows without bound and account/session deletion cannot actually delete content. |
| Eager reference counts | 2 | 5 | 2 | 2 | Cheap steady-state | Every reference mutation must atomically update another store; crashes, copied manifests, cycles, and references embedded in plugin JSON can cause leaks or premature deletion. |
| Leases/expiry only | 2 | 4 | 4 | 3 | Periodic renewal | Excellent for temporary uploads, but durable history can expire incorrectly and offline sessions need special treatment. |
| Tracing mark-and-sweep | 4 | 5 | 4 | 3 | Periodic scans | Correct if every durable reference source is enumerable; needs a grace period and costs a scan, but avoids distributed reference-count transactions. |
| Hybrid roots + leases + tracing GC | 5 | 5 | 4 | 2 | Scans plus lease bookkeeping | Durable references are traced roots, temporary/in-flight blobs use leases, and deletion occurs only after a grace/quarantine window. Most complete semantics, highest design cost. |

The hybrid model fits the ideal architecture: transcripts, durable resources,
provider-private state, plugin packages, and account records expose blob
references to a root enumerator; temporary uploads receive leases; tracing GC
marks roots and live leases; unreachable blobs enter quarantine before deletion.
It avoids reference counts and makes session/account deletion meaningful.

- [x] Use hybrid roots + leases + tracing GC, specified as part of the blob-store contract rather than appended later as cleanup machinery.
- [x] Plugin-defined durable resources expose references through a typed `list-blob-refs` contract; arbitrary JSON cannot be traced reliably.
- [x] Blob-backed media is an ordered transcript content primitive, not merely resource storage. Images and future modalities retain their exact message position and can be transformed by cache-stable, provenance-carrying projection/compaction policies without mutating canonical history.

### Consumers of plugin event schemas

A schema registry would not teach the kernel that an event is a todo, memory, or
computer observation. The event remains the generic `(plugin id, event type,
version, payload)` envelope. Consumers are:

1. **The emitting host boundary:** validate before durable append, catching a
   broken/upgraded plugin before it writes facts no consumer can decode.
2. **GPUI and other clients:** discover field structure, choose a renderer by
   namespaced type/version, and provide a generic JSON fallback.
3. **Observer plugins and projections:** validate the version they consume and
   build durable derived views without importing the emitter's code.
4. **Export/debug tooling:** explain events and redact fields marked sensitive or
   presentation-only without kernel-specific variants.
5. **Replay across plugin replacement/removal:** retain the schema identity (and
   ideally its digest/snapshot) that governed an old event even when the emitting
   package is no longer active.

Costs are activation-time schema validation, explicit schema evolution, and a
schema catalog that must outlive plugin activation. JSON Schema validates shape,
not domain semantics, so it does not replace plugin logic.

- [x] Every plugin event type, including runtime diagnostics, requires a registered, versioned schema. Snapshot schema digest/content in the durable event-type catalog, validate on emission, and always preserve unknown historical events with a generic client fallback.
- [x] Presentation metadata belongs primarily in event-type registration, with an optional small schema-validated per-event override.

### Session lineage and relationships

If session A asks a plugin during run/call X to create session B, **lineage** is
the immutable fact that B was created from A at X. It supports replay, a GPUI
session tree, debugging, and correlation. A **relationship** is broader mutable
domain data such as "planner for", "peer of", or "reports to"; that can live as
generic plugin-owned durable resources/events rather than kernel enum variants.

- [x] The kernel stores immutable creation lineage only. Arbitrary mutable relationship graphs belong to plugin-owned durable resources/events.
- [x] Authorization is outside this plan and outside Artist's claimed boundary, consistent with `arch.md`; the general session API performs structural validation but does not implement permissions or security.

### Child attachment and crash recovery

These terms concern what happens after a plugin creates another session:

- **Attached child:** aborting/stopping the parent also requests cancellation of
  the child.
- **Detached child:** the child remains independently addressable and continues
  when the parent finishes or its caller stops waiting.
- **Host restart:** an in-flight network stream cannot literally survive process
  death. Recovery can durably mark that run interrupted, then either leave the
  session idle, automatically schedule another input/attempt, or wait for an
  explicit caller action.

The general API avoids hidden defaults: creation explicitly chooses attachment
behavior and a durable recovery policy. Dropping an `await` or poll handle never
implies cancellation. The canonical transcript records every cancellation and
recovery transition. Orchestration plugins resolve these choices; this plan does
not expose a modal subagent tool to the model.

- [x] Child creation requires an explicit attached/detached choice, with no implicit default.
- [x] Recovery policy is explicit and durable, including `remain-interrupted`, `resume-queued-work`, and plugin-resolved recovery actions. Recovery never pretends to resume a lost provider byte stream or silently retries committed tool activity.
- [x] Session creation/control remains a general plugin host API; whether and how a model can invoke it belongs to orchestration plugins.

---

## Deferred architecture notes (outside this plan)

- **Generic tool pagination:** intentionally unresolved. Do not standardize a universal page/result/cursor envelope here. Preserve existing operation-specific cursor contracts such as resource polling, and revisit pagination only with concrete consumers and semantics for cursor ownership, consistency, expiry, cancellation, and replay.
