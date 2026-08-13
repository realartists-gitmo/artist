# Artist Component ABI Specification

Status: implemented horizontal-slice specification

This document specifies the component boundary used by tools, handlers, verbs,
and runtimes. The conformance tree contains one reloadable Rust/WASM package
for each universal verb; those packages delegate resource effects through the
explicit kernel capability bridge.

## 1. Purpose

The harness kernel is a small trusted nucleus. Functionality outside that
nucleus is represented by WebAssembly Components and communicates through the
WebAssembly Component Model.

The component boundary must be:

- language-independent;
- portable across host platforms;
- typed and versioned;
- capable of structured requests, results, errors, and host calls;
- compatible with resource handles, typed streaming, capabilities, and
  replaceable universal verbs.

The typed universal tool contracts are defined in the companion WIT package
at `crates/artist-component/wit/tool-surface/world.wit`. It defines the shared
URI, anchor, line, diff, error, polling, and request/result types plus the ten
verb interfaces: `read`, `write`, `edit`, `poll`, `send`, `run`, `abort`,
`delete`, `find`, and `grep`. Each verb package declares one contract identity
and one required resource capability. Its component entrypoint invokes only
that verb through the host kernel bridge; higher-level components compose verbs
through the contract registry.

The `read-stream`, `grep-stream`, and `poll-stream` operations use typed
component-model streams. Their element types are respectively
`read-result`, `anchored-text`, and `anchored-text`; adapters must not encode
these stream elements as arbitrary JSON chunks.

The execution context for `run` is supplied by the invocation context (working
URI and environment). The typed `run` request contains only its resource URI
and arguments. Cancellation, deadline, correlation, and authority are also
invocation context propagated by the kernel out of band; none are ordinary
tool arguments.

`poll` accumulates newly observed anchored text from each target's poll cursor;
regex matching considers only newly observed material. `Terminated` is valid
only for resources advertising termination capability. The initial reload trigger
is explicit; the runtime also exposes a debounced trigger that calls the same
transactional build/validate/activate path.

WIT is the interface-definition language. WASM components contain the
implementation. Provider-facing prose and JSON schemas are a separate layer.

## 2. Scope of this slice

This slice defines and exercises:

1. the canonical WIT package and worlds;
2. component identity and ABI versioning;
3. typed universal invocation and structured result/error transport;
4. the component-to-kernel host-call boundary;
5. lifecycle and failure semantics;
6. the minimum conformance harness;
7. Rust host bindings, one trivial test component, and ten verb packages.

The test component proves the ABI and lifecycle. The verb packages prove the
source-to-WASM build path and execute real kernel operations through the
capability bridge.

## 3. Explicit non-goals

This slice does not:

- duplicate privileged resource logic inside WASM; the components are the
  executable verb boundary and the kernel remains the resource backend;
- implement shell or Python bindings;
- intercept native filesystem paths;
- expose host paths, file descriptors, or subprocesses;
- define capability policy in full;
- register tool packages;
- define automatic filesystem watching as the reload trigger;
- define repository, AST, session, or runtime-specific interfaces;
- do not make JSON the kernel or universal-component semantic representation;
  JSON is explicitly confined to provider adapters and the quarantined legacy
  generic world.

Those systems consume this ABI in later horizontal slices.

## 4. Component model

The initial universal component worlds are conceptually:

    artist:component/world
      imports artist:host/host
      exports artist:component/component

Universal verb components export their verb-specific typed interface. They do
not receive a generic target-plus-JSON invocation. A package-local extension
may instead declare its own `tool.wit`; it must export a root `invoke` function
(or the interface name declared by its contract). The host reflects that
function's WIT parameter/result types only at the outer provider adapter,
while component-to-kernel calls still use the shared typed host worlds.
Package-local WIT imports are resolved against active package contracts during
activation. The registry links imported typed exports to the active dependency
generation; an unsatisfied or incompatible custom import rejects activation
before the new version becomes visible.

The legacy generic component world remains available only for compatibility
with pre-typed packages and is not a universal-verb or package-local execution
path.

The component exports:

- immutable identity and ABI metadata;
- its typed verb entrypoint, or a package-local `invoke` entrypoint.

The host provides:

- host-side invocation of a resource operation;
- structured host errors;
- cancellation/context metadata.

The host owns component instantiation, scheduling, memory, traps, and
resource lifetimes. Components do not receive ambient access to the host.

## 5. Canonical data model

The ABI must define WIT records for:

- component-info: name, version, ABI version, and declared interfaces;
- invocation: invocation ID, operation name, target resource address, and
  structured input;
- response: structured output and optional metadata;
- host-request: an operation against a resource;
- host-response: structured output and metadata;
- error: stable error kind, message, and structured details;
- invocation context: cancellation/deadline/correlation metadata propagated
  out of band rather than embedded in ordinary tool input.

Universal verb payloads are WIT records and the kernel's corresponding typed
Rust values. Canonical UTF-8 JSON exists only at provider adapters and in the
quarantined legacy generic world.

The implementation must document maximum payload size, text encoding, and
whether unknown metadata fields are preserved.

## 6. Resource identity

The ABI carries resource identity, not host filesystem authority.

An address is represented as its canonical URI string at the component
boundary. Bare OS paths are normalized by the kernel resolver before a
component sees them.

This revision uses host-issued opaque IDs with explicit owned and borrowed
markers. The host validates IDs, permits borrowed use, and permits release
only for owned handles. WIT resource-table handles remain a future ABI option.

## 7. Invocation semantics

Every invocation has a unique ID and receives one immutable context.

The host must distinguish:

- successful response;
- declared component error;
- host/resource error;
- cancellation;
- component trap or ABI violation.

An invocation must not silently continue after cancellation. The host owns
the final outcome if cancellation races with a response.

The legacy generic invocation test surface includes materialized chunks for
its JSON-oriented stream operation. The typed tool surface is separate: its
`read-stream`, `grep-stream`, and `poll-stream` operations are WIT
`stream<T>` functions, with typed element records and component-model async
bindings enabled in the host. A collecting adapter may be provided for
callers that cannot consume a stream directly, but collection is adapter
policy rather than the ABI.

## 8. Versioning and compatibility

The WIT package has an explicit package version and ABI version.

- A component declares the ABI version it was built against.
- The host rejects incompatible major versions before invocation.
- Compatible minor revisions may add optional metadata but must not change
  the meaning of existing fields.
- WIT interface names are stable identifiers, not display labels.
- Component identity and provider-facing tool identity are separate concepts.

The loader must report incompatibility as a structured load error, not as a
runtime trap.

## 9. Lifecycle

The host lifecycle is:

    discover -> validate -> instantiate -> describe -> invoke* -> cancel/unload

Instantiation must not execute an invocation. A component may be rejected
before invocation if its package, world, ABI version, or required imports are
invalid.

Persistent component state is not part of this slice. The host must therefore
not promise state survival across unload or replacement.

## 10. Package layout

The ABI package should live independently of any individual tool:

    crates/artist-component/
    ├── wit/world.wit
    ├── src/lib.rs
    └── conformance/echo/

Individual future extensions may include the ABI package and their own WIT
interfaces. A source-first tool package has this shape:

    tool.md              # prose and provider-facing metadata
    tool.wit             # package-local interface for non-universal tools
    Cargo.toml           # optional build metadata
    src/                 # authored implementation, e.g. src/lib.rs
    target/.../tool.wasm # generated component artifact/cache

The source language is not part of the ABI. Rust is the first supported
authoring path, and the component loader always executes the compiled WASM
artifact rather than source text.

The host also discovers an unregistered package-shaped tool directory:

    tool.md       # YAML frontmatter plus prose
    src/          # source-first implementation directory
    tool.wit      # package-local interface for non-universal tools
    tool.wasm     # optional prebuilt/cache artifact

Discovery parses and validates the Markdown metadata and records source and
artifact locations. It accepts either source or a prebuilt artifact, but does
not register or execute the tool. The component package builder provides the
source-to-component build boundary. It currently targets Rust/Cargo packages
producing `wasm32-wasip2` components and:

- invokes Cargo using machine-readable artifact output;
- captures compiler diagnostics on failure;
- validates the resulting artifact against the component ABI before accepting it;
- writes a JSON provenance sidecar containing source fingerprint, ABI, target,
  profile, compiler versions, and artifact hash;
- reuses an artifact only when its fingerprint and content hash still match;
- never writes new provenance or accepts an artifact after a failed build or
  failed ABI validation.

Tool builds use the Cargo `product` profile by default. That profile inherits
`release` and sets optimization level 3, disables incremental compilation and
overflow checks, strips the artifact, enables fat LTO, uses one codegen unit,
and sets stable `target-cpu=native` through the build environment.
The resulting artifact is therefore an optimized, host-targeted tool build;
the profile name is included in provenance and cache fingerprints.

The builder returns an immutable artifact path and provenance record. The
runtime coordinator now provides the first hot-replacement boundary:

- each package has an atomically selected active component version;
- reload builds and validates before acquiring the write lock and swapping;
- each activated version carries a monotonically increasing generation and
  provenance reference;
- cloned version leases pin in-flight invocations to the old component;
- failed builds or validation leave the previous active version untouched.

Automatic filesystem watching remains a later trigger layer over this
explicit reload operation. A debounced trigger is already present as a
reusable seam: quiet-period requests supersede one another and call the same
transactional build/validate/activate path.

## 11. Conformance exercise

Completion requires:

1. WIT definitions checked into the repository;
2. generated Rust bindings compiling;
3. a host-side loader/instance boundary;
4. a trivial component that returns a structured response;
5. tests for successful invocation, declared error, malformed response,
   incompatible ABI, cancellation, and trap classification;
6. no native filesystem or runtime access granted to the component;
7. a short example showing how a later verb could be layered on top without
   changing the ABI.

The conformance component is a test fixture, not a production tool.

The current implementation lives in the artist-component workspace crate and
uses Wasmtime's Component Model bindings. The conformance fixture targets
wasm32-wasip2 and receives no preopened filesystem or other host capability.

## 12. Decisions reserved for later slices

- exact capability representation and enforcement;
- resource-handle ownership and borrowing;
- component discovery and trust roots;
- filesystem-triggered hot replacement and reload policy;
- tool Markdown/frontmatter schema;
- provider-specific schema translation;
- runtime bindings for shell and Python;
- component-defined universal verbs.
