# Artist Resource Extensibility + WASM Runtime Refactor Goal

Status: **authoritative implementation goal**  
Target branch: `crortnite`  
Inspected baseline while authoring: `f902212a35f799d86825517e7b80d91411fd78f7`  
Scope: resource extensibility, shared WIT migration, runtime cleanup, package/hot-reload unification, AST proof migration  
Explicit non-goal: **do not implement Brush, `bash://`, PTYs, terminal emulation, or shell completion semantics in this goal**

This document supersedes the previous `artist-resources-extension-contract-v1.md` where they conflict.

The point of this goal is not merely to add `resources://`. It is to leave Artist with one coherent typed WASM architecture for:

- model/programmatic verbs under `tools://`;
- resource extensions under `resources://`;
- typed resource-to-resource composition;
- typed custom component composition;
- query-aware Artist URIs;
- real async/cancellation support;
- safe host filesystem primitives;
- shared package discovery/build/activation;
- external file-change hot reload;
- compact resource documentation seeding;
- failure-isolated self-modification.

Do not improvise a second plugin system while doing this.

---

# 0. Architectural invariants that remain locked

The universal model/programmatic verb surface remains exactly:

```text
read
write
edit
poll
send
run
abort
delete
find
grep
```

Do not add resource-specific model verbs.

Everything remains addressable through Artist resource URIs.

Tool source and resource-extension source are trusted under the current threat model.

WASM Components and typed WIT are the semantic boundary.

Do not introduce generic JSON dispatch such as:

```text
invoke("read", arbitrary_json)
handle(uri, verb, json)
```

Provider/model JSON adapters may exist **above** typed WIT boundaries. They must not become the internal component-to-component API.

Durable state belongs in resources/backing capabilities, not opaque long-lived WASM instance memory.

Compiled components may be cached. In-flight invocations pin active generations. Candidate compilation/validation must occur before atomic activation.

The canonical public error algebra remains:

```text
InvalidUri
InvalidInput
InvalidPattern
NotFound
WrongKind
InvalidAnchor
StaleAnchor
Immutable
Unsupported
PermissionDenied
Conflict
NotEmpty
Aborted
Internal
```

Do not add:

```text
NotLive
AlreadyLive
InvalidState
UnknownResourceKind
```

The existing anchor, read/write/edit/find/grep/poll/run/send semantics remain authoritative unless this document explicitly changes URI parsing/lowering.

---

# 1. The central resource abstraction

`resources://` is **not** a registry of URI schemes.

It is a registry of **resource extensions**.

A resource extension can:

1. introduce a namespace:
   ```text
   bash://...
   issue://...
   pr://...
   ```

2. add addressable child resources inside an existing namespace:
   ```text
   file:///src/main.rs/symbols/
   file:///src/main.rs/symbols/Foo
   file:///src/main.rs/symbols/Foo/callers
   ```

3. implement a universal verb for resources it recognizes;

4. reserve a synthetic resource subtree so unsupported operations do not accidentally fall through to an unrelated provider;

5. call other Artist resources through the same typed universal resource interfaces;

6. import explicitly granted primitive or custom WIT capabilities;

7. export custom typed WIT interfaces for other components;

8. publish compact machine-readable docs plus full Markdown prose.

Many resource extensions may coexist on the same URI scheme.

**Never implement one-provider-per-scheme ownership.**

The routing unit is:

```text
(verb, canonical resource URI without fragment)
    -> claim arbitration
    -> zero or one handling resource extension
```

A namespace provider is only one special case of a resource extension.

---

# 2. Do the known-art cleanup first

Before adding the new resource router, remove commodity machinery we should not own.

This work is part of the goal, not optional cleanup.

## 2.1 `cargo_metadata` + `escargot`

Add both:

```text
cargo_metadata
escargot
```

to `artist-component`.

They have separate jobs.

### `cargo_metadata` owns Cargo project metadata

Replace the hand-written:

```rust
CargoMetadata
CargoPackage
CargoTarget
cargo_metadata(...)
```

with:

```rust
cargo_metadata::MetadataCommand
cargo_metadata::{Metadata, Package, Target}
```

Use it to determine:

- the package corresponding to the authored manifest;
- package version;
- target name;
- target kind / crate type;
- other stable Cargo metadata needed for fingerprint/provenance.

### `escargot` owns Cargo build command construction and build-message capture

Replace the hand-written:

```rust
std::process::Command::new("cargo")
CargoBuildMessage
CargoMessageTarget
cargo_artifact(...)
manual stdout JSON line parsing
```

with:

```rust
escargot::CargoBuild
escargot::CommandMessages
escargot::format::Message::CompilerArtifact
```

Use `CargoBuild::exec()`, not `CargoBuild::run()`. Artist is building WASM component artifacts, including `cdylib` targets; it is not asking Escargot to execute the resulting artifact.

Configure Escargot with:

```text
manifest path
target wasm32-wasip2
RUSTFLAGS
target directory where Artist already requires one
```

and use `.arg(...)` / `.args(...)` for Cargo options Escargot does not model directly:

```text
--offline
--profile product
```

Use Escargot's compiler-artifact messages to select the `.wasm` emitted for the target identified through `cargo_metadata`.

Do not maintain a second Cargo JSON schema beside these crates.

Escargot is adopted because it is useful here: it removes Cargo subprocess construction/message plumbing without changing Artist's architecture. Its plugin/runtime concepts are irrelevant; this use is only its Cargo build API.

Preserve:

- source package build;
- precompiled `*.wasm` support;
- offline build semantics;
- target/profile selection;
- provenance hashing;
- cached artifact validation;
- failed-build-does-not-replace-active behavior.

Delete the duplicate Cargo protocol structs/parsers after migration.

## 2.2 `gray_matter`

Add:

```text
gray_matter
```

with YAML support.

Use it for both:

```text
tool.md
resource.md
```

frontmatter parsing.

Delete the custom `split_frontmatter` implementation.

Frontmatter parsing must return:

```rust
struct ParsedManifest<T> {
    frontmatter: T,
    prose: String,
}
```

or equivalent shared representation.

The Markdown body must remain exact enough for model-facing documentation. Do not trim internal prose structure aggressively. Trimming outer leading/trailing whitespace is fine.

After migration, remove `serde_yaml` from `artist-component` if it has no other users.

## 2.3 `url`

Artist already uses `url::Url`. Keep it as the URI parser.

Remove the current prohibition on query components.

Do not write a replacement URL parser.

Use `Url` for:

- scheme;
- authority;
- path;
- query storage;
- fragment storage;
- file URI conversion.

Artist adds validation/lowering rules on top.

Query parsing details are specified later in this document.

## 2.4 Real cancellation: `tokio_util::sync::CancellationToken`

Artist already uses `CancellationToken` in `artist-agent`.

Use it end-to-end internally.

Do **not** put a Rust `CancellationToken` into WIT or JSON.

Keep `InvocationContext` as serializable metadata for compatibility/logging:

```rust
pub struct InvocationContext {
    pub working_uri: Option<ResourceUri>,
    pub environment: Vec<EnvironmentEntry>,
    pub cancellation_token: Option<String>, // opaque diagnostic/correlation ID only
    pub deadline_ms: Option<u64>,
    pub correlation_id: Option<String>,
}
```

The string field is no longer the actual cancellation mechanism.

Add an internal non-serializable execution scope:

```rust
#[derive(Clone)]
pub struct InvocationScope {
    pub context: InvocationContext,
    pub cancellation: tokio_util::sync::CancellationToken,
}
```

or equivalent.

Rules:

- top-level agent execution seeds `InvocationScope.cancellation` from the agent/session cancellation token;
- nested resource/component calls inherit a `child_token()`;
- cancelling a parent cancels all descendants;
- cancelling/expiring a nested child must not cancel its parent;
- `InvocationContext` remains out-of-band from model verb request shapes;
- no cwd/env/cancellation fields are added to `ReadRequest`, `SendRequest`, etc.

Keep compatibility helpers such as `execute_operation_with_context` if useful, but internally route through a scope.

## 2.5 Wasmtime async execution

`artist-component` already enables Wasmtime async/component-async Cargo features. Actually use them.

Keep the current Wasmtime major during this goal:

```text
wasmtime 47.x
wasmtime-wasi 47.x
```

Do not combine this architectural migration with an unnecessary Wasmtime major-version upgrade.

Configure the shared engine with async support and interruption:

```rust
let mut config = wasmtime::Config::new();
config.wasm_component_model(true);
config.async_support(true);
config.wasm_component_model_async(true);
config.epoch_interruption(true);
config.consume_fuel(true);
```

Use the exact APIs available in the pinned 47.x version.

Replace synchronous component invocation:

```rust
instantiate(...)
func.call(...)
typed.call(...)
```

with async equivalents wherever the store is async:

```rust
instantiate_async(...)
func.call_async(...)
typed.call_async(...)
```

Use:

```rust
wasmtime_wasi::p2::add_to_linker_async
```

instead of `add_to_linker_sync` for these async stores.

Nested universal resource imports must be implemented as async Rust host functions while remaining ordinary blocking WIT calls from the guest's point of view.

Do not use `std::thread::spawn`, private Tokio runtimes, or `spawn_blocking` as the normal bridge for component -> kernel nested calls after this migration.

## 2.6 Wasmtime execution limits

Use Wasmtime's native store limit/fuel/epoch mechanisms. Do not author a second limiter subsystem.

Add a shared configuration:

```rust
pub struct WasmExecutionLimits {
    pub max_memory_bytes: usize,
    pub max_table_elements: usize,
    pub max_instances: usize,
    pub max_memories: usize,
    pub max_tables: usize,
}
```

Use `StoreLimits` / `StoreLimitsBuilder` and attach it through the store's `ResourceLimiter` path.

Use conservative defaults suitable for trusted-but-buggy extension code. Suggested v1 defaults:

```text
max memory per invocation store: 512 MiB
max table elements:             100_000
max component instances:        128
max memories:                    32
max tables:                      32
```

These numbers are implementation defaults, not public Artist ABI. Centralize them so they can be tuned later.

Enable epoch interruption for all component stores.

Run one lightweight engine-level epoch ticker owned by the component runtime. Do not spawn one ticker per invocation.

Cancellation/deadline behavior:

- `InvocationScope.cancellation` must be able to stop a running WASM call;
- `deadline_ms` must be enforced;
- cancellation/deadline must terminate or interrupt CPU-bound WASM, not only host I/O;
- use epoch interruption and async Wasmtime so dropping/timeout of the future cannot leave a CPU-bound guest monopolizing a worker;
- map user cancellation to canonical `Aborted`;
- map configured execution/resource limit failures to `Internal` unless a more specific canonical error already exists.

Fuel:

- general operations may use fuel for a hard execution ceiling if useful;
- claim execution **must** use a small dedicated fuel budget as specified below.

## 2.7 `cap-std`

Add `cap-std`.

Refactor the trusted filesystem substrate away from ambient `std::fs` root-confinement as the long-term primitive.

Open the project filesystem root once as a capability:

```rust
cap_std::fs::Dir
```

All filesystem operations beneath that root should be relative to this directory capability.

Keep absolute `file:///...` Artist URIs at the semantic layer. Convert them to paths relative to the project root before calling `Dir`.

Requirements:

- no absolute ambient path operations after relative conversion;
- symlink/path traversal must not escape the granted root;
- preserve existing file read/write/edit/delete/find/grep behavior;
- preserve atomic replacement semantics;
- preserve directory `/` semantics;
- preserve current public errors.

Do not expose `cap-std` itself to the model.

This becomes the trusted filesystem primitive that a future WASM filesystem resource component can use.

## 2.8 `notify` + `notify-debouncer-mini`

Add:

```text
notify
notify-debouncer-mini
```

Use them for external edits to both:

```text
tools/
resources/
```

Artist's own `write`/`edit` still marks packages dirty immediately.

The filesystem watcher exists so edits from an external editor, git checkout, formatter, code generator, etc. also enter the exact same package candidate/reload path.

Use one shared watcher service, not a watcher per package.

Debounce per path/package.

Suggested v1 debounce:

```text
200ms
```

Watch recursively, but ignore generated/noisy paths:

```text
target/
.git/
*.wasm.artist.json
temporary editor swap files
```

Do not ignore:

```text
tool.md
resource.md
tool.wit
resource.wit
Cargo.toml
Cargo.lock
src/**
source/**
tool.wasm
resource.wasm
```

A watcher event must only mark/reload the affected package. It must not poison the whole catalog.

Duplicate events from Artist's own writes are harmless: fingerprint equality should make redundant candidate activation a no-op.


## 2.9 Do **not** adopt `plugy`

Do not add the `plugy`, `plugy-runtime`, `plugy-core`, or `plugy-macros` crates.

This is not a general rejection of prior art. Plugy is actively the wrong abstraction for Artist's core extension runtime.

Plugy's published contract is based on:

```text
Rust traits + proc macros
wasm32-unknown-unknown core-WASM plugins
manual guest alloc/dealloc/message exchange
Serde/Bincode-style value transport
its own Runtime / Plugin / PluginHandle abstraction
```

Artist's locked contract is:

```text
WebAssembly Components
WIT-defined interfaces
Canonical ABI typed values
wasm32-wasip2 where applicable
Wasmtime component::Linker / Component / Instance
typed component-to-component imports/exports
Artist-owned generation/routing/capability semantics
```

Using Plugy would therefore require one of two bad outcomes:

1. put a second core-WASM/Serde plugin ABI beside the Component Model; or
2. replace/rewrite Plugy's central ABI/runtime machinery until little meaningful Plugy remains.

Both are actively harmful.

In particular, do not replace:

```text
artist:resource WIT
artist:tool WIT
Wasmtime Component Model linking
typed custom dependency resolution
resource claim routing
active generation pinning
```

with Plugy traits/macros/runtime handles.

Plugy's ergonomic lesson is still useful: Artist may later provide thin Rust proc-macro/SDK sugar over its **own WIT-generated interfaces**. If such sugar is authored, it must compile down to the authoritative Artist Component/WIT contracts and must not introduce Plugy's transport/runtime ABI.

---

# 3. Shared package/runtime abstraction before `resources://`

Do not clone `ToolPackage` and `ComponentRegistry` into parallel resource-specific implementations.

Generalize first.

Target concepts:

```rust
enum PackageKind {
    Tool,
    Resource,
}

struct ComponentPackage<M> {
    kind: PackageKind,
    root: PathBuf,
    manifest: M,
    prose: String,
    wasm: Option<PathBuf>,
    source: Option<PathBuf>,
    build_manifest: Option<PathBuf>,
    wit: Option<PathBuf>,
}

struct ActiveGeneration<M> {
    package_name: String,
    generation: u64,
    fingerprint: String,
    provenance: PathBuf,
    artifact: Arc<Vec<u8>>,
    manifest: M,
    prose: String,
    // validated component/runtime data
}
```

Exact generic factoring is an implementation choice. The semantics are not.

Shared machinery must own:

- frontmatter parsing;
- source/precompiled discovery;
- Cargo metadata;
- Cargo build;
- artifact discovery;
- fingerprinting;
- provenance;
- WIT parsing;
- component loading;
- dependency resolution;
- capability validation;
- candidate validation;
- generation numbering;
- atomic active swap;
- in-flight generation pinning;
- dirty tracking;
- watcher integration;
- malformed-package isolation.

Tool-specific and resource-specific logic should be small adapters over this machinery.

## 3.1 Activation failure isolation

Generalize the fixed tool isolation semantics directly:

### malformed inactive package

If a never-active package is malformed:

```text
skip it from active routing/catalogs
record/report its diagnostic
do not affect unrelated packages
```

### malformed edit to active package

If generation N is active and candidate N+1 is malformed:

```text
generation N remains fully active
```

This includes:

- code;
- docs;
- routes;
- dependency graph;
- capabilities;
- provider/model registration.

### atomic generation swap

For a successful candidate, all of the following swap together:

```text
artifact
manifest
prose/docs
routes
exports
dependencies
capabilities
precomputed export indices
```

Never publish new docs with old code or new routes with old dependencies.

---

# 4. Authoritative shared resource WIT

The following package identity is authoritative:

```wit
package artist:resource@1.0.0;
```

Do not use `artist:resource@0.x`.

Do not use `artist:tool` as the canonical home for resource operation types after this migration.

Create a shared WIT package under an appropriate stable project path, for example:

```text
crates/artist-component/wit/resource-surface/
```

The exact filesystem path can differ, but all components must consume the same package.

## 4.1 Exact `artist:resource@1.0.0` WIT

Use this semantic WIT exactly. Mechanical syntax adjustments needed by the pinned WIT parser are allowed only if they preserve these package/interface/type identities and signatures.

```wit
package artist:resource@1.0.0;

interface types {
    type uri = string;
    type anchor = string;
    type pattern = string;

    enum line-ending {
        none,
        lf,
        crlf,
        cr,
    }

    record anchored-line {
        anchor: anchor,
        text: string,
        ending: line-ending,
    }

    record anchored-text {
        uri: uri,
        lines: list<anchored-line>,
    }

    variant position {
        top,
        bottom,
        at(anchor),
    }

    enum error-code {
        invalid-uri,
        invalid-input,
        invalid-pattern,
        not-found,
        wrong-kind,
        invalid-anchor,
        stale-anchor,
        immutable,
        unsupported,
        permission-denied,
        conflict,
        not-empty,
        aborted,
        internal,
    }

    record error {
        code: error-code,
        uri: option<uri>,
        message: string,
    }

    record diff-hunk {
        old: list<anchored-line>,
        new: list<anchored-line>,
    }

    record anchored-diff {
        uri: uri,
        hunks: list<diff-hunk>,
    }

    record directory-result {
        uri: uri,
        entries: list<uri>,
    }

    variant read-result {
        text(anchored-text),
        directory(directory-result),
    }

    record read-request {
        uri: uri,
        at: option<position>,
        before: option<u32>,
        after: option<u32>,
    }

    record write-request {
        uri: uri,
        content: string,
    }

    record write-result {
        text: anchored-text,
    }

    variant insertion-point {
        top,
        bottom,
        before(anchor),
        after(anchor),
    }

    record replace-operation {
        start: anchor,
        end: option<anchor>,
        content: string,
    }

    record insert-operation {
        at: insertion-point,
        content: string,
    }

    variant edit-operation {
        replace(replace-operation),
        insert(insert-operation),
    }

    record edit-request {
        uri: uri,
        operations: list<edit-operation>,
    }

    record edit-result {
        text: anchored-text,
        diff: anchored-diff,
    }

    record find-request {
        roots: list<uri>,
        query: pattern,
    }

    variant grep-source {
        resources(list<uri>),
        text(list<anchored-text>),
    }

    record grep-request {
        pattern: pattern,
        source: grep-source,
    }

    record run-request {
        uri: uri,
        args: list<string>,
    }

    record send-request {
        uri: uri,
        content: string,
    }

    record poll-target {
        uri: uri,
        from-position: option<position>,
    }

    record regex-atom {
        target: u32,
        pattern: string,
    }

    variant poll-atom {
        changed(u32),
        regex(regex-atom),
        terminated(u32),
        timeout(u64),
    }

    variant poll-node {
        atom(poll-atom),
        all(list<u32>),
        any(list<u32>),
    }

    record poll-condition-wire {
        root: u32,
        nodes: list<poll-node>,
    }

    record poll-request {
        targets: list<poll-target>,
        until: option<poll-condition-wire>,
    }

    record poll-result {
        text: list<anchored-text>,
        satisfied: list<poll-atom>,
    }

    enum verb {
        read,
        write,
        edit,
        poll,
        send,
        run,
        abort,
        delete,
        find,
        grep,
    }

    enum claim-decision {
        pass,
        handle,
        reserve,
    }

    record claim-request {
        verb: verb,
        uri: uri,
    }
}

interface extension {
    use types.{claim-request, claim-decision};

    claim: func(request: claim-request) -> claim-decision;
}

interface read {
    use types.{read-request, read-result, error};

    read: func(requests: list<read-request>)
        -> list<result<read-result, error>>;
}

interface write {
    use types.{write-request, write-result, error};

    write: func(requests: list<write-request>)
        -> list<result<write-result, error>>;
}

interface edit {
    use types.{edit-request, edit-result, error};

    edit: func(requests: list<edit-request>)
        -> list<result<edit-result, error>>;
}

interface find {
    use types.{find-request, uri, error};

    find: func(request: find-request)
        -> result<list<uri>, error>;
}

interface grep {
    use types.{grep-request, anchored-text, error};

    grep: func(request: grep-request)
        -> result<list<anchored-text>, error>;
}

interface run {
    use types.{run-request, uri, error};

    run: func(requests: list<run-request>)
        -> list<result<uri, error>>;
}

interface send {
    use types.{send-request, uri, error};

    send: func(requests: list<send-request>)
        -> list<result<uri, error>>;
}

interface abort {
    use types.{uri, error};

    abort: func(uris: list<uri>)
        -> list<result<uri, error>>;
}

interface delete {
    use types.{uri, error};

    delete: func(uris: list<uri>)
        -> list<result<uri, error>>;
}

interface poll {
    use types.{poll-request, poll-result, error};

    poll: func(request: poll-request)
        -> result<poll-result, error>;
}

/*
 * Host-only binding helper.
 *
 * This world is not a resource-extension component contract.
 * It exists so the Rust host can generate canonical resource types and
 * async host traits for nested resource calls.
 */
world host-world {
    import read;
    import write;
    import edit;
    import find;
    import grep;
    import run;
    import send;
    import abort;
    import delete;
    import poll;
}
```

Important:

- `read-stream`, `grep-stream`, and `poll-stream` are **not** part of the underlying resource interfaces.
- Streaming remains a model/tool adapter concern for now.
- There is one canonical `types` interface.
- Do not duplicate resource types in `artist:tool`.
- `claim` deliberately does **not** return `result<...>`.
- Expected routing decisions are only `pass`, `handle`, `reserve`.
- A claim trap/runtime failure is an operation-level provider failure and maps to canonical `Internal`.

---

# 5. Authoritative `artist:tool` migration

This is a real WIT package migration. Do it deliberately.

The new tool WIT package identity is:

```wit
package artist:tool@1.0.0;
```

The existing Artist package frontmatter contract IDs remain:

```text
artist:tool:read@1
artist:tool:write@1
...
```

Those frontmatter contract IDs are Artist package identities. They are not the same thing as the WIT package semver.

Do not change model-facing tool names.

## 5.1 Tool interfaces reuse resource types

Each `artist:tool` verb interface imports its types from:

```text
artist:resource/types@1.0.0
```

Example:

```wit
package artist:tool@1.0.0;

interface read {
    use artist:resource/types@1.0.0.{
        read-request,
        read-result,
        error
    };

    read: func(requests: list<read-request>)
        -> list<result<read-result, error>>;

    read-stream: func(request: read-request)
        -> result<stream<read-result>, error>;
}
```

Do the equivalent for all ten tool interfaces.

The existing streaming exports may remain on the tool interfaces.

## 5.2 Tool worlds import resource interfaces directly

Delete the duplicated `artist:tool/host-read`, `host-write`, etc. semantic interfaces.

A tool world imports the corresponding resource interface.

Example:

```wit
world read-world {
    import resource-read: artist:resource/read@1.0.0;
    export read;
}
```

Equivalent:

```text
write-world  -> import artist:resource/write
edit-world   -> import artist:resource/edit
find-world   -> import artist:resource/find
grep-world   -> import artist:resource/grep
run-world    -> import artist:resource/run
send-world   -> import artist:resource/send
abort-world  -> import artist:resource/abort
delete-world -> import artist:resource/delete
poll-world   -> import artist:resource/poll
```

The universal tool guest code becomes a thin typed adapter:

```text
tool export -> imported artist:resource verb
```

No JSON conversion.

Update seeded tool WIT/source accordingly.

Because project-local universal tool packages are durable editable state, do not blindly overwrite arbitrary customized tool source.

Migration/repair may update:

- missing seeded files;
- files matching known untouched old seed content;
- clean projects created during tests.

Do not overwrite user-modified tool packages merely because the framework WIT version changed. If a customized old tool can no longer activate, surface a clear compatibility/build diagnostic and leave any previous active generation pinned until the package is migrated.

---

# 6. Resource package contract

Each resource package lives at:

```text
resources://<package>/
```

Backed by a project directory:

```text
resources/<package>/
```

Canonical authored layout:

```text
resources/<package>/
├── resource.md
├── Cargo.toml
├── Cargo.lock
├── resource.wit
├── src/
└── resource.wasm
```

A package may use source or precompiled WASM according to the same rules as tools.

`resource.md` is mandatory for discoverable resource packages.

## 6.1 Resource frontmatter

Authoritative v1 shape:

```yaml
---
name: artist-ast
description: Structured source-code resource projections
version: 0.1.0
contract: artist:resource:extension@1

routes:
  - schemes: [file]

exports:
  - read
  - find

capabilities:
  - resource.read

docs:
  - uri: "file://<path>/symbols/"
    summary: "Lists structured symbols defined by a source file."
    verbs: [read, find]

  - uri: "file://<path>/symbols/<symbol>"
    summary: "Structured information for one symbol."
    verbs: [read]

  - uri: "file://<path>/symbols/<symbol>/callers"
    summary: "Callers of the selected symbol."
    verbs: [read]
    query:
      - name: limit
        summary: "Maximum number of returned callers."
---

Full Markdown documentation follows.
```

Typed Rust frontmatter should be equivalent to:

```rust
struct ResourceFrontmatter {
    name: String,
    description: String,
    version: String,
    contract: String,
    routes: Vec<ResourceRoute>,
    exports: Vec<Verb>,
    capabilities: Vec<String>,
    docs: Vec<ResourceDoc>,
}

struct ResourceRoute {
    schemes: Vec<String>,
}

struct ResourceDoc {
    uri: String,
    summary: String,
    verbs: Vec<Verb>,
    query: Vec<QueryDoc>,
}

struct QueryDoc {
    name: String,
    summary: String,
}
```

Allow optional example fields later only if they do not alter routing.

## 6.2 `routes` are candidate indexing only

`routes` lists URI schemes for which the package's `claim` function may return `handle` or `reserve`.

Do not invent path-glob dispatch in v1.

Do not route from documentation patterns.

Do not use registration order.

A route may produce false-positive candidates.

A route must never omit a scheme for which the component intends to claim.

## 6.3 `exports` is authoritative standard-interface metadata

`exports` declares which universal `artist:resource/<verb>@1.0.0` interfaces the component physically exports.

Activation validation must prove:

```text
actual standard resource verb exports == frontmatter exports
```

Do not silently accept undeclared standard verb exports.

Custom non-universal WIT exports are allowed and are not listed in `exports`.

Every active resource package must physically export:

```text
artist:resource/extension@1.0.0
```

That export is mandatory and is not listed in `exports`.

A component may return `reserve` for a verb it does not export.

A component must never return `handle` for a verb absent from its `exports`; if it does, treat that as component contract violation -> canonical `Internal`.

## 6.4 Package-local `resource.wit`

A source resource package defines its exact concrete world.

Example AST component:

```wit
package artist-extension:ast@1.0.0;

world ast {
    import source-read: artist:resource/read@1.0.0;

    export artist:resource/extension@1.0.0;
    export artist:resource/read@1.0.0;
    export artist:resource/find@1.0.0;
}
```

A component that needs more nested resource operations imports them explicitly:

```wit
import nested-read: artist:resource/read@1.0.0;
import nested-write: artist:resource/write@1.0.0;
```

A component may also import/export arbitrary additional typed WIT contracts.

No generic JSON dependency API.

## 6.5 Capability validation

For each imported standard nested resource interface:

```text
artist:resource/read@1.0.0  -> requires capability resource.read
artist:resource/write@1.0.0 -> requires capability resource.write
...
```

If the component imports one but frontmatter does not declare the corresponding capability, candidate activation fails.

Declared resource capabilities that are not imported are allowed but should produce a diagnostic warning; they are not fatal.

Primitive host imports require their own explicit capabilities.

Custom component imports are resolved through the component dependency resolver.

If more than one active custom component export satisfies the same imported custom interface and no explicit disambiguation exists, candidate activation fails. Do not choose by registration order.

---

# 7. Host invocation of optional resource exports

Do not solve optional resource exports by serializing to JSON.

Use Wasmtime's typed component API.

Generate canonical Rust WIT types from `artist:resource/host-world`.

Instantiate the resource component with the dynamic linker that satisfies:

- nested standard resource imports;
- WASI imports;
- granted primitive imports;
- custom typed component dependencies.

Precompute export indices from the component at activation time.

For each declared standard export:

1. locate the nested interface export:
   ```text
   artist:resource/read@1.0.0
   ```

2. locate its nested function:
   ```text
   read
   ```

3. use `Instance::get_typed_func` / `Func::typed` with generated WIT types;

4. invoke with `TypedFunc::call_async`.

Do the same for:

```text
artist:resource/extension@1.0.0 / claim
```

No serde_json intermediary is permitted on these universal paths.

The existing dynamic JSON adapter may remain only for explicitly legacy/dynamic external adapter paths that are already quarantined. Do not route resource universal operations through it.

Precomputed export indices belong to the active generation and swap atomically with it.

---

# 8. Canonical Artist URI model

Artist URI syntax is:

```text
scheme://authority/path?query#anchor
```

Ordered semantic layers:

```text
path     = addressable resource hierarchy
query    = extension-defined modifiers/selectors/filters
fragment = textual anchor in the resolved view
```

## 8.1 Path is hierarchy

If the caller can descend into children, it belongs in the path.

Correct:

```text
file:///src/main.rs/symbols/
file:///src/main.rs/symbols/Foo
file:///src/main.rs/symbols/Foo/callers
```

Do not encode hierarchical AST resources as:

```text
file:///src/main.rs?symbols/Foo/callers
```

## 8.2 Query is non-hierarchical

Examples:

```text
repo://artist/issues/?state=open
repo://artist/issues/?author=adam&limit=20
file:///src/main.rs/symbols/?kind=function
pr://123/diff/4?context=10
file:///src/main.rs?stat
```

Authoritative grammar:

```text
query := item ("&" item)*
item  := key | key "=" value

key first char := ASCII letter or "_"
remaining key chars := ASCII letter, digit, "_", ".", "-"
```

Keys are case-sensitive.

These are distinct:

```text
?flag
?flag=
```

Empty values are valid.

Repeated keys are valid:

```text
?label=bug&label=urgent
```

Use standard URL percent encoding/decoding.

## 8.3 Query identity is ordered

The kernel does **not** normalize query semantics.

These are distinct canonical resource views:

```text
?state=open&label=bug
?label=bug&state=open
```

Likewise repeated-key ordering is preserved.

A resource extension may choose to interpret two views as semantically equivalent internally, but the kernel must not do so.

Routing-stack/cycle identity uses the canonical URI with query order preserved.

## 8.4 Use `url::Url`, preserve raw query

`ResourceUri` remains backed by `url::Url`.

Expose helpers equivalent to:

```rust
fn query(&self) -> Option<&str>;
fn query_items(&self) -> Result<Vec<QueryItem>, KernelError>;
fn without_fragment(&self) -> ResourceUri;
```

where:

```rust
struct QueryItem {
    key: String,
    value: Option<String>,
}
```

Preserve the raw query in the `Url` for identity/serialization.

For parsed `QueryItem` values:

- split raw query on `&`;
- detect presence/absence of `=`;
- use `url`'s form/percent decoding rather than authoring percent-decoding code;
- preserve item order;
- preserve repeated keys;
- preserve `None` vs `Some("")`.

Do not rebuild an already parsed URI from `query_pairs()` in a way that converts bare keys to `key=`.

## 8.5 Fragment ordering

The fragment is always last.

Valid:

```text
file:///a?state=open#BlueHorse
```

Invalid:

```text
file:///a#BlueHorse?state=open
```

Because `url::Url` would otherwise treat `?state=open` as part of the fragment, Artist validation must reject a fragment containing `?`.

The fragment is parsed as an Artist anchor.

Do not percent-decode arbitrary anchor syntax into a second grammar. Existing Artist anchor validation remains authoritative.

---

# 9. Exact fragment lowering rules

The fragment is not passed to resource `claim`.

Routing always receives:

```text
canonical path + query
```

with fragment removed.

The fragment only lowers where an existing universal request already has an explicit positional concept.

## 9.1 `read`

Input:

```text
read(file:///x#A)
```

lowers to:

```rust
ReadRequest {
    uri: file:///x,
    at: Some(Position::At(#A)),
    ...
}
```

Rules:

- fragment present + `at == None` -> lower to `Position::At`;
- fragment present + any explicit `at` -> `InvalidInput`;
- resulting `AnchoredText.uri` is the canonical path+query **without fragment**;
- if the selected resource is a directory/non-text resource that cannot honor an anchor -> `WrongKind`.

## 9.2 `poll`

Input target:

```text
file:///x#A
```

lowers to:

```rust
PollTarget {
    uri: file:///x,
    from_position: Some(Position::At(#A)),
}
```

Rules:

- fragment present + `from_position == None` -> lower;
- fragment present + explicit `from_position` -> whole poll request `InvalidInput`;
- all poll target indices remain unchanged after lowering.

## 9.3 `write`

Fragments are invalid.

Per-item result:

```text
InvalidInput
```

No mutation for that item.

## 9.4 `edit`

Fragments are invalid.

`EditRequest` already carries explicit operation anchors.

Do not invent an implicit edit anchor.

Per-item result:

```text
InvalidInput
```

## 9.5 `run`

Fragments are invalid per request item.

## 9.6 `send`

Fragments are invalid per request item.

## 9.7 `abort`

Fragments are invalid per URI item.

## 9.8 `delete`

Fragments are invalid per URI item.

## 9.9 `find`

Fragments in any root are invalid.

The whole `find` request fails `InvalidInput`.

## 9.10 `grep(Resources)`

Fragments in any resource source URI are invalid.

The whole grep fails `InvalidInput`.

## 9.11 `grep(Text)`

`AnchoredText.uri` identifies the text space itself and must not contain a fragment.

A fragment-bearing `AnchoredText.uri` is `InvalidInput`.

The anchors are already represented in `AnchoredText.lines`.

---

# 10. Resource claim contract

The mandatory control export is:

```text
artist:resource/extension@1.0.0
```

with:

```wit
claim: func(request: claim-request) -> claim-decision;
```

The request contains:

```text
verb
canonical URI including query
no fragment
```

## 10.1 `pass`

Meaning:

> This package does not claim this concrete `(verb, URI)` and does not prevent another extension from handling it.

## 10.2 `handle`

Meaning:

> This package is the implementation for this concrete `(verb, URI)`.

The package must export the corresponding standard verb interface.

## 10.3 `reserve`

Meaning:

> This URI belongs to a synthetic/resource subspace recognized by this package, but this verb must not fall through to another provider.

A unique `reserve` with no competing owner produces:

```text
Unsupported
```

Primary use: protect synthetic resource trees.

Example:

```text
file:///src/main.rs/symbols/Foo
```

AST:

```text
read -> handle
edit -> reserve
```

Filesystem must not interpret `.../symbols/Foo` as a real writable file.

---

# 11. Claim execution policy

Claims are routing logic, not ordinary resource operations.

They must be bounded and side-effect-free.

## 11.1 Restricted claim phase

Add an internal host execution phase:

```rust
enum ComponentPhase {
    Claim,
    Invoke,
}
```

During `Claim`:

- nested universal resource calls are denied;
- filesystem/network/process/PTY/KV mutation capabilities are denied;
- custom effectful host imports are denied;
- no user resource mutation is possible.

A package may compute over:

```text
verb
URI
query
package-local immutable configuration compiled/loaded into the component
```

Do not require resource existence checks during claim.

Existence/state checks belong to the actual verb implementation.

This keeps claim deterministic enough to reason about and prevents routing recursion.

## 11.2 Claim budget

Use both fuel and wall/epoch interruption.

V1 defaults:

```text
CLAIM_FUEL = 1_000_000
CLAIM_TIMEOUT = 50ms
```

These are internal constants, not public ABI.

A claim exceeding either budget fails the operation with canonical:

```text
Internal
```

Do not treat timeout/fuel exhaustion as `pass`.

## 11.3 Claim traps/failures

`claim` has no WIT error result.

Therefore:

```text
component trap
canonical ABI failure
missing required claim export
timeout
fuel exhaustion
host failure
```

all mean provider failure.

Routing aborts.

Result:

```text
Internal
```

Do **not** continue to another provider.

A broken active extension must never become invisible fallback behavior.

## 11.4 Claim caching

Do not cache claim decisions across independent operations in v1.

Reason: URI query semantics and future package behavior may be state-sensitive even though claim itself is restricted.

Allowed optimization:

Within one top-level `Operation`, memoize claim results for identical:

```text
(active generation, verb, canonical URI without fragment)
```

This is especially useful for repeated batch targets.

No cross-operation cache.

## 11.5 Generation pinning

At the beginning of a top-level operation:

1. snapshot the active candidate generations needed for candidate scheme lookup;
2. pin those `Arc<ActiveGeneration>` values;
3. run claims against those exact generations;
4. if one handles, invoke that exact generation.

A hot swap occurring between claim and invoke must not change the selected generation.

For batch operations, the active-generation snapshot is taken once for the whole top-level operation.

---

# 12. Deterministic routing

For one target `(verb, URI)`:

1. parse/canonicalize URI;
2. lower/reject fragment according to section 9;
3. select active candidate package generations whose `routes.schemes` contains the URI scheme;
4. sort candidates by package name only for deterministic diagnostics/execution ordering;
5. execute every candidate claim needed for arbitration;
6. collect owner assertions.

Owner assertion:

```text
handle | reserve
```

Resolution:

```text
zero owner assertions
    -> Unsupported

exactly one handle
    -> invoke that generation

exactly one reserve
    -> Unsupported

more than one owner assertion
    -> Conflict
```

Examples:

```text
handle + handle  -> Conflict
handle + reserve -> Conflict
reserve + reserve -> Conflict
```

Nothing executes on routing conflict.

Conflict diagnostics must name:

```text
verb
URI
competing package names
their decisions
```

Do not use first registration order.

Do not add priority.

Do not add middleware/next-provider chains in v1.

## 12.1 Why zero claims is `Unsupported`, not `NotFound`

`NotFound` is returned by a selected provider that recognizes the address space but determines the concrete resource does not exist.

If no extension handles the `(verb, URI)`, the kernel cannot assert existence semantics.

Therefore zero owner assertions is:

```text
Unsupported
```

This also makes verb augmentation possible.

A base filesystem provider can `pass` on `send(file://...)`, allowing another extension to add that verb later. If nobody does, the result is `Unsupported`.

---

# 13. Native compatibility adapter

Do not delete all native handlers before the resource router is proven.

Add a compatibility layer that lets native handlers participate in the same claim arbitration.

However, do **not** preserve "first matching TypedHandler wins" as architecture.

Native compatibility must produce explicit:

```text
pass
handle
reserve
```

## 13.1 `FileHandler`

Candidate scheme:

```text
file
```

Legacy projection segments currently recognized by `has_projection` include:

```text
symbols
map
show
implements
implementations
surface
deps
reverse-deps
trace
callers
callees
impact
```

Compatibility claim rules:

### URI path contains a recognized projection segment

For **all verbs**:

```text
pass
```

The AST/repository projection compatibility provider handles/reserves that synthetic subtree.

### Ordinary file path

For:

```text
read
write
edit
delete
find
grep(Resources)
```

return:

```text
handle
```

Do not perform filesystem existence checks in claim.

The actual operation returns `NotFound` where appropriate.

For unsupported verbs:

```text
pass
```

This intentionally allows future extensions to add verbs to file resources.

`grep(Text)` remains a kernel/tool pure-text path and is not routed to `FileHandler`.

## 13.2 `RepositoryHandler` / current AST projections

During compatibility:

### `file://` projection paths

If the path contains one of the currently recognized AST/repository projection segments:

For currently supported projection verbs:

```text
read -> handle
find -> handle where current behavior supports it
grep -> handle where current behavior supports it
```

For mutation/lifecycle verbs that must never fall through onto fake filesystem children:

```text
write
edit
send
run
abort
delete
poll
```

return:

```text
reserve
```

Do not claim ordinary non-projection `file://` paths in the compatibility router.

This removes the current accidental overlap where both FileHandler and RepositoryHandler can claim ordinary file reads.

Preserve ordinary-file observable behavior through FileHandler.

### `repo://`

For currently supported verbs:

```text
read
find
grep
```

return:

```text
handle
```

For unsupported verbs:

```text
pass
```

Do not reserve the whole `repo://` scheme; future packages must be able to augment it.

## 13.3 `SessionHandler`

Candidate scheme:

```text
session
```

For:

```text
read
write
send
poll
abort
delete
```

return:

```text
handle
```

For all other verbs:

```text
pass
```

No scheme monopoly.

## 13.4 `ToolsHandler` / `tools://`

`tools://` package-file access remains a bootstrap/native resource path during this goal.

Its resource operations must participate in the same router once practical.

Do not make loading `tools://` depend on editable resource extensions; it is part of bootstrap package access.

## 13.5 `resources://`

Same bootstrap rule.

The kernel/native bootstrap provider gives ordinary file-like access to resource package files so a model can inspect/edit:

```text
resources://ast/resource.md
resources://ast/src/lib.rs
resources://ast/resource.wit
```

Do not require `resources://` itself to be loaded by a `resources://` component.

---

# 14. Cross-resource nested calls

Resource components can import any standard `artist:resource/<verb>@1.0.0` interface declared in their world and permitted by capabilities.

The host implementation performs the same kernel typed operation with a child `InvocationScope`.

No JSON.

Example:

```text
read(file:///src/main.rs/symbols/Foo)
    -> resources://ast generation 7
        -> imported artist:resource/read
            -> kernel read(file:///src/main.rs)
                -> filesystem provider
        -> AST parse/projection
        -> return AnchoredText
```

Nested calls are normal resource calls and therefore:

- perform URI parsing;
- perform claim arbitration;
- pin their own selected provider generations;
- inherit working URI/environment metadata;
- inherit cancellation through a child token;
- inherit deadline, bounded by parent deadline.

## 14.1 Recursion protection

Maintain an internal routing stack in `InvocationScope` or sibling internal state.

Detect recursive re-entry of:

```text
(active resource package generation, verb, canonical URI without fragment)
```

within one invocation chain.

On repetition:

```text
Internal
```

with a diagnostic showing the cycle.

Do not add a public recursion-specific error.

Claim execution cannot call nested resources, so claim does not enter this recursion mechanism.

---

# 15. Batch semantics

Existing batching semantics remain authoritative.

Routing is logically per target URI.

The kernel may group resolved items by provider generation for efficiency only if all semantics remain identical.

Requirements:

- result order matches input order;
- independently fallible batch operations remain independently fallible;
- resource-level mutation atomicity remains;
- duplicate write URI validation happens before any write mutation;
- same-URI send chunks remain serialized in accepted order;
- one package generation is pinned consistently for the top-level operation snapshot.

## 15.1 `read`

Resolve each request target independently.

Provider may receive grouped requests after resolution.

## 15.2 `write`

Before routing/mutation:

- reject duplicate canonical URIs across the write batch;
- query is part of URI identity;
- fragments already invalid.

If duplicate validation fails, perform zero writes.

## 15.3 `edit`

Each URI resolves independently.

Operations for one URI remain atomic.

## 15.4 `run`, `send`, `abort`, `delete`

Resolve each item independently.

Preserve per-item result ordering.

## 15.5 `find`

Resolve each root independently.

Combine provider results according to existing deterministic find semantics.

A root with no handler produces `Unsupported` for the request.

## 15.6 `grep(Resources)`

Resolve each source independently.

Preserve existing grep result semantics and exact snapshot/anchor correctness.

## 15.7 `grep(Text)`

No resource-provider routing.

Operate directly on supplied `AnchoredText`.

## 15.8 `poll`

Resolve each target independently.

Cross-provider poll evaluation/aggregation remains a kernel responsibility.

The kernel owns:

- global target indices;
- `All`/`Any`;
- timeout;
- accumulated-text regex evaluation;
- default condition combination.

Do not push global PollCondition evaluation into one selected provider.

---

# 16. Resource documentation

Resource extensions are self-documenting.

`resource.md` has:

```text
frontmatter -> compact machine-readable docs
body        -> full prose
```

## 16.1 Compact model catalog

Generate a resource catalog from **active generations only**.

Conceptual output:

```text
Resources:

file://<path>
  Files and directories.
  read write edit find grep delete

file://<path>/symbols/
  Structured symbols defined by a source file.
  read find

file://<path>/symbols/<symbol>/callers
  Callers of a symbol.
  read
  query: limit

repo://<project>/issues/
  Repository issue listing.
  read find
  query: state author label limit
```

Do not dump full Markdown bodies into every prompt.

Use:

```text
description
docs[].uri
docs[].summary
docs[].verbs
docs[].query
```

for compact seeding.

Full docs remain readable:

```text
read(resources://ast/resource.md)
```

## 16.2 Docs are not routing

Never execute `docs[].uri` patterns.

They are human/model documentation only.

The typed `claim` export is authoritative routing.

## 16.3 Failure isolation

If a candidate edit to an active package breaks:

```text
old active docs remain seeded
old active routes remain used
old active code remains invoked
```

A malformed unrelated package must not erase the entire resource catalog.

A malformed never-active package is omitted.

---

# 17. `notify` hot-reload semantics

Create a shared package watcher service after the package-generation abstraction exists.

It watches:

```text
<project>/tools
<project>/resources
```

recursively.

Map changed path -> nearest containing package root.

Debounce/coalesce by package root.

On batch event:

```text
mark dirty(package)
attempt candidate reload(package)
```

Use the same reload path Artist invokes after its own package-file writes.

No separate "external reload" semantics.

## 17.1 Ignore rules

Ignore generated paths:

```text
**/target/**
**/*.wasm.artist.json
**/.git/**
editor swap/temp files
```

Do not ignore authored precompiled:

```text
tool.wasm
resource.wasm
```

## 17.2 Removal

If an inactive package directory disappears:

```text
nothing to do
```

If an active package is externally removed completely:

- do not instantly destroy its active generation merely because a transient filesystem event says the directory disappeared;
- debounce;
- after the debounce window, if package root is truly gone, deactivate that package atomically;
- unrelated packages remain active.

If an active package is temporarily malformed during a multi-file editor save:

```text
candidate fails
old active generation remains
later debounced event retries
```

This is why hot-reload activation must be transactional.

## 17.3 Watcher lifetime

The watcher is owned by the project/kernel runtime and remains alive for the kernel lifetime.

Dropping the kernel should cleanly stop the watcher.

No detached immortal thread/task.

---

# 18. Resource component activation

Resource candidate activation order:

1. discover package root;
2. parse `resource.md` with `gray_matter`;
3. validate frontmatter schema;
4. validate `contract == artist:resource:extension@1`;
5. validate routes:
   - nonempty;
   - valid scheme names;
6. validate `exports`:
   - no duplicates;
   - only ten universal verbs;
7. parse package-local `resource.wit` if present;
8. resolve Cargo package/target using `cargo_metadata`;
9. build or load precompiled component;
10. validate component binary;
11. prove mandatory export:
    ```text
    artist:resource/extension@1.0.0
    ```
12. prove exact declared standard universal exports;
13. validate standard nested resource imports against `capabilities`;
14. resolve custom typed WIT dependencies;
15. validate primitive capabilities;
16. precompute component export indices;
17. build runtime metadata:
    ```text
    routes
    docs
    export set
    dependency pins
    capabilities
    ```
18. compute fingerprint/provenance;
19. atomically publish active generation.

Nothing from the candidate is externally visible before step 19.

---

# 19. Claim/invoke linker policy

Use two logical authorities with the same compiled component.

## Claim store/linker

Purpose:

```text
call extension.claim only
```

Authority:

```text
no nested resource effects
no filesystem mutation
no network
no process/PTY
no KV mutation
small fuel/time budget
normal memory/store limits
```

It may need package-local pure custom imports only if they are demonstrably side-effect-free. Default deny.

## Invoke store/linker

Authority:

```text
exact frontmatter-granted capabilities
nested resource interfaces
custom typed dependencies
granted primitives
WASI according to package policy
```

Every invocation uses `InvocationScope`.

The active generation stores compiled component/dependency metadata, not a persistent instantiated store.

---

# 20. Primitive filesystem substrate with `cap-std`

Refactor filesystem behavior carefully before using it beneath WASM resource extensions.

Target:

```rust
struct FilesystemRoot {
    root_path: PathBuf,       // semantic mapping/debug only
    dir: cap_std::fs::Dir,    // actual capability
}
```

Convert:

```text
file:///absolute/project/path
```

to a relative path only after proving it lies under the configured project root.

Then use `Dir` APIs for:

```text
open
read
read_to_string
metadata
read_dir
create_dir_all
rename
remove_file
remove_dir
...
```

Atomic replace should remain temp-file + atomic rename semantics, but create temp files within the same capability/root and preferably same parent directory.

Do not regain ambient authority by converting back to absolute paths for the operation.

Search/index libraries that require absolute paths may temporarily receive validated host paths, but isolate this exception and do not let it become the primitive API exposed to resource WASM.

The long-term filesystem component should import a typed primitive filesystem capability implemented over this `Dir`.

Do not expose raw host `/` access.

---

# 21. AST proof migration

The first meaningful `resources://` proof must augment an existing namespace.

Do not prove only `fake://`.

Move current AST/repository file projections toward a WASM resource extension.

Package:

```text
resources://ast/
```

Candidate scheme:

```text
file
```

Addressable paths should preserve existing child hierarchy where practical:

```text
file:///src/main.rs/symbols/
file:///src/main.rs/symbols/Foo
file:///src/main.rs/symbols/Foo/callers
file:///src/main.rs/symbols/Foo/callees
file:///src/main.rs/symbols/Foo/impact
file:///src/main.rs/map
file:///src/main.rs/show/...
file:///src/main.rs/implements/...
file:///src/main.rs/surface
file:///src/main.rs/deps/...
file:///src/main.rs/reverse-deps/...
```

Do not convert child resources to query syntax simply because query support now exists.

Use queries only for modifiers:

```text
?limit=20
?kind=function
?context=10
```

## 21.1 AST claim behavior

For recognized AST projection paths:

```text
supported verb -> handle
unsupported mutation/lifecycle verb -> reserve
```

For ordinary file paths:

```text
pass
```

## 21.2 Underlying source access

Prefer:

```text
AST component
    -> imported artist:resource/read
    -> ordinary file resource
```

rather than granting AST ambient filesystem access.

If performance makes a narrow primitive read capability necessary, justify it explicitly and retain Artist URI/resource semantics at the public boundary.

## 21.3 Anchor correctness

AST projections return ordinary `AnchoredText`.

Preserve:

- stable anchors;
- line endings;
- exact view URI including query, excluding fragment;
- grep/edit anchor compatibility where semantics permit.

Do not derive anchors by line-number -> reopen-current-file after search.

---

# 22. Resource authoring ergonomics

The raw WIT remains authoritative, but Rust extension authors should not have to hand-write repetitive error conversion.

Add a small internal/helper crate or module only if it stays thin, for example:

```text
artist-resource-sdk
```

Allowed responsibilities:

- generated WIT types reexports;
- `Error` constructors:
  ```rust
  unsupported(uri, message)
  not_found(uri, message)
  wrong_kind(uri, message)
  conflict(uri, message)
  ```
- URI/query convenience parsing;
- anchor conversion;
- poll wire lower/lift helpers;
- guest-side boilerplate macros if genuinely useful.

Do not make this SDK a second contract.

Do not hide the WIT interfaces behind a generic dispatch trait.

A non-Rust component author must still be able to implement the WIT directly.

---

# 23. Custom typed component composition

Resource packages may export additional WIT interfaces beyond:

```text
artist:resource/extension
artist:resource/<universal verbs>
```

Other tools/resources may import them.

Reuse/generalize the existing custom dependency resolver.

Rules:

- match by WIT interface identity/version compatibility;
- pin dependency generations when activating the dependent generation;
- in-flight dependent invocation continues using pinned dependency generations;
- dependency hot swap does not mutate an already-active dependent generation's linked dependency graph;
- when the dependent itself reloads, it resolves current compatible dependencies;
- ambiguity is activation failure;
- missing dependency is activation failure;
- no arbitrary JSON dependency bridge.

---

# 24. Required tests: known-art migration

Before resource routing tests, add/keep tests proving the infrastructure cleanup.

## Cargo

- `cargo_metadata::MetadataCommand` resolves the package target used by a tool/resource package.
- `escargot::CargoBuild::exec()` performs the authored source build.
- Escargot `CompilerArtifact` messages locate the emitted WASM artifact for the metadata-selected target.
- `--offline`, `--target wasm32-wasip2`, custom `--profile product`, and `RUSTFLAGS` survive the wrapper.
- no hand-written Cargo JSON protocol structs remain.
- failed Cargo build retains active generation.
- precompiled artifact path still works.

## Frontmatter

Using `gray_matter`:

- valid YAML + body parses;
- `---` inside Markdown body does not truncate body;
- malformed YAML fails only that package;
- tool and resource manifests use the same parser.

## URI

- query now accepted;
- existing native path -> `file://` normalization preserved;
- fragment still accepted only if valid Artist anchor syntax;
- invalid `#anchor?query` rejected.

## Cancellation / async Wasmtime

- top-level cancellation interrupts a long-running component call;
- nested resource call receives cancellation;
- child timeout does not cancel parent;
- no private runtime/thread bridge required;
- async host resource import can await kernel operation.

## Store limits

- runaway memory growth is bounded;
- infinite/long CPU claim is interrupted;
- normal conformance components remain unaffected.

## cap-std

- normal file read/write works;
- `..` cannot escape;
- symlink escape cannot escape;
- directory operations remain correct;
- atomic edit/write remains atomic.

## notify

- external edit to `tool.md`/source reloads candidate;
- external edit to `resource.md`/source reloads candidate;
- malformed intermediate save retains old generation;
- unrelated package remains active;
- `target/` changes do not cause reload storm.

---

# 25. Required tests: URI/query/fragment

Valid:

```text
file:///a/b
file:///a/b?stat
file:///a/b?state=open
file:///a/b?state=open&label=bug
file:///a/b?label=a&label=b
file:///a/b?flag
file:///a/b?flag=
file:///a/b?state=open#BlueHorse
file:///a/b/symbols/Foo/callers?limit=20#Quartz17
```

Invalid:

```text
file:///a/b#BlueHorse?state=open
```

Prove:

- query item order preserved;
- repeated keys preserved;
- bare key vs empty value preserved;
- key validation enforced;
- trailing `/` directory hint survives query;
- query is part of `ResourceUri` equality/hash identity;
- fragment is excluded from claim URI;
- `without_fragment()` preserves query exactly;
- reversed query ordering yields distinct `ResourceUri`;
- `read(uri#A)` lowering;
- `poll(uri#A)` lowering;
- double positional specification rejected;
- fragment rejected for every other verb as specified.

---

# 26. Required tests: routing

Use real WASM claim components, not only native fake handlers.

At minimum:

## same scheme, different paths

A and B both advertise `fake`.

```text
A handle read(fake://x/a)
B handle read(fake://x/b)
```

Both coexist.

## same scheme, different verbs

```text
A handle read(fake://x)
B handle grep(fake://x)
```

Both coexist.

## pass

```text
A pass
B handle
```

B executes.

## reserve

```text
A reserve
B pass
```

Result `Unsupported`.

No invocation.

## ambiguous handle

```text
A handle
B handle
```

Result `Conflict`.

Nothing executes.

## handle + reserve

Result `Conflict`.

Nothing executes.

## reserve + reserve

Result `Conflict`.

## zero claims

Result `Unsupported`.

## claim trap

A candidate traps.

Result `Internal`.

Do not execute B even if B would handle.

## claim timeout/fuel

Result `Internal`.

## claim may not perform nested read

Attempted nested resource access in claim is denied.

No side effect.

## generation pin

Claim generation N, hot-swap package, invoke still uses N.

Next top-level operation uses N+1.

## query routing

Component can make different claim decisions for:

```text
fake://x?mode=a
fake://x?mode=b
```

Kernel passes query intact.

---

# 27. Required tests: typed component composition

At least one end-to-end resource extension must:

1. be loaded from `resources://`;
2. export mandatory `extension.claim`;
3. export `artist:resource/read@1.0.0`;
4. import `artist:resource/read@1.0.0` under a local alias;
5. claim a synthetic child URI;
6. nested-read another resource;
7. return an ordinary typed `ReadResult`.

No JSON conversion anywhere in this path.

Also prove a custom package-local WIT dependency works through the generalized dependency resolver.

---

# 28. Required tests: native compatibility

Before migrating native handlers away, prove:

## ordinary file

```text
read(file:///real.txt)
```

File compatibility -> handle.

Repository/AST compatibility -> pass.

## projection

```text
read(file:///real.rs/symbols/Foo)
```

File compatibility -> pass.

AST/repository compatibility -> handle.

## projection edit

```text
edit(file:///real.rs/symbols/Foo)
```

File -> pass.

AST -> reserve.

Result -> `Unsupported`.

No file named `symbols/Foo` is created.

## file verb augmentation

A WASM extension may handle a verb FileHandler passes on.

No FileHandler reserve blocks it.

## session

Supported session verbs handle; others pass.

---

# 29. Required tests: package activation and docs

Prove:

- malformed never-active resource package omitted;
- malformed package does not erase unrelated resource catalog;
- active generation N remains after malformed N+1;
- old docs remain after malformed N+1;
- routes/docs/code/capabilities/dependencies swap together;
- external watcher edits use same activation path;
- custom dependency generation pinned;
- active package removal deactivates only that package after debounce;
- in-flight generation remains usable after deactivation/hot swap.

---

# 30. Required tests: AST proof

At minimum:

- normal file read still works;
- AST `symbols` child read works;
- one deeper child such as `callers` works;
- AST component gets base source via typed nested `resource.read`;
- file and AST packages both advertise `file`;
- no conflict for ordinary files;
- no conflict for AST children;
- unsupported AST edit reserves;
- anchors remain stable across equivalent reads;
- query modifier reaches AST component unchanged;
- full `resource.md` readable through `resources://ast/resource.md`;
- compact AST docs appear in model resource catalog.

---

# 31. Implementation order / commit discipline

Implement in this order.

Do not jump ahead because later steps depend on earlier invariants.

## Phase A — commodity runtime cleanup

1. add `cargo_metadata` + `escargot`; use metadata for project/target discovery and Escargot for Cargo build/message plumbing; remove hand-written Cargo protocol structs/parsers;
2. add `gray_matter`; unify tool frontmatter parser;
3. add query support using `url`; add URI tests;
4. introduce `InvocationScope` + real `CancellationToken`;
5. migrate component runtime to Wasmtime async + async WASI;
6. wire StoreLimits/fuel/epoch interruption;
7. refactor filesystem substrate to `cap-std`.

Checkpoint:

```text
all existing behavior/tests green
no resources:// yet
```

## Phase B — shared WIT migration

1. add exact `artist:resource@1.0.0`;
2. generate host bindings from `host-world`;
3. migrate `artist:tool` to `1.0.0` and reuse resource types;
4. replace host-* imports with direct resource imports;
5. rebuild universal conformance guests/seeds;
6. remove duplicate old resource types/interfaces from tool WIT.

Checkpoint:

```text
all 10 tools still work through named WASM component path
no JSON semantic boundary
```

## Phase C — shared component package runtime

1. generalize package discovery/build/provenance;
2. generalize active generation;
3. generalize dependency pinning;
4. preserve tool failure isolation;
5. add watcher service with `notify-debouncer-mini`.

Checkpoint:

```text
tools:// still works
external tool edits hot-reload transactionally
```

## Phase D — resource package bootstrap

1. add bootstrap `resources://` file namespace;
2. add `ResourceFrontmatter`;
3. add resource package discovery/build;
4. add resource active generations;
5. add resource docs catalog;
6. watcher includes resources root.

Checkpoint:

```text
resource packages can build/activate/edit/hot-swap
no routing yet
```

## Phase E — typed claim router

1. add exact claim host invocation;
2. add restricted claim phase;
3. add claim budget;
4. add candidate scheme index;
5. add pass/handle/reserve arbitration;
6. add generation pinning;
7. add recursion stack for invoke;
8. add fragment lowering;
9. move kernel typed operation routing onto new router.

Checkpoint:

```text
real WASM fake resource extensions pass routing conformance
```

## Phase F — native compatibility

1. adapt FileHandler claims exactly as specified;
2. adapt Repository/AST claims exactly as specified;
3. adapt SessionHandler;
4. adapt bootstrap tools/resources handlers;
5. remove first-match typed-handler semantics from normal routing.

Checkpoint:

```text
existing CLI/agent behavior remains green under new arbitration
```

## Phase G — AST WASM proof

1. seed/write `resources://ast`;
2. move representative projections to the component;
3. nested typed read of source;
4. reserve unsupported mutations;
5. docs seeding;
6. migrate remaining AST projections where practical without changing semantics.

Checkpoint:

```text
file:// is demonstrably extensible by multiple active components
```

## Phase H — cleanup

Remove dead:

```text
old host-* WIT
old duplicate types
old sync component paths
old custom Cancellation
old Cargo JSON structs
old frontmatter splitter
first-match typed routing assumptions
projection kernel special cases that moved into AST resource package
```

Run formatting, clippy, full tests.

---

# 32. Things this goal must NOT invent

Do not add:

```text
one provider per URI scheme
provider priority
middleware ordering
next-provider callbacks
generic resource dispatch JSON
closed ResourceKind
resource-type compatibility matrices in tools
a second Error algebra
query-as-child hierarchy
?symbols for hierarchical AST resources
opaque persistent WASM instance state
process://
bash implementation
Brush
PTY
terminal emulator
TUI screen model
shell readiness protocol
prompt sentinels
new PollAtom
```

Do not broaden scope into terminal work.

Do not rewrite FFF.

Do not reopen `EditResult`, poll AST/wire semantics, canonical errors, or run/send semantics.

---

# 33. Audit checklist before calling this complete

The final audit should answer YES to all:

```text
[ ] cargo_metadata replaced custom Cargo metadata parsing
[ ] escargot owns Cargo build construction/message capture
[ ] no hand-written Cargo JSON protocol structs remain
[ ] plugy is not present in the dependency graph
[ ] gray_matter parses tool.md and resource.md
[ ] url supports ordered/repeated query identity
[ ] CancellationToken is real internal cancellation
[ ] Wasmtime universal paths are async
[ ] no spawn-thread/private-runtime component->kernel bridge remains
[ ] StoreLimits/fuel/epoch interruption wired
[ ] filesystem primitive uses cap-std capability root
[ ] notify watches tools and resources transactionally
[ ] one shared package-generation mechanism exists
[ ] artist:resource@1.0.0 WIT exactly exists
[ ] artist:tool uses resource types/imports
[ ] no duplicate host-* semantic resource API remains
[ ] resources:// package files are inspectable/editable
[ ] resource.md docs are active-generation metadata
[ ] extension.claim is mandatory
[ ] resource verb exports are optional and validated against frontmatter
[ ] optional exports invoked through typed Wasmtime API, not JSON
[ ] claim phase is side-effect-restricted
[ ] claim timeout/fuel enforced
[ ] claim failure aborts routing
[ ] claims are operation-generation pinned
[ ] no cross-operation claim cache
[ ] routing is pass/handle/reserve arbitration
[ ] zero claim = Unsupported
[ ] ambiguity = Conflict
[ ] fragment only lowers for read/poll
[ ] all other fragment uses are rejected precisely
[ ] query order/repeated keys affect ResourceUri identity
[ ] FileHandler passes AST projection subtrees
[ ] AST reserves unsupported mutations
[ ] multiple components coexist on file://
[ ] nested resource calls are typed and cancellation-aware
[ ] recursion cycle detection exists
[ ] malformed package isolation works
[ ] external partial saves retain old active generation
[ ] AST WASM proof augments file://
[ ] resource docs are compactly seeded
[ ] full resource docs remain readable
[ ] all existing universal verb tests remain green
[ ] no Brush/bash work entered this goal
```

---

# 34. Final architectural target

```text
                              MODEL / PROGRAMS
                                    |
                                    v
                         typed WASM tool components
                                tools://
                                    |
                                    v
                    artist:resource@1.0.0 interfaces
                                    |
                                    v
                       tiny claim/resource router
                                    |
                 candidate schemes + claim arbitration
                                    |
        +---------------------------+---------------------------+
        |                           |                           |
        v                           v                           v
 resources://filesystem       resources://ast         resources://github
        |                           |                           |
        +----------- typed nested resource calls --------------+
                                    |
                                    v
                       explicitly granted primitives
                  cap-std / network / clock / future PTY / KV
```

Core invariant:

> `tools://` makes Artist's actions editable.  
> `resources://` makes Artist's addressable world editable.

A resource extension is not a namespace owner.

The contract is:

```text
(verb, canonical URI path+query)
    -> active candidate generations
    -> bounded typed claim()
    -> conflict-safe arbitration
    -> typed resource operation
```

Composition happens through typed resource imports, not registration order, scheme monopoly, middleware chains, or JSON dispatch.
