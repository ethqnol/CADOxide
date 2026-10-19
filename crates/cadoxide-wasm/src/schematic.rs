//! Schematic Capture, Hierarchical Design & Multi-Sheet Navigation Engine
//! Pure Rust implementation executing inside WebAssembly.
//! Supports KiCad (.kicad_sch) and Altium (.SchDoc) migration, interactive rendering,
//! sub-sheet drilling, and bidirectional cross-probing with PCB layout.

use crate::altium_parser::{altium_coord_to_mm, extract_altium_text_records, CfbReader};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SchLabelType {
    Local,
    Global,
    Hierarchical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchPin {
    pub number: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub electrical_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchSymbol {
    pub id: String,
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub x: f32,
    pub y: f32,
    pub rot_deg: f32,
    pub pins: Vec<SchPin>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchWire {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub net_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchJunction {
    pub x: f32,
    pub y: f32,
    pub diameter: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchLabel {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub rot_deg: f32,
    pub label_type: SchLabelType,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchSubSheet {
    pub name: String,
    pub filename: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub pins: Vec<(String, String)>, // (name, electrical_type)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SchematicSheet {
    pub id: String,
    pub name: String,
    pub filename: String,
    pub symbols: Vec<SchSymbol>,
    pub wires: Vec<SchWire>,
    pub junctions: Vec<SchJunction>,
    pub labels: Vec<SchLabel>,
    pub sub_sheets: Vec<SchSubSheet>,
    pub bounds: [f32; 4], // [min_x, min_y, max_x, max_y]
}

impl SchematicSheet {
    pub fn new(id: &str, name: &str, filename: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            filename: filename.to_string(),
            symbols: Vec::new(),
            wires: Vec::new(),
            junctions: Vec::new(),
            labels: Vec::new(),
            sub_sheets: Vec::new(),
            bounds: [0.0, 0.0, 297.0, 210.0],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchematicDocument {
    pub root_sheet_id: String,
    pub active_sheet_id: String,
    pub hierarchy_path: Vec<String>, // Breadcrumbs of sheet IDs: e.g. ["root", "ampli_ht_horizontal"]
    pub sheets: HashMap<String, SchematicSheet>,
}

impl Default for SchematicDocument {
    fn default() -> Self {
        let mut sheets = HashMap::new();
        let root = SchematicSheet {
            id: "root".to_string(),
            name: "Root Sheet".to_string(),
            filename: "root.kicad_sch".to_string(),
            symbols: Vec::new(),
            wires: Vec::new(),
            junctions: Vec::new(),
            labels: Vec::new(),
            sub_sheets: Vec::new(),
            bounds: [0.0, 0.0, 297.0, 210.0], // A4 Landscape (mm)
        };
        sheets.insert("root".to_string(), root);

        Self {
            root_sheet_id: "root".to_string(),
            active_sheet_id: "root".to_string(),
            hierarchy_path: vec!["root".to_string()],
            sheets,
        }
    }
}

impl SchematicDocument {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_sheet(&self) -> Option<&SchematicSheet> {
        self.sheets.get(&self.active_sheet_id)
    }

    pub fn active_sheet_mut(&mut self) -> Option<&mut SchematicSheet> {
        self.sheets.get_mut(&self.active_sheet_id)
    }

    pub fn navigate_to_sheet(&mut self, sheet_id: &str) -> bool {
        if self.sheets.contains_key(sheet_id) {
            self.active_sheet_id = sheet_id.to_string();
            // Rebuild hierarchy path
            if sheet_id == self.root_sheet_id {
                self.hierarchy_path = vec![self.root_sheet_id.clone()];
            } else if !self.hierarchy_path.contains(&sheet_id.to_string()) {
                self.hierarchy_path.push(sheet_id.to_string());
            } else {
                // Truncate path to selected sheet
                if let Some(pos) = self.hierarchy_path.iter().position(|id| id == sheet_id) {
                    self.hierarchy_path.truncate(pos + 1);
                }
            }
            true
        } else {
            false
        }
    }

    pub fn navigate_up(&mut self) -> bool {
        if self.hierarchy_path.len() > 1 {
            self.hierarchy_path.pop();
            if let Some(parent_id) = self.hierarchy_path.last().cloned() {
                self.active_sheet_id = parent_id;
                return true;
            }
        }
        false
    }

    pub fn add_sheet(&mut self, sheet: SchematicSheet) {
        self.sheets.insert(sheet.id.clone(), sheet);
    }

    pub fn get_breadcrumb_path(&self) -> Vec<(String, String)> {
        self.hierarchy_path
            .iter()
            .map(|id| {
                let name = self.sheets.get(id).map(|s| s.name.clone()).unwrap_or_else(|| id.clone());
                (id.clone(), name)
            })
            .collect()
    }

    pub fn find_symbol_across_all_sheets(&self, reference: &str) -> Option<(String, &SchSymbol)> {
        for (sheet_id, sheet) in &self.sheets {
            if let Some(sym) = sheet.symbols.iter().find(|s| s.reference == reference) {
                return Some((sheet_id.clone(), sym));
            }
        }
        None
    }

    // ========================================================================
    // Interactive Editing Operations
    // ========================================================================

    pub fn add_wire(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, net_name: Option<String>) {
        if let Some(sheet) = self.active_sheet_mut() {
            sheet.wires.push(SchWire { x1, y1, x2, y2, net_name });
        }
    }

    pub fn add_junction(&mut self, x: f32, y: f32) {
        if let Some(sheet) = self.active_sheet_mut() {
            sheet.junctions.push(SchJunction { x, y, diameter: 1.0 });
        }
    }

    pub fn add_label(&mut self, text: String, x: f32, y: f32, label_type: SchLabelType) {
        if let Some(sheet) = self.active_sheet_mut() {
            sheet.labels.push(SchLabel {
                text,
                x,
                y,
                rot_deg: 0.0,
                label_type,
            });
        }
    }

    pub fn add_symbol(&mut self, reference: String, value: String, footprint: String, x: f32, y: f32) {
        let sym_id = format!("sym-{}", reference);
        if let Some(sheet) = self.active_sheet_mut() {
            sheet.symbols.push(SchSymbol {
                id: sym_id,
                reference,
                value,
                footprint,
                x,
                y,
                rot_deg: 0.0,
                pins: Vec::new(),
            });
        }
    }

    // ========================================================================
    // KiCad .kicad_sch S-Expression Parser
    // ========================================================================

    pub fn parse_kicad_schematic_str(&mut self, content: &str, sheet_id: &str, sheet_name: &str) {
        let mut sheet = SchematicSheet {
            id: sheet_id.to_string(),
            name: sheet_name.to_string(),
            filename: format!("{}.kicad_sch", sheet_id),
            symbols: Vec::new(),
            wires: Vec::new(),
            junctions: Vec::new(),
            labels: Vec::new(),
            sub_sheets: Vec::new(),
            bounds: [0.0, 0.0, 297.0, 210.0],
        };

        let mut lines = content.lines().map(|l| l.trim());
        let mut current_symbol: Option<SchSymbol> = None;
        let mut in_symbol = false;

        while let Some(line) = lines.next() {
            if line.starts_with("(symbol") {
                in_symbol = true;
                current_symbol = Some(SchSymbol {
                    id: format!("sym-{}", sheet.symbols.len() + 1),
                    reference: String::new(),
                    value: String::new(),
                    footprint: String::new(),
                    x: 0.0,
                    y: 0.0,
                    rot_deg: 0.0,
                    pins: Vec::new(),
                });
            } else if in_symbol {
                if line.starts_with("(at ") {
                    if let Some((x, y, rot)) = parse_sch_at(line) {
                        if let Some(ref mut sym) = current_symbol {
                            sym.x = x;
                            sym.y = y;
                            sym.rot_deg = rot;
                        }
                    }
                } else if line.starts_with("(property \"Reference\"") {
                    if let Some(val) = extract_prop_val(line) {
                        if let Some(ref mut sym) = current_symbol {
                            sym.reference = val;
                        }
                    }
                } else if line.starts_with("(property \"Value\"") {
                    if let Some(val) = extract_prop_val(line) {
                        if let Some(ref mut sym) = current_symbol {
                            sym.value = val;
                        }
                    }
                } else if line.starts_with("(property \"Footprint\"") {
                    if let Some(val) = extract_prop_val(line) {
                        if let Some(ref mut sym) = current_symbol {
                            sym.footprint = val;
                        }
                    }
                } else if line.starts_with("(pin ") {
                    let mut pin_num = String::new();
                    let pin_name = String::new();
                    let mut px = 0.0;
                    let mut py = 0.0;
                    if let Some(first_quote) = line.find('"') {
                        if let Some(second_quote) = line[first_quote + 1..].find('"') {
                            pin_num = line[first_quote + 1..first_quote + 1 + second_quote].to_string();
                        }
                    }
                    if let Some(at_idx) = line.find("(at ") {
                        if let Some((x, y, _)) = parse_sch_at(&line[at_idx..]) {
                            px = x;
                            py = y;
                        }
                    }
                    if let Some(ref mut sym) = current_symbol {
                        sym.pins.push(SchPin {
                            number: pin_num,
                            name: pin_name,
                            x: px,
                            y: py,
                            electrical_type: "passive".to_string(),
                        });
                    }
                } else if line == ")" && in_symbol {
                    if let Some(sym) = current_symbol.take() {
                        if !sym.reference.is_empty() {
                            sheet.symbols.push(sym);
                        }
                    }
                    in_symbol = false;
                }
            } else if line.starts_with("(wire") {
                // (wire (pts (xy 100 50) (xy 120 50)))
                if let Some((x1, y1, x2, y2)) = extract_wire_pts(line) {
                    sheet.wires.push(SchWire { x1, y1, x2, y2, net_name: None });
                }
            } else if line.starts_with("(junction") {
                if let Some((x, y)) = extract_xy(line) {
                    sheet.junctions.push(SchJunction { x, y, diameter: 1.0 });
                }
            } else if line.starts_with("(label ") {
                if let Some((txt, x, y, rot)) = extract_label_info(line) {
                    sheet.labels.push(SchLabel {
                        text: txt,
                        x,
                        y,
                        rot_deg: rot,
                        label_type: SchLabelType::Local,
                    });
                }
            } else if line.starts_with("(global_label ") {
                if let Some((txt, x, y, rot)) = extract_label_info(line) {
                    sheet.labels.push(SchLabel {
                        text: txt,
                        x,
                        y,
                        rot_deg: rot,
                        label_type: SchLabelType::Global,
                    });
                }
            } else if line.starts_with("(hierarchical_label ") {
                if let Some((txt, x, y, rot)) = extract_label_info(line) {
                    sheet.labels.push(SchLabel {
                        text: txt,
                        x,
                        y,
                        rot_deg: rot,
                        label_type: SchLabelType::Hierarchical,
                    });
                }
            } else if line.starts_with("(sheet") {
                // Hierarchical sub-sheet box
                let mut sub_name = "SubSheet".to_string();
                let mut sub_file = "sub.kicad_sch".to_string();
                let mut sx = 50.0;
                let mut sy = 50.0;
                let mut sw = 60.0;
                let mut sh = 40.0;

                // Read sheet block
                while let Some(sline) = lines.next() {
                    if sline.starts_with("(at ") {
                        if let Some((x, y, _)) = parse_sch_at(sline) {
                            sx = x;
                            sy = y;
                        }
                    } else if sline.starts_with("(size ") {
                        if let Some((w, h)) = extract_xy(sline) {
                            sw = w;
                            sh = h;
                        }
                    } else if sline.starts_with("(property \"Sheetname\"") || sline.starts_with("(property \"Sheet name\"") {
                        if let Some(v) = extract_prop_val(sline) {
                            sub_name = v;
                        }
                    } else if sline.starts_with("(property \"Sheetfile\"") || sline.starts_with("(property \"Sheet file\"") {
                        if let Some(v) = extract_prop_val(sline) {
                            sub_file = v;
                        }
                    } else if sline == ")" {
                        break;
                    }
                }

                sheet.sub_sheets.push(SchSubSheet {
                    name: sub_name,
                    filename: sub_file,
                    x: sx,
                    y: sy,
                    width: sw,
                    height: sh,
                    pins: Vec::new(),
                });
            }
        }

        self.sheets.insert(sheet_id.to_string(), sheet);
    }

    // ========================================================================
    // Altium .SchDoc Ingestion Pipeline
    // ========================================================================

    pub fn parse_altium_schdoc_bytes(&mut self, data: &[u8], sheet_id: &str, sheet_name: &str) -> Result<(), &'static str> {
        let cfb = CfbReader::new(data)?;
        let mut sheet = SchematicSheet {
            id: sheet_id.to_string(),
            name: sheet_name.to_string(),
            filename: format!("{}.SchDoc", sheet_id),
            symbols: Vec::new(),
            wires: Vec::new(),
            junctions: Vec::new(),
            labels: Vec::new(),
            sub_sheets: Vec::new(),
            bounds: [0.0, 0.0, 297.0, 210.0],
        };

        // Altium .SchDoc stores schematic primitives in the "FileHeader" stream
        if let Some(stream_data) = cfb.get_stream("FileHeader") {
            let records = extract_altium_text_records(&stream_data);
            for rec in records {
                let record_type = rec.get("RECORD").and_then(|r| r.parse::<i32>().ok()).unwrap_or(0);
                match record_type {
                    1 => {
                        // Component / Symbol
                        let designator = rec.get("DESIGNATOR")
                            .or_else(|| rec.get("SOURCEDESIGNATOR"))
                            .or_else(|| rec.get("LIBREFERENCE"))
                            .cloned()
                            .unwrap_or_else(|| "U?".to_string());
                        let comment = rec.get("COMMENT")
                            .or_else(|| rec.get("COMPONENTDESCRIPTION"))
                            .cloned()
                            .unwrap_or_default();
                        let footprint = rec.get("FOOTPRINT").cloned().unwrap_or_default();
                        let x = altium_coord_to_mm(rec.get("LOCATION.X"));
                        let y = altium_coord_to_mm(rec.get("LOCATION.Y"));
                        let rot = rec.get("ROTATION").and_then(|r| r.parse::<f32>().ok()).unwrap_or(0.0);

                        sheet.symbols.push(SchSymbol {
                            id: format!("sym-{}", designator),
                            reference: designator,
                            value: comment,
                            footprint,
                            x,
                            y,
                            rot_deg: rot,
                            pins: Vec::new(),
                        });
                    }
                    2 => {
                        // Pin
                        if let Some(sym) = sheet.symbols.last_mut() {
                            let pin_num = rec.get("NAME").or_else(|| rec.get("DESIGNATOR")).cloned().unwrap_or_default();
                            let pin_name = rec.get("PINNAME").or_else(|| rec.get("NAME")).cloned().unwrap_or_default();
                            let x = altium_coord_to_mm(rec.get("LOCATION.X"));
                            let y = altium_coord_to_mm(rec.get("LOCATION.Y"));
                            sym.pins.push(SchPin {
                                number: pin_num,
                                name: pin_name,
                                x,
                                y,
                                electrical_type: "Passive".to_string(),
                            });
                        }
                    }
                    4 => {
                        // Net Label
                        let text = rec.get("TEXT").cloned().unwrap_or_default();
                        let x = altium_coord_to_mm(rec.get("LOCATION.X"));
                        let y = altium_coord_to_mm(rec.get("LOCATION.Y"));
                        let rot = rec.get("ROTATION").and_then(|r| r.parse::<f32>().ok()).unwrap_or(0.0);
                        sheet.labels.push(SchLabel {
                            text,
                            x,
                            y,
                            rot_deg: rot,
                            label_type: SchLabelType::Local,
                        });
                    }
                    5 => {
                        // Wire Segment
                        let x1 = altium_coord_to_mm(rec.get("START.X").or_else(|| rec.get("X1")));
                        let y1 = altium_coord_to_mm(rec.get("START.Y").or_else(|| rec.get("Y1")));
                        let x2 = altium_coord_to_mm(rec.get("END.X").or_else(|| rec.get("X2")));
                        let y2 = altium_coord_to_mm(rec.get("END.Y").or_else(|| rec.get("Y2")));
                        sheet.wires.push(SchWire { x1, y1, x2, y2, net_name: None });
                    }
                    8 => {
                        // Junction
                        let x = altium_coord_to_mm(rec.get("LOCATION.X"));
                        let y = altium_coord_to_mm(rec.get("LOCATION.Y"));
                        sheet.junctions.push(SchJunction { x, y, diameter: 1.0 });
                    }
                    10 => {
                        // Hierarchical Sheet Symbol
                        let sheet_name = rec.get("NAME").cloned().unwrap_or_else(|| "SubSheet".to_string());
                        let sheet_file = rec.get("FILENAME").cloned().unwrap_or_else(|| "sub.SchDoc".to_string());
                        let x = altium_coord_to_mm(rec.get("LOCATION.X"));
                        let y = altium_coord_to_mm(rec.get("LOCATION.Y"));
                        let w = altium_coord_to_mm(rec.get("XSIZE")).max(40.0);
                        let h = altium_coord_to_mm(rec.get("YSIZE")).max(30.0);

                        sheet.sub_sheets.push(SchSubSheet {
                            name: sheet_name,
                            filename: sheet_file,
                            x,
                            y,
                            width: w,
                            height: h,
                            pins: Vec::new(),
                        });
                    }
                    _ => {}
                }
            }
        }

        self.sheets.insert(sheet_id.to_string(), sheet);
        Ok(())
    }

    // ========================================================================
    // Interactive SVG Generator for Active Sheet
    // ========================================================================

    pub fn render_active_sheet_svg(&self) -> String {
        let sheet = match self.active_sheet() {
            Some(s) => s,
            None => return "<svg viewBox=\"0 0 100 100\"><text x=\"10\" y=\"50\" fill=\"#888\">No Sheet Loaded</text></svg>".to_string(),
        };

        let mut svg = String::with_capacity(16384);
        svg.push_str(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 297 210" style="width:100%;height:100%;background:#13161c;cursor:crosshair;user-select:none;">"#);

        // Drawing Definitions: Markers & Grid pattern
        svg.push_str(r##"<defs>
          <pattern id="sch-grid" width="2.54" height="2.54" patternUnits="userSpaceOnUse">
            <circle cx="1.27" cy="1.27" r="0.15" fill="#2a303d" />
          </pattern>
          <marker id="sch-arrow" viewBox="0 0 10 10" refX="5" refY="5" markerWidth="4" markerHeight="4" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" fill="#60a5fa"/>
          </marker>
        </defs>"##);

        // Background & Grid
        svg.push_str(r##"<rect width="297" height="210" fill="#13161c" />"##);
        svg.push_str(r##"<rect width="297" height="210" fill="url(#sch-grid)" />"##);

        // Outer Sheet Border & Title Block
        svg.push_str(r##"<rect x="5" y="5" width="287" height="200" fill="none" stroke="#2a303d" stroke-width="0.5" />"##);
        svg.push_str(&format!(
            r##"<g id="title-block">
                <rect x="210" y="175" width="82" height="30" fill="#181b22" stroke="#2a303d" stroke-width="0.4" />
                <text x="213" y="183" font-family="sans-serif" font-size="3" font-weight="bold" fill="#60a5fa">{sheet_title}</text>
                <text x="213" y="190" font-family="monospace" font-size="2.2" fill="#94a3b8">File: {filename}</text>
                <text x="213" y="196" font-family="monospace" font-size="2.2" fill="#64748b">CADoxide High-Speed EDA</text>
              </g>"##,
            sheet_title = sheet.name,
            filename = sheet.filename
        ));

        // 1. Render Wires (Green)
        svg.push_str("<g id=\"sch-wires\" stroke=\"#4ade80\" stroke-width=\"0.35\" stroke-linecap=\"round\">");
        for w in &sheet.wires {
            svg.push_str(&format!(
                r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" />"##,
                w.x1, w.y1, w.x2, w.y2
            ));
        }
        svg.push_str("</g>");

        // 2. Render Junctions (Green dots)
        svg.push_str("<g id=\"sch-junctions\" fill=\"#4ade80\">");
        for j in &sheet.junctions {
            svg.push_str(&format!(
                r##"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" />"##,
                j.x, j.y, j.diameter * 0.5
            ));
        }
        svg.push_str("</g>");

        // 3. Render Net Labels
        svg.push_str("<g id=\"sch-labels\">");
        for lbl in &sheet.labels {
            let (fill, bg) = match lbl.label_type {
                SchLabelType::Global => ("#f59e0b", "#451a03"),
                SchLabelType::Hierarchical => ("#38bdf8", "#082f49"),
                SchLabelType::Local => ("#e2e8f0", "#1e293b"),
            };
            svg.push_str(&format!(
                r##"<g transform="translate({:.2},{:.2}) rotate({:.1})">
                    <rect x="-0.5" y="-2" width="{:.2}" height="3" rx="0.5" fill="{bg}" stroke="{fill}" stroke-width="0.2" />
                    <text x="1" y="0" font-family="monospace" font-size="2" font-weight="600" fill="{fill}">{text}</text>
                  </g>"##,
                lbl.x, lbl.y, lbl.rot_deg,
                (lbl.text.len() as f32) * 1.5 + 2.0,
                bg = bg, fill = fill, text = lbl.text
            ));
        }
        svg.push_str("</g>");

        // 4. Render Hierarchical Sub-Sheets (Clickable Drills)
        svg.push_str("<g id=\"sch-subsheets\">");
        for sub in &sheet.sub_sheets {
            svg.push_str(&format!(
                r##"<g class="sch-subsheet-box" data-sheet-file="{file}" data-sheet-name="{name}" style="cursor:pointer;">
                    <rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" fill="#162031" stroke="#38bdf8" stroke-width="0.5" rx="1.5" />
                    <text x="{tx:.2}" y="{ty:.2}" font-family="sans-serif" font-size="2.6" font-weight="bold" fill="#7dd3fc">{name}</text>
                    <text x="{tx:.2}" y="{ty2:.2}" font-family="monospace" font-size="2.0" fill="#94a3b8">File: {file}</text>
                    <text x="{bx:.2}" y="{by:.2}" font-family="sans-serif" font-size="1.8" fill="#38bdf8">[Double-click to drill sheet]</text>
                  </g>"##,
                x = sub.x, y = sub.y, w = sub.width, h = sub.height,
                tx = sub.x + 2.0, ty = sub.y + 5.0, ty2 = sub.y + 8.5,
                bx = sub.x + 2.0, by = sub.y + sub.height - 2.5,
                name = sub.name, file = sub.filename
            ));
        }
        svg.push_str("</g>");

        // 5. Render Symbols & Component Boxes
        svg.push_str("<g id=\"sch-symbols\">");
        for sym in &sheet.symbols {
            let w = 16.0f32;
            let h = 12.0f32;
            svg.push_str(&format!(
                r##"<g class="sch-symbol-box" data-sym-ref="{ref_des}" data-sym-val="{val}" data-sym-fp="{fp}" transform="translate({x:.2},{y:.2}) rotate({rot:.1})" style="cursor:pointer;">
                    <rect x="{ox:.2}" y="{oy:.2}" width="{w:.2}" height="{h:.2}" fill="#1e2430" stroke="#f87171" stroke-width="0.4" rx="1" />
                    <text x="{ox:.2}" y="{ry:.2}" font-family="monospace" font-size="2.5" font-weight="bold" fill="#f87171">{ref_des}</text>
                    <text x="{ox:.2}" y="{vy:.2}" font-family="sans-serif" font-size="2.0" fill="#cbd5e1">{val}</text>
                  </g>"##,
                ref_des = sym.reference, val = sym.value, fp = sym.footprint,
                x = sym.x, y = sym.y, rot = sym.rot_deg,
                ox = -w * 0.5, oy = -h * 0.5, w = w, h = h,
                ry = -h * 0.5 - 1.5, vy = h * 0.5 + 3.0
            ));
        }
        svg.push_str("</g>");

        svg.push_str("</svg>");
        svg
    }
}

// Helpers for KiCad S-Expression Parsing
fn extract_prop_val(line: &str) -> Option<String> {
    let parts: Vec<&str> = line.split('"').collect();
    if parts.len() >= 4 {
        Some(parts[3].to_string())
    } else {
        None
    }
}

fn parse_sch_at(line: &str) -> Option<(f32, f32, f32)> {
    let stripped = line.trim_start_matches("(at").trim_end_matches(')').trim();
    let parts: Vec<&str> = stripped.split_whitespace().collect();
    if parts.len() >= 2 {
        let x = parts[0].parse::<f32>().ok()?;
        let y = parts[1].parse::<f32>().ok()?;
        let rot = if parts.len() >= 3 {
            parts[2].parse::<f32>().unwrap_or(0.0)
        } else {
            0.0
        };
        Some((x, y, rot))
    } else {
        None
    }
}

fn extract_wire_pts(line: &str) -> Option<(f32, f32, f32, f32)> {
    let pts_idx = line.find("(pts")?;
    let rest = &line[pts_idx..];
    let mut coords = Vec::new();
    for token in rest.split("(xy") {
        if let Some(end) = token.find(')') {
            let inside = token[..end].trim();
            let mut nums = inside.split_whitespace();
            if let (Some(x), Some(y)) = (nums.next().and_then(|v| v.parse::<f32>().ok()), nums.next().and_then(|v| v.parse::<f32>().ok())) {
                coords.push((x, y));
            }
        }
    }
    if coords.len() >= 2 {
        Some((coords[0].0, coords[0].1, coords[1].0, coords[1].1))
    } else {
        None
    }
}

fn extract_xy(line: &str) -> Option<(f32, f32)> {
    let idx = line.find("(at ")?;
    let rest = &line[idx + 4..];
    let end = rest.find(')')?;
    let mut parts = rest[..end].trim().split_whitespace();
    let x = parts.next()?.parse::<f32>().ok()?;
    let y = parts.next()?.parse::<f32>().ok()?;
    Some((x, y))
}

fn extract_label_info(line: &str) -> Option<(String, f32, f32, f32)> {
    let first_quote = line.find('"')?;
    let second_quote = line[first_quote + 1..].find('"')? + first_quote + 1;
    let text = line[first_quote + 1..second_quote].to_string();

    let (x, y, rot) = if let Some(at_idx) = line.find("(at ") {
        parse_sch_at(&line[at_idx..])?
    } else {
        (0.0, 0.0, 0.0)
    };

    Some((text, x, y, rot))
}

pub fn parse_kicad_sch_str(content: &str, sheet_id: &str, sheet_name: &str) -> SchematicDocument {
    let mut doc = SchematicDocument::new();
    doc.parse_kicad_schematic_str(content, sheet_id, sheet_name);
    doc
}

pub fn parse_altium_schdoc_bytes(data: &[u8], sheet_id: &str, sheet_name: &str) -> Result<SchematicDocument, &'static str> {
    let mut doc = SchematicDocument::new();
    doc.parse_altium_schdoc_bytes(data, sheet_id, sheet_name)?;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kicad_schematic_parsing_and_svg_render() {
        let sch_content = r#"(kicad_sch (version 20211123) (generator eeschema)
  (uuid "root-uuid")
  (wire (pts (xy 50.0 60.0) (xy 80.0 60.0)))
  (junction (at 50.0 60.0) (diameter 1.0))
  (label "VCC" (at 50.0 60.0 0))
  (symbol (lib_id "Device:R") (at 80.0 60.0 0) (unit 1)
    (property "Reference" "R1" (id 0) (at 80.0 55.0 0))
    (property "Value" "10k" (id 1) (at 80.0 65.0 0))
    (property "Footprint" "Resistor_SMD:R_0603_1608Metric" (id 2) (at 80.0 70.0 0))
    (pin "1" (uuid "pin-1") (at 75.0 60.0 0))
    (pin "2" (uuid "pin-2") (at 85.0 60.0 0))
  )
  (sheet (at 100.0 100.0) (size 50.0 30.0)
    (property "Sheet name" "PowerSupply" (id 0) (at 100.0 95.0 0))
    (property "Sheet file" "power.kicad_sch" (id 1) (at 100.0 135.0 0))
  )
)"#;

        let doc = parse_kicad_sch_str(sch_content, "root", "Root Schematic");
        assert_eq!(doc.sheets.len(), 1); // Root sheet containing sub_sheets

        let root = doc.sheets.get("root").expect("Root sheet exists");
        assert_eq!(root.wires.len(), 1);
        assert_eq!(root.junctions.len(), 1);
        assert_eq!(root.labels.len(), 1);
        assert_eq!(root.symbols.len(), 1);
        assert_eq!(root.symbols[0].reference, "R1");
        assert_eq!(root.symbols[0].value, "10k");
        assert_eq!(root.symbols[0].pins.len(), 2);
        assert_eq!(root.sub_sheets.len(), 1);

        // SVG rendering check
        let svg = doc.render_active_sheet_svg();
        assert!(svg.contains("R1"));
        assert!(svg.contains("10k"));
        assert!(svg.contains("VCC"));
        assert!(svg.contains("PowerSupply"));
    }

    #[test]
    fn test_schematic_hierarchy_and_breadcrumbs() {
        let mut doc = SchematicDocument::new();
        let mut child_sheet = SchematicSheet::new("power_sub", "Power Subsystem", "power.kicad_sch");
        child_sheet.symbols.push(SchSymbol {
            id: "u1".to_string(),
            reference: "U1".to_string(),
            value: "LM7805".to_string(),
            footprint: "Package_TO_SOT_THT:TO-220-3_Vertical".to_string(),
            x: 50.0,
            y: 50.0,
            rot_deg: 0.0,
            pins: Vec::new(),
        });
        doc.add_sheet(child_sheet);

        let root = doc.sheets.get_mut("root").unwrap();
        root.sub_sheets.push(SchSubSheet {
            name: "Power Subsystem".to_string(),
            filename: "power.kicad_sch".to_string(),
            x: 100.0,
            y: 100.0,
            width: 50.0,
            height: 30.0,
            pins: Vec::new(),
        });

        // Initially at root
        assert_eq!(doc.active_sheet_id, "root");
        let breadcrumbs = doc.get_breadcrumb_path();
        assert_eq!(breadcrumbs.len(), 1);
        assert_eq!(breadcrumbs[0].1, "Root Sheet");

        // Navigate down to child sheet
        assert!(doc.navigate_to_sheet("power_sub"));
        assert_eq!(doc.active_sheet_id, "power_sub");
        let breadcrumbs2 = doc.get_breadcrumb_path();
        assert_eq!(breadcrumbs2.len(), 2);
        assert_eq!(breadcrumbs2[1].1, "Power Subsystem");

        // Cross-probing query finds symbol in sub-sheet
        let found = doc.find_symbol_across_all_sheets("U1");
        assert!(found.is_some());
        let (sheet_id, sym) = found.unwrap();
        assert_eq!(sheet_id, "power_sub");
        assert_eq!(sym.reference, "U1");

        // Navigate back up to root
        assert!(doc.navigate_up());
        assert_eq!(doc.active_sheet_id, "root");
    }
}

