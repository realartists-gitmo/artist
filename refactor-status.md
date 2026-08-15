# Harness refactor status

The current source of truth is `CRORTNITE_TOOL_SURFACE_IMPLEMENTATION.md`,
including its mandatory appended invocation-stream contract. Older notes in
this file are historical where they describe pre-batch or pre-invocation
behavior.

Updated 2026-08-14.

This note records the state of the current conformance pass against the latest
resource-extension audit. The pass is complete; the final changes are committed and pushed below.

## Direction

The harness is now centered on typed kernel operations and typed WASM
components. JSON is an outer/model adapter. Resources and tools are discovered
as self-describing packages, routed by URI ownership, activated as immutable
generations, and given invocation-scoped leases. The intended next direction
remains an extensible resource/tool graph, with shell and other runtime
namespaces added on top of these primitives rather than changing the
universal contracts.

## Finished in this pass

- Restored a real AST WASM proof. The AST component now claims file symbol
  projections, performs a typed nested `resource.read`, and implements the
  proof projections for symbols and callers. Native file projections are
  disabled in the application path where the WASM proof owns them.
- Corrected resource WIT capability validation to inspect parsed named
  interface imports, including aliases such as `source-read`, rather than
  scanning only unnamed interface keys or raw source lines.
- Added the clean catalog activation fast path. Reading the resource catalog
  no longer creates a new generation for an unchanged package.
- Removed the permanent generation archive. Invocation scopes retain exact
  `Arc` generation leases; old generations can be released once those scopes
  disappear.
- Completed claim-to-invoke route pinning for resource components. The invoke
  phase reuses the claim-time decision and generation instead of calling a
  hot-reloadable component's `claim()` a second time.
- Made universal-tool seed migration conservative. A seeded package is repaired
  only when its file still exactly matches known framework seed content, so a
  customized `src/lib.rs` is preserved. A regression test exercises this.
- Kept `grep(Text)` outside resource-provider routing. Supplied anchored text
  is handled directly by the filesystem/native typed path; resource extensions
  are considered only for resource-backed grep.
- Generalized the package build substrate further: Cargo target resolution,
  WASM artifact construction, file hashing, cache lookup, SHA-256 verification,
  and provenance matching are shared by tool and resource adapters.
- Added/retained regression coverage for AST nested reads, deeper projections,
  aliased capability imports, catalog generation stability, customized seed
  preservation, duplicate writes, polling, routing, and the typed universal
  surface.

## Verified

Verified:

- `cargo check -p artist-component -p artist-kernel -p artist-cli`
- `cargo test -p artist-component --lib` — 27 passed
- `cargo test -p artist-kernel --lib` — 49 passed
- `cargo test -p artist-cli --bin artist` — 32 passed
- `cargo test --workspace` — passed; one herdr integration test was flaky on
  the first run and passed when rerun in isolation
- focused clean-project, AST, catalog, and seed-migration tests
- `cargo fmt --all -- --check`
- `git diff --check`

## Completion checks

- Workspace tests, formatting, whitespace validation, and the final diff audit
  are complete.
- Resource-to-resource dependencies use the active immutable generation selected
  during activation, recursively pinning child generations into the host.

## Explicit non-goals

This pass does not implement Brush/bash, persistent shell behavior, new verbs,
new universal request fields, a command-completion protocol, or a new URI
namespace. Those remain downstream consumers of the typed operation/component
foundation.
