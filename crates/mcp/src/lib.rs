//! GridCraft's headless MCP server.
//!
//! [Model Context Protocol](https://modelcontextprotocol.io) over stdio: newline-delimited
//! JSON-RPC 2.0, hand-written (no async runtime). The server hosts an in-process
//! [`gridcraft_engine::Session`] and exposes supported engine operations as tools and resources.
//!
//! Entry points: [`Server::serve`] (stdio loop) and [`Server::handle_line`] (one message).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod backend;
mod headless;
mod server;
mod tools;

pub use backend::Backend;
pub use headless::Headless;
pub use server::{COMMANDS_URI, FUNCTIONS_URI, PROTOCOL_VERSION, SUPPORTED_VERSIONS, Server, WORKBOOK_URI};
pub use tools::{ToolResult, call_tool, tool_definitions};

#[cfg(test)]
mod tests;
