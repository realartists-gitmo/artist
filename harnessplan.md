Do not inspect git history or other branches. They are irrelevant.

# Artist harness plan

## Goal

Build a small, general agent runtime. It should support arbitrary workflows without putting workflow-specific ideas in the core.

Frontend is explicitly out of scope for now.

## Philosophical commitments

- Build on Rig.
- Use `serde` for data encoding and `schemars` for typed JSON schemas.
- Use CodeAct-style Python as the main model-facing interface.
- Each active agent gets its own Python REPL process.
- Make the `fff` Rust binary's Python bindings available in every REPL for fuzzy natural-language search.
- Harness behavior should be inspectable, extensible, and modifiable through plugins.
- Use Wasmtime and the current async WASI 0.3 APIs, not 0.2.
- Use PyO3 for the Python-side harness bridge, while keeping Python in a separate process from the Rust runtime.
- Prefer a small set of powerful primitives over many special-purpose tools.
- Never invalidate an existing model-cache prefix.

## Core abstraction

### Node

A **Node** is an immutable, versioned recipe for one kind of agent run. A node subsumes the idea of an agent "profile."

A node declares:

- its node-specific instructions (`NODE.md` or an equivalent stored value),
- its typed input,
- its typed output/yield contract,
- the capabilities and plugins available to it,
- its model settings and other execution policy.

A node does not hard-code which node runs next. Agents and plugins decide that dynamically.

### Run

A **Run** is one durable execution of a Node.

A run owns:

- an immutable reference to its Node version,
- its input,
- its append-only model conversation,
- its event history,
- one temporary Python REPL process while active,
- links to its parent, children, and forks,
- its current state,
- and, once complete, one typed result.

The Node is the recipe; the Run is one execution of that recipe.

### Run graph

Runs form a dynamic graph. The graph is not required to be declared in advance.

The basic scheduling operations are:

- **spawn**: start another node and return a handle immediately,
- **join**: wait for one or more handles and receive their typed results,
- **fork**: spawn new runs from the current node and exact model-context prefix,
- **replace**: end the current run in favor of one or more successor runs.

The default fork helper is **fork and join**: pause the original run, run all forks, collect their typed results, then resume the original. Continuing alongside forks and replacing the original must also be expressible from the same lower-level operations.

Delegation is simply spawning a child with a possibly different Node. Forking is spawning children from the current Node and context prefix.

## Minimal runtime parts

Keep the Rust architecture divided by responsibility, not by speculative framework layers:

- **Store** — atomically appends events and rebuilds current Run state from them.
- **Scheduler** — starts ready Runs, pauses waiting Runs, and wakes parents when children finish.
- **Agent loop** — uses Rig to call the model and append model/tool events.
- **Context builder** — constructs the fixed initial prefix and append-only continuation without rewriting history.
- **Python supervisor** — starts, monitors, and honestly restarts one Python process per active Run.
- **Plugin host** — loads versioned Wasm components, checks capabilities, and exposes approved calls through Python.

The event history is the source of truth. In-memory objects are disposable views rebuilt from that history after restart.

## Model-facing interface

Keep the provider-visible tool surface stable and very small:

1. **Python REPL** — normal work, plugin calls, spawning, forking, and joining.
2. **Ask** — durably pause for human input, then append the answer.
3. **Yield** — submit the node's final typed result.

Plugin functionality appears inside a stable Python harness API rather than changing the provider's tool definitions. This protects the model-cache prefix.

### Yield contract

`yield(value)`:

1. validates `value` against the Node's output schema,
2. returns a useful error to the agent if validation fails,
3. atomically records a valid result and completes the Run,
4. gives the parent only that typed result.

A child's full activity remains inspectable in storage but is not automatically inserted into the parent's context.

## Context and cache rules

Initial context is ordered as:

1. `SYSTEM.md` — universal instructions, including a short note encouraging agents to use `fff` when fuzzy natural-language search would help,
2. `AGENTS.md` — workspace instructions,
3. `NODE.md` — instructions for this Node/profile.

Artist uses the Rust `dirs` crate to locate the platform's global config directory, then uses an `.artist/` directory inside it. Workspaces also use a local `.artist/`. A local file replaces the matching global file, and a global file replaces the built-in default. The default system prompt lives at `prompts/SYSTEM.md` in the source repository and is compiled into the binary with `include_str!`; user files replace it at runtime without changing an existing Run.

The `fff` note should explain when it is useful, not require it for every search.

These initial hunks never change for an existing Run. Any hot change is appended as a new context event or diff. It must never rewrite earlier messages or tool definitions.

A fork shares the exact context prefix up to the fork point. Fork-specific input is appended after that shared prefix. This allows parallel branches to reuse the same model cache.

Node and plugin definitions are immutable and versioned. Editing one creates a new version. Existing Runs retain their old references and remain resumable.

## Python REPL boundary

Each active Run gets a separate normal Python process. Python is not hosted inside the main Rust process and is not expected to run inside WebAssembly.

The PyO3 module is a thin bridge from that Python process to the Rust harness. Plugin calls and run-graph operations cross this controlled boundary.

Every REPL environment also includes the Python bindings for the `fff` Rust binary. This is a standard built-in search capability rather than a changing provider-visible tool, so its presence does not alter the tool schema or invalidate the model-cache prefix.

Python memory is temporary:

- a fork starts with a fresh Python process,
- after a crash or restart, Python starts fresh,
- the harness preserves recorded history and durable resources,
- the agent is explicitly told when Python memory was lost,
- old Python commands are not automatically replayed because replay could repeat side effects.

Live Python state is never claimed to be copied or durably resumed.

## Workspace and files

Related runs use the same shared workspace files. Forks do not receive private file snapshots.

Concurrent writes may race or overwrite one another. The core does not add automatic merging or hidden locking. Workflows that need coordination can use separate paths or a plugin/resource with explicit locking.

Files are durable resources, not part of Python memory and not guaranteed to be fully represented by REPL history.

## Durability

A Run is append-only and resumable at completed action boundaries. Record meaningful state changes before exposing them as complete.

Examples include:

- run creation and state changes,
- completed model responses,
- Python requests and completed outputs,
- context diffs,
- child creation and completion,
- questions and answers,
- valid yields,
- interruption or cancellation.

A crash during an unfinished model request or Python command cannot resume at the exact CPU instruction. The action is marked interrupted and then retried or surfaced according to policy. Completed actions and valid yields are never silently lost.

## Plugin architecture

Everything beyond the minimal runtime should be a versioned plugin where practical. Plugins may provide:

- Python-callable capabilities,
- Node definitions,
- workflow/orchestration logic,
- durable resource types,
- lifecycle hooks,
- provider integrations.

Wasm plugins run under Wasmtime with explicit capabilities. The core owns persistence, scheduling, context invariants, schema validation, and process isolation; plugins should not be able to bypass those guarantees.

Self-inspection and self-modification happen by inspecting plugin/Node definitions and creating new versions. They do not mutate the historical definition of an active Run.

## Explicit non-goals for the first core

- Frontend or visual graph editor.
- A large collection of provider-visible tools.
- Automatic file merging between parallel agents.
- Perfect checkpointing of live Python memory.
- A static workflow language required for all workflows.
- Injecting complete child transcripts into parent contexts.
- Built-in free-form multi-agent chat. It can later be a plugin if needed.

## Concrete first implementation

The first core uses:

- SQLite in WAL/FULL-sync mode, with JSON event bodies and one transaction per completed graph change,
- a JSON-lines protocol over each Python child's stdin/stdout,
- a thin PyO3 extension for Python-to-Rust RPC, plus a pure-Python development fallback,
- `fff-search` installed in the managed Python environment and imported as `fff`,
- Wasmtime's async WASI 0.3 (`p3`) component host,
- one stable Wasm plugin export, `call(operation, payload-json)`, behind explicit capability grants,
- global and workspace `.artist/` config directories located with the `dirs` crate,
- `SYSTEM.md`, `AGENTS.md`, and per-Node instruction values as the concrete context names, with local-over-global replacement,
- interrupted Python/model actions surfaced honestly and never automatically replayed,
- a small scheduler that repairs interrupted runs and wakes completed joins.

Remaining policy choices are cancellation propagation, timeouts, budgets, model-provider deployment, and retry rules for specific capabilities known to be safe to retry. These do not change the core abstraction.
