use super::Request;
use rig_core::completion;

#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("Rig Responses conversion failed: {0}")]
    Rig(#[from] completion::CompletionError),
    #[error("Responses wire conversion failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// Convert Rig's portable messages and tools through its 0.41 Responses
/// mapping, then take ownership at Artist's wire boundary. This intentionally
/// retains Rig's handling of encrypted reasoning payloads and function call IDs.
impl TryFrom<(String, completion::CompletionRequest)> for Request {
    type Error = ConversionError;

    fn try_from(
        (model, request): (String, completion::CompletionRequest),
    ) -> Result<Self, Self::Error> {
        let upstream = rig_core::providers::openai::responses_api::CompletionRequest::try_from((
            model, request,
        ))?;
        Ok(serde_json::from_value(serde_json::to_value(upstream)?)?)
    }
}
