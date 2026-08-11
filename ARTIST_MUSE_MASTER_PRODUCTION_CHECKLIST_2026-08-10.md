# Artist + Muse Master Production Checklist

Date: 2026-08-10

Status: **AUTHORITATIVE EXECUTION CHECKLIST**

Sources merged:

- `ARTIST_UNIFIED_STATE_AND_EXECUTION_PLAN_2026-08-09(1)(1).md`
- `ARTIST_PRODUCTION_PLAN_CORRECTIVE_2026-08-10(1).md`
- implementation state previously inspected from supplied `artist.zip`, branch `Gortnite`, HEAD `65c23ae8535d57e60a93e5ae00c29430181a6a76`, with staged work on top.

This document replaces the split “production plan + corrective” workflow for execution. The agent should work from this checklist.

A checked item means the earlier production tracker recorded implementation/direct evidence for that specific slice and the corrective did not invalidate it. A checked item does **not** imply the enclosing subsystem is complete.

Do not use checked/total percentage as project progress.

Do not add ceremonial whole-system gates around bad or transitional systems. Direct tests for newly implemented contracts are normal engineering; giant verification bureaucracy is not a product requirement.

---

# 0. Locked product contract — do not re-decide casually

These are requirements/decisions, not progress indicators.

- Artist is a path-first execution harness. Every model-addressable thing is a real path or typed virtual path.
- Current core roots include `agent://`, `bash://`, `ask://`, `canvas://`, `computer://`, `artifact://`, `dict://`, `memory://`, `profile://`, `skill://`, and `tools://`.
- Production scope also includes typed/addressable eval, debugger/DAP, forge/repository, code/AST graph, relationship, and TTSR/rule resources where their final noun-space design requires roots/projections.
- `agent://<artist>/todo` is the only todo-tree location. Do not add a `todo://` alias.
- Directory and scheme-root `read` returns a compact immediate-child listing. `find` performs traversal/discovery.
- `read` is stateless.
- Text continuations are revision-bound. Any resource change makes the old continuation stale.
- Text logical lines use TECA-derived anchors.
- Canonical line identity is the exact logical-line bytes plus label-free CST kind/role ancestry plus the frozen bidirectional equivalent-sibling rank rule.
- `edit` resolves all targets against one pre-edit revision, rejects stale/overlapping targets, commits atomically, and returns rebased anchors.
- Existing-file `write` requires the exact current revision. External edits never get silently overwritten.
- Core verb surface remains `read`, `find`, `grep`, `edit`, `write`, `poll`, `send`, `run`, `stop`, `delete`, plus typed runtime creation such as `agent`, `bash`, `ask`, and `computer`.
- `stop` is graceful cancellation followed by fixed-grace force termination. `delete` is permanent removal and must not be conflated with stopping.
- Every failure returns a typed error, explanation, affected path/revision, and usable recovery action/path.
- `find` is frecency-ranked. Explicit reads/human opens update frecency; discovery does not.
- `grep.mode=auto` is literal-first and falls back to FFF fuzzy search only if literal search has no result.
- `read(bash://x)` is the append-only semantic terminal transcript.
- `poll(bash://x)` observes the current emulator screen.
- Terminal control bytes are never model text.
- Track `wezterm-term` upstream `main`; preserve the exact upstream revision in provenance where semantic reproducibility requires it.
- Artifacts are a **TECA namespace by design**.
- Artifact identity is derived from the canonical occurrence envelope plus exact bytes so byte-identical artifacts from distinct occurrences remain distinct.
- Artifact creation/routing is harness-owned; an agent/yielder must not be forced to perform a separate explicit artifact write merely to preserve produced artifacts.
- Raw binary is never injected into model text.
- The global `dict://` dictionary is permanent and never GC'd under the current production decision.
- Dictionary references use `§<opaque-prefix>`.
- Repetition uses `×<count>{value}` unless a later explicit grammar decision supersedes that exact notation.
- Fresh output remains ordinary text.
- Final compaction is lossless: exact structural condensation first, then SnapCompact lossless text-to-bitmap packing for vision-capable models; non-vision models receive exact condensed text.
- Lossy conversation summaries are not the target compaction architecture.
- Session transcripts are editable files, but Muse compiles captured original events rather than treating later transcript edits as historical truth.
- Completed agent yields are JSON validated against the applicable profile yield schema and exposed under `agent://<artist>/yields/<n>`.
- Skills are ordinary `skill://` files. Reading is activation; do not create hidden persistent skill activation state.
- Computer rung 0 remains direct path interaction plus an addressable capability catalog.
- Muse ingests captured Artist events and files on `read`, `edit`, and `write`, not proactively and not merely because `find`/`grep` discovered something.
- Muse ingestion failure never fails the originating successful Artist operation; it becomes a `memory://diagnostics/<id>` record.
- Muse is a model-produced semantic labeling/reasoning system constrained by deterministic extraction and validation; deterministic facts do not replace broad model labeling.
- Explicit facts, model labels, Bayesian beliefs, rule findings, auto-resolved asks, structured yields, and inferred hypotheses are semantically distinct.
- Normative reasoning derives only from rules, profiles, and project documents; similarity alone never establishes normative applicability.
- Ontology is append-only and interpreted at newest revision.
- Ontology candidates are inert until separately authorized promotion.
- Hard logical contradictions are rejected by construction and surfaced as diagnostics.
- Formal semantics starts with Artist tool/runtime behavior. Rust is the first programming-language formalization target.
- Autofire uses formal applicability, not turn cadence or cosine retrieval.

The required token-efficient TECA consumers already identified are:

1. logical-line anchors;
2. exact-substring dictionary identities.

Artifacts are also a TECA namespace by explicit design, but that is an artifact-identity decision, not a reason to invent arbitrary additional TECA consumers.

---

# 1. P0 — Correct current TECA/artifact implementation drift

## 1.1 Line anchors

- [x] Remove the old local v1 mnemonic allocator/token vocabulary from active address generation.
- [x] Depend on the published `teca` crate.
- [x] Construct deterministic occurrence identities before addressing.
- [x] Feed canonical occurrence-identity bytes into TECA.
- [x] Compute shortest uniqueness over structural TECA `AtomId`s.
- [x] Keep anchor generation stateless across agents/processes.
- [x] Replace generic `teca::render::render_text_prefix(...)` length+hex rendering with compact canonical TECA lexicon-atom rendering for model-facing `#...` anchors.
- [x] Treat the complete rendered anchor suffix as an opaque prefix over actual candidate TECA streams.
- [x] Resolve zero candidate matches as stale/unknown.
- [x] Resolve multiple matches as ambiguous and return longer sufficient candidates; never guess.
- [x] Resolve exactly one candidate as the target.
- [x] Confirm no actor/session/process-local allocator state participates in identity or rendering.
- [x] Ensure no resurrection of `mnemonic_words.txt`, local hash-to-word mapping, one-word/two-word slots, allocator cursors, or mnemonic tombstone allocation.

## 1.2 Exact-substring dictionary TECA identities

Existing global dictionary infrastructure is useful, but its current SHA-256/serial identity scheme is wrong.

- [x] Persist a permanent global `dict://` store.
- [x] Persist exact dictionary content atomically.
- [x] Provide model-facing `§...` references.
- [x] Delete SHA-256-truncation as the model-facing dictionary identity source.
- [x] Delete numeric collision suffix allocation as the dictionary address source.
- [x] Define dictionary semantic identity as the exact canonical substring.
- [x] Encode that substring as exact UTF-8 bytes with no normalization, trimming, case folding, hashing, summarization, fuzzy matching, or tokenization before TECA.
- [x] Feed the exact bytes directly into TECA.
- [x] Compute the shortest currently unique TECA prefix across known dictionary identities.
- [x] Render that TECA prefix after `§`.
- [x] Resolve `§` values against actual known TECA streams, not against heuristically parsed text fragments.
- [x] Zero matches => unknown/stale reference.
- [x] One match => expand to the exact persisted canonical substring.
- [x] Multiple matches => fail safely and return longer disambiguating references.
- [x] Never choose among colliding dictionary entries heuristically.
- [x] Keep dictionary discovery/profitability/definition placement/repetition/presentation logic in Artist rather than reimplementing TECA internals.

## 1.3 Artifact namespace is TECA by design

Do not reopen whether artifacts “should” use TECA.

- [x] Retain oversized-result artifact metadata projections under `artifact://`.
- [x] Keep raw artifact payload bytes out of normal model text.
- [x] Derive artifact semantic identity material from canonical occurrence envelope plus exact payload bytes.
- [x] Preserve occurrence distinction when identical bytes are emitted in distinct occurrences.
- [x] Preserve legacy UUID artifact reads where migration compatibility requires them.
- [ ] Replace SHA-256/truncated-hash artifact path generation with TECA-derived artifact paths.
- [ ] Preserve occurrence distinction in the TECA input material rather than relying on an unrelated rendered ID system.
- [ ] Ensure artifacts produced by tool results, yields, screenshots, images, or other supported binary/multimodal outputs are captured/routed by the harness without requiring the producing agent/yielder to manually write an artifact.
- [ ] `read(artifact://...)` must return metadata plus the appropriate visual/multimodal attachment when supported.
- [ ] Define programmatic raw-byte access separately from model-facing text projection if required.
- [ ] Keep binary/media type, provenance, occurrence identity, and payload revision addressable.

---

# 2. P1 — Finish the path/result substrate and remove split legacy semantics

## 2.1 Existing completed substrate slices

- [x] Return a revision from text `read`.
- [x] Require the revision for `edit`.
- [x] Require the revision for replacement of an existing file with `write`.
- [x] Reject stale revision mutations while holding the cross-process file lock.
- [x] Keep text batch edits snapshot-relative.
- [x] Reject overlapping/incompatible text edits.
- [x] Atomically commit successful text edit batches.
- [x] Return post-edit anchors.
- [x] Normalize stale write replacement races to typed `stale_revision`.
- [x] Normalize stale batch-edit/anchor failures to typed `stale_revision`.
- [x] Parse the currently declared virtual-path schemes without traversal/selectors.
- [x] Resolve existing `agent://`, `bash://`, `ask://`, `canvas://`, and `computer://` roots/session snapshots through the durable registry.
- [x] Ensure declared virtual roots never fall through into host-path interpretation.
- [x] Reject virtual paths at generic file `edit`/`write` boundaries until the owning runtime exposes a canonical mutable projection.
- [x] Make real-directory `read` an immediate-child listing rather than a recursive dump.
- [x] Rank unfiltered real-directory reads most-recent-first with deterministic lexical tie-breaking.
- [x] Bind text-backed virtual pagination continuations to exact preceding-page revisions.
- [x] Return stale recovery instead of silently retargeting a changed continuation.

## 2.2 Complete the namespace

- [ ] Add/freeze the production path shape for Python eval sessions.
- [ ] Add/freeze the production path shape for debugger/DAP sessions.
- [ ] Add/freeze the production path shape for forge/repository resources.
- [ ] Add/freeze code/symbol/AST graph projections.
- [ ] Add/freeze relationship projections for parent/children/predecessor/successor/pointers.
- [ ] Decide whether TTSR/rule resources need a dedicated root or projections under an existing root and freeze that ABI.
- [ ] Ensure every newly declared root is readable even before all specialized behavior is installed, without host-path fallthrough.
- [ ] Ensure all model-facing runtime resources use canonical real/typed virtual paths.
- [ ] Eliminate remaining bare legacy runtime IDs from the target model-facing ABI.
- [ ] Resolve the current one-shot native `process:<slug>` mismatch: either give process resources a canonical typed path/root or deliberately fold them into an existing typed runtime namespace.
- [ ] Remove legacy `list` from the target public surface when all enumeration paths are covered by `read`/`find`.
- [ ] Remove or quarantine legacy `abort`/`subagent` compatibility surfaces once `stop`/`agent` migration no longer needs them.

## 2.3 Universal result/error behavior

- [ ] Finish one path/result envelope across real files and virtual resources.
- [ ] Every failure must include a typed error code/type.
- [ ] Every failure must include a concise explanation.
- [ ] Every stale/path-sensitive failure must identify the affected path and relevant revision.
- [ ] Every recoverable failure must return a usable recovery action/path.
- [ ] Do not flatten structured subsystem facts into prose when machine-structured output is already available.

---

# 3. P1 — Search, read, grep, polling, and continuations

## 3.1 Existing completed behavior

- [x] `grep.mode=auto` performs literal search first and fuzzy fallback only if literal search returns no result.
- [x] Preserve explicit `literal`, `regex`, and `fuzzy` modes.
- [x] Use the FFF/neo-frizbee fuzzy engine for real and virtual-session text discovery.
- [x] Preserve canonical source order inside returned matching line chunks.
- [x] Route explicit virtual `find` scopes through the durable session registry.
- [x] Rank virtual matching resources by explicit-open frecency rather than discovery or lexical score.
- [x] Route explicit virtual `grep` scopes through canonical session snapshots.
- [x] Keep virtual-session discovery passive: `find`, `grep`, and root listings do not refresh recency.
- [x] Explicit resource `read` updates interaction recency.
- [x] Honor caller-adjusted real-text read limits without a fixed byte/line ceiling.
- [x] Preserve whole logical lines.

## 3.2 Remaining read/poll work

- [ ] Use exact active-model token accounting for `read`.
- [ ] Use exact active-model token accounting for `poll`.
- [ ] Never split a logical line to satisfy a token budget.
- [ ] Preserve no hard caller ceiling while maintaining a compact default window.
- [ ] Finish exact continuation behavior for every text-backed real/virtual resource.
- [ ] Ensure continuation revision/boundary semantics are exact under concurrent mutation.
- [ ] Preserve the locked `poll` timeout semantics: omitted = indefinite, zero = immediate.
- [ ] Preserve multi-target `poll` wait-for-all behavior, with already-settled targets immediately satisfied.
- [x] Regex-aware terminal `poll` evaluates current plus future canonical output.
- [x] Terminal matches return a revision-bound continuation strictly after the matched TECA anchor.
- [ ] Generalize the same “current plus future, continuation strictly after terminating anchor” semantics consistently to every pollable textual resource where applicable.
- [ ] Ensure poll's lost-wakeup behavior is correct under concurrent settlement/output.

---

# 4. P1 — Session lifecycle, agents, profiles, todos, asks, relationships

## 4.1 Existing lifecycle work

- [x] Add durable `stop`.
- [x] `stop` requests graceful cancellation and uses a fixed two-second restart-safe grace deadline before force cancellation.
- [x] Reject new input while a resource is stopping.
- [x] Add durable `delete`.
- [x] Reject deletion of live sessions.
- [x] Delete stopped records/queued input and release retained name leases where applicable.
- [x] Preserve unrelated Muse-derived records when deleting stopped agents.
- [x] Accept canonical `agent://`, `bash://`, `ask://`, `canvas://`, and `computer://` targets for lifecycle operations.
- [x] Reject relationship-child projections as if they were live runtime resources.
- [x] Add canonical `agent({profile, brief})` creation.
- [x] Claim a durable Artist roster name and start immediately.
- [x] Preserve `send(agent, brief)`/work-unit continuation semantics through the active lifecycle implementation.

## 4.2 Agent yields

- [x] Validate completed delegated work units against the resolved profile's optional inherited `yieldSchema`.
- [x] Fail the run rather than store an invalid “completed” yield.
- [x] Expose valid yields read-only at `agent://<artist>/yields/<n>`.
- [x] Surface resolved yield schema through `profile://`.
- [ ] Ensure artifact-producing yields are automatically captured into the TECA artifact namespace without requiring explicit artifact writes.
- [ ] Preserve agent identity, work-unit identity, yield schema/version, exact yielded content, and supporting transcript/tool evidence in Muse normalization.
- [ ] Treat a yield as a model-produced report/assertion, never automatically as an independently observed fact.

## 4.3 Profiles and skills

- [x] Expose resolved profiles through revisioned `profile://` snapshots.
- [x] Apply tool/skill allow/deny policy by strongest matching glob specificity.
- [x] Deny wins only exact specificity ties.
- [x] Absent allow-list means allow-all.
- [x] Present unmatched allow-list means deny-all.
- [x] Re-resolve live profile changes at completion boundaries.
- [x] Inject changed instructions through the standard non-interrupting request patch.
- [x] Immediately skip already-registered tools newly denied by a valid live policy edit.
- [x] Retain the last valid instructions/policy after an invalid profile edit.
- [x] Make `skill://` ordinary profile-governed files.
- [x] Reading a skill is the only activation.
- [x] Do not maintain hidden persistent skill activation state.
- [ ] Record exact resolved profile revision/digest for each governed work unit/turn boundary in Muse source context.
- [ ] Record the exact tool policy visible at that occurrence.
- [ ] Record the exact yield schema applicable at that occurrence.
- [ ] Record the exact profile instructions injected at that occurrence.

## 4.4 Todo semantics

- [x] Expose each retained agent's todo tree only at `agent://<artist>/todo`.
- [x] Keep `todo://` absent.
- [ ] Preserve one todo tree per owning agent/session.
- [ ] Do not expose hidden direct parent-todo access.
- [ ] Preserve states `Open{Idle}`, `Open{Active}`, `Closed{Done}`, `Closed{Failed}`, `Closed{Cancelled}`, `Closed{Inherited}`.
- [ ] Enforce: an open descendant forces all ancestors open.
- [ ] Enforce: closing a parent cascades `Closed{Inherited}` to still-open descendants.
- [ ] Enforce: `Inherited` is system-owned and never model-authored directly.
- [ ] Enforce: reopening a parent does not automatically reopen inherited descendants.
- [ ] Preserve deterministic nested-tree addressing.
- [ ] Preserve atomic all-or-nothing structural todo mutations.
- [ ] Keep status mutation semantically distinct from ordinary text replacement.
- [ ] Surface enough displacement/path-change information after structural mutation for the model to retain correct references.
- [ ] If universal `edit` is the public mutation surface, make it preserve the todo semantic model rather than reducing todo to arbitrary text.

## 4.5 Ask semantics

- [ ] `ask` creation returns/backgrounds immediately.
- [ ] Treat one ask batch as one human-decision work unit.
- [ ] Block explicitly through `poll`, never by making `ask` itself synchronously wait.
- [ ] Support 1..infinity questions.
- [ ] Support 0..infinity options per question.
- [ ] Zero options means free response.
- [ ] Keep arbitrary `header` out of the target schema.
- [ ] Keep model-controlled `multiSelect` out of the target schema.
- [ ] Human UI permits multi-select.
- [ ] Options carry ordinary option text plus mandatory `recommended: bool`.
- [ ] Preserve per-selection annotations/notes.
- [ ] Preserve dismissed answer as an empty selection set.
- [ ] Preserve original option order exactly; no sorting/deduplication may change index identity.
- [ ] If timeout auto-resolution remains enabled, select exactly the recommended options.
- [ ] If timeout auto-resolution has no recommended options, resolve as dismissed.
- [ ] Auto-resolution settles completed rather than cancelled.
- [ ] Persist `Human | AutoResolve` provenance.
- [ ] Do not encode a UX rule that hides provenance from the live model unless a later explicit decision restores that behavior.
- [ ] Muse must never normalize auto-resolution into “the human chose X” merely because the returned answer content is equivalent.
- [ ] Do not resurrect a separate MCP-only ask outbox merely because transport is stateless.

---

# 5. P1 — Relationship traversal and backing store

Relationship traversal is production scope, not an optional appendix.

## 5.1 Relationship model

- [x] Agent canonical paths already expose immediate read-only relationship children for todo/yields.
- [ ] Add addressable immediate parent.
- [ ] Add addressable children.
- [ ] Add immediate agent children where useful.
- [ ] Add predecessor.
- [ ] Add successor.
- [ ] Add general pointer/reference relationships to other resource paths where required.
- [ ] Preserve the rule that a spawned thing generally has one immediate parent.
- [ ] Allow that parent to be a non-agent runtime/resource.
- [ ] Keep predecessor/successor independent of parentage so handoff continuity is representable.
- [ ] Freeze the exact relationship path shape.
- [ ] Freeze whether relationships are explicit pointer resources, projections, aliases, or a deliberate mixture.
- [ ] Define cycle handling.
- [ ] Define broken-link handling.
- [ ] Define deletion behavior.
- [ ] Define relationship behavior after target deletion/GC.
- [ ] Define whether `find` follows relationship pointers by default.
- [ ] Define whether `grep` follows relationship pointers by default.
- [ ] Define relationship identity separately from target-resource identity where needed.

## 5.2 Relational store

This is a required architecture decision/implementation.

- [ ] Evaluate reuse of Mnestic/Cozo/RocksDB for Artist runtime relationships.
- [ ] Evaluate a purpose-built relationship registry/index.
- [ ] Evaluate any better existing relational/graph substrate discovered during implementation.
- [ ] Compare transaction semantics.
- [ ] Compare lifecycle/GC semantics.
- [ ] Compare migration requirements.
- [ ] Compare query/traversal behavior.
- [ ] Compare durability/restart behavior.
- [ ] Compare indexing requirements.
- [ ] Do not couple Artist runtime relationships, existing Artist memory, and Muse ontology/fact persistence merely because each is graph-shaped.
- [ ] Select and implement the relationship backing store deliberately.

---

# 6. P2 — Terminal, shell, Brush/uutils

## 6.1 Existing terminal implementation

- [x] Retain full live `bash://` semantic terminal transcript.
- [x] Give terminal transcript a strict revision.
- [x] TECA-anchor logical transcript lines.
- [x] Feed raw PTY output chunks into `wezterm-term`.
- [x] `read(bash://...)` uses the append-only sanitized semantic transcript.
- [x] `poll(bash://...)` evaluates the emulator's current visible screen.
- [x] Preserve cursor movement/erase/alternate-screen semantics instead of flattening them into append-only text for polling.
- [x] Strip terminal control traffic before model-visible transcript text.
- [x] Preserve full underlying live terminal output.

## 6.2 Terminal hardening still required

- [ ] Exercise/preserve difficult TUI behavior.
- [ ] Exercise/preserve alternate-screen behavior.
- [ ] Handle terminal queries/responses correctly.
- [ ] Handle tmux/passthrough cases where required.
- [ ] Preserve visually meaningful attribute state when it affects semantic interpretation.
- [ ] Route terminal images/graphics through binary/artifact handling rather than textual escape leakage.
- [ ] Record the exact `wezterm-term` upstream revision in provenance where Muse/replay semantics require exact behavior.

## 6.3 Brush/uutils workstream

- [ ] Inventory shell responsibilities currently implemented/inherited by Artist.
- [ ] Inventory coreutils responsibilities/dependencies.
- [ ] Evaluate Brush as the shell implementation where it preserves/improves Artist semantics.
- [ ] Evaluate uutils for coreutils behavior where it preserves/improves Artist semantics.
- [ ] Migrate appropriate responsibilities to Brush.
- [ ] Migrate appropriate responsibilities to uutils.
- [ ] Explicitly document boundaries where replacement is inappropriate.
- [ ] Preserve persistent shell lifecycle.
- [ ] Preserve PTY/terminal emulator behavior.
- [ ] Preserve deterministic environment/profile behavior.
- [ ] Preserve typed/programmatic failures.
- [ ] Preserve `run` semantics.

---

# 7. P2 — Artifacts, dictionary codec, repetition, and lossless compaction

## 7.1 Artifact routing

- [x] Artifact reads expose metadata rather than dumping binary into text.
- [x] Oversized-result payloads can be retained behind artifact/pagination machinery.
- [ ] Complete TECA artifact-path migration from Section 1.3.
- [ ] Support visual attachment rendering on `read(artifact://...)` where supported.
- [ ] Define binary/media programmatic access separately from model-facing text.
- [ ] Route screenshots/computer/canvas/terminal images through artifact semantics.
- [ ] Ensure yield-produced artifacts are captured automatically by the harness.

## 7.2 Dictionary presentation and lifecycle

- [ ] Complete TECA dictionary migration from Section 1.2.
- [ ] Freeze repeated-substring discovery strategy.
- [ ] Freeze profitability threshold/selection strategy.
- [ ] Freeze dictionary definition placement.
- [ ] Define when a new reference must be defined before use.
- [ ] Redefine required references after compaction, resume, or new conversation.
- [ ] Preserve permanent/no-GC dictionary policy unless explicitly superseded.
- [ ] Define exact known-entry indexing under `dict://`.

## 7.3 Repetition and codec grammar

- [ ] Implement `×<count>{value}` or the explicitly superseding frozen repetition grammar.
- [ ] Freeze punctuation/nesting grammar.
- [ ] Implement unambiguous escaping/quoting for literal reserved syntax such as `§`, repetition syntax, and selector-like punctuation.
- [ ] Guarantee exact decode(encode(x)) round trips over all valid text.
- [ ] Preserve exact canonical text independently from its compressed presentation.
- [ ] Never allow codec syntax itself to become accidental semantic source truth.

## 7.4 Dictionary-prefix history

- [ ] Handle shortest-prefix instability when future dictionary entries make an old prefix ambiguous.
- [ ] Keep canonical text/event data authoritative.
- [ ] Retain/version exact presentation emitted to a model when required for replay/training.
- [ ] Retain sufficient dictionary/version provenance to reconstruct what the model actually saw.

## 7.5 Presentation ↔ canonical mapping

- [ ] Maintain explicit lossless mapping from model-visible encoded spans to canonical decoded spans.
- [ ] Map canonical decoded spans to the underlying resource path.
- [ ] Map relevant spans to durable anchors/occurrences.
- [ ] Ensure Muse source grounding uses this mapping rather than presentation offsets.

## 7.6 Structural condensation + SnapCompact

- [ ] Remove lossy LLM summarization as the final compaction architecture.
- [ ] Implement exact structural condensation.
- [ ] Implement SnapCompact lossless text-to-bitmap packing for vision-capable models.
- [ ] Provide exact condensed text to non-vision models.
- [ ] Preserve continuation selectors through compaction.
- [ ] Preserve stale-recovery information through compaction.
- [ ] Preserve dictionary definitions needed to decode retained compressed text.
- [ ] Preserve exact paths/anchors needed for future calls.
- [ ] Preserve pending runtime resource paths.
- [ ] Preserve profile/schema/tool-surface version context.
- [ ] If an operational referent is not kept inline, guarantee a stable deterministic source can restore it.

---

# 8. P2 — Runtime creation, canvas, computer, tools

## 8.1 `run`

- [x] Add path-first `run` for real executables and explicitly interpreted scripts.
- [x] Quote arguments exactly.
- [x] Interpreted scripts create durable bash lifecycle resources.
- [x] Route explicit JavaScript/TypeScript canvas-source paths to their owning live canvas session.
- [x] Direct native executables create one-shot process snapshots.
- [ ] Resolve the remaining canonical typed-path identity for one-shot native process resources.
- [ ] Ensure `run` behavior is represented consistently in Muse events/results.

## 8.2 Computer rung 0

- [x] Publish a revisioned `computer://use/<action>` capability catalog.
- [x] Make catalog paging stale-safe.
- [x] Route `send(computer..., {intent,args?})` through declared capabilities.
- [x] Return runnable capability paths/reasons for unsupported intents.
- [ ] Complete remaining computer lifecycle/runtime migration under the path/result substrate.
- [ ] Ensure observations/screenshots route through canonical semantic snapshot + artifact handling.
- [ ] Ensure capability state visible to the model is recorded for Muse source interpretation.

## 8.3 `tools://`

- [x] Expose extension manifests and binary metadata without dumping raw WASM as model text.
- [x] Atomically compile/instantiate a replacement set before swap.
- [x] Retain the last working instance/layout if candidate activation fails.
- [x] Restart only replacement status/event tasks.
- [x] Support host-triggered reload.
- [x] Dispatch active callbacks locally through canonical `tools://<extension>/tool/<name>` run paths.
- [ ] Ensure final provider/tool schemas reflect active extensions at the intended refresh boundary.
- [ ] Record extension/tool-surface version context for Muse.
- [ ] Preserve tool/event provenance across hot swaps.

---

# 9. P2 — Python `eval://`

The previous Rust/evcxr direction is superseded. Benchmarks led to the explicit decision to use a Python REPL that can call back into Artist tools.

- [ ] Freeze the canonical `eval://` path hierarchy.
- [ ] Implement a persistent/session-shaped Python REPL.
- [ ] Make eval state addressable rather than hidden bespoke process state.
- [ ] Allow Python eval code to call back into permitted Artist tools.
- [ ] Route tool callbacks through the same permission/profile boundary as ordinary tool calls.
- [ ] Prevent eval from bypassing tool policy merely because it executes locally.
- [ ] Define persistence/reset semantics.
- [ ] Define imports/environment/package behavior.
- [ ] Define cwd/workspace behavior.
- [ ] Return structured/programmatic outputs.
- [ ] Return typed execution errors.
- [ ] Define relation to `run` and `bash://`.
- [ ] Ensure eval-created resources receive normal parent/child relationships.
- [ ] Normalize eval inputs/results/tool callbacks into Muse with exact source/tool context.
- [ ] Add direct contract tests for Python↔Artist-tool callbacks and policy enforcement.

---

# 10. P2 — Debugger/DAP

DAP is required; do not invent a private debugger multiplexer.

- [ ] Freeze canonical `debug://` session hierarchy.
- [ ] Implement DAP launch.
- [ ] Implement DAP attach.
- [ ] Implement breakpoints.
- [ ] Implement continue.
- [ ] Implement pause.
- [ ] Implement step in.
- [ ] Implement step over.
- [ ] Implement step out.
- [ ] Expose stack frames.
- [ ] Expose scopes.
- [ ] Expose variables.
- [ ] Implement debugger evaluate.
- [ ] Implement termination/cleanup.
- [ ] Map debugger source locations to durable Artist anchors.
- [ ] Use universal typed result/error behavior.
- [ ] Put debug sessions into parent/child/history relationships.
- [ ] Record debugger/server/capability context when observations enter Muse.
- [ ] Keep debugger state/projections addressable through the noun space.

---

# 11. P2 — LSP

LSP is mandatory production scope.

- [ ] Implement/mature an LSP multiplexer.
- [ ] Avoid accidental one-agent/one-server silos when shared servers are safe.
- [ ] Scope diagnostics/requests to the correct agent/worktree/project state.
- [ ] Attribute diagnostics/changes to responsible agents/worktrees where possible.
- [ ] Return definition/reference/symbol locations using durable Artist anchors.
- [ ] Expose symbols.
- [ ] Expose definitions.
- [ ] Expose references.
- [ ] Expose diagnostics.
- [ ] Expose hover/type information.
- [ ] Support rename where safe.
- [ ] Support code actions where safe.
- [ ] Expose relevant workspace/project state.
- [ ] Preserve server/version/provenance when results can differ by server build.
- [ ] Integrate with AST/code intelligence instead of blindly duplicating structural facts.
- [ ] Keep useful structured LSP results machine-structured.

---

# 12. P2 — AST/code intelligence and batched structural work

The existing AST work is a starting point, not the complete requirement.

## 12.1 Required code graph breadth

- [ ] Expose symbols.
- [ ] Expose definitions.
- [ ] Expose references.
- [ ] Expose callers.
- [ ] Expose callees.
- [ ] Expose dependencies.
- [ ] Expose implementations.
- [ ] Expose cycles.
- [ ] Expose traces.
- [ ] Expose impact.
- [ ] Expose related structures.
- [ ] Use accurate durable source anchors for every source-location-bearing result.
- [ ] Keep graph/relationship results machine-structured.

## 12.2 Structural operations

- [ ] Keep AST/query operations explicit when they cannot truthfully collapse into ordinary path/read semantics.
- [ ] Define exact structural-query contracts.
- [ ] Define exact structural-edit preconditions.
- [ ] Define exact structural-edit postconditions.
- [ ] Prevent partial tree corruption on failed structural edits.
- [ ] Attribute LSP/structural findings to responsible agent/change where possible.

## 12.3 First-class batching

- [x] Ordinary text edit already resolves all targets against one pre-edit snapshot and commits atomically.
- [ ] Add first-class batched reads where they materially reduce repeated/intermediate-state slop.
- [ ] Compose batching with structural/AST operations where safe.
- [ ] Keep dependent target semantics deterministic.
- [ ] Return exact per-operation failure when an atomic structural batch cannot commit.
- [ ] Do not silently partially apply a structural refactor.

---

# 13. P2 — TTSR / `artist-rules`

TTSR is production scope. Existing code is substantial but not final.

- [x] Retain existing declarative rule support.
- [x] Retain stream matching.
- [x] Retain abort/inject/retry-style rule actions where still semantically valid.
- [x] Retain per-session/per-turn firing machinery.
- [x] Retain retroactive scans where designed.
- [x] Retain WASM programmable rules where designed.
- [x] Retain hot reload.
- [ ] Audit all rule terminology/actions against the new path-first lifecycle.
- [ ] Decide/freeze canonical TTSR/rule path projection.
- [ ] Reconcile old `abort` semantics with public `stop` and any internal immediate-cancel primitive.
- [ ] Reconcile rule injection with the standard non-interrupting context-update primitive.
- [ ] Remove stale assumptions tied to old tool/session names.
- [ ] Ensure rewind/resume restores the intended rule state.
- [ ] Preserve exact rule/version provenance in captured events.
- [ ] Muse must distinguish a rule firing from the underlying fact that made it applicable.
- [ ] Muse must distinguish rule-injected text from independently observed source truth.
- [ ] Muse must distinguish retry/cancellation action from the semantic reason/evidence.
- [ ] Finish TTSR cleanup/completion rather than treating the crate's existence as subsystem completion.

---

# 14. P2 — Remote forge/API paths

Provider-specific APIs should be resolver implementation detail rather than model burden.

- [ ] Freeze canonical forge/repository path grammar.
- [ ] Address repositories.
- [ ] Address commits.
- [ ] Address blobs/trees.
- [ ] Address pull/merge requests.
- [ ] Address issues.
- [ ] Address comments/reviews.
- [ ] Address diffs.
- [ ] Address checks/statuses where useful and available.
- [ ] Resolve GitHub/GitLab/other provider mechanics behind the path resolver.
- [ ] Ensure canonical resource identity survives provider pagination/order.
- [ ] Define explicit cache freshness/version/ETag semantics.
- [ ] Allow immutable commits/blobs to be frozen/cache-forever.
- [ ] Never serve mutable PR/issue stale cache as if it were current.
- [ ] Distinguish authentication failure from not-found.
- [ ] Distinguish partial permission from not-found.
- [ ] Integrate remote source locations/diffs with durable Artist anchors where meaningful.
- [ ] Make write/edit/run support capability-specific; do not pretend every forge resource is mutable.

---

# 15. P3 — Muse mechanical baseline: preserve what is actually done

These are implementation baseline facts, not semantic-completion claims.

- [x] Repair/build/test/lint/document Muse v6 without intentionally changing semantic behavior.
- [x] Re-run the Muse v6 workspace suite.
- [x] Repair stale canonicalization/tool-provenance fixtures to the current scoped-variable and RFC-6901 source-path contracts.
- [x] Add Artist-session-to-Muse bridge over captured canonical envelopes.
- [x] Preserve event order and unknown-event residuals.
- [x] Reject unsupported schema versions rather than guessing.
- [x] Persist root/delegated live model tool calls/results as canonical `model.turn`/`tool.result` events with exact arguments/outcome/duration/attachments.
- [x] Formalize captured events only against a caller-supplied Muse registry snapshot.
- [x] Persist ingestion failures at `memory://diagnostics/<id>` without failing the successful Artist operation.
- [x] Store successful formalized occurrence documents atomically/idempotently.
- [x] Trigger best-effort ingestion after captured root/delegated `read`, `edit`, and `write`.
- [x] Expose `memory://documents/<id>`.
- [x] Expose inert `memory://candidates/<id>`.
- [x] Project source-explicit supported statements into a neutral deterministic fact base.
- [x] Keep unsupported modalities/negation/literals/n-ary relations/questions/commands/inferred structure out of global deterministic facts rather than approximating them.
- [x] Expose revisioned `memory://facts/<occurrence-document-id>`.
- [x] Add exact formal rule matching and `memory://findings/<id>`.
- [x] Inject newly applicable formal findings with source links/per-run deduplication.
- [x] Add source-grounded Beta–Bernoulli evidence ledger with immutable observations and explicit prior pin.
- [x] Keep estimates separate from formal facts.
- [x] Add guarded `memory://labels/<id>` admission with source provenance/ontology-snapshot equality.
- [x] Keep labels distinct from deterministic facts.
- [x] Require explicit per-rule opt-in before rules consume labels.

---

# 16. P3 — Muse semantic closure: this is the remaining semantic core

Passing v6 tests is not proof that v6's semantic architecture is adequate.

The following seven closure vectors are mandatory audit categories.

## 16.1 Embedded typed content

- [ ] Produce minimal counterexamples.
- [ ] Describe current v6 representation.
- [ ] Classify the failure as impossible, ambiguous, noncanonical, or merely awkward.
- [ ] Define the required semantic equivalence class.
- [ ] Define learned/model-facing representation.
- [ ] Define deterministic compiler behavior.
- [ ] Define canonicalization invariant.
- [ ] Define formal lowering/certification behavior.
- [ ] Define ontology resolution.
- [ ] Define source-grounding behavior.
- [ ] Add negative tests excluding equivalent competing encodings.

## 16.2 Generalized quantifier restrictor/scope structure

- [ ] Repeat the full closure analysis above specifically for arbitrary restrictor proposition versus nuclear scope.
- [ ] Do not mistake the existing optional ontology domain for an arbitrary restrictor proposition.

## 16.3 Ambiguity-set canonicality

- [ ] Define canonical equivalence across represented reading graphs, not merely deterministic ordering of reading IDs.
- [ ] Reject semantically equivalent but structurally competing representations when the contract requires one canonical form.

## 16.4 Referent/occurrence identity

- [ ] Resolve repeated mentions.
- [ ] Resolve coreference.
- [ ] Resolve event re-mention.
- [ ] Resolve nominalization/event equivalence where semantically applicable.
- [ ] Resolve alternative decomposition identity.
- [ ] Preserve exact source grounding separately from semantic identity.

## 16.5 Symbolic term composition

- [ ] Determine the required recursive/compositional term algebra.
- [ ] Ensure first-class composed semantic terms are representable canonically.
- [ ] Define cycle/termination rules.

## 16.6 Plurality/distributivity/coordination

- [ ] Move beyond proposition-only conjunction/disjunction where the semantic target requires plural/group/term composition.
- [ ] Represent distributive versus collective readings canonically.
- [ ] Preserve/source-ground coordination roles.

## 16.7 Equivalent formal decompositions

- [ ] Identify semantically equivalent alternative graph/decomposition forms.
- [ ] Define one canonical semantic projection/equivalence handling.
- [ ] Ensure formal lowering/certification does not depend on arbitrary equivalent decomposition choice.

## 16.8 Re-derive, do not blindly recreate lost v7

Treat these as reconstruction hypotheses, not evidence:

- [ ] Evaluate whether one recursive open-construction algebra is the right solution.
- [ ] Evaluate first-class proposition/content/term constructions.
- [ ] Evaluate lexical-scope validation requirements.
- [ ] Evaluate positional/source-anchored construction roles.
- [ ] Evaluate compositional Event/Situation terms.
- [ ] Evaluate canonical semantic projection for certification.
- [ ] Evaluate recursive ontology resolution through open terms.
- [ ] Evaluate semantic firewall rules.
- [ ] Record any newly discovered closure vectors beyond the historical seven.

---

# 17. P3 — Muse labeling, ontology evolution, Bayesian inference, autofire

## 17.1 Actual model labeling

- [ ] Implement the actual labeling-provider/runtime stage.
- [ ] Feed it the appropriate occurrence/document source.
- [ ] Constrain model-produced claims with deterministic facts.
- [ ] Require source provenance.
- [ ] Require the narrowest justified ontology type.
- [ ] Use the narrowest valid generic type for unknown detail rather than discarding the object.
- [ ] Keep model labels semantically separate from deterministic facts.
- [ ] Calibrate confidence.

## 17.2 Ontology candidate evaluation and promotion

- [x] Persist inert source-grounded ontology candidates with parentage/rules/examples/tests.
- [ ] Evaluate candidate rules/concepts against the retained real corpus in a sandbox.
- [ ] Keep candidate evaluation from changing global results.
- [ ] Implement a separately authorized promotion workflow.
- [ ] Require proof/evidence appropriate to promotion.
- [ ] Promote into a sealed newest-revision package.
- [ ] Preserve append-only ontology revision semantics.
- [ ] Leave automatic proof-backed promotion as a future capability unless explicitly scheduled; do not silently auto-promote today.

## 17.3 Bayesian inference

- [x] Preserve the current source-grounded Beta–Bernoulli ledger.
- [x] Preserve explicit prior-model pin.
- [x] Preserve immutable observations and separately addressed estimates.
- [ ] Define the broader calibrated Bayesian model family needed beyond Beta–Bernoulli.
- [ ] Define policy for updating/replacing prior models without destroying provenance.
- [ ] Keep posterior/inferred beliefs distinguishable from explicit facts.
- [ ] Surface supporting and contrary evidence.

## 17.4 Findings/autofire

- [x] Formal rules currently require exact typed conditions rather than similarity/partial matching.
- [x] Existing findings carry supporting document identities.
- [x] Existing findings inject through the standard non-interrupting context primitive.
- [ ] Build complete high-utility finding-selection policy.
- [ ] Include compact supporting evidence.
- [ ] Include compact contrary evidence.
- [ ] Preserve source links.
- [ ] Keep formal applicability as the trigger.
- [ ] Retain the future option to inject paths for selected cases.
- [ ] Never let cosine similarity substitute for normative/formal applicability.

---

# 18. P3 — Artist ↔ Muse source/training contract

Do not final-freeze the Artist-specific adapter while its model-visible surface is still moving. Version intermediate adapters.

## 18.1 Adapter boundary/versioning

- [x] Existing bridge rejects unsupported schema versions rather than interpreting them.
- [ ] Maintain a narrow versioned `Artist event/result IR -> Muse occurrence/tool normalization` boundary.
- [ ] Do not couple Artist's basic transport types to Muse internal semantic types.
- [ ] Preserve old transcript interpretation through the adapter/schema that actually produced those sessions.
- [ ] Never reinterpret old sessions as though they had new paths/tools/codec semantics.
- [ ] Record adapter/normalization version in training/source corpora.

## 18.2 Exact model-visible presentation + canonical meaning

- [ ] Preserve the exact presentation the model actually saw.
- [ ] Preserve the exact canonical decoded meaning.
- [ ] Preserve the explicit lossless mapping between them.
- [ ] Preserve underlying path/anchor source grounding.
- [ ] Do not train behavioral source as though the model saw expanded text it never received.
- [ ] Do not ground semantics only to presentation offsets.

## 18.3 Ask truth conditions

- [ ] Record model-visible ask answer content.
- [ ] Record `Human | AutoResolve` provenance.
- [ ] Record whether a human-decision occurrence actually happened.
- [ ] Never collapse equivalent presentation into false human provenance.

## 18.4 Structured yield semantics

- [ ] Preserve yield as a model-produced report/assertion.
- [ ] Preserve agent/work-unit/schema/source evidence.
- [ ] Require independent evidence before promoting a yielded assertion to an independently observed fact.

## 18.5 Dynamic model-visible tool/profile context

- [ ] Record active tool/capability surface version/context for each relevant occurrence.
- [ ] Record active extension/tool schema state.
- [ ] Record resolved profile revision/digest.
- [ ] Record active tool policy.
- [ ] Record applicable yield schema.
- [ ] Record injected profile instructions.
- [ ] Use that historical context when interpreting the model action.

## 18.6 `memory://` versus presentation codec

- [ ] Ensure Muse storage/indexing consumes canonical/source semantic content.
- [ ] Retain model-visible compressed presentation only as presentation/provenance where behaviorally relevant.
- [ ] Never index `§...` or repetition syntax as if it were the underlying remembered fact.

## 18.7 Final-freeze sequencing

Do not call the Artist-specific training/normalization contract final until these stabilize:

- [ ] model-visible tool/capability surface;
- [ ] path/address grammar;
- [ ] line/dictionary/artifact TECA presentation;
- [ ] reversible codec rendering;
- [ ] ask semantics;
- [ ] structured yield semantics;
- [ ] binary/multimodal presentation;
- [ ] profile live-update semantics;
- [ ] universal result/event IR.

---

# 19. P2/P3 — MCP/web invariants

Do not blindly restore an obsolete old protocol field/header contract, but preserve these invariants.

- [ ] Reconcile current MCP implementation with stable logical session identity.
- [ ] A logical MCP/web session must not receive a fresh unrelated Artist identity every request.
- [ ] Reconnects should resolve the same logical identity when stable transport identity exists.
- [ ] Parallel logical sessions must not collapse into one identity.
- [ ] Identity-free discovery must not consume roster identities.
- [ ] If transport has no stable identity, use an explicit threaded identity handle rather than accidental hidden server state.
- [ ] Model-addressed mailbox messages have one model consumer.
- [ ] Canvas/internal invokes must not drain messages intended for the model.
- [ ] Tool visibility remains governed by resolved profile/policy rather than redundant contradictory gates.
- [ ] Explicitly document what current implementation supersedes from the older MCP design.

---

# 20. GC/retention

Automatic GC remains intentionally open.

Until policy is designed:

- [x] Dictionary entries are not GC'd.
- [x] Muse-derived records survive ordinary runtime-resource deletion where the current implementation already preserves them.
- [ ] Automatic GC may delete only explicitly disposable caches and unreferenced temporary artifacts.
- [ ] Never automatically delete reachable resources.
- [ ] Never automatically delete retained names while still semantically referenced.
- [ ] Never automatically delete dictionary entries.
- [ ] Never automatically delete Muse records.
- [ ] Define when settled operational resources disappear from active enumeration.
- [ ] Define when an operational resource becomes history-only.
- [ ] Define manual-delete effects on parent/child/predecessor/successor relationships.
- [ ] Define artifact/yield retention.
- [ ] Define broken pointers after GC/deletion.

---

# 21. Execution order from here

The agent should not interpret this as a strict “finish every line in section N before touching N+1” gate. It is dependency-oriented priority.

## P0 — repair wrong ABI before more code depends on it

- [x] Compact TECA lexicon rendering for line anchors.
- [x] TECA exact-substring dictionary identity.
- [ ] TECA artifact-path identity.
- [ ] Automatic artifact capture/routing from producing occurrences/yields.

## P1 — finish the common harness substrate

- [ ] Complete canonical typed paths for remaining runtime/resource kinds.
- [ ] Remove split legacy IDs/surfaces from the target ABI.
- [ ] Finish universal result/error behavior.
- [ ] Finish exact token-budgeted read/poll and continuations.
- [ ] Finish relationships + choose/implement their backing store.
- [ ] Finish asks/todo semantics under the path-first surface.
- [ ] Finish process-resource path identity.

## P2 — complete model-facing context/runtime breadth

- [ ] Finish artifact/binary routing.
- [ ] Finish dictionary/repetition reversible codec.
- [ ] Finish exact structural condensation + SnapCompact.
- [ ] Finish terminal hardening.
- [ ] Execute Brush/uutils migration evaluation.
- [ ] Finish remaining computer/canvas/runtime migration.
- [ ] Implement Python eval with Artist-tool callbacks.
- [ ] Implement DAP debugger sessions.
- [ ] Implement LSP multiplexer/integration.
- [ ] Finish AST/code graph + structural batching.
- [ ] Finish TTSR cleanup/path integration.
- [ ] Implement forge/API virtual paths.

## P3 — finish Muse as a semantic system

- [ ] Run the seven-vector semantic closure redesign.
- [ ] Re-derive useful lost-v7 architectural ideas rather than blindly recreating them.
- [ ] Implement actual model labeling runtime.
- [ ] Implement candidate evaluation/promotion.
- [ ] Extend Bayesian inference as required.
- [ ] Finish finding-selection/autofire semantics.
- [ ] Finish exact presentation/canonical/source mappings.
- [ ] Finish versioned Artist↔Muse behavioral source contract.
- [ ] Final-freeze training/adapter semantics only after relevant Artist surfaces stabilize.

---

# 22. Already completed implementation baseline — do not redo unless corrective work requires touching it

This preserves the useful concrete progress recorded by the first production checklist.

- [x] Historical planning ledger replaced by a production-oriented plan/checklist.
- [x] TECA dependency integrated into line addressing.
- [x] Revisioned text read/edit/write protection.
- [x] Snapshot-relative atomic text edit batches.
- [x] Durable stop/delete mechanics.
- [x] TECA AGPL distribution metadata corrected in the workspace/hashline-tools and third-party notice retained.
- [x] Existing virtual root parsing and durable session registry integration.
- [x] Real/virtual FFF find/grep behavior and frecency policy.
- [x] Virtual host-path safety boundary for generic edit/write.
- [x] Existing Artist file-tool integration tests.
- [x] Existing `artist-agent` library suite recorded green at the earlier checkpoint.
- [x] Semantic terminal transcript + emulator-screen polling architecture.
- [x] Revisioned agent todo projection path.
- [x] `skill://` file/activation model.
- [x] `profile://` roster/snapshots.
- [x] `tools://` manifest/module projections.
- [x] Artist-session→Muse captured-event bridge.
- [x] Captured model/tool result persistence.
- [x] Muse snapshot-pinned formalization.
- [x] Muse diagnostics.
- [x] Muse occurrence-document retention.
- [x] Best-effort read/edit/write ingestion triggers.
- [x] Muse document projections.
- [x] Inert ontology candidate persistence.
- [x] Deterministic source-explicit fact projection.
- [x] Muse fact projections.
- [x] Exact formal-rule findings.
- [x] Finding injection plumbing.
- [x] Beta–Bernoulli ledger.
- [x] Guarded model-label admission storage.
- [x] Artifact metadata projection.
- [x] Immediate-child real-directory reads.
- [x] Permanent dictionary storage infrastructure.
- [x] Atomic WASM extension candidate reload.
- [x] Path-first `run` baseline.
- [x] Canonical `agent` creator baseline.
- [x] Profile tool/skill policy matching.
- [x] Passive discovery recency semantics.
- [x] Restart-safe stop grace behavior.
- [x] Canvas-source `run` routing.
- [x] Native process snapshot baseline.
- [x] Yield validation/projections.
- [x] Agent todo/yield relationship directory baseline.
- [x] Revision-bound pagination across text-backed virtual resources.
- [x] Canonical lifecycle path acceptance for existing runtime roots.
- [x] Computer rung-0 capability catalog baseline.
- [x] Live profile update baseline.
- [x] `tools://` local extension dispatch.
- [x] Muse v6 mechanical build/test/lint/document baseline.

---

# 23. Explicit non-goals / do-not-resurrect list

- [x] Do not reimplement TECA mathematics in Artist.
- [x] Do not restore mnemonic word allocators.
- [x] Do not restore old non-path session IDs as the target ABI.
- [x] Do not restore `list()` as the target enumeration primitive.
- [x] Do not create an MCP-specific ask outbox when durable path/session semantics already solve the problem.
- [x] Do not create hidden skill activation state.
- [x] Do not make lossy summarization the final compaction model.
- [x] Do not preserve Muse-v6 architecture solely because existing tests pass.
- [x] Do not claim lost semantic-v7 code existed merely because later notes described it.
- [x] Do not add giant ceremonial subsystem gates that prevent replacing inadequate current systems.
- [x] Do not reintroduce the removed generic capability-discovery section from the prior corrective as an independent product workstream.
- [x] Do not reintroduce the removed generic typed-mutation-algebra section from the prior corrective as an independent architectural requirement.
- [x] Do not revert eval to the superseded Rust/evcxr plan; the current decision is a Python REPL with callbacks into Artist tools.
- [x] Do not reopen whether artifacts use TECA; artifact paths are TECA by design.

---

# 24. Definition of project completion

Artist is not complete when the current checked implementation slices merely coexist.

Artist is complete for this plan when:

- [ ] the target model-facing ABI is consistently path-first without split legacy identity semantics;
- [ ] line anchors are genuinely compact TECA addresses, not generic hex wire strings;
- [ ] dictionary identities are exact-substring TECA prefixes;
- [ ] artifact paths are TECA-derived from occurrence envelope + exact bytes and artifact routing is harness-owned;
- [ ] revisions/continuations remain stale-safe under concurrent mutation;
- [ ] read/poll use exact active-model token accounting and preserve whole logical lines;
- [ ] terminal, artifacts, dictionary codec, repetition, and compaction preserve exact canonical meaning;
- [ ] relationships are first-class and backed by a deliberately selected persistence/query model;
- [ ] Python eval, DAP, LSP, AST/code graph, TTSR, Brush/uutils work, and forge paths are actually present;
- [ ] computer/canvas/tools/runtime creation use the unified path/result lifecycle;
- [ ] Muse resolves the seven known semantic closure vectors and any new ones discovered;
- [ ] Muse actually labels, evaluates ontology candidates, reasons probabilistically with calibrated provenance, and autofires only on formal applicability;
- [ ] Artist↔Muse normalization preserves exact canonical source plus the exact model-visible tool/profile/codec context that governed each occurrence;
- [ ] explicit facts, human decisions, auto-resolved answers, model reports/yields, rule findings, labels, and Bayesian inferences remain semantically distinguishable;
- [ ] no product requirement above is silently dropped merely because an earlier implementation did something narrower.
