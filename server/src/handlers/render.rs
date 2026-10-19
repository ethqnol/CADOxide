//! PCB Vector SVG Export, CAS Caching & 3D VRML Export Handlers

use crate::models::{ComputeJob, DrcRequest, LayerRenderResult, RenderRequest};
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;
use tracing::error;
use uuid::Uuid;

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

    if !pcb_path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("PCB file not found: {}", pcb_path.display()),
        ));
    }

    let out_dir = base_path.join(".cadoxide_renders");
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let layers_arg = payload.layers.join(",");

    let output = Command::new("kicad-cli")
        .env("XDG_CONFIG_HOME", "/tmp/kicad_cfg")
        .env("XDG_DATA_HOME", "/tmp/kicad_data")
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
            out_dir.to_str().unwrap(),
            pcb_path.to_str().unwrap(),
        ])
        .output()
        .map_err(|e| {
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
    let file_stem = pcb_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("board");

    if let Ok(entries) = std::fs::read_dir(&out_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("svg") {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with(file_stem) {
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
    let base_path = match &payload.project_path {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => state
            .workspace_root
            .join("public")
            .join("samples")
            .join("complex_hierarchy"),
    };
    let pcb_path = if PathBuf::from(&payload.pcb_filename).is_absolute() {
        PathBuf::from(&payload.pcb_filename)
    } else {
        base_path.join(&payload.pcb_filename)
    };
    let out_path = base_path.join("board.wrl");

    let _permit = state
        .worker_semaphore
        .acquire()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let out_str = out_path.to_string_lossy().to_string();
    let pcb_str = pcb_path.to_string_lossy().to_string();

    let status = tokio::task::spawn_blocking(move || {
        Command::new("kicad-cli")
            .env("XDG_CONFIG_HOME", "/tmp/kicad_cfg")
            .env("XDG_DATA_HOME", "/tmp/kicad_data")
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
            .status()
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
