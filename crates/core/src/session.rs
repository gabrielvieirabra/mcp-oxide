//! Session-affinity types.

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendId(pub String);

#[derive(Debug, Clone)]
pub struct Binding {
    pub session: SessionId,
    pub adapter: String,
    pub backend: BackendId,
    pub ttl: Duration,
}
