//! Real-Time Multi-Engineer Co-Layout & Spatial Net CRDT Synchronization Engine
//! Pure Rust implementation executing inside WebAssembly communicating over WebSockets.
//! Implements causal ordering (Lamport clocks), commutative delta mutations,
//! tombstone-based deletions, dynamic net leases, and room/project isolation.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

fn current_time_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0
    }
}

fn random_float() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Math::random()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0.42
    }
}

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
pub struct CrdtTrack {
    pub id: String,
    pub clock: u64,
    pub user_id: String,
    pub sx: f32,
    pub sy: f32,
    pub ex: f32,
    pub ey: f32,
    pub w: f32,
    pub layer: u32,
    pub net_id: u32,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CrdtVia {
    pub id: String,
    pub clock: u64,
    pub user_id: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub drill: f32,
    pub net_id: u32,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum CollabMessage {
    Join {
        project_id: String,
        user_id: String,
        user_name: String,
        color: String,
    },
    Leave {
        project_id: String,
        user_id: String,
    },
    Cursor {
        project_id: String,
        user_id: String,
        user_name: String,
        color: String,
        x: f32,
        y: f32,
        layer: u32,
        tool: String,
    },
    LockNet {
        project_id: String,
        user_id: String,
        user_name: String,
        net_id: u32,
        timeout_ms: u64,
    },
    UnlockNet {
        project_id: String,
        user_id: String,
        net_id: u32,
    },
    AddTrack {
        project_id: String,
        id: String,
        clock: u64,
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
        project_id: String,
        id: String,
        clock: u64,
        user_id: String,
        x: f32,
        y: f32,
        size: f32,
        drill: f32,
        net_id: u32,
    },
    DeletePrimitive {
        project_id: String,
        id: String,
        clock: u64,
        user_id: String,
    },
    SyncSnapshot {
        project_id: String,
        tracks: Vec<CrdtTrack>,
        vias: Vec<CrdtVia>,
        tombstones: Vec<String>,
        locked_nets: Vec<(u32, String, String)>,
    },
}

pub struct CollabManager {
    pub current_project_id: String,
    pub local_user_id: String,
    pub local_user_name: String,
    pub local_color: String,
    pub lamport_clock: u64,
    pub seq_counter: u64,
    pub peers: HashMap<String, PeerCursor>,
    pub locked_nets: HashMap<u32, (String, String, f64)>, // net_id -> (user_id, user_name, expiry_timestamp_ms)
    pub crdt_tracks: HashMap<String, CrdtTrack>,
    pub crdt_vias: HashMap<String, CrdtVia>,
    pub tombstones: HashSet<String>,
    pub ws: Option<web_sys::WebSocket>,
    pub is_connected: bool,
}

impl CollabManager {
    pub fn new() -> Self {
        let rand_val = random_float();
        let uid = format!("user-{:04x}", ((rand_val * 65535.0) as u32));
        let colors = ["#38bdf8", "#34d399", "#f43f5e", "#fbbf24", "#a855f7", "#ec4899", "#60a5fa"];
        let color_idx = ((rand_val * (colors.len() as f64)) as usize).min(colors.len() - 1);
        let color = colors[color_idx].to_string();
        let user_name = format!("Engineer-{}", &uid[5..]);

        Self {
            current_project_id: "complex_hierarchy".to_string(),
            local_user_id: uid,
            local_user_name: user_name,
            local_color: color,
            lamport_clock: 0,
            seq_counter: 0,
            peers: HashMap::new(),
            locked_nets: HashMap::new(),
            crdt_tracks: HashMap::new(),
            crdt_vias: HashMap::new(),
            tombstones: HashSet::new(),
            ws: None,
            is_connected: false,
        }
    }

    pub fn next_primitive_id(&mut self) -> (String, u64) {
        self.seq_counter += 1;
        self.lamport_clock += 1;
        let id = format!("{}_{}", self.local_user_id, self.seq_counter);
        (id, self.lamport_clock)
    }

    pub fn switch_room(&mut self, new_project_id: &str) {
        if self.current_project_id != new_project_id {
            // Send leave for previous room
            let leave_msg = CollabMessage::Leave {
                project_id: self.current_project_id.clone(),
                user_id: self.local_user_id.clone(),
            };
            self.send_message(&leave_msg);

            self.current_project_id = new_project_id.to_string();
            self.peers.clear();
            self.locked_nets.clear();
            self.crdt_tracks.clear();
            self.crdt_vias.clear();
            self.tombstones.clear();

            // Send join for new room
            let join_msg = CollabMessage::Join {
                project_id: self.current_project_id.clone(),
                user_id: self.local_user_id.clone(),
                user_name: self.local_user_name.clone(),
                color: self.local_color.clone(),
            };
            self.send_message(&join_msg);
        }
    }

    pub fn handle_incoming_message(&mut self, text: &str) -> Option<CollabMessage> {
        let msg: CollabMessage = serde_json::from_str(text).ok()?;
        let now_ts = current_time_ms();

        // Prune expired net locks
        self.locked_nets.retain(|_, (_, _, exp)| *exp > now_ts);

        match &msg {
            CollabMessage::Join { project_id, user_id, user_name, color } => {
                if project_id == &self.current_project_id && user_id != &self.local_user_id {
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
                            last_update_ts: now_ts,
                        },
                    );
                }
            }
            CollabMessage::Leave { project_id, user_id } => {
                if project_id == &self.current_project_id {
                    self.peers.remove(user_id);
                    self.locked_nets.retain(|_, (uid, _, _)| uid != user_id);
                }
            }
            CollabMessage::Cursor {
                project_id,
                user_id,
                user_name,
                color,
                x,
                y,
                layer,
                tool,
            } => {
                if project_id == &self.current_project_id && user_id != &self.local_user_id {
                    let entry = self.peers.entry(user_id.clone()).or_insert_with(|| PeerCursor {
                        user_id: user_id.clone(),
                        user_name: user_name.clone(),
                        color: color.clone(),
                        x: *x,
                        y: *y,
                        layer_mask: *layer,
                        active_tool: tool.clone(),
                        last_update_ts: now_ts,
                    });
                    entry.x = *x;
                    entry.y = *y;
                    entry.layer_mask = *layer;
                    entry.active_tool = tool.clone();
                    entry.last_update_ts = now_ts;
                }
            }
            CollabMessage::LockNet { project_id, user_id, user_name, net_id, timeout_ms } => {
                if project_id == &self.current_project_id {
                    let expiry = now_ts + (*timeout_ms as f64);
                    self.locked_nets.insert(*net_id, (user_id.clone(), user_name.clone(), expiry));
                }
            }
            CollabMessage::UnlockNet { project_id, user_id: _, net_id } => {
                if project_id == &self.current_project_id {
                    self.locked_nets.remove(net_id);
                }
            }
            CollabMessage::AddTrack {
                project_id,
                id,
                clock,
                user_id,
                sx,
                sy,
                ex,
                ey,
                w,
                layer,
                net_id,
            } => {
                if project_id == &self.current_project_id {
                    self.lamport_clock = self.lamport_clock.max(*clock) + 1;
                    if !self.tombstones.contains(id) {
                        self.crdt_tracks.insert(
                            id.clone(),
                            CrdtTrack {
                                id: id.clone(),
                                clock: *clock,
                                user_id: user_id.clone(),
                                sx: *sx,
                                sy: *sy,
                                ex: *ex,
                                ey: *ey,
                                w: *w,
                                layer: *layer,
                                net_id: *net_id,
                            },
                        );
                    }
                }
            }
            CollabMessage::AddVia {
                project_id,
                id,
                clock,
                user_id,
                x,
                y,
                size,
                drill,
                net_id,
            } => {
                if project_id == &self.current_project_id {
                    self.lamport_clock = self.lamport_clock.max(*clock) + 1;
                    if !self.tombstones.contains(id) {
                        self.crdt_vias.insert(
                            id.clone(),
                            CrdtVia {
                                id: id.clone(),
                                clock: *clock,
                                user_id: user_id.clone(),
                                x: *x,
                                y: *y,
                                size: *size,
                                drill: *drill,
                                net_id: *net_id,
                            },
                        );
                    }
                }
            }
            CollabMessage::DeletePrimitive { project_id, id, clock, user_id: _ } => {
                if project_id == &self.current_project_id {
                    self.lamport_clock = self.lamport_clock.max(*clock) + 1;
                    self.tombstones.insert(id.clone());
                    self.crdt_tracks.remove(id);
                    self.crdt_vias.remove(id);
                }
            }
            CollabMessage::SyncSnapshot { project_id, tracks, vias, tombstones, locked_nets } => {
                if project_id == &self.current_project_id {
                    for t in tombstones {
                        self.tombstones.insert(t.clone());
                    }
                    for tr in tracks {
                        if !self.tombstones.contains(&tr.id) {
                            self.crdt_tracks.insert(tr.id.clone(), tr.clone());
                        }
                    }
                    for v in vias {
                        if !self.tombstones.contains(&v.id) {
                            self.crdt_vias.insert(v.id.clone(), v.clone());
                        }
                    }
                    for (nid, uid, uname) in locked_nets {
                        self.locked_nets.insert(*nid, (uid.clone(), uname.clone(), now_ts + 30000.0));
                    }
                }
            }
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
            project_id: self.current_project_id.clone(),
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

    pub fn lock_net(&mut self, net_id: u32, timeout_ms: u64) {
        let now_ts = current_time_ms();
        self.locked_nets.insert(
            net_id,
            (
                self.local_user_id.clone(),
                self.local_user_name.clone(),
                now_ts + (timeout_ms as f64),
            ),
        );
        let msg = CollabMessage::LockNet {
            project_id: self.current_project_id.clone(),
            user_id: self.local_user_id.clone(),
            user_name: self.local_user_name.clone(),
            net_id,
            timeout_ms,
        };
        self.send_message(&msg);
    }

    pub fn unlock_net(&mut self, net_id: u32) {
        self.locked_nets.remove(&net_id);
        let msg = CollabMessage::UnlockNet {
            project_id: self.current_project_id.clone(),
            user_id: self.local_user_id.clone(),
            net_id,
        };
        self.send_message(&msg);
    }

    pub fn is_net_locked_by_other(&self, net_id: u32) -> Option<&str> {
        let now_ts = current_time_ms();
        if let Some((uid, uname, exp)) = self.locked_nets.get(&net_id) {
            if *exp > now_ts && uid != &self.local_user_id {
                return Some(uname.as_str());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collab_net_locking_and_conflict_avoidance() {
        let mut client_a = CollabManager::new();
        client_a.local_user_id = "user-alice".to_string();
        client_a.local_user_name = "Alice".to_string();

        let mut client_b = CollabManager::new();
        client_b.local_user_id = "user-bob".to_string();
        client_b.local_user_name = "Bob".to_string();

        // Initially net 42 is free
        assert_eq!(client_b.is_net_locked_by_other(42), None);

        // Alice locks net 42
        client_a.lock_net(42, 60000);
        let lock_msg = serde_json::to_string(&CollabMessage::LockNet {
            project_id: "complex_hierarchy".to_string(),
            user_id: "user-alice".to_string(),
            user_name: "Alice".to_string(),
            net_id: 42,
            timeout_ms: 60000,
        })
        .unwrap();

        // Bob receives Alice's lock
        client_b.handle_incoming_message(&lock_msg);
        assert_eq!(client_b.is_net_locked_by_other(42), Some("Alice"));

        // Alice releases lock
        client_a.unlock_net(42);
        let unlock_msg = serde_json::to_string(&CollabMessage::UnlockNet {
            project_id: "complex_hierarchy".to_string(),
            user_id: "user-alice".to_string(),
            net_id: 42,
        })
        .unwrap();

        // Bob receives Alice's unlock
        client_b.handle_incoming_message(&unlock_msg);
        assert_eq!(client_b.is_net_locked_by_other(42), None);
    }

    #[test]
    fn test_crdt_tracks_and_tombstone_prevention() {
        let mut client = CollabManager::new();

        let add_msg = serde_json::to_string(&CollabMessage::AddTrack {
            project_id: "complex_hierarchy".to_string(),
            id: "trk_100".to_string(),
            clock: 1,
            user_id: "user-alice".to_string(),
            sx: 10.0,
            sy: 20.0,
            ex: 30.0,
            ey: 20.0,
            w: 0.25,
            layer: 1,
            net_id: 3,
        })
        .unwrap();

        client.handle_incoming_message(&add_msg);
        assert!(client.crdt_tracks.contains_key("trk_100"));

        // Remove track
        let remove_msg = serde_json::to_string(&CollabMessage::DeletePrimitive {
            project_id: "complex_hierarchy".to_string(),
            id: "trk_100".to_string(),
            clock: 2,
            user_id: "user-alice".to_string(),
        })
        .unwrap();

        client.handle_incoming_message(&remove_msg);
        assert!(!client.crdt_tracks.contains_key("trk_100"));
        assert!(client.tombstones.contains("trk_100"));

        // Re-receiving add message (out of order network packet) must NOT resurrect deleted track
        client.handle_incoming_message(&add_msg);
        assert!(!client.crdt_tracks.contains_key("trk_100"));
    }
}

