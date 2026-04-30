//! Deployment providers.
//!
//! Phase 0 ships only the `noop-external` provider: the adapter is considered
//! already deployed out-of-band (external URL). Phases 3+ add real runtimes.

#![deny(unsafe_code)]

pub mod image_ref;

mod noop;
pub use noop::NoopExternalProvider;

#[cfg(feature = "docker")]
mod docker;
#[cfg(feature = "docker")]
pub use docker::{DockerConfig, DockerProvider};
