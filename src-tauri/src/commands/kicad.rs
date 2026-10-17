use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use tauri::command;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LayerRenderResult {
    pub layer: String,
    pub filename: String,
    pub svg_content: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DrcViolation {
    pub description: String,
    pub severity: String,
    pub type_name: Option<String>,
    pub pos_x: Option<f64>,
    pub pos_y: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DrcResult {
    pub success: bool,
    pub raw_json: String,
    pub violations_count: usize,
    pub violations: Vec<DrcViolation>,
}

#[command]
pub async fn get_kicad_version() -> Result<String, String> {
    let output = Command::new("kicad-cli")
        .env("XDG_CONFIG_HOME", "/tmp/kicad_cfg")
        .env("XDG_DATA_HOME", "/tmp/kicad_data")
        .arg("--version")
        .output()
        .map_err(|e| format!("Failed to run kicad-cli: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[command]
pub async fn render_pcb_layers(
    project_path: String,
    pcb_filename: String,
    layers: Vec<String>,
) -> Result<Vec<LayerRenderResult>, String> {
    let base_path = PathBuf::from(&project_path);
    let pcb_path = if PathBuf::from(&pcb_filename).is_absolute() {
        PathBuf::from(&pcb_filename)
    } else {
        base_path.join(&pcb_filename)
    };

    let out_dir = base_path.join(".cadenceflow").join("renders");

    tokio::fs::create_dir_all(&out_dir)
        .await
        .map_err(|e| e.to_string())?;

    let layers_arg = layers.join(",");

    // Run kicad-cli export with compatibility flags
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
        .map_err(|e| format!("Failed to execute kicad-cli: {}. Is KiCad installed?", e))?;

    if !output.status.success() {
        eprintln!(
            "kicad-cli warning: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut results = Vec::new();
    let file_stem = pcb_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("board");

    if let Ok(mut entries) = tokio::fs::read_dir(&out_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("svg") {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with(file_stem) {
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        let layer_id = layers.iter().find(|l| {
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

    Ok(results)
}

#[command]
pub async fn execute_drc(project_path: String, pcb_filename: String) -> Result<DrcResult, String> {
    let base_path = PathBuf::from(&project_path);
    let pcb_path = if PathBuf::from(&pcb_filename).is_absolute() {
        PathBuf::from(&pcb_filename)
    } else {
        base_path.join(&pcb_filename)
    };

    let report_path = base_path.join(".cadenceflow").join("drc_report.json");

    if let Some(parent) = report_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }

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
        .output()
        .map_err(|e| format!("Failed to spawn DRC check: {}", e))?;

    let raw_json = tokio::fs::read_to_string(&report_path)
        .await
        .unwrap_or_else(|_| "{}".to_string());

    let mut violations = Vec::new();
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw_json) {
        if let Some(list) = parsed.get("violations").and_then(|v| v.as_array()) {
            for v in list {
                let desc = v
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("Violation")
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

    Ok(DrcResult {
        success: output.status.success(),
        raw_json,
        violations_count,
        violations,
    })
}
