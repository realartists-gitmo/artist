# Frankenstein guide: grafting `hashline-tools` into your Rust agent harness

This folder is a **standalone crate** excised from RealArtist. It contains everything you need for:

1. **Read / write / edit** tools that expose deterministic **semantic v1 anchors** per line
2. **Language-neutral occurrence identities** through Artist's existing `artist-ast` / ast-grep stack, with exact-line fallback
3. A **multi-agent coordinator** with per-agent read/drift views
4. **SQLite coordination metadata** for agent registration and writer attribution — never anchor allocation
5. **Cross-process path locks** (`fs2` + lock files) so two harness processes do not race on the same path
6. **Whole-file BLAKE3** content hashes for conditional write/delete, independent of anchor identity

Shell tools, MCP, diagnostics, queues, screenshots, etc. were **not** included.

---

## Layout

```text
hashline-tools/
├── Cargo.toml
├── FRANKENSTEIN.md          ← you are here
├── docs/
│   └── semantic-anchors-v1.md ← identity/address ABI and textual grammar
├── examples/
│   └── basic.rs             ← end-to-end multi-agent smoke demo
└── src/
    ├── lib.rs               ← public re-exports
    ├── agent.rs             ← AgentId / AgentIdentity
    ├── error.rs             ← HashlineError (+ codes)
    ├── semantic_anchors.rs ← TECA-backed, stateless anchor rendering
    ├── scheme_v1.json       ← frozen generated v1 scheme metadata
    ├── semantic_anchors.rs  ← direct row mapping + shortest live rendered prefixes
    ├── anchor_table.rs      ← deterministic live binding addresses for non-file surfaces
    ├── file_tools.rs        ← FileToolManager (exact resolution + unit tests)
    ├── state.rs             ← SQLite StateStore (coordination metadata only)
    └── coordinator.rs       ← FileCoordinator + WriteCondition + content_hash
```

---

## Quick verify before grafting

```bash
cd hashline-tools
cargo test
cargo run --example basic
```

All unit tests (identity, addressing, manager, state, coordinator) should pass.

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

- A C toolchain for `rusqlite` (bundled SQLite) and the tree-sitter grammars already pulled by `artist-ast` / ast-grep.
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
| `file_tools.rs`, `semantic_anchors.rs`, and the `artist-ast`/`teca` dependencies | `coordinator.rs`, `state.rs`, `agent.rs`, `error.rs` |

Without coordinator/state you lose cross-process path locks, writer attribution, and the high-level conditional-write API — but single-process `FileToolManager` still computes the same v1 anchors because anchor identity/addressing has no persisted state.

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
    max_lines: None, // None = whole file; addressing is computed against the whole live file
}).await?;
// out.result.content  → "#token: fn main() {\n..."
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

// EDIT (batch of line ops; anchors are exact opaque strings beginning with '#')
let out = coord.edit_file(&actor, EditRequest {
    path: "src/main.rs".into(),
    operations: vec![
        EditOperation::Replace {
            anchor: "#token".into(),
            end_anchor: None,              // or Some(end_anchor) for inclusive range
            content: "fn main() { todo!() }".into(),
        },
        EditOperation::InsertAfter {
            anchor: "#other".into(),
            content: "// note\n".into(),
        },
        EditOperation::Delete {
            anchor: "#third".into(),
            end_anchor: None,
        },
    ],
}).await?;

// DELETE (requires matching whole-file BLAKE3)
let maybe_hash = coord.delete_file(&actor, path, expected_blake3).await?;

// PREVIEW (lock + exact resolve, no write)
let preview = coord.preview_edit_file(&actor, request).await?;
```

### Low level (single agent, no SQLite)

```rust
use hashline_tools::{FileToolManager, FileToolConfig, ReadFileRequest, WriteFileRequest, EditRequest};

let mut mgr = FileToolManager::with_config(FileToolConfig::default());
let view = mgr.read_file(ReadFileRequest { path, start_line: 1, max_lines: None }).await?;
// No anchor state is imported/exported: the same live text recomputes the same TECA addresses.
```

---

## Tool-schema sketch for your LLM harness

Expose four tools (names are suggestions):

| Tool | Args | Returns |
|------|------|---------|
| `read_file` | `path`, optional `start_line`, `max_lines` | `content` (`ANCHOR: line` text), `lines[]`, `content_hash`, `total_lines`, `anchor_usage` |
| `write_file` | `path`, `content`, `condition` (`absent` \| `any` \| `{content_hash}`) | same as read view + hash |
| `edit_file` | `path`, `operations[]` | before/after anchored views, new `content_hash` |
| `delete_file` | `path`, `expected_hash` | ok / hash mismatch |

**Critical instruction for the model** (also available as `hashline_tools::ANCHOR_USAGE`):

> Use the exact opaque anchor beginning with `#` exactly as returned before `: `. Do not trim, case-fold, Unicode-normalize, fuzzy-match, or include the following line text.

Edit ops:

```json
{ "op": "replace", "anchor": "#token", "end_anchor": null, "content": "..." }
{ "op": "delete", "anchor": "#token", "end_anchor": "#other" }
{ "op": "insert_before", "anchor": "#token", "content": "..." }
{ "op": "insert_after", "anchor": "#token", "content": "..." }
```

JSON `anchor` maps directly to Rust `EditOperation::* { anchor, ... }`.

---

## Stale anchors

Resolution is exact against the current live occurrence set. There is no stale relocation, confirmation retry, fuzzy match, or hidden guard. If an address no longer resolves — including when a newly live collision requires a longer prefix — re-read/re-map and use the exact current address.

Address collisions never mutate occurrence identity. They only extend the rendered v1 prefix.

---

## Multi-agent rules (do not skip)

1. **Agent IDs are coordination identity, not anchor identity.** They affect writer attribution/session metadata only.
2. **Anchors are shared deterministically.** The same occurrence identity in the same live set gets the same address for every agent/session/process.
3. **Path locks are global.** Write/edit/delete take an exclusive lock on the normalized path (in-process mutex + `flock` file under `lock_directory`).
4. **SQLite is coordination-only.** It stores agent registration and writer attribution. Opening the v1 store drops obsolete pre-v1 `anchor_states`; no anchor allocator state is loaded or saved.

---

## Wiring checklist

- [ ] Persist `hashline.db` and `path-locks/` under your harness data dir if you want coordination attribution/locks across calls.
- [ ] Map your harness session/user key → `AgentIdentity` for coordination metadata.
- [ ] Set `workspace_root` and `allow_outside_workspace: false` in production.
- [ ] Surface `ANCHOR_USAGE` in every file-tool result the model sees.
- [ ] Pass anchors byte-for-byte; never trim, case-fold, normalize, or fuzzy-match them.
- [ ] Prefer `edit_file` for surgical changes; use `write_file` + `ContentHash` for full rewrites.
- [x] Address logical-line identities through the published `teca` crate.

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

- Structured occurrence identity is binary TLV under `artist.anchor.identity.v1\0`: language, node kind, field/role, governing named-node ancestry with available semantic keys, canonical leaf content, then equivalent-occurrence rank.
- Unknown/unparseable text uses exact logical-line bytes plus equivalent-exact-line rank.
- Path/filename, line number, byte offset, neighbors, duplicate count, read history, and address collisions are never serialized into identity.
- Canonical identity bytes go directly into the published TECA address stream; there is no cryptographic pre-hash.
- Artist selects a shortest unique prefix over TECA structural atoms and uses TECA's boundary-preserving renderer.
- Visible grammar is `#TOKEN` or `#TOKEN‖TOKEN...`; the shortest rendered prefix unique among live occurrences is shown.
- Whole-file BLAKE3 via `content_hash(&[u8])` remains only a conditional write/delete guard and is not an anchor input.

---

## Provenance

Excised from the RealArtist monorepo:

- `crates/core/src/file_tools.rs`
- the former mnemonic allocator/vocabulary were replaced by the frozen v1 address/token artifacts
- `crates/tools/src/files.rs` → `coordinator.rs`
- `crates/tools/src/state.rs` (coordination metadata only)
- Agent types / write conditions / error codes slimmed from `crates/tools`

The current crate preserves the file/coordinator surface while replacing legacy hashline/mnemonic identity and allocator state with the versioned v1 semantic-address ABI.

---

## Support shape if something breaks

1. Re-run `cargo test -p hashline-tools` and the dependent `artist-ast`, `artist-tools`, and `artist-computer` suites.
2. Verify the generated v1 address/token/scheme files are byte-identical to their frozen artifacts.
3. Confirm `artist-ast::anchors` recognizes the language or intentionally falls back to exact line content.
4. Confirm the model is passing the exact `#...` address before `: `, without normalization.
5. On concurrent write failures, verify all writers use the same `lock_directory`; whole-file BLAKE3 guards are separate from anchor resolution.
