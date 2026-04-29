//! Gateway configuration (loaded from file + env).

use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logs: LogConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
        }
    }
}

fn default_bind() -> SocketAddr {
    "0.0.0.0:8080".parse().unwrap()
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct LogConfig {
    #[serde(default)]
    pub json: bool,
}

impl Config {
    /// Load config from `MCP_OXIDE_CONFIG` (file path, optional) merged with
    /// `MCP_OXIDE__…` env vars. Falls back to defaults.
    pub fn load() -> anyhow::Result<Self> {
        let mut fig = Figment::new();
        if let Ok(path) = std::env::var("MCP_OXIDE_CONFIG") {
            fig = fig.merge(Yaml::file(path));
        }
        let fig = fig.merge(Env::prefixed("MCP_OXIDE__").split("__"));
        Ok(fig.extract()?)
    }
}
