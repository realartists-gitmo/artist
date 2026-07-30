use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// A response output item classified for callers while retaining its complete
/// wire representation. In particular, unknown and compaction items are never
/// discarded, and message `phase` remains in the raw object.
#[derive(Clone, Debug, PartialEq)]
pub enum OutputItem {
    Message(Value),
    Reasoning(Value),
    FunctionCall(Value),
    Compaction(Value),
    Unknown(Value),
}

impl OutputItem {
    pub fn wire(&self) -> &Value {
        match self {
            Self::Message(v)
            | Self::Reasoning(v)
            | Self::FunctionCall(v)
            | Self::Compaction(v)
            | Self::Unknown(v) => v,
        }
    }

    /// Assistant phase (`commentary`, `final`, etc.) when supplied by the API.
    pub fn phase(&self) -> Option<&str> {
        match self {
            Self::Message(v) => v.get("phase").and_then(Value::as_str),
            _ => None,
        }
    }
}

impl Serialize for OutputItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.wire().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OutputItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(match value.get("type").and_then(Value::as_str) {
            Some("message") => Self::Message(value),
            Some("reasoning") => Self::Reasoning(value),
            Some("function_call") => Self::FunctionCall(value),
            Some("compaction") => Self::Compaction(value),
            _ => Self::Unknown(value),
        })
    }
}
