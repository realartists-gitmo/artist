# artist-ast

AST navigation engine: structural shape, public API surface, import and call
graphs, structural pattern search and rewrite.

Forked from [ast-bro](https://github.com/aeroxy/ast-bro) (MIT) at upstream
revision `d9bccef`. Original licence retained as `LICENSE-ast-bro`.

## What changed from upstream

The fork keeps the analysis engine essentially whole. Cutting engine capability
because it should not be an independent agent tool would be the wrong
abstraction, so the only deletions are the agent plumbing artist already owns:

| Removed | Why |
| --- | --- |
| `installers/` | artist owns its own tool installation |
| `mcp/` | artist does not expose internal subsystems over MCP |
| `hook/` | artist owns read interception |
| `prompt` command | artist owns prompting |
| the clap CLI in `lib.rs` | this is a library; see `cli` feature below |

Everything else — `core`, all 13 language adapters, `surface`, `deps`, `calls`,
`impact`, `context`, `graph_cache`, `search`, `run`, `squeeze` — is retained.

Two further differences:

- **`lib.rs` is a library.** Upstream's `[lib]` target exposed only
  `pub struct LineRange` and `pub fn run()`; every module was private. Modules
  are now public and `parse_file` / `walk_paths` / `walk_and_parse` are exported.
- **No renderer is part of the contract.** Upstream renders results against
  line numbers. Artist addresses lines by deterministic semantic occurrence anchor
  (`hashline-tools`), so callers take `ParseResult` / `Declaration` and render
  them themselves. The upstream renderers remain available but artist does not
  use them.

## Installation

This crate is a workspace member; nothing to install separately.

```toml
[dependencies]
artist-ast = { path = "../artist-ast", default-features = false }
```

`default-features = false` drops the `cli` feature and with it the `clap`
dependency, which is what consumers want — see below.

## Features

All default-on. The gates exist so a consumer that wants only structural shape
does not compile the graph, retrieval and rewrite engines.

| Feature | Gives you | Pulls in |
| --- | --- | --- |
| *(base)* | `core`, all 13 adapters, `file_filter`, `path_glob`, `project_root`, `main_helpers`, `search::{cache,chunker}` | — |
| `surface` | public API surface, re-export chain resolution | — |
| `graphs` | `deps` + `calls` + `graph_cache` (mutually recursive upstream, so one unit) | `rayon`, `bincode`, `fs2` |
| `impact` | cross-file blast radius | `graphs` |
| `context` | token-budgeted symbol context | `graphs` |
| `search` | hybrid BM25 + dense retrieval | `graphs`, `reqwest`, `tokenizers`, `safetensors`, `memmap2`, `wide`, `sha2`, `dirs` |
| `run` | structural pattern search and rewrite | `similar` |
| `squeeze` | reversible log/text compression | — |
| `cli` | the test-fixture binary; implies `full` | `clap` |

Measured: **78 transitive dependencies at base, 257 at `full`.** `search` is
what carries the weight — an HTTP stack, a tokenizer, and a safetensors loader.

Two submodules of `search` are always compiled: `cache`, because `graph_cache`
reuses its file-delta helpers, and `chunker`, because `cache` needs
`is_indexable`. Both are generic file handling rather than retrieval, and
`chunker` has no `crate::` references at all.

`serde_yaml`, `jsonc-parser` and `toml_edit` are dropped from upstream's
manifest — they served `installers/` only.

### The `cli` feature

The 152 vendored integration tests in `tests/` are end-to-end: they exec
`CARGO_BIN_EXE_ast-bro`. That is 3.7k lines of coverage over the language
adapters, which are the highest-risk code in the fork and the part most likely
to break as grammars move. Rather than discard it, the CLI is retained as a
test fixture behind `cli`, with a three-line `src/bin/ast-bro.rs` driving
`cli::run()`. On by default so `cargo test` works unqualified; artist depends on
this crate with `default-features = false`.

### Why there are no per-language features

Grammar compilation is the real binary-size lever (roughly 0.5–5 MB each), so
per-language gating was the obvious next step. It is not reachable from here.

`ast-grep-language` compiles happily with `default-features = false` and a
single grammar — verified, it pulls 3 `tree-sitter-*` crates instead of 27. But
`SupportLang::from_path` and the `LanguageExt` trait only exist under its
`builtin-parser` feature, and those are exactly what the adapter dispatch in
`main_helpers.rs` runs on. Selecting a grammar subset therefore removes the API
the adapters need; `builtin-parser` is all-or-nothing.

Closing that gap means replacing language detection with a hand-rolled registry
over the `tree-sitter-*` crates directly. That is precisely what oh-my-pi's
`pi-ast` does, in ~1,000 lines across `src/language/`, and is likely why it
hand-rolls the registry rather than depending on `ast-grep-language`. Not worth
it here: artist ships to other people's codebases and wants every adapter on
anyway.

## Known upstream issues

- `deps` resolves a Go module prefix by reading `go.mod` at the *project root*,
  where the root is the first ancestor holding `.git`. A `go.mod` nearer the
  file is discarded, so Go imports go unresolved in any repository whose root
  is not itself the Go module. `tests/deps_e2e.rs::go_module_prefix_strips_correctly`
  is `#[ignore]`d for this and carries the full analysis.
