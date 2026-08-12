# Artist runtime relationship store

Artist stores model-facing resource shortcuts in a dedicated per-project Mnestic
database at `.artist/state/relationships`. The pinned Mnestic package exports
the historical Rust crate name `cozo`; it is the maintained Cozo successor and
uses RocksDB in this configuration.

This store is deliberately separate from `artist-memory` and Muse. Runtime
relationships answer navigation/lifecycle questions about resources; they do
not assert semantic facts, schedule work, or become ontology evidence.

## Alternatives considered

| Option | Decision |
| --- | --- |
| Mnestic + RocksDB | Selected: durable embedded data, simple relation queries, and future indexed traversals without a service dependency. |
| Purpose-built registry/index | Rejected for relationships: it would duplicate durable storage, migration, and query work already supplied by Mnestic. The session registry remains the authority for session lifecycle. |
| Reuse memory or Muse graph | Rejected: graph shape is not a reason to merge ownership, retention, access policy, or semantic meaning. |

## Operational contract

- An edge has its own stable `relation://<id>` identity and durable creation
  timestamp. Mutations are single-store RocksDB writes; no cross-store
  transaction is implied.
- Read projections are one hop only. Cycles are valid and are never recursively
  followed by `read`, `find`, or `grep`.
- Session deletion removes every edge whose source or target is that canonical
  runtime path in the same deletion workflow. A relation to a real or remote
  resource is retained when Artist cannot authoritatively know that resource
  was deleted; it is a broken navigation pointer, not a deletion request.
- Reading a missing target returns that target's normal typed not-found result.
  Reading the `relation://` edge remains possible so the model can inspect or
  remove the broken pointer deliberately.
- Mnestic schema migration is owned by `relationships.rs`: the edge relation
  is created idempotently on project open. Any incompatible future migration
  must be explicit and preserve stable edge IDs or provide a recorded mapping.
- Current queries are exact source/target/kind filters over the small runtime
  edge relation. If cardinality proves it necessary, secondary indexes are an
  internal migration; they do not change `relation://` identity or one-hop
  semantics.
