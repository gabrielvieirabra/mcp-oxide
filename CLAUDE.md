# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Common commands

The `justfile` is the canonical task runner — `just --list` for the menu.

- `just test` — `cargo test --all-targets` across the workspace.
- `just test-full` — adds the `docker` feature (exercises the Docker `DeploymentProvider`).
- `cargo test -p <crate> <name>` — single test, e.g. `cargo test -p mcp-oxide-gateway proxy::`.
- `just lint` — clippy gate; runs **twice**, once with `--features docker` and once with `--no-default-features`. Both must pass with `-D warnings`. Pedantic lints are on workspace-wide.
- `cargo build --release` — produces the single static `mcp-oxide` binary.
- Smoke stack (docker compose): `just smoke-up` / `smoke-logs` / `smoke-down`. Gateway lands on `:8080`, mock MCP backends on `:18090/18091/18092`.
- `just smoke-token <sub> <roles>` — mints an HS256 token signed with `deploy/smoke/jwt.key` for hitting the live smoke stack. Requires `jq` + `openssl`.
- Toolchain pinned via `rust-toolchain.toml` (MSRV 1.88).

## Architecture

Read the ADRs in `docs/adr/` before making cross-cutting changes — they document constraints that aren't visible from the code alone. `PLAN.md` is the authoritative spec.

### Two planes, one binary (ADR-001)

The gateway binary serves two logical planes behind one HTTP listener, with the same auth/authz layers but distinct policy `Action` values:

- **Control plane** — REST CRUD for adapters and tools, admin-scoped. `crates/gateway/src/routes/control_plane.rs`.
- **Data plane** — MCP JSON-RPC 2.0 over streamable HTTP + SSE proxy with session affinity. `routes/data_plane.rs` for `/adapters/{name}/mcp`; `routes/tool_router.rs` for the aggregating `/mcp` endpoint.

Policy authors must distinguish planes; the `plane` field is part of the policy input. JSON-RPC `id`s are preserved verbatim through the proxy — never renumber. JSON-RPC errors map to HTTP per PLAN.md §2.2; do not leak upstream bodies.

### Provider model (ADR-002)

Every cloud-coupled concern is a **trait in `mcp-oxide-core`** with implementations gated behind Cargo features in dedicated crates:

| Trait | Crate |
|---|---|
| `IdProvider` | `crates/identity` |
| `PolicyEngine` | `crates/authz` |
| `DeploymentProvider` | `crates/deployment` |
| `MetadataStore` | `crates/metadata` |
| `SessionStore` | `crates/session` |
| `SecretProvider` | `crates/secrets` |
| `AuditSink` | `crates/audit` |
| `Telemetry` | `crates/observability` |

The gateway picks providers at compile time (features) and selects among the compiled-in set at runtime (`kind:` in config). Default fallbacks are **secure-by-default** — `NoopIdProvider` rejects all tokens and `DenyAllPolicyEngine` denies all requests, so misconfigured deployments fail closed. New providers go in their owning crate as a feature, not the gateway. `crates/testing` carries shared contract test fixtures.

### Crate layout

- `crates/core` — traits, domain types (`Adapter`, `Tool`, `Session`, `Policy*`), error types. No I/O.
- `crates/mcp` — MCP JSON-RPC 2.0 codec. Transport-agnostic.
- `crates/gateway` — Axum app, routing, middleware, proxy. Library + thin `main.rs`. `app::router` is the composition root; `AppState` holds the wired providers.
- The Helm chart (`deploy/helm/mcp-oxide`) is the canonical deployment artifact; `deploy/smoke/` is the docker-compose smoke stack used by `just smoke-*`.

### Transport (ADR-003)

End-to-end JSON-RPC 2.0 over streamable HTTP. SSE for server-initiated streams with W3C `traceparent` propagation. stdio MCP backends are out of scope — wrap them externally. SSE lifecycle (graceful drain, per-connection timeouts, concurrency caps) is load-bearing; touch with care.

### Kubernetes posture (ADR-004)

Gateway runs as a `StatefulSet` + headless Service (stable pod DNS for session affinity without a shared session store on the single-replica path). Distroless `cc-debian12:nonroot`, read-only rootfs, all caps dropped, `restricted` PodSecurity-compatible. Health endpoints are split: `/healthz/startup`, `/healthz/live`, `/readyz`. Managed workloads land in a configurable namespace (`mcp-workloads` by default) labeled `mcp-oxide.io/adapter=<name>`. No CRDs in v1 — state lives in `MetadataStore`.

## Conventions

- `unsafe_code = "forbid"` workspace-wide.
- Clippy `pedantic` is warn-by-default; CI runs `-D warnings`. Both feature combinations (`--features docker` and `--no-default-features`) must stay clean.
- Don't add new top-level dependencies in member crates — declare in `[workspace.dependencies]` in the root `Cargo.toml` and inherit with `workspace = true`.
- The OpenAPI spec at `openapi/mcp-oxide.openapi.yaml` is the contract for the control plane; keep it in sync with route changes.
