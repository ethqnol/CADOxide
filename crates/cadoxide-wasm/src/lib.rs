pub mod altium_parser;
pub mod crdt_sync;
pub mod drc_engine;
pub mod footprint_library;
pub mod gerber_exporter;
pub mod gl_renderer;
pub mod overlay_renderer;
pub mod parser;
pub mod router;
pub mod schematic;

use crdt_sync::{CollabManager, CollabMessage, PeerCursor};
use drc_engine::{validate_track_segment, validate_via_drop, DrcViolation};
use footprint_library::{get_standard_ipc_library, FootprintDefinition};
use gl_renderer::RustGpuRenderer;
use router::{RoutingMode, TopologicalRouter};
use overlay_renderer::{render_cad_overlay, OverlayConfig};
use parser::{
    compute_vector_diff_boards, generate_hdi_stress_board, generate_template_kicad_pcb,
    parse_kicad_pcb_str, push_raw, BoardText, CadDimension, CopperZone, ParsedBoard,
    LAYER_B_CRTYD, LAYER_B_CU, LAYER_B_FAB, LAYER_B_MASK, LAYER_B_PASTE, LAYER_B_SILKS,
    LAYER_DRC_MARKER, LAYER_EDGE_CUTS, LAYER_F_CRTYD, LAYER_F_CU, LAYER_F_FAB, LAYER_F_MASK,
    LAYER_F_PASTE, LAYER_F_SILKS, LAYER_IN1_CU, LAYER_IN2_CU, LAYER_MARGIN, LAYER_REVIEW_PIN,
    LAYER_USER_CMTS, LAYER_USER_DWGS,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use std::slice;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    Document, Element, Event,
    HtmlCanvasElement, HtmlElement, HtmlInputElement, HtmlSelectElement, KeyboardEvent,
    MouseEvent, WheelEvent,
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
    pub active_tab: String,    // "home" | "pcb" | "schematic" | "diff" | "drc" | "parts"
    pub appear_subtab: String, // "layers" | "objects" | "nets"
    pub active_project: String,
    pub project_name: String,
    pub board_rev_a: ParsedBoard,
    pub board_rev_b: ParsedBoard,
    pub board_diff: ParsedBoard,
    pub diff_stats: (usize, usize, usize),
    pub current_board: ParsedBoard,
    pub undo_stack: Vec<ParsedBoard>,
    pub parse_time_ms: f64,
    pub custom_board: Option<(String, ParsedBoard)>,

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
    pub drc_violations: Vec<DrcViolation>,
    pub drc_filter: String,
    pub min_clearance_rule_mm: f32,
    pub min_track_width_rule_mm: f32,
    pub min_drill_clearance_rule_mm: f32,

    pub parts_catalog: Vec<PartLibraryEntry>,
    pub parts_filter: String,

    pub footprint_library: Vec<FootprintDefinition>,
    pub selected_footprint_id: String,
    pub footprint_category_filter: String,
    pub footprint_search_query: String,
    pub active_footprint_to_place: Option<FootprintDefinition>,
    pub next_ref_indices: std::collections::HashMap<String, usize>,
    pub render_pending: bool,

    // Phase 2: Topological Router & Online DRC Guard
    pub router: TopologicalRouter,
    pub routing_mode: RoutingMode,
    pub stop_on_drc: bool,
    pub clearance_halo: Option<((f32, f32), f32)>,
    pub target_length_tuning_mm: f32,

    // Phase 2: Real-Time Multi-Engineer Co-Layout
    pub collab: CollabManager,

    // Phase 3: Bidirectional Schematic-PCB Cross-Probing
    pub cross_probe_info: Option<String>,
    pub schematic_doc: schematic::SchematicDocument,
}

impl AppController {
    pub fn mark_render_dirty(&mut self, state: &Rc<RefCell<AppController>>) {
        if self.render_pending {
            return;
        }
        self.render_pending = true;
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move || {
            let mut a = st.borrow_mut();
            a.render_pending = false;
            a.draw_all();
        }) as Box<dyn FnMut()>);
        if let Some(win) = web_sys::window() {
            let _ = win.request_animation_frame(cb.as_ref().unchecked_ref());
        }
        cb.forget();
    }

    pub fn schedule_render(state: &Rc<RefCell<AppController>>) {
        if state.borrow().render_pending {
            return;
        }
        state.borrow_mut().render_pending = true;
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move || {
            let mut a = st.borrow_mut();
            a.render_pending = false;
            a.draw_all();
        }) as Box<dyn FnMut()>);
        if let Some(win) = web_sys::window() {
            let _ = win.request_animation_frame(cb.as_ref().unchecked_ref());
        }
        cb.forget();
    }

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
                    let bounds = CopperZone::compute_bounds(&pts, std::slice::from_ref(&pts));
                    self.current_board.zones.push(CopperZone {
                        net_id,
                        net_name,
                        layer_mask: layer,
                        is_keepout,
                        is_teardrop: false,
                        bounds,
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

        if let Some(row) = self.doc.get_element_by_id("row-target-length") {
            if tool == CadTool::TuneTrackLength {
                let _ = row.class_list().remove_1("hidden");
            } else {
                let _ = row.class_list().add_1("hidden");
            }
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
            let peer_cursors: Vec<PeerCursor> = self.collab.peers.values().cloned().collect();
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
                is_dragging: self.is_dragging || self.is_dragging_primitive,
                clearance_halo: self.clearance_halo,
                peer_cursors: &peer_cursors,
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
            let mut html = String::from(r#"<option value="0">[All Nets]</option>"#);
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
                        let pts = self.router.compute_walkaround_route(
                            &self.current_board,
                            sx,
                            sy,
                            cx,
                            cy,
                            self.active_track_width_mm,
                            self.active_draw_layer,
                            net_id,
                            self.min_clearance_rule_mm,
                            self.route_posture_flip,
                        );

                        // Fast online DRC clearance check on live preview route
                        let mut has_drc_err = false;
                        for w_pair in pts.windows(2) {
                            if let Err(viol) = validate_track_segment(
                                &self.current_board,
                                w_pair[0].0,
                                w_pair[0].1,
                                w_pair[1].0,
                                w_pair[1].1,
                                self.active_track_width_mm,
                                self.active_draw_layer,
                                net_id,
                                self.min_clearance_rule_mm,
                            ) {
                                self.clearance_halo = Some(((viol.x, viol.y), self.min_clearance_rule_mm));
                                has_drc_err = true;
                                break;
                            }
                        }
                        if !has_drc_err {
                            self.clearance_halo = None;
                        }

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
                        let target_len = self.target_length_tuning_mm.max(10.0);
                        let pts = TopologicalRouter::build_length_tuned_meanders(
                            sx,
                            sy,
                            cx,
                            cy,
                            self.active_track_width_mm,
                            target_len,
                        );
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
                    if let Some(ref fp) = self.active_footprint_to_place {
                        for pad in &fp.pads {
                            let px = cx + pad.rel_x;
                            let py = cy + pad.rel_y;
                            let layer = if pad.is_tht {
                                (LAYER_F_CU | LAYER_B_CU) as f32
                            } else {
                                self.active_draw_layer as f32
                            };
                            let prim_type = if pad.is_tht { 1.0 } else { 2.0 };
                            let h = if pad.is_tht { pad.drill } else { pad.height };
                            push_raw(
                                &mut combined,
                                &mut count,
                                px,
                                py,
                                px,
                                py,
                                pad.width,
                                h,
                                layer,
                                prim_type,
                            );
                        }
                    } else {
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

            let router_str = match self.routing_mode {
                RoutingMode::Walkaround => "Walkaround",
                RoutingMode::PushAndShove => "Push & Shove",
                RoutingMode::Free45 => "Free 45°",
            };
            let drc_status = if self.clearance_halo.is_some() {
                "DRC: Conflict"
            } else {
                "DRC: Clean"
            };

            let probe_str = if let Some(ref info) = self.cross_probe_info {
                format!(" | {}", info)
            } else {
                String::new()
            };

            el.set_text_content(Some(&format!(
                "{} | {} | {} | {} prims ({:.1} ms){}",
                mode_badge, router_str, drc_status, prims, self.parse_time_ms, probe_str
            )));
        }
    }
}

fn default_parts_catalog() -> Vec<PartLibraryEntry> {
    Vec::new()
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
        active_tab: "home".to_string(),
        appear_subtab: "layers".to_string(),
        active_project: "complex_hierarchy".to_string(),
        project_name: "complex_hierarchy".to_string(),
        board_rev_a: empty_board.clone(),
        board_rev_b: empty_board.clone(),
        board_diff: empty_board.clone(),
        diff_stats: (0, 0, 0),
        current_board: empty_board,
        undo_stack: Vec::new(),
        parse_time_ms: 0.0,
        custom_board: None,
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
        drc_violations: Vec::new(),
        drc_filter: "all".to_string(),
        min_clearance_rule_mm: 0.20,
        min_track_width_rule_mm: 0.15,
        min_drill_clearance_rule_mm: 0.35,
        parts_catalog: default_parts_catalog(),
        parts_filter: String::new(),
        footprint_library: get_standard_ipc_library(),
        selected_footprint_id: "R_0805_2012Metric".to_string(),
        footprint_category_filter: "All".to_string(),
        footprint_search_query: String::new(),
        active_footprint_to_place: None,
        next_ref_indices: std::collections::HashMap::new(),
        render_pending: false,
        router: TopologicalRouter::new(),
        routing_mode: RoutingMode::Walkaround,
        stop_on_drc: true,
        clearance_halo: None,
        target_length_tuning_mm: 50.0,
        collab: CollabManager::new(),
        cross_probe_info: None,
        schematic_doc: schematic::SchematicDocument::new(),
    }));

    build_shell_dom(&doc, &root, state.clone());
    fetch_project_boards(state.clone(), "complex_hierarchy");
    sync_backend_parts(state.clone());
    init_collab_websocket(state.clone());
    switch_view_tab(&state, "home");
}

fn build_shell_dom(doc: &Document, root: &Element, state: Rc<RefCell<AppController>>) {
    doc.set_title("CADOxide - PCB Editor");
    root.set_inner_html(
        r##"
        <div class="cad-shell">
          <!-- Row 1: Native Desktop Application Menu Bar + Project Picker + Quick Actions -->
          <div class="cad-topmenu">
            <div class="cad-menu-items">
              <div id="brand-home-link" class="cad-brand" title="Return to Projects Home (Click to open project manager)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                  <rect x="1" y="1" width="14" height="14" rx="2" stroke="#ef4444" stroke-width="1.5"/>
                  <circle cx="5" cy="5" r="1.5" fill="#60a5fa"/>
                  <circle cx="11" cy="11" r="1.5" fill="#60a5fa"/>
                  <path d="M5 5L8 8H11V11" stroke="#ef4444" stroke-width="1.4" stroke-linecap="round"/>
                </svg>
                <span>CADOxide</span>
              </div>
              <div class="cad-divider-v"></div>
              <div class="cad-menu-dropdown-wrap">
                <button id="menu-btn-file" class="cad-menu-btn" data-menu="file">File ▾</button>
                <div class="cad-menu-dropdown hidden" id="dropdown-menu-file">
                  <div class="cad-menu-dropdown-item" id="menu-action-new">
                    <span>New Project...</span>
                    <span class="cad-menu-shortcut">Ctrl+N</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-open">
                    <span>Open .kicad_pcb...</span>
                    <span class="cad-menu-shortcut">Ctrl+O</span>
                  </div>
                  <div class="cad-menu-divider"></div>
                  <div class="cad-menu-dropdown-item" id="menu-action-save">
                    <span>Save / Export .kicad_pcb</span>
                    <span class="cad-menu-shortcut">Ctrl+S</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-gerber">
                    <span>Export Gerbers &amp; Drill ZIP</span>
                    <span class="cad-menu-shortcut">CAM</span>
                  </div>
                  <div class="cad-menu-divider"></div>
                  <div class="cad-menu-dropdown-item" id="menu-action-home">
                    <span>Projects Home Hub</span>
                    <span class="cad-menu-shortcut">Alt+H</span>
                  </div>
                </div>
              </div>

              <div class="cad-menu-dropdown-wrap">
                <button id="menu-btn-edit" class="cad-menu-btn" data-menu="edit">Edit ▾</button>
                <div class="cad-menu-dropdown hidden" id="dropdown-menu-edit">
                  <div class="cad-menu-dropdown-item" id="menu-action-undo">
                    <span>Undo</span>
                    <span class="cad-menu-shortcut">Ctrl+Z</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-rotate">
                    <span>Rotate Selected 90°</span>
                    <span class="cad-menu-shortcut">R</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-delete">
                    <span>Delete Selected Item</span>
                    <span class="cad-menu-shortcut">Del</span>
                  </div>
                  <div class="cad-menu-divider"></div>
                  <div class="cad-menu-dropdown-item" id="menu-action-deselect">
                    <span>Deselect All</span>
                    <span class="cad-menu-shortcut">Esc</span>
                  </div>
                </div>
              </div>

              <div class="cad-menu-dropdown-wrap">
                <button id="menu-btn-view" class="cad-menu-btn" data-menu="view">View ▾</button>
                <div class="cad-menu-dropdown hidden" id="dropdown-menu-view">
                  <div class="cad-menu-dropdown-item" id="menu-action-view-pcb">
                    <span>PCB 2D Layout Editor</span>
                    <span class="cad-menu-shortcut">1</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-view-3d">
                    <span>Toggle 3D Orbit View</span>
                    <span class="cad-menu-shortcut">3</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-view-sch">
                    <span>Schematic Sheet Viewer</span>
                    <span class="cad-menu-shortcut">2</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-view-diff">
                    <span>Visual Layer Revision Diff</span>
                    <span class="cad-menu-shortcut">4</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-view-drc">
                    <span>DRC Rule Workbench</span>
                    <span class="cad-menu-shortcut">5</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-view-bom">
                    <span>BOM &amp; Component Catalog</span>
                    <span class="cad-menu-shortcut">6</span>
                  </div>
                  <div class="cad-menu-divider"></div>
                  <div class="cad-menu-dropdown-item" id="menu-action-zoom-fit">
                    <span>Zoom to Fit Board</span>
                    <span class="cad-menu-shortcut">Home</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-zoom-in">
                    <span>Zoom In</span>
                    <span class="cad-menu-shortcut">+</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-zoom-out">
                    <span>Zoom Out</span>
                    <span class="cad-menu-shortcut">−</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-contrast">
                    <span>High-Contrast Mode</span>
                    <span class="cad-menu-shortcut">H</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-flip">
                    <span>Flip Board View</span>
                    <span class="cad-menu-shortcut">F</span>
                  </div>
                </div>
              </div>

              <div class="cad-menu-dropdown-wrap">
                <button id="menu-btn-place" class="cad-menu-btn" data-menu="place">Place ▾</button>
                <div class="cad-menu-dropdown hidden" id="dropdown-menu-place">
                  <div class="cad-menu-dropdown-item" id="menu-action-place-pad">
                    <span>Footprint / SMD Pad</span>
                    <span class="cad-menu-shortcut">P</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-fp-lib">
                    <span>Browse IPC Footprint Library...</span>
                    <span class="cad-menu-shortcut">A</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-via">
                    <span>Freestanding Via</span>
                    <span class="cad-menu-shortcut">V</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-zone">
                    <span>Filled Copper Zone</span>
                    <span class="cad-menu-shortcut">Z</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-keepout">
                    <span>Rule / Keepout Zone</span>
                    <span class="cad-menu-shortcut">K</span>
                  </div>
                  <div class="cad-menu-divider"></div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-line">
                    <span>Graphic Line</span>
                    <span class="cad-menu-shortcut">L</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-arc">
                    <span>Graphic Arc</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-circle">
                    <span>Graphic Circle</span>
                    <span class="cad-menu-shortcut">C</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-rect">
                    <span>Graphic Rectangle</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-poly">
                    <span>Graphic Polygon</span>
                    <span class="cad-menu-shortcut">Y</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-text">
                    <span>Text Item</span>
                    <span class="cad-menu-shortcut">T</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-dim">
                    <span>Linear Dimension</span>
                    <span class="cad-menu-shortcut">D</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-place-origin">
                    <span>Grid / Drill Origin</span>
                    <span class="cad-menu-shortcut">O</span>
                  </div>
                </div>
              </div>

              <div class="cad-menu-dropdown-wrap">
                <button id="menu-btn-route" class="cad-menu-btn" data-menu="route">Route ▾</button>
                <div class="cad-menu-dropdown hidden" id="dropdown-menu-route">
                  <div class="cad-menu-dropdown-item" id="menu-action-route-single">
                    <span>Interactive Single Track</span>
                    <span class="cad-menu-shortcut">X</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-route-diffpair">
                    <span>Differential Pair Route</span>
                    <span class="cad-menu-shortcut">6</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-route-tune">
                    <span>Length Tuning / Serpentine Meander</span>
                    <span class="cad-menu-shortcut">7</span>
                  </div>
                  <div class="cad-menu-divider"></div>
                  <div class="cad-menu-dropdown-item" id="menu-action-route-walk">
                    <span>Obstacle Mode: Walkaround</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-route-push">
                    <span>Obstacle Mode: Push &amp; Shove</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-route-free45">
                    <span>Obstacle Mode: Free 45°</span>
                  </div>
                </div>
              </div>

              <div class="cad-menu-dropdown-wrap">
                <button id="menu-btn-inspect" class="cad-menu-btn" data-menu="inspect">Inspect ▾</button>
                <div class="cad-menu-dropdown hidden" id="dropdown-menu-inspect">
                  <div class="cad-menu-dropdown-item" id="menu-action-inspect-drc">
                    <span>Design Rules Checker (DRC)</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-inspect-run-drc">
                    <span>Run Live Clearance DRC</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-inspect-measure">
                    <span>Measure Distance Between Points</span>
                    <span class="cad-menu-shortcut">M</span>
                  </div>
                  <div class="cad-menu-divider"></div>
                  <div class="cad-menu-dropdown-item" id="menu-action-inspect-bom">
                    <span>Bill of Materials &amp; Parts Sourcing</span>
                  </div>
                </div>
              </div>

              <div class="cad-menu-dropdown-wrap">
                <button id="menu-btn-tools" class="cad-menu-btn" data-menu="tools">Tools ▾</button>
                <div class="cad-menu-dropdown hidden" id="dropdown-menu-tools">
                  <div class="cad-menu-dropdown-item" id="menu-action-tools-fill">
                    <span>Fill All Copper Zones</span>
                    <span class="cad-menu-shortcut">B</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-tools-outline">
                    <span>Show Only Zone Outlines</span>
                  </div>
                  <div class="cad-menu-dropdown-item" id="menu-action-tools-hide-zones">
                    <span>Do Not Show Copper Zones</span>
                  </div>
                  <div class="cad-menu-divider"></div>
                  <div class="cad-menu-dropdown-item" id="menu-action-tools-gerbers">
                    <span>Generate Fabrication Gerbers (.zip)</span>
                  </div>
                </div>
              </div>
            </div>

            <!-- Right: Active Project Switcher + Quick Actions -->
            <div class="cad-menubar-group">
              <div class="cad-project-picker" style="display:flex;align-items:center;gap:5px;background:#101319;border:1px solid #28303d;border-radius:4px;padding:1px 6px;height:22px;">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="#60a5fa" stroke-width="2">
                  <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                </svg>
                <span style="font-size:10px;color:#717a8c;font-weight:600;text-transform:uppercase;letter-spacing:0.03em;">Project:</span>
                <select id="rust-project-select" class="cad-select" style="border:none;background:transparent;padding:0 16px 0 2px;height:20px;font-size:11px;font-weight:600;color:#e2e8f0;cursor:pointer;max-width:180px;" title="Switch active project">
                  <option value="complex_hierarchy">Audio Power Amp</option>
                  <option value="cm5_minima">CM5 Carrier Board</option>
                </select>
              </div>
              <div class="cad-divider-v"></div>
              <button id="btn-topbar-new" class="cad-btn" style="background:#142d1f;border-color:#22543d;color:#86efac;font-weight:600;" title="Start a new PCB project from template">+ New</button>
              <button id="btn-topbar-open" class="cad-btn" style="background:#1e232e;border-color:#333c4d;color:#e2e8f0;" title="Open .kicad_pcb file from computer">Open...</button>
              <button id="btn-export-pcb" class="cad-btn" style="background:#1e293b;border-color:#334155;color:#f8fafc;" title="Save / Download .kicad_pcb">Save</button>
              <a id="btn-export-gerbers" href="#" class="cad-btn" style="background:#162e24;border-color:#22543d;color:#6ee7b7;font-weight:600;" title="Generate &amp; Download Complete RS-274X Gerber &amp; NC Drill Fabrication ZIP">Gerbers</a>
              <div class="cad-divider-v"></div>
              <div id="collab-status-pill" class="cad-btn" style="background:#0d2319;border-color:#166534;color:#4ade80;font-size:11px;font-weight:600;display:flex;align-items:center;gap:6px;" title="Real-Time Co-Layout Connection Status">
                <span style="width:6px;height:6px;border-radius:50%;background:#22c55e;"></span>
                <span id="collab-status-text">Co-Layout Live</span>
              </div>
            </div>
          </div>

          <!-- Row 2: Workspace View Mode Tabs & Navigation Strip -->
          <div class="cad-tabbar">
            <div class="cad-tabs">
              <button id="tab-home" class="cad-tab cad-tab-active" title="Projects &amp; Templates Home (Alt+H)">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"></path><polyline points="9 22 9 12 15 12 15 22"></polyline></svg>
                <span>Home</span>
              </button>
              <button id="tab-pcb" class="cad-tab" title="2D Interactive PCB Layout Editor (1)">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="2" y="2" width="20" height="20" rx="3"></rect><circle cx="8" cy="8" r="2"></circle><circle cx="16" cy="16" r="2"></circle><path d="M8 10v4h8"></path></svg>
                <span>PCB Editor</span>
              </button>
              <button id="tab-schematic" class="cad-tab" title="Schematic Sheet Viewer (2)">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="3" width="18" height="18" rx="2"></rect><line x1="3" y1="9" x2="21" y2="9"></line><line x1="9" y1="21" x2="9" y2="9"></line></svg>
                <span>Schematic</span>
              </button>
              <button id="tab-diff" class="cad-tab" title="Visual Layer Revision Diff (4)">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="18" cy="18" r="3"></circle><circle cx="6" cy="6" r="3"></circle><path d="M13 6h3a2 2 0 0 1 2 2v7"></path><line x1="6" y1="9" x2="6" y2="21"></line></svg>
                <span>Layer Diff</span>
              </button>
              <button id="tab-drc" class="cad-tab" title="DRC Rule Workbench (5)">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path><line x1="12" y1="9" x2="12" y2="13"></line><line x1="12" y1="17" x2="12.01" y2="17"></line></svg>
                <span>DRC (<span id="drc-count-badge">0</span>)</span>
              </button>
              <button id="tab-parts" class="cad-tab" title="BOM &amp; Component Sourcing (6)">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="8" y1="6" x2="21" y2="6"></line><line x1="8" y1="12" x2="21" y2="12"></line><line x1="8" y1="18" x2="21" y2="18"></line><line x1="3" y1="6" x2="3.01" y2="6"></line><line x1="3" y1="12" x2="3.01" y2="12"></line><line x1="3" y1="18" x2="3.01" y2="18"></line></svg>
                <span>BOM (<span id="bom-count-badge">0</span>)</span>
              </button>
            </div>

            <!-- Right: Canvas Shortcuts & Footprint Browser Trigger -->
            <div id="tabbar-canvas-actions" class="cad-menubar-group">
              <button id="btn-undo" class="cad-btn" title="Undo (Ctrl+Z)">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7v6h6"/><path d="M21 17a9 9 0 0 0-9-9 9 9 0 0 0-6 2.3L3 13"/></svg>
                <span>Undo</span>
              </button>
              <button id="btn-rotate-selected" class="cad-btn" title="Rotate Selected 90° (R)">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.85.83 6.72 2.24L21 8"/><path d="M21 3v5h-5"/></svg>
              </button>
              <button id="btn-delete-selected" class="cad-btn cad-btn-danger" title="Delete Selected Item (Del)">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
              </button>
              <div class="cad-divider-v"></div>
              <button id="btn-zoom-in" class="cad-btn" title="Zoom In (+)">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/><line x1="11" y1="8" x2="11" y2="14"/><line x1="8" y1="11" x2="14" y2="11"/></svg>
              </button>
              <button id="btn-zoom-out" class="cad-btn" title="Zoom Out (-)">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/><line x1="8" y1="11" x2="14" y2="11"/></svg>
              </button>
              <button id="btn-zoom-fit" class="cad-btn" title="Zoom to Fit Board (Home)">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><path d="M8 3H5a2 2 0 0 0-2 2v3m18 0V5a2 2 0 0 0-2-2h-3m0 18h3a2 2 0 0 0 2-2v-3M3 16v3a2 2 0 0 0 2 2h3"/></svg>
                <span>Fit</span>
              </button>
              <div class="cad-divider-v"></div>
              <button id="btn-open-fp-browser" class="cad-btn" style="background:#162436;border-color:#388bfd;color:#79b8ff;font-weight:600;" title="Browse IPC-7351 Standard Component Footprints (A)">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8" cy="8" r="1.5"/><circle cx="16" cy="8" r="1.5"/><circle cx="8" cy="16" r="1.5"/><circle cx="16" cy="16" r="1.5"/></svg>
                <span>Footprints (A)</span>
              </button>
            </div>
          </div>

          <!-- Row 3: Active PCB Editor Parameters & Tooling -->
          <div id="cad-param-bar" class="cad-param-bar">
            <div class="cad-menubar-group">
              <span class="cad-param-label">Layer:</span>
              <select id="select-draw-layer" class="cad-select" style="width:90px;" title="Active Drawing / Routing Layer">
                <option value="1">F.Cu</option>
                <option value="4096">In1.Cu</option>
                <option value="8192">In2.Cu</option>
                <option value="2">B.Cu</option>
                <option value="16384">F.Paste</option>
                <option value="32768">B.Paste</option>
                <option value="8">F.Silkscreen</option>
                <option value="16">B.Silkscreen</option>
                <option value="32">F.Mask</option>
                <option value="64">B.Mask</option>
                <option value="1048576">Drawings</option>
                <option value="2097152">Comments</option>
                <option value="4">Edge.Cuts</option>
                <option value="4194304">Margin</option>
                <option value="65536">F.CrtYd</option>
                <option value="131072">B.CrtYd</option>
                <option value="262144">F.Fab</option>
                <option value="524288">B.Fab</option>
              </select>

              <span class="cad-param-label">W:</span>
              <select id="select-track-width" class="cad-select" style="width:78px;" title="Track Width">
                <option value="0.127">0.127 mm</option>
                <option value="0.200">0.200 mm</option>
                <option value="0.250" selected>0.250 mm</option>
                <option value="0.400">0.400 mm</option>
                <option value="0.800">0.800 mm</option>
              </select>

              <span class="cad-param-label">Via:</span>
              <select id="select-via-size" class="cad-select" style="width:86px;" title="Via Size / Drill">
                <option value="0.45,0.20">0.45/0.20</option>
                <option value="0.60,0.30" selected>0.60/0.30</option>
                <option value="0.80,0.40">0.80/0.40</option>
              </select>

              <span class="cad-param-label">Grid:</span>
              <select id="select-grid-snap" class="cad-select" style="width:74px;" title="Grid Step Size">
                <option value="0.05">0.05 mm</option>
                <option value="0.10">0.10 mm</option>
                <option value="0.25" selected>0.25 mm</option>
                <option value="0.50">0.50 mm</option>
                <option value="1.00">1.00 mm</option>
              </select>

              <span class="cad-param-label">Net:</span>
              <select id="rust-net-select" class="cad-select" style="width:115px;" title="Highlight electrical net across all layers">
                <option value="0">[All Nets]</option>
              </select>

              <div class="cad-divider-v"></div>
              <span class="cad-param-label">Router:</span>
              <select id="select-routing-mode" class="cad-select" style="width:125px;" title="Interactive Routing & Obstacle Handling Mode">
                <option value="walkaround" selected>Walkaround (Hug)</option>
                <option value="push">Push &amp; Shove</option>
                <option value="free45">Free 45°</option>
              </select>

              <label style="display:flex;align-items:center;gap:4px;font-size:11px;color:#cbd5e1;cursor:pointer;" title="Enforce online DRC rules during routing and placement">
                <input type="checkbox" id="check-stop-on-drc" checked style="accent-color:#388bfd;" /> DRC Guard
              </label>

              <div id="row-target-length" class="hidden" style="display:flex;align-items:center;gap:4px;">
                <span class="cad-param-label">Target:</span>
                <input id="input-target-length" type="number" step="1.0" min="5.0" max="500.0" value="50.0" class="cad-input" style="width:50px;" title="Target routed net length for serpentine tuning (mm)" />
                <span style="font-size:10px;color:#8892a4;">mm</span>
              </div>

              <div class="cad-divider-v"></div>
              <button id="btn-3d-stackup" class="cad-btn" title="Toggle 3D Multi-Layer Stackup Viewer (3)">3D</button>
            </div>

            <!-- Contextual 3D Stackup Controls -->
            <div id="toolbar-3d-controls" class="hidden cad-menubar-group">
              <div class="cad-divider-v"></div>
              <button id="btn-3d-drag-mode" class="cad-btn cad-btn-active" title="Toggle Left-Drag between 3D Orbit and 3D Pan">Orbit 3D</button>
              <button id="btn-3d-iso" class="cad-btn">Iso</button>
              <button id="btn-3d-top" class="cad-btn">Top</button>
              <button id="btn-3d-flip" class="cad-btn">Flip</button>
              <input id="input-3d-explode" type="range" min="20" max="250" value="100" style="width:55px;accent-color:#388bfd;cursor:pointer;" />
            </div>

            <!-- Contextual Diff Controls -->
            <div id="diff-toolbar-controls" class="hidden cad-menubar-group">
              <div class="cad-divider-v"></div>
              <button id="btn-diff-boolean" class="cad-btn cad-btn-active">Overlay</button>
              <button id="btn-diff-wipe" class="cad-btn">Split Wipe</button>
              <input id="input-wipe-slider" type="range" min="0" max="100" value="50" style="width:65px;accent-color:#388bfd;cursor:pointer;" />
            </div>
          </div>

          <!-- Main CAD Workspace (Left View Options Strip + Center Canvas + Right Routing/Drawing Strip + Right Appearance Dock) -->
          <div class="cad-workspace">
            <!-- Left Vertical Strip: KiCad Display & Viewing Options -->
            <aside id="cad-left-strip" class="cad-toolstrip" title="KiCad Display & Viewing Options">
              <button id="btn-opt-grid" class="cad-tool-btn cad-tool-btn-active" title="Show/Hide Grid">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><line x1="5.5" y1="2" x2="5.5" y2="14"/><line x1="10.5" y1="2" x2="10.5" y2="14"/><line x1="2" y1="5.5" x2="14" y2="5.5"/><line x1="2" y1="10.5" x2="14" y2="10.5"/></svg>
              </button>
              <button id="btn-opt-polar" class="cad-tool-btn" title="Toggle Polar Coordinates (R / θ)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M14 13H3L11 3"/><path d="M7 13 A5 5 0 0 0 5.5 10"/></svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-unit-in" class="cad-tool-btn" title="Set Units to Inches (in)">in</button>
              <button id="btn-unit-mil" class="cad-tool-btn" title="Set Units to Mils (mil)">mil</button>
              <button id="btn-unit-mm" class="cad-tool-btn cad-tool-btn-active" title="Set Units to Millimeters (mm)">mm</button>
              <div class="cad-tool-sep"></div>
              <button id="btn-opt-crosshair" class="cad-tool-btn" title="Toggle Full-Window Crosshair Cursor">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><line x1="8" y1="2" x2="8" y2="14"/><line x1="2" y1="8" x2="14" y2="8"/></svg>
              </button>
              <button id="btn-opt-pad-labels" class="cad-tool-btn cad-tool-btn-active" title="Show/Hide On-Pad Pin Numbers & Net Names">1/N</button>
              <button id="btn-opt-fp-text" class="cad-tool-btn cad-tool-btn-active" title="Show/Hide Footprint Reference & Value Labels">Ref</button>
              <button id="btn-opt-sketch" class="cad-tool-btn" title="Toggle Sketch / Outline Mode for Tracks & Pads">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><polygon points="3.5,12 6.5,4 13.5,4 10.5,12"/></svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-zone-filled" class="cad-tool-btn cad-tool-btn-active" title="Show Filled Areas of Zones">
                <svg viewBox="0 0 16 16" fill="currentColor"><rect x="2.5" y="2.5" width="11" height="11" rx="1.5"/></svg>
              </button>
              <button id="btn-zone-outline" class="cad-tool-btn" title="Show Only Zone Boundaries (Outlines)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><rect x="2.5" y="2.5" width="11" height="11" rx="1"/></svg>
              </button>
              <button id="btn-zone-off" class="cad-tool-btn" title="Do Not Show Copper Zones">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="2.5" y="2.5" width="11" height="11" rx="1" stroke-dasharray="2.5,1.5"/><line x1="3" y1="13" x2="13" y2="3" stroke-width="1.8"/></svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-opt-ratsnest" class="cad-tool-btn" title="Show/Hide Ratsnest Airwires">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="3" cy="13" r="1.6" fill="currentColor"/><circle cx="13" cy="3" r="1.6" fill="currentColor"/><line x1="3" y1="13" x2="13" y2="3" stroke-dasharray="2,2"/></svg>
              </button>
              <button id="btn-opt-highcontrast" class="cad-tool-btn" title="Cycle High-Contrast Mode (H): Normal / Dim / Hide Inactive">
                <svg viewBox="0 0 16 16" fill="currentColor"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 2 A6 6 0 0 1 8 14 Z"/></svg>
              </button>
              <button id="btn-opt-flipview" class="cad-tool-btn" title="Flip Board View Horizontally (F)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M2 5h11l-3-3"/><path d="M14 11H3l3 3"/></svg>
              </button>
            </aside>

            <!-- Center GPU PCB Canvas + Synchronized 2D CAD Overlay Canvas -->
            <div id="center-viewport" class="cad-viewport">
              <canvas id="rust-gpu-canvas" class="cad-canvas"></canvas>
              <canvas id="cad-overlay-canvas" class="cad-overlay-canvas"></canvas>

              <!-- Home Screen / Project Manager Hub View -->
              <div id="home-view-panel" class="home-view-panel">
                <div class="home-container">
                  <!-- Header: Title & Quick Actions -->
                  <div class="home-header">
                    <div>
                      <div class="home-title">CADoxide Projects</div>
                      <div class="home-subtitle">Select an existing board, start from a standard stackup, or import a KiCad PCB.</div>
                    </div>
                    <div class="home-actions">
                      <button id="btn-home-new-proj" class="home-btn-primary" title="Start a new PCB project from template">
                        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="12" y1="5" x2="12" y2="19"></line><line x1="5" y1="12" x2="19" y2="12"></line></svg>
                        <span>New Project</span>
                      </button>
                      <button id="btn-home-open-file" class="home-btn-secondary" title="Open .kicad_pcb file from disk">
                        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>
                        <span>Open File...</span>
                      </button>
                      <button id="btn-home-return-editor" class="home-btn-editor" title="Return to active PCB editor workspace">
                        <span>Active Editor</span>
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="9 18 15 12 9 6"></polyline></svg>
                      </button>
                    </div>
                  </div>

                  <!-- Quick Import Drop Strip -->
                  <div id="home-dropzone" class="home-drop-strip" title="Click or drop a .kicad_pcb file here to open">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#60a5fa" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline points="17 8 12 3 7 8"></polyline><line x1="12" y1="3" x2="12" y2="15"></line></svg>
                    <span class="home-drop-text">Drop a <code>.kicad_pcb</code> file here to open directly, or click to browse local files</span>
                    <input type="file" id="home-file-input" accept=".kicad_pcb" style="display:none;" />
                  </div>

                  <!-- Projects List Section -->
                  <div class="home-section">
                    <div class="home-section-header">
                      <span class="home-section-title">Projects</span>
                      <span style="font-size:11px;color:#64748b;">Click to open in editor</span>
                    </div>
                    <div class="home-project-list">
                      <!-- Project 1: Audio Power Amp -->
                      <div class="home-project-row" id="card-proj-complex" title="Open Audio Power Amp">
                        <div class="home-row-main">
                          <div class="home-row-icon">
                            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#94a3b8" stroke-width="1.8"><rect x="2" y="2" width="20" height="20" rx="3"></rect><path d="M7 2v20M17 2v20M2 12h20M2 7h5M2 17h5M17 17h5M17 7h5"></path></svg>
                          </div>
                          <div>
                            <div class="home-row-name">Audio Power Amp</div>
                            <div class="home-row-file">samples/complex_hierarchy/complex_hierarchy.kicad_pcb</div>
                          </div>
                        </div>
                        <div class="home-row-meta">
                          <span class="home-pill">2-Layer Copper</span>
                          <span class="home-pill">Hierarchical Schematic</span>
                        </div>
                        <button class="home-row-btn">
                          <span>Open</span>
                          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="9 18 15 12 9 6"></polyline></svg>
                        </button>
                      </div>

                      <!-- Project 2: CM5 Carrier Board -->
                      <div class="home-project-row" id="card-proj-cm5" title="Open CM5 Carrier Board">
                        <div class="home-row-main">
                          <div class="home-row-icon">
                            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#94a3b8" stroke-width="1.8"><rect x="2" y="2" width="20" height="20" rx="3"></rect><circle cx="12" cy="12" r="3"></circle><path d="M12 2v4M12 18v4M2 12h4M18 12h4"></path></svg>
                          </div>
                          <div>
                            <div class="home-row-name">CM5 Carrier Board</div>
                            <div class="home-row-file">samples/cm5_minima/CM5_MINIMA_3.kicad_pcb</div>
                          </div>
                        </div>
                        <div class="home-row-meta">
                          <span class="home-pill">4-Layer Stackup</span>
                          <span class="home-pill">Compute Module 5</span>
                        </div>
                        <button class="home-row-btn">
                          <span>Open</span>
                          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="9 18 15 12 9 6"></polyline></svg>
                        </button>
                      </div>

                      <div id="home-custom-projects-container"></div>
                    </div>
                  </div>

                  <!-- Standard Templates Section -->
                  <div class="home-section">
                    <div class="home-section-header">
                      <span class="home-section-title">New Board Templates</span>
                    </div>
                    <div class="home-templates-grid">
                      <div class="home-tmpl-box" id="card-tmpl-blank" title="Create 2-layer 100x80mm board">
                        <div class="home-tmpl-top">
                          <span class="home-tmpl-title">Blank 2-Layer</span>
                          <span class="home-pill">100 × 80 mm</span>
                        </div>
                        <div class="home-tmpl-desc">Standard rectangular outline with M3 corner mounting holes.</div>
                      </div>

                      <div class="home-tmpl-box" id="card-tmpl-rpi" title="Create Raspberry Pi HAT board">
                        <div class="home-tmpl-top">
                          <span class="home-tmpl-title">Raspberry Pi HAT</span>
                          <span class="home-pill">65 × 56 mm</span>
                        </div>
                        <div class="home-tmpl-desc">Official Pi 4/5 HAT standard form factor with 40-pin GPIO.</div>
                      </div>

                      <div class="home-tmpl-box" id="card-tmpl-iot" title="Create IoT sensor board">
                        <div class="home-tmpl-top">
                          <span class="home-tmpl-title">Compact Sensor</span>
                          <span class="home-pill">50 × 40 mm</span>
                        </div>
                        <div class="home-tmpl-desc">Compact dual-layer outline for MCU and BLE modules.</div>
                      </div>

                      <div class="home-tmpl-box" id="card-tmpl-eurocard" title="Create industrial Eurocard 3U board">
                        <div class="home-tmpl-top">
                          <span class="home-tmpl-title">Eurocard 3U</span>
                          <span class="home-pill">160 × 100 mm</span>
                        </div>
                        <div class="home-tmpl-desc">Standard 3U 19-inch subrack enclosure format.</div>
                      </div>
                    </div>
                  </div>

                </div>
              </div>

              <!-- Interactive Pan & Wheel-Zoom Schematic Editor View -->
              <div id="schematic-view-panel" class="hidden" style="position:absolute;inset:0;display:flex;flex-direction:column;background:#0d1117;z-index:30;">
                <div style="height:32px;background:#161b22;border-bottom:1px solid #30363d;display:flex;align-items:center;justify-content:space-between;padding:0 10px;">
                  <div style="display:flex;align-items:center;gap:6px;">
                    <button id="btn-sch-nav-up" class="cad-btn" style="padding:2px 8px;font-size:11px;" title="Navigate Up to Parent Schematic Sheet">▲ Up</button>
                    <span class="cad-param-label" style="color:#8b949e;font-size:11px;">Hierarchy:</span>
                    <div id="sch-breadcrumb-list" style="display:flex;align-items:center;gap:4px;"></div>
                  </div>
                  <div style="display:flex;align-items:center;gap:6px;">
                    <span style="font-size:11px;color:#8b949e;">Wheel to Zoom, Drag to Pan</span>
                    <button id="btn-sch-reset" class="cad-btn" style="padding:2px 8px;font-size:11px;">Reset Zoom (100%)</button>
                  </div>
                </div>
                <div id="schematic-viewport" style="flex:1;overflow:hidden;position:relative;background:#0d1117;cursor:grab;display:flex;align-items:center;justify-content:center;">
                  <div id="schematic-canvas-inner" style="transform-origin:center center;transition:none;display:flex;align-items:center;justify-content:center;width:100%;height:100%;">
                    <!-- Interactive SVG injected here by render_active_schematic_sheet() -->
                  </div>
                  <div id="sch-cross-probe-pill" style="position:absolute;bottom:14px;left:14px;background:rgba(15,23,42,0.92);backdrop-filter:blur(6px);border:1px solid #0284c7;border-radius:6px;padding:6px 12px;display:flex;align-items:center;gap:8px;font-size:11px;color:#e2e8f0;box-shadow:0 4px 12px rgba(0,0,0,0.5);z-index:40;pointer-events:none;">
                    <span style="display:inline-block;width:8px;height:8px;border-radius:50%;background:#38bdf8;box-shadow:0 0 6px #38bdf8;"></span>
                    <span id="sch-cross-probe-label">Cross-Probe Active: Click schematic symbol/wire to highlight in PCB layout</span>
                  </div>
                </div>
              </div>

              <!-- DRC Interactive Workbench View -->
              <div id="drc-view-panel" class="hidden" style="position:absolute;inset:0;display:flex;flex-direction:column;background:#121418;z-index:30;">
                <div class="cad-panel-header" style="height:38px;padding:0 14px;background:#161922;border-bottom:1px solid #282d3b;display:flex;align-items:center;justify-content:space-between;">
                  <div style="display:flex;align-items:center;gap:10px;">
                    <span style="font-weight:700;color:#f1f5f9;">DRC RULE CHECKER</span>
                    <button id="btn-run-drc" class="cad-btn" style="background:#1e3a5f;border-color:#388bfd;color:#79b8ff;font-weight:700;" title="Run live DRC rule check across all copper layers, pads, and nets">
                      <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor"><polygon points="5 3 19 12 5 21 5 3"/></svg>
                      <span>Run DRC</span>
                    </button>
                    <div class="cad-divider-v"></div>
                    <span class="cad-param-label">Min Spacing:</span>
                    <input id="input-drc-clearance" type="number" step="0.05" min="0.05" max="2.0" value="0.20" class="cad-input" style="width:55px;" />
                    <span style="font-size:10px;color:#8892a4;">mm</span>
                  </div>
                  <div style="display:flex;align-items:center;gap:5px;">
                    <button class="drc-filter-btn active" data-filter="all">All (<span id="drc-cnt-all">0</span>)</button>
                    <button class="drc-filter-btn" data-filter="error" style="color:#f87171;">Errors (<span id="drc-cnt-err">0</span>)</button>
                    <button class="drc-filter-btn" data-filter="warning" style="color:#fbbf24;">Warnings (<span id="drc-cnt-warn">0</span>)</button>
                    <button class="drc-filter-btn" data-filter="Clearance">Clearance</button>
                    <button class="drc-filter-btn" data-filter="Short">Shorts</button>
                    <button class="drc-filter-btn" data-filter="Unconnected">Unconnected</button>
                  </div>
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
            <aside id="cad-right-toolstrip" class="cad-toolstrip-right" title="KiCad Routing, Placement & Drawing Tools">
              <button id="btn-tool-select" class="cad-tool-btn cad-tool-btn-active" title="Select item(s) (S)">
                <svg width="13" height="13" viewBox="0 0 16 16" fill="currentColor"><path d="M3 2L13 7.5L8.5 9L7 13.5L3 2Z"/></svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-tool-route" class="cad-tool-btn" title="Route single track (X) — Press / to flip 45° posture">
                <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><path d="M2.5 13.5L6.5 9.5H10.5L13.5 6.5"/><circle cx="2.5" cy="13.5" r="1.4" fill="currentColor"/><circle cx="13.5" cy="6.5" r="1.4" fill="currentColor"/></svg>
              </button>
              <button id="btn-tool-diffpair" class="cad-tool-btn" title="Route differential pair (6)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 13.5L5.5 10H9.5L13 6.5"/><circle cx="2" cy="13.5" r="1.2" fill="currentColor"/><circle cx="13" cy="6.5" r="1.2" fill="currentColor"/><path d="M3.5 15L7 11.5H11L14.5 8"/><circle cx="3.5" cy="15" r="1.2" fill="currentColor"/><circle cx="14.5" cy="8" r="1.2" fill="currentColor"/></svg>
              </button>
              <button id="btn-tool-tune" class="cad-tool-btn" title="Tune length of a single track / Serpentine meander (7)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M2 8.5h2v-4.5h3.5v8h3.5v-8h3.5v4.5h1.5"/></svg>
              </button>
              <button id="btn-tool-via" class="cad-tool-btn" title="Add freestanding vias (V)">
                <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="8" r="5.2"/><circle cx="8" cy="8" r="2.1"/></svg>
              </button>
              <button id="btn-tool-pad" class="cad-tool-btn" title="Add footprint / SMD pad (P)">
                <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="3" y="4.5" width="10" height="7" rx="1.5"/></svg>
              </button>
              <button id="btn-tool-zone" class="cad-tool-btn" title="Add filled copper zone (Z) — Click polygon corners, Right-Click to close">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><polygon points="3,3.5 13,3.5 13,12.5 8.5,13.5 3,11"/><line x1="3" y1="6.5" x2="13" y2="6.5"/><line x1="3" y1="9.5" x2="13" y2="9.5"/></svg>
              </button>
              <button id="btn-tool-keepout" class="cad-tool-btn" title="Add rule area / keepout zone (K)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><rect x="2.5" y="2.5" width="11" height="11" rx="1.5" stroke-dasharray="2,1"/><line x1="3.5" y1="12.5" x2="12.5" y2="3.5" stroke-width="1.6"/></svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-tool-line" class="cad-tool-btn" title="Draw a line (L)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><line x1="3" y1="13" x2="13" y2="3"/></svg>
              </button>
              <button id="btn-tool-arc" class="cad-tool-btn" title="Draw an arc (A)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M3 12 A 6.5 6.5 0 0 1 13 12"/></svg>
              </button>
              <button id="btn-tool-circle" class="cad-tool-btn" title="Draw a circle (C)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><circle cx="8" cy="8" r="5.5"/></svg>
              </button>
              <button id="btn-tool-rect" class="cad-tool-btn" title="Draw a rectangle">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><rect x="2.5" y="4" width="11" height="8" rx="0.5"/></svg>
              </button>
              <button id="btn-tool-poly" class="cad-tool-btn" title="Draw a graphic polygon (Y)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><polygon points="8,2.5 13.5,6.5 11.5,13.5 4.5,13.5 2.5,6.5"/></svg>
              </button>
              <button id="btn-tool-text" class="cad-tool-btn" title="Add a text item (T)">
                <svg viewBox="0 0 16 16" fill="currentColor"><path d="M3 3.5h10v2.5h-3.8v7h-2.4v-7h-3.8z"/></svg>
              </button>
              <button id="btn-tool-dim" class="cad-tool-btn" title="Add aligned linear dimension (D)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><line x1="2" y1="8" x2="14" y2="8"/><polyline points="5,5.5 2,8 5,10.5"/><polyline points="11,5.5 14,8 11,10.5"/><line x1="2" y1="3" x2="2" y2="13"/><line x1="14" y1="3" x2="14" y2="13"/></svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-tool-origin" class="cad-tool-btn" title="Place grid/drill origin point (O)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><circle cx="8" cy="8" r="5.5"/><line x1="8" y1="1.5" x2="8" y2="14.5"/><line x1="1.5" y1="8" x2="14.5" y2="8"/></svg>
              </button>
              <button id="btn-tool-measure" class="cad-tool-btn" title="Measure between two points (M)">
                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M2.5 13.5L13.5 2.5"/><line x1="4.5" y1="11.5" x2="6.5" y2="13.5"/><line x1="7" y1="9" x2="9" y2="11"/><line x1="9.5" y1="6.5" x2="11.5" y2="8.5"/></svg>
              </button>
            </aside>

            <!-- Right Docked KiCad Appearance Manager, Layer Display Options, Selection Filter & Inspector -->
            <aside id="cad-right-sidebar" class="cad-sidebar">
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
          </footer>

          <!-- IPC-7351 Standard Footprint Library Browser Modal -->
          <div id="modal-footprint-browser" class="cad-modal-backdrop hidden">
            <div class="cad-modal-window">
              <div class="cad-modal-header">
                <div style="display:flex;align-items:center;gap:8px;">
                  <svg width="18" height="18" viewBox="0 0 16 16" fill="currentColor"><path d="M2 3H14V13H2V3Z" fill="none" stroke="currentColor" stroke-width="1.5"/><rect x="4" y="5" width="2.5" height="6" fill="currentColor"/><rect x="9.5" y="5" width="2.5" height="6" fill="currentColor"/></svg>
                  <span>IPC-7351 Standard Footprint Library Browser</span>
                </div>
                <button id="btn-close-fp-modal" class="cad-btn" style="padding:2px 6px;" title="Close">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                </button>
              </div>
              <div class="cad-modal-body">
                <!-- Category Column -->
                <div class="cad-fp-cat-list" id="fp-cat-list">
                  <div class="cad-fp-cat-item active" data-cat="All">All Categories</div>
                  <div class="cad-fp-cat-item" data-cat="Passives">Passives (R/C/L)</div>
                  <div class="cad-fp-cat-item" data-cat="Diodes &amp; LEDs">Diodes &amp; LEDs</div>
                  <div class="cad-fp-cat-item" data-cat="Transistors &amp; Regulators">Transistors &amp; Regulators</div>
                  <div class="cad-fp-cat-item" data-cat="ICs &amp; Logic">ICs &amp; Logic Packages</div>
                  <div class="cad-fp-cat-item" data-cat="Connectors &amp; THT">Connectors &amp; THT</div>
                </div>
                <!-- List Column -->
                <div class="cad-fp-grid-col">
                  <div style="padding:8px;border-bottom:1px solid #242834;">
                    <input id="input-fp-search" type="text" placeholder="Search footprint name, pitch..." class="cad-input" style="width:100%;box-sizing:border-box;" />
                  </div>
                  <div id="fp-items-list" style="flex:1;overflow-y:auto;"></div>
                </div>
                <!-- Preview Column -->
                <div class="cad-fp-preview-col" id="fp-preview-panel">
                  <div id="fp-preview-svg-container" style="display:flex;align-items:center;justify-content:center;min-height:220px;border-radius:4px;border:1px solid #222734;background:#101217;"></div>
                  <div id="fp-detail-specs" style="margin-top:14px;display:flex;flex-direction:column;gap:6px;"></div>
                </div>
              </div>
              <div class="cad-modal-footer">
                <span id="fp-selected-hint" style="margin-right:auto;font-size:11px;color:#8892a4;">Select a footprint to inspect specifications</span>
                <button id="btn-cancel-fp-modal" class="cad-btn">Cancel</button>
                <button id="btn-confirm-place-fp" class="cad-btn" style="background:#1d4ed8;border-color:#3b82f6;color:#ffffff;font-weight:700;">Place Footprint on PCB</button>
              </div>
            </div>
          </div>

          <!-- New Project Dialog Modal -->
          <div id="modal-new-project" class="cad-modal-backdrop hidden">
            <div class="cad-modal-window" style="width:520px;max-width:92vw;">
              <div class="cad-modal-header">
                <div style="display:flex;align-items:center;gap:8px;">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#60a5fa" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="12" y1="18" x2="12" y2="12"/><line x1="9" y1="15" x2="15" y2="15"/></svg>
                  <span style="font-weight:700;">Create New PCB Project</span>
                </div>
                <button id="btn-close-new-proj-modal" class="cad-btn" style="padding:2px 6px;" title="Close">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                </button>
              </div>
              <div class="cad-modal-body" style="flex-direction:column;padding:18px;gap:14px;background:#13151b;">
                <div>
                  <label style="display:block;font-size:11px;font-weight:700;color:#8b949e;text-transform:uppercase;margin-bottom:6px;">Project Name</label>
                  <input id="input-new-proj-name" type="text" value="New_PCB_Project" class="cad-input" style="width:100%;box-sizing:border-box;font-size:13px;padding:6px 10px;" />
                </div>
                <div>
                  <label style="display:block;font-size:11px;font-weight:700;color:#8b949e;text-transform:uppercase;margin-bottom:6px;">Starting Template / Form Factor</label>
                  <select id="select-new-proj-template" class="cad-select" style="width:100%;box-sizing:border-box;font-size:12.5px;padding:6px 10px;">
                    <option value="blank">Blank 2-Layer Board (100 × 80 mm)</option>
                    <option value="rpi_hat">Raspberry Pi HAT (65 × 56 mm with 40-pin GPIO)</option>
                    <option value="iot_sensor">IoT Micro Sensor (50 × 40 mm)</option>
                    <option value="eurocard">Eurocard 3U (160 × 100 mm)</option>
                    <option value="custom">Custom Dimensions...</option>
                  </select>
                </div>
                <div id="row-custom-dims" class="hidden" style="display:flex;gap:12px;background:#1a1d26;padding:10px 12px;border-radius:6px;border:1px solid #282d3c;">
                  <div style="flex:1;">
                    <label style="display:block;font-size:10.5px;color:#8b949e;margin-bottom:4px;">Width (mm)</label>
                    <input id="input-new-proj-width" type="number" step="1.0" min="10.0" max="1000.0" value="100.0" class="cad-input" style="width:100%;box-sizing:border-box;" />
                  </div>
                  <div style="flex:1;">
                    <label style="display:block;font-size:10.5px;color:#8b949e;margin-bottom:4px;">Height (mm)</label>
                    <input id="input-new-proj-height" type="number" step="1.0" min="10.0" max="1000.0" value="80.0" class="cad-input" style="width:100%;box-sizing:border-box;" />
                  </div>
                </div>
                <div style="font-size:11.5px;color:#6e7681;line-height:1.5;background:#11141a;padding:10px;border-radius:4px;border:1px dashed #21262d;">
                  Generates an industry-standard KiCad v8+ .kicad_pcb layout with Edge.Cuts board outline, corner mounting holes, and copper power nets ready for layout.
                </div>
              </div>
              <div class="cad-modal-footer">
                <button id="btn-cancel-new-proj-modal" class="cad-btn">Cancel</button>
                <button id="btn-confirm-create-proj" class="cad-btn" style="background:#238636;border-color:#2ea043;color:#ffffff;font-weight:700;">Create Project</button>
              </div>
            </div>
          </div>
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
    setup_home_and_project_hub(doc, state.clone());
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
            app.update_telemetry_hud();
            app.mark_render_dirty(&st);
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
                            let pts = app.router.compute_walkaround_route(
                                &app.current_board,
                                sx,
                                sy,
                                ex,
                                ey,
                                app.active_track_width_mm,
                                app.active_draw_layer,
                                net_id,
                                app.min_clearance_rule_mm,
                                app.route_posture_flip,
                            );

                            // Online DRC enforcement if stop_on_drc is enabled
                            if app.stop_on_drc {
                                let mut has_viol = false;
                                for pair in pts.windows(2) {
                                    if let Err(viol) = validate_track_segment(
                                        &app.current_board,
                                        pair[0].0,
                                        pair[0].1,
                                        pair[1].0,
                                        pair[1].1,
                                        app.active_track_width_mm,
                                        app.active_draw_layer,
                                        net_id,
                                        app.min_clearance_rule_mm,
                                    ) {
                                        app.clearance_halo = Some(((viol.x, viol.y), app.min_clearance_rule_mm));
                                        has_viol = true;
                                        break;
                                    }
                                }
                                if has_viol {
                                    app.rebuild_gpu_buffer_with_markers();
                                    return; // stop on illegal DRC violation!
                                }
                            }

                            app.push_undo();
                            let w = app.active_track_width_mm;
                            let layer = app.active_draw_layer;

                            // Shove conflicting tracks if in Push & Shove mode
                            if app.routing_mode == RoutingMode::PushAndShove {
                                let mut segs = Vec::new();
                                for pair in pts.windows(2) {
                                    segs.push((pair[0], pair[1]));
                                }
                                let min_clr = app.min_clearance_rule_mm;
                                TopologicalRouter::push_and_shove_obstacles(
                                    &mut app.current_board,
                                    &segs,
                                    w,
                                    layer,
                                    net_id,
                                    min_clr,
                                );
                            }

                            let mut last_idx = 0;
                            for pair in pts.windows(2) {
                                last_idx = app.current_board.add_primitive(
                                    pair[0].0,
                                    pair[0].1,
                                    pair[1].0,
                                    pair[1].1,
                                    w,
                                    w,
                                    layer,
                                    net_id,
                                    0.0,
                                );
                                let (prim_id, clock) = app.collab.next_primitive_id();
                                app.collab.send_message(&CollabMessage::AddTrack {
                                    project_id: app.collab.current_project_id.clone(),
                                    id: prim_id,
                                    clock,
                                    user_id: app.collab.local_user_id.clone(),
                                    sx: pair[0].0,
                                    sy: pair[0].1,
                                    ex: pair[1].0,
                                    ey: pair[1].1,
                                    w,
                                    layer,
                                    net_id,
                                });
                            }
                            app.selected_primitive_idx = Some(last_idx);
                            app.route_start_mm = Some((ex, ey, net_id));
                            app.populate_net_selector();
                            app.rebuild_gpu_buffer_with_markers();
                            app.render_inspector_panel();
                        }
                    } else {
                        // Check if net is locked by another engineer
                        if picked_net > 0 && app.collab.is_net_locked_by_other(picked_net).is_some() {
                            if let Some(lbl) = app.doc.get_element_by_id("sch-cross-probe-label") {
                                lbl.set_text_content(Some(&format!(
                                    "⚠ Net {} is locked by another engineer. Collision prevented.",
                                    picked_net
                                )));
                            }
                            return;
                        }
                        if picked_net > 0 {
                            app.collab.lock_net(picked_net, 15000);
                        }
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
                        let target_len = app.target_length_tuning_mm.max(10.0);
                        let pts = TopologicalRouter::build_length_tuned_meanders(
                            sx,
                            sy,
                            ex,
                            ey,
                            app.active_track_width_mm,
                            target_len,
                        );
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
                                let (prim_id, clock) = app.collab.next_primitive_id();
                                app.collab.send_message(&CollabMessage::AddTrack {
                                    project_id: app.collab.current_project_id.clone(),
                                    id: prim_id,
                                    clock,
                                    user_id: app.collab.local_user_id.clone(),
                                    sx: w_pair[0].0,
                                    sy: w_pair[0].1,
                                    ex: w_pair[1].0,
                                    ey: w_pair[1].1,
                                    w,
                                    layer,
                                    net_id,
                                });
                            }
                            app.collab.unlock_net(net_id);
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
                    let vsize = app.active_via_size_mm;
                    let vdrill = app.active_via_drill_mm;

                    // Sub-millisecond Online DRC Guard
                    if app.stop_on_drc {
                        if let Err(err) = validate_via_drop(
                            &app.current_board,
                            snapped.0,
                            snapped.1,
                            vsize,
                            net_id,
                            app.min_clearance_rule_mm,
                        ) {
                            app.clearance_halo = Some(((snapped.0, snapped.1), vsize * 0.5 + app.min_clearance_rule_mm));
                            if let Some(insp) = app.doc.get_element_by_id("rust-hover-inspector") {
                                insp.set_text_content(Some(&format!("[DRC Violation] {}", err.details)));
                            }
                            app.rebuild_gpu_buffer_with_markers();
                            return;
                        }
                    }

                    app.push_undo();
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
                    let (prim_id, clock) = app.collab.next_primitive_id();
                    app.collab.send_message(&CollabMessage::AddVia {
                        project_id: app.collab.current_project_id.clone(),
                        id: prim_id,
                        clock,
                        user_id: app.collab.local_user_id.clone(),
                        x: snapped.0,
                        y: snapped.1,
                        size: vsize,
                        drill: vdrill,
                        net_id,
                    });
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
                    if let Some(ref fp) = app.active_footprint_to_place.clone() {
                        let prefix = fp.ref_prefix.clone();
                        let ref_idx = app.next_ref_indices.entry(prefix).or_insert(1);
                        let curr_idx = *ref_idx;
                        *ref_idx += 1;
                        fp.instantiate_into_board(
                            &mut app.current_board,
                            snapped.0,
                            snapped.1,
                            layer,
                            net_id,
                            curr_idx,
                        );
                    } else {
                        let idx = app.current_board.add_primitive(
                            snapped.0, snapped.1, snapped.0, snapped.1, 1.4, 0.9, layer, net_id, 2.0,
                        );
                        app.selected_primitive_idx = Some(idx);
                    }
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
                app.mark_render_dirty(&st);
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
            let tool_str = match app.active_tool {
                CadTool::RouteTrack => "Route",
                CadTool::PlaceVia => "Via",
                CadTool::TuneTrackLength => "Tune",
                CadTool::Measure => "Measure",
                _ => "Cursor",
            };
            app.collab.broadcast_cursor(snapped.0, snapped.1, app.active_draw_layer, tool_str);
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
                app.mark_render_dirty(&st);
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
                    return;
                }
            }

            if was_dragging {
                app.draw_all();
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
            let was_dragging = app.is_dragging || app.is_dragging_primitive;
            if app.is_dragging_primitive {
                app.current_board.rebuild_index_and_nets();
            }
            app.is_dragging = false;
            app.is_dragging_primitive = false;
            if was_dragging {
                app.draw_all();
            }
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
        {
            let st = state.clone();
            let on_click = Closure::wrap(Box::new(move |e: MouseEvent| {
                if let Some(target) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                    // 1. Check if clicked a hierarchical sub-sheet box
                    if let Ok(Some(box_el)) = target.closest(".sch-subsheet-box") {
                        let sheet_file = box_el.get_attribute("data-sheet-file").unwrap_or_default();
                        let sheet_name = box_el.get_attribute("data-sheet-name").unwrap_or_default();
                        let target_id = sheet_file
                            .trim_end_matches(".kicad_sch")
                            .trim_end_matches(".SchDoc")
                            .to_string();

                        let mut app = st.borrow_mut();
                        if app.schematic_doc.sheets.contains_key(&target_id) {
                            app.schematic_doc.navigate_to_sheet(&target_id);
                        } else {
                            // Register sub-sheet
                            let new_sheet = schematic::SchematicSheet {
                                id: target_id.clone(),
                                name: sheet_name,
                                filename: sheet_file,
                                symbols: Vec::new(),
                                wires: Vec::new(),
                                junctions: Vec::new(),
                                labels: Vec::new(),
                                sub_sheets: Vec::new(),
                                bounds: [0.0, 0.0, 297.0, 210.0],
                            };
                            app.schematic_doc.sheets.insert(target_id.clone(), new_sheet);
                            app.schematic_doc.navigate_to_sheet(&target_id);
                        }
                        drop(app);
                        render_active_schematic_sheet(&st);
                        return;
                    }

                    // 2. Check if clicked a component symbol box (bidirectional cross-probe!)
                    if let Ok(Some(sym_el)) = target.closest(".sch-symbol-box") {
                        let ref_des = sym_el.get_attribute("data-sym-ref").unwrap_or_default();
                        let val = sym_el.get_attribute("data-sym-val").unwrap_or_default();
                        let mut app = st.borrow_mut();
                        if let Some(comp) = app
                            .current_board
                            .components
                            .iter()
                            .find(|c| c.reference == ref_des)
                            .cloned()
                        {
                            if let Some(ref mut r) = app.renderer {
                                r.center_mm = (comp.x, comp.y);
                            }
                            app.draw_all();
                            if let Some(lbl) = app.doc.get_element_by_id("sch-cross-probe-label") {
                                lbl.set_text_content(Some(&format!(
                                    "Cross-Probe: Selected {} ({}) at ({:.2}, {:.2}) mm in PCB Layout",
                                    ref_des, val, comp.x, comp.y
                                )));
                            }
                        }
                        return;
                    }
                }

                // 3. Fallback net highlight if clicking canvas
                let mut app = st.borrow_mut();
                let mut net_keys: Vec<u32> = app
                    .current_board
                    .nets
                    .keys()
                    .cloned()
                    .filter(|&k| k > 0)
                    .collect();
                net_keys.sort();
                if net_keys.is_empty() {
                    return;
                }

                let click_x = e.client_x() as usize;
                let click_y = e.client_y() as usize;
                let hash = (click_x.wrapping_mul(31)).wrapping_add(click_y);
                let chosen_net = net_keys[hash % net_keys.len()];

                let net_name = app
                    .current_board
                    .nets
                    .get(&chosen_net)
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| format!("Net-{}", chosen_net));
                let prim_count = app
                    .current_board
                    .nets
                    .get(&chosen_net)
                    .map(|n| n.segment_count + n.via_count + n.pad_count)
                    .unwrap_or(0);

                app.cross_probe_info = Some(format!(
                    "Cross-Probed Net: {} ({} prims)",
                    net_name, prim_count
                ));
                app.select_net_probe(chosen_net as i32);

                if let Some(lbl) = app.doc.get_element_by_id("sch-cross-probe-label") {
                    lbl.set_text_content(Some(&format!(
                        "Cross-Probed to PCB: Net {} (\"{}\") — {} primitives highlighted",
                        chosen_net, net_name, prim_count
                    )));
                }
                app.update_telemetry_hud();
            }) as Box<dyn FnMut(_)>);
            let _ = vp.add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref());
            on_click.forget();
        }
    }

    if let Some(btn_up) = doc.get_element_by_id("btn-sch-nav-up") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            if st.borrow_mut().schematic_doc.navigate_up() {
                render_active_schematic_sheet(&st);
            }
        }) as Box<dyn FnMut(_)>);
        let _ = btn_up.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(crumb_list) = doc.get_element_by_id("sch-breadcrumb-list") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(target) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if let Ok(Some(chip)) = target.closest(".sch-breadcrumb-btn") {
                    if let Some(sheet_id) = chip.get_attribute("data-sheet-id") {
                        if st.borrow_mut().schematic_doc.navigate_to_sheet(&sheet_id) {
                            render_active_schematic_sheet(&st);
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = crumb_list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
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

        if (e.ctrl_key() || e.meta_key()) && (key == "s" || key == "S") {
            e.prevent_default();
            drop(app);
            trigger_save_kicad_pcb(&state);
            return;
        }

        if (e.ctrl_key() || e.meta_key()) && (key == "o" || key == "O") {
            e.prevent_default();
            if let Some(inp) = app.doc.get_element_by_id("home-file-input").and_then(|el| el.dyn_into::<HtmlInputElement>().ok()) {
                inp.click();
            }
            return;
        }

        if (e.ctrl_key() || e.meta_key()) && (key == "n" || key == "N") {
            e.prevent_default();
            if let Some(m) = app.doc.get_element_by_id("modal-new-project") {
                let _ = m.class_list().remove_1("hidden");
            }
            return;
        }

        if key == "Home" || (e.alt_key() && (key == "h" || key == "H")) {
            e.prevent_default();
            drop(app);
            switch_view_tab(&state, "home");
            return;
        }

        match key.as_str() {
            "Escape" => {
                if app.active_tab == "home" {
                    drop(app);
                    switch_view_tab(&state, "pcb");
                    return;
                }
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

fn close_all_cad_menus(doc: &Document) {
    if let Ok(all_dds) = doc.query_selector_all(".cad-menu-dropdown") {
        for i in 0..all_dds.length() {
            if let Some(el) = all_dds.get(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                let _ = el.class_list().add_1("hidden");
            }
        }
    }
    if let Ok(all_btns) = doc.query_selector_all(".cad-menu-btn") {
        for i in 0..all_btns.length() {
            if let Some(el) = all_btns.get(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                let _ = el.class_list().remove_1("active");
            }
        }
    }
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

    macro_rules! bind_menu_item {
        ($doc:expr, $id:expr, $body:expr) => {
            if let Some(el) = $doc.get_element_by_id($id) {
                let doc_c = $doc.clone();
                let cb = Closure::wrap(Box::new(move |_e: Event| {
                    close_all_cad_menus(&doc_c);
                    $body;
                }) as Box<dyn FnMut(_)>);
                let _ = el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
                cb.forget();
            }
        };
    }

    // Bind Top Menu Bar Dropdown Toggles (File, Edit, View, Place, Route, Inspect, Tools)
    for menu_name in ["file", "edit", "view", "place", "route", "inspect", "tools"] {
        let btn_id = format!("menu-btn-{}", menu_name);
        let dropdown_id = format!("dropdown-menu-{}", menu_name);
        if let Some(btn) = doc.get_element_by_id(&btn_id) {
            let doc_clone = doc.clone();
            let dd_id = dropdown_id.clone();
            let btn_id_clone = btn_id.clone();
            let cb = Closure::wrap(Box::new(move |e: Event| {
                e.stop_propagation();
                let is_open = doc_clone
                    .get_element_by_id(&dd_id)
                    .map(|d| !d.class_list().contains("hidden"))
                    .unwrap_or(false);

                close_all_cad_menus(&doc_clone);

                if !is_open {
                    if let Some(d) = doc_clone.get_element_by_id(&dd_id) {
                        let _ = d.class_list().remove_1("hidden");
                    }
                    if let Some(b) = doc_clone.get_element_by_id(&btn_id_clone) {
                        let _ = b.class_list().add_1("active");
                    }
                }
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    // Close all menus when clicking anywhere outside
    {
        let doc_clone = doc.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            close_all_cad_menus(&doc_clone);
        }) as Box<dyn FnMut(_)>);
        let _ = doc.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // File Menu Items
    bind_menu_item!(doc, "menu-action-new", {
        if let Some(m) = doc.get_element_by_id("modal-new-project") {
            let _ = m.class_list().remove_1("hidden");
        }
    });
    bind_menu_item!(doc, "menu-action-open", {
        if let Some(inp) = doc.get_element_by_id("home-file-input").and_then(|el| el.dyn_into::<HtmlInputElement>().ok()) {
            inp.click();
        }
    });
    let st_save = state.clone();
    bind_menu_item!(doc, "menu-action-save", {
        trigger_save_kicad_pcb(&st_save);
    });
    let st_gerb = state.clone();
    bind_menu_item!(doc, "menu-action-gerber", {
        trigger_gerber_export(&st_gerb);
    });
    let st_home = state.clone();
    bind_menu_item!(doc, "menu-action-home", {
        switch_view_tab(&st_home, "home");
    });

    // Edit Menu Items
    let st_u = state.clone();
    bind_menu_item!(doc, "menu-action-undo", {
        st_u.borrow_mut().perform_undo();
    });
    let st_r = state.clone();
    bind_menu_item!(doc, "menu-action-rotate", {
        st_r.borrow_mut().rotate_selected_primitive();
    });
    let st_d = state.clone();
    bind_menu_item!(doc, "menu-action-delete", {
        st_d.borrow_mut().delete_selected_primitive();
    });
    let st_desel = state.clone();
    bind_menu_item!(doc, "menu-action-deselect", {
        let mut app = st_desel.borrow_mut();
        app.selected_primitive_idx = None;
        app.render_inspector_panel();
        app.draw_all();
    });

    // View Menu Items
    let st_vpcb = state.clone();
    bind_menu_item!(doc, "menu-action-view-pcb", {
        switch_view_tab(&st_vpcb, "pcb");
    });
    let st_v3d = state.clone();
    bind_menu_item!(doc, "menu-action-view-3d", {
        toggle_3d_mode(&mut st_v3d.borrow_mut());
    });
    let st_vsch = state.clone();
    bind_menu_item!(doc, "menu-action-view-sch", {
        switch_view_tab(&st_vsch, "schematic");
    });
    let st_vdiff = state.clone();
    bind_menu_item!(doc, "menu-action-view-diff", {
        switch_view_tab(&st_vdiff, "diff");
    });
    let st_vdrc = state.clone();
    bind_menu_item!(doc, "menu-action-view-drc", {
        switch_view_tab(&st_vdrc, "drc");
    });
    let st_vbom = state.clone();
    bind_menu_item!(doc, "menu-action-view-bom", {
        switch_view_tab(&st_vbom, "parts");
    });
    let st_zfit = state.clone();
    bind_menu_item!(doc, "menu-action-zoom-fit", {
        let mut app = st_zfit.borrow_mut();
        if let Some(ref mut r) = app.renderer {
            r.fit_to_board();
        }
        app.draw_all();
        app.update_telemetry_hud();
    });
    let st_zin = state.clone();
    bind_menu_item!(doc, "menu-action-zoom-in", {
        let mut app = st_zin.borrow_mut();
        if let Some(ref mut r) = app.renderer {
            r.scale_px_per_mm = (r.scale_px_per_mm * 1.25).min(500.0);
        }
        app.draw_all();
        app.update_telemetry_hud();
    });
    let st_zout = state.clone();
    bind_menu_item!(doc, "menu-action-zoom-out", {
        let mut app = st_zout.borrow_mut();
        if let Some(ref mut r) = app.renderer {
            r.scale_px_per_mm = (r.scale_px_per_mm * 0.8).max(1.5);
        }
        app.draw_all();
        app.update_telemetry_hud();
    });
    let st_cont = state.clone();
    bind_menu_item!(doc, "menu-action-contrast", {
        cycle_high_contrast(&mut st_cont.borrow_mut());
    });
    let st_flip = state.clone();
    bind_menu_item!(doc, "menu-action-flip", {
        toggle_flip_board(&mut st_flip.borrow_mut());
    });

    // Place Menu Items
    let st_pp = state.clone();
    bind_menu_item!(doc, "menu-action-place-pad", {
        st_pp.borrow_mut().set_active_tool(CadTool::PlacePad);
    });
    bind_menu_item!(doc, "menu-action-place-fp-lib", {
        if let Some(m) = doc.get_element_by_id("modal-footprint-browser") {
            let _ = m.class_list().remove_1("hidden");
        }
    });
    let st_pv = state.clone();
    bind_menu_item!(doc, "menu-action-place-via", {
        st_pv.borrow_mut().set_active_tool(CadTool::PlaceVia);
    });
    let st_pz = state.clone();
    bind_menu_item!(doc, "menu-action-place-zone", {
        st_pz.borrow_mut().set_active_tool(CadTool::AddZone);
    });
    let st_pk = state.clone();
    bind_menu_item!(doc, "menu-action-place-keepout", {
        st_pk.borrow_mut().set_active_tool(CadTool::AddKeepout);
    });
    let st_pl = state.clone();
    bind_menu_item!(doc, "menu-action-place-line", {
        st_pl.borrow_mut().set_active_tool(CadTool::DrawLine);
    });
    let st_pa = state.clone();
    bind_menu_item!(doc, "menu-action-place-arc", {
        st_pa.borrow_mut().set_active_tool(CadTool::DrawArc);
    });
    let st_pc = state.clone();
    bind_menu_item!(doc, "menu-action-place-circle", {
        st_pc.borrow_mut().set_active_tool(CadTool::DrawCircle);
    });
    let st_pr = state.clone();
    bind_menu_item!(doc, "menu-action-place-rect", {
        st_pr.borrow_mut().set_active_tool(CadTool::DrawRect);
    });
    let st_py = state.clone();
    bind_menu_item!(doc, "menu-action-place-poly", {
        st_py.borrow_mut().set_active_tool(CadTool::DrawPoly);
    });
    let st_pt = state.clone();
    bind_menu_item!(doc, "menu-action-place-text", {
        st_pt.borrow_mut().set_active_tool(CadTool::AddText);
    });
    let st_pdim = state.clone();
    bind_menu_item!(doc, "menu-action-place-dim", {
        st_pdim.borrow_mut().set_active_tool(CadTool::AddDimension);
    });
    let st_po = state.clone();
    bind_menu_item!(doc, "menu-action-place-origin", {
        st_po.borrow_mut().set_active_tool(CadTool::PlaceOrigin);
    });

    // Route Menu Items
    let st_rx = state.clone();
    bind_menu_item!(doc, "menu-action-route-single", {
        st_rx.borrow_mut().set_active_tool(CadTool::RouteTrack);
    });
    let st_rd = state.clone();
    bind_menu_item!(doc, "menu-action-route-diffpair", {
        st_rd.borrow_mut().set_active_tool(CadTool::RouteDiffPair);
    });
    let st_rt = state.clone();
    bind_menu_item!(doc, "menu-action-route-tune", {
        st_rt.borrow_mut().set_active_tool(CadTool::TuneTrackLength);
    });
    let st_rw = state.clone();
    bind_menu_item!(doc, "menu-action-route-walk", {
        let mut app = st_rw.borrow_mut();
        app.routing_mode = RoutingMode::Walkaround;
        app.router.mode = app.routing_mode;
        app.update_telemetry_hud();
    });
    let st_rp = state.clone();
    bind_menu_item!(doc, "menu-action-route-push", {
        let mut app = st_rp.borrow_mut();
        app.routing_mode = RoutingMode::PushAndShove;
        app.router.mode = app.routing_mode;
        app.update_telemetry_hud();
    });
    let st_rf = state.clone();
    bind_menu_item!(doc, "menu-action-route-free45", {
        let mut app = st_rf.borrow_mut();
        app.routing_mode = RoutingMode::Free45;
        app.router.mode = app.routing_mode;
        app.update_telemetry_hud();
    });

    // Inspect Menu Items
    let st_idrc = state.clone();
    bind_menu_item!(doc, "menu-action-inspect-drc", {
        switch_view_tab(&st_idrc, "drc");
    });
    let st_irun = state.clone();
    bind_menu_item!(doc, "menu-action-inspect-run-drc", {
        run_live_drc(&st_irun);
    });
    let st_im = state.clone();
    bind_menu_item!(doc, "menu-action-inspect-measure", {
        st_im.borrow_mut().set_active_tool(CadTool::Measure);
    });
    let st_ibom = state.clone();
    bind_menu_item!(doc, "menu-action-inspect-bom", {
        switch_view_tab(&st_ibom, "parts");
    });

    // Tools Menu Items
    let st_tf = state.clone();
    bind_menu_item!(doc, "menu-action-tools-fill", {
        let mut app = st_tf.borrow_mut();
        app.zone_display_mode = 0;
        app.draw_all();
    });
    let st_to = state.clone();
    bind_menu_item!(doc, "menu-action-tools-outline", {
        let mut app = st_to.borrow_mut();
        app.zone_display_mode = 1;
        app.draw_all();
    });
    let st_th = state.clone();
    bind_menu_item!(doc, "menu-action-tools-hide-zones", {
        let mut app = st_th.borrow_mut();
        app.zone_display_mode = 2;
        app.draw_all();
    });
    let st_tg = state.clone();
    bind_menu_item!(doc, "menu-action-tools-gerbers", {
        trigger_gerber_export(&st_tg);
    });

    // Save Button in Topbar
    if let Some(btn) = doc.get_element_by_id("btn-export-pcb") {
        let st_save_btn = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            trigger_save_kicad_pcb(&st_save_btn);
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

    // Routing Mode Selector
    if let Some(r_sel) = doc.get_element_by_id("select-routing-mode") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(target) = e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()) {
                let mut app = st.borrow_mut();
                app.routing_mode = match target.value().as_str() {
                    "push" => RoutingMode::PushAndShove,
                    "free45" => RoutingMode::Free45,
                    _ => RoutingMode::Walkaround,
                };
                app.router.mode = app.routing_mode;
                app.rebuild_gpu_buffer_with_markers();
                app.update_telemetry_hud();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = r_sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Stop on DRC Guard Checkbox
    if let Some(chk_drc) = doc.get_element_by_id("check-stop-on-drc") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(target) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                st.borrow_mut().stop_on_drc = target.checked();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = chk_drc.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Target Length for Serpentine Tuning
    if let Some(inp_len) = doc.get_element_by_id("input-target-length") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(target) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                let val: f32 = target.value().parse().unwrap_or(50.0);
                let mut app = st.borrow_mut();
                app.target_length_tuning_mm = val;
                app.router.target_length_mm = Some(val);
                app.rebuild_gpu_buffer_with_markers();
                app.update_telemetry_hud();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = inp_len.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref());
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
                if st.borrow().active_tab == "home" {
                    switch_view_tab(&st, "pcb");
                }
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
        ("tab-home", "home"),
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

    // Gerber Export Button
    if let Some(btn) = doc.get_element_by_id("btn-export-gerbers") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            e.prevent_default();
            trigger_gerber_export(&st);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Footprint Browser Modal Triggers
    if let Some(btn) = doc.get_element_by_id("btn-open-fp-browser") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            open_footprint_modal(&st);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    for id in ["btn-close-fp-modal", "btn-cancel-fp-modal"] {
        if let Some(btn) = doc.get_element_by_id(id) {
            let st = state.clone();
            let cb = Closure::wrap(Box::new(move |_e: Event| {
                close_footprint_modal(&st);
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }
    if let Some(btn) = doc.get_element_by_id("btn-confirm-place-fp") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            let sel_id = app.selected_footprint_id.clone();
            let fp = app.footprint_library.iter().find(|f| f.id == sel_id).cloned();
            if let Some(f) = fp {
                app.active_footprint_to_place = Some(f);
                app.set_active_tool(CadTool::PlacePad);
            }
            drop(app);
            close_footprint_modal(&st);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Footprint Search & Category Click Handlers
    if let Some(inp) = doc.get_element_by_id("input-fp-search") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                st.borrow_mut().footprint_search_query = input.value();
                render_footprint_modal_content(&st);
            }
        }) as Box<dyn FnMut(_)>);
        let _ = inp.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(cat_list) = doc.get_element_by_id("fp-cat-list") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if let Some(item) = el.closest(".cad-fp-cat-item").ok().flatten() {
                    if let Some(cat) = item.get_attribute("data-cat") {
                        st.borrow_mut().footprint_category_filter = cat;
                        render_footprint_modal_content(&st);
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = cat_list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(items_list) = doc.get_element_by_id("fp-items-list") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if let Some(item) = el.closest(".cad-fp-item").ok().flatten() {
                    if let Some(id) = item.get_attribute("data-fp-id") {
                        st.borrow_mut().selected_footprint_id = id;
                        render_footprint_modal_content(&st);
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = items_list.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // DRC Workbench Run & Filter Controls
    if let Some(btn) = doc.get_element_by_id("btn-run-drc") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            run_live_drc(&st);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(inp) = doc.get_element_by_id("input-drc-clearance") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                if let Ok(val) = input.value().parse::<f32>() {
                    st.borrow_mut().min_clearance_rule_mm = val.clamp(0.05, 5.0);
                    run_live_drc(&st);
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = inp.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(drc_panel) = doc.get_element_by_id("drc-view-panel") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if let Some(btn) = el.closest(".drc-filter-btn").ok().flatten() {
                    if let Some(f) = btn.get_attribute("data-filter") {
                        st.borrow_mut().drc_filter = f;
                        render_drc_workbench(&st);
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = drc_panel.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Clicking a DRC row flies the GPU camera to that exact violation coordinate
    if let Some(drc_list) = doc.get_element_by_id("drc-list-container") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if let Some(row) = el.closest(".drc-row").ok().flatten() {
                    let maybe_x = row.get_attribute("data-drc-x").and_then(|s| s.parse::<f32>().ok());
                    let maybe_y = row.get_attribute("data-drc-y").and_then(|s| s.parse::<f32>().ok());
                    if let (Some(vx), Some(vy)) = (maybe_x, maybe_y) {
                        switch_view_tab(&st, "pcb");
                        if vx > 1.0 && vy > 1.0 {
                            let mut app = st.borrow_mut();
                            if let Some(ref mut r) = app.renderer {
                                r.center_mm = (vx, vy);
                                r.scale_px_per_mm = 24.0;
                            }
                            app.draw_all();
                            app.update_telemetry_hud();
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

    if let Some(param_bar) = doc.get_element_by_id("cad-param-bar") {
        param_bar.set_scroll_left(0);
    }
}

pub fn switch_view_tab(state: &Rc<RefCell<AppController>>, mode: &str) {
    let mut app = state.borrow_mut();
    app.active_tab = mode.to_string();

    for (tid, m) in [
        ("tab-home", "home"),
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
        ("home-view-panel", "home"),
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

    // Toggle editor chrome: when in "home" mode, hide parameter bar, canvas action buttons, left toolstrip, right toolstrip, and right sidebar
    for editor_elem_id in [
        "cad-param-bar",
        "tabbar-canvas-actions",
        "cad-left-strip",
        "cad-right-toolstrip",
        "cad-right-sidebar",
    ] {
        if let Some(el) = app.doc.get_element_by_id(editor_elem_id) {
            if mode == "home" {
                let _ = el.class_list().add_1("hidden");
            } else {
                let _ = el.class_list().remove_1("hidden");
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
    drop(app);

    if mode == "drc" {
        render_drc_workbench(state);
    } else if mode == "schematic" {
        render_active_schematic_sheet(state);
    }
}

pub fn render_active_schematic_sheet(state: &Rc<RefCell<AppController>>) {
    let app = state.borrow();
    if let Some(inner) = app.doc.get_element_by_id("schematic-canvas-inner") {
        let svg = app.schematic_doc.render_active_sheet_svg();
        inner.set_inner_html(&svg);
    }
    if let Some(crumb_list) = app.doc.get_element_by_id("sch-breadcrumb-list") {
        let mut crumbs_html = String::new();
        for (i, sheet_id) in app.schematic_doc.hierarchy_path.iter().enumerate() {
            let sheet_name = app.schematic_doc.sheets.get(sheet_id).map(|s| s.name.as_str()).unwrap_or(sheet_id);
            let is_last = i == app.schematic_doc.hierarchy_path.len() - 1;
            let active_cls = if is_last { "cad-btn-active" } else { "" };
            crumbs_html.push_str(&format!(
                r#"<button class="cad-btn sch-breadcrumb-btn {active}" data-sheet-id="{id}" style="font-size:11px;padding:2px 8px;">{name}</button>"#,
                active = active_cls,
                id = sheet_id,
                name = sheet_name
            ));
            if !is_last {
                crumbs_html.push_str(r#"<span style="color:#64748b;font-size:10px;">/</span>"#);
            }
        }
        crumb_list.set_inner_html(&crumbs_html);
    }
}

fn render_parts_list(state: &Rc<RefCell<AppController>>) {
    let app = state.borrow();
    let container = match app.doc.get_element_by_id("parts-list-container") {
        Some(c) => c,
        None => return,
    };

    let q = app.parts_filter.to_lowercase();
    let mut html = String::new();

    if !app.parts_catalog.is_empty() {
        let mut count = 0;
        for p in &app.parts_catalog {
            if !q.is_empty()
                && !p.mpn.to_lowercase().contains(&q)
                && !p.manufacturer.to_lowercase().contains(&q)
                && !p.description.to_lowercase().contains(&q)
                && !p.footprint_name.to_lowercase().contains(&q)
            {
                continue;
            }
            count += 1;
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
        if let Some(badge) = app.doc.get_element_by_id("bom-count-badge") {
            badge.set_text_content(Some(&count.to_string()));
        }
    } else {
        // Dynamically populate live Bill of Materials from parsed board components!
        let mut count = 0;
        for comp in &app.current_board.components {
            if !q.is_empty()
                && !comp.reference.to_lowercase().contains(&q)
                && !comp.value.to_lowercase().contains(&q)
                && !comp.footprint.to_lowercase().contains(&q)
            {
                continue;
            }
            count += 1;
            html.push_str(&format!(
                r#"<div class="cad-table-row">
                     <div>
                       <div style="display:flex;align-items:center;gap:8px;">
                         <span style="font-family:monospace;font-weight:700;color:#79b8ff;font-size:12px;">{ref_des}</span>
                         <span style="color:#f0f3f8;font-size:11px;font-weight:600;">{val}</span>
                         <span style="font-family:monospace;font-size:10px;text-transform:uppercase;color:#38bdf8;">[{layer}]</span>
                       </div>
                       <div style="font-family:monospace;font-size:11px;color:#94a3b8;margin-top:2px;">Footprint: {fp}</div>
                       <div style="font-family:monospace;font-size:10px;color:#64748b;margin-top:2px;">Position: ({x:.2}, {y:.2}) mm | Rot: {rot:.1}°</div>
                     </div>
                     <div style="text-align:right;font-family:monospace;font-size:11px;">
                       <div style="color:#34d399;font-weight:600;">Placed</div>
                       <div style="font-size:10px;color:#8b93a6;">Active PCB</div>
                     </div>
                   </div>"#,
                ref_des = comp.reference,
                val = if comp.value.is_empty() { "—" } else { &comp.value },
                layer = comp.layer,
                fp = comp.footprint,
                x = comp.x,
                y = comp.y,
                rot = comp.rot_deg
            ));
        }
        if let Some(badge) = app.doc.get_element_by_id("bom-count-badge") {
            badge.set_text_content(Some(&count.to_string()));
        }
        if count == 0 {
            html = r#"<div style="padding:24px;text-align:center;color:#64748b;font-size:12px;">No components or BOM entries found on active board.</div>"#.to_string();
        }
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
    let proj_id_saved = project_id.to_string();
    {
        let mut app = state.borrow_mut();
        app.active_project = proj_id_saved.clone();
        app.project_name = proj_id_saved.clone();
        app.current_board = ParsedBoard::empty();
        app.board_rev_a = ParsedBoard::empty();
        app.board_rev_b = ParsedBoard::empty();
        app.board_diff = ParsedBoard::empty();
        app.undo_stack.clear();
        app.drc_items.clear();
        app.drc_violations.clear();
        app.selected_primitive_idx = None;
        app.route_start_mm = None;
        app.poly_points_mm.clear();
        app.collab.switch_room(&proj_id_saved);
        app.rebuild_gpu_buffer_with_markers();
        app.draw_all();

        if let Some(sel) = app
            .doc
            .get_element_by_id("rust-project-select")
            .and_then(|e| e.dyn_into::<HtmlSelectElement>().ok())
        {
            sel.set_value(&proj_id_saved);
        }
        if let Some(link) = app.doc.get_element_by_id("link-3d-vrml") {
            let href = if proj_id_saved == "cm5_minima" {
                "/samples/cm5_minima/board.wrl"
            } else {
                "/samples/complex_hierarchy/board.wrl"
            };
            let _ = link.set_attribute("href", href);
        }
    }

    // Check if it's a cached custom board
    let cached_board = {
        let app = state.borrow();
        if let Some((ref cname, ref cboard)) = app.custom_board {
            if cname == &proj_id_saved {
                Some(cboard.clone())
            } else {
                None
            }
        } else {
            None
        }
    };
    if let Some(b) = cached_board {
        {
            let mut app = state.borrow_mut();
            app.rel_origin_mm = (b.bounds[0], b.bounds[3]);
            app.board_rev_a = b.clone();
            app.board_rev_b = b.clone();
            app.board_diff = b.clone();
            app.current_board = b;
            app.undo_stack.clear();
            app.selected_primitive_idx = None;
            app.route_start_mm = None;
            app.poly_points_mm.clear();
            app.populate_net_selector();
            app.select_net_probe(0);
            app.rebuild_gpu_buffer_with_markers();
            if let Some(ref mut r) = app.renderer {
                r.fit_to_board();
            }
            app.draw_all();
            app.update_telemetry_hud();
        }
        render_parts_list(&state);
        run_live_drc(&state);
        return;
    }

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
        let proj_check = proj_id_saved.clone();
        let url_b_str = url_b.to_string();
        move |text_a| {
            let t0 = js_sys::Date::now();
            let board_a = parse_kicad_pcb_str(&text_a);
            let dt = js_sys::Date::now() - t0;
            {
                let mut app = st.borrow_mut();
                if app.active_project != proj_check {
                    return;
                }
                app.project_name = proj_check.clone();
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

            run_live_drc(&st);
            render_parts_list(&st);

            let sch_url = if proj_check == "cm5_minima" {
                None
            } else {
                Some("/samples/complex_hierarchy/complex_hierarchy.kicad_sch")
            };
            if let Some(sch_path) = sch_url {
                let st_sch = st.clone();
                fetch_text_with_callback(sch_path, move |sch_text| {
                    let doc = schematic::parse_kicad_sch_str(&sch_text, "root", "complex_hierarchy");
                    st_sch.borrow_mut().schematic_doc = doc;
                    render_active_schematic_sheet(&st_sch);
                });
            }

            let st2 = st.clone();
            let proj_check2 = proj_check.clone();
            fetch_text_with_callback(&url_b_str, move |text_b| {
                let board_b = parse_kicad_pcb_str(&text_b);
                let mut app = st2.borrow_mut();
                if app.active_project != proj_check2 {
                    return;
                }
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
        let proj_check = proj_id_saved.clone();
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
            if app.active_project != proj_check {
                return;
            }
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

pub fn trigger_save_kicad_pcb(state: &Rc<RefCell<AppController>>) {
    let (kicad_str, project_name, doc) = {
        let app = state.borrow();
        (
            app.current_board.export_kicad_pcb(),
            app.project_name.clone(),
            app.doc.clone(),
        )
    };
    let encoded = js_sys::encode_uri_component(&kicad_str);
    let href = format!("data:text/plain;charset=utf-8,{}", encoded);
    let filename = format!("{}.kicad_pcb", project_name);
    if let Ok(a) = doc.create_element("a") {
        let _ = a.set_attribute("href", &href);
        let _ = a.set_attribute("download", &filename);
        if let Some(html_a) = a.dyn_into::<web_sys::HtmlElement>().ok() {
            html_a.click();
        }
    }
}

pub fn trigger_gerber_export(state: &Rc<RefCell<AppController>>) {
    let (zip_bytes, project_name, doc) = {
        let app = state.borrow();
        let bytes = gerber_exporter::export_complete_fabrication_zip(&app.current_board, &app.active_project);
        (bytes, app.active_project.clone(), app.doc.clone())
    };

    let b64 = gerber_exporter::to_base64(&zip_bytes);
    let href = format!("data:application/zip;base64,{}", b64);
    let filename = format!("{}_Fabrication_Gerbers.zip", project_name);

    if let Ok(a) = doc.create_element("a") {
        let _ = a.set_attribute("href", &href);
        let _ = a.set_attribute("download", &filename);
        if let Some(html_a) = a.dyn_into::<web_sys::HtmlElement>().ok() {
            html_a.click();
        }
    }
}

pub fn run_live_drc(state: &Rc<RefCell<AppController>>) {
    let violations = {
        let app = state.borrow();
        drc_engine::run_full_drc(
            &app.current_board,
            app.min_clearance_rule_mm,
            app.min_track_width_rule_mm,
            app.min_drill_clearance_rule_mm,
        )
    };

    let mut drc_items = Vec::with_capacity(violations.len());
    for v in &violations {
        drc_items.push(DrcItem {
            description: v.title.clone(),
            severity: v.severity.clone(),
            type_name: v.rule_type.clone(),
            x: v.x,
            y: v.y,
        });
    }

    {
        let mut app = state.borrow_mut();
        app.drc_items = drc_items;
        app.drc_violations = violations;
        app.rebuild_gpu_buffer_with_markers();
    }

    render_drc_workbench(state);
}

pub fn render_drc_workbench(state: &Rc<RefCell<AppController>>) {
    let app = state.borrow();
    let total = app.drc_violations.len();
    let errors = app.drc_violations.iter().filter(|v| v.severity == "error").count();
    let warnings = app.drc_violations.iter().filter(|v| v.severity == "warning").count();

    if let Some(b) = app.doc.get_element_by_id("drc-count-badge") {
        b.set_text_content(Some(&total.to_string()));
    }
    if let Some(b) = app.doc.get_element_by_id("drc-cnt-all") {
        b.set_text_content(Some(&total.to_string()));
    }
    if let Some(b) = app.doc.get_element_by_id("drc-cnt-err") {
        b.set_text_content(Some(&errors.to_string()));
    }
    if let Some(b) = app.doc.get_element_by_id("drc-cnt-warn") {
        b.set_text_content(Some(&warnings.to_string()));
    }

    if let Ok(buttons) = app.doc.query_selector_all(".drc-filter-btn") {
        for i in 0..buttons.length() {
            if let Some(btn) = buttons.get(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                let f = btn.get_attribute("data-filter").unwrap_or_default();
                if f == app.drc_filter {
                    btn.set_class_name("drc-filter-btn active");
                } else {
                    btn.set_class_name("drc-filter-btn");
                }
            }
        }
    }

    if let Some(list_el) = app.doc.get_element_by_id("drc-list-container") {
        let filter = app.drc_filter.as_str();
        let filtered: Vec<&DrcViolation> = app
            .drc_violations
            .iter()
            .filter(|v| match filter {
                "all" => true,
                "error" => v.severity == "error",
                "warning" => v.severity == "warning",
                other => v.rule_type == other,
            })
            .collect();

        if filtered.is_empty() {
            list_el.set_inner_html(
                r##"<div style="padding:40px;text-align:center;color:#64748b;font-size:12px;">
                     <div style="display:flex;justify-content:center;margin-bottom:12px;">
                       <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="#10b981" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/></svg>
                     </div>
                     <div style="font-weight:600;color:#cbd5e1;">Zero DRC Violations Found</div>
                     <div style="margin-top:4px;">No clearance conflicts, shorts, or unrouted pins for this filter.</div>
                   </div>"##,
            );
            return;
        }

        let mut html = String::with_capacity(filtered.len() * 450);
        for v in filtered {
            let is_err = v.severity == "error";
            let badge_bg = if is_err { "rgba(239, 68, 68, 0.15)" } else { "rgba(245, 158, 11, 0.15)" };
            let badge_color = if is_err { "#f87171" } else { "#fbbf24" };
            let badge_border = if is_err { "rgba(239, 68, 68, 0.3)" } else { "rgba(245, 158, 11, 0.3)" };

            html.push_str(&format!(
                r#"<div data-drc-x="{x}" data-drc-y="{y}" class="drc-row cad-table-row" style="cursor:pointer;padding:10px 14px;border-bottom:1px solid #1e222d;display:flex;align-items:center;justify-content:space-between;">
                     <div style="display:flex;align-items:flex-start;gap:12px;">
                       <span style="font-size:10px;font-weight:700;padding:2px 6px;border-radius:3px;background:{badge_bg};color:{badge_color};border:1px solid {badge_border};text-transform:uppercase;">{sev}</span>
                       <div>
                         <div style="font-size:12px;font-weight:600;color:#f1f5f9;">{title}</div>
                         <div style="font-size:11px;color:#94a3b8;margin-top:2px;">{details}</div>
                         <div style="font-family:'JetBrains Mono',monospace;font-size:10px;color:#64748b;margin-top:3px;">Location: ({x:.3} mm, {y:.3} mm) | Layer: {layer}</div>
                       </div>
                     </div>
                     <button class="cad-btn" style="font-size:10px;color:#38bdf8;border-color:#0284c7;background:#0c4a6e;flex-shrink:0;">Zoom →</button>
                   </div>"#,
                x = v.x,
                y = v.y,
                badge_bg = badge_bg,
                badge_color = badge_color,
                badge_border = badge_border,
                sev = v.severity,
                title = v.title,
                details = v.details,
                layer = v.layer_name,
            ));
        }
        list_el.set_inner_html(&html);
    }
}

pub fn open_footprint_modal(state: &Rc<RefCell<AppController>>) {
    let app = state.borrow();
    if let Some(m) = app.doc.get_element_by_id("modal-footprint-browser") {
        let _ = m.class_list().remove_1("hidden");
    }
    drop(app);
    render_footprint_modal_content(state);
}

pub fn close_footprint_modal(state: &Rc<RefCell<AppController>>) {
    let app = state.borrow();
    if let Some(m) = app.doc.get_element_by_id("modal-footprint-browser") {
        let _ = m.class_list().add_1("hidden");
    }
}

pub fn render_footprint_modal_content(state: &Rc<RefCell<AppController>>) {
    let app = state.borrow();
    let cat_filter = app.footprint_category_filter.clone();
    let search = app.footprint_search_query.to_lowercase();
    let sel_id = app.selected_footprint_id.clone();

    if let Ok(items) = app.doc.query_selector_all(".cad-fp-cat-item") {
        for i in 0..items.length() {
            if let Some(item) = items.get(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                let cat = item.get_attribute("data-cat").unwrap_or_default();
                if cat == cat_filter {
                    item.set_class_name("cad-fp-cat-item active");
                } else {
                    item.set_class_name("cad-fp-cat-item");
                }
            }
        }
    }

    let filtered: Vec<&FootprintDefinition> = app
        .footprint_library
        .iter()
        .filter(|fp| {
            let matches_cat = cat_filter == "All" || fp.category == cat_filter;
            let matches_search = search.is_empty()
                || fp.name.to_lowercase().contains(&search)
                || fp.description.to_lowercase().contains(&search)
                || fp.ref_prefix.to_lowercase().contains(&search);
            matches_cat && matches_search
        })
        .collect();

    if let Some(list_el) = app.doc.get_element_by_id("fp-items-list") {
        let mut html = String::new();
        for fp in &filtered {
            let is_sel = fp.id == sel_id;
            let active_cls = if is_sel { "cad-fp-item active" } else { "cad-fp-item" };
            html.push_str(&format!(
                r#"<div class="{active_cls}" data-fp-id="{id}">
                     <div style="font-size:12px;font-weight:600;color:#f1f5f9;">{name}</div>
                     <div style="font-size:10.5px;color:#94a3b8;margin-top:2px;">{desc}</div>
                     <div style="font-family:'JetBrains Mono',monospace;font-size:10px;color:#64748b;margin-top:3px;">Pads: {pads} | Pitch: {pitch:.2} mm</div>
                   </div>"#,
                active_cls = active_cls,
                id = fp.id,
                name = fp.name,
                desc = fp.description,
                pads = fp.pads.len(),
                pitch = fp.pitch_mm,
            ));
        }
        list_el.set_inner_html(&html);
    }

    let sel_fp = app
        .footprint_library
        .iter()
        .find(|f| f.id == sel_id)
        .or_else(|| filtered.first().copied());

    if let Some(fp) = sel_fp {
        if let Some(svg_box) = app.doc.get_element_by_id("fp-preview-svg-container") {
            svg_box.set_inner_html(&fp.generate_svg_preview());
        }
        if let Some(detail_box) = app.doc.get_element_by_id("fp-detail-specs") {
            let tht_count = fp.pads.iter().filter(|p| p.is_tht).count();
            let smd_count = fp.pads.len() - tht_count;
            detail_box.set_inner_html(&format!(
                r#"<div style="font-size:14px;font-weight:700;color:#f1f5f9;">{name}</div>
                   <div style="font-size:11px;color:#94a3b8;line-height:1.4;">{desc}</div>
                   <div style="margin-top:8px;padding:8px 12px;background:#151820;border:1px solid #282d3b;border-radius:4px;display:grid;grid-template-columns:1fr 1fr;gap:6px;font-size:11px;">
                     <div><span style="color:#64748b;">Category:</span> <b style="color:#e2e8f0;">{cat}</b></div>
                     <div><span style="color:#64748b;">Ref Prefix:</span> <b style="color:#38bdf8;">{prefix}?</b></div>
                     <div><span style="color:#64748b;">Total Pads:</span> <b style="color:#e2e8f0;">{total_pads}</b></div>
                     <div><span style="color:#64748b;">Mount Type:</span> <b style="color:#e2e8f0;">{mount_type}</b></div>
                     <div><span style="color:#64748b;">Pitch:</span> <b style="color:#e2e8f0;">{pitch:.2} mm</b></div>
                     <div><span style="color:#64748b;">Body Size:</span> <b style="color:#e2e8f0;">{w:.2} x {h:.2} mm</b></div>
                   </div>"#,
                name = fp.name,
                desc = fp.description,
                cat = fp.category,
                prefix = fp.ref_prefix,
                total_pads = fp.pads.len(),
                mount_type = if tht_count > 0 && smd_count > 0 { "Mixed SMD/THT" } else if tht_count > 0 { "Through-Hole (THT)" } else { "Surface Mount (SMD)" },
                pitch = fp.pitch_mm,
                w = fp.body_w,
                h = fp.body_h,
            ));
        }
        if let Some(hint) = app.doc.get_element_by_id("fp-selected-hint") {
            hint.set_text_content(Some(&format!("Selected: {} ({})", fp.name, fp.category)));
        }
    }
}

pub fn load_custom_parsed_board(
    state: &Rc<RefCell<AppController>>,
    board: ParsedBoard,
    project_name: &str,
    file_kind: &str,
) {
    let t0 = js_sys::Date::now();
    let comp_count = board.components.len();
    {
        let mut app = state.borrow_mut();
        app.custom_board = Some((project_name.to_string(), board.clone()));
        app.project_name = project_name.to_string();
        app.active_project = project_name.to_string();
        app.collab.switch_room(project_name);
        app.rel_origin_mm = (board.bounds[0], board.bounds[3]);
        app.board_rev_a = board.clone();
        app.board_rev_b = board.clone();
        app.board_diff = board.clone();
        app.current_board = board;
        app.undo_stack.clear();
        app.selected_primitive_idx = None;
        app.route_start_mm = None;
        app.poly_points_mm.clear();
        app.parse_time_ms = js_sys::Date::now() - t0;
        app.populate_net_selector();
        app.select_net_probe(0);
        app.rebuild_gpu_buffer_with_markers();
        if let Some(ref mut r) = app.renderer {
            r.fit_to_board();
        }
        app.draw_all();
        app.update_telemetry_hud();

        if let Some(badge) = app.doc.get_element_by_id("bom-count-badge") {
            badge.set_text_content(Some(&comp_count.to_string()));
        }

        // Register custom project in topbar project selector
        if let Some(sel) = app
            .doc
            .get_element_by_id("rust-project-select")
            .and_then(|e| e.dyn_into::<HtmlSelectElement>().ok())
        {
            let current_val = sel.value();
            if current_val != project_name {
                if let Ok(opt) = app.doc.create_element("option") {
                    let _ = opt.set_attribute("value", project_name);
                    opt.set_text_content(Some(project_name));
                    let _ = sel.append_child(&opt);
                }
                sel.set_value(project_name);
            }
        }

        // Add to home projects list
        if let Some(container) = app.doc.get_element_by_id("home-custom-projects-container") {
            let row_html = format!(
                r##"<div class="home-project-row" title="Open {name}">
                     <div class="home-row-main">
                       <div class="home-row-icon">
                         <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#60a5fa" stroke-width="1.8"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline></svg>
                       </div>
                       <div>
                         <div class="home-row-name">{name}</div>
                         <div class="home-row-file">{kind}</div>
                       </div>
                     </div>
                     <div class="home-row-meta">
                       <span class="home-pill">Custom Board</span>
                       <span class="home-pill" style="color:#4ade80;">Active</span>
                     </div>
                     <button class="home-row-btn">
                       <span>Open</span>
                       <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="9 18 15 12 9 6"></polyline></svg>
                     </button>
                   </div>"##,
                name = project_name,
                kind = file_kind
            );
            container.set_inner_html(&row_html);
        }
    }
    render_parts_list(state);
    run_live_drc(state);
    switch_view_tab(state, "pcb");
}

pub fn load_kicad_pcb_string(state: &Rc<RefCell<AppController>>, content: &str, project_name: &str) {
    let board = parse_kicad_pcb_str(content);
    load_custom_parsed_board(state, board, project_name, "Loaded KiCad PCB Board");
}

pub fn handle_loaded_file_bytes(state: &Rc<RefCell<AppController>>, bytes: &[u8], filename: &str) {
    let lower = filename.to_lowercase();
    let is_cfb = altium_parser::CfbReader::is_cfb(bytes);

    if lower.ends_with(".pcbdoc") || lower.ends_with(".brd") || (is_cfb && !lower.ends_with(".schdoc")) {
        let b64 = gerber_exporter::to_base64(bytes);
        let format_hint = if lower.ends_with(".pcbdoc") {
            "altium"
        } else if lower.ends_with(".brd") {
            "eagle"
        } else {
            "auto"
        };
        let payload = serde_json::json!({
            "filename": filename,
            "content_base64": b64,
            "format": format_hint,
        });

        let state_clone = state.clone();
        let fname_clone = filename.to_string();
        let bytes_owned = bytes.to_vec();

        fetch_post_json_with_callback("/api/import-pcb", &payload.to_string(), move |resp_str| {
            if let Ok(resp_json) = serde_json::from_str::<serde_json::Value>(&resp_str) {
                if let Some(kicad_pcb_str) = resp_json.get("kicad_pcb").and_then(|v| v.as_str()) {
                    if !kicad_pcb_str.is_empty() {
                        load_kicad_pcb_string(&state_clone, kicad_pcb_str, &fname_clone);
                        return;
                    }
                }
            }
            // Fallback to client-side parser if server returned error or offline
            match altium_parser::parse_altium_pcbdoc_bytes(&bytes_owned) {
                Ok(board) => {
                    load_custom_parsed_board(&state_clone, board, &fname_clone, "Altium .PcbDoc Board");
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to parse Altium PcbDoc: {}", e).into());
                }
            }
        });
    } else if lower.ends_with(".schdoc") || (is_cfb && lower.ends_with(".schdoc")) {
        match schematic::parse_altium_schdoc_bytes(bytes, "root", filename) {
            Ok(doc) => {
                state.borrow_mut().schematic_doc = doc;
                render_active_schematic_sheet(state);
                switch_view_tab(state, "schematic");
            }
            Err(e) => {
                web_sys::console::error_1(&format!("Failed to parse Altium SchDoc: {}", e).into());
            }
        }
    } else if lower.ends_with(".kicad_sch") {
        if let Ok(text) = std::str::from_utf8(bytes) {
            let doc = schematic::parse_kicad_sch_str(text, "root", filename);
            state.borrow_mut().schematic_doc = doc;
            render_active_schematic_sheet(state);
            switch_view_tab(state, "schematic");
        }
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        if text.contains("(kicad_sch") {
            let doc = schematic::parse_kicad_sch_str(text, "root", filename);
            state.borrow_mut().schematic_doc = doc;
            render_active_schematic_sheet(state);
            switch_view_tab(state, "schematic");
        } else {
            load_kicad_pcb_string(state, text, filename);
        }
    }
}

fn read_file_as_pcb(state: Rc<RefCell<AppController>>, file: web_sys::File) {
    let file_name = file.name();
    let reader = match web_sys::FileReader::new() {
        Ok(r) => r,
        Err(_) => return,
    };
    let r_clone = reader.clone();
    let st = state.clone();
    let name_clone = file_name.clone();
    let onload = Closure::wrap(Box::new(move |_e: web_sys::Event| {
        if let Ok(js_val) = r_clone.result() {
            let uint8 = js_sys::Uint8Array::new(&js_val);
            let bytes = uint8.to_vec();
            handle_loaded_file_bytes(&st, &bytes, &name_clone);
        }
    }) as Box<dyn FnMut(_)>);
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    onload.forget();
    let _ = reader.read_as_array_buffer(&file);
}

pub fn setup_home_and_project_hub(doc: &Document, state: Rc<RefCell<AppController>>) {
    // Return to Projects Home (Brand Logo click or Topbar Home button)
    for btn_id in ["brand-home-link", "btn-topbar-home"] {
        if let Some(btn) = doc.get_element_by_id(btn_id) {
            let st = state.clone();
            let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
                switch_view_tab(&st, "home");
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    // 1. Open New Project Modal buttons
    for btn_id in ["btn-home-new-proj", "btn-topbar-new"] {
        if let Some(btn) = doc.get_element_by_id(btn_id) {
            let doc_clone = doc.clone();
            let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
                if let Some(m) = doc_clone.get_element_by_id("modal-new-project") {
                    let _ = m.class_list().remove_1("hidden");
                }
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    // Close New Project Modal buttons
    for btn_id in ["btn-close-new-proj-modal", "btn-cancel-new-proj-modal"] {
        if let Some(btn) = doc.get_element_by_id(btn_id) {
            let doc_clone = doc.clone();
            let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
                if let Some(m) = doc_clone.get_element_by_id("modal-new-project") {
                    let _ = m.class_list().add_1("hidden");
                }
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    // Template dropdown change: show/hide custom dimensions
    if let Some(sel) = doc.get_element_by_id("select-new-proj-template") {
        let doc_clone = doc.clone();
        let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
            if let Some(target) = e.target().and_then(|t| t.dyn_into::<HtmlSelectElement>().ok()) {
                let is_custom = target.value() == "custom";
                if let Some(row) = doc_clone.get_element_by_id("row-custom-dims") {
                    if is_custom {
                        let _ = row.class_list().remove_1("hidden");
                    } else {
                        let _ = row.class_list().add_1("hidden");
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Confirm Create Project in Modal
    if let Some(btn) = doc.get_element_by_id("btn-confirm-create-proj") {
        let st = state.clone();
        let doc_clone = doc.clone();
        let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
            let name = doc_clone
                .get_element_by_id("input-new-proj-name")
                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                .map(|i| i.value())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "New_Project".to_string());
            let template = doc_clone
                .get_element_by_id("select-new-proj-template")
                .and_then(|e| e.dyn_into::<HtmlSelectElement>().ok())
                .map(|s| s.value())
                .unwrap_or_else(|| "blank".to_string());
            let width: f32 = doc_clone
                .get_element_by_id("input-new-proj-width")
                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                .and_then(|i| i.value().parse().ok())
                .unwrap_or(100.0);
            let height: f32 = doc_clone
                .get_element_by_id("input-new-proj-height")
                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                .and_then(|i| i.value().parse().ok())
                .unwrap_or(80.0);

            let pcb_content = generate_template_kicad_pcb(&template, &name, width, height);
            if let Some(m) = doc_clone.get_element_by_id("modal-new-project") {
                let _ = m.class_list().add_1("hidden");
            }
            load_kicad_pcb_string(&st, &pcb_content, &name);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // 2. Direct Template Cards
    let template_cards = [
        ("card-tmpl-blank", "blank", "Blank_2Layer_Board", 100.0f32, 80.0f32),
        ("card-tmpl-rpi", "rpi_hat", "Raspberry_Pi_HAT", 65.0, 56.0),
        ("card-tmpl-iot", "iot_sensor", "IoT_Micro_Sensor", 50.0, 40.0),
        ("card-tmpl-eurocard", "eurocard", "Eurocard_3U", 160.0, 100.0),
    ];
    for (card_id, tmpl, name, w, h) in template_cards {
        if let Some(card) = doc.get_element_by_id(card_id) {
            let st = state.clone();
            let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
                let pcb_content = generate_template_kicad_pcb(tmpl, name, w, h);
                load_kicad_pcb_string(&st, &pcb_content, name);
            }) as Box<dyn FnMut(_)>);
            let _ = card.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    // 3. Sample Project Cards
    if let Some(card) = doc.get_element_by_id("card-proj-cm5") {
        let st = state.clone();
        let doc_clone = doc.clone();
        let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
            {
                let mut app = st.borrow_mut();
                if app.active_project != "cm5_minima" {
                    app.active_project = "cm5_minima".to_string();
                    drop(app);
                    fetch_project_boards(st.clone(), "cm5_minima");
                }
            }
            if let Some(sel) = doc_clone.get_element_by_id("rust-project-select").and_then(|e| e.dyn_into::<HtmlSelectElement>().ok()) {
                sel.set_value("cm5_minima");
            }
            switch_view_tab(&st, "pcb");
        }) as Box<dyn FnMut(_)>);
        let _ = card.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(card) = doc.get_element_by_id("card-proj-complex") {
        let st = state.clone();
        let doc_clone = doc.clone();
        let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
            {
                let mut app = st.borrow_mut();
                if app.active_project != "complex_hierarchy" {
                    app.active_project = "complex_hierarchy".to_string();
                    drop(app);
                    fetch_project_boards(st.clone(), "complex_hierarchy");
                }
            }
            if let Some(sel) = doc_clone.get_element_by_id("rust-project-select").and_then(|e| e.dyn_into::<HtmlSelectElement>().ok()) {
                sel.set_value("complex_hierarchy");
            }
            switch_view_tab(&st, "pcb");
        }) as Box<dyn FnMut(_)>);
        let _ = card.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Return to editor button
    if let Some(btn) = doc.get_element_by_id("btn-home-return-editor") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
            switch_view_tab(&st, "pcb");
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Custom projects container row click
    if let Some(container) = doc.get_element_by_id("home-custom-projects-container") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
            let active_proj = {
                let app = st.borrow();
                if let Some((ref cname, _)) = app.custom_board {
                    cname.clone()
                } else {
                    app.active_project.clone()
                }
            };
            fetch_project_boards(st.clone(), &active_proj);
            switch_view_tab(&st, "pcb");
        }) as Box<dyn FnMut(_)>);
        let _ = container.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // 4. File picker trigger buttons
    for btn_id in ["btn-home-open-file", "btn-topbar-open"] {
        if let Some(btn) = doc.get_element_by_id(btn_id) {
            let doc_clone = doc.clone();
            let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
                e.prevent_default();
                if let Some(inp) = doc_clone.get_element_by_id("home-file-input").and_then(|el| el.dyn_into::<HtmlInputElement>().ok()) {
                    inp.click();
                }
            }) as Box<dyn FnMut(_)>);
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    // File input change handler
    if let Some(inp) = doc.get_element_by_id("home-file-input").and_then(|el| el.dyn_into::<HtmlInputElement>().ok()) {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: web_sys::Event| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                if let Some(files) = input.files() {
                    if let Some(file) = files.get(0) {
                        read_file_as_pcb(st.clone(), file);
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = inp.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // 5. Drag & Drop on #home-dropzone
    if let Some(dropzone) = doc.get_element_by_id("home-dropzone") {
        let dz_click = dropzone.clone();
        let doc_clone = doc.clone();
        let click_cb = Closure::wrap(Box::new(move |_e: web_sys::Event| {
            if let Some(inp) = doc_clone.get_element_by_id("home-file-input").and_then(|el| el.dyn_into::<HtmlInputElement>().ok()) {
                inp.click();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = dz_click.add_event_listener_with_callback("click", click_cb.as_ref().unchecked_ref());
        click_cb.forget();

        let dz_dragover = dropzone.clone();
        let dragover_cb = Closure::wrap(Box::new(move |e: web_sys::DragEvent| {
            e.prevent_default();
            e.stop_propagation();
            let _ = dz_dragover.class_list().add_1("dragover");
        }) as Box<dyn FnMut(_)>);
        let _ = dropzone.add_event_listener_with_callback("dragover", dragover_cb.as_ref().unchecked_ref());
        dragover_cb.forget();

        let dz_dragleave = dropzone.clone();
        let dragleave_cb = Closure::wrap(Box::new(move |e: web_sys::DragEvent| {
            e.prevent_default();
            let _ = dz_dragleave.class_list().remove_1("dragover");
        }) as Box<dyn FnMut(_)>);
        let _ = dropzone.add_event_listener_with_callback("dragleave", dragleave_cb.as_ref().unchecked_ref());
        dragleave_cb.forget();

        let dz_drop = dropzone.clone();
        let st = state;
        let drop_cb = Closure::wrap(Box::new(move |e: web_sys::DragEvent| {
            e.prevent_default();
            e.stop_propagation();
            let _ = dz_drop.class_list().remove_1("dragover");
            if let Some(dt) = e.data_transfer() {
                if let Some(files) = dt.files() {
                    if let Some(file) = files.get(0) {
                        read_file_as_pcb(st.clone(), file);
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = dropzone.add_event_listener_with_callback("drop", drop_cb.as_ref().unchecked_ref());
        drop_cb.forget();
    }
}

fn init_collab_websocket(state: Rc<RefCell<AppController>>) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let host = window.location().host().unwrap_or_default();
    if host.is_empty() {
        return;
    }
    let ws_url = format!("ws://{}/api/collab/ws", host);
    let ws = match web_sys::WebSocket::new(&ws_url) {
        Ok(ws) => ws,
        Err(_) => return,
    };

    state.borrow_mut().collab.ws = Some(ws.clone());

    // OnOpen
    {
        let st = state.clone();
        let onopen = Closure::wrap(Box::new(move || {
            let mut app = st.borrow_mut();
            app.collab.is_connected = true;
            let join_msg = CollabMessage::Join {
                project_id: app.collab.current_project_id.clone(),
                user_id: app.collab.local_user_id.clone(),
                user_name: app.collab.local_user_name.clone(),
                color: app.collab.local_color.clone(),
            };
            app.collab.send_message(&join_msg);
            if let Some(pill) = app.doc.get_element_by_id("collab-status-pill") {
                let _ = pill.set_attribute(
                    "style",
                    "background:#0f291e;border-color:#166534;color:#4ade80;font-size:11px;font-weight:600;display:flex;align-items:center;gap:6px;cursor:pointer;",
                );
            }
            if let Some(txt) = app.doc.get_element_by_id("collab-status-text") {
                txt.set_text_content(Some("Co-Layout (Live)"));
            }
        }) as Box<dyn FnMut()>);
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen.forget();
    }

    // OnMessage
    {
        let st = state.clone();
        let onmsg = Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
            if let Some(text) = e.data().as_string() {
                let maybe_msg = {
                    let mut app = st.borrow_mut();
                    app.collab.handle_incoming_message(&text)
                };

                if let Some(msg) = maybe_msg {
                    let mut app = st.borrow_mut();
                    match msg {
                        CollabMessage::AddTrack {
                            sx,
                            sy,
                            ex,
                            ey,
                            w,
                            layer,
                            net_id,
                            ..
                        } => {
                            app.current_board.add_primitive(
                                sx, sy, ex, ey, w, w, layer, net_id, 0.0,
                            );
                            app.rebuild_gpu_buffer_with_markers();
                        }
                        CollabMessage::AddVia {
                            x,
                            y,
                            size,
                            drill,
                            net_id,
                            ..
                        } => {
                            app.current_board.add_primitive(
                                x,
                                y,
                                x,
                                y,
                                size,
                                drill,
                                LAYER_F_CU | LAYER_B_CU,
                                net_id,
                                1.0,
                            );
                            app.rebuild_gpu_buffer_with_markers();
                        }
                        CollabMessage::SyncSnapshot {
                            tracks,
                            vias,
                            tombstones,
                            locked_nets,
                            ..
                        } => {
                            let tomb_set: std::collections::HashSet<String> = tombstones.into_iter().collect();
                            for trk in tracks {
                                if !tomb_set.contains(&trk.id) {
                                    app.current_board.add_primitive(
                                        trk.sx, trk.sy, trk.ex, trk.ey, trk.w, trk.w, trk.layer, trk.net_id, 0.0,
                                    );
                                }
                            }
                            for via in vias {
                                if !tomb_set.contains(&via.id) {
                                    app.current_board.add_primitive(
                                        via.x, via.y, via.x, via.y, via.size, via.drill, LAYER_F_CU | LAYER_B_CU, via.net_id, 1.0,
                                    );
                                }
                            }
                            for (net_id, uid, uname) in locked_nets {
                                let exp = js_sys::Date::now() + 15000.0;
                                app.collab.locked_nets.insert(net_id, (uid, uname, exp));
                            }
                            app.rebuild_gpu_buffer_with_markers();
                        }
                        _ => {}
                    }
                    let peer_count = app.collab.peers.len();
                    if let Some(txt) = app.doc.get_element_by_id("collab-status-text") {
                        if peer_count > 0 {
                            txt.set_text_content(Some(&format!(
                                "Co-Layout ({} peers)",
                                peer_count + 1
                            )));
                        } else {
                            txt.set_text_content(Some("Co-Layout (Live)"));
                        }
                    }
                    app.mark_render_dirty(&st);
                }
            }
        }) as Box<dyn FnMut(_)>);
        ws.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));
        onmsg.forget();
    }

    // OnClose
    {
        let st = state;
        let onclose = Closure::wrap(Box::new(move || {
            let mut app = st.borrow_mut();
            app.collab.is_connected = false;
            if let Some(pill) = app.doc.get_element_by_id("collab-status-pill") {
                let _ = pill.set_attribute(
                    "style",
                    "background:#221815;border-color:#7f1d1d;color:#f87171;font-size:11px;font-weight:600;display:flex;align-items:center;gap:6px;cursor:pointer;",
                );
            }
            if let Some(txt) = app.doc.get_element_by_id("collab-status-text") {
                txt.set_text_content(Some("Co-Layout (Offline)"));
            }
        }) as Box<dyn FnMut()>);
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        onclose.forget();
    }
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

fn fetch_post_json_with_callback<F: FnOnce(String) + 'static>(url: &str, body_json: &str, callback: F) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_body(&JsValue::from_str(body_json));

    if let Ok(headers) = web_sys::Headers::new() {
        let _ = headers.set("Content-Type", "application/json");
        init.set_headers(&headers);
    }

    let request = match web_sys::Request::new_with_str_and_init(url, &init) {
        Ok(req) => req,
        Err(_) => return,
    };

    let promise = window.fetch_with_request(&request);
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
