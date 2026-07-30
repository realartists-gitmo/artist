//! Artist-owned wire types for the OpenAI Responses API.
//!
//! Adapted from Rig's `providers::openai::responses_api` module (rig-core 0.41.0),
//! licensed under the MIT License. These types are intentionally not connected
//! to the production provider path yet.

mod conversion;
mod output;
mod request;

pub use conversion::ConversionError;
pub use output::OutputItem;
pub use request::{ContextManagement, Reasoning, Request};

#[cfg(test)]
mod tests;
