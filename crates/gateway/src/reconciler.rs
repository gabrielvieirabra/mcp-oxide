//! In-process reconciler that drives the `DeploymentProvider` toward the
//! desired state stored in the `MetadataStore`.
//!
//! Phase 3.5a baseline: a single tokio task that wakes on a fixed interval,
//! lists every registered adapter/tool, asks the provider for the current
//! `DeploymentStatus`, and persists the result via `record_status`. On
//! `ready == false` (or any provider error) it re-applies the deployment so
//! drift caused by external killswitches converges back to the desired state.
//!
//! Multi-replica gateways gate this loop on `LeaderLock::is_leader` so only
//! one replica drives mutations against the provider; followers serve traffic
//! without poking the cluster.
//!
//! Future sub-phases extend this:
//! * 3.5b — replaces fixed polling with a kube `watcher` for the K8s provider.
//! * 3.5c — promotes the leader lock from `InProcessLeader` to a real K8s `Lease`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use mcp_oxide_core::providers::{
    DeploymentHandle, DeploymentKind, DeploymentSpec, DeploymentStatus, DeploymentStatusRecord,
    Filter, LeaderLock,
};
use parking_lot::Mutex;
use tokio::task::JoinHandle;

use crate::state::AppState;

/// Always-leader implementation. Picked when the gateway runs single-replica
/// or behind a load balancer that does not need lease coordination.
#[derive(Debug, Default)]
pub struct InProcessLeader;

#[async_trait]
impl LeaderLock for InProcessLeader {
    async fn is_leader(&self) -> bool {
        true
    }
    fn kind(&self) -> &'static str {
        "in-process"
    }
}

/// Settings driving the reconciler loop. Defaults are deliberately conservative
/// so a misconfigured deployment doesn't burn the provider's API budget.
#[derive(Debug, Clone)]
pub struct ReconcilerConfig {
    pub interval: Duration,
    pub max_attempts_before_giving_up: u32,
    /// Initial delay used by the per-target backoff. Doubles up to
    /// `interval` after each failed reapply.
    pub backoff_initial: Duration,
}

impl Default for ReconcilerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(10),
            max_attempts_before_giving_up: 5,
            backoff_initial: Duration::from_secs(2),
        }
    }
}

/// Per-(kind, tenant, name) bookkeeping for backoff. Lives only in memory —
/// the persistent `record_status` is the audit trail.
#[derive(Debug, Default, Clone)]
struct AttemptState {
    consecutive_failures: u32,
    next_attempt_after: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct Reconciler {
    state: AppState,
    leader: Arc<dyn LeaderLock>,
    cfg: ReconcilerConfig,
    attempts: Arc<Mutex<HashMap<(DeploymentKind, Option<String>, String), AttemptState>>>,
}

impl std::fmt::Debug for Reconciler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reconciler")
            .field("leader", &self.leader.kind())
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl Reconciler {
    #[must_use]
    pub fn new(state: AppState, leader: Arc<dyn LeaderLock>, cfg: ReconcilerConfig) -> Self {
        Self {
            state,
            leader,
            cfg,
            attempts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Spawn the reconciler as a background tokio task. The handle is
    /// returned for tests; production callers can drop it (the task lives
    /// for the duration of the runtime).
    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(self.cfg.interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Drop the immediate first tick so we don't hammer providers
            // before the rest of the bootstrap finishes (JWKS fetch etc.).
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = self.tick_once().await {
                    tracing::warn!(error = %e, "reconciler tick failed");
                }
            }
        })
    }

    /// One-shot tick exposed for tests. Production code calls this from the
    /// `spawn`'d loop.
    pub async fn tick_once(&self) -> anyhow::Result<TickSummary> {
        if !self.leader.is_leader().await {
            tracing::debug!("reconciler skipping tick: not leader");
            return Ok(TickSummary::default());
        }
        let adapters = self
            .state
            .metadata
            .list_adapters(&Filter::default())
            .await?;
        let tools = self.state.metadata.list_tools(&Filter::default()).await?;

        let mut summary = TickSummary::default();
        for a in adapters {
            // External-upstream adapters (`upstream` set) are not provider-
            // managed; nothing to reconcile.
            if a.upstream.is_some() {
                continue;
            }
            let spec = DeploymentSpec {
                name: a.name.clone(),
                kind: DeploymentKind::Adapter,
                adapter: Some(a.clone()),
                tool: None,
            };
            self.reconcile_one(DeploymentKind::Adapter, &a.name, a.tenant.as_deref(), spec, &mut summary)
                .await;
        }
        for t in tools {
            let spec = DeploymentSpec {
                name: t.name.clone(),
                kind: DeploymentKind::Tool,
                adapter: None,
                tool: Some(t.clone()),
            };
            self.reconcile_one(DeploymentKind::Tool, &t.name, t.tenant.as_deref(), spec, &mut summary)
                .await;
        }
        Ok(summary)
    }

    async fn reconcile_one(
        &self,
        kind: DeploymentKind,
        name: &str,
        tenant: Option<&str>,
        spec: DeploymentSpec,
        summary: &mut TickSummary,
    ) {
        let key = (kind, tenant.map(ToString::to_string), name.to_string());
        let now = Utc::now();

        // Honour the per-target backoff so repeated provider failures don't
        // hammer the API.
        if let Some(entry) = self.attempts.lock().get(&key).cloned() {
            if let Some(deadline) = entry.next_attempt_after {
                if now < deadline {
                    summary.skipped_due_to_backoff += 1;
                    return;
                }
            }
            if entry.consecutive_failures >= self.cfg.max_attempts_before_giving_up {
                summary.gave_up += 1;
                return;
            }
        }

        let handle = handle_for(name);
        let observed = match self.state.deployment.status(&handle).await {
            Ok(s) => s,
            Err(e) => DeploymentStatus {
                ready: false,
                replicas: 0,
                ready_replicas: 0,
                message: Some(format!("status probe failed: {e}")),
            },
        };
        let needs_apply = !observed.ready;

        let mut applied_outcome: Option<Result<(), String>> = None;
        if needs_apply {
            applied_outcome = Some(
                self.state
                    .deployment
                    .apply(&spec)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
            );
        }

        let final_status = match &applied_outcome {
            Some(Ok(())) => {
                // Re-probe after apply so the persisted status reflects what
                // we expect to be running now.
                self.state
                    .deployment
                    .status(&handle)
                    .await
                    .unwrap_or(observed)
            }
            Some(Err(msg)) => DeploymentStatus {
                ready: false,
                replicas: observed.replicas,
                ready_replicas: 0,
                message: Some(format!("apply failed: {msg}")),
            },
            None => observed,
        };

        // Update backoff bookkeeping.
        {
            let mut attempts = self.attempts.lock();
            let entry = attempts.entry(key.clone()).or_default();
            if final_status.ready {
                *entry = AttemptState::default();
            } else {
                entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
                let backoff = self
                    .cfg
                    .backoff_initial
                    .saturating_mul(1u32.checked_shl(entry.consecutive_failures.min(5)).unwrap_or(32))
                    .min(self.cfg.interval);
                entry.next_attempt_after = Some(now + chrono::Duration::from_std(backoff).unwrap_or_default());
            }
        }

        let record = DeploymentStatusRecord {
            status: final_status.clone(),
            observed_at: Utc::now(),
        };
        if let Err(e) = self
            .state
            .metadata
            .record_status(kind, name, tenant, &record)
            .await
        {
            tracing::warn!(error = %e, kind = ?kind, name = %name, "record_status failed");
        }

        if final_status.ready {
            summary.reconciled_ready += 1;
        } else {
            summary.reconciled_not_ready += 1;
        }
        if applied_outcome.is_some() {
            summary.applied += 1;
        }
    }
}

/// Build the deployment handle the same way the control plane does.
/// Kept here (instead of importing from `routes::control_plane_helpers`) so
/// the reconciler doesn't take a dependency on route-internal helpers.
fn handle_for(name: &str) -> DeploymentHandle {
    DeploymentHandle {
        id: name.to_string(),
        namespace: None,
        endpoint_url: None,
    }
}

/// Aggregate stats from a single tick — handy for tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct TickSummary {
    pub reconciled_ready: u32,
    pub reconciled_not_ready: u32,
    pub applied: u32,
    pub skipped_due_to_backoff: u32,
    pub gave_up: u32,
}
