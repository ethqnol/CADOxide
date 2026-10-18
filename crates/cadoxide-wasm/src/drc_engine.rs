use crate::parser::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct DrcViolation {
    pub id: usize,
    pub rule_type: String, // "Clearance", "Short", "Unconnected", "TrackWidth", "DrillHole"
    pub severity: String,  // "error" | "warning"
    pub title: String,
    pub details: String,
    pub x: f32,
    pub y: f32,
    pub layer_name: String,
    pub layer_mask: u32,
    pub measured_val: f32,
    pub required_val: f32,
}

pub fn run_full_drc(
    board: &ParsedBoard,
    min_clearance_mm: f32,
    min_track_width_mm: f32,
    min_drill_clearance_mm: f32,
) -> Vec<DrcViolation> {
    let mut violations = Vec::new();
    let mut v_id = 1;

    let copper_layers = [
        (LAYER_F_CU, "F.Cu"),
        (LAYER_B_CU, "B.Cu"),
        (LAYER_IN1_CU, "In1.Cu"),
        (LAYER_IN2_CU, "In2.Cu"),
    ];

    // 1. Minimum Track Width Check
    for i in 0..board.primitive_count {
        let base = i * FLOATS_PER_INSTANCE;
        if base + FLOATS_PER_INSTANCE > board.instances.len() {
            break;
        }
        let chunk = &board.instances[base..base + FLOATS_PER_INSTANCE];
        let layer_bits = chunk[6] as u32;
        let is_copper = (layer_bits & (LAYER_F_CU | LAYER_B_CU | LAYER_IN1_CU | LAYER_IN2_CU)) != 0;
        let raw_w = chunk[7];
        let net_id = (raw_w / 16.0).floor() as u32;
        let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;

        if is_copper && prim_type == 0 {
            let width = chunk[4];
            if width > 0.01 && width < min_track_width_mm - 1e-4 {
                let mid_x = (chunk[0] + chunk[2]) * 0.5;
                let mid_y = (chunk[1] + chunk[3]) * 0.5;
                let net_name = board
                    .nets
                    .get(&net_id)
                    .map(|n| n.name.as_str())
                    .unwrap_or("Unknown");
                violations.push(DrcViolation {
                    id: v_id,
                    rule_type: "TrackWidth".to_string(),
                    severity: "warning".to_string(),
                    title: format!(
                        "Track width ({:.3} mm) < limit ({:.3} mm)",
                        width, min_track_width_mm
                    ),
                    details: format!("Track Net '{}' width {:.3} mm at ({:.2}, {:.2}) is narrower than design rule limit.", net_name, width, mid_x, mid_y),
                    x: mid_x,
                    y: mid_y,
                    layer_name: if (layer_bits & LAYER_F_CU) != 0 { "F.Cu".to_string() } else { "B.Cu".to_string() },
                    layer_mask: layer_bits,
                    measured_val: width,
                    required_val: min_track_width_mm,
                });
                v_id += 1;
            }
        }
    }

    // 2. Copper Clearance & Electrical Shorts Check (via spatial buckets)
    for &(layer_mask, layer_name) in &copper_layers {
        let mut layer_prims: Vec<(usize, f32, f32, f32, f32, f32, f32, u32, i32)> = Vec::new();

        for i in 0..board.primitive_count {
            let base = i * FLOATS_PER_INSTANCE;
            if base + FLOATS_PER_INSTANCE > board.instances.len() {
                break;
            }
            let chunk = &board.instances[base..base + FLOATS_PER_INSTANCE];
            let layer_bits = chunk[6] as u32;
            if (layer_bits & layer_mask) == 0 {
                continue;
            }
            let raw_w = chunk[7];
            let net_id = (raw_w / 16.0).floor() as u32;
            let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;
            if prim_type > 5 {
                continue; // skip markers
            }
            layer_prims.push((
                i,
                chunk[0],
                chunk[1],
                chunk[2],
                chunk[3],
                chunk[4],
                chunk[5],
                net_id,
                prim_type,
            ));
        }

        let cell_size = (min_clearance_mm * 3.5).max(1.5);
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();

        for (idx, &(_, x0, y0, x1, y1, w, h, _, _)) in layer_prims.iter().enumerate() {
            let pad_rad = (w.max(h) * 0.5) + min_clearance_mm;
            let min_gx = (((x0.min(x1) - pad_rad) / cell_size).floor() as i32).max(-500);
            let max_gx = (((x0.max(x1) + pad_rad) / cell_size).floor() as i32).min(500);
            let min_gy = (((y0.min(y1) - pad_rad) / cell_size).floor() as i32).max(-500);
            let max_gy = (((y0.max(y1) + pad_rad) / cell_size).floor() as i32).min(500);

            for gx in min_gx..=max_gx {
                for gy in min_gy..=max_gy {
                    grid.entry((gx, gy)).or_default().push(idx);
                }
            }
        }

        let mut checked_pairs: HashSet<(usize, usize)> = HashSet::new();

        for (_cell, prim_indices) in &grid {
            if prim_indices.len() < 2 {
                continue;
            }
            for i in 0..prim_indices.len() {
                let a_idx = prim_indices[i];
                let a = &layer_prims[a_idx];
                for j in (i + 1)..prim_indices.len() {
                    let b_idx = prim_indices[j];
                    let pair_key = if a_idx < b_idx { (a_idx, b_idx) } else { (b_idx, a_idx) };
                    if !checked_pairs.insert(pair_key) {
                        continue;
                    }
                    let b = &layer_prims[b_idx];

                    // Only check different nets where both nets are non-zero
                    if a.7 == b.7 || a.7 == 0 || b.7 == 0 {
                        continue;
                    }

                    // Compute center-to-center or segment distance
                    let (ax, ay) = ((a.1 + a.3) * 0.5, (a.2 + a.4) * 0.5);
                    let (bx, by) = ((b.1 + b.3) * 0.5, (b.2 + b.4) * 0.5);
                    let dx = ax - bx;
                    let dy = ay - by;
                    let dist = (dx * dx + dy * dy).sqrt();

                    let rad_a = a.5.max(a.6) * 0.5;
                    let rad_b = b.5.max(b.6) * 0.5;
                    let edge_dist = (dist - rad_a - rad_b).max(0.0);

                    let net_a_name = board
                        .nets
                        .get(&a.7)
                        .map(|n| n.name.as_str())
                        .unwrap_or("NetA");
                    let net_b_name = board
                        .nets
                        .get(&b.7)
                        .map(|n| n.name.as_str())
                        .unwrap_or("NetB");

                    let mid_x = (ax + bx) * 0.5;
                    let mid_y = (ay + by) * 0.5;

                    if edge_dist < 0.02 {
                        // Electrical short
                        violations.push(DrcViolation {
                            id: v_id,
                            rule_type: "Short".to_string(),
                            severity: "error".to_string(),
                            title: format!("Electrical Short: '{}' and '{}'", net_a_name, net_b_name),
                            details: format!(
                                "Copper elements touch without clearance between Net '{}' and Net '{}' on {}.",
                                net_a_name, net_b_name, layer_name
                            ),
                            x: mid_x,
                            y: mid_y,
                            layer_name: layer_name.to_string(),
                            layer_mask,
                            measured_val: 0.0,
                            required_val: min_clearance_mm,
                        });
                        v_id += 1;
                    } else if edge_dist < min_clearance_mm {
                        // Clearance violation
                        violations.push(DrcViolation {
                            id: v_id,
                            rule_type: "Clearance".to_string(),
                            severity: "error".to_string(),
                            title: format!(
                                "Clearance violation ({:.3} mm < {:.3} mm)",
                                edge_dist, min_clearance_mm
                            ),
                            details: format!(
                                "Spacing between Net '{}' and Net '{}' on {} is {:.3} mm, less than required {:.3} mm.",
                                net_a_name, net_b_name, layer_name, edge_dist, min_clearance_mm
                            ),
                            x: mid_x,
                            y: mid_y,
                            layer_name: layer_name.to_string(),
                            layer_mask,
                            measured_val: edge_dist,
                            required_val: min_clearance_mm,
                        });
                        v_id += 1;
                    }

                    if violations.len() >= 120 {
                        break;
                    }
                }
                if violations.len() >= 120 {
                    break;
                }
            }
            if violations.len() >= 120 {
                break;
            }
        }
    }

    // 3. Unconnected Items (Disconnected Nets)
    let plane_nets: HashSet<u32> = board
        .zones
        .iter()
        .filter(|z| !z.is_keepout && !z.is_teardrop)
        .map(|z| z.net_id)
        .collect();

    for (&net_id, stats) in &board.nets {
        if net_id == 0 || plane_nets.contains(&net_id) {
            continue;
        }
        if stats.pad_count >= 2 {
            let needed_segments = stats.pad_count.saturating_sub(1);
            if stats.segment_count < needed_segments {
                // Find coordinates of first pad in this net for navigation
                let pad_coord = board
                    .pad_labels
                    .iter()
                    .find(|p| p.net_id == net_id)
                    .map(|p| (p.x, p.y))
                    .unwrap_or((0.0, 0.0));

                let unrouted = stats.pad_count.saturating_sub(stats.segment_count);
                violations.push(DrcViolation {
                    id: v_id,
                    rule_type: "Unconnected".to_string(),
                    severity: "warning".to_string(),
                    title: format!(
                        "Unconnected items: Net '{}' ({} unrouted connections)",
                        stats.name, unrouted
                    ),
                    details: format!(
                        "Net '{}' has {} pads but only {} track segments. Missing ratsnest connections remain.",
                        stats.name, stats.pad_count, stats.segment_count
                    ),
                    x: pad_coord.0,
                    y: pad_coord.1,
                    layer_name: "All Copper".to_string(),
                    layer_mask: LAYER_F_CU | LAYER_B_CU,
                    measured_val: stats.segment_count as f32,
                    required_val: needed_segments as f32,
                });
                v_id += 1;
            }
        }
    }

    // 4. Drill Hole Clearance Check
    let mut holes: Vec<(f32, f32, f32, usize)> = Vec::new();
    for i in 0..board.primitive_count {
        let base = i * FLOATS_PER_INSTANCE;
        if base + FLOATS_PER_INSTANCE > board.instances.len() {
            break;
        }
        let chunk = &board.instances[base..base + FLOATS_PER_INSTANCE];
        let raw_w = chunk[7];
        let net_id = (raw_w / 16.0).floor() as u32;
        let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;

        let drill_dia = if prim_type == 1 {
            chunk[5]
        } else if prim_type >= 2 && prim_type <= 5 && chunk[5] > 0.05 {
            chunk[5]
        } else {
            0.0
        };

        if drill_dia > 0.05 {
            holes.push((chunk[0], chunk[1], drill_dia, i));
        }
    }

    for i in 0..holes.len().min(400) {
        let h1 = holes[i];
        for j in (i + 1)..holes.len().min(400) {
            let h2 = holes[j];
            let dx = h1.0 - h2.0;
            let dy = h1.1 - h2.1;
            let center_dist = (dx * dx + dy * dy).sqrt();
            let edge_dist = center_dist - (h1.2 * 0.5) - (h2.2 * 0.5);

            if edge_dist < min_drill_clearance_mm && center_dist > 0.01 {
                violations.push(DrcViolation {
                    id: v_id,
                    rule_type: "DrillHole".to_string(),
                    severity: "error".to_string(),
                    title: format!(
                        "Hole-to-hole clearance ({:.3} mm < {:.3} mm)",
                        edge_dist, min_drill_clearance_mm
                    ),
                    details: format!(
                        "Drill holes at ({:.2}, {:.2}) and ({:.2}, {:.2}) are too close for PCB mechanical drilling.",
                        h1.0, h1.1, h2.0, h2.1
                    ),
                    x: (h1.0 + h2.0) * 0.5,
                    y: (h1.1 + h2.1) * 0.5,
                    layer_name: "Drill".to_string(),
                    layer_mask: LAYER_EDGE_CUTS,
                    measured_val: edge_dist,
                    required_val: min_drill_clearance_mm,
                });
                v_id += 1;
                if violations.len() >= 150 {
                    break;
                }
            }
        }
        if violations.len() >= 150 {
            break;
        }
    }

    violations
}
