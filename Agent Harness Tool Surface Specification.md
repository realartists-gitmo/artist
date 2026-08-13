# Agent Harness Tool Surface Specification

## 1. Purpose

The harness exposes a small verb surface over URI-addressed resources.

The core verbs are:

```text
read
write
edit
poll
send
run
abort
delete
find
grep
```

Each verb is implemented as a WebAssembly Component.

The same verb implementation is used by:

```text
model tool calls
Rust harness code
WASM components
Python
Bash
other harness subsystems
```

There are not separate implementations for these callers.

```text
Model JSON ──────┐
Python ──────────┤
Bash ────────────┤
WASM imports ────┼── Tool Registry ── WASM Component
Harness code ────┘
```

The model-facing JSON schema is an adapter over the component contract.

JSON is not the semantic tool API.

---

# 2. Fundamental Rule: Resources Have URIs

Everything addressable by tools is a resource.

Every resource has a URI.

Examples:

```text
file:///src/main.rs
repo://project/pr/12
agents://reviewer
processes://FrostMoon
tools://grep/
```

A URI ending in `/` identifies a directory.

```text
file:///src/
```

A URI not ending in `/` identifies a file.

```text
file:///src/main.rs
```

OS paths are normalized internally into `file:///` URIs.

Different namespaces implement different resources, but the model uses the same verbs against all of them.

A namespace may reject an operation that does not make sense for one of its resources.

---

# 3. Files

Every non-directory resource exposed through the textual tool surface is a file consisting of lines.

Some files are static.

Some files change over time.

For example:

```text
processes://FrostMoon
agents://reviewer
```

may continually append lines.

There is no separate stream representation for such resources.

Their updating state is their file contents.

Therefore:

```text
read(X)
grep(X)
poll(X)
```

all operate on the same textual line space and the same anchors.

---

# 4. Executions Are Resources

`run` does not make its input URI live.

It creates another resource representing the execution.

```text
run(file:///bin/tests)
    ↓
processes://FrostMoon
```

These are different resources:

```text
file:///bin/tests
processes://FrostMoon
```

The first is the executable.

The second is the execution.

The execution resource contains one textual line space which grows as output is produced.

Therefore:

```text
run(X)
  ↓ URI
read(URI)
grep(URI)
poll(URI)
send(URI)
abort(URI)
```

compose directly.

There is no general `live | dead` state attached to all URIs.

A resource simply supports whichever operations its implementation supports.

---

# 5. Anchors

Every line exposed through the textual resource system has an anchor.

Example:

```text
#CampfireDOOMJMP
```

Rules:

```text
begins with #
contains no whitespace
contains no additional #
variable length
opaque to callers
```

A line can therefore be globally addressed as:

```text
URI#Anchor
```

Example:

```text
file:///src/main.rs#CampfireDOOMJMP
```

Anchors are used instead of integer line numbers for persistent references to lines.

The anchor implementation itself determines how anchors are generated.

The tool contract requires only these properties:

1. An anchor resolves to at most one current line in its resource.
2. Unchanged lines preserve their anchors across edits.
3. Deleted or replaced lines lose their old anchors.
4. Newly created lines receive new anchors.
5. A syntactically valid anchor which no longer resolves produces `StaleAnchor`.

---

# 6. Line Records

The canonical semantic representation of textual resource contents is a sequence of line records.

```rust
struct AnchoredLine {
    anchor: Anchor,
    text: String,
    ending: LineEnding,
}

enum LineEnding {
    None,
    Lf,
    CrLf,
    Cr,
}

struct AnchoredText {
    uri: Uri,
    lines: Vec<AnchoredLine>,
}
```

`text` excludes the line terminator.

The terminator is represented separately so that textual contents can be reconstructed losslessly.

For example:

```text
foo\nbar
```

becomes:

```text
foo    LF
bar    NONE
```

This distinguishes it from:

```text
foo\nbar\n
```

where the second line has `LF`.

---

# 7. Canonical Textual Codec

WIT, Rust, and Python callers use typed line records directly.

Bash and other textual environments require a canonical encoding.

An `AnchoredText` value is encoded as:

```text
@@ file:///src/main.rs
#CampfireDOOMJMP	LF	fn main() {
#BlueHorse	LF	    println!("hello");
#Quartz17	NONE	}
```

Grammar:

```text
"@@ " URI "\n"

ANCHOR "\t" ENDING "\t" TEXT "\n"
```

Valid ending fields are:

```text
LF
CRLF
CR
NONE
```

Only the first two tabs are structural.

Tabs occurring afterwards belong to the source text.

Therefore source indentation containing tabs is preserved.

Blank source line:

```text
#Anchor	LF	
```

Empty file:

```text
@@ file:///empty
```

Multiple resources concatenate as separate groups:

```text
@@ file:///src/main.rs
#A	LF	fn main() {
#B	LF	}

@@ file:///src/lib.rs
#C	LF	pub mod parser;
```

This is the canonical shell/textual encoding of `AnchoredText`.

It is not the internal representation.

---

# 8. Shared Types

The verb surface deliberately uses a small number of shared types.

```text
Uri
Anchor
Position
Text
AnchoredLine
AnchoredText
AnchoredDiff
Error
PollCondition
```

Collections use normal lists:

```text
List<Uri>
List<AnchoredText>
```

These types are defined once in the shared WIT package and reused by all tool components.

Tools do not invent private equivalents.

---

# 9. Positions

```rust
enum Position {
    Top,
    Bottom,
    Anchor(Anchor),
}
```

Model representation:

```text
"top"
"bottom"
"#CampfireDOOMJMP"
```

`Position` is used when locating reads and poll cursors.

Editing uses a more precise target type described below.

---

# 10. Errors

All components use one error language.

```rust
struct Error {
    code: ErrorCode,
    uri: Option<Uri>,
    message: String,
}
```

Core codes:

```rust
enum ErrorCode {
    InvalidUri,
    InvalidInput,

    NotFound,
    WrongKind,

    InvalidAnchor,
    StaleAnchor,

    Immutable,
    Unsupported,
    PermissionDenied,

    Conflict,
    NotLive,
    NotEmpty,

    Aborted,
    Internal,
}
```

Expected domain outcomes are not errors.

```text
grep found no matches
    => success

process exited with nonzero status
    => process state

poll timeout predicate became true
    => success

write immutable resource
    => Immutable

send resource that cannot receive input
    => Unsupported

abort already-finished process
    => NotLive
```

A WASM component trap is not an expected tool failure.

The host converts a component trap into `Internal` and records its diagnostics.

---

# 11. Batching

Batch operations use lists of request records.

Never use parallel vectors.

Bad:

```rust
read(
    Vec<Uri>,
    Vec<Anchor>,
    Vec<Range>,
)
```

Correct:

```rust
read(Vec<ReadRequest>)
```

Batch results are independently fallible:

```rust
Vec<Result<T, Error>>
```

Failure against one resource does not discard successful operations against unrelated resources.

Where several mutations target the same resource, the resource-level operation is atomic.

---

# 12. `read`

Purpose:

> Observe existing resource contents.

Semantic contract:

```rust
read(
    requests: Vec<ReadRequest>
) -> Vec<Result<ReadResult, Error>>
```

```rust
struct ReadRequest {
    uri: Uri,
    at: Option<Position>,
    before: Option<u32>,
    after: Option<u32>,
}
```

Defaults:

```text
at     = Top
before = 0
after  = harness default window
```

For an anchor:

```text
before
    ↓
#anchor
    ↓
after
```

For `Top`, `before` is necessarily zero.

For `Bottom`, `after` is necessarily zero.

Invalid combinations return `InvalidInput`.

Result:

```rust
enum ReadResult {
    Text(AnchoredText),
    Directory {
        uri: Uri,
        entries: Vec<Uri>,
    },
}
```

Reading a file returns anchored text.

Reading a directory enumerates its immediate children.

`find` is used for recursive/name-based discovery.

Example model call:

```json
{
  "requests": [
    {
      "uri": "file:///src/main.rs",
      "at": "#CampfireDOOMJMP",
      "before": 20,
      "after": 40
    }
  ]
}
```

---

# 13. `write`

Purpose:

> Create a file or replace its complete textual contents.

Semantic contract:

```rust
write(
    requests: Vec<WriteRequest>
) -> Vec<Result<WriteResult, Error>>
```

```rust
struct WriteRequest {
    uri: Uri,
    content: String,
}
```

Successful result:

```rust
struct WriteResult {
    text: AnchoredText,
}
```

The complete newly anchored file is returned.

Therefore:

```text
write
  ↓ AnchoredText
grep
```

requires no reread.

A write to an immutable resource returns `Immutable`.

A write to a directory returns `WrongKind`.

Multiple write requests for the same URI in one batch are rejected as `InvalidInput`.

Each successful resource replacement is atomic.

Example:

```json
{
  "requests": [
    {
      "uri": "file:///src/foo.rs",
      "content": "fn foo() {}\n"
    }
  ]
}
```

---

# 14. `edit`

Purpose:

> Perform anchor-addressed textual mutations.

Semantic contract:

```rust
edit(
    requests: Vec<EditRequest>
) -> Vec<Result<EditResult, Error>>
```

One request contains all edits to one resource:

```rust
struct EditRequest {
    uri: Uri,
    operations: Vec<EditOperation>,
}
```

Operations:

```rust
enum EditOperation {
    Replace {
        start: Anchor,
        end: Option<Anchor>,
        content: String,
    },

    Insert {
        at: InsertionPoint,
        content: String,
    },
}
```

Insertion points:

```rust
enum InsertionPoint {
    Top,
    Bottom,
    Before(Anchor),
    After(Anchor),
}
```

For `Replace`:

```text
end = None
```

means exactly one line.

Otherwise `start..end` is inclusive.

Deleting lines is replacement with empty content.

Whole-file replacement is performed with `write`, not with a special `edit` mode.

## Edit atomicity

All operations within one `EditRequest` are interpreted against the same pre-edit file state.

All referenced anchors are validated before mutation.

Overlapping operations return `Conflict`.

If any operation against that resource fails, none of them are applied.

Independent resources in the outer batch remain independently fallible.

## Edit result

`edit` returns both resulting anchored text and an anchored diff.

```rust
struct EditResult {
    uri: Uri,
    changed: Vec<AnchoredText>,
    diff: AnchoredDiff,
}
```

`changed` contains the resulting regions touched by the edit, including all newly assigned anchors.

This supports immediate further operations without rereading.

```text
edit
  ↓ changed AnchoredText
grep
```

The diff exists for inspection of the mutation itself.

---

# 15. Anchored Diff

```rust
struct AnchoredDiff {
    uri: Uri,
    hunks: Vec<DiffHunk>,
}

struct DiffHunk {
    old: Vec<AnchoredLine>,
    new: Vec<AnchoredLine>,
}
```

For insertion:

```text
old = []
new = inserted lines
```

For deletion:

```text
old = deleted lines
new = []
```

For replacement both are populated.

Hunks are ordered by their position in the pre-edit file.

The textual rendering uses the same line-record codec:

```text
@@ file:///src/main.rs

-#OldA	LF	old();
-#OldB	LF	stuff();
+#NewA	LF	new();
+#NewB	LF	stuff();
```

The structured `AnchoredDiff` remains authoritative.

---

# 16. `find`

Purpose:

> Search resource names.

Semantic contract:

```rust
find(
    request: FindRequest
) -> Result<Vec<Uri>, Error>
```

```rust
struct FindRequest {
    roots: Vec<Uri>,
    query: String,
}
```

`find` operates over names/URIs.

It does not inspect file contents.

Its matching algorithm belongs to the `find` component implementation and is documented by that component.

Result:

```text
UriList
```

Canonical textual encoding:

```text
file:///src/main.rs
file:///src/lib.rs
file:///src/parser.rs
```

One URI per line.

Composition:

```text
find -> read
find -> grep
find -> run
find -> delete
```

---

# 17. `grep`

Purpose:

> Search textual contents.

Semantic contract:

```rust
grep(
    request: GrepRequest
) -> Result<Vec<AnchoredText>, Error>
```

```rust
struct GrepRequest {
    pattern: String,
    source: GrepSource,
}

enum GrepSource {
    Resources(Vec<Uri>),
    Text(Vec<AnchoredText>),
}
```

This permits both:

```text
find -> grep
```

and:

```text
read  -> grep
write -> grep
poll  -> grep
grep  -> grep
```

A grep result is not a separate match object.

It is anchored source text containing the matching lines.

Example:

```text
@@ file:///src/main.rs
#BlueHorse	LF	    unsafe { foo() }

@@ file:///src/lib.rs
#GreenFire	LF	    unsafe { bar() }
```

Those anchors are immediately valid inputs to `edit`.

Zero matches is a successful empty result.

---

# 18. `run`

Purpose:

> Execute a runnable resource and create an execution resource.

Semantic contract:

```rust
run(
    requests: Vec<RunRequest>
) -> Vec<Result<Uri, Error>>
```

```rust
struct RunRequest {
    uri: Uri,
    args: Vec<String>,
}
```

The invocation context supplies the inherited working resource context and authority.

Example:

```text
run(file:///bin/tests)
    ↓
processes://FrostMoon
```

Each successful request returns exactly one URI identifying the created execution resource.

The execution resource may immediately be passed to:

```text
read
grep
poll
send
abort
```

`run` itself does not wait for execution to finish.

---

# 19. `send`

Purpose:

> Supply textual input to a resource which accepts live input.

Semantic contract:

```rust
send(
    requests: Vec<SendRequest>
) -> Vec<Result<Uri, Error>>
```

```rust
struct SendRequest {
    uri: Uri,
    content: String,
}
```

The content is exact.

`send` never implicitly adds a newline.

Therefore:

```text
"cargo test\n"
```

and:

```text
"cargo test"
```

are different inputs.

The receiving resource determines what the text means.

Examples:

```text
agent       steering
terminal    input
REPL        expression/input
debugger    debugger command
process     stdin
```

Successful output is the target URI.

`send` does not implicitly read or poll afterwards.

---

# 20. `abort`

Purpose:

> Request termination of an active resource.

Semantic contract:

```rust
abort(
    uris: Vec<Uri>
) -> Vec<Result<Uri, Error>>
```

Successful output is the affected URI.

The resource is not deleted.

Its accumulated textual state remains readable.

```text
abort(processes://FrostMoon)

read(processes://FrostMoon)
```

remains valid.

---

# 21. `delete`

Purpose:

> Remove resources.

Semantic contract:

```rust
delete(
    uris: Vec<Uri>
) -> Vec<Result<Uri, Error>>
```

Successful output is the deleted URI.

Deleting a non-empty directory does not implicitly recurse.

It returns:

```text
NotEmpty
```

Recursive deletion is composition:

```text
find -> delete
```

rather than a separate deletion mode.

---

# 22. `poll`

Purpose:

> Observe an updating textual resource and optionally wait until a condition over watched resources becomes true.

Semantic contract:

```rust
poll(
    request: PollRequest
) -> Result<PollResult, Error>
```

Targets:

```rust
struct PollTarget {
    uri: Uri,
    from: Option<Position>,
}
```

Request:

```rust
struct PollRequest {
    targets: Vec<PollTarget>,
    until: Option<PollCondition>,
}
```

## Poll positions

Default:

```text
from = Bottom
```

Semantics:

```text
Top
    begin before the first currently available line

Anchor(A)
    begin immediately after A

Bottom
    begin after the current final line at poll invocation
```

The anchor itself is not returned for `Anchor(A)`.

`Bottom` therefore means:

> only observe material appearing after this poll begins.

## Poll output

```rust
struct PollResult {
    text: Vec<AnchoredText>,
    satisfied: Vec<PollAtom>,
}
```

`text` uses exactly the same `AnchoredText` type as `read`.

There is no poll-specific textual format.

`satisfied` reports the atomic predicates that are true when the root condition completes.

---

# 23. Poll Conditions

The semantic condition type is a recursive positive Boolean AST.

```rust
enum PollCondition {
    Atom(PollAtom),
    All(Vec<PollCondition>),
    Any(Vec<PollCondition>),
}
```

Atoms:

```rust
enum PollAtom {
    Changed {
        target: u32,
    },

    Regex {
        target: u32,
        pattern: String,
    },

    Terminated {
        target: u32,
    },

    Timeout {
        milliseconds: u64,
    },
}
```

`target` indexes `PollRequest.targets`.

This avoids repeating resource URIs throughout the expression and permits the same resource to be watched from different positions if necessary.

## Atomic semantics

### `Changed`

True once the target produces at least one line after its poll starting position.

### `Regex`

True once the regex matches textual material observed by this poll for that target.

Matching is performed over the accumulated observed text rather than individual transport chunks.

### `Terminated`

True once the target's proper termination hook reports termination.

If the target is already terminated when polling begins, the predicate is immediately true.

A resource without termination semantics makes this atom invalid and returns `Unsupported` before polling begins.

### `Timeout`

True once the specified duration has elapsed since this poll invocation began.

Timeout is therefore a normal predicate, not an error condition.

## Boolean semantics

```text
All([A, B, C])
```

means:

```text
A AND B AND C
```

```text
Any([A, B, C])
```

means:

```text
A OR B OR C
```

Arbitrary nesting is allowed.

Example:

```text
(X AND Y AND Z) OR A
```

is:

```rust
Any([
    All([
        Atom(X),
        Atom(Y),
        Atom(Z),
    ]),
    Atom(A),
])
```

`All([])` and `Any([])` are invalid.

There is no `Not`.

All current atomic predicates are monotonic during one poll invocation: once true, they remain true.

`poll` returns when the root expression first becomes true.

## Default condition

If `until` is absent, `poll` waits until any target:

```text
changes
OR
terminates
```

where supported.

Conceptually:

```rust
Any([
    Changed(target_0),
    Terminated(target_0),
    Changed(target_1),
    Terminated(target_1),
    ...
])
```

A termination atom is omitted for targets which do not expose termination semantics.

---

# 24. PollCondition at the WASM Component Boundary

The semantic type is recursive.

The current WIT value type system cannot directly encode recursive value types.

Therefore the component ABI uses a flat node arena.

```rust
struct PollConditionWire {
    root: u32,
    nodes: Vec<PollNode>,
}

enum PollNode {
    Atom(PollAtom),

    All {
        children: Vec<u32>,
    },

    Any {
        children: Vec<u32>,
    },
}
```

Each integer is an index into `nodes`.

Example:

```text
0 = Terminated(target 0)
1 = Terminated(target 1)
2 = All([0, 1])
3 = Regex(target 2, "FAILED")
4 = Any([2, 3])

root = 4
```

This represents:

```text
(terminated(0) AND terminated(1))
OR
regex(2, "FAILED")
```

Validation requires:

```text
root exists
all child indices exist
graph is acyclic
All/Any have at least one child
every target index exists
```

Language bindings lift this wire representation into the recursive `PollCondition` API and lower it again automatically.

Application code should normally manipulate the recursive form.

The flat representation exists only because it is the WASM component wire representation.

No JSON serialization is involved.

---

# 25. WASM Component Architecture

Every tool is a WASM Component registered under a tool identity.

Examples:

```text
tools://read/
tools://write/
tools://edit/
tools://grep/
```

Each component exports its typed WIT tool interface.

Each component declares typed WIT imports for other tools it uses.

For example, a higher-level component may import:

```text
read
grep
edit
```

and call those verbs directly.

It does not invoke:

```text
invoke("grep", arbitrary-json)
```

There is no generic JSON escape hatch between components.

The host tool registry satisfies component imports using the currently active compatible component exports.

This is the fundamental tool-to-tool composition mechanism.

---

# 26. One Registry, One Invocation Path

All callers ultimately enter the same registry.

```text
model adapter
Python binding
Bash adapter
Rust host
WASM component import
```

all resolve a tool through the same active registry and invoke the same component contract.

No caller reimplements tool semantics.

The invocation context includes at least:

```text
caller identity
working resource context
authority/capabilities
cancellation
trace identity
```

Nested tool calls inherit this context.

A tool cannot gain authority by calling another tool.

Effective authority cannot exceed the authority of its caller.

---

# 27. Component State

Tool components do not obtain durable hidden state merely by remaining instantiated.

Compiled components may be cached.

Component instances are invocation-scoped unless a specific runtime requirement justifies otherwise.

Durable state belongs in resources and is accessed through tools.

This preserves:

```text
everything durable is addressable
```

and prevents behavior from depending on inaccessible mutable WASM memory left over from an earlier invocation.

An in-flight invocation continues using the component instances and dependencies with which it began.

New invocations use the current registry.

---

# 28. Self-Modifiable Tools

Tool implementation lives under `tools://`.

A tool resource contains enough information to reconstruct its active component, including:

```text
source
WIT contract
manifest/model description
build configuration
build diagnostics
```

Editing or writing tool source causes the trusted harness to:

```text
source changed
    ↓
compile candidate component
    ↓
validate component
    ↓
validate required WIT contract
    ↓
success?
    ├─ yes -> atomically replace active component
    └─ no  -> keep previous active component
```

A failed build never replaces the working tool.

Build diagnostics remain readable through the resource system.

---

# 29. Contract Stability

Implementation is self-modifiable.

A registered contract is not silently mutable.

A replacement component may hot-swap under the same tool identity only if it still satisfies the registered WIT contract.

An incompatible WIT change is a contract change.

It must be installed as a new contract/version rather than silently replacing callers' expected interface.

Therefore:

```text
implementation revision
```

and:

```text
tool contract version
```

are different concepts.

Agents may freely iterate on implementation without destabilizing programmatic callers.

---

# 30. Model-Facing JSON

Models continue to call ordinary named tools through conventional JSON schemas.

Example:

```json
{
  "requests": [
    {
      "uri": "file:///src/main.rs",
      "at": "#CampfireDOOMJMP",
      "before": 20,
      "after": 40
    }
  ]
}
```

The adapter converts this into the typed component input.

The component never receives arbitrary provider JSON.

Similarly, typed component results are converted into conventional model-visible tool results.

The model interface is optimized for correct model use.

The component interface is optimized for exact programmatic composition.

They represent the same operation.

---

# 31. Model-Facing Tool Shapes

The canonical model shapes are therefore approximately:

## `read`

```json
{
  "requests": [
    {
      "uri": "string",
      "at": "top | bottom | #anchor",
      "before": 0,
      "after": 100
    }
  ]
}
```

## `write`

```json
{
  "requests": [
    {
      "uri": "string",
      "content": "string"
    }
  ]
}
```

## `edit`

```json
{
  "requests": [
    {
      "uri": "string",
      "operations": [
        {
          "replace": {
            "start": "#anchor",
            "end": "#anchor | null",
            "content": "string"
          }
        }
      ]
    }
  ]
}
```

or:

```json
{
  "requests": [
    {
      "uri": "string",
      "operations": [
        {
          "insert": {
            "at": {
              "before": "#anchor"
            },
            "content": "string"
          }
        }
      ]
    }
  ]
}
```

Insertion positions additionally permit:

```text
top
bottom
after anchor
```

## `find`

```json
{
  "roots": ["URI"],
  "query": "string"
}
```

## `grep`

```json
{
  "pattern": "string",
  "source": {
    "resources": ["URI"]
  }
}
```

The programmatic contract additionally permits `AnchoredText` directly as the source.

## `run`

```json
{
  "requests": [
    {
      "uri": "URI",
      "args": ["string"]
    }
  ]
}
```

## `send`

```json
{
  "requests": [
    {
      "uri": "URI",
      "content": "exact string"
    }
  ]
}
```

## `abort`

```json
{
  "uris": ["URI"]
}
```

## `delete`

```json
{
  "uris": ["URI"]
}
```

## `poll`

Conceptually:

```json
{
  "targets": [
    {
      "uri": "processes://one",
      "from": "bottom"
    },
    {
      "uri": "agents://reviewer",
      "from": "#SomeAnchor"
    }
  ],
  "until": {
    "any": [
      {
        "all": [
          {
            "terminated": {
              "target": 0
            }
          },
          {
            "terminated": {
              "target": 1
            }
          }
        ]
      },
      {
        "regex": {
          "target": 0,
          "pattern": "FAILED"
        }
      },
      {
        "timeout": {
          "milliseconds": 30000
        }
      }
    ]
  }
}
```

The model-facing adapter may expose this natural recursive JSON form.

The component boundary lowers it to `PollConditionWire`.

---

# 32. Python

Python bindings expose the semantic types directly.

Conceptually:

```python
files = find(...)
matches = grep(pattern, files)
```

and:

```python
process = run(...)

result = poll(
    targets=[process],
    until=Any(
        Terminated(process),
        Regex(process, "FAILED"),
        Timeout(30_000),
    ),
)
```

Python does not construct provider JSON.

---

# 33. Bash

Bash exposes thin commands over the same registry.

Examples:

```bash
harness find ... | harness grep ...
```

```bash
harness run ... | harness poll ...
```

Canonical codecs are used where typed values cross the byte-stream boundary.

Examples:

```text
Vec<Uri>
    => one URI per line

AnchoredText
    => canonical anchored line records
```

Bash wrappers contain no resource semantics.

They parse/encode values and call the same component registry.

---

# 34. Composition

The following compositions are intentionally valid:

```text
find  -> read
find  -> grep
find  -> run
find  -> delete

read  -> grep
grep  -> grep

write -> grep

edit.changed -> grep

run   -> read
run   -> grep
run   -> poll
run   -> send
run   -> abort

poll  -> grep
```

And component code can combine these arbitrarily:

```text
find
  ↓
grep
  ↓
anchors
  ↓
edit
  ↓
changed text
  ↓
grep
```

or:

```text
run A
run B
run C
  ↓
poll until
    (A terminated AND B terminated AND C terminated)
    OR
    regex(A, "FAILED")
    OR
    timeout(30s)
```

No model turn is required between these operations.

---

# 35. What Makes the System Programmable

Programmability does not come from adding orchestration verbs.

It comes from four properties:

```text
stable URI identity
+
small shared value algebra
+
typed component imports/exports
+
tools able to call tools
```

A program can therefore combine verbs without translating between unrelated private result formats.

The model uses the same operations through JSON adapters.

Bash uses textual codecs.

Python and Rust use typed bindings.

WASM components use typed component imports.

All operate on the same semantics.

---

# 36. Kernel Boundary

The trusted kernel is responsible only for machinery that must remain stable for the tool system itself to function.

At this layer that includes:

```text
URI parsing and namespace dispatch
resource capability enforcement
tool registry
WASM compilation/loading
component linking
component invocation
contract validation
invocation-context propagation
cancellation
atomic hot swap
```

The verb implementations themselves are WASM components.

Therefore behavior such as:

```text
how read is presented
how grep searches
how find ranks
how edit is implemented
how poll waits
how run launches
```

can live in replaceable tool components while the machinery required to load and call those components remains trusted.

---

# 37. Completeness Boundary

This specification fixes the tool-surface architecture.

It defines:

```text
resource identity
file/directory distinction
execution identity
anchor identity requirements
line-record representation
lossless line endings
canonical textual codec
shared value types
error language
batch semantics
mutation atomicity
read contract
write contract
edit contract
anchored diff contract
find contract
grep contract
run contract
send contract
abort contract
delete contract
poll contract
recursive poll-condition semantics
WIT-compatible PollCondition lowering
component composition
tool-to-tool invocation
invocation context
self-modification
hot-swap behavior
contract compatibility
model JSON adaptation
Python adaptation
Bash adaptation
```

The following are implementation choices below this abstraction and do not require changing these contracts:

```text
anchor-generation algorithm
namespace resolver internals
exact find ranking algorithm
exact regex engine used by shipped grep/poll components
process implementation
WASM compiler/toolchain implementation
storage implementation
scheduler implementation
model provider implementation
```

Those implementations must obey the contracts above.

They are not additional tool-surface concepts.

# Final Architectural Rule

The tool system is:

```text
URI-addressed resources
operated on by
a small set of typed WASM-component verbs
which exchange
a small set of common values
and may call
other verbs through the same registry.
```

Model JSON, Bash pipes, Python functions, Rust calls, and WASM imports are different representations of that one system.

No layer gets a separate tool semantics.