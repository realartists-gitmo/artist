# Artist

Artist is a small persistent, streaming agent harness. Rig owns model execution; the Artist kernel owns the durable session protocol; Wasmtime components supply behavior that should vary.

## Crates

- `artist-core`: stable IDs, commands, stream events, and append-only transcript types
- `artist-store`: in-memory and JSONL session stores behind one narrow trait
- `artist-kernel`: the single-owner session actor, streaming state machine, and transcript projection
- `artist-rig`: thin Rig streaming adapter and `rig-memory` policies
- `artist-resource`: canonical URI tree, deterministic routing, universal tools, FUSE projection, and FFF search
- `artist-plugin`: versioned WIT component host and ordered capability chains
- `artist-observe`: projection of public stream events into `rig-tap`
- `plugins/default`: lifecycle defaults and the terminal `file:///**` resource provider
- `plugins/ast-fixture`: Rust `?symbols/...` projections backed by the shared tool bridge
- `plugins/tool-fixture`: cross-component tool-call and recursion smoke fixture

The canonical transcript is the memory boundary. The kernel projects it into Rig messages for every request instead of letting Rig keep a second conversation log. This is intentional: Rig's automatic memory append only sees successful turns, while Artist must also preserve partial output and typed interruptions. `rig-memory` policies shape the projection without rewriting the transcript.

## Control semantics

- `input` queues a normal turn.
- `steer` queues a gentle notification. A Rig hook takes it immediately before the next model request; it never cancels the current request.
- `abort` cancels the active stream and appends its partial assistant content followed by a typed interruption.

Replacement is explicitly `abort`, then `input`.

## Build and test

```sh
make test
```

This formats and lints the Rust workspace, builds the default `wasm32-wasip2` component, invokes every WIT socket through Wasmtime, and runs deterministic tests.

On Linux with a usable `/dev/fuse`, the full Unix projection and shared FFF
index smoke test is:

```sh
make fuse-test
```

An opt-in real-provider smoke test is available without becoming a product path:

```sh
OPENAI_API_KEY=... cargo run -p artist-rig --example openai -- <streaming-model-name>
```

## Stable contracts

- The JSONL header and transcript entries use `artist-core::RECORD_VERSION`.
- The public command and stream protocol consists only of `artist-core` values.
- The plugin ABI is `artist:plugin@0.3.0` in `wit/plugin.wit`; tool and resource providers are separate interoperable contracts.
- Telemetry is derived from stream events and is never required to load or resume a session.

The complete resource-fabric contract and verification checklist live in
[`URI_RESOURCE_FABRIC_PLAN.md`](URI_RESOURCE_FABRIC_PLAN.md).
