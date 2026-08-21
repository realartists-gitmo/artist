//! ACP edge envelopes.
//!
//! ACP is kept at the transport edge: the daemon core continues to speak in
//! Artist's canonical request/response/event frames. This module owns only
//! JSON-RPC framing and leaves ACP method payload evolution to the adapter.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::RpcFrame;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum AcpId {
    String(String),
    Number(i64),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AcpRequest {
    pub jsonrpc: String,
    pub id: AcpId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AcpNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum AcpSessionUpdate {
    AgentMessageChunk {
        content: AcpContentChunk,
    },
    AgentThoughtChunk {
        content: AcpContentChunk,
    },
    ToolCall {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        title: String,
        status: String,
        #[serde(rename = "rawInput")]
        raw_input: Value,
    },
    ToolCallUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        status: String,
        #[serde(rename = "rawOutput", skip_serializing_if = "Option::is_none")]
        raw_output: Option<Value>,
    },
    #[serde(rename = "_artist_event")]
    ArtistEvent {
        event_type: String,
        payload: Value,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AcpContentChunk {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct AcpResponse {
    pub jsonrpc: String,
    pub id: AcpId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AcpError>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AcpError {
    pub code: i64,
    pub message: String,
}

pub struct AcpAdapter;

impl AcpAdapter {
    pub fn request(frame: &RpcFrame) -> Option<AcpRequest> {
        let RpcFrame::Request { id, method, params } = frame else {
            return None;
        };
        Some(AcpRequest {
            jsonrpc: "2.0".into(),
            id: AcpId::String(id.clone()),
            method: method.clone(),
            params: Some(params.clone()),
        })
    }

    pub fn notification(frame: &RpcFrame) -> Option<AcpNotification> {
        let RpcFrame::Event {
            stream_id,
            sequence,
            event_type,
            payload,
        } = frame
        else {
            return None;
        };
        let update = match event_type.as_str() {
            "agent.text" => AcpSessionUpdate::AgentMessageChunk {
                content: AcpContentChunk {
                    kind: "text".into(),
                    text: payload
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                },
            },
            "agent.reasoning" => AcpSessionUpdate::AgentThoughtChunk {
                content: AcpContentChunk {
                    kind: "text".into(),
                    text: payload
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                },
            },
            "agent.model_event" => match payload.pointer("/event/type").and_then(Value::as_str) {
                Some("text_delta") => AcpSessionUpdate::AgentMessageChunk {
                    content: AcpContentChunk {
                        kind: "text".into(),
                        text: payload
                            .pointer("/event/text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    },
                },
                Some("reasoning_delta") => AcpSessionUpdate::AgentThoughtChunk {
                    content: AcpContentChunk {
                        kind: "text".into(),
                        text: payload
                            .pointer("/event/text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    },
                },
                Some("tool_call_delta") => AcpSessionUpdate::ToolCall {
                    tool_call_id: payload
                        .pointer("/event/id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    title: payload
                        .pointer("/event/name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .into(),
                    status: "in_progress".into(),
                    raw_input: payload
                        .pointer("/event/arguments_delta")
                        .cloned()
                        .unwrap_or(Value::Null),
                },
                _ => AcpSessionUpdate::ArtistEvent {
                    event_type: event_type.clone(),
                    payload: payload.clone(),
                },
            },
            "agent.tool_requested" => AcpSessionUpdate::ToolCall {
                tool_call_id: payload
                    .pointer("/call/id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                title: payload
                    .pointer("/call/name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .into(),
                status: "in_progress".into(),
                raw_input: payload
                    .pointer("/call/arguments")
                    .cloned()
                    .unwrap_or(Value::Null),
            },
            "agent.tool_completed" => AcpSessionUpdate::ToolCallUpdate {
                tool_call_id: payload
                    .pointer("/result/call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                status: if payload
                    .pointer("/result/is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "failed"
                } else {
                    "completed"
                }
                .into(),
                raw_output: Some(
                    payload
                        .pointer("/result/content")
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
            },
            _ => AcpSessionUpdate::ArtistEvent {
                event_type: event_type.clone(),
                payload: payload.clone(),
            },
        };
        Some(AcpNotification {
            jsonrpc: "2.0".into(),
            method: "session/update".into(),
            params: Some(serde_json::json!({
                "sessionId": stream_id,
                "update": update,
                "_meta": {"artistSequence": sequence},
            })),
        })
    }

    pub fn cancel(session_id: impl Into<String>) -> AcpNotification {
        AcpNotification {
            jsonrpc: "2.0".into(),
            method: "session/cancel".into(),
            params: Some(serde_json::json!({"sessionId": session_id.into()})),
        }
    }

    pub fn response(frame: &RpcFrame) -> Option<AcpResponse> {
        let RpcFrame::Response { id, result, error } = frame else {
            return None;
        };
        Some(AcpResponse {
            jsonrpc: "2.0".into(),
            id: AcpId::String(id.clone()),
            result: result.clone(),
            error: error.as_ref().map(|error| AcpError {
                code: -32000,
                message: format!("{}: {}", error.code, error.message),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_canonical_requests_and_events_to_json_rpc_edges() {
        let request = RpcFrame::request("7", "session/prompt", serde_json::json!({"text": "hi"}));
        assert_eq!(
            AcpAdapter::request(&request).unwrap(),
            AcpRequest {
                jsonrpc: "2.0".into(),
                id: AcpId::String("7".into()),
                method: "session/prompt".into(),
                params: Some(serde_json::json!({"text": "hi"})),
            }
        );

        let event = RpcFrame::Event {
            stream_id: "s".into(),
            sequence: 4,
            event_type: "agent.text".into(),
            payload: serde_json::json!({"text": "hello"}),
        };
        let notification = AcpAdapter::notification(&event).unwrap();
        assert_eq!(notification.method, "session/update");
        let params = notification.params.unwrap();
        assert_eq!(params["sessionId"], "s");
        assert_eq!(params["update"]["sessionUpdate"], "agent_message_chunk");
        assert_eq!(params["update"]["content"]["text"], "hello");
        assert_eq!(AcpAdapter::cancel("s").method, "session/cancel");
    }
}
