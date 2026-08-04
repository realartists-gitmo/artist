# MCP tool execution contract

Artist publishes one common result envelope for every MCP tool while retaining each tool's specific `data` schema.

Successful calls return:

```json
{
  "ok": true,
  "data": {},
  "warnings": [],
  "nextActions": [],
  "meta": {
    "operationId": "op_...",
    "durationMs": 12
  }
}
```

Empty optional arrays are omitted. Failed calls set MCP `isError=true` and return:

```json
{
  "ok": false,
  "error": {
    "code": "input_validation_failed",
    "message": "...",
    "retryable": false,
    "fieldErrors": []
  },
  "nextActions": [],
  "meta": {
    "operationId": "op_...",
    "durationMs": 1
  }
}
```

Known recovery actions are typed. Examples include retrying, starting a timed-out shell command in the background, recovering an idempotent operation, selecting a workspace, and reading the next page of a bounded result. Tool and transport output cannot inject these actions; the Artist server creates them.

## Annotations

Every tool publishes all four MCP hints:

- `readOnlyHint`
- `destructiveHint`
- `idempotentHint`
- `openWorldHint`

The server rejects contradictory definitions, including a tool marked both read-only and destructive. Polymorphic tools such as `bash` and `computer` are annotated for their maximum possible effect.

## Bounded results

Inline structured and rendered output is bounded to 64 KiB. Larger results return a deterministic preview, `data: null`, and a `page` object containing an opaque cursor. Call the read-only `page` tool with that cursor to retrieve chunks of up to 64 KiB.

Page artifacts are stored under the Artist state directory for 24 hours. Reading a cursor is idempotent, so a client may safely retry after a lost response. Cursors are opaque random identifiers and cannot be changed into file paths or offsets.

## Progress

When an MCP caller supplies a progress token, Artist sends monotonically increasing progress notifications. Every call emits start and completion events. Native typed tools also emit execution and result-serialization phases. Long-running shell, computer, subagent, memory, and code-index operations emit a rate-limited heartbeat every five seconds.

Background shell work returns immediately with its session or operation identifier; later state is retrieved through the corresponding tool rather than keeping one MCP request open indefinitely.
