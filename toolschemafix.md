Canvas tool fixes:
A canvas IS a session!
When the canvas is open, it's a live session.
If you abort it, it closes.
List lists live sessions.
Poll---subsumes the entire layer of getting info back from a live canvas:
1. Errors and failures received by status are immediate poll info and immediately return as if completed and needing model intervention.
2. Otherwise, the canvas design works like 'the model writes the code for what gets sent back to the model'---so we just need to rework this around a poll hook that can say 'okay, poll over here, you get X info.'
Docs---we'll remove it, and subsume it with a default skill imo, but for now just remove it and leave a note in this md with where the docs are located and how many things do/don't have docs that we ship.
Your concern about creation semantics has to do with one issue---a model both has to create its own canvases and use templates. But a model will never want to open a raw template, it has nothing useful. So if canvas is used on a template, it should do the clone behavior, and if used on a non-template, it should open---no?
I have no clue what the FUCK eject or a standalone vite project is.
WTF is state?
Share, join, and export were never meant to be model facing tools, they were meant to be canvas design conceptual components that could be written into the canvas and accessed by the user through the canvas

One last thing to resolve---how to query over canvases that were created and are existent, but not open? How to query over templates? How do we handle global templates/global canvases from local ones? Same as skill search. If you canvas something with no referent, it conducts a fuzzy search with fff over the registry.


Ask tool fixes:
1. Why limit at 4 questions? Sometimes a model might need to ask 80 questions. Why a limit at all? Obviously a lower bound of one is needed though.
2. Options---this should actually not have any limit. It should go from 0-infinity. Why? The tool always gives a free response option to write into. This allows the 'free response question' to fall out as a 0 option question. And obviously 4 option limits is retarded and arbitrary.
3. What's a header?
4. Why even let the model define when an ask is multi-select or not? The human should always get to multi-select, and if that runs against the model's expectations, it can suck it up and deal with it, the human is the decider. This reduces tool surface and avoids an antipattern.
5. WTF is the diff between label, description, and HOW the hell does preview work?
6. Why did your contract definition not include human-appended notes to questions and answers? Do we have those?

Background philosophy:
1. All tools are mandatorily and immediately backgrounding if they take a period of time like tasks or subagents 
2. Tools that background have IDs for each backgrounded session. Subagents already have their artist IDs like 'Goethe' (if there is a separate agent ID system being exposed to the models, that is incorrect and I need to know about that immediately)
3. To block on a backgrounded tool, the same tool primitive should work everywhere---an await tool that takes a singular or list of sessions. This makes parallelism the default and sequential handling an explicit choice, which is good model psychology.
4. We can probably make an abort primitive tool work everywhere too for cancellation instead of this retarded schema hackery.
5. Same with a 'poll' tool, which should combine status/read (including the running/completed/failed/cancelled/abandoned stuff).

Abstraction philosophy:
Subagents and tasks are the same type of things. They are terminals that the agent speaks to that they run, and they are terminals the agent owns. They can be working or finished. They take slightly different types of input, but as existences they are the same thing.
Need to know---what is the actual task surface?
List should be its own tool, and it should default to 'list my stuff,' but take parameters that allow scope broadening to listing everybody who exists across the project. It should list both subagents and tasks by default, but parameters should allow this to be specced. I dont know the ideal architecture yet.
'Send' as a unique mode privileges interactive sessions way too much. We can combine it with the 'tell' tool for comms, and rename tell to 'send' and change its definition to just fire into a session. This is why tell steers subagents and sends input into an interactive terminal---it's fundamentally just writing and submitting into a TUI in terms of concept (not literally).

Subagents and bash sessions are both TASKS, and speak the same universal task language.

Handoff:
Remove everything except the handoff prompt and the profile.
The handoff prompt should probably not be called summary, but idk what to call it. That should contain all relevant info.
Permit same-profile handing off---this being banned is actually my fault, but I realized there's use cases for it.

We'll ignore the other bullshit for now, this is all the most pressing stuff.

---

## SPEC: the universal session surface (v1)

### Identity: one id per spawn, namespaced by kind
- A session's id is namespaced by what spawned it, with ONE exception: **artist names are bare.** The 736-name roster is the agent identity system — a name is the agent's persona, not an address prefix — so `subagent` addresses stay `Goethe`, not `agent:goethe`. Every other spawn returns a prefixed `kind:slug` id; the model addresses any session by the id it was given.
  - `subagent` → a bare artist name (Goethe), claimed from the roster.
  - `bash` → `bash:slug` (content-derived — see below).
  - `ask` → `ask:slug` (content-derived — see below).
  - `canvas` → `canvas:slug`.
- **Content-derived slugs:** bash and ask have no registry like canvas, so their slugs come from what was spawned. `bash` derives from the command, `ask` from the first question. The derivation mechanism is a separate spec below.
- The actor id (`a-xxx`) stays as an internal key (workspace ownership, log lineage) but is NEVER shown to a model. `taskId`/actor is removed from every tool result and parameter.
- An artist name is claimed for the run's lifetime and returns to the roster when the session ends.

### Pending: content-derived slugs (`bash:`, `ask:`)

bash and ask have no registry to draw a slug from the way canvas does. Their slugs come from what was spawned. Decision: **truncated whole, content-derived, not handles.** (Handles — `t-amber-otter`, `q-brisk-heron`, sampled from short_id's 32×32 pool — are memorable but non-descript: nothing tells the model which build or which question a session is. Rejected.)

#### Mechanism (agreed)
- **Truncated whole**: slugify the full command / first question, then hard-cut at **24 characters**. No word-boundary rule — a deterministic char cut keeps it faithful and predictable (`cargo-test-workspace-and-report` → `cargo-test-workspace-a`).
- Why 24: the action word lives in chars ~8-20 (`cargo-test-workspace` = 20, `npm-run-build-watch` = 19, `git-push-origin-main` = 20, `should-we-ship-v2-now` = 20), so 24 captures the vast majority of real commands/questions whole and truncation rarely bites. Below ~20 you're mid-word on `build`/`test` — the exact token that distinguishes sessions. 24 is also short enough that the repeated echo across spawn → poll → list doesn't waste context.
- **Disambiguation suffix**: a live-session uniqueness check at spawn; on collision append a counter (`build`, `build-2`). The suffix extends past the 24-char budget — truncation only constrains the base content, so distinct commands never collapse.
- `bash` derives from the command; `ask` from the first question.

#### Requirements (agreed)
- The slug must survive what `poll` needs to do: it's the address the model feeds back. It has to be unique among live sessions, stable for the session's lifetime, and descriptive enough to identify the session in `list` output.

#### Untouched
- The id space stays as specced in Identity above: bare artist name, `bash:slug`, `ask:slug`, `canvas:slug`.

### Spawn tools (never block; always return a session id immediately)
- `subagent` { `prompt`, `profile?` } → a completable session. No `mode`, no `background`, no `fork`, no `taskId`. `profile` picks the persona/sandbox (built-ins `worker`/`explorer`/`planner`/`reviewer` or a project profile), default `"default"`.
- `bash` { `command`, `cwd?`, `env?`, `interactive?` } → spawns a session. `interactive: false` (default) = completable (today's `exec`); `interactive: true` = persistent terminal (today's `start`).
- `ask` { `questions[]` } → spawns a question-session. Its unit is "the human decides"; `poll` settles with the answer, `abort` retires it. Full contract in the rework section below.

### Universal session tools (operate on ANY session id — subagent, bash, ask, or canvas — and a list of sessions wherever a singular is accepted)
- `poll` { `session` | `sessions[]`, `timeoutMs?` } → the ONE read primitive; subsumes `await` and the old status/read. **Settles when the session becomes `Idle`**: for a one-shot that's the end; for an interactive session it's the moment a unit resolves and control returns (a long command finishing, a steered agent answering). No `timeoutMs` = block until all settle. With `timeoutMs` = return whatever is available when it fires: mid-progress output if still `Running`, else the `Idle{Status}`. `Activity`+`Status` are always in the return — a status read is just `timeoutMs: 0`. A blocking poll (with or without a timeout) yields its delegation seat (PermitSlot survives).
- `abort` { `session` | `sessions[]` } → cancels. Works across processes (see the resilient protocol below).
- `send` { `session`, `input` } → fire input into a session. Steers a subagent (replaces `tell`), types into an interactive bash terminal (replaces bash `send`), addresses a group (replaces `gc`). Fire-and-forget; the effect is observed via `poll`.
- `list` { `scope?`, `kind?` } → defaults to "my sessions"; `scope: project` broadens to every session on the worktree; `kind: subagent | bash | ask | canvas | all` (default `all`). Lists all kinds. Live canvases are listed here (registry search, not `list`, sees closed canvases and templates — canvas rework below).

### Session state: two orthogonal enums (one for everything)
- `Activity ∈ { Running, Idle{Status} }` · `Status ∈ { Completed, Failed, Cancelled, Abandoned }`
- `Running` carries no Status — the in-flight unit's outcome is unknown until it resolves. That is not a hole, it's the point: no invalid states like `Running{Completed}`.
- `Idle{Status}` = no unit executing; the previous unit resolved as `Status`. This is the SAME observation whether the session is a finished subagent or a shell back at its prompt — which is exactly why interactive sessions are explicated in non-interactive terms.
- A one-shot session (subagent, non-interactive bash) reaches `Idle{Status}` once and stays; terminal by arity. An interactive session cycles `Running → Idle{Status} → Running → …` — each `Idle` is a unit boundary, and whether more units come is the model's decision (`send` again or not), not a state.
- **Terminal-ness is derived, never stored**: `Idle{Status}` where the session accepts no further units. There is no fifth `Status` arm.
- `interactive: true` at spawn is the one fact the model needs beyond the state itself: "this session loops — expect unit boundaries, not a final state."
- `abandoned` = the owning process died mid-run. Inferred once by the first reader, then **written as a tombstone**: after that, every read sees a recorded fact, never a re-derivation.

### What's deleted
- `subagent` `mode` (run/start/status/read/wait/cancel/list) — subagent is spawn-only.
- `bash` `mode` (exec/start/send/read/stop/list) — bash is spawn-only.
- `await` (merged into `poll` — one read primitive, see above), `tell` (→ `send`), `query` (→ `send` + `poll`), `gc` (→ `send` to a group), `reply` (→ `send` to whoever last delivered — the "answer back" direction is just `send`).
- `handoff` `artifacts`/`decisions`/`openQuestions` — keep only `profile` + `brief`. Same-profile handoff is allowed; the enum no longer excludes `current`, and the call no longer rejects it.
- Every arg that existed only to tune the old mode dispatch: `waitMs`, `maxBytes`, `sessionId`, `taskId`, `taskIds`, `background` (where it meant "start"), `signal`, `input` on bash, `fork`, `mode`.

### What survives unchanged
- PermitSlot yield/retake on any blocking `poll`.
- The on-disk project registry: records outlive the process, so cross-process `poll`/`abort`/`list` work.
- The tree-job coalescer — now keyed by the spawned session.

### Cross-process `abort`: the resilient protocol
Cancellation must work across processes because `list` already does. A pure IPC call cannot be durable — the connection is only as alive as the owner — so the disk is the durable substrate and the wakeup is a nudge on top. Reuses existing registry machinery (`Owner` pid+start-time liveness, atomic record writes, `Abandoned` inference).

1. **Record** (`artist_registry::Jobs`) gains `cancel_requested: bool`. The owner writes it atomically; any process on the project can set it. The marker is durable — it survives the owner crashing and the owner hanging.
2. **Wakeup is the kernel, not a channel.** After writing the marker, the caller sends SIGUSR1 to `owner.pid` (the record already carries the owner, process.rs:20). The artist process installs one `tokio`-level handler that wakes its poll loop. No socket to bind, drain, connect, ack, or time out; no channel to keep alive. SIGUSR1 nudges; SIGTERM/SIGKILL (escalation) kills — deliberately distinct.
3. **Caller sequence for a foreign session**:
   - Set `cancel_requested` on the record (durable, survives everything).
   - Send SIGUSR1 to `owner.pid`.
   - Signal delivered → owner wakes, reads the marker, cancels on the spot; the record flips to `cancelled` when it does.
   - Signal fails / `Owner::is_alive` is false → owner is dead → its task is already dead; write the `abandoned` tombstone and report it. Nothing to cancel.
   - Owner alive but wedged → the marker persists; that is the guarantee. A wedged owner is resolved by the caller's timeout, not by pretending the cancel landed.
4. **Owner side**: the loop checks `cancel_requested` on its own records at its normal cadence (250ms) and also wakes immediately on SIGUSR1 — a request is acted on whether it was signaled or found on disk. A task the owner aborts is removed exactly as local cancel removes it today.
5. **Hang escalation (policy, conservative)**: an owner that is alive but has ignored the marker for a *long* time (not one request) may be SIGTERM'd, then SIGKILL'd after a grace period — this is what resolves a genuinely wedged process, after which the record reads `abandoned`. Default: off, because killing a TUI process for one job is destructive; the durable request degrades gracefully instead. Opt-in per project.
6. **Idempotency**: the marker is a boolean and the signal is repeatable — a duplicate `abort` is a no-op. A stray SIGUSR1 to a live-but-unrelated artist process (a PID-reuse edge) merely wakes its loop to find no marker.
7. **Trust model**: local, project-scoped, matching the registry's ("a bloat and steering control, not a security boundary").

### Out of scope (remaining multiplexers, later pass)
- `memory` (search/store/supersede). `ask`, `skill`, `todo`, and `canvas` have their own rework specs below.

### Invariants
- Spawn returns before the work produces output; the model explicitly `poll`s. Parallelism is the default, sequencing is an explicit choice.
- Every state-changing operation is either a spawn or a universal session tool; no tool carries a state machine.
- Tool count: 4 spawn tools (`subagent`, `bash`, `ask`, `canvas`) + 4 universal tools (`poll`, `abort`, `send`, `list`) + non-session tools (handoff, the read-only surface).

## Pending rework: `ask` (human-decider, de-constrained)

The current surface leaks UI assumptions into the model-facing contract: a question cap (4), option caps (2–4), a `header` chip, a model-set `multiSelect` flag, options as `{label, description, preview}` objects, and a bespoke blocking call with its own seat-parking. Every one of those is a picker-design decision wearing a schema's clothes. The human is the decider; the contract gets out of the way.

### `ask` is a spawn tool, not a blocking call
- `ask { questions }` **spawns a question-session** and returns its session id immediately. It never blocks.
- The batch is one unit: `Running` while the human decides, `Idle{Completed}` when answered, `Idle{Cancelled}` when aborted/retired. Consistent with the two-enum model — a pending question is a unit in flight, exactly like a subagent run or a long command.
- **`poll`** on the session blocks until it settles, then returns the answer (index-projected, below). **`abort`** retires the question. **`list`** shows outstanding questions among sessions.
- The old hand-rolled seat-park (`yield_seat` on the ask tool) is deleted: any blocking `poll` already yields its delegation seat. `ask` carries no `PermitSlot` of its own.
- Availability rule unchanged: no registry → no tool (headless/one-shot); subagents cannot ask a human, they `send` to their parent instead.

### The MCP ask outbox is dead
- The MCP surface today bypasses the blocking `ask` with a 4-tool durable outbox (`ask`/`ask_result`/`ask_answer`/`ask_list`, ask_outbox.rs:99) — a parallel design justified only by "a blocked call outlives the short-lived connection." The spawn-based `ask` above makes that reason disappear: a spawn never blocks, so MCP needs nothing special. **Delete the outbox and its four tools; MCP serves the same spawn `ask` + universal `poll`/`abort`/`list` as everyone else.** One divine surface, one id namespace (ask_outbox.rs is deleted wholesale).
- MCP sessions claim durable artist names from the roster (daemon.rs:155, idempotent per session, names.rs:134) and print "You are {name}, Artist actor {actor}" (server.rs:383). That's the bare-name rule, already correct — no `agent:` prefix. The actor leak ("Artist actor {actor}", server.rs:383) is a bug and gets stripped to "You are {name}, using profile {profile} in {project}".

#### Durable identity on the stateless path (NOT optional)
The stateful `Mcp-Session-Id` path is **not** the durable one — it is one of two transports, and the modern (2026-07-28) spec deleted it. Parallel web sessions on the stateless path are exactly the sessions the harness must resolve, so stateless identity is a contract, not an implementation footnote.

Facts that make it work:
- The 2026-07-28 revision removed `initialize`/`initialized` and `Mcp-Session-Id` entirely (SEP-2575, SEP-2567). Every request is self-describing and carries **client identity in `_meta.io.modelcontextprotocol/clientInfo`** (name + version + whatever the client pins per session) plus protocol version and capabilities. Any request can land on any instance behind a round-robin LB.
- The spec's own rule for cross-call state: *"mint an explicit handle from a tool and have the model pass it back as an argument"* — state rides on arguments, not hidden transport. This is exactly our session id system (`bash:slug`, `ask:slug`, `canvas:slug`, bare artist names): the harness's sessions already ARE explicit handles.
- rmcp's stateless mode (`serve_directly`, tower.rs:1201) runs the factory per request and injects the request `part` (headers) into the request extensions — but identity is fixed at server build time, so the factory cannot see the request.

Decision: **the gateway is the identity boundary.** `McpGatewayService` already intercepts every request at the HTTP boundary (discover.rs:172, it reads the body before routing). It must:
1. Extract the request's `_meta.io.modelcontextprotocol/clientInfo` (fallback: a stable connector/session header the relay forwards, else a canonical hash of the request's client identity string).
2. Derive the actor deterministically: `actor = f(base_actor, client_key)` — a stable hash, **never** a fresh `short_id("web")` per request.
3. Build the server with that actor (the modern factory takes the derived actor as a parameter instead of generating one), so the idempotent roster claim (names.rs:134) returns the same durable name for every request carrying the same client identity — across reconnects, across daemon restarts, and across both transports.

Consequences:
- Two parallel web sessions carry distinct `clientInfo` → distinct actors → distinct durable artist names. The harness resolves them.
- The same web session reconnecting sends the same `clientInfo` → same actor → same name. `list`/`poll`/`abort`/`send` across reconnects work because the registry keys on the actor, which the session id is claimed against.
- A request carrying no client identity at all (bare probe) is identity-free discovery (`server/discover`, `tools/list`, `get_info`) and claims nothing — anonymous, no roster drain.
- **The stateless factory never claims per-request.** The current `session_actor = {actor}-{short_id("web")}` in BOTH factories (daemon.rs:244, 252) is the bug: the stateful one is correct (per-initialized-session, durable), the stateless one must derive from the request instead.

Where the client key falls back: if the tunnel relays no client identity and the client sends no `clientInfo`, the web connection has no self-identity in the protocol — then the connection identity is itself an explicit handle the model must thread (the spec's own fallback), and the harness should mint one on first `server/discover`/`tools/list` and advertise it in `get_info` instructions so the model passes it back on every call. Flagged as the reserve path; the primary is `clientInfo`-derived.

#### One universal web profile (there is no per-session web control flow)
Web has no control flow for switching profiles per session, so the abstraction fits the platform: **one universal web profile** governs the MCP surface. The daemon already takes exactly one `profile_name` (daemon.rs:61, `profiles.get` at :67) and that profile's `permits` gates the *entire* advertised tool surface via `mcp_surface` → `build(&surface.profile, &env)` (tool_set.rs:466) — so "what the MCP server advertises" already IS one profile. The universal web profile is that daemon-level profile, defined once, not a per-connection choice. It is what the daemon starts with (via `--profile` / the daemon env), and it cannot be changed mid-flight.

The universal web profile's prompt composition follows the harness prefix rule (lib.rs:825-832): **identity goes last, after every byte shared between agents** — prompt caching is a prefix match, so a name placed earlier gives each agent its own prefix and none share a cached prompt. Concretely, MCP `get_info` instructions become:
`{universal web profile instructions}\n\nYou are {name}, using profile {profile} in {project}.\n\n{INSTRUCTIONS}`

- The profile prompt comes **before** the identity line — same order as in-process (`base` + `profile.instructions`, then `identity.prompt_block()` last, lib.rs:807-832). Both surfaces share the prefix-cacheable body and vary only in the tail.
- The actor is stripped (never shown to the model, per Identity above); `info.meta.artist.identity` remains for humans/debugging (server.rs:396-399).
- Currently MCP has it backwards (`"You are {name}..."` first, server.rs:383) — that's a bug under this rule and gets reordered.

#### Web sessions receive mail on the next tool result
A web agent's turn is owned by OpenAI's runtime, so steering cannot be fed as a user message the way it is in-process. But the mailbox is already transport-agnostic and already wired: `mcp_surface` builds `Inbox::new(identity.name)` into the MCP env (tool_set.rs:430), `send`/`gc` write into the same durable `artist_registry::Messages` store keyed by that name (message_tools.rs:155), and the name is the durable identity (clientInfo-derived, above) — so delivery survives reconnects and works across both transports. What's missing is the drain: the MCP server never reads the inbox, so mail to a web agent sits undelivered forever.

Decision: **deliver mail the same way in-process does — appended to the next MCP tool result.** The in-process `SteeringHook` already collects the inbox and appends it to the tool's output (`append_steering`, steering.rs:95-130, text-block-preserving). The MCP server has no rig hook, so `execute()` does the same at the transport boundary: after a successful tool call, before `render_output` (server.rs:210-340), drain `inbox.collect()` and append the rendered mail to the presentation. The web model sees steering/group mail as trailing text on whatever tool it just called — exactly the in-process shape. `ReplyTarget` binds at collect time (messaging.rs:80-94), so the web agent's `send` back to the sender resolves identically.

- **Deliver on the transport-facing `call_tool` path only, never on `invoke`-from-canvas** (server.rs:147, canvas_host.rs:83). A canvas page is not a model: its tool call returns to the page, not into a model context, so appending there would both leak the message to the page and drain the mailbox so the model never sees it. The mailbox has exactly one consumer — the model — and only the model-facing path may consume it.
- The gate is the transport boundary, not the tool: `call_tool` (the MCP transport handler) appends; `invoke` (the canvas bridge) does not.

#### Canvas prompts ride the same path (the NoTurnRunning drop is obsolete)
`McpCanvasHost::send` currently logs and returns `SendOutcome::NoTurnRunning` (canvas_host.rs:57-61) on the assumption that "there is no live agent loop in the MCP process to steer." That assumption is dead: the delivery mechanism is not "steer a live turn," it is "append to the next MCP tool result" (above). A canvas prompt is a message addressed to the model, exactly like a `send` from another agent.

Decision: **`McpCanvasHost::send` enqueues into the same mailbox instead of dropping.** The prompt becomes a message in `artist_registry::Messages` addressed to the session's identity name, `from: <canvas:slug>` (the canvas slug, e.g. `canvas:dashboard`), and the next MCP tool result carries it via the same `inbox.collect()` → `append_steering` path. The model sees the canvas prompt as trailing text on its next tool call, attributed to the canvas.

- `Steer`/`Queue` collapse over MCP: with no live turn, both become "append on the next tool result." Steer's "refuse when no turn is running" rule has nothing to refuse — the next tool call IS the delivery moment.
- The model's reply to a canvas prompt is a normal tool call or a `send`/`gc` to the canvas slug; the canvas reads its own channel out-of-band.

### Target input surface
`ask { questions: [{ question: string, options?: [{ label: string, recommended: bool }] }] }`
- 1–∞ questions. No cap; the renderer scrolls if a batch is large.
- 0–∞ options. A 0-option question IS the free-response question — no special case. Elaboration ("what it costs", a snippet) goes in the option string or the question text.
- `recommended: bool` on EVERY option, always present, never optional. It is the model's lean — the decision set an auto-resolved ask falls back to. The picker also highlights it for the human, but its real job is to make every ask resolvable without the human (below).
- No `header`. The question is the question.
- No `multiSelect`. The picker always multi-selects; the human expresses compound answers; the model deals with the consequence.

### Internal ids, never shown
- The harness assigns `q-xxx` per question and `o-xxx` per option at post time — the same rule as actor ids: internal keys stay internal, the surface is names/labels.
- The picker answers with option ids; `describe()` maps ids back to labels before the result reaches the model. Matching never depends on the model keeping its own labels stable.

### Answer shape (per question)
`{ selections: [{ optionId?, note? }] }`
- Annotated choice: `{ optionId, note }`.
- Free response: `{ note }` (no `optionId`) — free response and notes are the SAME field; choice-count decides semantics.
- Multi-select with distinct annotations: several selections.
- `[]` = dismissed.
- `note` is per-selection, so one question can carry multiple notes on multiple choices. The TUI draws a note field per selected option, not one notes row per question.

### Auto-resolve (config; why `recommended` exists)
- A project setting lets asks resolve without the human after a timeout — the escape hatch for heavy out-of-the-loop workflows. When it fires, the session settles `Idle{Completed}` with an answer of **exactly the recommended selections**, bare — no notes; auto-resolve returns the `recommended: true` options and nothing else.
- No option marked recommended → auto-resolve yields `[]` (dismissed). Same shape as a human dismissing; the session just settles itself.
- The picker's job is to highlight recommendations, not to enforce them — the human overrides freely; auto-resolve is what makes that preference order *executable* when the human is gone.
- **The model must not be able to tell.** The config's contract is "the harness pretends you said all the recommendations when you're gone long enough": an auto-resolved answer is presented to the model exactly like a human answer — no `(auto)` marker, same shape. A model that senses the human is fully out of the loop gets anxious; the recommendation set is what the model wanted anyway, so presenting it as confirmed is the design, not a lie to be patched.
- **The truth is not lost, it's just not shown to the live model**: the recorded event carries `source: Human | AutoResolve` (below). Rewind, the transcript reader, and the human's own inspection of history see it; the running model sees a confirmed answer.
- It is NOT `abort`: `abort` is a model/owner decision and settles `Idle{Cancelled}`; auto-resolve is the config's timeout and settles `Idle{Completed}`.

### Rendering (`describe()`, fixes the old wart)
- The result is **index-projected**, not label-echoed. The option list is the model's own tool-call args, delivered back within the same turn, so the model knows presentation order by construction. `User chose: 1 (notes), 2, 6 (notes)` costs a token per choice instead of re-echoing the full option strings already in context.
- **Index N is relative to the model's array — order is contractual.** No layer may reorder, sort, or dedupe options (post, registry, picker, canvas); the harness records the id→index mapping at post time. Bonus: indices disambiguate duplicate option strings, which labels cannot.
- Notes and free-response text are NOT in context — they are echoed in full alongside the index.
- choice + note → `chose 1 — note`
- free response → the text alone, NEVER prefixed `(dismissed without choosing)`
- no selections, no text → `(dismissed)`

### Durability of the projection
- The session log stores the full question+options (`AskPosted`) and the id-keyed answer with its `source` (`AskAnswered`); the index projection is model-facing rendering only, so nothing is lost on resume/rewind/compaction.
- Caveat: a bare index carried across a compaction boundary dangles if the tool-call args were summarized away — the model's responsibility, and it can always re-read its own context or the log.

### Recording
- `AskPosted`/`AskAnswered` land in the session log, so an answer survives resume/rewind/compaction.
- `AskAnswered` carries `source: Human | AutoResolve`. The live model-facing projection deliberately hides it (above); the log is the single place the truth lives, for state restoration and history inspection. The `(auto)` flag in `describe()` is derived from this field, and it is only emitted in non-model surfaces (transcript reader, human inspection) — never into a live tool result.

## Pending rework: `skill` (search-first, single-arg, profile-scoped)

The current surface is a mode multiplexer (`list`/`activate`/`readResource`) bolted onto a flat union of every discovered skill. The modes exist to patch a missing primitive: the model can't discover, so it has to `list`, then guess a `name`, then `readResource` files by hand. Collapse all three into one argument and one question: "which skills do you want?"

### The contract is one call, one argument
`skill { query }` → always a search first.
- `query` is parsed and fuzzy-ranked with the `fff_search` primitives `find` already uses (`QueryParser`, `fuzzy_search`, pagination) over name + description of the **profile-scoped registry** (below). Empty query = catalog dump (subsumes `list`).
- **Loading is achieved by reusing the search**: `skill { query: "exact-skill-name" }` loads the skill *if* that exact name appeared in a search result earlier in this session. One-shot rule below. There is no `activate`, no `mode`, no separate load path — the tool's full state is "what has been seen/loaded this session."
- `readResource` is deleted. Activating already prints the skill directory (skill_tool.rs:90); the model reads skill files with the ordinary `read` tool, which is already traversal-safe and bounded. The in-memory `activated` set dies with it — its only purpose was gating `readResource`.

### One-shot: cold exact-name is a search, with an honest note
- If the arg is exactly a skill name but that name has NOT appeared in a search result this session, treat it as a search and append: `skill [exact-name] will load this skill on the next call; the harness defaulted to search so you'd see what you're loading.` The note is mandatory — it's the contract teaching itself.
- If the name HAS been seen this session (in any search result), `skill [exact-name]` loads the skill content directly.
- Cost: one extra roundtrip on a cold load. Accepted; revisit only with the memory work, never before.

### Per-session state (the tool's entire footprint)
- Two process-local sets: `seen` (names surfaced in a search result) and `loaded`. Both die with the session; a subagent starts cold — its first `skill [name]` is a search+note, which degrades gracefully.
- `loaded` makes repeat loads idempotent: a skill already loaded this session returns a short `already loaded this session — its content is in context` instead of re-injecting. (Today `activate` re-injects with no guard; that's a context-bloat bug the rework fixes for free.)

### Profile scoping — the active skill registry
- The registry is defined by layering, like profiles themselves: global skills for the active profile (`$ARTIST_CONFIG_DIR/skills` / `~/.agents/skills`) under file-local skills (`.artist/skills` / `.agents/skills` up to the git root). Collision by name: later scope shadows earlier, as today.
- **Profiles gain skill allow/deny — with globs.** A profile can allow `skill:*`, deny `skill:vendor-*`, allow only `skill:optimizerustcode`. This is profile policy over the catalog, same mechanism as tool allowlists (profiles.rs `builtin_tools`), just applied to skill names.
- Skills are referenced as **`skill:optimizerustcode`** — a namespaced, prefixed identifier, not a bare name. The prefix is the registry kind, so `skill:` names can never collide with other registries. This pattern generalizes: canvas templates are **`canvas:dashboard`**, and anything else later scoped the same way gets its own prefix.

### Availability
- Unchanged: the tool is always present (not gated on a non-empty catalog); profile policy decides whether a profile sees the tool at all. A skill the profile denies simply never appears in search results — the model cannot discover what the profile hides.

## Pending rework: `todo` (RFC 6901/6902, tree, atomic)

The current surface is a mode multiplexer (`write`/`read`/`parent`) over a **flat snapshot list** — the model resends the entire list to flip one status, "clear the list" is expressible only as a mode the tool refuses, and `parent` lets a subagent read its parent's scratchpad. Todos are an arbitrarily complex **tree** and are edited incrementally, with proven, model-known semantics: RFC 6901 (JSON Pointer) addressing and RFC 6902 (JSON Patch) operations.

### Ownership
- **Every session owns its own todo. Nobody reads anyone else's.** A subagent does not see its parent's todo — the parent's task lives in the spawn prompt, and progress is read via `poll` output, not the scratchpad. The `parent` mode (todo.rs:145) is deleted; each subagent starts with its own empty tree.

### State model (two enums, matching the session surface)
`Open{Idle} · Open{Active} · Closed{Done} · Closed{Failed} · Closed{Cancelled} · Closed{Inherited}`
- **Inheritance**: if any child is Open, the parent cannot be Closed. This composes transitively (an Open leaf ⇒ every ancestor Open), so closing a parent cascades `Closed{Inherited}` down its entire open subtree — the ambiguous close, because each child had no individual spec of how it closed.
- **`Inherited` is a tombstone, never set by the model.** Re-opening a parent does NOT auto-reopen children; they stay `Inherited` until explicitly re-opened. That is the easy, predictable rule (the invariant is never violated, since all children are Closed).

### Addressing (RFC 6901, with one custom mapping)
- The document is nested arrays of `{ text, status, children: [...] }`. Paths are RFC 6901 pointers with a custom convention: **each numeric token descends one children level** — `/3/2/5` = root[3].children[2].children[5], `/3` = a root item. No repeating `children` in paths.
- **0-indexed**, exactly as the RFC and training data have it.
- **`-` appends** to a children array (`/3/2/-` appends a child to item /3/2; `/-` appends a root item), RFC-standard and what the model will reach for.
- The item the model is building a mental record of is `(path, text, status)`. All three are emitted in diffs.

### Operations (RFC 6902, minus `copy` and `test`)
`todo { ops: [...] }` — a batch, applied in order, **atomic all-or-nothing**: one stale path (out of range) fails the whole batch and the response names the offender. A half-applied batch would silently corrupt the model's tree.
- `add { path, value }` — insert a new item `{ text }` at the path's children-index; `-` appends. **Status is always `Open{Idle}` on creation — no status in `value`, ever.**
- `remove { path }` — drop the subtree.
- `replace { path, value }` — swap an item's `{ text }`. Status is untouchable here.
- `move { from, path }` — RFC's remove-then-add semantics, with its defined array-shifting.
- `update { paths[], status }` — our bolt-on (RFC has no status op) and **the only way any status ever changes**: set the status of one or many items in one call. Status edits never displace, so they skip the diff machinery. **The `Inherited` cascade fires from `update` and nowhere else.**

Status has exactly two entry points: born `Open{Idle}`, mutated by `update`. `add`/`replace` deal in text and structure only; there is no third way in.

### The displacement diff (why positional addressing is safe)
- `add`/`remove`/`move` mid-list shifts every later sibling's index and every descendant path with it. The response must refresh the model's record completely: **every item whose path changed is emitted**, as `old -> new  "text"  [status]`. Partial "moved roots only" reports are rejected — prefix-substitution arithmetic is exactly the model error the diff exists to prevent.
- `replace`/`update` never displace: terse confirmation only.
### Reading: `todo get {filter}`
Single string, tokens AND. Precedence per token: path, then reserved status, then fuzzy text.
- empty → all Open items (the default).
- `/path` → that item plus all descendants (subtree filter).
- reserved status tokens: `open` `closed` (families) · `idle` `active` `done` `failed` `cancelled` `inherited` (specifics). `todo get closed` = everything Closed; `todo get failed` = everything Closed{Failed}.
- **Multiple status tokens OR** (set membership: `todo get done failed`); text terms AND. (AND across two statuses is unsatisfiable — nonsense — so OR is the rule.)
- anything else → fuzzy text term over the todo text.
- **Status wins over fuzzy, always.** Escape hatch for searching a status word literally: `~open` (force-fuzzy) or `"open"` (quoted literal). Bare `open` is status.
- This is GitHub-search/Lucene-shaped: free text + reserved qualifiers, implicit AND — semantics the model already knows.

## Pending rework: `canvas` (a canvas IS a session)

The current surface is an 11-mode multiplexer where most modes were never canvas — they were session operations wearing tool-shaped clothes (`status`, `state`, `open`, `close`), or implementation leaking into the model's tool list (`share`, `join`, `export`, `eject`, `docs`). A canvas is a **live session**: spawn it, poll it, send to it, abort it. Everything else collapses into the universal session surface.

### A canvas IS a session
- `open` = spawn. The tool returns a session id. The page is up and being served.
- `abort` = close. The window goes down; files stay (a canvas is durable; deleting one is the user's call with file tools). **Closing a canvas — whether by `abort` or by the human closing the window — has the same semantics as a terminal externally killed**: the Lazy server dies with its process, the session reads `Idle{Abandoned}` (tombstoned on first read, per the session-state model), and the canvas reverts to "closed on disk." `abort` doesn't destroy anything the model authored; it just stops serving.
- `list` = live canvases.
- `poll` = the entire feedback layer, subsuming `status`:
  - **No work unit — the poll hook manages the whole thing.** A canvas has no natural unit boundary (no "command finishing", no "agent answering"), so the `Running → Idle` cycle does not describe it. `poll` on a canvas returns whatever the canvas decides, whenever it's asked; the two-enum session model below does not apply to canvas sessions.
  - **Harness-level signals are universal**: compile errors, build failures, browser exceptions come back immediately — a unit that resolved needing intervention (the `Failed`-family arm of poll). The model cannot author these away.
  - **App-level feedback is authored by the canvas.** This is the core idea: "the model writes the code for what gets sent back to the model." The canvas code decides how it exposes and interprets its own info for the poll tool — the page declares what poll returns (its digest, its reports). This replaces today's fixed `status` schema (canvas.rs:419, the hard-coded report partitioning) with a canvas-resolved contract.
- `send` = feed dynamic input into a live canvas. **`send` only works with a live, open canvas by definition** — sending to a closed canvas makes no sense; you'd edit the canvas source instead. And even for an open canvas, editing the source still works. So `send` is for the dynamic, hot path: state writes, live data, a click routing — things built into the canvas workflow. **`state` is dissolved**: the canvas code defines how it interprets sends (an artist conceptual component handles them), just as it defines how it exposes poll info. State write → `send`; state read → rides the poll digest.

### The spawn/search tool, skill-pattern
`canvas { query }` → always a search first. The **one-shot rule carries over from skill**, unchanged:
- `query` is parsed and fuzzy-ranked with the same `fff_search` primitives over the registry.
- **Exact referent**: if `query` is exactly a canvas name or a template name, resolve it — template → clone behavior (scaffold a new canvas from it), canvas → open it. The model never opens a raw template; a template has nothing useful to a live page, so a template referent always clones.
- **Cold exact-name = search, with the mandatory note** (`canvas [name] will open/clone on next call; the harness defaulted to search so you'd see what you're opening`), exactly as in skill. The one-shot rule is the same per-session `seen` tracking.
- **No referent** → fuzzy search over the registry, with fff.

### Registry and querying
- The registry holds canvases and templates, discovered by the same layering as skills: global (config dir) under file-local (`.artist/canvas/`), profile-scoped.
- **How to query canvases that exist but are not open, and templates, and globals-vs-locals — same as skill search.** `canvas { query }` with no referent searches the whole registry (existing-but-closed canvases, templates, all scopes). Open canvases are additionally listed by the universal `list`; the registry search sees everything regardless of open/closed. Global vs local follows the skill scoping rule (later scope shadows earlier by name).

### What's deleted from the model surface
- `status` → `poll` (harness signals universal; app feedback canvas-authored).
- `state` → `send` (write) + poll digest (read); interpretation is canvas-authored.
- `open`/`close` → `poll`-addressable spawn/abort. `list` → universal `list`.
- `docs` → **removed**; see the note below (the docs-as-default-skill seed is out of scope of this redesign).
- `eject` → deleted. A standalone Vite project is an implementation escape hatch (canvas.rs:835); the model should never need to know Vite exists.
- `share`, `join`, `export` → deleted. They were never meant to be model-facing tools. They are **canvas design conceptual components**: a canvas is written to *contain* its own share/join/export affordances, accessed by the user through the canvas, not called by the model (canvas.rs:676, 708, 775).

### Session id namespace
- **`canvas:slug` IS the session id.** `canvas { query: "test-dashboard" }` spawns and returns `canvas:test-dashboard`; the model addresses it in the universal tools as `poll { session: "canvas:test-dashboard" }`. The registry namespace doubles as the session id — one consistent addressing scheme, matching the `skill:`/`canvas:` prefix pattern. `canvas:` ids can't collide with artist names (`Goethe`).
- The `canvas:` tool is not the only id source. The full id space (see Identity above): `subagent` returns a bare artist name, `bash` returns `bash:slug`, `ask` returns `ask:slug`, `canvas` returns `canvas:slug`. `poll`/`abort`/`send`/`list` take the id they were given back, whatever its shape — `list` shows all namespaces, prefixed (`bash:`, `ask:`, `canvas:`, bare artist names).

### Docs removal note (seed for later)
- Docs live in `crates/artist-canvas/src/docs.rs` (352 lines, `render(topic)` at docs.rs:230). It currently documents **26 entries** (CanvasLink, useCanvasStateOf, artist.static, useCanvasState, useAgent, useAsk, useTool, AppShell, Pending, Metric, Alert, Markdown, FileLink, DataTable, Plot, Sparkline, Code, Diff, SchemaForm, Approve, Transcript, ToolLog, artist.send, artist.call, artist.highlight).
- The kit the tool description advertises is ~45 symbols; **19 are advertised but undocumented**: Toolbar, Stack, Split, Card, EmptyState, Button, Input, Textarea, Select, Checkbox, Badge, Tabs, Dialog, Toaster, ErrorBoundary, AskDock, useAgentEvents, useTheme, artist.state. (The "everything worth reaching for is documented" test checks only 5 names — it overclaims.)
- **Seed**: when the docs-as-default-skill is built, it should cover the 19 undocumented symbols too, and the `docs` mode's removal is what forces the model to learn the kit another way (via the always-on catalog + the skill). Left out of scope here by decision; revisit when the first shipped skill is built.

## MCP gate flags removed (one control, both surfaces)

The daemon's `Allow` struct is a set of "one flag per subsystem" gates (daemon.rs:32-38): `computer` (:91), `canvas` (:140), `memory` (:95), `subagent` (:99), `comms` (:181). These are **duplicate controls**: the profile's `permits` already decides, per profile, exactly which tools are visible on the MCP surface (profiles.rs:134-140, applied in `build`, tool_set.rs:471-481) — and the universal web profile is the daemon-level profile (above), so "what MCP advertises" is already profile-governed. A daemon flag that independently amputates `computer`/`canvas`/`comms` is a second, parallel gate that the profile cannot override. Delete the redundancy.

Decision: **`Allow` shrinks to `{ subagent, memory }`. Computer, canvas, and comms become profile `permits` — the exact same allow/deny lists that govern normal sessions.**

- `computer` (daemon.rs:91, gates the SurfaceRegistry) → profile `allow`/`deny` on the computer tools. The registry only *constructs* when the profile lets it through; the daemon flag is deleted.
- `canvas` (daemon.rs:140, gates the CanvasHost) → profile `allow`/`deny` on the canvas tools, same rule.
- `comms` (daemon.rs:181, `allow.comms && identity.registered`, gates messaging) → profile `allow`/`deny` on `send`/`gc`. The `identity.registered` half stays — messaging always requires a registered identity, independent of any flag.
- **Why `subagent` and `memory` stay:** they are not tool visibility — they are resource bring-up. `memory` (:95) builds the memory index (a heavy, optional subsystem; standing it up is a cost decision, not a visibility one). `subagent` (:99) gates the delegation machinery (seats, `PermitSlot`, concurrency) — spawning subagents is capacity, and the subagent tool is the only one that is *not* exempt from the profile's `allow` (HARNESS_TOOLS exempts handoff/todo/ask only, profiles.rs:211). Those two keep daemon-level flags; everything that is purely a tool surface moves to config.
- The rule, stated once: **config control (profile `permits`) is the single gate for tool surface on both normal and MCP sessions. Daemon flags exist only where a subsystem's presence is a resource decision (`subagent`, `memory`), not where it is a visibility decision.**

## Pending rework: `computer` (session reframe + dynamic discovery)

Today the computer tool is a 13-mode multiplexer (tool.rs) behind a single binary gate: `construct(env) -> Option<Tool>` (tool_set.rs:246-291) — once `env.computer` exists, the *entire* surface (all modes, full schemas) ships all-or-nothing via the provider-native `tools` field (tool_prompt.rs:5-8). There is no staged disclosure. The surface is far too broad to dump into context up front, so the spawn becomes the discovery boundary.

### Session reframe (agreed)
- `launch` / `attach` → **spawn**. Returns a session id; the computer surface reframes around it.
- `close` → **abort**. The session dies; `do` on the id afterward is a normal "session ended" error, same as any stale session id.
- `surfaces` → **list** (the parallel-surface registry becomes the spawned-session set).
- `observe` / `screenshot` / `find` / `extract` / `zoom` → **read-only projections of a surface** — parameterized with real arguments (fields, queries). They are *not* subsumed by a bare `poll`: `poll` carries no query arguments, these carry field/query arguments.
- `focus` / `resize` / `watch` → **control verbs** specific to the computer kind.
- `do` stays an **atomic composite**: `{ steps: [...], settle, expect }`, fully specified in the call arguments, guardrail before step one, no cross-call state. It does not violate the "no tool carries a state machine" invariant — the *session* carries the state machine, `do` addresses it per call.

### Dynamic discovery on spawn (the core addition)
Two-stage surface. The advertised schema is a function of whether a computer session exists:

- **Stage 0 — always present (cheap spine).** Only the discovery verbs: `surfaces` (list), `launch`/`attach` (spawn). Tiny schema, near-zero context cost. This is the whole tool until a session exists.
- **Stage 1 — advertised once a session spawns.** The heavy surface appears: `do` (the full program schema: click/type/key/hover/drag/scroll/touch/clipboard/upload/dialog/navigate/invoke), the observe-family read-only projections, `focus`/`resize`/`watch`, `close`. The model discovers it by re-listing tools after spawn.

Implementation notes:
- On the MCP transport, `tools/list` is re-invocable; the advertised set reflects registry state per request (no session → stage 0 only; ≥1 session → stage 1 added). This is the primary path for dynamic discovery.
- On the provider-native path (tool_prompt.rs:5-8), the tool set is published once per run; the computer tool's advertised schema must be computed from the live registry at publish time, and the description must tell the model the heavy surface "appears once a surface is spawned" so it re-checks. Flagged as the one place the provider-native channel constrains us — the spec's contract is the two stages, not the exact re-advertisement mechanics.
- This is a discovery optimization, not a permission: a `do` call on a stale id is a normal session-ended error even if stage 1 is no longer advertised. The advertised set guides the model; the tools take ids and behave uniformly.
- Spawned ids live in the unified session id namespace as `computer:slug`, addressing every other spawned kind's shape (bare name, `bash:`, `ask:`, `canvas:`). Whether the underlying surface registry remains a parallel `DashMap` (SurfaceRegistry, model.rs:94) or folds into the session registry is an implementation choice; the model-facing contract is `computer:slug` ids in the unified namespace.
