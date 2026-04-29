# Changelog

All notable changes to this project will be documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Added — Data plane MVP

- **OIDC-generic IdProvider**: discovery (`/.well-known/openid-configuration`),
  JWKS cache with TTL + on-demand rotation, alg allowlist, iss/aud/exp/nbf
  validation, configurable claim paths (RSA, EC, Ed25519).
- **Static-jwt IdProvider**: HS*/RS*/ES*/Ed25519 from a local key for dev,
  tests, and trusted-mesh setups.
- **YAML RBAC PolicyEngine**: plane-aware rules, wildcard + prefix glob
  action matching, target tags, `*` wildcard role, default-deny.
- **Gateway library crate** (`mcp_oxide_gateway`): router factory, shared
  `AppState` with a test-friendly builder, exposed so integration tests and
  embedders consume the real code paths.
- **Auth extractor** building `UserContext` from `Authorization: Bearer`.
- **Data-plane proxy** `POST /adapters/{name}/mcp`:
  - JSON-RPC body forwarded with hop-by-hop headers filtered,
  - response body streamed (SSE passes through with `X-Accel-Buffering: no`
    and `Cache-Control: no-cache`),
  - bearer token is **not** forwarded upstream.
- **Error normalization**: upstream unavailable/timeout map to 502/504 with
  `{error, message}` JSON; never leaks upstream bodies.
- **Audit emission** for every data-plane call: trace_id, user, plane,
  action, target, decision, policy_id, latency_ms, upstream_status,
  request_hash (sha256), error.
- Extended config surface: `providers.identity`, `providers.authz`,
  `upstream.*`, `static_adapters[]`.
- Updated `config/gateway.example.yaml` with commented OIDC + static-jwt
  examples.

### Tests
- Identity: claim extraction (Keycloak shape), HS256 roundtrip, bad-issuer
  rejection.
- Authz: allow + default-deny matrix, action glob matching.
- Gateway unit: /healthz, /readyz, missing-token 401.
- Gateway integration (7 scenarios, real HTTP mock upstream):
  allowed request is proxied & token is not leaked, denied role → 403,
  missing token → 401, bad signature → 401, unknown adapter → 404,
  upstream unavailable → 502, wildcard role still gated by `required_roles`.

### Changed
- Bumped MSRV to 1.88 (ecosystem churn around time/icu crates).
- Split gateway into `lib` + `bin`; `main.rs` is now a thin wrapper.

### Added — Foundations

- Cargo workspace with 11 crates: `core`, `mcp`, `identity`, `authz`,
  `deployment`, `metadata`, `session`, `secrets`, `audit`, `observability`,
  `gateway`.
- Domain types and provider traits (`IdProvider`, `PolicyEngine`,
  `DeploymentProvider`, `MetadataStore`, `SessionStore`, `SecretProvider`,
  `AuditSink`, `ImageRegistry`).
- Secure-by-default provider stubs: `NoopIdProvider` (reject),
  `DenyAllPolicyEngine`, `InMemoryMetadataStore`, `InMemorySessionStore`,
  `EnvSecretProvider`, `StdoutAuditSink`, `NoopExternalProvider`.
- Gateway binary `mcp-oxide` with `/healthz`, `/healthz/startup`,
  `/healthz/live`, `/readyz`, `/livez`, `/`.
- Graceful shutdown on SIGINT/SIGTERM.
- OpenAPI 3.1 skeleton.
- Distroless-nonroot multi-stage Dockerfile (cargo-chef).
- Helm chart `deploy/helm/mcp-oxide` with StatefulSet + headless Service,
  PDB, optional HPA/NetworkPolicy/ServiceMonitor/PrometheusRule/Ingress/
  HTTPRoute, and `values-local.yaml` / `values-prod.yaml` profiles.
- ADR-001 (two-plane), ADR-002 (provider model), ADR-003 (transport),
  ADR-004 (Kubernetes posture).
- GitHub Actions CI: fmt, clippy `-D warnings`, test, cargo-deny, helm
  lint + template, kind smoke install, trivy image scan.
