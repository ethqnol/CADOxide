use std::collections::HashSet;

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

pub const FLOATS_PER_INSTANCE: usize = 8;

#[derive(Clone, Debug)]
pub struct ParsedBoard {
    pub instances: Vec<f32>,
    pub primitive_count: usize,
    pub bounds: [f32; 4], // [min_x, min_y, max_x, max_y]
}

/// Parses a KiCad `.kicad_pcb` file in pure Rust into a flat 8-float-per-instance GPU buffer.
pub fn parse_kicad_pcb_str(pcb_source: &str) -> ParsedBoard {
    let mut instances = Vec::with_capacity(8192 * FLOATS_PER_INSTANCE);
    let mut primitive_count = 0;

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

    ParsedBoard {
        instances,
        primitive_count,
        bounds,
    }
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
        ParsedBoard {
            instances: out_instances,
            primitive_count,
            bounds: board_b.bounds,
        },
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
                    0.0,
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
                    0.0,
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
                    1.0,
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
                    2.0,
                ),
            }
        }
    }

    ParsedBoard {
        instances,
        primitive_count,
        bounds,
    }
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
            if layer_mask != 0 {
                update_bounds(x1, y1);
                update_bounds(x2, y2);
                push_raw(instances, primitive_count, x1, y1, x2, y2, width, width, layer_mask as f32, 0.0);
            }
        }
    } else if is_token_start(block, "(via") {
        if let Some((x, y)) = extract_pair(block, "(at ") {
            let size = extract_single(block, "(size ").unwrap_or(0.8);
            let drill = extract_single(block, "(drill ").unwrap_or(0.4);
            update_bounds(x, y);
            let mask = LAYER_F_CU | LAYER_B_CU;
            push_raw(instances, primitive_count, x, y, x, y, size, drill, mask as f32, 1.0);
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
    } else if is_token_start(block, "(pad") && in_footprint {
        if let (Some((px, py, _prot)), Some((pw, ph))) = (
            parse_at_triple(block),
            extract_pair(block, "(size "),
        ) {
            let (wx, wy) = transform_point(px, py, fp_x, fp_y, fp_rot_rad);
            let layer_mask = extract_layer_mask(block);
            if layer_mask != 0 {
                update_bounds(wx, wy);
                push_raw(instances, primitive_count, wx, wy, wx, wy, pw, ph, layer_mask as f32, 2.0);
            }
        }
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

fn extract_layer_mask(s: &str) -> u32 {
    let mut mask = 0u32;
    if s.contains("\"*.Cu\"") {
        mask |= LAYER_F_CU | LAYER_B_CU;
    }
    if s.contains("\"F.Cu\"") {
        mask |= LAYER_F_CU;
    }
    if s.contains("\"B.Cu\"") {
        mask |= LAYER_B_CU;
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
