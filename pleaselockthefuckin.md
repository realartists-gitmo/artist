The governing invariant is: **the kernel routes, hosts, and enforces contracts; behavior is implemented by components.** A subsystem may have a host-side socket/contract, but the implementation behind that socket is a component selected through the URL/component system.

Anything below that sounds like “put compaction/provider/profile/process policy into the kernel” should be treated as a bug in the checklist.

## Architectural invariants

* [ ] Keep the kernel limited to primitives: URI parsing/routing, namespace registration, resource contracts, component loading/execution, generation pinning/lifecycle, and the minimum host plumbing required to run those things.
* [ ] Do not add feature implementations to the kernel because they are broadly useful.
* [ ] Represent swappable behavior as component-backed URL subsystems.
* [ ] Treat `url://...` as the registration/discovery mechanism for these subsystems, with concrete components implementing the corresponding namespace.
* [ ] Compaction is a component socket, not an `AgentEngine`/kernel compactor.
* [ ] Providers and provider authentication/configuration are component-backed.
* [ ] Profiles/prompts are component-backed.
* [ ] Model-facing tools are component-provided contracts.
* [ ] Processes used by `run`/`poll`/`signal` are component-owned resources, not kernel process management.
* [ ] Future commands follow the same pattern, e.g. a command socket/namespace with command extensions behind it. Do not build a central command switch into the kernel.
* [ ] Do not resurrect an old crate/API merely because Gortnite had it.
* [ ] Do not add `read_many`.
* [ ] Do not add separate model-facing `send`, `stop`, or `delete` agent tools.
* [ ] Do not add a generic pagination/cursor/recovery framework.
* [ ] Do not add AST tools yet.
* [ ] Do not add durable memory.
* [ ] Do not add rules/TTSR.
* [ ] Do not add ask-user.
* [ ] Do not add MCP.
* [ ] Do not add computer-use.
* [ ] Do not add Muse.
* [ ] Do not add durable shells/process resurrection yet.

## Current correctness holes

* [ ] Fix context projection so `ContextEvent::Replace` actually replaces the old contribution in the next model request.
* [ ] Fix context projection so `ContextEvent::Remove` actually removes the contribution from the next model request.
* [ ] Stop incrementally accumulating old system/context messages inside the provider message vector.
* [ ] Before every provider request, rebuild the active context prefix from the current `ContextController` snapshot, then append the durable conversation portion.
* [ ] Ensure applying the same composition update twice cannot silently duplicate model context.
* [ ] Allow only one active turn per session unless the session has explicitly forked work.
* [ ] Allow different sessions to execute concurrently.
* [ ] Make cancellation able to interrupt an in-flight provider request instead of merely setting a flag checked afterward.
* [ ] Make component/tool invocation cancellation propagate into the component runtime where supported.
* [ ] Check steering/stdin before accepting a provider response as final.
* [ ] Fix the current case where a no-tool final response can finish before queued steering is observed.
* [ ] Persist accepted steering/stdin input as part of the session history.
* [ ] Preserve provider usage information across every model call in a turn rather than losing intermediate usage.
* [ ] Do not silently discard failure to persist a turn-aborted event.
* [ ] Once a session/transcript is closed, do not reopen the same durable log for active writes.
* [ ] Validate event-log schema versions while reopening.
* [ ] Reject or explicitly migrate unsupported event versions instead of replaying them as though they were current.
* [ ] Preserve structured component/tool errors instead of flattening them to static strings.
* [ ] Fix WASM/component `Unavailable` so it remains `Unavailable`; do not convert it to `NotFound`.

## Daemon and session execution

The daemon is a host/orchestrator. It should assemble components and drive sessions. It should not become the implementation site for every capability.

* [ ] Give each open session a live runtime object containing its durable event log and the component/resource handles needed to run it.
* [ ] Resolve the active provider through the component/provider subsystem.
* [ ] Resolve the active profile/prompt composition through components.
* [ ] Resolve the active tool surface through component exports.
* [ ] Resolve the optional compaction component through the compaction socket.
* [ ] Make `session/open` actually restore enough runtime state to execute another turn.
* [ ] Add the daemon operation that starts a model turn for an open session.
* [ ] Emit live session events while that turn runs rather than making the connection wait for one final monolithic response.
* [ ] Emit text deltas, reasoning deltas when supported, tool calls, tool results, usage, completion, abort, and error events.
* [ ] Give emitted session events monotonically increasing sequence numbers.
* [ ] Allow a client to subscribe/reconnect from a known sequence number.
* [ ] Do not block the request-reading loop on the full model turn.
* [ ] Keep the active turn in its own task so stdin writes and signals can be processed while the turn is running.
* [ ] Use generic resource/tool semantics for runtime control where possible.
* [ ] Steering to an agent/session is a `write` to its stdin resource, not an `agent/send` API.
* [ ] Cancellation/stopping is expressed through `signal`, not a separate `agent/stop` tool.
* [ ] Agent retirement/removal uses generic resource mutation semantics rather than an `agent/delete` tool.
* [ ] Expose current session state sufficiently for clients to determine idle/running/completed/failed/closed.
* [ ] Persist enough metadata to reopen the session without guessing its provider, profile, identity, or current lifecycle state.

## Conversation replay and resume

The event log is authoritative. Reopening a session must rebuild the model conversation that would have existed if the daemon never stopped.

* [ ] Record every accepted user input.
* [ ] Record every provider assistant output required for subsequent context.
* [ ] Record model tool calls including their call IDs.
* [ ] Record tool results associated with those exact calls.
* [ ] Record context composition mutations.
* [ ] Record profile changes and handoffs.
* [ ] Record yield/fork lifecycle events that alter execution state.
* [ ] Reconstruct the provider-neutral conversation from the durable event stream.
* [ ] Reconstruct the active composed context separately from the durable conversational transcript.
* [ ] Reconstruct the active tool surface/profile state.
* [ ] Never rerun a tool call that has a durable completed result.
* [ ] If the daemon died after recording a tool call but before recording a result, treat that invocation as interrupted rather than blindly rerunning the mutation.
* [ ] Persist explicit turn boundaries.
* [ ] On reopen, distinguish a cleanly completed turn from an interrupted turn.
* [ ] Make interrupted-turn recovery deterministic.
* [ ] Preserve image/resource references during replay.
* [ ] Preserve enough provider-specific continuation metadata when a provider genuinely requires it, but keep the provider-neutral event log authoritative.
* [ ] Add replay tests that run a conversation, close the daemon, reopen it, and verify that the next provider request sees the same logical conversation.

## Compaction socket

Do not implement a local compaction policy.

* [ ] Remove the current hand-written `IdentityCompactor` behavior as the architecture for production compaction.
* [ ] Define a minimal compaction component contract/socket.
* [ ] Register/discover that socket through the URL/component system, conceptually along the `url://compaction` / `compaction://...` pattern.
* [ ] Allow zero compaction components to be installed.
* [ ] Allow a session/profile to select a compaction implementation by component/resource identity when one exists.
* [ ] Give a selected compaction component the provider-neutral context and the minimum model/context-limit metadata it needs.
* [ ] Let the component return the compacted provider-neutral context.
* [ ] Do not encode preservation heuristics, tool-call heuristics, summary policy, memory policy, or token-selection rules in the host.
* [ ] If no compaction component is selected/available, allow the upstream provider implementation to use its own native compaction/context-management capability.
* [ ] If neither a component nor the provider can compact and the context is too large, return a clear context-limit failure.
* [ ] Persist the fact/result of host-visible compaction sufficiently for replay.
* [ ] Leave the actual library of compaction components for later.

## Provider configuration and authentication

This is component functionality.

* [ ] Define the minimal provider component contract required by the session host.
* [ ] Keep provider-neutral request/response/event types in shared contracts.
* [ ] Move provider-specific configuration interpretation behind provider components.
* [ ] Move provider-specific authentication behavior behind provider components.
* [ ] Support stable provider/configuration IDs rather than resolving solely by display name.
* [ ] Support multiple configured accounts/endpoints for the same provider type.
* [ ] Allow profiles/sessions to refer to a provider configuration by stable identity.
* [ ] Keep API keys/tokens out of session event logs.
* [ ] Keep secrets out of `Debug`, diagnostics, tracing, and tool-visible errors.
* [ ] Support OpenAI API-key authentication.
* [ ] Restore ChatGPT authentication/OAuth support through the appropriate provider component rather than making OAuth a daemon-global special case.
* [ ] Support refresh/re-authentication inside the component responsible for that authentication scheme.
* [ ] Distinguish authentication failure from provider/API failure.
* [ ] Allow provider configuration to carry provider-specific endpoint/model options without polluting the universal session schema with every provider's fields.

## Provider catalog

Each provider is an implementation of the provider component contract, not another kernel branch.

* [ ] Restore Anthropic.
* [ ] Restore Azure OpenAI.
* [ ] Restore ChatGPT.
* [ ] Restore Cohere.
* [ ] Restore GitHub Copilot.
* [ ] Restore DeepSeek.
* [ ] Restore Gemini.
* [ ] Restore Groq.
* [ ] Restore Hugging Face.
* [ ] Restore Hyperbolic.
* [ ] Restore Llamafile.
* [ ] Restore MiniMax.
* [ ] Restore Mira.
* [ ] Restore Mistral.
* [ ] Restore Moonshot.
* [ ] Restore Ollama.
* [ ] Restore OpenAI.
* [ ] Restore OpenRouter.
* [ ] Restore Perplexity.
* [ ] Restore Together.
* [ ] Restore xAI.
* [ ] Restore Xiaomi/MiMo.
* [ ] Restore Z.AI.
* [ ] Make every provider report capabilities truthfully.
* [ ] Give providers a shared conformance test suite for text generation, streaming where claimed, tool calling where claimed, cancellation where claimed, usage reporting, and error mapping.

## OpenAI component

* [ ] Implement actual network streaming.
* [ ] Emit text deltas as they arrive.
* [ ] Emit reasoning events when the API/model provides them.
* [ ] Emit tool-call deltas and completed tool calls correctly.
* [ ] Capture usage from the actual API response.
* [ ] Make cancellation abort the HTTP request.
* [ ] Encode assistant tool calls correctly when continuing a conversation.
* [ ] Encode tool results using native OpenAI tool-output items with the correct call ID.
* [ ] Support custom-tool output correctly if the provider contract exposes custom tools.
* [ ] Preserve image inputs.
* [ ] Preserve resource/provider-extension inputs that OpenAI actually supports.
* [ ] Reject malformed tool calls rather than fabricating empty IDs or names.
* [ ] Parse refusals/incomplete responses explicitly.
* [ ] Map HTTP/provider failures into meaningful provider errors.
* [ ] Honor retry/rate-limit information supplied by OpenAI without creating a universal retry-policy framework.
* [ ] Expose structured output only when the shared request contract can actually request it.
* [ ] Set capability flags from implemented behavior, not intended future behavior.

## Minimal model-facing tool result contract

Do not rebuild `artist-tool-api`.

* [ ] Define one small provider-neutral result envelope for model-facing tool execution.
* [ ] Represent success explicitly.
* [ ] Carry the tool's actual structured/text output on success.
* [ ] Represent failure explicitly.
* [ ] Give failures a stable machine-readable code.
* [ ] Give failures a useful human/model-readable message.
* [ ] Allow a tool to attach tool-specific structured details when needed, such as stale TECA information.
* [ ] Do not add universal `NextAction`, page metadata, recovery-operation objects, category taxonomies, or giant annotation structures unless a concrete current consumer requires them.
* [ ] Preserve the original diagnostic message across component → host → model boundaries.
* [ ] Keep provider formatting separate: provider components translate this neutral result into OpenAI/Anthropic/etc. tool-output syntax.

## Base verb component

The model-facing base surface is:

`read`, `write`, `move`, `edit`, `find`, `grep`, `run`, `poll`, `signal`.

Nothing else gets added just because an older branch had it.

### `read`

* [ ] Read through resource URIs.
* [ ] Support bounded reads.
* [ ] Do not load an arbitrarily large resource into memory merely to return a small range.
* [ ] Return enough metadata to tell the caller whether the returned content is partial.
* [ ] Handle non-text resources explicitly instead of silently pretending everything is UTF-8.
* [ ] Return TECA addresses for editable text.
* [ ] Treat line numbers as presentation information, not stable edit identity.

### `write`

* [ ] Write through resource URIs.
* [ ] Make whole-resource replacement actually remove old trailing bytes.
* [ ] Do not ignore truncation/set-size failures.
* [ ] Make replacement atomic where the backing resource provider can support it.
* [ ] Return whether a resource was created or replaced.
* [ ] Preserve structured provider/resource errors.

### `move`

* [ ] Implement resource movement/rename through the resource provider.
* [ ] Preserve normal destination-directory semantics.
* [ ] Use generic move/removal semantics for agent/resource retirement rather than inventing a model-facing `delete` verb.
* [ ] Do not create a second delete API specifically for agents.

### `edit`

* [ ] Use TECA addresses.
* [ ] Resolve every edit in a batch against one immutable pre-edit snapshot.
* [ ] Validate every referenced address before performing any mutation.
* [ ] Reject stale addresses.
* [ ] Reject ambiguous addresses.
* [ ] Reject incompatible overlapping edits.
* [ ] Apply the validated batch atomically.
* [ ] Stop applying edit 2 against text already changed by edit 1.
* [ ] Do not ignore truncate/set-size failures.
* [ ] Return updated TECA information after a successful edit.

### `find`

* [ ] Search resources, not assumed local filesystem paths.
* [ ] Support the intended literal/glob/fuzzy modes from the verb contract.
* [ ] Return resource URIs.
* [ ] Bound traversal and returned result count.
* [ ] Report truncation when a configured result bound is hit.
* [ ] Avoid recursive traversal cycles.
* [ ] Do not build a generic cursor/pagination subsystem for this.

### `grep`

* [ ] Search through readable resource providers.
* [ ] Support literal and regex matching required by the contract.
* [ ] Return resource URI plus match information and TECA address where applicable.
* [ ] Bound traversal and result count.
* [ ] Report truncation when the bound is hit.
* [ ] Do not silently swallow every unreadable/binary-resource error.
* [ ] Do not build generic pagination infrastructure.

### `run`

* [ ] Implement `run` as a component-backed verb.
* [ ] Start a process through a component-owned process resource subsystem.
* [ ] Return a process/resource handle usable by `poll` and `signal`.
* [ ] Expose stdin/stdout/stderr through resource semantics where appropriate.
* [ ] Support command, arguments, environment, and working resource/directory as required by the contract.
* [ ] Keep this process state ephemeral.
* [ ] Do not persist shells.
* [ ] Do not attempt process resurrection after daemon restart.
* [ ] Do not create a durable machine-wide process registry.

### `poll`

* [ ] Operate on the resource/handle returned by `run`.
* [ ] Return current process state.
* [ ] Return newly available output without requiring the process to have exited.
* [ ] Support a bounded wait when requested.
* [ ] Treat a poll timeout as "no state change yet", not necessarily process failure.

### `signal`

* [ ] Operate on the same process/agent resource model.
* [ ] Support the signal operations the backing component genuinely implements.
* [ ] Return `Unsupported` for signals the host platform/component cannot provide.
* [ ] Use this mechanism for stopping/cancelling agents rather than adding `agent/stop`.

## TECA

* [ ] Make TECA the stable addressing mechanism used by `read`, `edit`, and matching tools where appropriate.
* [ ] Derive addresses deterministically from one resource snapshot.
* [ ] Keep address identity independent of line number.
* [ ] Detect stale addresses after incompatible edits.
* [ ] Preserve descendant identity across an ancestor declaration rename where the semantic target is otherwise unchanged.
* [ ] Rank equivalent siblings within the correct semantic parent, not globally across the file.
* [ ] Make batch edits resolve all addresses against the same original snapshot.
* [ ] Keep the anchor implementation stateless with respect to "what this model previously read."
* [ ] Isolate the exact byte serialization/hash input used to construct the TECA address behind one tiny function.
* [ ] Put an explicit `TODO` on that byte representation.
* [ ] Do not treat the temporary byte format as a stable public contract.
* [ ] Do not copy the old hashline implementation blindly.

## Profiles, prompts, and live composition

* [ ] Resolve profiles through components/resources rather than hard-coded `AgentEngine` branches.
* [ ] Allow a profile to provide post-system instructions.
* [ ] Allow a profile to provide identity instructions.
* [ ] Allow a profile to select/default provider and model.
* [ ] Allow a profile to define the permitted model-facing tool surface.
* [ ] Allow a profile to define required tools.
* [ ] Allow a profile to define its `yield` contract.
* [ ] Allow fork/subagent profiles to expose a different `yield` schema from the default profile.
* [ ] Recompute the model-facing tool definitions when the active profile changes.
* [ ] Keep shared/global prompt composition separate from profile-specific post-system/identity material.
* [ ] Discover project/local composition components through the normal extension mechanism.
* [ ] Start the live composition watcher in the actual daemon/session runtime.
* [ ] Apply valid component/composition changes between provider requests.
* [ ] If a live update is invalid, retain the last valid component generation/composition.
* [ ] Persist enough profile/component identity in session events to explain/replay profile transitions.

## `yield` tool

`yield` is harness control, but its model-facing contract comes from the active profile.

Default contract:

```text
complete: true | false
remaining?: string
```

* [ ] Generate the `yield` tool schema dynamically from the active profile.
* [ ] Use the default `complete`/`remaining` contract when the profile does not override it.
* [ ] Record every yield durably.
* [ ] Do not use another LLM inference pass to decide whether a goal/fork/subagent is finished.
* [ ] If `complete: false`, keep the execution alive and tell the model to continue.
* [ ] If `remaining` is supplied, preserve it as the model's own description of unfinished work.
* [ ] If `yield` is used outside explicit goal mode, still honor the reported completion state rather than treating the call as invalid.
* [ ] A premature `complete: false` outside goal mode should simply cause the harness to continue.
* [ ] Profile-specific yield outputs may carry additional structured completion data required by that profile.

## `fork(list<tasks>)`

* [ ] Add `fork` as a harness-provided model-facing tool where the active profile permits it.
* [ ] Accept a list of mostly independent task briefs.
* [ ] Capture the caller's model context once at fork time.
* [ ] Reuse that identical captured context as the ingress for every fork.
* [ ] Append only each fork's individual task to its copied execution context.
* [ ] Keep all forks in the same workspace/resource universe.
* [ ] Give each fork an independent execution stream/session identity sufficient for logging and control.
* [ ] Let each fork run concurrently.
* [ ] Require each fork to terminate through its profile's `yield` contract.
* [ ] Do not make the parent model repeatedly inspect fork transcripts to infer whether they are done.
* [ ] Notify/wake the parent execution when all forks have yielded completion or terminal failure.
* [ ] Make the completed fork outputs available to the parent in structured form.
* [ ] Reject `handoff` from inside a fork.
* [ ] Use ordinary `write`, `read`, `signal`, and resource operations for controlling fork executions; do not create `fork/send`, `fork/stop`, etc.

## `handoff(profile, brief)`

* [ ] Add `handoff` as a harness-provided model-facing tool where permitted.
* [ ] Preserve the same agent identity.
* [ ] Record the handoff durably.
* [ ] Switch the active profile.
* [ ] Recompute permissions/tool surface from the new profile.
* [ ] Recompute provider/model defaults when the new profile changes them.
* [ ] Replace the active post-system/profile/identity instructions with those of the new profile.
* [ ] Reset active model conversation context to zero.
* [ ] Drop provider-private continuation lineage that would violate that reset.
* [ ] Append the supplied `brief` as the new context ingress.
* [ ] Do not delete the old durable transcript; context reset and transcript erasure are different things.
* [ ] Reject handoff when executing as a fork.
* [ ] Reject handoff when executing as a subagent whose parent is waiting for that execution's specific yield contract.

## Agent identity and lifecycle

* [ ] Keep stable agent IDs.
* [ ] Represent agents through their resource namespace, including stdin/stdout/transcript as applicable.
* [ ] Expose lifecycle/status through resource attributes or the component contract rather than a parallel collection of ad-hoc tools.
* [ ] Write to `agent://<id>/stdin` to communicate with a running agent.
* [ ] Read its output/transcript through resource reads.
* [ ] Use `signal` to interrupt/terminate it.
* [ ] Use generic resource movement/removal semantics to retire it.
* [ ] Do not add `send`.
* [ ] Do not add `stop`.
* [ ] Do not add `delete`.
* [ ] Keep completed historical transcripts readable.
* [ ] Do not permit writes back into a closed historical transcript.
* [ ] Track parent/fork relationships only to the extent required to implement fork completion and lifecycle correctly.
* [ ] Do not rebuild the old machine-wide mailboxes/roster architecture just for parity.

## Extension/component system

* [ ] Make extension packages the delivery mechanism for capability implementations.
* [ ] Validate declared component roles against their actual WIT exports/imports.
* [ ] Validate namespace/resource claims before activation.
* [ ] Reject conflicting namespace claims deterministically.
* [ ] Keep component ABI/version compatibility explicit.
* [ ] Finish source-component compilation/build support separately from runtime execution.
* [ ] Cache successful component builds by their actual inputs.
* [ ] Return compiler/component diagnostics as structured errors.
* [ ] Prepare and validate a new component generation completely before activating it.
* [ ] Keep the previous generation active if replacement preparation fails.
* [ ] Pin in-flight calls to the generation they started on.
* [ ] Retire an old generation only after its in-flight pins are gone.
* [ ] Start event-role components when their generation activates.
* [ ] Stop/clean up event-role components when their generation retires.
* [ ] Ensure component background work is cancelled during retirement.
* [ ] Apply reasonable execution limits to untrusted WASM components.
* [ ] Make extension deletion/removal actually remove its exported tools/resources on the next valid generation.
* [ ] Clarify which root component/package owns application bootstrap so that ownership is not split between hard-coded daemon behavior and extension behavior.

## Image and resource handling

Do this through existing resource/component semantics; do not invent a giant artifact framework.

* [ ] Preserve provider-neutral image message parts instead of dropping them.
* [ ] Preserve provider-neutral resource references instead of dropping them.
* [ ] Record image/resource URIs in the durable conversation so replay survives restart.
* [ ] Resolve referenced bytes through the resource provider/component at provider-call time.
* [ ] Preserve MIME/type information needed by provider adapters.
* [ ] Encode OpenAI image inputs using the format the OpenAI API actually expects.
* [ ] Let other provider components implement their own supported image transport.
* [ ] Reject image input cleanly when the selected provider does not support it.
* [ ] Do not put enormous base64 blobs directly into ordinary session events when the content already exists as a resource.
* [ ] When a component/tool produces binary/image output, prefer returning a resource URI plus metadata to the model-facing layer.
* [ ] Do not create a separate durable-memory/artifact knowledge subsystem as part of this work.

## Reconnectable session host

Implement the useful behavior, not the old Gortnite crate verbatim.

* [ ] Give each live session a host-owned execution lifetime independent of one client connection.
* [ ] Do not kill a running turn merely because its client disconnects.
* [ ] Journal emitted session events with monotonically increasing sequence numbers.
* [ ] Let a reconnecting client provide the last sequence it received.
* [ ] Replay missed events from that point.
* [ ] Expose current runtime phase after reconnect.
* [ ] Allow stdin writes and signals after reconnect.
* [ ] Prevent two reconnecting clients from accidentally starting the same prompt twice.
* [ ] Give prompt-start operations stable request IDs sufficient for deduplication.
* [ ] Keep this mechanism about connection/session continuity; do not turn it into a second capability framework beside components.

## Observability

* [ ] Emit structured turn-started/completed/aborted events.
* [ ] Record provider latency.
* [ ] Record tool/component latency.
* [ ] Record provider failures by provider/error class.
* [ ] Record component/tool failures by component/tool/error class.
* [ ] Record token/usage data when providers supply it.
* [ ] Record cancellations.
* [ ] Record component generation activation/retirement/failure.
* [ ] Carry session ID, turn ID, provider-call ID, and tool-call ID through diagnostic events.
* [ ] Automatically redact known credentials/tokens from diagnostics.
* [ ] Ensure component/provider error formatting cannot accidentally dump auth headers.
* [ ] Keep observability non-authoritative; session/event storage remains the source of truth.
* [ ] Route pluggable observability sinks through components/events rather than baking every exporter into the daemon.

## Durable state

* [ ] Version durable workspace/session metadata.
* [ ] Version event records.
* [ ] Add explicit migrations when a durable schema changes.
* [ ] Reject unsupported future versions.
* [ ] Make workspace/session registry writes crash-safe.
* [ ] Make registry replacement safe on Windows as well as Unix.
* [ ] Use appropriate cross-process locking so two daemons cannot corrupt the same state root.
* [ ] Flush durable registry/session changes sufficiently to survive normal crash/restart scenarios.
* [ ] Recover an incomplete final JSONL/event write without discarding valid preceding events.
* [ ] Treat corruption in the middle of an event log as corruption, not as an ordinary truncated tail.
* [ ] Preserve stable session IDs across restart.
* [ ] Preserve active profile/provider/component references required to resume.
* [ ] Preserve incomplete-turn state explicitly.
* [ ] Test crash points around user-event write, provider completion, tool-call write, tool-result write, yield, fork completion, and handoff.

## Hardening

* [ ] Run formatting across the actual workspace rather than an arbitrary subset.
* [ ] Run `cargo check` for the full workspace.
* [ ] Run Clippy with warnings treated as failures for maintained crates.
* [ ] Run the full test workspace on Linux, macOS, and Windows.
* [ ] Build and validate the WASM components used by the real runtime in CI.
* [ ] Add an end-to-end fake-provider test for user → model → final response.
* [ ] Add an end-to-end fake-provider test for user → model tool call → component tool → model continuation → completion.
* [ ] Add a restart/replay end-to-end test.
* [ ] Add cancellation while an HTTP provider request is active.
* [ ] Add cancellation while a component tool call is active.
* [ ] Add stdin/steering arriving just before a no-tool final response.
* [ ] Add concurrent independent sessions.
* [ ] Add fork concurrency and parent wake-up tests.
* [ ] Add handoff context-reset tests.
* [ ] Add extension hot-replacement tests with calls pinned to the retiring generation.
* [ ] Add invalid-extension-update tests proving the previous generation stays active.
* [ ] Add OpenAI request/response fixtures covering tool calls, tool results, images, reasoning, refusal/incomplete responses, and malformed tool calls.
* [ ] Add TECA tests for stale detection, same-snapshot batch edits, ancestor rename stability, equivalent-sibling identity, and overlap rejection.
* [ ] Add filesystem/resource tests for truncation failures and shorter replacement writes.
* [ ] Add Windows tests for the state/locking paths that currently rely on Unix-like assumptions.

The things intentionally absent from this architecture are just as important: **no `read_many`, no AST layer, no durable memory, no rules engine, no ask-user, no MCP server, no computer-use, no Muse, no durable process resurrection, no generic pagination framework, and no restoration of old agent-specific `send`/`stop`/`delete` verbs.**
