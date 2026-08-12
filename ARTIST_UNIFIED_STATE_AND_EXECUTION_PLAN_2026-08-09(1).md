# Artist + Muse Authoritative Production Plan

## Objective

Build Artist as a path-first execution harness and Muse as the semantic compiler/reasoner that turns Artist interactions and explicitly interacted-with project files into typed, logically usable memory.

## Locked Artist architecture

- Every addressable thing is a real path or a typed virtual path. Core roots are `agent://`, `bash://`, `ask://`, `canvas://`, `computer://`, `artifact://`, `dict://`, `memory://`, `profile://`, `skill://`, and `tools://`.
- Persistent agent names come from the global Artist roster and are unique while retained. Other runtime resources use content-derived slugs with serial collision suffixes. Computer names derive from target identity plus serial.
- Relationships are read-only filesystem projections/shortcuts under the canonical resource path. `agent://goethe/todo` is the only todo-tree file; do not add `todo://` aliases.
- Directory and scheme-root `read` returns a compact immediate-child listing. `find` performs ranked traversal.
- `read` is stateless. Continuations carry strict TECA-addressed resource revisions; any change makes them stale and requires a fresh read.
- Text resources use TECA anchors on logical lines. Identity input is canonical line bytes plus label-free CST kind/role ancestry, followed by bidirectional equivalent-sibling rank: left half `L1..L⌊n/2⌋`, right half `R⌈n/2⌉..R1`; an odd center belongs to the right side.
- `edit` resolves all targets against one pre-edit revision, rejects stale or overlapping edits, commits atomically, and returns rebased anchors. `write` creates or fully replaces content and requires the current TECA revision when replacing existing content. External edits cause stale failure, never an automatic merge or overwrite.
- Tool surface: `read`, `find`, `grep`, `edit`, `write`, `poll`, `send`, `run`, `stop`, `delete`, plus typed runtime creation for `agent`, `bash`, `ask`, and `computer`.
- `run(path)` executes scripts/executables, canvas sources, and active `tools://` resources. Scripts yield persistent bash sessions; native executables yield one-shot process resources. `agent({profile, brief})` claims a roster name and starts immediately. `bash({command?})` creates a persistent shell with an optional first command.
- Live definition edits apply to the next run except canvas source, which hot-reloads when supported and otherwise reports restart-required.
- `stop` requests graceful cancellation, then force-terminates after a fixed grace period. `delete` permanently removes the resource, editable transcript, child yields, and name reservation; Muse-derived records remain.
- Every failure returns a typed error, explanation, affected path/revision, and a usable recovery action or path.

## Search, terminal, codec, and artifacts

- `find` is frecency-ranked. Explicit reads and human opens update frecency; discovery does not. Unfiltered listings are most-recent-first.
- `grep.mode` defaults to `auto` and supports `literal`, `regex`, and `fuzzy`. Auto returns literal matches first and falls back to FFF fuzzy search when literal search has no result. File chunks are frecency-ranked; lines stay in source order.
- Read/poll use exact active-model token accounting, never split a logical line, and permit caller-adjusted limits with no hard ceiling.
- `poll` waits for resource activity, regex match, settlement, or timeout; omitted timeout is indefinite and zero is immediate. Multi-target polling waits for all targets, with settled targets immediately satisfied.
- `poll(path, match)` atomically evaluates current plus future canonical output. It returns a continuation selector strictly after the terminating anchor for later-only matching.
- `read(bash://x)` returns the append-only semantic terminal transcript. `poll(bash://x)` observes the current emulator screen. Raw control bytes never enter model text. Track `wezterm-term` upstream `main`.
- Artifacts use TECA paths derived from canonical occurrence envelope plus exact bytes, so identical bytes from distinct occurrences stay distinct. `read(artifact://…)` returns metadata and visual attachment when supported; raw binary is never model text.
- The global `dict://` dictionary is permanent and never GC’d. References use `§<opaque-prefix>` and runs use `×<count>{value}`. New references are defined before use and redefined after compaction, resume, or new conversation.
- Fresh output remains text. Conversation compaction first applies exact structural condensation, then SnapCompact’s lossless text-to-bitmap packing for vision-capable models; non-vision models receive exact condensed text. No summaries or semantic loss.

## Sessions, profiles, computer, and self-modification

- Session transcripts are ordinary editable files. Muse compiles captured original events, not later transcript edits.
- A completed agent yield is JSON validated against the current profile JSON Schema and stored at `agent://<artist>/yields/<n>`. `send(agent, brief)` begins the next work unit.
- Profile edits apply through the unified non-interrupting context-update primitive. Tool policy uses allow/deny rules; absent allow means allow all; most-specific match wins, then deny wins ties.
- Skills are ordinary `skill://` files. Reading a skill is the only activation; do not create hidden persistent skill state.
- Computer rung 0 is direct path interaction. Surfaces are `computer://<slug>` and default reads return unified semantic snapshots. `send(surface, intent)` lets Artist select the best permitted capability from addressable catalog files under `computer://use/…`. Failures return ranked runnable capability paths and reasons.
- `tools://` ships after Muse core. WASM revisions hot-swap once compile and ABI/schema loading succeed. Broken revisions remain active for iterative repair. `run(tools://path, args)` invokes them locally; provider schemas refresh on restart.

## Muse contract

- Muse ingests captured Artist events and files on `read`, `edit`, and `write`, never proactively or from grep/find alone. Ingestion failures do not fail the originating Artist operation; they appear at `memory://diagnostics/<id>`.
- Muse is a broad model-produced semantic labeling system backed by deterministic extraction and validation. Deterministic facts constrain model claims; they do not excuse weak model labeling.
- Every admitted object gets the narrowest justified ontology type. Unknown detail uses the narrowest valid generic type rather than being discarded.
- Muse answers with best inference, calibrated confidence, and compact supporting/contrary evidence. Explicit facts and inferred hypotheses are always distinguishable.
- Normative reasoning derives only from rules, profiles, and project documents. A model action that satisfies a prohibited typed condition fires the applicable memory/rule; similarity alone never does.
- Ontology is append-only and interpreted at its newest revision. New concepts/relations begin as typed candidates with parentage, declared rules, source-grounded examples, and tests. Candidate rules may evaluate against the real corpus in a sandbox but do not affect global results until promotion.
- Automatic ontology promotion is a future proof-backed capability. Preserve an explicit prior-model pinflag for the Bayesian engine; update Bayesian beliefs online. Hard logical contradictions are rejected by construction and emitted as diagnostics.
- Start formal semantics with Artist tool/runtime behavior. Rust remains the first language formalization target; language formalization is a north-star extension and must never be made structurally impossible.
- Autofire is driven by formal applicability from current labels, not turn cadence or cosine retrieval. Inject compact, high-utility, source-linked findings through the standard non-interrupting context primitive. Retain a future option to inject paths for selected cases.

## Execution order

1. Implement the resolver/path/result substrate, exact TECA anchor migration, revision semantics, and VFS projections.
2. Migrate `read`, `find`, `grep`, `edit`, `write`, `poll`, `send`, `run`, `stop`, `delete`, and session lifecycle.
3. Implement terminal, artifact, dictionary, codec, structural condensation, and SnapCompact integration.
4. Build hybrid runtime creation, profiles, asks, canvases, computer rung 0, and capability catalog.
5. Repair Muse v6 until it parses, compiles, tests, lints, documents, and builds. Do not alter semantics during mechanical repair.
6. Review each Muse subsystem against the required typed semantic funnel. Keep only code that serves the target; surface every replacement recommendation as “current code does X; required design needs Y.”
7. Build Muse ingestion, ontology candidates, diagnostics, Bayesian inference, autofire, and the Artist adapter around the finalized Artist event/result contracts.
8. Add `tools://` WASM editing, compile/load hot reload, and local `run` dispatch.
9. Correct TECA license metadata before distribution.

## Production evidence

- TECA anchors remain deterministic across agents/processes; no local mnemonic allocator, hash-to-word scheme, or actor-local address state remains.
- Stale revisions never retarget mutations; anchor rebasing, duplicate ranking, batch edits, and continuations are deterministic.
- Codec and structural condensation are exact round trips.
- Terminal, artifact, session, run, stop, and error behavior work under live concurrency and restart.
- Muse architecture review demonstrates normative compliance, project dependency difficulty, considered/rejected alternatives, explicit identity resolution, personal-fact inference, unknown/conflicting evidence, and source-linked confidence.
- Every semantic answer separates explicit evidence from Bayesian inference and never admits a hard ontology/rule contradiction.

## Post-core GC policy

Automatic GC remains intentionally open. Until its policy is designed, GC may delete only explicitly disposable caches and unreferenced temporary artifacts; it never deletes reachable resources, names, dictionary entries, or Muse records.

## Implementation tracking

This checklist is a durable execution record. A checked item has implementation and direct verification evidence; it does not imply that adjacent or downstream requirements are complete.

- [x] Replace the historical plan/ledger with this authoritative production plan.
- [x] Replace the local v1 line-address allocator and token vocabulary with the published `teca` crate.
- [x] Preserve deterministic, stateless shortest-prefix anchor rendering over TECA structural atoms.
- [x] Return a revision from text `read`; require it for `edit` and existing-file `write`; reject stale revisions while holding the cross-process file lock.
- [x] Keep batch edits snapshot-relative, overlap-checked, and atomically committed; return post-edit anchors.
- [x] Add durable `stop` and `delete` session verbs. Delete rejects live sessions, removes stopped records and queued input, and releases a retained name lease.
- [x] Record TECA's AGPL-3.0-only distribution obligation in `THIRD_PARTY_LICENSES.md`.
- [x] Parse every locked virtual-path scheme without allowing traversal/selectors, and resolve `agent://`, `bash://`, `ask://`, `canvas://`, and `computer://` roots plus session snapshots through the durable registry.
- [x] Make `grep.mode=auto` literal-first with an FFF fuzzy fallback; preserve explicit `literal`, `regex`, and `fuzzy` modes.
- [x] Apply the same FFF neo-frizbee fuzzy engine to typed virtual-session snapshot lines (without manufacturing host files or a shadow index), while preserving canonical source-order lines after a literal-free auto fallback.
- [x] Route explicit virtual `find` scopes through the durable session registry, preserving real-path FFF discovery while filtering virtual paths fuzzily and ranking matching resources by explicit-open frecency (never discovery or lexical score).
- [x] Route explicit virtual `grep` scopes through canonical session snapshots, with literal-first auto behavior and no host-path interpretation.
- [x] Reject virtual paths at generic file `edit`/`write` boundaries until their owning runtime supplies a canonical mutable projection; they cannot accidentally create host files named like resources.
- [x] Verify the Artist file-tool integration suite, including semantic terminal snapshots, TECA revisions, and virtual-path boundary regressions.
- [x] Verify the full `artist-agent` library suite (226 tests), including guarded writes, the unified tool construction surface, canonical lifecycle paths, virtual FFF grep fallback, virtual-find frecency ordering, live profile-policy enforcement, and emulator-backed bash polling.
- [x] Retain and render the full live `bash://` semantic terminal transcript with a strict content revision and TECA-anchored logical lines.
- [x] Feed every raw PTY output chunk into `wezterm-term` pinned to upstream `main`; retain the append-only sanitized transcript for `read(bash://…)`, while `poll(bash://…)` evaluates the emulator’s visible screen so cursor movement, erases, and alternate-screen behavior are not flattened into text.
- [x] Add regex-aware `poll` over current and future canonical output; terminal matches return a revision-bound continuation strictly after the matched TECA anchor.
- [x] Preserve full live terminal output and strip terminal control traffic before it becomes model-visible transcript text.
- [x] Make every declared virtual scheme root readable even when its typed resolver has not yet been installed; such roots never fall through to host-path handling.
- [x] Honor explicit real-text `read` limits without fixed byte or line ceilings, while preserving whole logical lines and the compact default window.
- [x] Expose each retained agent's durable todo tree only at `agent://<artist>/todo`, with a revisioned TECA rendering and no `todo://` alias.
- [x] Make `skill://` a profile-governed ordinary-file namespace: the root lists permitted skills and reading `skill://<name>` is the sole activation, with no hidden activation state.
- [x] Make `profile://` list the resolved profile roster and read revisioned profile snapshots consistently in main, delegated, and MCP tool environments.
- [x] Expose discovered WASM extension manifests and binary metadata under `tools://<id>/manifest` and `tools://<id>/module`, without ever routing raw binary bytes into model text.
- [x] Add an Artist-session-to-Muse bridge over captured canonical envelopes; it preserves order and unknown-event residuals and rejects unsupported schema versions rather than interpreting them.
- [x] Persist root and delegated live model tool calls/results as matching canonical `model.turn`/`tool.result` session events, including exact arguments, outcome, duration, and attachment references, so read/edit/write interactions supply the Muse adapter's formal input.
- [x] Allow the Artist-session bridge to formalize captured events only against a caller-supplied Muse registry snapshot, preserving explicit ontology provenance.
- [x] Add durable, atomically written Muse failure records at `memory://diagnostics/<id>` so ingestion can report a failure without changing the originating Artist operation's result.
- [x] Provide a best-effort ingestion adapter that returns typed documents on success or a persisted diagnostic on failure, without propagating an ingestion failure to its Artist caller.
- [x] Retain successful formalized occurrence documents atomically under `.artist/state/muse/documents`, idempotently by semantic document id and without overwriting an id collision.
- [x] Trigger best-effort Muse ingestion after captured root and delegated `read`, `edit`, and `write` results only; it flushes canonical events, pins an explicit snapshot, and leaves Artist's successful tool result unchanged on every ingest/store failure.
- [x] Expose retained Muse occurrence documents as revisioned `memory://documents/<id>` projections, alongside `memory://diagnostics`, without treating either as host paths.
- [x] Add inert, source-grounded ontology candidates under `memory://candidates/<id>`: content-derived identity, parentage, declared rules, examples, tests, and retained occurrence-document validation are required; candidates cannot mutate the registry or global results.
- [x] Project only source-explicit supported occurrence statements into Muse's neutral formal fact base; unsupported modalities, negation, literals, n-ary relations, questions, commands, and inferred/model structure never become global facts by approximation.
- [x] Retain those deterministic fact bases atomically as revisioned `memory://facts/<occurrence-document-id>` projections; differing replays cannot overwrite the same source identity.
- [x] Add source-grounded formal rules and `memory://findings/<id>` projections: a finding fires only when every exact typed condition is in the retained fact corpus, carries its supporting document identities, and never uses similarity or partial matching.
- [x] Inject newly applicable formal findings through Artist's standard non-interrupting request-patch context primitive, with compact `memory://findings` and `memory://documents` links and per-run deduplication.
- [x] Add an online, source-grounded Beta–Bernoulli evidence ledger with immutable observations, an explicit prior-model pin, exact posterior predictive PPM, and separately addressable prior/observation/estimate records; estimates are never admitted as formal facts.
- [x] Add guarded `memory://labels/<id>` admission for model-produced occurrence labels: model derivation, source-document provenance, structural validity, and exact ontology-snapshot equality are required; labels remain distinct from deterministic facts and rules consume them only through explicit per-rule opt-in.
- [x] Expose retained oversized-result artifacts at `artifact://<id>` as revisioned metadata only; payload bytes remain behind the pagination continuation and never enter the artifact read projection.
- [x] Make real-directory `read` return an immediate-child listing with a deterministic revision, never a recursive filesystem dump.
- [x] Add the permanent global `dict://` store with content-derived opaque `§` references, collision suffixes, atomic persistence, and no GC/delete path.
- [x] Add an atomic extension-manager candidate reload: compile/instantiate replacements before swap, retain the last working instance/layout when activation fails, and restart only the replacement set's status/event tasks.
- [x] Add the path-first `run` verb for real executable and explicitly interpreted script paths; it quotes arguments exactly and creates the canonical durable `bash://` lifecycle resource.
- [x] Add canonical `agent({profile, brief})` creation as a compatibility-preserving front door to the retained roster lifecycle; it claims a durable Artist name, begins work immediately, and returns that canonical id while legacy `subagent({profile, prompt})` remains available during migration.
- [x] Apply profile tool and skill allow/deny policy by strongest matching glob specificity, with deny winning only exact ties; preserve absent-allow as allow-all and a present unmatched allow-list as deny-all.
- [x] Make unfiltered real-directory `read` projections immediate-child, most-recent-first listings with deterministic lexical tie-breaking and a content-derived revision.
- [x] Keep virtual-session discovery passive: `find`, `grep`, and root listings no longer refresh durable recency, while an explicit resource `read` updates it; unfiltered session roots rank by that interaction recency.
- [x] Make `stop` a durable graceful-cancellation request with a fixed two-second, restart-safe grace deadline; it rejects new input while stopping and force-cancels only after that deadline, while legacy `abort` remains immediate.
- [x] Derive paged `artifact://` identities from the canonical tool-result occurrence envelope and exact payload bytes, with an atomically allocated serial for repeated identical occurrences; preserve reads of legacy UUID artifacts.
- [x] Route explicit JavaScript/TypeScript canvas-source paths through `run` to their owning live canvas session; ordinary scripts and non-source canvas files retain their normal path behavior.
- [x] Make direct native executable `run` calls allocate one-shot `process:<slug>` resources with explicit process snapshots, while interpreted scripts retain the canonical durable `bash:<slug>` lifecycle.
- [x] Retain each completed delegated work unit as structured JSON validated against the resolved profile's optional inherited `yieldSchema`; schema failures produce a failed run rather than an invalid completed yield. Expose valid yields only at revisioned, read-only `agent://<artist>/yields/<n>` projections, and surface the resolved schema through `profile://`.
- [x] Make every retained agent's canonical path a read-only relationship directory that lists `agent://<artist>/todo` and, after a completed work unit, `agent://<artist>/yields`; no `todo://` alias is introduced.
- [x] Bind real-file and every text-backed virtual-resource pagination continuation (including bash, skills, profiles, dictionary values, todos, yields, computer capabilities, artifacts, Muse memory, and tools metadata) to the exact revision returned by the preceding page; a missing or changed revision produces a `stale_revision` recovery error instead of retargeting the offset.
- [x] Normalize full-file write replacement races into typed `stale_revision` recovery errors: a missing revision is rejected, an external edit cannot be overwritten, and only a fresh exact revision commits the replacement.
- [x] Normalize stale batch-edit/anchor failures into the same typed `stale_revision` recovery error, preserving atomic rejection and requiring a fresh anchored read before retry.
- [x] Accept direct canonical `agent://`, `bash://`, `ask://`, `canvas://`, and `computer://` resource paths for lifecycle `poll`, `send`, `stop`, and `delete` targets while retaining legacy durable ids; reject relationship-child paths rather than treating them as runtime resources.
- [x] Verify stopped-agent deletion removes the retained resource record and its embedded yield snapshot while preserving unrelated Muse-derived records; the independently keyed top-level conversation transcript is not guessed or deleted.
- [x] Publish the rung-0 computer capability catalog as revisioned, stale-safe paged read-only `computer://use/<action>` resources, and route `send(computer:<slug>, {intent, args?})` only through those declared capabilities; unsupported intents return the runnable catalog paths.
- [x] Re-resolve a root or delegated agent's active profile at completion boundaries and inject a changed resolved profile's replacement instructions through the standard non-interrupting `RequestPatch` context path; immediately skip any already-registered tool newly denied by a valid live policy edit, and retain the last valid instructions/policy after an invalid edit.
- [x] Correct TECA distribution metadata: the workspace and `hashline-tools` now publish the effective `AGPL-3.0-only` license rather than the incompatible MIT/Apache expression, alongside the retained third-party notice.
- [x] Dispatch active extension callbacks locally through canonical `tools://<extension>/tool/<name>` run paths, with read-only tool metadata/listing projections and no raw WASM text exposure.
- [ ] Implement typed real/virtual path resolver roots and read-only relationship projections.
- [ ] Migrate all declared runtime resources and tool verbs onto the unified path/result substrate.
- [ ] Implement exact token-budgeted read/poll continuations, terminal emulator behavior, artifact routing, dictionary, and lossless codec/compaction.
- [ ] Complete runtime creation, profile context updates, asks, canvases, computer rung 0, and capability catalog migration.
- [x] Repair, build, test, lint, and document Muse v6 without changing semantic behavior.
- [x] Re-run Muse v6's complete workspace suite and repair its stale canonicalization/tool-provenance fixtures to the current scoped-variable and RFC-6901 source-path contracts; all Muse workspace tests pass.
- [ ] Complete the Muse semantic funnel, Artist event adapter, memory diagnostics, candidates, Bayesian inference, and formal-applicability autofire.
- [x] Implement `tools://` WASM compile/load hot reload and local dispatch: a stable shared manager compiles and instantiates a candidate set before atomically swapping it, retains the last working revision on activation failure, restarts only replacement tasks, exposes host-triggered reload through `EmbeddedRuntime::reload_extensions`, and supplies live `tools://` read/run resolution without stale per-turn declarations.

## Current Muse runtime replacement recommendations

- Current code admits validated, source-provenanced model labels but does not yet run a labeling provider; required design needs that provider stage, constrained by deterministic occurrence/document facts.
- Current code does persist source-grounded ontology candidates but has no candidate-evaluation or promotion workflow; required design needs sandbox corpus evaluation and a separately authorized, proof-backed promotion path into a sealed newest-revision package.
- Current code has a durable online Beta–Bernoulli ledger with a pinned prior and separately addressed estimates; required design still needs the broader calibrated Bayesian model family and policy for updating prior models.
- Current context injection now applies formal retained findings with source links and per-run deduplication; required design still needs a complete policy for selecting among all high-utility findings and the future path-injection option.
