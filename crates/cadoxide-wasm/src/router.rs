//! Real-time topological Push-and-Shove & Obstacle-Avoidance Router
//! Pure Rust implementation executing inside WebAssembly.

use crate::parser::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoutingMode {
    Walkaround,
    PushAndShove,
    Free45,
}

#[derive(Clone, Debug)]
pub struct Obstacle {
    pub instance_idx: usize,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub radius: f32,
    pub net_id: u32,
    pub is_round: bool,
    pub is_movable: bool,
}

impl Obstacle {
    pub fn center(&self) -> (f32, f32) {
        ((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }

    pub fn dist_to_point(&self, px: f32, py: f32) -> f32 {
        let (cx, cy) = self.center();
        let dx = px - cx;
        let dy = py - cy;
        (dx * dx + dy * dy).sqrt()
    }

    pub fn dist_to_segment(&self, sx: f32, sy: f32, ex: f32, ey: f32) -> f32 {
        let (cx, cy) = self.center();
        let l2 = (ex - sx).powi(2) + (ey - sy).powi(2);
        if l2 < 1e-6 {
            return ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
        }
        let t = (((cx - sx) * (ex - sx) + (cy - sy) * (ey - sy)) / l2).clamp(0.0, 1.0);
        let proj_x = sx + t * (ex - sx);
        let proj_y = sy + t * (ey - sy);
        ((cx - proj_x).powi(2) + (cy - proj_y).powi(2)).sqrt()
    }
}

pub struct TopologicalRouter {
    pub mode: RoutingMode,
    pub target_length_mm: Option<f32>,
}

impl TopologicalRouter {
    pub fn new() -> Self {
        Self {
            mode: RoutingMode::Walkaround,
            target_length_mm: None,
        }
    }

    /// Query obstacles on the board that conflict with a given net on a layer.
    pub fn query_obstacles_in_rect(
        board: &ParsedBoard,
        min_x: f32,
        min_y: f32,
        max_x: f32,
        max_y: f32,
        layer_mask: u32,
        net_id: u32,
        clearance_mm: f32,
    ) -> Vec<Obstacle> {
        let mut obstacles = Vec::new();
        let copper_mask = layer_mask & (LAYER_F_CU | LAYER_B_CU | LAYER_IN1_CU | LAYER_IN2_CU);
        if copper_mask == 0 {
            return obstacles;
        }

        let margin = clearance_mm + 2.0;
        let query_box = [min_x - margin, min_y - margin, max_x + margin, max_y + margin];

        let min_cx = (query_box[0] / SPATIAL_CELL_MM).floor() as i32;
        let max_cx = (query_box[2] / SPATIAL_CELL_MM).floor() as i32;
        let min_cy = (query_box[1] / SPATIAL_CELL_MM).floor() as i32;
        let max_cy = (query_box[3] / SPATIAL_CELL_MM).floor() as i32;

        let mut seen = std::collections::HashSet::new();

        for cx in min_cx..=max_cx {
            for cy in min_cy..=max_cy {
                if let Some(indices) = board.spatial_grid.get(&(cx, cy)) {
                    for &idx_u32 in indices {
                        let idx = idx_u32 as usize;
                        if !seen.insert(idx) {
                            continue;
                        }
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
                        let prim_type = (raw_w - (prim_net as f32) * 16.0).round() as i32;

                        // Only consider obstacles of DIFFERENT nets (or net 0)
                        if prim_net == net_id && net_id > 0 {
                            continue;
                        }

                        let x0 = chunk[0];
                        let y0 = chunk[1];
                        let x1 = chunk[2];
                        let y1 = chunk[3];
                        let w = chunk[4];
                        let h = chunk[5];
                        let rad = (w.max(h) * 0.5) + clearance_mm;
                        let is_movable = prim_type == 0 || prim_type == 1; // Tracks and vias can be shoved; pads and borders are fixed

                        obstacles.push(Obstacle {
                            instance_idx: idx,
                            x0,
                            y0,
                            x1,
                            y1,
                            radius: rad,
                            net_id: prim_net,
                            is_round: prim_type == 1 || prim_type == 2 || (x0 == x1 && y0 == y1),
                            is_movable,
                        });
                    }
                }
            }
        }

        obstacles
    }

    /// Compute an obstacle-avoiding walkaround route from start to end with 45-degree bends.
    pub fn compute_walkaround_route(
        &self,
        board: &ParsedBoard,
        sx: f32,
        sy: f32,
        ex: f32,
        ey: f32,
        track_width_mm: f32,
        layer_mask: u32,
        net_id: u32,
        clearance_mm: f32,
        posture_flip: bool,
    ) -> Vec<(f32, f32)> {
        let (kx, ky) = compute_45_knee_point(sx, sy, ex, ey, posture_flip);

        if self.mode == RoutingMode::Free45 {
            return vec![(sx, sy), (kx, ky), (ex, ey)];
        }

        let min_x = sx.min(ex).min(kx);
        let max_x = sx.max(ex).max(kx);
        let min_y = sy.min(ey).min(ky);
        let max_y = sy.max(ey).max(ky);

        let obstacles = Self::query_obstacles_in_rect(
            board,
            min_x,
            min_y,
            max_x,
            max_y,
            layer_mask,
            net_id,
            clearance_mm,
        );

        if obstacles.is_empty() {
            return vec![(sx, sy), (kx, ky), (ex, ey)];
        }

        // Test segments for collisions
        let segs = [((sx, sy), (kx, ky)), ((kx, ky), (ex, ey))];
        let mut path = vec![(sx, sy)];

        for (p0, p1) in segs {
            let mut colliding_obs: Vec<&Obstacle> = obstacles
                .iter()
                .filter(|obs| {
                    let d = obs.dist_to_segment(p0.0, p0.1, p1.0, p1.1);
                    d < (obs.radius + track_width_mm * 0.5)
                })
                .collect();

            if colliding_obs.is_empty() {
                path.push(p1);
                continue;
            }

            // Sort colliding obstacles along segment from p0 to p1
            colliding_obs.sort_by(|a, b| {
                let da = (a.center().0 - p0.0).powi(2) + (a.center().1 - p0.1).powi(2);
                let db = (b.center().0 - p0.0).powi(2) + (b.center().1 - p0.1).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });

            let mut curr = p0;
            for obs in colliding_obs {
                let (ox, oy) = obs.center();
                let effective_r = obs.radius + track_width_mm * 0.5;

                // Segment direction vector
                let sdx = p1.0 - curr.0;
                let sdy = p1.1 - curr.1;
                let slen = (sdx * sdx + sdy * sdy).sqrt().max(0.01);
                let norm_x = -sdy / slen;
                let norm_y = sdx / slen;

                // Pick hug direction (side with shortest detour or posture alignment)
                let side_sign = if posture_flip { -1.0 } else { 1.0 };
                let detour_apex = (
                    ox + norm_x * effective_r * side_sign,
                    oy + norm_y * effective_r * side_sign,
                );

                // Add 45-degree lead-in to detour apex
                let lead_in = compute_45_knee_point(curr.0, curr.1, detour_apex.0, detour_apex.1, posture_flip);
                if (lead_in.0 - curr.0).abs() > 0.05 || (lead_in.1 - curr.1).abs() > 0.05 {
                    path.push(lead_in);
                }
                path.push(detour_apex);
                curr = detour_apex;
            }

            let lead_out = compute_45_knee_point(curr.0, curr.1, p1.0, p1.1, !posture_flip);
            if (lead_out.0 - curr.0).abs() > 0.05 || (lead_out.1 - curr.1).abs() > 0.05 {
                path.push(lead_out);
            }
            path.push(p1);
        }

        simplify_polyline(&path)
    }

    /// Shove conflicting tracks away from the newly laid track segments.
    pub fn push_and_shove_obstacles(
        board: &mut ParsedBoard,
        new_segments: &[((f32, f32), (f32, f32))],
        track_width: f32,
        layer_mask: u32,
        net_id: u32,
        clearance_mm: f32,
    ) -> usize {
        let mut shoved_count = 0;
        let safe_dist = track_width * 0.5 + clearance_mm;

        for &(p0, p1) in new_segments {
            let min_x = p0.0.min(p1.0) - safe_dist - 2.0;
            let max_x = p0.0.max(p1.0) + safe_dist + 2.0;
            let min_y = p0.1.min(p1.1) - safe_dist - 2.0;
            let max_y = p0.1.max(p1.1) + safe_dist + 2.0;

            let obstacles = Self::query_obstacles_in_rect(
                board,
                min_x,
                min_y,
                max_x,
                max_y,
                layer_mask,
                net_id,
                clearance_mm,
            );

            let sdx = p1.0 - p0.0;
            let sdy = p1.1 - p0.1;
            let slen = (sdx * sdx + sdy * sdy).sqrt();
            if slen < 0.1 {
                continue;
            }
            let norm_x = -sdy / slen;
            let norm_y = sdx / slen;

            for obs in obstacles {
                if !obs.is_movable {
                    continue; // Fixed obstacles (pads, cutouts) cannot be shoved
                }
                let d = obs.dist_to_segment(p0.0, p0.1, p1.0, p1.1);
                let needed = safe_dist + obs.radius;
                if d < needed {
                    let push_amount = ((needed - d) + 0.05).min(2.5); // Safe clamp
                    // Check which side of the line the obstacle center is on
                    let (cx, cy) = obs.center();
                    let side = (cx - p0.0) * norm_x + (cy - p0.1) * norm_y;
                    let sign = if side >= 0.0 { 1.0 } else { -1.0 };

                    let shift_x = norm_x * push_amount * sign;
                    let shift_y = norm_y * push_amount * sign;

                    // Move obstacle in board instance buffer
                    if board.translate_primitive_in_place(obs.instance_idx, shift_x, shift_y) {
                        shoved_count += 1;

                        // Cascade shove: query secondary obstacles displaced by this obstacle
                        let sub_obs = Self::query_obstacles_in_rect(
                            board,
                            cx + shift_x - obs.radius,
                            cy + shift_y - obs.radius,
                            cx + shift_x + obs.radius,
                            cy + shift_y + obs.radius,
                            layer_mask,
                            obs.net_id,
                            clearance_mm,
                        );
                        for s_obs in sub_obs {
                            if s_obs.instance_idx != obs.instance_idx && s_obs.is_movable {
                                let s_dist = s_obs.dist_to_point(cx + shift_x, cy + shift_y);
                                let s_needed = obs.radius + s_obs.radius;
                                if s_dist < s_needed {
                                    let s_push = (s_needed - s_dist + 0.05).min(1.5);
                                    let _ = board.translate_primitive_in_place(
                                        s_obs.instance_idx,
                                        shift_x.signum() * s_push * 0.7,
                                        shift_y.signum() * s_push * 0.7,
                                    );
                                    shoved_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        if shoved_count > 0 {
            board.rebuild_index_and_nets();
        }

        shoved_count
    }

    /// Generate accordion / serpentine meanders to tune a track to an exact target length.
    pub fn build_length_tuned_meanders(
        sx: f32,
        sy: f32,
        ex: f32,
        ey: f32,
        track_width_mm: f32,
        target_length_mm: f32,
    ) -> Vec<(f32, f32)> {
        let dx = ex - sx;
        let dy = ey - sy;
        let straight_len = (dx * dx + dy * dy).sqrt();

        if straight_len < 1.0 || straight_len >= target_length_mm {
            return vec![(sx, sy), (ex, ey)];
        }

        let excess_needed = target_length_mm - straight_len;
        let ux = dx / straight_len;
        let uy = dy / straight_len;
        let nx = -uy;
        let ny = ux;

        let pitch = (track_width_mm * 3.5).max(0.7);
        let max_bumps = ((straight_len * 0.75) / pitch).floor() as usize;
        if max_bumps < 1 {
            return vec![(sx, sy), (ex, ey)];
        }

        let bumps = max_bumps.clamp(2, 28);
        // Excess per bump = 2 * amplitude
        let required_amp = (excess_needed / (bumps as f32 * 2.0)).clamp(0.4, 6.0);

        let mut pts = Vec::with_capacity(bumps * 4 + 4);
        pts.push((sx, sy));

        let margin_start = straight_len * 0.12;
        let meander_span = straight_len * 0.76;
        let step_len = meander_span / (bumps as f32);

        for i in 0..bumps {
            let base_dist = margin_start + (i as f32) * step_len;
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            let amp = required_amp * sign;

            let p_in = (
                sx + ux * (base_dist + step_len * 0.15),
                sy + uy * (base_dist + step_len * 0.15),
            );
            let p_peak1 = (
                sx + ux * (base_dist + step_len * 0.25) + nx * amp,
                sy + uy * (base_dist + step_len * 0.25) + ny * amp,
            );
            let p_peak2 = (
                sx + ux * (base_dist + step_len * 0.75) + nx * amp,
                sy + uy * (base_dist + step_len * 0.75) + ny * amp,
            );
            let p_out = (
                sx + ux * (base_dist + step_len * 0.85),
                sy + uy * (base_dist + step_len * 0.85),
            );

            pts.push(p_in);
            pts.push(p_peak1);
            pts.push(p_peak2);
            pts.push(p_out);
        }

        pts.push((ex, ey));
        pts
    }
}

pub fn compute_45_knee_point(sx: f32, sy: f32, ex: f32, ey: f32, flip: bool) -> (f32, f32) {
    let dx = ex - sx;
    let dy = ey - sy;
    let adx = dx.abs();
    let ady = dy.abs();
    if !flip {
        if adx > ady {
            (sx + dx.signum() * (adx - ady), sy)
        } else {
            (sx, sy + dy.signum() * (ady - adx))
        }
    } else if adx > ady {
        (sx + dx.signum() * ady, ey)
    } else {
        (ex, sy + dy.signum() * adx)
    }
}

fn simplify_polyline(pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut out = Vec::with_capacity(pts.len());
    out.push(pts[0]);
    for i in 1..pts.len() {
        let prev = *out.last().unwrap();
        let curr = pts[i];
        if (curr.0 - prev.0).abs() > 0.02 || (curr.1 - prev.1).abs() > 0.02 {
            out.push(curr);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_45_knee_point() {
        let (kx, ky) = compute_45_knee_point(0.0, 0.0, 10.0, 5.0, false);
        assert!((kx - 5.0).abs() < 1e-3);
        assert!((ky - 0.0).abs() < 1e-3);
    }

    #[test]
    fn test_meander_length_tuning() {
        let pts = TopologicalRouter::build_length_tuned_meanders(0.0, 0.0, 20.0, 0.0, 0.25, 30.0);
        assert!(pts.len() > 2);
    }

    #[test]
    fn test_push_and_shove_movable_vs_fixed_obstacle() {
        let mut board = ParsedBoard::empty();
        // Fixed obstacle: Pad at (10.0, 10.0), net 1 (prim_type 2.0 = pad)
        let pad_idx = board.add_primitive(10.0, 10.0, 10.0, 10.0, 1.5, 1.5, LAYER_F_CU, 1, 2.0);

        // Movable obstacle: Track passing through (10.0, 5.0) -> (10.0, 7.0), net 2 (prim_type 0.0 = track)
        let track_idx = board.add_primitive(10.0, 5.0, 10.0, 7.0, 0.25, 0.25, LAYER_F_CU, 2, 0.0);

        // Lay a new conflicting track segment along Y=6.0 from X=8.0 to X=12.0 on net 3
        let new_segments = [((8.0, 6.0), (12.0, 6.0))];
        let shoved = TopologicalRouter::push_and_shove_obstacles(
            &mut board,
            &new_segments,
            0.25,
            LAYER_F_CU,
            3,
            0.2,
        );

        assert!(shoved > 0, "Movable track must be shoved away");

        // Verify track moved
        let track_prim = board.get_primitive(track_idx).unwrap();
        assert!(
            (track_prim[1] - 5.0).abs() > 0.05 || (track_prim[3] - 7.0).abs() > 0.05,
            "Track must have been displaced from original Y coordinates"
        );

        // Verify pad did NOT move
        let pad_prim = board.get_primitive(pad_idx).unwrap();
        assert_eq!(pad_prim[0], 10.0);
        assert_eq!(pad_prim[1], 10.0);
    }
}
