# ADR-003: MCP transport — JSON-RPC 2.0 over streamable HTTP + SSE

- Status: Accepted
- Date: 2026-04-29

## Context

The Model Context Protocol uses JSON-RPC 2.0. Two transports are widely
deployed: stdio (subprocess) and streamable HTTP (POST for requests, SSE
for server-initiated notifications and streamed results). Gateways operate
naturally over the network; stdio would require the gateway to spawn every
backend, which breaks our `DeploymentProvider` abstraction.

## Decision

mcp-oxide speaks **MCP JSON-RPC 2.0 over streamable HTTP** end-to-end on
both client-facing and upstream-facing sides:

- `POST /adapters/{name}/mcp` and `POST /mcp` accept JSON-RPC requests.
- Server-initiated streams use `text/event-stream` (SSE) with W3C
  `traceparent` header propagation.
- JSON-RPC `id` is preserved verbatim through the proxy; no renumbering.
- JSON-RPC errors are mapped to HTTP status codes per the table in PLAN.md
  §2.2 without leaking upstream bodies.
- For backends exposing stdio MCP, a separate out-of-scope adapter
  (`mcp-proxy` style bridge) would wrap them into HTTP — not our concern
  in v1.

## Consequences

- Gateway is horizontally scalable (no per-process backend ownership).
- SSE forces careful lifecycle handling (graceful drain, per-connection
  timeouts, concurrency caps) — tracked in Phase 11 K8s hardening.
- Clients must use an MCP client that supports streamable HTTP.

## Alternatives considered

- **stdio passthrough**: would require an MCP subprocess per gateway
  replica, conflicting with the `DeploymentProvider` trait and with
  Kubernetes-native scaling.
- **WebSocket**: not part of the current MCP spec surface; SSE is
  already the de-facto streaming transport.
