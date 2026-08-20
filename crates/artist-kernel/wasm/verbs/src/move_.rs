//! Implementation of the URI-shaped `move` verb.
//!
//! An omitted destination means delete. A supplied destination is interpreted
//! by the provider: existing directories receive the source basename, while a
//! nonexistent destination is the exact target URI.

use std::sync::Arc;

use artist_kernel::{Kernel, ResourceError, ResourceErrorCode, ResourceUri};
use async_trait::async_trait;

use crate::host::{VerbError, VerbTool};

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct MoveRequest {
    pub source: String,
    pub destination: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveError {
    InvalidUri(String),
    SelectorNotAllowed(String),
    Resource(ResourceError),
}

impl std::fmt::Display for MoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUri(value) => write!(f, "invalid resource URI: {value}"),
            Self::SelectorNotAllowed(value) => {
                write!(f, "move does not accept URI queries or fragments: {value}")
            }
            Self::Resource(error) => write!(f, "move failed: {error}"),
        }
    }
}

impl std::error::Error for MoveError {}

#[async_trait]
pub trait ResourceMover: Send + Sync {
    async fn move_resource(
        &self,
        source: &ResourceUri,
        destination: &ResourceUri,
    ) -> Result<(), ResourceError>;

    async fn delete_resource(&self, source: &ResourceUri) -> Result<(), ResourceError>;
}

#[derive(Clone)]
pub struct KernelMover {
    kernel: Arc<Kernel>,
}

impl KernelMover {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

#[async_trait]
impl ResourceMover for KernelMover {
    async fn move_resource(
        &self,
        source: &ResourceUri,
        destination: &ResourceUri,
    ) -> Result<(), ResourceError> {
        self.kernel.move_uri(source, destination).await
    }

    async fn delete_resource(&self, source: &ResourceUri) -> Result<(), ResourceError> {
        self.kernel.delete_uri(source).await
    }
}

/// Execute one move request.
pub async fn move_resource<M: ResourceMover>(
    mover: &M,
    request: MoveRequest,
) -> Result<(), MoveError> {
    let source = parse_mutation_uri(&request.source)?;
    match request.destination {
        Some(destination) => {
            let destination_uri = parse_mutation_uri(&destination)?;
            mover
                .move_resource(&source, &destination_uri)
                .await
                .map_err(MoveError::Resource)?;
            Ok(())
        }
        None => {
            mover
                .delete_resource(&source)
                .await
                .map_err(MoveError::Resource)?;
            Ok(())
        }
    }
}

pub struct MoveVerb<M> {
    mover: M,
}

impl<M> MoveVerb<M> {
    pub fn new(mover: M) -> Self {
        Self { mover }
    }
}

#[async_trait]
impl<M: ResourceMover + 'static> VerbTool<MoveRequest, ()> for MoveVerb<M> {
    fn name(&self) -> &str {
        "move"
    }

    async fn call(&self, requests: Vec<MoveRequest>) -> Vec<Result<(), VerbError>> {
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            results.push(move_resource(&self.mover, request).await.map_err(map_error));
        }
        results
    }
}

fn parse_mutation_uri(value: &str) -> Result<ResourceUri, MoveError> {
    let uri = value
        .parse::<ResourceUri>()
        .map_err(|error| MoveError::InvalidUri(error.to_string()))?;
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err(MoveError::SelectorNotAllowed(value.to_string()));
    }
    Ok(uri)
}

fn map_error(error: MoveError) -> VerbError {
    match error {
        MoveError::InvalidUri(_) | MoveError::SelectorNotAllowed(_) => VerbError::InvalidArgument,
        MoveError::Resource(error) => match error.code {
            ResourceErrorCode::NotFound => VerbError::NotFound,
            ResourceErrorCode::PermissionDenied => VerbError::PermissionDenied,
            ResourceErrorCode::Conflict => VerbError::Conflict,
            ResourceErrorCode::Unsupported => VerbError::Unsupported,
            _ => VerbError::Internal,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingMover {
        moves: Mutex<Vec<(String, String)>>,
        deletes: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ResourceMover for RecordingMover {
        async fn move_resource(
            &self,
            source: &ResourceUri,
            destination: &ResourceUri,
        ) -> Result<(), ResourceError> {
            self.moves
                .lock()
                .unwrap()
                .push((source.to_string(), destination.to_string()));
            Ok(())
        }

        async fn delete_resource(&self, source: &ResourceUri) -> Result<(), ResourceError> {
            self.deletes.lock().unwrap().push(source.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn destination_is_optional_delete_or_move() {
        let mover = RecordingMover::default();
        move_resource(
            &mover,
            MoveRequest {
                source: "/old.txt".into(),
                destination: Some("/new.txt".into()),
            },
        )
        .await
        .unwrap();

        move_resource(
            &mover,
            MoveRequest {
                source: "/new.txt".into(),
                destination: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            mover.deletes.lock().unwrap().as_slice(),
            &["files:///new.txt"]
        );
    }

    #[tokio::test]
    async fn mutation_selectors_are_rejected() {
        let error = move_resource(
            &RecordingMover::default(),
            MoveRequest {
                source: "files:///old.txt#anchor".into(),
                destination: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, MoveError::SelectorNotAllowed(_)));
    }
}
