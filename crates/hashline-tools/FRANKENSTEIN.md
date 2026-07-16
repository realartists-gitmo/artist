# Frankenstein guide: grafting `hashline-tools` into your Rust agent harness

This folder is a **standalone crate** excised from RealArtist. It contains everything you need for:

1. **Read / write / edit** tools that expose stable **one-token mnemonic anchors** per line  
2. **Hidden line hashes** (xxh3 → Crockford base32) for stale detection  
3. A **multi-agent coordinator** with per-agent in-memory managers  
4. **SQLite persistence** of issued anchors so restarts do not forget tokens  
5. **Cross-process path locks** (`fs2` + lock files) so two harness processes do not race on the same path  
6. **Whole-file BLAKE3** content hashes for conditional write/delete  

Shell tools, MCP, diagnostics, queues, screenshots, etc. were **not** included.

---

## Layout

```text
hashline-tools/
├── Cargo.toml
├── FRANKENSTEIN.md          ← you are here
├── docs/
│   └── mnemonic-anchors.md  ← semantics of the token system
├── examples/
│   └── basic.rs             ← end-to-end multi-agent smoke demo
└── src/
    ├── lib.rs               ← public re-exports
    ├── agent.rs             ← AgentId / AgentIdentity
    ├── error.rs             ← HashlineError (+ codes)
    ├── mnemonic_anchors.rs  ← token allocator / reconcilation
    ├── mnemonic_words.txt   ← ~3k one-token vocabulary (do not drop)
    ├── file_tools.rs        ← FileToolManager (core algorithm + unit tests)
    ├── state.rs             ← SQLite StateStore (agents + anchor_states only)
    └── coordinator.rs       ← FileCoordinator + WriteCondition + content_hash
```

---

## Quick verify before grafting

```bash
cd hashline-tools
cargo test
cargo run --example basic
```

All unit tests (manager, anchors, state) should pass.

---

## Dependency graft (Cargo)

### Option A — path dependency (recommended while iterating)

Copy this folder into your repo (e.g. `third_party/hashline-tools` or `crates/hashline-tools`) and add:

```toml
# your-harness/Cargo.toml
[dependencies]
hashline-tools = { path = "third_party/hashline-tools" }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "fs", "sync"] }
anyhow = "1"
```

System requirements:

- A C toolchain for `rusqlite` (bundled SQLite) and `tree-sitter` / `tree-sitter-rust` (used only to stabilize line IDs on `.rs` files).
- Linux/macOS advisory file locks via `fs2` (Windows may need different locking if you care).

### Option B — vendored workspace member

```toml
# workspace Cargo.toml
[workspace]
members = ["crates/your-harness", "crates/hashline-tools"]
```

If this crate is nested under another workspace tree without being a member, keep the empty `[workspace]` table already present in this package’s `Cargo.toml` (or add it to the parent’s `exclude`).

### Option C — copy modules only

If you refuse a separate crate, the minimal file set is:

| Must copy | Optional |
|-----------|----------|
| `file_tools.rs`, `mnemonic_anchors.rs`, `mnemonic_words.txt` | `coordinator.rs`, `state.rs`, `agent.rs`, `error.rs` |

Without coordinator/state you lose multi-agent isolation, restart durability, path locks, and BLAKE3 write conditions — but single-process `FileToolManager` still works.

---

## API map (what to call from the harness)

### High level (most harnesses)

```rust
use hashline_tools::{
    AgentIdentity, EditOperation, EditRequest, FileCoordinator, FileToolConfig,
    ReadFileRequest, WriteCondition, ANCHOR_USAGE, content_hash,
};

// Once at process start:
let coord = FileCoordinator::open(
    FileToolConfig {
        workspace_root: Some(workspace_dir.clone()),
        allow_outside_workspace: false,
        follow_symlinks: false,
    },
    data_dir.join("hashline.db"),   // SQLite
    data_dir.join("path-locks"),    // lock file directory
)?;

// Per tool call: map your session user / agent key into AgentIdentity
let actor = AgentIdentity::from_id(your_agent_key)?;

// READ
let out = coord.read_file(&actor, ReadFileRequest {
    path: "src/main.rs".into(),
    start_line: 1,
    max_lines: None, // None = whole file; partial reads keep tombstones
}).await?;
// out.result.content  → "time | fn main() {\n..."
// out.result.lines    → structured { line_number, anchor, text }
// out.content_hash    → whole-file BLAKE3 hex
// show ANCHOR_USAGE to the model

// WRITE (create / replace / hash-gated)
let out = coord.write_file(
    &actor,
    "src/main.rs".into(),
    full_source.into(),
    WriteCondition::ContentHash { hash: expected_blake3 },
    // or WriteCondition::Absent  (create only)
    // or WriteCondition::Any
).await?;

// EDIT (batch of line ops; anchors are the bare tokens)
let out = coord.edit_file(&actor, EditRequest {
    path: "src/main.rs".into(),
    operations: vec![
        EditOperation::Replace {
            hash: "time".into(),           // field name is historical: pass mnemonic
            end_hash: None,                // or Some(end_anchor) for inclusive range
            content: "fn main() { todo!() }".into(),
        },
        EditOperation::InsertAfter {
            hash: "people".into(),
            content: "// note\n".into(),
        },
        EditOperation::Delete {
            hash: "know".into(),
            end_hash: None,
        },
    ],
}).await?;

// DELETE (requires matching whole-file BLAKE3)
let maybe_hash = coord.delete_file(&actor, path, expected_blake3).await?;

// PREVIEW (lock + resolve, no write, no persist)
let preview = coord.preview_edit_file(&actor, request).await?;
```

### Low level (single agent, no SQLite)

```rust
use hashline_tools::{FileToolManager, FileToolConfig, ReadFileRequest, WriteFileRequest, EditRequest};

let mut mgr = FileToolManager::with_config(FileToolConfig::default());
let view = mgr.read_file(ReadFileRequest { path, start_line: 1, max_lines: None }).await?;
// on restart: mgr.import_issued_prefixes(saved); ... export_issued_prefixes()
```

`EditOperation` fields are still named `hash` / `end_hash` for historical reasons; **values are mnemonic tokens**, not the hidden line hashes. Models never see the hidden hashes.

---

## Tool-schema sketch for your LLM harness

Expose four tools (names are suggestions):

| Tool | Args | Returns |
|------|------|---------|
| `read_file` | `path`, optional `start_line`, `max_lines` | `content` (`anchor \| line` text), `lines[]`, `content_hash`, `total_lines`, `anchor_usage` |
| `write_file` | `path`, `content`, `condition` (`absent` \| `any` \| `{content_hash}`) | same as read view + hash |
| `edit_file` | `path`, `operations[]` | before/after anchored views, new `content_hash` |
| `delete_file` | `path`, `expected_hash` | ok / hash mismatch |

**Critical instruction for the model** (also available as `hashline_tools::ANCHOR_USAGE`):

> Use only the bare mnemonic token before ` | `. For the rendered line `time | beta`, pass anchor `"time"` (not `"time | beta"`).

Edit ops:

```json
{ "op": "replace", "anchor": "time", "end_anchor": null, "content": "..." }
{ "op": "delete", "anchor": "time", "end_anchor": "people" }
{ "op": "insert_before", "anchor": "time", "content": "..." }
{ "op": "insert_after", "anchor": "time", "content": "..." }
```

Map JSON `anchor` → Rust `EditOperation::* { hash: anchor, ... }`.

---

## Stale anchors and confirmation

If another agent (or a human) changed a line after anchors were issued, the hidden guard may uniquely identify a *different* current line. The manager returns a typed error:

```rust
use hashline_tools::ConfirmationRequired;

match coord.edit_file(&actor, request).await {
    Ok(ok) => { /* ... */ }
    Err(err) => {
        if let Some(conf) = err.downcast_ref::<ConfirmationRequired>() {
            // Tell the model: resubmit THE EXACT SAME edit batch to confirm,
            // or re-read the file. conf.candidate_anchor is the current token.
            // conf.context is a small anchored window around the candidate.
        } else {
            // other I/O / validation failures
        }
    }
}
```

Exact retry of the same operation fingerprint applies the edit against the candidate; any change invalidates the pending confirmation.

---

## Multi-agent rules (do not skip)

1. **Stable agent IDs.** `AgentIdentity::from_id("session-42")` — use the same string across process restarts for that agent, or their anchors reset.
2. **One coordinator per process is fine.** Managers are keyed by agent id inside the coordinator.
3. **Anchors are not shared across agents.** Agent A’s `time` is not agent B’s `time` even for the same file. Each agent must read (or write) before editing.
4. **Path locks are global.** Write/edit/delete take an exclusive lock on the normalized path (in-process mutex + `flock` file under `lock_directory`).
5. **SQLite is the durability layer.** After every successful read/write/edit the full issued-prefix map is rewritten for that agent. Partial reads keep tombstones for lines outside the window; full-file reads reclaim dead handles.

---

## Wiring checklist

- [ ] Persist `hashline.db` and `path-locks/` under your harness data dir (not the user’s repo, unless you want that).
- [ ] Map your harness’s session/user/agent key → `AgentIdentity`.
- [ ] Set `workspace_root` and `allow_outside_workspace: false` in production.
- [ ] Surface `ANCHOR_USAGE` in every file-tool result the model sees.
- [ ] On edit errors, special-case `ConfirmationRequired` before generic failure.
- [ ] Prefer `edit_file` for surgical changes; use `write_file` + `ContentHash` for full rewrites.
- [ ] Never ask the model for the hidden line hash or whole-file hash as an *edit target* — only as the write/delete condition.
- [ ] Include `mnemonic_words.txt` in the same directory as `mnemonic_anchors.rs` (`include_str!`).

---

## What was deliberately left out

| Left in RealArtist | Why |
|--------------------|-----|
| MCP server / tool JSON schemas | Host-specific; re-schema in your harness |
| Shell / tmux tooling | Unrelated |
| Diagnostics / rust-analyzer | Unrelated |
| Processing queue | Unrelated |
| AST structural rewrites | Separate tool (`ast-bro`) |
| Schemars / serde on public types | Add if you want auto JSON Schema |
| Full `ToolError` shell codes | Slimmed to `HashlineError` |

---

## File identity notes (advanced)

- Line IDs: xxh3 of line bytes → 13-char Crockford base32; duplicates disambiguated with neighbor hashes and (for `.rs`) tree-sitter named-node ancestry.
- Visible anchors: mnemonic words from `mnemonic_words.txt`, packed with a hidden guard prefix (`full_hash + U+001F + short_prefix`).
- Whole-file hash: BLAKE3 hex via `content_hash(&[u8])`.
- Internal cursor metadata keys: `__hashline_internal_primary_cursor__` / `__hashline_internal_secondary_cursor__` (stored in the same map as anchors; ignore them in UI).

---

## Provenance

Excised from the RealArtist monorepo:

- `crates/core/src/file_tools.rs`
- `crates/core/src/mnemonic_anchors.rs`
- `crates/core/src/mnemonic_words.txt`
- `crates/tools/src/files.rs` → `coordinator.rs`
- `crates/tools/src/state.rs` (anchor + agents tables only)
- Agent types / write conditions / error codes slimmed from `crates/tools`

Internal branding was renamed from `realartist` → `hashline` where it was only cosmetic (temp files, cursor keys). Behavior matches the source tests (30 unit tests included).

---

## Support shape if something breaks

1. Re-run `cargo test` inside this crate.  
2. Check that `mnemonic_words.txt` is present and not re-encoded.  
3. Confirm agent ids are stable across restarts.  
4. Confirm the model is passing bare tokens, not `token | line text`.  
5. On flaky concurrent writes, ensure all writers go through the same `lock_directory`.
