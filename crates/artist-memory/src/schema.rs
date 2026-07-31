//! Relation and index definitions, applied idempotently at open.
//!
//! This is the read model. The session event log is the system of record, and
//! everything here is a rebuildable projection of it — the same shape the rest
//! of the harness already uses.
//!
//! **Two different supersession mechanisms, on purpose.** The searchable
//! relations (`fact`, `chunk`) use a `live` flag with partial indexes (SCD2, in
//! warehousing terms), because their whole job is ranked top-k retrieval and
//! that needs HNSW/FTS/LSH. The relational layer (`stmt`) uses a real
//! `Validity` column instead.
//!
//! An earlier version of this design claimed the two were mutually exclusive —
//! that bitemporality rules out every index. That is true of **`TxTime`**
//! (transaction time) only: `reject_txtime_relation` in mnestic checks
//! `has_txtime()` and nothing else. A `Validity` column carries no such
//! restriction, and *valid* time is the axis a memory system actually wants —
//! when a belief held, not when the row was written. Conflating the two cost
//! this design the ability to ask what it used to believe.
//!
//! Two things here are load-bearing and were confirmed against a live RocksDB
//! instance rather than read from documentation:
//!
//! * The HNSW index is created with a **create-time** `filter: live`. Query-time
//!   `filter:` is only a post-filter over the `ef` candidate pool, so a selective
//!   one silently returns fewer than `k` rows. Building the filter into the index
//!   means superseded facts leave recall on `:update`, which the probe confirmed.
//! * The **full-text** index needs that condition too, not just the vector one:
//!   retrieval fuses a BM25 leg with the semantic leg, so an unfiltered text
//!   index keeps resurrecting superseded facts through the other half of the
//!   query. `::fts` spells it `extract_filter`, which rewrites the extractor to
//!   yield null — and so skip the row — when the condition fails.
//!
//! There used to be a third index here, a MinHash-LSH over 5-gram shingles,
//! used as the admission check before a write. It was removed at version 4
//! because **it is not deterministic**: the permutations are seeded when the
//! index is built, so the same pair of texts was flagged in 2 of 5 runs and
//! missed in the other 3. An admission policy cannot be built on a coin flip,
//! and its 0.8 target threshold was in any case far too high — measured
//! against real revisions of stored facts it would have missed six in seven.
//! Candidates now come from the same hybrid search that serves recall, and the
//! decision is made exactly, in `admission.rs`.

/// Bumped whenever the DDL below changes in a way that needs a rebuild.
pub const SCHEMA_VERSION: i64 = 4;

/// Embedding width. CodeRankEmbed emits 768; the fallback bge-small emits 384,
/// so this travels with the model choice and a change forces a reindex.
pub const DIM: usize = 768;

/// Relations. Applied in order; each is skipped when it already exists.
pub const RELATIONS: &[(&str, &str)] = &[
    (
        // NOT `_meta`: a leading underscore marks a transaction-scoped
        // ephemeral relation in CozoScript, so `:create _meta` fails with
        // "write operation attempted in a read-only transaction".
        "meta",
        r#":create meta { key: String => value: String }"#,
    ),
    (
        "fact",
        r#"
        :create fact {
            fact_id: Int
            =>
            subject: String,
            predicate: String,
            object: String,
            text: String,
            emb: <F32; 768>,
            live: Bool default true,
            superseded_by: Int? default null,
            source_session: String default '',
            source_seq: Int default 0,
            origin: String default 'unknown',
            created_at: Float default now(),
        }
        "#,
    ),
    (
        "chunk",
        r#"
        :create chunk {
            chunk_id: Int
            =>
            path: String,
            lang: String,
            start_line: Int,
            end_line: Int,
            body: String,
            emb: <F32; 768>,
            content_hash: String,
            mtime: Float default 0.0,
        }
        "#,
    ),
    // ---- the relational layer -------------------------------------------
    //
    // One shape for everything. A sort is a statement, sort membership is a
    // statement, and whether a sort has an authority is a statement — so
    // meeting a new kind of thing is a write and never a migration.
    //
    // Arity is carried in a side relation rather than a `holds2/holds3/holds4`
    // ladder, because a ladder would put a bound in storage that the logic does
    // not have. `stmt` + `arg` admits any arity at the cost of one join.
    (
        "sym",
        r#":create sym { sym_id: Int => name: String }"#,
    ),
    // Valid time, not a liveness flag.
    //
    // The original plan ruled bitemporality out on the grounds that it rejects
    // every index — but that restriction is on `TxTime` (transaction time)
    // only; `reject_txtime_relation` checks `has_txtime()` and nothing else. A
    // `Validity` column is unrestricted, and valid time is what a memory system
    // actually needs: *when the belief held*, not when the row was written.
    //
    // Retraction is a `:put` with `[t, false]`, so supersession becomes part of
    // the history the logic can quantify over rather than a flag that hides it.
    (
        "stmt",
        r#"
        :create stmt {
            stmt_id: Int,
            at: Validity
            =>
            pred: Int,
            arity: Int,
            source_session: String default '',
            source_seq: Int default 0,
            origin: String default 'unknown',
        }
        "#,
    ),
    (
        "arg",
        r#":create arg { stmt_id: Int, pos: Int => val: Int }"#,
    ),
    // ---- the universal object graph -------------------------------------
    //
    // One representation for everything. A fact is not a privileged row here;
    // `(prefers adam tabs)` is an expression node stored exactly the way a
    // query, a lambda, a residual or a continuation is. `stmt`/`arg` above are
    // retained as a compatibility adapter and are strictly weaker: they cannot
    // hold a formula at all.
    (
        "object",
        r#"
        :create object {
            hi: Int, lo: Int
            =>
            kind: Int,
            name: String,
            has_name: Bool,
            payload: Bytes,
            created_at: Float default now(),
        }
        "#,
    ),
    (
        "operand",
        r#":create operand { hi: Int, lo: Int, position: Int => value_hi: Int, value_lo: Int }"#,
    ),
    (
        "binder_var",
        r#"
        :create binder_var {
            hi: Int, lo: Int, position: Int
            =>
            var_hi: Int, var_lo: Int,
            dom_hi: Int? default null, dom_lo: Int? default null,
        }
        "#,
    ),
];

/// Indexes, keyed by the name used to detect existence via `::indices`.
pub const INDEXES: &[(&str, &str, &str)] = &[
    (
        "fact",
        "emb_idx",
        r#"
        ::hnsw create fact:emb_idx {
            dim: 768, dtype: F32, fields: [emb], distance: Cosine,
            m: 16, ef_construction: 200,
            filter: live,
        }
        "#,
    ),
    (
        "fact",
        "text_fts",
        r#"
        ::fts create fact:text_fts {
            extractor: text, extract_filter: live,
            tokenizer: Simple,
            filters: [Lowercase, Stemmer('English'), Stopwords('en')]
        }
        "#,
    ),
    (
        "chunk",
        "emb_idx",
        r#"
        ::hnsw create chunk:emb_idx {
            dim: 768, dtype: F32, fields: [emb], distance: Cosine,
            m: 16, ef_construction: 200,
        }
        "#,
    ),
    (
        "chunk",
        "body_fts",
        r#"
        ::fts create chunk:body_fts {
            extractor: body, tokenizer: Simple,
            filters: [Lowercase, Stemmer('English')]
        }
        "#,
    ),
];
