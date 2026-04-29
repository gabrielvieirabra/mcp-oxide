# ADR-001: Two-plane architecture

- Status: Accepted
- Date: 2026-04-29
- Deciders: mcp-oxide contributors

## Context

mcp-oxide must (a) manage the lifecycle of N MCP server backends and (b)
route client traffic to them with session affinity. These two concerns have
very different API shapes (CRUD-on-state vs. streaming proxy), rate-limit
profiles, authorization semantics (admin vs. end-user), and SLOs.

The `microsoft/mcp-gateway` reference project separates these into a
*control plane* and a *data plane*, which matches well-known patterns in
service meshes and API gateways.

## Decision

mcp-oxide exposes two logical planes behind the same HTTP listener:

1. **Control plane** — RESTful CRUD for *adapters* (MCP servers) and
   *tools*. Protected by an admin scope; writes require elevated roles.
   Routes: `/adapters{/name{/status,/logs}}`, `/tools{/name{/status,/logs}}`.
2. **Data plane** — streamable HTTP / SSE MCP JSON-RPC proxy with
   session-aware routing. Routes: `/adapters/{name}/mcp` (targeted) and
   `/mcp` (tool gateway router that aggregates registered tools).

Both planes share the same authentication layer (`IdProvider`) and the same
authorization layer (`PolicyEngine`) but with distinct `Action` values in
the policy input so that operators can write separate rules (e.g. RBAC for
control plane, ABAC on tool tags for data plane).

## Consequences

- Routes and middleware stacks can be specialized per plane (e.g. stricter
  rate limits on data plane; admin-only policy default on control plane).
- A single binary still handles both; no split deployment required in v1.
- Gateway API `HTTPRoute` can easily split the planes behind different
  policies (documented in the Helm chart).
- Policy authors must be aware of the `plane` field — documented in
  policies.yaml examples.

## Alternatives considered

- **Single plane with method-based authz**: simpler but conflates user and
  admin surfaces; made rate limiting and CORS harder to reason about.
- **Two binaries**: clean separation but doubles operational cost for no
  initial value.
