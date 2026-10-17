use axum::{
    extract::{Path, State},
    http::{header, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, Semaphore};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing::{error, info};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ComputeJob {
    pub job_id: String,
    pub job_type: String, // "render_pcb" | "execute_drc"
    pub status: String,   // "queued" | "processing" | "completed" | "failed"
    pub priority: String, // "high" | "bulk"
    pub created_at: String,
    pub completed_at: Option<String>,
    pub cache_hit: bool,
}

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Connection>>,
    workspace_root: PathBuf,
    // Content-Addressable Storage (CAS) In-Memory Cache
    render_cache: Arc<Mutex<HashMap<String, Vec<LayerRenderResult>>>>,
    drc_cache: Arc<Mutex<HashMap<String, DrcResult>>>,
    // Message Broker & Worker Pool Telemetry
    jobs: Arc<Mutex<HashMap<String, ComputeJob>>>,
    job_sender: mpsc::Sender<String>,
    worker_semaphore: Arc<Semaphore>,
    cache_hits: Arc<AtomicU64>,
    cache_misses: Arc<AtomicU64>,
    jobs_completed: Arc<AtomicU64>,
    node_id: String,
}

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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RenderRequest {
    pub project_path: Option<String>,
    pub pcb_filename: String,
    pub layers: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DrcRequest {
    pub project_path: Option<String>,
    pub pcb_filename: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReviewThread {
    pub id: String,
    pub revision_id: String,
    pub file_type: String,
    pub sheet_or_layer: String,
    pub coord_x: f64,
    pub coord_y: f64,
    pub status: String,
    pub created_by: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
    pub messages: Vec<ReviewMessage>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReviewMessage {
    pub id: String,
    pub thread_id: String,
    pub user_id: String,
    pub user_name: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct CreateThreadRequest {
    pub revision_id: String,
    pub file_type: String,
    pub sheet_or_layer: String,
    pub coord_x: f64,
    pub coord_y: f64,
    pub created_by: String,
    pub initial_message: String,
}

#[derive(Deserialize)]
pub struct AddMessageRequest {
    pub thread_id: String,
    pub user_id: String,
    pub user_name: String,
    pub body: String,
}

#[derive(Deserialize)]
pub struct UpdateThreadStatusRequest {
    pub status: String,
}

fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS review_threads (
            id TEXT PRIMARY KEY,
            revision_id TEXT NOT NULL,
            file_type TEXT NOT NULL,
            sheet_or_layer TEXT NOT NULL,
            coord_x REAL NOT NULL,
            coord_y REAL NOT NULL,
            status TEXT NOT NULL DEFAULT 'open',
            created_by TEXT NOT NULL,
            created_at TEXT NOT NULL,
            resolved_at TEXT
        );

        CREATE TABLE IF NOT EXISTS review_messages (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL REFERENCES review_threads(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL,
            user_name TEXT NOT NULL,
            body TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        "#,
    )?;

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM review_threads", [], |row| row.get(0))
        .unwrap_or(0);

    if count == 0 {
        let thread_id_1 = "thread-sample-01";
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO review_threads (id, revision_id, file_type, sheet_or_layer, coord_x, coord_y, status, created_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                thread_id_1,
                "rev-main",
                "pcb",
                "F.Cu",
                128.5,
                82.0,
                "open",
                "ElecLead_Alice",
                now
            ],
        )?;

        conn.execute(
            "INSERT INTO review_messages (id, thread_id, user_id, user_name, body, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "msg-sample-01",
                thread_id_1,
                "user-alice",
                "ElecLead_Alice",
                "High speed diff pair needs tighter clearance around via cluster here to prevent impedance discontinuity.",
                now
            ],
        )?;

        let thread_id_2 = "thread-sample-02";
        conn.execute(
            "INSERT INTO review_threads (id, revision_id, file_type, sheet_or_layer, coord_x, coord_y, status, created_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                thread_id_2,
                "rev-main",
                "pcb",
                "Edge.Cuts",
                160.0,
                110.0,
                "resolved",
                "MechEng_Bob",
                now
            ],
        )?;

        conn.execute(
            "INSERT INTO review_messages (id, thread_id, user_id, user_name, body, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "msg-sample-02",
                thread_id_2,
                "user-bob",
                "MechEng_Bob",
                "Mounting hole clearance verified against DIN 7985 M3 standoffs.",
                now
            ],
        )?;
    }

    Ok(())
}

async fn get_version(State(state): State<AppState>) -> impl IntoResponse {
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
                "system": "CadenceFlow Distributed Axum Engine v1.2",
                "dual_mode": "Local & Remote Load-Balanced"
            }))
        }
        Err(e) => Json(serde_json::json!({
            "available": false,
            "error": e.to_string(),
            "node_id": state.node_id,
            "system": "CadenceFlow Distributed Axum Engine v1.2"
        })),
    }
}

async fn get_cluster_metrics(State(state): State<AppState>) -> impl IntoResponse {
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

async fn render_pcb(
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

    let out_dir = base_path.join(".cadenceflow_renders");
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

async fn execute_drc(
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

    let report_path = base_path.join(".cadenceflow_drc.json");

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

async fn get_threads(
    State(state): State<AppState>,
) -> Result<Json<Vec<ReviewThread>>, (StatusCode, String)> {
    let conn = state
        .db
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, revision_id, file_type, sheet_or_layer, coord_x, coord_y, status, created_by, created_at, resolved_at
             FROM review_threads ORDER BY created_at DESC",
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(ReviewThread {
                id: row.get(0)?,
                revision_id: row.get(1)?,
                file_type: row.get(2)?,
                sheet_or_layer: row.get(3)?,
                coord_x: row.get(4)?,
                coord_y: row.get(5)?,
                status: row.get(6)?,
                created_by: row.get(7)?,
                created_at: row.get(8)?,
                resolved_at: row.get(9)?,
                messages: Vec::new(),
            })
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut threads = Vec::new();
    for thread_res in rows {
        let mut thread =
            thread_res.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut msg_stmt = conn
            .prepare(
                "SELECT id, thread_id, user_id, user_name, body, created_at
                 FROM review_messages WHERE thread_id = ?1 ORDER BY created_at ASC",
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let msg_rows = msg_stmt
            .query_map(params![thread.id], |row| {
                Ok(ReviewMessage {
                    id: row.get(0)?,
                    thread_id: row.get(1)?,
                    user_id: row.get(2)?,
                    user_name: row.get(3)?,
                    body: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        for msg in msg_rows.flatten() {
            thread.messages.push(msg);
        }

        threads.push(thread);
    }

    Ok(Json(threads))
}

async fn create_thread(
    State(state): State<AppState>,
    Json(payload): Json<CreateThreadRequest>,
) -> Result<Json<ReviewThread>, (StatusCode, String)> {
    let conn = state
        .db
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let thread_id = format!("thread-{}", Uuid::new_v4());
    let msg_id = format!("msg-{}", Uuid::new_v4());
    let now = Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO review_threads (id, revision_id, file_type, sheet_or_layer, coord_x, coord_y, status, created_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            thread_id,
            payload.revision_id,
            payload.file_type,
            payload.sheet_or_layer,
            payload.coord_x,
            payload.coord_y,
            "open",
            payload.created_by,
            now
        ],
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let initial_msg = ReviewMessage {
        id: msg_id.clone(),
        thread_id: thread_id.clone(),
        user_id: payload.created_by.clone(),
        user_name: payload.created_by.clone(),
        body: payload.initial_message,
        created_at: now.clone(),
    };

    conn.execute(
        "INSERT INTO review_messages (id, thread_id, user_id, user_name, body, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            initial_msg.id,
            initial_msg.thread_id,
            initial_msg.user_id,
            initial_msg.user_name,
            initial_msg.body,
            initial_msg.created_at
        ],
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ReviewThread {
        id: thread_id,
        revision_id: payload.revision_id,
        file_type: payload.file_type,
        sheet_or_layer: payload.sheet_or_layer,
        coord_x: payload.coord_x,
        coord_y: payload.coord_y,
        status: "open".to_string(),
        created_by: payload.created_by,
        created_at: now,
        resolved_at: None,
        messages: vec![initial_msg],
    }))
}

async fn add_message(
    State(state): State<AppState>,
    Json(payload): Json<AddMessageRequest>,
) -> Result<Json<ReviewMessage>, (StatusCode, String)> {
    let conn = state
        .db
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let msg_id = format!("msg-{}", Uuid::new_v4());
    let now = Utc::now().to_rfc3339();

    let msg = ReviewMessage {
        id: msg_id,
        thread_id: payload.thread_id,
        user_id: payload.user_id,
        user_name: payload.user_name,
        body: payload.body,
        created_at: now,
    };

    conn.execute(
        "INSERT INTO review_messages (id, thread_id, user_id, user_name, body, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            msg.id,
            msg.thread_id,
            msg.user_id,
            msg.user_name,
            msg.body,
            msg.created_at
        ],
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(msg))
}

async fn update_thread_status(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Json(payload): Json<UpdateThreadStatusRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let conn = state
        .db
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let resolved_at = if payload.status == "resolved" {
        Some(Utc::now().to_rfc3339())
    } else {
        None
    };

    conn.execute(
        "UPDATE review_threads SET status = ?1, resolved_at = ?2 WHERE id = ?3",
        params![payload.status, resolved_at, thread_id],
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cadenceflow_server=info,tower_http=info".into()),
        )
        .init();

    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let db_path = workspace_root.join(".cadenceflow_local.db");

    let conn = Connection::open(&db_path).expect("Failed to open SQLite database");
    init_db(&conn).expect("Failed to initialize database tables");

    let (tx, mut rx) = mpsc::channel::<String>(256);
    tokio::spawn(async move {
        while let Some(job_id) = rx.recv().await {
            info!("Broker dispatched compute job: {}", job_id);
        }
    });

    let node_id = std::env::var("NODE_ID").unwrap_or_else(|_| "cadence-node-primary".to_string());

    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        workspace_root: workspace_root.clone(),
        render_cache: Arc::new(Mutex::new(HashMap::new())),
        drc_cache: Arc::new(Mutex::new(HashMap::new())),
        jobs: Arc::new(Mutex::new(HashMap::new())),
        job_sender: tx,
        worker_semaphore: Arc::new(Semaphore::new(4)),
        cache_hits: Arc::new(AtomicU64::new(0)),
        cache_misses: Arc::new(AtomicU64::new(0)),
        jobs_completed: Arc::new(AtomicU64::new(0)),
        node_id,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let app = Router::new()
        .route("/api/version", get(get_version))
        .route("/api/cluster/metrics", get(get_cluster_metrics))
        .route("/api/render-pcb", post(render_pcb))
        .route("/api/execute-drc", post(execute_drc))
        .route("/api/threads", get(get_threads).post(create_thread))
        .route("/api/threads/:id/status", post(update_thread_status))
        .route("/api/messages", post(add_message))
        .nest_service(
            "/samples",
            ServeDir::new(workspace_root.join("public/samples")),
        )
        .layer(cors)
        .with_state(state);

    let port = 8080;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    info!("CadenceFlow Server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
