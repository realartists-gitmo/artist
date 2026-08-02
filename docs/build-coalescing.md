# Tree jobs and build coalescing

## Status

Specified, not implemented. This document covers concurrent expensive commands
in a shared worktree: what several agents running `cargo test` at once should
do, why the answer is not worktree isolation, and how the same treatment
generalizes past cargo.

Attribution (see Core invariants 5) depends on agent identity infrastructure
Artist does not have. That dependency is called out in Open questions and is the
only part of this design that is blocked; coalescing itself is not.

The decisive architectural choice is:

> A build is a function of the worktree, not of who asked for it. There is one
> job per coalescing key per project, every agent waiting on that key waits on
> that job, and a fresh blocking request for the same key supersedes the running
> one rather than queueing behind it.

## Core invariants

The implementation is invalid if any of these invariants is violated:

1. Only a blocking request may supersede a running job. A backgrounded request
   joins a running job or queues behind it, and never cancels one.
2. A running job is superseded only by a request with the same *coalescing key*.
   A request that merely shares an exclusion domain queues; it never preempts.
3. A requester whose job was cancelled without being superseded stays waiting
   and is re-served. It is never handed an error, and never handed the results
   of a command it did not ask for.
4. Regrounding is triggered by job requests and by nothing else. There is no
   file watcher, no edit hook, and no mtime polling.
5. A result shared by more than one waiter names whose changes it contains.
6. Results are never shared across different coalescing keys, including where
   one command's scope contains the other's.
7. Exclusion domain and coalescing key are separate. Sharing a domain forces
   serialization; only sharing a key permits a shared result. A command with no
   declared descriptor is treated as its own domain and its own key — unknown
   commands run as they do today rather than being guessed at.

## The problem

Several agents work in one worktree. Agent one runs `cargo test`; rustc starts.
Agent two makes a small change and runs `cargo test`. Today this goes badly in
two ways at once, and the second is not obvious.

**We already pay for serialization.** Cargo takes an exclusive lock on the build
directory, so the second build does not run in parallel — it blocks. Both
flavours are routinely observed in practice:

```
Blocking waiting for file lock on package cache
cargo: giving up waiting on build lock /tmp/.cargo-mux/…
```

So the choice is not "parallel versus serial". It is already serial. The only
question is whether the serialization is smart.

**And the first build is already invalid.** Cargo fingerprints and reads source
progressively across a build. When the tree mutates underneath it, the result is
a *torn read*: some crates compiled from pre-edit source, some from post-edit,
corresponding to a tree state that never existed on disk. Worse than a wrong
answer, fingerprints are written against artifacts that do not match them, which
degrades the next incremental build too.

Together: the status quo pays the full serialization cost *and* returns an
incoherent answer for it. Letting agent one's build finish was never actually an
option — it only looked like one.

## Why not worktree isolation

The obvious alternative — one worktree per agent — is rejected:

- **Disk.** For a Rust project the cost is not the source tree, it is `target/`.
  A worktree of Artist is a few hundred megabytes of source beside tens of
  gigabytes of build cache. Five agents is not five times the repo, it is five
  times the build cache.
- **Isolation converts merge conflicts into semantic conflicts.** Two agents
  each adding a field to the same struct in separate worktrees merge cleanly and
  produce wrong code. A textual conflict is visible, tooled, and resolvable; a
  clean merge of two incompatible designs is a bug found at runtime days later.
  This is a downgrade sold as safety.
- **Resource contention gets worse, not better.** Isolated worktrees have
  separate build locks, so nothing serializes them and every agent's cargo
  claims every core.

**The trap to avoid** if worktrees are ever revisited: pointing divergent
worktrees at a shared `CARGO_TARGET_DIR`. It appears to solve the disk problem,
but each build invalidates the other's artifacts, trading disk for permanent
full rebuilds — and cargo's build lock serializes them anyway. `sccache` is the
tool that actually helps there, because it is keyed by input hash rather than by
tree.

Artist instead shares one worktree and reports drift, which is why every tool a
subagent receives is wrapped by `tool_prompt::guard` in `tool_set::build`.

## Tree jobs

Nothing above is specific to cargo. The abstraction is a **tree job**: an
expensive, mutually-exclusive command whose result is a function of the worktree
rather than of the caller, and which keeps incremental state a concurrent edit
can poison. Cargo is the sharpest instance, not the only one.

A descriptor declares four things:

| Field | Meaning |
|---|---|
| **Exclusion domain** | What must not run concurrently — usually a directory that gets locked or mutated (`target/`, `node_modules/`, a daemon's output base). |
| **Coalescing key** | What may share a result. Strictly finer than the domain. |
| **Scope** | Which subtree the result is a function of, for invalidation and attribution. |
| **Shareable** | Whether the result may be handed to a waiter who did not launch it at all. |

The distinction between the first two is the one that matters and is easy to
miss. `cargo build` and `cargo test` share an exclusion domain — one `target/`
lock — so they must serialize. They do **not** share a coalescing key: a build
result is not a test result. Two `cargo test -p artist-agent` invocations share
both. Collapsing these into one concept either over-shares results or
under-parallelizes, and the failure of the first kind is silent.

Candidates worth declaring, all exhibiting the same lock-plus-incremental-state
shape:

- **cargo** — `build`, `test`, `check`, `clippy`, `fmt --check`, `doc` all
  contend on one `target/` lock. Note `rust-analyzer` is a fifth contender with
  its own target directory, and checks on save.
- **Gradle and Maven** — a daemon plus explicit lock files; the heaviest
  offenders on a laptop by some margin.
- **Bazel / Buck** — two commands in one workspace already block on the output
  base lock, which is exactly this design implemented upstream.
- **Package installs** — `npm`/`pnpm`/`yarn install`, `uv`/`pip`/`poetry`,
  `bundle install`. These mutate a shared tree (`node_modules/`, a virtualenv)
  rather than a cache, so concurrency is not merely wasteful but corrupting.
- **TypeScript** — `tsc --build` keeps `.tsbuildinfo`, with the same poisoning
  failure as cargo fingerprints. Bundlers (`vite build`, `webpack`) share it.
- **Go** — `go build`/`go test`. `GOCACHE` is content-addressed so poisoning is
  not a concern, but the CPU contention and result-sharing arguments both hold.
- **Docker / BuildKit** — its own cache lock.
- **Test runners generally** — `pytest`, `jest`, `go test`: the pure
  "same tree, same command, shareable result" case with no lock at all, where
  coalescing is a straight saving.

Unknown commands are not guessed at. Absent a descriptor, a command is its own
domain and its own key, which reproduces today's behaviour exactly.

## The design

One single-flight cell per `(project root, coalescing key)`, carrying a
generation counter and the set of waiters on the current generation.

**Request.** An agent asks for command `C`, resolving to key `K`. If a job on
`K` is running and the request blocks, the requester joins its waiter set and
the job is cancelled and restarted from the current tree — the generation bumps
and every existing waiter is carried onto the new generation. If the request was
backgrounded (invariant 1), the requester joins the running job as it stands and
does not restart it. If a job on a different key in the same exclusion domain is
running, the request queues.

**Supersession, not invalidation.** The trigger is the arrival of a request, not
the mutation of a file. An edit alone does nothing. This is what makes a file
watcher unnecessary, and it is self-debouncing: a burst of edits emits no events
at all, and only the build request that follows them does anything. It also
matches what agents actually do, which is edit and then test.

The alternative — cancelling on edit — has no good completion. Either the
requester gets nothing and must ask again, or the harness rebuilds speculatively
on every write, at which point it is `cargo watch`.

**Resolution.** When a build completes, every waiter on its generation receives
the same result. Because the worktree is shared, that result already reflects
every agent's changes, including the changes of the agent whose earlier build
was superseded.

**Attribution.** The failure mode of a shared result is not staleness, it is
misattribution: an agent receives a compile error in a file it never touched,
does not know that, and "fixes" another agent's in-progress work. Every shared
result therefore names the agents whose edits it contains, and which paths each
touched. This is drift reporting one level up — the same idea as
`tool_prompt::guard`, at build granularity rather than file granularity.

## Why there is no anti-starvation floor

Preemptive invalidation usually needs a floor to guarantee that some job
eventually finishes. This design does not, because **every agent that can
preempt is an agent that is currently blocked.**

A blocking request parks the agent that issued it, so an agent waiting on a job
cannot issue another one. With `N` agents the worst case is each in turn issuing
a request and parking, until every agent is a waiter and none remains that could
preempt. The job then runs to completion. Bounded by `N`, terminating, no floor
required.

Latency is also no worse than today. Under queueing, `N` staggered requests cost
exactly `N` job times. Under coalescing the worst case is the same and the
common case is far better, because later requesters find a job already in flight
that covers their changes.

Builds are run in the foreground by default, and an agent that deviates
generally has a reason to — so the correct posture is correct-by-default with an
escape available, not a prohibition. Invariant 1 is what makes the escape free:
a backgrounded request is a requester that is not a waiter, which is exactly the
shape that would void the bound, so backgrounding is permitted but does not
carry the right to preempt. A background request joins the running job if its
coalescing key matches, and otherwise queues. Nothing is forbidden; the bound
survives.

The cost is borne where it belongs: an agent that backgrounds a build, edits,
and asks again gets its second result later than a blocking agent would, because
its request waits rather than cancelling the job other agents are parked on.

## Not in v1

- **Subsumption.** A workspace-wide `cargo test` already covers a concurrent
  `cargo test -p artist-agent`, and could absorb its requester instead of making
  it queue. This needs a command-subsumption rule, and invariant 6 forbids it
  until that rule exists.
- **A shared jobserver.** Cargo speaks the GNU make jobserver protocol, so a
  harness-owned token pool passed to every spawned build would let concurrent
  builds share one CPU budget rather than each claiming every core. This is the
  correct fix for cross-*project* contention, which coalescing does not address
  — coalescing is per project root. Current cargo behaviour needs verifying
  before relying on it.
- **Descriptors beyond cargo.** The abstraction is general (see Tree jobs) and
  cargo is the only descriptor worth writing first, because it is the one whose
  contention we actually observe. The rest are additions to a table, not
  redesigns.

## Where the code would live

| Concern | Module |
|---|---|
| Single-flight cell, generation, waiter set | new, alongside `artist-tools/src/bash.rs` |
| Tree-job descriptor table, key and domain resolution | same |
| Attribution annotation on a shared result | `artist-agent/src/tool_prompt.rs` (with `guard`) |
| Per-project global keyed by root | pattern exists in `artist-agent/src/profiles.rs` (`project_semaphore`) |
| Generation-and-cancel primitive | pattern exists in `artist-agent/src/tool_registry.rs` |

`ToolRegistryHandle` is worth reading first: `publish` bumps a generation and
cancels the previous generation's in-flight calls with *"the turn moved on
before this canvas call finished"*. A build coalescer is the same primitive with
a different message — the tree moved on before this build finished.

## Open questions

- **Agent identity and inter-agent communication.** Invariant 5 requires stable
  per-agent identity and a record of which agent touched which path. Artist has
  actor ids (`artist_tools::short_id`) and a per-actor workspace, but no
  general identity or messaging layer. This gates attribution only —
  coalescing, supersession, and the no-starvation property need none of it, so
  the two can ship in either order. Unspecified; to be designed separately.
- **Cancellation safety.** Whether cargo tolerates being killed mid-build
  without leaving the target directory in a state that degrades the next build.
  Cargo is believed to write artifacts atomically, but this is the assumption
  the whole design rests on and it must be verified, not assumed.
- **What counts as the same coalescing key.** Argument-order normalization,
  environment differences, and `--features` all affect whether two invocations
  may share a result. Invariant 6 is currently enforced by exact match, which is
  safe and will over-queue.
- **Cancellation safety per descriptor.** The question below is asked of cargo,
  but each tree job answers it differently, and the package installers answer it
  worst: a killed `npm install` can leave `node_modules/` partially written in a
  way no subsequent command detects. A descriptor may need to declare itself
  non-preemptible, in which case a matching request queues rather than
  superseding.

## Related docs

- `docs/profiles.md` — subagent dispatch, concurrency limit, tool policy
- `docs/architecture.md` — the agent loop and workspace layout
