# Resource Capabilities Implementation Plan

## Objective

Complete the universal resource vocabulary and make line-addressed editing and
resource introspection kernel capabilities without introducing any execution
provider or session implementation.

The completed model-facing vocabulary is exactly:

```text
read
find
grep
write
edit
move
run
signal
poll
```

Every verb remains independently packaged as one WASM component. Resource
providers remain independently packaged by operation. No component may bundle
multiple model tools or multiple resource operations.

This work does not add `bash://`, `python://`, another execution namespace, an
execution-session state machine, or a test-only plugin. Those concepts are not
needed to finish the generic contracts.

## Locked decisions

### URI projections

Artist's existing URI semantics remain authoritative:

- The URL path identifies the base resource.
- The complete query is a slash-delimited projection path.
- Query text is not parsed as `key=value` parameters.
- Fragments remain invalid.
- A projected URI is a distinct logical resource backed by its base resource,
  not a representation flag on an otherwise identical URI.

Kernel metadata occupies the reserved root projection `meta`:

```text
file:///work/a.rs?meta                 metadata for file:///work/a.rs
file:///work/a.rs?meta/symbols/foo     metadata for file:///work/a.rs?symbols/foo
```

The first projection segment `meta` is kernel-owned. Providers must not
register projection routes beginning with `meta`. Remaining segments after
`meta` identify the projection whose metadata is requested.

### Execution-neutral control

`run` and `signal` are generic resource operations even when no loaded provider
supports them.

`run` starts work through a target resource and returns the URI created for
that work. Its generic request is:

```text
target: URI
input: string
cwd?: URI
env?: list<{ name: string, value: string }>
timeout_ms?: positive integer
```

The generic contract does not define command lines, interpreters, processes,
stdin, output children, exit codes, or a state machine. A future provider will
define those resources and advertise their details through kernel metadata.

`signal` sends an out-of-band control request to an existing resource:

```text
uri: URI
name: string
payload?: string
```

The signal name and payload semantics belong to the selected provider. The
provider declares supported signals and payload schemas in its route metadata.
This is the general replacement for embedding modes such as start, send, stop,
and OS signals in a noun-specific tool.

Agent-stream abort and resource signaling are independent. Aborting a model
run never implicitly signals resources created by earlier tool calls. Runtime
shutdown and provider cleanup are provider-lifetime concerns and do not enter
the universal resource ABI.

### Polling

Keep the current poll contract unchanged:

- Poll starts at the resource's current bottom.
- It accumulates only subsequently appended UTF-8 text.
- A regex returns on match, close, or timeout.
- Poll without a regex returns on close or timeout.
- Outcomes remain `matched`, `closed`, and `timed-out`.

No execution-specific state or terminal outcome is added to poll.

### Line-anchor editing

Editing is line-addressed, batched, and atomic. It does not use exact-text
replacement and does not accept natural-language instructions.

The model-facing edit request is:

```text
uri: URI
operations: non-empty list<
  replace { start: anchor, end?: anchor, content: string }
  delete { start: anchor, end?: anchor }
  insert-before { anchor: anchor, content: string }
  insert-after { anchor: anchor, content: string }
>
```

All anchors in a batch resolve against one immutable snapshot. The batch fails
without writing when:

- an anchor is unknown, stale, or ambiguous;
- a range end precedes its start;
- two operations overlap;
- multiple operations target the same insertion point incompatibly;
- the resource changes before the provider commits the resolved edit; or
- the selected provider does not advertise `edit`.

Successful application preserves all bytes outside resolved ranges, including
line terminators. CRLF files remain CRLF except where replacement content
explicitly introduces different bytes. The provider commits the entire batch
or none of it.

## TECA anchor design

Use `teca = "=1.0.0"`. Version 1.0.0 is MIT-licensed, has MSRV 1.88, and
provides the stateful `Neighborhood` abstraction needed for canonical
shortest-unique addresses.

Do not port `main`'s mnemonic-word allocator, hidden xxHash bindings,
tree-sitter special case, per-agent SQLite anchor map, or retry-confirmation
gate. Retain the useful behavioral properties from `main`: whole-snapshot
resolution, atomic batches, overlap rejection, reverse-order byte application,
line-ending preservation, coordinated writes, updated anchors, and a concise
diff.

### Canonical line identities

TECA neighborhoods require distinct source bytes for distinct rows. Build one
identity for every line in the complete resource snapshot:

```text
line bytes
record separator
previous line bytes or BOF marker
record separator
next line bytes or EOF marker
record separator
occurrence ordinal among identical triples
```

Length-prefix every field before concatenation; separators alone are not an
unambiguous encoding. Exclude the line terminator from `line bytes`, but retain
its exact byte range separately for editing.

This identity has intentional conservative invalidation:

- edits elsewhere leave an unchanged line addressable;
- changing a line invalidates its anchor;
- changing an immediate neighbor may invalidate the anchor;
- duplicate and blank lines remain distinguishable; and
- inserting an indistinguishable duplicate may invalidate anchors in that
  duplicate neighborhood rather than risk editing the wrong line.

Insert every identity into one `teca::Neighborhood` and store the original line
index as its opaque identifier. Neighborhood construction covers the complete
resource even when `read` returns only a requested line window. Partial and full
reads of the same snapshot therefore render identical anchors for the same
line.

### Anchor wire form

Render a `NeighborhoodAddress` with TECA's canonical default lexicon. Each atom
must be emitted as its UTF-8 lexicon token and atoms must be joined with `.`.
The adapter must validate once that every embedded default atom is non-empty,
UTF-8, alphanumeric, and contains neither `.` nor `:`.

Example model presentation:

```text
ember: fn main() {
pixel.river:     work();
solar: }
```

Build a reverse lexicon map to parse model-provided anchors back into
`NeighborhoodAddress`. Reject empty components, unknown atoms, malformed UTF-8,
and prefixes that TECA resolves as ambiguous or absent.

Anchors require no issuance ledger: they are resolved against the current
canonical neighborhood. A formerly valid prefix that is no longer unique is
stale by construction. No anchor state is written into the transcript or a
side database.

## Core and WIT contracts

### Resource operations

Extend `ResourceOperation` in Rust and WIT with:

```text
run
signal
```

Replace the reserved instruction-based edit request with two distinct levels:

1. `anchored-edit-request`, accepted only by the host resource bridge from the
   model-facing edit component.
2. `edit-request`, delivered to a selected resource provider after the kernel
   resolves anchors.

The provider-facing request contains:

```text
uri: URI
expected_sha256: lowercase hex string
replacements: non-empty list<{
  start_byte: u64
  end_byte: u64
  text: string
}>
```

Provider replacements are sorted, non-overlapping byte ranges into the exact
UTF-8 snapshot identified by `expected_sha256`. The provider must compare the
current content and apply under the same operation-level lock. A mismatch is a
typed conflict, not a generic provider failure.

Add typed replies:

```text
edited { revision: string }
started { uri: URI }
signaled
```

Add a typed `conflict` resource error containing the target URI and current
revision. Preserve the existing not-found, unsupported, invalid, and provider
errors.

### Host resource imports

Keep raw `handle` for provider composition, FUSE, and internal consumers. Add
focused host imports for model presentation:

```text
read-anchored(read-request) -> anchored-read-reply
edit-anchored(anchored-edit-request) -> anchored-edit-reply
metadata(uri) -> resource-metadata
```

`read-anchored` reads the complete raw text through the router, constructs its
TECA neighborhood, and then applies `start-line`/`line-count` only to rendering.

All model-facing text locations use anchors rather than line numbers. In
particular, grep's native index positions are translated against a fresh,
complete routed snapshot before returning matches or context, and the result is
rejected if the indexed text no longer agrees with that snapshot. Find returns
resource URIs and has no text-location output to translate.

`edit-anchored` performs this transaction:

1. Read the complete raw UTF-8 snapshot through the router.
2. Compute SHA-256 and build its TECA line neighborhood.
3. Resolve every anchor and validate the complete batch.
4. Convert operations to non-overlapping byte replacements.
5. Route one provider-facing edit request with the expected revision.
6. Re-read the committed text and verify the returned revision.
7. Return a unified diff plus anchors for inserted/replaced lines.

The tool output must use the shared model-output budget, truncate only at UTF-8
boundaries, and state explicitly when the diff or updated-anchor list was
truncated.

## Kernel-owned metadata

Resource metadata is derived and served by the kernel. Providers advertise
facts at route registration; they do not implement `?meta` reads and no model
tool description hard-codes loaded resource kinds.

Extend route declarations with signal definitions:

```text
signal-definition {
  name: string
  description: string
  payload-schema: JSON Schema string
}
```

For a requested target URI, the router evaluates its normal specificity and
load-order rules independently for every operation. The kernel synthesizes:

```json
{
  "uri": "canonical target URI",
  "operations": ["read", "children", "write", "edit", "move", "run", "signal", "poll"],
  "signals": [
    {
      "name": "provider-defined name",
      "description": "provider-defined description",
      "payload_schema": {}
    }
  ]
}
```

Only selected, currently routable operations appear. Ordering is the enum's
canonical order; signal definitions are ordered by name. JSON is canonical and
stable. Plugin identity, load order, native implementation details, and
unsupported operations are not exposed.

The router recognizes the reserved `meta` projection before ordinary provider
selection, reconstructs the target URI from the remaining projection segments,
and returns the synthesized JSON as a readable text resource. `metadata(uri)`
returns the same typed data directly to plugins. The ordinary `read` tool can
therefore read metadata without an `inspect` tool.

## Component layout

Add these narrow model-tool components:

```text
plugins/tool-edit       -> artist.tool.edit
plugins/tool-run        -> artist.tool.run
plugins/tool-signal     -> artist.tool.signal
```

Each exports exactly one definition and routes exactly one request through
focused host-resource imports. Descriptions define only the generic verb
semantics. They must not enumerate or mention concrete resource schemes,
providers, interpreters, processes, or future plugins.

Add the narrow filesystem edit component:

```text
plugins/file-edit       -> artist.file.edit
```

It advertises only `resources`, registers only `edit` on `file:///**`, and
delegates compare-and-apply mechanics through a focused native-filesystem host
import. No run or signal resource provider is added.

Update the plugin SDK only for shared WIT boilerplate, typed schema generation,
and resource-reply rendering. It remains an `rlib`, never a loadable component.

## Native filesystem edit mechanics

Add one native filesystem import accepting the resolved edit request. Its host
implementation must:

- resolve the canonical `file://` URI;
- serialize Artist edits to the same path with a shared per-path lock;
- read the current bytes while holding that lock;
- reject non-UTF-8 input;
- compare SHA-256 with `expected_sha256`;
- validate every byte boundary and non-overlap invariant defensively;
- apply replacements from the greatest byte offset to the least;
- write a temporary sibling, flush it, and atomically rename it over the target;
- preserve the target's existing permissions; and
- return the committed SHA-256 revision.

External programs do not honor Artist's in-process lock. The final revision
comparison immediately before replacement detects observed interference, but
the filesystem contract must not claim a cross-process compare-and-swap that
the operating system does not provide.

## Code map

Create or modify the following areas:

- `wit/plugin.wit`: run, signal, anchored edit, resolved edit, metadata, typed
  replies/errors, route signal declarations, and focused host imports.
- `crates/artist-resource/src/resource.rs`: matching core types.
- `crates/artist-resource/src/anchor.rs`: TECA line identities, neighborhood
  construction, rendering/parsing, operation resolution, and edit validation.
- `crates/artist-resource/src/router.rs`: operation introspection and reserved
  metadata projection synthesis.
- `crates/artist-resource/src/filesystem.rs`: resolved atomic edit mechanics.
- `crates/artist-resource/src/lib.rs`: export the new focused modules/types.
- `crates/artist-plugin/src/lib.rs`: WIT/core conversions and implementations
  of `read-anchored`, `edit-anchored`, and `metadata` host imports.
- `plugins/sdk`: updated narrow-component macros and reply helpers.
- `plugins/tool-read`: switch model reads to `read-anchored`.
- `plugins/tool-edit`, `plugins/tool-run`, `plugins/tool-signal`: one universal
  model tool each.
- `plugins/file-edit`: one filesystem resource operation.
- `plugins/Cargo.toml`, `plugins/Cargo.lock`, `Makefile`, `README.md`, `arch.md`,
  and `URI_RESOURCE_FABRIC_PLAN.md`: component inventory and final contracts.

Do not add a general execution service, process registry, session registry,
executor lifecycle state, or provider-specific metadata to the kernel.

## Horizontal implementation order

1. Define every Rust and WIT data contract, conversion, and error before
   implementing behavior. Add round-trip tests for every variant.
2. Implement TECA addressing and resolved edit planning as a pure resource
   module with no filesystem, Wasmtime, or tool dependencies.
3. Implement router metadata synthesis entirely from registered routes and
   verify it across overlapping operation-specific providers.
4. Implement filesystem compare-and-apply behind the generic resolved-edit
   provider contract.
5. Wire focused host imports and add the four narrow production components.
6. Update the production Wasmtime smoke to load every component, assert one
   advertised capability and one definition/operation per component, and
   exercise edit plus unsupported run/signal routing.
7. Update architecture documents only after code and tests agree on the final
   names and schemas.

No step introduces a temporary alternate contract, compatibility shim, bundled
component, fixture component, noun-specific tool description, or host-native
model tool.

## Verification contract

### TECA and read

- Same bytes always produce the same full neighborhood and rendered anchors.
- Construction is independent of insertion order.
- Full and partial reads render identical anchors for shared lines.
- Unique, duplicate, blank, Unicode, LF, CRLF, and unterminated final lines are
  covered.
- Malformed, unknown, stale, and ambiguous anchors fail distinctly.
- A colliding insertion lengthens addresses safely; removal permits longer old
  addresses to continue resolving where TECA guarantees it.

### Edit planning and application

- Replace/delete ranges and before/after insertion work individually and in
  compatible batches.
- Every operation resolves against the original snapshot.
- Overlap, reversed ranges, duplicate incompatible targets, invalid UTF-8
  boundaries, empty batches, revision mismatch, and unsupported providers fail
  without writing.
- Batches apply atomically in reverse byte order.
- CRLF, missing final newline, permissions, and unrelated bytes are preserved.
- The reply contains a correct diff and usable anchors from the committed view.
- Concurrent Artist edits to one URI serialize; the stale request receives a
  typed conflict.

### Metadata

- `?meta` describes the base resource.
- `?meta/<projection>` describes the corresponding projected resource.
- Provider attempts to register `meta` as their first projection segment fail.
- Metadata respects specificity, operation-level routing, and load order.
- Metadata never exposes unselected providers or internal plugin details.
- Signal definitions round-trip their JSON schemas and render canonically.

### Run and signal

- Tool schemas are closed and contain no provider-specific nouns.
- With no provider loaded, both return the existing typed unsupported/not-found
  routing result.
- Mock providers are ordinary in-process unit-test `ResourceProvider`
  implementations inside `artist-resource`; no test plugin is created.
- Run returns the provider-created canonical URI unchanged.
- Signal forwards its name and payload unchanged and returns typed
  acknowledgement.
- Agent abort tests prove no implicit signal request is emitted.

### Repository gates

- Host and plugin formatting checks pass.
- Host and plugin clippy pass with warnings denied.
- Host and plugin tests pass.
- Every production component builds for `wasm32-wasip2`.
- The Wasmtime production smoke exercises all advertised sockets.
- Source audit finds no bundled tools, multi-operation resource components,
  fixture plugins, conventional query parsing, or executor/session code.

## References

- Existing line-anchor implementation inspected on `main`:
  `crates/artist-tools/src/{read,edit}.rs` and
  `crates/hashline-tools/src/{file_tools,coordinator,mnemonic_anchors}.rs`.
- TECA 1.0.0 API and semantics: <https://docs.rs/teca/1.0.0/teca/>
- TECA crate metadata: <https://crates.io/crates/teca/1.0.0>
