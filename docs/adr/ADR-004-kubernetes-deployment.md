# ADR-004: Kubernetes deployment posture

- Status: Accepted
- Date: 2026-04-29

## Context

Kubernetes is a first-class deployment target (see PLAN.md §8). The gateway
must (a) run HA with session-affinity stickiness, (b) optionally act as a
controller that materializes adapters/tools as K8s workloads when using
the `kubernetes` `DeploymentProvider`, and (c) stay compatible with
restricted Pod Security Standards.

## Decision

1. **Gateway workload**: `StatefulSet` + headless `Service` (plus a regular
   `ClusterIP` `Service` for clients).
   - StatefulSet gives stable per-pod DNS names, which the headless
     Service exposes. A future external session-affinity layer can map
     `session_id → pod-N.headless` without a shared session store.
2. **Container image**: `gcr.io/distroless/cc-debian12:nonroot`, static
   binary, non-root (uid 65532), `readOnlyRootFilesystem: true`, all
   capabilities dropped, seccomp `RuntimeDefault`. Compatible with the
   `restricted` PodSecurity admission profile.
3. **Lifecycle**:
   - `terminationGracePeriodSeconds: 60`.
   - `preStop` sleep to let Service endpoints converge before the process
     starts draining.
   - Graceful shutdown on `SIGTERM` with SSE drain (Phase 1+ will honor a
     configurable budget; Phase 11 promotes it to production-grade).
   - Startup / liveness / readiness probes separated
     (`/healthz/startup`, `/healthz/live`, `/readyz`).
4. **HA**: `PodDisruptionBudget` with `minAvailable: 2` at 3 replicas;
   anti-affinity preferred by hostname; topology spread across zones.
5. **Reconciler leadership** (Phase 3+): Kubernetes `Lease`-based leader
   election (`kube-runtime` `LeaseLock`) so only one replica drives
   reconciliation at a time.
6. **Workloads managed by the gateway** (adapters/tools) are materialized
   as `StatefulSet` + headless Service in a configurable namespace
   (`mcp-workloads`), labeled `mcp-oxide.io/adapter=<name>` for scoping
   and NetworkPolicy isolation.
7. **Security boundary**: dedicated `ServiceAccount` with a namespace-
   scoped `Role` over the workloads namespace. Optional cluster-scoped
   mode for multi-namespace deployments.

## Consequences

- The Helm chart is the canonical deployment artifact and is validated in
  CI against kind/k3d from Phase 0.
- Some operators prefer Deployments over StatefulSets; we accept the minor
  trade-off because stable pod identities simplify session affinity and
  leader election.
- No CRDs in v1 — state lives in `MetadataStore`. A future Operator mode
  (Phase 12) will add optional CRDs mirroring the same state.

## Alternatives considered

- **`Deployment` + ConsistentHash session router**: works but requires a
  shared session store from day one; StatefulSet keeps single-replica dev
  viable.
- **Operator-first architecture**: rejected as default because it forces
  a K8s dependency on the control surface, contradicting the
  cloud-agnostic goal.
