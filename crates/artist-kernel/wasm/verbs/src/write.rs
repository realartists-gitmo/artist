//! Whole-resource replacement for the model-facing `write` verb.
//!
//! This verb intentionally has no position, range, patch, or edit mode. A
//! request supplies the complete new UTF-8 content of one resource. Missing
//! regular files are created; existing regular files are truncated first.

use std::sync::Arc;

use artist_kernel::{Kernel, NodeKind, ResourceError, ResourceErrorCode, ResourceUri};
use async_trait::async_trait;

use crate::host::{VerbError, VerbTool};

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct WriteRequest {
    pub uri: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    InvalidUri(String),
    SelectorNotAllowed(String),
    Resource(ResourceError),
    TooLarge,
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUri(value) => write!(f, "invalid resource URI: {value}"),
            Self::SelectorNotAllowed(value) => {
                write!(f, "write does not accept URI queries or fragments: {value}")
            }
            Self::Resource(error) => write!(f, "write failed: {error}"),
            Self::TooLarge => write!(f, "write content exceeds the kernel write capacity"),
        }
    }
}

impl std::error::Error for WriteError {}

#[async_trait]
pub trait ResourceWriter: Send + Sync {
    async fn attrs(&self, uri: &ResourceUri)
    -> Result<artist_kernel::ProviderAttrs, ResourceError>;
    async fn create_file(&self, uri: &ResourceUri) -> Result<(), ResourceError>;
    async fn set_size(&self, uri: &ResourceUri, size: u64) -> Result<(), ResourceError>;
    async fn write(&self, uri: &ResourceUri, content: &[u8]) -> Result<u32, ResourceError>;
}

#[derive(Clone)]
pub struct KernelWriter {
    kernel: Arc<Kernel>,
}

impl KernelWriter {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

#[async_trait]
impl ResourceWriter for KernelWriter {
    async fn attrs(
        &self,
        uri: &ResourceUri,
    ) -> Result<artist_kernel::ProviderAttrs, ResourceError> {
        self.kernel.attrs_uri(uri).await
    }

    async fn create_file(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        self.kernel.create_file_uri(uri).await.map(|_| ())
    }

    async fn set_size(&self, uri: &ResourceUri, size: u64) -> Result<(), ResourceError> {
        self.kernel.set_size_uri(uri, size).await
    }

    async fn write(&self, uri: &ResourceUri, content: &[u8]) -> Result<u32, ResourceError> {
        self.kernel.write_uri(uri, 0, content).await
    }
}

pub struct WriteVerb<W> {
    writer: W,
}

impl<W> WriteVerb<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
}

#[async_trait]
impl<W: ResourceWriter + 'static> VerbTool<WriteRequest, ()> for WriteVerb<W> {
    fn name(&self) -> &str {
        "write"
    }

    async fn call(&self, requests: Vec<WriteRequest>) -> Vec<Result<(), VerbError>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            results.push(
                write_resource(&self.writer, request)
                    .await
                    .map_err(map_error),
            );
        }
        results
    }
}

pub async fn write_resource<W: ResourceWriter>(
    writer: &W,
    request: WriteRequest,
) -> Result<(), WriteError> {
    let uri = parse_uri(&request.uri)?;
    let content = request.content.into_bytes();
    if content.len() > u32::MAX as usize {
        return Err(WriteError::TooLarge);
    }

    match writer.attrs(&uri).await {
        Ok(attrs) => {
            if attrs.kind == NodeKind::Directory {
                return Err(WriteError::Resource(ResourceError::new(
                    ResourceErrorCode::IsDir,
                    "cannot write a directory",
                )));
            }
            writer
                .set_size(&uri, 0)
                .await
                .map_err(WriteError::Resource)?;
        }
        Err(error) if error.code == ResourceErrorCode::NotFound => writer
            .create_file(&uri)
            .await
            .map_err(WriteError::Resource)?,
        Err(error) => return Err(WriteError::Resource(error)),
    }

    if !content.is_empty() {
        writer
            .write(&uri, &content)
            .await
            .map_err(WriteError::Resource)?;
    }
    Ok(())
}

fn parse_uri(value: &str) -> Result<ResourceUri, WriteError> {
    let uri = value
        .parse::<ResourceUri>()
        .map_err(|error| WriteError::InvalidUri(error.to_string()))?;
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err(WriteError::SelectorNotAllowed(value.to_string()));
    }
    Ok(uri)
}

fn map_error(error: WriteError) -> VerbError {
    match error {
        WriteError::InvalidUri(_) | WriteError::SelectorNotAllowed(_) | WriteError::TooLarge => {
            VerbError::InvalidArgument
        }
        WriteError::Resource(error) => match error.code {
            ResourceErrorCode::NotFound => VerbError::NotFound,
            ResourceErrorCode::PermissionDenied => VerbError::PermissionDenied,
            ResourceErrorCode::Conflict => VerbError::Conflict,
            ResourceErrorCode::Unsupported => VerbError::Unsupported,
            ResourceErrorCode::IsDir => VerbError::InvalidArgument,
            _ => VerbError::Internal,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingWriter {
        files: Mutex<Vec<String>>,
        sizes: Mutex<Vec<(String, u64)>>,
        writes: Mutex<Vec<(String, Vec<u8>)>>,
    }

    #[async_trait]
    impl ResourceWriter for RecordingWriter {
        async fn attrs(
            &self,
            _uri: &ResourceUri,
        ) -> Result<artist_kernel::ProviderAttrs, ResourceError> {
            Err(ResourceError::new(ResourceErrorCode::NotFound, "missing"))
        }

        async fn create_file(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
            self.files.lock().unwrap().push(uri.to_string());
            Ok(())
        }

        async fn set_size(&self, uri: &ResourceUri, size: u64) -> Result<(), ResourceError> {
            self.sizes.lock().unwrap().push((uri.to_string(), size));
            Ok(())
        }

        async fn write(&self, uri: &ResourceUri, content: &[u8]) -> Result<u32, ResourceError> {
            self.writes
                .lock()
                .unwrap()
                .push((uri.to_string(), content.to_vec()));
            Ok(content.len() as u32)
        }
    }

    #[tokio::test]
    async fn missing_resource_is_created_and_replaced_whole() {
        let writer = RecordingWriter::default();
        write_resource(
            &writer,
            WriteRequest {
                uri: "notes.txt".into(),
                content: "hello".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            writer.files.lock().unwrap().as_slice(),
            &["file:///notes.txt"]
        );
        assert_eq!(writer.writes.lock().unwrap()[0].1, b"hello");
    }

    #[tokio::test]
    async fn selectors_are_rejected() {
        let error = write_resource(
            &RecordingWriter::default(),
            WriteRequest {
                uri: "notes.txt?kind".into(),
                content: "hello".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, WriteError::SelectorNotAllowed(_)));
    }
}
