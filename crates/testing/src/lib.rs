//! Mock MCP servers and helpers for testing `mcp-oxide`.
//!
//! # Overview
//!
//! The crate provides [`MockMcp`], an in-process MCP-over-HTTP server with
//! programmable fixtures, fault injection, and request recording. It is used
//! by:
//!
//! - unit and integration tests inside this workspace,
//! - end-to-end tests that spin up the real gateway binary against mock
//!   backends,
//! - the `mock-mcp` binary in `src/bin/mock_mcp.rs`, which packages the same
//!   machinery into a container image used by `deploy/smoke/docker-compose.yaml`.
//!
//! # Example
//!
//! ```no_run
//! # use mcp_oxide_testing::MockMcp;
//! # use serde_json::json;
//! # async fn _example() -> anyhow::Result<()> {
//! let mock = MockMcp::builder()
//!     .tool("weather", json!({ "forecast": "sunny" }))
//!     .build()
//!     .await?;
//!
//! println!("mock running at {}", mock.base_url());
//! # Ok(()) }
//! ```
//!
//! # Transport
//!
//! MCP JSON-RPC 2.0 over HTTP POST. All requests go to a single `/mcp` path
//! (mirroring the gateway's upstream shape). SSE is not yet simulated; MCP
//! `tools/list` and `tools/call` are covered with request/response pairs.

#![deny(unsafe_code)]

pub mod fixture;
pub mod mock;

pub use fixture::{FaultInjection, MockFixture, ToolFixture};
pub use mock::{MockMcp, MockMcpBuilder, RecordedRequest};
