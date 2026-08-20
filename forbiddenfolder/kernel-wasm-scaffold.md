# Kernel WASM Surface — Scaffold

The wasm host lives inside the kernel. Three contract families, one host.

## Stack

- `wasmtime` 47.0.3, `wasmtime-wasi` 47.0.3 — feature `p3` (WASIp3), not `p2`.
- `component-model-async` + `component-model-bytes`, `wasm_component_model_async(true)`.
- Async host, first-class. The kernel is tokio + async-trait; the events contract is
  inherently async. No sync path.
- Data crossing the model boundary is **TOON, not JSON**.

## Layout

```
crates/artist-kernel/
  wasm/                        # host runtime: Engine/Store, linker, loader, classification
    src/
    nouns/                     # noun contract: wit + host glue (WasmNamespace -> kernel Namespace)
      src/
      wit/
    verbs/                     # verb contract: family shape + dispatch
      src/
      wit/
    events/                    # event contract: broker, subscribe/emit, lifecycle
      src/
      wit/
```

## Decisions

1. **Async now.** Host-first, `p3`, no sync path.
2. **Nouns** — an extension is a namespace. The wasm component exports the namespace
   interface (path-based: lookup/readdir/getattr/read); the kernel mounts it and maps
   FUSE inos to paths internally so wasm never sees FUSE internals. Host glue:
   `WasmNamespace` implements the kernel `Namespace` trait.
3. **Verbs** — batch-native family shape (each verb exports one function taking
   `list<request>` returning `list<result<response, error>>`), but the **model sees
   scalar** — one tool call per request. The batch is a harness↔component optimization,
   not a model-facing surface. No specific verbs defined yet; the family shape and
   dispatch path only.
4. **Events** — the typed event stream contract. An extension subscribes to, reacts to,
   and emits typed events. Host glue is the broker.
5. **Lifecycle: minimal.** Load-on-demand, one live instance per extension. Hot-reload
   is deferred — it gets its own subcrate later, not now.
6. **Classification** — by exported WIT interface inspection at load time, no manifest.
   An extension's class is which of the three families its exported interfaces satisfy.
   A component can export multiple families.
7. **No shared WIT types package yet.** Each contract subcrate owns its `wit/`. Factor
   shared types when the first real duplicate appears — not before.
8. **Filesystem: full `wasi:filesystem` rooted at the namespace universe.** The
   `wasmtime-wasi` stock filesystem host is cap-std / real-directory based
   (`Vec<(Dir, String)>` preopens). Our universe is virtual (namespace URIs, not OS
   dirs), so we **implement the `wasi:filesystem` interface ourselves on top of kernel
   `Vfs`** (bindgen the p3 filesystem wit, write our own host). Same idiomatic
   guest-facing interface; our host behind it. This is the piece that makes the whole
   design cohere — extensions read any namespace (including `files://`, other
   extensions' namespaces) through ordinary WASI file APIs.

## Verb contract principles (locked)

These govern the verb family shape. Do not violate them:

- Prefer **flat scalar arguments** over nested structures.
- Prefer **homogeneous operations** over unions, optional modes, or "do one of several
  things" arguments.
- **Push batching/vectorization into the tool implementation** rather than asking the
  model to serialize batches.
- Avoid clever encodings merely to save schema space. **"Boring" shapes are closer to
  model training.**
- **Minimize the number of syntactically exact decisions** the model must make.
- **Make parsers tolerant and validators strict.** The runtime should recover from
  predictable model mistakes.
- Model-facing data is **TOON, not JSON**.
