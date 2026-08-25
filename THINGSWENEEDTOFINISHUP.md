There are several things nukeme needs to solve first or the same architectural pressure will start accumulating again.

Plugin lifecycle must actually participate in execution. Currently prompt/context/hooks/model/events exist in WIT, but the normal SessionHandle -> RigModel path doesn't run them. Tools/resources are the only plugin capabilities that are truly integrated end-to-end. The ABI already advertises all eight capability kinds, but most current implementations are effectively placeholders.

This is the first blocker. TTSR, memory injection, provider transformation, lifecycle reporting, and several other Gortnite features all depend on real hook/context/event composition.

Plugins need durable, scoped state. Current provider-state is host-owned shared state, but the implementation behind it is an in-memory map. That is sufficient for concurrent component calls; it is not the durability model needed for todos, memory, rules, identities, MCP caches, relationships, etc. The solution probably should not be “make provider-state a database.” Those subsystems should expose durable resources, while ephemeral component state remains ephemeral.

Plugins need to emit durable lifecycle facts. This one is particularly important for your frontend plan.

Gortnite's UI could ultimately derive rich state because its session event schema knew about everything. nukeme correctly removed all that feature-specific knowledge from the core, but the current answer is a fixed StreamEventKind containing only kernel/model concepts.

If every future plugin requires adding:

StreamEventKind::MemoryWritten
StreamEventKind::AgentStarted
StreamEventKind::TodoUpdated
StreamEventKind::ComputerObserved
...

then “everything is a plugin” will have failed at the observability boundary.

I think you need a generic durable plugin-event envelope roughly at the conceptual level of:

plugin id
event type
scope / correlation
structured payload
optional presentation metadata

with those events entering the canonical session record when session-relevant. GPUI can then learn concrete event types later without the kernel knowing them.

The resource fabric needs a non-text story. Gortnite has content-addressed image attachments and computer observations. Current ResourceReply is fundamentally textual/control-oriented. Before computer use returns, I'd establish blobs/attachments/rich content as a first-class resource concept rather than special-casing screenshots later.

The tool contract should regain some Gortnite semantics. nukeme::ToolDefinition currently has name, description, input schema and effects. Gortnite's Artist-owned contract additionally carried output schemas, category, read-only/destructive/idempotent/open-world annotations, structured failures, next-action hints, pagination information and progress reporting.

Model providers are not actually plugins yet. This is another substantial discrepancy with a literal “everything is a plugin” goal. PluginCapability::Model currently means a configuration transform. Actual StreamingModel implementations are native objects manually registered with ProfileModelRouter.

Provider/account infrastructure multiple credential types, ChatGPT subscription auth, Copilot OAuth, provider-specific API selection, per-account default models/reasoning -- see if rig can make our life easier here tbh
Provider-native statefulness and optimization. Gortnite tracks provider conversation chains, capability probes, provider-private context, uploaded-file handles, Gemini prefix caching, reasoning settings, context windows, fast mode, overload retry state, etc. These need to return without contaminating the kernel.

An extension should be able to ask the host/kernel boundary to:

create an Artist session with a specified profile + initial input;
receive a durable session ID/handle;
send input/messages to it;
inspect or subscribe to its state/events;
stop/cancel it;
optionally await its terminal result or structured yield.

The kernel should own the session identity, lineage/relationship metadata, canonical event log, and authorization. The host/runtime should actually schedule and execute the new session. I would not put process/task spawning inside the kernel itself.
