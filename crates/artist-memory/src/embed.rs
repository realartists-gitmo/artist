//! Local embeddings via rten (pure-Rust ONNX).
//!
//! Nothing here is specific to one model. Everything that differs between
//! models — the instruction prefixes, the context ceiling, and whether a
//! post-graph projection is needed — lives in `embed.json` beside the weights,
//! because those are properties of the model rather than of this crate. Two
//! models are in play: CodeRankEmbed (the original, code-specialised) and
//! EmbeddingGemma (memory-specialised); see `scripts/export-embedding-model.sh`.
//!
//! Measured properties on CPU, which drive the choices below:
//!
//! * The graph declares int64 inputs but rten narrows them to **i32**, and its
//!   readers reject I64 outright — so tensors are built as `i32`.
//! * `sentence_embedding` is pooled **inside the graph** but **not
//!   L2-normalized**, so normalization happens here. Which pooling is in the
//!   graph is a property of the export: CodeRankEmbed pools CLS, EmbeddingGemma
//!   pools the masked mean. Keeping it in the graph is deliberate — masked mean
//!   pooling has a silent failure mode if padding is allowed to contribute.
//! * Throughput is ~6-7 sequences/sec at batch 32 with peak RSS ~1.1 GB. Batch
//!   is kept smaller by default to bound memory; a full repository index is a
//!   background job, never a blocking startup step.
//! * The published int8 export measured 0.909 mean cosine against fp32 and
//!   moved 14% of top-1 results, so fp32 is what we load. Note that finding is
//!   about *post-hoc* quantization; a quantization-aware-trained export is a
//!   different mechanism and would need measuring separately.

use anyhow::{Context, Result, anyhow};
use rten::{Model, NodeId};
use rten_tensor::NdTensor;
use rten_tensor::prelude::*;
use rten_text::tokenizer::{EncodeOptions, Tokenizer};
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;

/// The prefix CodeRankEmbed requires on *queries* only. Kept as a named
/// constant because getting it wrong is silent: omitting it, or applying it to
/// documents, measurably degrades retrieval. It is **not** a default — models
/// that want it say so in `embed.json`, and EmbeddingGemma measurably prefers
/// no instruction at all on memory retrieval.
pub const CODERANK_QUERY_PREFIX: &str = "Represent this query for searching relevant code: ";

/// Tokens per sequence when `embed.json` does not say.
///
/// Was 256, which truncated most real code chunks. That is worse under mean
/// pooling than under CLS pooling: a mean over a truncated window is a
/// different vector, where CLS at least summarises what it saw. Attention is
/// quadratic in length, so this is a real cost — but a chunk whose signature
/// and body are severed is not worth embedding at all.
pub const DEFAULT_MAX_TOKENS: usize = 512;

/// Sequences per forward pass. Small enough to bound peak RSS on a machine
/// under memory pressure; throughput is near-flat between 8 and 32.
pub const BATCH: usize = 8;

/// Model-side configuration, read from `embed.json` beside the weights.
///
/// Absent file means all defaults, which is a usable no-instruction model at
/// [`DEFAULT_MAX_TOKENS`] with no projection.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    /// Which model these vectors came from.
    ///
    /// Load-bearing under replication, and not covered by [`DEFAULT_MAX_TOKENS`]
    /// or by the schema's width check: two models can share a dimension and
    /// still embed into **different spaces**. Mixing their vectors in one index
    /// does not error — it silently returns bad neighbours, which is the worst
    /// failure mode available. A peer's vector is reusable only if this matches;
    /// otherwise the text has to be re-embedded.
    pub id: String,
    /// Instruction prepended to queries only.
    pub query_prefix: String,
    /// Instruction prepended to documents only. Rare; most models want none.
    pub document_prefix: String,
    /// Tokens per sequence before truncation. The ceiling is a model property
    /// (CodeRankEmbed 8192, EmbeddingGemma 2048), so it belongs here.
    pub max_tokens: usize,
    /// Optional projection applied after the graph: a row-major `[out][in]`
    /// f32 matrix, little-endian, named relative to the model directory.
    ///
    /// EmbeddingGemma's sentence-transformers head is two `Dense` layers with
    /// no bias and Identity activation, so 768→3072→768 composes into a single
    /// 768×768 matrix. Keeping it out of the graph is what makes Matryoshka
    /// truncation a *narrower matmul* rather than a post-hoc slice.
    pub dense: Option<String>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            query_prefix: String::new(),
            document_prefix: String::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
            dense: None,
        }
    }
}

/// A post-graph projection: `y[o] = sum_i x[i] * w[o][i]`, row-major.
struct Dense {
    /// Input width; must equal the graph's output width.
    cols: usize,
    w: Vec<f32>,
}

impl Dense {
    fn load(path: &Path, cols_hint: usize) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading projection {}", path.display()))?;
        if bytes.len() % 4 != 0 {
            return Err(anyhow!(
                "{} is {} bytes, not a whole number of f32",
                path.display(),
                bytes.len()
            ));
        }
        let w: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        if cols_hint == 0 || !w.len().is_multiple_of(cols_hint) {
            return Err(anyhow!(
                "{} holds {} values, not a multiple of the {cols_hint}-wide input",
                path.display(),
                w.len()
            ));
        }
        Ok(Self { cols: cols_hint, w })
    }

    /// Rows available, i.e. the widest embedding this projection can emit.
    fn rows(&self) -> usize {
        self.w.len() / self.cols
    }

    /// Project `x`, keeping only the first `out` rows. Matryoshka truncation is
    /// exactly this: the leading rows are the informative ones, so a narrower
    /// embedding is a smaller matmul rather than a slice of a larger one.
    fn apply(&self, x: &[f32], out: usize) -> Vec<f32> {
        (0..out)
            .map(|o| {
                let row = &self.w[o * self.cols..(o + 1) * self.cols];
                row.iter().zip(x).map(|(w, v)| w * v).sum()
            })
            .collect()
    }
}

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
    config: ModelConfig,
    dense: Option<Dense>,
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
            let tokenizer =
                Tokenizer::from_json(json).map_err(|e| anyhow!("building tokenizer: {e:?}"))?;
            *slot = Some((key, tokenizer));
        }
        let (_, tokenizer) = slot.as_ref().expect("tokenizer just installed");
        f(tokenizer)
    })
}

impl Embedder {
    /// Load a model directory containing `model.onnx` and `tokenizer.json`,
    /// plus an optional `embed.json` and the projection it names.
    ///
    /// rten has loaded `.onnx` directly since v0.23, so there is no conversion
    /// step and no Python anywhere in the build. Producing that `.onnx` is a
    /// separate, offline concern — see `scripts/export-embedding-model.sh`.
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

        let config = Self::read_config(dir)?;

        // The projection's input width is whatever the graph emits, which is
        // only known once a forward pass runs. `graph_dim` is that width when a
        // projection is present, and `dim` otherwise — see `forward`.
        let dense = match config.dense.as_deref() {
            Some(name) => {
                let path = dir.join(name);
                let graph_dim = Self::graph_output_width(&model).unwrap_or(dim);
                let d = Dense::load(&path, graph_dim)?;
                if dim > d.rows() {
                    return Err(anyhow!(
                        "{} projects to at most {} dims, but dim is {dim}",
                        path.display(),
                        d.rows()
                    ));
                }
                Some(d)
            }
            None => None,
        };

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
                config,
                dense,
            }),
        })
    }

    fn read_config(dir: &Path) -> Result<ModelConfig> {
        let path = dir.join("embed.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            // Absent is fine and means defaults; anything else is not, because
            // silently falling back would change retrieval behaviour invisibly.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ModelConfig::default()),
            Err(e) => Err(anyhow!("reading {}: {e}", path.display())),
        }
    }

    /// Trailing dimension of the graph's declared output shape, when static.
    fn graph_output_width(model: &Model) -> Option<usize> {
        let id = model.find_node("sentence_embedding")?;
        let info = model.node_info(id)?;
        let shape = info.shape()?;
        match shape.last()? {
            rten::Dimension::Fixed(n) => Some(*n),
            rten::Dimension::Symbolic(_) => None,
        }
    }

    /// Width of the vectors this embedder emits.
    pub fn dim(&self) -> usize {
        self.inner.dim
    }

    /// Model-side configuration in force, after defaults.
    pub fn config(&self) -> &ModelConfig {
        &self.inner.config
    }

    /// Identity of the embedding space these vectors live in.
    pub fn model_id(&self) -> &str {
        &self.inner.config.id
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
                let prefix = if is_query {
                    &self.config.query_prefix
                } else {
                    &self.config.document_prefix
                };
                let prefixed;
                let input = if prefix.is_empty() {
                    text.as_str()
                } else {
                    prefixed = format!("{prefix}{text}");
                    prefixed.as_str()
                };
                let options = EncodeOptions {
                    max_chunk_len: Some(self.config.max_tokens),
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

        // Pad to the longest sequence in this batch rather than to `max_tokens`:
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
        if rows == 0 || !data.len().is_multiple_of(rows) {
            return Err(anyhow!(
                "model returned {} values for {rows} rows",
                data.len()
            ));
        }
        let graph_dim = data.len() / rows;

        // Without a projection the graph must already emit the target width.
        // With one, the graph emits the projection's input width and `dim` is
        // the Matryoshka truncation applied on the way out.
        match &self.dense {
            None if graph_dim != self.dim => {
                return Err(anyhow!(
                    "model emits {graph_dim}-wide vectors but dim is {}; \
                     either set dim to match or give the model a projection",
                    self.dim
                ));
            }
            Some(d) if graph_dim != d.cols => {
                return Err(anyhow!(
                    "projection expects {}-wide input but the model emits {graph_dim}",
                    d.cols
                ));
            }
            _ => {}
        }

        Ok(data
            .chunks(graph_dim)
            .map(|row| {
                // Pooling is in-graph; the projection and L2 normalization are
                // not. Normalizing last matters: truncating a normalized vector
                // would leave it off the unit sphere.
                let mut v = match &self.dense {
                    Some(d) => d.apply(row, self.dim),
                    None => row.to_vec(),
                };
                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for x in v.iter_mut() {
                        *x /= norm;
                    }
                }
                v
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Row-major `[out][in]`, so `apply` must read rows contiguously. Getting
    /// this transposed produces plausible-looking vectors that are silently
    /// wrong, which is the whole reason it is tested rather than eyeballed.
    #[test]
    fn dense_applies_rows_not_columns() {
        // 3x2: out0 = x0, out1 = x1, out2 = x0 + x1.
        let d = Dense {
            cols: 2,
            w: vec![1.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        };
        assert_eq!(d.rows(), 3);
        assert_eq!(d.apply(&[2.0, 5.0], 3), vec![2.0, 5.0, 7.0]);
    }

    /// Matryoshka truncation is a narrower matmul, so the first `k` outputs
    /// must equal the first `k` of the full projection — not a slice taken
    /// after normalization, which would be off the unit sphere.
    #[test]
    fn dense_truncation_is_a_prefix_of_the_full_projection() {
        let d = Dense {
            cols: 2,
            w: vec![1.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        };
        let full = d.apply(&[2.0, 5.0], 3);
        let short = d.apply(&[2.0, 5.0], 2);
        assert_eq!(short, full[..2].to_vec());
    }

    #[test]
    fn dense_rejects_a_width_that_does_not_divide() {
        let dir = std::env::temp_dir().join("artist-embed-dense-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.bin");
        // Five f32s cannot be a whole number of 2-wide rows.
        let bytes: Vec<u8> = (0..5u32).flat_map(|i| (i as f32).to_le_bytes()).collect();
        std::fs::write(&path, bytes).unwrap();
        assert!(Dense::load(&path, 2).is_err());
        let _ = std::fs::remove_file(&path);
    }

    /// An absent `embed.json` must mean defaults, not an error — but a present
    /// and malformed one must fail loudly, because silently falling back would
    /// change retrieval behaviour with no signal.
    #[test]
    fn missing_config_is_defaults_and_bad_config_is_an_error() {
        let dir = std::env::temp_dir().join("artist-embed-config-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let cfg = Embedder::read_config(&dir).expect("absent config is fine");
        assert_eq!(cfg.max_tokens, DEFAULT_MAX_TOKENS);
        assert!(cfg.query_prefix.is_empty());
        assert!(cfg.dense.is_none());

        std::fs::write(dir.join("embed.json"), r#"{"max_tokens": 1024}"#).unwrap();
        let cfg = Embedder::read_config(&dir).expect("partial config uses defaults");
        assert_eq!(cfg.max_tokens, 1024);
        assert!(cfg.query_prefix.is_empty());

        std::fs::write(dir.join("embed.json"), "{ not json").unwrap();
        assert!(Embedder::read_config(&dir).is_err());

        // A typo'd key is an error rather than a silently ignored setting.
        std::fs::write(dir.join("embed.json"), r#"{"queryPrefix": "x"}"#).unwrap();
        assert!(Embedder::read_config(&dir).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
