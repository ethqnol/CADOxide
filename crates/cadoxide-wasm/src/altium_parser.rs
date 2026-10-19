//! Altium Designer Binary Parser (.PcbDoc and .SchDoc)
//! Pure Rust zero-dependency implementation of Microsoft Compound Document File (OLE2 / CFB)
//! and Altium binary/ASCII primitive stream deserialization.

use crate::parser::{
    BoardComponent, BoardText, NetStats, PadLabel, ParsedBoard,
    FLOATS_PER_INSTANCE, LAYER_B_CU, LAYER_B_MASK, LAYER_B_SILKS, LAYER_EDGE_CUTS, LAYER_F_CU,
    LAYER_F_MASK, LAYER_F_SILKS, LAYER_IN1_CU, LAYER_IN2_CU, SPATIAL_CELL_MM,
};
use std::collections::HashMap;

// ============================================================================
// 1. Compound File Binary (CFB / OLE2) Container Parser
// ============================================================================

const CFB_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const ENDOFCHAIN: u32 = 0xFFFFFFFE;
const FREESECT: u32 = 0xFFFFFFFF;

#[derive(Clone, Debug)]
pub struct CfbEntry {
    pub name: String,
    pub entry_type: u8, // 1 = Storage, 2 = Stream, 5 = Root
    pub start_sector: u32,
    pub stream_size: u64,
}

#[derive(Debug)]
pub struct CfbReader<'a> {
    data: &'a [u8],
    sector_size: usize,
    mini_sector_size: usize,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    pub entries: Vec<CfbEntry>,
}

impl<'a> CfbReader<'a> {
    pub fn is_cfb(data: &[u8]) -> bool {
        data.len() >= 512 && data[0..8] == CFB_SIGNATURE
    }

    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if !Self::is_cfb(data) {
            return Err("Invalid CFB signature");
        }

        let sector_shift = u16::from_le_bytes([data[30], data[31]]) as usize;
        let mini_sector_shift = u16::from_le_bytes([data[32], data[33]]) as usize;
        let sector_size = 1 << sector_shift;
        let mini_sector_size = 1 << mini_sector_shift;

        let num_fat_sectors = u32::from_le_bytes([data[44], data[45], data[46], data[47]]) as usize;
        let dir_first_sector = u32::from_le_bytes([data[48], data[49], data[50], data[51]]);
        let mini_fat_first_sector = u32::from_le_bytes([data[60], data[61], data[62], data[63]]);
        let num_mini_fat_sectors = u32::from_le_bytes([data[64], data[65], data[66], data[67]]) as usize;
        let difat_first_sector = u32::from_le_bytes([data[68], data[69], data[70], data[71]]);
        let num_difat_sectors = u32::from_le_bytes([data[72], data[73], data[74], data[75]]) as usize;

        // Collect DIFAT sector indices
        let mut difat = Vec::new();
        for i in 0..109 {
            let offset = 76 + i * 4;
            if offset + 4 <= 512 {
                let s = u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
                if s != FREESECT && s != ENDOFCHAIN {
                    difat.push(s);
                }
            }
        }

        // Chained DIFAT sectors if file exceeds ~7MB
        let mut curr_difat = difat_first_sector;
        let entries_per_difat = ((sector_size / 4) as usize).saturating_sub(1);
        for _ in 0..num_difat_sectors {
            if curr_difat == ENDOFCHAIN || curr_difat == FREESECT {
                break;
            }
            let s_offset = 512 + (curr_difat as usize) * sector_size;
            if s_offset + sector_size <= data.len() {
                for i in 0..entries_per_difat {
                    let off = s_offset + i * 4;
                    let s = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
                    if s != FREESECT && s != ENDOFCHAIN {
                        difat.push(s);
                    }
                }
                let next_off = s_offset + entries_per_difat * 4;
                curr_difat = u32::from_le_bytes([data[next_off], data[next_off + 1], data[next_off + 2], data[next_off + 3]]);
            } else {
                break;
            }
        }

        // Build FAT
        let mut fat = Vec::new();
        for &fat_sec in difat.iter().take(num_fat_sectors) {
            let s_offset = 512 + (fat_sec as usize) * sector_size;
            if s_offset + sector_size <= data.len() {
                for i in 0..(sector_size / 4) {
                    let off = s_offset + i * 4;
                    let next_sec = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
                    fat.push(next_sec);
                }
            }
        }

        // Helper to read a sector chain
        let read_chain = |start: u32, fat_table: &[u32], sec_size: usize, raw_data: &[u8], is_mini: bool| -> Vec<u8> {
            let mut out = Vec::new();
            let mut curr = start;
            let mut visited = std::collections::HashSet::new();

            while curr != ENDOFCHAIN && curr != FREESECT {
                if !visited.insert(curr) {
                    break; // prevent infinite loops on corrupted files
                }
                let offset = if is_mini {
                    (curr as usize) * sec_size
                } else {
                    512 + (curr as usize) * sec_size
                };

                if offset + sec_size <= raw_data.len() {
                    out.extend_from_slice(&raw_data[offset..offset + sec_size]);
                } else {
                    break;
                }

                if (curr as usize) < fat_table.len() {
                    curr = fat_table[curr as usize];
                } else {
                    break;
                }
            }
            out
        };

        // Read Mini-FAT
        let mut mini_fat = Vec::new();
        if num_mini_fat_sectors > 0 && mini_fat_first_sector != ENDOFCHAIN {
            let mini_fat_bytes = read_chain(mini_fat_first_sector, &fat, sector_size, data, false);
            for chunk in mini_fat_bytes.chunks_exact(4) {
                mini_fat.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
        }

        // Read Directory entries
        let dir_bytes = read_chain(dir_first_sector, &fat, sector_size, data, false);
        let mut entries = Vec::new();
        for chunk in dir_bytes.chunks_exact(128) {
            let name_len = u16::from_le_bytes([chunk[64], chunk[65]]) as usize;
            let entry_type = chunk[66];
            let start_sector = u32::from_le_bytes([chunk[116], chunk[117], chunk[118], chunk[119]]);
            let stream_size = u64::from_le_bytes([
                chunk[120], chunk[121], chunk[122], chunk[123],
                chunk[124], chunk[125], chunk[126], chunk[127],
            ]);

            let mut name = String::new();
            if name_len >= 2 {
                let actual_len = (name_len - 2).min(64);
                let u16_chars: Vec<u16> = chunk[0..actual_len]
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect();
                name = String::from_utf16_lossy(&u16_chars);
            }

            if entry_type != 0 && !name.is_empty() {
                entries.push(CfbEntry {
                    name,
                    entry_type,
                    start_sector,
                    stream_size,
                });
            }
        }

        // Read Mini-Stream container (root directory stream)
        let mini_stream = if let Some(root) = entries.iter().find(|e| e.entry_type == 5) {
            read_chain(root.start_sector, &fat, sector_size, data, false)
        } else {
            Vec::new()
        };

        Ok(Self {
            data,
            sector_size,
            mini_sector_size,
            fat,
            mini_fat,
            mini_stream,
            entries,
        })
    }

    pub fn get_stream(&self, name: &str) -> Option<Vec<u8>> {
        let entry = self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))?;
        if entry.entry_type != 2 {
            return None;
        }

        let size = entry.stream_size as usize;
        let bytes = if size < 4096 {
            // Read from Mini-Stream
            let mut out = Vec::new();
            let mut curr = entry.start_sector;
            let mut visited = std::collections::HashSet::new();

            while curr != ENDOFCHAIN && curr != FREESECT {
                if !visited.insert(curr) {
                    break;
                }
                let offset = (curr as usize) * self.mini_sector_size;
                if offset + self.mini_sector_size <= self.mini_stream.len() {
                    out.extend_from_slice(&self.mini_stream[offset..offset + self.mini_sector_size]);
                } else {
                    break;
                }
                if (curr as usize) < self.mini_fat.len() {
                    curr = self.mini_fat[curr as usize];
                } else {
                    break;
                }
            }
            out.truncate(size);
            out
        } else {
            // Read standard sector chain
            let mut out = Vec::new();
            let mut curr = entry.start_sector;
            let mut visited = std::collections::HashSet::new();

            while curr != ENDOFCHAIN && curr != FREESECT {
                if !visited.insert(curr) {
                    break;
                }
                let offset = 512 + (curr as usize) * self.sector_size;
                if offset + self.sector_size <= self.data.len() {
                    out.extend_from_slice(&self.data[offset..offset + self.sector_size]);
                } else {
                    break;
                }
                if (curr as usize) < self.fat.len() {
                    curr = self.fat[curr as usize];
                } else {
                    break;
                }
            }
            out.truncate(size);
            out
        };

        Some(bytes)
    }

    pub fn list_stream_names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }
}

// ============================================================================
// 2. Altium Property Block Parser (|KEY=VALUE|...)
// ============================================================================

pub fn parse_pipe_properties(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for token in text.split('|') {
        if let Some(eq_pos) = token.find('=') {
            let key = token[..eq_pos].trim().to_uppercase();
            let val = token[eq_pos + 1..].trim().to_string();
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

/// Split an Altium binary or ASCII stream into records.
/// Records in Altium streams begin with either a 2-byte or 4-byte length prefix, or pipe characters.
pub fn extract_altium_text_records(stream_bytes: &[u8]) -> Vec<HashMap<String, String>> {
    let mut records = Vec::new();
    let mut idx = 0;

    while idx < stream_bytes.len() {
        // Search for '|' character
        if let Some(pipe_pos) = stream_bytes[idx..].iter().position(|&b| b == b'|') {
            let start = idx + pipe_pos;
            // Find end of record (null byte or start of next length prefix)
            let mut end = stream_bytes.len();
            for i in start..stream_bytes.len() {
                if stream_bytes[i] == 0 {
                    end = i;
                    break;
                }
            }
            if end > start {
                if let Ok(s) = std::str::from_utf8(&stream_bytes[start..end]) {
                    let props = parse_pipe_properties(s);
                    if !props.is_empty() {
                        records.push(props);
                    }
                }
            }
            idx = end + 1;
        } else {
            break;
        }
    }

    records
}

// Convert Altium internal coordinate units (1 mil = 10000 units, 1 mm = 393700.787 units, or direct mils) to mm
pub fn altium_coord_to_mm(val_str: Option<&String>) -> f32 {
    let s = match val_str {
        Some(s) => s.trim(),
        None => return 0.0,
    };

    // If unit suffix exists (e.g. "120mil", "3.5mm", "12mil")
    if let Some(mil_str) = s.strip_suffix("mil") {
        return mil_str.parse::<f32>().unwrap_or(0.0) * 0.0254;
    }
    if let Some(mm_str) = s.strip_suffix("mm") {
        return mm_str.parse::<f32>().unwrap_or(0.0);
    }
    if let Some(in_str) = s.strip_suffix("in") {
        return in_str.parse::<f32>().unwrap_or(0.0) * 25.4;
    }

    // Pure numeric value: Altium uses 10,000 units per mil (i.e. 10000 = 1 mil = 0.0254 mm)
    if let Ok(num) = s.parse::<f64>() {
        if num.abs() > 1000.0 {
            // High-resolution internal unit: 1 mil = 10000 units
            return (num * (0.0254 / 10000.0)) as f32;
        } else {
            // Assume direct mils if small integer/float
            return (num * 0.0254) as f32;
        }
    }

    0.0
}

pub fn altium_layer_to_mask(layer_str: Option<&String>) -> u32 {
    let s = match layer_str {
        Some(s) => s.to_uppercase(),
        None => return LAYER_F_CU,
    };

    match s.as_str() {
        "1" | "TOPLAYER" | "TOP" | "F.CU" => LAYER_F_CU,
        "2" | "BOTTOMLAYER" | "BOTTOM" | "B.CU" => LAYER_B_CU,
        "3" | "MID1" | "INTERNALPLANE1" | "IN1.CU" => LAYER_IN1_CU,
        "4" | "MID2" | "INTERNALPLANE2" | "IN2.CU" => LAYER_IN2_CU,
        "TOP_OVERLAY" | "TOPOVERLAY" | "F.SILKS" => LAYER_F_SILKS,
        "BOTTOM_OVERLAY" | "BOTTOMOVERLAY" | "B.SILKS" => LAYER_B_SILKS,
        "TOP_SOLDER" | "TOPSOLDER" | "F.MASK" => LAYER_F_MASK,
        "BOTTOM_SOLDER" | "BOTTOMSOLDER" | "B.MASK" => LAYER_B_MASK,
        "KEEPOUT" | "OUTLINE" | "MECHANICAL1" | "EDGE.CUTS" => LAYER_EDGE_CUTS,
        _ => LAYER_F_CU,
    }
}

// ============================================================================
// 3. Altium .PcbDoc Migration Pipeline -> ParsedBoard
// ============================================================================

pub fn parse_altium_pcbdoc_bytes(data: &[u8]) -> Result<ParsedBoard, &'static str> {
    let cfb = CfbReader::new(data)?;

    let mut instances: Vec<f32> = Vec::new();
    let mut primitive_count = 0usize;
    let mut nets: HashMap<u32, NetStats> = HashMap::new();
    let mut pad_labels: Vec<PadLabel> = Vec::new();
    let mut texts: Vec<BoardText> = Vec::new();
    let mut components: Vec<BoardComponent> = Vec::new();

    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;

    let update_bounds = |x: f32, y: f32, min_x: &mut f32, min_y: &mut f32, max_x: &mut f32, max_y: &mut f32| {
        if x < *min_x { *min_x = x; }
        if x > *max_x { *max_x = x; }
        if y < *min_y { *min_y = y; }
        if y > *max_y { *max_y = y; }
    };

    // Map Net Name -> Net ID
    let mut net_name_to_id: HashMap<String, u32> = HashMap::new();
    let mut next_net_id = 1u32;

    // 1. Parse Nets Stream if available
    for stream_name in ["Nets", "NETS", "Board6/Nets"] {
        if let Some(stream_data) = cfb.get_stream(stream_name) {
            let records = extract_altium_text_records(&stream_data);
            for rec in records {
                if let Some(name) = rec.get("NAME").or_else(|| rec.get("NETNAME")) {
                    if !name.is_empty() && !net_name_to_id.contains_key(name) {
                        let nid = next_net_id;
                        next_net_id += 1;
                        net_name_to_id.insert(name.clone(), nid);
                        nets.insert(nid, NetStats {
                            id: nid,
                            name: name.clone(),
                            total_length_mm: 0.0,
                            segment_count: 0,
                            via_count: 0,
                            pad_count: 0,
                        });
                    }
                }
            }
        }
    }

    // Helper to get or insert net id
    let get_or_create_net = |name: &str, nets: &mut HashMap<u32, NetStats>, map: &mut HashMap<String, u32>, next_id: &mut u32| -> u32 {
        if name.is_empty() || name == "?" || name == "No Net" {
            return 0;
        }
        if let Some(&id) = map.get(name) {
            id
        } else {
            let id = *next_id;
            *next_id += 1;
            map.insert(name.to_string(), id);
            nets.insert(id, NetStats {
                id,
                name: name.to_string(),
                total_length_mm: 0.0,
                segment_count: 0,
                via_count: 0,
                pad_count: 0,
            });
            id
        }
    };

    // 2. Parse Tracks
    for stream_name in ["Tracks", "TRACKS", "Board6/Tracks"] {
        if let Some(stream_data) = cfb.get_stream(stream_name) {
            let records = extract_altium_text_records(&stream_data);
            for rec in records {
                let x1 = altium_coord_to_mm(rec.get("X1").or_else(|| rec.get("START.X")));
                let y1 = altium_coord_to_mm(rec.get("Y1").or_else(|| rec.get("START.Y")));
                let x2 = altium_coord_to_mm(rec.get("X2").or_else(|| rec.get("END.X")));
                let y2 = altium_coord_to_mm(rec.get("Y2").or_else(|| rec.get("END.Y")));
                let width = altium_coord_to_mm(rec.get("WIDTH")).max(0.1);
                let layer_mask = altium_layer_to_mask(rec.get("LAYER"));
                let net_name = rec.get("NET").or_else(|| rec.get("NETNAME")).cloned().unwrap_or_default();
                let net_id = get_or_create_net(&net_name, &mut nets, &mut net_name_to_id, &mut next_net_id);

                update_bounds(x1, y1, &mut min_x, &mut min_y, &mut max_x, &mut max_y);
                update_bounds(x2, y2, &mut min_x, &mut min_y, &mut max_x, &mut max_y);

                let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
                if let Some(ns) = nets.get_mut(&net_id) {
                    ns.total_length_mm += len;
                    ns.segment_count += 1;
                }

                let encoded = (net_id as f32) * 16.0;
                instances.extend_from_slice(&[x1, y1, x2, y2, width, width, layer_mask as f32, encoded]);
                primitive_count += 1;
            }
        }
    }

    // 3. Parse Vias
    for stream_name in ["Vias", "VIAS", "Board6/Vias"] {
        if let Some(stream_data) = cfb.get_stream(stream_name) {
            let records = extract_altium_text_records(&stream_data);
            for rec in records {
                let x = altium_coord_to_mm(rec.get("X").or_else(|| rec.get("LOCATION.X")));
                let y = altium_coord_to_mm(rec.get("Y").or_else(|| rec.get("LOCATION.Y")));
                let diameter = altium_coord_to_mm(rec.get("DIAMETER").or_else(|| rec.get("HOLESIZE"))).max(0.4);
                let drill = altium_coord_to_mm(rec.get("HOLESIZE")).max(0.2);
                let net_name = rec.get("NET").or_else(|| rec.get("NETNAME")).cloned().unwrap_or_default();
                let net_id = get_or_create_net(&net_name, &mut nets, &mut net_name_to_id, &mut next_net_id);

                update_bounds(x, y, &mut min_x, &mut min_y, &mut max_x, &mut max_y);

                if let Some(ns) = nets.get_mut(&net_id) {
                    ns.via_count += 1;
                }

                let layer_mask = LAYER_F_CU | LAYER_B_CU;
                let encoded = (net_id as f32) * 16.0 + 1.0;
                instances.extend_from_slice(&[x, y, x, y, diameter, drill, layer_mask as f32, encoded]);
                primitive_count += 1;
            }
        }
    }

    // 4. Parse Pads
    for stream_name in ["Pads", "PADS", "Board6/Pads"] {
        if let Some(stream_data) = cfb.get_stream(stream_name) {
            let records = extract_altium_text_records(&stream_data);
            for rec in records {
                let x = altium_coord_to_mm(rec.get("X").or_else(|| rec.get("LOCATION.X")));
                let y = altium_coord_to_mm(rec.get("Y").or_else(|| rec.get("LOCATION.Y")));
                let size_x = altium_coord_to_mm(rec.get("SIZEX").or_else(|| rec.get("XSIZE"))).max(0.2);
                let size_y = altium_coord_to_mm(rec.get("SIZEY").or_else(|| rec.get("YSIZE"))).max(0.2);
                let hole_size = altium_coord_to_mm(rec.get("HOLESIZE"));
                let layer_mask = altium_layer_to_mask(rec.get("LAYER"));
                let net_name = rec.get("NET").or_else(|| rec.get("NETNAME")).cloned().unwrap_or_default();
                let pin_name = rec.get("NAME").or_else(|| rec.get("PADNAME")).cloned().unwrap_or_else(|| "1".to_string());
                let net_id = get_or_create_net(&net_name, &mut nets, &mut net_name_to_id, &mut next_net_id);

                update_bounds(x, y, &mut min_x, &mut min_y, &mut max_x, &mut max_y);

                if let Some(ns) = nets.get_mut(&net_id) {
                    ns.pad_count += 1;
                }

                pad_labels.push(PadLabel {
                    x,
                    y,
                    w: size_x,
                    h: size_y,
                    pin: pin_name,
                    net_id,
                    net_name,
                    layer_mask,
                });

                let pad_type = if hole_size > 0.05 { 1.0 } else { 2.0 };
                let encoded = (net_id as f32) * 16.0 + pad_type;
                instances.extend_from_slice(&[x, y, x, y, size_x, size_y.max(hole_size), layer_mask as f32, encoded]);
                primitive_count += 1;
            }
        }
    }

    // 5. Parse Components
    for stream_name in ["Components", "COMPONENTS", "Board6/Components"] {
        if let Some(stream_data) = cfb.get_stream(stream_name) {
            let records = extract_altium_text_records(&stream_data);
            for rec in records {
                let x = altium_coord_to_mm(rec.get("X").or_else(|| rec.get("LOCATION.X")));
                let y = altium_coord_to_mm(rec.get("Y").or_else(|| rec.get("LOCATION.Y")));
                let rot = rec.get("ROTATION").and_then(|r| r.parse::<f32>().ok()).unwrap_or(0.0);
                let designator = rec.get("SOURCEDESIGNATOR")
                    .or_else(|| rec.get("DESIGNATOR"))
                    .or_else(|| rec.get("NAME"))
                    .cloned()
                    .unwrap_or_else(|| "U?".to_string());
                let comment = rec.get("SOURCECOMMENT")
                    .or_else(|| rec.get("COMMENT"))
                    .cloned()
                    .unwrap_or_default();
                let footprint = rec.get("FOOTPRINT")
                    .or_else(|| rec.get("PATTERN"))
                    .cloned()
                    .unwrap_or_default();

                components.push(BoardComponent {
                    reference: designator.clone(),
                    value: comment,
                    footprint,
                    x,
                    y,
                    rot_deg: rot,
                    layer: "F.Cu".to_string(),
                });

                texts.push(BoardText {
                    x,
                    y: y - 1.5,
                    angle_deg: rot,
                    text: designator,
                    size_mm: 1.2,
                    layer_mask: LAYER_F_SILKS,
                    is_ref: true,
                });
            }
        }
    }

    // Fallback bounds if empty board
    if min_x > max_x {
        min_x = 0.0;
        max_x = 100.0;
        min_y = 0.0;
        max_y = 100.0;
    }

    // Add outline if missing
    let bounds = [min_x - 5.0, min_y - 5.0, max_x + 5.0, max_y + 5.0];

    // Build uniform spatial hash grid
    let mut spatial_grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for i in 0..primitive_count {
        let base = i * FLOATS_PER_INSTANCE;
        let x0 = instances[base];
        let y0 = instances[base + 1];
        let x1 = instances[base + 2];
        let y1 = instances[base + 3];

        let min_cx = (x0.min(x1) / SPATIAL_CELL_MM).floor() as i32;
        let max_cx = (x0.max(x1) / SPATIAL_CELL_MM).floor() as i32;
        let min_cy = (y0.min(y1) / SPATIAL_CELL_MM).floor() as i32;
        let max_cy = (y0.max(y1) / SPATIAL_CELL_MM).floor() as i32;

        for cx in min_cx..=max_cx {
            for cy in min_cy..=max_cy {
                spatial_grid.entry((cx, cy)).or_default().push(i as u32);
            }
        }
    }

    Ok(ParsedBoard {
        instances,
        primitive_count,
        bounds,
        spatial_grid,
        nets,
        zones: Vec::new(),
        total_pads: pad_labels.len(),
        pad_labels,
        texts,
        dimensions: Vec::new(),
        ratsnest_lines: Vec::new(),
        components,
        total_vias: 0,
        total_tracks: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_altium_coord_to_mm() {
        // Altium mil coordinates: 1000mil = 25.4mm
        let s1 = "1000mil".to_string();
        let mm = altium_coord_to_mm(Some(&s1));
        assert!((mm - 25.4).abs() < 1e-3);

        // Native mm coordinates
        let s2 = "12.5mm".to_string();
        let mm2 = altium_coord_to_mm(Some(&s2));
        assert!((mm2 - 12.5).abs() < 1e-3);

        // Raw internal DXPs (1 internal unit = 1/10000 mil = 0.00000254 mm)
        let s3 = "10000000".to_string();
        let mm3 = altium_coord_to_mm(Some(&s3));
        assert!((mm3 - 25.4).abs() < 1e-2);
    }

    #[test]
    fn test_altium_pipe_record_parsing() {
        let stream = b"|RECORD=Track|X1=1000mil|Y1=2000mil|X2=3000mil|Y2=2000mil|WIDTH=10mil|NET=GND|LAYER=TopLayer|\0";
        let recs = extract_altium_text_records(stream);
        assert_eq!(recs.len(), 1);
        let rec = &recs[0];
        assert_eq!(rec.get("RECORD").map(|s| s.as_str()), Some("Track"));
        assert_eq!(rec.get("X1").map(|s| s.as_str()), Some("1000mil"));
        assert_eq!(rec.get("NET").map(|s| s.as_str()), Some("GND"));
        assert_eq!(rec.get("LAYER").map(|s| s.as_str()), Some("TopLayer"));
    }

    #[test]
    fn test_cfb_invalid_signature_error() {
        let dummy_data = [0u8; 512];
        let result = CfbReader::new(&dummy_data);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Invalid CFB signature");
    }
}

