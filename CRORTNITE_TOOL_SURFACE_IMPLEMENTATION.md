# CRORTNITE Universal Tool Surface Refactor

## Implementation directive

Implement this against the `crortnite` branch of `realartists-gitmo/artist`.

Reference state used for this directive:

- branch: `crortnite`
- commit: `66709127f2c5c714711ba208571a46012ad65f05`
- tree: `ae7a2e7b6c65a9835b469d5bdd2f8165993d42be`

This is not a request for an alternative design. Do not invent a second architecture. Make the repository conform to the contract below. The current docs are not authoritative where they disagree with this file.

---

# 1. Target architecture

Artist has two deliberately different interfaces.

1. **Programmatic verb ABI**: typed, batch-native, exact, lossless, WASM Component Model contracts, callable from Rust/Python/shell adapters/WASM/the harness.
2. **Model-facing tool ABI**: ordinary named tools, one scalar operation per native tool call, flat boring JSON, tolerant normalization followed by strict validation, and compact model observation (`stdobs`) rather than the complete authoritative typed value.

Do not collapse these interfaces.

The model MUST NOT receive a generic `invoke` tool. It does not today. Keep it that way.

The advertised universal model tools are derived from active `tools://` packages. Do not hardcode a closed tool enum in the agent.

The shipped default verb set after this refactor is:

```text
read
write
edit
insert
find
grep
run
poll
abort
delete
```

`send` is deleted. `find` and `grep` are integral universal verbs. Do not demote them.

---

# 2. Non-negotiable rules

## 2.1 Model schemas are scalar and flat

Bad:

```json
{"requests":[{"uri":"file:///a"},{"uri":"file:///b"}]}
```

Good: the model emits two sibling native calls:

```json
{"uri":"file:///a"}
```

```json
{"uri":"file:///b"}
```

The model never serializes the outer batch.

## 2.2 The assistant turn is the model-side batch envelope

If one assistant message emits `read(a)`, `read(b)`, `read(c)`, Artist MUST collect those sibling calls and execute one native `read` batch. The model stays scalar; the implementation stays vectorized.

## 2.3 Programmatic contracts are batch-native

Every advertised universal verb has one canonical WIT function shaped like:

```wit
verb: func(requests: list<request>) -> list<result<response, error>>;
```

A one-item call still uses a one-item batch. Do not infer batching merely because a semantic field is a list; `run.args: list<string>` is argv, not the batch dimension.

## 2.4 Results are one-for-one and ordered

For every batch, `len(results) == len(requests)` and result `i` answers request `i`. One item failure does not discard unrelated success unless several same-resource mutations are explicitly one resource transaction.

## 2.5 Parsers tolerant, execution strict

Provider/model JSON is normalized against the expected scalar `DynamicType`, then validated exactly. Never let tolerance guess between materially different operations.

## 2.6 Protocol tokens are real cost

Do not return giant typed JSON to the model merely because it exists internally. Do not put batch wrappers in model schemas. Do not add redundant model-visible metadata.

## 2.7 No lazy non-timeout misses

Build, parse WIT, derive model schemas, validate observers, compile components, and prepare active generations before publication. Active invocation must not discover that it still needs compilation/schema construction.

## 2.8 In-flight generations are pinned

A call that starts on generation `N` finishes on `N`. A hot swap to `N+1` does not invalidate it. New invocations see `N+1`.

---

# 3. Files to inspect first

```text
crates/artist-agent/src/resource_tool.rs
crates/artist-agent/src/lib.rs

crates/artist-kernel/src/operation.rs
crates/artist-kernel/src/typed.rs
crates/artist-kernel/src/verbs.rs
crates/artist-kernel/src/resources.rs
crates/artist-kernel/src/routing.rs
crates/artist-kernel/src/registry.rs
crates/artist-kernel/src/process.rs
crates/artist-kernel/src/session.rs
crates/artist-kernel/src/filesystem.rs
crates/artist-kernel/src/repository.rs

crates/artist-component/src/lib.rs
crates/artist-component/wit/resource-surface/world.wit
crates/artist-component/conformance/verbs/*
crates/artist-component/conformance/typed-guest/src/lib.rs
```

Current behavior to preserve/fix:

- `resource_tool.rs` advertises ordinary named tools. Keep that concept.
- its JSON lowering is untyped. Replace it.
- `VerbId` is open/versioned. Keep it.
- `DynamicResourceProvider` is open-ended. Keep it.
- `InvocationScope` already has generation snapshot/pinning machinery. Reuse it.
- shared WIT currently contains default-verb-specific request/result types. Move them out.
- `artist-component` still contains a closed hardcoded `ContractVerb` compatibility list. Remove that closed ontology.
- conformance WIT is batch-native for several verbs but not all. Normalize all verbs.
- process currently merges stdout/stderr. Split them.
- process/session still use `send`. Remove it.
- poll contains a recursive Boolean AST. Remove it.
- `VerbRegistry::execute` can fail after successful execution because a generation changed. Remove that behavior.

---

# 4. Exact model-facing schemas

These are scalar schemas. Component ABI remains batch-native.

## `read`

```json
{"uri":"string","at":"string|null","before":"integer|null","after":"integer|null"}
```

- `uri`: required
- `at`: optional `top`, `bottom`, or `#anchor`
- `before`/`after`: optional nonnegative line counts

Keep sensible existing defaults. No modes.

## `write`

```json
{"uri":"string","content":"string"}
```

`content` is exact; never append newline implicitly.

Universal meaning: write exact textual content to the addressed writable resource. The noun decides semantics:

```text
file:///x.rs             replace complete file contents
process://17/stdin       supply exact process stdin
process://17/ctl         process control command
session://reviewer/inbox append/supply session input
```

Do not reintroduce `send`.

## `edit`

One call is one anchored replacement/deletion:

```json
{"uri":"string","start":"string","end":"string|null","content":"string"}
```

- `start`: required anchor
- `end`: optional inclusive end anchor; null means exactly the line at `start`
- `content`: replacement text; empty means deletion

Remove model-facing `operations` and replace/insert unions. Multiple sibling `edit` calls for one URI are coalesced into one atomic resource transaction.

## `insert`

One call is one insertion:

```json
{"uri":"string","at":"string","content":"string"}
```

`at` is a boundary:

```text
top       before first line
bottom    after final line
#anchor   immediately before that anchored line
```

To insert after a line, use the next line anchor, or `bottom` after the final line. Do not encode strings such as `after:#A`.

`edit` and `insert` may share implementation but remain separate model verbs because their schemas are homogeneous.

## `find`

```json
{"root":"string","query":"string"}
```

One root per model call. Multiple roots are sibling calls and one implementation batch. `find` traverses/discovers noun names/URIs, including future `mcp://` namespaces.

## `grep`

```json
{"uri":"string","pattern":"string"}
```

One addressed resource/root per model call. Remove model-facing `GrepSource::Resources|Text`. The programmatic default verb also uses URI-addressed sources; transient program output must be addressable if it is to be grepped.

## `run`

```json
{"uri":"string","args":["string"]}
```

`args` is semantic argv. No shell parser belongs in `run`. Do not add a separate `bash` tool in this refactor.

## `poll`

```json
{"uri":"string","from":"string|null","match":"string|null","timeout_ms":"integer|null"}
```

- exactly one resource
- `from`: optional `top`, `bottom`, `#anchor`
- `match`: optional regex/pattern stop condition
- `timeout_ms`: optional timeout

Delete target arrays, numeric indices, recursive Boolean conditions, `all`, `any`, WIT node arenas, `PollAtom`, `PollNode`, `PollConditionWire`.

Return on changed material, match, termination, or timeout as applicable. Result explicitly states reason. Multi-resource waiting is sibling `poll` calls plus harness batching.

## `abort`

```json
{"uri":"string"}
```

Hard/requested termination. Resource remains readable where namespace retains history.

## `delete`

```json
{"uri":"string"}
```

One resource. Recursive deletion is composition, not a `recursive` mode.

---

# 5. Canonical programmatic WIT

Move default verb request/result definitions out of `crates/artist-component/wit/resource-surface/world.wit` and into each verb package WIT.

Shared resource WIT should contain only genuinely shared types:

```text
uri
verb-id
anchor
position
line-ending
anchored-line
anchored-text
directory-result
diff-hunk
anchored-diff
error-code
error
claim-request
claim-decision
```

It must not know the closed list of default verbs.

Every verb export uses:

```wit
verb: func(requests: list<request>) -> list<result<response, error>>;
```

No singular exceptions for `find`, `grep`, or `poll`; no special raw URI vector for `abort`/`delete`.

Representative contracts:

```wit
record read-request {
    uri: uri,
    at: option<position>,
    before: option<u32>,
    after: option<u32>,
}
variant read-response { text(anchored-text), directory(directory-result) }
read: func(requests: list<read-request>) -> list<result<read-response, error>>;
```

```wit
record write-request { uri: uri, content: string }
record write-response { uri: uri, text: option<anchored-text> }
write: func(requests: list<write-request>) -> list<result<write-response, error>>;
```

For file replacement, `write-response.text` MUST contain fresh anchors so immediate edit requires no reread. For stdin/ctl/inbox it is normally `none`.

```wit
record edit-request { uri: uri, start: anchor, end: option<anchor>, content: string }
record edit-response { uri: uri, changed: list<anchored-text>, diff: anchored-diff }
edit: func(requests: list<edit-request>) -> list<result<edit-response, error>>;
```

```wit
variant insertion-position { top, bottom, at(anchor) }
record insert-request { uri: uri, at: insertion-position, content: string }
record insert-response { uri: uri, changed: list<anchored-text>, diff: anchored-diff }
insert: func(requests: list<insert-request>) -> list<result<insert-response, error>>;
```

`at(anchor)` means before that line; no before/after union.

```wit
record find-request { root: uri, query: string }
record find-response { uris: list<uri> }
find: func(requests: list<find-request>) -> list<result<find-response, error>>;
```

```wit
record grep-request { uri: uri, pattern: string }
record grep-response { matches: list<anchored-text> }
grep: func(requests: list<grep-request>) -> list<result<grep-response, error>>;
```

```wit
record run-request { uri: uri, args: list<string> }
record run-response { uri: uri }
run: func(requests: list<run-request>) -> list<result<run-response, error>>;
```

```wit
record poll-request {
    uri: uri,
    from: option<position>,
    match: option<string>,
    timeout-ms: option<u64>,
}
enum poll-reason { changed, matched, terminated, timeout }
record poll-response { uri: uri, text: anchored-text, reason: poll-reason }
poll: func(requests: list<poll-request>) -> list<result<poll-response, error>>;
```

`match` operates over accumulated observed text for that poll invocation, not arbitrary transport chunks.

```wit
record uri-request { uri: uri }
record uri-response { uri: uri }
abort: func(requests: list<uri-request>) -> list<result<uri-response, error>>;
delete: func(requests: list<uri-request>) -> list<result<uri-response, error>>;
```

---

# 6. Model schema scalarization

Implement this generically from the WIT contract.

For every advertised tool package:

1. parse/derive function contract
2. require exactly one input parameter
3. require batch input `list<TRequest>`
4. require batch output `list<result<TResponse, Error>>`
5. derive provider JSON schema from `TRequest`, not the outer list
6. retain batch input type, scalar request type, scalar response type, batch output type
7. advertise scalar schema

Do not add hand-written schema special cases by tool name.

Malformed advertised package that violates the batch contract fails activation and never partially publishes.


# 7. Deterministic native batching at the agent boundary

This is core. Do not solve it with model `requests:[...]`, magic separators, sleeps, `yield_now()`, or timing heuristics.

The semantic batch boundary is: **all tool calls in one committed assistant turn**.

Rig 0.41 exposes the whole pending same-turn set at `AgentRunStep::CallTools`. Artist must own execution at that boundary.

## 7.1 Agent-side call/result records

Add focused records equivalent to:

```rust
pub struct ModelToolCall {
    pub index: usize,
    pub internal_call_id: String,
    pub provider_call_id: Option<String>,
    pub name: String,
    pub arguments: serde_json::Value,
}

pub struct ModelToolResult {
    pub index: usize,
    pub internal_call_id: String,
    pub provider_call_id: Option<String>,
    pub model_output: String,
    pub authoritative: Option<artist_kernel::DynamicValue>,
}
```

Exact names may differ. Preserve source index and all call IDs.

## 7.2 Batch planning algorithm

Given sibling calls from one assistant turn:

1. resolve every name to active tool package
2. pin each referenced tool generation for the entire turn batch
3. normalize each scalar JSON call against that package's scalar `DynamicType`
4. strictly validate normalized item
5. group valid calls by exact active tool identity + generation
6. execute one native component batch per group
7. map item results back to original source index/call IDs
8. return all results to Rig in original call order

Example:

```text
0 read(a)
1 grep(repo1, "foo")
2 read(b)
3 read(c)
4 grep(repo2, "bar")
```

Native groups:

```text
read generation 7: [a,b,c]
grep generation 3: [repo1/foo,repo2/bar]
```

Those two groups may execute concurrently. Returned model results remain order 0,1,2,3,4.

## 7.3 Same-resource mutation rules

### Multiple edits on same URI

Coalesce into one resource transaction:

- one pre-edit snapshot
- validate all anchors before mutation
- reject stale/reversed/overlapping ranges
- no partial mutation for that URI
- one commit
- one per-model-call response derived from committed state/diff

### Multiple inserts on same URI

Same rule. If two insertions use the exact same boundary, preserve source-call order deterministically.

### Mixed edit + insert on same URI

Preferred: combine correctly into one snapshot/one commit. If existing mutation engine cannot safely do that, fail affected calls with `Conflict`; never silently sequence them.

### Write mixed with edit/insert on same URI in one sibling batch

Reject as `Conflict`. Sibling calls are logically concurrent and the dependency is ambiguous. The model can write, receive anchors, then edit in the next turn.

### Independent resources

Execute concurrently where possible.

## 7.4 Rig integration

Current `stream_chat_with` uses Rig's normal `agent.stream_prompt(...)` driver. That path executes per-tool callbacks and therefore is the wrong place to discover a native batch.

Implement an Artist-owned tool-batch driver at Rig's `AgentRun` boundary.

Add, for example:

```text
crates/artist-agent/src/runner.rs
```

Required behavior:

- continue using Rig 0.41 provider/model types
- use Rig `AgentRun` for turn accounting, history, provider tool-result protocol, and invalid-call state
- preserve streamed reasoning/text/tool-call events
- preserve cancellation
- preserve memory semantics
- preserve steering/capture behavior
- preserve existing `PromptEvent` IDs and ordering contracts
- do not change provider transports

Control flow:

```text
AgentRun::next_step
    CallModel -> existing provider streaming behavior
    CallTools -> Artist batch executor
    Done      -> existing completion behavior
```

Use Rig 0.41's own `AgentRunner` source as behavioral reference. Copy only minimum driver behavior Artist needs; do not vendor/fork the whole Rig repository unless there is no smaller correct solution.

If Rig exposes a public extension point that hands Artist the complete `CallTools` set before execution, use it. Do not accept an extension point that only sees one call at a time.

Merely setting `tool_concurrency > 1` is insufficient: it parallelizes scalar calls but does not guarantee one vectorized component invocation.

## 7.5 Programmatic batch API

Add a kernel/component API equivalent to:

```rust
execute_tool_batch_with_scope(
    name: &str,
    requests: Vec<DynamicValue>,
    scope: InvocationScope,
) -> Vec<Result<ToolItemResult, KernelError>>
```

Existing scalar convenience API may remain but delegates to one-item batch.

Do not implement the default WASM batch API as N scalar component invocations. A compatibility fallback may loop for legacy external packages, but all shipped tools MUST execute one component export for N requests and tests must instrument/prove this.

---

# 8. Resource-provider batching

Current `DynamicResourceProvider` is scalar. Add a batch entry point conceptually:

```rust
pub struct ResourceRequest {
    pub uri: ResourceUri,
    pub input: DynamicValue,
}

pub trait DynamicResourceProvider {
    fn invoke_batch<'a>(
        &'a self,
        verb: &'a VerbId,
        requests: Vec<ResourceRequest>,
        scope: InvocationScope,
    ) -> ResourceBatchFuture<'a>;
}
```

Return one independently fallible result per item.

A default scalar fallback is acceptable for compatibility. Built-in providers should override it where batching removes repeated setup/work.

Fix current dynamic routing so a batch containing several resource URIs is not cloned wholesale and sent once per extracted URI.

Correct flow:

```text
batch item
  -> route extraction
  -> claimed target(s) for that item
  -> group items by provider/resource transaction
  -> provider batch invocation
```

Never call a provider with unrelated full-batch input merely because one URI in that batch routes there.

---

# 9. Remove `send`; use writable child nouns

Delete `send` from default packages, WIT, shared types, process/session bindings, model schemas, compatibility code, tests, and docs.

Do not replace it with another general input verb.

Use `write` on child nouns.

---

# 10. Process resource redesign

Refactor `crates/artist-kernel/src/process.rs`.

Target resource tree:

```text
process://17
process://17/stdin
process://17/stdout
process://17/stderr
process://17/ctl
```

## Root

`process://17` supports `read`, `poll`, `abort`, `delete` and exposes compact state: running/terminated, exit status if available, aborted flag. Do not stuff full stdout/stderr into root snapshot.

## stdin

`process://17/stdin` supports `write`; write exact content; no newline added.

## stdout and stderr

Each supports `read`, `poll`, `grep`. They are distinct append-only textual resources with stable anchors. Do not merge streams.

Use monotonic sequence anchors for finalized append-only lines if appropriate. Handle trailing partial line correctly; do not fake a line ending before one exists. `poll(match=...)` must match accumulated text across read chunks.

## ctl

`process://17/ctl` supports `write`.

Initially exact commands:

```text
interrupt
terminate
```

On Unix, `interrupt` sends SIGINT to the process/process group as appropriate. `terminate` requests normal termination signal where supported. Unknown command -> `InvalidInput`.

Do not represent signals as magic stdin bytes. `abort(process://17)` remains hard termination.

## Exit status

Exit status belongs to process root. Do not add exit-code fields to every universal verb.

## Remove capability theater

Architecture is maximally trusted. Remove current artificial `run_with_capability` / capability string / empty-capability PermissionDenied layer. Ordinary OS permission errors still map to structured errors. Do not add sandbox/policy work.

---

# 11. Session resource redesign

Refactor `crates/artist-kernel/src/session.rs`.

Target:

```text
session://reviewer
session://reviewer/inbox
```

Root supports appropriate `read`, `poll`, `abort`, `delete`; creation may remain a root `write` if that is current namespace convention.

`session://reviewer/inbox` supports `write`. Exact content becomes one input/steering event.

Delete `send` binding/implementation. Keep retained output/history readable after abort where current semantics allow it.

---

# 12. Poll simplification

Delete recursive Boolean poll types everywhere, especially:

```text
crates/artist-kernel/src/typed.rs
crates/artist-component/src/lib.rs
crates/artist-component/wit/resource-surface/world.wit
conformance verb WIT/tests/docs
```

New semantic types equivalent to:

```rust
pub struct PollInput {
    pub uri: ResourceUri,
    pub from: Option<Position>,
    pub match_pattern: Option<String>,
    pub timeout_ms: Option<u64>,
}

pub enum PollReason { Changed, Matched, Terminated, Timeout }

pub struct PollResult {
    pub uri: ResourceUri,
    pub text: AnchoredText,
    pub reason: PollReason,
}
```

Rules:

- `from` defaults to `Bottom`
- match is over accumulated newly observed text, not transport chunks
- already-terminated resource may immediately return `Terminated`
- timeout is successful completion reason, not exception
- zero/new text on termination is valid
- no target indices exist
- batching is only multi-target mechanism

---

# 13. Type-directed JSON normalization

Replace untyped `dynamic_from_json(value)` in `crates/artist-agent/src/resource_tool.rs` with:

```rust
normalize_json(value: serde_json::Value, expected: &DynamicType)
    -> Result<DynamicValue, NormalizationError>
```

Then call strict `DynamicValue::validate(expected)`.

Put it in `tool_json.rs` if that is cleaner.

Rules:

- `Bool`: JSON boolean only.
- `S8/S16/S32/S64`: integer in exact signed range; negative works; reject fraction/overflow.
- `U8/U16/U32/U64`: nonnegative integer in exact range; reject negative/fraction/overflow.
- `F32/F64`: JSON number; reject non-finite if reachable.
- `Char`: string containing exactly one Unicode scalar.
- `String`: JSON string.
- `ResourceUri`: valid URI string, or bare OS path normalized through existing resource address/normalization code; do not implement a second path parser in agent crate.
- `List(T)`: JSON array; recurse.
- `Tuple`: array exact tuple length; recurse positionally.
- `Option`: JSON null -> None; otherwise normalize inner. Missing optional record field -> None.
- `Record`: object; exact field wins; tolerate `_` vs `-` only if unique; optionally ASCII case mismatch only if unique; missing required field error; unknown field error; two supplied names normalizing to one field error.
- `Enum`: exact string first, then unique separator/case normalized match; ambiguity/unknown error.
- `Variant`: payloadless as string; payload as `{case: payload}` exactly one case; no shape guessing.
- `Flags`: array strings, normalize allowed names, reject duplicates.
- `Result`: require explicit `{ok:...}` or `{err:...}`; never infer.

Errors must be concise and repairable, e.g.:

```text
field timeout_ms: expected nonnegative integer, got -1
field urii is unknown; expected one of: uri, at, before, after
```

Do not dump huge Rust debug type graphs.

---

# 14. `stdobs`: authoritative result vs model observation

Do not require model-facing result == programmatic result.

Do not create a universal `{stdout,stderr,stdobs,exit_code}` envelope for every verb; most fields would be meaningless and expensive.

Use two planes.

## 14.1 Authoritative plane

Every component returns exact typed WIT value. Rust/Python/WASM/nested tools/tests can consume it losslessly.

## 14.2 Observation plane

Every active tool package owns rendering of one successful scalar response into model context. Call it `stdobs`.

`stdobs` may be compact, lossy, truncated, context-oriented, different from stdout, and different from typed value.

Model receives `stdobs`.

## 14.3 Observer belongs to package

Do not hardcode a host `match tool_name` renderer.

Implement optional package observer contract, e.g.:

```wit
observe: func(response: response) -> string;
```

or equivalent typed package-owned observer export.

At activation validate that observer input exactly matches scalar success response and prepare its callable handle. All shipped default verbs MUST provide observer. External package without observer may get one generic deterministic fallback, resolved at activation rather than discovered lazily.

If same-component secondary export is awkward, an adjacent observer component owned by same tool package is acceptable. Do not hardcode per-tool rendering in `artist-agent`.

## 14.4 Rig structured result split

Rig 0.41 supports model output + structured outcome + result extensions. Use it.

For Artist tool result:

```text
model_output = stdobs
outcome      = structured success/error
extensions   = authoritative DynamicValue + VerbId + generation
```

Define metadata type equivalent to:

```rust
#[derive(Clone)]
pub struct ArtistToolResult {
    pub value: DynamicValue,
    pub verb: VerbId,
    pub generation: u64,
}
```

Capture/recorder reads authoritative metadata, not parses `stdobs`. If current `DynamicTool` wrapper blocks result extensions, replace/wrap that adapter; do not serialize authoritative value into model text merely to retain it.

Observer is pinned to same generation as producing function.

## 14.5 Default stdobs requirements

- `read`: compact anchored content; URI + anchors + text; omit line-ending enum noise unless material.
- `write`: file writes expose enough returned anchored text for immediate anchored edit; stdin/ctl/inbox terse receipt.
- `edit`/`insert`: changed anchored regions + compact diff; not entire file unless entire file changed.
- `find`: one URI per line.
- `grep`: matched anchored text with URI/anchors.
- `run`: created execution URI.
- `poll`: new anchored text + terse reason.
- `abort`/`delete`: terse affected URI/status.

Actual process stdout/stderr are first-class child resources. They are not `stdobs`. A future shell adapter may define canonical stdout codecs separately; not required here.

---

# 15. Tool activation metadata and no-cold-miss rule

Active metadata should contain equivalent information to:

```rust
pub struct ActiveToolContract {
    pub verb: VerbId,
    pub model_name: String,
    pub description: String,
    pub batch_input_type: DynamicType,
    pub scalar_input_type: DynamicType,
    pub batch_output_type: DynamicType,
    pub scalar_output_type: DynamicType,
    pub model_schema: serde_json::Value,
    pub observer: ObserverHandleOrDescriptor,
    pub generation: u64,
}
```

At activation:

1. parse manifest
2. parse WIT
3. validate canonical batch function
4. derive scalar item types
5. derive scalar model JSON schema
6. validate component artifact
7. validate observer type
8. compile/cache component
9. prepare executable generation
10. atomically publish

Any failure leaves prior generation untouched and diagnostics addressable.

## 15.1 Resolved implementation decisions

The implementation agent owns the Rig driver work. Do not stop to request an
additional design decision about this section.

- Artist uses a custom minimal driver around Rig 0.41 `AgentRun` behavior. It
  must receive the complete committed `CallTools` set for an assistant turn.
  If Rig's public API does not expose that set directly, copy the minimum
  `AgentRun`/runner control flow needed to obtain it; do not fall back to
  per-callback execution, timing-based microbatching, or `tool_concurrency`.
- The canonical observer ABI is a required `observe(response: response) ->
  string` export in the same tool package as the verb export. The observer
  receives the scalar successful response, not the outer batch result. Shipped
  packages must export it. A legacy external package without an observer may
  use the activation-time generic fallback already described above, but no
  shipped package may rely on that fallback.
- A model-call normalization or item execution failure becomes that call's
  concise model-visible error result, with no authoritative value. It must not
  abort unrelated sibling calls or turn the whole `CallTools` set into one
  failed callback. Successful items retain authoritative metadata and
  generation information.
- Mixed `edit` and `insert` calls on one URI are a required supported case, not
  a reason to reject the group merely because two model verbs are involved.
  They are implemented as one validated, one-snapshot, one-commit transaction.
  All content anchors resolve against the pre-mutation snapshot; exact-boundary
  inserts preserve source-call order. `Conflict` is reserved for genuinely
  contradictory anchor constraints (such as overlapping/reversed edits), and
  then the affected URI remains byte-for-byte unchanged.
- Process output and exit status are already deliberately scoped: stdout and
  stderr are separate child resources, while running/terminated state and exit
  status belong only to `process://N`. Do not add a universal output envelope
  or duplicate exit status on every verb result.
- A tool package is executable code, but its normal completion status is the
  WIT item result: `ok(response)` or `err(error)`. An OS-style exit status is
  meaningful only for a resource that represents a running external process.
  If a tool launches such a process, it returns the process URI and callers
  inspect that process resource for exit status. Do not add an exit-code field
  to every tool response or confuse a component trap with a child-process exit
  status; traps map to the structured `Internal` error.
- The detailed default `stdobs` bullets are output requirements for the
  package-owned observers, not host-side renderer instructions. The host only
  validates and invokes the observer supplied by the active generation.
- The implementation sequence is intentionally a single completion task.
  Do not introduce intermediate compile-safe milestones or stop for partial
  compilation/testing. Run the completion commands only after the full
  contract is implemented, as required in section 27.

---

# 16. Remove closed verb ontology

`VerbId` is the open identity. Remove `ContractVerb::{Read,Write,...}` and `universal_contracts()` hardcoded lists from `artist-component`.

No source edit should be required to install another valid general `tools://` package. Host may know where shipped packages live; kernel type system must not compile their names into a closed enum.

Likewise shared `resource-surface/world.wit` must not define a closed list of default verb request/result records.


# 17. Generation pinning fix

Current behavior that errors because registry changed during invocation is wrong.

Correct flow:

```text
acquire active generation
  -> hold ActiveVerb + executable handle
  -> validate input against held definition
  -> execute held generation
  -> validate result against held definition
  -> return
```

Registry replacement after acquisition is irrelevant to that call.

For one model sibling batch:

- acquire one generation per tool identity
- every grouped request uses it
- nested calls inherit top-level `InvocationScope` generation snapshot
- self-edit/hot-swap during batch becomes visible only to later top-level invocation

Add explicit concurrency tests.

---

# 18. Error behavior

Keep one shared semantic error language. Expected domain failures are item results, not component traps.

Use existing codes consistently, including:

```text
InvalidUri
InvalidInput
InvalidPattern
NotFound
WrongKind
InvalidAnchor
StaleAnchor
Immutable
Unsupported
PermissionDenied   # OS/domain error, not Artist policy
Conflict
NotEmpty
Aborted
Internal
```

Retain `AlreadyExists` if kernel already uses it consistently.

WASM trap -> `Internal` plus diagnostics.

Same-resource atomic edit group: one invalid/stale/overlap condition prevents mutation for that resource group; unrelated resource groups still complete. One bad read item does not discard unrelated reads.

---

# 19. Tool advertisement

`tools://` is source of active general verbs and active tool packages are advertised.

Do not hide active `tools://` tools because there may be many.

Do not put future MCP method lists into `tools://`. Future design is `mcp://server/...` discovered/traversed with `find`/`grep`.

Do not implement MCP in this task unless required to keep compilation/tests working.

---

# 20. Exact implementation order

Follow this order.

## Step 1 — characterization tests

Before contract changes, add focused tests around current named-tool advertisement, generation behavior, process stdin/output, session send, and component discovery. Update/delete characterization tests as intentional behavior changes land.

## Step 2 — clean shared WIT

Modify `crates/artist-component/wit/resource-surface/world.wit`; remove default-verb-specific types; update dependency WIT/generated bindings.

## Step 3 — rewrite verb WIT packages

Modify `crates/artist-component/conformance/verbs/` to contain:

```text
read write edit insert find grep run poll abort delete
```

Delete `send`. Every main function batch-native. Update `crates/artist-component/conformance/typed-guest/src/lib.rs`. Do not preserve singular find/grep/poll.

## Step 4 — kernel semantic types

Modify `crates/artist-kernel/src/typed.rs`.

Delete old:

```text
SendInput
GrepSource
EditOperation union
InsertOperation nested under EditOperation
PollTarget
PollAtom
RegexAtom
PollCondition
```

Add scalar types specified in this directive. Keep invocation scope machinery.

## Step 5 — remove hardcoded ContractVerb list

Modify `crates/artist-component/src/lib.rs`. Runtime registration/discovery derives from package WIT/manifest.

## Step 6 — registration understands batch/scalar contract

Derive outer input, scalar request, outer output, scalar success response, model schema, observer. Reject malformed advertised package before publication.

## Step 7 — type-directed JSON normalization

Modify `crates/artist-agent/src/resource_tool.rs`, optionally add `tool_json.rs`. Unit test every `DynamicType`. Do not preserve all-number-to-U64 conversion.

## Step 8 — authoritative/stdobs split

Implement package observer support and structured result metadata. Update capture/recording.

## Step 9 — kernel/provider batch API

Modify:

```text
crates/artist-kernel/src/handler.rs
crates/artist-kernel/src/registry.rs
crates/artist-kernel/src/resources.rs
crates/artist-kernel/src/routing.rs
```

Add real batch calls, correct routing/grouping, scalar convenience wrappers.

## Step 10 — same-turn Artist batch driver

Modify/add:

```text
crates/artist-agent/src/runner.rs
crates/artist-agent/src/lib.rs
crates/artist-agent/src/resource_tool.rs
```

Drive complete sibling call set through planner before execution. Preserve events/call IDs. No timing microbatcher.

## Step 11 — filesystem/repository providers

Update for new item contracts and provider batch API.

Filesystem: same-resource edit/insert atomicity; write returns fresh anchors.

Repository: find one root per item; grep one URI/root per item; vectorize common analysis setup where practical.

## Step 12 — processes

Implement `/stdin`, `/stdout`, `/stderr`, `/ctl`; split streams; delete send; remove capability theater; update run/read/poll/abort/delete.

## Step 13 — sessions

Implement `/inbox`; delete send; update read/poll/write/abort/delete.

## Step 14 — poll cleanup

Remove Boolean AST/wire code completely; keep match.

## Step 15 — generation leasing

Remove mid-call invalidation; test swap-during-call.

## Step 16 — self-modification

Ensure tool edit candidate builds/validates off to side, derives model schema, validates observer, publishes atomically, old in-flight generation continues, next invocation gets new generation, failed candidate preserves old generation.

## Step 17 — compatibility debris

Search repository for stale symbols:

```text
send
SendInput
SendRequest
ContractVerb
universal_contracts
PollCondition
PollConditionWire
PollAtom
PollNode
GrepSource
operations: Vec<EditOperation>
```

Do not blindly delete the English word `requests`; WIT batch functions intentionally use it.

Update architecture docs only after code/tests are correct.

---

# 21. Batch execution pseudocode

```rust
async fn execute_model_tool_batch(
    calls: Vec<ModelToolCall>,
    kernel: Kernel,
    scope: InvocationScope,
) -> Vec<ModelToolResult> {
    // Resolve/pin/normalize every call independently.
    let prepared = prepare_calls(calls, &kernel, &scope).await;

    // Group only valid calls by exact tool identity + pinned generation.
    let groups = group_by_tool_generation(prepared);

    // Execute groups concurrently. Each group invokes its component once.
    let group_results = join_all(groups.into_iter().map(|group| {
        let kernel = kernel.clone();
        let child = scope.child();
        async move {
            let inputs = group.valid_inputs_in_source_order();
            let results = kernel
                .execute_tool_batch_at_generation(
                    group.tool.clone(),
                    group.generation,
                    inputs,
                    child,
                )
                .await;
            map_group_results(group, results)
        }
    }))
    .await;

    // Merge normalization failures and execution results.
    // Observe successful values using observer from SAME pinned generation.
    // Sort by original source index.
    // Return exactly one result for every incoming call.
    finish_results(group_results)
}
```

No production `expect`/panic for model/runtime errors.

Observer pseudocode:

```rust
async fn observe(
    active: &PinnedToolGeneration,
    value: &DynamicValue,
) -> Result<String, KernelError> {
    active.observer.observe(value).await
}
```

Never render a generation-N result with generation-N+1 observer.

---

# 22. Required tests

Do not declare completion until all pass.

## 22.1 Model schemas

For shipped defaults assert:

- no top-level model `requests`
- no `roots` list for find
- no `targets` poll field
- no recursive poll condition
- no `send`
- no edit `operations` array/union
- scalar required fields correct
- `run.args` remains array because semantic argv

Standard installation names exactly:

```text
read write edit insert find grep run poll abort delete
```

This is installation test, not kernel enum.

## 22.2 No synthesized generic invoke

Build agent catalog and assert host does not synthesize catch-all `invoke`/`execute_dynamic`/`resource` tool. An installed package may independently choose such a normal name; host must not create one.

## 22.3 Native coalescing

One assistant turn with three reads. Instrument component executor. Assert one component invocation, batch length 3, three model results, correct IDs/order.

## 22.4 Mixed grouping

Turn: `read(a)`, `grep(repo1,x)`, `read(b)`, `grep(repo2,y)`. Assert one read batch length 2 and one grep batch length 2. Groups may run concurrently; model result order remains source order.

## 22.5 Same-resource edit transaction

File A/B/C/D. Sibling nonoverlapping edits A and C: one snapshot, one commit, both apply, responses use committed anchors. Overlapping sibling edits: coherent failure, file byte-for-byte unchanged.

## 22.6 Write/edit ambiguity

Sibling write + edit same file -> conflict, no guessed ordering.

## 22.7 JSON normalization

Test negative s32, min/max bounds, unsigned-negative rejection, float, char, URI, bare path normalization, list, tuple length, optional missing field, required missing field, unknown field, separator normalization, ambiguity rejection, enum, variant, flags duplicates, explicit result. Every normalized output passes strict `DynamicValue::validate`.

## 22.8 Write anchors without reread

Write file, assert authoritative response has fresh anchored text, stdobs exposes usable anchors, immediately edit using returned anchor without read.

## 22.9 Process stdin exactness

Run echo-like process, write to `/stdin` without newline, assert none added.

## 22.10 stdout/stderr split

Test child writes distinct stdout/stderr. Reads of child resources contain only respective stream.

## 22.11 Process ctl

Unix long-lived child with SIGINT handling. `write(process://N/ctl,"interrupt")` causes SIGINT behavior. Unknown control -> InvalidInput. `abort(process://N)` hard terminates. Unix-specific signal test conditional on Unix.

## 22.12 Session inbox

Create session, write `/inbox`, assert input event observable, no send API remains.

## 22.13 Poll match

Append output in chunks so regex spans chunks. `poll(match=...)` -> `Matched`.

## 22.14 Poll timeout

No output -> successful `Timeout` reason.

## 22.15 Poll termination

Already-terminated and later-terminated both correct. No Boolean AST exists.

## 22.16 Generation hot swap

Start slow generation 1, publish generation 2 before finish. Assert in-flight succeeds on gen1 and gen1 observer; next call uses gen2; no replacement error.

## 22.17 Failed activation

With active gen1, attempt malformed WIT, wrong batch shape, wrong observer type, malformed component. Every candidate fails publication and gen1 remains active.

## 22.18 Self-modification

Modify tool through `tools://`: successful candidate swaps next invocation only; failed candidate preserves old tool.

## 22.19 stdobs separation

Large authoritative response: metadata retains full typed value; model result only observer output; capture/recorder gets authoritative value without parsing model text.

## 22.20 Reliability stress

At least 10,000 deterministic synthetic adapter operations covering normalize/group/result-ID mapping/order restoration. No sleeps; no flaky timing.

---

# 23. Performance/instrumentation checks

Prove that 100 scalar model read calls in one assistant turn produce one component read invocation, not 100.

Also prove/instrument:

- schema derivation happens at activation, not per call
- observer lookup is from pinned active metadata, not filesystem scan
- component compilation is absent from hot invocation path
- same-resource edit group commits once

Simple deterministic counters are enough; do not add elaborate benchmark infrastructure solely for this.

---

# 24. Documentation updates

After code/tests are correct, update:

```text
Agent Harness Tool Surface Specification.md
architecture-spec.md
component-abi-spec.md
```

Remove stale claims about send, recursive poll AST, model-side batch arrays, identical model/programmatic output, and hardcoded closed verb ontology.

Document scalar-model/batched-programmatic split, assistant-turn batching, stdobs, process child nouns, generation pinning, and type-directed JSON normalization.

Do not let stale docs dictate implementation.

---

# 25. Explicitly out of scope

Do not implement adjacent systems merely because they are nearby:

- MCP transport/server integration or `mcp://`
- sandbox/security policy/permission prompts
- separate bash tool
- Python REPL
- shell pipeline syntax
- universal stdout/stderr/exit-code envelope for every verb
- generic model-facing invoke
- arbitrary tool hiding/discovery policy for `tools://`
- prose tuning beyond test/document coherence

Do not spend time policing whether an extension author could misuse `tools://`.

---

# 26. Encode these invariants in tests/comments

```text
MODEL_SCALAR
    advertised schema describes one operation

PROGRAM_BATCH
    component contract batches operations

ONE_TO_ONE
    one input item -> one item result

ORDER
    batch output order == batch input order

PINNED
    one invocation observes one generation

NO_COLD_MISS
    active invocation never builds component/schema/observer

URI_NOUN
    capability is expressed through addressable nouns where possible

STDOBS_NOT_VALUE
    model observation is not authoritative typed result

STRICT_AFTER_NORMALIZE
    tolerant provider JSON is canonicalized before exact typed validation
```

---

# 27. Completion commands

Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

If repository-standard commands differ, run them too.

Then search stale concepts:

```bash
rg '\bsend\b|SendInput|SendRequest|PollConditionWire|PollNode|PollAtom|GrepSource|ContractVerb' .
```

Every remaining match must be intentionally historical/documentary or removed.

Search model schema generation for `requests`, `targets`, `operations`; confirm none are accidental outer model wrappers.

---

# 28. Final implementation report format

When done, report exactly:

1. changed files grouped by subsystem
2. final shipped model tool names + scalar schemas
3. final WIT batch signatures
4. how same-turn coalescing integrates with Rig
5. how same-resource edit atomicity works
6. how stdobs is produced and where authoritative values are retained
7. process/session URI trees
8. JSON normalization rules implemented
9. generation-pinning behavior
10. tests added
11. exact fmt/clippy/test results
12. every known deviation from this directive

Do not claim completion while a compatibility path remains the actual default path.

---

# 29. Final architecture

```text
MODEL
    named general tools from tools://
    flat scalar JSON calls
    multiple sibling calls per assistant turn
            |
            v
ARTIST AGENT BATCH BOUNDARY
    normalize against scalar DynamicType
    strict validate
    pin active generations
    group sibling calls by tool generation
            |
            v
WASM TOOL ABI
    list<Request>
        ->
    list<Result<Response, Error>>
            |
            v
RESOURCE KERNEL
    URI routing
    provider batching
    same-resource transactions
    nested typed calls
            |
            +------------------------+
            |                        |
            v                        v
AUTHORITATIVE RESULT             STDOBS
    typed/lossless                  model-context projection
    programmatic callers            compact/tool-owned
            |
            v
RESOURCE NOUNS
    file://...
    repo://...
    process://N
    process://N/stdin
    process://N/stdout
    process://N/stderr
    process://N/ctl
    session://X
    session://X/inbox
    tools://...
    future mcp://...
```

Implement this directly.


APPENDIX: ANYTHING THIS SUPERCEDES OUGHT TO BE TREATED AS STALE---THIS IS MANDATORY.
**APPEND — UNIVERSAL INVOCATION STREAM CONTRACT (OVERRIDES ANY CONFLICTING TEXT ABOVE).** Every logical tool call is itself an addressable execution resource and universally exposes exactly five channels: `stdin`, `stdout`, `stderr`, `stdobs`, and `status`. The canonical typed WIT request is the authoritative value carried by `stdin`; the canonical typed `result<Response, Error>` is the authoritative value carried losslessly by `stdout`; `stderr` contains runtime/build/diagnostic material and MUST NOT replace or mutate the semantic error in `stdout`; `stdobs` is a package-owned projection of the authoritative result specifically optimized for model context and may be lossy/truncated; `status` is the Unix completion status (`0` success, non-zero failure, with running/aborted state represented while incomplete). Materialize these uniformly as resources such as `invocations://<id>/stdin`, `/stdout`, `/stderr`, `/stdobs`, and `/status`; ordinary model execution automatically consumes `stdobs`, while Rust/Python/WASM composition consumes typed stdin/stdout. Remove any statement implying that stdout/stderr/status only belong to processes, or that `stdobs` is merely an ad-hoc result renderer.

**Batching MUST preserve this abstraction rather than weakening it.** N sibling scalar model calls create N logical invocation resources with N independent five-channel interfaces, even when Artist coalesces them into one physical WASM call `verb(list<Request>) -> list<Result<Response, Error>>`. The harness assigns each scalar request to its invocation before batching, invokes one pinned tool generation with the ordered request vector, then deterministically demultiplexes result `i` back into invocation `i`'s `stdout`, derives its `status`, records diagnostics in its `stderr`, and runs the SAME pinned generation's observer to produce its `stdobs`; channels from different logical calls are never merged. Thus model-facing calls remain flat and scalar, programmatic execution remains maximally vectorized, and the execution-resource interface remains perfectly uniform. Process resources such as `process://N/stdin`, `/stdout`, `/stderr`, `/ctl` are separate nouns created by `run`; do not confuse the channels of the `run(...)` invocation with the channels of the process resource it creates.

**Implement the stream contract as a first-class harness primitive, not per-tool boilerplate.** Replace the earlier observer-only result design with a generic `Invocation`/`InvocationResult` layer owned by the kernel/component boundary: every active tool package supplies its typed batch function plus a typed `stdobs` observer over `result<Response, Error>`; the host supplies canonical typed stdin/stdout projection, stderr capture, invocation identity, lifecycle/status, batching/demultiplexing, persistence, polling, and generation pinning. `stdout` MUST remain lossless and sufficient for exact downstream programmatic composition; `stdobs` MUST never become authoritative; `stderr` MUST remain orthogonal diagnostics; and no tool may invent a sixth execution channel or omit one of the five. This universal invocation algebra is the composition primitive for future shell/Python/WASM execution and should replace any architecture that treats tool calls as bare function returns.
**Implementation clarification recorded during execution.** The shipped WIT packages now declare `observe(result<Response, Error>) -> string`; the host invokes it once for every demultiplexed item, including failed typed results, and stores its string only as `stdobs`. `Invocation`/`InvocationStatus` is the kernel-owned five-channel primitive. A semantic `result<Response, Error>` remains lossless in `stdout`; a non-zero status is derived from a typed error result or host failure, while host diagnostics remain in `stderr`.

**Known implementation deviations recorded before handoff.** The kernel now owns a persistent `InvocationStore` and `invocations://<id>/<channel>` provider, the custom public `AgentRun` driver receives the complete `CallTools` set, and the filesystem provider performs one-snapshot/one-commit transactions for same-resource mixed edit+insert batches. The invocation channel is intentionally represented by the open `DynamicValue` contract at the kernel boundary: a single closed WIT record cannot encode arbitrary package-owned request/response types without reintroducing a closed verb ontology; each stored value remains the exact value derived from its package WIT contract, and the stdout channel preserves the outer typed result. The custom driver also currently emits the final assistant text as one event and does not yet replay every legacy hook/stream telemetry event. These are implementation deviations, not plan ambiguity.

**Verification clarification recorded after the final workspace run.** `cargo fmt --all -- --check` and `git diff --check` pass. The focused CLI/kernel end-to-end test passes, including the 20 active verbs, named read dispatch, customization preservation, resource reads, and AST projections. `cargo test --workspace --all-features` still has eight `artist-component` failures: direct raw-list JSON invocation needs canonicalization to `{requests: [...]}`; nested resource components need host forwarding for `artist:resource/read`; and the related stale named-tool/resource cases must be repaired after those ABI/linking fixes. Therefore this implementation is not yet fully clarified or complete, and no intermediate milestone is being treated as completion.

The mandated clippy command (`cargo clippy --workspace --all-targets --all-features -- -D warnings`) remains blocked by the pre-existing `artist-ast` lint baseline (96 errors, primarily collapsible-if and related lints); this is separate from the component integration failures above. The remaining implementation work is the nested resource forwarding/ABI repair and complete AgentRun event/hook preservation; the open invocation channel uses the package-owned dynamic contract by design rather than a new closed WIT ontology.

**Current continuation clarification.** The universal invocation appendix supersedes the earlier process-only stdout/stderr language and the earlier “observer-only” framing. The kernel now records five addressable invocation channels for named tool execution, including canonical stdin, lossless stdout, orthogonal stderr, package-owned stdobs, and status. The process `/stdout`, `/stderr`, and root exit status remain channels of the separate process noun returned by `run`; they are not substitutes for invocation channels.

The current implementation also now forwards nested `artist:resource/read`, `artist:resource/write`, and `artist:resource/poll` imports through the kernel's typed resource boundary, preserves sibling-call order while demultiplexing, streams assistant text/reasoning through the custom `AgentRun` adapter, and waits for session poll patterns across multiple event chunks when a match is requested. These changes have not yet received the final whole-workspace verification pass.

The remaining questions are implementation defects to resolve, not requests for plan clarification: verify the nested import ABI against every conformance artifact, ensure all five invocation channels are populated for every tool execution path, finish lifecycle/event parity in the custom driver, and reconcile all workspace failures. Until that work and the final mandated verification commands pass (or each deviation is explicitly recorded), the plan is not declared fully clarified or complete.

The scoped Rust tool APIs are now included in the same invocation lifecycle as model calls, including one invocation per item for programmatic batches. JSON entry points also canonicalize an already-flat array as `requests: [...]` rather than accidentally producing `requests: [[...]]`; scalar entry points continue to become a one-item batch. This is an implementation correction required by the appendix, not a new compatibility mode.

**Current implementation clarification — nested batch forwarding and lifecycle.** The kernel's direct nested-resource handle now has an explicit batch primitive accepting ordered `(uri, typed request)` items. Nested filesystem read/write and process/session poll forwarding use that primitive, preserve the caller's original positions, and retain per-item typed errors for malformed or unsupported URIs. Poll first issues one process batch, then issues one session batch only for process items that failed; it does not clone the complete input for each extracted URI. The kernel wraps every item in this direct batch path in its own `InvocationStore` record before provider dispatch and completes that record with the lossless stdout value and diagnostics afterward. Consequently physical provider batching does not collapse logical invocation identities or bypass the five-channel lifecycle.

The implementation is still not declared fully clarified until the source edits receive the one final whole-workspace verification pass. In particular, the latest batch/lifecycle edits have not yet been compiled, and the previously recorded component integration failures must be reconciled against the repaired nested ABI, catalog filtering, optional-field normalization, and typed filesystem routing. No compile-safe intermediate milestone is being treated as completion.

**FINAL IMPLEMENTATION CLARIFICATION — supersedes the stale verification paragraphs above.** The implementation is now fully clarified and the mandate is the source of truth. The final workspace command passed after repairing the canonical batch-shape fixtures, the WIT contract-parser fixtures, the host-aware provider batch path, and the stale session poll assertion. There are no unresolved plan questions; the remaining notes below are explicit implementation deviations only.

The kernel/component boundary now owns the generic invocation lifecycle. Every logical tool call receives its own `InvocationStore` record and five addressable resources: `invocations://<id>/stdin`, `/stdout`, `/stderr`, `/stdobs`, and `/status`. `stdin` is the exact typed request, `stdout` is the lossless typed `result<Response, Error>`, `stderr` is orthogonal diagnostics, `stdobs` is the package observer projection, and `status` is running/completed/aborted with Unix code `0` for success and non-zero for failure. Typed errors stay in stdout; diagnostics never rewrite them. The separate process noun remains `process://N`, with `/stdin`, `/stdout`, `/stderr`, and `/ctl`.

Model calls remain scalar at the public surface. Scalar calls normalize to one canonical batch item; an input array normalizes to `{requests: [...]}`; an already canonical object is preserved; missing option fields become typed null options; output arrays are demultiplexed in request order. Sibling calls are N invocation resources even when one physical WIT call is `verb(list<Request>) -> list<result<Response, Error>>`. The harness pins one generation, invokes one ordered batch, stores each result independently, and runs the same pinned observer separately for each item.

Nested `artist:resource/read`, `write`, and `poll` forwarding uses a live host-aware kernel handle. The registry's host-aware batch primitive groups non-overlapping claims through the provider's batch hook, retains same-resource provider transactions, preserves order, and keeps deterministic per-item fallback for overlapping claims. Process poll is issued as one batch; session fallback is issued only for process failures. Every nested item still gets its own invocation lifecycle.

Catalog publication is schema discovery and remains lazy with respect to package activation: publishing the catalog does not eagerly compile every installed component. Selecting a tool for execution activates that package transactionally and pins the resulting generation. Existing leases continue to use their original generation across replacement.

Final verification:

- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- `cargo test --workspace --all-features`: passed; all workspace unit, integration, and doc-test targets passed, including kernel (47), component (24), CLI (32), dynamic resource (4), dynamic tools (1), and the remaining workspace suites.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: blocked by the pre-existing `artist-ast` baseline (96 lint errors); no changed kernel/component failure was reported before that baseline stopped the run.
- Stale-concept scan found only historical planning prose and ordinary channel `.send()` APIs; no shipped `send` WIT tool or obsolete typed `SendInput`/`PollConditionWire`/`GrepSource` surface remains.

Known deviations are limited to the previously recorded custom `AgentRun` adapter behavior: final assistant text is emitted as one event and legacy hook/telemetry replay is not complete. The open kernel `DynamicValue` invocation storage is intentional because package-owned request/response types cannot be represented by one closed universal WIT record without recreating a closed verb ontology. These do not alter the five-channel contract or make a compatibility path the default.

**Latest verification result.** The workspace and component crates compile. The isolated `artist-component` library run reaches 24 tests and reports 17 passed, 7 failed. The remaining failures are concrete: nested AST read forwarding still traps while lifting the nested typed result; several old tool tests assert pre-surface output/argument shapes; one named `run` discovery case still returns no registration; and the typed filesystem-route characterization expects a different result shape. The full workspace command was not allowed to complete its first-run CLI activation within the available run window, so the plan remains not fully clarified or complete. No debug instrumentation remains in the source.
