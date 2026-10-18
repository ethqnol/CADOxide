use crate::gl_renderer::RustGpuRenderer;
use crate::parser::{
    ParsedBoard, LAYER_B_CU, LAYER_B_FAB, LAYER_B_SILKS, LAYER_EDGE_CUTS, LAYER_F_CRTYD,
    LAYER_F_CU, LAYER_F_FAB, LAYER_F_SILKS, LAYER_IN1_CU, LAYER_IN2_CU, LAYER_USER_CMTS,
    LAYER_USER_DWGS,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::{CanvasRenderingContext2d, Document, HtmlCanvasElement};

pub struct OverlayConfig<'a> {
    pub zone_display_mode: u8, // 0 = Filled, 1 = Outlines Only, 2 = Off
    pub show_pad_labels: bool,
    pub show_pad_nets: bool,
    pub show_fp_text: bool,
    pub show_ratsnest: bool,
    pub show_crosshair: bool,
    pub cursor_mm: (f32, f32),
    pub aux_origin_mm: (f32, f32),
    pub poly_preview_pts: &'a [(f32, f32)],
    pub active_draw_layer: u32,
    pub is_zone_preview: bool,
    pub is_keepout_preview: bool,
}

fn set_prop(ctx: &CanvasRenderingContext2d, key: &str, val: &str) {
    let _ = js_sys::Reflect::set(ctx, &JsValue::from_str(key), &JsValue::from_str(val));
}

fn set_num_prop(ctx: &CanvasRenderingContext2d, key: &str, val: f64) {
    let _ = js_sys::Reflect::set(ctx, &JsValue::from_str(key), &JsValue::from_f64(val));
}

fn z_for_layer(mask: u32) -> f32 {
    if (mask & LAYER_F_SILKS) != 0 {
        5.5
    } else if (mask & LAYER_F_CU) != 0 {
        2.2
    } else if (mask & LAYER_IN1_CU) != 0 {
        0.8
    } else if (mask & LAYER_IN2_CU) != 0 {
        -0.8
    } else if (mask & LAYER_B_CU) != 0 {
        -2.2
    } else if (mask & LAYER_B_SILKS) != 0 {
        -5.5
    } else {
        0.0
    }
}

pub fn render_cad_overlay(
    doc: &Document,
    renderer: &RustGpuRenderer,
    board: &ParsedBoard,
    cfg: &OverlayConfig,
) {
    let canvas_el = match doc.get_element_by_id("cad-overlay-canvas") {
        Some(el) => el,
        None => return,
    };
    let canvas: HtmlCanvasElement = match canvas_el.dyn_into() {
        Ok(c) => c,
        Err(_) => return,
    };

    let gpu_el = match doc.get_element_by_id("rust-gpu-canvas") {
        Some(el) => el,
        None => return,
    };
    let gpu_canvas: HtmlCanvasElement = match gpu_el.dyn_into() {
        Ok(c) => c,
        Err(_) => return,
    };

    let cw = gpu_canvas.width();
    let ch = gpu_canvas.height();
    if canvas.width() != cw {
        canvas.set_width(cw);
    }
    if canvas.height() != ch {
        canvas.set_height(ch);
    }

    let ctx: CanvasRenderingContext2d = match canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into().ok())
    {
        Some(c) => c,
        None => return,
    };

    let w_f64 = cw as f64;
    let h_f64 = ch as f64;
    ctx.clear_rect(0.0, 0.0, w_f64, h_f64);

    let active_mask = renderer.active_layer_mask;
    let scale = renderer.scale_px_per_mm;
    let hc_mode = renderer.high_contrast_mode;
    let active_draw_bit = renderer.active_draw_layer_bit;

    // ========================================================================
    // 1. Copper Zones & Teardrop Pours (Filled / Outline / Keepout)
    // ========================================================================
    if cfg.zone_display_mode < 2 && renderer.diff_mode < 0.5 {
        for zone in &board.zones {
            if (zone.layer_mask & active_mask) == 0 {
                continue;
            }
            if hc_mode == 2 && (zone.layer_mask & active_draw_bit) == 0 {
                continue;
            }
            // Teardrops only need to be drawn when zoomed in past 5 px/mm
            if zone.is_teardrop && scale < 5.0 {
                continue;
            }

            let dim_factor = if hc_mode == 1 && (zone.layer_mask & active_draw_bit) == 0 {
                0.28
            } else if renderer.selected_net_id > 0
                && zone.net_id != (renderer.selected_net_id as u32)
            {
                0.20
            } else {
                1.0
            };

            let (fill_rgb, stroke_rgb) = if zone.is_keepout {
                ("224, 56, 216", "235, 80, 225")
            } else if (zone.layer_mask & LAYER_F_CU) != 0 {
                ("200, 52, 52", "225, 72, 72")
            } else if (zone.layer_mask & LAYER_B_CU) != 0 {
                ("40, 120, 200", "64, 150, 235")
            } else if (zone.layer_mask & LAYER_IN1_CU) != 0 {
                ("194, 178, 56", "218, 202, 76")
            } else if (zone.layer_mask & LAYER_IN2_CU) != 0 {
                ("212, 123, 40", "235, 145, 62")
            } else {
                ("148, 162, 176", "180, 194, 208")
            };

            let z_layer = z_for_layer(zone.layer_mask);

            if cfg.zone_display_mode == 0 && !zone.is_keepout {
                let fill_alpha = (if zone.is_teardrop { 0.68 } else { 0.34 }) * dim_factor;
                let stroke_alpha = 0.72 * dim_factor;
                set_prop(
                    &ctx,
                    "fillStyle",
                    &format!("rgba({}, {:.3})", fill_rgb, fill_alpha),
                );
                set_prop(
                    &ctx,
                    "strokeStyle",
                    &format!("rgba({}, {:.3})", stroke_rgb, stroke_alpha),
                );
                set_num_prop(&ctx, "lineWidth", if zone.is_teardrop { 0.8 } else { 1.2 });

                let polys_to_draw = if !zone.filled_polys.is_empty() {
                    &zone.filled_polys
                } else {
                    std::slice::from_ref(&zone.outline)
                };

                for poly in polys_to_draw {
                    if poly.len() < 3 {
                        continue;
                    }
                    ctx.begin_path();
                    let (x0, y0) = renderer.board_mm_to_device_px(poly[0].0, poly[0].1, z_layer);
                    ctx.move_to(x0, y0);
                    for pt in &poly[1..] {
                        let (px, py) = renderer.board_mm_to_device_px(pt.0, pt.1, z_layer);
                        ctx.line_to(px, py);
                    }
                    ctx.close_path();
                    ctx.fill();
                    if !zone.is_teardrop {
                        ctx.stroke();
                    }
                }
            }

            // Always draw zone outline boundary (or keepout hatched outline) when not teardrop
            if !zone.is_teardrop && zone.outline.len() >= 3 {
                let stroke_alpha = 0.82 * dim_factor;
                set_prop(
                    &ctx,
                    "strokeStyle",
                    &format!("rgba({}, {:.3})", stroke_rgb, stroke_alpha),
                );
                set_num_prop(&ctx, "lineWidth", 1.35);
                ctx.begin_path();
                let (x0, y0) = renderer.board_mm_to_device_px(
                    zone.outline[0].0,
                    zone.outline[0].1,
                    z_layer,
                );
                ctx.move_to(x0, y0);
                for pt in &zone.outline[1..] {
                    let (px, py) = renderer.board_mm_to_device_px(pt.0, pt.1, z_layer);
                    ctx.line_to(px, py);
                }
                ctx.close_path();
                ctx.stroke();

                if zone.is_keepout {
                    set_prop(
                        &ctx,
                        "fillStyle",
                        &format!("rgba({}, {:.3})", fill_rgb, 0.14 * dim_factor),
                    );
                    ctx.fill();
                }
            }
        }
    }

    // ========================================================================
    // 2. Ratsnest Airwires (Unrouted / Same-Net Pad Connections)
    // ========================================================================
    let target_net = renderer.selected_net_id;
    if (cfg.show_ratsnest || target_net > 0) && renderer.diff_mode < 0.5 && board.pad_labels.len() > 1 {
        set_prop(&ctx, "strokeStyle", "rgba(160, 205, 255, 0.65)");
        set_num_prop(&ctx, "lineWidth", 1.1);

        // Nets that have copper plane fills are already fully connected
        let plane_nets: std::collections::HashSet<u32> = board
            .zones
            .iter()
            .filter(|z| !z.is_keepout && !z.is_teardrop)
            .map(|z| z.net_id)
            .collect();

        // Group pads by net_id
        let mut pads_by_net: std::collections::HashMap<u32, Vec<(f32, f32)>> =
            std::collections::HashMap::new();
        for pad in &board.pad_labels {
            if pad.net_id == 0 {
                continue;
            }
            if target_net > 0 {
                if pad.net_id != (target_net as u32) {
                    continue;
                }
            } else if plane_nets.contains(&pad.net_id) {
                // Skip plane nets (GND, etc.) on global ratsnest to avoid spiderwebs across copper fills
                continue;
            } else if let Some(stats) = board.nets.get(&pad.net_id) {
                // If net is already fully routed with tracks, don't clutter the board
                if stats.segment_count > 0 && stats.segment_count >= stats.pad_count.saturating_sub(1) {
                    continue;
                }
            }
            pads_by_net.entry(pad.net_id).or_default().push((pad.x, pad.y));
        }

        ctx.begin_path();
        // Compute Minimum Spanning Tree (MST via Prim's algorithm) for each net to avoid criss-crossing
        for (_nid, pts) in pads_by_net {
            if pts.len() < 2 {
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
                    let p0 = pts[best_edge.0];
                    let p1 = pts[best_edge.1];
                    let (x0, y0) = renderer.board_mm_to_device_px(p0.0, p0.1, 2.2);
                    let (x1, y1) = renderer.board_mm_to_device_px(p1.0, p1.1, 2.2);
                    ctx.move_to(x0, y0);
                    ctx.line_to(x1, y1);
                }
                in_tree[best_edge.1] = true;
                tree_count += 1;
            }
        }
        ctx.stroke();
    }

    // ========================================================================
    // 3. Footprint Reference / Value Labels & Board Text
    // ========================================================================
    if cfg.show_fp_text && scale >= 4.5 && renderer.diff_mode < 0.5 {
        set_prop(&ctx, "textAlign", "center");
        set_prop(&ctx, "textBaseline", "middle");

        for txt in &board.texts {
            if (txt.layer_mask & active_mask) == 0 {
                continue;
            }
            if hc_mode == 2 && (txt.layer_mask & active_draw_bit) == 0 {
                continue;
            }
            let z_layer = z_for_layer(txt.layer_mask);
            let (px, py) = renderer.board_mm_to_device_px(txt.x, txt.y, z_layer);
            if px < -120.0 || px > w_f64 + 120.0 || py < -60.0 || py > h_f64 + 60.0 {
                continue;
            }

            let font_px = (txt.size_mm * scale * 0.92).clamp(8.0, 32.0);
            if font_px < 8.2 && !txt.is_ref {
                continue;
            }

            let alpha = if hc_mode == 1 && (txt.layer_mask & active_draw_bit) == 0 {
                0.22
            } else {
                0.92
            };

            let color = if (txt.layer_mask & LAYER_F_SILKS) != 0 {
                format!("rgba(242, 237, 161, {:.2})", alpha)
            } else if (txt.layer_mask & LAYER_B_SILKS) != 0 {
                format!("rgba(232, 178, 166, {:.2})", alpha)
            } else if (txt.layer_mask & LAYER_F_CU) != 0 {
                format!("rgba(225, 75, 75, {:.2})", alpha)
            } else if (txt.layer_mask & LAYER_B_CU) != 0 {
                format!("rgba(75, 155, 235, {:.2})", alpha)
            } else if (txt.layer_mask & LAYER_F_CRTYD) != 0 {
                format!("rgba(224, 56, 216, {:.2})", alpha)
            } else if (txt.layer_mask & LAYER_EDGE_CUTS) != 0 {
                format!("rgba(216, 194, 56, {:.2})", alpha)
            } else if (txt.layer_mask & (LAYER_F_FAB | LAYER_B_FAB)) != 0 {
                format!("rgba(155, 165, 180, {:.2})", alpha * 0.85)
            } else if (txt.layer_mask & (LAYER_USER_DWGS | LAYER_USER_CMTS)) != 0 {
                format!("rgba(148, 182, 224, {:.2})", alpha)
            } else {
                format!("rgba(210, 218, 228, {:.2})", alpha)
            };

            set_prop(
                &ctx,
                "font",
                &format!("600 {:.1}px 'JetBrains Mono', monospace", font_px),
            );
            set_prop(&ctx, "fillStyle", &color);

            // Unescape \n and render multiline text block with proper vertical alignment
            let clean_text = txt.text.replace("\\n", "\n");
            let lines: Vec<&str> = clean_text.split('\n').collect();
            let line_height = (font_px * 1.25) as f64;
            let total_height = (lines.len() as f64) * line_height;
            let start_y = py - (total_height * 0.5) + (line_height * 0.5);

            for (i, line) in lines.iter().enumerate() {
                let _ = ctx.fill_text(line, px, start_y + (i as f64 * line_height));
            }
        }
    }

    // ========================================================================
    // 4. KiCad On-Pad Pin Numbers & Net Name Labels (1 / GND, +12V, etc.)
    // ========================================================================
    if (cfg.show_pad_labels || cfg.show_pad_nets) && scale >= 9.5 && renderer.diff_mode < 0.5 {
        set_prop(&ctx, "textAlign", "center");
        set_prop(&ctx, "textBaseline", "middle");

        for pad in &board.pad_labels {
            if (pad.layer_mask & active_mask) == 0 {
                continue;
            }
            let z_layer = if (pad.layer_mask & LAYER_F_CU) != 0 {
                2.2
            } else if (pad.layer_mask & LAYER_B_CU) != 0 {
                -2.2
            } else {
                0.0
            };
            let (px, py) = renderer.board_mm_to_device_px(pad.x, pad.y, z_layer);
            if px < -40.0 || px > w_f64 + 40.0 || py < -40.0 || py > h_f64 + 40.0 {
                continue;
            }

            let pw = (pad.w * scale) as f64;
            let ph = (pad.h * scale) as f64;
            let min_dim = pw.min(ph);
            let max_dim = pw.max(ph);
            if max_dim < 18.0 || min_dim < 10.0 {
                continue;
            }

            let has_pin = cfg.show_pad_labels && !pad.pin.is_empty();
            let has_net = cfg.show_pad_nets && !pad.net_name.is_empty();

            if has_pin && has_net && min_dim >= 22.0 {
                let fsize = (min_dim * 0.28).clamp(8.0, 14.0);
                set_prop(
                    &ctx,
                    "font",
                    &format!("700 {:.1}px 'JetBrains Mono', monospace", fsize),
                );
                set_prop(&ctx, "fillStyle", "rgba(255, 255, 255, 0.92)");
                let _ = ctx.fill_text(&pad.pin, px, py - fsize * 0.58);

                let nsize = (fsize * 0.86).clamp(7.5, 12.0);
                set_prop(
                    &ctx,
                    "font",
                    &format!("600 {:.1}px 'JetBrains Mono', monospace", nsize),
                );
                set_prop(&ctx, "fillStyle", "rgba(220, 242, 255, 0.88)");
                let short_net = if pad.net_name.len() > 8 {
                    &pad.net_name[..8]
                } else {
                    &pad.net_name
                };
                let _ = ctx.fill_text(short_net, px, py + fsize * 0.62);
            } else if has_pin {
                let fsize = (min_dim * 0.42).clamp(8.0, 14.0);
                set_prop(
                    &ctx,
                    "font",
                    &format!("700 {:.1}px 'JetBrains Mono', monospace", fsize),
                );
                set_prop(&ctx, "fillStyle", "rgba(255, 255, 255, 0.90)");
                let _ = ctx.fill_text(&pad.pin, px, py);
            } else if has_net {
                let fsize = (min_dim * 0.34).clamp(7.5, 12.0);
                set_prop(
                    &ctx,
                    "font",
                    &format!("600 {:.1}px 'JetBrains Mono', monospace", fsize),
                );
                set_prop(&ctx, "fillStyle", "rgba(220, 242, 255, 0.88)");
                let _ = ctx.fill_text(&pad.net_name, px, py);
            }
        }
    }

    // ========================================================================
    // 5. Linear Caliper Dimensions (CadDimension)
    // ========================================================================
    if !board.dimensions.is_empty() {
        set_prop(&ctx, "strokeStyle", "rgba(148, 198, 255, 0.92)");
        set_prop(&ctx, "fillStyle", "rgba(225, 240, 255, 0.96)");
        set_num_prop(&ctx, "lineWidth", 1.4);
        set_prop(&ctx, "textAlign", "center");
        set_prop(&ctx, "textBaseline", "bottom");
        set_prop(&ctx, "font", "600 11px 'JetBrains Mono', monospace");

        for dim in &board.dimensions {
            let (x0, y0) = renderer.board_mm_to_device_px(dim.x0, dim.y0, 5.5);
            let (x1, y1) = renderer.board_mm_to_device_px(dim.x1, dim.y1, 5.5);
            ctx.begin_path();
            ctx.move_to(x0, y0);
            ctx.line_to(x1, y1);
            ctx.stroke();

            let dx = dim.x1 - dim.x0;
            let dy = dim.y1 - dim.y0;
            let dist = (dx * dx + dy * dy).sqrt();
            let mx = (x0 + x1) * 0.5;
            let my = (y0 + y1) * 0.5 - 4.0;
            let _ = ctx.fill_text(&format!("{:.3} mm", dist), mx, my);
        }
    }

    // ========================================================================
    // 6. Auxiliary / Relative Origin Marker & Live Polygon/Zone Preview
    // ========================================================================
    {
        let (ox, oy) = renderer.board_mm_to_device_px(cfg.aux_origin_mm.0, cfg.aux_origin_mm.1, 0.0);
        set_prop(&ctx, "strokeStyle", "rgba(235, 95, 95, 0.82)");
        set_num_prop(&ctx, "lineWidth", 1.2);
        ctx.begin_path();
        ctx.move_to(ox - 10.0, oy);
        ctx.line_to(ox + 10.0, oy);
        ctx.move_to(ox, oy - 10.0);
        ctx.line_to(ox, oy + 10.0);
        let _ = ctx.arc(ox, oy, 5.5, 0.0, std::f64::consts::TAU);
        ctx.stroke();
    }

    if !cfg.poly_preview_pts.is_empty() {
        let color_rgb = if cfg.is_keepout_preview {
            "224, 56, 216"
        } else if (cfg.active_draw_layer & LAYER_F_CU) != 0 {
            "225, 72, 72"
        } else if (cfg.active_draw_layer & LAYER_B_CU) != 0 {
            "64, 150, 235"
        } else {
            "243, 208, 107"
        };

        set_prop(&ctx, "strokeStyle", &format!("rgba({}, 0.95)", color_rgb));
        set_prop(&ctx, "fillStyle", &format!("rgba({}, 0.24)", color_rgb));
        set_num_prop(&ctx, "lineWidth", 1.6);

        ctx.begin_path();
        let (x0, y0) = renderer.board_mm_to_device_px(
            cfg.poly_preview_pts[0].0,
            cfg.poly_preview_pts[0].1,
            2.2,
        );
        ctx.move_to(x0, y0);
        for pt in &cfg.poly_preview_pts[1..] {
            let (px, py) = renderer.board_mm_to_device_px(pt.0, pt.1, 2.2);
            ctx.line_to(px, py);
        }
        let (cx, cy) =
            renderer.board_mm_to_device_px(cfg.cursor_mm.0, cfg.cursor_mm.1, 2.2);
        ctx.line_to(cx, cy);
        if cfg.is_zone_preview || cfg.poly_preview_pts.len() >= 2 {
            ctx.close_path();
            ctx.fill();
        }
        ctx.stroke();
    }

    // ========================================================================
    // 7. Full-Window Engineering Crosshair Cursor
    // ========================================================================
    if cfg.show_crosshair {
        let (cx, cy) =
            renderer.board_mm_to_device_px(cfg.cursor_mm.0, cfg.cursor_mm.1, 0.0);
        set_prop(&ctx, "strokeStyle", "rgba(180, 195, 215, 0.34)");
        set_num_prop(&ctx, "lineWidth", 1.0);
        ctx.begin_path();
        ctx.move_to(0.0, cy);
        ctx.line_to(w_f64, cy);
        ctx.move_to(cx, 0.0);
        ctx.line_to(cx, h_f64);
        ctx.stroke();
    }
}
