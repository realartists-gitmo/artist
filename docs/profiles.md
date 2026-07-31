# Profiles, handoff, and fallback

## Status

Implemented. This document specifies the architecture for agent profiles in
Artist and replaces the subagent-role system formerly described in
`subagents.md`.

Deviations from the design as built, and the parts deliberately left for later,
are recorded in Open questions.

The decisive architectural choice is:

> A profile is one object. The session root, a delegated subagent, and a handoff
> target are three instantiation modes of that same object. There is no
> indirection layer between a profile and the model it runs on: a profile names
> its provider, model, and thinking configuration directly.

## Core invariants

The implementation is invalid if any of these invariants is violated:

1. A profile fully determines its routing: provider account, model, and thinking
   configuration are each specified independently and explicitly.
2. The main agent is instantiated from a profile. There is no separate code path
   that constructs the session-root agent from raw provider configuration.
3. A handoff is functionally identical to clearing the session and starting a
   new one on the target profile with the handoff payload as its opening task.
4. A handoff is terminal for the outgoing agent. There is no return, no stack,
   and no resumption of the outgoing context.
5. Handoff and compaction share one mechanism — a conversation reset event
   carrying a replacement context — and divide the work by profile. A handoff
   always changes profile and is always initiated by the model or the user. A
   profile cannot hand off to itself; context management within a profile is
   compaction's job and is never a handoff.
6. Profile tool policy is not a security boundary. It exists to prevent tool
   bloat and to keep models from reaching for inappropriate tools.
7. Todo state is owned by the harness, not by the model context, and survives a
   context wipe without summarization.
8. Fallback never replays a turn in which tool calls have already executed.

## The profile object

Defined in markdown with YAML frontmatter, one file per profile. The body of the
file is the system prompt. Discovery layers project over global, project
winning by name; built-in profiles are replaced by a same-named definition at
either layer.

```markdown
---
name: reviewer
description: Reviews a change for correctness
extends: default
provider: anthropic-work
model: claude-opus-5
thinking:
  mode: on
  level: high
tools:
  allow: [read, find, grep]
  deny: [bash]
---

Review like a code owner. Lead with concrete findings ordered by severity.
```

### Routing is a triple, fully differentiated

`provider`, `model`, and `thinking` are three independent fields.

`provider` names a configured provider account, not a provider kind. Artist
stores accounts (`SavedProvider`: provider kind, credentials, model, API
variant, reasoning effort), and a bare model string cannot disambiguate two
ChatGPT logins, two Azure deployments, or Responses versus Chat Completions on
OpenAI. The profile names the account; the model is selected on it.

`thinking` is two fields, not one enum, because providers constrain them
jointly:

- Anthropic: `thinking: {type: adaptive|disabled}` plus
  `output_config: {effort: low|medium|high|xhigh|max}`. The fixed token budget
  (`budget_tokens`) is removed and returns 400 on current models. On Opus 5,
  `disabled` is only valid at effort `high` or below; pairing it with `xhigh` or
  `max` is a 400. On Fable 5 thinking cannot be disabled at all.
- OpenAI and the ChatGPT backend: `reasoning: {effort, summary}`, a different
  wire shape with an overlapping but non-identical level set.

Artist therefore stores a normalized `{mode, level}` pair and translates per
provider at request construction. Validity is a function of
(provider, model, mode, level) together and is checked at configuration load,
not at first request.

### Inheritance

Delegation concurrency is set separately, in `profiles.toml` beside the
`profiles/` directory (`[settings] max_concurrent`, default 4), layered
global-then-project like the profiles themselves. It is a session-wide limit
rather than a property of any one profile.

`extends` names another profile. Merge is per-field and whole-field: a field
absent in the child is taken from the parent, a field present in the child
replaces it entirely.

`thinking` is one field for merge purposes, not two. A child that sets `level`
inherits nothing from the parent's `thinking` — it takes the child's block as
written. Per-subfield merge would let a child set `level: max` while silently
inheriting `mode: off`, which is a 400 on Anthropic and is not visible from
reading either file.

Inheritance chains resolve at load. A cycle is a configuration diagnostic that
disables the profiles involved.

### Tool policy

`allow` intersects the parent's available tools; `deny` is then subtracted. Deny
wins. Entries are glob patterns matched against full tool names, so the policy
covers MCP tools and extension tools, not only the built-in set:

```yaml
tools:
  allow: [read, find, grep, "mcp:github/*"]
  deny: ["mcp:github/create_*"]
```

The built-in names are `bash`, `read`, `find`, `grep`, `edit`, `write`, `skill`,
`subagent`, and `handoff`. A pattern matching nothing is a diagnostic, not an
error — MCP servers come and go.

Subagents cannot invoke `subagent` or `handoff` regardless of policy.

### The default profile

`default` is always present and is what a session launches with when the user
names none. It is an ordinary profile and can be replaced by a same-named
definition at either configuration layer. There are no capability flags:
every profile can be launched, delegated to, and handed off to.

Because the session root is instantiated from it, `default` carries the **full
system prompt** as its body rather than a delegation blurb — a profile's
instructions are its complete prompt, which is what makes launching a focused
profile genuinely leaner. Delegating to `default` therefore hands a subagent the
whole prompt; `worker` is the lean general-purpose alternative.

Upgrading preserves customization: when `profiles/default.md` is first
scaffolded and a pre-profile `prompts/main.md` exists, the profile is seeded
from it.

## Instantiation modes

One profile, three modes:

| Mode | Context at start | Terminates by |
|---|---|---|
| Session root | Empty, or replayed on resume | User exit |
| Subagent | Empty, or forked from parent | Returning its output to the parent |
| Handoff target | The handoff payload | Handing off again, or user exit |

`stream_chat_as` instantiates the session root from a named profile and is what
`stream_chat` calls with `default`. A delegated subagent goes through the same
profile lookup, the same account resolution, and the same candidate fallback.

## Handoff

### Semantics

The outgoing agent calls the `handoff` tool with a target profile and a payload.
The tool does not return. The run ends, the model context is discarded, and a
new run begins on the target profile seeded with the payload.

The user sees one continuous session. The pre-handoff transcript stays on
screen, followed by the handoff tool call and a divider. A banner reports the
profile change; Artist is yolo by default and does not gate the handoff behind a
confirmation.

`/handoff <profile>` triggers the same mechanism from the user side. It does not
construct the payload directly: the current agent is told the user has initiated
a handoff and generates the payload itself, so the summary is written by the
context that has the information.

### Why context is discarded rather than the prompt swapped

Swapping the system prompt in place is the worst possible edit for prompt
caching — the prompt is the prefix, so mutating it invalidates the entire
conversation behind it and the full history is re-billed. A handoff instead
starts from a preamble plus a payload, which is small enough that a cold cache
costs almost nothing.

This makes handoff the primary context-management primitive: it prunes input
tokens without the lossiness of summarizing a conversation into itself.

### Payload

Structured, not free prose:

- `summary` — what was done and what remains.
- `read_files` / `modified_files` — carried from the same tracking compaction
  uses.
- `todos` — the harness-owned list, verbatim.
- `jobs` — live background subagents with their task ids.
- `chain` — the sequence of profiles this session has passed through.

`chain` exists because a handoff cannot hand back, but it can hand *onward* into
a cycle: planner to worker to planner, each hop a fresh instance that cannot
otherwise see the loop. The chain in the payload makes hop N aware of hops
1..N-1. There is no harness-enforced depth cap — the chain is context for the
model, not a limit on it.

### What carries across

| State | Behavior |
|---|---|
| Todos | Carried verbatim in the payload |
| Background subagents | Adopted; appended to the payload as live jobs |
| Read/modified file sets | Carried as structured fields |
| Stream rules (TTSR) | Not reset — once-per-session rules stay fired |
| Session log | Same session, new conversation lineage |
| Skills, MCP connections | Re-resolved for the new profile |

Only background subagents can be in flight at handoff time; a foreground
subagent blocks the tool loop, so the model cannot issue a handoff while one is
running. The registry is process-global and keyed by project root, so jobs
survive a handoff without intervention — what the design adds is telling the new
profile they exist.

### Resume and rewind

Resuming a session after a handoff uses the profile that was active when the
session was last written. Rewinding past a handoff boundary restores the profile
that was active at that point in the history — the rewind masks the handoff
event along with everything after it.

### Relationship to compaction

Artist's compaction already works by emitting a conversation reset carrying a
replacement context; the `ConversationCompacted` event is audit metadata and the
adjacent reset snapshot is authoritative. Handoff is the same mechanism with a
different summary generator and a profile swap.

The two never overlap:

| | Compaction | Handoff |
|---|---|---|
| Profile | Never changes | Always changes |
| Initiated by | The harness, on context exhaustion | The model, or the user |
| Replacement written by | The compaction summarizer | The outgoing agent |

There is no automatic handoff. A profile running out of context is compacted,
not handed off — a handoff to the same profile would be compaction with a worse
summarizer and no ability to reuse the prior summary chain.

Compaction's file tracking walks assistant tool calls, extracts the `path`
argument, and buckets `read` against `write`/`edit`, chaining the lists across
successive compactions by parsing them back out of the previous summary. Files
touched through `bash` are invisible to it. Handoff consumes the same tracking
and should converge on the same concepts as compaction semantics are revised.

## Todos

Harness-owned state, mutated through a tool call, recorded as session events.

- One list per session. Subagents get their own lists and have read-only access
  to their parent's.
- Recorded as events in the session log, so replay, resume, and rewind work on
  todos for free.
- Rendered in the TUI.
- Carried verbatim across a handoff.

The list reaches the model through tool results and, across a boundary, through
the handoff seed. It is deliberately **not** injected into the system prompt: a
mutable list there would invalidate the conversation's cached prefix on every
mutation. Artist already avoids this shape for skills, riding them on the user
turn to keep the preamble a stable cache prefix; a per-turn todo injection, if
one is ever wanted, belongs there too.

Writes are whole-list snapshots rather than deltas, so replay is
last-writer-wins per owner and rewind stays correct without folding an edit
history.

## Fallback

A profile carries an ordered list of routing candidates. Each candidate is a
full routing triple, so a fallback may cross providers, models, and thinking
configurations at once.

```yaml
candidates:
  - provider: chatgpt-personal
    model: gpt-5
    thinking: {mode: on, level: high}
  - provider: anthropic-work
    model: claude-opus-5
    thinking: {mode: on, level: high}
  - provider: ollama-local
    model: qwen3-coder
    thinking: {mode: off}
```

Ordered, not round-robin: candidate 0 is always preferred while healthy. Three
consecutive failures on a candidate trip it; any success resets the count. A
tripped candidate is skipped until its cooldown expires, at which point it is
retried in its normal priority position. Breaker state is per process and
in-memory — a stale on-disk record outliving the outage it describes is worse
than re-learning the outage once per launch.

Failover happens at turn boundaries (invariant 8). Artist executes tool calls
inside the streaming loop, so a provider failure after three `bash` calls have
run cannot be recovered by replaying the turn on another candidate — the
completed calls are committed history, and the next candidate resumes from the
accumulated messages.

A downgrade is reported in the TUI. A silent move to a weaker model is
discovered hours later.

### Error classification is a prerequisite

Only genuine availability failures trip a failover: authentication, payment,
rate limiting, server errors, and transport failures. Malformed requests,
schema errors, and context-length overruns reproduce identically on every
candidate, so failing over on them burns the whole list and surfaces the last
error instead of the real one.

Artist cannot make this distinction today. Rig collapses every HTTP-level
provider failure into `CompletionError::ProviderError(String)` with no status
code, and `llm_provider::Error` covers only OAuth, configuration, and login
transport — not completion traffic. Classification must therefore be introduced
either at the HTTP layer, before rig wraps the response, or as a per-provider
interpretation of the error string. This is the first piece of fallback work and
it gates the rest.

### Candidates are not universally substitutable

A candidate that cannot serve the run is skipped rather than attempted, and the
reason is named in the exhaustion report. Implemented:

- **Unresolvable account.** A `provider:` naming no configured account, or an
  ambiguous display name, is skipped rather than silently falling back to the
  session's account.
- **No model.** A candidate with neither its own model nor one on its resolved
  account is skipped.
- **Thinking configuration.** Translation clamps rather than sends a request the
  provider will reject — disabled thinking above `high` effort drops the level
  on Anthropic, and providers with no reasoning controls receive none.

**Not implemented: context-window fitting.** A 32k-window local model listed
behind a 200k conversation will be attempted and will fail. Artist does not
track per-model context windows, so there is nothing to check against; adding
the check means adding model metadata first. Tool-calling-shape compatibility
is unchecked for the same reason.

### Exhaustion

When every candidate is tripped or incompatible, the run fails loudly. The
reported error is the most recent genuine provider failure, not the last skip
reason, and the failure names every candidate that was tried or skipped and why.
Exhaustion does not silently degrade and does not trigger a handoff.

## Implementation notes

Client construction is centralized in `rig_provider::RigClient::build`, which
takes any `SavedProvider`. That is all cross-provider routing needs: the
candidate loop resolves an account and hands it to the same builder the session
root uses, so a delegated or handed-off profile can run on a different account —
and a different provider — from the parent that spawned it. `stream_chat_with`
and `run_agent_with` stay generic over the concrete client type, so each arm
monomorphizes separately.

The seam for fallback and handoff already exists: `stream_chat_with` re-enters its retry loop
and rebuilds the client, preamble, tool set, and agent from the current seed on
every attempt, because stream rules needed that. Three triggers, one mechanism:

| Trigger | On re-entry |
|---|---|
| Stream-rule retry (exists) | Same profile, same candidate, injected reminder |
| Fallback | Same profile, next candidate, same seed |
| Handoff | New profile, its candidate, seed replaced by the payload |

## Where the code lives

| Concern | Module |
|---|---|
| Profile object, discovery, inheritance, tool policy | `artist-agent/src/profiles.rs` |
| Account lookup | `llm-provider/src/set.rs` (`ProviderSet`) |
| Per-provider thinking translation | `artist-agent/src/thinking.rs` |
| Classification, breaker, exhaustion report | `artist-agent/src/fallback.rs` |
| Handoff tool and payload | `artist-agent/src/handoff.rs` |
| Todo store and tool | `artist-agent/src/todo.rs` |
| Session root, candidate loop, handoff loop | `artist-agent/src/lib.rs` |
| Client construction for a resolved account | `artist-agent/src/rig_provider.rs` (`RigClient`) |
| Subagent dispatch and candidate loop | `artist-agent/src/delegate.rs` |
| `handoff.performed`, `todo.updated`, `active_profile` | `artist-session/src/{event,replay}.rs` |

## Open questions

- **Context-window fitting** for fallback candidates, which needs per-model
  metadata Artist does not yet track. See Candidates are not universally
  substitutable.
- **Migration** from the existing `subagents.toml`: the file is no longer read
  and is not deleted. Its per-role prompts under `prompts/subagents/` are
  likewise orphaned rather than removed.
