//! Third-Party CAD & PCB Import Handlers (Altium, Eagle, PADS, Cadstar, etc.)
//! Leverages kicad-cli's production-grade importer in a sandboxed, non-blocking worker.

use crate::state::AppState;
use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use uuid::Uuid;

#[derive(Deserialize, Serialize, Debug)]
pub struct ImportPcbRequest {
    pub filename: String,
    pub content_base64: Option<String>,
    pub format: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ImportPcbResponse {
    pub success: bool,
    pub format: String,
    pub filename: String,
    pub kicad_pcb: String,
    pub report: Option<String>,
}

/// Helper to decode standard base64 without external crates
fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let clean: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = clean.as_bytes();
    let mut out = Vec::with_capacity((bytes.len() * 3) / 4);

    let decode_val = |b: u8| -> Result<u8, String> {
        match b {
            b'A'..=b'Z' => Ok(b - b'A'),
            b'a'..=b'z' => Ok(b - b'a' + 26),
            b'0'..=b'9' => Ok(b - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            b'=' => Ok(0),
            _ => Err(format!("Invalid base64 character: {}", b as char)),
        }
    };

    let mut i = 0;
    while i < bytes.len() {
        if i + 4 > bytes.len() {
            break;
        }
        let b0 = decode_val(bytes[i])?;
        let b1 = decode_val(bytes[i + 1])?;
        let b2 = decode_val(bytes[i + 2])?;
        let b3 = decode_val(bytes[i + 3])?;

        let triple = ((b0 as u32) << 18) | ((b1 as u32) << 12) | ((b2 as u32) << 6) | (b3 as u32);

        out.push(((triple >> 16) & 0xFF) as u8);
        if bytes[i + 2] != b'=' {
            out.push(((triple >> 8) & 0xFF) as u8);
        }
        if bytes[i + 3] != b'=' {
            out.push((triple & 0xFF) as u8);
        }

        i += 4;
    }
    Ok(out)
}

/// Core function to run kicad-cli pcb import safely in a worker task
pub async fn run_kicad_import(
    raw_bytes: Vec<u8>,
    filename: String,
    format_hint: Option<String>,
) -> Result<ImportPcbResponse, (StatusCode, String)> {
    let session_id = Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("cadoxide_import_{}", session_id));

    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to create temp dir: {}", e)))?;

    // Determine safe file extension
    let safe_filename = PathBuf::from(&filename)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("board.input")
        .to_string();

    let input_path = temp_dir.join(&safe_filename);
    let output_path = temp_dir.join("imported_board.kicad_pcb");
    let report_path = temp_dir.join("import_report.json");

    if let Err(e) = std::fs::write(&input_path, &raw_bytes) {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to write input file: {}", e)));
    }

    let format_arg = format_hint.unwrap_or_else(|| {
        let lower = safe_filename.to_lowercase();
        if lower.ends_with(".pcbdoc") {
            "altium".to_string()
        } else if lower.ends_with(".brd") {
            "eagle".to_string()
        } else if lower.ends_with(".asc") || lower.ends_with(".pcb") {
            "pads".to_string()
        } else {
            "auto".to_string()
        }
    });

    let input_str = input_path.to_string_lossy().to_string();
    let output_str = output_path.to_string_lossy().to_string();
    let report_str = report_path.to_string_lossy().to_string();
    let format_val = format_arg.clone();
    let temp_dir_for_worker = temp_dir.clone();

    // Run kicad-cli pcb import inside spawn_blocking
    let import_result = tokio::task::spawn_blocking(move || {
        let cfg_dir = temp_dir_for_worker.join("cfg");
        let data_dir = temp_dir_for_worker.join("data");
        let _ = std::fs::create_dir_all(&cfg_dir);
        let _ = std::fs::create_dir_all(&data_dir);

        let mut cmd = Command::new("kicad-cli");
        cmd.env("XDG_CONFIG_HOME", cfg_dir.to_string_lossy().as_ref())
            .env("XDG_DATA_HOME", data_dir.to_string_lossy().as_ref())
            .args([
                "pcb",
                "import",
                "--format",
                &format_val,
                "-o",
                &output_str,
                "--report-format",
                "json",
                "--report-file",
                &report_str,
                &input_str,
            ]);

        cmd.output()
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Task join error: {}", e)))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to spawn kicad-cli: {}", e)))?;

    let report_content = std::fs::read_to_string(&report_path).ok();
    let pcb_content = std::fs::read_to_string(&output_path);

    // Cleanup temporary directory
    let _ = std::fs::remove_dir_all(&temp_dir);

    match pcb_content {
        Ok(kicad_pcb) => Ok(ImportPcbResponse {
            success: import_result.status.success(),
            format: format_arg,
            filename: safe_filename,
            kicad_pcb,
            report: report_content,
        }),
        Err(_) => {
            let stderr = String::from_utf8_lossy(&import_result.stderr).to_string();
            let msg = if !stderr.is_empty() {
                stderr
            } else {
                "kicad-cli could not import this board format.".to_string()
            };
            Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("PCB import failed: {}", msg),
            ))
        }
    }
}

/// JSON endpoint for PCB Import
pub async fn import_pcb_json(
    State(_state): State<AppState>,
    Json(payload): Json<ImportPcbRequest>,
) -> Result<Json<ImportPcbResponse>, (StatusCode, String)> {
    let bytes = match payload.content_base64 {
        Some(ref b64) => decode_base64(b64).map_err(|e| (StatusCode::BAD_REQUEST, e))?,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Missing 'content_base64' payload".to_string(),
            ))
        }
    };

    let res = run_kicad_import(bytes, payload.filename, payload.format).await?;
    Ok(Json(res))
}

/// Multipart form-data endpoint for direct file uploads
pub async fn import_pcb_multipart(
    State(_state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ImportPcbResponse>, (StatusCode, String)> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename: String = "board.PcbDoc".to_string();
    let mut format_hint: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" || name == "board" {
            if let Some(fname) = field.file_name() {
                filename = fname.to_string();
            }
            if let Ok(data) = field.bytes().await {
                file_bytes = Some(data.to_vec());
            }
        } else if name == "format" {
            if let Ok(txt) = field.text().await {
                if !txt.is_empty() {
                    format_hint = Some(txt);
                }
            }
        }
    }

    let bytes = file_bytes.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "No file data received in multipart payload".to_string(),
        )
    })?;

    let res = run_kicad_import(bytes, filename, format_hint).await?;
    Ok(Json(res))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_decode() {
        let input = "SGVsbG8gQ0FET3hpZGUh";
        let decoded = decode_base64(input).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "Hello CADOxide!");
    }
}
