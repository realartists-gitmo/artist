# First-class plugin packages

Artist treats the plugin catalog—not a bare WebAssembly file—as the deployment
and iteration unit. The host owns the catalog rooted at `plugins:///`; narrow
WASM resource components expose exactly one operation each for read, children,
write, edit, move, and signal.

## Package layout

Every loadable directory directly below the catalog root contains
`plugin.json`:

```json
{
  "format": 1,
  "id": "artist.tool.read",
  "cargo_package": "artist-tool-read",
  "component": "artist_tool_read.wasm"
}
```

The directory contains the plugin's ordinary source and build inputs. Shared
catalog inputs such as `Cargo.toml`, `Cargo.lock`, and `sdk/` are visible in the
same resource tree; the host ABI is projected read-only at
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

Package format `1` is the only accepted format. Artist is pre-production: bump
the format directly and discard stale packages; do not add migrations or legacy
package decoders.

## Lifecycle

The package root advertises three signals:

- `build` runs the real package tests and builds `wasm32-wasip2`, captures all
  diagnostics, and installs an inactive candidate.
- `activate` instantiates and validates the existing candidate, then replaces
  registrations owned by the declared plugin ID.
- `build-and-activate` performs both operations in order.

Signals accept no payload. Their target is a package root such as
`plugins:///tool-read`.

Activation verifies that package and component identities match, exactly one
extensibility capability is advertised, tool/resource/command components do not
aggregate definitions, JSON Schemas and routes are valid, and global names do
not collide with another owner. Validation happens before replacement. A failed
build or activation records diagnostics and leaves the previous live instance
and active artifact untouched. Existing invocations retain their old `Arc`, so
replacement does not invalidate work already in flight.

Successful activation copies the candidate to `active.wasm`, records source,
candidate, and active SHA-256 revisions (including the source revision from
which each artifact was built), and changes `status.json` to `active`. Any
source mutation marks the package dirty and makes the old candidate ineligible
for activation; source changes detected during a build reject that build. The
source revision covers the package plus shared Cargo, SDK, and WIT inputs.
`PluginHost` restores those active artifacts from the catalog on startup.

Profiles gate `plugins:` access through the same tool/effect/operation/resource
policy used by every other resource. A read-only profile can inspect packages
without being able to edit, build, or activate them.
