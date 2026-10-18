use std::collections::{HashMap, HashSet};

pub const LAYER_F_CU: u32 = 1 << 0;
pub const LAYER_B_CU: u32 = 1 << 1;
pub const LAYER_EDGE_CUTS: u32 = 1 << 2;
pub const LAYER_F_SILKS: u32 = 1 << 3;
pub const LAYER_B_SILKS: u32 = 1 << 4;
pub const LAYER_F_MASK: u32 = 1 << 5;
pub const LAYER_B_MASK: u32 = 1 << 6;
pub const LAYER_DRC_MARKER: u32 = 1 << 7;
pub const LAYER_REVIEW_PIN: u32 = 1 << 8;
pub const LAYER_DIFF_UNCHANGED: u32 = 1 << 9;
pub const LAYER_DIFF_REMOVED: u32 = 1 << 10;
pub const LAYER_DIFF_ADDED: u32 = 1 << 11;
pub const LAYER_IN1_CU: u32 = 1 << 12;
pub const LAYER_IN_CU: u32 = LAYER_IN1_CU;
pub const LAYER_IN2_CU: u32 = 1 << 13;
pub const LAYER_F_PASTE: u32 = 1 << 14;
pub const LAYER_B_PASTE: u32 = 1 << 15;
pub const LAYER_F_CRTYD: u32 = 1 << 16;
pub const LAYER_B_CRTYD: u32 = 1 << 17;
pub const LAYER_F_FAB: u32 = 1 << 18;
pub const LAYER_B_FAB: u32 = 1 << 19;
pub const LAYER_USER_DWGS: u32 = 1 << 20;
pub const LAYER_USER_CMTS: u32 = 1 << 21;
pub const LAYER_MARGIN: u32 = 1 << 22;

pub const ALL_VISIBLE_LAYERS_DEFAULT: u32 = LAYER_F_CU
    | LAYER_B_CU
    | LAYER_IN1_CU
    | LAYER_IN2_CU
    | LAYER_F_SILKS
    | LAYER_B_SILKS
    | LAYER_EDGE_CUTS
    | LAYER_F_CRTYD
    | LAYER_B_CRTYD
    | LAYER_F_FAB
    | LAYER_USER_DWGS
    | LAYER_USER_CMTS
    | LAYER_MARGIN
    | LAYER_DRC_MARKER
    | LAYER_REVIEW_PIN;

pub const FLOATS_PER_INSTANCE: usize = 8;
pub const SPATIAL_CELL_MM: f32 = 2.5;

#[derive(Clone, Debug, Default)]
pub struct NetStats {
    pub id: u32,
    pub name: String,
    pub total_length_mm: f32,
    pub segment_count: usize,
    pub via_count: usize,
    pub pad_count: usize,
}

#[derive(Clone, Debug)]
pub struct CopperZone {
    pub net_id: u32,
    pub net_name: String,
    pub layer_mask: u32,
    pub is_keepout: bool,
    pub is_teardrop: bool,
    pub bounds: [f32; 4], // [min_x, min_y, max_x, max_y]
    pub outline: Vec<(f32, f32)>,
    pub filled_polys: Vec<Vec<(f32, f32)>>,
}

impl CopperZone {
    pub fn compute_bounds(outline: &[(f32, f32)], filled_polys: &[Vec<(f32, f32)>]) -> [f32; 4] {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for pt in outline {
            if pt.0 < min_x { min_x = pt.0; }
            if pt.0 > max_x { max_x = pt.0; }
            if pt.1 < min_y { min_y = pt.1; }
            if pt.1 > max_y { max_y = pt.1; }
        }
        for poly in filled_polys {
            for pt in poly {
                if pt.0 < min_x { min_x = pt.0; }
                if pt.0 > max_x { max_x = pt.0; }
                if pt.1 < min_y { min_y = pt.1; }
                if pt.1 > max_y { max_y = pt.1; }
            }
        }
        if min_x <= max_x {
            [min_x, min_y, max_x, max_y]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        }
    }
}

#[derive(Clone, Debug)]
pub struct PadLabel {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub pin: String,
    pub net_id: u32,
    pub net_name: String,
    pub layer_mask: u32,
}

#[derive(Clone, Debug)]
pub struct BoardText {
    pub x: f32,
    pub y: f32,
    pub angle_deg: f32,
    pub text: String,
    pub size_mm: f32,
    pub layer_mask: u32,
    pub is_ref: bool,
}

#[derive(Clone, Debug)]
pub struct CadDimension {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub layer_mask: u32,
}

#[derive(Clone, Debug)]
pub struct HoveredPrimitiveInfo {
    pub instance_idx: usize,
    pub kind: &'static str,
    pub prim_type: i32,
    pub layer_name: &'static str,
    pub net_id: u32,
    pub net_name: String,
    pub summary: String,
    pub center_x: f32,
    pub center_y: f32,
}

#[derive(Clone, Debug, Default)]
pub struct ParsedBoard {
    pub instances: Vec<f32>,
    pub primitive_count: usize,
    pub bounds: [f32; 4], // [min_x, min_y, max_x, max_y]
    pub spatial_grid: HashMap<(i32, i32), Vec<u32>>,
    pub nets: HashMap<u32, NetStats>,
    pub zones: Vec<CopperZone>,
    pub pad_labels: Vec<PadLabel>,
    pub texts: Vec<BoardText>,
    pub dimensions: Vec<CadDimension>,
    pub ratsnest_lines: Vec<((f32, f32), (f32, f32), u32)>,
    pub total_pads: usize,
    pub total_vias: usize,
    pub total_tracks: usize,
}

impl ParsedBoard {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_spatial_grid(
        instances: Vec<f32>,
        primitive_count: usize,
        bounds: [f32; 4],
        nets: HashMap<u32, NetStats>,
    ) -> Self {
        let mut board = Self {
            instances,
            primitive_count,
            bounds,
            spatial_grid: HashMap::new(),
            nets,
            zones: Vec::new(),
            pad_labels: Vec::new(),
            texts: Vec::new(),
            dimensions: Vec::new(),
            ratsnest_lines: Vec::new(),
            total_pads: 0,
            total_vias: 0,
            total_tracks: 0,
        };
        board.rebuild_index_and_nets();
        board
    }

    pub fn rebuild_ratsnest(&mut self) {
        self.ratsnest_lines.clear();
        let plane_nets: std::collections::HashSet<u32> = self
            .zones
            .iter()
            .filter(|z| !z.is_keepout && !z.is_teardrop)
            .map(|z| z.net_id)
            .collect();

        let mut pads_by_net: HashMap<u32, Vec<(f32, f32)>> = HashMap::new();
        for pad in &self.pad_labels {
            if pad.net_id == 0 || plane_nets.contains(&pad.net_id) {
                continue;
            }
            if let Some(stats) = self.nets.get(&pad.net_id) {
                if stats.segment_count > 0 && stats.segment_count >= stats.pad_count.saturating_sub(1) {
                    continue;
                }
            }
            pads_by_net.entry(pad.net_id).or_default().push((pad.x, pad.y));
        }

        for (nid, pts) in pads_by_net {
            if pts.len() < 2 {
                continue;
            }
            if pts.len() > 30 {
                for i in 0..pts.len() - 1 {
                    self.ratsnest_lines.push((pts[i], pts[i + 1], nid));
                }
                continue;
            }

            let mut in_tree = vec![false; pts.len()];
            in_tree[0] = true;
            let mut tree_count = 1;

            while tree_count < pts.len() {
                let mut best_dist = f32::MAX;
                let mut best_edge = (0, 0);

                for i in 0..pts.len() {
                    if !in_tree[i] {
                        continue;
                    }
                    for j in 0..pts.len() {
                        if in_tree[j] {
                            continue;
                        }
                        let dx = pts[i].0 - pts[j].0;
                        let dy = pts[i].1 - pts[j].1;
                        let d = dx * dx + dy * dy;
                        if d < best_dist {
                            best_dist = d;
                            best_edge = (i, j);
                        }
                    }
                }

                if best_dist < 4000.0 {
                    self.ratsnest_lines.push((pts[best_edge.0], pts[best_edge.1], nid));
                }
                in_tree[best_edge.1] = true;
                tree_count += 1;
            }
        }
    }

    /// O(1) sub-microsecond spatial hash query for real-time cursor hover & click inspection.
    pub fn query_nearest(
        &self,
        mm_x: f32,
        mm_y: f32,
        visible_mask: u32,
        max_dist_mm: f32,
    ) -> Option<HoveredPrimitiveInfo> {
        self.query_nearest_filtered(mm_x, mm_y, visible_mask, max_dist_mm, 0xFFFF)
    }

    /// O(1) spatial hash query with KiCad Selection Filter mask:
    /// bit 0 (1) = Tracks, bit 1 (2) = Vias, bit 2 (4) = Pads, bit 3 (8) = Graphics
    pub fn query_nearest_filtered(
        &self,
        mm_x: f32,
        mm_y: f32,
        visible_mask: u32,
        max_dist_mm: f32,
        sel_filter_mask: u32,
    ) -> Option<HoveredPrimitiveInfo> {
        let cx = (mm_x / SPATIAL_CELL_MM).floor() as i32;
        let cy = (mm_y / SPATIAL_CELL_MM).floor() as i32;

        let mut best_idx: Option<usize> = None;
        let mut best_dist = max_dist_mm;

        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(bucket) = self.spatial_grid.get(&(cx + dx, cy + dy)) {
                    for &raw_idx in bucket {
                        let idx = raw_idx as usize;
                        let base = idx * FLOATS_PER_INSTANCE;
                        if base + FLOATS_PER_INSTANCE > self.instances.len() {
                            continue;
                        }
                        let chunk = &self.instances[base..base + FLOATS_PER_INSTANCE];
                        let layer_bits = chunk[6] as u32;
                        if (layer_bits & visible_mask) == 0 {
                            continue;
                        }

                        let raw_w = chunk[7];
                        let net_id = (raw_w / 16.0).floor() as u32;
                        let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;
                        let is_copper = (layer_bits
                            & (LAYER_F_CU | LAYER_B_CU | LAYER_IN1_CU | LAYER_IN2_CU))
                            != 0;

                        // Selection filter check
                        let allowed = match prim_type {
                            0 => {
                                if is_copper {
                                    (sel_filter_mask & 1) != 0
                                } else {
                                    (sel_filter_mask & 8) != 0
                                }
                            }
                            1 => (sel_filter_mask & 2) != 0 || (sel_filter_mask & 4) != 0,
                            2..=4 => (sel_filter_mask & 4) != 0,
                            _ => (sel_filter_mask & 8) != 0,
                        };
                        if !allowed {
                            continue;
                        }

                        let x0 = chunk[0];
                        let y0 = chunk[1];
                        let x1 = chunk[2];
                        let y1 = chunk[3];
                        let r = (chunk[4] * 0.5).max(0.10);

                        let dist = if prim_type == 0 {
                            let vx = x1 - x0;
                            let vy = y1 - y0;
                            let len_sq = vx * vx + vy * vy;
                            let t = if len_sq > 1e-6 {
                                (((mm_x - x0) * vx + (mm_y - y0) * vy) / len_sq).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            let px = x0 + t * vx;
                            let py = y0 + t * vy;
                            (((mm_x - px).powi(2) + (mm_y - py).powi(2)).sqrt() - r).max(0.0)
                        } else {
                            (((mm_x - x0).powi(2) + (mm_y - y0).powi(2)).sqrt() - r).max(0.0)
                        };

                        if dist < best_dist {
                            best_dist = dist;
                            best_idx = Some(idx);
                        }
                    }
                }
            }
        }

        let idx = best_idx?;
        let base = idx * FLOATS_PER_INSTANCE;
        let chunk = &self.instances[base..base + FLOATS_PER_INSTANCE];
        let x0 = chunk[0];
        let y0 = chunk[1];
        let x1 = chunk[2];
        let y1 = chunk[3];
        let w = chunk[4];
        let h = chunk[5];
        let layer_bits = chunk[6] as u32;
        let raw_w = chunk[7];
        let net_id = (raw_w / 16.0).floor() as u32;
        let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;

        let net_name = self
            .nets
            .get(&net_id)
            .map(|n| n.name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if net_id > 0 {
                    format!("Net #{}", net_id)
                } else {
                    "Unconnected".to_string()
                }
            });

        let layer_name = layer_bits_to_name(layer_bits);

        let net_tag = if net_id > 0 {
            format!(" | Net: {}", net_name)
        } else {
            String::new()
        };

        let (kind, summary) = match prim_type {
            0 => {
                let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
                (
                    "Track",
                    format!(
                        "Track [{}] W={:.3}mm L={:.3}mm{}",
                        layer_name, w, len, net_tag
                    ),
                )
            }
            1 => (
                "Via / PTH",
                format!(
                    "Via/PTH [{}] Ø{:.2}mm / Drill Ø{:.2}mm{}",
                    layer_name, w, h, net_tag
                ),
            ),
            2 | 3 => (
                "SMD Pad",
                format!("Pad [{}] {:.2}×{:.2}mm{}", layer_name, w, h, net_tag),
            ),
            4 => (
                "Rect PTH Pad",
                format!(
                    "Pad [{}] {:.2}×{:.2}mm / Drill Ø{:.2}mm{}",
                    layer_name, w, w, h, net_tag
                ),
            ),
            8 => (
                "Circle",
                format!("Circle [{}] Ø{:.2}mm", layer_name, (w + h) * 0.5),
            ),
            _ => (
                "Marker",
                format!("Marker @ ({:.2}, {:.2}) mm", x0, y0),
            ),
        };

        Some(HoveredPrimitiveInfo {
            instance_idx: idx,
            kind,
            prim_type,
            layer_name,
            net_id,
            net_name,
            summary,
            center_x: (x0 + x1) * 0.5,
            center_y: (y0 + y1) * 0.5,
        })
    }

    pub fn get_primitive(&self, idx: usize) -> Option<[f32; 8]> {
        let base = idx * FLOATS_PER_INSTANCE;
        if base + FLOATS_PER_INSTANCE > self.instances.len() {
            return None;
        }
        let s = &self.instances[base..base + FLOATS_PER_INSTANCE];
        Some([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
    }

    pub fn set_primitive(&mut self, idx: usize, data: [f32; 8]) -> bool {
        let base = idx * FLOATS_PER_INSTANCE;
        if base + FLOATS_PER_INSTANCE > self.instances.len() {
            return false;
        }
        self.instances[base..base + FLOATS_PER_INSTANCE].copy_from_slice(&data);
        self.rebuild_index_and_nets();
        true
    }

    pub fn rotate_primitive_90(&mut self, idx: usize) -> bool {
        let base = idx * FLOATS_PER_INSTANCE;
        if base + FLOATS_PER_INSTANCE > self.instances.len() {
            return false;
        }
        let x0 = self.instances[base];
        let y0 = self.instances[base + 1];
        let x1 = self.instances[base + 2];
        let y1 = self.instances[base + 3];
        let w = self.instances[base + 4];
        let h = self.instances[base + 5];
        let raw_w = self.instances[base + 7];
        let net_id = (raw_w / 16.0).floor() as u32;
        let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;

        if prim_type == 0 {
            let cx = 0.5 * (x0 + x1);
            let cy = 0.5 * (y0 + y1);
            let dx0 = x0 - cx;
            let dy0 = y0 - cy;
            let dx1 = x1 - cx;
            let dy1 = y1 - cy;
            self.instances[base] = cx - dy0;
            self.instances[base + 1] = cy + dx0;
            self.instances[base + 2] = cx - dy1;
            self.instances[base + 3] = cy + dx1;
        } else if prim_type == 2 || prim_type == 3 {
            self.instances[base + 4] = h;
            self.instances[base + 5] = w;
        }
        self.rebuild_index_and_nets();
        true
    }

    pub fn translate_primitive_in_place(&mut self, idx: usize, dx: f32, dy: f32) -> bool {
        let base = idx * FLOATS_PER_INSTANCE;
        if base + FLOATS_PER_INSTANCE > self.instances.len() {
            return false;
        }
        self.instances[base] += dx;
        self.instances[base + 1] += dy;
        self.instances[base + 2] += dx;
        self.instances[base + 3] += dy;
        true
    }

    pub fn delete_primitive(&mut self, idx: usize) -> bool {
        let base = idx * FLOATS_PER_INSTANCE;
        if base + FLOATS_PER_INSTANCE > self.instances.len() {
            return false;
        }
        self.instances.drain(base..base + FLOATS_PER_INSTANCE);
        self.rebuild_index_and_nets();
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_primitive(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        w: f32,
        h: f32,
        layer_mask: u32,
        net_id: u32,
        prim_type: f32,
    ) -> usize {
        let packed_w = (net_id as f32) * 16.0 + prim_type;
        let idx = self.instances.len() / FLOATS_PER_INSTANCE;
        self.instances
            .extend_from_slice(&[x0, y0, x1, y1, w, h, layer_mask as f32, packed_w]);
        self.rebuild_index_and_nets();
        idx
    }

    pub fn rebuild_index_and_nets(&mut self) {
        self.primitive_count = self.instances.len() / FLOATS_PER_INSTANCE;
        self.spatial_grid.clear();
        self.total_pads = 0;
        self.total_vias = 0;
        self.total_tracks = 0;

        for stats in self.nets.values_mut() {
            stats.total_length_mm = 0.0;
            stats.segment_count = 0;
            stats.via_count = 0;
            stats.pad_count = 0;
        }

        let copper_mask = LAYER_F_CU | LAYER_B_CU | LAYER_IN1_CU | LAYER_IN2_CU;

        for (idx, chunk) in self.instances.chunks_exact(FLOATS_PER_INSTANCE).enumerate() {
            let x0 = chunk[0];
            let y0 = chunk[1];
            let x1 = chunk[2];
            let y1 = chunk[3];
            let r = chunk[4].max(chunk[5]).max(0.25);
            let layer_bits = chunk[6] as u32;
            let raw_w = chunk[7];
            let net_id = (raw_w / 16.0).floor() as u32;
            let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;

            let min_cx = ((x0.min(x1) - r) / SPATIAL_CELL_MM).floor() as i32;
            let max_cx = ((x0.max(x1) + r) / SPATIAL_CELL_MM).floor() as i32;
            let min_cy = ((y0.min(y1) - r) / SPATIAL_CELL_MM).floor() as i32;
            let max_cy = ((y0.max(y1) + r) / SPATIAL_CELL_MM).floor() as i32;

            let span_x = (max_cx - min_cx).min(24);
            let span_y = (max_cy - min_cy).min(24);

            for cx in min_cx..=(min_cx + span_x) {
                for cy in min_cy..=(min_cy + span_y) {
                    self.spatial_grid
                        .entry((cx, cy))
                        .or_default()
                        .push(idx as u32);
                }
            }

            if prim_type == 0 && (layer_bits & copper_mask) != 0 {
                self.total_tracks += 1;
            } else if prim_type == 1 {
                self.total_vias += 1;
            } else if (2..=4).contains(&prim_type) {
                self.total_pads += 1;
            }

            if net_id > 0 {
                let entry = self.nets.entry(net_id).or_insert_with(|| NetStats {
                    id: net_id,
                    name: format!("Net-{}", net_id),
                    ..Default::default()
                });
                if prim_type == 0 && (layer_bits & copper_mask) != 0 {
                    let seg_len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
                    entry.total_length_mm += seg_len;
                    entry.segment_count += 1;
                } else if prim_type == 1 {
                    entry.via_count += 1;
                } else if (2..=4).contains(&prim_type) {
                    entry.pad_count += 1;
                }
            }
        }
    }

    pub fn export_kicad_pcb(&self) -> String {
        let mut out = String::with_capacity(self.primitive_count * 96 + 512);
        out.push_str("(kicad_pcb (version 20240108) (generator \"cadoxide_eda\")\n");
        out.push_str("  (general (thickness 1.6))\n");
        out.push_str("  (layers\n    (0 \"F.Cu\" signal)\n    (1 \"In1.Cu\" signal)\n    (2 \"In2.Cu\" signal)\n    (31 \"B.Cu\" signal)\n    (36 \"B.SilkS\" user)\n    (37 \"F.SilkS\" user)\n    (38 \"B.Mask\" user)\n    (39 \"F.Mask\" user)\n    (44 \"Edge.Cuts\" user)\n    (46 \"B.CrtYd\" user)\n    (47 \"F.CrtYd\" user)\n    (48 \"B.Fab\" user)\n    (49 \"F.Fab\" user)\n  )\n");

        let mut net_list: Vec<_> = self.nets.values().collect();
        net_list.sort_by_key(|n| n.id);
        out.push_str("  (net 0 \"\")\n");
        for n in net_list {
            if n.id > 0 {
                out.push_str(&format!("  (net {} \"{}\")\n", n.id, n.name));
            }
        }

        for chunk in self.instances.chunks_exact(FLOATS_PER_INSTANCE) {
            let x0 = chunk[0];
            let y0 = chunk[1];
            let x1 = chunk[2];
            let y1 = chunk[3];
            let w = chunk[4];
            let h = chunk[5];
            let layer_bits = chunk[6] as u32;
            let raw_w = chunk[7];
            let net_id = (raw_w / 16.0).floor() as u32;
            let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;
            let layer_str = layer_bits_to_name(layer_bits);

            match prim_type {
                0 => {
                    if (layer_bits & (LAYER_F_CU | LAYER_B_CU | LAYER_IN1_CU | LAYER_IN2_CU)) != 0 {
                        out.push_str(&format!(
                            "  (segment (start {:.4} {:.4}) (end {:.4} {:.4}) (width {:.4}) (layer \"{}\") (net {}))\n",
                            x0, y0, x1, y1, w, layer_str, net_id
                        ));
                    } else {
                        out.push_str(&format!(
                            "  (gr_line (start {:.4} {:.4}) (end {:.4} {:.4}) (width {:.4}) (layer \"{}\"))\n",
                            x0, y0, x1, y1, w, layer_str
                        ));
                    }
                }
                1 => {
                    out.push_str(&format!(
                        "  (via (at {:.4} {:.4}) (size {:.4}) (drill {:.4}) (layers \"F.Cu\" \"B.Cu\") (net {}))\n",
                        x0, y0, w, h, net_id
                    ));
                }
                8 => {
                    let r = (w + h) * 0.25;
                    out.push_str(&format!(
                        "  (gr_circle (center {:.4} {:.4}) (end {:.4} {:.4}) (width {:.4}) (layer \"{}\"))\n",
                        x0, y0, x0 + r, y0, (w - h) * 0.5, layer_str
                    ));
                }
                _ => {}
            }
        }

        for t in &self.texts {
            if !t.is_ref {
                out.push_str(&format!(
                    "  (gr_text \"{}\" (at {:.4} {:.4} {:.1}) (layer \"{}\") (effects (font (size {:.2} {:.2}))))\n",
                    t.text,
                    t.x,
                    t.y,
                    t.angle_deg,
                    layer_bits_to_name(t.layer_mask),
                    t.size_mm,
                    t.size_mm
                ));
            }
        }

        out.push_str(")\n");
        out
    }
}

pub fn layer_bits_to_name(layer_bits: u32) -> &'static str {
    if (layer_bits & LAYER_F_CU) != 0 && (layer_bits & LAYER_B_CU) != 0 {
        "Multi/PTH"
    } else if (layer_bits & LAYER_F_CU) != 0 {
        "F.Cu"
    } else if (layer_bits & LAYER_IN1_CU) != 0 {
        "In1.Cu"
    } else if (layer_bits & LAYER_IN2_CU) != 0 {
        "In2.Cu"
    } else if (layer_bits & LAYER_B_CU) != 0 {
        "B.Cu"
    } else if (layer_bits & LAYER_F_PASTE) != 0 {
        "F.Paste"
    } else if (layer_bits & LAYER_B_PASTE) != 0 {
        "B.Paste"
    } else if (layer_bits & LAYER_F_SILKS) != 0 {
        "F.SilkS"
    } else if (layer_bits & LAYER_B_SILKS) != 0 {
        "B.SilkS"
    } else if (layer_bits & LAYER_F_MASK) != 0 {
        "F.Mask"
    } else if (layer_bits & LAYER_B_MASK) != 0 {
        "B.Mask"
    } else if (layer_bits & LAYER_EDGE_CUTS) != 0 {
        "Edge.Cuts"
    } else if (layer_bits & LAYER_MARGIN) != 0 {
        "Margin"
    } else if (layer_bits & LAYER_F_CRTYD) != 0 {
        "F.CrtYd"
    } else if (layer_bits & LAYER_B_CRTYD) != 0 {
        "B.CrtYd"
    } else if (layer_bits & LAYER_F_FAB) != 0 {
        "F.Fab"
    } else if (layer_bits & LAYER_B_FAB) != 0 {
        "B.Fab"
    } else if (layer_bits & LAYER_USER_DWGS) != 0 {
        "User.Drawings"
    } else if (layer_bits & LAYER_USER_CMTS) != 0 {
        "User.Comments"
    } else {
        "F.Cu"
    }
}

/// Parses a KiCad `.kicad_pcb` file in pure Rust into GPU instances, copper zones, pad labels, and texts.
pub fn parse_kicad_pcb_str(pcb_source: &str) -> ParsedBoard {
    let mut instances = Vec::with_capacity(8192 * FLOATS_PER_INSTANCE);
    let mut primitive_count = 0;
    let mut nets: HashMap<u32, NetStats> = HashMap::new();
    let mut zones: Vec<CopperZone> = Vec::new();
    let mut pad_labels: Vec<PadLabel> = Vec::new();
    let mut texts: Vec<BoardText> = Vec::new();

    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;

    let mut in_footprint = false;
    let mut fp_depth: i32 = 0;
    let mut fp_x: f32 = 0.0;
    let mut fp_y: f32 = 0.0;
    let mut fp_rot_rad: f32 = 0.0;

    let mut active_block = String::with_capacity(4096);
    let mut block_depth: i32 = 0;
    let mut collecting_block = false;
    let mut collecting_zone = false;

    for raw_line in pcb_source.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let line_delta = count_parens(line);

        if collecting_block {
            active_block.push(' ');
            active_block.push_str(line);
            block_depth += line_delta;
            if in_footprint {
                fp_depth += line_delta;
            }
            if block_depth <= 0 {
                collecting_block = false;
                if collecting_zone {
                    collecting_zone = false;
                    parse_zone_block(&active_block, &mut zones);
                } else {
                    process_primitive_block(
                        &active_block,
                        in_footprint,
                        fp_x,
                        fp_y,
                        fp_rot_rad,
                        &mut instances,
                        &mut primitive_count,
                        &mut nets,
                        &mut pad_labels,
                        &mut texts,
                        &mut min_x,
                        &mut min_y,
                        &mut max_x,
                        &mut max_y,
                    );
                }
                active_block.clear();
            }
            if in_footprint && fp_depth <= 0 {
                in_footprint = false;
            }
            continue;
        }

        // Top-level net declaration: (net 11 "GND")
        if !in_footprint && is_token_start(line, "(net") {
            if let Some((net_id, Some(net_name))) = extract_net_def(line) {
                if net_id > 0 {
                    let entry = nets.entry(net_id).or_insert_with(|| NetStats {
                        id: net_id,
                        name: net_name.clone(),
                        ..Default::default()
                    });
                    if entry.name.is_empty() {
                        entry.name = net_name;
                    }
                }
            }
            continue;
        }

        if is_token_start(line, "(footprint") || is_token_start(line, "(module") {
            in_footprint = true;
            fp_depth = line_delta;
            fp_x = 0.0;
            fp_y = 0.0;
            fp_rot_rad = 0.0;
            continue;
        }

        if in_footprint {
            let prev_fp_depth = fp_depth;
            fp_depth += line_delta;
            if fp_depth <= 0 {
                in_footprint = false;
            } else if prev_fp_depth == 1 && line.starts_with("(at ") {
                if let Some((x, y, rot)) = parse_at_triple(line) {
                    fp_x = x;
                    fp_y = y;
                    fp_rot_rad = rot.to_radians();
                }
            }
        }

        if !in_footprint && is_token_start(line, "(zone") {
            if line_delta <= 0 {
                parse_zone_block(line, &mut zones);
            } else {
                active_block.clear();
                active_block.push_str(line);
                block_depth = line_delta;
                collecting_block = true;
                collecting_zone = true;
            }
            continue;
        }

        if is_token_start(line, "(segment")
            || is_token_start(line, "(via")
            || is_token_start(line, "(gr_line")
            || is_token_start(line, "(fp_line")
            || is_token_start(line, "(gr_rect")
            || is_token_start(line, "(fp_rect")
            || is_token_start(line, "(gr_circle")
            || is_token_start(line, "(fp_circle")
            || is_token_start(line, "(gr_arc")
            || is_token_start(line, "(fp_arc")
            || is_token_start(line, "(gr_text")
            || is_token_start(line, "(fp_text")
            || (in_footprint
                && (is_token_start(line, "(pad") || is_token_start(line, "(property")))
        {
            if line_delta <= 0 {
                process_primitive_block(
                    line,
                    in_footprint,
                    fp_x,
                    fp_y,
                    fp_rot_rad,
                    &mut instances,
                    &mut primitive_count,
                    &mut nets,
                    &mut pad_labels,
                    &mut texts,
                    &mut min_x,
                    &mut min_y,
                    &mut max_x,
                    &mut max_y,
                );
            } else {
                active_block.clear();
                active_block.push_str(line);
                block_depth = line_delta;
                collecting_block = true;
                collecting_zone = false;
            }
        }
    }

    // Backfill pad_labels net_name from top-level nets table if any were missing
    for pl in &mut pad_labels {
        if pl.net_name.is_empty() && pl.net_id > 0 {
            if let Some(n) = nets.get(&pl.net_id) {
                pl.net_name = n.name.clone();
            }
        }
    }

    let bounds = if min_x < max_x && min_y < max_y {
        [min_x - 2.0, min_y - 2.0, max_x + 2.0, max_y + 2.0]
    } else {
        [70.0, 40.0, 190.0, 150.0]
    };

    let mut board = ParsedBoard::with_spatial_grid(instances, primitive_count, bounds, nets);
    board.zones = zones;
    board.pad_labels = pad_labels;
    board.texts = texts;
    board.rebuild_ratsnest();
    board
}

fn parse_zone_block(block: &str, zones: &mut Vec<CopperZone>) {
    let (net_id, net_name_opt) = extract_net_def(block).unwrap_or((0, None));
    let net_name = extract_quoted_after(block, "(net_name ")
        .or(net_name_opt)
        .unwrap_or_default();
    let layer_mask = extract_layer_mask(block);
    if layer_mask == 0 {
        return;
    }
    let is_keepout = block.contains("(keepout");
    let is_teardrop = block.contains("$teardrop");

    let mut outline = Vec::new();
    if let Some(poly_idx) = block.find("(polygon") {
        let rest = &block[poly_idx..];
        let end_idx = rest.find("(filled_polygon").unwrap_or(rest.len());
        outline = extract_xy_pts(&rest[..end_idx]);
    }

    let mut filled_polys = Vec::new();
    let mut search_from = 0;
    while let Some(rel_idx) = block[search_from..].find("(filled_polygon") {
        let start = search_from + rel_idx;
        let sub = &block[start + 15..];
        let next_fp = sub.find("(filled_polygon").unwrap_or(sub.len());
        let pts = extract_xy_pts(&sub[..next_fp]);
        if pts.len() >= 3 {
            filled_polys.push(pts);
        }
        search_from = start + 15 + next_fp;
    }

    if !outline.is_empty() || !filled_polys.is_empty() {
        let bounds = CopperZone::compute_bounds(&outline, &filled_polys);
        zones.push(CopperZone {
            net_id,
            net_name,
            layer_mask,
            is_keepout,
            is_teardrop,
            bounds,
            outline,
            filled_polys,
        });
    }
}

fn extract_xy_pts(s: &str) -> Vec<(f32, f32)> {
    let mut pts = Vec::new();
    let mut cursor = 0;
    while let Some(idx) = s[cursor..].find("(xy ") {
        let after = cursor + idx + 4;
        if let Some(close) = s[after..].find(')') {
            let inner = &s[after..after + close];
            let mut it = inner.split_whitespace();
            if let (Some(xs), Some(ys)) = (it.next(), it.next()) {
                if let (Ok(x), Ok(y)) = (xs.parse::<f32>(), ys.parse::<f32>()) {
                    pts.push((x, y));
                }
            }
            cursor = after + close + 1;
        } else {
            break;
        }
    }
    pts
}

/// Computes a pure-Rust geometric vector diff between Revision A and Revision B!
pub fn compute_vector_diff_boards(
    board_a: &ParsedBoard,
    board_b: &ParsedBoard,
) -> (ParsedBoard, usize, usize, usize) {
    let quantize = |inst: &[f32]| -> (i32, i32, i32, i32, i32, u32) {
        (
            (inst[0] * 100.0).round() as i32,
            (inst[1] * 100.0).round() as i32,
            (inst[2] * 100.0).round() as i32,
            (inst[3] * 100.0).round() as i32,
            (inst[4] * 100.0).round() as i32,
            inst[6] as u32,
        )
    };

    let mut set_a = HashSet::with_capacity(board_a.primitive_count);
    for chunk in board_a.instances.chunks_exact(FLOATS_PER_INSTANCE) {
        set_a.insert(quantize(chunk));
    }

    let mut set_b = HashSet::with_capacity(board_b.primitive_count);
    for chunk in board_b.instances.chunks_exact(FLOATS_PER_INSTANCE) {
        set_b.insert(quantize(chunk));
    }

    let mut out_instances =
        Vec::with_capacity((board_a.primitive_count + board_b.primitive_count) * FLOATS_PER_INSTANCE);
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut unchanged = 0usize;

    for chunk in board_a.instances.chunks_exact(FLOATS_PER_INSTANCE) {
        let key = quantize(chunk);
        let mut inst = [0.0f32; FLOATS_PER_INSTANCE];
        inst.copy_from_slice(chunk);
        if set_b.contains(&key) {
            inst[6] = (chunk[6] as u32 | LAYER_DIFF_UNCHANGED) as f32;
            unchanged += 1;
        } else {
            inst[6] = (chunk[6] as u32 | LAYER_DIFF_REMOVED) as f32;
            removed += 1;
        }
        out_instances.extend_from_slice(&inst);
    }

    for chunk in board_b.instances.chunks_exact(FLOATS_PER_INSTANCE) {
        let key = quantize(chunk);
        if !set_a.contains(&key) {
            let mut inst = [0.0f32; FLOATS_PER_INSTANCE];
            inst.copy_from_slice(chunk);
            inst[6] = (chunk[6] as u32 | LAYER_DIFF_ADDED) as f32;
            added += 1;
            out_instances.extend_from_slice(&inst);
        }
    }

    let primitive_count = out_instances.len() / FLOATS_PER_INSTANCE;
    (
        ParsedBoard::with_spatial_grid(
            out_instances,
            primitive_count,
            board_b.bounds,
            board_b.nets.clone(),
        ),
        added,
        removed,
        unchanged,
    )
}

/// Generates a valid, complete KiCad .kicad_pcb string for a starter project template.
pub fn generate_template_kicad_pcb(
    template: &str,
    project_name: &str,
    width_mm: f32,
    height_mm: f32,
) -> String {
    let mut s = String::with_capacity(4096);
    s.push_str("(kicad_pcb (version 20240108) (generator \"cadoxide_eda\")\n");
    s.push_str("  (general (thickness 1.6))\n");
    s.push_str("  (layers\n    (0 \"F.Cu\" signal)\n    (1 \"In1.Cu\" signal)\n    (2 \"In2.Cu\" signal)\n    (31 \"B.Cu\" signal)\n    (36 \"B.SilkS\" user)\n    (37 \"F.SilkS\" user)\n    (38 \"B.Mask\" user)\n    (39 \"F.Mask\" user)\n    (44 \"Edge.Cuts\" user)\n    (46 \"B.CrtYd\" user)\n    (47 \"F.CrtYd\" user)\n    (48 \"B.Fab\" user)\n    (49 \"F.Fab\" user)\n  )\n");
    s.push_str("  (net 0 \"\")\n");
    s.push_str("  (net 1 \"GND\")\n");
    s.push_str("  (net 2 \"+3V3\")\n");
    s.push_str("  (net 3 \"+5V\")\n");

    let (w, h) = match template {
        "rpi_hat" => (65.0, 56.0),
        "iot_sensor" => (50.0, 40.0),
        "eurocard" => (160.0, 100.0),
        _ => (width_mm.max(25.0), height_mm.max(25.0)),
    };

    let x0 = 10.0f32;
    let y0 = 10.0f32;
    let x1 = x0 + w;
    let y1 = y0 + h;

    // Edge.Cuts rectangular boundary
    s.push_str(&format!(
        "  (gr_line (start {:.2} {:.2}) (end {:.2} {:.2}) (layer \"Edge.Cuts\") (width 0.15))\n",
        x0, y0, x1, y0
    ));
    s.push_str(&format!(
        "  (gr_line (start {:.2} {:.2}) (end {:.2} {:.2}) (layer \"Edge.Cuts\") (width 0.15))\n",
        x1, y0, x1, y1
    ));
    s.push_str(&format!(
        "  (gr_line (start {:.2} {:.2}) (end {:.2} {:.2}) (layer \"Edge.Cuts\") (width 0.15))\n",
        x1, y1, x0, y1
    ));
    s.push_str(&format!(
        "  (gr_line (start {:.2} {:.2}) (end {:.2} {:.2}) (layer \"Edge.Cuts\") (width 0.15))\n",
        x0, y1, x0, y0
    ));

    // Corner mounting holes (M3: 3.2mm drill, or Pi HAT: 2.75mm drill)
    let drill_diam = if template == "rpi_hat" { 2.75 } else { 3.2 };
    let pad_diam = drill_diam + 2.4;
    let inset = if template == "rpi_hat" { 3.5 } else { 4.5 };
    let holes = [
        (x0 + inset, y0 + inset),
        (x1 - inset, y0 + inset),
        (x1 - inset, y1 - inset),
        (x0 + inset, y1 - inset),
    ];
    for (i, &(hx, hy)) in holes.iter().enumerate() {
        s.push_str(&format!(
            "  (footprint \"MountingHole:MountingHole_{:.1}mm_M{:.1}\" (layer \"F.Cu\")\n    (at {:.2} {:.2})\n    (property \"Reference\" \"H{}\" (at {:.2} {:.2}) (layer \"F.SilkS\") (effects (font (size 1 1) (thickness 0.15))))\n    (property \"Value\" \"MountingHole\" (at {:.2} {:.2}) (layer \"F.Fab\") (effects (font (size 1 1) (thickness 0.15))))\n    (pad \"1\" thru_hole circle (at 0 0) (size {:.2} {:.2}) (drill {:.2}) (layers \"*.Cu\" \"*.Mask\"))\n  )\n",
            drill_diam, drill_diam, hx, hy, i + 1, hx, hy - 2.8, hx, hy + 2.8, pad_diam, pad_diam, drill_diam
        ));
    }

    // If Raspberry Pi HAT template, add standard 40-pin header pads (2x20 at 2.54mm pitch)
    if template == "rpi_hat" {
        let pin_start_x = x0 + 7.0;
        let pin_start_y = y0 + 3.5;
        s.push_str(&format!(
            "  (footprint \"Connector_PinHeader_2.54mm:PinHeader_2x20_P2.54mm_Vertical\" (layer \"F.Cu\")\n    (at {:.2} {:.2})\n    (property \"Reference\" \"J1\" (at {:.2} {:.2}) (layer \"F.SilkS\") (effects (font (size 1 1) (thickness 0.15))))\n    (property \"Value\" \"Raspberry_Pi_GPIO_40Pin\" (at {:.2} {:.2}) (layer \"F.Fab\") (effects (font (size 1 1) (thickness 0.15))))\n",
            pin_start_x + 24.13, pin_start_y + 1.27, pin_start_x, pin_start_y - 2.0, pin_start_x, pin_start_y + 4.5
        ));
        for col in 0..20 {
            let px = (col as f32) * 2.54;
            let pin1 = col * 2 + 1;
            let pin2 = col * 2 + 2;
            let net1 = if pin1 == 1 { 2 } else if pin1 == 9 || pin1 == 25 || pin1 == 39 { 1 } else { 0 };
            let net2 = if pin2 == 2 || pin2 == 4 { 3 } else if pin2 == 6 || pin2 == 14 || pin2 == 20 || pin2 == 30 || pin2 == 34 { 1 } else { 0 };
            s.push_str(&format!(
                "    (pad \"{}\" thru_hole circle (at {:.2} 0) (size 1.7 1.7) (drill 1.0) (layers \"*.Cu\" \"*.Mask\") (net {}))\n",
                pin1, px, net1
            ));
            s.push_str(&format!(
                "    (pad \"{}\" thru_hole circle (at {:.2} 2.54) (size 1.7 1.7) (drill 1.0) (layers \"*.Cu\" \"*.Mask\") (net {}))\n",
                pin2, px, net2
            ));
        }
        s.push_str("  )\n");
    }

    // Title and metadata silkscreen text
    let clean_title = if project_name.trim().is_empty() {
        "NEW_PROJECT"
    } else {
        project_name.trim()
    };
    s.push_str(&format!(
        "  (gr_text \"{}\" (at {:.2} {:.2}) (layer \"F.SilkS\") (effects (font (size 2.2 2.2) (thickness 0.3))))\n",
        clean_title, x0 + w * 0.5, y0 + h * 0.5 - 2.0
    ));
    s.push_str(&format!(
        "  (gr_text \"CADOxide EDA · Rev 1.0\" (at {:.2} {:.2}) (layer \"F.SilkS\") (effects (font (size 1.3 1.3) (thickness 0.2))))\n",
        x0 + w * 0.5, y0 + h * 0.5 + 3.0
    ));

    s.push_str(")\n");
    s
}

/// Generates a 100,000+ primitive High-Density Interconnect (HDI) stress-test board in Rust memory.
pub fn generate_hdi_stress_board(target_primitives: usize) -> ParsedBoard {
    let mut instances = Vec::with_capacity(target_primitives * FLOATS_PER_INSTANCE);
    let mut primitive_count = 0;

    let min_x = 70.0_f32;
    let min_y = 40.0_f32;
    let width = 120.0_f32;
    let height = 110.0_f32;
    let bounds = [min_x, min_y, min_x + width, min_y + height];

    let mx = min_x + width;
    let my = min_y + height;
    let edge = LAYER_EDGE_CUTS as f32;
    push_raw(&mut instances, &mut primitive_count, min_x, min_y, mx, min_y, 0.3, 0.3, edge, 0.0);
    push_raw(&mut instances, &mut primitive_count, mx, min_y, mx, my, 0.3, 0.3, edge, 0.0);
    push_raw(&mut instances, &mut primitive_count, mx, my, min_x, my, 0.3, 0.3, edge, 0.0);
    push_raw(&mut instances, &mut primitive_count, min_x, my, min_x, min_y, 0.3, 0.3, edge, 0.0);

    let cols = ((target_primitives as f32).sqrt() as usize).max(10);
    let rows = (target_primitives / cols).max(10);
    let dx = (width - 6.0) / (cols as f32);
    let dy = (height - 6.0) / (rows as f32);

    for r in 0..rows {
        let y = min_y + 3.0 + (r as f32) * dy;
        for c in 0..cols {
            if primitive_count >= target_primitives {
                break;
            }
            let x = min_x + 3.0 + (c as f32) * dx;
            let idx = r * cols + c;
            let synthetic_net = ((idx % 16) + 1) as f32;

            match idx % 4 {
                0 => push_raw(
                    &mut instances,
                    &mut primitive_count,
                    x,
                    y,
                    x + dx * 0.78,
                    y + dy * 0.38,
                    0.11,
                    0.11,
                    LAYER_F_CU as f32,
                    synthetic_net * 16.0,
                ),
                1 => push_raw(
                    &mut instances,
                    &mut primitive_count,
                    x,
                    y,
                    x + dx * 0.38,
                    y + dy * 0.82,
                    0.11,
                    0.11,
                    LAYER_B_CU as f32,
                    synthetic_net * 16.0,
                ),
                2 => push_raw(
                    &mut instances,
                    &mut primitive_count,
                    x,
                    y,
                    x,
                    y,
                    0.26,
                    0.13,
                    (LAYER_F_CU | LAYER_B_CU) as f32,
                    synthetic_net * 16.0 + 1.0,
                ),
                _ => push_raw(
                    &mut instances,
                    &mut primitive_count,
                    x,
                    y,
                    x,
                    y,
                    0.22,
                    0.22,
                    (LAYER_F_CU | LAYER_F_SILKS) as f32,
                    synthetic_net * 16.0 + 2.0,
                ),
            }
        }
    }

    ParsedBoard::with_spatial_grid(instances, primitive_count, bounds, HashMap::new())
}

fn is_token_start(line: &str, token: &str) -> bool {
    if !line.starts_with(token) {
        return false;
    }
    match line.as_bytes().get(token.len()) {
        None | Some(b' ') | Some(b'\t') | Some(b'(') | Some(b')') => true,
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn process_primitive_block(
    block: &str,
    in_footprint: bool,
    fp_x: f32,
    fp_y: f32,
    fp_rot_rad: f32,
    instances: &mut Vec<f32>,
    primitive_count: &mut usize,
    nets: &mut HashMap<u32, NetStats>,
    pad_labels: &mut Vec<PadLabel>,
    texts: &mut Vec<BoardText>,
    min_x: &mut f32,
    min_y: &mut f32,
    max_x: &mut f32,
    max_y: &mut f32,
) {
    let mut update_bounds = |x: f32, y: f32| {
        if x > 1.0 && y > 1.0 && x < 1000.0 && y < 1000.0 {
            if x < *min_x {
                *min_x = x;
            }
            if y < *min_y {
                *min_y = y;
            }
            if x > *max_x {
                *max_x = x;
            }
            if y > *max_y {
                *max_y = y;
            }
        }
    };

    if is_token_start(block, "(segment") {
        if let (Some((x1, y1)), Some((x2, y2))) = (
            extract_pair(block, "(start "),
            extract_pair(block, "(end "),
        ) {
            let width = extract_single(block, "(width ").unwrap_or(0.25);
            let layer_mask = extract_layer_mask(block);
            let net_id = extract_single(block, "(net ").unwrap_or(0.0) as u32;
            if layer_mask != 0 {
                update_bounds(x1, y1);
                update_bounds(x2, y2);
                if net_id > 0 {
                    let seg_len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
                    let entry = nets.entry(net_id).or_insert_with(|| NetStats {
                        id: net_id,
                        ..Default::default()
                    });
                    entry.total_length_mm += seg_len;
                    entry.segment_count += 1;
                }
                let encoded = (net_id as f32) * 16.0;
                push_raw(
                    instances,
                    primitive_count,
                    x1,
                    y1,
                    x2,
                    y2,
                    width,
                    width,
                    layer_mask as f32,
                    encoded,
                );
            }
        }
    } else if is_token_start(block, "(via") {
        if let Some((x, y)) = extract_pair(block, "(at ") {
            let size = extract_single(block, "(size ").unwrap_or(0.8);
            let drill = extract_single(block, "(drill ").unwrap_or(0.4);
            let net_id = extract_single(block, "(net ").unwrap_or(0.0) as u32;
            update_bounds(x, y);
            if net_id > 0 {
                let entry = nets.entry(net_id).or_insert_with(|| NetStats {
                    id: net_id,
                    ..Default::default()
                });
                entry.via_count += 1;
            }
            let mask = LAYER_F_CU | LAYER_B_CU;
            let encoded = (net_id as f32) * 16.0 + 1.0;
            push_raw(
                instances,
                primitive_count,
                x,
                y,
                x,
                y,
                size,
                drill,
                mask as f32,
                encoded,
            );
        }
    } else if is_token_start(block, "(gr_line") || is_token_start(block, "(fp_line") {
        if let (Some((lx1, ly1)), Some((lx2, ly2))) = (
            extract_pair(block, "(start "),
            extract_pair(block, "(end "),
        ) {
            let width = extract_single(block, "(width ").unwrap_or(0.15);
            let layer_mask = extract_layer_mask(block);
            if layer_mask != 0 {
                let (x1, y1, x2, y2) = if is_token_start(block, "(fp_line") && in_footprint {
                    let (tx1, ty1) = transform_point(lx1, ly1, fp_x, fp_y, fp_rot_rad);
                    let (tx2, ty2) = transform_point(lx2, ly2, fp_x, fp_y, fp_rot_rad);
                    (tx1, ty1, tx2, ty2)
                } else {
                    (lx1, ly1, lx2, ly2)
                };
                update_bounds(x1, y1);
                update_bounds(x2, y2);
                push_raw(instances, primitive_count, x1, y1, x2, y2, width, width, layer_mask as f32, 0.0);
            }
        }
    } else if is_token_start(block, "(gr_rect") || is_token_start(block, "(fp_rect") {
        if let (Some((lx1, ly1)), Some((lx2, ly2))) = (
            extract_pair(block, "(start "),
            extract_pair(block, "(end "),
        ) {
            let width = extract_single(block, "(width ").unwrap_or(0.18);
            let layer_mask = extract_layer_mask(block);
            if layer_mask != 0 {
                let (x1, y1, x2, y2) = if is_token_start(block, "(fp_rect") && in_footprint {
                    let (tx1, ty1) = transform_point(lx1, ly1, fp_x, fp_y, fp_rot_rad);
                    let (tx2, ty2) = transform_point(lx2, ly2, fp_x, fp_y, fp_rot_rad);
                    (tx1, ty1, tx2, ty2)
                } else {
                    (lx1, ly1, lx2, ly2)
                };
                update_bounds(x1, y1);
                update_bounds(x2, y2);
                push_raw(instances, primitive_count, x1, y1, x2, y1, width, width, layer_mask as f32, 0.0);
                push_raw(instances, primitive_count, x2, y1, x2, y2, width, width, layer_mask as f32, 0.0);
                push_raw(instances, primitive_count, x2, y2, x1, y2, width, width, layer_mask as f32, 0.0);
                push_raw(instances, primitive_count, x1, y2, x1, y1, width, width, layer_mask as f32, 0.0);
            }
        }
    } else if is_token_start(block, "(gr_circle") || is_token_start(block, "(fp_circle") {
        if let (Some((lcx, lcy)), Some((lex, ley))) = (
            extract_pair(block, "(center "),
            extract_pair(block, "(end "),
        ) {
            let width = extract_single(block, "(width ").unwrap_or(0.15);
            let layer_mask = extract_layer_mask(block);
            if layer_mask != 0 {
                let (cx, cy, ex, ey) = if is_token_start(block, "(fp_circle") && in_footprint {
                    let (tcx, tcy) = transform_point(lcx, lcy, fp_x, fp_y, fp_rot_rad);
                    let (tex, tey) = transform_point(lex, ley, fp_x, fp_y, fp_rot_rad);
                    (tcx, tcy, tex, tey)
                } else {
                    (lcx, lcy, lex, ley)
                };
                let radius = ((ex - cx).powi(2) + (ey - cy).powi(2)).sqrt();
                update_bounds(cx - radius, cy - radius);
                update_bounds(cx + radius, cy + radius);
                let outer_d = radius * 2.0 + width;
                let inner_d = (radius * 2.0 - width).max(0.0);
                push_raw(
                    instances,
                    primitive_count,
                    cx,
                    cy,
                    cx,
                    cy,
                    outer_d,
                    inner_d,
                    layer_mask as f32,
                    8.0,
                );
            }
        }
    } else if is_token_start(block, "(gr_arc") || is_token_start(block, "(fp_arc") {
        if let (Some((lsx, lsy)), Some((lmx, lmy)), Some((lex, ley))) = (
            extract_pair(block, "(start "),
            extract_pair(block, "(mid "),
            extract_pair(block, "(end "),
        ) {
            let width = extract_single(block, "(width ").unwrap_or(0.15);
            let layer_mask = extract_layer_mask(block);
            if layer_mask != 0 {
                let (sx, sy, mx, my, ex, ey) =
                    if is_token_start(block, "(fp_arc") && in_footprint {
                        let (tsx, tsy) = transform_point(lsx, lsy, fp_x, fp_y, fp_rot_rad);
                        let (tmx, tmy) = transform_point(lmx, lmy, fp_x, fp_y, fp_rot_rad);
                        let (tex, tey) = transform_point(lex, ley, fp_x, fp_y, fp_rot_rad);
                        (tsx, tsy, tmx, tmy, tex, tey)
                    } else {
                        (lsx, lsy, lmx, lmy, lex, ley)
                    };
                update_bounds(sx, sy);
                update_bounds(mx, my);
                update_bounds(ex, ey);
                emit_arc_segments(
                    instances,
                    primitive_count,
                    sx,
                    sy,
                    mx,
                    my,
                    ex,
                    ey,
                    width,
                    layer_mask as f32,
                );
            }
        }
    } else if is_token_start(block, "(pad") && in_footprint {
        if let (Some((px, py, prot_deg)), Some((raw_pw, raw_ph))) = (
            parse_at_triple(block),
            extract_pair(block, "(size "),
        ) {
            let (wx, wy) = transform_point(px, py, fp_x, fp_y, fp_rot_rad);
            let pad_rot_rad = prot_deg.to_radians();
            let (pw, ph) = if pad_rot_rad.sin().abs() > 0.707 {
                (raw_ph, raw_pw)
            } else {
                (raw_pw, raw_ph)
            };
            let drill = extract_single(block, "(drill ").unwrap_or(0.0);
            let is_circle_or_oval = block.contains(" circle ") || block.contains(" oval ");
            let layer_mask = extract_layer_mask(block);
            let (net_id, net_name_opt) = extract_net_def(block).unwrap_or((0, None));
            let pin_num = extract_pad_pin_number(block);

            if layer_mask != 0 {
                update_bounds(wx, wy);
                let mut resolved_net_name = net_name_opt.clone().unwrap_or_default();
                if net_id > 0 {
                    let entry = nets.entry(net_id).or_insert_with(|| NetStats {
                        id: net_id,
                        ..Default::default()
                    });
                    entry.pad_count += 1;
                    if entry.name.is_empty() {
                        if let Some(n) = net_name_opt {
                            entry.name = n;
                        }
                    }
                    if resolved_net_name.is_empty() {
                        resolved_net_name = entry.name.clone();
                    }
                }
                let (out_w, out_h, pad_prim) = if drill > 0.01 {
                    if is_circle_or_oval {
                        (pw.max(ph), drill, 1.0)
                    } else {
                        (pw.max(ph), drill, 4.0)
                    }
                } else if is_circle_or_oval {
                    (pw, ph, 3.0)
                } else {
                    (pw, ph, 2.0)
                };

                pad_labels.push(PadLabel {
                    x: wx,
                    y: wy,
                    w: pw,
                    h: ph,
                    pin: pin_num,
                    net_id,
                    net_name: resolved_net_name,
                    layer_mask,
                });

                let encoded = (net_id as f32) * 16.0 + pad_prim;
                push_raw(
                    instances,
                    primitive_count,
                    wx,
                    wy,
                    wx,
                    wy,
                    out_w,
                    out_h,
                    layer_mask as f32,
                    encoded,
                );
            }
        }
    } else if (is_token_start(block, "(property")
        || is_token_start(block, "(fp_text")
        || is_token_start(block, "(gr_text"))
        && !block.contains("(hide yes)")
        && !block.contains(" hide ")
    {
        if let Some((lx, ly, lrot)) = parse_at_triple(block) {
            let layer_mask = extract_layer_mask(block);
            if layer_mask != 0 {
                let (wx, wy) = if in_footprint && !is_token_start(block, "(gr_text") {
                    transform_point(lx, ly, fp_x, fp_y, fp_rot_rad)
                } else {
                    (lx, ly)
                };
                let size_mm = extract_pair(block, "(size ")
                    .map(|(sx, _)| sx)
                    .unwrap_or(1.0);
                if let Some((txt, is_ref)) = extract_text_value(block) {
                    if !txt.is_empty() && txt != "~" {
                        texts.push(BoardText {
                            x: wx,
                            y: wy,
                            angle_deg: lrot,
                            text: txt.replace("\\n", "\n"),
                            size_mm,
                            layer_mask,
                            is_ref,
                        });
                    }
                }
            }
        }
    }
}

fn extract_pad_pin_number(block: &str) -> String {
    let rest = block.strip_prefix("(pad").unwrap_or(block).trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        if let Some(end_q) = stripped.find('"') {
            return stripped[..end_q].to_string();
        }
    }
    rest.split_whitespace().next().unwrap_or("").to_string()
}

fn extract_quoted_after(s: &str, prefix: &str) -> Option<String> {
    let idx = s.find(prefix)?;
    let rest = s[idx + prefix.len()..].trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        let end_q = stripped.find('"')?;
        Some(stripped[..end_q].to_string())
    } else {
        let end_p = rest.find(')')?;
        Some(rest[..end_p].trim().to_string())
    }
}

fn extract_text_value(block: &str) -> Option<(String, bool)> {
    if is_token_start(block, "(property") {
        let mut quotes = Vec::new();
        let mut in_q = false;
        let mut start = 0;
        for (i, ch) in block.char_indices() {
            if ch == '"' {
                if in_q {
                    quotes.push(&block[start..i]);
                    in_q = false;
                    if quotes.len() >= 2 {
                        break;
                    }
                } else {
                    in_q = true;
                    start = i + 1;
                }
            }
        }
        if quotes.len() >= 2 {
            let prop_name = quotes[0];
            let prop_val = quotes[1];
            if prop_name == "Reference" {
                return Some((prop_val.to_string(), true));
            } else if prop_name == "Value" {
                return Some((prop_val.to_string(), false));
            }
        }
        None
    } else if is_token_start(block, "(fp_text") {
        let is_ref = block.contains("(fp_text reference");
        let is_val = block.contains("(fp_text value");
        if !is_ref && !is_val {
            return None;
        }
        let first_q = block.find('"')?;
        let rest = &block[first_q + 1..];
        let end_q = rest.find('"')?;
        Some((rest[..end_q].to_string(), is_ref))
    } else if is_token_start(block, "(gr_text") {
        let first_q = block.find('"')?;
        let rest = &block[first_q + 1..];
        let end_q = rest.find('"')?;
        Some((rest[..end_q].to_string(), false))
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_arc_segments(
    instances: &mut Vec<f32>,
    primitive_count: &mut usize,
    sx: f32,
    sy: f32,
    mx: f32,
    my: f32,
    ex: f32,
    ey: f32,
    width: f32,
    layer_mask: f32,
) {
    let d = 2.0 * (sx * (my - ey) + mx * (ey - sy) + ex * (sy - my));
    if d.abs() < 1e-4 {
        push_raw(instances, primitive_count, sx, sy, mx, my, width, width, layer_mask, 0.0);
        push_raw(instances, primitive_count, mx, my, ex, ey, width, width, layer_mask, 0.0);
        return;
    }
    let s2 = sx * sx + sy * sy;
    let m2 = mx * mx + my * my;
    let e2 = ex * ex + ey * ey;
    let cx = (s2 * (my - ey) + m2 * (ey - sy) + e2 * (sy - my)) / d;
    let cy = (s2 * (ex - mx) + m2 * (sx - ex) + e2 * (mx - sx)) / d;
    let r = ((sx - cx).powi(2) + (sy - cy).powi(2)).sqrt();

    let a_start = (sy - cy).atan2(sx - cx);
    let a_mid = (my - cy).atan2(mx - cx);
    let a_end = (ey - cy).atan2(ex - cx);

    let tau = std::f32::consts::TAU;
    let ccw_mid = (a_mid - a_start).rem_euclid(tau);
    let ccw_end = (a_end - a_start).rem_euclid(tau);
    let sweep = if ccw_mid <= ccw_end {
        ccw_end
    } else {
        ccw_end - tau
    };

    let steps = 8;
    let mut prev_x = sx;
    let mut prev_y = sy;
    for i in 1..=steps {
        let t = (i as f32) / (steps as f32);
        let ang = a_start + sweep * t;
        let nx = cx + r * ang.cos();
        let ny = cy + r * ang.sin();
        push_raw(
            instances,
            primitive_count,
            prev_x,
            prev_y,
            nx,
            ny,
            width,
            width,
            layer_mask,
            0.0,
        );
        prev_x = nx;
        prev_y = ny;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn push_raw(
    instances: &mut Vec<f32>,
    primitive_count: &mut usize,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    w: f32,
    h: f32,
    layer_mask: f32,
    prim_type: f32,
) {
    instances.extend_from_slice(&[x1, y1, x2, y2, w, h, layer_mask, prim_type]);
    *primitive_count += 1;
}

fn count_parens(s: &str) -> i32 {
    let mut delta = 0i32;
    let mut in_quotes = false;
    for b in s.bytes() {
        if b == b'"' {
            in_quotes = !in_quotes;
        } else if !in_quotes {
            if b == b'(' {
                delta += 1;
            } else if b == b')' {
                delta -= 1;
            }
        }
    }
    delta
}

fn transform_point(lx: f32, ly: f32, fx: f32, fy: f32, rot_rad: f32) -> (f32, f32) {
    let (s, c) = rot_rad.sin_cos();
    let rx = lx * c - ly * s;
    let ry = lx * s + ly * c;
    (fx + rx, fy + ry)
}

fn parse_at_triple(s: &str) -> Option<(f32, f32, f32)> {
    let idx = s.find("(at ")?;
    let rest = &s[idx + 4..];
    let end = rest.find(')')?;
    let inner = &rest[..end];
    let mut parts = inner.split_whitespace();
    let x: f32 = parts.next()?.parse().ok()?;
    let y: f32 = parts.next()?.parse().ok()?;
    let rot: f32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    Some((x, y, rot))
}

fn extract_pair(s: &str, prefix: &str) -> Option<(f32, f32)> {
    let idx = s.find(prefix)?;
    let rest = &s[idx + prefix.len()..];
    let end = rest.find(')')?;
    let inner = &rest[..end];
    let mut parts = inner.split_whitespace();
    let x: f32 = parts.next()?.parse().ok()?;
    let y: f32 = parts.next()?.parse().ok()?;
    Some((x, y))
}

fn extract_single(s: &str, prefix: &str) -> Option<f32> {
    let idx = s.find(prefix)?;
    let rest = &s[idx + prefix.len()..];
    let end = rest.find(')')?;
    let inner = rest[..end].trim();
    inner.split_whitespace().next()?.parse().ok()
}

fn extract_net_def(s: &str) -> Option<(u32, Option<String>)> {
    let idx = s.find("(net ")?;
    let rest = &s[idx + 5..];
    let end = rest.find(')')?;
    let inner = rest[..end].trim();
    let mut parts = inner.splitn(2, char::is_whitespace);
    let id: u32 = parts.next()?.parse().ok()?;
    let name = parts
        .next()
        .map(|n| n.trim().trim_matches('"').to_string())
        .filter(|n| !n.is_empty());
    Some((id, name))
}

pub fn extract_layer_mask(s: &str) -> u32 {
    let mut mask = 0u32;
    if s.contains("\"*.Cu\"") {
        mask |= LAYER_F_CU | LAYER_B_CU | LAYER_IN1_CU | LAYER_IN2_CU;
    }
    if s.contains("\"F.Cu\"") {
        mask |= LAYER_F_CU;
    }
    if s.contains("\"B.Cu\"") {
        mask |= LAYER_B_CU;
    }
    if s.contains("\"In1.Cu\"") {
        mask |= LAYER_IN1_CU;
    }
    if s.contains("\"In2.Cu\"")
        || s.contains("\"In3.Cu\"")
        || s.contains("\"In4.Cu\"")
        || s.contains("\"In5.Cu\"")
        || s.contains("\"In6.Cu\"")
    {
        mask |= LAYER_IN2_CU;
    }
    if s.contains("\"F.Paste\"") || s.contains("\"*.Paste\"") {
        mask |= LAYER_F_PASTE;
    }
    if s.contains("\"B.Paste\"") || s.contains("\"*.Paste\"") {
        mask |= LAYER_B_PASTE;
    }
    if s.contains("\"Edge.Cuts\"") {
        mask |= LAYER_EDGE_CUTS;
    }
    if s.contains("\"Margin\"") {
        mask |= LAYER_MARGIN;
    }
    if s.contains("\"F.SilkS\"") || s.contains("\"F.Silkscreen\"") {
        mask |= LAYER_F_SILKS;
    }
    if s.contains("\"B.SilkS\"") || s.contains("\"B.Silkscreen\"") {
        mask |= LAYER_B_SILKS;
    }
    if s.contains("\"*.Mask\"") {
        mask |= LAYER_F_MASK | LAYER_B_MASK;
    }
    if s.contains("\"F.Mask\"") {
        mask |= LAYER_F_MASK;
    }
    if s.contains("\"B.Mask\"") {
        mask |= LAYER_B_MASK;
    }
    if s.contains("\"F.CrtYd\"") || s.contains("\"F.Courtyard\"") {
        mask |= LAYER_F_CRTYD;
    }
    if s.contains("\"B.CrtYd\"") || s.contains("\"B.Courtyard\"") {
        mask |= LAYER_B_CRTYD;
    }
    if s.contains("\"F.Fab\"") {
        mask |= LAYER_F_FAB;
    }
    if s.contains("\"B.Fab\"") {
        mask |= LAYER_B_FAB;
    }
    if s.contains("\"Dwgs.User\"") || s.contains("\"User.Drawings\"") {
        mask |= LAYER_USER_DWGS;
    }
    if s.contains("\"Cmts.User\"") || s.contains("\"User.Comments\"") {
        mask |= LAYER_USER_CMTS;
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_spatial_and_net_length() {
        let sample_pcb = r#"
        (kicad_pcb
          (net 11 "GND")
          (segment (start 100.0 80.0) (end 120.0 80.0) (width 0.25) (layer "F.Cu") (net 11))
          (segment (start 120.0 80.0) (end 120.0 95.0) (width 0.20) (layer "In1.Cu") (net 11))
          (via (at 120.0 80.0) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 11))
        )
        "#;
        let board = parse_kicad_pcb_str(sample_pcb);
        assert_eq!(board.primitive_count, 3);
        let gnd = board.nets.get(&11).expect("Net 11 GND should exist");
        assert_eq!(gnd.name, "GND");
        assert_eq!(gnd.segment_count, 2);
        assert_eq!(gnd.via_count, 1);
        assert!((gnd.total_length_mm - 35.0).abs() < 0.01);

        let hit = board
            .query_nearest(105.0, 80.05, LAYER_F_CU, 1.5)
            .expect("Should hit F.Cu track");
        assert_eq!(hit.kind, "Track");
        assert_eq!(hit.net_id, 11);
        assert_eq!(hit.net_name, "GND");
    }

    #[test]
    fn test_vector_diff_engine() {
        let pcb_a = r#"(kicad_pcb
            (segment (start 10 10) (end 20 10) (width 0.25) (layer "F.Cu"))
        )"#;
        let pcb_b = r#"(kicad_pcb
            (segment (start 10 10) (end 20 10) (width 0.25) (layer "F.Cu"))
            (segment (start 10 15) (end 25 15) (width 0.40) (layer "F.Cu"))
        )"#;
        let ba = parse_kicad_pcb_str(pcb_a);
        let bb = parse_kicad_pcb_str(pcb_b);
        let (_diff, added, removed, unchanged) = compute_vector_diff_boards(&ba, &bb);
        assert_eq!(added, 1);
        assert_eq!(removed, 0);
        assert_eq!(unchanged, 1);
    }
}
