//! DRC Execution & Violation Diagnostics Handlers
//! Hardened against path traversal, blocking Tokio workers, and directory races.

use crate::handlers::render::resolve_safe_pcb_path;
use crate::models::{DrcRequest, DrcResult, DrcViolation};
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use std::process::Command;
use std::sync::atomic::Ordering;
use uuid::Uuid;

pub async fn execute_drc(
    State(state): State<AppState>,
    Json(payload): Json<DrcRequest>,
) -> Result<Json<DrcResult>, (StatusCode, String)> {
    let cas_key = format!(
        "cas:drc:{}:{}",
        payload.project_path.as_deref().unwrap_or("default"),
        payload.pcb_filename
    );

    if let Ok(cache) = state.drc_cache.lock() {
        if let Some(cached) = cache.get(&cas_key) {
            state.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Json(cached.clone()));
        }
    }
    state.cache_misses.fetch_add(1, Ordering::Relaxed);

    let _permit = state
        .worker_semaphore
        .acquire()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let pcb_path = resolve_safe_pcb_path(
        &state.workspace_root,
        payload.project_path.as_deref(),
        &payload.pcb_filename,
    )?;

    let drc_session_id = Uuid::new_v4().to_string();
    let report_path = std::env::temp_dir().join(format!("cadoxide_drc_{}.json", drc_session_id));
    let pcb_str = pcb_path.to_string_lossy().to_string();
    let report_str = report_path.to_string_lossy().to_string();

    let output = tokio::task::spawn_blocking(move || {
        let cfg_dir = std::env::temp_dir().join(format!("kicad_cfg_{}", drc_session_id));
        let data_dir = std::env::temp_dir().join(format!("kicad_data_{}", drc_session_id));
        let _ = std::fs::create_dir_all(&cfg_dir);
        let _ = std::fs::create_dir_all(&data_dir);

        let res = Command::new("kicad-cli")
            .env("XDG_CONFIG_HOME", cfg_dir.to_string_lossy().as_ref())
            .env("XDG_DATA_HOME", data_dir.to_string_lossy().as_ref())
            .args([
                "pcb",
                "drc",
                "--format",
                "json",
                "--severity-all",
                "--units",
                "mm",
                "-o",
                &report_str,
                &pcb_str,
            ])
            .output();

        let _ = std::fs::remove_dir_all(&cfg_dir);
        let _ = std::fs::remove_dir_all(&data_dir);
        res
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Spawn error: {}", e)))?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to execute kicad-cli: {}", e),
        )
    })?;

    let (success, raw_json) = {
        let json_str = std::fs::read_to_string(&report_path).unwrap_or_else(|_| "{}".to_string());
        let _ = std::fs::remove_file(&report_path);
        (output.status.success(), json_str)
    };

    let mut violations = Vec::new();
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw_json) {
        if let Some(list) = parsed.get("violations").and_then(|v| v.as_array()) {
            for v in list {
                let desc = v
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("DRC Violation")
                    .to_string();
                let severity = v
                    .get("severity")
                    .and_then(|s| s.as_str())
                    .unwrap_or("error")
                    .to_string();
                let type_name = v.get("type").and_then(|t| t.as_str()).map(|s| s.to_string());

                let mut pos_x = None;
                let mut pos_y = None;

                if let Some(pos) = v.get("pos").or_else(|| v.get("position")) {
                    pos_x = pos.get("x").and_then(|x| x.as_f64());
                    pos_y = pos.get("y").and_then(|y| y.as_f64());
                } else if let Some(items) = v.get("items").and_then(|i| i.as_array()) {
                    if let Some(first_item) = items.first() {
                        if let Some(pos) =
                            first_item.get("pos").or_else(|| first_item.get("position"))
                        {
                            pos_x = pos.get("x").and_then(|x| x.as_f64());
                            pos_y = pos.get("y").and_then(|y| y.as_f64());
                        }
                    }
                }

                violations.push(DrcViolation {
                    description: desc,
                    severity,
                    type_name,
                    pos_x,
                    pos_y,
                });
            }
        }
    }

    let violations_count = violations.len();
    let res = DrcResult {
        success,
        raw_json,
        violations_count,
        violations,
    };

    if let Ok(mut cache) = state.drc_cache.lock() {
        cache.insert(cas_key, res.clone());
    }
    state.jobs_completed.fetch_add(1, Ordering::Relaxed);

    Ok(Json(res))
}
