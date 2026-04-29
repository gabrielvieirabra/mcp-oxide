//! Core domain types, errors, and provider traits for mcp-oxide.
//!
//! This crate defines the stable contracts that every provider (identity,
//! authorization, deployment, metadata, session, secrets, audit) implements.
//! It intentionally contains no I/O.

#![deny(unsafe_code)]

pub mod adapter;
pub mod audit;
pub mod error;
pub mod identity;
pub mod policy;
pub mod providers;
pub mod session;
pub mod tool;

pub use error::{Error, Result};
