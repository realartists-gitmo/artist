//! The one-shot CLI consumer of the default host-side verb components.
//!
//! This module deliberately accepts a verb name plus a TOON value instead of
//! growing one bespoke Rust command implementation per verb. The typed verb
//! implementations remain the source of operation semantics.

use anyhow::{Context, anyhow};
use artist_component::{
    ComponentHost, ProfileDocument, UrlCompositionSource, install_profile_view, install_prompt_view,
};
use artist_component::{ComponentToolRegistry, ToolError};
use artist_kernel::Kernel;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ToolRunner {
    host: ComponentHost,
    tools: ComponentToolRegistry,
    global_profile_root: PathBuf,
    local_profile_root: PathBuf,
}

impl ToolRunner {
    pub fn component_host(&self) -> &ComponentHost {
        &self.host
    }
}

impl ToolRunner {
    pub async fn new(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        let kernel = Arc::new(Kernel::with_files_root(root.clone()));
        let global = extension_root()?;
        let global_profile_root = global.join(".artist/profile");
        let local_profile_root = root.join(".artist/profile");
        install_prompt_view(
            &kernel,
            global.join(".artist/prompt"),
            root.join(".artist/prompt"),
        );
        install_profile_view(
            &kernel,
            global_profile_root.clone(),
            local_profile_root.clone(),
        );
        let local = root.join(".artist/url");
        let host = ComponentHost::start(kernel, UrlCompositionSource::new(global, local))
            .await
            .context("start component host")?;
        let tools = host.tools();
        if tools.names().is_empty() {
            return Err(anyhow!("URL composition registered no model-facing tools"));
        }
        Ok(Self {
            host,
            tools,
            global_profile_root,
            local_profile_root,
        })
    }

    pub fn load_profile(&self, name: &str) -> anyhow::Result<ProfileDocument> {
        ProfileDocument::load(&self.global_profile_root, &self.local_profile_root, name)
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
        if !self.tools.names().iter().any(|name| name == verb) {
            return Err(anyhow!("unknown tool {verb:?}"));
        }
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let request =
                toon_format::encode_default(&request).context("encode TOON tool request")?;
            let result = self.tools.invoke(verb, request.as_bytes()).await;
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

fn format_invocation_error(error: ToolError) -> &'static str {
    match error {
        ToolError::InvalidArgument(_) => "invalid_argument",
        ToolError::NotFound(_) => "not_found",
        ToolError::Unsupported(_) => "unsupported",
        ToolError::PermissionDenied(_) => "permission_denied",
        ToolError::Conflict(_) => "conflict",
        ToolError::Aborted(_) => "aborted",
        ToolError::Unavailable(_) => "unavailable",
        ToolError::Internal(_) => "internal",
    }
}

pub fn validate_root(root: &Path) -> anyhow::Result<()> {
    if !root.is_dir() {
        return Err(anyhow!("tool root is not a directory: {}", root.display()));
    }
    Ok(())
}

fn extension_root() -> anyhow::Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("ARTIST_EXTENSIONS_DIR") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("extensions"));
            if let Some(parent) = parent.parent() {
                candidates.push(parent.join("extensions"));
            }
        }
    }
    if let Ok(current) = std::env::current_dir() {
        for ancestor in current.ancestors() {
            candidates.push(ancestor.join("extensions"));
        }
    }
    candidates.dedup();
    candidates.into_iter().find(|path| path.is_dir()).ok_or_else(|| anyhow!(
        "no extension directory found; set ARTIST_EXTENSIONS_DIR or install extensions beside the Artist executable"
    ))
}
