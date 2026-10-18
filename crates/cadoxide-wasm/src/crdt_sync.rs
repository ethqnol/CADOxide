//! Real-Time Multi-Engineer Co-Layout & Spatial Net CRDT Synchronization Engine
//! Pure Rust implementation communicating over WebSockets in WebAssembly.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PeerCursor {
    pub user_id: String,
    pub user_name: String,
    pub color: String,
    pub x: f32,
    pub y: f32,
    pub layer_mask: u32,
    pub active_tool: String,
    pub last_update_ts: f64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum CollabMessage {
    Join {
        user_id: String,
        user_name: String,
        color: String,
    },
    Leave {
        user_id: String,
    },
    Cursor {
        user_id: String,
        user_name: String,
        color: String,
        x: f32,
        y: f32,
        layer: u32,
        tool: String,
    },
    LockNet {
        user_id: String,
        user_name: String,
        net_id: u32,
    },
    UnlockNet {
        user_id: String,
        net_id: u32,
    },
    AddTrack {
        user_id: String,
        sx: f32,
        sy: f32,
        ex: f32,
        ey: f32,
        w: f32,
        layer: u32,
        net_id: u32,
    },
    AddVia {
        user_id: String,
        x: f32,
        y: f32,
        size: f32,
        drill: f32,
        net_id: u32,
    },
    DeletePrimitive {
        user_id: String,
        idx: usize,
    },
}

pub struct CollabManager {
    pub local_user_id: String,
    pub local_user_name: String,
    pub local_color: String,
    pub peers: HashMap<String, PeerCursor>,
    pub locked_nets: HashMap<u32, (String, String)>, // net_id -> (user_id, user_name)
    pub ws: Option<web_sys::WebSocket>,
    pub is_connected: bool,
}

impl CollabManager {
    pub fn new() -> Self {
        let rand_val = js_sys::Math::random();
        let uid = format!("user-{:04x}", ((rand_val * 65535.0) as u32));
        let colors = ["#38bdf8", "#34d399", "#f43f5e", "#fbbf24", "#a855f7", "#ec4899", "#60a5fa"];
        let color_idx = ((rand_val * (colors.len() as f64)) as usize).min(colors.len() - 1);
        let color = colors[color_idx].to_string();
        let user_name = format!("Engineer-{}", &uid[5..]);

        Self {
            local_user_id: uid,
            local_user_name: user_name,
            local_color: color,
            peers: HashMap::new(),
            locked_nets: HashMap::new(),
            ws: None,
            is_connected: false,
        }
    }

    pub fn handle_incoming_message(&mut self, text: &str) -> Option<CollabMessage> {
        let msg: CollabMessage = serde_json::from_str(text).ok()?;
        match &msg {
            CollabMessage::Join { user_id, user_name, color } => {
                if user_id != &self.local_user_id {
                    self.peers.insert(
                        user_id.clone(),
                        PeerCursor {
                            user_id: user_id.clone(),
                            user_name: user_name.clone(),
                            color: color.clone(),
                            x: 0.0,
                            y: 0.0,
                            layer_mask: 1,
                            active_tool: "Select".to_string(),
                            last_update_ts: js_sys::Date::now(),
                        },
                    );
                }
            }
            CollabMessage::Leave { user_id } => {
                self.peers.remove(user_id);
                self.locked_nets.retain(|_, (uid, _)| uid != user_id);
            }
            CollabMessage::Cursor {
                user_id,
                user_name,
                color,
                x,
                y,
                layer,
                tool,
            } => {
                if user_id != &self.local_user_id {
                    let entry = self.peers.entry(user_id.clone()).or_insert_with(|| PeerCursor {
                        user_id: user_id.clone(),
                        user_name: user_name.clone(),
                        color: color.clone(),
                        x: *x,
                        y: *y,
                        layer_mask: *layer,
                        active_tool: tool.clone(),
                        last_update_ts: js_sys::Date::now(),
                    });
                    entry.x = *x;
                    entry.y = *y;
                    entry.layer_mask = *layer;
                    entry.active_tool = tool.clone();
                    entry.last_update_ts = js_sys::Date::now();
                }
            }
            CollabMessage::LockNet { user_id, user_name, net_id } => {
                self.locked_nets.insert(*net_id, (user_id.clone(), user_name.clone()));
            }
            CollabMessage::UnlockNet { user_id: _, net_id } => {
                self.locked_nets.remove(net_id);
            }
            _ => {}
        }
        Some(msg)
    }

    pub fn send_message(&self, msg: &CollabMessage) {
        if let Some(ref ws) = self.ws {
            if ws.ready_state() == web_sys::WebSocket::OPEN {
                if let Ok(text) = serde_json::to_string(msg) {
                    let _ = ws.send_with_str(&text);
                }
            }
        }
    }

    pub fn broadcast_cursor(&self, x: f32, y: f32, layer: u32, tool: &str) {
        let msg = CollabMessage::Cursor {
            user_id: self.local_user_id.clone(),
            user_name: self.local_user_name.clone(),
            color: self.local_color.clone(),
            x,
            y,
            layer,
            tool: tool.to_string(),
        };
        self.send_message(&msg);
    }

    pub fn is_net_locked_by_other(&self, net_id: u32) -> Option<&str> {
        if let Some((uid, uname)) = self.locked_nets.get(&net_id) {
            if uid != &self.local_user_id {
                return Some(uname.as_str());
            }
        }
        None
    }
}

