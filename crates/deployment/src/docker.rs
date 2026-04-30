//! Docker deployment provider for local development.
//!
//! Spawns containers for adapters and tools, exposing them on a dedicated
//! Docker network. Uses the Docker API via `bollard`.

use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, InspectContainerOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::network::{ConnectNetworkOptions, CreateNetworkOptions};
use bollard::Docker;
use futures::stream::{BoxStream, StreamExt};
use mcp_oxide_core::{
    providers::{
        DeploymentHandle, DeploymentProvider, DeploymentSpec, DeploymentStatus, Endpoint, LogLine,
    },
    session::BackendId,
    Error, Result,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Docker deployment provider configuration.
#[derive(Debug, Clone)]
pub struct DockerConfig {
    pub socket: String,
    pub network: String,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            socket: "/var/run/docker.sock".into(),
            network: "mcp-oxide".into(),
        }
    }
}

/// Docker-based deployment provider.
///
/// Manages individual containers for each adapter/tool. Containers are named
/// `mcp-oxide-{kind}-{name}` and attached to a shared network.
pub struct DockerProvider {
    docker: Docker,
    network: String,
    handles: Arc<RwLock<HashMap<String, DeploymentHandle>>>,
}

impl std::fmt::Debug for DockerProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerProvider")
            .field("network", &self.network)
            .finish_non_exhaustive()
    }
}

impl DockerProvider {
    /// Create a new Docker provider with the given configuration.
    pub async fn new(config: DockerConfig) -> Result<Self> {
        let docker = if config.socket == "unix:///var/run/docker.sock" || config.socket.is_empty() {
            Docker::connect_with_socket_defaults().map_err(|e| Error::Internal(e.to_string()))?
        } else {
            Docker::connect_with_socket(&config.socket, 120, bollard::API_DEFAULT_VERSION)
                .map_err(|e| Error::Internal(e.to_string()))?
        };

        let provider = Self {
            docker,
            network: config.network,
            handles: Arc::new(RwLock::new(HashMap::new())),
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

    fn parse_image_ref(reference: &str) -> (String, Option<String>) {
        let parts: Vec<&str> = reference.splitn(2, ':').collect();
        let image = parts[0].to_string();
        let tag = parts.get(1).map(|s| (*s).to_string());
        (image, tag)
    }

    async fn pull_image(&self, reference: &str) -> Result<()> {
        info!(image = %reference, "Pulling image");
        let (image, tag) = Self::parse_image_ref(reference);
        let tag = tag.unwrap_or_else(|| "latest".into());

        let mut stream = self.docker.create_image(
            Some(CreateImageOptions {
                from_image: image.clone(),
                tag: tag.clone(),
                ..Default::default()
            }),
            None,
            None,
        );

        while let Some(event) = stream.next().await {
            match event {
                Ok(info) => {
                    if let Some(status) = info.status {
                        debug!(status = %status, "Pull progress");
                    }
                }
                Err(e) => {
                    error!(error = %e, "Pull failed");
                    return Err(Error::Internal(format!("pull image {reference}: {e}")));
                }
            }
        }

        info!(image = %reference, "Image pulled");
        Ok(())
    }

    async fn create_container(&self, spec: &DeploymentSpec) -> Result<String> {
        let kind_str = match spec.kind {
            mcp_oxide_core::providers::DeploymentKind::Adapter => "adapter",
            mcp_oxide_core::providers::DeploymentKind::Tool => "tool",
        };

        let (image_ref, port, env_vars) = if let Some(adapter) = &spec.adapter {
            (
                &adapter.image.reference,
                adapter.endpoint.port,
                adapter.env.iter().map(|e| format!("{}={}", e.name, e.value)).collect::<Vec<_>>(),
            )
        } else if let Some(tool) = &spec.tool {
            (
                &tool.image.reference,
                tool.endpoint.port,
                tool.env.iter().map(|e| format!("{}={}", e.name, e.value)).collect::<Vec<_>>(),
            )
        } else {
            return Err(Error::Internal("DeploymentSpec has neither adapter nor tool".into()));
        };

        self.pull_image(image_ref).await?;

        let container_name = Self::container_name(kind_str, &spec.name);

        let mut labels = HashMap::new();
        labels.insert("mcp-oxide.io/managed", "true");
        labels.insert("mcp-oxide.io/kind", kind_str);
        labels.insert("mcp-oxide.io/name", &spec.name);

        let config = Config {
            image: Some(image_ref.clone()),
            env: Some(env_vars),
            labels: Some(labels.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()),
            exposed_ports: Some({
                #[allow(clippy::zero_sized_map_values)]
                {
                    let mut ports = HashMap::new();
                    ports.insert(format!("{port}/tcp"), HashMap::new());
                    ports
                }
            }),
            host_config: Some(bollard::service::HostConfig {
                network_mode: Some(self.network.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        match self
            .docker
            .create_container(Some(CreateContainerOptions { name: &container_name, platform: None }), config.clone())
            .await
        {
            Ok(_) => {
                info!(container = %container_name, "Container created");
            }
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 409, .. }) => {
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
            .map_err(|e| Error::Internal(format!("connect network: {e}")))?;

        self.docker
            .start_container(&container_name, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| Error::Internal(format!("start container: {e}")))?;

        info!(container = %container_name, "Container started");
        Ok(container_name)
    }
}

#[async_trait]
impl DeploymentProvider for DockerProvider {
    async fn apply(&self, spec: &DeploymentSpec) -> Result<DeploymentHandle> {
        let kind_str = match spec.kind {
            mcp_oxide_core::providers::DeploymentKind::Adapter => "adapter",
            mcp_oxide_core::providers::DeploymentKind::Tool => "tool",
        };

        let existing_name = Self::container_name(kind_str, &spec.name);

        if let Ok(info) = self.docker.inspect_container(&existing_name, None).await {
            if info.state.as_ref().and_then(|s| s.running).unwrap_or(false) {
                debug!(container = %existing_name, "Container already running");
                let handle = DeploymentHandle {
                    id: existing_name.clone(),
                    namespace: Some(self.network.clone()),
                };
                self.handles.write().await.insert(spec.name.clone(), handle.clone());
                return Ok(handle);
            }
        }

        let container_name = self.create_container(spec).await?;
        let handle = DeploymentHandle {
            id: container_name,
            namespace: Some(self.network.clone()),
        };
        self.handles.write().await.insert(spec.name.clone(), handle.clone());
        Ok(handle)
    }

    async fn delete(&self, handle: &DeploymentHandle) -> Result<()> {
        debug!(container = %handle.id, "Deleting container");
        
        self.docker
            .stop_container(&handle.id, Some(StopContainerOptions { t: 10 }))
            .await
            .ok();

        self.docker
            .remove_container(&handle.id, Some(RemoveContainerOptions { force: true, ..Default::default() }))
            .await
            .map_err(|e| Error::Internal(format!("remove container: {e}")))?;

        let key = handle.id.strip_prefix("mcp-oxide-adapter-")
            .or_else(|| handle.id.strip_prefix("mcp-oxide-tool-"))
            .unwrap_or(&handle.id);
        self.handles.write().await.remove(key);

        info!(container = %handle.id, "Container deleted");
        Ok(())
    }

    async fn status(&self, handle: &DeploymentHandle) -> Result<DeploymentStatus> {
        let info = self
            .docker
            .inspect_container(&handle.id, None::<InspectContainerOptions>)
            .await
            .map_err(|e| Error::Internal(format!("inspect container: {e}")))?;

        let state = info.state.ok_or_else(|| Error::Internal("no state".into()))?;
        
        Ok(DeploymentStatus {
            ready: state.running.unwrap_or(false),
            replicas: 1,
            ready_replicas: u32::from(state.running.unwrap_or(false)),
            message: state.status.map(|s| format!("{s:?}")),
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
                    Some(LogLine { ts: ts.to_string(), line: rest.to_string() })
                }
                Err(_) => None,
            }
        })))
    }

    async fn endpoints(&self, handle: &DeploymentHandle) -> Result<Vec<Endpoint>> {
        let info = self
            .docker
            .inspect_container(&handle.id, None)
            .await
            .map_err(|e| Error::Internal(format!("inspect container: {e}")))?;

        let container_name = info.name.unwrap_or_else(|| handle.id.clone());
        let name = container_name.strip_prefix('/').unwrap_or(&container_name);

        let port = 8080u16;

        let url = format!("http://{name}:{port}/mcp");
        
        Ok(vec![Endpoint {
            url,
            backend_id: BackendId(handle.id.clone()),
        }])
    }

    fn kind(&self) -> &'static str {
        "docker"
    }
}
