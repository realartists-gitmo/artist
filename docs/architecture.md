# Artist architecture

Artist is currently in the first stage of a ground-up VFS refactor. The old
noun-specific execution surfaces have been removed so the next tool layer can
be built around one addressable filesystem contract.

## Current workspace

The active crates are:

- `artist-cli`: process entrypoint, provider/session setup, and a minimal
  interactive prompt loop.
- `artist-agent`: the Rig-backed completion loop, steering, cancellation,
  provider adapters and conversation persistence integration.
- `artist-session`: append-only session events, replay projections, history,
  attachments, and provider context.
- `artist-herdr`: optional lifecycle reporting when launched under Herdr.
- `llm-provider`: provider records, credentials, OAuth, and provider metadata.

The removed crates and modules were the old built-in filesystem tools and
hashline coordinator, MCP client, WASM extensions, stream rules/TTSR,
subagents, and compaction layer. Their schemas are intentionally not being
carried forward into the VFS design.

## Current execution path

```text
CLI → provider/session selection → agent loop → model stream
                                  ↘ event-sourced session log
```

The agent/CLI resource path dispatches through the VFS kernel. Native files,
sessions, repositories, and self-modifying tool packages are registered as
handlers; tool package files are addressed as `tools://<package>/...`.

## Session storage

Sessions remain append-only event logs under the Artist config directory:

```text
sessions/<project-key>/<session-id>/
  events.jsonl
  transcript.md
  attachments/<sha>
  writer.lock
```

The log is the durable source for conversation history and replay. Unknown
event fields remain forward-compatible, and projections can be regenerated.
The previous compaction and rule-specific event families are gone from the
active schema.

## Refactor boundary

The next layer should make the VFS kernel the sole model-facing registry. A
node kind declares its serialization and supported verbs; schemes are address
roots, not separate tool APIs. Backends such as shells, agents, MCP servers,
repository APIs, and future WASM components should register node kinds with
the kernel rather than expose independent model tools.

`tools://` is the self-modifying tool-definition namespace. Package files use
the ordinary file verbs, but executable packages are registered as named model
tools. The model calls `read` directly with an ordinary resource target;
`tools://read/tool.md` remains available for inspecting the package itself.
Package edits become active on the next agent attempt; failed builds leave the
previous component version active. Compilation, capability checking, lifecycle,
and version pinning belong to the component boundary; the implementation
itself is WASM.

The package catalog is contract-based rather than an enum of the ten current
verbs. Universal contracts receive the typed shared WIT adapters; extension
contracts retain their namespace, interface, version, prose, schemas, and
capabilities in the dynamic registration catalog used to construct named
harness tools.

## Deferred work

- Finish node-kind serialization and capability declarations across all
  handlers.
- Reintroduce shell, agents, MCP, and authorization as VFS backends.
- Revisit memory, compaction, TTSRs, LSP/DAP, and richer UI on top of the new
  primitives.
