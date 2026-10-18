use crate::parser::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct FootprintPadTemplate {
    pub rel_x: f32,
    pub rel_y: f32,
    pub width: f32,
    pub height: f32,
    pub drill: f32,
    pub is_tht: bool,
    pub pad_num: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct FootprintLineTemplate {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct FootprintDefinition {
    pub id: String,
    pub name: String,
    pub category: String, // "Passives", "Diodes & LEDs", "Transistors & Regulators", "ICs & Logic", "Connectors & THT"
    pub description: String,
    pub ref_prefix: String,
    pub body_w: f32,
    pub body_h: f32,
    pub pitch_mm: f32,
    pub pads: Vec<FootprintPadTemplate>,
    pub silkscreen_lines: Vec<FootprintLineTemplate>,
}

impl FootprintDefinition {
    pub fn generate_svg_preview(&self) -> String {
        let view_w = (self.body_w.max(6.0) + 4.0) * 20.0;
        let view_h = (self.body_h.max(6.0) + 4.0) * 20.0;
        let center_x = view_w * 0.5;
        let center_y = view_h * 0.5;
        let scale = 20.0;

        let mut svg = format!(
            r##"<svg viewBox="0 0 {view_w:.1} {view_h:.1}" width="100%" height="220" style="background:#131519;border-radius:4px;" xmlns="http://www.w3.org/2000/svg">
            <defs>
              <pattern id="grid" width="20" height="20" patternUnits="userSpaceOnUse">
                <path d="M 20 0 L 0 0 0 20" fill="none" stroke="rgba(255,255,255,0.06)" stroke-width="0.8"/>
              </pattern>
            </defs>
            <rect width="100%" height="100%" fill="url(#grid)"/>"##
        );

        // Courtyard / Body outline
        let bx = center_x - (self.body_w * scale * 0.5);
        let by = center_y - (self.body_h * scale * 0.5);
        let bw = self.body_w * scale;
        let bh = self.body_h * scale;
        svg.push_str(&format!(
            r##"<rect x="{bx:.1}" y="{by:.1}" width="{bw:.1}" height="{bh:.1}" fill="rgba(224, 56, 216, 0.08)" stroke="#e038d8" stroke-width="1.2" stroke-dasharray="3,3" rx="2"/>"##
        ));

        // Silkscreen lines
        for l in &self.silkscreen_lines {
            let x0 = center_x + (l.x0 * scale);
            let y0 = center_y + (l.y0 * scale);
            let x1 = center_x + (l.x1 * scale);
            let y1 = center_y + (l.y1 * scale);
            svg.push_str(&format!(
                r##"<line x1="{x0:.1}" y1="{y0:.1}" x2="{x1:.1}" y2="{y1:.1}" stroke="#f2eda1" stroke-width="1.5" stroke-linecap="round"/>"##
            ));
        }

        // Pads
        for p in &self.pads {
            let px = center_x + (p.rel_x * scale);
            let py = center_y + (p.rel_y * scale);
            let pw = p.width * scale;
            let ph = p.height * scale;
            let rx = px - pw * 0.5;
            let ry = py - ph * 0.5;

            if p.is_tht {
                // THT annular ring & drill
                svg.push_str(&format!(
                    r##"<circle cx="{px:.1}" cy="{py:.1}" r="{:.1}" fill="#c8b43c" stroke="#eab308" stroke-width="1.5"/>
                       <circle cx="{px:.1}" cy="{py:.1}" r="{:.1}" fill="#0e1013"/>"##,
                    pw * 0.5,
                    p.drill * scale * 0.5
                ));
            } else {
                // SMD pad (Red F.Cu)
                let fill = if p.pad_num == "1" { "#e14b4b" } else { "#c83434" };
                svg.push_str(&format!(
                    r##"<rect x="{rx:.1}" y="{ry:.1}" width="{pw:.1}" height="{ph:.1}" fill="{fill}" stroke="#ff6b6b" stroke-width="1" rx="2"/>"##
                ));
            }

            // Pad number label
            svg.push_str(&format!(
                r##"<text x="{px:.1}" y="{py:.1}" font-family="JetBrains Mono, monospace" font-size="9" font-weight="700" fill="#ffffff" text-anchor="middle" dominant-baseline="central">{num}</text>"##,
                num = p.pad_num
            ));
        }

        // Pin 1 indicator dot
        if let Some(p1) = self.pads.iter().find(|p| p.pad_num == "1") {
            let dot_x = center_x + (p1.rel_x * scale);
            let dot_y = center_y + (p1.rel_y * scale) - (p1.height * scale * 0.5) - 6.0;
            svg.push_str(&format!(
                r##"<circle cx="{dot_x:.1}" cy="{dot_y:.1}" r="2.5" fill="#f2eda1"/>"##
            ));
        }

        // Component reference text centered
        svg.push_str(&format!(
            r##"<text x="{center_x:.1}" y="{:.1}" font-family="JetBrains Mono, monospace" font-size="11" font-weight="700" fill="#f2eda1" text-anchor="middle">{prefix}?</text>
            </svg>"##,
            by - 6.0,
            prefix = self.ref_prefix
        ));

        svg
    }

    pub fn instantiate_into_board(
        &self,
        board: &mut ParsedBoard,
        center_x: f32,
        center_y: f32,
        layer_mask: u32,
        net_id: u32,
        ref_idx: usize,
    ) {
        let is_top = (layer_mask & LAYER_B_CU) == 0;
        let pad_layer = if is_top { LAYER_F_CU } else { LAYER_B_CU };
        let silk_layer = if is_top { LAYER_F_SILKS } else { LAYER_B_SILKS };

        // 1. Add pads
        for pad in &self.pads {
            let px = center_x + pad.rel_x;
            let py = center_y + pad.rel_y;

            if pad.is_tht {
                // THT via-like pad (drilled)
                board.add_primitive(
                    px,
                    py,
                    px,
                    py,
                    pad.width,
                    pad.drill,
                    LAYER_F_CU | LAYER_B_CU,
                    net_id,
                    1.0,
                );
            } else {
                // SMD pad
                board.add_primitive(
                    px,
                    py,
                    px,
                    py,
                    pad.width,
                    pad.height,
                    pad_layer,
                    net_id,
                    2.0, // rect pad
                );
            }

            // Register pad label
            board.pad_labels.push(PadLabel {
                x: px,
                y: py,
                w: pad.width,
                h: pad.height,
                pin: pad.pad_num.clone(),
                net_id,
                net_name: format!("Net-{}", net_id),
                layer_mask: if pad.is_tht { LAYER_F_CU | LAYER_B_CU } else { pad_layer },
            });
        }

        // 2. Add silkscreen lines
        for l in &self.silkscreen_lines {
            let x0 = center_x + l.x0;
            let y0 = center_y + l.y0;
            let x1 = center_x + l.x1;
            let y1 = center_y + l.y1;
            board.add_primitive(x0, y0, x1, y1, 0.15, 0.0, silk_layer, 0, 0.0);
        }

        // 3. Add reference designator text
        let ref_label = format!("{}{}", self.ref_prefix, ref_idx);
        board.texts.push(BoardText {
            x: center_x,
            y: center_y - (self.body_h * 0.5) - 0.9,
            angle_deg: 0.0,
            text: ref_label,
            size_mm: 1.0,
            layer_mask: silk_layer,
            is_ref: true,
        });

        board.rebuild_ratsnest();
    }
}

pub fn get_standard_ipc_library() -> Vec<FootprintDefinition> {
    vec![
        // ====================================================================
        // Passives (Resistors & Capacitors)
        // ====================================================================
        FootprintDefinition {
            id: "R_0402_1005Metric".to_string(),
            name: "R_0402 (1005 Metric)".to_string(),
            category: "Passives".to_string(),
            description: "Resistor SMD 0402 (1.0 x 0.5 mm), IPC-7351 standard nominal density".to_string(),
            ref_prefix: "R".to_string(),
            body_w: 1.6,
            body_h: 0.9,
            pitch_mm: 1.0,
            pads: vec![
                FootprintPadTemplate { rel_x: -0.50, rel_y: 0.0, width: 0.55, height: 0.60, drill: 0.0, is_tht: false, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x:  0.50, rel_y: 0.0, width: 0.55, height: 0.60, drill: 0.0, is_tht: false, pad_num: "2".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -0.9, y0: -0.45, x1: 0.9, y1: -0.45 },
                FootprintLineTemplate { x0:  0.9, y0: -0.45, x1: 0.9, y1:  0.45 },
                FootprintLineTemplate { x0:  0.9, y0:  0.45, x1: -0.9, y1:  0.45 },
                FootprintLineTemplate { x0: -0.9, y0:  0.45, x1: -0.9, y1: -0.45 },
            ],
        },
        FootprintDefinition {
            id: "R_0603_1608Metric".to_string(),
            name: "R_0603 (1608 Metric)".to_string(),
            category: "Passives".to_string(),
            description: "Resistor SMD 0603 (1.6 x 0.8 mm), IPC-7351 standard nominal density".to_string(),
            ref_prefix: "R".to_string(),
            body_w: 2.3,
            body_h: 1.2,
            pitch_mm: 1.6,
            pads: vec![
                FootprintPadTemplate { rel_x: -0.80, rel_y: 0.0, width: 0.80, height: 0.90, drill: 0.0, is_tht: false, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x:  0.80, rel_y: 0.0, width: 0.80, height: 0.90, drill: 0.0, is_tht: false, pad_num: "2".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -1.2, y0: -0.65, x1: 1.2, y1: -0.65 },
                FootprintLineTemplate { x0:  1.2, y0: -0.65, x1: 1.2, y1:  0.65 },
                FootprintLineTemplate { x0:  1.2, y0:  0.65, x1: -1.2, y1:  0.65 },
                FootprintLineTemplate { x0: -1.2, y0:  0.65, x1: -1.2, y1: -0.65 },
            ],
        },
        FootprintDefinition {
            id: "R_0805_2012Metric".to_string(),
            name: "R_0805 (2012 Metric)".to_string(),
            category: "Passives".to_string(),
            description: "Resistor SMD 0805 (2.0 x 1.25 mm), IPC-7351 standard nominal density".to_string(),
            ref_prefix: "R".to_string(),
            body_w: 2.8,
            body_h: 1.6,
            pitch_mm: 2.0,
            pads: vec![
                FootprintPadTemplate { rel_x: -1.00, rel_y: 0.0, width: 1.00, height: 1.30, drill: 0.0, is_tht: false, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x:  1.00, rel_y: 0.0, width: 1.00, height: 1.30, drill: 0.0, is_tht: false, pad_num: "2".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -1.5, y0: -0.85, x1: 1.5, y1: -0.85 },
                FootprintLineTemplate { x0:  1.5, y0: -0.85, x1: 1.5, y1:  0.85 },
                FootprintLineTemplate { x0:  1.5, y0:  0.85, x1: -1.5, y1:  0.85 },
                FootprintLineTemplate { x0: -1.5, y0:  0.85, x1: -1.5, y1: -0.85 },
            ],
        },
        FootprintDefinition {
            id: "C_0805_2012Metric".to_string(),
            name: "C_0805 (2012 Metric)".to_string(),
            category: "Passives".to_string(),
            description: "Capacitor SMD 0805 (2.0 x 1.25 mm), multilayer ceramic chip capacitor (MLCC)".to_string(),
            ref_prefix: "C".to_string(),
            body_w: 2.8,
            body_h: 1.6,
            pitch_mm: 2.0,
            pads: vec![
                FootprintPadTemplate { rel_x: -1.00, rel_y: 0.0, width: 1.00, height: 1.35, drill: 0.0, is_tht: false, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x:  1.00, rel_y: 0.0, width: 1.00, height: 1.35, drill: 0.0, is_tht: false, pad_num: "2".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -0.4, y0: -0.8, x1: 0.4, y1: -0.8 },
                FootprintLineTemplate { x0: -0.4, y0:  0.8, x1: 0.4, y1:  0.8 },
            ],
        },
        FootprintDefinition {
            id: "C_Radial_D6.3mm_P2.5mm".to_string(),
            name: "C_Radial_D6.3mm (THT Electrolytic)".to_string(),
            category: "Passives".to_string(),
            description: "Aluminum Electrolytic Capacitor, Radial Lead 2.5 mm Pitch, 6.3 mm Diameter".to_string(),
            ref_prefix: "C".to_string(),
            body_w: 6.8,
            body_h: 6.8,
            pitch_mm: 2.5,
            pads: vec![
                FootprintPadTemplate { rel_x: -1.25, rel_y: 0.0, width: 1.6, height: 1.6, drill: 0.8, is_tht: true, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x:  1.25, rel_y: 0.0, width: 1.6, height: 1.6, drill: 0.8, is_tht: true, pad_num: "2".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -3.2, y0: -3.2, x1: 3.2, y1: -3.2 },
                FootprintLineTemplate { x0:  3.2, y0: -3.2, x1: 3.2, y1:  3.2 },
                FootprintLineTemplate { x0:  3.2, y0:  3.2, x1: -3.2, y1:  3.2 },
                FootprintLineTemplate { x0: -3.2, y0:  3.2, x1: -3.2, y1: -3.2 },
                FootprintLineTemplate { x0: -2.4, y0: -1.2, x1: -2.4, y1: -2.4 }, // plus marker
                FootprintLineTemplate { x0: -3.0, y0: -1.8, x1: -1.8, y1: -1.8 },
            ],
        },

        // ====================================================================
        // Diodes & LEDs
        // ====================================================================
        FootprintDefinition {
            id: "D_SOD-123".to_string(),
            name: "D_SOD-123".to_string(),
            category: "Diodes & LEDs".to_string(),
            description: "SOD-123 Plastic Surface Mount Diode Package with Cathode Band".to_string(),
            ref_prefix: "D".to_string(),
            body_w: 3.8,
            body_h: 1.8,
            pitch_mm: 3.0,
            pads: vec![
                FootprintPadTemplate { rel_x: -1.55, rel_y: 0.0, width: 0.9, height: 1.2, drill: 0.0, is_tht: false, pad_num: "1".to_string() }, // Cathode
                FootprintPadTemplate { rel_x:  1.55, rel_y: 0.0, width: 0.9, height: 1.2, drill: 0.0, is_tht: false, pad_num: "2".to_string() }, // Anode
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -1.9, y0: -0.9, x1: 1.9, y1: -0.9 },
                FootprintLineTemplate { x0:  1.9, y0: -0.9, x1: 1.9, y1:  0.9 },
                FootprintLineTemplate { x0:  1.9, y0:  0.9, x1: -1.9, y1:  0.9 },
                FootprintLineTemplate { x0: -1.9, y0:  0.9, x1: -1.9, y1: -0.9 },
                FootprintLineTemplate { x0: -0.8, y0: -0.9, x1: -0.8, y1:  0.9 }, // Cathode bar
            ],
        },
        FootprintDefinition {
            id: "LED_0805_2012Metric".to_string(),
            name: "LED_0805 (2012 Metric)".to_string(),
            category: "Diodes & LEDs".to_string(),
            description: "SMD 0805 Surface Mount LED with Green/Red Polarity Markings".to_string(),
            ref_prefix: "D".to_string(),
            body_w: 2.8,
            body_h: 1.6,
            pitch_mm: 2.0,
            pads: vec![
                FootprintPadTemplate { rel_x: -1.00, rel_y: 0.0, width: 1.0, height: 1.3, drill: 0.0, is_tht: false, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x:  1.00, rel_y: 0.0, width: 1.0, height: 1.3, drill: 0.0, is_tht: false, pad_num: "2".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -1.4, y0: -0.8, x1: 1.4, y1: -0.8 },
                FootprintLineTemplate { x0:  1.4, y0: -0.8, x1: 1.4, y1:  0.8 },
                FootprintLineTemplate { x0:  1.4, y0:  0.8, x1: -1.4, y1:  0.8 },
                FootprintLineTemplate { x0: -1.4, y0:  0.8, x1: -1.4, y1: -0.8 },
                FootprintLineTemplate { x0: -0.7, y0: -0.7, x1: -0.7, y1:  0.7 }, // cathode line
            ],
        },

        // ====================================================================
        // Transistors & Regulators
        // ====================================================================
        FootprintDefinition {
            id: "SOT-23".to_string(),
            name: "SOT-23 (TO-236AB)".to_string(),
            category: "Transistors & Regulators".to_string(),
            description: "3-pin SOT-23 small-signal transistor & MOSFET package".to_string(),
            ref_prefix: "Q".to_string(),
            body_w: 3.1,
            body_h: 2.6,
            pitch_mm: 1.9,
            pads: vec![
                FootprintPadTemplate { rel_x: -0.95, rel_y: -1.0, width: 0.7, height: 1.0, drill: 0.0, is_tht: false, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x:  0.95, rel_y: -1.0, width: 0.7, height: 1.0, drill: 0.0, is_tht: false, pad_num: "2".to_string() },
                FootprintPadTemplate { rel_x:  0.00, rel_y:  1.0, width: 0.7, height: 1.0, drill: 0.0, is_tht: false, pad_num: "3".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -1.5, y0: -0.5, x1: -1.5, y1: 0.5 },
                FootprintLineTemplate { x0:  1.5, y0: -0.5, x1:  1.5, y1: 0.5 },
                FootprintLineTemplate { x0: -0.5, y0: -0.8, x1:  0.5, y1: -0.8 },
            ],
        },
        FootprintDefinition {
            id: "SOT-223-3".to_string(),
            name: "SOT-223-3 (with Thermal Tab)".to_string(),
            category: "Transistors & Regulators".to_string(),
            description: "SOT-223 3-lead voltage regulator package with large heatsink tab (e.g. AMS1117)".to_string(),
            ref_prefix: "U".to_string(),
            body_w: 7.4,
            body_h: 7.2,
            pitch_mm: 2.3,
            pads: vec![
                FootprintPadTemplate { rel_x: -2.3, rel_y: -3.1, width: 1.0, height: 1.8, drill: 0.0, is_tht: false, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x:  0.0, rel_y: -3.1, width: 1.0, height: 1.8, drill: 0.0, is_tht: false, pad_num: "2".to_string() },
                FootprintPadTemplate { rel_x:  2.3, rel_y: -3.1, width: 1.0, height: 1.8, drill: 0.0, is_tht: false, pad_num: "3".to_string() },
                FootprintPadTemplate { rel_x:  0.0, rel_y:  3.1, width: 3.4, height: 1.8, drill: 0.0, is_tht: false, pad_num: "4".to_string() }, // TAB
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -3.5, y0: -1.8, x1: 3.5, y1: -1.8 },
                FootprintLineTemplate { x0:  3.5, y0: -1.8, x1: 3.5, y1:  1.8 },
                FootprintLineTemplate { x0:  3.5, y0:  1.8, x1: -3.5, y1:  1.8 },
                FootprintLineTemplate { x0: -3.5, y0:  1.8, x1: -3.5, y1: -1.8 },
            ],
        },

        // ====================================================================
        // Integrated Circuits (ICs & Logic)
        // ====================================================================
        FootprintDefinition {
            id: "SOIC-8_3.9x4.9mm_P1.27mm".to_string(),
            name: "SOIC-8 (1.27 mm Pitch)".to_string(),
            category: "ICs & Logic".to_string(),
            description: "SOIC-8 Small Outline IC (3.9 x 4.9 mm), 1.27 mm pin pitch (Op-Amps, EEPROMs, 555)".to_string(),
            ref_prefix: "U".to_string(),
            body_w: 6.2,
            body_h: 5.2,
            pitch_mm: 1.27,
            pads: vec![
                FootprintPadTemplate { rel_x: -2.7, rel_y: -1.905, width: 1.5, height: 0.65, drill: 0.0, is_tht: false, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x: -2.7, rel_y: -0.635, width: 1.5, height: 0.65, drill: 0.0, is_tht: false, pad_num: "2".to_string() },
                FootprintPadTemplate { rel_x: -2.7, rel_y:  0.635, width: 1.5, height: 0.65, drill: 0.0, is_tht: false, pad_num: "3".to_string() },
                FootprintPadTemplate { rel_x: -2.7, rel_y:  1.905, width: 1.5, height: 0.65, drill: 0.0, is_tht: false, pad_num: "4".to_string() },
                FootprintPadTemplate { rel_x:  2.7, rel_y:  1.905, width: 1.5, height: 0.65, drill: 0.0, is_tht: false, pad_num: "5".to_string() },
                FootprintPadTemplate { rel_x:  2.7, rel_y:  0.635, width: 1.5, height: 0.65, drill: 0.0, is_tht: false, pad_num: "6".to_string() },
                FootprintPadTemplate { rel_x:  2.7, rel_y: -0.635, width: 1.5, height: 0.65, drill: 0.0, is_tht: false, pad_num: "7".to_string() },
                FootprintPadTemplate { rel_x:  2.7, rel_y: -1.905, width: 1.5, height: 0.65, drill: 0.0, is_tht: false, pad_num: "8".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -2.0, y0: -2.5, x1: 2.0, y1: -2.5 },
                FootprintLineTemplate { x0:  2.0, y0: -2.5, x1: 2.0, y1:  2.5 },
                FootprintLineTemplate { x0:  2.0, y0:  2.5, x1: -2.0, y1:  2.5 },
                FootprintLineTemplate { x0: -2.0, y0:  2.5, x1: -2.0, y1: -2.5 },
                FootprintLineTemplate { x0: -0.5, y0: -2.5, x1: 0.5, y1: -2.5 }, // notch
            ],
        },
        FootprintDefinition {
            id: "QFN-32_5x5mm_P0.5mm".to_string(),
            name: "QFN-32 (5x5 mm, 0.5 mm Pitch)".to_string(),
            category: "ICs & Logic".to_string(),
            description: "Quad Flat No-Lead 32-pin package with center thermal ground pad (RP2040, ATmega328PB)".to_string(),
            ref_prefix: "U".to_string(),
            body_w: 5.5,
            body_h: 5.5,
            pitch_mm: 0.5,
            pads: {
                let mut p = Vec::new();
                let span = 2.45;
                // Left side (1..=8)
                for i in 0..8 {
                    let y = -1.75 + (i as f32) * 0.5;
                    p.push(FootprintPadTemplate { rel_x: -span, rel_y: y, width: 0.8, height: 0.28, drill: 0.0, is_tht: false, pad_num: (i + 1).to_string() });
                }
                // Bottom side (9..=16)
                for i in 0..8 {
                    let x = -1.75 + (i as f32) * 0.5;
                    p.push(FootprintPadTemplate { rel_x: x, rel_y: span, width: 0.28, height: 0.8, drill: 0.0, is_tht: false, pad_num: (i + 9).to_string() });
                }
                // Right side (17..=24)
                for i in 0..8 {
                    let y = 1.75 - (i as f32) * 0.5;
                    p.push(FootprintPadTemplate { rel_x: span, rel_y: y, width: 0.8, height: 0.28, drill: 0.0, is_tht: false, pad_num: (i + 17).to_string() });
                }
                // Top side (25..=32)
                for i in 0..8 {
                    let x = 1.75 - (i as f32) * 0.5;
                    p.push(FootprintPadTemplate { rel_x: x, rel_y: -span, width: 0.28, height: 0.8, drill: 0.0, is_tht: false, pad_num: (i + 25).to_string() });
                }
                // Thermal Pad (EP)
                p.push(FootprintPadTemplate { rel_x: 0.0, rel_y: 0.0, width: 3.4, height: 3.4, drill: 0.0, is_tht: false, pad_num: "33".to_string() });
                p
            },
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -2.6, y0: -2.6, x1: -2.2, y1: -2.6 },
                FootprintLineTemplate { x0: -2.6, y0: -2.6, x1: -2.6, y1: -2.2 },
                FootprintLineTemplate { x0:  2.6, y0: -2.6, x1:  2.6, y1: -2.2 },
                FootprintLineTemplate { x0:  2.6, y0:  2.6, x1:  2.6, y1:  2.2 },
            ],
        },

        // ====================================================================
        // Connectors & THT
        // ====================================================================
        FootprintDefinition {
            id: "USB_C_Receptacle_16P".to_string(),
            name: "USB-C Receptacle (16-Pin USB 2.0)".to_string(),
            category: "Connectors & THT".to_string(),
            description: "USB Type-C 16-pin connector with 4 mechanical shield through-hole grounding tabs".to_string(),
            ref_prefix: "J".to_string(),
            body_w: 9.6,
            body_h: 7.8,
            pitch_mm: 0.5,
            pads: vec![
                // 4 Shield THT grounding lugs
                FootprintPadTemplate { rel_x: -4.3, rel_y: -2.8, width: 1.6, height: 2.2, drill: 1.0, is_tht: true, pad_num: "SH1".to_string() },
                FootprintPadTemplate { rel_x:  4.3, rel_y: -2.8, width: 1.6, height: 2.2, drill: 1.0, is_tht: true, pad_num: "SH2".to_string() },
                FootprintPadTemplate { rel_x: -4.3, rel_y:  1.6, width: 1.6, height: 2.2, drill: 1.0, is_tht: true, pad_num: "SH3".to_string() },
                FootprintPadTemplate { rel_x:  4.3, rel_y:  1.6, width: 1.6, height: 2.2, drill: 1.0, is_tht: true, pad_num: "SH4".to_string() },
                // GND / VBUS / CC1 / D+ / D- / CC2
                FootprintPadTemplate { rel_x: -2.8, rel_y: 2.4, width: 0.35, height: 1.2, drill: 0.0, is_tht: false, pad_num: "A1".to_string() },
                FootprintPadTemplate { rel_x: -2.0, rel_y: 2.4, width: 0.35, height: 1.2, drill: 0.0, is_tht: false, pad_num: "A4".to_string() },
                FootprintPadTemplate { rel_x: -0.7, rel_y: 2.4, width: 0.30, height: 1.2, drill: 0.0, is_tht: false, pad_num: "A6".to_string() },
                FootprintPadTemplate { rel_x:  0.7, rel_y: 2.4, width: 0.30, height: 1.2, drill: 0.0, is_tht: false, pad_num: "A7".to_string() },
                FootprintPadTemplate { rel_x:  2.0, rel_y: 2.4, width: 0.35, height: 1.2, drill: 0.0, is_tht: false, pad_num: "B9".to_string() },
                FootprintPadTemplate { rel_x:  2.8, rel_y: 2.4, width: 0.35, height: 1.2, drill: 0.0, is_tht: false, pad_num: "B12".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -4.5, y0: -3.8, x1:  4.5, y1: -3.8 },
                FootprintLineTemplate { x0:  4.5, y0: -3.8, x1:  4.5, y1:  3.2 },
                FootprintLineTemplate { x0:  4.5, y0:  3.2, x1: -4.5, y1:  3.2 },
                FootprintLineTemplate { x0: -4.5, y0:  3.2, x1: -4.5, y1: -3.8 },
            ],
        },
        FootprintDefinition {
            id: "PinHeader_1x04_P2.54mm_Vertical".to_string(),
            name: "PinHeader_1x04 (2.54 mm Pitch)".to_string(),
            category: "Connectors & THT".to_string(),
            description: "1x4-pin 2.54 mm (0.1 in) standard vertical header for UART/I2C/Power".to_string(),
            ref_prefix: "J".to_string(),
            body_w: 10.4,
            body_h: 2.8,
            pitch_mm: 2.54,
            pads: vec![
                FootprintPadTemplate { rel_x: -3.81, rel_y: 0.0, width: 1.7, height: 1.7, drill: 1.0, is_tht: true, pad_num: "1".to_string() },
                FootprintPadTemplate { rel_x: -1.27, rel_y: 0.0, width: 1.7, height: 1.7, drill: 1.0, is_tht: true, pad_num: "2".to_string() },
                FootprintPadTemplate { rel_x:  1.27, rel_y: 0.0, width: 1.7, height: 1.7, drill: 1.0, is_tht: true, pad_num: "3".to_string() },
                FootprintPadTemplate { rel_x:  3.81, rel_y: 0.0, width: 1.7, height: 1.7, drill: 1.0, is_tht: true, pad_num: "4".to_string() },
            ],
            silkscreen_lines: vec![
                FootprintLineTemplate { x0: -5.1, y0: -1.3, x1: 5.1, y1: -1.3 },
                FootprintLineTemplate { x0:  5.1, y0: -1.3, x1: 5.1, y1:  1.3 },
                FootprintLineTemplate { x0:  5.1, y0:  1.3, x1: -5.1, y1:  1.3 },
                FootprintLineTemplate { x0: -5.1, y0:  1.3, x1: -5.1, y1: -1.3 },
            ],
        },
    ]
}
