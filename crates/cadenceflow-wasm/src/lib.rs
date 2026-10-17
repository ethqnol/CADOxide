use std::cell::RefCell;
use std::slice;

/// Layer bitmask constants matching WebGPU WGSL / WebGL2 GLSL shader uniforms
pub const LAYER_F_CU: u32 = 1 << 0;
pub const LAYER_B_CU: u32 = 1 << 1;
pub const LAYER_EDGE_CUTS: u32 = 1 << 2;
pub const LAYER_F_SILKS: u32 = 1 << 3;
pub const LAYER_B_SILKS: u32 = 1 << 4;
pub const LAYER_F_MASK: u32 = 1 << 5;
pub const LAYER_B_MASK: u32 = 1 << 6;

/// 8 floats (32 bytes) per GPU instance:
/// [x1, y1, x2, y2, width, param2 (drill or height), layer_mask_f32, prim_type]
/// prim_type: 0.0 = capsule segment, 1.0 = via ring, 2.0 = SMD/TH pad
pub const FLOATS_PER_INSTANCE: usize = 8;

struct EngineState {
    instance_buffer: Vec<f32>,
    primitive_count: usize,
    bounds: [f32; 4],
    diff_stats: [u32; 3], // [added, removed, unchanged]
}

thread_local! {
    static STATE: RefCell<EngineState> = RefCell::new(EngineState {
        instance_buffer: Vec::with_capacity(100_000 * FLOATS_PER_INSTANCE),
        primitive_count: 0,
        bounds: [70.0, 40.0, 190.0, 150.0],
        diff_stats: [0, 0, 0],
    });
}

#[no_mangle]
pub extern "C" fn wasm_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

#[no_mangle]
pub extern "C" fn wasm_dealloc(ptr: *mut u8, cap: usize) {
    if !ptr.is_null() && cap > 0 {
        unsafe {
            let _ = Vec::from_raw_parts(ptr, 0, cap);
        }
    }
}

#[no_mangle]
pub extern "C" fn wasm_instances_ptr() -> *const f32 {
    STATE.with(|s| s.borrow().instance_buffer.as_ptr())
}

#[no_mangle]
pub extern "C" fn wasm_instances_float_len() -> usize {
    STATE.with(|s| s.borrow().instance_buffer.len())
}

#[no_mangle]
pub extern "C" fn wasm_primitive_count() -> usize {
    STATE.with(|s| s.borrow().primitive_count)
}

#[no_mangle]
pub extern "C" fn wasm_bounds_ptr() -> *const f32 {
    STATE.with(|s| s.borrow().bounds.as_ptr())
}

#[no_mangle]
pub extern "C" fn wasm_diff_stats_ptr() -> *const u32 {
    STATE.with(|s| s.borrow().diff_stats.as_ptr())
}

/// Zero-GC Rust parser for KiCad 6/7/8/9/10 `.kicad_pcb` S-expression files.
/// Handles both single-line and multi-line S-expression blocks directly in WASM linear memory.
#[no_mangle]
pub extern "C" fn wasm_parse_kicad_pcb(ptr: *const u8, len: usize) -> usize {
    if ptr.is_null() || len == 0 {
        return 0;
    }
    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    let pcb_source = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return 0,
    };

    STATE.with(|cell| {
        let mut st = cell.borrow_mut();
        st.instance_buffer.clear();
        st.primitive_count = 0;

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

            // If we are inside a multi-line primitive block (e.g. (segment ...), (pad ...), (fp_line ...))
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
                        &mut st,
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

            // Detect start of footprint block
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

            // Detect start of a geometric primitive block
            if is_token_start(line, "(segment")
                || is_token_start(line, "(via")
                || is_token_start(line, "(gr_line")
                || is_token_start(line, "(fp_line")
                || is_token_start(line, "(gr_rect")
                || is_token_start(line, "(fp_rect")
                || (in_footprint && is_token_start(line, "(pad"))
            {
                if line_delta <= 0 {
                    // Single-line primitive block
                    process_primitive_block(
                        line,
                        in_footprint,
                        fp_x,
                        fp_y,
                        fp_rot_rad,
                        &mut st,
                        &mut min_x,
                        &mut min_y,
                        &mut max_x,
                        &mut max_y,
                    );
                } else {
                    // Multi-line primitive block
                    active_block.clear();
                    active_block.push_str(line);
                    block_depth = line_delta;
                    collecting_block = true;
                }
            }
        }

        if min_x < max_x && min_y < max_y {
            st.bounds = [min_x - 2.0, min_y - 2.0, max_x + 2.0, max_y + 2.0];
        }

        st.primitive_count
    })
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
    st: &mut EngineState,
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
                push_inst(st, x1, y1, x2, y2, width, width, layer_mask as f32, 0.0);
            }
        }
    } else if is_token_start(block, "(via") {
        if let Some((x, y)) = extract_pair(block, "(at ") {
            let size = extract_single(block, "(size ").unwrap_or(0.8);
            let drill = extract_single(block, "(drill ").unwrap_or(0.4);
            update_bounds(x, y);
            let mask = LAYER_F_CU | LAYER_B_CU;
            push_inst(st, x, y, x, y, size, drill, mask as f32, 1.0);
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
                push_inst(st, x1, y1, x2, y2, width, width, layer_mask as f32, 0.0);
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
                push_inst(st, x1, y1, x2, y1, width, width, layer_mask as f32, 0.0);
                push_inst(st, x2, y1, x2, y2, width, width, layer_mask as f32, 0.0);
                push_inst(st, x2, y2, x1, y2, width, width, layer_mask as f32, 0.0);
                push_inst(st, x1, y2, x1, y1, width, width, layer_mask as f32, 0.0);
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
                push_inst(st, wx, wy, wx, wy, pw, ph, layer_mask as f32, 2.0);
            }
        }
    }
}

/// Generates a synthetic High-Density Interconnect (HDI) stress-test PCB in WASM linear memory
#[no_mangle]
pub extern "C" fn wasm_generate_hdi_stress_test(target_primitives: usize) -> usize {
    STATE.with(|cell| {
        let mut st = cell.borrow_mut();
        st.instance_buffer.clear();
        st.primitive_count = 0;

        let min_x = 70.0_f32;
        let min_y = 40.0_f32;
        let width = 120.0_f32;
        let height = 110.0_f32;
        st.bounds = [min_x, min_y, min_x + width, min_y + height];

        let mx = min_x + width;
        let my = min_y + height;
        let edge = LAYER_EDGE_CUTS as f32;
        push_inst(&mut st, min_x, min_y, mx, min_y, 0.3, 0.3, edge, 0.0);
        push_inst(&mut st, mx, min_y, mx, my, 0.3, 0.3, edge, 0.0);
        push_inst(&mut st, mx, my, min_x, my, 0.3, 0.3, edge, 0.0);
        push_inst(&mut st, min_x, my, min_x, min_y, 0.3, 0.3, edge, 0.0);

        let cols = ((target_primitives as f32).sqrt() as usize).max(10);
        let rows = (target_primitives / cols).max(10);
        let dx = (width - 6.0) / (cols as f32);
        let dy = (height - 6.0) / (rows as f32);

        for r in 0..rows {
            let y = min_y + 3.0 + (r as f32) * dy;
            for c in 0..cols {
                if st.primitive_count >= target_primitives {
                    break;
                }
                let x = min_x + 3.0 + (c as f32) * dx;
                let idx = r * cols + c;

                match idx % 4 {
                    0 => {
                        push_inst(
                            &mut st,
                            x,
                            y,
                            x + dx * 0.78,
                            y + dy * 0.38,
                            0.11,
                            0.11,
                            LAYER_F_CU as f32,
                            0.0,
                        );
                    }
                    1 => {
                        push_inst(
                            &mut st,
                            x,
                            y,
                            x + dx * 0.38,
                            y + dy * 0.82,
                            0.11,
                            0.11,
                            LAYER_B_CU as f32,
                            0.0,
                        );
                    }
                    2 => {
                        push_inst(
                            &mut st,
                            x,
                            y,
                            x,
                            y,
                            0.26,
                            0.13,
                            (LAYER_F_CU | LAYER_B_CU) as f32,
                            1.0,
                        );
                    }
                    _ => {
                        push_inst(
                            &mut st,
                            x,
                            y,
                            x,
                            y,
                            0.22,
                            0.22,
                            (LAYER_F_CU | LAYER_F_SILKS) as f32,
                            2.0,
                        );
                    }
                }
            }
        }

        st.primitive_count
    })
}

/// High-speed WASM RGBA Pixel Boolean Diff Kernel
#[no_mangle]
pub extern "C" fn wasm_compute_rgba_diff(
    rev_a_ptr: *const u8,
    rev_b_ptr: *const u8,
    out_ptr: *mut u8,
    byte_len: usize,
) {
    if rev_a_ptr.is_null() || rev_b_ptr.is_null() || out_ptr.is_null() || byte_len < 4 {
        return;
    }
    let rev_a = unsafe { slice::from_raw_parts(rev_a_ptr, byte_len) };
    let rev_b = unsafe { slice::from_raw_parts(rev_b_ptr, byte_len) };
    let out = unsafe { slice::from_raw_parts_mut(out_ptr, byte_len) };

    let mut added: u32 = 0;
    let mut removed: u32 = 0;
    let mut unchanged: u32 = 0;

    let mut i = 0;
    while i + 3 < byte_len {
        let a_on = rev_a[i + 3] > 25;
        let b_on = rev_b[i + 3] > 25;

        if b_on && !a_on {
            out[i] = 34;
            out[i + 1] = 197;
            out[i + 2] = 94;
            out[i + 3] = 255;
            added += 1;
        } else if a_on && !b_on {
            out[i] = 239;
            out[i + 1] = 68;
            out[i + 2] = 68;
            out[i + 3] = 255;
            removed += 1;
        } else if a_on && b_on {
            out[i] = 74;
            out[i + 1] = 85;
            out[i + 2] = 104;
            out[i + 3] = 210;
            unchanged += 1;
        } else {
            out[i] = 0;
            out[i + 1] = 0;
            out[i + 2] = 0;
            out[i + 3] = 0;
        }
        i += 4;
    }

    STATE.with(|s| {
        s.borrow_mut().diff_stats = [added, removed, unchanged];
    });
}

fn push_inst(
    st: &mut EngineState,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    w: f32,
    h: f32,
    layer_mask: f32,
    prim_type: f32,
) {
    st.instance_buffer
        .extend_from_slice(&[x1, y1, x2, y2, w, h, layer_mask, prim_type]);
    st.primitive_count += 1;
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
