# Artist Gortnite schema audit

Status: training-critical source pin and deterministic-adapter contract.

## Authority pin

Muse's Artist adapter is defined against this exact source revision:

- Repository: `realartists-gitmo/artist`
- Branch used for selection: `Gortnite`
- Commit: `656383b4906a796727b09a249ef5da7e60f51b81`
- Tree: `d37315bb8d9345d51d37699867f6b1e8c1b0631d`
- Session event schema file: `crates/artist-session/src/event.rs`
- Session event schema blob: `903add9c1a33947423ed82820718edffe74ca35a`
- Tool registry file: `crates/artist-agent/src/tool_set.rs`

The branch name is not a training-contract identity because it may move. The commit SHA is.

## Event-envelope contract

Every persisted `events.jsonl` record is an envelope with:

- `v: u32`: payload schema version.
- `seq: u64`: source-log order.
- `ts: u64`: Unix milliseconds.
- `session: String`: session identity.
- `run: Option<String>`: one stream-chat invocation. Retries may mint a new run.
- `lineage: String`: agent scope (`main` or delegated lineage).
- `kind: String`: stable event kind.
- `payload: JSON`: versioned event payload.

Artist deliberately degrades unknown event kinds and undecodable known payloads to unknown rather than rejecting the log. Muse must preserve the original kind/payload and must not silently drop such records.

Known event kinds at the pin:

`session.created`, `run.started`, `run.usage`, `run.finished`, `task.started`, `task.updated`, `task.finished`, `change.recorded`, `turn.user`, `model.turn`, `tool.result`, `tool.result.images`, `steering.delivered`, `delegate.started`, `delegate.finished`, `conversation.messages`, `conversation.compacted`, `history.rewind`, `legacy.turn`, `rule.fired`, `rule.injection`, `rule.retro_findings`, `handoff.performed`, `todo.updated`, `provider.context.v1`, `canvas.created`, `canvas.opened`, `canvas.state`, `ask.posted`, `ask.answered`, `memory.written`, `computer.stage_opened`, `computer.stage_closed`, `computer.launched`, `computer.observed`, `computer.acted`, `computer.elided`.

## Message/content contract

`turn.user` and `model.turn` contain ordered `ContentBlock` values:

- `text`
- `tool_call { id, call_id?, name, arguments, signature? }`
- `reasoning_summary { id?, text }`
- `reasoning_text { id?, text, signature? }`
- `reasoning_encrypted { id?, data }`
- `reasoning_redacted { id?, data }`
- `image { attachment, media_type? }`
- `opaque { rig }`

Training prose is extracted only from semantically available prose blocks. Encrypted/redacted reasoning is not reconstructed. Opaque blocks remain opaque unless a later pinned adapter understands them.

A model-turn tool call's `id` is Artist/Rig's internal call identity. `tool.result.internal_call_id` is the corresponding structured result key. `tool_call_id`, when present, preserves the provider-facing call identity separately.

## Tool-result contract

`tool.result` carries:

- `internal_call_id`
- optional `tool_call_id`
- `name`
- exact JSON `arguments`
- model-visible result text after steering rewrite
- structured outcome: success, error, skipped, or denied
- optional duration in milliseconds

A result outcome is a report about invocation status. It is not proof that requested external effects occurred.

`tool.result.images` associates content-addressed image references with the same `internal_call_id` and is result payload evidence, not a new tool invocation.

## Independent effect evidence

`change.recorded` is the authoritative independent filesystem mutation evidence currently present in the session log. It contains:

- `tool_call_id`
- `path`
- `before_digest`
- `after_digest`
- inline diff or diff-attachment reference

The adapter therefore emits a requested filesystem effect from a tool call only when the call semantics justify that request, and emits an observed filesystem transition only from `change.recorded` (or another independently observational event). A successful `write`, `edit`, `ast_rewrite`, shell command, or arbitrary dynamic tool result cannot be promoted to an observed file mutation by status alone.

A path string denotes a path. It does not establish that a file/directory exists.

## Process/task evidence

Artist persists addressable Bash task lifecycle independently from generic tool results:

- `task.started { task, command, persistent }`
- `task.updated { task, output }`
- `task.finished { task, exit_code?, interrupted }`

These events establish process/task lifecycle facts directly from the harness record. They must be joined by `task`, not guessed from command text.

## Computer evidence

The computer subsystem supplies explicit structured evidence:

- stage opened/closed
- program launched on a surface
- surface observed with epoch/rung/fullness/node/byte/image metadata
- action program executed with each action, requested label, independently resolved target name, payload, and outcome
- context observations elided

The distinction between `ComputerStep.label` and `resolved_name` is semantically load-bearing: the first records what the model claimed as target; the second records what Artist resolved. Muse must not collapse them.

## Built-in tool surface

`artist-agent/src/tool_set.rs` is the single exhaustive built-in registry at the pin. It contains 31 tools:

`bash`, `read`, `find`, `grep`, `edit`, `write`, `skill`, `todo`, `memory`, `code_map`, `code_show`, `code_surface`, `code_implements`, `code_deps`, `code_cycles`, `code_calls`, `code_trace`, `code_impact`, `code_search`, `code_related`, `ast_query`, `ast_rewrite`, `computer`, `canvas`, `handoff`, `subagent`, `poll`, `abort`, `send`, `list`, `ask`.

MCP and extension tools are dynamic and are intentionally not enumerable at compile time. Muse therefore may specialize known built-ins but must preserve unknown tool name, specification identity when available, arguments, result, and effects through the generic tooling representation.

## Adapter boundary

Artist-specific parsing lives in `muse-artist-adapter`; generic semantics live in `muse-tooling` and `muse-occurrence`.

The Artist adapter may:

- map Artist IDs into explicit source/run/record identity scopes;
- join `model.turn` tool-call blocks with `tool.result` by internal call ID;
- map structured Artist outcomes into generic reported outcomes;
- infer *requested* effect families from pinned built-in tool schemas/names where deterministic;
- attach independent observed effects from `change.recorded`, task lifecycle, and computer lifecycle records;
- preserve Artist-specific source evidence and source field paths.

It may not:

- infer an observed effect from success alone;
- infer file existence from a path argument;
- reinterpret provider-private context;
- invent semantics for unknown payloads;
- collapse model claims and harness observations;
- collapse user and agent first-person perspective;
- make the shared IR depend on Artist.

## Corpus consequence

Artist event logs are required for deterministic adapter conformance, not for bulk prose volume. Bulk SLM examples can be non-tool prose from open coding-agent transcript corpora because the SLM's contract is generic prose -> occurrence/proposition IR. Structured tool calls/results bypass the SLM whenever their source already supplies machine-readable semantics.
