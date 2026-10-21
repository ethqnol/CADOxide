//! PCB Vector SVG Export, CAS Caching & 3D VRML Export Handlers
//! Hardened against path traversal, blocking Tokio workers, and directory races.

use crate::models::{ComputeJob, DrcRequest, LayerRenderResult, RenderRequest};
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use tracing::error;
use uuid::Uuid;

/// Resolves a PCB file path securely, guaranteeing it cannot escape the workspace root.
pub fn resolve_safe_pcb_path(
    workspace_root: &Path,
    project_path: Option<&str>,
    pcb_filename: &str,
) -> Result<PathBuf, (StatusCode, String)> {
    if pcb_filename.contains("..") {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid pcb_filename: '..' traversal component forbidden".to_string(),
        ));
    }

    let file_path = Path::new(pcb_filename);
    let canonical_ws = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());

    if file_path.is_absolute() {
        let canonical_file = file_path.canonicalize().map_err(|_| {
            (
                StatusCode::NOT_FOUND,
                format!("PCB file not found: {}", pcb_filename),
            )
        })?;
        if !canonical_file.starts_with(&canonical_ws) {
            return Err((
                StatusCode::FORBIDDEN,
                "Access denied: path escapes workspace".to_string(),
            ));
        }
        return Ok(canonical_file);
    }

    let base_path = match project_path {
        Some(p) => {
            if p.contains("..") {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Invalid project_path: '..' traversal component forbidden".to_string(),
                ));
            }
            let candidate = workspace_root.join(p);
            if candidate.exists() {
                candidate
            } else {
                workspace_root.join("public/samples").join(p)
            }
        }
        None => workspace_root.join("public/samples/complex_hierarchy"),
    };

    let pcb_path = base_path.join(pcb_filename);
    let canonical_pcb = pcb_path.canonicalize().map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            format!("PCB file not found: {}", pcb_path.display()),
        )
    })?;

    if !canonical_pcb.starts_with(&canonical_ws) {
        return Err((
            StatusCode::FORBIDDEN,
            "Access denied: path escapes workspace".to_string(),
        ));
    }

    Ok(canonical_pcb)
}

pub async fn render_pcb(
    State(state): State<AppState>,
    Json(payload): Json<RenderRequest>,
) -> Result<Json<Vec<LayerRenderResult>>, (StatusCode, String)> {
    let mut sorted_layers = payload.layers.clone();
    sorted_layers.sort();
    let cas_key = format!(
        "cas:render:{}:{}:{}",
        payload.project_path.as_deref().unwrap_or("default"),
        payload.pcb_filename,
        sorted_layers.join(",")
    );

    // 1. Check Content-Addressable Cache (O(1) sub-ms return)
    if let Ok(cache) = state.render_cache.lock() {
        if let Some(cached) = cache.get(&cas_key) {
            state.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Json(cached.clone()));
        }
    }
    state.cache_misses.fetch_add(1, Ordering::Relaxed);

    // 2. Register Job in Broker Telemetry & Acquire Worker Pool Permit
    let job_id = format!("job-{}", Uuid::new_v4());
    if let Ok(mut jobs) = state.jobs.lock() {
        jobs.insert(
            job_id.clone(),
            ComputeJob {
                job_id: job_id.clone(),
                job_type: "render_pcb".to_string(),
                status: "processing".to_string(),
                priority: "high".to_string(),
                created_at: Utc::now().to_rfc3339(),
                completed_at: None,
                cache_hit: false,
            },
        );
    }
    let _ = state.job_sender.try_send(job_id.clone());

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

    let layers_arg = payload.layers.join(",");
    let pcb_str = pcb_path.to_string_lossy().to_string();
    let file_stem = pcb_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("board")
        .to_string();

    // Isolated unique output directory per render job
    let render_session_id = Uuid::new_v4().to_string();
    let out_dir = std::env::temp_dir().join(format!("cadoxide_renders_{}", render_session_id));
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let out_dir_str = out_dir.to_string_lossy().to_string();

    let output = tokio::task::spawn_blocking(move || {
        let cfg_dir = std::env::temp_dir().join(format!("kicad_cfg_{}", render_session_id));
        let data_dir = std::env::temp_dir().join(format!("kicad_data_{}", render_session_id));
        let _ = std::fs::create_dir_all(&cfg_dir);
        let _ = std::fs::create_dir_all(&data_dir);

        let res = Command::new("kicad-cli")
            .env("XDG_CONFIG_HOME", cfg_dir.to_string_lossy().as_ref())
            .env("XDG_DATA_HOME", data_dir.to_string_lossy().as_ref())
            .args([
                "pcb",
                "export",
                "svg",
                "--mode-multi",
                "--layers",
                &layers_arg,
                "--exclude-drawing-sheet",
                "--fit-page-to-board",
                "-o",
                &out_dir_str,
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
        let _ = std::fs::remove_dir_all(&out_dir);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to execute kicad-cli: {}", e),
        )
    })?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr).to_string();
        error!("kicad-cli export error: {}", err_msg);
    }

    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&out_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("svg") {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with(&file_stem) {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let layer_id = payload.layers.iter().find(|l| {
                            let norm = l.replace('.', "_");
                            name.contains(&norm)
                                || (l.as_str() == "F.Cu" && name.contains("top_copper"))
                                || (l.as_str() == "B.Cu" && name.contains("bottom_copper"))
                                || (l.as_str() == "Edge.Cuts" && name.contains("Edge_Cuts"))
                                || (l.as_str() == "F.SilkS" && name.contains("F_Silkscreen"))
                                || (l.as_str() == "B.SilkS" && name.contains("B_Silkscreen"))
                                || (l.as_str() == "F.Mask" && name.contains("F_Mask"))
                                || (l.as_str() == "B.Mask" && name.contains("B_Mask"))
                        });

                        results.push(LayerRenderResult {
                            layer: layer_id.cloned().unwrap_or_else(|| name.to_string()),
                            filename: name.to_string(),
                            svg_content: content,
                        });
                    }
                }
            }
        }
    }

    let _ = std::fs::remove_dir_all(&out_dir);

    // Store in CAS Cache & Complete Job
    if let Ok(mut cache) = state.render_cache.lock() {
        cache.insert(cas_key, results.clone());
    }
    if let Ok(mut jobs) = state.jobs.lock() {
        if let Some(j) = jobs.get_mut(&job_id) {
            j.status = "completed".to_string();
            j.completed_at = Some(Utc::now().to_rfc3339());
        }
    }
    state.jobs_completed.fetch_add(1, Ordering::Relaxed);

    Ok(Json(results))
}

pub async fn export_3d_vrml(
    State(state): State<AppState>,
    Json(payload): Json<DrcRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let pcb_path = resolve_safe_pcb_path(
        &state.workspace_root,
        payload.project_path.as_deref(),
        &payload.pcb_filename,
    )?;

    let out_dir = pcb_path
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .to_path_buf();
    let out_path = out_dir.join("board.wrl");

    let _permit = state
        .worker_semaphore
        .acquire()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let out_str = out_path.to_string_lossy().to_string();
    let pcb_str = pcb_path.to_string_lossy().to_string();

    let status = tokio::task::spawn_blocking(move || {
        let session_id = Uuid::new_v4().to_string();
        let cfg_dir = std::env::temp_dir().join(format!("kicad_cfg_{}", session_id));
        let data_dir = std::env::temp_dir().join(format!("kicad_data_{}", session_id));
        let _ = std::fs::create_dir_all(&cfg_dir);
        let _ = std::fs::create_dir_all(&data_dir);

        let res = Command::new("kicad-cli")
            .env("XDG_CONFIG_HOME", cfg_dir.to_string_lossy().as_ref())
            .env("XDG_DATA_HOME", data_dir.to_string_lossy().as_ref())
            .args([
                "pcb",
                "export",
                "vrml",
                "--units",
                "mm",
                "-o",
                &out_str,
                &pcb_str,
            ])
            .status();

        let _ = std::fs::remove_dir_all(&cfg_dir);
        let _ = std::fs::remove_dir_all(&data_dir);
        res
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state.jobs_completed.fetch_add(1, Ordering::Relaxed);

    Ok(Json(serde_json::json!({
        "success": status.success(),
        "vrml_path": out_path.to_string_lossy(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_traversal_prevention() {
        let ws = std::env::current_dir().unwrap();
        // Disallow '..' escape
        assert!(resolve_safe_pcb_path(&ws, Some("../"), "board.kicad_pcb").is_err());
        assert!(resolve_safe_pcb_path(&ws, None, "../../../etc/passwd").is_err());
        assert!(resolve_safe_pcb_path(&ws, None, "/etc/shadow").is_err());
    }
}
