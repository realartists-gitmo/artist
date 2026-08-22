# Artist build plan

## Implementation status

Slices 1–8 are implemented. Slices 9–10 remain intentionally open.

Prompt composition is component-owned: the host passes ordered fragments
through only the components advertising the WIT `prompt` capability, then the
resulting `InitialContext` is frozen into the canonical session record. The
host itself does not interpret or compose prompt content.

## Direction

Artist is a persistent, streaming agent harness built on Rig. The kernel stays small. Everything that should vary is exposed through a plugin socket.

We will build horizontally: establish one architectural layer at a time across the whole system, then move upward. We are not trying to prove a polished coding-agent workflow early.

The three control primitives are deliberately separate:

- **Input** adds a normal message for the model to answer.
- **Steer** adds a gentle notification to the next model request. It never stops the current request. If the current run finishes before another model request, the notification remains queued for the next run.
- **Abort** stops the active request. The session records the partial assistant content followed by a typed interruption event.

Replacing an active request is therefore two explicit operations: **abort, then input**.

## Slice 1: Shared language

Define the small set of types every layer uses:

- session, run, message, and event IDs
- input, steer, and abort commands
- streamed output events
- transcript entries
- completed, failed, and interrupted run outcomes
- typed interruption causes
- plugin identity and capability descriptions

Write down the state transitions and serialization format. Keep provider, Rig, Wasmtime, storage, and transport types out of these core types.

**Done when:** the types serialize, deserialize, and reject invalid state transitions in unit tests.

## Slice 2: Canonical session record

Build the append-only record of a session.

- Compose initial context once and freeze it before the first input.
- Append normal inputs, assistant messages, tool activity, interruption events, and compaction artifacts.
- Never edit earlier entries.
- Store partial assistant content before its interruption event.
- Give every entry a monotonic sequence number.

Start with a simple file-backed store plus an in-memory implementation for tests. Storage should expose a narrow trait so its format does not leak into the kernel. + BIG AGREE, GOOD CATCH!

**Done when:** a session can be saved, loaded, and projected back to the same ordered transcript. Uh... how is this the case when we just agreed that storage serializtion should not leak into the kernel???

## Slice 3: Streaming run state machine

Build the model-independent run loop against a fake streaming model.

- Accept one input and start one run.
- Forward text and tool-call deltas as typed events.
- Accumulate assistant text while it streams.
- Persist the final assistant message on completion.
- Persist partial text and a typed interruption on abort.
- Allow only one active run per session.

This slice establishes behavior without a network call or real model.

**Done when:** deterministic tests cover completion, failure, and interruption at several points in a stream.

## Slice 4: Live session control

Put each active session behind a single command queue.

- Input starts a normal turn when the session is ready; input received while busy waits as the next normal turn.
- Steer queues a notification for the next model-request boundary.
- Steering does not cancel, truncate, or restart anything.
- Abort cancels the active request and records what happened.
- Abort followed by input performs an explicit restart with new instructions.
- User commands and harness-generated commands use the same API.

**Done when:** concurrency tests prove command ordering, gentle steering, explicit abort-plus-input, and single-run ownership.

## Slice 5: Rig adapter

Connect the state machine to Rig while keeping Rig behind a thin adapter.

- Use Rig's streaming runner only; do not add a single-shot path.
- Translate the canonical transcript into Rig messages.
- Translate Rig deltas, tool calls, tool results, errors, and usage into core events.
- Deliver queued steering immediately before the next Rig model request.
- Connect cancellation to abort.
- Keep the fake model for fast kernel tests.

Add one real provider adapter as a development example, not as a product feature.

**Done when:** the same kernel tests pass through the Rig adapter, with a small opt-in live smoke test.

## Slice 6: Context and memory

Separate immutable history from the context sent to a model.

- Implement Rig's conversation-memory boundary over the session store.
- Treat model context as a projection of the canonical transcript.
- Offer `rig-memory`'s no-op, sliding-window, token-window, and default compaction policies.
- Record compaction as a new artifact instead of rewriting history.
- Preserve tool-call and tool-result pairing in every projection.
- Define the adapter point for a plugin-provided context policy.

Normal turns must retain a stable prompt prefix. Prefix changes are allowed only when context limits require compaction.

**Done when:** each policy produces valid context while the canonical session record remains byte-for-byte unchanged.

## Slice 7: WIT component contract

Define one versioned WIT world for plugins, with focused sockets for:

- prompt composition
- tools
- context handling and compaction
- hooks
- model/provider configuration
- event and telemetry consumers

Use plain data at the boundary. Do not expose Rust, Rig, or Wasmtime implementation details. Include capability discovery so a plugin only participates in sockets it implements.

**Done when:** example identity/no-op components compile against the contract and the contract can evolve additively.

## Slice 8: Wasmtime plugin host

Build the smallest runtime that can load components and call the WIT sockets.

- Discover and register plugins.
- Route each socket through an ordered plugin chain.
- Convert between WIT values and core values in one place.
- Report plugin failures as ordinary typed harness errors.
- Keep plugin lifecycle and configuration simple.

Do not add permissions, sandbox policy, security machinery, process isolation, or worktree isolation. EVER

**Done when:** each socket can invoke a tiny fixture component independently. The sockets do not yet need to form a complete agent workflow.

## Slice 9: Default coding plugins

Move useful behavior out of the host and into a minimal default plugin set.

- compose `SYSTEM.md`, `AGENTS.md`, and other configured context
- expose a small coding-tool catalog
- provide the default context policy
- provide basic lifecycle hooks

Keep each plugin narrow. The host should only coordinate plugins, not duplicate their behavior.

**Done when:** removing a default plugin cleanly removes only its advertised capability.

## Slice 10: Unified observability and hardening

Connect kernel, Rig, memory, and plugin activity to Artist-owned observation sinks.

- Preserve session, run, tool-call, and provider correlation IDs.
- Emit timing, token usage, failures, interruptions, compactions, and plugin calls.
- Keep telemetry separate from the durable transcript and public stream protocol.
- Add replay, malformed-data, cancellation-race, and compatibility tests.
- Document the stable public contracts.

**Done when:** a recorded session can explain what happened without telemetry being required to restore or continue it.

## Deferred throughout

- frontend and UI
- polished end-to-end product flows
- broad provider support
- large tool catalogs
- plugin hot reload
- permissions, sandboxing, security, and isolation
- speculative abstractions without a current socket or caller
