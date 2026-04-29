# mcp-oxide — MCP Gateway (Rust, Cloud-Agnostic, Pluggable)

> Reverse proxy + management layer for **multiple MCP servers**. Same two-plane architecture as [`microsoft/mcp-gateway`](https://github.com/microsoft/mcp-gateway) (control plane + data plane, adapters, tool router, session-aware routing) — but every cloud/IdP/runtime concern is a **trait-based provider** that can be swapped (Keycloak+GCP, Entra+AKS, Auth0+EKS, plain-Docker+SQLite, …).

---

## 0. Goals & Non-Goals

### Goals
- Host and route traffic to **N MCP servers** registered at runtime.
- **Two control surfaces**:
  - **Control plane** — RESTful CRUD for *adapters* (MCP servers) and *tools*.
  - **Data plane** — streamable HTTP / SSE proxy with session affinity.
- **Tool Gateway Router**: a built-in MCP server that aggregates registered tools and dispatches `tools/call` to the right tool backend.
- **Pluggable providers** for every cloud-coupled capability:
  - Identity (OIDC-generic, Keycloak, Entra, Cognito, Auth0, GCP IAP/IAM, …).
  - Authorization (YAML RBAC, OPA, Cedar).
  - Deployment runtime (Kubernetes, Nomad, Docker, local process, Cloud Run, ECS, Fly.io).
  - Metadata store (Postgres, SQLite, Redis, Cosmos DB, Firestore, DynamoDB, in-memory).
  - Session store (in-memory, Redis, NATS KV, Memcached).
  - Container registry (any OCI-compliant: ACR, ECR, GCR, Docker Hub, Harbor).
  - Secret provider (env, Vault, K8s Secrets, AWS SM, GCP SM, Azure KV).
  - Audit sink (stdout, file, Kafka, NATS, S3-compatible, Postgres).
  - Telemetry (OTLP generic).
- **Cloud-agnostic deployment**: ships as a single static binary + container image; reference Helm chart and Docker Compose included.
- **Production-Kubernetes-ready** as a first-class deployment target (see §8):
  - Distroless, non-root, read-only-rootfs container image.
  - Helm chart with sane defaults: HPA, PDB, NetworkPolicy, ServiceMonitor, PodSecurity, anti-affinity.
  - In-cluster kube-native mode (ServiceAccount + RBAC, leader-elected reconciler, StatefulSet + headless Service for session-affinity replicas).
  - Gateway API / Ingress agnostic; works behind any L7 (NGINX, Traefik, Envoy Gateway, Istio, Contour, GKE/AKS/EKS-native LBs).

### Non-Goals
- Not tied to Kubernetes or any specific cloud. K8s runtime is *one provider* among others — but it **is** a first-class, production-hardened target with a supported Helm chart.
- Not an admin UI (v1). Only APIs + CLI.
- Not an IdP. Consumes tokens only.
- Not a secrets manager. Consumes providers.
- No CRD/Operator in v1 (MetadataStore holds state). CRD/Operator mode is on the roadmap (§8.11).

### Glossary
- **Adapter** — registered MCP server (image + metadata) exposed at `/adapters/{name}/mcp`.
- **Tool** — registered capability with MCP tool definition, backed by a dedicated server, routed via `/mcp`.
- **Tool Gateway Router** — internal MCP server instance that aggregates tools and dispatches calls.
- **Provider** — trait implementation for a pluggable concern.
- **PDP / PEP** — Policy Decision Point (OPA/Cedar/YAML) / Enforcement Point (gateway).

---

## 1. Architecture

```
                        ┌──────────────────────────────────────────────────┐
                        │                    Clients                       │
                        │  Agent / MCP client              Admin / CLI     │
                        └────────┬──────────────────────────────┬──────────┘
                                 │ Bearer (JWT/OIDC)            │ Bearer
                                 ▼                              ▼
┌──────────────────────────────────────────────────────────────────────────┐
│                              mcp-oxide Gateway                           │
│ ┌──────────────────────────────────────────────────────────────────────┐ │
│ │  Edge middleware: trace-id · rate-limit · body-limit · timeout       │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│ ┌────────────────── Auth (pluggable IdProvider) ──────────────────────┐  │
│ │   OIDC discovery · JWKS cache · claims → UserContext                │  │
│ └─────────────────────────────────────────────────────────────────────┘  │
│ ┌────────────────── AuthZ (pluggable PolicyEngine) ───────────────────┐  │
│ │   YAML RBAC · OPA sidecar · Cedar embedded                          │  │
│ └─────────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│ ┌─────────── Data Plane ──────────┐   ┌──────── Control Plane ─────────┐ │
│ │ /adapters/{name}/mcp            │   │ /adapters   CRUD                │ │
│ │ /mcp  (Tool Router)             │   │ /tools      CRUD                │ │
│ │ Session-aware affinity          │   │ /adapters/{n}/status | /logs    │ │
│ └───────────┬─────────────────────┘   └────────────┬───────────────────┘ │
│             │                                      │                     │
│ ┌───────────▼──────────────┐   ┌───────────────────▼────────────────┐    │
│ │  SessionStore (plug)     │   │  Deployment (plug) · Secrets (plug)│    │
│ │  MetadataStore (plug)    │   │  Registry (plug)                   │    │
│ └──────────────────────────┘   └────────────────────────────────────┘    │
│                                                                          │
│ ┌──────────── Observability (OTLP) · Audit (plug sinks) ────────────┐    │
│ └───────────────────────────────────────────────────────────────────┘    │
└───────────────────────────┬───────────────────────┬──────────────────────┘
                            │ JSON-RPC / SSE        │
                 ┌──────────▼───────────┐   ┌───────▼────────────┐
                 │  Adapter MCP Servers │   │ Tool Servers       │
                 │  mcp-a · mcp-b · …   │   │ tool-x · tool-y ·… │
                 └──────────────────────┘   └────────────────────┘
```

### Request lifecycles

**Data plane — adapter proxy**
1. `POST /adapters/{name}/mcp` (JSON-RPC or SSE).
2. Edge middleware + AuthN + AuthZ (`action = mcp.invoke`, `resource = adapter:{name}`).
3. Resolve session → backend instance via `SessionStore` (sticky).
4. Stream JSON-RPC / SSE to backend through `mcp::Proxy`.
5. Emit audit event + metrics.

**Data plane — tool router**
1. `POST /mcp` (client sees a single MCP server).
2. Auth + policy check on `tools/call` using tool metadata (tags, domain).
3. Router inspects `tools/call.name` → looks up `ToolRegistry` → dispatches to the right Tool server.
4. Results streamed back.

**Control plane**
1. `POST /adapters` with image, version, env, scaling → `MetadataStore.put` + `Deployment.apply`.
2. `GET /adapters/{n}/status` → `Deployment.status`.
3. `POST /tools` with tool definition + image → deploys tool server + registers definition in `ToolRegistry`.

---

## 2. Contracts

### 2.1 Transport
Gateway speaks MCP **JSON-RPC 2.0** over streamable HTTP (+ SSE) end-to-end.

### 2.2 Control-plane API (OpenAPI generated)

| Method | Path | Purpose |
|--------|------|---------|
| POST   | `/adapters` | create+deploy adapter |
| GET    | `/adapters` | list |
| GET    | `/adapters/{name}` | read |
| PUT    | `/adapters/{name}` | update |
| DELETE | `/adapters/{name}` | delete |
| GET    | `/adapters/{name}/status` | deployment status |
| GET    | `/adapters/{name}/logs` | logs (SSE stream) |
| POST   | `/tools` | register+deploy tool |
| GET    | `/tools` | list |
| GET    | `/tools/{name}` | read |
| PUT    | `/tools/{name}` | update |
| DELETE | `/tools/{name}` | delete |
| GET    | `/tools/{name}/status` | deployment status |
| GET    | `/tools/{name}/logs` | logs (SSE stream) |
| GET    | `/healthz` / `/readyz` / `/metrics` | ops |

### 2.3 Adapter spec (create payload)
```json
{
  "name": "mcp-aws",
  "description": "AWS ops MCP server",
  "image": { "ref": "registry.example.com/mcp-aws:1.4.0" },
  "endpoint": { "port": 8080, "path": "/mcp" },
  "replicas": 2,
  "env": [{ "name": "LOG_LEVEL", "value": "info" }],
  "secretRefs": [{ "name": "AWS_CREDS", "provider": "vault", "key": "kv/mcp/aws" }],
  "required_roles": ["mcp.engineer"],
  "tags": ["aws"],
  "resources": { "cpu": "500m", "memory": "512Mi" },
  "health": { "path": "/healthz", "port": 8080 },
  "session_affinity": "sticky"
}
```

### 2.4 Tool spec (create payload)
```json
{
  "name": "weather",
  "description": "Weather lookup",
  "image": { "ref": "registry.example.com/weather-tool:1.0.0" },
  "endpoint": { "port": 8000, "path": "/mcp" },
  "tool_definition": {
    "name": "weather",
    "title": "Weather",
    "description": "Get current weather",
    "input_schema": {
      "type": "object",
      "properties": { "location": { "type": "string" } },
      "required": ["location"]
    },
    "annotations": { "readOnly": true }
  },
  "tags": ["public","readonly"],
  "required_roles": []
}
```

### 2.5 Error taxonomy (HTTP + JSON-RPC)

| Condition                  | HTTP | JSON-RPC code | Message              |
|----------------------------|------|---------------|----------------------|
| Missing/invalid token      | 401  | -32001        | unauthenticated      |
| Policy deny                | 403  | -32002        | forbidden            |
| Adapter/tool not found     | 404  | -32601        | not_found            |
| Conflict (name exists)     | 409  | -32010        | conflict             |
| Rate limit                 | 429  | -32003        | rate_limited         |
| Upstream unavailable       | 502  | -32004        | upstream_unavailable |
| Upstream timeout           | 504  | -32005        | upstream_timeout     |
| Validation                 | 400  | -32602        | invalid_params       |
| Internal                   | 500  | -32603        | internal_error       |

Upstream bodies and stack traces never leak to clients.

### 2.6 Identity model
```rust
pub struct UserContext {
    pub sub: String,
    pub tenant: Option<String>,
    pub roles: Vec<String>,
    pub groups: Vec<String>,
    pub scopes: Vec<String>,
    pub claims: serde_json::Value, // raw, for ABAC
}
```

### 2.7 Audit record
```json
{
  "ts": "2025-01-01T12:00:00Z",
  "trace_id": "…",
  "user": { "sub": "…", "tenant": "…", "roles": [] },
  "plane": "data",
  "action": "tools/call",
  "target": { "kind": "tool", "name": "weather" },
  "decision": "allow",
  "policy_id": "opa:bundle@42",
  "latency_ms": 42,
  "upstream_status": "ok",
  "request_hash": "sha256:…",
  "error": null
}
```

---

## 3. Pluggable Providers (the core design)

Every cloud-coupled concern is a Rust trait with multiple impls selectable via config. Binary ships with all first-party impls; custom impls can be linked as crates.

### 3.1 `IdProvider` — authentication
```rust
#[async_trait]
pub trait IdProvider: Send + Sync {
    async fn validate(&self, token: &str) -> Result<UserContext, AuthError>;
    async fn refresh_keys(&self) -> Result<(), AuthError>;
}
```
First-party impls:
- `oidc-generic` (discovery + JWKS) — Keycloak, Auth0, Okta, Cognito, Entra ID, GCP Identity Platform.
- `static-jwt` — single signing key (dev/offline).
- `header-passthrough` — trusted reverse proxy supplies claims (service-mesh mTLS).

### 3.2 `PolicyEngine` — authorization
```rust
#[async_trait]
pub trait PolicyEngine: Send + Sync {
    async fn decide(&self, input: &PolicyInput) -> Result<Decision, PolicyError>;
}
```
First-party impls:
- `yaml-rbac` (RBAC v1, see §5.1).
- `opa-sidecar` (HTTP to OPA, Rego bundles).
- `cedar-embedded` (in-process).

### 3.3 `DeploymentProvider` — lifecycle of MCP/Tool servers
```rust
#[async_trait]
pub trait DeploymentProvider: Send + Sync {
    async fn apply(&self, spec: &DeploymentSpec) -> Result<DeploymentHandle, DeployError>;
    async fn delete(&self, handle: &DeploymentHandle) -> Result<(), DeployError>;
    async fn status(&self, handle: &DeploymentHandle) -> Result<DeploymentStatus, DeployError>;
    async fn logs(&self, handle: &DeploymentHandle) -> BoxStream<'static, LogLine>;
    async fn endpoints(&self, handle: &DeploymentHandle) -> Result<Vec<Endpoint>, DeployError>;
}
```
First-party impls:
- `kubernetes` — StatefulSet + headless Service (parity with microsoft/mcp-gateway).
- `docker` — local Docker daemon (dev / single-node).
- `nomad` — HashiCorp Nomad.
- `process` — spawn subprocess (bare-metal / edge).
- `noop-external` — adapter already running, gateway only routes (for proxying external/remote MCP servers).
- *Community (roadmap)*: `cloudrun`, `ecs`, `flyio`, `lambda-fn-url`.

### 3.4 `MetadataStore` — adapter/tool registry persistence
```rust
#[async_trait]
pub trait MetadataStore: Send + Sync {
    async fn put_adapter(&self, a: &Adapter) -> Result<()>;
    async fn get_adapter(&self, name: &str) -> Result<Option<Adapter>>;
    async fn list_adapters(&self, filter: &Filter) -> Result<Vec<Adapter>>;
    async fn delete_adapter(&self, name: &str) -> Result<()>;
    // ... same for tools, with revision/etag for optimistic concurrency
}
```
First-party impls:
- `postgres` (SQLx), `sqlite` (SQLx), `redis`, `in-memory`.
- Cloud optional via features: `cosmosdb`, `firestore`, `dynamodb`.

### 3.5 `SessionStore` — session affinity for stateful MCP
```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn resolve(&self, session_id: &str, adapter: &str) -> Result<Option<BackendId>>;
    async fn bind(&self, session_id: &str, adapter: &str, backend: BackendId, ttl: Duration) -> Result<()>;
    async fn drop(&self, session_id: &str) -> Result<()>;
}
```
First-party impls: `in-memory` (single-replica), `redis`, `nats-kv`.

### 3.6 `ImageRegistry` — resolve + pull policy
Thin abstraction over OCI. Auth is config-driven (basic, bearer, cloud-keychain helper binary, IRSA/Workload Identity via env).

### 3.7 `SecretProvider` — resolve `secretRefs`
```rust
#[async_trait]
pub trait SecretProvider: Send + Sync {
    async fn get(&self, key: &SecretRef) -> Result<SecretValue>;
}
```
First-party: `env`, `file`, `k8s`, `vault`, `aws-secrets-manager`, `gcp-secret-manager`, `azure-key-vault`.

### 3.8 `AuditSink` — append-only event stream
First-party: `stdout` (default), `file` (JSONL, rotated), `object-store` (S3-compatible), `kafka`, `nats`, `postgres`.

### 3.9 `Telemetry`
OTLP generic (HTTP/gRPC). Any vendor (Jaeger, Tempo, Honeycomb, Datadog, New Relic, GCP Trace, Azure Monitor, AWS X-Ray collector).

### 3.10 Provider selection model
```yaml
providers:
  identity:
    kind: "oidc-generic"           # oidc-generic | static-jwt | header-passthrough
    config: { issuer: "...", audiences: [...] }
  authz:
    kind: "yaml-rbac"              # yaml-rbac | opa-sidecar | cedar-embedded
    config: { path: "/etc/mcp-oxide/policies.yaml" }
  deployment:
    kind: "kubernetes"             # kubernetes | docker | nomad | process | noop-external
    config: { namespace: "mcp", kubeconfig: "in-cluster" }
  metadata_store:
    kind: "postgres"               # postgres | sqlite | redis | in-memory
    config: { dsn: "postgres://..." }
  session_store:
    kind: "redis"                  # in-memory | redis | nats-kv
    config: { url: "redis://..." }
  secrets:
    kind: "vault"                  # env | file | k8s | vault | aws-sm | gcp-sm | azure-kv
    config: { addr: "https://vault:8200", auth: { kind: "kubernetes", role: "mcp" } }
  audit:
    sinks: ["stdout", "kafka"]
    config:
      kafka: { brokers: "...", topic: "mcp.audit" }
  telemetry:
    otlp_endpoint: "http://otel-collector:4317"
```

---

## 4. Example deployment matrices

Showing how the *same binary* is wired differently.

### 4.1 GCP + Keycloak
```yaml
providers:
  identity:      { kind: oidc-generic, config: { issuer: "https://keycloak.example/realms/app", audiences: ["mcp-gateway"] } }
  authz:         { kind: opa-sidecar,  config: { url: "http://localhost:8181" } }
  deployment:    { kind: kubernetes,   config: { namespace: "mcp" } }      # GKE
  metadata_store:{ kind: postgres,     config: { dsn: "$POSTGRES_DSN" } }  # Cloud SQL
  session_store: { kind: redis,        config: { url: "$REDIS_URL" } }     # Memorystore
  secrets:       { kind: gcp-sm,       config: { project: "my-proj" } }
  audit:         { sinks: ["stdout","object-store"], config: { object_store: { kind: "gcs", bucket: "mcp-audit" } } }
  telemetry:     { otlp_endpoint: "http://otel-collector:4317" }
```

### 4.2 Azure + Entra ID (microsoft/mcp-gateway parity)
```yaml
providers:
  identity:      { kind: oidc-generic, config: { issuer: "https://login.microsoftonline.com/<tenant>/v2.0", audiences: ["<app-id>"] } }
  authz:         { kind: yaml-rbac,    config: { path: "/etc/mcp-oxide/policies.yaml" } }
  deployment:    { kind: kubernetes,   config: { namespace: "adapter" } }   # AKS
  metadata_store:{ kind: postgres }                                         # or future cosmosdb feature
  session_store: { kind: redis }                                            # Azure Cache
  secrets:       { kind: azure-kv }
  audit:         { sinks: ["stdout"] }
  telemetry:     { otlp_endpoint: "..." }                                   # App Insights OTLP
```

### 4.3 AWS + Cognito
```yaml
providers:
  identity:      { kind: oidc-generic, config: { issuer: "https://cognito-idp.<region>.amazonaws.com/<pool>", audiences: ["<client>"] } }
  authz:         { kind: opa-sidecar }
  deployment:    { kind: kubernetes }                                       # EKS
  metadata_store:{ kind: postgres }                                         # RDS
  session_store: { kind: redis }                                            # ElastiCache
  secrets:       { kind: aws-sm }
  audit:         { sinks: ["stdout","kafka"] }                              # MSK
```

### 4.4 Local dev (zero cloud)
```yaml
providers:
  identity:      { kind: static-jwt,   config: { pub_key_path: "dev.pub" } }
  authz:         { kind: yaml-rbac,    config: { path: "./policies.yaml" } }
  deployment:    { kind: docker,       config: { socket: "/var/run/docker.sock" } }
  metadata_store:{ kind: sqlite,       config: { path: "./mcp-oxide.db" } }
  session_store: { kind: in-memory }
  secrets:       { kind: env }
  audit:         { sinks: ["stdout"] }
```

---

## 5. Phases

Each phase: tagged release, CHANGELOG entry, passing CI, ADR(s) merged.

### Phase 0 — Foundations (1d)
- Cargo workspace, CI (fmt, clippy `-D warnings`, test, deny, audit, trivy).
- Trait scaffolding for all providers (empty impls where applicable).
- ADR-001 Architecture (two-plane).
- ADR-002 Provider model + feature flags.
- ADR-003 Transport (JSON-RPC / SSE passthrough).
- ADR-004 Kubernetes deployment posture (StatefulSet + headless Service + leader-elected reconciler).
- OpenAPI skeleton (`openapi/mcp-oxide.openapi.yaml`).
- **Dockerfile skeleton** (distroless-nonroot, multi-stage, cargo-chef cache).
- **Helm chart skeleton** at `deploy/helm/mcp-oxide` (`helm lint` clean; templates stubbed to grow phase-by-phase).
- `kind`/`k3d` smoke job in CI from day one.

**DoD**: CI green (fmt, clippy, test, helm lint, kubeconform, trivy); `cargo run` serves `/healthz` with provider summary; `helm install` on kind succeeds.

---

### Phase 1 — Data Plane MVP (3–4d)

Scope: route traffic to **pre-registered (config-file)** adapters. No control plane yet. No deployment provider yet. The gateway does **not** deploy; it proxies to URLs you declare.

- `oidc-generic` IdProvider.
- `yaml-rbac` PolicyEngine.
- `/adapters/{name}/mcp` proxy with JSON-RPC + SSE passthrough.
- `in-memory` SessionStore; session-id header affinity.
- `noop-external` DeploymentProvider.
- Structured audit to stdout.
- Static adapter list in config for bootstrap:
  ```yaml
  static_adapters:
    - name: mcp-aws
      upstream: "http://mcp-aws:8080/mcp"
      required_roles: ["mcp.engineer"]
  ```

**DoD**: valid JWT + allowed role → streams through; denied → 403; MCP down → 502 clean.

---

### Phase 2 — Control Plane + MetadataStore (3–4d)
- CRUD for `/adapters` and `/tools`.
- `MetadataStore`: first-party `sqlite` + `postgres`.
- Optimistic concurrency via ETag.
- OpenAPI published, typed clients generated (`oapi-codegen`).
- Admin-only policy for control plane.

**DoD**: adapters added at runtime are routable without restart.

---

### Phase 3 — DeploymentProvider + Tool Router (5–6d)
- `DeploymentProvider` trait; first-party impls: `kubernetes`, `docker`, `noop-external`.
- **Kubernetes impl**: `kube` crate; adapters materialized as `StatefulSet` + headless `Service` + `ConfigMap`/`Secret` (+ optional `ServiceAccount` per adapter); ownerReferences for GC; `Event` emission; log streaming via `Pod/log` subresource; status watched via `kube::runtime::watcher`.
- Reconciler loop with `Controller::run`: desired (MetadataStore) → actual (provider), exponential backoff + status events; leader-elected via `Lease` (phase 11 promotes to production hardening).
- `/adapters/{n}/status` and `/adapters/{n}/logs` (SSE).
- **Tool Gateway Router** implemented as an in-process MCP server that:
  - Aggregates `tools/list` from registered tools.
  - Dispatches `tools/call` to the tool's endpoint.
  - Runs in N replicas with session affinity when horizontally scaled.
- `ImageRegistry` abstraction + Docker/K8s pull (K8s uses `imagePullSecrets` resolved via `SecretProvider`).

**DoD**: registering a tool via API causes it to be deployed and callable at `POST /mcp` without a gateway restart.

---

### Phase 4 — Observability & Hardening (2d)
- OTLP tracing (W3C tracecontext); spans: `http`, `authn`, `authz`, `deploy.*`, `upstream.*`.
- Prometheus metrics:
  - `gateway_requests_total{plane,action,target,decision,status}`
  - `gateway_request_duration_seconds`
  - `gateway_authz_denied_total`
  - `gateway_upstream_errors_total{kind}`
  - `gateway_deployment_reconcile_total{provider,result}`
  - `gateway_active_sse_connections`
  - `gateway_reconciler_leader{state}` (gauge)
- Graceful shutdown: `/admin/drain`, SSE drain budget, SIGTERM handler; `terminationGracePeriodSeconds` aware.
- Circuit breaker on upstream; readiness wiring to JWKS + MetadataStore + SessionStore + DeploymentProvider.
- Error normalization (table above).
- Grafana dashboard JSON committed to `deploy/helm/mcp-oxide/dashboards/`.

---

### Phase 5 — Advanced AuthZ (OPA / Cedar) (2d)
- `opa-sidecar` engine (HTTP `/v1/data/mcp/authz/allow`, timeout 50ms, configurable fallback).
- `cedar-embedded` engine.
- Policy input includes tool metadata + adapter tags + user + env.
- Decision logs correlated with audit records.

---

### Phase 6 — Protection (1–2d)
- Rate limiting (per user / tenant / tool) with `governor` + Redis option.
- Concurrency caps.
- Per-method timeouts.
- Request size limits.
- Retry (idempotent only).

---

### Phase 7 — Secrets & Registry Providers (2d)
- `SecretProvider` impls: `env`, `k8s`, `vault`, `aws-sm`, `gcp-sm`, `azure-kv`.
- Pull-secret helpers per registry (IRSA / Workload Identity / service principal / basic).
- Injection into deployment specs by the runtime provider.

---

### Phase 8 — Session Store pluggable + HA (2d)
- `redis` and `nats-kv` SessionStore.
- Multi-replica gateway with sticky routing across replicas.
- Health-checked backend pool per adapter.

---

### Phase 9 — Audit sinks (1–2d)
- `file` (rotated JSONL), `object-store` (S3/GCS/Azure via `object_store` crate), `kafka`, `nats`, `postgres`.
- Bounded channel + at-least-once + PII redaction policy.
- Optional HMAC-chained tamper-evident audit.

---

### Phase 10 — Multi-cluster / Multi-runtime Routing (optional, 2d)
- Route by tag/domain to different DeploymentProviders (e.g. some tools on K8s, others on Cloud Run).
- Federation: an upstream that is itself an mcp-oxide instance.

---

### Phase 11 — Kubernetes Production Hardening (3–4d)

Promote the K8s path from “works” to “production-ready”. Deliverables align with §8.

- **Image**: distroless-nonroot multi-stage Dockerfile; `cosign` signing; SBOM (`syft`) + SLSA L3 provenance from GitHub Actions OIDC; `trivy` gate in CI.
- **Helm chart** (`deploy/helm/mcp-oxide`) with production defaults:
  - `StatefulSet` + headless Service + ClusterIP Service.
  - `HPA` (CPU + custom metrics: p95 latency, active SSE connections).
  - `PodDisruptionBudget` (`minAvailable: 2`).
  - `NetworkPolicy` default-deny + scoped allow.
  - `ServiceMonitor` + `PodMonitor` + `PrometheusRule` (§8.12 alerts).
  - `SecurityContext` (nonroot, readOnlyRootFilesystem, drop ALL caps, seccomp RuntimeDefault).
  - Anti-affinity + topology spread constraints.
  - Gateway API `HTTPRoute` + legacy `Ingress` variants; cert-manager-ready.
  - ServiceAccount + namespace-scoped Role (opt-in ClusterRole) for the `kubernetes` DeploymentProvider.
- **Lifecycle**: startup/liveness/readiness probes; `preStop` drain hook; `/admin/drain` endpoint; SSE graceful drain up to budget; `terminationGracePeriodSeconds: 60`.
- **Leader-elected reconciler** via Kubernetes `Lease` (`kube::runtime` `LeaseLock`); metric on leader transitions.
- **Hot reload** of config on SIGHUP where safe; schema-migrated MetadataStore on boot behind lease.
- **Profiles**: `values-local.yaml`, `values-prod.yaml`, `values-gke.yaml`, `values-aks.yaml`, `values-eks.yaml`.
- **CI**: `kind`/`k3d` matrix with `helm lint`, `kubeconform`, `kube-score`, `polaris`, `kyverno test`, `conftest`, `checkov`; install chart + run smoke/integration/chaos (pod kill during SSE).
- **Docs**: `docs/kubernetes.md` (install, upgrade, RBAC, multi-tenancy, troubleshooting).

**DoD**: `helm install` on kind passes smoke; rolling update with live SSE clients loses 0 requests; NetworkPolicy blocks unauthorized egress; image scans clean; reconciler survives pod kill within 30s.

---

### Phase 12 — CRD / Operator mode (optional roadmap, 2d)
- Optional `Adapter` and `Tool` CRDs mirrored to/from MetadataStore.
- Controller built with `kube-runtime`; GitOps-friendly (`kubectl apply -f adapter.yaml`).
- Parity with REST API (both write paths converge into MetadataStore).

---

## 6. Policy model examples

### 6.1 YAML RBAC v1
```yaml
version: 1
default: deny
rules:
  # Control plane
  - plane: control
    action: "adapters.*"
    allow_roles: ["mcp.admin"]
  - plane: control
    action: "adapters.read"
    allow_roles: ["mcp.engineer", "mcp.admin"]

  # Data plane
  - plane: data
    action: "mcp.invoke"
    target_tags: ["public"]
    allow_roles: ["*"]
  - plane: data
    action: "tools/call"
    target_tags: ["mutating"]
    allow_roles: ["mcp.admin"]
  - plane: data
    action: "tools/call"
    target: "weather"
    allow_roles: ["*"]
```

### 6.2 OPA input document
```json
{
  "user":   { "sub":"...", "roles":["mcp.engineer"], "tenant":"acme", "claims":{...} },
  "action": { "plane":"data", "method":"tools/call", "tool":"weather" },
  "resource":{ "kind":"tool", "name":"weather", "tags":["public"], "required_roles":[] },
  "env":    { "ip":"1.2.3.4", "time":"...", "region":"sa-east-1" }
}
```

---

## 7. Directory Structure

```
mcp-oxide/
├── Cargo.toml                 # workspace
├── crates/
│   ├── gateway/               # binary
│   │   └── src/{main.rs, app.rs, config.rs, error.rs, routes.rs}
│   ├── core/                  # domain types, traits, errors
│   │   └── src/{adapter.rs, tool.rs, session.rs, policy.rs, audit.rs, providers.rs}
│   ├── mcp/                   # JSON-RPC + SSE client/proxy/tool router
│   ├── identity/              # oidc-generic, static-jwt, header-passthrough
│   ├── authz/                 # yaml-rbac, opa-sidecar, cedar-embedded
│   ├── deployment/            # kubernetes, docker, nomad, process, noop-external
│   ├── metadata/              # postgres, sqlite, redis, in-memory
│   ├── session/               # in-memory, redis, nats-kv
│   ├── secrets/               # env, file, k8s, vault, aws-sm, gcp-sm, azure-kv
│   ├── audit/                 # stdout, file, object-store, kafka, nats, postgres
│   └── observability/         # OTLP, metrics
├── openapi/
│   └── mcp-oxide.openapi.yaml
├── config/
│   ├── gateway.example.yaml
│   ├── policies.yaml
│   └── profiles/              # gcp-keycloak.yaml, azure-entra.yaml, aws-cognito.yaml, local-dev.yaml
├── policies/                  # rego bundles
│   └── authz.rego
├── deploy/
│   ├── Dockerfile             # multi-stage: cargo-chef → distroless-nonroot
│   ├── docker-compose.yaml
│   ├── helm/mcp-oxide/        # reference Helm chart (production-grade)
│   │   ├── Chart.yaml
│   │   ├── values.yaml
│   │   ├── values-prod.yaml
│   │   └── templates/
│   │       ├── statefulset.yaml           # gateway replicas (session affinity)
│   │       ├── service-headless.yaml      # pod-addressable for sticky routing
│   │       ├── service.yaml               # ClusterIP for clients
│   │       ├── ingress.yaml               # optional; Gateway API variant also provided
│   │       ├── httproute.yaml             # Gateway API
│   │       ├── hpa.yaml                   # CPU + custom metrics
│   │       ├── pdb.yaml                   # PodDisruptionBudget
│   │       ├── networkpolicy.yaml         # default-deny + allowlist
│   │       ├── servicemonitor.yaml        # Prometheus Operator
│   │       ├── prometheusrule.yaml        # alerts
│   │       ├── serviceaccount.yaml
│   │       ├── role.yaml / rolebinding.yaml
│   │       ├── clusterrole.yaml (opt-in)  # for cluster-scoped deploy mode
│   │       ├── configmap.yaml
│   │       ├── secret.yaml                # or ExternalSecrets refs
│   │       ├── poddisruptionbudget.yaml
│   │       ├── podmonitor.yaml
│   │       ├── tracesampling-configmap.yaml
│   │       └── tests/                     # helm test hooks
│   └── k8s/                   # raw manifests (generated from helm template)
├── tests/
│   ├── integration/
│   └── load/
├── docs/
│   ├── architecture.md
│   ├── providers.md
│   └── adr/
└── .github/workflows/ci.yml
```

### Cargo features
Each provider is a Cargo feature (default off except dev-friendly ones) so the binary compiles lean:
```
default = ["oidc-generic","yaml-rbac","sqlite","in-memory-session","stdout-audit","noop-external","docker"]
full    = [ ...everything... ]
```

---

## 8. Kubernetes Production Readiness

Kubernetes is a first-class deployment target. Everything below is shipped and enforced by the reference Helm chart in `deploy/helm/mcp-oxide`, validated in CI against a real cluster (kind/k3d + a prod-profile cluster), and documented in `docs/kubernetes.md`.

### 8.1 Container image

- **Base**: `gcr.io/distroless/static:nonroot` (static Rust binary).
- **Build**: multi-stage Dockerfile with `cargo-chef` for dependency caching; `-C target-feature=+crt-static`.
- **Size target**: < 40 MB compressed, < 80 MB uncompressed.
- **User**: `nonroot` (uid 65532), no shell, no package manager.
- **Filesystem**: `readOnlyRootFilesystem: true`; writable paths limited to `emptyDir`-mounted `/tmp` and `/var/cache/mcp-oxide`.
- **Supply chain**:
  - Pinned to digest in Helm `values.yaml`.
  - SBOM generated (`syft`), attached via `cosign attest`.
  - Provenance attestation (SLSA level 3) from GitHub Actions OIDC.
  - Image signed with `cosign` (keyless). Chart references signature verification via Kyverno/Cosign policies.
  - `cargo audit` + `cargo deny` + `trivy` gates in CI; build fails on HIGH+ CVEs.
- **Labels**: OCI labels (`org.opencontainers.image.*`) populated from CI.
- **Multi-arch**: `linux/amd64` + `linux/arm64`.

### 8.2 Workload topology

- **Gateway** deployed as `StatefulSet` with a **headless Service** so peers (and the LB) can address individual pods for session stickiness.
- **Replicas**: default 3; `updateStrategy: RollingUpdate` with `maxUnavailable: 1`.
- **Tool Gateway Router** runs either (a) in-process in each gateway replica (default, simpler) or (b) as a separate `StatefulSet` for independent scaling (optional values toggle).
- **Reconciler**: leader-elected via `Lease` object (one active reconciler across replicas; others stand by). Uses `kube` crate's `Controller::run` with `LeaseLock`.
- **Adapter/Tool backends**: created by the `kubernetes` DeploymentProvider as `StatefulSet` + headless Service in a configurable namespace (default `mcp-workloads`), labeled `mcp-oxide.io/adapter=<name>` for selection and NetworkPolicy scope.

### 8.3 Probes & graceful lifecycle

- **Startup probe**: `/healthz/startup` — 60s window for JWKS fetch + store ping + deployment-provider handshake.
- **Liveness probe**: `/healthz/live` — checks event loop, no deadlocks; tolerant of transient upstream issues.
- **Readiness probe**: `/healthz/ready` — fails when:
  - JWKS unavailable AND no cached keys (strict mode), OR
  - MetadataStore unhealthy, OR
  - SessionStore unhealthy, OR
  - Gateway draining.
- **Graceful shutdown**:
  - `terminationGracePeriodSeconds: 60`.
  - `preStop` hook: `sleep 5` (let endpoints converge), then POST `/admin/drain` to stop accepting new connections, let in-flight JSON-RPC + SSE complete (up to a configurable budget).
  - SIGTERM → drain mode → close listeners → flush audit sink → exit.
- **PDB**: `minAvailable: 2` (for 3 replicas) to protect during node drains.

### 8.4 Pod security

- **Pod Security Standards**: `restricted`.
- **SecurityContext**:
  ```yaml
  securityContext:
    runAsNonRoot: true
    runAsUser: 65532
    runAsGroup: 65532
    fsGroup: 65532
    seccompProfile: { type: RuntimeDefault }
  containers:
    - securityContext:
        allowPrivilegeEscalation: false
        readOnlyRootFilesystem: true
        capabilities: { drop: ["ALL"] }
  ```
- **AppArmor**: `runtime/default` annotation.
- **Admission**: chart is compatible with OPA Gatekeeper / Kyverno baseline + restricted constraints.

### 8.5 Resource management

- **Default requests/limits** (values.yaml):
  ```yaml
  resources:
    requests: { cpu: "250m", memory: "256Mi" }
    limits:   { cpu: "2",    memory: "1Gi"  }
  ```
- **QoS class**: Burstable by default; `Guaranteed` profile available via values.
- **Vertical recommendations** emitted as VPA `Recommender`-compatible annotations (optional).

### 8.6 Horizontal autoscaling

- **HPA v2** scaling on:
  - `cpu` 70 % target,
  - `gateway_request_duration_seconds` p95 via Prometheus Adapter (optional),
  - `gateway_active_sse_connections` (custom metric) — scales on sticky connection load.
- **min/max replicas** configurable (default 3–20).
- **Scale-down stabilization**: 5 min to avoid thrashing during SSE drains.

### 8.7 Scheduling & resilience

- **Topology spread** across zones (`topology.kubernetes.io/zone`) with `maxSkew: 1`, `whenUnsatisfiable: ScheduleAnyway`.
- **Anti-affinity**: soft pod anti-affinity by hostname.
- **Node selectors / tolerations** configurable.
- **PriorityClass**: values include a `system-cluster-critical`-adjacent option for platform installs.
- **Graceful node drain**: PDB + drain endpoint make `kubectl drain` safe.

### 8.8 Networking

- **Service**: `ClusterIP` + headless companion for sticky routing.
- **Ingress**: chart ships both `Ingress` (networking.k8s.io/v1) and **Gateway API** (`HTTPRoute`) variants; users pick one.
- **TLS**: cert-manager-compatible annotations + `Certificate` resource (optional dependency `cert-manager`).
- **NetworkPolicy** (default-deny posture):
  - Ingress: allow from configured Ingress namespace + Prometheus + health-check ranges.
  - Egress: allow to IdP issuer, OPA sidecar, MetadataStore, SessionStore, SecretProvider, OTLP collector, Kubernetes API (for deployment provider), configured adapters/tools namespace.
- **Service mesh** ready: annotations for Istio / Linkerd injection; mTLS compatible (and recommended for data plane to backends).
- **Gateway API**: `HTTPRoute` with header-based match for control plane (`/adapters`, `/tools`) separated from data plane (`/mcp`, `/adapters/*/mcp`) to enable per-plane rate limits and auth filters.

### 8.9 In-cluster RBAC (for `kubernetes` DeploymentProvider)

- Dedicated `ServiceAccount` with minimal RBAC:
  - `adapters.mcp-oxide.io` scope namespace (default `mcp-workloads`): full control over `Deployment`/`StatefulSet`/`Service`/`ConfigMap`/`Secret`/`Pod/log`/`Pod/exec` (log tailing).
  - Own namespace: `Lease` (leader election), `Event` (create), `ConfigMap`/`Secret` (read).
- **Two modes** (toggle in values):
  - `namespace-scoped` (default, `Role` + `RoleBinding`).
  - `cluster-scoped` (`ClusterRole` + `ClusterRoleBinding`) for managing adapters in multiple namespaces.
- Permissions enumerated and tested with `kubectl auth can-i`-based assertions in CI.
- Optional integration with IRSA (EKS), Workload Identity (GKE), Azure Workload Identity (AKS) for cloud SDK credentials.

### 8.10 Configuration & secrets

- **Config**: `ConfigMap` mounted at `/etc/mcp-oxide/gateway.yaml`; SIGHUP hot-reload where safe.
- **Secrets**: either native `Secret` or references to `ExternalSecrets` / `SecretProviderClass` (CSI Secrets Store). Chart supports both without opinion.
- **Env overlays**: env vars use `MCP_OXIDE__` prefix with `__` as separator (matches `figment`/`config-rs`).

### 8.11 State, persistence & multi-tenancy

- **Stateless gateway**: all state in `MetadataStore` + `SessionStore`.
- **PVCs**: none for the gateway by default. Optional emptyDir for local audit spooling.
- **Multi-tenancy**:
  - Tenant claim required (configurable).
  - Workload namespace can be per-tenant (`mcp-workloads-<tenant>`) with `NetworkPolicy` isolation.
  - Chart supports multi-release installs in the same cluster with non-conflicting cluster-scoped resources.
- **CRD / Operator mode (roadmap)**: optional `Adapter` / `Tool` CRDs + controller mirroring MetadataStore, enabling GitOps (`kubectl apply -f adapter.yaml`) without losing CRUD-API parity.

### 8.12 Observability on Kubernetes

- **Prometheus**: `/metrics` on port `9090`; `ServiceMonitor` + `PodMonitor` templates; standard labels (`app.kubernetes.io/*`).
- **PrometheusRule**: ships alerts for:
  - readiness flapping,
  - error ratio > 1 % (5m),
  - p95 latency budget breach,
  - JWKS refresh failures,
  - reconciler leader loss,
  - deployment-provider errors,
  - audit sink backpressure / dropped events.
- **Tracing**: OTLP to collector (Service or DaemonSet) via env var; W3C tracecontext preserved across hops including upstream MCP.
- **Logs**: JSON to stdout; compatible with Fluent Bit / Vector / Loki / CloudWatch / Stackdriver.
- **Dashboards**: Grafana JSON dashboard committed to `deploy/helm/mcp-oxide/dashboards/`.

### 8.13 Deployment profiles (Helm)

- `values-local.yaml` — kind/k3d, in-memory providers.
- `values-prod.yaml` — 3 replicas, HPA on, NetworkPolicy strict, PDB, distroless-nonroot, ServiceMonitor, PodSecurity `restricted`.
- `values-gke.yaml`, `values-aks.yaml`, `values-eks.yaml` — cloud specifics (Workload Identity annotations, ingress class, StorageClass hints for optional DB sidecars).

### 8.14 Upgrade & rollback

- **Rolling updates**: compatible with SSE via drain hook + PDB.
- **Helm tests** (`helm test`): spin ephemeral client pods, run smoke JSON-RPC flow.
- **Migrations**: MetadataStore schema managed by `sqlx::migrate!`; runs on boot behind a lease; rollback via `--version` pinning + backward-compatible schema policy (N and N-1 compatible).
- **Blue/green**: supported via two Helm releases behind a weighted HTTPRoute; session store shared.

### 8.15 CI validation for K8s

- `kind` + `k3d` matrix jobs:
  - `helm lint`, `helm template | kubeconform`, `kube-score`, `polaris`, `kyverno test`, `checkov`.
  - Install chart, run smoke + integration tests.
  - Chaos Mesh optional job: kill gateway pod mid-SSE, assert client reconnects.
- `conftest` with Rego rules guarding against regressions (no `latest` tag, no privileged, no hostPath, …).

### 8.16 Compliance posture
- PodSecurity `restricted`, NetworkPolicy default-deny, SBOM + signed images, SLSA L3 provenance, audit logs with optional HMAC chain → aligns with CIS Kubernetes Benchmark, NIST SP 800-190, and SOC 2 CC7/CC8 controls.

---

## 9. Non-Functional Requirements

| Attribute         | Target                                                         |
|-------------------|----------------------------------------------------------------|
| Proxy overhead    | p50 < 5ms, p95 < 20ms, p99 < 50ms (excluding upstream)         |
| Throughput        | ≥ 5k RPS per instance on 4 vCPU (non-streaming)                |
| Control-plane CRUD| p95 < 100ms (excluding deployment provider)                    |
| Availability      | 99.9 % per month with 3 replicas + PDB; graceful drain ≤ 30s    |
| Security          | Alg allowlist, aud/iss pinning, no claim values in logs        |
| Memory            | < 250 MB steady, < 500 MB peak                                  |
| Cold start        | < 500 ms to ready (warm JWKS + store reachable)                 |
| Image size        | < 40 MB compressed (distroless static)                          |
| K8s upgrade       | Zero-downtime rolling update at 3 replicas; SSE drained cleanly |
| Reconciler        | Survives pod restart; convergence ≤ 30s after leader re-elect   |

---

## 10. Threat Model (STRIDE, abbreviated)
- **Spoofing**: strict OIDC validation (kid required, alg allowlist, iss/aud pinning).
- **Tampering**: TLS to all providers; optional HMAC-chained audit; signed container images + SLSA provenance.
- **Repudiation**: every data-plane and control-plane call audited with trace_id + request_hash.
- **Information disclosure**: redaction; no upstream error bodies to clients; secrets never logged.
- **DoS**: body size, timeouts, rate limits, concurrency caps, SSE connection caps; PDB + HPA for capacity.
- **EoP**: default-deny policy; role-claim paths explicit; separate admin scope for control plane; least-privilege K8s RBAC for DeploymentProvider.

---

## 11. Testing Strategy
- **Unit**: policy eval matrix, JWT edge cases, JSON-RPC error mapping, reconciler state machine, each provider’s contract.
- **Provider contract tests**: reusable test suite runs against every impl of a trait.
- **Integration**: `axum::TestServer` + Dockerized: OPA, Postgres, Redis, MinIO, Keycloak, Vault, mock MCP.
- **E2E on Kubernetes**:
  - `kind` + `k3d` matrix in CI.
  - Install via Helm, run smoke + scenario suites (adapter CRUD, tool register → call, multi-replica session affinity, graceful SSE drain on rollout, NetworkPolicy enforcement).
  - Chart validation: `helm lint`, `kubeconform`, `kube-score`, `polaris`, `kyverno test`, `checkov`, `conftest`.
- **Property**: RBAC monotonic under role addition (`proptest`).
- **Load**: k6 / goose — burst, sustained, slow-SSE; autoscaling behavior verified.
- **Chaos**: MCP down, JWKS down, OPA down, deployment provider unreachable, DB partitioned, gateway pod killed mid-SSE (Chaos Mesh).
- **Security**: `cargo audit`, `cargo deny`, fuzz JSON-RPC parser (`cargo fuzz`), `trivy` image scan, `cosign verify` in CI gate.

---

## 12. Risks & Mitigations

| Risk                                     | Mitigation                                                  |
|------------------------------------------|-------------------------------------------------------------|
| Provider surface becomes unbounded       | Strict trait contracts + shared contract test suite         |
| K8s deployment complexity                | Reconciler isolated behind trait; swappable with `docker`/`process` |
| Policy complexity explosion              | Default-deny + OPA bundle tests + deny-reason metrics       |
| Session affinity under rolling restarts  | External SessionStore (Redis/NATS); drain hook; PDB         |
| Cloud SDK bloat                          | Every provider behind Cargo feature                         |
| Multi-tenant isolation bugs              | Tenant claim mandatory (configurable); per-tenant namespace + NetworkPolicy; isolation tests |
| Helm chart drift vs binary behavior      | Chart and binary versioned together; CI installs chart against built image |
| Reconciler split-brain                   | `Lease`-based leader election; observability on leader transitions |

---

## 13. Rollout Plan
1. **Shadow mode**: deploy gateway alongside an existing MCP; mirror read-only traffic, log decisions, don't enforce.
2. **Enforce read-only tools** first (`readOnly: true`).
3. **Enforce mutating tools** gradually.
4. **Control-plane onboarding**: migrate one-off servers behind `noop-external`, then convert to managed.
5. **Cutover**: gateway is the only ingress to MCP servers (NetworkPolicy + cloud security groups lockdown).
6. **K8s-specific rollout** stages: (a) `kind` smoke → (b) dev cluster with `values-local.yaml` → (c) staging with `values-prod.yaml` minus HPA → (d) production with full `values-prod.yaml`.

---

## 14. Milestones (effort)

| Phase | Effort | Gate                                         |
|-------|--------|----------------------------------------------|
| 0     | 1d     | Workspace + traits + CI green                |
| 1     | 3–4d   | Data-plane MVP w/ static adapters            |
| 2     | 3–4d   | Control-plane CRUD + metadata store          |
| 3     | 5–6d   | Deployment providers + tool router           |
| 4     | 2d     | Observability + hardening                    |
| 5     | 2d     | OPA / Cedar                                  |
| 6     | 1–2d   | Protection                                   |
| 7     | 2d     | Secrets + registry providers                 |
| 8     | 2d     | Pluggable session store + HA                 |
| 9     | 1–2d   | Audit sinks                                  |
| 10    | 2d     | Multi-runtime routing (optional)             |
| 11    | 3–4d   | **K8s production hardening**: Helm chart (HPA/PDB/NetworkPolicy/ServiceMonitor/PrometheusRule), distroless image + cosign + SBOM + SLSA, leader-elected reconciler, drain hook, kind/k3d E2E in CI |
| 12    | 2d     | CRD/Operator mode (optional, roadmap)        |

Critical path to production-Kubernetes-ready cross-cloud: **~25–32 dev-days**.

---

## 15. Next Step
Start Phase 0: workspace + trait scaffolding + CI + base Dockerfile (distroless-nonroot) + empty Helm chart skeleton so K8s posture grows phase-by-phase, not as a last-minute add-on.
Motto: **Provider-first. Cloud-agnostic. Kubernetes-native. Ship → observe → iterate.**
