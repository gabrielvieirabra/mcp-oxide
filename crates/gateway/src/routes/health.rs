//! Health / readiness / liveness endpoints.

use axum::{extract::State, Json};
use serde_json::json;

use crate::state::AppState;

pub async fn root(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "name": "mcp-oxide",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": s.started_at.elapsed().as_secs(),
    }))
}

pub async fn healthz(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "providers": s.provider_summary(),
    }))
}

pub async fn startup() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn live() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn ready(State(_s): State<AppState>) -> Json<serde_json::Value> {
    // Phase 0: always ready once the process is up. Phases 2+ wire
    // MetadataStore / SessionStore / IdProvider readiness checks.
    Json(json!({ "status": "ok" }))
}
