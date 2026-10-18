use serde::{Deserialize, Serialize};
use tauri::command;

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

#[command]
pub async fn get_review_threads(_project_path: String) -> Result<Vec<ReviewThread>, String> {
    // In Tauri, this connects to the local SQLite database file at .cadoxide/reviews.db
    let now = chrono::Utc::now().to_rfc3339();
    Ok(vec![
        ReviewThread {
            id: "thread-sample-01".to_string(),
            revision_id: "rev-head".to_string(),
            file_type: "pcb".to_string(),
            sheet_or_layer: "F.Cu".to_string(),
            coord_x: 128.5,
            coord_y: 82.0,
            status: "open".to_string(),
            created_by: "ElecLead_Alice".to_string(),
            created_at: now.clone(),
            resolved_at: None,
            messages: vec![ReviewMessage {
                id: "msg-sample-01".to_string(),
                thread_id: "thread-sample-01".to_string(),
                user_id: "user-alice".to_string(),
                user_name: "ElecLead_Alice".to_string(),
                body: "High speed diff pair needs tighter clearance around via cluster here to prevent impedance discontinuity.".to_string(),
                created_at: now.clone(),
            }],
        },
        ReviewThread {
            id: "thread-sample-02".to_string(),
            revision_id: "rev-head".to_string(),
            file_type: "pcb".to_string(),
            sheet_or_layer: "Edge.Cuts".to_string(),
            coord_x: 160.0,
            coord_y: 110.0,
            status: "resolved".to_string(),
            created_by: "MechEng_Bob".to_string(),
            created_at: now.clone(),
            resolved_at: Some(now.clone()),
            messages: vec![ReviewMessage {
                id: "msg-sample-02".to_string(),
                thread_id: "thread-sample-02".to_string(),
                user_id: "user-bob".to_string(),
                user_name: "MechEng_Bob".to_string(),
                body: "Mounting hole clearance verified against DIN 7985 M3 standoffs.".to_string(),
                created_at: now,
            }],
        },
    ])
}
