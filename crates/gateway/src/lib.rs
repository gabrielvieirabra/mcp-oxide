//! mcp-oxide gateway library entry points.
//!
//! The binary at `src/main.rs` is a thin wrapper around this library.
//! Integration tests and embedders consume the library directly.

#![deny(unsafe_code)]

pub mod app;
pub mod auth;
pub mod config;
pub mod error;
pub mod proxy;
pub mod routes;
pub mod state;

pub use app::router;
pub use config::Config;
pub use state::AppState;
