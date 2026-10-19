//! Design Revisions, Git Tracking & 3D Assets Handlers

use crate::models::DesignRevisionItem;
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};

pub async fn get_revisions(
    State(state): State<AppState>,
) -> Result<Json<Vec<DesignRevisionItem>>, (StatusCode, String)> {
    let conn = state
        .db
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, project_slug, git_commit_hash, git_branch, author_name, commit_message, render_status, glb_3d_asset_url, created_at
             FROM design_revisions ORDER BY created_at DESC",
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(DesignRevisionItem {
                id: row.get(0)?,
                project_slug: row.get(1)?,
                git_commit_hash: row.get(2)?,
                git_branch: row.get(3)?,
                author_name: row.get(4)?,
                commit_message: row.get(5)?,
                render_status: row.get(6)?,
                glb_3d_asset_url: row.get(7)?,
                created_at: row.get(8)?,
            })
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut list = Vec::new();
    for r in rows {
        list.push(r.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
    }
    Ok(Json(list))
}
