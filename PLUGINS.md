# First-class plugin packages

Artist treats the plugin catalog—not a bare WebAssembly file—as the deployment
and iteration unit. The host owns the catalog rooted at `plugins:///`; narrow
WASM resource components can expose multiple routes and operations from one
language-neutral component.

## Package layout

Every loadable directory directly below the catalog root contains
`plugin.json`:

```json
{
  "format": 2,
  "id": "artist.tool.read",
  "build": {"adapter": "cargo", "package": "artist-tool-read"},
  "component": "artist_tool_read.wasm"
}
```

The directory contains either a prebuilt component or ordinary source plus an
optional build-adapter declaration. Cargo is the built-in adapter, not part of
the package model. Shared Rust adapter inputs such as `Cargo.toml`, `Cargo.lock`,
and `sdk/` are visible in the same resource tree; the host ABI is projected read-only at
`plugins:///_abi/plugin.wit`. `target/` is omitted from resource listings. Generated
per-package state lives under `.artist/`:

```text
plugins:///tool-read/
├── plugin.json
├── Cargo.toml
├── src/lib.rs
└── .artist/
    ├── candidate.wasm
    ├── active.wasm
    └── status.json
```

`.artist/status.json` is readable but all `.artist/` state is host-owned. Binary
artifacts are intentionally omitted from the UTF-8 resource tree. Models edit
source and manifests, never forge build status or active artifacts.

Package format `2` is the only accepted format. Artist is pre-production: bump
the format directly and discard stale packages; do not add migrations or legacy
package decoders.

## Lifecycle

The package root advertises three signals:

- `build` runs the declared build adapter (Cargo runs tests and builds
  `wasm32-wasip2`) or stages the package's prebuilt component, then installs an
  inactive candidate.
- `activate` instantiates and validates the existing candidate, then replaces
  registrations owned by the declared plugin ID.
- `build-and-activate` performs both operations in order.

Signals accept no payload. Their target is a package root such as
`plugins:///tool-read`.

Activation verifies that package and component identities match, exactly one
extensibility capability is advertised, JSON Schemas and complete resource
route sets are valid, and global names do not collide with another owner.
Resource components may aggregate routes and operations. Validation happens
before replacement. A failed
build or activation records diagnostics and leaves the previous live instance
and active artifact untouched. Existing invocations retain their old `Arc`, so
replacement does not invalidate work already in flight.

Successful activation copies the candidate to `active.wasm`, records source,
candidate, and active SHA-256 revisions (including the source revision from
which each artifact was built), and changes `status.json` to `active`. Any
source mutation marks the package dirty and makes the old candidate ineligible
for activation; source changes detected during a build reject that build. The
source revision always covers the package and, for the Cargo adapter only, its
shared Cargo, SDK, and WIT inputs.
`PluginHost` restores those active artifacts from the catalog on startup.

Lifecycle providers compose deterministically by ascending descriptor
`priority`, then plugin ID. Prompt, context, and model providers are pipelines;
each receives the preceding provider's complete output, so a later provider's
value is authoritative when both modify the same field or fragment. Events are
ordered and fail-fast. Hook rewrites are retained in order, and the first stop
is terminal and prevents lower-priority hooks from running. Resource calls instantiate independent Wasmtime stores so a
long awaited call cannot block unrelated calls into the same logical provider;
guest memory is request-local, while the imported `provider-state` namespace
persists across calls and supplies atomic compare-and-swap for safely shared
mutable provider state. Host resource services remain shared as well.

Profiles gate `plugins:` access through the same tool/effect/operation/resource
policy used by every other resource. A read-only profile can inspect packages
without being able to edit, build, or activate them.
