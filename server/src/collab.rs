//! Real-Time Collaboration & Spatial Net CRDT Synchronization Engine
//! Features room-isolated broadcast channels, snapshot direct reply, and automatic lock cleanup.

use crate::state::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use tokio::sync::{broadcast, mpsc};

pub async fn collab_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_collab_socket(socket, state))
}

fn get_room_tx(state: &AppState, project_id: &str) -> broadcast::Sender<String> {
    let mut channels = state.room_channels.lock().unwrap_or_else(|e| e.into_inner());
    channels
        .entry(project_id.to_string())
        .or_insert_with(|| {
            let (tx, _) = broadcast::channel::<String>(512);
            tx
        })
        .clone()
}

pub async fn handle_collab_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let rooms = state.rooms.clone();

    // Internal channel to send messages to this specific client's WebSocket sink
    let (client_tx, mut client_rx) = mpsc::channel::<String>(256);

    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = client_rx.recv().await {
            if sender.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    let state_clone = state.clone();
    let mut recv_task = tokio::spawn(async move {
        let mut current_room: Option<String> = None;
        let mut current_user: Option<(String, String)> = None;
        let mut room_forward_task: Option<tokio::task::JoinHandle<()>> = None;

        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(text) = msg {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let project_id = val
                        .get("project_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default")
                        .to_string();

                    // If user is joining or switching rooms
                    if msg_type == "Join" {
                        current_room = Some(project_id.clone());
                        if let (Some(uid), Some(uname)) = (
                            val.get("user_id").and_then(|v| v.as_str()),
                            val.get("user_name").and_then(|v| v.as_str()),
                        ) {
                            current_user = Some((uid.to_string(), uname.to_string()));
                        }

                        let room_tx = get_room_tx(&state_clone, &project_id);

                        // Abort prior forwarder task if any
                        if let Some(handle) = room_forward_task.take() {
                            handle.abort();
                        }

                        // Forward messages from this room's broadcast channel to this client
                        let mut room_rx = room_tx.subscribe();
                        let client_tx_clone = client_tx.clone();
                        room_forward_task = Some(tokio::spawn(async move {
                            while let Ok(broadcast_msg) = room_rx.recv().await {
                                if client_tx_clone.send(broadcast_msg).await.is_err() {
                                    break;
                                }
                            }
                        }));

                        // Prepare and send snapshot ONLY to the joining client (not to the entire room)
                        let snap = {
                            let mut rooms_guard = rooms.lock().unwrap_or_else(|e| e.into_inner());
                            let room = rooms_guard.entry(project_id.clone()).or_default();

                            // Prune expired locks
                            let now = chrono::Utc::now().timestamp_millis() as f64;
                            room.locked_nets.retain(|_, (_, _, exp)| *exp > now);

                            let active_locks: Vec<_> = room
                                .locked_nets
                                .iter()
                                .map(|(k, (uid, uname, _))| (*k, uid.clone(), uname.clone()))
                                .collect();

                            serde_json::json!({
                                "type": "SyncSnapshot",
                                "project_id": project_id,
                                "tracks": room.tracks.values().collect::<Vec<_>>(),
                                "vias": room.vias.values().collect::<Vec<_>>(),
                                "tombstones": room.tombstones.iter().collect::<Vec<_>>(),
                                "locked_nets": active_locks,
                            })
                        };

                        let _ = client_tx.send(snap.to_string()).await;
                        continue;
                    }

                    // For operational messages, update room state
                    let target_room_id = current_room.clone().unwrap_or_else(|| project_id.clone());
                    let mut rooms_guard = rooms.lock().unwrap_or_else(|e| e.into_inner());
                    let room = rooms_guard.entry(target_room_id.clone()).or_default();

                    match msg_type {
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
                    drop(rooms_guard);

                    // Broadcast ONLY to the specific room
                    let room_tx = get_room_tx(&state_clone, &target_room_id);
                    let _ = room_tx.send(text);
                }
            }
        }

        // Cleanup on disconnect: release any locks held by this user
        if let (Some(ref room_id), Some((ref uid, _))) = (&current_room, &current_user) {
            let mut rooms_guard = rooms.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(room) = rooms_guard.get_mut(room_id) {
                let mut released_nets = Vec::new();
                room.locked_nets.retain(|net_id, (owner_uid, _, _)| {
                    if owner_uid == uid {
                        released_nets.push(*net_id);
                        false
                    } else {
                        true
                    }
                });
                drop(rooms_guard);

                let room_tx = get_room_tx(&state_clone, room_id);
                for net_id in released_nets {
                    let unlock_msg = serde_json::json!({
                        "type": "UnlockNet",
                        "project_id": room_id,
                        "net_id": net_id,
                        "user_id": uid,
                    });
                    let _ = room_tx.send(unlock_msg.to_string());
                }
            }
        }

        if let Some(handle) = room_forward_task {
            handle.abort();
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}
