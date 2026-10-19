//! System Telemetry, kicad-cli Detection & Distributed Cluster Metrics

use crate::models::ComputeJob;
use crate::state::AppState;
use axum::{extract::State, response::IntoResponse, Json};
use std::process::Command;
use std::sync::atomic::Ordering;

pub async fn get_version(State(state): State<AppState>) -> impl IntoResponse {
    let output = Command::new("kicad-cli")
        .env("XDG_CONFIG_HOME", "/tmp/kicad_cfg")
        .env("XDG_DATA_HOME", "/tmp/kicad_data")
        .arg("--version")
        .output();

    match output {
        Ok(out) => {
            let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Json(serde_json::json!({
                "available": true,
                "version": ver,
                "node_id": state.node_id,
                "system": "CADoxide Distributed Axum Engine v1.2",
                "dual_mode": "Local & Remote Load-Balanced"
            }))
        }
        Err(e) => Json(serde_json::json!({
            "available": false,
            "error": e.to_string(),
            "node_id": state.node_id,
            "system": "CADoxide Distributed Axum Engine v1.2"
        })),
    }
}

pub async fn get_cluster_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let hits = state.cache_hits.load(Ordering::Relaxed);
    let misses = state.cache_misses.load(Ordering::Relaxed);
    let completed = state.jobs_completed.load(Ordering::Relaxed);
    let available_permits = state.worker_semaphore.available_permits();
    let recent_jobs: Vec<ComputeJob> = state
        .jobs
        .lock()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();

    Json(serde_json::json!({
        "node_id": state.node_id,
        "worker_pool": {
            "max_concurrency": 4,
            "idle_workers": available_permits,
            "busy_workers": 4 - available_permits,
        },
        "cas_cache": {
            "hits": hits,
            "misses": misses,
            "hit_ratio_percent": if hits + misses > 0 { (hits as f64 / (hits + misses) as f64) * 100.0 } else { 100.0 }
        },
        "broker": {
            "total_jobs_completed": completed,
            "recent_jobs_count": recent_jobs.len(),
            "recent_jobs": recent_jobs,
        }
    }))
}
