use crate::parser::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DrcViolation {
    pub id: usize,
    pub rule_type: String, // "Clearance" | "Short" | "TrackWidth" | "Unconnected" | "DrillHole"
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

/// Calculates minimum distance between point (px, py) and segment (x0, y0)-(x1, y1)
pub fn dist_point_to_segment(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> (f32, f32, f32) {
    let vx = x1 - x0;
    let vy = y1 - y0;
    let len_sq = vx * vx + vy * vy;
    if len_sq < 1e-8 {
        let dx = px - x0;
        let dy = py - y0;
        return ((dx * dx + dy * dy).sqrt(), x0, y0);
    }
    let t = (((px - x0) * vx + (py - y0) * vy) / len_sq).clamp(0.0, 1.0);
    let qx = x0 + t * vx;
    let qy = y0 + t * vy;
    let dx = px - qx;
    let dy = py - qy;
    ((dx * dx + dy * dy).sqrt(), qx, qy)
}

/// Computes the exact minimum Euclidean distance between two 2D line segments,
/// returning (min_distance, closest_pt_on_s1, closest_pt_on_s2).
pub fn dist_segment_to_segment(
    x0: f32, y0: f32, x1: f32, y1: f32,
    x2: f32, y2: f32, x3: f32, y3: f32,
) -> (f32, (f32, f32), (f32, f32)) {
    let ux = x1 - x0;
    let uy = y1 - y0;
    let vx = x3 - x2;
    let vy = y3 - y2;
    let wx = x0 - x2;
    let wy = y0 - y2;

    let a = ux * ux + uy * uy; // |u|^2
    let b = ux * vx + uy * vy; // u . v
    let c = vx * vx + vy * vy; // |v|^2
    let d = ux * wx + uy * wy; // u . w
    let e = vx * wx + vy * wy; // v . w

    let eps = 1e-7;

    if a < eps && c < eps {
        let dx = x0 - x2;
        let dy = y0 - y2;
        return ((dx * dx + dy * dy).sqrt(), (x0, y0), (x2, y2));
    }
    if a < eps {
        let (dist, qx, qy) = dist_point_to_segment(x0, y0, x2, y2, x3, y3);
        return (dist, (x0, y0), (qx, qy));
    }
    if c < eps {
        let (dist, px, py) = dist_point_to_segment(x2, y2, x0, y0, x1, y1);
        return (dist, (px, py), (x2, y2));
    }

    let denom = a * c - b * b;
    let (mut s, mut t);

    if denom < eps {
        s = 0.0;
        t = (e / c).clamp(0.0, 1.0);
    } else {
        s = ((b * e - c * d) / denom).clamp(0.0, 1.0);
        t = ((b * s + e) / c).clamp(0.0, 1.0);
    }

    if t <= 0.0 {
        t = 0.0;
        s = (-d / a).clamp(0.0, 1.0);
    } else if t >= 1.0 {
        t = 1.0;
        s = ((b - d) / a).clamp(0.0, 1.0);
    }

    if s <= 0.0 {
        s = 0.0;
        t = (e / c).clamp(0.0, 1.0);
    } else if s >= 1.0 {
        s = 1.0;
        t = ((b + e) / c).clamp(0.0, 1.0);
    }

    let px = x0 + s * ux;
    let py = y0 + s * uy;
    let qx = x2 + t * vx;
    let qy = y2 + t * vy;

    let dx = px - qx;
    let dy = py - qy;
    ((dx * dx + dy * dy).sqrt(), (px, py), (qx, qy))
}

/// Executes complete production-grade PCB Design Rule Check (DRC) over board primitives.
pub fn execute_drc(
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

    // 1. Min Track Width Rule Check
    for i in 0..board.primitive_count {
        let base = i * FLOATS_PER_INSTANCE;
        if base + FLOATS_PER_INSTANCE > board.instances.len() {
            break;
        }
        let chunk = &board.instances[base..base + FLOATS_PER_INSTANCE];
        let layer_bits = chunk[6] as u32;
        let is_copper = (layer_bits
            & (LAYER_F_CU | LAYER_B_CU | LAYER_IN1_CU | LAYER_IN2_CU))
            != 0;

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
                    details: format!(
                        "Track Net '{}' width {:.3} mm at ({:.2}, {:.2}) is narrower than design rule limit.",
                        net_name, width, mid_x, mid_y
                    ),
                    x: mid_x,
                    y: mid_y,
                    layer_name: if (layer_bits & LAYER_F_CU) != 0 {
                        "F.Cu".to_string()
                    } else {
                        "B.Cu".to_string()
                    },
                    layer_mask: layer_bits,
                    measured_val: width,
                    required_val: min_track_width_mm,
                });
                v_id += 1;
            }
        }
    }

    // 2. Copper Clearance & Electrical Shorts Check (via exact capsule geometry & spatial buckets)
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
                continue; // skip non-copper markers
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

        let cell_size = (min_clearance_mm * 4.0).max(2.5);
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();

        for (idx, &(_, x0, y0, x1, y1, w, h, _, _)) in layer_prims.iter().enumerate() {
            let pad_rad = (w.max(h) * 0.5) + min_clearance_mm;
            let min_gx = (((x0.min(x1) - pad_rad) / cell_size).floor() as i32).max(-1000);
            let max_gx = (((x0.max(x1) + pad_rad) / cell_size).floor() as i32).min(1000);
            let min_gy = (((y0.min(y1) - pad_rad) / cell_size).floor() as i32).max(-1000);
            let max_gy = (((y0.max(y1) + pad_rad) / cell_size).floor() as i32).min(1000);

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
                    let pair_key = if a_idx < b_idx {
                        (a_idx, b_idx)
                    } else {
                        (b_idx, a_idx)
                    };
                    if !checked_pairs.insert(pair_key) {
                        continue;
                    }
                    let b = &layer_prims[b_idx];

                    // Don't check between same non-zero net
                    if a.7 == b.7 && a.7 > 0 {
                        continue;
                    }

                    // Geometry skeletons:
                    // If prim_type == 0 (Track): (x0, y0) to (x1, y1), radius = w * 0.5
                    // If prim_type != 0 (Pad/Via): center at (x0, y0), radius = max(w, h) * 0.5
                    let (ax0, ay0, ax1, ay1, rad_a) = if a.8 == 0 {
                        (a.1, a.2, a.3, a.4, a.5 * 0.5)
                    } else {
                        (a.1, a.2, a.1, a.2, a.5.max(a.6) * 0.5)
                    };

                    let (bx0, by0, bx1, by1, rad_b) = if b.8 == 0 {
                        (b.1, b.2, b.3, b.4, b.5 * 0.5)
                    } else {
                        (b.1, b.2, b.1, b.2, b.5.max(b.6) * 0.5)
                    };

                    let (skeleton_dist, pt_a, pt_b) =
                        dist_segment_to_segment(ax0, ay0, ax1, ay1, bx0, by0, bx1, by1);

                    let edge_dist = (skeleton_dist - rad_a - rad_b).max(0.0);

                    let net_a_name = if a.7 == 0 {
                        "Unassigned/Net0".to_string()
                    } else {
                        board
                            .nets
                            .get(&a.7)
                            .map(|n| n.name.clone())
                            .unwrap_or_else(|| format!("Net{}", a.7))
                    };
                    let net_b_name = if b.7 == 0 {
                        "Unassigned/Net0".to_string()
                    } else {
                        board
                            .nets
                            .get(&b.7)
                            .map(|n| n.name.clone())
                            .unwrap_or_else(|| format!("Net{}", b.7))
                    };

                    let mid_x = (pt_a.0 + pt_b.0) * 0.5;
                    let mid_y = (pt_a.1 + pt_b.1) * 0.5;

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

                    if violations.len() >= 5000 {
                        break;
                    }
                }
                if violations.len() >= 5000 {
                    break;
                }
            }
            if violations.len() >= 5000 {
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

    // 4. Drill Hole Clearance Check (Sorted sweep-line O(N log N) without artificial caps)
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

    holes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    for i in 0..holes.len() {
        let h1 = holes[i];
        for j in (i + 1)..holes.len() {
            let h2 = holes[j];
            let dx = h2.0 - h1.0;
            // Early prune once dx exceeds maximum possible collision radius + clearance
            if dx > min_drill_clearance_mm + 5.0 {
                break;
            }

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
                if violations.len() >= 5000 {
                    break;
                }
            }
        }
        if violations.len() >= 5000 {
            break;
        }
    }

    violations
}

/// Alias for execute_drc for backward compatibility
pub fn run_full_drc(
    board: &ParsedBoard,
    min_clearance_mm: f32,
    min_track_width_mm: f32,
    min_drill_clearance_mm: f32,
) -> Vec<DrcViolation> {
    execute_drc(board, min_clearance_mm, min_track_width_mm, min_drill_clearance_mm)
}

/// Fast pre-flight online DRC verification for a single candidate track segment using exact capsule distance.
pub fn validate_track_segment(
    board: &ParsedBoard,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    width: f32,
    layer_mask: u32,
    net_id: u32,
    min_clearance_mm: f32,
) -> Result<(), DrcViolation> {
    let copper_mask = layer_mask & (LAYER_F_CU | LAYER_B_CU | LAYER_IN1_CU | LAYER_IN2_CU);
    if copper_mask == 0 {
        return Ok(());
    }

    let safe_dist = width * 0.5 + min_clearance_mm;
    let min_x = x0.min(x1) - safe_dist - 0.5;
    let max_x = x0.max(x1) + safe_dist + 0.5;
    let min_y = y0.min(y1) - safe_dist - 0.5;
    let max_y = y0.max(y1) + safe_dist + 0.5;

    let min_cx = (min_x / SPATIAL_CELL_MM).floor() as i32;
    let max_cx = (max_x / SPATIAL_CELL_MM).floor() as i32;
    let min_cy = (min_y / SPATIAL_CELL_MM).floor() as i32;
    let max_cy = (max_y / SPATIAL_CELL_MM).floor() as i32;

    for cx in min_cx..=max_cx {
        for cy in min_cy..=max_cy {
            if let Some(indices) = board.spatial_grid.get(&(cx, cy)) {
                for &idx_u32 in indices {
                    let idx = idx_u32 as usize;
                    let base = idx * FLOATS_PER_INSTANCE;
                    if base + FLOATS_PER_INSTANCE > board.instances.len() {
                        continue;
                    }
                    let chunk = &board.instances[base..base + FLOATS_PER_INSTANCE];
                    let prim_layer = chunk[6] as u32;
                    if (prim_layer & copper_mask) == 0 {
                        continue;
                    }
                    let raw_w = chunk[7];
                    let prim_net = (raw_w / 16.0).floor() as u32;
                    if prim_net == net_id && net_id > 0 {
                        continue; // same electrical net
                    }

                    let prim_type = (raw_w - (prim_net as f32) * 16.0).round() as i32;
                    let ox0 = chunk[0];
                    let oy0 = chunk[1];
                    let (ox1, oy1, other_rad) = if prim_type == 0 {
                        (chunk[2], chunk[3], chunk[4] * 0.5)
                    } else {
                        (ox0, oy0, chunk[4].max(chunk[5]) * 0.5)
                    };

                    let (d, _, (ocx, ocy)) =
                        dist_segment_to_segment(x0, y0, x1, y1, ox0, oy0, ox1, oy1);

                    let edge_clearance = (d - other_rad - (width * 0.5)).max(0.0);
                    if edge_clearance < 0.02 {
                        let other_net_name = board
                            .nets
                            .get(&prim_net)
                            .map(|n| n.name.as_str())
                            .unwrap_or("Net-?");
                        return Err(DrcViolation {
                            id: 0,
                            rule_type: "Short".to_string(),
                            severity: "error".to_string(),
                            title: format!("Electrical Short with '{}'", other_net_name),
                            details: format!(
                                "Track segment touches copper of Net '{}' at ({:.2}, {:.2}).",
                                other_net_name, ocx, ocy
                            ),
                            x: ocx,
                            y: ocy,
                            layer_name: "Copper".to_string(),
                            layer_mask: copper_mask,
                            measured_val: 0.0,
                            required_val: min_clearance_mm,
                        });
                    } else if edge_clearance < min_clearance_mm {
                        let other_net_name = board
                            .nets
                            .get(&prim_net)
                            .map(|n| n.name.as_str())
                            .unwrap_or("Net-?");
                        return Err(DrcViolation {
                            id: 0,
                            rule_type: "Clearance".to_string(),
                            severity: "error".to_string(),
                            title: format!(
                                "Clearance violation ({:.3} mm < {:.3} mm)",
                                edge_clearance, min_clearance_mm
                            ),
                            details: format!(
                                "Track segment is too close to Net '{}' at ({:.2}, {:.2}).",
                                other_net_name, ocx, ocy
                            ),
                            x: ocx,
                            y: ocy,
                            layer_name: "Copper".to_string(),
                            layer_mask: copper_mask,
                            measured_val: edge_clearance,
                            required_val: min_clearance_mm,
                        });
                    }
                }
            }
        }
    }

    Ok(())
}

/// Fast pre-flight verification for placing a via at (x, y).
pub fn validate_via_drop(
    board: &ParsedBoard,
    x: f32,
    y: f32,
    size: f32,
    net_id: u32,
    min_clearance_mm: f32,
) -> Result<(), DrcViolation> {
    let all_copper = LAYER_F_CU | LAYER_B_CU | LAYER_IN1_CU | LAYER_IN2_CU;
    validate_track_segment(board, x, y, x, y, size, all_copper, net_id, min_clearance_mm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segment_to_segment_crossing() {
        // Horizontal segment from (0, 10) to (100, 10)
        // Vertical segment from (20, 0) to (20, 30)
        let (dist, p, q) = dist_segment_to_segment(0.0, 10.0, 100.0, 10.0, 20.0, 0.0, 20.0, 30.0);
        assert!(dist < 1e-5, "Crossing segments must have 0 distance, got {}", dist);
        assert!((p.0 - 20.0).abs() < 1e-4 && (p.1 - 10.0).abs() < 1e-4);
        assert!((q.0 - 20.0).abs() < 1e-4 && (q.1 - 10.0).abs() < 1e-4);
    }

    #[test]
    fn test_segment_to_segment_parallel() {
        let (dist, _, _) = dist_segment_to_segment(0.0, 0.0, 10.0, 0.0, 0.0, 2.0, 10.0, 2.0);
        assert!((dist - 2.0).abs() < 1e-5, "Parallel segments at y=0 and y=2 must have distance 2.0, got {}", dist);
    }

    #[test]
    fn test_drc_catches_crossing_tracks_of_different_nets() {
        let mut board = ParsedBoard::empty();
        let mut instances = Vec::new();
        let mut prim_count = 0;

        // Track 1 (Net 1): (0, 10) to (100, 10), width 0.2
        push_raw(
            &mut instances,
            &mut prim_count,
            0.0,
            10.0,
            100.0,
            10.0,
            0.2,
            0.2,
            LAYER_F_CU as f32,
            16.0 * 1.0 + 0.0, // net 1, track
        );

        // Track 2 (Net 2): (20, 0) to (20, 30), width 0.2
        // Note: Track 1 midpoint is (50, 10), Track 2 midpoint is (20, 15).
        // Distance between midpoints is sqrt(30^2 + 5^2) = 30.4mm!
        // The old midpoint DRC engine completely missed this collision.
        push_raw(
            &mut instances,
            &mut prim_count,
            20.0,
            0.0,
            20.0,
            30.0,
            0.2,
            0.2,
            LAYER_F_CU as f32,
            16.0 * 2.0 + 0.0, // net 2, track
        );

        board.instances = instances;
        board.primitive_count = prim_count;

        let mut nets = HashMap::new();
        nets.insert(1, NetStats { id: 1, name: "Net1".to_string(), pad_count: 2, segment_count: 1, via_count: 0, total_length_mm: 100.0 });
        nets.insert(2, NetStats { id: 2, name: "Net2".to_string(), pad_count: 2, segment_count: 1, via_count: 0, total_length_mm: 30.0 });
        board.nets = nets;

        let violations = execute_drc(&board, 0.15, 0.15, 0.25);
        let short = violations.iter().find(|v| v.rule_type == "Short");
        assert!(short.is_some(), "Crossing tracks of different nets MUST trigger Short violation!");
        let v = short.unwrap();
        assert_eq!(v.measured_val, 0.0);
    }
}
