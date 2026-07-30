# Architecture

Artist is temporarily an offline maintenance binary while its model runtime is
being rebuilt.

## Current workspace

- `artist-cli`: exposes `rules` and `sessions` maintenance commands.
- `artist-session`: stores and renders existing event-log sessions.
- `artist-rules`: parses and evaluates stream-rule definitions.
- `artist-tools`: reusable local filesystem and process tools.
- `artist-extensions`: reusable WASM extension infrastructure.
- `hashline-tools`: hashline editing primitives.

## Removed runtime

The former `artist-agent` and `llm-provider` crates were removed together with:

- Rig-native completion-client dispatch;
- provider registration and selection;
- API-key, bearer-token, and OAuth login flows;
- model catalog discovery and selection;
- prompt submission, streaming chat, delegation, and model-backed compaction;
- interactive provider/model commands and the chat TUI.

The current `artist` binary has no path that performs model authentication or a
model API request. Existing provider configuration files are intentionally not
read, modified, migrated, or deleted.

Some reusable crates still use Rig data and tool traits. Those types are not a
provider runtime and are not reachable as model-call functionality from the
binary. They can be migrated behind project-owned interfaces as the replacement
runtime is implemented.
