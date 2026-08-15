# Artist Component ABI

Artist packages are open-ended, versioned WIT components. The host discovers
packages and validates their exports; it does not compile a closed list of
verb names into the kernel.

Each shipped verb package exports one batch-native operation:

```wit
verb: func(requests: list<request>)
    -> list<result<response, error>>;
observe: func(response: result<response, error>) -> string;
```

The result list is ordered and one-for-one with the request list. A component
failure for one item is an item error, not permission to discard unrelated
items. Components are authoritative typed programs. `observe` is the compact,
possibly lossy model projection and is never parsed by programmatic callers.

The model-facing adapter advertises one scalar request schema per active
package. An assistant turn containing sibling calls is grouped by exact active
package generation and sent through one component batch export.

Every invocation has `invocations://ID/stdin`, `/stdout`, `/stderr`,
`/stdobs`, and `/status`. Resource input is expressed through writable child
nouns. Processes use `process://N/stdin`, `/stdout`, `/stderr`, and `/ctl`; sessions use
`session://name/inbox`. Process exit status belongs to the process root. Pure
component completion uses WIT success/error results rather than an OS exit
code.

Activation parses WIT, validates the batch shape, derives scalar types and
schemas, validates the result observer, prepares the component, and publishes an
immutable generation atomically. An invocation holds its generation lease to
completion, including observer rendering. A failed candidate leaves the old
generation active.
