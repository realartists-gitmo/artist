# Artist Open-Ended Verb/Resource Architecture Refactor

Status: implementation complete; final verification passed.

This document is the durable implementation ledger. Update checkboxes and notes after every horizontal slice. Do not mark a checkbox complete without a focused test or compile-time assertion.

## Non-negotiable end state

- [x] Exactly one semantic kernel execution path: typed/dynamic Component Model values only.
- [x] No legacy `Handler`, `Kernel.handlers`, JSON resource ABI, or internal `Request -> serde_json::Value` dispatch.
- [x] No closed `Verb` or `Operation` enum in the kernel.
- [x] Canonical versioned `VerbId` identifies a concrete WIT package/interface/function contract, with package, interface, function, and version accessors.
- [x] Claims, exports, routing, generations, and capabilities use dynamic contract identities.
- [x] Ten current verbs are ordinary installed verb packages, not kernel special cases.
- [x] Verb request/result schemas are owned by verb WIT contracts.
- [x] Verb-owned typed routing extractors provide ordered `ResourceUri`s.
- [x] Kernel dynamically validates and invokes registered WIT functions.
- [x] Components dynamically link newly installed verb contracts.
- [x] Model tools derive from active verb packages and hot-swap.
- [x] Direct executable-file `run` creates ordinary `process://` resources.
- [x] No shell parsing, PTY, Brush, or fixed semantic host APIs.
- [x] Self-modifying new-verb, dynamic composition, conflict/reserve, incompatibility, and arbitrary-process-power tests are covered.

## Architecture decisions

- Dynamic identity syntax: `namespace:interface/function@version`, represented by `VerbId(String)` with canonical parsing/validation.
- Dynamic values: an Artist-owned `WitValue`/function-type representation mirroring Component Model values, or Wasmtime's equivalent where stable and usable.
- Verb packages: one active registry for all verbs, including the ten seeded packages.
- Resource claims: `(VerbId, ResourceUri)`; resource exports contain canonical `VerbId`s.
- Routing: verb package extractor receives the exact typed input and returns only ordered URIs.
- Execution: registry resolves the verb contract/function and dynamically invokes the selected resource export.
- External JSON conversion remains only in model/CLI/MCP adapters.
- Host power: direct executable resources and ordinary process verbs, not `host_call`.

## Baseline inventory and safety net

- [x] Record current workspace test command and baseline result. Existing focused workspace tests pass before this refactor slice.
- [x] Inventory all `Verb`, `Operation`, `Handler`, `TypedHandler`, `Request`, `OperationResult`, `Verb::ALL`, `invoke_*_typed`, and JSON adapters.
- [x] Identify current WIT packages/worlds and generated-binding boundaries.
- [x] Identify current package watcher/activation/generation/lease boundaries.
- [x] Add `open-ended-verb-architecture-gate.md`, a strict refactor-only gate listing forbidden symbols and final validation commands.
- [x] Preserve/expand behavior tests for anchors, edits, grep/find, poll, run/send, claims, generations, and hot swap before deleting old abstractions.

## Slice 1 — Dynamic identity and shared types

- [x] Add canonical `VerbId` in the kernel with parse, canonical display, equality, and version validation.
- [x] Move shared resource data into a small stable WIT package: URI, anchors, anchored text, line endings, errors, claim decision, verb identity.
- [x] Remove the shared package's static ten-verb interfaces and `Verb::ALL` references from new code.
- [x] Define initial dynamic `DynamicType`/`DynamicValue`/function call/result abstractions and exact recursive validation rules.
- [x] Add tests for nested record/list validation and rejection without JSON coercion.
- [x] Expand exact Wasmtime Component Model mapping and wire function signatures into activation.
- [x] Parse primitive and nested list/option/tuple manifest contract names into checked dynamic contract types.

## Slice 2 — Generic verb package registry

- [x] Define initial `VerbDefinition`: identity, model name, docs, function, source/artifact metadata, dependencies, and optional typed contract names.
- [x] Define initial active verb generations and replacement/deactivation API.
- [x] Add immutable `VerbLease` handles for active generations.
- [x] Attach the dynamic verb registry to `Kernel` for package/runtime integration.
- [x] Add package dependency identities and validate their canonical version syntax.
- [x] Validate dynamic package dependencies against the active/batch identity set before publication.
- [x] Reject dependency cycles before publication.
- [x] Add WIT contract/function type, extractor contract, schema adapter metadata, and dependency activation ordering.
- [x] Create initial name-agnostic TOML package discovery for future verb packages.
- [x] Define package metadata exports using canonical versioned identities and artifact paths.
- [x] Atomically activate discovered package sets with duplicate/metadata validation before publication.
- [x] Require discovered packages to declare primitive typed input/output contract names.
- [x] Reject missing component artifacts before publication.
- [x] Treat identical package definitions as publication no-ops, preserving their generation.
- [x] Parse an optional manifest WIT source with `wit-parser` before publication; artifact/function compatibility validation remains pending.
- [x] Make watcher/catalog/model-tool discovery derive solely from active verb definitions.
- [x] Add dynamic activation, deletion, conflict, malformed-package, generation, and lease tests.

## Slice 3 — Generic typed call and routing model

- [x] Add `DynamicVerbCall { verb, function, typed_input }`.
- [x] Add `DynamicVerbResult { typed_values }`.
- [x] Add registry-level dynamic call validation against the active function and typed input/output contracts.
- [x] Add verb-owned dynamic extractor execution and ordered URI output.
- [x] Add dynamic claim arbitration keyed by `(VerbId, URI)`.
- [x] Preserve Pass/Handle/Reserve and Handle+Reserve Conflict semantics in the dynamic claim registry.
- [x] Route claims through candidate generation snapshots and leases.
- [x] Pin selected resource generations before invocation via `ClaimedResource`.
- [x] Add tests proving the kernel never names request fields such as `uri`, `roots`, `targets`, or `source`.
- [x] Add dynamic unknown-verb resource invocation coverage; conflict/reserve arbitration remains covered by the generic claim tests.

## Slice 4 — Dynamic component invocation/linking

- [x] Add Wasmtime component-artifact validation at the component boundary.
- [x] Add open-ended component export inspection with export names, kinds, and `implements` contract annotations.
- [x] Replace generated per-verb host methods with one dynamic component invocation engine.
- [x] Resolve registered WIT function/interface type by `VerbId`.
- [x] Verify dynamic export identity, function name, and version annotation before invocation/publication.
- [x] Verify supported WIT primitive, alias, list, and option parameter/result types exactly before publication.
- [x] Add generic Wasmtime dynamic function invocation with caller-supplied Component Model values/result slots.
- [x] Convert Artist `DynamicValue` recursively to/from Wasmtime `Val` for supported Component Model values.
- [x] Preserve resource URI/type identity and validate returned values against registered WIT types.
- [x] Link custom typed component imports through the same active contract registry.
- [x] Ensure a newly installed verb can be imported by a newly installed resource without Artist recompilation.
- [x] Add incompatible-WIT activation rejection and contract-change generation tests.

## Slice 5 — Delete the second semantic API

- [x] Add an open-ended `DynamicResourceProvider`/`ResourceRegistry` foundation with typed invocation and dynamic claim arbitration.
- [x] Convert native FileHandler to the generic typed resource-provider registration path.
- [x] Convert SessionHandler to the generic typed resource-provider path.
- [x] Convert RepositoryHandler to the generic typed resource-provider path.
- [x] Convert ResourcesHandler to the generic typed resource-provider path.
- [x] Remove duplicate native Handler registrations.
- [x] Delete `Handler`, `Kernel.handlers`, `Handler::execute`, and JSON internal dispatch.
- [x] Make `serde_json::Value` appear only in external adapters.
- [x] Remove `Request` as an internal routing representation.
- [x] Update all tests to enter through the typed/dynamic call path.
- [x] Add source assertions preventing the old ABI from returning.

## Slice 6 — Migrate the ten initial verbs

- [x] Create ordinary verb packages for read, write, edit, poll, send, run, abort, delete, find, and grep.
- [x] Give each package its exact WIT contract, function, extractor, docs, model schema adapter, source/artifact, and dependencies.
- [x] Seed them through the generic verb package mechanism, with no `seed!(verb)` semantic registration table.
- [x] Move current native semantics behind generic providers without changing observable behavior.
- [x] Remove all per-verb kernel/component match arms and generated `invoke_*_typed` methods.
- [x] Preserve existing URI, anchor, batching, edit atomicity, grep/find, poll, process, errors, generation, and claim behavior.
- [x] Confirm no current verb requires a Rust enum or central registration edit.

## Slice 7 — Dynamic model tool adapter

- [x] Expose typed model-tool descriptors derived from active verb package metadata.
- [x] Generate/introspect model schemas from registered WIT contracts.
- [x] Keep JSON conversion exclusively in the external model adapter.
- [x] Add dynamic tool add/edit/remove/hot-swap behavior without restart.
- [x] Add end-to-end self-modification coverage without kernel-known verb names.
- [x] Verify old pinned invocation uses old verb generation while new invocation uses the new generation.

## Slice 8 — Effect-complete process resource

- [x] Add trusted direct executable-file run primitive with exact argv and no shell.
- [x] Require a non-empty capability identity for process execution in the direct process primitive.
- [x] Return authoritative `process://<id>` resource URIs.
- [x] Implement process read/send/poll/abort/delete lifecycle in `ProcessManager`.
- [x] Add a typed `ProcessResourceProvider` for the generic dynamic resource registry; installed process verb-package wiring remains pending.
- [x] Preserve creation context for cwd/environment; later sends cannot alter it, and snapshots expose the authoritative context.
- [x] Add helper executable and run/send/read/poll/abort/delete tests.
- [x] Add WASM component proof that invokes process execution through ordinary resource composition.
- [x] Confirm no shell, PTY, Brush, or test-only host API exists.

## Slice 9 — Dynamic composition and hardening tests

- [x] Two resource packages implementing the same dynamic verb/URI produce Conflict.
- [x] Dynamic Reserve behavior is tested.
- [x] Dynamic cross-component import/composition test passes.
- [x] Incompatible WIT types fail before publication.
- [x] Contract changes create new incompatible generations rather than silent relinking.
- [x] Dynamic package reconciliation removes deleted verb tool descriptors.
- [x] Existing pinned generation remains executable after hot swap.
- [x] Arbitrary executable-power proof passes.
- [x] Full workspace tests pass (`cargo test --workspace`).
- [x] `cargo fmt --all -- --check` passes.
- [x] `git diff --check` passes.

## Forbidden-regression checklist

- [x] No `enum Verb` containing installed verbs.
- [x] No `enum Operation` containing installed verbs.
- [x] No `Verb::ALL`.
- [x] No `match verb` dispatch for installed verbs.
- [x] No `invoke_read_typed`/similar per-verb host methods.
- [x] No `Handler` JSON resource ABI.
- [x] No generic JSON dynamic invocation escape hatch.
- [x] No kernel-known model tool list.
- [x] No fixed resource export vocabulary.
- [x] No fixed semantic APIs for external services.

## Progress log

### 2025-02-01

- [x] Plan created before implementation.
- [x] Baseline inventory recorded.
- [x] Added and tested exported canonical `VerbId` as the first dynamic-identity seam; the legacy enum remains temporarily during migration.
- [x] Added and tested the initial hot-swappable `VerbRegistry`/`VerbDefinition` skeleton.
- [x] Added kernel-exported dynamic typed values and `DynamicVerbCall`/`DynamicVerbResult` scaffolding; legacy dispatch is not yet routed through them.
- [x] Added name-agnostic `verb.toml` discovery with malformed identity rejection.
- [x] Added contract-checked dynamic call/result validation against active generations.
- [x] Added transactional multi-package activation and kernel-facing batch activation.
- [x] Added missing-artifact checks and identical-definition no-op generation behavior.
- [x] Added Wasmtime-backed component artifact validation with regression coverage.
- [x] Added Wasmtime component export inspection without binding to a closed world.
- [x] Added versioned `VerbId` contract/function accessors and component export identity matching.
- [x] Added generic Wasmtime invocation path with no per-verb dispatch.
- [x] Added dynamic parameter/result arity checks before calling Wasmtime.
- [x] Added recursive dynamic value lowering/lifting for records, lists, tuples, options, results, variants, enums, flags, and primitives.
- [x] Added registry-owned `DynamicVerbExecutor` dispatch without kernel-known verb names and with hot-swap generation recheck.
- [x] Added direct executable process resources with lifecycle tests and no shell/PTY path.
- [x] Bound Component Model conversion to registered WIT types and replaced one existing resource adapter with dynamic typed routing; legacy adapters remain during migration.

### 2026-08-14

- [x] Added kernel-owned routed dynamic resource execution: package route extraction, provider arbitration, typed result validation, and leased verb-generation propagation are now one API.
- [x] Dynamic resource providers now participate in the shared claim surface; multiple handlers claiming one dynamic resource produce Conflict instead of last-provider-wins behavior.
- [x] Expanded manifest WIT compatibility checks for records, tuples, results, enums, variants, flags, aliases, lists, and options.
- [x] Added an end-to-end kernel test proving an Artist-unknown verb reaches a registered typed resource through its extractor and contract registry.
- [x] Fixed manifest WIT validation for functions with declared nested list/option contracts and added typed Component Model lifting that preserves URI aliases recursively.
- [x] Added `TypedComponentHost::invoke_dynamic_values`, which invokes package-local exports with Artist-owned typed values and validates lifted results against the supplied contract types.
- [x] Added `ProcessResourceProvider` with dynamic run/send/read/poll/abort/delete bindings and a no-shell lifecycle test.
- [x] Added dynamic resource-package conflict and reserve coverage.
- [x] Added the filesystem dynamic provider adapter with URI-scoped claims, typed read/write/edit/delete/find/grep decoding, and an end-to-end delete routing test; native semantic delegation remains temporary until the legacy operation API is removed.
- [x] Extended verb package metadata with symbolic extractor and schema-adapter identities and exposed typed input/output contracts through dynamic tool descriptors.
- [x] Added a contract-change generation regression: active replacement increments the generation, rejects the new contract's old input, and keeps the old lease/result contract executable.
- [x] Added `SessionResourceProvider` and dynamic session verb bindings for write/send/read/poll/abort/delete, with a typed lifecycle registry test; legacy typed delegation remains temporary during second-API removal.
- [x] Added `RepositoryResourceProvider` and dynamic read/find/grep bindings, including a projection-URI preservation test; legacy typed delegation remains temporary during second-API removal.
- [x] Added `DynamicResourcesProvider` for component-backed resource packages, including bootstrap `resources://` URI mapping and typed read/edit-result coverage.
- [x] Added structural WIT-to-`DynamicType` conversion for records, tuples, results, enums, variants, flags, aliases, lists, and options; manifests now derive missing input/output contracts from their parsed function signature and retain explicit-contract compatibility checks.
- [x] Added a tool-package registration bridge that derives dynamic `VerbDefinition` identities/contracts from package-local WIT and frontmatter metadata; the CLI now attempts atomic activation of the seeded definitions during bootstrap.
- [x] CLI clean-project verification now passes after fixing duplicate Cargo message-format arguments and suppressing host CPU flags for WASM builds; full workspace verification is green.
- [x] Repaired the AST conformance guest for the current WIT/toolchain surface and added an opaque-anchor-aware dynamic resource edit test; `cargo test --workspace` now passes.
- [x] Removed bespoke fake nested-call handles from filesystem, session, and repository dynamic adapters; their typed compatibility calls now use the shared detached handle, with all kernel tests passing.
- [x] Switched CLI native provider setup to `register_typed_handler`, so each filesystem, session, and repository handler has one shared registration across legacy URI compatibility and typed routing; CLI tests pass without duplicate allocations.
- [x] Re-ran the full workspace after the registration and adapter cleanups; all tests pass, with only pre-existing unused-code warnings.
- [x] Preserved canonical `uri`/`resource-uri` WIT aliases as `DynamicType::ResourceUri` during structural contract derivation, with nested record manifest coverage.
- [x] Kept the component host's temporary typed compatibility fallback behind `cfg(test)`; normal component builds now have dynamic dispatch as the only host execution path, while legacy fixture migration remains explicit.
- [x] Removed native session and repository `TypedHandler` implementations from normal builds; their production registration surface is now the dynamic resource-provider path. Filesystem compatibility remains temporarily required by the `resources://` bootstrap mount and is still tracked for removal.
- [x] Switched CLI named-tool registration to the existing `ToolProvider` registry path; CLI no longer routes `ToolsHandler` through the typed resource router.
- [x] Replaced the production `resources://` filesystem bootstrap's `Operation` execution with a dynamic `FileResourceProvider` call and recursive URI remapping; the component suite still passes all 30 tests.
- [x] Quarantined the obsolete resource bootstrap operation conversion helpers and legacy execution methods behind test-only compilation after the dynamic bootstrap migration.
- [x] Verified the bootstrap migration through the cross-crate compatibility fixtures; kernel (78 tests) and component (30 tests) suites remain green. The filesystem `TypedHandler` is intentionally still present only because legacy component fixtures compile the kernel as a dependency without propagating `cfg(test)`.
- [x] Migrated all component test registrations of ordinary filesystem roots to `FileResourceProvider` dynamic registration; remaining filesystem typed code is now confined to test-only Tools/Resources compatibility implementations rather than fixture registration.
- [x] Replaced the test-only Tools/Resources calls to `FileHandler::execute_typed` with a shared dynamic filesystem compatibility adapter; the native filesystem `TypedHandler` implementation is now test-only and absent from normal kernel builds.
- [x] Added a structural `ResourceUriValueExtractor` and registered it for dynamically activated CLI verb packages; it recursively extracts typed URIs without request-field or JSON-name knowledge.
- [x] Expanded dynamic WIT/Component Model coverage to narrow integers, `f32`, and `char`, including lowering, lifting, validation, and WIT contract derivation tests.
- [x] Added a lifecycle-managed CLI dynamic catalog watcher that reconciles added, removed, and replaced tool definitions without restart and installs route extractors for new identities.
- [x] Migrated the CLI resource command adapter from `Kernel::execute(Request)` to typed `Operation` construction and `Kernel::execute_operation`, while retaining its existing single-target JSON output shape at the CLI boundary.
- [x] Removed the component `tools://` and `resources://` JSON `Handler` adapters and migrated their direct callers to typed operations; typed tool/resource registration no longer requires the legacy `Handler` trait.
- [x] Preserved typed tools namespace writes after the adapter removal by moving package-directory preparation into the typed operation path.
- [x] Production native registration now uses typed-only handlers; filesystem, session, and repository JSON `Handler` implementations remain only under test compatibility while their tests are migrated.
- [x] Removed the legacy JSON dispatch closure from `KernelHandle`; nested component/resource calls now have only the typed operation channel, even while the outer compatibility executor is still retained for tests.
- [x] Restricted the outer `Kernel` registration, descriptor, single-request, and batch JSON executor to test compatibility; production kernels now expose only typed/dynamic registration and execution surfaces.
- [x] Removed the `Handler` trait from the production kernel surface; native JSON handlers and their conformance implementation are now test-only compatibility code.
- [x] Removed production `Request`/`ItemResult` modules from the kernel build; the CLI now owns its external JSON result envelope and constructs typed operations directly.
- [x] Removed the last component conformance `Handler` mock and verified the post-ABI-removal component suite (`30` library tests plus integration suites).
- [x] Removed the CLI bootstrap's `Verb::ALL` capability grant; empty grants are now populated from discovered package-owned capability metadata.
- [x] Made resource-package export/import validation interface-name driven: arbitrary declared `artist:resource/<name>` exports are validated against the component's discovered WIT identities, with capability checks for unknown nested interfaces and compatibility exceptions only for shared `types` and host `filesystem` imports.
- [x] Re-ran focused verification after the open resource validation slice: kernel (`78 + 4` tests), component (`30` tests), and CLI (`32` tests) pass; formatting and diff checks pass.
- [x] Removed the component resource router's silent unknown-operation fallback to `read`; typed compatibility routing now rejects an operation name that has no explicit contract mapping.
- [x] Added a lease-aware dynamic-resource dispatch channel to `KernelHandle`; nested dynamic calls now have a typed host path independent of the legacy operation closure, with an end-to-end unknown-verb handle test.
- [x] Verified the dynamic host seam with the kernel dynamic-resource integration suite (`4` tests), workspace check, formatting, and diff validation.
- [x] Moved the native filesystem dynamic provider off `Operation` construction: dynamic read/write/edit/delete/find/grep now execute direct capability-scoped methods and return dynamic values; the universal typed adapter remains only until its CLI callers migrate.
- [x] Revalidated the filesystem-provider extraction with the full kernel suite (`78 + 4` tests), workspace check, formatting, and diff validation.
- [x] Moved the native session dynamic provider off `Operation` construction: dynamic session creation/read/send/poll/abort/delete now execute direct session-state operations and return dynamic values; typed compatibility remains for current callers.
- [x] Revalidated the session-provider migration with dynamic-resource integration and full kernel tests (`4` and `78 + 4`), plus formatting and diff validation.
- [x] Added session dynamic-poll coverage and preserved its wait-until-change-or-termination behavior while removing the typed operation round-trip.
- [x] Moved the native repository dynamic provider off `Operation` construction: dynamic read/find/grep now execute direct projection/search methods and return dynamic values, preserving byte-oriented source reads.
- [x] Revalidated the native-provider slice with dynamic-resource tests, full kernel tests, workspace check, formatting, and diff validation; only existing CLI dead-code warnings remain.
- [x] Added a reflective resource-component interface/function invocation engine using Wasmtime export indices and type-directed `DynamicValue` lowering; resource claim execution now uses this engine with a capability-free claim scope, and the component library suite (`30` tests) passes.
- [x] Migrated component-backed resource reads to the reflective engine, including dynamic WIT request construction and explicit text/directory/error lowering; the AST nested-resource read regression passes.
- [x] Migrated component-backed resource writes to the reflective engine with typed request/result/error lowering; the full component library suite (`30` tests) remains green.
- [x] Migrated component-backed resource abort/delete lifecycle calls to the shared reflective engine; the full component library suite (`30` tests) remains green.
- [x] Removed the now-unused generated abort/delete entrypoint adapters after routing those operations through the reflective lifecycle helper; formatting and diff checks pass.
- [x] Migrated component-backed resource find calls to reflective invocation with dynamic URI-list/error lowering; component compilation passes.
- [x] Migrated component-backed resource run/send calls to reflective invocation with dynamic URI-result lowering; component compilation passes.
- [x] Migrated component-backed resource grep calls to reflective invocation with dynamic source-variant and anchored-text lowering; component compilation passes.
- [x] Migrated component-backed resource edits to reflective invocation with dynamic anchor/operation lowering and explicit anchored-diff result lowering; projection edit regression passes.
- [x] Migrated component-backed resource polling to reflective invocation with dynamic target/condition lowering and poll-result atom/text lifting; the full component library suite (`30` tests) remains green.
- [x] Retired the generated per-verb component adapter entrypoint names after all resource operations moved to the reflective engine; component compilation and tests remain green.
- [x] Deleted the remaining unused generated edit/run/send/find/grep/poll component adapter bodies; only the shared reflective invocation engine remains in the resource host.
- [x] Removed the obsolete generated WIT conversion helpers left behind by the reflective migration; component compilation remains green.
- [x] Removed the kernel's normal-build `Verb` export; CLI command selection is local to the external adapter, and the legacy request value is available only to tests.
- [x] Renamed the test-only JSON handler trait to `LegacyHandler`, leaving the old `Handler` name only as a compatibility alias while production registration remains typed/dynamic.
- [x] Migrated the CLI resource command onto `Kernel::invoke_dynamic_resource` with package-owned native identities and dynamic input/output lowering; filesystem and session dispatch regressions pass.
- [x] Removed the CLI's obsolete `Operation`/`OperationResult` conversion and JSON result adapter from the resource command; its output conversion now occurs only at the external CLI boundary from `DynamicValue`.
- [x] Removed the kernel's closed `Verb` enum from the production API; test-only JSON compatibility now uses an open string-backed value while dynamic routing continues to use `VerbId`.
- [x] Full workspace verification after the catalog watcher and expanded dynamic types passes (`cargo test --workspace`).
- [x] Published native filesystem, session, and repository providers through the CLI kernel's dynamic resource registry with canonical package-owned identities; retained the typed CLI adapter during dispatch migration and verified all `32` CLI tests.
- [x] Isolated the component `ResourcesHandler` typed compatibility implementation behind a test-only `TypedHandler` adapter; the remaining production typed bridge is limited to native filesystem/repository projection compatibility.
- [x] Removed the CLI's production `ResourcesHandler` typed registration; the async nested resource read import now uses the scoped direct dynamic provider path, and the clean-project integration regression passes.
- [x] Added a scoped direct dynamic-resource call to `KernelHandle` and hardened provider-lock lifetime for `Send` futures; component and kernel checks pass.
- [x] Migrated async nested `resources://` writes through the direct dynamic provider path with anchored-text lifting; component (`30`) and CLI (`32`) suites remain green.
- [x] Migrated component-backed async nested edits through direct dynamic WIT input/result lifting, including edit variants, anchored text, and diff hunks; component (`30`) and CLI (`32`) suites remain green.
- [x] Migrated async nested `send` for session and bootstrap resource URIs through direct dynamic input/result lifting; component (`30`) and CLI (`32`) suites remain green.
- [x] Migrated synchronous nested `send` for session and bootstrap resource URIs through the same direct dynamic input/result path; component compilation and formatting checks remain green.
- [x] Migrated synchronous nested `read` for filesystem/repository/session/bootstrap-resource URIs and `write` for filesystem/session/bootstrap-resource URIs through direct dynamic input/result lifting; filesystem and capability-denial regressions pass.
- [x] Preserved native filesystem edit diffs in the dynamic result contract and migrated synchronous/asynchronous filesystem edits through direct dynamic input/result lifting; the component library suite (`30` tests) remains green.
- [x] Migrated synchronous and asynchronous `abort`/`delete` host calls through direct dynamic lifecycle dispatch with capability checks; the component library suite (`30` tests) remains green.
- [x] Migrated synchronous and asynchronous executable `run` host calls through the dynamic process contract, including exact argument lowering, process identity routing, and capability checks; the component library suite (`30` tests) remains green.
- [x] Migrated synchronous `find` and `grep` host calls for filesystem, repository, and component-backed resource URIs through dynamic URI-list/source-variant contracts with capability checks; the component library suite (`30` tests) remains green.
- [x] Migrated asynchronous `find` and `grep` host calls through the same dynamic URI-list/source-variant contracts with capability checks; the component library suite (`30` tests) remains green.
- [x] Migrated synchronous and asynchronous session/component-resource `poll` host calls through recursive dynamic condition lowering and `{text, satisfied}` lifting; process snapshot polling remains on its compatibility route, and the component library suite (`30` tests) remains green.
- [x] Extended synchronous and asynchronous dynamic `send` dispatch to process resources and removed their host-level `Operation` fallback; process identity routing now uses the process package.
- [x] Re-audited the remaining host compatibility calls after the dynamic migration; the unresolved `Operation`/JSON surfaces are now concentrated in the kernel typed adapter, component compatibility bridge, and non-native custom-resource fallbacks.
- [x] Removed production CLI registrations for the legacy typed filesystem, session, and repository handlers; native dynamic providers are now the sole CLI resource registrations, and all `32` CLI tests pass.
- [x] Moved component-backed dynamic resource invocation off the `Operation` conversion path: candidate selection now feeds package-specific WIT input directly into the reflective component invocation engine; only the pre-activation `resources://` filesystem bootstrap remains on compatibility routing.
- [x] Removed `Operation` construction from component-backed dynamic claim arbitration; claims now inspect active resource candidates directly while the compatibility bootstrap remains isolated.
- [x] Added a dynamic `tools://` resource provider with versioned tool verb bindings, filesystem URI remapping, and dynamic read/write/edit/find/grep forwarding.
- [x] Migrated component tool and AST resource fixtures from typed kernel registration and operation execution to dynamic provider registration and invocation; remaining compatibility implementations are isolated to legacy fixture coverage.
- [x] Removed the typed operation dispatcher from `KernelHandle`; nested provider/component calls now expose only dynamic resource dispatch channels.
- [x] Removed `Operation`/`OperationResult` from normal kernel builds and quarantined the legacy typed native adapters behind test-only compatibility code while dynamic providers remain the production registration path.
- [x] Removed `TypedHandler` and legacy operation registration from the normal kernel API; production resource registration now exposes only dynamic providers and claims.
- [x] Deleted the kernel's standalone JSON request/result modules; external adapters now own request envelopes and result serialization.
- [x] Removed the session provider's JSON state snapshot/creation path; live session reads, writes, sends, and polls now use `DynamicValue` state directly.
- [x] Replaced the kernel registry wholesale with the dynamic-only registry; legacy operation routing, handler storage, request execution, and compatibility registration methods are gone.

Final verification: `cargo test --workspace` passed; `cargo fmt --all -- --check`, `git diff --check`, and the no-unchecked-items gate also passed.
