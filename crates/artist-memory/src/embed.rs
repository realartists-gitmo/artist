//! Local embeddings via rten (pure-Rust ONNX) and CodeRankEmbed.
//!
//! Measured properties of this model on CPU, which drive the choices below:
//!
//! * The graph declares int64 inputs but rten narrows them to **i32**, and its
//!   readers reject I64 outright — so tensors are built as `i32`.
//! * `sentence_embedding` is **CLS-pooled inside the graph** but **not
//!   L2-normalized**, so normalization happens here.
//! * Throughput is ~6-7 sequences/sec at batch 32 with peak RSS ~1.1 GB. Batch
//!   is kept smaller by default to bound memory; a full repository index is a
//!   background job, never a blocking startup step.
//! * The published int8 export measured 0.909 mean cosine against fp32 and
//!   moved 14% of top-1 results, so fp32 is what we load.

use anyhow::{Context, Result, anyhow};
use rten::{Model, NodeId};
use rten_tensor::NdTensor;
use rten_tensor::prelude::*;
use rten_text::tokenizer::{EncodeOptions, Tokenizer};
use std::path::Path;
use std::sync::Arc;

/// CodeRankEmbed requires this prefix on *queries* only. Omitting it, or
/// applying it to documents, measurably degrades retrieval.
pub const QUERY_PREFIX: &str = "Represent this query for searching relevant code: ";

/// Tokens per sequence. Longer inputs are truncated by the tokenizer.
pub const MAX_TOKENS: usize = 256;

/// Sequences per forward pass. Small enough to bound peak RSS on a machine
/// under memory pressure; throughput is near-flat between 8 and 32.
pub const BATCH: usize = 8;

#[derive(Clone)]
pub struct Embedder {
    inner: Arc<Inner>,
}

struct Inner {
    model: Model,
    /// `rten_text::Tokenizer` is neither `Send` nor `Sync` — it holds
    /// `Box<dyn Model/Normalizer/PreTokenizer>`. So the *source* travels
    /// between threads and each blocking worker builds its own copy once, via
    /// the thread-local below. Tokio reuses blocking threads, so the parse cost
    /// is paid a handful of times per process rather than per call.
    tokenizer_json: Arc<str>,
    input_ids: NodeId,
    attention_mask: NodeId,
    sentence_embedding: NodeId,
    dim: usize,
}

thread_local! {
    static TOKENIZER: std::cell::RefCell<Option<(usize, Tokenizer)>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with this thread's tokenizer, building it on first use. Keyed by
/// the source pointer so a differently-configured embedder rebuilds rather
/// than silently reusing another model's vocabulary.
fn with_tokenizer<T>(json: &Arc<str>, f: impl FnOnce(&Tokenizer) -> Result<T>) -> Result<T> {
    let key = Arc::as_ptr(json) as *const u8 as usize;
    TOKENIZER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().is_none_or(|(k, _)| *k != key) {
            let tokenizer = Tokenizer::from_json(json)
                .map_err(|e| anyhow!("building tokenizer: {e:?}"))?;
            *slot = Some((key, tokenizer));
        }
        let (_, tokenizer) = slot.as_ref().expect("tokenizer just installed");
        f(tokenizer)
    })
}

impl Embedder {
    /// Load a model directory containing `model.onnx` and `tokenizer.json`.
    ///
    /// rten has loaded `.onnx` directly since v0.23, so there is no conversion
    /// step and no Python anywhere in the build.
    pub async fn load(dir: impl AsRef<Path>, dim: usize) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::load_blocking(&dir, dim))
            .await
            .map_err(|e| anyhow!("embedder load panicked: {e}"))?
    }

    fn load_blocking(dir: &Path, dim: usize) -> Result<Self> {
        let model_path = dir.join("model.onnx");
        let tokenizer_path = dir.join("tokenizer.json");
        let model = Model::load_file(&model_path)
            .map_err(|e| anyhow!("loading {}: {e}", model_path.display()))?;
        let tokenizer_json: Arc<str> = std::fs::read_to_string(&tokenizer_path)
            .with_context(|| format!("reading {}", tokenizer_path.display()))?
            .into();
        // Fail at load rather than on the first embed if the vocabulary is bad.
        Tokenizer::from_json(&tokenizer_json)
            .map_err(|e| anyhow!("parsing {}: {e:?}", tokenizer_path.display()))?;

        let node = |name: &str| {
            model
                .find_node(name)
                .ok_or_else(|| anyhow!("model has no node named {name}"))
        };
        Ok(Self {
            inner: Arc::new(Inner {
                input_ids: node("input_ids")?,
                attention_mask: node("attention_mask")?,
                sentence_embedding: node("sentence_embedding")?,
                model,
                tokenizer_json,
                dim,
            }),
        })
    }

    pub fn dim(&self) -> usize {
        self.inner.dim
    }

    /// Embed documents. Order is preserved.
    pub async fn embed_documents(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.embed(texts, false).await
    }

    /// Embed a single query, applying the required instruction prefix.
    pub async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let mut out = self.embed(vec![text.to_owned()], true).await?;
        out.pop()
            .ok_or_else(|| anyhow!("embedder returned no rows for a query"))
    }

    async fn embed(&self, texts: Vec<String>, is_query: bool) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let inner = Arc::clone(&self.inner);
        // rten runs its own internal rayon pool, so one blocking task already
        // fans out across cores. Issuing many concurrent inferences would
        // oversubscribe rather than speed anything up.
        tokio::task::spawn_blocking(move || inner.embed_blocking(&texts, is_query))
            .await
            .map_err(|e| anyhow!("embedding panicked: {e}"))?
    }
}

impl Inner {
    fn embed_blocking(&self, texts: &[String], is_query: bool) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            out.extend(self.forward(batch, is_query)?);
        }
        Ok(out)
    }

    fn forward(&self, batch: &[String], is_query: bool) -> Result<Vec<Vec<f32>>> {
        let encoded = with_tokenizer(&self.tokenizer_json, |tokenizer| {
            let mut encoded = Vec::with_capacity(batch.len());
            for text in batch {
                let prefixed;
                let input = if is_query {
                    prefixed = format!("{QUERY_PREFIX}{text}");
                    prefixed.as_str()
                } else {
                    text.as_str()
                };
                let options = EncodeOptions {
                    max_chunk_len: Some(MAX_TOKENS),
                    ..Default::default()
                };
                let ids = tokenizer
                    .encode(input, Some(options))
                    .map_err(|e| anyhow!("tokenizing: {e:?}"))?
                    .token_ids()
                    .to_vec();
                encoded.push(ids);
            }
            Ok(encoded)
        })?;

        // Pad to the longest sequence in this batch rather than to MAX_TOKENS:
        // attention is quadratic in length, so short batches run much faster.
        let width = encoded.iter().map(|e| e.len()).max().unwrap_or(1).max(1);
        let rows = encoded.len();

        let mut ids = NdTensor::<i32, 2>::zeros([rows, width]);
        let mut mask = NdTensor::<i32, 2>::zeros([rows, width]);
        for (row, tokens) in encoded.iter().enumerate() {
            for (col, token) in tokens.iter().enumerate() {
                ids[[row, col]] = *token as i32;
                mask[[row, col]] = 1;
            }
        }

        let outputs = self
            .model
            .run(
                vec![
                    (self.input_ids, ids.view().into()),
                    (self.attention_mask, mask.view().into()),
                ],
                &[self.sentence_embedding],
                None,
            )
            .map_err(|e| anyhow!("inference failed: {e}"))?;

        let tensor: rten_tensor::Tensor<f32> = outputs
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("model returned no outputs"))?
            .try_into()
            .context("sentence_embedding was not an f32 tensor")?;

        let data = tensor.to_vec();
        if data.len() != rows * self.dim {
            return Err(anyhow!(
                "expected {rows}x{} embedding values, got {}",
                self.dim,
                data.len()
            ));
        }

        Ok(data
            .chunks(self.dim)
            .map(|row| {
                // CLS pooling is in-graph; L2 normalization is not.
                let norm = row.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 0.0 {
                    row.iter().map(|v| v / norm).collect()
                } else {
                    row.to_vec()
                }
            })
            .collect())
    }
}
