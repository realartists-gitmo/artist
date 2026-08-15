# Artist Harness Tool Surface Specification

The model sees ordinary named tools derived from active `tools://` packages.
The shipped names are:

```text
read write edit insert find grep run poll abort delete
```

Each model call is scalar JSON. The assistant turn is the batch envelope: all
sibling calls in one committed assistant turn are collected, normalized,
generation-pinned, grouped by tool identity, and executed through one native
batch export per group. Results return in source-call order.

The programmatic ABI is WIT and batch-native:

```wit
verb: func(requests: list<request>)
    -> list<result<response, error>>;
```

The model never emits the outer `requests` list. The host performs tolerant
type-directed JSON normalization and then exact validation. Unknown fields,
ambiguous spellings, wrong numeric ranges, malformed URIs, and invalid union
shapes are repairable item errors.

`edit` is one anchored replacement/deletion; `insert` is one anchored
insertion. Sibling edits/inserts on one URI resolve against one snapshot,
validate all anchors and ranges, and commit once. Overlap or contradiction
leaves that URI unchanged. A write mixed with a mutation is a conflict.

`poll` addresses one URI and has `from`, `match`, and `timeout_ms`. Its result
contains anchored text and exactly one reason: changed, matched, terminated,
or timeout. Multi-resource waiting is sibling calls.

Process resources are:

```text
process://N
process://N/stdin
process://N/stdout
process://N/stderr
process://N/ctl
```

Session resources are:

```text
session://name
session://name/inbox
```

There is no `send` verb, generic model `invoke`, or shell parser. Every logical
call has invocation channels `stdin`, `stdout`, `stderr`, `stdobs`, and
`status`; component `result<Response, Error>` values remain authoritative in
`stdout`, while the package-owned `observe(result<Response, Error>)` export
supplies compact model context.
