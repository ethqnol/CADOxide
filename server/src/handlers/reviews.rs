//! Design Review Threads, Comments & In-Context Annotations Handlers

use crate::models::{
    AddMessageRequest, CreateThreadRequest, ReviewMessage, ReviewThread, UpdateThreadStatusRequest,
};
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use rusqlite::params;
use uuid::Uuid;

pub async fn get_threads(
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

pub async fn create_thread(
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

pub async fn add_message(
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

pub async fn update_thread_status(
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
