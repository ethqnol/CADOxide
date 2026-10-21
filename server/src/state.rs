//! Shared Application State and Concurrency Primitives for CADoxide Server

use crate::models::{ComputeJob, DrcResult, LayerRenderResult, RoomCrdtState};
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

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub workspace_root: PathBuf,
    // Content-Addressable Storage (CAS) In-Memory Cache
    pub render_cache: Arc<Mutex<HashMap<String, Vec<LayerRenderResult>>>>,
    pub drc_cache: Arc<Mutex<HashMap<String, DrcResult>>>,
    // Message Broker & Worker Pool Telemetry
    pub jobs: Arc<Mutex<HashMap<String, ComputeJob>>>,
    pub job_sender: mpsc::Sender<String>,
    pub worker_semaphore: Arc<Semaphore>,
    pub cache_hits: Arc<AtomicU64>,
    pub cache_misses: Arc<AtomicU64>,
    pub jobs_completed: Arc<AtomicU64>,
    pub node_id: String,
    #[allow(dead_code)]
    pub collab_tx: Arc<broadcast::Sender<String>>,
    pub rooms: Arc<Mutex<HashMap<String, RoomCrdtState>>>,
    pub room_channels: Arc<Mutex<HashMap<String, broadcast::Sender<String>>>>,
}

