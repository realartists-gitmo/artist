# Base Verb Contracts

Status: design draft for inspection.

This document defines the model-facing base tool surface. It does not define the kernel VFS trait, WASI host implementation, provider interfaces, or component-internal APIs. Components may use the capability-scoped WASI filesystem internally; these are the contracts exposed to the model.

Tool payloads and results use TOON at provider boundaries. The examples below use illustrative TOON notation: they show semantic fields and shapes, not JSON schemas disguised as TOON.

## Common rules

Every resource is addressed by an opaque URI:

```text
files:///workspace/src/main.rs
files:///workspace/src/main.rs?kind
proc://run-42/stdout
proc://run-42/status
proc://run-42/stdin
```

A readable URI projection, such as `?kind`, is a resource consumed with `read`. The model does not see inode numbers, descriptors, provider traits, or WASI handles.

Each successful result has an operation-specific body. The component ABI may batch homogeneous requests, but the model sees one call and one result per requested operation.

Every failure has this shape:

```toon
ok: false
error:
  code: not-found
  message: resource does not exist
  uri: files:///workspace/missing.rs
```

Base error codes are `invalid-argument`, `not-found`, `not-directory`, `is-directory`, `conflict`, `permission-denied`, `unsupported`, `timeout`, `cancelled`, `stale-anchor`, `process-failed`, and `internal`. Callers branch on `code`, not on the diagnostic message.

## `read`

Read a resource or readable URI projection.

### Input

```toon
uri: files:///workspace/src/main.rs
offset: 0
limit: 12000
```

`offset` and `limit` are optional byte coordinates. Omitting `limit` means the provider's maximum reasonable response size. The result says whether more content exists.

### Output: text resource

```toon
ok: true
uri: files:///workspace/src/main.rs
encoding: utf8
content: "fn main() {\\n    println!(\\"hello\\");\\n}\\n"
offset: 0
bytes: 42
truncated: false
next_offset: null
anchors:
  - anchor: teca:v1:...
    start: 0
    end: 10
```

`anchors` are opaque content coordinates usable by `edit`; `start` and `end` are byte offsets in the returned content, not line numbers.

Binary resources use `encoding: base64`:

```toon
ok: true
uri: files:///workspace/image.bin
encoding: base64
content: "AAECAwQ="
offset: 0
bytes: 5
truncated: false
next_offset: null
anchors: []
```

For `read(files:///workspace/src/main.rs?kind)`, the result is:

```toon
ok: true
uri: files:///workspace/src/main.rs?kind
encoding: utf8
content: file
bytes: 4
truncated: false
next_offset: null
anchors: []
```

The base `?kind` values are `file`, `directory`, `stream`, `process`, and `other`.

## `write`

Create or replace a resource's complete contents. This is not an insertion or append operation.

### Input

```toon
uri: files:///workspace/src/main.rs
encoding: utf8
content: "fn main() {\\n    println!(\\"changed\\");\\n}\\n"
```

`encoding` is `utf8` or `base64`. A missing file is created when permitted; missing parent directories are not created.

### Output

```toon
ok: true
uri: files:///workspace/src/main.rs
created: false
replaced: true
bytes: 45
```

## `move`

Relocate a resource, rename it, or delete it.

### Move

```toon
source: files:///workspace/src/old.rs
destination: files:///workspace/archive/old.rs
```

If `destination` is an existing directory, the source basename is preserved.

### Rename

```toon
source: files:///workspace/src/old.rs
destination: files:///workspace/src/new.rs
```

If `destination` does not exist, it is created as that exact destination URI; the source kind and contents are preserved.

### Delete

```toon
source: files:///workspace/src/old.rs
destination: null
```

Directories are not recursively deleted by this contract. A non-empty directory normally returns `conflict`.

### Output

```toon
ok: true
source: files:///workspace/src/old.rs
destination: files:///workspace/archive/old.rs
deleted: false
```

Deletion returns the same shape with `destination: null` and `deleted: true`.

## `edit`

Apply anchor-based edits to one resource. This is the coding-oriented write operation: it avoids forcing the model to rewrite an entire file.

### Input

```toon
uri: files:///workspace/src/main.rs
changes[2]{anchor,replacement}:
  teca:v1:anchor-for-old-call,"println!(\\"old\\");"
  teca:v1:anchor-for-old-return,"Ok(new_value)"
```

Each anchor identifies an exact span from a prior `read`. All changes apply to one source snapshot. Overlapping or ambiguous anchors fail the entire operation with `stale-anchor` or `conflict`; partial edits are not success. An empty replacement deletes the anchored span.

### Output

```toon
ok: true
uri: files:///workspace/src/main.rs
changes_applied: 2
bytes_before: 184
bytes_after: 191
anchors_invalidated: true
```

The caller rereads the resource for fresh anchors.

## `find`

Enumerate resource URIs under an optional root. This is the enumeration/search capability; there is no separate model-facing `list` tool.

### Input

```toon
root: files:///workspace
query: "glob:**/*.rs"
limit: 50
```

Query forms are `glob:...` for shell-style path globs, `lit:...` for literal path substrings, and `rg:...` for regular expressions against URI paths. An unprefixed query uses the provider's default fuzzy path search.

### Output

```toon
ok: true
root: files:///workspace
query: "glob:**/*.rs"
matches[3]{uri,kind}:
  files:///workspace/src/main.rs,file
  files:///workspace/src/lib.rs,file
  files:///workspace/tests/main.rs,file
truncated: false
next: null
```

`find` returns URIs and kinds only. Content is retrieved with `read`.

## `grep`

Search file contents under an optional root.

### Input

```toon
root: files:///workspace/src
query: "rg:TODO|FIXME"
path: "glob:**/*.rs"
limit: 100
```

`path` is optional. If omitted, all searchable resources under `root` are searched. Query forms are `lit:...` and `rg:...`; an unprefixed query is literal text.

### Output

```toon
ok: true
root: files:///workspace/src
query: "rg:TODO|FIXME"
matches[2]{uri,anchor,text}:
  files:///workspace/src/lib.rs,teca:v1:...,"// TODO: remove workaround"
  files:///workspace/src/main.rs,teca:v1:...,"// FIXME: handle error"
truncated: false
next: null
```

## `run`

Start an executable resource and return the process resource URI.

### Input: explicit executable

```toon
executable: files:///usr/bin/python3
target: files:///workspace/scripts/build.py
arguments[2]:
  --verbose
  --check
environment:
  RUST_LOG: info
working_directory: files:///workspace
```

`target`, `arguments`, `environment`, and `working_directory` are optional. The executable must be provider-approved.

### Input: provider default executable

```toon
executable: null
target: bash:///workspace/scripts/build.sh
arguments[1]:
  --strict
```

When `executable` is null, the target URI's provider may supply executable semantics for that scheme.

### Output

```toon
ok: true
process: proc://run-42
stdin: proc://run-42/stdin
stdout: proc://run-42/stdout
stderr: proc://run-42/stderr
status: proc://run-42/status
```

The child URIs are ordinary resources. `stdin` is writable; `stdout`, `stderr`, and `status` are readable.

## `poll`

Wait for an observable resource to produce a matching event or reach a state. Reading retrieves current bytes; polling waits for change or completion.

### Input

```toon
target: proc://run-42/stdout
match: "rg:Finished|error"
timeout_ms: 30000
```

`match` is optional. Without it, `poll` returns on the next observable event. `timeout_ms: 0` performs a non-blocking poll.

### Output: match

```toon
ok: true
target: proc://run-42/stdout
event: data
matched: true
content: "Finished building target app"
anchor: teca:v1:...
```

### Output: timeout

```toon
ok: true
target: proc://run-42/stdout
event: timeout
matched: false
content: null
```

## `signal`

Send a process-control signal. This is the one lifecycle operation not well represented as ordinary byte writing.

### Input

```toon
process: proc://run-42
signal: terminate
```

Base signals are `interrupt`, `terminate`, `kill`, `pause`, and `continue`. Providers may reject unsupported signals.

### Output

```toon
ok: true
process: proc://run-42
signal: terminate
accepted: true
```

There is no separate `abort` or `send` verb. `write(process/stdin, content)` replaces `send`; `signal` covers process control.

## Deliberately absent verbs

```text
list    find enumerates resources
stat    read(uri?kind) exposes kind; projections are readable resources
delete  move(uri, null)
rename  move(source, destination)
send    write(process/stdin, content)
abort   signal(process, terminate|kill)
```

The proposed base set is:

```text
read, write, move, edit, find, grep, run, poll, signal
```

