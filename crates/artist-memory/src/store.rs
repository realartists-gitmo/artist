//! The Mnestic-backed store.
//!
//! Every `run_script` call is blocking, so all of them go through
//! `spawn_blocking`: the TUI input loop must never stall on a query. The
//! `DbInstance` is an `Arc` of storage handles internally and is cheap to
//! clone, so each task gets its own clone rather than sharing a lock.

use crate::schema::{DIM, INDEXES, RELATIONS, SCHEMA_VERSION};
use crate::types::{CodeHit, Fact, Hit, NewFact, Scope};
use anyhow::{Context, Result, anyhow};
use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability, ScriptRunOptions};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// Ceiling on any single read. Guards against a pathological recursion pinning
/// a blocking thread for the life of the process.
const READ_TIMEOUT_SECS: f64 = 5.0;
/// Process-wide floor, including writes and index builds.
const DEFAULT_TIMEOUT_SECS: f64 = 300.0;

/// What `reconcile` changed, and what the caller still owes.
#[derive(Debug, Default)]
pub struct Reconciliation {
    /// Facts dropped because a rewind masked the event that created them.
    pub removed: Vec<i64>,
    /// Facts the log knows about that the store has lost. These need
    /// re-embedding before they can be reinserted.
    pub missing: Vec<artist_session::MemoryWritten>,
}

impl Reconciliation {
    pub fn is_clean(&self) -> bool {
        self.removed.is_empty() && self.missing.is_empty()
    }
}

#[derive(Clone)]
pub struct MemoryStore {
    db: Arc<DbInstance>,
    scope: Scope,
    next_fact_id: Arc<AtomicI64>,
    next_chunk_id: Arc<AtomicI64>,
}

impl MemoryStore {
    /// Open (creating if absent) the store at `path` and bring the schema up to
    /// date. Blocking work runs on a blocking thread.
    pub async fn open(path: impl AsRef<Path>, scope: Scope) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(&path, scope))
            .await
            .map_err(|e| anyhow!("memory store open panicked: {e}"))?
    }

    fn open_blocking(path: &Path, scope: Scope) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let db = DbInstance::new("rocksdb", path, "")
            .map_err(|e| anyhow!("opening memory store at {}: {e:?}", path.display()))?;
        db.set_default_query_timeout(Some(DEFAULT_TIMEOUT_SECS));

        let store = Self {
            db: Arc::new(db),
            scope,
            next_fact_id: Arc::new(AtomicI64::new(0)),
            next_chunk_id: Arc::new(AtomicI64::new(0)),
        };
        store.apply_schema()?;
        store.seed_id_counters()?;
        Ok(store)
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    // ---- schema ---------------------------------------------------------

    fn existing_relations(&self) -> Result<Vec<String>> {
        let rows = self.script_blocking("::relations", BTreeMap::new(), false)?;
        Ok(rows
            .rows
            .iter()
            .filter_map(|r| r.first().and_then(|v| v.get_str().map(str::to_owned)))
            .collect())
    }

    fn existing_indexes(&self, relation: &str) -> Vec<String> {
        // `::indices` errors on a relation with none; treat that as empty.
        match self.script_blocking(&format!("::indices {relation}"), BTreeMap::new(), false) {
            Ok(rows) => rows
                .rows
                .iter()
                .filter_map(|r| r.first().and_then(|v| v.get_str().map(str::to_owned)))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn apply_schema(&self) -> Result<()> {
        let present = self.existing_relations()?;
        for (name, ddl) in RELATIONS {
            if present.iter().any(|r| r == name) {
                continue;
            }
            self.script_blocking(ddl, BTreeMap::new(), true)
                .with_context(|| format!("creating relation {name}"))?;
        }

        for (relation, index, ddl) in INDEXES {
            let have = self.existing_indexes(relation);
            // `::indices` reports either the bare name or `rel:name`.
            let exists = have
                .iter()
                .any(|i| i == index || i == &format!("{relation}:{index}"));
            if exists {
                continue;
            }
            self.script_blocking(ddl, BTreeMap::new(), true)
                .with_context(|| format!("creating index {relation}:{index}"))?;
        }

        let mut params = BTreeMap::new();
        params.insert("v".to_string(), DataValue::from(SCHEMA_VERSION.to_string()));
        self.script_blocking(
            "?[key, value] <- [['schema_version', $v]] :put meta {key => value}",
            params,
            true,
        )
        .context("recording schema version")?;
        Ok(())
    }

    /// Read the stored schema version, if any.
    pub async fn schema_version(&self) -> Result<Option<i64>> {
        let rows = self
            .script(
                "?[value] := *meta{key: 'schema_version', value}",
                BTreeMap::new(),
                false,
            )
            .await?;
        Ok(rows
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_str())
            .and_then(|s| s.parse().ok()))
    }

    fn seed_id_counters(&self) -> Result<()> {
        // Aggregations belong in the rule *head* in CozoScript; writing
        // `m = max(x)` in the body fails with `eval::no_implementation`.
        for (script, counter) in [
            ("?[max(fact_id)] := *fact{fact_id}", &self.next_fact_id),
            ("?[max(chunk_id)] := *chunk{chunk_id}", &self.next_chunk_id),
        ] {
            let rows = self.script_blocking(script, BTreeMap::new(), false)?;
            let max = rows
                .rows
                .first()
                .and_then(|r| r.first())
                .and_then(|v| v.get_int())
                .unwrap_or(-1);
            counter.store(max + 1, Ordering::SeqCst);
        }
        Ok(())
    }

    // ---- script plumbing -------------------------------------------------

    fn script_blocking(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
        mutable: bool,
    ) -> Result<NamedRows> {
        let mutability = if mutable {
            ScriptMutability::Mutable
        } else {
            ScriptMutability::Immutable
        };
        let options = if mutable {
            ScriptRunOptions::default()
        } else {
            ScriptRunOptions::default().with_timeout(READ_TIMEOUT_SECS)
        };
        self.db
            .run_script_with_options(script, params, mutability, options)
            .map_err(|e| anyhow!("{e:?}"))
    }

    /// Run a script off the async runtime's worker threads.
    pub async fn script(
        &self,
        script: &'static str,
        params: BTreeMap<String, DataValue>,
        mutable: bool,
    ) -> Result<NamedRows> {
        let this = self.clone();
        let script = script.to_string();
        tokio::task::spawn_blocking(move || this.script_blocking(&script, params, mutable))
            .await
            .map_err(|e| anyhow!("memory query panicked: {e}"))?
    }

    async fn script_owned(
        &self,
        script: String,
        params: BTreeMap<String, DataValue>,
        mutable: bool,
    ) -> Result<NamedRows> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.script_blocking(&script, params, mutable))
            .await
            .map_err(|e| anyhow!("memory query panicked: {e}"))?
    }

    // ---- writes ----------------------------------------------------------

    /// Insert facts, returning their allocated ids. Embeddings must already be
    /// `DIM` wide; a mismatch is rejected here rather than by a Datalog error.
    pub async fn put_facts(&self, facts: &[NewFact]) -> Result<Vec<i64>> {
        if facts.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::with_capacity(facts.len());
        let mut rows = Vec::with_capacity(facts.len());
        for fact in facts {
            if fact.embedding.len() != DIM {
                return Err(anyhow!(
                    "embedding width {} does not match schema DIM {DIM}",
                    fact.embedding.len()
                ));
            }
            let id = self.next_fact_id.fetch_add(1, Ordering::SeqCst);
            ids.push(id);
            rows.push(DataValue::List(vec![
                DataValue::from(id),
                DataValue::from(fact.subject.as_str()),
                DataValue::from(fact.predicate.as_str()),
                DataValue::from(fact.object.as_str()),
                DataValue::from(fact.text.as_str()),
                DataValue::List(
                    fact.embedding
                        .iter()
                        .map(|f| DataValue::from(*f as f64))
                        .collect(),
                ),
                DataValue::from(fact.source_session.as_str()),
                DataValue::from(fact.source_seq),
                DataValue::from(fact.origin.as_str()),
            ]));
        }

        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));
        // CozoScript has no bracket indexing, so destructure through a const rule.
        self.script(
            r#"
            raw[fact_id, subject, predicate, object, text, raw_emb,
                source_session, source_seq, origin] <- $rows
            ?[fact_id, subject, predicate, object, text, emb,
              source_session, source_seq, origin] :=
                raw[fact_id, subject, predicate, object, text, raw_emb,
                    source_session, source_seq, origin],
                emb = vec(raw_emb)
            :put fact {fact_id => subject, predicate, object, text, emb,
                       source_session, source_seq, origin}
            "#,
            params,
            true,
        )
        .await?;
        Ok(ids)
    }

    /// Retire `old` in favour of `new`.
    ///
    /// The retired row stays, so `superseded_by` can still be followed, but
    /// every index over `fact` is filtered on `live` — so it leaves recall
    /// without any explicit index maintenance here. History is not written
    /// alongside: the `memory.written` event already records the supersession,
    /// with the session and sequence a copy of the row could not carry.
    pub async fn supersede(&self, old: i64, new: i64) -> Result<()> {
        let mut params = BTreeMap::new();
        params.insert("old".to_string(), DataValue::from(old));
        params.insert("new".to_string(), DataValue::from(new));
        self.script(
            r#"
            ?[fact_id, live, superseded_by] := fact_id = $old, live = false,
                                               superseded_by = $new
            :update fact {fact_id => live, superseded_by}
            "#,
            params,
            true,
        )
        .await?;
        Ok(())
    }

    /// Drop a fact outright. Used by `forget`, and by rewind reconciliation.
    pub async fn forget(&self, id: i64) -> Result<()> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(id));
        self.script("?[fact_id] := fact_id = $id :rm fact {fact_id}", params, true)
            .await?;
        Ok(())
    }

    // ---- reads -----------------------------------------------------------

    /// Live facts a candidate write might be restating or revising.
    ///
    /// This is ordinary retrieval, not a special-purpose probe. The dedicated
    /// LSH index it replaces was not deterministic — the same pair of texts was
    /// flagged in 2 of 5 runs — and its 0.8 target missed most real revisions;
    /// see `schema.rs`. Reusing the recall query costs nothing extra (the
    /// embedding is needed for the write anyway), is reproducible, and finds
    /// paraphrases that shingle overlap never could. Deciding what to *do* with
    /// these is [`crate::admit`]'s job.
    pub async fn revision_candidates(
        &self,
        text: &str,
        embedding: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, String)>> {
        Ok(self
            .search_facts(text, embedding, k)
            .await?
            .into_iter()
            .map(|hit| (hit.id, hit.text))
            .collect())
    }

    /// Hybrid retrieval: HNSW and BM25 legs fused by reciprocal rank.
    ///
    /// The semantic leg negates distance because RRF requires every list
    /// normalized so that higher is better.
    ///
    /// **Either leg may be absent, and the query still runs.** Both used to be
    /// named unconditionally in one fused script, which made every leg a single
    /// point of failure for the whole channel:
    ///
    /// * no embedder — a cold start, a missing model file — failed the query on
    ///   a dimension mismatch, even though BM25 needs no vector at all;
    /// * and the BM25 leg parses `$qt` as a *search expression*, so ordinary
    ///   user text failed it. Measured over thirteen realistic queries, seven
    ///   were rejected: anything containing `-`, `?`, `/`, `+`, `%` or `@`, and
    ///   the empty string. Since the prompt-conditioned channel passes the
    ///   user's raw message, that covered any question, any path, and any
    ///   mention of a hyphenated crate name — silently, because `recall`
    ///   discards the error.
    ///
    /// Degrading costs ranking quality. Failing cost the entire channel.
    pub async fn search_facts(
        &self,
        query_text: &str,
        query_vec: &[f32],
        k: usize,
    ) -> Result<Vec<Hit>> {
        let leg_k = (k * 4).max(20);
        let ef = (leg_k * 4).max(100);
        let text_query = fts_query(query_text);
        let has_vector = query_vec.len() == DIM;
        if !has_vector && text_query.is_none() {
            return Ok(Vec::new());
        }
        let mut params = BTreeMap::new();
        params.insert(
            "qv".to_string(),
            DataValue::List(query_vec.iter().map(|f| DataValue::from(*f as f64)).collect()),
        );
        params.insert(
            "qt".to_string(),
            DataValue::from(text_query.clone().unwrap_or_default()),
        );

        let semantic = format!(
            "sem[id, score] := ~fact:emb_idx{{ fact_id: id | query: vec($qv), k: {leg_k}, \
             ef: {ef}, bind_distance: __d }}, score = -__d"
        );
        let lexical = format!(
            "txt[id, score] := ~fact:text_fts{{ fact_id: id | query: $qt, k: {leg_k}, \
             bind_score: score }}"
        );
        let ranking = match (has_vector, text_query.is_some()) {
            (true, true) => format!(
                r#"
            {semantic}
            {lexical}
            combined[__lid, id, score] := sem[id, score], __lid = 'semantic'
            combined[__lid, id, score] := txt[id, score], __lid = 'text'
            fused[id, score] <~ ReciprocalRankFusion(combined[__lid, id, score], k: 60.0)
            "#
            ),
            (true, false) => format!(
                r#"
            {semantic}
            fused[id, score] := sem[id, score]
            "#
            ),
            (false, true) => format!(
                r#"
            {lexical}
            fused[id, score] := txt[id, score]
            "#
            ),
            (false, false) => unreachable!("guarded above"),
        };

        let script = format!(
            r#"{ranking}
            ?[id, score, text, subject, predicate, object, origin, created_at] :=
                fused[id, score],
                *fact{{fact_id: id, text, subject, predicate, object, origin, created_at}}
            :order -score
            :limit {k}
            "#
        );
        let rows = self.script_owned(script, params, false).await?;
        Ok(rows
            .rows
            .iter()
            .filter_map(|r| {
                Some(Hit {
                    id: r[0].get_int()?,
                    score: r[1].get_float()?,
                    text: r[2].get_str()?.to_owned(),
                    subject: r[3].get_str().unwrap_or_default().to_owned(),
                    predicate: r[4].get_str().unwrap_or_default().to_owned(),
                    object: r[5].get_str().unwrap_or_default().to_owned(),
                    origin: r[6].get_str().unwrap_or_default().to_owned(),
                    created_at: r[7].get_float().unwrap_or_default(),
                    scope: self.scope,
                })
            })
            .collect())
    }

    /// Every live fact, for rebuild and inspection.
    pub async fn live_facts(&self) -> Result<Vec<Fact>> {
        let rows = self
            .script(
                r#"?[fact_id, subject, predicate, object, text, origin] :=
                       *fact{fact_id, subject, predicate, object, text, origin, live: true}
                   :order fact_id"#,
                BTreeMap::new(),
                false,
            )
            .await?;
        Ok(rows
            .rows
            .iter()
            .filter_map(|r| {
                Some(Fact {
                    id: r[0].get_int()?,
                    subject: r[1].get_str()?.to_owned(),
                    predicate: r[2].get_str()?.to_owned(),
                    object: r[3].get_str()?.to_owned(),
                    text: r[4].get_str()?.to_owned(),
                    origin: r[5].get_str().unwrap_or_default().to_owned(),
                })
            })
            .collect())
    }

    pub async fn fact_count(&self) -> Result<usize> {
        let rows = self
            .script(
                "?[count(fact_id)] := *fact{fact_id, live: true}",
                BTreeMap::new(),
                false,
            )
            .await?;
        Ok(rows
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0) as usize)
    }

    /// Ids of every fact currently stored, live or retired.
    pub async fn all_fact_ids(&self) -> Result<Vec<i64>> {
        let rows = self
            .script("?[fact_id] := *fact{fact_id}", BTreeMap::new(), false)
            .await?;
        Ok(rows.rows.iter().filter_map(|r| r[0].get_int()).collect())
    }

    /// Reconcile the store against the session log.
    ///
    /// Replay runs over `visible_events`, so a rewind that masks a
    /// `memory.written` event makes the corresponding fact disappear here too —
    /// the same property `TodoStore` gets, for the same reason. Facts present
    /// in the store but absent from the visible log are dropped; facts in the
    /// log but missing from the store are reported so the caller can re-embed
    /// and reinsert them (embedding is not recorded in the event, since it is
    /// derivable and would bloat the log by 3 KB per fact).
    pub async fn reconcile(&self, events: &[artist_session::Envelope]) -> Result<Reconciliation> {
        let mut expected: std::collections::BTreeMap<i64, artist_session::MemoryWritten> =
            Default::default();
        let mut retired: std::collections::BTreeSet<i64> = Default::default();

        for envelope in artist_session::visible_events(events) {
            if let artist_session::SessionEvent::MemoryWritten(written) = envelope.event() {
                if written.scope != self.scope.as_str() {
                    continue;
                }
                if let Some(old) = written.superseded {
                    retired.insert(old);
                }
                expected.insert(written.fact_id, written);
            }
        }

        let mut removed = Vec::new();
        for id in self.all_fact_ids().await? {
            if !expected.contains_key(&id) {
                self.forget(id).await?;
                removed.push(id);
            }
        }

        let present: std::collections::BTreeSet<i64> =
            self.all_fact_ids().await?.into_iter().collect();
        let missing = expected
            .into_values()
            .filter(|w| !present.contains(&w.fact_id))
            .collect();

        // Re-apply retirements: a rewind can restore a fact that a later
        // supersession had retired, and vice versa.
        for old in retired {
            if present.contains(&old) {
                self.mark_retired(old).await?;
            }
        }

        Ok(Reconciliation { removed, missing })
    }

    async fn mark_retired(&self, id: i64) -> Result<()> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(id));
        self.script(
            "?[fact_id, live] := fact_id = $id, live = false :update fact {fact_id => live}",
            params,
            true,
        )
        .await?;
        Ok(())
    }

    /// Insert facts at ids chosen by the caller, used when replaying the log.
    pub async fn put_facts_with_ids(&self, facts: &[(i64, NewFact)]) -> Result<()> {
        if facts.is_empty() {
            return Ok(());
        }
        let mut rows = Vec::with_capacity(facts.len());
        for (id, fact) in facts {
            if fact.embedding.len() != DIM {
                return Err(anyhow!(
                    "embedding width {} does not match schema DIM {DIM}",
                    fact.embedding.len()
                ));
            }
            rows.push(DataValue::List(vec![
                DataValue::from(*id),
                DataValue::from(fact.subject.as_str()),
                DataValue::from(fact.predicate.as_str()),
                DataValue::from(fact.object.as_str()),
                DataValue::from(fact.text.as_str()),
                DataValue::List(
                    fact.embedding
                        .iter()
                        .map(|f| DataValue::from(*f as f64))
                        .collect(),
                ),
                DataValue::from(fact.source_session.as_str()),
                DataValue::from(fact.source_seq),
                DataValue::from(fact.origin.as_str()),
            ]));
            self.next_fact_id.fetch_max(*id + 1, Ordering::SeqCst);
        }
        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));
        self.script(
            r#"
            raw[fact_id, subject, predicate, object, text, raw_emb,
                source_session, source_seq, origin] <- $rows
            ?[fact_id, subject, predicate, object, text, emb,
              source_session, source_seq, origin] :=
                raw[fact_id, subject, predicate, object, text, raw_emb,
                    source_session, source_seq, origin],
                emb = vec(raw_emb)
            :put fact {fact_id => subject, predicate, object, text, emb,
                       source_session, source_seq, origin}
            "#,
            params,
            true,
        )
        .await?;
        Ok(())
    }

    // ---- code chunks -----------------------------------------------------

    /// The content hashes currently indexed for `path`, so an incremental pass
    /// can re-embed only what actually moved. A full 40k-chunk index takes
    /// close to two hours, so this comparison is what makes indexing a
    /// resumable background job rather than a startup stall.
    pub async fn chunk_hashes(&self, path: &str) -> Result<Vec<(i64, String)>> {
        let mut params = BTreeMap::new();
        params.insert("path".to_string(), DataValue::from(path));
        let rows = self
            .script(
                "?[chunk_id, content_hash] := *chunk{chunk_id, path, content_hash}, path = $path",
                params,
                false,
            )
            .await?;
        Ok(rows
            .rows
            .iter()
            .filter_map(|r| Some((r[0].get_int()?, r[1].get_str()?.to_owned())))
            .collect())
    }

    /// Replace every chunk for one file. Callers pass only chunks whose hash
    /// changed; unchanged chunks keep their existing embedding.
    pub async fn put_chunks(&self, chunks: &[(crate::Chunk, Vec<f32>)]) -> Result<Vec<i64>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::with_capacity(chunks.len());
        let mut rows = Vec::with_capacity(chunks.len());
        for (chunk, embedding) in chunks {
            if embedding.len() != DIM {
                return Err(anyhow!(
                    "embedding width {} does not match schema DIM {DIM}",
                    embedding.len()
                ));
            }
            let id = self.next_chunk_id.fetch_add(1, Ordering::SeqCst);
            ids.push(id);
            rows.push(DataValue::List(vec![
                DataValue::from(id),
                DataValue::from(chunk.path.as_str()),
                DataValue::from(chunk.lang.as_str()),
                DataValue::from(chunk.start_line as i64),
                DataValue::from(chunk.end_line as i64),
                DataValue::from(chunk.body.as_str()),
                DataValue::List(embedding.iter().map(|f| DataValue::from(*f as f64)).collect()),
                DataValue::from(chunk.content_hash.as_str()),
            ]));
        }
        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));
        self.script(
            r#"
            raw[chunk_id, path, lang, start_line, end_line, body, raw_emb, content_hash] <- $rows
            ?[chunk_id, path, lang, start_line, end_line, body, emb, content_hash] :=
                raw[chunk_id, path, lang, start_line, end_line, body, raw_emb, content_hash],
                emb = vec(raw_emb)
            :put chunk {chunk_id => path, lang, start_line, end_line, body, emb, content_hash}
            "#,
            params,
            true,
        )
        .await?;
        Ok(ids)
    }

    pub async fn remove_chunks(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut params = BTreeMap::new();
        params.insert(
            "ids".to_string(),
            DataValue::List(ids.iter().map(|i| DataValue::List(vec![DataValue::from(*i)])).collect()),
        );
        self.script(
            "?[chunk_id] <- $ids :rm chunk {chunk_id}",
            params,
            true,
        )
        .await?;
        Ok(())
    }

    /// Drop every chunk belonging to a file that no longer exists.
    pub async fn remove_path(&self, path: &str) -> Result<()> {
        let ids: Vec<i64> = self
            .chunk_hashes(path)
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        self.remove_chunks(&ids).await
    }

    pub async fn chunk_count(&self) -> Result<usize> {
        let rows = self
            .script("?[count(chunk_id)] := *chunk{chunk_id}", BTreeMap::new(), false)
            .await?;
        Ok(rows
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0) as usize)
    }

    /// Hybrid search over indexed source.
    pub async fn search_chunks(
        &self,
        query_text: &str,
        query_vec: &[f32],
        k: usize,
    ) -> Result<Vec<CodeHit>> {
        let leg_k = (k * 4).max(20);
        let ef = (leg_k * 4).max(100);
        let mut params = BTreeMap::new();
        params.insert(
            "qv".to_string(),
            DataValue::List(query_vec.iter().map(|f| DataValue::from(*f as f64)).collect()),
        );
        params.insert("qt".to_string(), DataValue::from(query_text));
        let script = format!(
            r#"
            sem[id, score] := ~chunk:emb_idx{{ chunk_id: id | query: vec($qv), k: {leg_k},
                                               ef: {ef}, bind_distance: __d }}, score = -__d
            txt[id, score] := ~chunk:body_fts{{ chunk_id: id | query: $qt, k: {leg_k},
                                                bind_score: score }}
            combined[__lid, id, score] := sem[id, score], __lid = 'semantic'
            combined[__lid, id, score] := txt[id, score], __lid = 'text'
            fused[id, score] <~ ReciprocalRankFusion(combined[__lid, id, score], k: 60.0)
            ?[id, score, path, lang, start_line, end_line, body] :=
                fused[id, score],
                *chunk{{chunk_id: id, path, lang, start_line, end_line, body}}
            :order -score
            :limit {k}
            "#
        );
        let rows = self.script_owned(script, params, false).await?;
        Ok(rows
            .rows
            .iter()
            .filter_map(|r| {
                Some(CodeHit {
                    score: r[1].get_float()?,
                    path: r[2].get_str()?.to_owned(),
                    lang: r[3].get_str().unwrap_or_default().to_owned(),
                    start_line: r[4].get_int()? as u32,
                    end_line: r[5].get_int()? as u32,
                    body: r[6].get_str()?.to_owned(),
                })
            })
            .collect())
    }

    // ---- durability ------------------------------------------------------

    /// Logical, version-independent dump. The store is a projection: the
    /// canonical record is the session event log, but an export makes a
    /// storage-format break a recoverable event rather than data loss.
    pub async fn export_json(&self) -> Result<String> {
        let this = self.clone();
        let wrapped = tokio::task::spawn_blocking(move || {
            this.db
                .export_relations_str(r#"{"relations":["fact","chunk","meta"]}"#)
        })
        .await
        .map_err(|e| anyhow!("export panicked: {e}"))?;

        // `export_relations_str` wraps its payload as {"ok":…,"data":…} but
        // the importer expects the bare relation map, so unwrap here and let
        // export/import compose.
        let value: serde_json::Value =
            serde_json::from_str(&wrapped).context("export returned invalid JSON")?;
        if !value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            let message = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(anyhow!("export failed: {message}"));
        }
        let data = value
            .get("data")
            .ok_or_else(|| anyhow!("export payload had no `data` field"))?;
        Ok(serde_json::to_string(data)?)
    }

    pub async fn import_json(&self, payload: String) -> Result<()> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            this.db
                .import_relations_str_with_err(&payload)
                .map_err(|e| anyhow!("{e:?}"))
        })
        .await
        .map_err(|e| anyhow!("import panicked: {e}"))??;
        // Bulk paths bypass index maintenance and only warn via `log`.
        self.reindex().await
    }

    /// Rebuild every search index. Prefer this after any bulk import.
    pub async fn reindex(&self) -> Result<()> {
        for (relation, index, ddl) in INDEXES {
            let drop = format!("::{} drop {relation}:{index}", index_kind(index));
            let _ = self.script_owned(drop, BTreeMap::new(), true).await;
            self.script_owned(ddl.to_string(), BTreeMap::new(), true)
                .await
                .with_context(|| format!("rebuilding {relation}:{index}"))?;
        }
        Ok(())
    }
}

/// `::hnsw drop` / `::fts drop` / `::lsh drop` — a full drop-and-create is far
/// faster than `::reindex`, which walks the incremental per-row path while
/// holding the relation write lock.
/// Reduce arbitrary text to something the FTS query parser accepts.
///
/// `query:` on a search atom is not a string to match — it is an expression in
/// the index's own little query language, where `-`, `?`, `/`, `+`, `%` and `@`
/// are syntax. Callers pass whatever the user typed, so the parser has to be
/// treated as hostile input territory: keep alphanumerics and whitespace,
/// collapse the rest to spaces. `hot-reloads` becomes `hot reloads`, which the
/// tokenizer would have produced anyway; `crates/artist-cli/src/main.rs`
/// becomes its path segments.
///
/// Terms are joined with `OR` because the leg is **conjunctive by default**:
/// `canvas zzzznotpresent` matches nothing even when `canvas` matches, so a
/// query of more than a handful of words matches nothing at all. The
/// prompt-conditioned channel passes the user's entire message, which made the
/// BM25 half of hybrid retrieval effectively dead there — it could only fire
/// when the user happened to type a literal subset of a stored fact. BM25 still
/// ranks, so a document matching more terms scores higher; disjunction changes
/// what is *eligible*, not what wins.
///
/// Returns `None` when nothing searchable survives, so the caller can drop the
/// leg rather than send an empty query — which is itself a parse error.
fn fts_query(text: &str) -> Option<String> {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let terms: Vec<&str> = cleaned
        .split_whitespace()
        // A literal `AND`/`OR`/`NOT` in the user's text would become an
        // operator once interpolated, and two operators in a row is a parse
        // error. They are English stopwords, so dropping them costs no recall.
        .filter(|t| !matches!(t.to_ascii_uppercase().as_str(), "AND" | "OR" | "NOT"))
        .collect();
    if terms.is_empty() {
        return None;
    }
    Some(terms.join(" OR "))
}

#[cfg(test)]
mod tests {
    use super::fts_query;

    #[test]
    fn fts_query_survives_ordinary_user_text() {
        // Each of these previously failed the whole retrieval query.
        assert_eq!(fts_query("hot-reloads").as_deref(), Some("hot OR reloads"));
        assert_eq!(
            fts_query("artist-memory").as_deref(),
            Some("artist OR memory")
        );
        assert_eq!(
            fts_query("fix crates/artist-cli/src/main.rs").as_deref(),
            Some("fix OR crates OR artist OR cli OR src OR main OR rs")
        );
        assert_eq!(fts_query("C++ interop").as_deref(), Some("C OR interop"));
        assert_eq!(fts_query("50% faster").as_deref(), Some("50 OR faster"));
    }

    #[test]
    fn fts_query_drops_operator_words_and_empty_input() {
        assert_eq!(fts_query("tabs AND spaces").as_deref(), Some("tabs OR spaces"));
        assert_eq!(fts_query("").as_deref(), None);
        assert_eq!(fts_query("   ?!  ").as_deref(), None);
        assert_eq!(fts_query("AND OR NOT").as_deref(), None);
    }

    #[test]
    fn fts_query_keeps_non_ascii_words() {
        assert_eq!(fts_query("café — naïve").as_deref(), Some("café OR naïve"));
    }
}

fn index_kind(index: &str) -> &'static str {
    match index {
        "emb_idx" => "hnsw",
        _ => "fts",
    }
}
