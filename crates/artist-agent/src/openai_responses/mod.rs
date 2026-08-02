//! Artist-owned wire types for the OpenAI Responses API.
//!
//! Adapted from Rig's `providers::openai::responses_api` module (rig-core 0.41.0),
//! licensed under the MIT License. These types are intentionally not connected
//! to the production provider path yet.

mod conversion;
mod output;
mod request;
mod transport;

pub use conversion::ConversionError;
pub use output::OutputItem;
pub use request::{ContextManagement, PromptRef, Reasoning, Request};
pub use transport::{
    ArtistOpenAiModel, Client as ArtistOpenAiClient, Credentials, Response, StreamResponse,
};

/// Only explicit client capability/route failures permit local fallback.
pub(crate) fn is_unsupported_compaction(error: &rig_core::completion::CompletionError) -> bool {
    let text = error.to_string();
    ["HTTP 400", "HTTP 404", "HTTP 422"]
        .iter()
        .any(|status| text.contains(status))
}

#[cfg(test)]
mod tests;
