//! Docker deployment provider for local development.
//!
//! Spawns containers for adapters and tools, exposing them on a dedicated
//! Docker bridge network, and enforces a hardened default security context
//! (non-root user, read-only rootfs, dropped capabilities, `no-new-privileges`,
//! resource limits). The gateway routes to containers by their in-network DNS
//! name (preferred) or IP (fallback).

use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, InspectContainerOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::network::{ConnectNetworkOptions, CreateNetworkOptions};
use bollard::service::{ContainerStateStatusEnum, HostConfig, RestartPolicy, RestartPolicyNameEnum};
use bollard::Docker;
use futures::stream::{BoxStream, StreamExt};
use mcp_oxide_core::{
    adapter::{Endpoint as AdapterEndpoint, EnvVar, Resources},
    providers::{
        DeploymentHandle, DeploymentKind, DeploymentProvider, DeploymentSpec, DeploymentStatus,
        Endpoint, LogLine,
    },
    session::BackendId,
    Error, Result,
};
use std::collections::HashMap;
use tracing::{debug, info, warn};

use crate::image_ref::ImageRef;

/// Docker deployment provider configuration.
#[derive(Debug, Clone)]
pub struct DockerConfig {
    pub socket: String,
    pub network: String,
    /// Connection timeout (seconds) used when talking to the Docker daemon.
    pub connect_timeout_s: u64,
    /// Refuse images whose registry is not on this list (empty = no check).
    /// Matched against the first path component of the parsed image
    /// reference; `docker.io` is implicit for unqualified names.
    pub allowed_registries: Vec<String>,
    /// If `true`, reject image references that are not digest-pinned. Useful
    /// for production profiles; disabled by default for dev convenience.
    pub require_digest_pinning: bool,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            socket: "/var/run/docker.sock".into(),
            network: "mcp-oxide".into(),
            connect_timeout_s: 120,
            allowed_registries: vec![],
            require_digest_pinning: false,
        }
    }
}

/// Docker-based deployment provider.
pub struct DockerProvider {
    docker: Docker,
    network: String,
    allowed_registries: Vec<String>,
    require_digest_pinning: bool,
}

impl std::fmt::Debug for DockerProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerProvider")
            .field("network", &self.network)
            .field("allowed_registries", &self.allowed_registries)
            .field("require_digest_pinning", &self.require_digest_pinning)
            .finish_non_exhaustive()
    }
}

impl DockerProvider {
    /// Create a new Docker provider with the given configuration.
    pub async fn new(config: DockerConfig) -> Result<Self> {
        let docker = if config.socket == "unix:///var/run/docker.sock" || config.socket.is_empty() {
            Docker::connect_with_socket_defaults().map_err(|e| Error::Internal(e.to_string()))?
        } else {
            Docker::connect_with_socket(
                &config.socket,
                config.connect_timeout_s,
                bollard::API_DEFAULT_VERSION,
            )
            .map_err(|e| Error::Internal(e.to_string()))?
        };

        let provider = Self {
            docker,
            network: config.network,
            allowed_registries: config.allowed_registries,
            require_digest_pinning: config.require_digest_pinning,
        };

        provider.ensure_network().await?;
        Ok(provider)
    }

    async fn ensure_network(&self) -> Result<()> {
        use bollard::errors::Error::DockerResponseServerError;
        match self
            .docker
            .inspect_network::<String>(&self.network, None)
            .await
        {
            Ok(_) => {
                debug!(network = %self.network, "Docker network exists");
            }
            Err(DockerResponseServerError { status_code: 404, .. }) => {
                info!(network = %self.network, "Creating Docker network");
                self.docker
                    .create_network(CreateNetworkOptions {
                        name: self.network.clone(),
                        driver: "bridge".into(),
                        check_duplicate: true,
                        ..Default::default()
                    })
                    .await
                    .map_err(|e| Error::Internal(format!("create network: {e}")))?;
            }
            Err(e) => {
                return Err(Error::Internal(format!("inspect network: {e}")));
            }
        }
        Ok(())
    }

    fn container_name(kind: &str, name: &str) -> String {
        format!("mcp-oxide-{kind}-{name}")
    }

    fn kind_str(kind: DeploymentKind) -> &'static str {
        match kind {
            DeploymentKind::Adapter => "adapter",
            DeploymentKind::Tool => "tool",
        }
    }

    fn check_image_policy(&self, image: &ImageRef) -> Result<()> {
        if self.require_digest_pinning && !image.is_digest_pinned() {
            return Err(Error::InvalidRequest(format!(
                "image '{}' is not digest-pinned and require_digest_pinning=true",
                image.name
            )));
        }
        if !self.allowed_registries.is_empty() {
            // Extract first path component; default to docker.io for unqualified.
            let head = image.name.split('/').next().unwrap_or(&image.name);
            let registry = if head.contains('.') || head.contains(':') {
                head
            } else {
                "docker.io"
            };
            if !self.allowed_registries.iter().any(|r| r == registry) {
                return Err(Error::InvalidRequest(format!(
                    "image registry '{registry}' is not in the allowed_registries list"
                )));
            }
        }
        Ok(())
    }

    async fn pull_image(&self, image: &ImageRef) -> Result<()> {
        // Avoid logging the full reference at info level to reduce the
        // chance of leaking private-registry paths into log aggregators.
        info!(name = %image.name, "Pulling image");

        let mut opts = CreateImageOptions::<String> {
            from_image: image.name.clone(),
            ..Default::default()
        };
        if let Some(digest) = image.digest.as_deref() {
            // Digest references use the `@<algo>:<hex>` form as the "tag"
            // field for bollard.
            opts.tag = format!("@{digest}");
        } else {
            opts.tag = image.effective_tag().to_string();
        }

        let mut stream = self.docker.create_image(Some(opts), None, None);

        while let Some(event) = stream.next().await {
            match event {
                Ok(info) => {
                    if let Some(status) = info.status {
                        debug!(status = %status, "Pull progress");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Pull failed");
                    return Err(Error::Internal(format!("pull image: {e}")));
                }
            }
        }
        Ok(())
    }

    fn extract_spec(
        spec: &DeploymentSpec,
    ) -> Result<(&str, &AdapterEndpoint, &[EnvVar], &Resources)> {
        if let Some(a) = &spec.adapter {
            return Ok((&a.image.reference, &a.endpoint, &a.env, &a.resources));
        }
        if let Some(t) = &spec.tool {
            return Ok((&t.image.reference, &t.endpoint, &t.env, &t.resources));
        }
        Err(Error::Internal(
            "DeploymentSpec has neither adapter nor tool".into(),
        ))
    }

    async fn create_container(&self, spec: &DeploymentSpec) -> Result<String> {
        let kind_str = Self::kind_str(spec.kind);
        let (image_ref_str, endpoint, env, resources) = Self::extract_spec(spec)?;

        let image = ImageRef::parse(image_ref_str)?;
        self.check_image_policy(&image)?;
        self.pull_image(&image).await?;

        let container_name = Self::container_name(kind_str, &spec.name);
        let env_vars: Vec<String> = env
            .iter()
            .map(|e| format!("{}={}", e.name, e.value))
            .collect();

        let mut labels: HashMap<String, String> = HashMap::new();
        labels.insert("mcp-oxide.io/managed".into(), "true".into());
        labels.insert("mcp-oxide.io/kind".into(), kind_str.into());
        labels.insert("mcp-oxide.io/name".into(), spec.name.clone());

        let exposed_ports = {
            #[allow(clippy::zero_sized_map_values)]
            {
                let mut ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
                ports.insert(format!("{}/tcp", endpoint.port), HashMap::new());
                ports
            }
        };

        // -----------------------------------------------------------------
        // Hardened HostConfig: non-root, read-only rootfs, dropped caps,
        // no-new-privileges, resource limits. The workload is expected to
        // write only to /tmp (tmpfs) and its endpoint is HTTP-over-loopback
        // within the Docker network.
        // -----------------------------------------------------------------
        let nano_cpus = parse_cpu_limit(resources.cpu.as_deref());
        let memory_bytes = parse_memory_limit(resources.memory.as_deref());

        let mut tmpfs: HashMap<String, String> = HashMap::new();
        tmpfs.insert("/tmp".into(), "rw,noexec,nosuid,size=64m".into());

        let host_config = HostConfig {
            network_mode: Some(self.network.clone()),
            readonly_rootfs: Some(true),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            pids_limit: Some(256),
            auto_remove: Some(false),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                maximum_retry_count: None,
            }),
            tmpfs: Some(tmpfs),
            nano_cpus,
            memory: memory_bytes,
            memory_swap: memory_bytes, // disable swap escape: swap == mem
            oom_score_adj: Some(500),
            ..Default::default()
        };

        let config = Config {
            image: Some(image_ref_str.to_string()),
            env: Some(env_vars),
            labels: Some(labels),
            exposed_ports: Some(exposed_ports),
            user: Some("65532:65532".into()),
            host_config: Some(host_config),
            ..Default::default()
        };

        match self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name: &container_name,
                    platform: None,
                }),
                config,
            )
            .await
        {
            Ok(_) => {
                info!(container = %container_name, "Container created");
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => {
                debug!(container = %container_name, "Container already exists, reusing");
            }
            Err(e) => {
                return Err(Error::Internal(format!("create container: {e}")));
            }
        }

        self.docker
            .connect_network(
                &self.network,
                ConnectNetworkOptions::<String> {
                    container: container_name.clone(),
                    endpoint_config: bollard::service::EndpointSettings::default(),
                },
            )
            .await
            .ok(); // idempotent: 403/409 are fine

        self.docker
            .start_container(&container_name, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| Error::Internal(format!("start container: {e}")))?;

        info!(container = %container_name, "Container started");
        Ok(container_name)
    }

    /// Inspect-based endpoint URL using the in-network DNS name (preferred)
    /// and an IP fallback for callers not on the same network.
    async fn resolve_endpoint_url(
        &self,
        container_id: &str,
        port: u16,
        path: &str,
    ) -> Result<String> {
        let info = self
            .docker
            .inspect_container(container_id, None::<InspectContainerOptions>)
            .await
            .map_err(|e| Error::Internal(format!("inspect container: {e}")))?;

        let container_name = info.name.clone().unwrap_or_else(|| container_id.into());
        let dns = container_name
            .strip_prefix('/')
            .unwrap_or(&container_name)
            .to_string();

        // Prefer the network-scoped IP if present; callers outside the
        // network will get a routable address even if Docker's embedded DNS
        // isn't reachable.
        let ip = info
            .network_settings
            .as_ref()
            .and_then(|ns| ns.networks.as_ref())
            .and_then(|nets| nets.get(&self.network))
            .and_then(|n| n.ip_address.clone())
            .filter(|s| !s.is_empty());

        let host = ip.unwrap_or(dns);
        let clean_path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Ok(format!("http://{host}:{port}{clean_path}"))
    }
}

/// Convert a Kubernetes-style CPU string (`"500m"`, `"2"`, `"1.5"`) into
/// `nano_cpus` for Docker (`billionths` of a CPU). `None` on parse failure.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn parse_cpu_limit(s: Option<&str>) -> Option<i64> {
    let raw = s?.trim();
    if raw.is_empty() {
        return None;
    }
    let (num, scale): (f64, f64) = if let Some(ms) = raw.strip_suffix('m') {
        (ms.parse().ok()?, 1_000_000.0)
    } else {
        (raw.parse().ok()?, 1_000_000_000.0)
    };
    if !num.is_finite() || num <= 0.0 {
        return None;
    }
    // 2^63 is approximately 9.22e18; compare as f64 which is safe for this
    // order-of-magnitude check. Loss of precision at the boundary is
    // irrelevant because CPU quotas never approach that scale.
    let nanos = (num * scale).round();
    if nanos <= 0.0 || nanos >= 9.223_372e18 {
        return None;
    }
    Some(nanos as i64)
}

/// Convert `"512Mi"`, `"1Gi"`, `"256M"`, or a raw byte count into bytes.
/// Capped at `i64::MAX` to match Docker's field type.
fn parse_memory_limit(s: Option<&str>) -> Option<i64> {
    let raw = s?.trim();
    if raw.is_empty() {
        return None;
    }
    let (num_str, mult): (&str, u64) = if let Some(v) = raw.strip_suffix("Gi") {
        (v, 1024 * 1024 * 1024)
    } else if let Some(v) = raw.strip_suffix("Mi") {
        (v, 1024 * 1024)
    } else if let Some(v) = raw.strip_suffix("Ki") {
        (v, 1024)
    } else if let Some(v) = raw.strip_suffix('G') {
        (v, 1_000_000_000)
    } else if let Some(v) = raw.strip_suffix('M') {
        (v, 1_000_000)
    } else if let Some(v) = raw.strip_suffix('K') {
        (v, 1_000)
    } else {
        (raw, 1)
    };
    let num: u64 = num_str.trim().parse().ok()?;
    let bytes = num.saturating_mul(mult).min(i64::MAX as u64);
    i64::try_from(bytes).ok()
}

fn map_status(s: Option<ContainerStateStatusEnum>) -> Option<String> {
    s.map(|s| match s {
        ContainerStateStatusEnum::CREATED => "created",
        ContainerStateStatusEnum::RUNNING => "running",
        ContainerStateStatusEnum::PAUSED => "paused",
        ContainerStateStatusEnum::RESTARTING => "restarting",
        ContainerStateStatusEnum::REMOVING => "removing",
        ContainerStateStatusEnum::EXITED => "exited",
        ContainerStateStatusEnum::DEAD => "dead",
        ContainerStateStatusEnum::EMPTY => "unknown",
    }
    .to_string())
}

#[async_trait]
impl DeploymentProvider for DockerProvider {
    async fn apply(&self, spec: &DeploymentSpec) -> Result<DeploymentHandle> {
        let kind_str = Self::kind_str(spec.kind);
        let (_, endpoint, _, _) = Self::extract_spec(spec)?;
        let container_name = Self::container_name(kind_str, &spec.name);

        // Idempotent: if already running, reuse.
        if let Ok(info) = self.docker.inspect_container(&container_name, None).await {
            if info.state.as_ref().and_then(|s| s.running).unwrap_or(false) {
                debug!(container = %container_name, "Container already running");
                let url = self
                    .resolve_endpoint_url(&container_name, endpoint.port, &endpoint.path)
                    .await
                    .ok();
                return Ok(DeploymentHandle {
                    id: container_name,
                    namespace: Some(self.network.clone()),
                    endpoint_url: url,
                });
            }
        }

        let container_name = self.create_container(spec).await?;
        let url = self
            .resolve_endpoint_url(&container_name, endpoint.port, &endpoint.path)
            .await
            .ok();
        Ok(DeploymentHandle {
            id: container_name,
            namespace: Some(self.network.clone()),
            endpoint_url: url,
        })
    }

    async fn delete(&self, handle: &DeploymentHandle) -> Result<()> {
        debug!(container = %handle.id, "Deleting container");

        if let Err(e) = self
            .docker
            .stop_container(&handle.id, Some(StopContainerOptions { t: 10 }))
            .await
        {
            // Tolerate stopping an already-stopped or missing container;
            // escalate anything else so the control plane doesn't silently
            // drop orphaned resources.
            match e {
                bollard::errors::Error::DockerResponseServerError { status_code, .. }
                    if status_code == 404 || status_code == 304 => {}
                other => {
                    warn!(error = %other, container = %handle.id, "stop failed");
                }
            }
        }

        match self
            .docker
            .remove_container(
                &handle.id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(()) => {}
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                debug!(container = %handle.id, "already gone");
            }
            Err(e) => return Err(Error::Internal(format!("remove container: {e}"))),
        }

        info!(container = %handle.id, "Container deleted");
        Ok(())
    }

    async fn status(&self, handle: &DeploymentHandle) -> Result<DeploymentStatus> {
        let info = self
            .docker
            .inspect_container(&handle.id, None::<InspectContainerOptions>)
            .await
            .map_err(|e| Error::Internal(format!("inspect container: {e}")))?;

        let state = info
            .state
            .ok_or_else(|| Error::Internal("no state".into()))?;

        let running = state.running.unwrap_or(false);
        Ok(DeploymentStatus {
            ready: running,
            replicas: 1,
            ready_replicas: u32::from(running),
            message: map_status(state.status),
        })
    }

    async fn logs(&self, handle: &DeploymentHandle) -> Result<BoxStream<'static, LogLine>> {
        let stream = self.docker.logs(
            &handle.id,
            Some(LogsOptions::<String> {
                follow: true,
                stdout: true,
                stderr: true,
                timestamps: true,
                ..Default::default()
            }),
        );

        Ok(Box::pin(stream.filter_map(|event| async move {
            match event {
                Ok(output) => {
                    let line = output.to_string();
                    let (ts, rest) = line.split_once(' ').unwrap_or(("", ""));
                    Some(LogLine {
                        ts: ts.to_string(),
                        line: rest.to_string(),
                    })
                }
                Err(_) => None,
            }
        })))
    }

    async fn endpoints(&self, handle: &DeploymentHandle) -> Result<Vec<Endpoint>> {
        // Prefer the URL captured at `apply` time to avoid a second round-trip
        // to the Docker daemon on every tools/call.
        if let Some(url) = handle.endpoint_url.clone() {
            return Ok(vec![Endpoint {
                url,
                backend_id: BackendId(handle.id.clone()),
            }]);
        }

        // Fallback: inspect the container. This path is hit when the handle
        // was reconstructed from just a name (e.g. on a status endpoint that
        // hasn't been upgraded to read the stored handle yet).
        let info = self
            .docker
            .inspect_container(&handle.id, None)
            .await
            .map_err(|e| Error::Internal(format!("inspect container: {e}")))?;

        let container_name = info.name.clone().unwrap_or_else(|| handle.id.clone());
        let dns = container_name
            .strip_prefix('/')
            .unwrap_or(&container_name)
            .to_string();

        let ip = info
            .network_settings
            .as_ref()
            .and_then(|ns| ns.networks.as_ref())
            .and_then(|nets| nets.get(&self.network))
            .and_then(|n| n.ip_address.clone())
            .filter(|s| !s.is_empty());

        let host = ip.unwrap_or(dns);
        let url = format!("http://{host}:8080/mcp");
        Ok(vec![Endpoint {
            url,
            backend_id: BackendId(handle.id.clone()),
        }])
    }

    fn kind(&self) -> &'static str {
        "docker"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_limit_parses() {
        assert_eq!(parse_cpu_limit(Some("500m")), Some(500_000_000));
        assert_eq!(parse_cpu_limit(Some("2")), Some(2_000_000_000));
        assert_eq!(parse_cpu_limit(Some("1.5")), Some(1_500_000_000));
        assert_eq!(parse_cpu_limit(None), None);
        assert_eq!(parse_cpu_limit(Some("")), None);
        assert_eq!(parse_cpu_limit(Some("bogus")), None);
    }

    #[test]
    fn memory_limit_parses() {
        assert_eq!(parse_memory_limit(Some("512Mi")), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory_limit(Some("1Gi")), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory_limit(Some("500M")), Some(500_000_000));
        assert_eq!(parse_memory_limit(Some("256")), Some(256));
        assert_eq!(parse_memory_limit(None), None);
    }
}
