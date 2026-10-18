pub mod gl_renderer;
pub mod overlay_renderer;
pub mod parser;

use gl_renderer::RustGpuRenderer;
use overlay_renderer::{render_cad_overlay, OverlayConfig};
use parser::{
    compute_vector_diff_boards, generate_hdi_stress_board, parse_kicad_pcb_str, push_raw,
    BoardText, CadDimension, CopperZone, ParsedBoard, LAYER_B_CRTYD, LAYER_B_CU, LAYER_B_FAB,
    LAYER_B_MASK, LAYER_B_PASTE, LAYER_B_SILKS, LAYER_DRC_MARKER, LAYER_EDGE_CUTS, LAYER_F_CRTYD,
    LAYER_F_CU, LAYER_F_FAB, LAYER_F_MASK, LAYER_F_PASTE, LAYER_F_SILKS, LAYER_IN1_CU,
    LAYER_IN2_CU, LAYER_MARGIN, LAYER_REVIEW_PIN, LAYER_USER_CMTS, LAYER_USER_DWGS,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use std::slice;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    Document, Element, Event, HtmlCanvasElement, HtmlElement, HtmlInputElement, HtmlSelectElement,
    KeyboardEvent, MouseEvent, WheelEvent,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CadTool {
    SelectMove,
    RouteTrack,
    RouteDiffPair,
    TuneTrackLength,
    PlaceVia,
    PlacePad,
    AddZone,
    AddKeepout,
    DrawLine,
    DrawArc,
    DrawCircle,
    DrawRect,
    DrawPoly,
    AddText,
    AddDimension,
    PlaceOrigin,
    Measure,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DisplayUnits {
    Mm,
    Mil,
    Inch,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DrcItem {
    pub description: String,
    pub severity: String,
    pub type_name: String,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PartLibraryEntry {
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub value: String,
    pub footprint_name: String,
    pub symbol_name: String,
    pub lifecycle_status: String,
    pub unit_price_usd: f64,
    pub digikey_stock: i64,
    pub mouser_stock: i64,
}

pub struct AppController {
    pub doc: Document,
    pub renderer: Option<RustGpuRenderer>,
    pub active_tab: String,    // "pcb" | "schematic" | "diff" | "drc" | "parts"
    pub appear_subtab: String, // "layers" | "objects" | "nets"
    pub active_project: String,
    pub board_rev_a: ParsedBoard,
    pub board_rev_b: ParsedBoard,
    pub board_diff: ParsedBoard,
    pub diff_stats: (usize, usize, usize),
    pub current_board: ParsedBoard,
    pub undo_stack: Vec<ParsedBoard>,
    pub parse_time_ms: f64,

    // Mouse & Camera Interaction State
    pub is_dragging: bool,
    pub drag_button: i16,
    pub is_dragging_primitive: bool,
    pub drag_prim_last_mm: (f32, f32),
    pub last_mouse: (f64, f64),
    pub mouse_down_pos: (f64, f64),
    pub drag_3d_pan_mode: bool,

    // Interactive CAD Tool & Editing State
    pub active_tool: CadTool,
    pub active_draw_layer: u32,
    pub active_track_width_mm: f32,
    pub active_via_size_mm: f32,
    pub active_via_drill_mm: f32,
    pub grid_snap_mm: f32,
    pub selected_primitive_idx: Option<usize>,
    pub route_start_mm: Option<(f32, f32, u32)>,
    pub poly_points_mm: Vec<(f32, f32)>,
    pub route_posture_flip: bool,
    pub cursor_snapped_mm: (f32, f32),
    pub rel_origin_mm: (f32, f32),

    // KiCad Display & Viewing Options
    pub units: DisplayUnits,
    pub polar_coords: bool,
    pub show_crosshair: bool,
    pub show_pad_labels: bool,
    pub show_pad_nets: bool,
    pub show_fp_text: bool,
    pub show_ratsnest: bool,
    pub zone_display_mode: u8, // 0 = Filled, 1 = Outlines, 2 = Off

    // KiCad Selection Filter Flags
    pub sel_footprints: bool,
    pub sel_tracks: bool,
    pub sel_vias: bool,
    pub sel_pads: bool,
    pub sel_zones: bool,
    pub sel_graphics: bool,

    // Schematic Pan & Zoom State
    pub sch_zoom: f64,
    pub sch_pan: (f64, f64),
    pub sch_dragging: bool,
    pub sch_last_mouse: (f64, f64),

    pub drc_items: Vec<DrcItem>,
    pub parts_catalog: Vec<PartLibraryEntry>,
    pub parts_filter: String,
}

impl AppController {
    pub fn sel_filter_mask(&self) -> u32 {
        (if self.sel_tracks { 1 } else { 0 })
            | (if self.sel_vias { 2 } else { 0 })
            | (if self.sel_pads || self.sel_footprints { 4 } else { 0 })
            | (if self.sel_graphics { 8 } else { 0 })
    }

    pub fn snap_mm(&self, x: f32, y: f32) -> (f32, f32) {
        let g = self.grid_snap_mm.max(0.01);
        ((x / g).round() * g, (y / g).round() * g)
    }

    pub fn format_unit(&self, val_mm: f32) -> String {
        match self.units {
            DisplayUnits::Mm => format!("{:.4}", val_mm),
            DisplayUnits::Mil => format!("{:.1}", val_mm * 39.370079),
            DisplayUnits::Inch => format!("{:.4}", val_mm / 25.4),
        }
    }

    pub fn unit_suffix(&self) -> &'static str {
        match self.units {
            DisplayUnits::Mm => "mm",
            DisplayUnits::Mil => "mil",
            DisplayUnits::Inch => "in",
        }
    }

    pub fn push_undo(&mut self) {
        if self.undo_stack.len() >= 25 {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.current_board.clone());
    }

    pub fn perform_undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.current_board = prev;
            self.selected_primitive_idx = None;
            self.route_start_mm = None;
            self.poly_points_mm.clear();
            if let Some(ref mut r) = self.renderer {
                r.selected_instance_id = -1;
            }
            self.populate_net_selector();
            self.rebuild_gpu_buffer_with_markers();
            self.render_inspector_panel();
        }
    }

    pub fn delete_selected_primitive(&mut self) {
        if let Some(idx) = self.selected_primitive_idx {
            self.push_undo();
            if self.current_board.delete_primitive(idx) {
                self.selected_primitive_idx = None;
                if let Some(ref mut r) = self.renderer {
                    r.selected_instance_id = -1;
                }
                self.populate_net_selector();
                self.rebuild_gpu_buffer_with_markers();
                self.render_inspector_panel();
            }
        }
    }

    pub fn rotate_selected_primitive(&mut self) {
        if let Some(idx) = self.selected_primitive_idx {
            self.push_undo();
            self.current_board.rotate_primitive_90(idx);
            self.rebuild_gpu_buffer_with_markers();
            self.render_inspector_panel();
        }
    }

    pub fn finish_polygon_or_zone(&mut self) {
        if self.poly_points_mm.len() >= 3 {
            self.push_undo();
            let pts = self.poly_points_mm.clone();
            let layer = self.active_draw_layer;
            match self.active_tool {
                CadTool::AddZone | CadTool::AddKeepout => {
                    let is_keepout = self.active_tool == CadTool::AddKeepout;
                    let net_id = if is_keepout {
                        0
                    } else {
                        self.renderer
                            .as_ref()
                            .map(|r| r.selected_net_id.max(0) as u32)
                            .unwrap_or(1)
                    };
                    let net_name = self
                        .current_board
                        .nets
                        .get(&net_id)
                        .map(|n| n.name.clone())
                        .unwrap_or_else(|| "GND".to_string());
                    self.current_board.zones.push(CopperZone {
                        net_id,
                        net_name,
                        layer_mask: layer,
                        is_keepout,
                        is_teardrop: false,
                        outline: pts.clone(),
                        filled_polys: vec![pts],
                    });
                }
                CadTool::DrawPoly => {
                    for i in 0..pts.len() {
                        let p0 = pts[i];
                        let p1 = pts[(i + 1) % pts.len()];
                        self.current_board
                            .add_primitive(p0.0, p0.1, p1.0, p1.1, 0.18, 0.18, layer, 0, 0.0);
                    }
                }
                _ => {}
            }
        }
        self.poly_points_mm.clear();
        self.route_start_mm = None;
        self.rebuild_gpu_buffer_with_markers();
        self.render_inspector_panel();
    }

    pub fn set_active_tool(&mut self, tool: CadTool) {
        self.active_tool = tool;
        self.route_start_mm = None;
        self.poly_points_mm.clear();

        for (btn_id, t) in [
            ("btn-tool-select", CadTool::SelectMove),
            ("btn-tool-route", CadTool::RouteTrack),
            ("btn-tool-diffpair", CadTool::RouteDiffPair),
            ("btn-tool-tune", CadTool::TuneTrackLength),
            ("btn-tool-via", CadTool::PlaceVia),
            ("btn-tool-pad", CadTool::PlacePad),
            ("btn-tool-zone", CadTool::AddZone),
            ("btn-tool-keepout", CadTool::AddKeepout),
            ("btn-tool-line", CadTool::DrawLine),
            ("btn-tool-arc", CadTool::DrawArc),
            ("btn-tool-circle", CadTool::DrawCircle),
            ("btn-tool-rect", CadTool::DrawRect),
            ("btn-tool-poly", CadTool::DrawPoly),
            ("btn-tool-text", CadTool::AddText),
            ("btn-tool-dim", CadTool::AddDimension),
            ("btn-tool-origin", CadTool::PlaceOrigin),
            ("btn-tool-measure", CadTool::Measure),
        ] {
            if let Some(el) = self.doc.get_element_by_id(btn_id) {
                el.set_class_name(if t == tool {
                    "cad-tool-btn cad-tool-btn-active"
                } else {
                    "cad-tool-btn"
                });
            }
        }

        let tool_name = match tool {
            CadTool::SelectMove => "Select item(s) (S)",
            CadTool::RouteTrack => "Route single track (X) [/ flips posture]",
            CadTool::RouteDiffPair => "Route differential pair (6)",
            CadTool::TuneTrackLength => "Tune length of a single track (7)",
            CadTool::PlaceVia => "Add freestanding vias (V)",
            CadTool::PlacePad => "Add footprint / SMD pad (P)",
            CadTool::AddZone => "Add filled copper zone (Z) [Right-click closes]",
            CadTool::AddKeepout => "Add rule area / keepout (K)",
            CadTool::DrawLine => "Draw a line (L)",
            CadTool::DrawArc => "Draw an arc (A)",
            CadTool::DrawCircle => "Draw a circle (C)",
            CadTool::DrawRect => "Draw a rectangle (R)",
            CadTool::DrawPoly => "Draw a graphic polygon (Y)",
            CadTool::AddText => "Add a text item (T)",
            CadTool::AddDimension => "Add aligned linear dimension (D)",
            CadTool::PlaceOrigin => "Place grid/drill origin point (O)",
            CadTool::Measure => "Measure between two points (M)",
        };
        if let Some(lbl) = self.doc.get_element_by_id("active-tool-badge") {
            lbl.set_text_content(Some(tool_name));
        }

        self.rebuild_gpu_buffer_with_markers();
        self.render_inspector_panel();
    }

    pub fn set_active_draw_layer(&mut self, layer_bit: u32) {
        self.active_draw_layer = layer_bit;
        if let Some(ref mut r) = self.renderer {
            r.active_draw_layer_bit = layer_bit;
        }
        if let Some(sel) = self
            .doc
            .get_element_by_id("select-draw-layer")
            .and_then(|e| e.dyn_into::<HtmlSelectElement>().ok())
        {
            sel.set_value(&layer_bit.to_string());
        }
        // Update active triangle indicator in the layer table
        let rows = self.doc.query_selector_all(".cad-layer-row").ok();
        if let Some(list) = rows {
            for i in 0..list.length() {
                if let Some(el) = list.item(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                    let row_bit = el
                        .get_attribute("data-layer-bit")
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or(0);
                    el.set_class_name(if row_bit == layer_bit {
                        "cad-layer-row cad-layer-row-active"
                    } else {
                        "cad-layer-row"
                    });
                    if let Some(arr) = el.query_selector(".cad-active-arrow").ok().flatten() {
                        arr.set_text_content(Some(if row_bit == layer_bit { "▶" } else { "" }));
                    }
                }
            }
        }
        self.draw_all();
    }

    pub fn draw_all(&mut self) {
        if let Some(ref mut r) = self.renderer {
            r.grid_step_mm = self.grid_snap_mm;
            r.active_draw_layer_bit = self.active_draw_layer;
            r.draw();
        }
        if let Some(ref r) = self.renderer {
            let base = if self.active_tab == "diff" {
                &self.board_diff
            } else {
                &self.current_board
            };
            let cfg = OverlayConfig {
                zone_display_mode: self.zone_display_mode,
                show_pad_labels: self.show_pad_labels,
                show_pad_nets: self.show_pad_nets,
                show_fp_text: self.show_fp_text,
                show_ratsnest: self.show_ratsnest,
                show_crosshair: self.show_crosshair,
                cursor_mm: self.cursor_snapped_mm,
                aux_origin_mm: self.rel_origin_mm,
                poly_preview_pts: &self.poly_points_mm,
                active_draw_layer: self.active_draw_layer,
                is_zone_preview: self.active_tool == CadTool::AddZone
                    || self.active_tool == CadTool::AddKeepout,
                is_keepout_preview: self.active_tool == CadTool::AddKeepout,
            };
            render_cad_overlay(&self.doc, r, base, &cfg);
        }
    }

    pub fn select_net_probe(&mut self, net_id: i32) {
        if let Some(ref mut r) = self.renderer {
            r.selected_net_id = net_id.max(0);
        }
        if let Some(sel) = self
            .doc
            .get_element_by_id("rust-net-select")
            .and_then(|e| e.dyn_into::<HtmlSelectElement>().ok())
        {
            sel.set_value(&net_id.max(0).to_string());
        }
        self.draw_all();
        self.render_inspector_panel();
    }

    pub fn populate_net_selector(&self) {
        let mut net_list: Vec<_> = self.current_board.nets.values().collect();
        net_list.sort_by(|a, b| {
            b.total_length_mm
                .partial_cmp(&a.total_length_mm)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some(sel) = self.doc.get_element_by_id("rust-net-select") {
            let mut html = String::from(r#"<option value="0">Net Filter: All Nets</option>"#);
            for n in net_list.iter().take(100) {
                let label = if n.name.is_empty() {
                    format!("Net #{}", n.id)
                } else {
                    n.name.clone()
                };
                html.push_str(&format!(
                    r#"<option value="{}">{} ({:.2} mm | {}v)</option>"#,
                    n.id, label, n.total_length_mm, n.via_count
                ));
            }
            sel.set_inner_html(&html);
        }

        if let Some(nets_panel) = self.doc.get_element_by_id("appear-nets-content") {
            let mut html = String::from(
                r#"<div style="padding:4px 8px;border-bottom:1px solid #282c36;display:flex;justify-content:space-between;font-size:10px;color:#8e96a8;">
                     <span>Net Name</span><span>Length / Pads</span>
                   </div>"#,
            );
            html.push_str(
                r#"<div data-net-id="0" class="cad-layer-row"><span style="color:#79b8ff;">[Show All Nets]</span><span class="cad-layer-type">Reset</span></div>"#,
            );
            for n in net_list.iter().take(120) {
                let label = if n.name.is_empty() {
                    format!("Net #{}", n.id)
                } else {
                    n.name.clone()
                };
                html.push_str(&format!(
                    r#"<div data-net-id="{id}" class="cad-layer-row">
                         <span style="overflow:hidden;text-overflow:ellipsis;">{label}</span>
                         <span class="cad-layer-type">{len:.1}mm ({pads}p)</span>
                       </div>"#,
                    id = n.id,
                    label = label,
                    len = n.total_length_mm,
                    pads = n.pad_count
                ));
            }
            nets_panel.set_inner_html(&html);
        }
    }

    pub fn compute_45_knee(&self, sx: f32, sy: f32, ex: f32, ey: f32) -> (f32, f32) {
        let dx = ex - sx;
        let dy = ey - sy;
        let adx = dx.abs();
        let ady = dy.abs();
        if !self.route_posture_flip {
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

    pub fn build_serpentine_points(
        &self,
        sx: f32,
        sy: f32,
        ex: f32,
        ey: f32,
    ) -> Vec<(f32, f32)> {
        let dx = ex - sx;
        let dy = ey - sy;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.8 {
            return vec![(sx, sy), (ex, ey)];
        }
        let ux = dx / len;
        let uy = dy / len;
        let nx = -uy;
        let ny = ux;
        let amp = (self.active_track_width_mm * 3.2).max(0.65);
        let pitch = (self.active_track_width_mm * 2.8).max(0.55);
        let steps = ((len / pitch).floor() as usize).clamp(2, 24);
        let mut pts = Vec::with_capacity(steps * 2 + 2);
        pts.push((sx, sy));
        for i in 0..steps {
            let t0 = (i as f32 + 0.2) / (steps as f32);
            let t1 = (i as f32 + 0.8) / (steps as f32);
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            pts.push((sx + ux * len * t0 + nx * amp * sign, sy + uy * len * t0 + ny * amp * sign));
            pts.push((sx + ux * len * t1 + nx * amp * sign, sy + uy * len * t1 + ny * amp * sign));
        }
        pts.push((ex, ey));
        pts
    }

    pub fn rebuild_gpu_buffer_with_markers(&mut self) {
        let base = if self.active_tab == "diff" {
            &self.board_diff
        } else {
            &self.current_board
        };

        let mut combined = base.instances.clone();
        let mut count = base.primitive_count;

        if self.active_tab != "diff" {
            for d in &self.drc_items {
                if d.x > 1.0 && d.y > 1.0 {
                    push_raw(
                        &mut combined,
                        &mut count,
                        d.x,
                        d.y,
                        d.x,
                        d.y,
                        1.3,
                        0.5,
                        LAYER_DRC_MARKER as f32,
                        5.0,
                    );
                }
            }

            let (cx, cy) = self.cursor_snapped_mm;
            match self.active_tool {
                CadTool::RouteTrack => {
                    if let Some((sx, sy, net_id)) = self.route_start_mm {
                        let packed_w = (net_id as f32) * 16.0;
                        let (kx, ky) = self.compute_45_knee(sx, sy, cx, cy);
                        push_raw(
                            &mut combined,
                            &mut count,
                            sx,
                            sy,
                            kx,
                            ky,
                            self.active_track_width_mm,
                            self.active_track_width_mm,
                            self.active_draw_layer as f32,
                            packed_w,
                        );
                        push_raw(
                            &mut combined,
                            &mut count,
                            kx,
                            ky,
                            cx,
                            cy,
                            self.active_track_width_mm,
                            self.active_track_width_mm,
                            self.active_draw_layer as f32,
                            packed_w,
                        );
                    }
                }
                CadTool::RouteDiffPair => {
                    if let Some((sx, sy, net_id)) = self.route_start_mm {
                        let dx = cx - sx;
                        let dy = cy - sy;
                        let len = (dx * dx + dy * dy).sqrt().max(0.01);
                        let nx = -dy / len;
                        let ny = dx / len;
                        let half_pitch = self.active_track_width_mm * 1.1;
                        let packed_p = (net_id as f32) * 16.0;
                        let packed_n = ((net_id + 1) as f32) * 16.0;
                        for (sign, pw) in [(1.0f32, packed_p), (-1.0f32, packed_n)] {
                            let ox = nx * half_pitch * sign;
                            let oy = ny * half_pitch * sign;
                            let (kx, ky) = self.compute_45_knee(sx + ox, sy + oy, cx + ox, cy + oy);
                            push_raw(
                                &mut combined,
                                &mut count,
                                sx + ox,
                                sy + oy,
                                kx,
                                ky,
                                self.active_track_width_mm,
                                self.active_track_width_mm,
                                self.active_draw_layer as f32,
                                pw,
                            );
                            push_raw(
                                &mut combined,
                                &mut count,
                                kx,
                                ky,
                                cx + ox,
                                cy + oy,
                                self.active_track_width_mm,
                                self.active_track_width_mm,
                                self.active_draw_layer as f32,
                                pw,
                            );
                        }
                    }
                }
                CadTool::TuneTrackLength => {
                    if let Some((sx, sy, net_id)) = self.route_start_mm {
                        let pts = self.build_serpentine_points(sx, sy, cx, cy);
                        let packed_w = (net_id as f32) * 16.0;
                        for w_pair in pts.windows(2) {
                            push_raw(
                                &mut combined,
                                &mut count,
                                w_pair[0].0,
                                w_pair[0].1,
                                w_pair[1].0,
                                w_pair[1].1,
                                self.active_track_width_mm,
                                self.active_track_width_mm,
                                self.active_draw_layer as f32,
                                packed_w,
                            );
                        }
                    }
                }
                CadTool::PlaceVia => {
                    push_raw(
                        &mut combined,
                        &mut count,
                        cx,
                        cy,
                        cx,
                        cy,
                        self.active_via_size_mm,
                        self.active_via_drill_mm,
                        (LAYER_F_CU | LAYER_B_CU) as f32,
                        1.0,
                    );
                }
                CadTool::PlacePad => {
                    push_raw(
                        &mut combined,
                        &mut count,
                        cx,
                        cy,
                        cx,
                        cy,
                        1.4,
                        0.9,
                        self.active_draw_layer as f32,
                        2.0,
                    );
                }
                CadTool::DrawLine | CadTool::DrawArc => {
                    if let Some((sx, sy, _)) = self.route_start_mm {
                        push_raw(
                            &mut combined,
                            &mut count,
                            sx,
                            sy,
                            cx,
                            cy,
                            0.18,
                            0.18,
                            self.active_draw_layer as f32,
                            0.0,
                        );
                    }
                }
                CadTool::DrawCircle => {
                    if let Some((sx, sy, _)) = self.route_start_mm {
                        let rad = ((cx - sx).hypot(cy - sy)).max(0.2);
                        let w = 0.18f32;
                        push_raw(
                            &mut combined,
                            &mut count,
                            sx,
                            sy,
                            sx,
                            sy,
                            (rad + w * 0.5) * 2.0,
                            (rad - w * 0.5).max(0.02) * 2.0,
                            self.active_draw_layer as f32,
                            8.0,
                        );
                    }
                }
                CadTool::DrawRect => {
                    if let Some((sx, sy, _)) = self.route_start_mm {
                        let layer = self.active_draw_layer as f32;
                        push_raw(&mut combined, &mut count, sx, sy, cx, sy, 0.18, 0.18, layer, 0.0);
                        push_raw(&mut combined, &mut count, cx, sy, cx, cy, 0.18, 0.18, layer, 0.0);
                        push_raw(&mut combined, &mut count, cx, cy, sx, cy, 0.18, 0.18, layer, 0.0);
                        push_raw(&mut combined, &mut count, sx, cy, sx, sy, 0.18, 0.18, layer, 0.0);
                    }
                }
                CadTool::Measure | CadTool::AddDimension => {
                    if let Some((sx, sy, _)) = self.route_start_mm {
                        push_raw(
                            &mut combined,
                            &mut count,
                            sx,
                            sy,
                            cx,
                            cy,
                            0.15,
                            0.15,
                            LAYER_REVIEW_PIN as f32,
                            0.0,
                        );
                        push_raw(
                            &mut combined,
                            &mut count,
                            sx,
                            sy,
                            sx,
                            sy,
                            1.1,
                            1.1,
                            LAYER_REVIEW_PIN as f32,
                            6.0,
                        );
                        push_raw(
                            &mut combined,
                            &mut count,
                            cx,
                            cy,
                            cx,
                            cy,
                            1.1,
                            1.1,
                            LAYER_REVIEW_PIN as f32,
                            6.0,
                        );
                    }
                }
                _ => {}
            }
        }

        let bounds = base.bounds;
        let sel_idx = self.selected_primitive_idx.map(|i| i as i32).unwrap_or(-1);
        if let Some(ref mut r) = self.renderer {
            r.selected_instance_id = sel_idx;
            r.upload_instances(&combined, bounds);
        }
        self.draw_all();
        self.update_telemetry_hud();
        self.update_export_pcb_link();
    }

    pub fn update_export_pcb_link(&self) {
        if let Some(link) = self.doc.get_element_by_id("btn-export-pcb") {
            let kicad_str = self.current_board.export_kicad_pcb();
            let encoded = js_sys::encode_uri_component(&kicad_str);
            let data_uri = format!("data:text/plain;charset=utf-8,{}", encoded);
            let _ = link.set_attribute("href", &data_uri);
        }
    }

    pub fn apply_schematic_transform(&self) {
        if let Some(inner) = self
            .doc
            .get_element_by_id("schematic-canvas-inner")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        {
            let _ = inner.style().set_property(
                "transform",
                &format!(
                    "translate({:.1}px, {:.1}px) scale({:.3})",
                    self.sch_pan.0, self.sch_pan.1, self.sch_zoom
                ),
            );
        }
    }

    pub fn render_inspector_panel(&self) {
        let container = match self.doc.get_element_by_id("cad-inspector-content") {
            Some(c) => c,
            None => return,
        };

        let mut html = String::new();

        if let Some(idx) = self.selected_primitive_idx {
            if let Some(p) = self.current_board.get_primitive(idx) {
                let x0 = p[0];
                let y0 = p[1];
                let x1 = p[2];
                let y1 = p[3];
                let w = p[4];
                let h = p[5];
                let layer_bits = p[6] as u32;
                let raw_w = p[7];
                let net_id = (raw_w / 16.0).floor() as u32;
                let prim_type = (raw_w - (net_id as f32) * 16.0).round() as i32;

                let type_label = match prim_type {
                    0 => "Copper Track / Line",
                    1 => "Plated Through-Hole Via",
                    2 | 3 => "SMD Component Pad",
                    4 => "Rect PTH Pad",
                    8 => "Graphic Circle",
                    _ => "CAD Primitive",
                };

                let sel_fcu = if layer_bits == LAYER_F_CU { "selected" } else { "" };
                let sel_in1 = if layer_bits == LAYER_IN1_CU { "selected" } else { "" };
                let sel_in2 = if layer_bits == LAYER_IN2_CU { "selected" } else { "" };
                let sel_bcu = if layer_bits == LAYER_B_CU { "selected" } else { "" };
                let sel_pth = if (layer_bits & LAYER_F_CU) != 0 && (layer_bits & LAYER_B_CU) != 0 {
                    "selected"
                } else {
                    ""
                };
                let sel_silk = if (layer_bits & LAYER_F_SILKS) != 0 { "selected" } else { "" };
                let sel_crtyd = if (layer_bits & LAYER_F_CRTYD) != 0 { "selected" } else { "" };
                let sel_edge = if (layer_bits & LAYER_EDGE_CUTS) != 0 { "selected" } else { "" };

                html.push_str(&format!(
                    r##"
                    <div style="border-bottom:1px solid #282c36;">
                      <div style="padding:5px 8px;background:#1d2129;display:flex;align-items:center;justify-content:space-between;">
                        <span style="font-weight:700;color:#79b8ff;font-size:10.5px;">{type_label} (#{idx})</span>
                        <div style="display:flex;gap:4px;">
                          <button id="btn-prop-rotate" class="cad-btn" style="height:19px;padding:0 5px;font-size:10px;">Rot 90° (R)</button>
                          <button id="btn-prop-delete" class="cad-btn cad-btn-danger" style="height:19px;padding:0 5px;font-size:10px;">Del</button>
                        </div>
                      </div>
                      <div class="cad-prop-grid">
                        <span class="cad-prop-label">Layer</span>
                        <select id="prop-layer" class="cad-select" style="width:100%;">
                          <option value="1" {sel_fcu}>F.Cu</option>
                          <option value="4096" {sel_in1}>In1.Cu</option>
                          <option value="8192" {sel_in2}>In2.Cu</option>
                          <option value="2" {sel_bcu}>B.Cu</option>
                          <option value="3" {sel_pth}>F.Cu + B.Cu (PTH/Via)</option>
                          <option value="8" {sel_silk}>F.Silkscreen</option>
                          <option value="65536" {sel_crtyd}>F.Courtyard</option>
                          <option value="4" {sel_edge}>Edge.Cuts</option>
                        </select>
                        <span class="cad-prop-label">Net ID</span>
                        <input id="prop-net" type="number" step="1" min="0" value="{net_id}" class="cad-input" />
                        <span class="cad-prop-label">Start X (mm)</span>
                        <input id="prop-x0" type="number" step="0.05" value="{x0:.3}" class="cad-input" />
                        <span class="cad-prop-label">Start Y (mm)</span>
                        <input id="prop-y0" type="number" step="0.05" value="{y0:.3}" class="cad-input" />
                        <span class="cad-prop-label">End X (mm)</span>
                        <input id="prop-x1" type="number" step="0.05" value="{x1:.3}" class="cad-input" />
                        <span class="cad-prop-label">End Y (mm)</span>
                        <input id="prop-y1" type="number" step="0.05" value="{y1:.3}" class="cad-input" />
                        <span class="cad-prop-label">Width / Ø</span>
                        <input id="prop-w" type="number" step="0.02" min="0.05" value="{w:.3}" class="cad-input" />
                        <span class="cad-prop-label">Height/Drill</span>
                        <input id="prop-h" type="number" step="0.02" min="0.05" value="{h:.3}" class="cad-input" />
                      </div>
                      <div style="padding:0 8px 6px 8px;">
                        <button id="btn-apply-props" class="cad-btn cad-btn-active" style="width:100%;justify-content:center;">Apply Properties</button>
                      </div>
                    </div>
                    "##,
                    type_label = type_label,
                    idx = idx,
                    sel_fcu = sel_fcu,
                    sel_in1 = sel_in1,
                    sel_in2 = sel_in2,
                    sel_bcu = sel_bcu,
                    sel_pth = sel_pth,
                    sel_silk = sel_silk,
                    sel_crtyd = sel_crtyd,
                    sel_edge = sel_edge,
                    net_id = net_id,
                    x0 = x0,
                    y0 = y0,
                    x1 = x1,
                    y1 = y1,
                    w = w,
                    h = h
                ));
            }
        }

        let active_net_id = self
            .renderer
            .as_ref()
            .map(|r| r.selected_net_id)
            .unwrap_or(0);
        if active_net_id > 0 {
            let nid = active_net_id as u32;
            let stats = self
                .current_board
                .nets
                .get(&nid)
                .cloned()
                .unwrap_or_default();
            let name = if stats.name.is_empty() {
                format!("Net #{}", nid)
            } else {
                stats.name
            };
            let len_mils = stats.total_length_mm * 39.3701;
            let delay_ps = stats.total_length_mm * 6.67;

            html.push_str(&format!(
                r##"
                <div style="padding:6px 8px;border-bottom:1px solid #282c36;background:#14181f;">
                  <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:4px;">
                    <span style="font-weight:700;color:#34d399;font-family:monospace;font-size:10.5px;">NET #{nid}: {name}</span>
                    <button id="btn-clear-net-insp" class="cad-btn" style="height:17px;padding:0 5px;font-size:9.5px;">Clear</button>
                  </div>
                  <div style="display:grid;grid-template-columns:1fr 1fr;gap:2px 6px;font-family:monospace;font-size:10.5px;">
                    <span style="color:#8b93a6;">Routed Length:</span>
                    <span style="color:#f0f4f8;text-align:right;font-weight:600;">{len_mm:.3} mm ({len_mils:.1} mil)</span>
                    <span style="color:#8b93a6;">FR4 Delay (Td):</span>
                    <span style="color:#60a5fa;text-align:right;">{delay_ps:.1} ps</span>
                    <span style="color:#8b93a6;">Segs / Vias / Pads:</span>
                    <span style="color:#d0d6e0;text-align:right;">{segs}s / {vias}v / {pads}p</span>
                  </div>
                </div>
                "##,
                nid = nid,
                name = name,
                len_mm = stats.total_length_mm,
                len_mils = len_mils,
                delay_ps = delay_ps,
                segs = stats.segment_count,
                vias = stats.via_count,
                pads = stats.pad_count
            ));
        }

        container.set_inner_html(&html);
    }

    pub fn update_telemetry_hud(&self) {
        let b = &self.current_board;
        if let Some(sum_el) = self.doc.get_element_by_id("kicad-summary-bar") {
            let zones_cnt = b
                .zones
                .iter()
                .filter(|z| !z.is_teardrop)
                .count();
            let teardrops_cnt = b.zones.iter().filter(|z| z.is_teardrop).count();
            sum_el.set_inner_html(&format!(
                r#"<span>Pads: <b style="color:#e2e8f0;">{pads}</b></span>
                   <span>Vias: <b style="color:#e2e8f0;">{vias}</b></span>
                   <span>Track Segments: <b style="color:#e2e8f0;">{tracks}</b></span>
                   <span>Copper Zones: <b style="color:#e2e8f0;">{zones}</b> (+{td} teardrops)</span>
                   <span>Nets: <b style="color:#e2e8f0;">{nets}</b></span>
                   <span>DRC Violations: <b style="color:#f59e0b;">{drc}</b></span>"#,
                pads = b.total_pads,
                vias = b.total_vias,
                tracks = b.total_tracks,
                zones = zones_cnt,
                td = teardrops_cnt,
                nets = b.nets.len(),
                drc = self.drc_items.len(),
            ));
        }

        let scale = self
            .renderer
            .as_ref()
            .map(|r| r.scale_px_per_mm)
            .unwrap_or(8.0);
        if let Some(z_el) = self.doc.get_element_by_id("rust-zoom-hud") {
            if self.active_tab == "schematic" {
                z_el.set_text_content(Some(&format!("Z {:.2}", self.sch_zoom * 10.0)));
            } else {
                z_el.set_text_content(Some(&format!("Z {:.1}", scale)));
            }
        }

        let (cx, cy) = self.cursor_snapped_mm;
        let (ox, oy) = self.rel_origin_mm;
        let dx = cx - ox;
        let dy = cy - oy;
        let dist = (dx * dx + dy * dy).sqrt();

        if let Some(c_el) = self.doc.get_element_by_id("rust-coord-hud") {
            if self.polar_coords {
                let ang = dy.atan2(dx).to_degrees();
                c_el.set_text_content(Some(&format!(
                    "R {} {}   θ {:+.1}°",
                    self.format_unit(dist),
                    self.unit_suffix(),
                    ang
                )));
            } else {
                c_el.set_text_content(Some(&format!(
                    "X {}   Y {}",
                    self.format_unit(cx),
                    self.format_unit(cy)
                )));
            }
        }

        if let Some(rel_el) = self.doc.get_element_by_id("rust-rel-coord-hud") {
            rel_el.set_text_content(Some(&format!(
                "dx {}   dy {}   dist {}",
                self.format_unit(dx),
                self.format_unit(dy),
                self.format_unit(dist)
            )));
        }

        if let Some(g_el) = self.doc.get_element_by_id("rust-grid-hud") {
            g_el.set_text_content(Some(&format!(
                "grid X {} Y {}   {}",
                self.format_unit(self.grid_snap_mm),
                self.format_unit(self.grid_snap_mm),
                self.unit_suffix()
            )));
        }

        if let Some(el) = self.doc.get_element_by_id("rust-hud-stats") {
            let prims = self.current_board.primitive_count;
            let mode_badge = self
                .renderer
                .as_ref()
                .map(|r| {
                    if r.mode_3d_explode > 0.05 {
                        "3D Stackup"
                    } else if r.flip_board_view {
                        "2D Flipped"
                    } else {
                        "2D Ortho"
                    }
                })
                .unwrap_or("2D Ortho");
            el.set_text_content(Some(&format!(
                "{} | {} prims ({:.1} ms)",
                mode_badge, prims, self.parse_time_ms
            )));
        }
    }
}

fn default_parts_catalog() -> Vec<PartLibraryEntry> {
    vec![
        PartLibraryEntry {
            mpn: "BCM2712-CM5-MINIMA".to_string(),
            manufacturer: "Raspberry Pi Ltd".to_string(),
            description: "Compute Module 5 High-Speed B2B Carrier Interface".to_string(),
            value: "CM5-Mod".to_string(),
            footprint_name: "Module:RPi_CM5_DF40C_100DS".to_string(),
            symbol_name: "MCU_Module:RPi_CM5".to_string(),
            lifecycle_status: "active".to_string(),
            unit_price_usd: 65.00,
            digikey_stock: 4820,
            mouser_stock: 3150,
        },
        PartLibraryEntry {
            mpn: "STM32H743VIT6".to_string(),
            manufacturer: "STMicroelectronics".to_string(),
            description: "32-bit ARM Cortex-M7 MCU 480MHz 2MB Flash LQFP-100".to_string(),
            value: "480MHz".to_string(),
            footprint_name: "Package_QFP:LQFP-100_14x14mm_P0.5mm".to_string(),
            symbol_name: "MCU_ST_STM32H7:STM32H743VITx".to_string(),
            lifecycle_status: "active".to_string(),
            unit_price_usd: 9.42,
            digikey_stock: 18400,
            mouser_stock: 12910,
        },
        PartLibraryEntry {
            mpn: "TPS62130RGTR".to_string(),
            manufacturer: "Texas Instruments".to_string(),
            description: "3V-17V 3A Step-Down Buck Converter VQFN-16".to_string(),
            value: "3.3V / 3A".to_string(),
            footprint_name: "Package_DFN_QFN:VQFN-16-1EP_3x3mm_P0.5mm".to_string(),
            symbol_name: "Regulator_Switching:TPS62130".to_string(),
            lifecycle_status: "active".to_string(),
            unit_price_usd: 1.85,
            digikey_stock: 64200,
            mouser_stock: 41000,
        },
        PartLibraryEntry {
            mpn: "GRM155R71C104KA88D".to_string(),
            manufacturer: "Murata Electronics".to_string(),
            description: "MLCC 100nF 16V X7R 0402 Decoupling Capacitor".to_string(),
            value: "100nF".to_string(),
            footprint_name: "Capacitor_SMD:C_0402_1005Metric".to_string(),
            symbol_name: "Device:C_Small".to_string(),
            lifecycle_status: "active".to_string(),
            unit_price_usd: 0.008,
            digikey_stock: 1250000,
            mouser_stock: 980000,
        },
        PartLibraryEntry {
            mpn: "TUSB8041RGCR".to_string(),
            manufacturer: "Texas Instruments".to_string(),
            description: "4-Port USB 3.1 Gen1 SuperSpeed 5Gbps Hub Controller".to_string(),
            value: "USB3-Hub".to_string(),
            footprint_name: "Package_DFN_QFN:VQFN-64-1EP_9x9mm_P0.5mm".to_string(),
            symbol_name: "Interface_USB:TUSB8041".to_string(),
            lifecycle_status: "nrnd".to_string(),
            unit_price_usd: 4.60,
            digikey_stock: 1420,
            mouser_stock: 610,
        },
        PartLibraryEntry {
            mpn: "LAN7800-I/Y9X".to_string(),
            manufacturer: "Microchip Technology".to_string(),
            description: "USB 3.1 Gen1 to 10/100/1000 Gigabit Ethernet Bridge".to_string(),
            value: "GbE-PHY".to_string(),
            footprint_name: "Package_DFN_QFN:QFN-48-1EP_6x6mm_P0.4mm".to_string(),
            symbol_name: "Interface_Ethernet:LAN7800".to_string(),
            lifecycle_status: "active".to_string(),
            unit_price_usd: 3.95,
            digikey_stock: 9320,
            mouser_stock: 7400,
        },
        PartLibraryEntry {
            mpn: "SI5351A-B-GT".to_string(),
            manufacturer: "Skyworks / Silicon Labs".to_string(),
            description: "I2C Programmable Any-Frequency CMOS Clock Generator".to_string(),
            value: "25MHz-PLL".to_string(),
            footprint_name: "Package_SO:MSOP-10_3x3mm_P0.5mm".to_string(),
            symbol_name: "Oscillator:Si5351A".to_string(),
            lifecycle_status: "obsolete".to_string(),
            unit_price_usd: 2.75,
            digikey_stock: 0,
            mouser_stock: 145,
        },
        PartLibraryEntry {
            mpn: "DF40C-100DS-0.4V(51)".to_string(),
            manufacturer: "Hirose Electric".to_string(),
            description: "100-Pos 0.4mm Pitch High-Density Mezzanine Receptacle".to_string(),
            value: "100P-B2B".to_string(),
            footprint_name: "Connector_Hirose:Hirose_DF40C_100DS_0.4mm".to_string(),
            symbol_name: "Connector_Generic:Conn_02x50".to_string(),
            lifecycle_status: "active".to_string(),
            unit_price_usd: 2.18,
            digikey_stock: 28900,
            mouser_stock: 19500,
        },
    ]
}

#[wasm_bindgen(start)]
pub fn start_cadoxide_app() {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let doc = match window.document() {
        Some(d) => d,
        None => return,
    };
    let root = match doc.get_element_by_id("root") {
        Some(r) => r,
        None => return,
    };

    let empty_board = ParsedBoard::empty();

    let state = Rc::new(RefCell::new(AppController {
        doc: doc.clone(),
        renderer: None,
        active_tab: "pcb".to_string(),
        appear_subtab: "layers".to_string(),
        active_project: "complex_hierarchy".to_string(),
        board_rev_a: empty_board.clone(),
        board_rev_b: empty_board.clone(),
        board_diff: empty_board.clone(),
        diff_stats: (0, 0, 0),
        current_board: empty_board,
        undo_stack: Vec::new(),
        parse_time_ms: 0.0,
        is_dragging: false,
        drag_button: 0,
        is_dragging_primitive: false,
        drag_prim_last_mm: (0.0, 0.0),
        last_mouse: (0.0, 0.0),
        mouse_down_pos: (0.0, 0.0),
        drag_3d_pan_mode: false,
        active_tool: CadTool::SelectMove,
        active_draw_layer: LAYER_F_CU,
        active_track_width_mm: 0.25,
        active_via_size_mm: 0.60,
        active_via_drill_mm: 0.30,
        grid_snap_mm: 0.25,
        selected_primitive_idx: None,
        route_start_mm: None,
        poly_points_mm: Vec::new(),
        route_posture_flip: false,
        cursor_snapped_mm: (130.0, 95.0),
        rel_origin_mm: (130.0, 95.0),
        units: DisplayUnits::Mm,
        polar_coords: false,
        show_crosshair: false,
        show_pad_labels: true,
        show_pad_nets: true,
        show_fp_text: true,
        show_ratsnest: false,
        zone_display_mode: 0,
        sel_footprints: true,
        sel_tracks: true,
        sel_vias: true,
        sel_pads: true,
        sel_zones: true,
        sel_graphics: true,
        sch_zoom: 1.0,
        sch_pan: (0.0, 0.0),
        sch_dragging: false,
        sch_last_mouse: (0.0, 0.0),
        drc_items: Vec::new(),
        parts_catalog: default_parts_catalog(),
        parts_filter: String::new(),
    }));

    build_shell_dom(&doc, &root, state.clone());
    fetch_project_boards(state.clone(), "complex_hierarchy");
    sync_backend_parts(state);
}

fn build_shell_dom(doc: &Document, root: &Element, state: Rc<RefCell<AppController>>) {
    root.set_inner_html(
        r##"
        <div class="cad-shell">
          <!-- Row 1: Native KiCad Pcbnew Desktop Menu Bar + Mode Tabs -->
          <div class="cad-topmenu">
            <div class="cad-menu-items">
              <div class="cad-brand">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                  <rect x="1" y="1" width="14" height="14" rx="2" stroke="#c83434" stroke-width="1.5"/>
                  <circle cx="5" cy="5" r="1.5" fill="#79b8ff"/>
                  <circle cx="11" cy="11" r="1.5" fill="#79b8ff"/>
                  <path d="M5 5L8 8H11V11" stroke="#c83434" stroke-width="1.4" stroke-linecap="round"/>
                </svg>
                <span>CADoxide — PCB Editor</span>
              </div>
              <div class="cad-divider-v"></div>
              <button id="menu-file-save" class="cad-menu-btn" title="File → Export .kicad_pcb">File</button>
              <button id="menu-edit-undo" class="cad-menu-btn" title="Edit → Undo (Ctrl+Z)">Edit</button>
              <button id="menu-view-3d" class="cad-menu-btn" title="View → Toggle 3D Viewer (3)">View</button>
              <button id="menu-place-pad" class="cad-menu-btn" title="Place → Footprint / Pad (P)">Place</button>
              <button id="menu-route-single" class="cad-menu-btn" title="Route → Single Track (X)">Route</button>
              <button id="menu-inspect-drc" class="cad-menu-btn" title="Inspect → Design Rules Checker">Inspect</button>
              <button id="menu-tools-fill" class="cad-menu-btn" title="Tools → Fill All Zones (B)">Tools</button>
            </div>

            <!-- Center Workspace Mode Tabs -->
            <div class="cad-tabs">
              <button id="tab-pcb" class="cad-tab cad-tab-active">PCB Editor</button>
              <button id="tab-schematic" class="cad-tab">Schematic Editor</button>
              <button id="tab-diff" class="cad-tab">Layer Diff</button>
              <button id="tab-drc" class="cad-tab">DRC (<span id="drc-count-badge">69</span>)</button>
              <button id="tab-parts" class="cad-tab">BOM (8)</button>
            </div>

            <!-- Right Project Selector -->
            <div class="cad-menubar-group">
              <select id="rust-project-select" class="cad-select" title="Active KiCad PCB File">
                <option value="complex_hierarchy">*complex_hierarchy.kicad_pcb (2-Layer + Copper Pour)</option>
                <option value="cm5_minima">*CM5_MINIMA_3.kicad_pcb (4-Layer RPi CM5 HDI)</option>
              </select>
              <button id="btn-100k-hdi" class="cad-btn" title="Generate 100,000-primitive synthetic HDI stress board">HDI-100K</button>
            </div>
          </div>

          <!-- Row 2: KiCad Auxiliary & Action Toolbar (Track Width, Via Size, Grid, Zoom, Zone Fill, 3D) -->
          <div class="cad-param-bar">
            <div class="cad-menubar-group">
              <a id="btn-export-pcb" href="#" download="edited_board.kicad_pcb" class="cad-btn" title="Save / Download .kicad_pcb">Save .kicad_pcb</a>
              <a id="link-3d-vrml" href="/samples/complex_hierarchy/board.wrl" download class="cad-btn" title="Export 3D VRML (.wrl)">Export .wrl</a>
              <div class="cad-divider-v"></div>
              <button id="btn-undo" class="cad-btn" title="Undo (Ctrl+Z)">↶ Undo</button>
              <button id="btn-rotate-selected" class="cad-btn" title="Rotate Selected 90° (R)">↻ Rot 90°</button>
              <button id="btn-delete-selected" class="cad-btn cad-btn-danger" title="Delete Selected Item (Del)">✕ Delete</button>
              <div class="cad-divider-v"></div>
              <button id="btn-zoom-in" class="cad-btn" title="Zoom In (+)">Zoom +</button>
              <button id="btn-zoom-out" class="cad-btn" title="Zoom Out (-)">Zoom −</button>
              <button id="btn-zoom-fit" class="cad-btn" title="Zoom to Fit Board (Home)">Fit Board</button>
              <div class="cad-divider-v"></div>

              <span class="cad-param-label">Layer:</span>
              <select id="select-draw-layer" class="cad-select" title="Active Drawing / Routing Layer">
                <option value="1">F.Cu (Front Copper)</option>
                <option value="4096">In1.Cu (Inner 1)</option>
                <option value="8192">In2.Cu (Inner 2)</option>
                <option value="2">B.Cu (Back Copper)</option>
                <option value="16384">F.Paste</option>
                <option value="32768">B.Paste</option>
                <option value="8">F.Silkscreen</option>
                <option value="16">B.Silkscreen</option>
                <option value="32">F.Mask</option>
                <option value="64">B.Mask</option>
                <option value="1048576">User.Drawings</option>
                <option value="2097152">User.Comments</option>
                <option value="4">Edge.Cuts</option>
                <option value="4194304">Margin</option>
                <option value="65536">F.Courtyard</option>
                <option value="131072">B.Courtyard</option>
                <option value="262144">F.Fab</option>
                <option value="524288">B.Fab</option>
              </select>

              <span class="cad-param-label">Track:</span>
              <select id="select-track-width" class="cad-select" title="Pre-defined Track Width">
                <option value="0.127">Track: 0.127 mm (5 mil)</option>
                <option value="0.200">Track: 0.200 mm (8 mil)</option>
                <option value="0.250" selected>Track: 0.250 mm (10 mil)</option>
                <option value="0.400">Track: 0.400 mm (16 mil)</option>
                <option value="0.800">Track: 0.800 mm (31 mil)</option>
              </select>

              <span class="cad-param-label">Via:</span>
              <select id="select-via-size" class="cad-select" title="Pre-defined Via Size / Drill">
                <option value="0.45,0.20">Via: 0.45 / 0.20 mm</option>
                <option value="0.60,0.30" selected>Via: 0.60 / 0.30 mm</option>
                <option value="0.80,0.40">Via: 0.80 / 0.40 mm</option>
              </select>

              <span class="cad-param-label">Grid:</span>
              <select id="select-grid-snap" class="cad-select" title="Grid Step Size">
                <option value="0.05">Grid: 0.05 mm (2 mil)</option>
                <option value="0.10">Grid: 0.10 mm (4 mil)</option>
                <option value="0.25" selected>Grid: 0.25 mm (10 mil)</option>
                <option value="0.50">Grid: 0.50 mm (20 mil)</option>
                <option value="1.00">Grid: 1.00 mm (39.4 mil)</option>
              </select>
            </div>

            <!-- Contextual 3D Stackup Controls -->
            <div id="toolbar-3d-controls" class="hidden cad-menubar-group">
              <div class="cad-divider-v"></div>
              <button id="btn-3d-drag-mode" class="cad-btn cad-btn-active" title="Toggle Left-Drag between 3D Orbit and 3D Pan">Left-Drag: Orbit 3D</button>
              <button id="btn-3d-iso" class="cad-btn">Iso</button>
              <button id="btn-3d-top" class="cad-btn">Top</button>
              <button id="btn-3d-flip" class="cad-btn">Flip</button>
              <input id="input-3d-explode" type="range" min="20" max="250" value="100" style="width:60px;accent-color:#388bfd;cursor:pointer;" />
            </div>

            <!-- Contextual Diff Controls -->
            <div id="diff-toolbar-controls" class="hidden cad-menubar-group">
              <div class="cad-divider-v"></div>
              <button id="btn-diff-boolean" class="cad-btn cad-btn-active">Overlay</button>
              <button id="btn-diff-wipe" class="cad-btn">Split Wipe</button>
              <input id="input-wipe-slider" type="range" min="0" max="100" value="50" style="width:70px;accent-color:#388bfd;cursor:pointer;" />
            </div>

            <div class="cad-menubar-group">
              <select id="rust-net-select" class="cad-select" style="width:170px;" title="Highlight electrical net across all layers">
                <option value="0">Net Filter: All Nets</option>
              </select>
              <button id="btn-3d-stackup" class="cad-btn" title="Toggle 3D Multi-Layer Stackup Viewer (3)">3D Viewer (3)</button>
            </div>
          </div>

          <!-- Main CAD Workspace (Left View Options Strip + Center Canvas + Right Routing/Drawing Strip + Right Appearance Dock) -->
          <div class="cad-workspace">
            <!-- Left Vertical Strip: KiCad Display & Viewing Options -->
            <aside class="cad-toolstrip" title="KiCad Display & Viewing Options">
              <button id="btn-opt-grid" class="cad-tool-btn cad-tool-btn-active" title="Show/Hide Grid">#</button>
              <button id="btn-opt-polar" class="cad-tool-btn" title="Toggle Polar Coordinates (R / θ)">∠</button>
              <div class="cad-tool-sep"></div>
              <button id="btn-unit-in" class="cad-tool-btn" title="Set Units to Inches (in)">in</button>
              <button id="btn-unit-mil" class="cad-tool-btn" title="Set Units to Mils (mil)">mil</button>
              <button id="btn-unit-mm" class="cad-tool-btn cad-tool-btn-active" title="Set Units to Millimeters (mm)">mm</button>
              <div class="cad-tool-sep"></div>
              <button id="btn-opt-crosshair" class="cad-tool-btn" title="Toggle Full-Window Crosshair Cursor">┼</button>
              <button id="btn-opt-pad-labels" class="cad-tool-btn cad-tool-btn-active" title="Show/Hide On-Pad Pin Numbers & Net Names">1/N</button>
              <button id="btn-opt-fp-text" class="cad-tool-btn cad-tool-btn-active" title="Show/Hide Footprint Reference & Value Labels">Ref</button>
              <button id="btn-opt-sketch" class="cad-tool-btn" title="Toggle Sketch / Outline Mode for Tracks & Pads">▱</button>
              <div class="cad-tool-sep"></div>
              <button id="btn-zone-filled" class="cad-tool-btn cad-tool-btn-active" title="Show Filled Areas of Zones">■Z</button>
              <button id="btn-zone-outline" class="cad-tool-btn" title="Show Only Zone Boundaries (Outlines)">□Z</button>
              <button id="btn-zone-off" class="cad-tool-btn" title="Do Not Show Copper Zones">×Z</button>
              <div class="cad-tool-sep"></div>
              <button id="btn-opt-ratsnest" class="cad-tool-btn" title="Show/Hide Ratsnest Airwires">⧖</button>
              <button id="btn-opt-highcontrast" class="cad-tool-btn" title="Cycle High-Contrast Mode (H): Normal / Dim / Hide Inactive">◐</button>
              <button id="btn-opt-flipview" class="cad-tool-btn" title="Flip Board View Horizontally (F)">⇄</button>
            </aside>

            <!-- Center GPU PCB Canvas + Synchronized 2D CAD Overlay Canvas -->
            <div id="center-viewport" class="cad-viewport">
              <canvas id="rust-gpu-canvas" class="cad-canvas"></canvas>
              <canvas id="cad-overlay-canvas" class="cad-overlay-canvas"></canvas>

              <!-- Interactive Pan & Wheel-Zoom Schematic Editor View -->
              <div id="schematic-view-panel" class="hidden" style="position:absolute;inset:0;display:flex;flex-direction:column;background:#0e1013;z-index:30;">
                <div style="height:28px;background:#16181d;border-bottom:1px solid #262a33;display:flex;align-items:center;justify-content:space-between;padding:0 10px;">
                  <div style="display:flex;align-items:center;gap:6px;">
                    <span class="cad-param-label">Schematic Sheet:</span>
                    <button data-sch="/samples/complex_hierarchy/complex_hierarchy.svg" class="sch-btn cad-btn cad-btn-active">Root Sheet</button>
                    <button data-sch="/samples/complex_hierarchy/complex_hierarchy-ampli_ht_horizontal.svg" class="sch-btn cad-btn">ampli_ht_horizontal.kicad_sch</button>
                    <button data-sch="/samples/complex_hierarchy/complex_hierarchy-ampli_ht_vertical.svg" class="sch-btn cad-btn">ampli_ht_vertical.kicad_sch</button>
                  </div>
                  <div style="display:flex;align-items:center;gap:5px;">
                    <span style="font-size:11px;color:#8e96a8;">Wheel to Zoom, Drag to Pan</span>
                    <button id="btn-sch-reset" class="cad-btn">Reset Zoom (100%)</button>
                  </div>
                </div>
                <div id="schematic-viewport" style="flex:1;overflow:hidden;position:relative;background:#f8fafc;cursor:grab;display:flex;align-items:center;justify-content:center;">
                  <div id="schematic-canvas-inner" style="transform-origin:center center;transition:none;display:flex;align-items:center;justify-content:center;">
                    <img id="schematic-svg-img" src="/samples/complex_hierarchy/complex_hierarchy.svg" draggable="false" style="max-width:92vw;max-height:82vh;pointer-events:none;" />
                  </div>
                </div>
              </div>

              <!-- DRC Audit Table View -->
              <div id="drc-view-panel" class="hidden" style="position:absolute;inset:0;display:flex;flex-direction:column;background:#121418;z-index:30;">
                <div class="cad-panel-header" style="height:28px;padding:0 14px;">
                  <span>DESIGN RULE CHECK (DRC) VIOLATIONS</span>
                  <span style="color:#8e96a8;font-weight:400;">Click any violation row to center &amp; zoom PCB viewport</span>
                </div>
                <div id="drc-list-container" style="flex:1;overflow-y:auto;"></div>
              </div>

              <!-- BOM & Component Library View -->
              <div id="parts-view-panel" class="hidden" style="position:absolute;inset:0;display:flex;flex-direction:column;background:#121418;z-index:30;">
                <div class="cad-panel-header" style="height:32px;padding:0 14px;">
                  <span>BILL OF MATERIALS &amp; COMPONENT LIBRARY</span>
                  <input id="input-parts-search" type="text" placeholder="Filter MPN, manufacturer, footprint..." class="cad-select" style="width:240px;" />
                </div>
                <div id="parts-list-container" style="flex:1;overflow-y:auto;"></div>
              </div>
            </div>

            <!-- Right Vertical Strip: KiCad Interactive CAD Routing, Placement & Drawing Tools (Docked left of Appearance Panel) -->
            <aside class="cad-toolstrip-right" title="KiCad Routing, Placement & Drawing Tools">
              <button id="btn-tool-select" class="cad-tool-btn cad-tool-btn-active" title="Select item(s) (S)">
                <svg width="13" height="13" viewBox="0 0 16 16" fill="currentColor"><path d="M3 2L13 7.5L8.5 9L7 13.5L3 2Z"/></svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-tool-route" class="cad-tool-btn" title="Route single track (X) — Press / to flip 45° posture">
                <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><path d="M2.5 13.5L6.5 9.5H10.5L13.5 6.5"/><circle cx="2.5" cy="13.5" r="1.4" fill="currentColor"/><circle cx="13.5" cy="6.5" r="1.4" fill="currentColor"/></svg>
              </button>
              <button id="btn-tool-diffpair" class="cad-tool-btn" title="Route differential pair (6)">DP</button>
              <button id="btn-tool-tune" class="cad-tool-btn" title="Tune length of a single track / Serpentine meander (7)">∿</button>
              <button id="btn-tool-via" class="cad-tool-btn" title="Add freestanding vias (V)">
                <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="8" r="5.2"/><circle cx="8" cy="8" r="2.1"/></svg>
              </button>
              <button id="btn-tool-pad" class="cad-tool-btn" title="Add footprint / SMD pad (P)">
                <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="3" y="4.5" width="10" height="7" rx="1.5"/></svg>
              </button>
              <button id="btn-tool-zone" class="cad-tool-btn" title="Add filled copper zone (Z) — Click polygon corners, Right-Click to close">▤</button>
              <button id="btn-tool-keepout" class="cad-tool-btn" title="Add rule area / keepout zone (K)">⊘</button>
              <div class="cad-tool-sep"></div>
              <button id="btn-tool-line" class="cad-tool-btn" title="Draw a line (L)">╱</button>
              <button id="btn-tool-arc" class="cad-tool-btn" title="Draw an arc (A)">⌒</button>
              <button id="btn-tool-circle" class="cad-tool-btn" title="Draw a circle (C)">◯</button>
              <button id="btn-tool-rect" class="cad-tool-btn" title="Draw a rectangle">▭</button>
              <button id="btn-tool-poly" class="cad-tool-btn" title="Draw a graphic polygon (Y)">⬠</button>
              <button id="btn-tool-text" class="cad-tool-btn" title="Add a text item (T)">T</button>
              <button id="btn-tool-dim" class="cad-tool-btn" title="Add aligned linear dimension (D)">↔</button>
              <div class="cad-tool-sep"></div>
              <button id="btn-tool-origin" class="cad-tool-btn" title="Place grid/drill origin point (O)">⊕</button>
              <button id="btn-tool-measure" class="cad-tool-btn" title="Measure between two points (M)">📏</button>
            </aside>

            <!-- Right Docked KiCad Appearance Manager, Layer Display Options, Selection Filter & Inspector -->
            <aside class="cad-sidebar">
              <div class="cad-panel-header">
                <span>Appearance</span>
                <span style="color:#79b8ff;font-weight:500;">KiCad Stack</span>
              </div>
              <div class="cad-appear-tabs">
                <button id="appear-tab-layers" class="cad-appear-tab cad-appear-tab-active">Layers</button>
                <button id="appear-tab-objects" class="cad-appear-tab">Objects</button>
                <button id="appear-tab-nets" class="cad-appear-tab">Nets</button>
              </div>

              <!-- Sub-Tab 1: All 18 KiCad Layers -->
              <div id="appear-layers-content" class="cad-layer-list">
                <div class="cad-layer-row cad-layer-row-active" data-layer-bit="1">
                  <span class="cad-layer-left"><span class="cad-active-arrow">▶</span><input type="checkbox" data-bit="1" checked /><span class="cad-swatch" style="background:#C83434;"></span><span>F.Cu</span></span>
                  <span class="cad-layer-type">Front Copper</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="4096">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="4096" checked /><span class="cad-swatch" style="background:#C2B238;"></span><span>In1.Cu</span></span>
                  <span class="cad-layer-type">Inner 1</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="8192">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="8192" checked /><span class="cad-swatch" style="background:#D47B28;"></span><span>In2.Cu</span></span>
                  <span class="cad-layer-type">Inner 2</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="2">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="2" checked /><span class="cad-swatch" style="background:#2878C8;"></span><span>B.Cu</span></span>
                  <span class="cad-layer-type">Back Copper</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="16384">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="16384" checked /><span class="cad-swatch" style="background:#9E8282;"></span><span>F.Paste</span></span>
                  <span class="cad-layer-type">Front Paste</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="32768">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="32768" checked /><span class="cad-swatch" style="background:#6E9E9E;"></span><span>B.Paste</span></span>
                  <span class="cad-layer-type">Back Paste</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="8">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="8" checked /><span class="cad-swatch" style="background:#F2EDA1;"></span><span>F.Silkscreen</span></span>
                  <span class="cad-layer-type">Front Silk</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="16">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="16" checked /><span class="cad-swatch" style="background:#E8B2A6;"></span><span>B.Silkscreen</span></span>
                  <span class="cad-layer-type">Back Silk</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="32">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="32" checked /><span class="cad-swatch" style="background:#6B285C;"></span><span>F.Mask</span></span>
                  <span class="cad-layer-type">Front Mask</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="64">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="64" checked /><span class="cad-swatch" style="background:#1E6E64;"></span><span>B.Mask</span></span>
                  <span class="cad-layer-type">Back Mask</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="1048576">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="1048576" checked /><span class="cad-swatch" style="background:#94A2B0;"></span><span>User.Drawings</span></span>
                  <span class="cad-layer-type">Drawings</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="2097152">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="2097152" checked /><span class="cad-swatch" style="background:#5C94D8;"></span><span>User.Comments</span></span>
                  <span class="cad-layer-type">Comments</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="4">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="4" checked /><span class="cad-swatch" style="background:#D8C238;"></span><span>Edge.Cuts</span></span>
                  <span class="cad-layer-type">Outline</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="4194304">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="4194304" checked /><span class="cad-swatch" style="background:#D838B8;"></span><span>Margin</span></span>
                  <span class="cad-layer-type">Margin</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="65536">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="65536" checked /><span class="cad-swatch" style="background:#E038D8;"></span><span>F.Courtyard</span></span>
                  <span class="cad-layer-type">Front CrtYd</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="131072">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="131072" checked /><span class="cad-swatch" style="background:#28C8E0;"></span><span>B.Courtyard</span></span>
                  <span class="cad-layer-type">Back CrtYd</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="262144">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="262144" checked /><span class="cad-swatch" style="background:#888E96;"></span><span>F.Fab</span></span>
                  <span class="cad-layer-type">Front Fab</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="524288">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="524288" checked /><span class="cad-swatch" style="background:#586278;"></span><span>B.Fab</span></span>
                  <span class="cad-layer-type">Back Fab</span>
                </div>
                <div class="cad-layer-row" data-layer-bit="128">
                  <span class="cad-layer-left"><span class="cad-active-arrow"></span><input type="checkbox" data-bit="128" checked /><span class="cad-swatch" style="background:#F59E0B;"></span><span>DRC Markers</span></span>
                  <span class="cad-layer-type">Violations</span>
                </div>
              </div>

              <!-- Sub-Tab 2: Objects Visibility & Opacity Sliders -->
              <div id="appear-objects-content" class="hidden cad-layer-list" style="padding:6px 8px;">
                <label class="cad-layer-row"><span class="cad-layer-left"><input id="obj-chk-pad-labels" type="checkbox" checked /><span>Pad Numbers</span></span><span class="cad-layer-type">Pin #</span></label>
                <label class="cad-layer-row"><span class="cad-layer-left"><input id="obj-chk-pad-nets" type="checkbox" checked /><span>Pad Net Names</span></span><span class="cad-layer-type">Net Label</span></label>
                <label class="cad-layer-row"><span class="cad-layer-left"><input id="obj-chk-fp-text" type="checkbox" checked /><span>Footprint Text</span></span><span class="cad-layer-type">Ref / Val</span></label>
                <label class="cad-layer-row"><span class="cad-layer-left"><input id="obj-chk-ratsnest" type="checkbox" /><span>Ratsnest Airwires</span></span><span class="cad-layer-type">Unrouted</span></label>
                <label class="cad-layer-row"><span class="cad-layer-left"><input id="obj-chk-grid" type="checkbox" checked /><span>Canvas Grid</span></span><span class="cad-layer-type">Metric</span></label>
              </div>

              <!-- Sub-Tab 3: Nets List -->
              <div id="appear-nets-content" class="hidden cad-layer-list"></div>

              <!-- KiCad Layer Display Options -->
              <div class="cad-panel-header">
                <span>Layer Display Options</span>
                <span>H / F</span>
              </div>
              <div class="cad-display-opts">
                <div style="margin-bottom:4px;display:flex;align-items:center;justify-content:space-between;">
                  <span style="font-size:10px;color:#8e96a8;">Inactive layers (H):</span>
                  <div style="display:flex;gap:8px;">
                    <label style="display:flex;align-items:center;gap:3px;cursor:pointer;"><input type="radio" name="hc-mode" value="0" checked />Normal</label>
                    <label style="display:flex;align-items:center;gap:3px;cursor:pointer;"><input type="radio" name="hc-mode" value="1" />Dim</label>
                    <label style="display:flex;align-items:center;gap:3px;cursor:pointer;"><input type="radio" name="hc-mode" value="2" />Hide</label>
                  </div>
                </div>
                <div style="display:flex;align-items:center;justify-content:space-between;gap:6px;">
                  <label style="display:flex;align-items:center;gap:4px;cursor:pointer;"><input id="chk-flip-board" type="checkbox" />Flip board view (F)</label>
                  <select id="select-layer-preset" class="cad-select" style="width:118px;">
                    <option value="all">Preset: All Layers</option>
                    <option value="front">Front Layers Only</option>
                    <option value="back">Back Layers Only</option>
                    <option value="copper">Copper Layers Only</option>
                    <option value="fab">Fab &amp; Courtyards</option>
                  </select>
                </div>
              </div>

              <!-- KiCad Selection Filter -->
              <div class="cad-panel-header">
                <span>Selection Filter</span>
                <span>Hit Mask</span>
              </div>
              <div id="selection-filter-box" class="cad-filter-grid">
                <label class="cad-filter-item"><input id="sel-chk-all" type="checkbox" checked /><b>All items</b></label>
                <label class="cad-filter-item"><input id="sel-chk-fp" type="checkbox" checked />Footprints</label>
                <label class="cad-filter-item"><input id="sel-chk-tracks" type="checkbox" checked />Tracks</label>
                <label class="cad-filter-item"><input id="sel-chk-vias" type="checkbox" checked />Vias</label>
                <label class="cad-filter-item"><input id="sel-chk-pads" type="checkbox" checked />Pads</label>
                <label class="cad-filter-item"><input id="sel-chk-zones" type="checkbox" checked />Zones</label>
                <label class="cad-filter-item"><input id="sel-chk-gfx" type="checkbox" checked />Graphics</label>
                <label class="cad-filter-item"><input type="checkbox" checked />Text</label>
                <label class="cad-filter-item"><input type="checkbox" checked />Keepouts</label>
              </div>

              <!-- Inspector & Selected Object Properties -->
              <div class="cad-panel-header">
                <span>Inspector &amp; Properties</span>
                <span id="active-tool-badge" style="color:#79b8ff;font-weight:600;text-transform:none;">Select item(s) (S)</span>
              </div>
              <div id="cad-inspector-content" style="flex:1;overflow-y:auto;"></div>
            </aside>
          </div>

          <!-- Bottom 2-Row KiCad Pcbnew Status Bar -->
          <footer class="cad-statusbar-wrap">
            <div id="kicad-summary-bar" class="cad-statusbar-summary">
              <span>Pads: <b>0</b></span>
              <span>Vias: <b>0</b></span>
              <span>Track Segments: <b>0</b></span>
              <span>Copper Zones: <b>0</b></span>
              <span>Nets: <b>0</b></span>
            </div>
            <div class="cad-statusbar">
              <div id="rust-zoom-hud" class="cad-status-cell" style="width:74px;color:#e5e9f0;">Z 8.0</div>
              <div id="rust-coord-hud" class="cad-status-cell" style="width:185px;color:#e5e9f0;">X 130.0000   Y 95.0000</div>
              <div id="rust-rel-coord-hud" class="cad-status-cell" style="width:255px;color:#9aa2b2;" title="Press Spacebar to zero relative origin (dx/dy/dist)">dx 0.0000   dy 0.0000   dist 0.0000</div>
              <div id="rust-grid-hud" class="cad-status-cell" style="width:155px;color:#9aa2b2;">grid X 0.2500 Y 0.2500 mm</div>
              <div id="rust-hover-inspector" class="cad-status-cell cad-status-cell-flex" style="color:#79b8ff;">
                Ready — Press Spacebar to zero dx/dy, X to Route, Z for Zone, 7 for Length Tune, H for High-Contrast, F to Flip
              </div>
              <div id="rust-hud-stats" class="cad-status-cell" style="border-right:none;color:#949eb0;">2D Ortho | 0 prims</div>
            </div>
          </footer>
        </div>
        "##,
    );

    if let Some(canvas_el) = doc.get_element_by_id("rust-gpu-canvas") {
        if let Ok(canvas) = canvas_el.dyn_into::<HtmlCanvasElement>() {
            if let Ok(renderer) = RustGpuRenderer::new(canvas.clone()) {
                state.borrow_mut().renderer = Some(renderer);
                bind_canvas_interactions(&canvas, state.clone());
            }
        }
    }

    bind_toolbar_and_tabs(doc, state.clone());
    bind_kicad_display_and_appearance(doc, state.clone());
    bind_schematic_interactions(doc, state.clone());
    bind_keyboard_shortcuts(doc, state.clone());
    state.borrow().render_inspector_panel();
    render_parts_list(&state);
}

fn bind_canvas_interactions(canvas: &HtmlCanvasElement, state: Rc<RefCell<AppController>>) {
    // Right-Click closes polygon/zone or cancels route; in 3D it pans
    {
        let st = state.clone();
        let on_ctx = Closure::wrap(Box::new(move |e: MouseEvent| {
            e.prevent_default();
            let mut app = st.borrow_mut();
            if !app.poly_points_mm.is_empty() {
                app.finish_polygon_or_zone();
            } else if app.route_start_mm.is_some() {
                app.route_start_mm = None;
                app.rebuild_gpu_buffer_with_markers();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = canvas
            .add_event_listener_with_callback("contextmenu", on_ctx.as_ref().unchecked_ref());
        on_ctx.forget();
    }

    // Double-Click closes polygon/zone
    {
        let st = state.clone();
        let on_dbl = Closure::wrap(Box::new(move |e: MouseEvent| {
            e.prevent_default();
            let mut app = st.borrow_mut();
            if !app.poly_points_mm.is_empty() {
                app.finish_polygon_or_zone();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = canvas
            .add_event_listener_with_callback("dblclick", on_dbl.as_ref().unchecked_ref());
        on_dbl.forget();
    }

    // Wheel Zoom towards cursor
    {
        let st = state.clone();
        let on_wheel = Closure::wrap(Box::new(move |e: WheelEvent| {
            e.prevent_default();
            let factor = if e.delta_y() < 0.0 { 1.16 } else { 0.86 };
            let mut app = st.borrow_mut();
            if let Some(ref mut r) = app.renderer {
                r.zoom_at_cursor(factor, e.client_x() as f64, e.client_y() as f64);
            }
            app.draw_all();
            app.update_telemetry_hud();
        }) as Box<dyn FnMut(_)>);
        let _ = canvas.add_event_listener_with_callback("wheel", on_wheel.as_ref().unchecked_ref());
        on_wheel.forget();
    }

    // Mouse Down (CAD Tool Action, Drag Selected Primitive, or Camera Pan/Orbit)
    {
        let st = state.clone();
        let on_down = Closure::wrap(Box::new(move |e: MouseEvent| {
            let mut app = st.borrow_mut();
            let btn = e.button();
            let cx = e.client_x() as f64;
            let cy = e.client_y() as f64;
            app.last_mouse = (cx, cy);
            app.mouse_down_pos = (cx, cy);
            app.drag_button = btn;

            if btn == 1 || btn == 2 {
                app.is_dragging = true;
                app.is_dragging_primitive = false;
                return;
            }
            if btn != 0 {
                return;
            }

            let raw_mm = app
                .renderer
                .as_ref()
                .map(|r| r.screen_to_board_mm(cx, cy))
                .unwrap_or((130.0, 95.0));
            let snapped = app.snap_mm(raw_mm.0, raw_mm.1);
            let active_mask = app
                .renderer
                .as_ref()
                .map(|r| r.active_layer_mask)
                .unwrap_or(0x7FFFFF);
            let is_3d = app
                .renderer
                .as_ref()
                .map(|r| r.mode_3d_explode > 0.05)
                .unwrap_or(false);

            if is_3d {
                app.is_dragging = true;
                app.is_dragging_primitive = false;
                return;
            }

            let sel_mask = app.sel_filter_mask();

            match app.active_tool {
                CadTool::SelectMove => {
                    let hit = app.current_board.query_nearest_filtered(
                        raw_mm.0,
                        raw_mm.1,
                        active_mask,
                        1.6,
                        sel_mask,
                    );
                    if let Some(ref h) = hit {
                        if Some(h.instance_idx) == app.selected_primitive_idx {
                            app.push_undo();
                            app.is_dragging = true;
                            app.is_dragging_primitive = true;
                            app.drag_prim_last_mm = snapped;
                            return;
                        }
                    }
                    app.is_dragging = true;
                    app.is_dragging_primitive = false;
                }
                CadTool::RouteTrack => {
                    let hit = app
                        .current_board
                        .query_nearest(raw_mm.0, raw_mm.1, active_mask, 1.6);
                    let start_pt = hit
                        .as_ref()
                        .map(|h| (h.center_x, h.center_y))
                        .unwrap_or(snapped);
                    let picked_net = hit
                        .as_ref()
                        .map(|h| h.net_id)
                        .filter(|&n| n > 0)
                        .unwrap_or_else(|| {
                            app.renderer
                                .as_ref()
                                .map(|r| r.selected_net_id.max(0) as u32)
                                .unwrap_or(1)
                        });

                    if let Some((sx, sy, net_id)) = app.route_start_mm {
                        let (ex, ey) = snapped;
                        if (ex - sx).abs() > 0.01 || (ey - sy).abs() > 0.01 {
                            app.push_undo();
                            let (kx, ky) = app.compute_45_knee(sx, sy, ex, ey);
                            let w = app.active_track_width_mm;
                            let layer = app.active_draw_layer;
                            if (kx - sx).abs() > 0.01 || (ky - sy).abs() > 0.01 {
                                app.current_board
                                    .add_primitive(sx, sy, kx, ky, w, w, layer, net_id, 0.0);
                            }
                            let last_idx = app
                                .current_board
                                .add_primitive(kx, ky, ex, ey, w, w, layer, net_id, 0.0);
                            app.selected_primitive_idx = Some(last_idx);
                            app.route_start_mm = Some((ex, ey, net_id));
                            app.populate_net_selector();
                            app.rebuild_gpu_buffer_with_markers();
                            app.render_inspector_panel();
                        }
                    } else {
                        app.route_start_mm = Some((start_pt.0, start_pt.1, picked_net));
                        if picked_net > 0 {
                            app.select_net_probe(picked_net as i32);
                        }
                        app.rebuild_gpu_buffer_with_markers();
                    }
                }
                CadTool::RouteDiffPair => {
                    if let Some((sx, sy, net_id)) = app.route_start_mm {
                        let (ex, ey) = snapped;
                        if (ex - sx).abs() > 0.01 || (ey - sy).abs() > 0.01 {
                            app.push_undo();
                            let dx = ex - sx;
                            let dy = ey - sy;
                            let len = (dx * dx + dy * dy).sqrt().max(0.01);
                            let nx = -dy / len;
                            let ny = dx / len;
                            let half_pitch = app.active_track_width_mm * 1.1;
                            let w = app.active_track_width_mm;
                            let layer = app.active_draw_layer;
                            for (sign, nid) in [(1.0f32, net_id), (-1.0f32, net_id + 1)] {
                                let ox = nx * half_pitch * sign;
                                let oy = ny * half_pitch * sign;
                                let (kx, ky) =
                                    app.compute_45_knee(sx + ox, sy + oy, ex + ox, ey + oy);
                                app.current_board.add_primitive(
                                    sx + ox,
                                    sy + oy,
                                    kx,
                                    ky,
                                    w,
                                    w,
                                    layer,
                                    nid,
                                    0.0,
                                );
                                app.current_board.add_primitive(
                                    kx,
                                    ky,
                                    ex + ox,
                                    ey + oy,
                                    w,
                                    w,
                                    layer,
                                    nid,
                                    0.0,
                                );
                            }
                            app.route_start_mm = Some((ex, ey, net_id));
                            app.populate_net_selector();
                            app.rebuild_gpu_buffer_with_markers();
                        }
                    } else {
                        app.route_start_mm = Some((snapped.0, snapped.1, 1));
                        app.rebuild_gpu_buffer_with_markers();
                    }
                }
                CadTool::TuneTrackLength => {
                    if let Some((sx, sy, net_id)) = app.route_start_mm {
                        let (ex, ey) = snapped;
                        let pts = app.build_serpentine_points(sx, sy, ex, ey);
                        if pts.len() >= 2 {
                            app.push_undo();
                            let w = app.active_track_width_mm;
                            let layer = app.active_draw_layer;
                            for w_pair in pts.windows(2) {
                                app.current_board.add_primitive(
                                    w_pair[0].0,
                                    w_pair[0].1,
                                    w_pair[1].0,
                                    w_pair[1].1,
                                    w,
                                    w,
                                    layer,
                                    net_id,
                                    0.0,
                                );
                            }
                            app.route_start_mm = None;
                            app.populate_net_selector();
                            app.rebuild_gpu_buffer_with_markers();
                            app.render_inspector_panel();
                        }
                    } else {
                        let hit = app
                            .current_board
                            .query_nearest(raw_mm.0, raw_mm.1, active_mask, 1.8);
                        let nid = hit.map(|h| h.net_id).filter(|&n| n > 0).unwrap_or(1);
                        app.route_start_mm = Some((snapped.0, snapped.1, nid));
                        app.rebuild_gpu_buffer_with_markers();
                    }
                }
                CadTool::PlaceVia => {
                    let hit = app
                        .current_board
                        .query_nearest(raw_mm.0, raw_mm.1, active_mask, 1.5);
                    let net_id = hit
                        .map(|h| h.net_id)
                        .filter(|&n| n > 0)
                        .unwrap_or_else(|| {
                            app.renderer
                                .as_ref()
                                .map(|r| r.selected_net_id.max(0) as u32)
                                .unwrap_or(1)
                        });
                    app.push_undo();
                    let vsize = app.active_via_size_mm;
                    let vdrill = app.active_via_drill_mm;
                    let idx = app.current_board.add_primitive(
                        snapped.0,
                        snapped.1,
                        snapped.0,
                        snapped.1,
                        vsize,
                        vdrill,
                        LAYER_F_CU | LAYER_B_CU,
                        net_id,
                        1.0,
                    );
                    app.selected_primitive_idx = Some(idx);
                    app.populate_net_selector();
                    app.rebuild_gpu_buffer_with_markers();
                    app.render_inspector_panel();
                }
                CadTool::PlacePad => {
                    let net_id = app
                        .renderer
                        .as_ref()
                        .map(|r| r.selected_net_id.max(0) as u32)
                        .unwrap_or(1);
                    app.push_undo();
                    let layer = app.active_draw_layer;
                    let idx = app.current_board.add_primitive(
                        snapped.0, snapped.1, snapped.0, snapped.1, 1.4, 0.9, layer, net_id, 2.0,
                    );
                    app.selected_primitive_idx = Some(idx);
                    app.populate_net_selector();
                    app.rebuild_gpu_buffer_with_markers();
                    app.render_inspector_panel();
                }
                CadTool::AddZone | CadTool::AddKeepout | CadTool::DrawPoly => {
                    app.poly_points_mm.push(snapped);
                    app.draw_all();
                }
                CadTool::DrawLine | CadTool::DrawArc => {
                    if let Some((sx, sy, _)) = app.route_start_mm {
                        let (ex, ey) = snapped;
                        if (ex - sx).abs() > 0.01 || (ey - sy).abs() > 0.01 {
                            app.push_undo();
                            let layer = app.active_draw_layer;
                            let idx = app
                                .current_board
                                .add_primitive(sx, sy, ex, ey, 0.18, 0.18, layer, 0, 0.0);
                            app.selected_primitive_idx = Some(idx);
                            app.route_start_mm = Some((ex, ey, 0));
                            app.rebuild_gpu_buffer_with_markers();
                            app.render_inspector_panel();
                        }
                    } else {
                        app.route_start_mm = Some((snapped.0, snapped.1, 0));
                        app.rebuild_gpu_buffer_with_markers();
                    }
                }
                CadTool::DrawCircle => {
                    if let Some((sx, sy, _)) = app.route_start_mm {
                        let rad = ((snapped.0 - sx).hypot(snapped.1 - sy)).max(0.2);
                        app.push_undo();
                        let w = 0.18f32;
                        let layer = app.active_draw_layer;
                        let idx = app.current_board.add_primitive(
                            sx,
                            sy,
                            sx,
                            sy,
                            (rad + w * 0.5) * 2.0,
                            (rad - w * 0.5).max(0.02) * 2.0,
                            layer,
                            0,
                            8.0,
                        );
                        app.selected_primitive_idx = Some(idx);
                        app.route_start_mm = None;
                        app.rebuild_gpu_buffer_with_markers();
                        app.render_inspector_panel();
                    } else {
                        app.route_start_mm = Some((snapped.0, snapped.1, 0));
                        app.rebuild_gpu_buffer_with_markers();
                    }
                }
                CadTool::DrawRect => {
                    if let Some((sx, sy, _)) = app.route_start_mm {
                        let (ex, ey) = snapped;
                        if (ex - sx).abs() > 0.05 && (ey - sy).abs() > 0.05 {
                            app.push_undo();
                            let layer = app.active_draw_layer;
                            app.current_board
                                .add_primitive(sx, sy, ex, sy, 0.18, 0.18, layer, 0, 0.0);
                            app.current_board
                                .add_primitive(ex, sy, ex, ey, 0.18, 0.18, layer, 0, 0.0);
                            app.current_board
                                .add_primitive(ex, ey, sx, ey, 0.18, 0.18, layer, 0, 0.0);
                            let idx = app
                                .current_board
                                .add_primitive(sx, ey, sx, sy, 0.18, 0.18, layer, 0, 0.0);
                            app.selected_primitive_idx = Some(idx);
                            app.route_start_mm = None;
                            app.rebuild_gpu_buffer_with_markers();
                            app.render_inspector_panel();
                        }
                    } else {
                        app.route_start_mm = Some((snapped.0, snapped.1, 0));
                        app.rebuild_gpu_buffer_with_markers();
                    }
                }
                CadTool::AddText => {
                    app.push_undo();
                    let layer = app.active_draw_layer;
                    let idx_num = app.current_board.texts.len() + 1;
                    app.current_board.texts.push(BoardText {
                        x: snapped.0,
                        y: snapped.1,
                        angle_deg: 0.0,
                        size_mm: 1.2,
                        text: format!("LABEL_{}", idx_num),
                        layer_mask: layer,
                        is_ref: true,
                    });
                    app.draw_all();
                }
                CadTool::AddDimension => {
                    if let Some((sx, sy, _)) = app.route_start_mm {
                        app.push_undo();
                        let layer = app.active_draw_layer;
                        app.current_board.dimensions.push(CadDimension {
                            x0: sx,
                            y0: sy,
                            x1: snapped.0,
                            y1: snapped.1,
                            layer_mask: layer,
                        });
                        app.route_start_mm = None;
                        app.rebuild_gpu_buffer_with_markers();
                    } else {
                        app.route_start_mm = Some((snapped.0, snapped.1, 0));
                        app.rebuild_gpu_buffer_with_markers();
                    }
                }
                CadTool::PlaceOrigin => {
                    app.rel_origin_mm = snapped;
                    app.draw_all();
                    app.update_telemetry_hud();
                }
                CadTool::Measure => {
                    if app.route_start_mm.is_some() {
                        app.route_start_mm = None;
                    } else {
                        app.route_start_mm = Some((snapped.0, snapped.1, 0));
                    }
                    app.rebuild_gpu_buffer_with_markers();
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ =
            canvas.add_event_listener_with_callback("mousedown", on_down.as_ref().unchecked_ref());
        on_down.forget();
    }

    // Mouse Move (3D Orbit/Pan, Drag-Move Object, Live Tool Preview, O(1) Spatial Hover)
    {
        let st = state.clone();
        let on_move = Closure::wrap(Box::new(move |e: MouseEvent| {
            let mut app = st.borrow_mut();
            let cx = e.client_x() as f64;
            let cy = e.client_y() as f64;

            if app.is_dragging {
                let dx = cx - app.last_mouse.0;
                let dy = cy - app.last_mouse.1;
                app.last_mouse = (cx, cy);

                if app.is_dragging_primitive {
                    if let Some(idx) = app.selected_primitive_idx {
                        let raw_mm = app
                            .renderer
                            .as_ref()
                            .map(|r| r.screen_to_board_mm(cx, cy))
                            .unwrap_or((0.0, 0.0));
                        let snapped = app.snap_mm(raw_mm.0, raw_mm.1);
                        let step_dx = snapped.0 - app.drag_prim_last_mm.0;
                        let step_dy = snapped.1 - app.drag_prim_last_mm.1;
                        if step_dx.abs() > 1e-4 || step_dy.abs() > 1e-4 {
                            app.drag_prim_last_mm = snapped;
                            app.current_board
                                .translate_primitive_in_place(idx, step_dx, step_dy);
                            app.rebuild_gpu_buffer_with_markers();
                        }
                    }
                    return;
                }

                let pan_mode_3d = app.drag_3d_pan_mode;
                let drag_btn = app.drag_button;
                if let Some(ref mut r) = app.renderer {
                    if r.mode_3d_explode > 0.05 {
                        if drag_btn == 1 || drag_btn == 2 || e.shift_key() || pan_mode_3d {
                            r.pan_by_css_px(dx, dy);
                        } else {
                            r.orbit_by_css_px(dx, dy);
                        }
                    } else {
                        r.pan_by_css_px(dx, dy);
                    }
                }
                app.draw_all();
                return;
            }

            let mut coord_pair = (130.0f32, 95.0f32);
            let mut active_mask = 0x7FFFFFu32;
            if let Some(ref r) = app.renderer {
                coord_pair = r.screen_to_board_mm(cx, cy);
                active_mask = r.active_layer_mask;
            }
            let snapped = app.snap_mm(coord_pair.0, coord_pair.1);
            app.cursor_snapped_mm = snapped;
            app.update_telemetry_hud();

            if app.active_tool != CadTool::SelectMove {
                if app.active_tool == CadTool::Measure || app.active_tool == CadTool::AddDimension {
                    if let Some((sx, sy, _)) = app.route_start_mm {
                        let dx = snapped.0 - sx;
                        let dy = snapped.1 - sy;
                        let dist = (dx * dx + dy * dy).sqrt();
                        let mils = dist * 39.3701;
                        let ang = dy.atan2(dx).to_degrees();
                        if let Some(insp) = app.doc.get_element_by_id("rust-hover-inspector") {
                            insp.set_text_content(Some(&format!(
                                "Caliper: Dist = {:.3} mm ({:.1} mil) | ΔX = {:+.3} mm, ΔY = {:+.3} mm | Angle = {:.1}°",
                                dist, mils, dx, dy, ang
                            )));
                        }
                    }
                }
                app.rebuild_gpu_buffer_with_markers();
                return;
            }

            let sel_mask = app.sel_filter_mask();
            let hit = app.current_board.query_nearest_filtered(
                coord_pair.0,
                coord_pair.1,
                active_mask,
                1.8,
                sel_mask,
            );
            let new_hover_id = hit.as_ref().map(|h| h.instance_idx as i32).unwrap_or(-1);

            if let Some(insp) = app.doc.get_element_by_id("rust-hover-inspector") {
                if let Some(ref info) = hit {
                    insp.set_text_content(Some(&info.summary));
                } else {
                    insp.set_text_content(Some(
                        "Ready — Press Spacebar to zero dx/dy, X to Route, Z for Zone, 7 for Length Tune, H for High-Contrast",
                    ));
                }
            }

            let mut need_redraw = app.show_crosshair;
            if let Some(ref mut r) = app.renderer {
                if r.hovered_instance_id != new_hover_id {
                    r.hovered_instance_id = new_hover_id;
                    need_redraw = true;
                }
            }
            if need_redraw {
                app.draw_all();
            }
        }) as Box<dyn FnMut(_)>);
        let _ =
            canvas.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref());
        on_move.forget();
    }

    // Mouse Up (Finalize Object Drag or Click-to-Select Object & Net)
    {
        let st = state.clone();
        let on_up = Closure::wrap(Box::new(move |e: MouseEvent| {
            let mut app = st.borrow_mut();
            let was_dragging = app.is_dragging;
            let was_dragging_prim = app.is_dragging_primitive;
            app.is_dragging = false;
            app.is_dragging_primitive = false;

            if was_dragging_prim {
                app.current_board.rebuild_index_and_nets();
                app.populate_net_selector();
                app.rebuild_gpu_buffer_with_markers();
                app.render_inspector_panel();
                return;
            }

            if was_dragging
                && app.active_tab == "pcb"
                && app.active_tool == CadTool::SelectMove
                && e.button() == 0
            {
                let dx = (e.client_x() as f64) - app.mouse_down_pos.0;
                let dy = (e.client_y() as f64) - app.mouse_down_pos.1;
                if (dx * dx + dy * dy).sqrt() < 5.0 {
                    let mut coord_pair = (0.0f32, 0.0f32);
                    let mut active_mask = 0x7FFFFFu32;
                    if let Some(ref r) = app.renderer {
                        coord_pair =
                            r.screen_to_board_mm(e.client_x() as f64, e.client_y() as f64);
                        active_mask = r.active_layer_mask;
                    }
                    let sel_mask = app.sel_filter_mask();
                    let hit = app.current_board.query_nearest_filtered(
                        coord_pair.0,
                        coord_pair.1,
                        active_mask,
                        1.8,
                        sel_mask,
                    );
                    let sel_idx = hit.as_ref().map(|h| h.instance_idx);
                    app.selected_primitive_idx = sel_idx;
                    let net_to_select = hit.map(|h| h.net_id as i32).unwrap_or(0);
                    let sel_i32 = sel_idx.map(|i| i as i32).unwrap_or(-1);
                    if let Some(ref mut r) = app.renderer {
                        r.selected_instance_id = sel_i32;
                    }
                    app.select_net_probe(net_to_select);
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = canvas.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref());
        on_up.forget();
    }

    // Mouse Leave
    {
        let st = state;
        let on_leave = Closure::wrap(Box::new(move |_e: MouseEvent| {
            let mut app = st.borrow_mut();
            if app.is_dragging_primitive {
                app.current_board.rebuild_index_and_nets();
            }
            app.is_dragging = false;
            app.is_dragging_primitive = false;
        }) as Box<dyn FnMut(_)>);
        let _ = canvas
            .add_event_listener_with_callback("mouseleave", on_leave.as_ref().unchecked_ref());
        on_leave.forget();
    }
}

fn bind_kicad_display_and_appearance(doc: &Document, state: Rc<RefCell<AppController>>) {
    // Left Strip Display Buttons
    if let Some(btn) = doc.get_element_by_id("btn-opt-grid") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            let mut on = true;
            if let Some(ref mut r) = app.renderer {
                r.show_grid = !r.show_grid;
                on = r.show_grid;
            }
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_class_name(if on {
                    "cad-tool-btn cad-tool-btn-active"
                } else {
                    "cad-tool-btn"
                });
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-opt-polar") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            app.polar_coords = !app.polar_coords;
            let on = app.polar_coords;
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_class_name(if on {
                    "cad-tool-btn cad-tool-btn-active"
                } else {
                    "cad-tool-btn"
                });
            }
            app.update_telemetry_hud();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Units (in / mil / mm)
    for (btn_id, u) in [
        ("btn-unit-in", DisplayUnits::Inch),
        ("btn-unit-mil", DisplayUnits::Mil),
        ("btn-unit-mm", DisplayUnits::Mm),
    ] {
        if let Some(btn) = doc.get_element_by_id(btn_id) {
            let st = state.clone();
            let cb = Closure::wrap(Box::new(move |_e: Event| {
                let mut app = st.borrow_mut();
                app.units = u;
                for (id, unit_val) in [
                    ("btn-unit-in", DisplayUnits::Inch),
                    ("btn-unit-mil", DisplayUnits::Mil),
                    ("btn-unit-mm", DisplayUnits::Mm),
                ] {
                    if let Some(b) = app.doc.get_element_by_id(id) {
                        b.set_class_name(if unit_val == u {
                            "cad-tool-btn cad-tool-btn-active"
                        } else {
                            "cad-tool-btn"
                        });
                    }
                }
                app.update_telemetry_hud();
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    if let Some(btn) = doc.get_element_by_id("btn-opt-crosshair") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            app.show_crosshair = !app.show_crosshair;
            let on = app.show_crosshair;
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_class_name(if on {
                    "cad-tool-btn cad-tool-btn-active"
                } else {
                    "cad-tool-btn"
                });
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-opt-pad-labels") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            app.show_pad_labels = !app.show_pad_labels;
            app.show_pad_nets = app.show_pad_labels;
            let on = app.show_pad_labels;
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_class_name(if on {
                    "cad-tool-btn cad-tool-btn-active"
                } else {
                    "cad-tool-btn"
                });
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-opt-fp-text") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            app.show_fp_text = !app.show_fp_text;
            let on = app.show_fp_text;
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_class_name(if on {
                    "cad-tool-btn cad-tool-btn-active"
                } else {
                    "cad-tool-btn"
                });
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-opt-sketch") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            let mut on = false;
            if let Some(ref mut r) = app.renderer {
                r.sketch_mode = !r.sketch_mode;
                on = r.sketch_mode;
            }
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_class_name(if on {
                    "cad-tool-btn cad-tool-btn-active"
                } else {
                    "cad-tool-btn"
                });
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // 3 Copper Zone Display Mode Buttons (Filled / Outlines / Off)
    for (btn_id, zmode) in [
        ("btn-zone-filled", 0u8),
        ("btn-zone-outline", 1u8),
        ("btn-zone-off", 2u8),
    ] {
        if let Some(btn) = doc.get_element_by_id(btn_id) {
            let st = state.clone();
            let cb = Closure::wrap(Box::new(move |_e: Event| {
                let mut app = st.borrow_mut();
                app.zone_display_mode = zmode;
                for (id, m) in [
                    ("btn-zone-filled", 0u8),
                    ("btn-zone-outline", 1u8),
                    ("btn-zone-off", 2u8),
                ] {
                    if let Some(b) = app.doc.get_element_by_id(id) {
                        b.set_class_name(if m == zmode {
                            "cad-tool-btn cad-tool-btn-active"
                        } else {
                            "cad-tool-btn"
                        });
                    }
                }
                app.draw_all();
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    if let Some(btn) = doc.get_element_by_id("btn-opt-ratsnest") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            app.show_ratsnest = !app.show_ratsnest;
            let on = app.show_ratsnest;
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_class_name(if on {
                    "cad-tool-btn cad-tool-btn-active"
                } else {
                    "cad-tool-btn"
                });
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-opt-highcontrast") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            cycle_high_contrast(&mut app);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-opt-flipview") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            toggle_flip_board(&mut app);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Appearance Sub-Tabs (Layers | Objects | Nets)
    for (btn_id, sub, panel_id) in [
        ("appear-tab-layers", "layers", "appear-layers-content"),
        ("appear-tab-objects", "objects", "appear-objects-content"),
        ("appear-tab-nets", "nets", "appear-nets-content"),
    ] {
        if let Some(btn) = doc.get_element_by_id(btn_id) {
            let st = state.clone();
            let sub_str = sub.to_string();
            let target_panel = panel_id.to_string();
            let cb = Closure::wrap(Box::new(move |_e: Event| {
                let mut app = st.borrow_mut();
                app.appear_subtab = sub_str.clone();
                for (bid, pid) in [
                    ("appear-tab-layers", "appear-layers-content"),
                    ("appear-tab-objects", "appear-objects-content"),
                    ("appear-tab-nets", "appear-nets-content"),
                ] {
                    if let Some(b) = app.doc.get_element_by_id(bid) {
                        b.set_class_name(if bid == btn_id {
                            "cad-appear-tab cad-appear-tab-active"
                        } else {
                            "cad-appear-tab"
                        });
                    }
                    if let Some(p) = app.doc.get_element_by_id(pid) {
                        if pid == target_panel {
                            let _ = p.class_list().remove_1("hidden");
                        } else {
                            let _ = p.class_list().add_1("hidden");
                        }
                    }
                }
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    // Objects Sub-Tab Checkboxes
    if let Some(obj_panel) = doc.get_element_by_id("appear-objects-content") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(inp) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                let mut app = st.borrow_mut();
                match inp.id().as_str() {
                    "obj-chk-pad-labels" => app.show_pad_labels = inp.checked(),
                    "obj-chk-pad-nets" => app.show_pad_nets = inp.checked(),
                    "obj-chk-fp-text" => app.show_fp_text = inp.checked(),
                    "obj-chk-ratsnest" => app.show_ratsnest = inp.checked(),
                    "obj-chk-grid" => {
                        if let Some(ref mut r) = app.renderer {
                            r.show_grid = inp.checked();
                        }
                    }
                    _ => {}
                }
                app.draw_all();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = obj_panel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Nets Sub-Tab Clicks
    if let Some(nets_panel) = doc.get_element_by_id("appear-nets-content") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if let Some(row) = el.closest(".cad-layer-row").ok().flatten() {
                    if let Some(nid_str) = row.get_attribute("data-net-id") {
                        if let Ok(nid) = nid_str.parse::<i32>() {
                            st.borrow_mut().select_net_probe(nid);
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = nets_panel.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // High-Contrast Radio Buttons, Flip Board Checkbox & Layer Presets
    if let Some(chk_flip) = doc.get_element_by_id("chk-flip-board") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            toggle_flip_board(&mut app);
        }) as Box<dyn FnMut(_)>);
        let _ = chk_flip.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Ok(radios) = doc.query_selector_all("input[name='hc-mode']") {
        for i in 0..radios.length() {
            if let Some(r_el) = radios.item(i).and_then(|n| n.dyn_into::<HtmlInputElement>().ok()) {
                let st = state.clone();
                let cb = Closure::wrap(Box::new(move |e: Event| {
                    if let Some(inp) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                        let mode: u8 = inp.value().parse().unwrap_or(0);
                        let mut app = st.borrow_mut();
                        if let Some(ref mut r) = app.renderer {
                            r.high_contrast_mode = mode;
                        }
                        if let Some(btn) = app.doc.get_element_by_id("btn-opt-highcontrast") {
                            btn.set_class_name(if mode > 0 {
                                "cad-tool-btn cad-tool-btn-active"
                            } else {
                                "cad-tool-btn"
                            });
                        }
                        app.draw_all();
                    }
                }) as Box<dyn FnMut(_)>);
                let _ = r_el.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
                cb.forget();
            }
        }
    }

    if let Some(preset_sel) = doc.get_element_by_id("select-layer-preset") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(sel) = e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()) {
                let mask: u32 = match sel.value().as_str() {
                    "front" => {
                        LAYER_F_CU
                            | LAYER_F_PASTE
                            | LAYER_F_SILKS
                            | LAYER_F_MASK
                            | LAYER_F_CRTYD
                            | LAYER_F_FAB
                            | LAYER_EDGE_CUTS
                    }
                    "back" => {
                        LAYER_B_CU
                            | LAYER_B_PASTE
                            | LAYER_B_SILKS
                            | LAYER_B_MASK
                            | LAYER_B_CRTYD
                            | LAYER_B_FAB
                            | LAYER_EDGE_CUTS
                    }
                    "copper" => LAYER_F_CU | LAYER_IN1_CU | LAYER_IN2_CU | LAYER_B_CU | LAYER_EDGE_CUTS,
                    "fab" => {
                        LAYER_F_CRTYD
                            | LAYER_B_CRTYD
                            | LAYER_F_FAB
                            | LAYER_B_FAB
                            | LAYER_EDGE_CUTS
                            | LAYER_MARGIN
                            | LAYER_USER_DWGS
                            | LAYER_USER_CMTS
                    }
                    _ => 0x7FFFFF,
                };
                let mut app = st.borrow_mut();
                if let Some(ref mut r) = app.renderer {
                    r.active_layer_mask = mask;
                }
                if let Ok(chks) = app.doc.query_selector_all("#appear-layers-content input[type='checkbox']") {
                    for i in 0..chks.length() {
                        if let Some(chk) = chks.item(i).and_then(|n| n.dyn_into::<HtmlInputElement>().ok()) {
                            let bit = chk
                                .get_attribute("data-bit")
                                .and_then(|s| s.parse::<u32>().ok())
                                .unwrap_or(0);
                            chk.set_checked((mask & bit) != 0);
                        }
                    }
                }
                app.draw_all();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = preset_sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Selection Filter Checkboxes
    if let Some(filter_box) = doc.get_element_by_id("selection-filter-box") {
        let st = state;
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(inp) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                let mut app = st.borrow_mut();
                let checked = inp.checked();
                match inp.id().as_str() {
                    "sel-chk-all" => {
                        app.sel_footprints = checked;
                        app.sel_tracks = checked;
                        app.sel_vias = checked;
                        app.sel_pads = checked;
                        app.sel_zones = checked;
                        app.sel_graphics = checked;
                        for id in [
                            "sel-chk-fp",
                            "sel-chk-tracks",
                            "sel-chk-vias",
                            "sel-chk-pads",
                            "sel-chk-zones",
                            "sel-chk-gfx",
                        ] {
                            if let Some(c) = app
                                .doc
                                .get_element_by_id(id)
                                .and_then(|el| el.dyn_into::<HtmlInputElement>().ok())
                            {
                                c.set_checked(checked);
                            }
                        }
                    }
                    "sel-chk-fp" => app.sel_footprints = checked,
                    "sel-chk-tracks" => app.sel_tracks = checked,
                    "sel-chk-vias" => app.sel_vias = checked,
                    "sel-chk-pads" => app.sel_pads = checked,
                    "sel-chk-zones" => app.sel_zones = checked,
                    "sel-chk-gfx" => app.sel_graphics = checked,
                    _ => {}
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = filter_box.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

fn cycle_high_contrast(app: &mut AppController) {
    let mut next_mode = 0u8;
    if let Some(ref mut r) = app.renderer {
        r.high_contrast_mode = (r.high_contrast_mode + 1) % 3;
        next_mode = r.high_contrast_mode;
    }
    if let Some(btn) = app.doc.get_element_by_id("btn-opt-highcontrast") {
        btn.set_class_name(if next_mode > 0 {
            "cad-tool-btn cad-tool-btn-active"
        } else {
            "cad-tool-btn"
        });
    }
    if let Ok(radios) = app.doc.query_selector_all("input[name='hc-mode']") {
        for i in 0..radios.length() {
            if let Some(r_el) = radios.item(i).and_then(|n| n.dyn_into::<HtmlInputElement>().ok()) {
                r_el.set_checked(r_el.value() == next_mode.to_string());
            }
        }
    }
    app.draw_all();
}

fn toggle_flip_board(app: &mut AppController) {
    let mut flipped = false;
    if let Some(ref mut r) = app.renderer {
        r.flip_board_view = !r.flip_board_view;
        flipped = r.flip_board_view;
    }
    if let Some(btn) = app.doc.get_element_by_id("btn-opt-flipview") {
        btn.set_class_name(if flipped {
            "cad-tool-btn cad-tool-btn-active"
        } else {
            "cad-tool-btn"
        });
    }
    if let Some(chk) = app
        .doc
        .get_element_by_id("chk-flip-board")
        .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
    {
        chk.set_checked(flipped);
    }
    app.draw_all();
    app.update_telemetry_hud();
}

fn bind_schematic_interactions(doc: &Document, state: Rc<RefCell<AppController>>) {
    if let Some(vp) = doc.get_element_by_id("schematic-viewport") {
        {
            let st = state.clone();
            let on_wheel = Closure::wrap(Box::new(move |e: WheelEvent| {
                e.prevent_default();
                let factor = if e.delta_y() < 0.0 { 1.18 } else { 0.85 };
                let mut app = st.borrow_mut();
                app.sch_zoom = (app.sch_zoom * factor).clamp(0.25, 10.0);
                app.apply_schematic_transform();
                app.update_telemetry_hud();
            }) as Box<dyn FnMut(_)>);
            let _ = vp.add_event_listener_with_callback("wheel", on_wheel.as_ref().unchecked_ref());
            on_wheel.forget();
        }
        {
            let st = state.clone();
            let on_down = Closure::wrap(Box::new(move |e: MouseEvent| {
                let mut app = st.borrow_mut();
                app.sch_dragging = true;
                app.sch_last_mouse = (e.client_x() as f64, e.client_y() as f64);
            }) as Box<dyn FnMut(_)>);
            let _ =
                vp.add_event_listener_with_callback("mousedown", on_down.as_ref().unchecked_ref());
            on_down.forget();
        }
        {
            let st = state.clone();
            let on_move = Closure::wrap(Box::new(move |e: MouseEvent| {
                let mut app = st.borrow_mut();
                if app.sch_dragging {
                    let cx = e.client_x() as f64;
                    let cy = e.client_y() as f64;
                    app.sch_pan.0 += cx - app.sch_last_mouse.0;
                    app.sch_pan.1 += cy - app.sch_last_mouse.1;
                    app.sch_last_mouse = (cx, cy);
                    app.apply_schematic_transform();
                    app.update_telemetry_hud();
                }
            }) as Box<dyn FnMut(_)>);
            let _ =
                vp.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref());
            on_move.forget();
        }
        {
            let st = state.clone();
            let on_up = Closure::wrap(Box::new(move |_e: MouseEvent| {
                st.borrow_mut().sch_dragging = false;
            }) as Box<dyn FnMut(_)>);
            let _ = vp.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref());
            on_up.forget();
        }
    }

    if let Some(btn_reset) = doc.get_element_by_id("btn-sch-reset") {
        let st = state;
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            app.sch_zoom = 1.0;
            app.sch_pan = (0.0, 0.0);
            app.apply_schematic_transform();
            app.update_telemetry_hud();
        }) as Box<dyn FnMut(_)>);
        let _ = btn_reset.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

fn bind_keyboard_shortcuts(doc: &Document, state: Rc<RefCell<AppController>>) {
    let cb = Closure::wrap(Box::new(move |e: KeyboardEvent| {
        if let Some(target) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
            let tag = target.tag_name().to_uppercase();
            if tag == "INPUT" || tag == "SELECT" || tag == "TEXTAREA" {
                return;
            }
        }

        let key = e.key();
        let mut app = state.borrow_mut();
        if (e.ctrl_key() || e.meta_key()) && (key == "z" || key == "Z") {
            e.prevent_default();
            app.perform_undo();
            return;
        }

        match key.as_str() {
            "Escape" => {
                if !app.poly_points_mm.is_empty() {
                    app.poly_points_mm.clear();
                    app.draw_all();
                } else if app.route_start_mm.is_some() {
                    app.route_start_mm = None;
                    app.rebuild_gpu_buffer_with_markers();
                } else {
                    app.set_active_tool(CadTool::SelectMove);
                }
            }
            "Enter" => {
                if !app.poly_points_mm.is_empty() {
                    app.finish_polygon_or_zone();
                }
            }
            " " => {
                e.prevent_default();
                app.rel_origin_mm = app.cursor_snapped_mm;
                app.draw_all();
                app.update_telemetry_hud();
            }
            "/" => {
                e.prevent_default();
                app.route_posture_flip = !app.route_posture_flip;
                app.rebuild_gpu_buffer_with_markers();
            }
            "s" | "S" => app.set_active_tool(CadTool::SelectMove),
            "x" | "X" => app.set_active_tool(CadTool::RouteTrack),
            "6" => app.set_active_tool(CadTool::RouteDiffPair),
            "7" => app.set_active_tool(CadTool::TuneTrackLength),
            "v" | "V" => app.set_active_tool(CadTool::PlaceVia),
            "p" | "P" => app.set_active_tool(CadTool::PlacePad),
            "z" | "Z" => app.set_active_tool(CadTool::AddZone),
            "k" | "K" => app.set_active_tool(CadTool::AddKeepout),
            "l" | "L" => app.set_active_tool(CadTool::DrawLine),
            "a" | "A" => app.set_active_tool(CadTool::DrawArc),
            "c" | "C" => app.set_active_tool(CadTool::DrawCircle),
            "y" | "Y" => app.set_active_tool(CadTool::DrawPoly),
            "t" | "T" => app.set_active_tool(CadTool::AddText),
            "d" | "D" => app.set_active_tool(CadTool::AddDimension),
            "o" | "O" => app.set_active_tool(CadTool::PlaceOrigin),
            "m" | "M" => app.set_active_tool(CadTool::Measure),
            "r" | "R" => app.rotate_selected_primitive(),
            "b" | "B" => {
                app.zone_display_mode = 0;
                app.draw_all();
            }
            "h" | "H" => cycle_high_contrast(&mut app),
            "f" | "F" => toggle_flip_board(&mut app),
            "Delete" | "Backspace" => {
                e.prevent_default();
                app.delete_selected_primitive();
            }
            "3" => toggle_3d_mode(&mut app),
            _ => {}
        }
    }) as Box<dyn FnMut(_)>);
    let _ = doc.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
    cb.forget();
}

fn toggle_3d_mode(app: &mut AppController) {
    let mut is_3d = false;
    if let Some(ref mut r) = app.renderer {
        r.mode_3d_explode = if r.mode_3d_explode > 0.05 { 0.0 } else { 1.0 };
        is_3d = r.mode_3d_explode > 0.05;
    }
    if let Some(btn) = app.doc.get_element_by_id("btn-3d-stackup") {
        btn.set_class_name(if is_3d {
            "cad-btn cad-btn-active"
        } else {
            "cad-btn"
        });
    }
    if let Some(ctrl) = app.doc.get_element_by_id("toolbar-3d-controls") {
        if is_3d {
            let _ = ctrl.class_list().remove_1("hidden");
        } else {
            let _ = ctrl.class_list().add_1("hidden");
        }
    }
    app.draw_all();
    app.update_telemetry_hud();
}

fn bind_toolbar_and_tabs(doc: &Document, state: Rc<RefCell<AppController>>) {
    // Right Vertical Tool Strip Buttons
    for (btn_id, tool) in [
        ("btn-tool-select", CadTool::SelectMove),
        ("btn-tool-route", CadTool::RouteTrack),
        ("btn-tool-diffpair", CadTool::RouteDiffPair),
        ("btn-tool-tune", CadTool::TuneTrackLength),
        ("btn-tool-via", CadTool::PlaceVia),
        ("btn-tool-pad", CadTool::PlacePad),
        ("btn-tool-zone", CadTool::AddZone),
        ("btn-tool-keepout", CadTool::AddKeepout),
        ("btn-tool-line", CadTool::DrawLine),
        ("btn-tool-arc", CadTool::DrawArc),
        ("btn-tool-circle", CadTool::DrawCircle),
        ("btn-tool-rect", CadTool::DrawRect),
        ("btn-tool-poly", CadTool::DrawPoly),
        ("btn-tool-text", CadTool::AddText),
        ("btn-tool-dim", CadTool::AddDimension),
        ("btn-tool-origin", CadTool::PlaceOrigin),
        ("btn-tool-measure", CadTool::Measure),
    ] {
        if let Some(btn) = doc.get_element_by_id(btn_id) {
            let st = state.clone();
            let cb = Closure::wrap(Box::new(move |_e: Event| {
                st.borrow_mut().set_active_tool(tool);
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    // Top Menu Bar Quick Actions
    if let Some(btn) = doc.get_element_by_id("menu-edit-undo") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            st.borrow_mut().perform_undo();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("menu-view-3d") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            toggle_3d_mode(&mut st.borrow_mut());
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("menu-place-pad") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            st.borrow_mut().set_active_tool(CadTool::PlacePad);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("menu-route-single") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            st.borrow_mut().set_active_tool(CadTool::RouteTrack);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("menu-inspect-drc") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            switch_view_tab(&st, "drc");
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("menu-tools-fill") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            app.zone_display_mode = 0;
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Undo, Rotate & Delete Buttons
    if let Some(btn) = doc.get_element_by_id("btn-undo") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            st.borrow_mut().perform_undo();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("btn-rotate-selected") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            st.borrow_mut().rotate_selected_primitive();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("btn-delete-selected") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            st.borrow_mut().delete_selected_primitive();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Active Layer, Track Width, Via Size, Grid Snap Selectors
    if let Some(sel) = doc.get_element_by_id("select-draw-layer") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(t) = e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()) {
                let bit = t.value().parse().unwrap_or(LAYER_F_CU);
                st.borrow_mut().set_active_draw_layer(bit);
            }
        }) as Box<dyn FnMut(_)>);
        let _ = sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(sel) = doc.get_element_by_id("select-track-width") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(t) = e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()) {
                st.borrow_mut().active_track_width_mm = t.value().parse().unwrap_or(0.25);
            }
        }) as Box<dyn FnMut(_)>);
        let _ = sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(sel) = doc.get_element_by_id("select-via-size") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(t) = e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()) {
                let val = t.value();
                let mut parts = val.split(',');
                let sz: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.60);
                let dr: f32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.30);
                let mut app = st.borrow_mut();
                app.active_via_size_mm = sz;
                app.active_via_drill_mm = dr;
            }
        }) as Box<dyn FnMut(_)>);
        let _ = sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(sel) = doc.get_element_by_id("select-grid-snap") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(t) = e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()) {
                let mut app = st.borrow_mut();
                app.grid_snap_mm = t.value().parse().unwrap_or(0.25);
                app.draw_all();
                app.update_telemetry_hud();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Delegated Inspector Panel Clicks
    if let Some(insp_panel) = doc.get_element_by_id("cad-inspector-content") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                let id = el.id();
                if id == "btn-prop-delete" {
                    st.borrow_mut().delete_selected_primitive();
                } else if id == "btn-prop-rotate" {
                    st.borrow_mut().rotate_selected_primitive();
                } else if id == "btn-clear-net-insp" {
                    st.borrow_mut().select_net_probe(0);
                } else if id == "btn-apply-props" {
                    let mut app = st.borrow_mut();
                    if let Some(idx) = app.selected_primitive_idx {
                        if let Some(old) = app.current_board.get_primitive(idx) {
                            let read_f32 = |id_str: &str, fallback: f32| -> f32 {
                                app.doc
                                    .get_element_by_id(id_str)
                                    .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                                    .and_then(|i| i.value().parse::<f32>().ok())
                                    .unwrap_or(fallback)
                            };
                            let layer_val: u32 = app
                                .doc
                                .get_element_by_id("prop-layer")
                                .and_then(|e| e.dyn_into::<HtmlSelectElement>().ok())
                                .and_then(|s| s.value().parse::<u32>().ok())
                                .unwrap_or(old[6] as u32);
                            let net_id = read_f32("prop-net", (old[7] / 16.0).floor()) as u32;
                            let prim_type = old[7] - ((old[7] / 16.0).floor() * 16.0);
                            let new_data = [
                                read_f32("prop-x0", old[0]),
                                read_f32("prop-y0", old[1]),
                                read_f32("prop-x1", old[2]),
                                read_f32("prop-y1", old[3]),
                                read_f32("prop-w", old[4]).max(0.05),
                                read_f32("prop-h", old[5]).max(0.05),
                                layer_val as f32,
                                (net_id as f32) * 16.0 + prim_type,
                            ];
                            app.push_undo();
                            app.current_board.set_primitive(idx, new_data);
                            app.populate_net_selector();
                            app.rebuild_gpu_buffer_with_markers();
                            app.render_inspector_panel();
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = insp_panel.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Net Probe Dropdown Selector
    if let Some(net_sel) = doc.get_element_by_id("rust-net-select") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(target) = e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()) {
                let net_id: i32 = target.value().parse().unwrap_or(0);
                st.borrow_mut().select_net_probe(net_id);
            }
        }) as Box<dyn FnMut(_)>);
        let _ = net_sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Project Selector
    if let Some(sel) = doc.get_element_by_id("rust-project-select") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(target) = e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()) {
                let proj = target.value();
                if let Some(link) = st.borrow().doc.get_element_by_id("link-3d-vrml") {
                    let href = if proj == "cm5_minima" {
                        "/samples/cm5_minima/board.wrl"
                    } else {
                        "/samples/complex_hierarchy/board.wrl"
                    };
                    let _ = link.set_attribute("href", href);
                }
                fetch_project_boards(st.clone(), &proj);
            }
        }) as Box<dyn FnMut(_)>);
        let _ = sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // 3D Stackup Orbit Toggle & 3D Camera Controls
    if let Some(btn) = doc.get_element_by_id("btn-3d-stackup") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            toggle_3d_mode(&mut app);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-3d-drag-mode") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            app.drag_3d_pan_mode = !app.drag_3d_pan_mode;
            let pan_mode = app.drag_3d_pan_mode;
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_text_content(Some(if pan_mode {
                    "Left-Drag: Pan 3D"
                } else {
                    "Left-Drag: Orbit 3D"
                }));
            }
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-3d-iso") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if let Some(ref mut r) = app.renderer {
                r.yaw_rad = -0.45;
                r.pitch_rad = 0.62;
                r.fit_to_board();
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-3d-top") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if let Some(ref mut r) = app.renderer {
                r.yaw_rad = 0.0;
                r.pitch_rad = 0.05;
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-3d-flip") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if let Some(ref mut r) = app.renderer {
                r.yaw_rad += std::f32::consts::PI;
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(slider) = doc.get_element_by_id("input-3d-explode") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(inp) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                let val: f32 = inp.value().parse().unwrap_or(100.0);
                let mut app = st.borrow_mut();
                if let Some(ref mut r) = app.renderer {
                    r.mode_3d_explode = (val / 100.0).max(0.1);
                }
                app.draw_all();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = slider.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // 100k HDI Stress Test Button
    if let Some(btn) = doc.get_element_by_id("btn-100k-hdi") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let t0 = js_sys::Date::now();
            let hdi_board = generate_hdi_stress_board(100_000);
            let dt = js_sys::Date::now() - t0;
            {
                let mut app = st.borrow_mut();
                app.current_board = hdi_board;
                app.parse_time_ms = dt;
                app.selected_primitive_idx = None;
                app.populate_net_selector();
                app.select_net_probe(0);
            }
            switch_view_tab(&st, "pcb");
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Zoom In / Out / Fit
    if let Some(btn) = doc.get_element_by_id("btn-zoom-in") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if app.active_tab == "schematic" {
                app.sch_zoom = (app.sch_zoom * 1.25).min(10.0);
                app.apply_schematic_transform();
            } else if let Some(ref mut r) = app.renderer {
                r.scale_px_per_mm = (r.scale_px_per_mm * 1.25).min(500.0);
            }
            app.draw_all();
            app.update_telemetry_hud();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("btn-zoom-out") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if app.active_tab == "schematic" {
                app.sch_zoom = (app.sch_zoom * 0.8).max(0.25);
                app.apply_schematic_transform();
            } else if let Some(ref mut r) = app.renderer {
                r.scale_px_per_mm = (r.scale_px_per_mm * 0.8).max(1.5);
            }
            app.draw_all();
            app.update_telemetry_hud();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("btn-zoom-fit") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if app.active_tab == "schematic" {
                app.sch_zoom = 1.0;
                app.sch_pan = (0.0, 0.0);
                app.apply_schematic_transform();
            } else if let Some(ref mut r) = app.renderer {
                r.fit_to_board();
            }
            app.draw_all();
            app.update_telemetry_hud();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // View Mode Tabs
    for (tab_id, mode_name) in [
        ("tab-pcb", "pcb"),
        ("tab-schematic", "schematic"),
        ("tab-diff", "diff"),
        ("tab-drc", "drc"),
        ("tab-parts", "parts"),
    ] {
        if let Some(btn) = doc.get_element_by_id(tab_id) {
            let st = state.clone();
            let mode_str = mode_name.to_string();
            let cb = Closure::wrap(Box::new(move |_e: Event| {
                switch_view_tab(&st, &mode_str);
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    // Parts Search Input Filter
    if let Some(inp_el) = doc.get_element_by_id("input-parts-search") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(inp) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                st.borrow_mut().parts_filter = inp.value();
                render_parts_list(&st);
            }
        }) as Box<dyn FnMut(_)>);
        let _ = inp_el.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Clicking a DRC row flies the GPU camera to that violation
    if let Some(drc_list) = doc.get_element_by_id("drc-list-container") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if let Some(row) = el.closest(".drc-row").ok().flatten() {
                    if let Some(idx_str) = row.get_attribute("data-drc-idx") {
                        if let Ok(idx) = idx_str.parse::<usize>() {
                            let coords = st.borrow().drc_items.get(idx).map(|d| (d.x, d.y));
                            if let Some((vx, vy)) = coords {
                                switch_view_tab(&st, "pcb");
                                if vx > 1.0 && vy > 1.0 {
                                    let mut app = st.borrow_mut();
                                    if let Some(ref mut r) = app.renderer {
                                        r.center_mm = (vx, vy);
                                        r.scale_px_per_mm = 26.0;
                                    }
                                    app.draw_all();
                                    app.update_telemetry_hud();
                                }
                            }
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = drc_list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Diff Mode Controls
    if let Some(btn) = doc.get_element_by_id("btn-diff-boolean") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if let Some(ref mut r) = app.renderer {
                r.diff_mode = 1.0;
            }
            if let Some(b1) = app.doc.get_element_by_id("btn-diff-boolean") {
                b1.set_class_name("cad-btn cad-btn-active");
            }
            if let Some(b2) = app.doc.get_element_by_id("btn-diff-wipe") {
                b2.set_class_name("cad-btn");
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(btn) = doc.get_element_by_id("btn-diff-wipe") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if let Some(ref mut r) = app.renderer {
                r.diff_mode = 2.0;
            }
            if let Some(b1) = app.doc.get_element_by_id("btn-diff-boolean") {
                b1.set_class_name("cad-btn");
            }
            if let Some(b2) = app.doc.get_element_by_id("btn-diff-wipe") {
                b2.set_class_name("cad-btn cad-btn-active");
            }
            app.draw_all();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(slider) = doc.get_element_by_id("input-wipe-slider") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(inp) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                let val: f32 = inp.value().parse().unwrap_or(50.0);
                let mut app = st.borrow_mut();
                if app.active_tab == "diff" {
                    if let Some(ref mut r) = app.renderer {
                        r.diff_mode = 2.0;
                        r.wipe_ratio = (val / 100.0).clamp(0.0, 1.0);
                    }
                    app.draw_all();
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = slider.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Layer Visibility Checkboxes & Active Layer Selection in Appearance Table
    if let Some(list) = doc.get_element_by_id("appear-layers-content") {
        let st = state.clone();
        let cb_chg = Closure::wrap(Box::new(move |e: Event| {
            if let Some(inp) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                if let Some(bit_str) = inp.get_attribute("data-bit") {
                    if let Ok(bit) = bit_str.parse::<u32>() {
                        let mut app = st.borrow_mut();
                        if let Some(ref mut r) = app.renderer {
                            if inp.checked() {
                                r.active_layer_mask |= bit;
                            } else {
                                r.active_layer_mask &= !bit;
                            }
                        }
                        app.draw_all();
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = list.add_event_listener_with_callback("change", cb_chg.as_ref().unchecked_ref());
        cb_chg.forget();

        let st_clk = state.clone();
        let cb_clk = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if el.tag_name().to_uppercase() == "INPUT" {
                    return;
                }
                if let Some(row) = el.closest(".cad-layer-row").ok().flatten() {
                    if let Some(bit_str) = row.get_attribute("data-layer-bit") {
                        if let Ok(bit) = bit_str.parse::<u32>() {
                            st_clk.borrow_mut().set_active_draw_layer(bit);
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = list.add_event_listener_with_callback("click", cb_clk.as_ref().unchecked_ref());
        cb_clk.forget();
    }

    // Schematic Sheet Switcher
    if let Some(sch_panel) = doc.get_element_by_id("schematic-view-panel") {
        let st = state;
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if let Some(src) = el.get_attribute("data-sch") {
                    let mut app = st.borrow_mut();
                    if let Some(img) = app.doc.get_element_by_id("schematic-svg-img") {
                        let _ = img.set_attribute("src", &src);
                    }
                    app.sch_zoom = 1.0;
                    app.sch_pan = (0.0, 0.0);
                    app.apply_schematic_transform();
                    app.update_telemetry_hud();
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = sch_panel.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

pub fn switch_view_tab(state: &Rc<RefCell<AppController>>, mode: &str) {
    let mut app = state.borrow_mut();
    app.active_tab = mode.to_string();

    for (tid, m) in [
        ("tab-pcb", "pcb"),
        ("tab-schematic", "schematic"),
        ("tab-diff", "diff"),
        ("tab-drc", "drc"),
        ("tab-parts", "parts"),
    ] {
        if let Some(btn) = app.doc.get_element_by_id(tid) {
            if m == mode {
                btn.set_class_name("cad-tab cad-tab-active");
            } else {
                btn.set_class_name("cad-tab");
            }
        }
    }

    if let Some(diff_ctrl) = app.doc.get_element_by_id("diff-toolbar-controls") {
        if mode == "diff" {
            let _ = diff_ctrl.class_list().remove_1("hidden");
        } else {
            let _ = diff_ctrl.class_list().add_1("hidden");
        }
    }

    for (panel_id, target_mode) in [
        ("schematic-view-panel", "schematic"),
        ("drc-view-panel", "drc"),
        ("parts-view-panel", "parts"),
    ] {
        if let Some(panel) = app.doc.get_element_by_id(panel_id) {
            if mode == target_mode {
                let _ = panel.class_list().remove_1("hidden");
            } else {
                let _ = panel.class_list().add_1("hidden");
            }
        }
    }

    if mode == "pcb" || mode == "diff" {
        if let Some(ref mut r) = app.renderer {
            r.diff_mode = if mode == "diff" { 1.0 } else { 0.0 };
        }
        app.rebuild_gpu_buffer_with_markers();
        if let Some(ref mut r) = app.renderer {
            r.fit_to_board();
        }
        app.draw_all();
    }
    app.update_telemetry_hud();
}

fn render_parts_list(state: &Rc<RefCell<AppController>>) {
    let app = state.borrow();
    let container = match app.doc.get_element_by_id("parts-list-container") {
        Some(c) => c,
        None => return,
    };

    let q = app.parts_filter.to_lowercase();
    let mut html = String::new();
    for p in &app.parts_catalog {
        if !q.is_empty()
            && !p.mpn.to_lowercase().contains(&q)
            && !p.manufacturer.to_lowercase().contains(&q)
            && !p.description.to_lowercase().contains(&q)
            && !p.footprint_name.to_lowercase().contains(&q)
        {
            continue;
        }

        let status_color = match p.lifecycle_status.as_str() {
            "active" => "#34d399",
            "nrnd" => "#fbbf24",
            _ => "#f87171",
        };

        html.push_str(&format!(
            r#"<div class="cad-table-row">
                 <div>
                   <div style="display:flex;align-items:center;gap:8px;">
                     <span style="font-family:monospace;font-weight:700;color:#79b8ff;font-size:12px;">{mpn}</span>
                     <span style="color:#8b93a6;font-size:11px;">{mfr}</span>
                     <span style="font-family:monospace;font-size:10px;text-transform:uppercase;color:{status_color};">[{status}]</span>
                   </div>
                   <div style="color:#d0d6e2;font-size:11px;margin-top:2px;">{desc}</div>
                   <div style="font-family:monospace;font-size:10px;color:#6c768a;margin-top:2px;">Footprint: {fp} | Symbol: {sym}</div>
                 </div>
                 <div style="text-align:right;font-family:monospace;font-size:11px;">
                   <div style="color:#f0f3f8;font-weight:600;">${price:.3}</div>
                   <div style="font-size:10px;color:#8b93a6;">DK: {dk} | Mouser: {ms}</div>
                 </div>
               </div>"#,
            mpn = p.mpn,
            mfr = p.manufacturer,
            status_color = status_color,
            status = p.lifecycle_status,
            desc = p.description,
            fp = p.footprint_name,
            sym = p.symbol_name,
            price = p.unit_price_usd,
            dk = p.digikey_stock,
            ms = p.mouser_stock
        ));
    }
    container.set_inner_html(&html);
}

fn sync_backend_parts(state: Rc<RefCell<AppController>>) {
    fetch_text_with_callback("/api/parts", move |json_str| {
        if let Ok(parts) = serde_json::from_str::<Vec<PartLibraryEntry>>(&json_str) {
            if !parts.is_empty() {
                state.borrow_mut().parts_catalog = parts;
                render_parts_list(&state);
            }
        }
    });
}

fn fetch_project_boards(state: Rc<RefCell<AppController>>, project_id: &str) {
    let (url_a, url_b, drc_url) = if project_id == "cm5_minima" {
        (
            "/samples/cm5_minima/CM5_MINIMA_3.kicad_pcb",
            "/samples/cm5_minima/CM5_MINIMA_3.kicad_pcb",
            "/samples/cm5_minima/drc.json",
        )
    } else {
        (
            "/samples/complex_hierarchy/complex_hierarchy.kicad_pcb",
            "/samples/complex_hierarchy_revB/complex_hierarchy.kicad_pcb",
            "/samples/complex_hierarchy/drc.json",
        )
    };

    fetch_text_with_callback(url_a, {
        let st = state.clone();
        let url_b_str = url_b.to_string();
        move |text_a| {
            let t0 = js_sys::Date::now();
            let board_a = parse_kicad_pcb_str(&text_a);
            let dt = js_sys::Date::now() - t0;
            {
                let mut app = st.borrow_mut();
                app.rel_origin_mm = (board_a.bounds[0], board_a.bounds[3]);
                app.board_rev_a = board_a.clone();
                app.current_board = board_a;
                app.undo_stack.clear();
                app.selected_primitive_idx = None;
                app.route_start_mm = None;
                app.poly_points_mm.clear();
                app.parse_time_ms = dt;
                app.populate_net_selector();
                app.select_net_probe(0);
                app.rebuild_gpu_buffer_with_markers();
                if let Some(ref mut r) = app.renderer {
                    r.fit_to_board();
                }
                app.draw_all();
                app.update_telemetry_hud();
            }

            let st2 = st.clone();
            fetch_text_with_callback(&url_b_str, move |text_b| {
                let board_b = parse_kicad_pcb_str(&text_b);
                let mut app = st2.borrow_mut();
                let (diff_board, add, rem, unch) =
                    compute_vector_diff_boards(&app.board_rev_a, &board_b);
                app.board_rev_b = board_b;
                app.board_diff = diff_board;
                app.diff_stats = (add, rem, unch);
                if app.active_tab == "diff" {
                    app.rebuild_gpu_buffer_with_markers();
                }
            });
        }
    });

    fetch_text_with_callback(drc_url, {
        let st = state;
        move |drc_json| {
            let mut items = Vec::new();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&drc_json) {
                if let Some(arr) = val.get("violations").and_then(|v| v.as_array()) {
                    for v in arr {
                        let desc = v
                            .get("description")
                            .and_then(|s| s.as_str())
                            .unwrap_or("DRC Violation")
                            .to_string();
                        let sev = v
                            .get("severity")
                            .and_then(|s| s.as_str())
                            .unwrap_or("error")
                            .to_string();
                        let tname = v
                            .get("type")
                            .and_then(|s| s.as_str())
                            .unwrap_or("rule")
                            .to_string();
                        let mut px = 0.0f32;
                        let mut py = 0.0f32;
                        if let Some(it) = v
                            .get("items")
                            .and_then(|i| i.as_array())
                            .and_then(|a| a.first())
                        {
                            if let Some(pos) = it.get("pos") {
                                px = pos.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                                py = pos.get("y").and_then(|y| y.as_f64()).unwrap_or(0.0) as f32;
                            }
                        }
                        items.push(DrcItem {
                            description: desc,
                            severity: sev,
                            type_name: tname,
                            x: px,
                            y: py,
                        });
                    }
                }
            }

            let mut app = st.borrow_mut();
            if let Some(badge) = app.doc.get_element_by_id("drc-count-badge") {
                badge.set_text_content(Some(&items.len().to_string()));
            }
            if let Some(list_el) = app.doc.get_element_by_id("drc-list-container") {
                let mut html = String::new();
                for (i, d) in items.iter().take(100).enumerate() {
                    let sev_color = if d.severity == "error" {
                        "#f87171"
                    } else {
                        "#fbbf24"
                    };
                    html.push_str(&format!(
                        r#"<div data-drc-idx="{i}" class="drc-row cad-table-row" style="cursor:pointer;">
                             <div>
                               <div style="font-size:11px;font-weight:600;color:#e2e8f0;">{desc}</div>
                               <div style="font-family:monospace;font-size:10px;color:#8b93a6;margin-top:2px;">({x:.3} mm, {y:.3} mm) | Rule: {tname}</div>
                             </div>
                             <span style="font-family:monospace;font-size:10px;text-transform:uppercase;color:{sev_color};">[{sev}]</span>
                           </div>"#,
                        i = i,
                        desc = d.description,
                        x = d.x,
                        y = d.y,
                        tname = d.type_name,
                        sev_color = sev_color,
                        sev = d.severity
                    ));
                }
                list_el.set_inner_html(&html);
            }
            app.drc_items = items;
            app.rebuild_gpu_buffer_with_markers();
        }
    });
}

fn fetch_text_with_callback<F: FnOnce(String) + 'static>(url: &str, callback: F) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let promise = window.fetch_with_str(url);
    let cb_cell = Rc::new(RefCell::new(Some(callback)));

    let on_resp = Closure::wrap(Box::new(move |resp_val: JsValue| {
        if let Ok(resp) = resp_val.dyn_into::<web_sys::Response>() {
            if let Ok(text_promise) = resp.text() {
                let cb_inner = cb_cell.clone();
                let on_text = Closure::wrap(Box::new(move |text_val: JsValue| {
                    if let Some(s) = text_val.as_string() {
                        if let Some(f) = cb_inner.borrow_mut().take() {
                            f(s);
                        }
                    }
                }) as Box<dyn FnMut(JsValue)>);
                let _ = text_promise.then(&on_text);
                on_text.forget();
            }
        }
    }) as Box<dyn FnMut(JsValue)>);
    let _ = promise.then(&on_resp);
    on_resp.forget();
}

// ============================================================================
// C-ABI Exports for Headless / Node.js Benchmark Compatibility
// ============================================================================
thread_local! {
    static BENCH_BOARD: RefCell<ParsedBoard> = RefCell::new(ParsedBoard::empty());
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
pub extern "C" fn wasm_parse_kicad_pcb(ptr: *const u8, len: usize) -> usize {
    if ptr.is_null() || len == 0 {
        return 0;
    }
    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    let s = match std::str::from_utf8(bytes) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let parsed = parse_kicad_pcb_str(s);
    let count = parsed.primitive_count;
    BENCH_BOARD.with(|b| *b.borrow_mut() = parsed);
    count
}

#[no_mangle]
pub extern "C" fn wasm_generate_hdi_stress_test(target_primitives: usize) -> usize {
    let parsed = generate_hdi_stress_board(target_primitives);
    let count = parsed.primitive_count;
    BENCH_BOARD.with(|b| *b.borrow_mut() = parsed);
    count
}

#[no_mangle]
pub extern "C" fn wasm_instances_ptr() -> *const f32 {
    BENCH_BOARD.with(|b| b.borrow().instances.as_ptr())
}

#[no_mangle]
pub extern "C" fn wasm_instances_float_len() -> usize {
    BENCH_BOARD.with(|b| b.borrow().instances.len())
}

#[no_mangle]
pub extern "C" fn wasm_bounds_ptr() -> *const f32 {
    BENCH_BOARD.with(|b| b.borrow().bounds.as_ptr())
}
