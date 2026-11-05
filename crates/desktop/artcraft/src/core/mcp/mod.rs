//! Local MCP (Model Context Protocol) server.
//!
//! Lets MCP clients such as Claude Code and Claude Desktop generate images and
//! videos through the running desktop app, using the signed-in ArtCraft account.
//! See `_docs/mcp.md` for setup.

pub mod mcp_http_server;
pub mod mcp_protocol;
pub mod mcp_server_config;
pub mod mcp_tauri_executor;
pub mod mcp_tools;
