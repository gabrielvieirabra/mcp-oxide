//! MCP JSON-RPC + SSE client, reverse proxy, and tool router.
//!
//! Scaffolding only in Phase 0. Implementation lands in Phase 1 (proxy) and
//! Phase 3 (tool router).

#![deny(unsafe_code)]

pub mod jsonrpc;

pub const JSONRPC_VERSION: &str = "2.0";
