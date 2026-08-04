//! Artist as an MCP server.
//!
//! Publish the harness's own tools over MCP so a web agent — ChatGPT, say, via
//! an OpenAI Secure MCP Tunnel — can drive a project with exactly the tools the
//! CLI gives a local session. The tool *surface* is built by
//! [`artist_agent::tool_set::mcp_surface`]; this crate adapts those tools onto
//! the wire and makes the request path survive transport death.

pub mod admin;
pub mod canvas_host;
pub mod daemon;
pub mod envelope;
pub mod server;

pub use daemon::{Allow, McpDaemon};
pub use server::McpServer;
