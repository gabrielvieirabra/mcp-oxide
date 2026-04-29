# Changelog

All notable changes to this project will be documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Added — Phase 0: Foundations

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
