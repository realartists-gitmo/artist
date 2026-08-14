# Artist Open-Ended Verb/Resource Architecture Refactor

Status: planning complete; implementation not started.

This document is the durable implementation ledger. Update checkboxes and notes after every horizontal slice. Do not mark a checkbox complete without a focused test or compile-time assertion.

## Non-negotiable end state

- [ ] Exactly one semantic kernel execution path: typed/dynamic Component Model values only.
- [ ] No legacy `Handler`, `Kernel.handlers`, JSON resource ABI, or internal `Request -> serde_json::Value` dispatch.
- [ ] No closed `Verb` or `Operation` enum in the kernel.
- [ ] Canonical versioned `VerbId` identifies a concrete WIT contract/interface.
- [ ] Claims, exports, routing, generations, and capabilities use dynamic contract identities.
- [ ] Ten current verbs are ordinary installed verb packages, not kernel special cases.
- [ ] Verb request/result schemas are owned by verb WIT contracts.
- [ ] Verb-owned typed routing extractors provide ordered `ResourceUri`s.
- [ ] Kernel dynamically validates and invokes registered WIT functions.
- [ ] Components dynamically link newly installed verb contracts.
- [ ] Model tools derive from active verb packages and hot-swap.
- [ ] Direct executable-file `run` creates ordinary `process://` resources.
- [ ] No shell parsing, PTY, Brush, or fixed semantic host APIs.
- [ ] Self-modifying new-verb, dynamic composition, conflict/reserve, incompatibility, and arbitrary-process-power tests pass.

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
- [ ] Identify current package watcher/activation/generation/lease boundaries.
- [ ] Add a refactor-only compile/test gate document listing forbidden symbols.
- [ ] Preserve/expand behavior tests for anchors, edits, grep/find, poll, run/send, claims, generations, and hot swap before deleting old abstractions.

## Slice 1 — Dynamic identity and shared types

- [x] Add canonical `VerbId` in the kernel with parse, canonical display, equality, and version validation.
- [ ] Move shared resource data into a small stable WIT package: URI, anchors, anchored text, line endings, errors, claim decision, verb identity.
- [ ] Remove the shared package's static ten-verb interfaces and `Verb::ALL` references from new code.
- [x] Define initial dynamic `DynamicType`/`DynamicValue`/function call/result abstractions and exact recursive validation rules.
- [x] Add tests for nested record/list validation and rejection without JSON coercion.
- [ ] Expand exact Wasmtime Component Model mapping and wire function signatures into activation.

## Slice 2 — Generic verb package registry

- [x] Define initial `VerbDefinition`: identity, model name, docs, function, source/artifact metadata.
- [x] Define initial active verb generations and replacement/deactivation API; lease integration remains pending.
- [x] Attach the dynamic verb registry to `Kernel` for package/runtime integration.
- [ ] Add WIT contract/function type, extractor contract, schema adapter metadata, dependencies, and immutable invocation leases.
- [x] Create initial name-agnostic TOML package discovery for future verb packages.
- [x] Define package metadata exports using canonical versioned identities and artifact paths.
- [x] Atomically activate discovered package sets with duplicate/metadata validation before publication.
- [x] Reject missing component artifacts before publication.
- [x] Treat identical package definitions as publication no-ops, preserving their generation.
- [ ] Validate actual WIT contracts and artifact compatibility before publication.
- [ ] Make watcher/catalog/model-tool discovery derive solely from active verb definitions.
- [ ] Add dynamic activation, deletion, conflict, malformed-package, generation, and lease tests.

## Slice 3 — Generic typed call and routing model

- [ ] Add `DynamicVerbCall { verb, function, typed_input }`.
- [ ] Add `DynamicVerbResult { typed_values }`.
- [x] Add registry-level dynamic call validation against the active function and typed input/output contracts.
- [ ] Add verb-owned extractor execution and URI ordering.
- [ ] Route claims using only `(VerbId, URI)` and candidate generation snapshots.
- [ ] Preserve Pass/Handle/Reserve and multiple-owner Conflict semantics.
- [ ] Pin selected resource generations before invocation.
- [ ] Add tests proving the kernel never names request fields such as `uri`, `roots`, `targets`, or `source`.
- [ ] Add dynamic conflict and reserve tests for a verb unknown to Artist source.

## Slice 4 — Dynamic component invocation/linking

- [ ] Replace generated per-verb host methods with one dynamic component invocation engine.
- [ ] Resolve registered WIT function/interface type by `VerbId`.
- [ ] Verify resource export identity, function name, parameter types, result types, and version exactly before publication.
- [ ] Invoke Component Model values dynamically and validate returned values.
- [ ] Link custom typed component imports through the same active contract registry.
- [ ] Ensure a newly installed verb can be imported by a newly installed resource without Artist recompilation.
- [ ] Add incompatible-WIT activation rejection and contract-change generation tests.

## Slice 5 — Delete the second semantic API

- [ ] Convert native FileHandler to the generic typed resource-provider registration path.
- [ ] Convert SessionHandler to the generic typed resource-provider path.
- [ ] Convert RepositoryHandler to the generic typed resource-provider path.
- [ ] Convert ResourcesHandler to the generic typed resource-provider path.
- [ ] Remove duplicate native Handler registrations.
- [ ] Delete `Handler`, `Kernel.handlers`, `Handler::execute`, and JSON internal dispatch.
- [ ] Make `serde_json::Value` appear only in external adapters.
- [ ] Remove `Request` as an internal routing representation.
- [ ] Update all tests to enter through the typed/dynamic call path.
- [ ] Add source assertions preventing the old ABI from returning.

## Slice 6 — Migrate the ten initial verbs

- [ ] Create ordinary verb packages for read, write, edit, poll, send, run, abort, delete, find, and grep.
- [ ] Give each package its exact WIT contract, function, extractor, docs, model schema adapter, source/artifact, and dependencies.
- [ ] Seed them through the generic verb package mechanism, with no `seed!(verb)` semantic registration table.
- [ ] Move current native semantics behind generic providers without changing observable behavior.
- [ ] Remove all per-verb kernel/component match arms and generated `invoke_*_typed` methods.
- [ ] Preserve existing URI, anchor, batching, edit atomicity, grep/find, poll, process, errors, generation, and claim behavior.
- [ ] Confirm no current verb requires a Rust enum or central registration edit.

## Slice 7 — Dynamic model tool adapter

- [ ] Generate model-facing tool definitions from active verb packages.
- [ ] Generate/introspect model schemas from registered WIT contracts.
- [ ] Keep JSON conversion exclusively in the external model adapter.
- [ ] Add dynamic tool add/edit/remove/hot-swap behavior without restart.
- [ ] Add end-to-end `uppercase` self-modification test; `uppercase` must not occur in Artist source.
- [ ] Verify old pinned invocation uses old verb generation while new invocation uses the new generation.

## Slice 8 — Effect-complete process resource

- [ ] Add trusted direct executable-file run primitive with exact argv and no shell.
- [ ] Require a dynamic capability identity for process execution.
- [ ] Return authoritative `process://<id>` resource URIs.
- [ ] Integrate process read/send/poll/abort/delete with generic verb/resource routing.
- [ ] Preserve creation context for cwd/environment; later sends cannot alter it.
- [ ] Add helper executable and run/send/read/poll/abort/delete tests.
- [ ] Add WASM component proof that invokes process execution through ordinary resource composition.
- [ ] Confirm no shell, PTY, Brush, or test-only host API exists.

## Slice 9 — Dynamic composition and hardening tests

- [ ] Two resource packages implementing the same dynamic verb/URI produce Conflict.
- [ ] Dynamic Reserve behavior is tested.
- [ ] Dynamic cross-component import/composition test passes.
- [ ] Incompatible WIT types fail before publication.
- [ ] Contract changes create new incompatible generations rather than silent relinking.
- [ ] New verb deletion removes model tool visibility.
- [ ] Existing pinned generation remains executable after hot swap.
- [ ] Arbitrary executable-power proof passes.
- [ ] Full workspace tests pass.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `git diff --check` passes.

## Forbidden-regression checklist

- [ ] No `enum Verb` containing installed verbs.
- [ ] No `enum Operation` containing installed verbs.
- [ ] No `Verb::ALL`.
- [ ] No `match verb` dispatch for installed verbs.
- [ ] No `invoke_read_typed`/similar per-verb host methods.
- [ ] No `Handler` JSON resource ABI.
- [ ] No generic JSON dynamic invocation escape hatch.
- [ ] No kernel-known model tool list.
- [ ] No fixed resource export vocabulary.
- [ ] No fixed semantic APIs for external services.

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
- [ ] Next slice: typed dynamic component invocation and actual WIT contract activation.
