//! The resource-independent implementation of the `read` verb.
//!
//! This module owns URI/position/range semantics. Anchor generation is an
//! injected seam because the Teca addressing formula is intentionally not
//! selected yet.

use std::sync::Arc;

use artist_kernel::{Kernel, ResourceError, ResourceUri};
use async_trait::async_trait;

use crate::host::{VerbError, VerbTool};

const READ_CHUNK_SIZE: u32 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ReadRequest {
    pub uri: String,
    pub range: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct AnchoredLine {
    pub anchor: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ReadRange {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

impl ReadRange {
    pub fn parse(value: &str) -> Result<Self, ReadError> {
        let (start, end) = value
            .split_once("..")
            .ok_or_else(|| ReadError::InvalidRange(value.to_string()))?;
        let range = Self {
            start: parse_bound(start)?,
            end: parse_bound(end)?,
        };
        if let (Some(start), Some(end)) = (range.start, range.end)
            && start > end
        {
            return Err(ReadError::InvalidRange(value.to_string()));
        }
        Ok(range)
    }

    pub fn render(&self) -> String {
        format!(
            "{}..{}",
            render_lower_bound(self.start),
            render_upper_bound(self.end)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ReadResponse {
    pub uri: String,
    pub position: Option<String>,
    pub range: ReadRange,
    pub lines: Vec<AnchoredLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    InvalidUri(String),
    InvalidRange(String),
    InvalidPosition(String),
    PositionNotFound(String),
    Resource(ResourceError),
    InvalidUtf8,
    Addressing(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUri(value) => write!(f, "invalid resource URI: {value}"),
            Self::InvalidRange(value) => write!(f, "invalid read range: {value}"),
            Self::InvalidPosition(value) => write!(f, "invalid read position: {value}"),
            Self::PositionNotFound(value) => write!(f, "read position not found: {value}"),
            Self::Resource(error) => write!(f, "resource read failed: {error}"),
            Self::InvalidUtf8 => write!(f, "resource is not valid UTF-8"),
            Self::Addressing(error) => write!(f, "could not address resource lines: {error}"),
        }
    }
}

impl std::error::Error for ReadError {}

fn map_error(error: ReadError) -> VerbError {
    match error {
        ReadError::InvalidUri(_)
        | ReadError::InvalidRange(_)
        | ReadError::InvalidPosition(_)
        | ReadError::InvalidUtf8 => VerbError::InvalidArgument,
        ReadError::PositionNotFound(_) => VerbError::NotFound,
        ReadError::Resource(error) => match error.code {
            artist_kernel::ResourceErrorCode::NotFound => VerbError::NotFound,
            artist_kernel::ResourceErrorCode::PermissionDenied => VerbError::PermissionDenied,
            artist_kernel::ResourceErrorCode::Unsupported => VerbError::Unsupported,
            _ => VerbError::Internal,
        },
        ReadError::Addressing(_) => VerbError::Internal,
    }
}

/// The future Teca implementation belongs here. It must return one stable,
/// ordered address for each exact source line and resolve an address against
/// the current file state. Do not replace this seam with an Artist-owned hash
/// or line-number identity.
pub trait LineAddresser: Send + Sync {
    fn address_lines(&self, source: &str) -> Result<Vec<AnchoredLine>, ReadError>;
}

#[async_trait]
pub trait ResourceReader: Send + Sync {
    async fn read_all(&self, uri: &ResourceUri) -> Result<Vec<u8>, ReadError>;
}

/// Batch-native host implementation of the read verb. A WASM guest can use
/// the same WIT contract; this is the direct host path for default activation
/// and tests.
pub struct ReadVerb<R, A> {
    reader: R,
    addresser: A,
}

impl<R, A> ReadVerb<R, A> {
    pub fn new(reader: R, addresser: A) -> Self {
        Self { reader, addresser }
    }
}

#[async_trait]
impl<R, A> VerbTool<ReadRequest, ReadResponse> for ReadVerb<R, A>
where
    R: ResourceReader + 'static,
    A: LineAddresser + 'static,
{
    fn name(&self) -> &str {
        "read"
    }

    async fn call(&self, requests: Vec<ReadRequest>) -> Vec<Result<ReadResponse, VerbError>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            results.push(
                read(&self.reader, &self.addresser, request)
                    .await
                    .map_err(map_error),
            );
        }
        results
    }
}

#[derive(Clone)]
pub struct KernelReader {
    kernel: Arc<Kernel>,
}

impl KernelReader {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

#[async_trait]
impl ResourceReader for KernelReader {
    async fn read_all(&self, uri: &ResourceUri) -> Result<Vec<u8>, ReadError> {
        let mut bytes = Vec::new();
        let mut offset = 0;
        loop {
            let chunk = self
                .kernel
                .read_uri(uri, offset, READ_CHUNK_SIZE)
                .await
                .map_err(ReadError::Resource)?;
            if chunk.is_empty() {
                break;
            }
            offset = offset
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| ReadError::Addressing("resource is too large".into()))?;
            bytes.extend_from_slice(&chunk);
            if chunk.len() < READ_CHUNK_SIZE as usize {
                break;
            }
        }
        Ok(bytes)
    }
}

/// Execute one read request against a resource reader and line addresser.
pub async fn read<R: ResourceReader, A: LineAddresser>(
    reader: &R,
    addresser: &A,
    request: ReadRequest,
) -> Result<ReadResponse, ReadError> {
    let uri: ResourceUri = request
        .uri
        .parse::<ResourceUri>()
        .map_err(|error| ReadError::InvalidUri(error.to_string()))?;
    let base = uri.without_fragment();
    let bytes = reader.read_all(&base).await?;
    let source = std::str::from_utf8(&bytes).map_err(|_| ReadError::InvalidUtf8)?;
    let lines = addresser.address_lines(source)?;
    let position = uri.fragment().map(str::to_owned);
    let position_index = match position.as_deref() {
        None => 0,
        Some(fragment) => resolve_position(&lines, fragment)?,
    };
    let range = match request.range.as_deref() {
        Some(value) => ReadRange::parse(value)?,
        None => default_range(),
    };
    let start = range
        .start
        .map(|offset| position_index as i64 + offset)
        .unwrap_or(0)
        .max(0) as usize;
    let end = range
        .end
        .map(|offset| position_index as i64 + offset + 1)
        .map(|end| end.max(0) as usize)
        .unwrap_or(lines.len())
        .min(lines.len());
    Ok(ReadResponse {
        uri: base.to_string(),
        position,
        range,
        lines: lines.get(start..end).unwrap_or_default().to_vec(),
    })
}

/// Default context around the selected line. Keep this a function: it is the
/// deliberate replacement point for future token-aware budgeting.
fn default_range() -> ReadRange {
    // TODO: replace line-count context with a token-aware budget when the
    // read surface has a tokenizer policy.
    ReadRange {
        start: Some(-200),
        end: Some(200),
    }
}

fn resolve_position(lines: &[AnchoredLine], fragment: &str) -> Result<usize, ReadError> {
    if fragment.is_empty() {
        return Err(ReadError::InvalidPosition(fragment.to_string()));
    }
    if let Some(index) = lines.iter().position(|line| line.anchor == fragment) {
        return Ok(index);
    }
    // Compatibility fallback: line numbers are 1-based. Anchor identity wins
    // above, so numeric anchors remain valid if Teca ever emits one.
    if let Ok(line_number) = fragment.parse::<usize>() {
        if (1..=lines.len()).contains(&line_number) {
            return Ok(line_number - 1);
        }
    }
    let (anchor, offset) = split_position_offset(fragment)?;
    let base = lines
        .iter()
        .position(|line| line.anchor == anchor)
        .ok_or_else(|| ReadError::PositionNotFound(anchor.to_string()))?;
    let target = base as i64 + offset;
    if target < 0 || target >= lines.len() as i64 {
        return Err(ReadError::PositionNotFound(fragment.to_string()));
    }
    Ok(target as usize)
}

fn split_position_offset(value: &str) -> Result<(&str, i64), ReadError> {
    let split = value
        .char_indices()
        .rev()
        .find(|(_, character)| *character == '+' || *character == '-');
    let Some((index, _)) = split else {
        return Err(ReadError::PositionNotFound(value.to_string()));
    };
    if index == 0 {
        return Err(ReadError::InvalidPosition(value.to_string()));
    }
    let (anchor, offset) = value.split_at(index);
    let offset = offset
        .parse::<i64>()
        .map_err(|_| ReadError::InvalidPosition(value.to_string()))?;
    Ok((anchor, offset))
}

fn parse_bound(value: &str) -> Result<Option<i64>, ReadError> {
    if value.is_empty() || value == "-inf" || value == "+inf" || value == "inf" {
        return Ok(None);
    }
    value
        .parse::<i64>()
        .map(Some)
        .map_err(|_| ReadError::InvalidRange(value.to_string()))
}

fn render_lower_bound(value: Option<i64>) -> String {
    value
        .map(|value| format!("{value:+}"))
        .unwrap_or_else(|| "-inf".to_string())
}

fn render_upper_bound(value: Option<i64>) -> String {
    value
        .map(|value| format!("{value:+}"))
        .unwrap_or_else(|| "+inf".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestAddresser;

    impl LineAddresser for TestAddresser {
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

    struct TestReader;

    #[async_trait]
    impl ResourceReader for TestReader {
        async fn read_all(&self, _uri: &ResourceUri) -> Result<Vec<u8>, ReadError> {
            Ok(b"zero\none\ntwo\nthree\nfour\n".to_vec())
        }
    }

    #[test]
    fn ranges_accept_open_and_infinite_bounds() {
        assert_eq!(ReadRange::parse("..+4").unwrap().end, Some(4));
        assert_eq!(ReadRange::parse("-inf..+inf").unwrap().start, None);
        assert!(ReadRange::parse("+4..-4").is_err());
    }

    #[tokio::test]
    async fn read_defaults_to_top_with_two_hundred_line_context() {
        let response = read(
            &TestReader,
            &TestAddresser,
            ReadRequest {
                uri: "file:///note.txt".into(),
                range: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(response.position, None);
        assert_eq!(response.lines[0].anchor, "a0");
        assert_eq!(response.lines.len(), 5);
    }

    #[tokio::test]
    async fn read_resolves_anchor_offsets_and_relative_range() {
        let response = read(
            &TestReader,
            &TestAddresser,
            ReadRequest {
                uri: "file:///note.txt#a1+1".into(),
                range: Some("-1..+1".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(response.position, Some("a1+1".into()));
        assert_eq!(
            response
                .lines
                .iter()
                .map(|line| line.anchor.as_str())
                .collect::<Vec<_>>(),
            vec!["a1", "a2", "a3"]
        );
    }
}
