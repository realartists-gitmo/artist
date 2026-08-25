# Profiles and terminal controls

Every production session starts with a named profile. Profiles are ordinary
read-only resources rooted at `profiles:///`; the default native backing tree is
`.artist/profiles` beneath the host working directory.

Each immediate child is a profile and must contain these files:

```text
.artist/profiles/
  planner/
    instructions.md
    profile.json
```

`instructions.md` is injected after system and AGENTS fragments and before the
identity fragment. `profile.json` has three optional fields:

```json
{
  "yield_schema": {
    "type": "object",
    "additionalProperties": false,
    "required": ["plan"],
    "properties": { "plan": { "type": "string" } }
  },
  "policy": {
    "default": "allow",
    "rules": [
      { "decision": "deny", "effects": ["mutate", "execute", "session-control", "unknown"] },
      { "decision": "allow", "tools": ["yield"] }
    ]
  },
  "models": [
    { "provider": "openai", "model": "small", "parameters": {} },
    { "provider": "openai", "model": "large", "parameters": {} }
  ]
}
```

The complete profile directory catalog is snapshotted on activation and drives
the `handoff.profile` enum. Instructions, policy, yield schema, model routes,
and catalog are immutable for that profile epoch. Filesystem changes affect the
next activation, never an already-running epoch.

## Policy

Tools declare one or more effects: `observe`, `mutate`, `execute`,
`session-control`, or `unknown`. Policy starts at its explicit `default`, then
applies every matching rule in declaration order; the last matching rule wins.
A rule may match tool-name globs, effects, resource operation names, and
canonical resource URI globs. Empty match lists are unconstrained.

Policy is enforced twice: while producing the model's tool catalog and again at
execution. Resource-specific rules are enforced for each resource operation,
including individual search results. A deny-list profile can deny selected
effects; a strict read-only profile can instead default-deny and explicitly
allow `observe`.

## Yield

Without an override, `yield` accepts exactly:

```json
{ "completed": true, "remainder": "optional text" }
```

`completed` is required. `remainder` is optional and may be a string or null.
A profile may replace this input schema completely with any valid JSON Schema.
The effective schema is advertised to the model and validated again before the
tool runs. A successful yield is terminal: later tool calls from that model
response are not executed, and the transcript records a typed `yielded` run
outcome containing the payload.

## Handoff

`handoff` accepts a profile from the snapshotted catalog and a brief string. A
successful call ends the current run, activates a fresh profile snapshot, and
starts a new projection epoch in the same durable transcript and session. The
initial system/AGENTS context and identity are retained; prior conversation,
tool history, and compactions are excluded from the new model projection.

Pending steering notices are consumed and appended to the brief in queue order.
Queued normal inputs are explicitly recorded as superseded. The combined brief
is recorded and becomes the first harness input of the new epoch.

## Model routes

`models` is an ordered fallback list. Each `(provider, model)` pair resolves to
an implementation registered in `ProfileModelRouter`; its `parameters` object
is applied to every provider request on that route. A successful route is sticky
for the session/profile epoch.

Routes are keyed by `(provider, account, api-variant, model)`. Resolution goes
through an installed provider source — the activated provider-plugin registry
or a host registry — never through application-constructed model objects.
Production runtimes build sessions with
`SessionRuntime::from_provider_source`; the router selects an account for the
provider deterministically (explicit `account`, else the single candidate or
the one marked `default`), fetches credentials by opaque reference from the
credential store, and asks the provider driver to open the streaming model.
Account descriptors persist in durable scoped storage; secrets never appear in
profiles, routes, errors, transcripts, or events.

Provider drivers must pass the shared conformance battery
(`artist_provider::conformance`) before shipping: plain completion,
streaming-event ordering, and mid-run cancellation to a typed `Interrupted`.

Fallback occurs only after an error and only before any tool activity. Partial
text from a failed side-effect-free attempt is cleared with `TextReset` before
the next route. Once a tool call or result is visible, the failure is terminal,
so Artist never repeats a tool execution. Handoff starts a new profile epoch and
therefore a new sticky-route decision.
