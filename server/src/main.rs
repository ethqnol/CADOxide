//! CADoxide High-Performance Distributed Server
//! 100% Pure Rust Axum Architecture with Content-Addressable Storage (CAS)
//! and Real-Time Multi-User CRDT WebSocket Synchronization.

mod collab;
mod db;
mod handlers;
mod models;
mod state;

use axum::{
    http::{header, Method},
    routing::{get, post},
    Router,
};
use rusqlite::Connection;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::AtomicU64,
        Arc, Mutex,
    },
};
use tokio::sync::{broadcast, mpsc, Semaphore};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing::info;

use crate::collab::collab_ws_handler;
use crate::db::init_db;
use crate::handlers::{
    drc::execute_drc,
    importer::{import_pcb_json, import_pcb_multipart},
    parts::get_parts,
    render::{export_3d_vrml, render_pcb},
    reviews::{add_message, create_thread, get_threads, update_thread_status},
    revisions::get_revisions,
    system::{get_cluster_metrics, get_version},
};
use crate::state::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cadoxide_server=info,tower_http=info".into()),
        )
        .init();

    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let db_path = workspace_root.join(".cadoxide_local.db");

    let conn = Connection::open(&db_path).expect("Failed to open SQLite database");
    init_db(&conn).expect("Failed to initialize database tables");

    let (tx, mut rx) = mpsc::channel::<String>(256);
    tokio::spawn(async move {
        while let Some(job_id) = rx.recv().await {
            info!("Broker dispatched compute job: {}", job_id);
        }
    });

    let node_id = std::env::var("NODE_ID").unwrap_or_else(|_| "cadoxide-node-primary".to_string());

    let (collab_tx, _) = broadcast::channel::<String>(256);
    let collab_tx = Arc::new(collab_tx);

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
        collab_tx,
        rooms: Arc::new(Mutex::new(HashMap::new())),
        room_channels: Arc::new(Mutex::new(HashMap::new())),
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
        .route("/api/import-pcb", post(import_pcb_json))
        .route("/api/import-pcb/upload", post(import_pcb_multipart))
        .route("/api/export-3d", post(export_3d_vrml))
        .route("/api/parts", get(get_parts))
        .route("/api/revisions", get(get_revisions))
        .route("/api/threads", get(get_threads).post(create_thread))
        .route("/api/threads/:id/status", post(update_thread_status))
        .route("/api/messages", post(add_message))
        .route("/api/collab/ws", get(collab_ws_handler))
        .nest_service(
            "/samples",
            ServeDir::new(workspace_root.join("public/samples")),
        )
        .fallback_service(ServeDir::new(workspace_root.join("public")))
        .layer(cors)
        .with_state(state);

    let port = 8080;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    info!("CADoxide Server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
