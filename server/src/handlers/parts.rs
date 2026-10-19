//! Parts Library, Component Pricing & Stock Availability Handlers

use crate::models::PartItem;
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};

pub async fn get_parts(
    State(state): State<AppState>,
) -> Result<Json<Vec<PartItem>>, (StatusCode, String)> {
    let conn = state
        .db
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, mpn, manufacturer, description, value, footprint_name, symbol_name, lifecycle_status, unit_price_usd, digikey_stock, mouser_stock
             FROM parts_library ORDER BY mpn ASC",
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(PartItem {
                id: row.get(0)?,
                mpn: row.get(1)?,
                manufacturer: row.get(2)?,
                description: row.get(3)?,
                value: row.get(4)?,
                footprint_name: row.get(5)?,
                symbol_name: row.get(6)?,
                lifecycle_status: row.get(7)?,
                unit_price_usd: row.get(8)?,
                digikey_stock: row.get(9)?,
                mouser_stock: row.get(10)?,
            })
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut list = Vec::new();
    for r in rows {
        list.push(r.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?);
    }
    Ok(Json(list))
}
