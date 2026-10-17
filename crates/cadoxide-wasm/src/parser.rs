use std::collections::{HashMap, HashSet};

pub const LAYER_F_CU: u32 = 1 << 0;
pub const LAYER_B_CU: u32 = 1 << 1;
pub const LAYER_EDGE_CUTS: u32 = 1 << 2;
pub const LAYER_F_SILKS: u32 = 1 << 3;
pub const LAYER_B_SILKS: u32 = 1 << 4;
pub const LAYER_F_MASK: u32 = 1 << 5;
pub const LAYER_B_MASK: u32 = 1 << 6;
/// Special layer bits for GPU-instanced Review Pins & DRC Markers
pub const LAYER_DRC_MARKER: u32 = 1 << 7;
pub const LAYER_REVIEW_PIN: u32 = 1 << 8;
/// Special layer bits for GPU Vector Diff Engine:
/// Bit 9 = Unchanged copper, Bit 10 = Removed in Rev B (Red), Bit 11 = Added in Rev B (Green)
pub const LAYER_DIFF_UNCHANGED: u32 = 1 << 9;
pub const LAYER_DIFF_REMOVED: u32 = 1 << 10;
pub const LAYER_DIFF_ADDED: u32 = 1 << 11;
pub const LAYER_IN_CU: u32 = 1 << 12;

pub const FLOATS_PER_INSTANCE: usize = 8;
const SPATIAL_CELL_MM: f32 = 2.5;

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
pub struct HoveredPrimitiveInfo {
    pub instance_idx: usize,
    pub kind: &'static str,
    pub layer_name: &'static str,
    pub net_id: u32,
    pub net_name: String,
    pub summary: String,
    pub center_x: f32,
    pub center_y: f32,
}

#[derive(Clone, Debug)]
pub struct ParsedBoard {
    pub instances: Vec<f32>,
    pub primitive_count: usize,
    pub bounds: [f32; 4], // [min_x, min_y, max_x, max_y]
    pub spatial_grid: HashMap<(i32, i32), Vec<u32>>,
    pub nets: HashMap<u32, NetStats>,
}

impl ParsedBoard {
    pub fn with_spatial_grid(
        instances: Vec<f32>,
        primitive_count: usize,
        bounds: [f32; 4],
        nets: HashMap<u32, NetStats>,
    ) -> Self {
        let mut spatial_grid: HashMap<(i32, i32), Vec<u32>> =
            HashMap::with_capacity((primitive_count / 4).max(64));

        for (idx, chunk) in instances.chunks_exact(FLOATS_PER_INSTANCE).enumerate() {
            let x0 = chunk[0];
            let y0 = chunk[1];
            let x1 = chunk[2];
            let y1 = chunk[3];
            let r = chunk[4].max(chunk[5]).max(0.25);

            let min_cx = ((x0.min(x1) - r) / SPATIAL_CELL_MM).floor() as i32;
            let max_cx = ((x0.max(x1) + r) / SPATIAL_CELL_MM).floor() as i32;
            let min_cy = ((y0.min(y1) - r) / SPATIAL_CELL_MM).floor() as i32;
            let max_cy = ((y0.max(y1) + r) / SPATIAL_CELL_MM).floor() as i32;

            // Clamp span for very long board-outline segments
            let span_x = (max_cx - min_cx).min(24);
            let span_y = (max_cy - min_cy).min(24);

            for cx in min_cx..=(min_cx + span_x) {
                for cy in min_cy..=(min_cy + span_y) {
                    spatial_grid.entry((cx, cy)).or_default().push(idx as u32);
                }
            }
        }

        Self {
            instances,
            primitive_count,
            bounds,
            spatial_grid,
            nets,
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

                        let x0 = chunk[0];
                        let y0 = chunk[1];
                        let x1 = chunk[2];
                        let y1 = chunk[3];
                        let r = (chunk[4] * 0.5).max(0.10);
                        let raw_w = chunk[7];
                        let net_id = (raw_w / 16.0).floor() as u32;
                        let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;

                        let dist = if prim_type == 0 {
                            // Point-to-segment distance for capsule tracks
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

        let layer_name = if (layer_bits & LAYER_F_CU) != 0 && (layer_bits & LAYER_B_CU) != 0 {
            "Multi/PTH"
        } else if (layer_bits & LAYER_F_CU) != 0 {
            "F.Cu"
        } else if (layer_bits & LAYER_IN_CU) != 0 {
            "In1..4.Cu"
        } else if (layer_bits & LAYER_B_CU) != 0 {
            "B.Cu"
        } else if (layer_bits & LAYER_EDGE_CUTS) != 0 {
            "Edge.Cuts"
        } else if (layer_bits & LAYER_F_SILKS) != 0 {
            "F.SilkS"
        } else if (layer_bits & LAYER_B_SILKS) != 0 {
            "B.SilkS"
        } else if (layer_bits & LAYER_F_MASK) != 0 {
            "F.Mask"
        } else if (layer_bits & LAYER_B_MASK) != 0 {
            "B.Mask"
        } else {
            "Other"
        };

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
            layer_name,
            net_id,
            net_name,
            summary,
            center_x: (x0 + x1) * 0.5,
            center_y: (y0 + y1) * 0.5,
        })
    }
}

/// Parses a KiCad `.kicad_pcb` file in pure Rust into a flat 8-float-per-instance GPU buffer.
pub fn parse_kicad_pcb_str(pcb_source: &str) -> ParsedBoard {
    let mut instances = Vec::with_capacity(8192 * FLOATS_PER_INSTANCE);
    let mut primitive_count = 0;
    let mut nets: HashMap<u32, NetStats> = HashMap::new();

    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;

    let mut in_footprint = false;
    let mut fp_x: f32 = 0.0;
    let mut fp_y: f32 = 0.0;
    let mut fp_rot_rad: f32 = 0.0;
    let mut fp_depth: i32 = 0;

    let mut active_block = String::with_capacity(512);
    let mut block_depth: i32 = 0;
    let mut collecting_block = false;

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
                process_primitive_block(
                    &active_block,
                    in_footprint,
                    fp_x,
                    fp_y,
                    fp_rot_rad,
                    &mut instances,
                    &mut primitive_count,
                    &mut nets,
                    &mut min_x,
                    &mut min_y,
                    &mut max_x,
                    &mut max_y,
                );
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
            || (in_footprint && is_token_start(line, "(pad"))
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
            }
        }
    }

    let bounds = if min_x < max_x && min_y < max_y {
        [min_x - 2.0, min_y - 2.0, max_x + 2.0, max_y + 2.0]
    } else {
        [70.0, 40.0, 190.0, 150.0]
    };

    ParsedBoard::with_spatial_grid(instances, primitive_count, bounds, nets)
}

/// Computes a pure-Rust geometric vector diff between Revision A and Revision B!
/// Returns (diff_board, added_count, removed_count, unchanged_count).
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

    // 1. Add unchanged & removed primitives from Board A
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

    // 2. Add newly added primitives from Board B
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
            if layer_mask != 0 {
                update_bounds(wx, wy);
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
                }
                let (out_w, out_h, pad_prim) = if drill > 0.01 {
                    if is_circle_or_oval {
                        (pw.max(ph), drill, 1.0) // Circular/Oval Plated Through-Hole Pad
                    } else {
                        (pw.max(ph), drill, 4.0) // Square/Rect Plated Through-Hole Pad (Pin 1)
                    }
                } else if is_circle_or_oval {
                    (pw, ph, 3.0) // Circular/Oval SMD Pad
                } else {
                    (pw, ph, 2.0) // Rectangular/Roundrect SMD Pad
                };
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
    // Compute circumcenter of 3 points (sx,sy), (mx,my), (ex,ey)
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

fn extract_layer_mask(s: &str) -> u32 {
    let mut mask = 0u32;
    if s.contains("\"*.Cu\"") {
        mask |= LAYER_F_CU | LAYER_B_CU | LAYER_IN_CU;
    }
    if s.contains("\"F.Cu\"") {
        mask |= LAYER_F_CU;
    }
    if s.contains("\"B.Cu\"") {
        mask |= LAYER_B_CU;
    }
    if s.contains("\"In1.Cu\"")
        || s.contains("\"In2.Cu\"")
        || s.contains("\"In3.Cu\"")
        || s.contains("\"In4.Cu\"")
    {
        mask |= LAYER_IN_CU;
    }
    if s.contains("\"Edge.Cuts\"") {
        mask |= LAYER_EDGE_CUTS;
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
