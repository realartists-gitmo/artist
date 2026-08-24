# Transcript v1: permanent canonical session record

## Purpose

Artist's durable conversation is an append-only, provider-neutral event log.
It is the source of truth for replay and resume. Model messages, UI streams,
memory windows, compaction views, and telemetry are projections; none of them
may become a second canonical conversation.

This document records the defects in the prototype format and the permanent
v1 contract that resolves each one.

## Issues being fixed

1. **Asymmetric termination.** Success was inferred from
   `assistant-message.complete = true`, while failure and interruption had
   separate terminal entries. `RunOutcome` existed but was not stored.
2. **Incomplete invariants.** Duplicate message/input/steering/run IDs,
   repeated steering delivery, repeated tool results, and successful runs with
   unresolved tools were accepted.
3. **Unsafe concurrent append.** A store append had no expected-tail
   precondition. Two writers could validate the same tail and append conflicting
   sequence numbers.
4. **Incomplete crash recovery.** JSON objects were appended one per event.
   A torn last line made the whole session unreadable, and a logical multi-entry
   transition was not one physical commit.
5. **Quadratic append behavior.** The kernel cloned and revalidated the whole
   record for every append, and the file store reread and revalidated it again.
6. **No evolution boundary.** The decoder only accepted one exact Rust enum and
   version. There was no explicit migration path or compatibility policy.
7. **Unvalidated public representation.** Public fields and derived
   deserialization allowed callers to construct a `SessionRecord` without
   proving its invariants.

## Canonical domain contract

- `SessionRecord` has private fields and can only be created or decoded through
  validating APIs. Read-only access is provided by getters.
- Every entry has a zero-based, gap-free sequence number and deterministic
  event ID.
- Every run begins with `run-started` and ends exactly once with
  `run-finished { outcome }`.
- `RunOutcome` is one of:
  - `completed { message-id }`
  - `failed { message-id?, error }`
  - `interrupted { message-id, cause }`
- Assistant content is stored independently from the terminal outcome. This
  keeps message data uniform while the terminal event says why the run ended.
- IDs are unique in their namespace. An input starts at most one run. An
  assistant message belongs to the active run. A steering notification is
  delivered at most once.
- A tool call has at most one result. Successful completion requires every tool
  call in that run to be resolved. Failure and interruption may preserve open
  calls as evidence of cancellation.
- Compaction is an appended artifact referring only to an earlier sequence. It
  never rewrites or deletes canonical entries.
- Full replay validation and successful incremental append use the same state
  transition reducer.

## Store contract

`append(session, expected-sequence, entries)` is an optimistic transaction.
The store atomically commits the complete batch only when its durable tail is
exactly `expected-sequence`; otherwise it returns a typed conflict containing
the actual sequence. The in-memory and file stores implement identical
semantics.

The file store serializes writers with an operating-system file lock, so the
precondition also holds across independent `FileStore` instances and
processes—not merely within one Rust object.

## Physical file format

Format version 2 is newline-delimited framed JSON:

1. One `header` frame freezes the session ID and initial context.
2. Each append is one `batch` frame containing its expected sequence and all
   entries in that logical transition.
3. Every frame contains a SHA-256 hash of its canonical payload. Batch payloads
   include the preceding frame hash, forming an integrity chain.
4. A batch is visible only as a complete newline-terminated, hash-valid frame.
5. On recovery, an unterminated final byte suffix is a torn write and is
   ignored. Malformed, hash-invalid, or sequence-invalid completed frames are
   corruption and are rejected.
6. Successful creation and append call `sync_data` before returning.

Ordinary appends use an in-process validated tail cache. The cache is reused
only when the locked file length still matches; an external writer invalidates
it and causes a single replay. Thus the common append path is proportional to
the new batch, while loading and audits remain proportional to the log.

## Pre-production evolution policy

- `RECORD_VERSION = 5` identifies the only accepted canonical domain schema.
- `FILE_FORMAT_VERSION = 2` identifies the framed physical log.
- Artist is not in production. Only the exact current record and file versions
  are decoded; every stale or unknown version and event variant is rejected.
- Schema or invariant changes bump the relevant version and invalidate existing
  development data. Do not add migration types, compatibility branches,
  rewrite-on-read behavior, or legacy decoders.

## Required evidence

- Uniform completion, failure, and interruption replay tests.
- Adversarial tests for every uniqueness, steering, tool, and terminal
  invariant.
- Two-writer expected-sequence conflict tests for memory and independent file
  stores.
- Torn-tail recovery, completed-frame corruption, hash-chain, and atomic-batch
  tests.
- Exact-version acceptance and exhaustive stale-version rejection tests.
- A long-session test demonstrating incremental append validation does not
  perform full replay on the successful hot path.
- Existing kernel, Rig, plugin, resource replay, formatting, and lint gates.
