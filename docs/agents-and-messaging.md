# Agents, identity, and messaging

## Status

Specified, not implemented. This document covers three things that turned out
to be one subsystem: what an agent *is* once there are many of them, how they
are named and addressed, and how they talk to each other.

It also absorbs the subagent/task distinction: they are the same primitive here,
and `docs/profiles.md` should be read as describing how a *profile* is chosen,
not what a running agent is.

The decisive architectural choice is:

> A subagent and a background task are one primitive: a **job**. Anything that
> starts one can background it or wait on it, and a handoff inherits it. The
> only difference is that a subagent is agentic. Identity and addressing are
> properties of the job, not of the tool that created it.

## Core invariants

The implementation is invalid if any of these invariants is violated:

1. One identity per agent. The artist name is a rendering of the actor id
   through a durable mapping, never a second independent id.
2. A name is bound for the life of the session that holds it, across handoffs,
   and is released only when that session can no longer be resumed.
3. Names are unique per machine at any instant. Allocation is therefore a
   filesystem-level operation, not a per-process one.
4. Identity appears in the system prompt only as a trailing block, after every
   byte that is shared between agents.
5. A message is content, never policy. Inter-agent messages are delivered
   through the same channel as user steering and never as system-role content.
6. A group is a durable set with a durable creator, not a live predicate. The
   selector is evaluated once, at construction.
7. `reply` addresses the last communication the caller was *made aware of* —
   bound at injection time, never read from a mailbox at call time.
8. Any tool that blocks on another agent releases its delegation seat for the
   duration of the wait.
9. Message delivery reuses the steering delivery ledger. There is exactly one
   mechanism for "was this injected already", shared with TTSR.
10. There is one wait primitive. `await` and the subagent `wait` mode are the
    same code path or one is an alias for the other.

## Jobs: subagents and tasks unified

Today `delegate.rs` has a `subagent` tool with modes (`run`, `start`, `wait`,
`status`, `read`, `cancel`, `list`) and a `DelegateJobs` registry, while
`docs/build-coalescing.md` describes a separate notion of a long-running tree
job. These are the same shape and should be one registry.

A **job** is anything with an id, a lifecycle, and a result:

| Property | Applies to a subagent | Applies to a build |
|---|---|---|
| Can be started in the foreground (blocking) | ✅ | ✅ |
| Can be backgrounded and awaited later | ✅ | ✅ |
| Reports a typed termination reason | ✅ | ✅ |
| Scoped to the project, so a handoff cannot lose it | ✅ | ✅ |
| Runs a model and consumes a delegation seat | ✅ | ❌ |

The last row is the only difference, and it is a property of the job's kind
rather than of its interface. `await` therefore takes job ids without caring
which kind they are — waiting on two subagents and a test run in one call is
the natural expression of "tell me when everything I kicked off is done".

**Handoff inheritance already works, and is broader than a handoff.** No seat is
ever inherited: every child claims its own in `run_agent`, and the
`parent_permit` threaded into a child's environment is *that child's* seat,
carried down so the child can yield it when it in turn blocks on a grandchild.
Nothing about a handoff touches a running job.

The registry is `DelegateJobs::for_project`, backed by a
`OnceLock<DashMap<PathBuf, Registry>>` keyed by project root — so jobs are
scoped to the **project**, not to the session or the profile. A handoff keeps
them because it never had the power to lose them. `docs/profiles.md` invariant 4
is untouched: the outgoing agent is still terminal, and jobs were simply never
its property.

The consequence worth naming is the one that scoping creates rather than
solves: **two sessions in the same project share one job registry and one
delegation semaphore.** Session A can `list`, `read`, `wait` on, and `cancel`
session B's background subagent, and `max_concurrent` bounds the two of them
together. That is the correct default — it matches the "every agent in this
repository" addressing scope — but it is a sharing decision that was never
explicitly made, and `cancel` across sessions is the sharp edge.

## Identity

### Why names at all

Actor ids are `a-7f3` — short random tokens. They tokenize poorly, are hard for
a human to hold in working memory, and read as noise in a transcript. Artist
names from `docs/artists.md` are famous, tokenize well, and a human can tell
Monet from Bach at a glance. The roster is **736 unique names** with no
duplicates.

The name is for addressing and display. It is not a second identity — invariant
1 — because two identities for one thing is exactly the divergence class that
`crates/artist-agent/src/tool_set.rs` exists to prevent. The actor id stays the
internal key for workspace ownership, todo ownership, and log lineage
(`{conversation_id}:delegate:{actor}`); the name is a rendering of it.

### Allocation

Names are unique per machine, which makes allocation a shared-state problem: an
artist session is a process, and several run at once. The per-process
`OnceLock<DashMap<..>>` pattern used by `profiles::project_semaphore` cannot
express this. Allocation needs an on-disk registry with locking.

That registry is also what `tell`/`query` need in order to route a name to a
live session — which is why **identity allocation and message routing are one
subsystem rather than two**. A design that builds them separately will build two
directories of live agents and they will disagree.

**Release** happens when a session can no longer be resumed — not when it goes
idle, not when the process exits. A resumable session in history still owns its
name, because a user returning to it expects to still be talking to Monet.

**Exhaustion degrades rather than fails.** With 736 names, exhaustion requires
hundreds of concurrent live agents. If it happens, a new agent's name is its raw
actor id. Addressing still works — ids are addressable — and only the aesthetic
degrades, at exactly the scale where a human has stopped addressing agents
individually anyway. No suffixing scheme, no `Monet-2`, no mechanism.

### Where the name lives

Identity is true of every agent in every mode, so it belongs in the system
prompt rather than in a user turn or a tool description:

> You are {NAME}, an Artist in the so-named agentic coding harness.

**It must be the last block of the system prompt.** Prompt caching is a prefix
match: the cache key is the exact bytes up to a breakpoint. A name at the top
gives every agent a different prefix and no two agents ever share a cached
prompt. A name in a trailing block, with the breakpoint on the last shared
block before it, keeps the whole shared body cacheable across every agent on the
machine and reprocesses only the name.

This matters most in exactly our case — a fan-out of subagents spawned together,
all sharing a base prompt. Anthropic's own documentation lists *"f-string
interpolating session/user ID into system prompt → per-user prefix; no
cross-user sharing"* as a caching anti-pattern; the agent name is that pattern,
and the block split is what defuses it.

Two related notes:

- **Session-start assignment is compatible with the existing invariant** that
  the system prompt never moves within a session (pinned by
  `the_system_prompt_never_moves_within_a_session` in `tests/agent_loop.rs`).
- **The Gemini explicit-cache path** (`gemini_cache.rs`, `cached_context`)
  bypasses the preamble entirely when a cached prefix is in use. A per-agent
  name in that preamble would defeat reuse the same way; the name block has to
  sit outside the cached content there too.

### Handoff

The name belongs to the **session**, not the profile. A handoff replaces the
profile and clears the context but the session persists, so Monet stays Monet
and becomes a reviewer. Addressing is continuous across a handoff, which is what
makes it safe for another agent to hold a name and message it later.

## The message surface

Four verbs for talking, two for waiting and returning.

### `tell(target, message)`

Non-blocking. Delivers to the target as a steer — the same channel a user prompt
steer uses — and returns immediately.

### `query(target, message)`

Blocks until the target sends something back to the caller, and returns that as
the tool result.

**A reply is any communication back to the caller**, not a specific tool. If B
answers A's query with a question of its own, that question *is* the reply that
satisfies A's block. The word "reply" here is the ordinary one; `reply()` below
is a distinct convenience for addressing.

**Delivery is a steer, not an interrupt.** Steering is less disruptive: the
target folds the message into its next turn rather than having its current work
torn down.

**A deadline is required.** Two-cycles resolve themselves — if A queries B and B
queries A, B's message to A satisfies A's block, A can then answer, and B
unblocks. Longer cycles do not: A→B, B→C, C→A leaves every agent waiting on
someone who is waiting on someone else, and no message reaches an agent that is
waiting for it. There is no general cycle detection worth building, so the
deadline is the answer.

**The wait must be cancellable.** "Cancellable" here means the block can be
ended from outside the tool: by the deadline, by the user interrupting, by TTSR
aborting the turn, or by the target dying. Without it, a blocked `query` is a
hole in the agent loop that nothing can get out of — the run cannot be cancelled
and the process cannot shut down cleanly. Concretely this is a `select!` over the
reply and a `CancellationToken`, the same shape
`ToolRegistryHandle::execute` already uses to cancel canvas calls when the turn
moves on.

### `gc(selector, message)` — groups

Sends to a well-defined group: everyone in this repository, every subagent of X,
everyone running the `implementer` profile.

**A group is a durable set, not a live predicate.** The selector is evaluated
once, at creation; the result is materialised into a group with an id, and the
membership plus the creator are recorded durably. Later traffic addresses the
group id.

This is the load-bearing decision in the whole messaging design, because a
standing predicate breaks three things at once: membership differs between send
and delivery, "reply to the group" has no defined audience, and every message
re-runs a query over mutable state. It also resolves the manager case — a
manager starting a group of their reports is not in the selector that defines it
— not as a special case but as a consequence: the creator is a member because
membership is recorded, and recording it is what groups *are*.

Materialisation also de-risks expressiveness. Because the selector runs once, an
arbitrarily powerful selector language costs one evaluation rather than one per
delivery, so the expressiveness question can be answered later without
re-architecting.

### `reply(message)`

Addresses the last communication the caller was **made aware of** — the same
audience, whether that was one agent or a group.

**Bound at injection, not at call.** A message sitting in the inbox that has not
been injected into the caller's context yet must not become the target of a
reply the caller writes before seeing it. So the reply target is captured when a
message is *delivered into context* and carried as part of that turn's state; it
is never read from a mailbox when the tool is invoked.

Two edges to settle:

- Nothing ever delivered → `reply` fails cleanly rather than guessing.
- Two messages delivered in the same injection batch → needs a stated rule
  (last in batch), or `reply` takes an optional explicit target.

### Discovery

`tell(target)` presupposes knowing the target exists. Discovery is a **directory
read with tree knowledge**: scoped to the current repository by default, with
wider scopes available — inter-repository coordination is a real want, as herdr
demonstrates.

The directory is the same on-disk registry that allocates names. Scope is a
filter over a machine-global namespace, not a separate namespace per repo.

### `await(jobs…)`

Blocks until the named jobs finish, and returns each one's conclusion text plus
a typed termination reason: finished naturally, failed with a provider error,
cancelled by the user, timed out, superseded.

The motivating case is a parent that spawns five research subagents and wants to
sleep until all five reports are in. It takes several ids and waits for all of
them.

**This is the same primitive as `subagent mode:"wait"`** — see invariant 10.
Shipping both is the exact bug class that `tool_set.rs` was written to close:
two mechanisms that must agree and silently drift. The termination reason should
be the same enum the session log already records, for the same reason.

Like `query`, `await` blocks and therefore yields its delegation seat.

### `yield(value)` — structured return

Subagent-only. Ends the run and returns a schema-conforming object to the parent
instead of prose.

**Implemented as a forced tool call, not a provider `response_format`.** This is
the portability decision. Structured output is not uniform across providers —
OpenAI's `json_schema` strict mode, Anthropic's tool-forcing, Gemini's
`responseSchema` each accept a different JSON Schema subset, and the ChatGPT
subscription transport may expose none of it. Layer `candidates` failover on top
and a single run can move between providers mid-flight, so a schema that works
on candidate 1 and 400s on candidate 2 turns failover into a correctness hazard.
Every provider we target supports tools; a synthetic return tool whose parameters
*are* the schema has one dialect and validation we own.

It drops into the existing structure unchanged: a `Tool::Yield` variant in
`tool_set.rs`, constructed only when the run carries an output schema, absent at
the session root — the exact mirror of `handoff` being `None` in a child.

**Schemas belong to profiles, not to call sites.** A parent authoring a schema
per call is guessing the shape of an answer it has not seen, which produces a
confidently-filled-in wrong structure instead of a hedge. A `reviewer` that
ships a fixed `findings[]` shape returns the same thing regardless of who spawned
it. This is also the answer to the profile-flatness problem recorded in Open
questions: a per-role return schema is a real differentiator between `explorer`,
`planner` and `reviewer`, which today differ by one sentence of prompt each.

Two failure modes to design against:

- **Always include a free-text field, and make "blocked" a first-class
  variant.** The most valuable thing a child produces is often the thing the
  schema had no field for — "I couldn't do this because X". A rigid schema drops
  it silently and the parent proceeds on a lie.
- **Constrain only the final emission.** Schema-constrained generation across a
  long reasoning run degrades quality. The forced-tool shape gives this for
  free: the child reasons normally and structure applies once, at the end.

**What happens if a child never yields** is undecided and must be settled before
implementation — see Open questions.

## Delivery

### Reuse the steering ledger

TTSR aborts a run and re-injects from the current seed. A message delivered
during an aborted turn must not be delivered twice on the retry. This problem is
already solved for steering and pinned by
`ttsr_tests::delivered_steering_survives_abort_without_double_delivery`.

Inter-agent messages go through that same ledger. A second delivery path would
reintroduce a bug we have already fixed and tested.

This also gives invariant 7 its implementation: the reply target is captured
where steering records delivery, and rides the turn.

### Not the system prompt

Messages are never delivered as system-role content, even on providers that
support mid-conversation system messages. Two reasons:

1. **Authority.** A message from a peer agent is content. Delivering it as
   system-role content would give every agent operator-level authority over
   every other agent — Bach could instruct Monet with the weight of the harness
   itself.
2. **Layer.** The system prompt is identity and policy, fixed for the session. A
   message is an event. Mutating the prompt to carry an event is the wrong shape
   regardless of what it costs.

There is also no caching argument for it: prefix caching only prices the prefix,
and a message appended at the tail of the message list invalidates nothing.

### Interaction with the delegation semaphore

`query` and `await` block. An agent blocked on another agent is not working, and
`crates/artist-agent/src/delegate.rs` already has the primitive for this:
`PermitSlot::yield_seat` / `retake`, built so that uncapped delegation depth
cannot starve. The same reasoning applies verbatim — N agents blocked in `query`
while holding seats fills the semaphore and nothing runs. Invariant 8 is that
rule.

## Where the code would live

| Concern | Module |
|---|---|
| Name roster | `docs/artists.md` (736 entries) |
| Machine-global registry: allocation, release, directory | new, shared across processes, on-disk with locking |
| Message routing | same module — see Identity → Allocation |
| Group materialisation and durable membership | same module |
| `tell` / `query` / `gc` / `reply` / `await` / `yield` tools | new, registered through `artist-agent/src/tool_set.rs` |
| Availability gating (`yield` child-only, etc.) | `ToolEnv` in `artist-agent/src/tool_set.rs` |
| Delivery and the double-delivery ledger | `artist-agent/src/steering.rs` + `ttsr.rs` |
| Seat yield/retake while blocked | `artist-agent/src/delegate.rs` (`PermitSlot`) |
| Job registry (subagents + tasks) | `artist-agent/src/delegate_jobs.rs`, generalised |
| Identity block in the prompt | `artist-agent/src/prompt_config.rs` |

## Open questions

- **What happens when a child never yields.** A run that exhausts its turns, or
  decides the task was impossible, has produced no return value. This is the
  same hole as the free-text/blocked variant above and must be decided before
  `yield` is written. Note the reference implementation we looked at
  (`oh-my-pi`'s `requireYieldTool` / `outputSchema`) does not document an answer
  either.
- **The job registry and the delegation semaphore are per-*process*, not
  per-machine.** Both are `OnceLock` statics keyed by project root
  (`delegate_jobs::REGISTRIES`, `profiles::project_semaphore`), so two `artist`
  processes on the same repository share neither. Two consequences: `await`
  cannot reach a job started by another process, and `max_concurrent` is
  enforced per process rather than per project — N processes give N times the
  configured concurrency on one machine. This is the same defect the name
  allocator has to solve anyway (see Identity → Allocation), so the on-disk
  registry built for names is the natural home for jobs too, and building it
  once for both is the recommendation.
- **Selector expressiveness for `gc`.** Materialisation makes this cheap to
  defer, but the initial predicate set still has to be chosen.
- **Runaway chatter.** A tells B, B tells A, indefinitely. No cycle detection is
  proposed; this is token burn of the same accepted class as uncapped delegation
  depth, and is recorded rather than solved.
- **Profile flatness.** The five shipped profiles occupy two distinct capability
  configurations — `default`/`worker` are unrestricted and identical,
  `explorer`/`planner`/`reviewer` are byte-identical read-only sets — differing
  only by one sentence of prompt and a description. Per-role `yield` schemas are
  the proposed differentiator. Separately: `delegate.rs` falls back to
  `"default"` for an unnamed subagent, which is the profile that adds *nothing*
  to the shared prompt; `worker` is the better fallback and that is a one-word
  change.
- **No shipped profile exercises `candidates`, `provider`, `model`, or
  `thinking`.** The routing half of the profile system ships dark and
  undemonstrated.

## Related docs

- `docs/profiles.md` — profile object, handoff, fallback, tool policy
- `docs/build-coalescing.md` — tree jobs, which this unifies with subagents
- `docs/architecture.md` — agent loop, event-sourced sessions
