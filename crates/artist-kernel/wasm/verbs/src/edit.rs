//! Anchor-based replacement of existing resource spans.
//!
//! The resolver is deliberately a seam. The default compatibility resolver
//! understands the temporary `a0` line addresses and decimal line numbers;
//! Teca will replace it without changing the edit contract.

use std::sync::Arc;

use artist_kernel::{Kernel, ResourceError, ResourceErrorCode, ResourceUri};
use async_trait::async_trait;

use crate::host::{VerbError, VerbTool};

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct EditChange {
    pub anchor: String,
    pub replacement: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct EditRequest {
    pub uri: String,
    pub changes: Vec<EditChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    InvalidUri(String),
    InvalidChanges(String),
    StaleAnchor(String),
    InvalidUtf8,
    Resource(ResourceError),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUri(value) => write!(f, "invalid edit URI: {value}"),
            Self::InvalidChanges(value) => write!(f, "invalid edit changes: {value}"),
            Self::StaleAnchor(value) => write!(f, "stale edit anchor: {value}"),
            Self::InvalidUtf8 => write!(f, "edit target is not valid UTF-8"),
            Self::Resource(error) => write!(f, "edit failed: {error}"),
        }
    }
}

impl std::error::Error for EditError {}

#[async_trait]
pub trait ResourceEditor: Send + Sync {
    async fn read_all(&self, uri: &ResourceUri) -> Result<Vec<u8>, ResourceError>;
    async fn replace_all(&self, uri: &ResourceUri, content: &[u8]) -> Result<(), ResourceError>;
}

#[derive(Clone)]
pub struct KernelEditor {
    kernel: Arc<Kernel>,
}

impl KernelEditor {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

#[async_trait]
impl ResourceEditor for KernelEditor {
    async fn read_all(&self, uri: &ResourceUri) -> Result<Vec<u8>, ResourceError> {
        let mut content = Vec::new();
        let mut offset = 0;
        loop {
            let chunk = self.kernel.read_uri(uri, offset, 1024 * 1024).await?;
            if chunk.is_empty() {
                break;
            }
            offset += chunk.len() as u64;
            let done = chunk.len() < 1024 * 1024;
            content.extend_from_slice(&chunk);
            if done {
                break;
            }
        }
        Ok(content)
    }

    async fn replace_all(&self, uri: &ResourceUri, content: &[u8]) -> Result<(), ResourceError> {
        self.kernel.set_size_uri(uri, 0).await?;
        if !content.is_empty() {
            self.kernel.write_uri(uri, 0, content).await?;
        }
        Ok(())
    }
}

pub trait AnchorResolver: Send + Sync {
    fn resolve(&self, source: &str, anchor: &str) -> Result<(usize, usize), EditError>;
}

/// Temporary line resolver. Exact anchors win; decimal values are 1-based.
pub struct CompatibilityAnchorResolver;

impl AnchorResolver for CompatibilityAnchorResolver {
    fn resolve(&self, source: &str, anchor: &str) -> Result<(usize, usize), EditError> {
        let requested = anchor
            .strip_prefix('a')
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| {
                anchor
                    .parse::<usize>()
                    .ok()
                    .map(|line| line.saturating_sub(1))
            })
            .ok_or_else(|| EditError::StaleAnchor(anchor.to_string()))?;
        let mut start = 0;
        for (index, line) in source.split_inclusive('\n').enumerate() {
            let content_end = start + line.trim_end_matches(['\n', '\r']).len();
            if index == requested {
                return Ok((start, content_end));
            }
            start += line.len();
        }
        if requested == source.lines().count() && source.is_empty() {
            return Ok((0, 0));
        }
        Err(EditError::StaleAnchor(anchor.to_string()))
    }
}

pub struct EditVerb<E, R> {
    editor: E,
    resolver: R,
}

impl<E, R> EditVerb<E, R> {
    pub fn new(editor: E, resolver: R) -> Self {
        Self { editor, resolver }
    }
}

#[async_trait]
impl<E: ResourceEditor + 'static, R: AnchorResolver + 'static> VerbTool<EditRequest, ()>
    for EditVerb<E, R>
{
    fn name(&self) -> &str {
        "edit"
    }

    async fn call(&self, requests: Vec<EditRequest>) -> Vec<Result<(), VerbError>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            results.push(
                edit_resource(&self.editor, &self.resolver, request)
                    .await
                    .map_err(map_error),
            );
        }
        results
    }
}

pub async fn edit_resource<E: ResourceEditor, R: AnchorResolver>(
    editor: &E,
    resolver: &R,
    request: EditRequest,
) -> Result<(), EditError> {
    let uri = request
        .uri
        .parse::<ResourceUri>()
        .map_err(|error| EditError::InvalidUri(error.to_string()))?;
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err(EditError::InvalidUri(
            "edit target cannot contain selectors".into(),
        ));
    }
    if request.changes.is_empty() {
        return Err(EditError::InvalidChanges(
            "at least one change is required".into(),
        ));
    }
    let bytes = editor.read_all(&uri).await.map_err(EditError::Resource)?;
    let source = String::from_utf8(bytes).map_err(|_| EditError::InvalidUtf8)?;
    let mut spans = Vec::with_capacity(request.changes.len());
    for change in request.changes {
        let (start, end) = resolver.resolve(&source, &change.anchor)?;
        spans.push((start, end, change.replacement));
    }
    spans.sort_by_key(|(start, _, _)| *start);
    for pair in spans.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(EditError::InvalidChanges("changes overlap".into()));
        }
    }
    let mut output = source;
    for (start, end, replacement) in spans.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    editor
        .replace_all(&uri, output.as_bytes())
        .await
        .map_err(EditError::Resource)
}

fn map_error(error: EditError) -> VerbError {
    match error {
        EditError::InvalidUri(_) | EditError::InvalidChanges(_) | EditError::InvalidUtf8 => {
            VerbError::InvalidArgument
        }
        EditError::StaleAnchor(_) => VerbError::Conflict,
        EditError::Resource(error) => match error.code {
            ResourceErrorCode::NotFound => VerbError::NotFound,
            ResourceErrorCode::PermissionDenied => VerbError::PermissionDenied,
            ResourceErrorCode::IsDir => VerbError::InvalidArgument,
            ResourceErrorCode::Conflict => VerbError::Conflict,
            _ => VerbError::Internal,
        },
    }
}
