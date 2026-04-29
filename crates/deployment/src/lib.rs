//! Deployment providers.
//!
//! Phase 0 ships only the `noop-external` provider: the adapter is considered
//! already deployed out-of-band (external URL). Phases 3+ add real runtimes.

#![deny(unsafe_code)]

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use mcp_oxide_core::{
    providers::{
        DeploymentHandle, DeploymentProvider, DeploymentSpec, DeploymentStatus, Endpoint, LogLine,
    },
    session::BackendId,
    Result,
};

/// A deployment provider that assumes workloads are managed externally.
#[derive(Debug, Default)]
pub struct NoopExternalProvider;

#[async_trait]
impl DeploymentProvider for NoopExternalProvider {
    async fn apply(&self, spec: &DeploymentSpec) -> Result<DeploymentHandle> {
        Ok(DeploymentHandle {
            id: spec.name.clone(),
            namespace: None,
        })
    }

    async fn delete(&self, _handle: &DeploymentHandle) -> Result<()> {
        Ok(())
    }

    async fn status(&self, _handle: &DeploymentHandle) -> Result<DeploymentStatus> {
        Ok(DeploymentStatus {
            ready: true,
            replicas: 1,
            ready_replicas: 1,
            message: Some("external".into()),
        })
    }

    async fn logs(&self, _handle: &DeploymentHandle) -> Result<BoxStream<'static, LogLine>> {
        Ok(Box::pin(stream::empty()))
    }

    async fn endpoints(&self, handle: &DeploymentHandle) -> Result<Vec<Endpoint>> {
        Ok(vec![Endpoint {
            url: format!("external://{}", handle.id),
            backend_id: BackendId(handle.id.clone()),
        }])
    }

    fn kind(&self) -> &'static str {
        "noop-external"
    }
}
