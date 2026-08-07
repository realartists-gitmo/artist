//! Modern MCP `server/discover` support at the HTTP boundary.
//!
//! The 2026-07-28 MCP revision makes `server/discover` a mandatory RPC: a
//! client asks what protocol versions, capabilities, and identity a server
//! offers before doing anything else. OpenAI's tunnel control plane probes it
//! when a connector is created or validated, and it fails connector setup when
//! it errors.
//!
//! rmcp 2.2 speaks the older initialize-based (legacy) era and has no handler
//! for `server/discover`, so an incoming probe falls through to its stateful
//! POST path and comes back as HTTP 422. That is exactly the failure the
//! tunnel relay logs as `rpc_method=server/discover status_code=422`, which the
//! ChatGPT UI surfaces as "Error creating connector".
//!
//! This module presents the endpoint as a dual-era server:
//!
//! - `server/discover` is intercepted and answered at the HTTP boundary with
//!   what this server genuinely offers. It advertises the modern `2026-07-28`
//!   revision **and** the legacy initialize-based revisions, so a modern
//!   client (OpenAI's runtime) and the older tunnel probe both accept it.
//! - Requests that name the modern revision (via the `MCP-Protocol-Version`
//!   header or `params._meta.io.modelcontextprotocol/protocolVersion` in the
//!   body) are routed to a stateless rmcp instance (`json_response`), which
//!   builds a fresh server per request exactly as the modern spec expects.
//! - Everything else — the legacy `initialize` handshake and per-session tool
//!   calls, including the SSE stream and session termination verbs — is routed
//!   to the original stateful rmcp instance, so existing sessions behave
//!   exactly as before.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::Request;
use axum::http::Response;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::Service;

/// The protocol revisions this endpoint advertises and can serve.
///
/// The modern `2026-07-28` revision comes first: rmcp 2.2's stateless mode
/// (`stateful_mode: false` + `json_response: true`) serves its requests,
/// building a fresh server per request. The initialize-based revisions that
/// follow are served by the original stateful rmcp instance, whose `LATEST`
/// is 2025-11-25.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2026-07-28",
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

/// The first revision served by the modern stateless instance. Anything at or
/// above this in the protocol version is a modern request.
pub const MODERN_MIN_VERSION: &str = "2026-07-28";

/// How long a client may cache this discovery response, in milliseconds.
const CACHE_TTL_MS: u64 = 3_600_000;

/// A serializable, cacheable `DiscoverResult` describing this endpoint.
#[derive(Clone)]
pub struct DiscoverReply {
    result: Arc<Value>,
}

impl DiscoverReply {
    pub fn new(server_title: String, server_description: String, instructions: String) -> Self {
        let result = json!({
            "resultType": "complete",
            "supportedVersions": SUPPORTED_PROTOCOL_VERSIONS,
            "capabilities": {
                "tools": {},
            },
            "instructions": instructions,
            "ttlMs": CACHE_TTL_MS,
            "cacheScope": "public",
            "_meta": {
                "io.modelcontextprotocol/serverInfo": {
                    "name": server_title,
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "artist": {
                    "description": server_description,
                },
            },
        });
        Self {
            result: Arc::new(result),
        }
    }

    /// If `body` is a `server/discover` JSON-RPC request, answer it. Returns
    /// `None` for every other method so the request can be forwarded.
    fn answer(&self, body: &[u8]) -> Option<Response<Body>> {
        let message: Value = serde_json::from_slice(body).ok()?;
        if message.get("method").and_then(Value::as_str) != Some("server/discover") {
            return None;
        }
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let protocol_version = request_protocol_version(&message);
        let response = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": (*self.result).clone(),
        });
        let body = Body::from(serde_json::to_vec(&response).expect("serialize discover reply"));
        Some(
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .header("mcp-protocol-version", protocol_version)
                .body(body)
                .expect("valid discover response"),
        )
    }
}

/// The requested protocol version, when the probe names one; otherwise the
/// version this reply is expressed against.
fn request_protocol_version(message: &Value) -> &str {
    message
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
        .filter(|version| version.starts_with("20") && version.contains('-'))
        .unwrap_or("2026-07-28")
}

/// A [`tower::Service`] that answers `server/discover` and then routes every
/// other request to the appropriate rmcp instance: modern stateless requests
/// to `modern`, everything else (the legacy `initialize` handshake, per-session
/// tool calls, SSE and session-termination verbs) to `legacy`.
#[derive(Clone)]
pub struct McpGatewayService<L, M> {
    legacy: L,
    modern: M,
    reply: DiscoverReply,
}

impl<L, M> McpGatewayService<L, M> {
    pub fn new(legacy: L, modern: M, reply: DiscoverReply) -> Self {
        Self {
            legacy,
            modern,
            reply,
        }
    }
}

impl<L, M> Service<Request<Body>> for McpGatewayService<L, M>
where
    L: Service<Request<Body>, Error = Infallible> + Clone + Send + Sync + 'static,
    M: Service<Request<Body>, Error = Infallible> + Clone + Send + Sync + 'static,
    L::Response: IntoResponse,
    M::Response: IntoResponse,
    L::Future: Send + 'static,
    M::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let _ = self.legacy.poll_ready(cx);
        let _ = self.modern.poll_ready(cx);
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let mut legacy = self.legacy.clone();
        let mut modern = self.modern.clone();
        let reply = self.reply.clone();
        Box::pin(async move {
            let (parts, body) = request.into_parts();
            let bytes = match BodyExt::collect(body).await {
                Ok(collected) => collected.to_bytes(),
                Err(_) => {
                    return Ok(Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::from("failed to read request body"))
                        .expect("valid error response"));
                }
            };
            if let Some(response) = reply.answer(&bytes) {
                return Ok(response);
            }
            let message: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let request = Request::from_parts(parts, Body::from(bytes));
            let response = if is_modern_request(&request.headers(), &message) {
                modern
                    .call(request)
                    .await
                    .expect("modern MCP service is infallible")
                    .into_response()
            } else {
                legacy
                    .call(request)
                    .await
                    .expect("legacy MCP service is infallible")
                    .into_response()
            };
            Ok(response)
        })
    }
}

/// Is this request spoken in the modern (2026-07-28 and later) stateless era?
///
/// A request is modern when it names a modern revision, either in the
/// `MCP-Protocol-Version` header or in the body's
/// `params._meta.io.modelcontextprotocol/protocolVersion`. ISO-8601 protocol
/// versions compare lexicographically, so `>= MODERN_MIN_VERSION` picks up the
/// modern revision and any newer one.
fn is_modern_request(headers: &axum::http::HeaderMap, message: &Value) -> bool {
    let header_modern = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|version| version >= MODERN_MIN_VERSION);
    if header_modern {
        return true;
    }
    body_is_modern(message)
}

fn body_is_modern(message: &Value) -> bool {
    message
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
        .is_some_and(|version| version >= MODERN_MIN_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply() -> DiscoverReply {
        DiscoverReply::new(
            "Artist".into(),
            "Artist MCP harness".into(),
            "instructions".into(),
        )
    }

    #[test]
    fn discover_request_is_answered() {
        let response = reply()
            .answer(br#"{"jsonrpc":"2.0","id":"openai-mcp-discover","method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#)
            .expect("discover must be answered");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(response.headers()["mcp-protocol-version"], "2026-07-28");
        let body = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(response.into_body().collect())
            .expect("collect body")
            .to_bytes();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], "openai-mcp-discover");
        assert_eq!(value["result"]["resultType"], "complete");
        assert_eq!(
            value["result"]["supportedVersions"],
            json!(SUPPORTED_PROTOCOL_VERSIONS)
        );
        assert_eq!(value["result"]["capabilities"]["tools"], json!({}));
        assert_eq!(
            value["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "Artist"
        );
        assert!(
            value["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["version"]
                .as_str()
                .is_some()
        );
        assert_eq!(value["result"]["ttlMs"], CACHE_TTL_MS);
        assert_eq!(value["result"]["cacheScope"], "public");
    }

    #[test]
    fn other_methods_are_not_intercepted() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        assert!(reply().answer(body).is_none());
    }

    #[test]
    fn numeric_id_is_echoed() {
        let response = reply()
            .answer(br#"{"jsonrpc":"2.0","id":7,"method":"server/discover","params":{}}"#)
            .expect("discover must be answered");
        let body = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(response.into_body().collect())
            .expect("collect body")
            .to_bytes();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["id"], 7);
    }

    /// A stub inner service that answers with its tag, so a test can tell which
    /// of the two rmcp instances a request was routed to.
    #[derive(Clone)]
    struct TaggedService(&'static str);

    impl Service<Request<Body>> for TaggedService {
        type Response = Response<Body>;
        type Error = Infallible;
        type Future =
            std::pin::Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _request: Request<Body>) -> Self::Future {
            let tag = self.0;
            Box::pin(async move {
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(tag))
                    .expect("valid tagged response"))
            })
        }
    }

    fn gateway() -> McpGatewayService<TaggedService, TaggedService> {
        McpGatewayService::new(TaggedService("legacy"), TaggedService("modern"), reply())
    }

    #[tokio::test]
    async fn router_intercepts_discover_and_forwards_everything_else() {
        use axum::Router;
        use axum::body::Body as AxumBody;
        use axum::extract::Request as AxumRequest;
        use axum::http::Method;
        use tower::ServiceExt;

        let router = Router::new().nest_service("/mcp", gateway());

        let discover = router
            .clone()
            .oneshot(
                AxumRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(AxumBody::from(
                        br#"{"jsonrpc":"2.0","id":"openai-mcp-discover","method":"server/discover","params":{}}"#.to_vec(),
                    ))
                    .expect("build discover request"),
            )
            .await
            .expect("router responds");
        assert_eq!(discover.status(), StatusCode::OK);
        assert_eq!(discover.headers()["content-type"], "application/json");
        let body = axum::body::to_bytes(discover.into_body(), 1 << 20)
            .await
            .expect("collect body");
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["id"], "openai-mcp-discover");
        assert_eq!(value["result"]["resultType"], "complete");
        assert_eq!(
            value["result"]["supportedVersions"][0], "2026-07-28",
            "the modern revision must be advertised"
        );

        // A legacy initialize without any modern version marker is forwarded to
        // the stateful (legacy) instance.
        let forwarded = router
            .clone()
            .oneshot(
                AxumRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(AxumBody::from(
                        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#.to_vec(),
                    ))
                    .expect("build initialize request"),
            )
            .await
            .expect("router responds");
        let body = axum::body::to_bytes(forwarded.into_body(), 1 << 20)
            .await
            .expect("collect body");
        assert_eq!(&body[..], b"legacy", "legacy requests must stay stateful");

        // A modern request naming 2026-07-28 in the body is routed to the
        // stateless instance.
        let modern = router
            .clone()
            .oneshot(
                AxumRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(AxumBody::from(
                        br#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#.to_vec(),
                    ))
                    .expect("build modern request"),
            )
            .await
            .expect("router responds");
        let body = axum::body::to_bytes(modern.into_body(), 1 << 20)
            .await
            .expect("collect body");
        assert_eq!(&body[..], b"modern", "modern requests must go stateless");

        // The MCP-Protocol-Version header alone also selects the modern path.
        let header_modern = router
            .oneshot(
                AxumRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("mcp-protocol-version", "2026-07-28")
                    .body(AxumBody::from(
                        br#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#.to_vec(),
                    ))
                    .expect("build header-modern request"),
            )
            .await
            .expect("router responds");
        let body = axum::body::to_bytes(header_modern.into_body(), 1 << 20)
            .await
            .expect("collect body");
        assert_eq!(
            &body[..],
            b"modern",
            "the protocol version header must select stateless mode"
        );
    }

    #[test]
    fn modern_version_selection() {
        let message = |version: &str| {
            serde_json::from_str::<Value>(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":"{version}"}}}}}}"#
            ))
            .unwrap()
        };
        assert!(body_is_modern(&message("2026-07-28")));
        assert!(!body_is_modern(&message("2025-11-25")));
        assert!(!body_is_modern(&message("2025-06-18")));
        assert!(!body_is_modern(&serde_json::from_str::<Value>(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#
        )
        .unwrap()));
    }
}
