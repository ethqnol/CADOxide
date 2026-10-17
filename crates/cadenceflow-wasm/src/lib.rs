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
                        "<span class='text-emerald-300 font-bold'>⚡ NET #{}: {}</span> &bull; \
                         <span class='text-white font-semibold'>Length: {:.2} mm ({:.0} mils)</span> &bull; \
                         <span class='text-cyan-300'>Flight Delay: {:.1} ps</span> &bull; \
                         <span class='text-slate-300'>{} traces / {} vias / {} pads</span>",
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

            let mut html =
                String::from(r#"<option value="0">🔬 Probe Net (Click trace or select)...</option>"#);
            for n in net_list.iter().take(80) {
                let label = if n.name.is_empty() {
                    format!("Net #{}", n.id)
                } else {
                    n.name.clone()
                };
                html.push_str(&format!(
                    r#"<option value="{}">{} ({:.1} mm • {} vias)</option>"#,
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
            // Append DRC markers as GPU-instanced rings (layer bit LAYER_DRC_MARKER = 128)
            for d in &self.drc_items {
                if d.x > 1.0 && d.y > 1.0 {
                    push_raw(
                        &mut combined,
                        &mut count,
                        d.x,
                        d.y,
                        d.x,
                        d.y,
                        1.4,
                        0.5,
                        LAYER_DRC_MARKER as f32,
                        2.5,
                    );
                }
            }

            // Append Review Thread Pins as GPU-instanced circles (layer bit LAYER_REVIEW_PIN = 256)
            for t in &self.threads {
                let ptype = if t.resolved { 3.0 } else { 2.4 };
                push_raw(
                    &mut combined,
                    &mut count,
                    t.x,
                    t.y,
                    t.x,
                    t.y,
                    2.6,
                    2.6,
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
            if self.active_tab == "diff" {
                let (add, rem, unch) = self.diff_stats;
                el.set_inner_html(&format!(
                    "<span class='text-emerald-400 font-semibold'>+{} added</span> &bull; \
                     <span class='text-rose-400 font-semibold'>-{} removed</span> &bull; \
                     <span class='text-slate-400'>{} unchanged</span> &bull; \
                     <span class='text-indigo-400'>1 GPU Draw Call ({} inst)</span>",
                    add, rem, unch, prims
                ));
            } else {
                let mode_badge = self
                    .renderer
                    .as_ref()
                    .map(|r| {
                        if r.mode_3d_explode > 0.05 {
                            "3D Stackup Orbit"
                        } else {
                            "2D Ortho + LOD Cull"
                        }
                    })
                    .unwrap_or("2D Ortho");
                el.set_inner_html(&format!(
                    "<span class='text-emerald-400 font-semibold'>{}</span> &bull; \
                     <span class='text-slate-200'>{} GPU instances</span> &bull; \
                     <span class='text-indigo-400'>Parse+Grid: {:.1}ms (1 draw call)</span>",
                    mode_badge, prims, self.parse_time_ms
                ));
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
pub fn start_cadenceflow_app() {
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
        r#"
        <div class="w-screen h-screen flex flex-col bg-slate-950 text-slate-100 overflow-hidden select-none font-sans">
          <!-- Top Pure-Rust Header Bar -->
          <header class="h-14 bg-slate-900 border-b border-slate-800 px-4 flex items-center justify-between shrink-0 z-30">
            <div class="flex items-center space-x-3">
              <div class="w-8 h-8 rounded-lg bg-gradient-to-tr from-amber-500 to-indigo-600 flex items-center justify-center font-bold text-white shadow-lg">
                RS
              </div>
              <div>
                <div class="flex items-center space-x-2">
                  <span class="font-bold text-slate-100 tracking-tight text-base">CadenceFlow</span>
                  <span class="text-[10px] uppercase font-mono px-2 py-0.5 rounded bg-emerald-500/20 text-emerald-300 border border-emerald-500/40 font-semibold">
                    100% Pure Rust + WASM GPU
                  </span>
                </div>
              </div>
              <div class="h-4 w-px bg-slate-800 mx-1"></div>
              <select id="rust-project-select" class="bg-slate-950 border border-slate-700 text-xs text-slate-200 rounded-md px-2.5 py-1.5">
                <option value="complex_hierarchy">Audio Amp (complex_hierarchy.kicad_pcb)</option>
                <option value="cm5_minima">Raspberry Pi CM5 Minima (CM5_MINIMA_3.kicad_pcb)</option>
              </select>
            </div>

            <!-- View Mode Tabs -->
            <div class="flex items-center bg-slate-950 p-1 rounded-lg border border-slate-800 space-x-1">
              <button id="tab-pcb" class="px-3 py-1 rounded text-xs font-medium bg-indigo-600 text-white">PCB Canvas (GPU SDF)</button>
              <button id="tab-diff" class="px-3 py-1 rounded text-xs font-medium text-slate-400 hover:text-slate-200">Visual Diff (GPU Shader)</button>
              <button id="tab-schematic" class="px-3 py-1 rounded text-xs font-medium text-slate-400 hover:text-slate-200">Schematic Sheet</button>
              <button id="tab-drc" class="px-3 py-1 rounded text-xs font-medium text-slate-400 hover:text-slate-200">DRC Audit (<span id="drc-count-badge">69</span>)</button>
              <button id="tab-parts" class="px-3 py-1 rounded text-xs font-medium text-slate-400 hover:text-slate-200">Parts &amp; BOM (8)</button>
            </div>

            <!-- Right Benchmark & Telemetry Controls -->
            <div class="flex items-center space-x-2">
              <button id="btn-100k-hdi" class="px-2.5 py-1 rounded bg-amber-500/20 hover:bg-amber-500/30 border border-amber-500/40 text-amber-300 text-xs font-mono font-semibold cursor-pointer">
                ⚡ 100k HDI Stress Test
              </button>
              <div id="cluster-status-badge" class="px-2.5 py-1 rounded bg-slate-950 border border-slate-800 text-[11px] font-mono text-emerald-400">
                KiCad 10.0.7 &bull; CAS Active
              </div>
            </div>
          </header>

          <!-- Main Workspace Container -->
          <div class="flex-1 flex overflow-hidden relative">
            <!-- Center GPU Canvas & Overlay Area -->
            <div id="center-viewport" class="flex-1 h-full relative overflow-hidden bg-slate-950 flex flex-col">
              <!-- Floating GPU Toolbar -->
              <div id="gpu-toolbar" class="absolute top-4 left-4 z-20 flex items-center space-x-2 bg-slate-900/95 border border-slate-800 rounded-lg p-1.5 shadow-xl">
                <button id="btn-zoom-in" class="px-2.5 py-1 rounded hover:bg-slate-800 text-slate-200 text-xs font-mono font-bold" title="Zoom In">+</button>
                <button id="btn-zoom-out" class="px-2.5 py-1 rounded hover:bg-slate-800 text-slate-200 text-xs font-mono font-bold" title="Zoom Out">-</button>
                <button id="btn-zoom-fit" class="px-2.5 py-1 rounded hover:bg-slate-800 text-slate-200 text-xs font-mono" title="Fit Board">Fit</button>
                <div class="w-px h-4 bg-slate-800"></div>
                <button id="btn-3d-stackup" class="px-2.5 py-1 rounded bg-slate-800 hover:bg-indigo-600 text-slate-200 hover:text-white text-xs font-medium transition-colors" title="Toggle 3D Exploded Multi-Layer Stackup Orbit (Shift+Drag to rotate)">
                  🧊 3D Stackup Orbit
                </button>
                <a id="link-3d-vrml" href="/samples/complex_hierarchy/board.wrl" download class="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 text-cyan-300 text-xs font-mono" title="Download Headless KiCad 3D VRML Export (.wrl)">
                  ⬇ .WRL 3D
                </a>
                <div class="w-px h-4 bg-slate-800"></div>
                <select id="rust-net-select" class="bg-slate-950 border border-slate-700 text-xs text-emerald-300 rounded px-2 py-1 font-mono max-w-[230px]" title="Select or click any trace/pad/via to X-Ray its electrical net">
                  <option value="0">🔬 Probe Net (Click trace)...</option>
                </select>
                <div class="w-px h-4 bg-slate-800"></div>
                <button id="btn-drop-pin" class="px-2.5 py-1 rounded bg-slate-800 hover:bg-rose-600 text-slate-200 hover:text-white text-xs font-medium transition-colors">
                  📍 Drop Review Pin
                </button>

                <!-- Diff Mode Controls (Visible when Visual Diff tab active) -->
                <div id="diff-toolbar-controls" class="hidden flex items-center space-x-2 pl-2 border-l border-slate-800">
                  <button id="btn-diff-boolean" class="px-2.5 py-1 rounded bg-indigo-600 text-white text-xs font-medium">Boolean Shader</button>
                  <button id="btn-diff-wipe" class="px-2.5 py-1 rounded bg-slate-800 text-slate-300 text-xs font-medium">Split Wipe</button>
                  <input id="input-wipe-slider" type="range" min="0" max="100" value="50" class="w-28 accent-cyan-400 cursor-pointer" />
                </div>
              </div>

              <!-- Probed Electrical Net & Signal Length X-Ray Banner -->
              <div id="net-probe-banner" class="hidden absolute top-16 left-4 z-20 bg-slate-900/95 border border-emerald-500/50 rounded-lg px-3.5 py-2 shadow-2xl flex items-center space-x-3 text-xs font-mono">
                <div id="net-probe-info"></div>
                <button id="btn-clear-net" class="px-2 py-0.5 rounded bg-slate-800 hover:bg-rose-600 text-slate-200 hover:text-white text-[11px]">✕ Clear</button>
              </div>

              <!-- Bottom Coordinate, O(1) Spatial Hover Inspector & Rust GPU Telemetry HUD -->
              <div class="absolute bottom-4 left-4 right-4 z-20 flex items-center justify-between pointer-events-none">
                <div class="bg-slate-900/95 border border-slate-800 rounded-md px-3 py-1.5 shadow-lg flex items-center space-x-3 text-xs font-mono">
                  <span id="rust-coord-hud" class="text-cyan-400 font-semibold min-w-[165px]">X: 130.00 mm  Y: 95.00 mm</span>
                  <div class="w-px h-3 bg-slate-700"></div>
                  <div id="rust-hud-stats" class="text-[11px] text-slate-300">Initializing Rust GPU SDF Pipeline...</div>
                </div>
                <div id="rust-hover-inspector" class="bg-slate-900/95 border border-slate-800 rounded-md px-3 py-1.5 shadow-lg text-[11px] font-mono text-cyan-300">
                  O(1) Spatial Grid Ready — Hover or click any trace, via, or pad to probe net
                </div>
              </div>

              <!-- Primary Full-Screen Rust WebGL2/WGPU Instanced SDF Canvas -->
              <canvas id="rust-gpu-canvas" class="block w-full h-full cursor-grab active:cursor-grabbing"></canvas>

              <!-- Schematic Hierarchy Overlay -->
              <div id="schematic-view-panel" class="hidden absolute inset-0 bg-slate-950 flex flex-col p-6 overflow-auto z-30">
                <div class="flex items-center space-x-2 mb-4">
                  <span class="text-xs font-semibold uppercase tracking-wider text-slate-400">Schematic Sheet Hierarchy:</span>
                  <button data-sch="/samples/complex_hierarchy/complex_hierarchy.svg" class="sch-btn px-3 py-1 rounded bg-indigo-600 text-white text-xs">Root Sheet</button>
                  <button data-sch="/samples/complex_hierarchy/complex_hierarchy-ampli_ht_horizontal.svg" class="sch-btn px-3 py-1 rounded bg-slate-800 text-slate-300 text-xs">Ampli Horizontal</button>
                  <button data-sch="/samples/complex_hierarchy/complex_hierarchy-ampli_ht_vertical.svg" class="sch-btn px-3 py-1 rounded bg-slate-800 text-slate-300 text-xs">Ampli Vertical</button>
                </div>
                <div class="flex-1 bg-white rounded-xl p-4 overflow-auto flex items-center justify-center shadow-2xl">
                  <img id="schematic-svg-img" src="/samples/complex_hierarchy/complex_hierarchy.svg" class="max-w-full max-h-full object-contain" />
                </div>
              </div>

              <!-- DRC Audit Overlay -->
              <div id="drc-view-panel" class="hidden absolute inset-0 bg-slate-950 flex flex-col p-6 overflow-y-auto z-30">
                <div class="flex items-center justify-between mb-4">
                  <h2 class="text-sm font-bold text-slate-100">KiCad DRC Audit Report (Parsed in Rust)</h2>
                  <span class="text-xs font-mono text-amber-400">Click any violation to jump GPU camera to coordinate</span>
                </div>
                <div id="drc-list-container" class="space-y-2"></div>
              </div>

              <!-- Centralized Component Library & Supply Chain BOM Overlay -->
              <div id="parts-view-panel" class="hidden absolute inset-0 bg-slate-950 flex flex-col p-6 overflow-y-auto z-30">
                <div class="flex items-center justify-between mb-4">
                  <div>
                    <h2 class="text-sm font-bold text-slate-100">Enterprise Component Library &amp; Supply Chain Telemetry</h2>
                    <p class="text-xs text-slate-400 mt-0.5">Live MPN lifecycle verification &amp; cached DigiKey / Mouser stock index</p>
                  </div>
                  <input id="input-parts-search" type="text" placeholder="Filter MPN, manufacturer, footprint..." class="w-72 bg-slate-900 border border-slate-700 rounded-md px-3 py-1.5 text-xs text-slate-100" />
                </div>
                <div id="parts-list-container" class="space-y-2"></div>
              </div>
            </div>

            <!-- Right Sidebar: PCB Layer Stack + Review Pin Threads -->
            <aside class="w-80 bg-slate-900 border-l border-slate-800 flex flex-col z-20 shrink-0">
              <!-- Layer Stack Section -->
              <div class="p-3 border-b border-slate-800">
                <div class="text-xs font-semibold uppercase tracking-wider text-slate-300 mb-2">
                  GPU Shader Layer Uniforms
                </div>
                <div id="layer-controls-list" class="space-y-1.5 text-xs">
                  <label class="flex items-center justify-between p-1.5 rounded bg-slate-950/70 border border-slate-800 cursor-pointer">
                    <span class="flex items-center space-x-2">
                      <input type="checkbox" data-bit="1" checked class="layer-toggle accent-amber-500" />
                      <span class="w-2.5 h-2.5 rounded-full bg-amber-500"></span>
                      <span>F.Cu (Front Copper)</span>
                    </span>
                    <span class="font-mono text-[10px] text-slate-400">Bit 0</span>
                  </label>
                  <label class="flex items-center justify-between p-1.5 rounded bg-slate-950/70 border border-slate-800 cursor-pointer">
                    <span class="flex items-center space-x-2">
                      <input type="checkbox" data-bit="4096" checked class="layer-toggle accent-purple-500" />
                      <span class="w-2.5 h-2.5 rounded-full bg-purple-500"></span>
                      <span>In1..In4.Cu (Inner Cu)</span>
                    </span>
                    <span class="font-mono text-[10px] text-slate-400">Bit 12</span>
                  </label>
                  <label class="flex items-center justify-between p-1.5 rounded bg-slate-950/70 border border-slate-800 cursor-pointer">
                    <span class="flex items-center space-x-2">
                      <input type="checkbox" data-bit="2" checked class="layer-toggle accent-blue-500" />
                      <span class="w-2.5 h-2.5 rounded-full bg-blue-500"></span>
                      <span>B.Cu (Bottom Copper)</span>
                    </span>
                    <span class="font-mono text-[10px] text-slate-400">Bit 1</span>
                  </label>
                  <label class="flex items-center justify-between p-1.5 rounded bg-slate-950/70 border border-slate-800 cursor-pointer">
                    <span class="flex items-center space-x-2">
                      <input type="checkbox" data-bit="4" checked class="layer-toggle accent-slate-200" />
                      <span class="w-2.5 h-2.5 rounded-full bg-slate-200"></span>
                      <span>Edge.Cuts (Outline)</span>
                    </span>
                    <span class="font-mono text-[10px] text-slate-400">Bit 2</span>
                  </label>
                  <label class="flex items-center justify-between p-1.5 rounded bg-slate-950/70 border border-slate-800 cursor-pointer">
                    <span class="flex items-center space-x-2">
                      <input type="checkbox" data-bit="8" checked class="layer-toggle accent-slate-100" />
                      <span class="w-2.5 h-2.5 rounded-full bg-slate-100"></span>
                      <span>F.SilkS (Silkscreen)</span>
                    </span>
                    <span class="font-mono text-[10px] text-slate-400">Bit 3</span>
                  </label>
                  <label class="flex items-center justify-between p-1.5 rounded bg-slate-950/70 border border-slate-800 cursor-pointer">
                    <span class="flex items-center space-x-2">
                      <input type="checkbox" data-bit="32" class="layer-toggle accent-emerald-500" />
                      <span class="w-2.5 h-2.5 rounded-full bg-emerald-500"></span>
                      <span>F.Mask (Solder Mask)</span>
                    </span>
                    <span class="font-mono text-[10px] text-slate-400">Bit 5</span>
                  </label>
                  <label class="flex items-center justify-between p-1.5 rounded bg-slate-950/70 border border-slate-800 cursor-pointer">
                    <span class="flex items-center space-x-2">
                      <input type="checkbox" data-bit="128" checked class="layer-toggle accent-amber-400" />
                      <span class="w-2.5 h-2.5 rounded-full bg-amber-400"></span>
                      <span>DRC Violation Pins</span>
                    </span>
                    <span class="font-mono text-[10px] text-slate-400">Bit 7</span>
                  </label>
                  <label class="flex items-center justify-between p-1.5 rounded bg-slate-950/70 border border-slate-800 cursor-pointer">
                    <span class="flex items-center space-x-2">
                      <input type="checkbox" data-bit="256" checked class="layer-toggle accent-rose-500" />
                      <span class="w-2.5 h-2.5 rounded-full bg-rose-500"></span>
                      <span>Review Thread Pins</span>
                    </span>
                    <span class="font-mono text-[10px] text-slate-400">Bit 8</span>
                  </label>
                </div>
              </div>

              <!-- New Pin Form (Hidden until user clicks on board in Drop Pin Mode) -->
              <div id="new-pin-box" class="hidden p-3 bg-rose-500/10 border-b border-rose-500/30 space-y-2">
                <div class="flex items-center justify-between text-xs font-semibold text-rose-400">
                  <span id="new-pin-coord-label">New Pin</span>
                  <button id="btn-cancel-pin" class="text-slate-400 hover:text-white">✕</button>
                </div>
                <input id="input-pin-author" type="text" value="HardwareReviewer" class="w-full bg-slate-950 border border-slate-700 rounded px-2 py-1 text-xs text-slate-200" />
                <input id="input-pin-msg" type="text" placeholder="Enter design review comment..." class="w-full bg-slate-950 border border-slate-700 rounded px-2 py-1 text-xs text-slate-200" />
                <button id="btn-save-pin" class="w-full py-1 rounded bg-rose-600 hover:bg-rose-500 text-white text-xs font-semibold">
                  Create Pin Thread
                </button>
              </div>

              <!-- Review Threads List -->
              <div class="p-3 border-b border-slate-800 flex items-center justify-between">
                <span class="text-xs font-semibold uppercase tracking-wider text-slate-300">Redline Review Threads</span>
                <span id="threads-count-badge" class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-slate-800 text-slate-400">2</span>
              </div>
              <div id="threads-list-container" class="flex-1 overflow-y-auto p-2 space-y-2"></div>
            </aside>
          </div>
        </div>
        "#,
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
                        btn.set_text_content(Some("📍 Drop Review Pin"));
                    }
                    if let Some(lbl) = app.doc.get_element_by_id("new-pin-coord-label") {
                        lbl.set_text_content(Some(&format!("New Pin at ({:.1}mm, {:.1}mm)", mm_x, mm_y)));
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
                    "X: {:.2} mm  Y: {:.2} mm",
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
                    insp.set_text_content(Some(&format!("🔍 {}", info.summary)));
                } else {
                    insp.set_text_content(Some(
                        "O(1) Spatial Grid Ready — Hover or click any trace, via, or pad to probe net",
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
                    // User clicked on the PCB canvas: probe the clicked trace/via/pad's electrical net!
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
                el.set_text_content(Some(if is_3d {
                    "🧊 3D Stackup: ON (Shift+Drag)"
                } else {
                    "🧊 3D Stackup Orbit"
                }));
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
        }) as Box<dyn FnMut(_)>);
        let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    // Drop Pin Toggle
    if let Some(btn) = doc.get_element_by_id("btn-drop-pin") {
        let st = state.clone();
        let cb = Closure::wrap(Box::new(move |e: Event| {
            let mut app = st.borrow_mut();
            app.drop_pin_mode = !app.drop_pin_mode;
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
                el.set_text_content(Some(if app.drop_pin_mode {
                    "🎯 Click Board..."
                } else {
                    "📍 Drop Review Pin"
                }));
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

                        // Fire-and-forget POST /api/threads to persist in SQLite
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

    // View Mode Tabs ("tab-pcb", "tab-diff", "tab-schematic", "tab-drc", "tab-parts")
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

    // Diff Mode Controls (Boolean Shader vs Split Wipe Slider)
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
                btn.set_class_name("px-3 py-1 rounded text-xs font-medium bg-indigo-600 text-white");
            } else {
                btn.set_class_name(
                    "px-3 py-1 rounded text-xs font-medium text-slate-400 hover:text-slate-200",
                );
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

        let status_cls = match p.lifecycle_status.as_str() {
            "active" => "text-emerald-400 bg-emerald-500/10 border-emerald-500/30",
            "nrnd" => "text-amber-400 bg-amber-500/10 border-amber-500/30",
            _ => "text-rose-400 bg-rose-500/10 border-rose-500/30",
        };

        html.push_str(&format!(
            r#"<div class="p-3 rounded-lg border border-slate-800 bg-slate-900/70 flex items-center justify-between">
                 <div class="space-y-1">
                   <div class="flex items-center space-x-2">
                     <span class="text-xs font-mono font-bold text-cyan-300">{mpn}</span>
                     <span class="text-xs text-slate-400">&bull; {mfr}</span>
                     <span class="text-[10px] font-mono uppercase px-2 py-0.5 rounded border {status_cls}">{status}</span>
                   </div>
                   <div class="text-xs text-slate-200">{desc}</div>
                   <div class="text-[11px] font-mono text-slate-400">Footprint: {fp} &bull; Symbol: {sym}</div>
                 </div>
                 <div class="text-right font-mono text-xs space-y-1">
                   <div class="text-slate-100 font-bold">${price:.3} / unit</div>
                   <div class="text-[11px] text-emerald-400">DigiKey: {dk} &bull; Mouser: {ms}</div>
                 </div>
               </div>"#,
            mpn = p.mpn,
            mfr = p.manufacturer,
            status_cls = status_cls,
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
        let dot_color = if t.resolved { "bg-emerald-500" } else { "bg-rose-500" };
        let resolve_label = if t.resolved { "✓ Resolved" } else { "Resolve" };
        let body = t
            .messages
            .first()
            .map(|m| m.body.as_str())
            .unwrap_or("Review pin");
        html.push_str(&format!(
            r#"<div data-idx="{idx}" class="thread-card p-2.5 rounded-lg border border-slate-800 bg-slate-950/70 hover:border-indigo-500 cursor-pointer transition-all">
                 <div class="flex items-center justify-between mb-1">
                   <span class="flex items-center space-x-1.5">
                     <span class="w-2 h-2 rounded-full {dot_color}"></span>
                     <span class="text-xs font-semibold text-slate-200">{author}</span>
                   </span>
                   <div class="flex items-center space-x-1">
                     <span class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-slate-900 text-cyan-400 border border-slate-800">
                       {x:.1}mm, {y:.1}mm
                     </span>
                     <button data-resolve-idx="{idx}" class="px-1.5 py-0.5 rounded bg-slate-800 hover:bg-emerald-600 text-[10px] text-slate-300 hover:text-white">{resolve_label}</button>
                   </div>
                 </div>
                 <p class="text-xs text-slate-300 leading-relaxed">{body}</p>
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
            let node = val
                .get("node_id")
                .and_then(|v| v.as_str())
                .unwrap_or("cadence-node-primary");
            let workers = val
                .get("worker_pool")
                .and_then(|w| w.get("max_workers"))
                .and_then(|v| v.as_u64())
                .unwrap_or(4);
            let app = st_metrics.borrow();
            if let Some(badge) = app.doc.get_element_by_id("cluster-status-badge") {
                badge.set_text_content(Some(&format!(
                    "{} ({} Workers • CAS Online)",
                    node, workers
                )));
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
                    let badge_cls = if d.severity == "error" {
                        "text-red-400 bg-red-500/10 border-red-500/30"
                    } else {
                        "text-amber-400 bg-amber-500/10 border-amber-500/30"
                    };
                    html.push_str(&format!(
                        r#"<div data-drc-idx="{i}" class="drc-row p-3 rounded-lg border border-slate-800 bg-slate-900/70 hover:border-indigo-500 flex items-center justify-between cursor-pointer">
                             <div>
                               <div class="text-xs font-semibold text-slate-200">{desc}</div>
                               <div class="text-[11px] font-mono text-slate-400 mt-0.5">({x:.2} mm, {y:.2} mm) &bull; {tname}</div>
                             </div>
                             <span class="text-[10px] font-mono uppercase px-2 py-0.5 rounded border {badge_cls}">{sev}</span>
                           </div>"#,
                        i = i,
                        desc = d.description,
                        x = d.x,
                        y = d.y,
                        tname = d.type_name,
                        badge_cls = badge_cls,
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
