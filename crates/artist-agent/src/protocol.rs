//! Transport-neutral NDJSON frames for daemon clients.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RpcFrame {
    Request {
        id: String,
        method: String,
        params: Value,
    },
    Response {
        id: String,
        result: Option<Value>,
        error: Option<RpcError>,
    },
    Event {
        stream_id: String,
        sequence: u64,
        event_type: String,
        payload: Value,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("invalid NDJSON frame: {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("NDJSON frame exceeds the {MAX_FRAME_BYTES}-byte limit")]
    TooLarge,
    #[error("NDJSON frame must contain exactly one line")]
    MultipleLines,
    #[error("response must contain exactly one of result or error")]
    InvalidResponse,
}

/// Maximum serialized RPC frame accepted by the daemon transport. Large
/// resources belong in the resource/session stores, not in an unbounded line.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

impl RpcFrame {
    pub fn request(id: impl Into<String>, method: impl Into<String>, params: Value) -> Self {
        Self::Request {
            id: id.into(),
            method: method.into(),
            params,
        }
    }

    pub fn response(id: impl Into<String>, result: Value) -> Self {
        Self::Response {
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    pub fn error(
        id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Response {
            id: id.into(),
            result: None,
            error: Some(RpcError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }

    pub fn to_line(&self) -> Result<String, FrameError> {
        let invalid_response = match self {
            Self::Response { result, error, .. } => result.is_some() == error.is_some(),
            _ => false,
        };
        if invalid_response {
            return Err(FrameError::InvalidResponse);
        }
        Ok(format!("{}\n", serde_json::to_string(self)?))
    }

    pub fn from_line(line: &str) -> Result<Self, FrameError> {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.len() > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge);
        }
        if trimmed.contains(['\r', '\n']) {
            return Err(FrameError::MultipleLines);
        }
        let frame: Self = serde_json::from_str(trimmed)?;
        let invalid_response = match &frame {
            Self::Response { result, error, .. } => result.is_some() == error.is_some(),
            _ => false,
        };
        if invalid_response {
            return Err(FrameError::InvalidResponse);
        }
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_event_round_trip_as_one_line() {
        let frames = [
            RpcFrame::request("1", "session/prompt", serde_json::json!({"text": "hi"})),
            RpcFrame::Event {
                stream_id: "s".into(),
                sequence: 2,
                event_type: "agent.text".into(),
                payload: serde_json::json!({"text": "hello"}),
            },
        ];
        for frame in frames {
            let line = frame.to_line().unwrap();
            assert_eq!(line.matches('\n').count(), 1);
            assert_eq!(RpcFrame::from_line(&line).unwrap(), frame);
        }
    }

    #[test]
    fn response_requires_result_or_error() {
        let frame = RpcFrame::Response {
            id: "1".into(),
            result: None,
            error: None,
        };
        assert!(matches!(frame.to_line(), Err(FrameError::InvalidResponse)));
        assert!(matches!(
            RpcFrame::from_line(
                "{\"kind\":\"response\",\"id\":\"1\",\"result\":{},\"error\":{\"code\":\"x\",\"message\":\"y\"}}"
            ),
            Err(FrameError::InvalidResponse)
        ));
    }

    #[test]
    fn oversized_frames_are_rejected_before_json_parsing() {
        let line = format!(
            "{{\"kind\":\"request\",\"id\":\"1\",\"method\":\"x\",\"params\":\"{}\"}}",
            "x".repeat(MAX_FRAME_BYTES)
        );
        assert!(matches!(
            RpcFrame::from_line(&line),
            Err(FrameError::TooLarge)
        ));
    }
}
