# URI Resource Fabric: Unix as the Harness

## Summary

Replace named coding tools as the central abstraction with one logical,
text-only resource tree addressed by URI. WASM plugins contribute routes and
resources; the model receives a small universal verb set; shells and REPLs call
the same tool registry programmatically.

Resource queries represent virtual descendants:

```text
src/lib.rs                         → file:///workspace/src/lib.rs
src/lib.rs?symbols                 → virtual symbols root
src/lib.rs?symbols/foo/callers     → callers of symbol `foo`
```

The logical tree is canonical. An ephemeral FUSE mount exposes it to ordinary
programs, and one host-owned FFF engine crawls that complete mount to drive
`find` and `grep`.

## Public contracts

- Add `ResourceUri`, backed by `url::Url`:
  - Schemeless paths resolve to absolute `file://` URIs from the session working directory.
  - The URL path identifies the base resource.
  - The complete query string is an Artist projection path split on `/`; it is not `key=value` metadata.
  - Fragments are rejected.
  - Literal path characters are percent-encoded, keeping the projection delimiter unambiguous.
  - Provide explicit `base()`, `projection_segments()`, and `descend_projection()` operations rather than relying on `Url::join`.
- Replace the WIT tool center with two interoperable contracts:
  - Tool providers export definitions and invocation.
  - Resource providers export declarative routes and generic resource handling.
  - Preserve prompt, context, hook, model, and event lifecycle sockets.
  - Host imports expose `list-tools` and `call-tool`; every WASM plugin may enumerate and call the complete registry.
  - Track nested tool calls and reject recursive invocation cycles before re-entering a plugin.
- A resource route declares a base URI glob, optional projection glob,
  supported operations, plugin identity, and registration order. Selection
  prefers the greatest number of literal segments, then the fewest wildcards,
  then plugin load order.
- Core resource operations are:
  - `read(uri, start_line?, line_count?)`
  - `children(uri)` as an internal topology operation
  - `write(uri, text)`
  - `move(from, to?)`; a null destination means remove
  - `poll(uri, match?, timeout_ms?)`
  - `edit(uri, instructions)` is reserved in the protocol, skeletal, and unadvertised.
- Cross-provider move is supported only when one selected provider explicitly handles both URIs.
- Model-facing universal tools are `read`, `find`, `grep`, `write`, `move`, and `poll`.
  - `find(uri, glob?, max_depth?, cursor?, limit?)` subsumes directory listing; null glob with depth one lists immediate descendants.
  - `grep(uri, regex, include_glob?, context?, cursor?, limit?)` searches text bodies.
  - Default pagination is 50 find results and 100 grep matches.
  - Results preserve canonical resource URIs and continuation cursors.
- Poll begins at the current bottom and accumulates only subsequently appended
  text. With a regex it returns on match, close, or timeout; without one it
  returns only on close or timeout. The structured reply contains `text` and a
  `matched`, `closed`, or `timed-out` outcome. Snapshot-only resources return a
  typed unsupported-operation error.

The component ABI is `artist:plugin@0.3.0`; the original proposal's `0.2.0`
step is superseded by the requested 0.3 package version.

## Implementation sequence

1. **Logical resource core**
   - Add URI parsing, projection traversal, requests/replies, typed errors, route declarations, and deterministic routing.
   - Permit a logical node to have both readable text and virtual children.
   - Keep resource state out of the canonical conversation transcript; only resulting model tool calls and results remain durable.
2. **Unified tool dispatcher**
   - Use one registry for Rig, WASM plugins, shells, and REPLs.
   - Give each invocation a call stack and correlation ID.
   - Convert definitions into Rig dynamic streaming tools.
   - Keep cross-plugin callbacks in-process; there is no native plugin ABI.
3. **WASM resource routing**
   - Use `artist:plugin@0.3.0` for the WIT package and components.
   - Register operation-scoped URI globs and handle plain-WIT resource requests.
   - Synthesize query-root children from projection routes.
   - Provide a fixture that reads Rust through the shared tool bridge and exposes `?symbols/...` descendants.
4. **FUSE projection**
   - Use `fuser` 0.17 as host infrastructure.
   - Mount one ephemeral scheme-rooted tree; launch shells/REPLs in the `file` subtree and expose `ARTIST_ROOT`.
   - Project query descendants as distinct FUSE names such as `rust.rs?symbols/`.
   - Delegate lookup, enumeration, reads, writes, moves, and removals to the router.
   - Use only fixed process-owned kernel attributes and clean up through an owned background-session guard.
5. **Unified FFF search**
   - Pin `fff-search` 0.10.5 with its default `ripgrep` feature.
   - Own one native index over the complete FUSE mount after route registration.
   - Crawl every registered enumerable route without allowlists.
   - Translate FFF paths back to canonical URIs.
   - Use FFF path search for `find` and regex grep for `grep`, keeping typed pagination internally and concise model rendering.
   - Trigger refresh after write, move, remove, plugin registration, or topology change.
6. **Default filesystem component**
   - Replace `read_file`, `write_file`, and `list_directory` with universal tools.
   - Register terminal `file://**` routes and delegate native mechanics through focused host imports.
   - Keep the router, URI model, tool bridge, FUSE projection, and search engine domain-neutral.

## Verification plan

- URI tests: relative shorthand, percent encoding, canonical round trips, query descent, fragment rejection.
- Router tests: base/projection globs, specificity, load-order ties, unsupported operations, provider-defined moves.
- Tool registry tests: model invocation, WASM-to-host callbacks, cross-plugin calls, correlation IDs, and direct/indirect recursion rejection.
- Resource tests: ordinary and projected reads coexist on one base node.
- Poll tests: append, regex match, close, timeout, and typed unsupported snapshot behavior.
- FUSE tests: ordinary access, query traversal, routed write/move/remove, and URI/path round trips.
- FFF tests: one index finds and greps ordinary files and WASM-provided projections.
- Kernel replay tests: resource activity appears only as existing tool-call/tool-result transcript entries.
- Component smoke: build default and fixture components against WIT 0.3 and exercise every socket.
- Linux FUSE runtime tests are gated on `/dev/fuse`; URI, routing, plugin, and registry tests are mount-independent.

## Assumptions and deferred decisions

- WIT 0.1 has no external compatibility obligation; 0.3 replaces it cleanly.
- Plugins remain WASM components. FUSE and FFF are native host mechanisms.
- Bodies are UTF-8 text; vision and binary resources need a future model-aware path.
- MCP, `skill://`, action-node invocation, and the edit instruction language are deferred.
- Frontend/UI is deferred. Permissions, sandboxing, security policy, and isolation are anti-features and remain out of scope.
