//! Real-Time Collaboration & Spatial Net CRDT Synchronization Engine

use crate::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{sink::SinkExt, stream::StreamExt};

pub async fn collab_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_collab_socket(socket, state))
}

pub async fn handle_collab_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.collab_tx.subscribe();
    let tx = state.collab_tx.clone();
    let rooms = state.rooms.clone();

    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if sender.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(text) = msg {
                // Parse message to maintain CRDT state per room
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let project_id = val
                        .get("project_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default")
                        .to_string();

                    let mut rooms_guard = rooms.lock().unwrap();
                    let room = rooms_guard.entry(project_id.clone()).or_default();

                    match msg_type {
                        "Join" => {
                            // Prepare snapshot for this project and broadcast catch-up
                            let snap = serde_json::json!({
                                "type": "SyncSnapshot",
                                "project_id": project_id,
                                "tracks": room.tracks.values().collect::<Vec<_>>(),
                                "vias": room.vias.values().collect::<Vec<_>>(),
                                "tombstones": room.tombstones.iter().collect::<Vec<_>>(),
                                "locked_nets": room.locked_nets.iter().map(|(k, (uid, uname, _))| (*k, uid.clone(), uname.clone())).collect::<Vec<_>>(),
                            });
                            let _ = tx.send(snap.to_string());
                        }
                        "AddTrack" => {
                            if let Some(id) = val.get("id").and_then(|v| v.as_str()) {
                                if !room.tombstones.contains(id) {
                                    room.tracks.insert(id.to_string(), val.clone());
                                }
                            }
                        }
                        "AddVia" => {
                            if let Some(id) = val.get("id").and_then(|v| v.as_str()) {
                                if !room.tombstones.contains(id) {
                                    room.vias.insert(id.to_string(), val.clone());
                                }
                            }
                        }
                        "DeletePrimitive" => {
                            if let Some(id) = val.get("id").and_then(|v| v.as_str()) {
                                room.tombstones.insert(id.to_string());
                                room.tracks.remove(id);
                                room.vias.remove(id);
                            }
                        }
                        "LockNet" => {
                            if let (Some(net_id), Some(uid), Some(uname)) = (
                                val.get("net_id").and_then(|v| v.as_u64()),
                                val.get("user_id").and_then(|v| v.as_str()),
                                val.get("user_name").and_then(|v| v.as_str()),
                            ) {
                                let timeout_ms = val
                                    .get("timeout_ms")
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(15000.0);
                                let exp =
                                    (chrono::Utc::now().timestamp_millis() as f64) + timeout_ms;
                                room.locked_nets.insert(
                                    net_id as u32,
                                    (uid.to_string(), uname.to_string(), exp),
                                );
                            }
                        }
                        "UnlockNet" => {
                            if let Some(net_id) = val.get("net_id").and_then(|v| v.as_u64()) {
                                room.locked_nets.remove(&(net_id as u32));
                            }
                        }
                        _ => {}
                    }
                }
                let _ = tx.send(text);
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}
