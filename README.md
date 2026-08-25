# Artist

Artist is a small persistent, streaming agent harness. Rig owns model execution; the Artist kernel owns the durable session protocol; Wasmtime components supply behavior that should vary.

Artist deliberately provides no permission, security, sandbox, confinement, or
isolation boundary. Resource scopes organize routing; they do not restrict
access. Deployments that need a boundary must provide it outside Artist.

## Crates

- `artist-core`: stable IDs, commands, stream events, and append-only transcript types
- `artist-store`: in-memory and JSONL session stores behind one narrow trait
- `artist-kernel`: the single-owner session actor, streaming state machine, and transcript projection
- `artist-rig`: thin Rig streaming adapter and `rig-memory` policies
- `artist-resource`: canonical URI tree, deterministic routing, platform mount adapters, FFF search, durable scoped storage (`store:///`), content-addressed blobs with roots+leases+GC
- `artist-plugin`: versioned WIT component host and ordered capability chains; per-session plugin-event sinks commit emissions durably before the emitter is released
- `artist-provider`: provider/account contracts, credential boundaries, provider-private state, and the reusable driver conformance battery (`artist_provider::conformance`)
- `artist-runtime`: host-managed session registry (create/resume/steer/replay/cancel), explicit attachment + recovery policies, idempotent creates, graceful shutdown; production sessions resolve models through a `ModelProviderSource` via `SessionRuntime::from_provider_source`
- `artist-observe`: backend-neutral observations projected from public stream events
- `plugins/{prompt,context,hooks,model,events}`: one lifecycle capability per WASM component
- `plugins/file-{read,children,write,edit,move}`: one `file:///**` operation per WASM component
- `plugins/profiles-{read,children}`: one `profiles:///**` operation per WASM component
- `plugins/plugins-{read,children,write,edit,move,signal}`: one `plugins:///**` package operation per WASM component
- `plugins/tool-{read,find,grep,write,edit,move,run,signal,poll,yield,handoff}`: one model-facing tool per WASM component
- `plugins/notes`: reference domain plugin — persists state through `store:///global/...` (no `provider-state`) and emits a generic durable `artist.notes.added` event
- `plugins/sdk`: shared WIT boilerplate; this library is not a plugin component

The canonical transcript is the memory boundary. The kernel projects it into Rig messages for every request instead of letting Rig keep a second conversation log. This is intentional: Rig's automatic memory append only sees successful turns, while Artist must also preserve partial output and typed interruptions. `rig-memory` policies shape the projection without rewriting the transcript. The permanent record and physical-log contracts are specified in [`TRANSCRIPT_V1.md`](TRANSCRIPT_V1.md).

Initial prompt composition is also component-owned. The host supplies ordered
fragments to the WIT prompt socket of each component advertising that
capability, and freezes the composed result into the session record before the
first input. With no prompt component loaded, the host leaves fragments
untouched.

## Control semantics

- `input` queues a normal turn.
- `steer` queues a gentle notification. A Rig hook takes it immediately before the next model request; it never cancels the current request.
- `abort` cancels the active stream and appends its partial assistant content followed by a typed interruption.
- `yield` validates the active profile's result schema and ends the run with a typed payload.
- `handoff` snapshots another profile, starts a fresh projection epoch with the same identity, and queues its brief plus pending steering as the first input.

Replacement is explicitly `abort`, then `input`.

## Build and test

```sh
make test
```

This formats and lints the Rust workspace, builds every `wasm32-wasip2`
component, verifies that each advertises exactly one capability, invokes the
production sockets through Wasmtime, and runs deterministic tests.

With a usable native mount driver (FUSE, macFUSE, or WinFsp), the full filesystem projection and shared FFF
index smoke test is:

```sh
make fuse-test
```

Windows builds either place the matching WinFsp DLL beside the executable or
enable `artist-resource/winfsp-system` to discover a system installation.

An opt-in real-provider smoke test is available without becoming a product path:

```sh
OPENAI_API_KEY=... cargo run -p artist-rig --example openai -- <streaming-model-name>
```

## Stable contracts

- Canonical snapshots use `artist-core::RECORD_VERSION = 5` and deserialize only
  through the validating record reducer. Every other record version is rejected.
- File-backed sessions use `artist-store::FILE_FORMAT_VERSION = 2`: a frozen
  header followed by SHA-256-chained, atomic JSON batch frames. Every other
  physical format or embedded record version is rejected. Artist is
  pre-production: never add migration support; discard stale development data.
- The public command and stream protocol consists only of `artist-core` values.
- With locked Rig 0.42, `AgentRunner` stream errors are terminal; Artist
  preserves partial output and closes the run instead of expecting a later
  recovery item from that stream.
- The plugin ABI is `artist:plugin@0.8.0` in `wit/plugin.wit`; tool, resource,
  and slash-command providers are separate interoperable contracts. Slash
  commands have globally unique names, receive raw trailing arguments, return
  harness-facing output plus typed kernel actions, and are never profile-gated.
  The host has no native model-facing tools: every tool registers from its own
  WASM component into the shared registry below Rig.
- Telemetry is derived from stream events and is never required to load or resume a session.

The base resource-fabric contract lives in
[`URI_RESOURCE_FABRIC_PLAN.md`](URI_RESOURCE_FABRIC_PLAN.md). Kernel metadata,
execution-neutral control, and TECA line-anchor editing are specified in
[`RESOURCE_CAPABILITIES_IMPLEMENTATION_PLAN.md`](RESOURCE_CAPABILITIES_IMPLEMENTATION_PLAN.md).
Profile layout, policy, yield, handoff, and model routing are specified in
[`PROFILES.md`](PROFILES.md).
Inspectable source packages, candidate builds, validation, and live activation
are specified in [`PLUGINS.md`](PLUGINS.md).
