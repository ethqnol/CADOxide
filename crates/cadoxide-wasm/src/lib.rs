pub mod gl_renderer;
pub mod parser;

use gl_renderer::RustGpuRenderer;
use parser::{
    compute_vector_diff_boards, generate_hdi_stress_board, parse_kicad_pcb_str, push_raw,
    LAYER_DRC_MARKER, LAYER_REVIEW_PIN, ParsedBoard,
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
    Document, Element, Event, HtmlCanvasElement, HtmlInputElement, HtmlSelectElement, MouseEvent,
    RequestInit, Response, WheelEvent,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct ReviewMessage {
    pub author: String,
    pub body: String,
    pub time: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ReviewPinThread {
    pub id: String,
    pub layer: String,
    pub x: f32,
    pub y: f32,
    pub resolved: bool,
    pub author: String,
    pub messages: Vec<ReviewMessage>,
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
    pub active_tab: String, // "pcb" | "diff" | "schematic" | "drc" | "parts"
    pub active_project: String,
    pub board_rev_a: ParsedBoard,
    pub board_rev_b: ParsedBoard,
    pub board_diff: ParsedBoard,
    pub diff_stats: (usize, usize, usize), // (added, removed, unchanged)
    pub current_board: ParsedBoard,
    pub parse_time_ms: f64,
    pub is_dragging: bool,
    pub last_mouse: (f64, f64),
    pub mouse_down_pos: (f64, f64),
    pub drop_pin_mode: bool,
    pub pending_pin_coords: Option<(f32, f32)>,
    pub selected_thread_idx: usize,
    pub threads: Vec<ReviewPinThread>,
    pub drc_items: Vec<DrcItem>,
    pub parts_catalog: Vec<PartLibraryEntry>,
    pub parts_filter: String,
    pub active_schematic_svg: String,
}

impl AppController {
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
        if let Some(banner) = self.doc.get_element_by_id("net-probe-banner") {
            if net_id > 0 {
                let nid = net_id as u32;
                let stats = self.current_board.nets.get(&nid).cloned().unwrap_or_default();
                let name = if stats.name.is_empty() {
                    format!("Net #{}", nid)
                } else {
                    stats.name
                };
                let len_mils = stats.total_length_mm * 39.3701;
                let delay_ps = stats.total_length_mm * 6.67;
                if let Some(info_el) = self.doc.get_element_by_id("net-probe-info") {
                    info_el.set_inner_html(&format!(
                        "<span style='color:#34d399;font-weight:700;'>NET #{}: {}</span> \
                         <span style='color:#4b5563;margin:0 6px;'>|</span> \
                         <span style='color:#f3f4f6;'>Length: <b>{:.3} mm</b> ({:.1} mil)</span> \
                         <span style='color:#4b5563;margin:0 6px;'>|</span> \
                         <span style='color:#60a5fa;'>Td(FR4): {:.1} ps</span> \
                         <span style='color:#4b5563;margin:0 6px;'>|</span> \
                         <span style='color:#9ca3af;'>{} segs, {} vias, {} pads</span>",
                        nid,
                        name,
                        stats.total_length_mm,
                        len_mils,
                        delay_ps,
                        stats.segment_count,
                        stats.via_count,
                        stats.pad_count
                    ));
                }
                let _ = banner.class_list().remove_1("hidden");
            } else {
                let _ = banner.class_list().add_1("hidden");
            }
        }
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

            // Append Review Thread Pins as GPU-instanced targets (layer bit LAYER_REVIEW_PIN = 256, prim_type = 6.0 / 7.0)
            for t in &self.threads {
                let ptype = if t.resolved { 7.0 } else { 6.0 };
                push_raw(
                    &mut combined,
                    &mut count,
                    t.x,
                    t.y,
                    t.x,
                    t.y,
                    2.2,
                    2.2,
                    LAYER_REVIEW_PIN as f32,
                    ptype,
                );
            }
        }

        let bounds = base.bounds;
        if let Some(ref mut r) = self.renderer {
            r.upload_instances(&combined, bounds);
            r.draw();
        }
        self.update_telemetry_hud();
    }

    pub fn update_telemetry_hud(&self) {
        if let Some(el) = self.doc.get_element_by_id("rust-hud-stats") {
            let prims = self
                .renderer
                .as_ref()
                .map(|r| r.instance_count)
                .unwrap_or(0);
            let scale = self
                .renderer
                .as_ref()
                .map(|r| r.scale_px_per_mm)
                .unwrap_or(8.0);
            if let Some(zoom_el) = self.doc.get_element_by_id("rust-zoom-hud") {
                let grid_step = if scale > 24.0 {
                    "0.25 mm"
                } else if scale > 4.0 {
                    "1.00 mm"
                } else {
                    "10.0 mm"
                };
                zoom_el.set_text_content(Some(&format!(
                    "Grid: {}   Scale: {:.1} px/mm",
                    grid_step, scale
                )));
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

/// Automatically boots the 100% Pure Rust Frontend when the WASM module is loaded
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

    let initial_threads = vec![
        ReviewPinThread {
            id: "pin-1".to_string(),
            layer: "F.Cu".to_string(),
            x: 128.5,
            y: 82.0,
            resolved: false,
            author: "ElecLead_Alice".to_string(),
            messages: vec![ReviewMessage {
                author: "ElecLead_Alice".to_string(),
                body: "High-speed differential pair needs tighter clearance around via cluster to prevent return current discontinuity.".to_string(),
                time: "14:10".to_string(),
            }],
        },
        ReviewPinThread {
            id: "pin-2".to_string(),
            layer: "Edge.Cuts".to_string(),
            x: 160.0,
            y: 110.0,
            resolved: true,
            author: "MechEng_Bob".to_string(),
            messages: vec![ReviewMessage {
                author: "MechEng_Bob".to_string(),
                body: "Mounting hole clearance verified against DIN 7985 M3 standoffs.".to_string(),
                time: "13:45".to_string(),
            }],
        },
    ];

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
        parse_time_ms: 0.0,
        is_dragging: false,
        last_mouse: (0.0, 0.0),
        mouse_down_pos: (0.0, 0.0),
        drop_pin_mode: false,
        pending_pin_coords: None,
        selected_thread_idx: 0,
        threads: initial_threads,
        drc_items: Vec::new(),
        parts_catalog: default_parts_catalog(),
        parts_filter: String::new(),
        active_schematic_svg: "/samples/complex_hierarchy/complex_hierarchy.svg".to_string(),
    }));

    build_shell_dom(&doc, &root, state.clone());
    fetch_project_boards(state.clone(), "complex_hierarchy");
    sync_backend_telemetry_and_parts(state);
}

fn build_shell_dom(doc: &Document, root: &Element, state: Rc<RefCell<AppController>>) {
    root.set_inner_html(
        r##"
        <div class="cad-shell">
          <!-- Top Industrial EDA Menu & Document Bar -->
          <header class="cad-menubar">
            <div class="cad-menubar-group">
              <div class="cad-brand">
                <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                  <rect x="1" y="1" width="14" height="14" rx="1.5" stroke="#c83434" stroke-width="1.5"/>
                  <circle cx="5" cy="5" r="1.5" fill="#d4b047"/>
                  <circle cx="11" cy="11" r="1.5" fill="#d4b047"/>
                  <path d="M5 5L8 8H11V11" stroke="#c83434" stroke-width="1.4" stroke-linecap="round"/>
                </svg>
                <span>CADoxide</span>
              </div>
              <div class="cad-divider-v"></div>
              <select id="rust-project-select" class="cad-select" title="Active PCB Document">
                <option value="complex_hierarchy">complex_hierarchy.kicad_pcb (2-Layer Analog/HV)</option>
                <option value="cm5_minima">CM5_MINIMA_3.kicad_pcb (4-Layer RPi CM5 Carrier)</option>
              </select>
              <button id="btn-100k-hdi" class="cad-btn" title="Generate 100,000-primitive synthetic HDI test board">
                HDI-100K Bench
              </button>
            </div>

            <!-- Center Workspace Mode Switcher -->
            <div class="cad-tabs">
              <button id="tab-pcb" class="cad-tab cad-tab-active">PCB Layout</button>
              <button id="tab-diff" class="cad-tab">Layer Diff</button>
              <button id="tab-schematic" class="cad-tab">Schematic</button>
              <button id="tab-drc" class="cad-tab">DRC (<span id="drc-count-badge">69</span>)</button>
              <button id="tab-parts" class="cad-tab">BOM (8)</button>
            </div>

            <!-- Right Engineering & Net Controls -->
            <div class="cad-menubar-group">
              <div id="diff-toolbar-controls" class="hidden cad-menubar-group">
                <button id="btn-diff-boolean" class="cad-btn cad-btn-active">Overlay</button>
                <button id="btn-diff-wipe" class="cad-btn">Split Wipe</button>
                <input id="input-wipe-slider" type="range" min="0" max="100" value="50" style="width:90px;accent-color:#3b82f6;cursor:pointer;" />
                <div class="cad-divider-v"></div>
              </div>
              <select id="rust-net-select" class="cad-select" style="width:195px;" title="Highlight electrical net across all layers">
                <option value="0">Net Filter: All Nets</option>
              </select>
              <a id="link-3d-vrml" href="/samples/complex_hierarchy/board.wrl" download class="cad-btn" title="Export KiCad 3D VRML Model (.wrl)">
                Export .WRL
              </a>
              <div id="cluster-status-badge" class="cad-btn" style="color:#34d399;border-color:#1e3a2f;background:#0d1915;cursor:default;">
                KiCad 10.0.7
              </div>
            </div>
          </header>

          <!-- Main CAD Workspace (Left Toolstrip + Center Canvas + Right Appearance/Inspector Dock) -->
          <div class="cad-workspace">
            <!-- Left Vertical CAD Tool Strip -->
            <aside class="cad-toolstrip">
              <button id="btn-tool-select" class="cad-tool-btn cad-tool-btn-active" title="Select / Probe Net (Click any trace, via, or pad)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                  <path d="M3 2L13 7.5L8.5 9L7 13.5L3 2Z"/>
                </svg>
              </button>
              <button id="btn-drop-pin" class="cad-tool-btn" title="Place Design Review Annotation Marker">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
                  <circle cx="8" cy="7" r="4"/>
                  <path d="M8 11V15M8 1V3M1 7H3M13 7H15"/>
                </svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-zoom-in" class="cad-tool-btn" title="Zoom In">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
                  <path d="M8 3V13M3 8H13"/>
                </svg>
              </button>
              <button id="btn-zoom-out" class="cad-tool-btn" title="Zoom Out">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
                  <path d="M3 8H13"/>
                </svg>
              </button>
              <button id="btn-zoom-fit" class="cad-tool-btn" title="Fit Board to Viewport">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
                  <path d="M2 6V2H6M10 2H14V6M14 10V14H10M6 14H2V10"/>
                </svg>
              </button>
              <div class="cad-tool-sep"></div>
              <button id="btn-3d-stackup" class="cad-tool-btn" title="Toggle 3D Multi-Layer Stackup Orbit (Hold Shift + Drag to rotate)">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
                  <path d="M8 1.5L14 5V11L8 14.5L2 11V5L8 1.5Z"/>
                  <path d="M2 5L8 8.5L14 5M8 8.5V14.5"/>
                </svg>
              </button>
            </aside>

            <!-- Center GPU PCB Canvas & Overlays -->
            <div id="center-viewport" class="cad-viewport">
              <!-- Compact Floating Net Measurement Readout (Visible when a net is probed) -->
              <div id="net-probe-banner" class="hidden cad-net-hud">
                <div id="net-probe-info"></div>
                <button id="btn-clear-net" class="cad-btn" style="height:20px;padding:0 6px;font-size:10px;">Clear</button>
              </div>

              <!-- Primary Full-Screen Rust WebGL2 Instanced SDF Canvas -->
              <canvas id="rust-gpu-canvas" class="cad-canvas"></canvas>

              <!-- Schematic Hierarchy View -->
              <div id="schematic-view-panel" class="hidden absolute inset-0 flex flex-col z-30" style="background:#0c0e14;padding:12px;">
                <div style="display:flex;align-items:center;gap:6px;margin-bottom:10px;">
                  <span style="font-size:11px;font-weight:600;color:#8b93a6;text-transform:uppercase;">Sheet Hierarchy:</span>
                  <button data-sch="/samples/complex_hierarchy/complex_hierarchy.svg" class="sch-btn cad-btn cad-btn-active">Root Sheet</button>
                  <button data-sch="/samples/complex_hierarchy/complex_hierarchy-ampli_ht_horizontal.svg" class="sch-btn cad-btn">ampli_ht_horizontal.kicad_sch</button>
                  <button data-sch="/samples/complex_hierarchy/complex_hierarchy-ampli_ht_vertical.svg" class="sch-btn cad-btn">ampli_ht_vertical.kicad_sch</button>
                </div>
                <div class="flex-1 bg-white overflow-auto flex items-center justify-center" style="border:1px solid #252b3b;border-radius:2px;padding:12px;">
                  <img id="schematic-svg-img" src="/samples/complex_hierarchy/complex_hierarchy.svg" class="max-w-full max-h-full object-contain" />
                </div>
              </div>

              <!-- DRC Audit Table View -->
              <div id="drc-view-panel" class="hidden absolute inset-0 flex flex-col overflow-y-auto z-30" style="background:#0c0e14;">
                <div class="cad-panel-header" style="height:32px;padding:0 14px;">
                  <span>DESIGN RULE CHECK (DRC) VIOLATIONS</span>
                  <span style="color:#949eb2;font-weight:400;text-transform:none;">Click any row to center viewport on violation</span>
                </div>
                <div id="drc-list-container" style="flex:1;overflow-y:auto;"></div>
              </div>

              <!-- BOM & Component Library View -->
              <div id="parts-view-panel" class="hidden absolute inset-0 flex flex-col overflow-y-auto z-30" style="background:#0c0e14;">
                <div class="cad-panel-header" style="height:36px;padding:0 14px;">
                  <span>BILL OF MATERIALS &amp; COMPONENT LIBRARY</span>
                  <input id="input-parts-search" type="text" placeholder="Filter MPN, manufacturer, footprint..." class="cad-select" style="width:240px;" />
                </div>
                <div id="parts-list-container" style="flex:1;overflow-y:auto;"></div>
              </div>
            </div>

            <!-- Right Docked Appearance & Annotations Panel -->
            <aside class="cad-sidebar">
              <div class="cad-panel-header">
                <span>Layers &amp; Objects</span>
                <span>Visibility</span>
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
                <label class="cad-layer-row">
                  <span class="cad-layer-left">
                    <input type="checkbox" data-bit="256" checked />
                    <span class="cad-swatch" style="background:#ea384c;"></span>
                    <span>Annotations</span>
                  </span>
                  <span class="cad-layer-type">Review Pins</span>
                </label>
              </div>

              <!-- New Annotation Form (Shown when placing a review marker) -->
              <div id="new-pin-box" class="hidden" style="padding:8px 10px;background:#181c28;border-top:1px solid #2b3245;border-bottom:1px solid #2b3245;">
                <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:6px;font-size:11px;font-weight:600;color:#f87171;">
                  <span id="new-pin-coord-label">New Marker</span>
                  <button id="btn-cancel-pin" class="cad-btn" style="height:18px;padding:0 5px;">✕</button>
                </div>
                <input id="input-pin-author" type="text" value="HardwareEng" class="cad-select" style="width:100%;margin-bottom:5px;" />
                <input id="input-pin-msg" type="text" placeholder="Enter annotation note..." class="cad-select" style="width:100%;margin-bottom:6px;" />
                <button id="btn-save-pin" class="cad-btn cad-btn-active" style="width:100%;justify-content:center;">
                  Save Annotation
                </button>
              </div>

              <!-- Design Annotations Section -->
              <div class="cad-panel-header">
                <span>Design Annotations</span>
                <span id="threads-count-badge">2</span>
              </div>
              <div id="threads-list-container" style="flex:1;overflow-y:auto;"></div>
            </aside>
          </div>

          <!-- Bottom Docked Industrial CAD Status Bar -->
          <footer class="cad-statusbar">
            <div id="rust-coord-hud" class="cad-status-cell" style="width:210px;color:#e2e8f0;">
              X: 130.000 mm   Y:  95.000 mm
            </div>
            <div id="rust-zoom-hud" class="cad-status-cell" style="width:205px;color:#94a3b8;">
              Grid: 1.00 mm   Scale: 8.0 px/mm
            </div>
            <div id="rust-hover-inspector" class="cad-status-cell cad-status-cell-flex" style="color:#93c5fd;">
              Ready — Click any copper track, via, or pad to inspect net length
            </div>
            <div id="rust-hud-stats" class="cad-status-cell" style="border-right:none;color:#94a3b8;">
              2D Ortho | 0 primitives
            </div>
          </footer>
        </div>
        "##,
    );

    // Initialize WebGL2 Instanced SDF Renderer on `#rust-gpu-canvas`
    if let Some(canvas_el) = doc.get_element_by_id("rust-gpu-canvas") {
        if let Ok(canvas) = canvas_el.dyn_into::<HtmlCanvasElement>() {
            if let Ok(renderer) = RustGpuRenderer::new(canvas.clone()) {
                state.borrow_mut().renderer = Some(renderer);
                bind_canvas_interactions(&canvas, state.clone());
            }
        }
    }

    bind_toolbar_and_tabs(doc, state.clone());
    render_threads_sidebar(&state);
    render_parts_list(&state);
}

fn bind_canvas_interactions(canvas: &HtmlCanvasElement, state: Rc<RefCell<AppController>>) {
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

    // Mouse Down (Start Pan/Orbit or Drop Review Pin)
    {
        let st = state.clone();
        let on_down = Closure::wrap(Box::new(move |e: MouseEvent| {
            if e.button() != 0 {
                return;
            }
            let mut app = st.borrow_mut();
            let cx = e.client_x() as f64;
            let cy = e.client_y() as f64;
            if app.drop_pin_mode {
                if let Some(ref r) = app.renderer {
                    let (mm_x, mm_y) = r.screen_to_board_mm(cx, cy);
                    app.pending_pin_coords = Some((mm_x, mm_y));
                    app.drop_pin_mode = false;
                    if let Some(btn) = app.doc.get_element_by_id("btn-drop-pin") {
                        btn.set_class_name("cad-tool-btn");
                    }
                    if let Some(sel_btn) = app.doc.get_element_by_id("btn-tool-select") {
                        sel_btn.set_class_name("cad-tool-btn cad-tool-btn-active");
                    }
                    if let Some(lbl) = app.doc.get_element_by_id("new-pin-coord-label") {
                        lbl.set_text_content(Some(&format!(
                            "Marker @ ({:.2}, {:.2}) mm",
                            mm_x, mm_y
                        )));
                    }
                    if let Some(box_el) = app.doc.get_element_by_id("new-pin-box") {
                        let _ = box_el.class_list().remove_1("hidden");
                    }
                }
                return;
            }
            app.is_dragging = true;
            app.last_mouse = (cx, cy);
            app.mouse_down_pos = (cx, cy);
        }) as Box<dyn FnMut(_)>);
        let _ =
            canvas.add_event_listener_with_callback("mousedown", on_down.as_ref().unchecked_ref());
        on_down.forget();
    }

    // Mouse Move (Pan/Orbit GPU Camera + O(1) Spatial Grid Hover Inspection)
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
                if let Some(ref mut r) = app.renderer {
                    if r.mode_3d_explode > 0.05 && e.shift_key() {
                        r.orbit_by_css_px(dx, dy);
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
            if let Some(hud) = app.doc.get_element_by_id("rust-coord-hud") {
                hud.set_text_content(Some(&format!(
                    "X: {:7.3} mm   Y: {:7.3} mm",
                    coord_pair.0, coord_pair.1
                )));
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
                        "Ready — Click any copper track, via, or pad to inspect net length",
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

    // Mouse Up (Detect Click-to-Probe Net vs Drag Pan)
    {
        let st = state.clone();
        let on_up = Closure::wrap(Box::new(move |e: MouseEvent| {
            let mut app = st.borrow_mut();
            let was_dragging = app.is_dragging;
            app.is_dragging = false;
            if was_dragging && app.active_tab == "pcb" {
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
                    let net_to_select = hit.map(|h| h.net_id as i32).unwrap_or(0);
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
            st.borrow_mut().is_dragging = false;
        }) as Box<dyn FnMut(_)>);
        let _ = canvas
            .add_event_listener_with_callback("mouseleave", on_leave.as_ref().unchecked_ref());
        on_leave.forget();
    }
}

fn bind_toolbar_and_tabs(doc: &Document, state: Rc<RefCell<AppController>>) {
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

    // Clear Net Probe Button
    if let Some(clear_btn) = doc.get_element_by_id("btn-clear-net") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            st.borrow_mut().select_net_probe(0);
        }) as Box<dyn FnMut(_)>);
        let _ = clear_btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
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

    // 3D Stackup Orbit Toggle Button
    if let Some(btn) = doc.get_element_by_id("btn-3d-stackup") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            let mut is_3d = false;
            if let Some(ref mut r) = app.renderer {
                r.mode_3d_explode = if r.mode_3d_explode > 0.05 { 0.0 } else { 1.0 };
                is_3d = r.mode_3d_explode > 0.05;
                r.draw();
            }
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_class_name(if is_3d {
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

    // 100k HDI Stress Test Button
    if let Some(btn) = doc.get_element_by_id("btn-100k-hdi") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let t0 = js_sys::Date::now();
            let hdi_board = generate_hdi_stress_board(100_000);
            let dt = js_sys::Date::now() - t0;
            let mut app = st.borrow_mut();
            app.active_tab = "pcb".to_string();
            app.current_board = hdi_board;
            app.parse_time_ms = dt;
            if let Some(ref mut r) = app.renderer {
                r.diff_mode = 0.0;
            }
            app.populate_net_selector();
            app.select_net_probe(0);
            app.rebuild_gpu_buffer_with_markers();
            if let Some(ref mut r) = app.renderer {
                r.fit_to_board();
                r.draw();
            }
            app.update_telemetry_hud();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Zoom In / Out / Fit
    if let Some(btn) = doc.get_element_by_id("btn-zoom-in") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            if let Some(ref mut r) = app.renderer {
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
            if let Some(ref mut r) = app.renderer {
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
            if let Some(ref mut r) = app.renderer {
                r.fit_to_board();
                r.draw();
            }
            app.update_telemetry_hud();
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Select Tool & Drop Pin Tool Toggle
    if let Some(btn) = doc.get_element_by_id("btn-tool-select") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            app.drop_pin_mode = false;
            if let Some(s) = app.doc.get_element_by_id("btn-tool-select") {
                s.set_class_name("cad-tool-btn cad-tool-btn-active");
            }
            if let Some(p) = app.doc.get_element_by_id("btn-drop-pin") {
                p.set_class_name("cad-tool-btn");
            }
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-drop-pin") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let mut app = st.borrow_mut();
            app.drop_pin_mode = !app.drop_pin_mode;
            let active = app.drop_pin_mode;
            if let Some(p) = app.doc.get_element_by_id("btn-drop-pin") {
                p.set_class_name(if active {
                    "cad-tool-btn cad-tool-btn-active"
                } else {
                    "cad-tool-btn"
                });
            }
            if let Some(s) = app.doc.get_element_by_id("btn-tool-select") {
                s.set_class_name(if active {
                    "cad-tool-btn"
                } else {
                    "cad-tool-btn cad-tool-btn-active"
                });
            }
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    if let Some(btn) = doc.get_element_by_id("btn-cancel-pin") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            let app = st.borrow();
            if let Some(box_el) = app.doc.get_element_by_id("new-pin-box") {
                let _ = box_el.class_list().add_1("hidden");
            }
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Save New Pin Thread Button (Persists both locally and to Axum SQLite Backend)
    if let Some(btn) = doc.get_element_by_id("btn-save-pin") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |_e: Event| {
            {
                let mut app = st.borrow_mut();
                if let Some((x, y)) = app.pending_pin_coords.take() {
                    let author = app
                        .doc
                        .get_element_by_id("input-pin-author")
                        .and_then(|el| el.dyn_into::<HtmlInputElement>().ok())
                        .map(|i| i.value())
                        .unwrap_or_else(|| "Reviewer".to_string());
                    let msg = app
                        .doc
                        .get_element_by_id("input-pin-msg")
                        .and_then(|el| el.dyn_into::<HtmlInputElement>().ok())
                        .map(|i| i.value())
                        .unwrap_or_default();
                    if !msg.trim().is_empty() {
                        let new_id = format!("pin-{}", app.threads.len() + 1);
                        app.threads.insert(
                            0,
                            ReviewPinThread {
                                id: new_id,
                                layer: "F.Cu".to_string(),
                                x,
                                y,
                                resolved: false,
                                author: author.clone(),
                                messages: vec![ReviewMessage {
                                    author: author.clone(),
                                    body: msg.clone(),
                                    time: "Just now".to_string(),
                                }],
                            },
                        );

                        let payload = serde_json::json!({
                            "revision_id": "rev-main",
                            "file_type": "pcb",
                            "sheet_or_layer": "F.Cu",
                            "coord_x": x,
                            "coord_y": y,
                            "created_by": author,
                            "initial_message": msg,
                        });
                        post_json_fire_and_forget("/api/threads", &payload.to_string());
                    }
                    if let Some(box_el) = app.doc.get_element_by_id("new-pin-box") {
                        let _ = box_el.class_list().add_1("hidden");
                    }
                    app.rebuild_gpu_buffer_with_markers();
                }
            }
            render_threads_sidebar(&st);
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // View Mode Tabs
    for (tab_id, mode_name) in [
        ("tab-pcb", "pcb"),
        ("tab-diff", "diff"),
        ("tab-schematic", "schematic"),
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
                            let coords = st
                                .borrow()
                                .drc_items
                                .get(idx)
                                .map(|d| (d.x, d.y));
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
                if let Some(ref mut r) = app.renderer {
                    r.diff_mode = 2.0;
                    r.wipe_ratio = (val / 100.0).clamp(0.0, 1.0);
                    r.draw();
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
        let doc_clone = doc.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                if let Some(src) = el.get_attribute("data-sch") {
                    if let Some(img) = doc_clone.get_element_by_id("schematic-svg-img") {
                        let _ = img.set_attribute("src", &src);
                    }
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
        ("tab-diff", "diff"),
        ("tab-schematic", "schematic"),
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
        app.update_telemetry_hud();
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
                     <span style="font-family:monospace;font-weight:700;color:#60a5fa;font-size:12px;">{mpn}</span>
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

fn render_threads_sidebar(state: &Rc<RefCell<AppController>>) {
    let app = state.borrow();
    if let Some(badge) = app.doc.get_element_by_id("threads-count-badge") {
        badge.set_text_content(Some(&app.threads.len().to_string()));
    }
    let container = match app.doc.get_element_by_id("threads-list-container") {
        Some(c) => c,
        None => return,
    };

    let mut html = String::new();
    for (idx, t) in app.threads.iter().enumerate() {
        let dot_color = if t.resolved { "#10b981" } else { "#ef4444" };
        let resolve_label = if t.resolved { "RESOLVED" } else { "OPEN" };
        let body = t
            .messages
            .first()
            .map(|m| m.body.as_str())
            .unwrap_or("Annotation");
        html.push_str(&format!(
            r#"<div data-idx="{idx}" class="thread-card" style="padding:7px 9px;border-bottom:1px solid #1e2330;cursor:pointer;">
                 <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:4px;">
                   <span style="display:flex;align-items:center;gap:6px;font-weight:600;color:#d8dee9;font-size:11px;">
                     <span style="width:7px;height:7px;border-radius:1px;background:{dot_color};display:inline-block;"></span>
                     <span>{author}</span>
                   </span>
                   <div style="display:flex;align-items:center;gap:4px;">
                     <span style="font-family:monospace;font-size:10px;color:#60a5fa;">
                       ({x:.1}, {y:.1})
                     </span>
                     <button data-resolve-idx="{idx}" class="cad-btn" style="height:18px;padding:0 5px;font-size:9px;">{resolve_label}</button>
                   </div>
                 </div>
                 <div style="font-size:11px;color:#94a3b8;line-height:1.35;">{body}</div>
               </div>"#,
            idx = idx,
            dot_color = dot_color,
            author = t.author,
            x = t.x,
            y = t.y,
            resolve_label = resolve_label,
            body = body
        ));
    }
    container.set_inner_html(&html);

    let st = state.clone();
    let cb = Closure::wrap(Box::new(move |e: Event| {
        if let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
            if let Some(res_idx_str) = el.get_attribute("data-resolve-idx") {
                if let Ok(idx) = res_idx_str.parse::<usize>() {
                    {
                        let mut app = st.borrow_mut();
                        if let Some(thread) = app.threads.get_mut(idx) {
                            thread.resolved = !thread.resolved;
                            let status_str = if thread.resolved { "resolved" } else { "open" };
                            let url = format!("/api/threads/{}/status", thread.id);
                            let payload = serde_json::json!({ "status": status_str });
                            post_json_fire_and_forget(&url, &payload.to_string());
                        }
                        app.rebuild_gpu_buffer_with_markers();
                    }
                    render_threads_sidebar(&st);
                    return;
                }
            }
            if let Some(card) = el.closest(".thread-card").ok().flatten() {
                if let Some(idx_str) = card.get_attribute("data-idx") {
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        let mut app = st.borrow_mut();
                        if let Some(thread) = app.threads.get(idx).cloned() {
                            if let Some(ref mut r) = app.renderer {
                                r.center_mm = (thread.x, thread.y);
                                r.scale_px_per_mm = 18.0;
                                r.draw();
                            }
                            app.update_telemetry_hud();
                        }
                    }
                }
            }
        }
    }) as Box<dyn FnMut(_)>);
    let _ = container.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
    cb.forget();
}

fn sync_backend_telemetry_and_parts(state: Rc<RefCell<AppController>>) {
    // Sync /api/cluster/metrics
    let st_metrics = state.clone();
    fetch_text_with_callback("/api/cluster/metrics", move |json_str| {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_str) {
            let workers = val
                .get("worker_pool")
                .and_then(|w| w.get("max_concurrency"))
                .and_then(|v| v.as_u64())
                .unwrap_or(4);
            let app = st_metrics.borrow();
            if let Some(badge) = app.doc.get_element_by_id("cluster-status-badge") {
                badge.set_text_content(Some(&format!("ONLINE ({}W)", workers)));
            }
        }
    });

    // Sync /api/parts
    let st_parts = state;
    fetch_text_with_callback("/api/parts", move |json_str| {
        if let Ok(parts) = serde_json::from_str::<Vec<PartLibraryEntry>>(&json_str) {
            if !parts.is_empty() {
                st_parts.borrow_mut().parts_catalog = parts;
                render_parts_list(&st_parts);
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

    // Fetch Rev A PCB
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

            // Fetch Rev B PCB & Compute Pure-Rust Vector Diff
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

    // Fetch DRC Report JSON
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
        if let Ok(resp) = resp_val.dyn_into::<Response>() {
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

fn post_json_fire_and_forget(url: &str, body_json: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_body(&JsValue::from_str(body_json));
    if let Ok(headers) = web_sys::Headers::new() {
        let _ = headers.set("Content-Type", "application/json");
        init.set_headers(&headers);
    }
    let _ = window.fetch_with_str_and_init(url, &init);
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
