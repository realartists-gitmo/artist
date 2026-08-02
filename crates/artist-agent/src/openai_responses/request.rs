use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Artist-owned create-response request. Provider extensions remain lossless in
/// `extra`, allowing this vendored boundary to evolve independently from Rig.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub model: String,
    pub input: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// A system prompt the provider is already holding, referenced instead of
    /// restated.
    ///
    /// Mutually exclusive with `instructions` in practice: the point is that
    /// the preamble stops being part of the request, so it can neither be
    /// re-uploaded every turn nor drift between turns. This is the protocol
    /// enforcing what [`crate::prefix::PrefixFreezer`] enforces in our own
    /// code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    /// Chain this request onto a response the provider is still holding,
    /// instead of restating the conversation.
    ///
    /// Only meaningful with `store: true`, and only where the endpoint retains
    /// responses — which the Codex backend does not. Populated from
    /// [`artist_session::ChainState`]; see that module for what invalidates a
    /// chain and why the local event log stays authoritative regardless.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_management: Vec<ContextManagement>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A reference to a stored prompt.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PromptRef {
    pub id: String,
    /// Pinned rather than floating: an unpinned reference would let the
    /// preamble change underneath a running session, which is the exact thing
    /// the prefix freezer exists to prevent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Reasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Opaque encrypted reasoning context returned by OpenAI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A context-management policy (including compaction policies). Kept open so
/// newly introduced policy fields survive deserialize/serialize cycles.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContextManagement {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
