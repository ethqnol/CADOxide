//! Domain Models, DTOs and Request/Response Payloads for CADoxide Server

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct RoomCrdtState {
    pub tracks: HashMap<String, serde_json::Value>,
    pub vias: HashMap<String, serde_json::Value>,
    pub tombstones: HashSet<String>,
    pub locked_nets: HashMap<u32, (String, String, f64)>, // net_id -> (user_id, user_name, expiry_timestamp_ms)
}

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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PartItem {
    pub id: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub value: String,
    pub footprint_name: String,
    pub symbol_name: String,
    pub lifecycle_status: String,
    pub unit_price_usd: f64,
    pub digikey_stock: i64,
    pub mouser_stock: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DesignRevisionItem {
    pub id: String,
    pub project_slug: String,
    pub git_commit_hash: String,
    pub git_branch: String,
    pub author_name: String,
    pub commit_message: String,
    pub render_status: String,
    pub glb_3d_asset_url: String,
    pub created_at: String,
}
