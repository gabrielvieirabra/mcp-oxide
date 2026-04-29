# ADR-002: Provider model and feature flags

- Status: Accepted
- Date: 2026-04-29

## Context

mcp-oxide must stay cloud-agnostic while shipping first-class integrations
for common clouds (GCP, AWS, Azure) and on-prem stacks (Keycloak, Vault,
Postgres, Redis, Docker, Kubernetes, Nomad). Bundling every SDK into a
single binary causes bloat, slow compile times, and unnecessary CVE
exposure. Not bundling them forces users to compile from source.

## Decision

Every cloud-coupled concern is a **trait in `mcp-oxide-core`** with one
concrete implementation per crate/feature:

- `IdProvider` (identity)
- `PolicyEngine` (authorization)
- `DeploymentProvider` (runtime)
- `MetadataStore` (persistence for adapters/tools)
- `SessionStore` (affinity)
- `SecretProvider`
- `AuditSink`
- `ImageRegistry`
- `Telemetry`

Each implementation crate (`crates/identity`, `crates/authz`, …) declares
**Cargo features per provider**, off by default unless dev-friendly:

```
default = ["oidc-generic","yaml-rbac","sqlite","in-memory-session","stdout-audit","noop-external","docker"]
full    = [ ...all providers... ]
```

The gateway binary picks a compile-time set of features and a runtime
config (`kind: …`) to select among the compiled-in implementations. Unused
providers are linker-removed.

Default provider fallbacks are **secure-by-default**: `NoopIdProvider`
(reject every token) and `DenyAllPolicyEngine` (default-deny) are wired at
boot when no real providers are configured, so a misconfigured deployment
fails closed.

## Consequences

- Lean default builds (< 40 MB distroless image).
- Users can write out-of-tree providers by depending only on
  `mcp-oxide-core`.
- Every provider ships a **contract test suite** (added in later phases)
  that validates trait semantics uniformly.
- Runtime provider selection is captured in `/healthz` (`providers` map)
  for debuggability.

## Alternatives considered

- **Dynamic loading (dylib / WASM)**: adds cross-platform complexity; deferred.
- **gRPC plugin protocol à la Vault**: increases the attack surface inside
  the gateway process; deferred.
