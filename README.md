# Artist

Artist is a small, durable agent runtime built around two objects:

- A **Node** is an immutable, versioned recipe for an agent.
- A **Run** is one append-only execution of a Node.

Runs can spawn other nodes, fork their exact model-context prefix, wait for typed results, or replace themselves. Python memory is temporary; events and shared workspace files are durable.

## What currently works

- SQLite event storage and complete run reconstruction
- JSON Schema validation for node inputs and yielded results
- Dynamic `spawn`, `fork`, `join`, and `replace` operations
- Exact cached-context prefix inheritance for forks
- Durable Ask/Answer and typed Yield state transitions
- One separate, persistent Python process per active REPL session
- Python-to-Rust graph calls through the `artist` object
- PyO3 native bridge, with a source-tree Python fallback
- `fff-search` bindings preinstalled and exposed as `fff`
- Rig completion-model adapter with a fixed Python/Ask/Yield tool surface
- Wasmtime async WASI 0.3 component validation and capability checks
- Crash recovery for unfinished model/Python actions
- A CLI for inspecting and exercising every core operation

The frontend and a production model-provider service are intentionally not included yet.

## Set up

Requirements: current Rust, Python 3.10+, and [`uv`](https://docs.astral.sh/uv/).

```bash
cargo build
cargo run -- init
```

`init` creates the workspace `.artist/` directory and `.artist/artist.db`. It also creates a global `.artist/` inside the platform config directory (for example `~/.config/.artist/` on Linux), seeds its `SYSTEM.md`, and runs `uv sync` for the managed Python environment.

Configuration uses the same layout at both levels. A file in the workspace `.artist/` replaces the matching global file; a global file replaces the built-in default. The built-in system prompt is the real `prompts/SYSTEM.md` in this repository and is compiled into the binary. Edit the global `.artist/SYSTEM.md`, or create a workspace `.artist/SYSTEM.md`, to replace it without rebuilding Artist. `init` never overwrites existing user files.

## Minimal example

Create node instructions:

```bash
printf 'Solve the given task and yield {"answer": "..."}.\n' > NODE.md
```

Create the node:

```bash
cargo run -- node create worker NODE.md \
  --input-schema '{"type":"object","required":["task"]}' \
  --output-schema '{"type":"object","required":["answer"]}'
```

Use the printed node ID:

```bash
cargo run -- run start NODE_ID '{"task":"test the runtime"}'
cargo run -- run show RUN_ID
cargo run -- python RUN_ID 'fff is not None; 20 + 22'
cargo run -- run yield RUN_ID '{"answer":"done"}'
```

JSON arguments can be loaded from files by prefixing the path with `@`.

## Core source files

- `src/domain.rs` — Node, Run, and event types
- `src/store.rs` — transactional SQLite event store
- `src/runtime.rs` — graph operations and state rules
- `src/context.rs` — append-only prompt construction and fork prefixes
- `src/scheduler.rs` — joins and crash recovery
- `src/python.rs` / `python/worker.py` — isolated persistent REPL
- `crates/artist-python` — native PyO3 RPC bridge
- `src/rig_driver.rs` — stable Rig provider boundary
- `src/plugin.rs` — Wasmtime WASI 0.3 plugin host

See `harnessplan.md` for the architectural contract.
