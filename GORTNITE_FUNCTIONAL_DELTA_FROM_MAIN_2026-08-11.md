# Gortnite functional delta from `main`

This is an idea-level inventory of the committed `main...Gortnite` delta and
the separate dirty worktree layer as inspected on 2026-08-11.  Each runtime
bullet states the changed boundary, state owner, input/output behavior, or
removed mechanism—not merely that a feature area was “added.” It is not a
claim that an item is correct, complete, tested, desired, or suitable for
cherry-picking. It deliberately distinguishes runtime behavior from tests,
documentation, vendoring, and migration mechanics.

The compared base is `main` / `origin/main` at `7e731616`; local `Gortnite`
is `f47f28a`.  The committed range comprises the 96 commits also present on
`origin/Gortnite` plus seven local P0 commits.  Dirty worktree changes are not
committed by `Gortnite` and are listed separately below.

## Committed functional ideas: provider, prompt, profile, and conversation

- Fixes transferred CodeRabbit findings from an earlier Gortnite lineage into this lineage (`51a575a`, `a3aa8a0`).
- Aligns rendered tool continuation lines under the TUI tool icon (`c6ca6ad`).
- Renders edit/write diffs with mnemonic anchor gutters rather than line numbers (`358f657`).
- Replaces the closed `RigClient` enum dispatch with generic provider dispatch (`04fd136`).
- Corrects expiry checks to use the intended absent-or-valid predicate (`ee3978a`).
- Makes session reopening tolerate writer-lock handoff (`15a59f5`).
- Gives each handoff hop its own provider-private conversation lineage (`4989e1a`).
- Runs the test suite with the release profile (`671b89e`).
- Keeps required harness tools from being removed by profile allow-list intersection (`16d582a`).
- Composes profile bodies on top of the shared prompt rather than replacing it (`7b95aac`).
- Stops profile/config discovery from creating scaffold files as a side effect (`2bf421b`).
- Moves tool definitions into the tool API contract rather than reconstructing them elsewhere (`834ef31`).
- Makes input atom-range bookkeeping total for all input shapes (`a44a1f3`).
- Answers orphaned provider tool calls before sending a subsequent request (`e5aee67`).
- Makes text/atom pairing invariants structural rather than best-effort (`f436a0c`).
- Expands OpenAI's injected parallel-tool wrapper before interpreting calls (`b439e83`).
- Contains tool panics and hallucinated tool names so they become tool failures rather than aborting the agent run (`6ea9232`).
- Adds CLI discovery/display of Artist profiles (`d6c75a4`).
- Encapsulates the durable-prefix context clamp (`25a5a31`).
- Flushes the event writer before reading session logs in the compaction test path (`ee2a65a`).
- Drains queued prompts while the interactive input box is idle (`cf585df`).
- Introduces profile-based agent construction, fallback/provider selection, handoff, thinking controls, and todo handling while removing the old subagent module (`d8727c7`).
- Adds profile and ChatGPT-web-mesh architecture documentation (`d8727c7`).
- Adds property tests for input atom/pairing invariants (`1e718e2`).

## Committed functional ideas: canvas and shared canvas runtime

- Serves hot-reloading React canvas applications without requiring a Node development server (`c496c49`).
- Adds a canvas bridge, component kit, templates, and Fast Refresh integration (`58f53a5`).
- Completes the page-to-agent side of the canvas bridge (`c91de31`).
- Opens canvases in windows owned by Artist (`ac6c3f0`).
- Adds canvases to Artist's palette/navigation surface (`141bffd`).
- Fixes rendered-canvas defects discovered by exercising an actual canvas (`5730b0d`).
- Preserves native form chrome, tab focus rings, and suppresses swap flash (`49ef455`).
- Stops shared canvas-state writes from unnecessarily reloading pages (`d084b7e`).
- Prevents canvas pages from quitting the session or running shells (`94ef5c5`).
- Adds named color families and color-flipping utilities to the canvas kit (`fe0c555`).
- Makes bridge actions report what they did (`048315e`).
- Removes the model's raw outbound-request primitive from the canvas surface (`f5d921d`).
- Lets the model inspect what it built in a canvas (`bf6a77d`).
- Finalizes/pins the canvas visual kit and template system (`4dac419`).
- Repairs mismatches between promises printed by the canvas subsystem and its actual behavior (`359a4e4`).
- Stops hooks from polling every token and permits test directories (`0af7149`).
- Stops temporary files produced by writers from triggering canvas reload (`6d174db`).
- Adds template rendering, bridge-host, and bridge-watch test harnesses (`3e585ef`, `7a824d7`, `2bd0182`).
- Uses CDN dependencies for canvas pages and makes missing module failures visible (`73b5cf6`).
- Persists a window's last position across every exit (`6819d32`).
- Reports which canvas build is displayed and its age since check (`b2f6c40`).
- Tests the window-spawn path and report-window halves (`0d34979`, `9623a30`).
- Lets a canvas be exported and opened away from the machine that created it (`bc5fce5`).
- Adds multi-party shared canvas operation without a central Artist server (`12435b7`).
- Makes shared-state convergence explicit (`a13f5a3`).
- Produces a self-contained canvas export without a mode flag (`7aead67`).
- Keeps shared canvases current and reports values lost during merge (`6633b32`).
- Merges concurrent canvas edits instead of selecting one side, with an inspectable result (`202d397`).
- Documents composed canvas class-name behavior (`c0a1815`).

## Committed functional ideas: file anchors, tools, artifacts, and terminal

- Adds a drift watch around tool execution: after a successful tool call, changed observed files are appended to the model-visible result rather than silently invalidating the model's file view (`e32b8f2`).
- Keys hashline anchor state by conversation/actor, so two concurrent or resumed conversations do not overwrite each other's anchor mapping (`835e68e`).
- Writes the responsible actor/process into hashline coordinator records, allowing later attribution of a modification and expiration of unowned anchor state (`64998c8`).
- Deletes/retires obsolete anchor-state machinery rather than maintaining two authority paths (`407fa62`).
- Replaces a checked-in 68,399-token anchor vocabulary/table with compact TECA lexicon atoms and opaque-prefix resolution (`672ae91`).
- Adds a dictionary identity path based on an exact text substring encoded as a TECA identity, rather than only a generic hash/name (`6e8d9c9`).
- Changes artifact IDs so their TECA source material incorporates both the occurrence envelope and payload bytes; identical bytes from different occurrences can therefore remain distinct artifacts (`69e64f5`).
- Adds page-store records for binary/media payloads containing occurrence identity, content type/media type, payload revision, and raw-programmatic access separate from text presentation (`d8d9ec9`).
- Intercepts base64 image result blocks from every guarded tool, stores decoded bytes as artifacts, and emits an artifact pointer rather than retaining the raw image bytes in model result text (`8786af4`).
- Removes the older build-coalescing feature/docs and rewrites parts of tool-prompt result handling (`65c23ae`).
- Adds managed durable bash sessions, a separate one-shot bash wrapper, bounded tool-result pagination, and a semantic anchor implementation used by code/computer/MCP paths (`5a0b915`).
- Adds persisted HTTP/MCP identity records intended to map transport identity to Artist identity, instead of generating a new identity at every daemon operation (`5a0b915`).
- Updates the rules crate to the current Wasmtime exception feature spelling (`6f6ff8b`).

## Committed functional ideas: logic and memory

- Adds source-naming answers and queryable axes to the logic subsystem (`3f8535b`).
- Adds a semantic kernel in which rules derive conclusions rather than merely accepting them (`5ec09db`).
- Removes a defeasible rule when kernel semantics must win (`1ca2956`).
- Removes the constructive tier and gives contradiction a defined landing place (`d4c5106`).
- Evaluates implication according to the stated semantic kernel (`7c1d2c1`).
- Exposes `Conflicted` to logic property tests (`3d2791b`).
- Documents/reconciles strict-kernel and defeasibility alternatives (`52a3104`, `57888bb5`, `7b6f49b`, `60f4fd9`, `9554564`).
- Retracts an unsound `Instance`/`Instantiate` claim, then implements a per-bit repair (`4fb71fa`, `03618f0`).
- Repairs debt regressions and costs the repair (`5e71db5`).
- Makes the soundness generator self-referential and adds generated quantifier coverage (`da52ea8`, `24c2d99`).
- Property-tests the evaluator against its kernel and repairs discovered defects (`7df1dab`).
- Repairs four memory provenance defects and adds the first direct coverage (`408f012`).
- Ensures a withdrawn claim no longer vouches for downstream state (`e09c506`).
- Adds denier-path memory coverage (`e1ad7f3`).
- Prevents an origin from impersonating a session (`3a90bb3`).

## Committed functional ideas: MCP, session host, registry, computer, and UI

- Gives each MCP tool a declared output JSON schema in its advertised contract (`85799e4`).
- Adds a daemon harness that can serve MCP over its configured transport and a script/path for tunnel deployment (`33fc351`, `16bf499`).
- Adds MCP result paging/recovery records so a bounded result can be continued or a keyed operation can be recovered without re-running arbitrary work (`885b020`).
- Adds an ask-session protocol: prompt questions/options are persisted and sent through an outbox/UI path, instead of blocking the tool call for a human response (`f5c3981`).
- Deletes the earlier ask-outbox, message-tool, delegate-job, registry-job, and tool coalescing implementations and moves their responsibilities into the newer session/registry/UI paths (`47ce542`).
- Rewrites the ask UI, canvas server/registry, session tools, MCP server/pagination, and bash implementation to that newer path (`47ce542`).
- Adds durable session registry rows and process records that hold lifecycle, snapshot, cancellation, ownership/name, and last-seen state (`656383b`).
- Adds agent lifecycle verbs—including poll, stop, delete, and send—against those registry rows (`656383b`).
- Adds MCP discovery endpoints and tests that expose daemon/server capabilities (`656383b`).
- Adds computer-use environment detection and a human-driven computer control layer, plus a native canvas window implementation (`9805980`).
- Vendors WGPU core/HAL to support that graphics/computer stack (`9805980`).
- Reworks Wayland stage export, CDP surface control, GPUI host/window ownership, session-host daemon behavior, and the CLI's former canvas-window path (`9cd45ec`).
- Adds Artist roster-name retention and HTTP-identity persistence as lifecycle foundations (`5a0b915`).

## Committed functional ideas: major imported subsystems and dependencies

- Adds `artist-ast` as a multi-language source-analysis crate: Rust, TypeScript, Python, Go, Java, Kotlin, C#, C++, PHP, Ruby, Scala, SQL, and Markdown adapters contribute symbols/calls/imports/dependencies/surfaces/search data.
- Adds AST operations for symbol lookup, definitions, implementations, call graph/call trace, dependency graph/cycles, impact, code search, and related-structure queries, plus CLI/MCP renderers and fixtures.
- Adds `artist-computer` as a surface abstraction over AT-SPI, Chrome DevTools Protocol, PTY, programmatic, and screen surfaces; it includes observations, actions, accessibility anchors, macros, and Wayland staging.
- Adds `artist-canvas` as the state/server/bridge/shared-peer/export implementation, and `artist-gpui`/`artist-ui-core` as native UI integration layers.
- Adds `artist-session-host` as a daemon boundary for session/computer host control and `artist-mcp-server` as the MCP host/daemon boundary.
- Adds `artist-memory`, `artist-registry`, `artist-config`, and `artist-tool-api` as separately compiled persistence/config/tool-contract crates.
- Vendors Zed GPUI, WEF, WGPU core, and WGPU HAL into source control instead of obtaining them only through upstream crates at build time.
- Adds packaging, launch, verification, and operational scripts for these new runtime surfaces.

## Committed functional ideas: local-only P0/Muse layer after `origin/Gortnite`

- Introduces `muse_v6` as a nested Rust workspace rather than a dependency of the Artist workspace: it contains its own lockfile, packages, ontology/provenance/occurrence/tooling crates, audit corpora, and static-verification scripts (`7d34e29`).
- Adds an Artist event-to-Muse adapter that reads persisted session envelopes and emits Muse-normalized records; it does not make Artist transport types depend on Muse semantic types (`7d34e29`).
- Adds an Artist-side `muse_hook` that flushes captured events, invokes normalization, and stores diagnostics as best-effort post-observation work; it does not block the original tool result (`7d34e29`).
- Adds a session-side Muse ingestion layer for facts, documents, diagnostics, labels, rules, candidates, Bayesian observations/priors, and findings (`7d34e29`).
- Adds `ResourcePath` parsing for real paths versus typed `scheme://` paths, with a shared enum for resource schemes and stable rendering of virtual paths (`7d34e29`).
- Makes `read`, `find`, and `grep` understand typed runtime namespaces instead of treating all non-file references as host paths (`7d34e29`).
- Adds virtual projections for agent/session state, skills, profiles, tools, memory, dictionaries, artifacts, bash output, canvas, computer capability, and extension resources (`7d34e29`).
- Adds a global persistent dictionary store and `dict://` projection; emitted references use a prefix lookup over retained dictionary entries (`7d34e29`).
- Adds a profile hook that detects resolved profile changes and applies profile/context updates between model calls rather than only at session creation (`7d34e29`).
- Adds a `run` action that classifies script/canvas/native executable targets and creates durable bash/canvas/process-style resource records instead of returning only command text (`7d34e29`).
- Makes lifecycle tools accept several typed resource paths and routes stop/delete/poll/send through session registry records (`7d34e29`).
- Revises bash tool output and path identity to expose durable terminal/session state through the typed noun space (`7d34e29`).
- Revises read/edit/write/find/grep to use the typed resource-path resolver and anchor-aware result shapes (`7d34e29`).
- Revises extension-manager activation to retain the last working module when a candidate replacement fails (`7d34e29`).
- Removes the 68,399-token anchor-address table and old v1 anchor-address implementation in favor of compact semantic/TECA anchor handling (`7d34e29`, `672ae91`).
- Adds the master production checklist and the unified-state execution-plan documents as planning artifacts (`7d34e29`).
- Changes the checklist to claim tool-result image routing through artifact semantics is complete (`f47f28a`).

## Dirty worktree functional ideas not committed on `Gortnite`

- Adds a second public tool named `read_many` instead of extending `read`. Its request is `{paths: string[], limit?: integer}`; it rejects virtual paths, directories, and media; it returns `{snapshot: "artist-coordinator", reads: [...]}` rather than normal `read` output.
- Makes `read_many` acquire all real-file coordinator locks in normalized order, read every requested file while those locks are held, and return results in caller order. It rejects duplicate targets.
- Makes `read_many` return each real file's content hash, total line count, line anchors/text, `hasMore`, and a synthesized single-file `read(path, offset, revision)` continuation.
- Registers `read_many` in the tool bundle, default tool set, read-only profile, tool contract, and model-visible tool registry. This means it is actually exposed to models, not just an internal helper.
- Adds `FileCoordinator::write_files_atomic`: it locks every target, validates all write conditions before replacing any file, replaces each file, and attempts rollback if an unexpected replacement error occurs.
- Changes AST rewrite application to use that atomic batch write primitive instead of issuing sequential writes that can leave a partial refactor.
- Adds `RelationshipStore` backed by a project-local `.artist/state/relationships` Mnestic/Cozo RocksDB database; rows contain edge id, source path, edge kind, target path, and creation time.
- Enforces at most one inbound `child` edge per target in that store, while allowing cycles and independent `successor` edges.
- Adds readable one-hop projections such as `agent://id/parent`, `agent://id/children`, `agent://id/predecessor`, and `agent://id/successor`; these return paths rather than recursively traversing the graph.
- Adds a public `relationship` mutation/traversal tool, in addition to the read projections.
- Adds `eval://<artist>` persistent Python kernels keyed by project plus Artist identity. The kernel is a long-lived child process, not a fresh process per call.
- Gives Python code a callback protocol that asks the currently published Artist tool registry to execute a tool, attempting to preserve the active profile/tool-policy boundary rather than exposing direct Rust internals.
- Creates an `eval` registry resource and parent `agent://… --child--> eval://…` relationship when the kernel is first used.
- Adds framed JSON-RPC transport shared by debugger and LSP clients; it launches a child process and exchanges `Content-Length` framed JSON messages.
- Adds `debug` operations for DAP `initialize`, launch/attach, configurationDone, breakpoints, continue/pause/step, stack/scopes/variables/evaluate, disconnect, and process termination.
- Resolves debug adapters from an optional global Artist cache first, then `PATH`, with built-in names for Python/debugpy, LLDB, Go/delve, and user-supplied command paths.
- Adds project-scoped shared LSP clients keyed by project root plus server command/arguments, rather than one LSP server per agent call.
- Makes LSP clients issue `didOpen`/`didChange` with complete current file text and monotonically increasing document version before requests.
- Adds LSP methods for symbols, definition, references, diagnostics, hover, rename, and code actions; location results are post-annotated with current Artist anchors.
- Adds a read-only `forge` tool whose canonical paths begin `forge://github/<owner>/<repo>/…`; it shells out to authenticated `gh` and wraps JSON or diff responses with freshness metadata.
- Caches only SHA-addressed immutable forge objects in process memory; mutable issue/PR-like paths are fetched again instead of served from that cache.
- Adds read projections for `forge://`, `rules://`, `relation://`, `eval://`, and `debug://`, plus `process://` normalization, to the virtual read dispatcher.
- Removes public tool registrations for `list`, `abort`, and `subagent`; retains/uses `read` discovery, `stop`, and `agent` as replacements in the modified registry.
- Adds a shared structured failure envelope at the portable-tool boundary. It includes a code, explanation, optional affected path, and expected/actual revisions when available.
- Changes stale errors from ad hoc strings toward `stale_revision` structured errors, including real and virtual continuation paths.
- Captures completed delegated yields as JSON artifacts in the parent page/artifact store and records an artifact identifier on the work-unit yield event.
- Adds artifact image capture after every successful tool result at the guard layer: base64 image blocks are stored as artifacts and replaced in presentation with `artifact://` instructions.
- Adds rule firing/injection provenance and an explicit `abort_and_retry` action field to session events; rule-state reconstruction now uses visible/unmasked event history.
- Bumps session event schema from v6 to v7 and adds optional `ToolResultEvent.presentation` with `visible`, `canonical`, byte spans, optional source path/anchors/occurrence, and encoding label.
- Records ordinary fresh tool output as `encoding: "literal"`, where visible and canonical text are identical; it does not currently preserve the original structured tool-result blocks that were flattened first.
- Adds a reversible text grammar: reserved `§`, `×`, `{`, `}`, and backslash are escaped; repeated identical Unicode scalars may compress to `×<count>{value}`; decode rejects malformed syntax.
- Adds a byte-span table mapping each encoded grammar item to its decoded canonical byte range, including a single span for compressed repetition.
- Replaces the active CLI local-summary compaction branch with a checkpoint message containing a presentation-encoded JSON serialization of prior Rig messages, then retains a recent message suffix.
- Externalizes inline image bytes to attachment references before serializing that checkpoint, so raw base64 image data does not enter checkpoint text.
- Expands a previous exact checkpoint before producing the next one, rather than nesting checkpoints; legacy summary checkpoints cannot be expanded and remain literal historical input.
- Removes the runtime use of the previous local LLM-summary and provider-side Responses-compaction paths from the modified CLI compaction function.
- Adds profile/tool context events containing the resolved profile name/digest, full instructions/digest, active tool names, serialized tool definitions/digest, yield schema/digest, and extension provenance.
- Extends the Muse adapter to recognize v7 and to use `presentation.canonical` for structured result semantics while emitting `presentation.visible` as model-visible prose when that field exists.
- Changes the master checklist by marking broad subsystems complete without a matching requirement-by-requirement completion audit.

## Nonfunctional committed changes

- Moves a test module to satisfy clippy (`8fdbbb5`).
- Merges `origin/main` and later merges `main` into Gortnite (`a1390bd`, `468364b`).
- Adds/revises design, handoff, architecture, semantic, and verification documentation across Artist and Muse.
- Adds a binary handoff archive and several planning/status artifacts.

## Currently in checklist but not complete

- Do not flatten structured subsystem facts into prose when machine-structured output is already available.
- Use exact active-model token accounting for `read`.
- Use exact active-model token accounting for `poll`.
- Never split a logical line to satisfy a token budget.
- Preserve no hard caller ceiling while maintaining a compact default window.
- Finish exact continuation behavior for every text-backed real/virtual resource.
- Ensure continuation revision/boundary semantics are exact under concurrent mutation.
- Preserve the locked `poll` timeout semantics: omitted = indefinite, zero = immediate.
- Preserve multi-target `poll` wait-for-all behavior, with already-settled targets immediately satisfied.
- Generalize the same “current plus future, continuation strictly after terminating anchor” semantics consistently to every pollable textual resource where applicable.
- Ensure poll's lost-wakeup behavior is correct under concurrent settlement/output.
- If universal `edit` is the public mutation surface, make it preserve the todo semantic model rather than reducing todo to arbitrary text.
- Exercise/preserve difficult TUI behavior.
- Handle tmux/passthrough cases where required.
- Preserve visually meaningful attribute state when it affects semantic interpretation.
- Route terminal images/graphics through binary/artifact handling rather than textual escape leakage.
- Inventory shell responsibilities currently implemented/inherited by Artist.
- Inventory coreutils responsibilities/dependencies.
- Evaluate Brush as the shell implementation where it preserves/improves Artist semantics.
- Evaluate uutils for coreutils behavior where it preserves/improves Artist semantics.
- Migrate appropriate responsibilities to Brush.
- Migrate appropriate responsibilities to uutils.
- Explicitly document boundaries where replacement is inappropriate.
- Preserve persistent shell lifecycle.
- Preserve PTY/terminal emulator behavior.
- Preserve deterministic environment/profile behavior.
- Preserve typed/programmatic failures.
- Preserve `run` semantics.
- Complete TECA dictionary migration from Section 1.2.
- Freeze dictionary definition placement.
- Define when a new reference must be defined before use.
- Redefine required references after compaction, resume, or new conversation.
- Retain/version exact presentation emitted to a model when required for replay/training.
- Retain sufficient dictionary/version provenance to reconstruct what the model actually saw.
- Maintain explicit lossless mapping from model-visible encoded spans to canonical decoded spans.
- Map canonical decoded spans to the underlying resource path.
- Map relevant spans to durable anchors/occurrences.
- Ensure Muse source grounding uses this mapping rather than presentation offsets.
- Implement SnapCompact lossless text-to-bitmap packing for vision-capable models.
- Preserve continuation selectors through compaction.
- Preserve stale-recovery information through compaction.
- Preserve dictionary definitions needed to decode retained compressed text.
- Preserve exact paths/anchors needed for future calls.
- Preserve pending runtime resource paths.
- Preserve profile/schema/tool-surface version context.
- If an operational referent is not kept inline, guarantee a stable deterministic source can restore it.
- Complete remaining computer lifecycle/runtime migration under the path/result substrate.
- Ensure observations/screenshots route through canonical semantic snapshot + artifact handling.
- Ensure capability state visible to the model is recorded for Muse source interpretation.
- Use universal typed result/error behavior.
- Compose batching with structural/AST operations where safe.
- Keep dependent target semantics deterministic.
- Finish TTSR cleanup/completion rather than treating the crate's existence as subsystem completion.
- Resolve GitHub/GitLab/other provider mechanics behind the path resolver.
- Ensure canonical resource identity survives provider pagination/order.
- Integrate remote source locations/diffs with durable Artist anchors where meaningful.
- Make write/edit/run support capability-specific; do not pretend every forge resource is mutable.
- Produce minimal counterexamples.
- Describe current v6 representation.
- Classify the failure as impossible, ambiguous, noncanonical, or merely awkward.
- Define the required semantic equivalence class.
- Define learned/model-facing representation.
- Define deterministic compiler behavior.
- Define canonicalization invariant.
- Define formal lowering/certification behavior.
- Define ontology resolution.
- Define source-grounding behavior.
- Add negative tests excluding equivalent competing encodings.
- Repeat the full closure analysis above specifically for arbitrary restrictor proposition versus nuclear scope.
- Do not mistake the existing optional ontology domain for an arbitrary restrictor proposition.
- Define canonical equivalence across represented reading graphs, not merely deterministic ordering of reading IDs.
- Reject semantically equivalent but structurally competing representations when the contract requires one canonical form.
- Resolve repeated mentions.
- Resolve coreference.
- Resolve event re-mention.
- Resolve nominalization/event equivalence where semantically applicable.
- Resolve alternative decomposition identity.
- Preserve exact source grounding separately from semantic identity.
- Determine the required recursive/compositional term algebra.
- Ensure first-class composed semantic terms are representable canonically.
- Define cycle/termination rules.
- Move beyond proposition-only conjunction/disjunction where the semantic target requires plural/group/term composition.
- Represent distributive versus collective readings canonically.
- Preserve/source-ground coordination roles.
- Identify semantically equivalent alternative graph/decomposition forms.
- Define one canonical semantic projection/equivalence handling.
- Ensure formal lowering/certification does not depend on arbitrary equivalent decomposition choice.
- Evaluate whether one recursive open-construction algebra is the right solution.
- Evaluate first-class proposition/content/term constructions.
- Evaluate lexical-scope validation requirements.
- Evaluate positional/source-anchored construction roles.
- Evaluate compositional Event/Situation terms.
- Evaluate canonical semantic projection for certification.
- Evaluate recursive ontology resolution through open terms.
- Evaluate semantic firewall rules.
- Record any newly discovered closure vectors beyond the historical seven.
- Implement the actual labeling-provider/runtime stage.
- Feed it the appropriate occurrence/document source.
- Constrain model-produced claims with deterministic facts.
- Require source provenance.
- Require the narrowest justified ontology type.
- Use the narrowest valid generic type for unknown detail rather than discarding the object.
- Keep model labels semantically separate from deterministic facts.
- Calibrate confidence.
- Evaluate candidate rules/concepts against the retained real corpus in a sandbox.
- Keep candidate evaluation from changing global results.
- Implement a separately authorized promotion workflow.
- Require proof/evidence appropriate to promotion.
- Promote into a sealed newest-revision package.
- Preserve append-only ontology revision semantics.
- Leave automatic proof-backed promotion as a future capability unless explicitly scheduled; do not silently auto-promote today.
- Define the broader calibrated Bayesian model family needed beyond Beta–Bernoulli.
- Define policy for updating/replacing prior models without destroying provenance.
- Keep posterior/inferred beliefs distinguishable from explicit facts.
- Surface supporting and contrary evidence.
- Build complete high-utility finding-selection policy.
- Include compact supporting evidence.
- Include compact contrary evidence.
- Preserve source links.
- Keep formal applicability as the trigger.
- Retain the future option to inject paths for selected cases.
- Never let cosine similarity substitute for normative/formal applicability.
- Preserve the exact presentation the model actually saw.
- Preserve the exact canonical decoded meaning.
- Preserve the explicit lossless mapping between them.
- Preserve underlying path/anchor source grounding.
- Do not train behavioral source as though the model saw expanded text it never received.
- Do not ground semantics only to presentation offsets.
- Preserve yield as a model-produced report/assertion.
- Preserve agent/work-unit/schema/source evidence.
- Require independent evidence before promoting a yielded assertion to an independently observed fact.
- Ensure Muse storage/indexing consumes canonical/source semantic content.
- Retain model-visible compressed presentation only as presentation/provenance where behaviorally relevant.
- Never index `§...` or repetition syntax as if it were the underlying remembered fact.
- Freeze the final training-facing model-visible tool/capability surface.
- Freeze the final path/address grammar.
- Freeze the final line/dictionary/artifact TECA presentation.
- Freeze the final reversible codec rendering.
- Freeze the final ask semantics.
- Freeze the final structured yield semantics.
- Freeze the final binary/multimodal presentation.
- Freeze the final profile live-update semantics.
- Freeze the final universal result/event IR.
- Reconcile current MCP implementation with stable logical session identity.
- A logical MCP/web session must not receive a fresh unrelated Artist identity every request.
- Reconnects should resolve the same logical identity when stable transport identity exists.
- Parallel logical sessions must not collapse into one identity.
- Identity-free discovery must not consume roster identities.
- If transport has no stable identity, use an explicit threaded identity handle rather than accidental hidden server state.
- Model-addressed mailbox messages have one model consumer.
- Canvas/internal invokes must not drain messages intended for the model.
- Tool visibility remains governed by resolved profile/policy rather than redundant contradictory gates.
- Explicitly document what current implementation supersedes from the older MCP design.
- Automatic GC may delete only explicitly disposable caches and unreferenced temporary artifacts.
- Never automatically delete reachable resources.
- Never automatically delete retained names while still semantically referenced.
- Never automatically delete dictionary entries.
- Never automatically delete Muse records.
- Define when settled operational resources disappear from active enumeration.
- Define when an operational resource becomes history-only.
- Define manual-delete effects on parent/child/predecessor/successor relationships.
- Define artifact/yield retention.
- Define broken pointers after GC/deletion.
- Automatic artifact capture/routing from producing occurrences/yields.
- Complete canonical typed paths for remaining runtime/resource kinds.
- Remove split legacy IDs/surfaces from the target ABI.
- Finish universal result/error behavior.
- Finish exact token-budgeted read/poll and continuations.
- Finish relationships + choose/implement their backing store.
- Finish asks/todo semantics under the path-first surface.
- Finish artifact/binary routing.
- Finish dictionary/repetition reversible codec.
- Finish exact structural condensation + SnapCompact.
- Finish terminal hardening.
- Execute Brush/uutils migration evaluation.
- Finish remaining computer/canvas/runtime migration.
- Implement Python eval with Artist-tool callbacks.
- Implement DAP debugger sessions.
- Implement LSP multiplexer/integration.
- Finish AST/code graph + structural batching.
- Finish TTSR cleanup/path integration.
- Implement forge/API virtual paths.
- Run the seven-vector semantic closure redesign.
- Re-derive useful lost-v7 architectural ideas rather than blindly recreating them.
- Implement actual model labeling runtime.
- Implement candidate evaluation/promotion.
- Extend Bayesian inference as required.
- Finish finding-selection/autofire semantics.
- Finish exact presentation/canonical/source mappings.
- Finish versioned Artist↔Muse behavioral source contract.
- Final-freeze training/adapter semantics only after relevant Artist surfaces stabilize.
- Make the target model-facing ABI consistently path-first without split legacy identity semantics.
- Make line anchors genuinely compact TECA addresses, not generic hex wire strings.
- Make dictionary identities exact-substring TECA prefixes.
- Make artifact paths TECA-derived from occurrence envelope + exact bytes and artifact routing harness-owned.
- Keep revisions/continuations stale-safe under concurrent mutation.
- Make read/poll use exact active-model token accounting and preserve whole logical lines.
- Make terminal, artifacts, dictionary codec, repetition, and compaction preserve exact canonical meaning.
- Make relationships first-class and back them with a deliberately selected persistence/query model.
- Make Python eval, DAP, LSP, AST/code graph, TTSR, Brush/uutils work, and forge paths actually present.
- Make computer/canvas/tools/runtime creation use the unified path/result lifecycle.
- Make Muse resolve the seven known semantic closure vectors and any new ones discovered.
- Make Muse actually label, evaluate ontology candidates, reason probabilistically with calibrated provenance, and autofire only on formal applicability.
- Make Artist↔Muse normalization preserve exact canonical source plus exact model-visible tool/profile/codec context for each occurrence.
- Keep explicit facts, human decisions, auto-resolved answers, model reports/yields, rule findings, labels, and Bayesian inferences semantically distinguishable.
- Do not silently drop any product requirement because an earlier implementation did something narrower.
