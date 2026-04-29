//! Metadata store implementations.

#![deny(unsafe_code)]

#[cfg(feature = "in-memory")]
mod memory;

#[cfg(feature = "in-memory")]
pub use memory::InMemoryMetadataStore;
