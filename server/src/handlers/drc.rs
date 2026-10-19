//! DRC Execution & Violation Diagnostics Handlers

use crate::models::{DrcRequest, DrcResult, DrcViolation};
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;

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

    let base_path = match payload.project_path {
        Some(ref p) => {
            let candidate = PathBuf::from(p);
            if candidate.exists() {
                candidate
            } else {
                state.workspace_root.join("public/samples").join(p)
            }
        }
        None => state.workspace_root.join("public/samples/complex_hierarchy"),
    };

    let pcb_path = if PathBuf::from(&payload.pcb_filename).is_absolute() {
        PathBuf::from(&payload.pcb_filename)
    } else {
        base_path.join(&payload.pcb_filename)
    };

    let report_path = base_path.join(".cadoxide_drc.json");

    let output = Command::new("kicad-cli")
        .env("XDG_CONFIG_HOME", "/tmp/kicad_cfg")
        .env("XDG_DATA_HOME", "/tmp/kicad_data")
        .args([
            "pcb",
            "drc",
            "--format",
            "json",
            "--severity-all",
            "--units",
            "mm",
            "-o",
            report_path.to_str().unwrap(),
            pcb_path.to_str().unwrap(),
        ])
        .output();

    let (success, raw_json) = match output {
        Ok(out) => {
            let json_str =
                std::fs::read_to_string(&report_path).unwrap_or_else(|_| "{}".to_string());
            (out.status.success(), json_str)
        }
        Err(e) => (false, format!(r#"{{"error": "{}"}}"#, e)),
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
