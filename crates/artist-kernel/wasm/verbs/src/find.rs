//! The model-facing `find` verb and its resident FFF-backed implementation.
//!
//! FFF is deliberately behind this boundary. The model supplies one search
//! string; it does not receive FFF's query grammar or its scoring model.

use std::path::PathBuf;
use std::time::Duration;

use artist_kernel::ResourceUri;
use async_trait::async_trait;
use fff_query_parser::{Constraint, FFFQuery, FuzzyQuery, glob_detect::has_wildcards};
use fff_search::{
    FFFMode, FilePicker, FilePickerOptions, FuzzySearchOptions, MixedItemRef, PaginationArgs,
    SharedFilePicker, SharedFrecency,
};

use crate::host::{VerbError, VerbTool};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct FindRequest {
    pub uri: String,
    pub query: String,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindKind {
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct FindResult {
    pub uri: String,
    pub kind: FindKind,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct FindResponse {
    pub results: Vec<FindResult>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindError {
    InvalidUri(String),
    UnsupportedNamespace(String),
    InvalidQuery(String),
    InvalidCursor(String),
    OutsideRoot(String),
    Backend(String),
}

impl std::fmt::Display for FindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUri(value) => write!(f, "invalid find URI: {value}"),
            Self::UnsupportedNamespace(value) => {
                write!(f, "find does not support namespace: {value}")
            }
            Self::InvalidQuery(value) => write!(f, "invalid find query: {value}"),
            Self::InvalidCursor(value) => write!(f, "invalid find cursor: {value}"),
            Self::OutsideRoot(value) => write!(f, "find URI is outside the indexed root: {value}"),
            Self::Backend(value) => write!(f, "find backend failed: {value}"),
        }
    }
}

impl std::error::Error for FindError {}

#[async_trait]
pub trait FindBackend: Send + Sync {
    async fn find(
        &self,
        root: &ResourceUri,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<FindPage, FindError>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindPage {
    pub results: Vec<FindResult>,
    pub next_offset: Option<usize>,
}

pub struct FindVerb<B> {
    backend: B,
}

impl<B> FindVerb<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<B: FindBackend + 'static> VerbTool<FindRequest, FindResponse> for FindVerb<B> {
    fn name(&self) -> &str {
        "find"
    }

    async fn call(&self, requests: Vec<FindRequest>) -> Vec<Result<FindResponse, VerbError>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let result = async {
                let root = parse_search_root(&request.uri)?;
                let limit = request
                    .limit
                    .map(|value| value as usize)
                    .unwrap_or(DEFAULT_LIMIT);
                if limit == 0 || limit > MAX_LIMIT {
                    return Err(FindError::InvalidQuery(
                        "limit must be between 1 and 500".into(),
                    ));
                }
                if request.query.trim().is_empty() {
                    return Err(FindError::InvalidQuery("query must not be empty".into()));
                }
                let offset = parse_cursor(request.cursor.as_deref())?;
                let page = self
                    .backend
                    .find(&root, &request.query, offset, limit)
                    .await?;
                Ok(FindResponse {
                    results: page.results,
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

fn parse_search_root(value: &str) -> Result<ResourceUri, FindError> {
    let uri = value
        .parse::<ResourceUri>()
        .map_err(|error| FindError::InvalidUri(error.to_string()))?;
    if uri.scheme() != "file" || !uri.authority().is_empty() {
        return Err(FindError::UnsupportedNamespace(value.into()));
    }
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err(FindError::InvalidUri(
            "find roots cannot contain queries or fragments".into(),
        ));
    }
    Ok(uri)
}

fn parse_cursor(value: Option<&str>) -> Result<usize, FindError> {
    value
        .unwrap_or("0")
        .parse()
        .map_err(|_| FindError::InvalidCursor(value.unwrap_or_default().into()))
}

fn map_error(error: FindError) -> VerbError {
    match error {
        FindError::InvalidUri(_)
        | FindError::UnsupportedNamespace(_)
        | FindError::InvalidQuery(_)
        | FindError::InvalidCursor(_)
        | FindError::OutsideRoot(_) => VerbError::InvalidArgument,
        FindError::Backend(_) => VerbError::Internal,
    }
}

/// A long-lived FFF index for one host filesystem root.
#[derive(Clone)]
pub struct FffFindIndex {
    pub(crate) picker: SharedFilePicker,
    pub(crate) host_root: PathBuf,
    pub(crate) uri_root: ResourceUri,
    pub(crate) exclusions: Vec<PathBuf>,
}

impl FffFindIndex {
    pub fn start(
        host_root: impl Into<PathBuf>,
        uri_root: ResourceUri,
        exclusions: Vec<PathBuf>,
    ) -> Result<Self, FindError> {
        let host_root = host_root.into();
        let picker = SharedFilePicker::default();
        FilePicker::new_with_shared_state(
            picker.clone(),
            SharedFrecency::default(),
            FilePickerOptions {
                base_path: host_root.to_string_lossy().into_owned(),
                mode: FFFMode::Ai,
                enable_content_indexing: false,
                watch: true,
                ..Default::default()
            },
        )
        .map_err(|error| FindError::Backend(error.to_string()))?;
        Ok(Self {
            picker,
            host_root,
            uri_root,
            exclusions,
        })
    }

    pub fn wait_for_scan(&self, timeout: Duration) -> bool {
        self.picker.wait_for_scan(timeout)
    }

    fn relative_root<'a>(&self, uri: &'a ResourceUri) -> Result<&'a str, FindError> {
        let base = self.uri_root.path().trim_matches('/');
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
            .ok_or_else(|| FindError::OutsideRoot(uri.to_string()))
    }

    pub(crate) fn excluded(&self, relative: &str) -> bool {
        let candidate = self.host_root.join(relative);
        self.exclusions
            .iter()
            .any(|excluded| candidate == *excluded || candidate.starts_with(excluded))
    }
}

#[async_trait]
impl FindBackend for FffFindIndex {
    async fn find(
        &self,
        root: &ResourceUri,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<FindPage, FindError> {
        let relative_root = self.relative_root(root)?;
        let guard = self
            .picker
            .read()
            .map_err(|error| FindError::Backend(error.to_string()))?;
        let picker = guard
            .as_ref()
            .ok_or_else(|| FindError::Backend("FFF index is not initialized".into()))?;

        let fff_query = if has_wildcards(query) {
            FFFQuery {
                raw_query: query,
                constraints: vec![Constraint::Glob(query)],
                fuzzy_query: FuzzyQuery::Empty,
                location: None,
            }
        } else {
            FFFQuery {
                raw_query: query,
                constraints: vec![],
                fuzzy_query: FuzzyQuery::Text(query),
                location: None,
            }
        };

        let mut backend_offset = offset;
        let mut results = Vec::with_capacity(limit);
        let mut next_offset = None;
        while results.len() < limit {
            let raw = picker.fuzzy_search_mixed(
                &fff_query,
                None,
                FuzzySearchOptions {
                    pagination: PaginationArgs {
                        offset: backend_offset,
                        limit,
                    },
                    ..Default::default()
                },
            );
            let raw_len = raw.items.len();
            let raw_total = raw.total_matched;
            let mut consumed = 0usize;
            for (item, score) in raw.items.into_iter().zip(raw.scores.into_iter()) {
                consumed += 1;
                let (relative, kind) = match item {
                    MixedItemRef::File(file) => (file.relative_path(picker), FindKind::File),
                    MixedItemRef::Dir(dir) => (dir.relative_path(picker), FindKind::Directory),
                };
                let relative = relative.trim_end_matches('/');
                if !within(relative, relative_root) || self.excluded(relative) {
                    continue;
                }
                let result_uri = uri_for_relative(root, relative, relative_root)?;
                results.push((result_uri, kind, score.total));
                if results.len() == limit {
                    break;
                }
            }
            backend_offset = backend_offset.saturating_add(consumed);
            if consumed < raw_len {
                next_offset = Some(backend_offset);
                break;
            }
            if raw_len < limit || backend_offset >= raw_total {
                next_offset = None;
                break;
            }
            next_offset = Some(backend_offset);
        }

        results.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
        Ok(FindPage {
            results: results
                .into_iter()
                .map(|(uri, kind, _score)| FindResult { uri, kind })
                .collect(),
            next_offset,
        })
    }
}

fn within(path: &str, root: &str) -> bool {
    root.is_empty() || path == root || path.starts_with(&format!("{root}/"))
}

fn uri_for_relative(
    root: &ResourceUri,
    relative: &str,
    relative_root: &str,
) -> Result<String, FindError> {
    let child = if relative_root.is_empty() {
        relative
    } else {
        relative
            .strip_prefix(relative_root)
            .and_then(|value| value.strip_prefix('/'))
            .ok_or_else(|| FindError::Backend("FFF returned an inconsistent path".into()))?
    };
    let mut uri = root.clone();
    for segment in child.split('/') {
        uri = uri
            .child(segment)
            .map_err(|error| FindError::Backend(error.to_string()))?;
    }
    Ok(uri.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fake;

    #[async_trait]
    impl FindBackend for Fake {
        async fn find(
            &self,
            root: &ResourceUri,
            query: &str,
            _offset: usize,
            _limit: usize,
        ) -> Result<FindPage, FindError> {
            Ok(FindPage {
                results: vec![FindResult {
                    uri: format!("{root}/main.rs"),
                    kind: FindKind::File,
                }],
                next_offset: if query == "again" { Some(1) } else { None },
            })
        }
    }

    #[tokio::test]
    async fn find_preserves_request_order_and_opaque_cursor() {
        let verb = FindVerb::new(Fake);
        let results = verb
            .call(vec![FindRequest {
                uri: "src".into(),
                query: "again".into(),
                limit: Some(10),
                cursor: None,
            }])
            .await;
        let response = results[0].as_ref().unwrap();
        assert_eq!(response.next_cursor.as_deref(), Some("1"));
    }

    #[test]
    fn scope_and_relative_uri_are_strict() {
        assert!(within("src/lib.rs", "src"));
        assert!(!within("src-old/lib.rs", "src"));
        let root = ResourceUri::from_path("src").unwrap();
        assert_eq!(
            uri_for_relative(&root, "src/lib.rs", "src").unwrap(),
            "file:///src/lib.rs"
        );
    }

    #[test]
    fn fff_backend_finds_files_and_directories_under_a_uri_root() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let host_root = std::env::temp_dir().join(format!("artist-find-{suffix}"));
        fs::create_dir_all(host_root.join("src/nested")).unwrap();
        fs::write(host_root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(host_root.join("src/nested/lib.rs"), "pub fn x() {}\n").unwrap();
        fs::write(host_root.join("README.md"), "readme\n").unwrap();

        let index =
            FffFindIndex::start(&host_root, ResourceUri::root("file").unwrap(), vec![]).unwrap();
        assert!(index.wait_for_scan(Duration::from_secs(10)));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let page = runtime
            .block_on(index.find(&ResourceUri::from_path("src").unwrap(), "*.rs", 0, 20))
            .unwrap();

        assert!(page.results.iter().any(|result| {
            result.uri == "file:///src/main.rs" && result.kind == FindKind::File
        }));
        assert!(page.results.iter().any(|result| {
            result.uri == "file:///src/nested/lib.rs" && result.kind == FindKind::File
        }));
        assert!(
            !page
                .results
                .iter()
                .any(|result| result.uri == "file:///README.md")
        );
        let directories = runtime
            .block_on(index.find(&ResourceUri::from_path("src").unwrap(), "nested", 0, 20))
            .unwrap();
        assert!(directories.results.iter().any(|result| {
            result.uri == "file:///src/nested" && result.kind == FindKind::Directory
        }));

        drop(index);
        fs::remove_dir_all(host_root).unwrap();
    }
}
