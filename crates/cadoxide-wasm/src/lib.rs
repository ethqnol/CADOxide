pub mod gl_renderer;
pub mod parser;

use gl_renderer::RustGpuRenderer;
use parser::{
    compute_vector_diff_boards, generate_hdi_stress_board, parse_kicad_pcb_str, push_raw,
    LAYER_B_CU, LAYER_DRC_MARKER, LAYER_EDGE_CUTS, LAYER_F_CU, LAYER_F_SILKS, LAYER_IN_CU,
    LAYER_REVIEW_PIN, ParsedBoard,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
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
    PlaceVia,
    PlacePad,
    DrawLine,
    Measure,
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
    pub active_tab: String, // "pcb" | "schematic" | "diff" | "drc" | "parts"
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
    pub drag_3d_pan_mode: bool, // false = Left-drag Orbits 3D, true = Left-drag Pans 3D

    // Interactive CAD Tool & Editing State
    pub active_tool: CadTool,
    pub active_draw_layer: u32,
    pub active_track_width_mm: f32,
    pub active_via_size_mm: f32,
    pub active_via_drill_mm: f32,
    pub grid_snap_mm: f32,
    pub selected_primitive_idx: Option<usize>,
    pub route_start_mm: Option<(f32, f32, u32)>, // (x, y, net_id)
    pub cursor_snapped_mm: (f32, f32),

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
    pub fn snap_mm(&self, x: f32, y: f32) -> (f32, f32) {
        let g = self.grid_snap_mm.max(0.01);
        ((x / g).round() * g, (y / g).round() * g)
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

    pub fn set_active_tool(&mut self, tool: CadTool) {
        self.active_tool = tool;
        self.route_start_mm = None;

        for (btn_id, t) in [
            ("btn-tool-select", CadTool::SelectMove),
            ("btn-tool-route", CadTool::RouteTrack),
            ("btn-tool-via", CadTool::PlaceVia),
            ("btn-tool-pad", CadTool::PlacePad),
            ("btn-tool-line", CadTool::DrawLine),
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
            CadTool::SelectMove => "Select / Move (S)",
            CadTool::RouteTrack => "Route Track (X)",
            CadTool::PlaceVia => "Place Plated Via (V)",
            CadTool::PlacePad => "Place SMD Pad (P)",
            CadTool::DrawLine => "Draw Line / Outline (L)",
            CadTool::Measure => "Measure Distance (M)",
        };
        if let Some(lbl) = self.doc.get_element_by_id("active-tool-badge") {
            lbl.set_text_content(Some(tool_name));
        }

        self.rebuild_gpu_buffer_with_markers();
        self.render_inspector_panel();
    }

    pub fn select_net_probe(&mut self, net_id: i32) {
        if let Some(ref mut r) = self.renderer {
            r.selected_net_id = net_id.max(0);
            r.draw();
        }
        if let Some(sel) = self
            .doc
            .get_element_by_id("rust-net-select")
            .and_then(|e| e.dyn_into::<HtmlSelectElement>().ok())
        {
            sel.set_value(&net_id.max(0).to_string());
        }
        self.render_inspector_panel();
    }

    pub fn populate_net_selector(&self) {
        if let Some(sel) = self.doc.get_element_by_id("rust-net-select") {
            let mut net_list: Vec<_> = self.current_board.nets.values().collect();
            net_list.sort_by(|a, b| {
                b.total_length_mm
                    .partial_cmp(&a.total_length_mm)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            let mut html = String::from(r#"<option value="0">Net Filter: All Nets</option>"#);
            for n in net_list.iter().take(80) {
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
            // Append DRC markers as GPU-instanced rings (layer bit LAYER_DRC_MARKER = 128, prim_type = 5.0)
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

            // Live Interactive CAD Tool Preview Overlay on GPU Canvas
            let (cx, cy) = self.cursor_snapped_mm;
            match self.active_tool {
                CadTool::RouteTrack => {
                    if let Some((sx, sy, net_id)) = self.route_start_mm {
                        let packed_w = (net_id as f32) * 16.0 + 0.0;
                        // 45-degree posture knee calculation
                        let dx = cx - sx;
                        let dy = cy - sy;
                        let adx = dx.abs();
                        let ady = dy.abs();
                        let (kx, ky) = if adx > ady {
                            (sx + dx.signum() * (adx - ady), sy)
                        } else {
                            (sx, sy + dy.signum() * (ady - adx))
                        };
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
                CadTool::DrawLine => {
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
                CadTool::Measure => {
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
                            1.2,
                            1.2,
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
                            1.2,
                            1.2,
                            LAYER_REVIEW_PIN as f32,
                            6.0,
                        );
                    }
                }
                CadTool::SelectMove => {}
            }
        }

        let bounds = base.bounds;
        let sel_idx = self.selected_primitive_idx.map(|i| i as i32).unwrap_or(-1);
        if let Some(ref mut r) = self.renderer {
            r.selected_instance_id = sel_idx;
            r.upload_instances(&combined, bounds);
            r.draw();
        }
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

        // 1. Selected Primitive Property Editor
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

                let sel_fcu = if (layer_bits & LAYER_F_CU) != 0 && (layer_bits & LAYER_B_CU) == 0 {
                    "selected"
                } else {
                    ""
                };
                let sel_incu = if (layer_bits & LAYER_IN_CU) != 0 {
                    "selected"
                } else {
                    ""
                };
                let sel_bcu = if (layer_bits & LAYER_B_CU) != 0 && (layer_bits & LAYER_F_CU) == 0 {
                    "selected"
                } else {
                    ""
                };
                let sel_pth = if (layer_bits & LAYER_F_CU) != 0 && (layer_bits & LAYER_B_CU) != 0 {
                    "selected"
                } else {
                    ""
                };
                let sel_silk = if (layer_bits & LAYER_F_SILKS) != 0 {
                    "selected"
                } else {
                    ""
                };
                let sel_edge = if (layer_bits & LAYER_EDGE_CUTS) != 0 {
                    "selected"
                } else {
                    ""
                };

                html.push_str(&format!(
                    r##"
                    <div style="border-bottom:1px solid #262a33;">
                      <div style="padding:7px 10px;background:#1a1d24;display:flex;align-items:center;justify-content:space-between;">
                        <span style="font-weight:700;color:#f3d06b;font-size:11px;">{type_label} (#{idx})</span>
                        <button id="btn-prop-delete" class="cad-btn cad-btn-danger" style="height:20px;padding:0 6px;font-size:10px;">Delete (Del)</button>
                      </div>
                      <div class="cad-prop-grid">
                        <span class="cad-prop-label">Layer</span>
                        <select id="prop-layer" class="cad-select" style="width:100%;">
                          <option value="1" {sel_fcu}>F.Cu (Front Copper)</option>
                          <option value="4096" {sel_incu}>In1..4.Cu (Inner)</option>
                          <option value="2" {sel_bcu}>B.Cu (Back Copper)</option>
                          <option value="3" {sel_pth}>F.Cu + B.Cu (PTH/Via)</option>
                          <option value="8" {sel_silk}>F.SilkS (Silkscreen)</option>
                          <option value="4" {sel_edge}>Edge.Cuts (Outline)</option>
                        </select>
                        <span class="cad-prop-label">Net ID</span>
                        <input id="prop-net" type="number" step="1" min="0" value="{net_id}" class="cad-input" />
                        <span class="cad-prop-label">Start / X (mm)</span>
                        <input id="prop-x0" type="number" step="0.05" value="{x0:.3}" class="cad-input" />
                        <span class="cad-prop-label">Start / Y (mm)</span>
                        <input id="prop-y0" type="number" step="0.05" value="{y0:.3}" class="cad-input" />
                        <span class="cad-prop-label">End X (mm)</span>
                        <input id="prop-x1" type="number" step="0.05" value="{x1:.3}" class="cad-input" />
                        <span class="cad-prop-label">End Y (mm)</span>
                        <input id="prop-y1" type="number" step="0.05" value="{y1:.3}" class="cad-input" />
                        <span class="cad-prop-label">Width / Ø (mm)</span>
                        <input id="prop-w" type="number" step="0.02" min="0.05" value="{w:.3}" class="cad-input" />
                        <span class="cad-prop-label">Height/Drill</span>
                        <input id="prop-h" type="number" step="0.02" min="0.05" value="{h:.3}" class="cad-input" />
                      </div>
                      <div style="padding:0 10px 9px 10px;display:flex;gap:6px;">
                        <button id="btn-apply-props" class="cad-btn cad-btn-active" style="flex:1;justify-content:center;">Apply Properties</button>
                      </div>
                    </div>
                    "##,
                    type_label = type_label,
                    idx = idx,
                    sel_fcu = sel_fcu,
                    sel_incu = sel_incu,
                    sel_bcu = sel_bcu,
                    sel_pth = sel_pth,
                    sel_silk = sel_silk,
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
        } else {
            let tool_help = match self.active_tool {
                CadTool::SelectMove => {
                    "Click any track, via, or pad to inspect & edit its properties. Drag a selected object to move it on the grid."
                }
                CadTool::RouteTrack => {
                    "Click on any pad, via, or grid point to start routing a 45° copper trace. Click to place segments; Right-Click or Esc to finish."
                }
                CadTool::PlaceVia => {
                    "Click anywhere on the board to place a plated through-hole via connecting F.Cu and B.Cu."
                }
                CadTool::PlacePad => {
                    "Click anywhere on the board to place an SMD component pad on the active layer."
                }
                CadTool::DrawLine => {
                    "Click start point and end point to draw a graphic line or Edge.Cuts board outline."
                }
                CadTool::Measure => {
                    "Click start point and move cursor to measure exact ΔX, ΔY, Euclidean distance (mm/mils), and angle."
                }
            };
            html.push_str(&format!(
                r#"<div style="padding:9px 10px;border-bottom:1px solid #262a33;color:#949eb0;font-size:11px;line-height:1.45;">
                     {tool_help}
                   </div>"#,
                tool_help = tool_help
            ));
        }

        // 2. Electrical Net Length & Delay Report (if a net is selected)
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
                <div style="padding:9px 10px;border-bottom:1px solid #262a33;background:#14181f;">
                  <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:6px;">
                    <span style="font-weight:700;color:#34d399;font-family:monospace;font-size:11px;">NET #{nid}: {name}</span>
                    <button id="btn-clear-net-insp" class="cad-btn" style="height:18px;padding:0 6px;font-size:10px;">Clear</button>
                  </div>
                  <div style="display:grid;grid-template-columns:1fr 1fr;gap:4px 8px;font-family:monospace;font-size:11px;">
                    <span style="color:#8b93a6;">Routed Length:</span>
                    <span style="color:#f0f4f8;text-align:right;font-weight:600;">{len_mm:.3} mm ({len_mils:.1} mil)</span>
                    <span style="color:#8b93a6;">FR4 Delay (Td):</span>
                    <span style="color:#60a5fa;text-align:right;">{delay_ps:.1} ps</span>
                    <span style="color:#8b93a6;">Segments / Vias:</span>
                    <span style="color:#d0d6e0;text-align:right;">{segs} segs / {vias} vias</span>
                    <span style="color:#8b93a6;">Connected Pads:</span>
                    <span style="color:#d0d6e0;text-align:right;">{pads} pads</span>
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

        // 3. Keyboard Shortcuts Quick Reference
        html.push_str(
            r#"
            <div style="padding:9px 10px;font-size:11px;color:#7c8596;">
              <div style="font-weight:700;font-size:10px;text-transform:uppercase;color:#8e96a8;margin-bottom:6px;">CAD Shortcuts</div>
              <div style="display:grid;grid-template-columns:64px 1fr;gap:3px 6px;font-family:monospace;font-size:10.5px;">
                <span style="color:#d4b047;">S / Esc</span><span>Select &amp; Move Object</span>
                <span style="color:#d4b047;">X</span><span>Interactive Route Track</span>
                <span style="color:#d4b047;">V</span><span>Place Plated Via</span>
                <span style="color:#d4b047;">P</span><span>Place SMD Pad</span>
                <span style="color:#d4b047;">L</span><span>Draw Graphic / Outline</span>
                <span style="color:#d4b047;">M</span><span>Point-to-Point Ruler</span>
                <span style="color:#d4b047;">3</span><span>Toggle 3D Stackup Viewer</span>
                <span style="color:#d4b047;">Del</span><span>Delete Selected Object</span>
                <span style="color:#d4b047;">Ctrl+Z</span><span>Undo Last CAD Edit</span>
              </div>
            </div>
            "#,
        );

        container.set_inner_html(&html);
    }

    pub fn update_telemetry_hud(&self) {
        if let Some(el) = self.doc.get_element_by_id("rust-hud-stats") {
            let prims = self.current_board.primitive_count;
            let scale = self
                .renderer
                .as_ref()
                .map(|r| r.scale_px_per_mm)
                .unwrap_or(8.0);
            if let Some(zoom_el) = self.doc.get_element_by_id("rust-zoom-hud") {
                if self.active_tab == "schematic" {
                    zoom_el.set_text_content(Some(&format!(
                        "Schematic Zoom: {:.0}%   Pan: ({:.0}, {:.0})",
                        self.sch_zoom * 100.0,
                        self.sch_pan.0,
                        self.sch_pan.1
                    )));
                } else {
                    zoom_el.set_text_content(Some(&format!(
                        "Snap: {:.2} mm   Scale: {:.1} px/mm",
                        self.grid_snap_mm, scale
                    )));
                }
            }
            if self.active_tab == "diff" {
                let (add, rem, unch) = self.diff_stats;
                el.set_inner_html(&format!(
                    "<span style='color:#34d399;'>+{}</span> / \
                     <span style='color:#f87171;'>-{}</span> / \
                     <span>{} unch</span> | {} prims",
                    add, rem, unch, prims
                ));
            } else {
                let mode_badge = self
                    .renderer
                    .as_ref()
                    .map(|r| {
                        if r.mode_3d_explode > 0.05 {
                            "3D Stackup"
                        } else {
                            "2D Ortho"
                        }
                    })
                    .unwrap_or("2D Ortho");
                el.set_text_content(Some(&format!(
                    "{} | {} primitives ({:.1} ms)",
                    mode_badge, prims, self.parse_time_ms
                )));
            }
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

    let empty_board = ParsedBoard {
        instances: Vec::new(),
        primitive_count: 0,
        bounds: [70.0, 40.0, 190.0, 150.0],
        spatial_grid: HashMap::new(),
        nets: HashMap::new(),
    };

    let state = Rc::new(RefCell::new(AppController {
        doc: doc.clone(),
        renderer: None,
        active_tab: "pcb".to_string(),
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
        cursor_snapped_mm: (130.0, 95.0),
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
          <!-- Tier 1: Top Document & Workspace Mode Bar (32px) -->
          <header class="cad-menubar">
            <div class="cad-menubar-group">
              <div class="cad-brand">
                <svg width="15" height="15" viewBox="0 0 16 16" fill="none">
                  <rect x="1" y="1" width="14" height="14" rx="1.5" stroke="#c83434" stroke-width="1.5"/>
                  <circle cx="5" cy="5" r="1.5" fill="#d4b047"/>
                  <circle cx="11" cy="11" r="1.5" fill="#d4b047"/>
                  <path d="M5 5L8 8H11V11" stroke="#c83434" stroke-width="1.4" stroke-linecap="round"/>
                </svg>
                <span>CADoxide</span>
              </div>
              <div class="cad-divider-v"></div>
              <select id="rust-project-select" class="cad-select" title="Active KiCad PCB File">
                <option value="complex_hierarchy">complex_hierarchy.kicad_pcb (2-Layer Analog)</option>
                <option value="cm5_minima">CM5_MINIMA_3.kicad_pcb (4-Layer RPi CM5)</option>
              </select>
              <button id="btn-100k-hdi" class="cad-btn" title="Generate 100,000-primitive synthetic HDI test board">
                HDI-100K
              </button>
              <a id="btn-export-pcb" href="#" download="edited_board.kicad_pcb" class="cad-btn" title="Download edited board as .kicad_pcb">
                Save .kicad_pcb
              </a>
              <a id="link-3d-vrml" href="/samples/complex_hierarchy/board.wrl" download class="cad-btn" title="Export KiCad 3D VRML Model (.wrl)">
                Export .wrl
              </a>
            </div>

            <!-- Center Flat Desktop Workspace Tabs -->
            <div class="cad-tabs">
              <button id="tab-pcb" class="cad-tab cad-tab-active">PCB Editor</button>
              <button id="tab-schematic" class="cad-tab">Schematic Editor</button>
              <button id="tab-diff" class="cad-tab">Layer Diff</button>
              <button id="tab-drc" class="cad-tab">DRC (<span id="drc-count-badge">69</span>)</button>
              <button id="tab-parts" class="cad-tab">BOM (8)</button>
            </div>

            <!-- Right Edit Actions -->
            <div class="cad-menubar-group">
              <button id="btn-undo" class="cad-btn" title="Undo Last Edit (Ctrl+Z)">
                Undo
              </button>
              <button id="btn-delete-selected" class="cad-btn cad-btn-danger" title="Delete Selected Primitive (Del)">
                Delete
              </button>
            </div>
          </header>

          <!-- Tier 2: Contextual CAD Routing / 3D / Diff Parameters Bar (28px) -->
          <div class="cad-param-bar">
            <div class="cad-menubar-group">
              <span class="cad-param-label">Tool:</span>
              <span id="active-tool-badge" style="font-family:monospace;font-size:11px;color:#f3d06b;font-weight:600;padding:0 4px;">Select / Move (S)</span>
              <div class="cad-divider-v"></div>

              <span class="cad-param-label">Layer:</span>
              <select id="select-draw-layer" class="cad-select" title="Active Drawing / Routing Layer">
                <option value="1">F.Cu (Front Copper)</option>
                <option value="4096">In1..4.Cu (Inner)</option>
                <option value="2">B.Cu (Back Copper)</option>
                <option value="8">F.SilkS (Silkscreen)</option>
                <option value="4">Edge.Cuts (Outline)</option>
              </select>

              <span class="cad-param-label">Track W:</span>
              <select id="select-track-width" class="cad-select" title="Active Copper Track Width">
                <option value="0.127">0.127 mm (5 mil)</option>
                <option value="0.200">0.200 mm (8 mil)</option>
                <option value="0.250" selected>0.250 mm (10 mil)</option>
                <option value="0.400">0.400 mm (16 mil)</option>
                <option value="0.800">0.800 mm (31 mil)</option>
              </select>

              <span class="cad-param-label">Via:</span>
              <select id="select-via-size" class="cad-select" title="Active Plated Via Outer / Drill Diameter">
                <option value="0.45,0.20">0.45 / 0.20 mm</option>
                <option value="0.60,0.30" selected>0.60 / 0.30 mm</option>
                <option value="0.80,0.40">0.80 / 0.40 mm</option>
              </select>

              <span class="cad-param-label">Grid:</span>
              <select id="select-grid-snap" class="cad-select" title="Coordinate Grid Snap Step">
                <option value="0.05">0.05 mm</option>
                <option value="0.10">0.10 mm</option>
                <option value="0.25" selected>0.25 mm</option>
                <option value="0.50">0.50 mm</option>
                <option value="1.00">1.00 mm</option>
              </select>
            </div>

            <!-- Contextual 3D Stackup Controls (Shown when 3D Mode is active) -->
            <div id="toolbar-3d-controls" class="hidden cad-menubar-group">
              <div class="cad-divider-v"></div>
              <button id="btn-3d-drag-mode" class="cad-btn cad-btn-active" title="Toggle Left-Mouse Drag between 3D Orbit and 3D Pan (Right-drag or Shift-drag always Pans)">
                Left-Drag: Orbit 3D
              </button>
              <button id="btn-3d-iso" class="cad-btn" title="Isometric 3D View">Iso</button>
              <button id="btn-3d-top" class="cad-btn" title="Top-Down 3D View">Top</button>
              <button id="btn-3d-flip" class="cad-btn" title="Flip Board (Bottom View)">Flip</button>
              <span class="cad-param-label">Z-Spread:</span>
              <input id="input-3d-explode" type="range" min="20" max="250" value="100" style="width:70px;accent-color:#d4b047;cursor:pointer;" />
            </div>

            <!-- Contextual Diff Controls (Shown only on Layer Diff Tab) -->
            <div id="diff-toolbar-controls" class="hidden cad-menubar-group">
              <div class="cad-divider-v"></div>
              <button id="btn-diff-boolean" class="cad-btn cad-btn-active">Overlay</button>
              <button id="btn-diff-wipe" class="cad-btn">Split Wipe</button>
              <input id="input-wipe-slider" type="range" min="0" max="100" value="50" style="width:80px;accent-color:#d4b047;cursor:pointer;" />
            </div>

            <div class="cad-menubar-group">
              <select id="rust-net-select" class="cad-select" style="width:190px;" title="Highlight electrical net across all layers">
                <option value="0">Net Filter: All Nets</option>
              </select>
            </div>
          </div>

          <!-- Main CAD Workspace (Left Toolstrip + Center Viewport + Right Dock) -->
          <div class="cad-workspace">
            <!-- Left Vertical CAD Tool Strip -->
            <aside class="cad-toolstrip">
              <button id="btn-tool-select" class="cad-tool-btn cad-tool-btn-active" title="Select / Move Object (S) — Click to inspect/edit, Drag to move">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                  <path d="M3 2L13 7.5L8.5 9L7 13.5L3 2Z"/>
                </svg>
              </button>
              <button id="btn-tool-route" class="cad-tool-btn" title="Route Copper Track (X) — Click start &amp; end points to route traces">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
                  <path d="M2.5 13.5L6.5 9.5H10.5L13.5 6.5"/>
                  <circle cx="2.5" cy="13.5" r="1.5" fill="currentColor"/>
                  <circle cx="13.5" cy="6.5" r="1.5" fill="currentColor"/>
                </svg>
              </button>
              <button id="btn-tool-via" class="cad-tool-btn" title="Place Plated Through-Hole Via (V)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
                  <circle cx="8" cy="8" r="5.5"/>
                  <circle cx="8" cy="8" r="2.2"/>
                </svg>
              </button>
              <button id="btn-tool-pad" class="cad-tool-btn" title="Place SMD Component Pad (P)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
                  <rect x="3" y="4.5" width="10" height="7" rx="1.5"/>
                </svg>
              </button>
              <button id="btn-tool-line" class="cad-tool-btn" title="Draw Graphic / Edge.Cuts Line (L)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
                  <path d="M2.5 13.5L13.5 2.5"/>
                </svg>
              </button>
              <button id="btn-tool-measure" class="cad-tool-btn" title="Point-to-Point Distance Ruler (M)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
                  <path d="M2 11L11 2L14 5L5 14L2 11Z"/>
                  <path d="M5 8L6.5 9.5M7.5 5.5L9 7M10 3L11.5 4.5"/>
                </svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-zoom-in" class="cad-tool-btn" title="Zoom In (+)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
                  <path d="M8 3V13M3 8H13"/>
                </svg>
              </button>
              <button id="btn-zoom-out" class="cad-tool-btn" title="Zoom Out (-)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
                  <path d="M3 8H13"/>
                </svg>
              </button>
              <button id="btn-zoom-fit" class="cad-tool-btn" title="Fit to Viewport (Home)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
                  <path d="M2 6V2H6M10 2H14V6M14 10V14H10M6 14H2V10"/>
                </svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-3d-stackup" class="cad-tool-btn" title="Toggle 3D Multi-Layer Stackup Viewer (3) — Drag to Orbit, Right-Drag/Shift-Drag to Pan">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
                  <path d="M8 1.5L14 5V11L8 14.5L2 11V5L8 1.5Z"/>
                  <path d="M2 5L8 8.5L14 5M8 8.5V14.5"/>
                </svg>
              </button>
            </aside>

            <!-- Center GPU PCB Canvas & Overlays -->
            <div id="center-viewport" class="cad-viewport">
              <canvas id="rust-gpu-canvas" class="cad-canvas"></canvas>

              <!-- Interactive Pan & Wheel-Zoom Schematic Editor View -->
              <div id="schematic-view-panel" class="hidden" style="position:absolute;inset:0;display:flex;flex-direction:column;background:#0e1013;z-index:30;">
                <div style="height:30px;background:#16181d;border-bottom:1px solid #262a33;display:flex;align-items:center;justify-content:space-between;padding:0 10px;">
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
                <div class="cad-panel-header" style="height:30px;padding:0 14px;">
                  <span>DESIGN RULE CHECK (DRC) VIOLATIONS</span>
                  <span style="color:#8e96a8;font-weight:400;text-transform:none;">Click any violation row to center &amp; zoom PCB viewport</span>
                </div>
                <div id="drc-list-container" style="flex:1;overflow-y:auto;"></div>
              </div>

              <!-- BOM & Component Library View -->
              <div id="parts-view-panel" class="hidden" style="position:absolute;inset:0;display:flex;flex-direction:column;background:#121418;z-index:30;">
                <div class="cad-panel-header" style="height:34px;padding:0 14px;">
                  <span>BILL OF MATERIALS &amp; COMPONENT LIBRARY</span>
                  <input id="input-parts-search" type="text" placeholder="Filter MPN, manufacturer, footprint..." class="cad-select" style="width:240px;" />
                </div>
                <div id="parts-list-container" style="flex:1;overflow-y:auto;"></div>
              </div>
            </div>

            <!-- Right Docked Appearance & Object Inspector Panel -->
            <aside class="cad-sidebar">
              <div class="cad-panel-header">
                <span>Layers &amp; Visibility</span>
                <span>On/Off</span>
              </div>
              <div id="layer-controls-list" class="cad-layer-list">
                <label class="cad-layer-row">
                  <span class="cad-layer-left">
                    <input type="checkbox" data-bit="1" checked />
                    <span class="cad-swatch" style="background:#c83434;"></span>
                    <span>F.Cu</span>
                  </span>
                  <span class="cad-layer-type">Front Copper</span>
                </label>
                <label class="cad-layer-row">
                  <span class="cad-layer-left">
                    <input type="checkbox" data-bit="4096" checked />
                    <span class="cad-swatch" style="background:#bf8f28;"></span>
                    <span>In1..In4.Cu</span>
                  </span>
                  <span class="cad-layer-type">Inner Copper</span>
                </label>
                <label class="cad-layer-row">
                  <span class="cad-layer-left">
                    <input type="checkbox" data-bit="2" checked />
                    <span class="cad-swatch" style="background:#3370c2;"></span>
                    <span>B.Cu</span>
                  </span>
                  <span class="cad-layer-type">Back Copper</span>
                </label>
                <label class="cad-layer-row">
                  <span class="cad-layer-left">
                    <input type="checkbox" data-bit="8" checked />
                    <span class="cad-swatch" style="background:#d6e3e8;"></span>
                    <span>F.SilkS</span>
                  </span>
                  <span class="cad-layer-type">Silkscreen</span>
                </label>
                <label class="cad-layer-row">
                  <span class="cad-layer-left">
                    <input type="checkbox" data-bit="32" />
                    <span class="cad-swatch" style="background:#1c8a61;"></span>
                    <span>F.Mask</span>
                  </span>
                  <span class="cad-layer-type">Solder Mask</span>
                </label>
                <label class="cad-layer-row">
                  <span class="cad-layer-left">
                    <input type="checkbox" data-bit="4" checked />
                    <span class="cad-swatch" style="background:#e0c238;"></span>
                    <span>Edge.Cuts</span>
                  </span>
                  <span class="cad-layer-type">Board Outline</span>
                </label>
                <label class="cad-layer-row">
                  <span class="cad-layer-left">
                    <input type="checkbox" data-bit="128" checked />
                    <span class="cad-swatch" style="background:#f59e0b;"></span>
                    <span>DRC Markers</span>
                  </span>
                  <span class="cad-layer-type">Violations</span>
                </label>
              </div>

              <!-- Object Properties & Net Inspector Section -->
              <div class="cad-panel-header">
                <span>Inspector &amp; Properties</span>
                <span>CAD Edit</span>
              </div>
              <div id="cad-inspector-content" style="flex:1;overflow-y:auto;"></div>
            </aside>
          </div>

          <!-- Bottom Docked Industrial CAD Status Bar -->
          <footer class="cad-statusbar">
            <div id="rust-coord-hud" class="cad-status-cell" style="width:215px;color:#e5e9f0;">
              X: 130.000 mm   Y:  95.000 mm
            </div>
            <div id="rust-zoom-hud" class="cad-status-cell" style="width:215px;color:#949eb0;">
              Snap: 0.25 mm   Scale: 8.0 px/mm
            </div>
            <div id="rust-hover-inspector" class="cad-status-cell cad-status-cell-flex" style="color:#f3d06b;">
              Ready — Select tool (S), Route Track (X), Place Via (V), Pad (P), or click any object to edit
            </div>
            <div id="rust-hud-stats" class="cad-status-cell" style="border-right:none;color:#949eb0;">
              2D Ortho | 0 primitives
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
    bind_schematic_interactions(doc, state.clone());
    bind_keyboard_shortcuts(doc, state.clone());
    state.borrow().render_inspector_panel();
    render_parts_list(&state);
}

fn bind_canvas_interactions(canvas: &HtmlCanvasElement, state: Rc<RefCell<AppController>>) {
    // Prevent default right-click context menu on CAD canvas so Right-Click finishes routes or pans in 3D
    {
        let st = state.clone();
        let on_ctx = Closure::wrap(Box::new(move |e: MouseEvent| {
            e.prevent_default();
            let mut app = st.borrow_mut();
            if app.route_start_mm.is_some() {
                app.route_start_mm = None;
                app.rebuild_gpu_buffer_with_markers();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = canvas
            .add_event_listener_with_callback("contextmenu", on_ctx.as_ref().unchecked_ref());
        on_ctx.forget();
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
                r.draw();
            }
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

            // Middle or Right mouse button always pans camera
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
                .unwrap_or(4607);
            let is_3d = app
                .renderer
                .as_ref()
                .map(|r| r.mode_3d_explode > 0.05)
                .unwrap_or(false);

            // In 3D mode, left-dragging orbits or pans the 3D stackup directly
            if is_3d {
                app.is_dragging = true;
                app.is_dragging_primitive = false;
                return;
            }

            match app.active_tool {
                CadTool::SelectMove => {
                    // Check if clicking on an already-selected primitive to drag-move it
                    let hit = app
                        .current_board
                        .query_nearest(raw_mm.0, raw_mm.1, active_mask, 1.6);
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
                            let dx = ex - sx;
                            let dy = ey - sy;
                            let adx = dx.abs();
                            let ady = dy.abs();
                            let (kx, ky) = if adx > ady {
                                (sx + dx.signum() * (adx - ady), sy)
                            } else {
                                (sx, sy + dy.signum() * (ady - adx))
                            };
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
                CadTool::DrawLine => {
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
                        // In 3D mode: Right-drag (2), Middle-drag (1), Shift+Left-drag, or Pan-Toggle pans; Left-drag orbits!
                        if drag_btn == 1 || drag_btn == 2 || e.shift_key() || pan_mode_3d {
                            r.pan_by_css_px(dx, dy);
                        } else {
                            r.orbit_by_css_px(dx, dy);
                        }
                    } else {
                        r.pan_by_css_px(dx, dy);
                    }
                    r.draw();
                }
                return;
            }

            let mut coord_pair = (130.0f32, 95.0f32);
            let mut active_mask = 4607u32;
            if let Some(ref r) = app.renderer {
                coord_pair = r.screen_to_board_mm(cx, cy);
                active_mask = r.active_layer_mask;
            }
            let snapped = app.snap_mm(coord_pair.0, coord_pair.1);
            app.cursor_snapped_mm = snapped;

            if let Some(hud) = app.doc.get_element_by_id("rust-coord-hud") {
                hud.set_text_content(Some(&format!(
                    "X: {:7.3} mm   Y: {:7.3} mm",
                    snapped.0, snapped.1
                )));
            }

            // If a drawing/routing/measuring tool is active, update live GPU preview
            if app.active_tool != CadTool::SelectMove {
                if app.active_tool == CadTool::Measure {
                    if let Some((sx, sy, _)) = app.route_start_mm {
                        let dx = snapped.0 - sx;
                        let dy = snapped.1 - sy;
                        let dist = (dx * dx + dy * dy).sqrt();
                        let mils = dist * 39.3701;
                        let ang = dy.atan2(dx).to_degrees();
                        if let Some(insp) = app.doc.get_element_by_id("rust-hover-inspector") {
                            insp.set_text_content(Some(&format!(
                                "Ruler: Dist = {:.3} mm ({:.1} mil) | ΔX = {:+.3} mm, ΔY = {:+.3} mm | Angle = {:.1}°",
                                dist, mils, dx, dy, ang
                            )));
                        }
                    }
                }
                app.rebuild_gpu_buffer_with_markers();
                return;
            }

            // O(1) Spatial Grid Query in Rust WASM
            let hit = app
                .current_board
                .query_nearest(coord_pair.0, coord_pair.1, active_mask, 1.8);
            let new_hover_id = hit.as_ref().map(|h| h.instance_idx as i32).unwrap_or(-1);

            if let Some(insp) = app.doc.get_element_by_id("rust-hover-inspector") {
                if let Some(ref info) = hit {
                    insp.set_text_content(Some(&info.summary));
                } else {
                    insp.set_text_content(Some(
                        "Ready — Click any object to select & edit properties, or choose Route (X) / Via (V) / Pad (P)",
                    ));
                }
            }

            if let Some(ref mut r) = app.renderer {
                if r.hovered_instance_id != new_hover_id {
                    r.hovered_instance_id = new_hover_id;
                    r.draw();
                }
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

            if was_dragging && app.active_tab == "pcb" && app.active_tool == CadTool::SelectMove && e.button() == 0 {
                let dx = (e.client_x() as f64) - app.mouse_down_pos.0;
                let dy = (e.client_y() as f64) - app.mouse_down_pos.1;
                if (dx * dx + dy * dy).sqrt() < 5.0 {
                    let mut coord_pair = (0.0f32, 0.0f32);
                    let mut active_mask = 4607u32;
                    if let Some(ref r) = app.renderer {
                        coord_pair = r.screen_to_board_mm(e.client_x() as f64, e.client_y() as f64);
                        active_mask = r.active_layer_mask;
                    }
                    let hit = app
                        .current_board
                        .query_nearest(coord_pair.0, coord_pair.1, active_mask, 1.8);
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

fn bind_schematic_interactions(doc: &Document, state: Rc<RefCell<AppController>>) {
    if let Some(vp) = doc.get_element_by_id("schematic-viewport") {
        // Mouse wheel zoom on Schematic
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
        // Mouse drag pan on Schematic
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
        // Ignore shortcuts when typing inside an <input> or <select>
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
                if app.route_start_mm.is_some() {
                    app.route_start_mm = None;
                    app.rebuild_gpu_buffer_with_markers();
                } else {
                    app.set_active_tool(CadTool::SelectMove);
                }
            }
            "s" | "S" => app.set_active_tool(CadTool::SelectMove),
            "x" | "X" => app.set_active_tool(CadTool::RouteTrack),
            "v" | "V" => app.set_active_tool(CadTool::PlaceVia),
            "p" | "P" => app.set_active_tool(CadTool::PlacePad),
            "l" | "L" => app.set_active_tool(CadTool::DrawLine),
            "m" | "M" => app.set_active_tool(CadTool::Measure),
            "Delete" | "Backspace" => {
                e.prevent_default();
                app.delete_selected_primitive();
            }
            "3" => {
                toggle_3d_mode(&mut app);
            }
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
        r.draw();
    }
    if let Some(btn) = app.doc.get_element_by_id("btn-3d-stackup") {
        btn.set_class_name(if is_3d {
            "cad-tool-btn cad-tool-btn-active"
        } else {
            "cad-tool-btn"
        });
    }
    if let Some(ctrl) = app.doc.get_element_by_id("toolbar-3d-controls") {
        if is_3d {
            let _ = ctrl.class_list().remove_1("hidden");
        } else {
            let _ = ctrl.class_list().add_1("hidden");
        }
    }
    app.update_telemetry_hud();
}

fn bind_toolbar_and_tabs(doc: &Document, state: Rc<RefCell<AppController>>) {
    // Tool Strip Buttons
    for (btn_id, tool) in [
        ("btn-tool-select", CadTool::SelectMove),
        ("btn-tool-route", CadTool::RouteTrack),
        ("btn-tool-via", CadTool::PlaceVia),
        ("btn-tool-pad", CadTool::PlacePad),
        ("btn-tool-line", CadTool::DrawLine),
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

    // Undo & Delete Buttons
    if let Some(btn) = doc.get_element_by_id("btn-undo") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            st.borrow_mut().perform_undo();
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
                st.borrow_mut().active_draw_layer = t.value().parse().unwrap_or(LAYER_F_CU);
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
                app.update_telemetry_hud();
            }
        }) as Box<dyn FnMut(_)>);
        let _ = sel.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Delegated Inspector Panel Clicks (Apply Properties, Delete Primitive, Clear Net)
    if let Some(insp_panel) = doc.get_element_by_id("cad-inspector-content") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                let id = el.id();
                if id == "btn-prop-delete" {
                    st.borrow_mut().delete_selected_primitive();
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
                r.draw();
            }
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
                r.draw();
            }
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
                r.draw();
            }
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
                    r.draw();
                }
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

    // Zoom In / Out / Fit (Works on BOTH PCB Canvas and Schematic Editor!)
    if let Some(btn) = doc.get_element_by_id("btn-zoom-in") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if app.active_tab == "schematic" {
                app.sch_zoom = (app.sch_zoom * 1.25).min(10.0);
                app.apply_schematic_transform();
            } else if let Some(ref mut r) = app.renderer {
                r.scale_px_per_mm = (r.scale_px_per_mm * 1.25).min(500.0);
                r.draw();
            }
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
                r.draw();
            }
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
                r.draw();
            }
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
                                        r.draw();
                                    }
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

    // Diff Mode Controls (Overlay vs Split Wipe Slider)
    if let Some(btn) = doc.get_element_by_id("btn-diff-boolean") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if let Some(ref mut r) = app.renderer {
                r.diff_mode = 1.0;
                r.draw();
            }
            if let Some(b1) = app.doc.get_element_by_id("btn-diff-boolean") {
                b1.set_class_name("cad-btn cad-btn-active");
            }
            if let Some(b2) = app.doc.get_element_by_id("btn-diff-wipe") {
                b2.set_class_name("cad-btn");
            }
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
                r.draw();
            }
            if let Some(b1) = app.doc.get_element_by_id("btn-diff-boolean") {
                b1.set_class_name("cad-btn");
            }
            if let Some(b2) = app.doc.get_element_by_id("btn-diff-wipe") {
                b2.set_class_name("cad-btn cad-btn-active");
            }
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
                        r.draw();
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = slider.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Layer Visibility Checkboxes
    if let Some(list) = doc.get_element_by_id("layer-controls-list") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
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
                            r.draw();
                        }
                    }
                }
            }
        }) as Box<dyn FnMut(_)>);
        let _ = list.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
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
            r.draw();
        }
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
                     <span style="font-family:monospace;font-weight:700;color:#f3d06b;font-size:12px;">{mpn}</span>
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
                app.board_rev_a = board_a.clone();
                app.current_board = board_a;
                app.undo_stack.clear();
                app.selected_primitive_idx = None;
                app.route_start_mm = None;
                app.parse_time_ms = dt;
                app.populate_net_selector();
                app.select_net_probe(0);
                app.rebuild_gpu_buffer_with_markers();
                if let Some(ref mut r) = app.renderer {
                    r.fit_to_board();
                    r.draw();
                }
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
    static BENCH_BOARD: RefCell<ParsedBoard> = RefCell::new(ParsedBoard {
        instances: Vec::new(),
        primitive_count: 0,
        bounds: [70.0, 40.0, 190.0, 150.0],
        spatial_grid: HashMap::new(),
        nets: HashMap::new(),
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
