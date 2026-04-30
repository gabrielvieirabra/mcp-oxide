//! A deployment provider that assumes workloads are managed externally.
//!
//! The adapter/tool is running somewhere reachable; the gateway only proxies.
//! Used when the caller supplies `adapter.upstream` explicitly.

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

fn extract_upstream(spec: &DeploymentSpec) -> Option<String> {
    spec.adapter.as_ref().and_then(|a| a.upstream.clone())
}

#[async_trait]
impl DeploymentProvider for NoopExternalProvider {
    async fn apply(&self, spec: &DeploymentSpec) -> Result<DeploymentHandle> {
        Ok(DeploymentHandle {
            id: spec.name.clone(),
            namespace: None,
            endpoint_url: extract_upstream(spec),
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
        // Noop-external can only report an endpoint when the caller passed
        // one at apply-time. Otherwise return empty so the data plane fails
        // closed rather than routing to an invented URL.
        let Some(url) = handle.endpoint_url.clone() else {
            return Ok(vec![]);
        };
        Ok(vec![Endpoint {
            url,
            backend_id: BackendId(handle.id.clone()),
        }])
    }

    fn kind(&self) -> &'static str {
        "noop-external"
    }
}
