//! Tool payload codecs at the provider boundary.
//!
//! Components own the typed meaning of a tool. Provider adapters own the
//! native wire protocol. This module owns the translation between the
//! provider-neutral data model and the representation a model-facing tool
//! mode expects.

use serde_json::Value;
use thiserror::Error;

use crate::{ToolCall, ToolDefinition, ToolResult};

/// Wire representation used for tool payloads at a model provider boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolWireMode {
    /// A free-form custom tool whose payload is encoded as TOON.
    ToonCustom,
    /// A traditional function tool whose arguments are JSON.
    JsonFunction,
}

impl ToolWireMode {
    pub const fn is_toon(self) -> bool {
        matches!(self, Self::ToonCustom)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolPayloadError {
    #[error("failed to serialize {mode:?} tool payload: {message}")]
    Encode { mode: ToolWireMode, message: String },
    #[error("failed to parse {mode:?} tool payload: {message}")]
    Decode { mode: ToolWireMode, message: String },
}

/// Codec contract used by provider adapters. Components remain typed and do
/// not depend on a provider serialization format.
pub trait ToolPayloadCodec: Send + Sync {
    fn mode(&self) -> ToolWireMode;

    fn encode_value(&self, value: &Value) -> Result<String, ToolPayloadError>;

    fn decode_value(&self, payload: &str) -> Result<Value, ToolPayloadError>;

    fn encode_schema(&self, tool: &ToolDefinition) -> Result<String, ToolPayloadError> {
        self.encode_value(&tool.input_schema)
    }

    fn encode_input(&self, call: &ToolCall) -> Result<String, ToolPayloadError> {
        self.encode_value(&call.arguments)
    }

    fn decode_input(&self, payload: &str) -> Result<Value, ToolPayloadError> {
        self.decode_value(payload)
    }

    fn encode_output(&self, result: &ToolResult) -> Result<String, ToolPayloadError> {
        let value = serde_json::to_value(result).map_err(|error| ToolPayloadError::Encode {
            mode: self.mode(),
            message: error.to_string(),
        })?;
        self.encode_value(&value)
    }
}

/// The built-in codec implementation for the supported tool modes.
#[derive(Clone, Copy, Debug)]
pub struct StandardToolPayloadCodec {
    mode: ToolWireMode,
}

impl StandardToolPayloadCodec {
    pub const fn new(mode: ToolWireMode) -> Self {
        Self { mode }
    }

    pub const fn toon() -> Self {
        Self::new(ToolWireMode::ToonCustom)
    }

    pub const fn json() -> Self {
        Self::new(ToolWireMode::JsonFunction)
    }
}

impl ToolPayloadCodec for StandardToolPayloadCodec {
    fn mode(&self) -> ToolWireMode {
        self.mode
    }

    fn encode_value(&self, value: &Value) -> Result<String, ToolPayloadError> {
        match self.mode {
            ToolWireMode::JsonFunction => {
                serde_json::to_string(value).map_err(|error| ToolPayloadError::Encode {
                    mode: self.mode,
                    message: error.to_string(),
                })
            }
            ToolWireMode::ToonCustom => {
                toon_format::encode_default(value).map_err(|error| ToolPayloadError::Encode {
                    mode: self.mode,
                    message: error.to_string(),
                })
            }
        }
    }

    fn decode_value(&self, payload: &str) -> Result<Value, ToolPayloadError> {
        match self.mode {
            ToolWireMode::JsonFunction => {
                serde_json::from_str(payload).map_err(|error| ToolPayloadError::Decode {
                    mode: self.mode,
                    message: error.to_string(),
                })
            }
            ToolWireMode::ToonCustom => {
                toon_format::decode_default(payload).map_err(|error| ToolPayloadError::Decode {
                    mode: self.mode,
                    message: error.to_string(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContentPart;

    fn call() -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({
                "path": "src/lib.rs",
                "lines": [1, 2, 3]
            }),
        }
    }

    fn definition() -> ToolDefinition {
        ToolDefinition {
            name: "read_file".into(),
            description: Some("Read a file".into()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        }
    }

    #[test]
    fn toon_codec_round_trips_tool_input_and_schema() {
        let codec = StandardToolPayloadCodec::toon();
        let input = codec.encode_input(&call()).unwrap();
        let schema = codec.encode_schema(&definition()).unwrap();

        assert!(input.contains("path:"));
        assert!(schema.contains("type:"));
        assert_eq!(codec.decode_input(&input).unwrap(), call().arguments);
    }

    #[test]
    fn json_codec_remains_a_compatible_fallback() {
        let codec = StandardToolPayloadCodec::json();
        let input = codec.encode_input(&call()).unwrap();

        assert_eq!(input, serde_json::to_string(&call().arguments).unwrap());
        assert_eq!(codec.decode_input(&input).unwrap(), call().arguments);
    }

    #[test]
    fn output_codec_preserves_tool_result_envelope() {
        let result = ToolResult {
            call_id: "call-1".into(),
            content: vec![ContentPart::Text {
                text: "hello".into(),
            }],
            is_error: false,
        };
        let codec = StandardToolPayloadCodec::toon();
        let decoded = codec
            .decode_value(&codec.encode_output(&result).unwrap())
            .unwrap();

        assert_eq!(decoded["call_id"], "call-1");
        assert_eq!(decoded["is_error"], false);
        assert_eq!(decoded["content"][0]["type"], "text");
    }

    #[test]
    fn invalid_payload_reports_the_active_mode() {
        let error = StandardToolPayloadCodec::toon()
            .decode_input("not: [valid")
            .unwrap_err();

        assert!(matches!(
            error,
            ToolPayloadError::Decode {
                mode: ToolWireMode::ToonCustom,
                ..
            }
        ));
    }
}
