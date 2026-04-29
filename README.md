# mcp-oxide

[![CI](https://github.com/anomalyco/mcp-oxide/actions/workflows/ci.yml/badge.svg)](https://github.com/anomalyco/mcp-oxide/actions/workflows/ci.yml)

Cloud-agnostic, pluggable MCP Gateway in Rust. Control plane + data plane, adapters, tool router, session-aware routing but every cloud / IdP / runtime concern is a trait-based provider you can swap (Keycloak + GCP, Entra + AKS, Auth0 + EKS, plain Docker + SQLite, …).

## Quick start

```bash
# Build
cargo build --release

# Run with defaults (all providers are noop/deny-all until configured)
./target/release/mcp-oxide

# Probe
curl -s http://localhost:8080/healthz | jq .
```

## Kubernetes (Helm)

```bash
helm install mcp-oxide deploy/helm/mcp-oxide \
  --namespace mcp-oxide --create-namespace \
  -f deploy/helm/mcp-oxide/values-local.yaml
```

Production profile:

```bash
helm install mcp-oxide deploy/helm/mcp-oxide \
  --namespace mcp-oxide --create-namespace \
  -f deploy/helm/mcp-oxide/values-prod.yaml
```

## Docs
- [`docs/adr/`](./docs/adr) — architectural decision records
- [`openapi/mcp-oxide.openapi.yaml`](./openapi/mcp-oxide.openapi.yaml) — API spec

## License

Dual-licensed under Apache-2.0 or MIT at your option. See [`LICENSE`](./LICENSE).
