Make polling lossless.

Current problem: polling only returns output appended after that individual poll call starts, so output produced between polls can be lost.

Fix:

Add an opaque continuation cursor to the poll protocol.
PollRequest accepts an optional cursor.
PollReply returns the next cursor.
A caller using the returned cursor must receive every byte/event exactly once across sequential polls unless it intentionally changes/discards the cursor.
Preserve the existing match/close/timeout behavior.
Update the model-facing poll tool accordingly.
Add tests covering output emitted between consecutive poll calls.
Remove the one-route/one-operation restriction on resource components.

Current problem: the resource ABI supports multiple routes and multiple operations per route, but plugin activation rejects them.

Fix:

Allow one resource component to declare multiple routes.
Allow each route to declare multiple operations.
Keep validation atomic: validate the entire candidate route set before replacing the active owner's routes.
Do not weaken ownership/conflict validation.
Make resource_component! actually generic.

Current problem: the generic resource-component macro hardcodes file:///**.

Fix:

Change resource_component! so the caller supplies the route declaration, including base glob, optional projection glob, supported operations, and signals.
Remove the hardcoded file:///**.
Migrate existing callers.
Do not add scheme-specific generic macros unless they materially reduce code.
Correct resource-topology invalidation.

Current problem: router topology generation changes for route registration/replacement and successful Move, but other operations can also create/remove/change visible resource topology.

Fix:

Define which successful resource operations can invalidate topology.
For now, conservatively invalidate after successful Write, Edit, Move, Run, and Signal.
Keep Read, Children, and Poll non-invalidating.
Add tests proving topology watchers are notified.
Correctness takes precedence over minimizing rescans.

Spec item 5 rejected--we are specifically maintaining default allow-all.

Decide the concurrency model for WASM resource components.

Current problem: each loaded component has one Wasmtime store behind one mutex, and the mutex is held across an awaited resource call. A long-running resource call prevents other calls into that same component.

Decision needed — 6B locked.

6B — Resource components support concurrent operations. Redesign hosting so independent calls into one logical resource provider can proceed concurrently while safely sharing provider state.

Enforce declared resource signals.

Current problem: routes advertise named signals and payload schemas, but invocation does not enforce those declarations.

Fix:

A Signal request must name a signal advertised by the selected route.
Reject undeclared signal names.
Validate the payload against that signal's advertised JSON schema.
Define absent payload consistently, preferably as JSON null.
Keep route metadata and runtime enforcement sourced from the same declaration.
Add tests for undeclared signals, malformed payloads, valid payloads, and routes with Signal support but no matching named signal.
Define lifecycle extension composition semantics.

Current problem: prompt/context/model/hooks/events plugins are iterated in loaded-slot order, making behavior depend on runtime activation order rather than declared semantics.

Decision needed — 8B.

8B — Ordered composition. Add explicit deterministic ordering/priority metadata and define composition/conflict semantics for each lifecycle capability.

Decide whether plugin source packages are Rust-specific or language-neutral.

Current problem: the runtime boundary is WIT/component-model based, but package discovery/build/activation assumes Cargo and Rust source packages.

Decision needed — 9B selected.
9B — Plugin packages are language-neutral components. Separate component activation from source building. Cargo becomes one build adapter rather than part of the package model.

My preference: 9B if third-party/extensible plugin development is a serious goal; otherwise 9A is much simpler and perfectly defensible.

Make the resource fabric mount layer cross-platform.

Current problem: the aggregate Artist resource filesystem/search fabric is implemented only for Linux.

Fix:

Keep the logical resource graph → mounted filesystem → FFF search architecture.
Abstract the mount backend behind a platform adapter.
Linux: current FUSE backend.
macOS: macFUSE backend.
Windows: WinFsp/FUSE-compatible backend.
Keep ResourceRouter, projection semantics, URI mapping, topology generation, and search behavior platform-independent.
Platform adapters should contain only filesystem-integration differences; resource semantics must not fork by OS.
Add shared conformance tests that run against each mount backend.

That is a much smaller and cleaner issue than my previous 10B. There is no architectural reason to decouple search from the mounted fabric merely because Linux was the first implementation.
