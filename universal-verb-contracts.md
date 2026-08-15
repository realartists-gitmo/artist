# Universal Verb Contracts

The model-facing surface is scalar and flat. The programmatic Component Model
surface is batch-native: every installed verb exports one function accepting a
list of request records and returning an equally long list of
`result<response,error>` values in input order.

The shipped installation contains these named tools:

```text
read write edit insert find grep run poll abort delete
```

No generic `invoke` tool and no `send` verb exist.

## Model request shapes

```json
read   {"uri":"string","at":"string|null","before":"integer|null","after":"integer|null"}
write  {"uri":"string","content":"string"}
edit   {"uri":"string","start":"string","end":"string|null","content":"string"}
insert {"uri":"string","at":"string","content":"string"}
find   {"root":"string","query":"string"}
grep   {"uri":"string","pattern":"string"}
run    {"uri":"string","args":["string"]}
poll   {"uri":"string","from":"string|null","match":"string|null","timeout_ms":"integer|null"}
abort  {"uri":"string"}
delete {"uri":"string"}
```

These schemas describe one operation. Multiple model calls emitted in one
assistant turn are collected by Artist and grouped into native batches.

`edit` is one anchored replacement or deletion. `insert` is one anchored
insertion, with `top`, `bottom`, or `#anchor` meaning before that line. Multiple
edits and inserts against one URI resolve against one pre-mutation snapshot,
validate together, and commit once.

`run.args` is argv, never shell syntax. Process output is exposed through
`process://N/stdout` and `process://N/stderr`; process lifecycle and exit status
belong to `process://N`.

## Programmatic contract shape

Every package has this shape, with package-owned request/response records:

```wit
verb: func(requests: list<request>)
    -> list<result<response, error>>;
observe: func(response: response) -> string;
```

The observer is a package-owned model projection. It is not the authoritative
typed response. Component callers, nested tools, capture, and recording retain
the complete typed value.

JSON is normalized against the scalar request `DynamicType` and then strictly
validated. Normalization may accept unique spelling/case/separator variants,
but it never guesses between materially different shapes.
