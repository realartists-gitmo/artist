//! The one-shot CLI consumer of the default host-side verb components.
//!
//! This module deliberately accepts a verb name plus a TOON value instead of
//! growing one bespoke Rust command implementation per verb. The typed verb
//! implementations remain the source of operation semantics.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use artist_kernel::Kernel;
use artist_kernel::VerbInvocationError;
use artist_wasm_verbs::ToonVerbHandler;
use artist_wasm_verbs::edit::{CompatibilityAnchorResolver, EditVerb, KernelEditor};
use artist_wasm_verbs::find::FffFindIndex;
use artist_wasm_verbs::find::FindVerb;
use artist_wasm_verbs::grep::GrepVerb;
use artist_wasm_verbs::move_::{KernelMover, MoveVerb};
use artist_wasm_verbs::read::{AnchoredLine, KernelReader, LineAddresser, ReadError, ReadVerb};
use artist_wasm_verbs::write::{KernelWriter, WriteVerb};
use serde_json::Value;

/// A temporary compatibility addresser for the default CLI path. The actual
/// Teca anchor implementation will replace this seam without changing the
/// CLI or read contract.
struct CompatibilityLineAddresser;

impl LineAddresser for CompatibilityLineAddresser {
    fn address_lines(&self, source: &str) -> Result<Vec<AnchoredLine>, ReadError> {
        Ok(source
            .lines()
            .enumerate()
            .map(|(index, content)| AnchoredLine {
                anchor: format!("a{index}"),
                content: content.to_string(),
            })
            .collect())
    }
}

pub struct ToolRunner {
    kernel: Arc<Kernel>,
}

impl ToolRunner {
    pub fn new(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        let kernel = Arc::new(Kernel::with_files_root(root.clone()));
        let find_index = FffFindIndex::start(
            root.clone(),
            artist_kernel::ResourceUri::root("files")?,
            Vec::new(),
        )
        .map_err(|error| anyhow!(error.to_string()))?;
        if !find_index.wait_for_scan(Duration::from_secs(30)) {
            return Err(anyhow!("filesystem index did not finish scanning"));
        }
        kernel.register_verb(ToonVerbHandler::new(
            "read",
            ReadVerb::new(
                KernelReader::new(Arc::clone(&kernel)),
                CompatibilityLineAddresser,
            ),
        ));
        kernel.register_verb(ToonVerbHandler::new(
            "write",
            WriteVerb::new(KernelWriter::new(Arc::clone(&kernel))),
        ));
        kernel.register_verb(ToonVerbHandler::new(
            "edit",
            EditVerb::new(
                KernelEditor::new(Arc::clone(&kernel)),
                CompatibilityAnchorResolver,
            ),
        ));
        kernel.register_verb(ToonVerbHandler::new(
            "move",
            MoveVerb::new(KernelMover::new(Arc::clone(&kernel))),
        ));
        kernel.register_verb(ToonVerbHandler::new(
            "find",
            FindVerb::new(find_index.clone()),
        ));
        kernel.register_verb(ToonVerbHandler::new(
            "grep",
            GrepVerb::new(find_index.clone()),
        ));
        Ok(Self { kernel })
    }

    /// Decode one scalar request or a list of requests and return the same
    /// shape of result envelope. A failed item is represented in-band and the
    /// boolean says whether every item succeeded.
    pub async fn call_toon(&self, verb: &str, payload: &str) -> anyhow::Result<(String, bool)> {
        let input: Value =
            toon_format::decode_default(payload).context("decode TOON tool input")?;
        let is_batch = input.is_array();
        let output = self.call_registered(verb, input).await?;
        let output = if is_batch {
            output
        } else {
            output
                .as_array()
                .and_then(|values| values.first())
                .cloned()
                .ok_or_else(|| anyhow!("tool returned no result"))?
        };
        let ok = if is_batch {
            output
                .as_array()
                .is_some_and(|values| values.iter().all(|value| value["ok"] == true))
        } else {
            output["ok"] == true
        };
        let encoded = toon_format::encode_default(&output).context("encode TOON tool output")?;
        Ok((encoded, ok))
    }

    async fn call_registered(&self, verb: &str, input: Value) -> anyhow::Result<Value> {
        let requests = match input {
            Value::Array(values) => values,
            value => vec![value],
        };
        if !self.kernel.verb_names().iter().any(|name| name == verb) {
            return Err(anyhow!("unknown tool {verb:?}"));
        }
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let request =
                toon_format::encode_default(&request).context("encode TOON tool request")?;
            let result = self.kernel.invoke_verb(verb, request.as_bytes()).await;
            results.push(match result {
                Ok(response) => {
                    let response: Value = toon_format::decode_default(
                        std::str::from_utf8(&response)
                            .context("TOON verb response is not UTF-8")?,
                    )
                    .context("decode TOON verb response")?;
                    serde_json::json!({ "ok": true, "response": response })
                }
                Err(error) => serde_json::json!({
                    "ok": false,
                    "error": format_invocation_error(error),
                }),
            });
        }
        Ok(Value::Array(results))
    }
}

fn format_invocation_error(error: VerbInvocationError) -> &'static str {
    match error {
        VerbInvocationError::InvalidArgument(_) => "invalid_argument",
        VerbInvocationError::NotFound(_) => "not_found",
        VerbInvocationError::Unsupported(_) => "unsupported",
        VerbInvocationError::PermissionDenied(_) => "permission_denied",
        VerbInvocationError::Conflict(_) => "conflict",
        VerbInvocationError::Aborted(_) => "aborted",
        VerbInvocationError::Internal(_) => "internal",
    }
}

pub fn validate_root(root: &Path) -> anyhow::Result<()> {
    if !root.is_dir() {
        return Err(anyhow!("tool root is not a directory: {}", root.display()));
    }
    Ok(())
}
