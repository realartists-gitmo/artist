Implementation contract: [`MAJOR_FIX_PLAN.md`](MAJOR_FIX_PLAN.md).

This fix list is intended to DECREASE the amount of work we're doing and code we're rolling by using more dependencies and using our current dependencies better.

A. Search indexing: SHOULD outsource substantially more to fff-search

This is the clearest case.

SearchEngine currently constructs:

FilePicker::new(...)
picker.collect_files()

and wraps the picker in its own Mutex. Every Artist resource-generation change causes refresh() to throw that picker away and build another one. ResourceFabric maintains a Tokio task specifically to watch router generations and invoke those complete refreshes.

But FFF's intended long-running application API is SharedFilePicker plus FilePicker::new_with_shared_state. Its documentation explicitly describes that route as spawning background indexing and filesystem watching; the simple FilePicker::new route used by Artist does not spawn that machinery.

So the current:

Mutex<FilePicker>
manual synchronous initial collection
AtomicU64 generation
refresh()
reconstruct-the-entire-index behavior

is reproducing lifecycle functionality that FFF already owns.

The Artist router-generation signal still has value because virtual FUSE topology can change outside normal filesystem events. But it should trigger/resynchronize FFF's existing index rather than destroy/recreate the picker.

There is a second underuse here. find() invokes FFF glob search with:

PaginationArgs {
    offset: 0,
    limit: usize::MAX,
}

then manually filters, sorts, deduplicates, skips and takes. Yet FFF's glob() is explicitly a paginated operation.

grep() similarly sets page_limit: usize::MAX, materializes all matching results, then paginates itself. FFF already exposes both file_offset/next_file_offset pagination and page_limit.

Recommended direction:

SharedFilePicker
background indexing/watching
push root/glob constraints into FFF where possible
use FFF pagination/cursors directly
retain Artist's generation signal only for virtual topology invalidation/rescan

This is both outsourcing and materially better use of an existing dependency.

B. History shaping/compaction: SHOULD let Rig own the request-history mechanics

RigModel currently manually:

converts Artist history to Rig messages;
invokes MemoryPolicy::apply_with_demoted;
invokes TemplateCompactor;
splices the summary into history;
emits ContextCompacted;
passes the finished history into .history(history).

This is exactly the area rig-memory has expanded to cover. It provides PolicyMemory, DemotingPolicyMemory, CompactingMemory, SlidingWindowMemory, TokenWindowMemory, and the Compactor abstraction.

More importantly, Rig 0.42's RequestPatch::history exists specifically for per-model-call history replacement such as context-window compaction/summarization while leaving the persisted transcript untouched.

That is a particularly good match for Artist because Artist should retain ownership of its canonical event-sourced transcript. I would not hand canonical session persistence over to Rig's ConversationMemory.

Instead:

Artist remains canonical storage.
Project canonical Artist history into Rig history.
A Rig hook applies the rig-memory policy/compactor.
The hook supplies the resulting history through RequestPatch::history.
Artist records the resulting compaction through its existing ContextCompacted event path.

That also applies shaping at each completion call in Rig's multi-turn tool loop, rather than only performing one pre-run transformation.

So: keep Artist's persistence; outsource the model-request history manipulation.

C. Wasmtime host: SHOULD use its async embedding APIs

artist-plugin creates its own multithreaded Tokio Runtime, stores it in every host state, and implements a helper effectively doing:

if already_inside_tokio {
    block_in_place(|| runtime.block_on(future))
} else {
    runtime.block_on(future)
}

This exists because the Wasmtime host layer is synchronous while ToolRegistry and ResourceRouter are asynchronous.

Wasmtime explicitly has an async embedding mode for integrating async Rust host functions without blocking the host, and Wasmtime WASI has the corresponding add_to_linker_async path. Wasmtime 38-era examples use Config::async_support(true) with the async WASI linker.

The current Cargo configuration does not enable Wasmtime's async feature.

I would change this boundary:

enable Wasmtime async support;
use async component instantiation/calls;
use async WASI linker integration;
make the relevant PluginHost operations async;
directly await ToolRegistry/ResourceRouter;
remove the private Tokio runtime and block_on bridge.

This is a meaningful existing-API underutilization, not merely stylistic cleanup.

D. Filesystem operations exist twice: SHOULD consolidate onto artist-resource

artist-resource::FilesystemProvider implements read, children, write and move using the resource abstraction.

Separately, artist-plugin::HostState implements the WIT native-filesystem host imports by directly doing std::fs::read_to_string, read_dir, write, directory creation, etc.

The default WASM plugin then exposes filesystem resources by forwarding its calls to that second native-filesystem implementation.

That gives Artist two implementations of essentially the same filesystem semantics.

I would make the native-filesystem WIT host an adapter over the existing artist-resource filesystem/resource implementation rather than maintaining independent filesystem logic. The async Wasmtime change above makes this considerably cleaner.

No new external crate is required. This is a case where an existing internal dependency is underused.

E. Filesystem confinement: SHOULD use a capability filesystem if root is a security boundary

FilesystemProvider::path() checks:

if path.starts_with(&self.root)

before normal filesystem operations.

That is lexical confinement; it is not symlink-safe confinement.

If FilesystemProvider::root is intended merely as organizational scoping, leave it alone.

If it is intended to prevent access outside the root, this is functionality that should not be implemented with pathname checks. cap-std::fs::Dir exists specifically to perform filesystem operations relative to an opened directory capability and prevents traversal outside that capability, including relevant symlink cases.

So this is conditional:

Security boundary → use cap-std/capability I/O.
Not a security boundary → do not add the dependency.

F. Built-in tool schemas: SHOULD use typed serde + schemars

At audit time, the host-native built-in tool layer manually defined JSON Schemas with serde_json::json!, while its runtime implementation separately manually extracted the same arguments through string_arg, u64_arg, etc.

That creates two representations of each built-in tool contract.

You could define typed request structures and derive serialization/schema information with serde + schemars.

Resolved direction: the six built-ins now live in six narrow `artist.tool.*`
WASM components. Each owns one typed serde/schemars request contract and calls
generic host resource/search imports. Native host code no longer defines or
registers model-facing tools.

2. Existing dependencies being underutilized
fff-search: YES — substantially

This is the largest case.

Currently unused or defeated by Artist's wrapper:

SharedFilePicker
intended background scanning/watching lifecycle
native glob pagination
grep file_offset/next_file_offset
bounded page_limit
native constraint/filtering machinery that could push down some Artist post-filtering

FFF explicitly describes itself as intended for long-running editors and AI agents, with real-time indexing, grep, constraints and shared state.

Assessment: significantly underutilized.

rig-memory: YES — substantially

Artist uses the individual policies and TemplateCompactor, but manually orchestrates exactly the higher-level workflow the crate provides abstractions for.

The important available surface includes PolicyMemory, DemotingPolicyMemory, CompactingMemory, and the policy/compactor composition model.

I would not blindly adopt Rig's persistence backend, but the history-shaping mechanics should move closer to Rig's intended abstraction.

Assessment: substantially underutilized.

rig-agent: YES — moderately

Artist is already using the correct AgentRunner, DynamicTool, hooks, and streaming machinery. That part is good.

The missed API is primarily RequestPatch::history. Rig documents it specifically as a per-turn context compaction/summarization mechanism that does not mutate persisted history. Artist currently uses only RequestPatch::extra_context for steering.

Rig also has a richer ToolSet, but I would not replace Artist's ToolRegistry with it. Artist's registry is below the Rig integration layer and additionally handles plugin ownership, cross-plugin invocation, correlation IDs, and recursion detection. Making artist-resource depend on Rig just to reuse ToolSet would create the wrong dependency direction.

Assessment: moderately underutilized, specifically around request-history hooks; not around tool registry.

wasmtime / wasmtime-wasi: YES — moderately

The component model/bindgen/resource table/WASI integration are already being used appropriately.

The important unused capability is asynchronous embedding. The branch instead maintains its own runtime/blocking bridge.

Assessment: moderately underutilized.

These are intended to repair correctness:

P0 — Resume loses durably acknowledged queued inputs and steering.

input() and steer() first append their transcript entry, then put the work into the in-memory VecDeque/Steering. But resume() reconstructs neither of those queues; Session::new() always starts with an empty input queue and empty steering inbox.

Therefore:

persist Input
acknowledge caller
crash before RunStarted
resume
that input remains permanently recorded but is never executed.

Likewise, a persisted SteeringQueued without a SteeringDelivered is lost after restart.

The existing tests cover an unfinished active run and undelivered steering while the same process remains alive, but not either condition across resume.

SHOULD fix. Rebuild on resume:

pending inputs = Inputs never referenced by RunStarted
pending steering = SteeringQueueds not covered by SteeringDelivered
preserve transcript order.

This is probably the most important new correctness finding.

P0 — The Rig 0.42 streaming adapter appears stale against Rig 0.42's stream lifecycle.

Artist matches MultiTurnStreamItem and treats StreamAssistantItem(ToolCall) as its tool-call event. It ignores ToolExecutionCommitted; there is no ToolExecutionStart arm in the current branch.

Rig 0.42 explicitly changed this API: a streamed assistant ToolCall now means only the model emitted a tool request. ToolExecutionStart means Rig actually began executing it after validation/hooks. Calls can be skipped, repaired, rejected, or dropped without ever executing.

So two things need checking immediately:

The exhaustive translate() match looks inconsistent with the locked rig-agent 0.42.0 API and may simply be stale.
Even if compilation is currently satisfied through some exact API detail, the semantics are wrong for execution telemetry.

SHOULD handle ToolExecutionStart distinctly.

P0 — Current rig-tap telemetry falsely equates “model requested tool” with “tool executed.”

artist-observe receives Artist's ToolCall and immediately emits rig_tap::EventKind::ToolInvoked.

Under Rig 0.42, that is no longer valid: model emission and actual tool execution are separate lifecycle stages.

This can report a tool as executed when Rig rejected or skipped it.

SHOULD:

retain canonical “model requested tool” information if Artist wants it;
introduce/use actual execution-start information for ToolInvoked;
correlate by Rig's internal_call_id;
emit completion only for tools that actually executed.

This also fixes the TapObserver.tools map retaining calls that never produce results.

P0/P1 — Artist throws away Rig 0.42's normalized terminal metadata, then invents "stop".

On FinalResponse, Artist keeps token usage and output only. It drops the richer completion metadata.

Then artist-observe unconditionally reports:

finish_reason: Some("stop")

for every successful run.

Rig 0.42 already normalizes provider finish reasons into:

Stop
Length
ToolCalls
ContentFilter
Other(String)

and deliberately preserves unknown provider reasons.

It also added normalized provider/model/message/response metadata and raw terminal information to its completion boundary.

SHOULD preserve the normalized Rig metadata through ModelEvent/StreamEvent as appropriate instead of reconstructing it.

At minimum: preserve the real finish reason.

This directly matches the branch plan's requirement to preserve provider correlation IDs.

P0/P1 — Investigate first-error termination against Rig 0.42's recoverable streaming-error contract.

Artist's Rig adapter currently does this on any stream error:

Err(error) => {
    yield Err(ModelError(error.to_string()));
    return;
}

Rig 0.42 explicitly says malformed provider frames can surface as an Err while the stream continues, and consumers of that normalized streaming layer must drain until None; an Err by itself is not necessarily terminal.

Because Artist sits behind AgentRunner, the coding agent should verify precisely which recoverable errors the agent stream forwards. If they reach this loop, Artist currently turns a recoverable malformed frame into a failed session.

High-priority compatibility test: malformed frame → subsequent valid data → genuine terminal. Artist should not prematurely kill a turn if Rig says that error is recoverable.

P1 — ModelError(String) wastes Rig's typed error API.

Everything from Rig is flattened immediately to error.to_string(). Artist telemetry consequently reports every failure as ErrorClass::Unknown, retriable: false, with no provider code or HTTP status.

Rig 0.42 has been moving specifically toward preserving provider responses, HTTP failure metadata, request IDs, and typed error classifications instead of flattening them into strings.

SHOULD retain a backend-neutral failure structure in the adapter containing the useful classification/correlation information. The durable transcript can still store a human-readable string if that is the intended stable format.

P1 — Transcript validation and persistence become roughly quadratic as sessions grow.

SessionRecord::append() pushes one item and then calls validate(), which walks the complete transcript and rebuilds its validation HashSets/HashMap every time.

The kernel makes this worse: every append clones the entire SessionRecord, then invokes that full validation once for each new entry.

MemoryStore clones the whole record again. FileStore::append() reloads the entire JSONL file, parses and validates it, then validates each appended record again before finally adding a few lines.

For long sessions this is a major scaling problem.

SHOULD introduce incremental validation state. Maintain things such as:

active run
known input IDs
queued steering IDs
call IDs
current sequence
previous-entry state

Then validating one new entry is normally O(1). Keep the full validate() for loading untrusted persisted data and tests.

No new dependency is necessary.

P1 — FileStore::append() has no same-session write serialization.

MemoryStore protects its state with a shared lock. FileStore is only a PathBuf; two cloned stores can independently:

load the same session at sequence N;
both validate an entry at N;
both append it.

If Artist guarantees globally that exactly one SessionHandle can ever exist per session, enforce that invariant somewhere. Otherwise FileStore needs per-session serialization around load/validate/append.

I would not introduce a database merely for this. An in-process per-session mutex is sufficient unless cross-process writers are actually supported.

P1 — FUSE reads entire resources just to obtain metadata.

classify() calls Children; if that fails, it performs a complete Read merely to calculate text.len(). getattr() does another complete read for file size. Actual FUSE read() also obtains the whole resource then slices out the requested byte range.

This is especially costly because FFF is indexing the FUSE tree. A filesystem crawl can therefore trigger content reads just to classify nodes.

Metadata is intentionally deferred for a separate design discussion; do not add
a generic operation in this slice.

Then:

FilesystemProvider → tokio::fs::metadata
append-only resource → current string length
other providers can answer cheaply where possible
FUSE doesn't have to read content to implement metadata.

This is an Artist abstraction improvement rather than another crate.

P1 — FUSE writes are whole-resource read/modify/write operations per kernel write callback.

Every FUSE write(offset, data) currently:

reads the complete resource;
copies it into bytes;
modifies a range;
converts the complete result back to UTF-8;
rewrites the complete resource.

setattr(size) does essentially the same.

That can become extremely expensive when ordinary Unix programs issue multiple writes. It also becomes race-prone if FUSE concurrency is enabled.

If FUSE is intended to behave like a normal writable filesystem, SHOULD either add byte/range operations to the resource contract or maintain real per-open file state and commit appropriately.

P1 — FUSE inode tables never reclaim anything despite fuser providing the exact lifecycle API.

Each newly encountered resource URI permanently receives an entry in both:

nodes: HashMap<u64, Node>
keys: HashMap<String, u64>

and next only increases.

fuser::Filesystem::forget exists specifically for inode reference lifetime management: each lookup can acquire a reference and forget(nlookup) releases references.

SHOULD use forget/batch_forget and reclaim dynamic resource inodes when no longer referenced.

This is a direct case of underusing the existing crate API.

P1 — rig-tap 0.3.0 is carrying an entire obsolete Rig core alongside Rig 0.42.

The branch lockfile contains both:

rig-core 0.37.0
rig-core 0.42.0

because rig-tap 0.3.0 depends on Rig 0.37 while Artist/Rig-memory/Rig-agent are on 0.42.

rig-tap's own current manifest confirms its mandatory rig-core = "0.37.0" dependency.

That explains an important thing from the first audit: do not simply replace Artist's telemetry adapter with rig-tap::TelemetryHook right now. Its Rig hook types belong to Rig 0.37, not Artist's 0.42 runtime.

Do this: remove rig-tap altogether, analyze if it was useful and what we'd need to replace it.

So this is real dependency skew, but not something to solve by force-fitting the existing hook.

P2 — fuser's abi-7-31 feature is literally a no-op.

Artist enables:

fuser = { version = "0.17", default-features = false, features = ["abi-7-31"] }

fuser 0.17 states that all abi-7-xx feature flags are ignored; its manifest defines abi-7-31 = [].

Remove the feature.

fuser 0.18 has already removed those flags entirely, though I would treat upgrading to 0.18 as a separate small dependency update rather than coupling it to this cleanup.

P2 — grep(regex) passes a promised regex through FFF's query-language parser.

Artist exposes an argument named regex and separately exposes include_glob, but then calls:

let query = parse_grep_query(regex);

before using regex mode.

FFF's parse_grep_query intentionally interprets parts of its input as search constraints. Its own source example says a query containing *.rs gets that portion removed from the grep text and converted to an extension constraint.

That means Artist's supposedly raw regex language can collide with FFF's query syntax.

SHOULD either use/build a raw FFF regex query without constraint parsing, or explicitly define Artist's argument as an FFF query instead of calling it a regex.

At minimum add tests for patterns containing things resembling *.rs, exclusions, path constraints, and type:rust.

P2 — ResourceRouter duplicates its generation state unnecessarily.

It maintains both:

AtomicU64 generation
watch::Sender<u64> topology

and manually keeps them synchronized.

Tokio's watch sender already stores the current value and supports borrow() plus atomic in-place send_modify(), even when no receivers currently exist.

This could simply become one source of truth:

generation() reads the watched value
changed() increments through send_modify.

Small cleanup, but it removes redundant synchronization.

P2 — Router generation currently treats every successful write as a topology change.

Any successful Write or Move increments router generation.

Today that causes SearchEngine::synchronize() to completely rebuild FFF.

So changing the contents of an existing text file can provoke a complete logical search-tree rescan.

This further strengthens the previous FFF recommendation: let FFF's watcher handle ordinary file create/modify/delete events and reserve the router generation signal for things FFF cannot observe, primarily logical route/projection topology changes.

FFF already has thread-safe live watcher/shared-index and coalesced full-rescan facilities.

P2 — ToolRegistry and much of ResourceRouter probably do not need async locks.

ToolRegistry uses tokio::sync::RwLock, making register, definitions, and owner asynchronous even though their critical sections are tiny in-memory map operations and no .await occurs while the lock is held.

The router has the same pattern around a small route vector; the selected provider is cloned before the actual async call.

A normal std::sync::RwLock would simplify the API and remove several artificial async/blocking crossings, particularly in the plugin host.

COULD/SHOULD simplify, unless there is an intended contention pattern not represented by the current branch.

P3 — Rig's tool concurrency is available but entirely unused.

Rig 0.42 supports per-run tool_concurrency(n) with deterministic atomic batch commit/surface semantics. Default remains one.

Artist never exposes it.

COULD expose it as an opt-in RigModel/model configuration setting. Do not globally increase it: Artist has side-effecting tools such as writes/moves, so concurrency needs to be explicitly chosen.

P3 — Artist intentionally discards richer Rig streaming content.

Current translation ignores reasoning, reasoning deltas, unknown/provider-native streamed items, etc., and flattens text/JSON/image tool-result content into one string.

Rig deliberately preserves those richer/unknown items for forward compatibility, including provider-hosted tool results.
