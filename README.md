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

## Frontend console

The operator console lives in [`frontend`](./frontend). It is a React +
TypeScript + Vite app that talks to the gateway API and stores only local
connection settings in the browser.

```bash
cd frontend
npm install
npm run dev
```

With the gateway running on `:8080`, the Vite dev server proxies API calls
from `:5173`.

## Docs
- [`docs/adr/`](./docs/adr) — architectural decision records
- [`openapi/mcp-oxide.openapi.yaml`](./openapi/mcp-oxide.openapi.yaml) — API spec

## License

Dual-licensed under Apache-2.0 or MIT at your option. See [`LICENSE`](./LICENSE).
