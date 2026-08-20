//! Content search over file-shaped resources.
//!
//! The model-facing query is literal text. FFF is an implementation detail;
//! its fuzzy/constraint grammar is deliberately not exposed here.

use artist_kernel::ResourceUri;
use async_trait::async_trait;
use fff_search::{GrepMode, GrepSearchOptions};

use crate::find::FffFindIndex;
use crate::host::{VerbError, VerbTool};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct GrepRequest {
    pub uri: String,
    pub query: String,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct GrepMatch {
    pub uri: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct GrepResponse {
    pub matches: Vec<GrepMatch>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrepError {
    InvalidUri(String),
    InvalidQuery(String),
    InvalidCursor(String),
    OutsideRoot(String),
    Backend(String),
}

impl std::fmt::Display for GrepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUri(value) => write!(f, "invalid grep URI: {value}"),
            Self::InvalidQuery(value) => write!(f, "invalid grep query: {value}"),
            Self::InvalidCursor(value) => write!(f, "invalid grep cursor: {value}"),
            Self::OutsideRoot(value) => write!(f, "grep URI is outside the indexed root: {value}"),
            Self::Backend(value) => write!(f, "grep backend failed: {value}"),
        }
    }
}

impl std::error::Error for GrepError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrepPage {
    pub matches: Vec<GrepMatch>,
    pub next_offset: Option<usize>,
}

#[async_trait]
pub trait GrepBackend: Send + Sync {
    async fn grep(
        &self,
        root: &ResourceUri,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<GrepPage, GrepError>;
}

pub struct GrepVerb<B> {
    backend: B,
}

impl<B> GrepVerb<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: GrepBackend + 'static> VerbTool<GrepRequest, GrepResponse> for GrepVerb<B> {
    fn name(&self) -> &str {
        "grep"
    }

    async fn call(&self, requests: Vec<GrepRequest>) -> Vec<Result<GrepResponse, VerbError>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let result = async {
                let root = parse_root(&request.uri)?;
                if request.query.trim().is_empty() {
                    return Err(GrepError::InvalidQuery("query must not be empty".into()));
                }
                let limit = request
                    .limit
                    .map(|value| value as usize)
                    .unwrap_or(DEFAULT_LIMIT);
                if limit == 0 || limit > MAX_LIMIT {
                    return Err(GrepError::InvalidQuery(
                        "limit must be between 1 and 500".into(),
                    ));
                }
                let offset = request
                    .cursor
                    .as_deref()
                    .unwrap_or("0")
                    .parse()
                    .map_err(|_| GrepError::InvalidCursor(request.cursor.unwrap_or_default()))?;
                let page = self
                    .backend
                    .grep(&root, &request.query, offset, limit)
                    .await?;
                Ok(GrepResponse {
                    matches: page.matches,
                    next_cursor: page.next_offset.map(|value| value.to_string()),
                })
            }
            .await
            .map_err(map_error);
            results.push(result);
        }
        results
    }
}

fn parse_root(value: &str) -> Result<ResourceUri, GrepError> {
    let uri = value
        .parse::<ResourceUri>()
        .map_err(|error| GrepError::InvalidUri(error.to_string()))?;
    if uri.scheme() != "files" || !uri.authority().is_empty() {
        return Err(GrepError::InvalidUri(value.into()));
    }
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err(GrepError::InvalidUri(
            "grep roots cannot contain selectors".into(),
        ));
    }
    Ok(uri)
}

fn map_error(error: GrepError) -> VerbError {
    match error {
        GrepError::InvalidUri(_)
        | GrepError::InvalidQuery(_)
        | GrepError::InvalidCursor(_)
        | GrepError::OutsideRoot(_) => VerbError::InvalidArgument,
        GrepError::Backend(_) => VerbError::Internal,
    }
}

#[async_trait]
impl GrepBackend for FffFindIndex {
    async fn grep(
        &self,
        root: &ResourceUri,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<GrepPage, GrepError> {
        let requested_root = relative_root(root, &self.uri_root)?;
        let guard = self
            .picker
            .read()
            .map_err(|error| GrepError::Backend(error.to_string()))?;
        let picker = guard
            .as_ref()
            .ok_or_else(|| GrepError::Backend("FFF index is not initialized".into()))?;
        let result = picker.grep_raw(
            query,
            &GrepSearchOptions {
                mode: GrepMode::PlainText,
                file_offset: offset,
                page_limit: limit,
                ..Default::default()
            },
        );
        let mut matches = Vec::with_capacity(result.matches.len());
        for item in result.matches {
            let relative = result.files[item.file_index].relative_path(picker);
            if !within(&relative, requested_root) || self.excluded(&relative) {
                continue;
            }
            let uri = uri_for_relative(root, &relative, requested_root)?;
            matches.push(GrepMatch {
                uri: uri
                    .parse::<ResourceUri>()
                    .map_err(|error| GrepError::Backend(error.to_string()))?
                    .with_fragment(item.line_number.to_string())
                    .to_string(),
                content: item.line_content,
            });
        }
        Ok(GrepPage {
            matches,
            next_offset: (result.next_file_offset != 0).then_some(result.next_file_offset),
        })
    }
}

fn relative_root<'a>(uri: &'a ResourceUri, base: &ResourceUri) -> Result<&'a str, GrepError> {
    let base = base.path().trim_matches('/');
    let requested = uri.path().trim_matches('/');
    if base.is_empty() {
        return Ok(requested);
    }
    if requested == base {
        return Ok("");
    }
    requested
        .strip_prefix(base)
        .and_then(|value| value.strip_prefix('/'))
        .ok_or_else(|| GrepError::OutsideRoot(uri.to_string()))
}

fn within(path: &str, root: &str) -> bool {
    root.is_empty() || path == root || path.starts_with(&format!("{root}/"))
}

fn uri_for_relative(
    root: &ResourceUri,
    relative: &str,
    relative_root: &str,
) -> Result<String, GrepError> {
    let child = if relative_root.is_empty() {
        relative
    } else {
        relative
            .strip_prefix(relative_root)
            .and_then(|value| value.strip_prefix('/'))
            .ok_or_else(|| GrepError::Backend("FFF returned an inconsistent path".into()))?
    };
    let mut uri = root.clone();
    for segment in child.split('/') {
        uri = uri
            .child(segment)
            .map_err(|error| GrepError::Backend(error.to_string()))?;
    }
    Ok(uri.to_string())
}
