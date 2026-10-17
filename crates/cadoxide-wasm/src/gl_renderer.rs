use wasm_bindgen::JsCast;
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as GL, WebGlBuffer, WebGlProgram, WebGlShader,
    WebGlVertexArrayObject,
};

const BG_VS_SRC: &str = r#"#version 300 es
precision highp float;

out vec2 v_ndc;

void main() {
  vec2 quad[6] = vec2[6](
    vec2(-1.0, -1.0),
    vec2( 1.0, -1.0),
    vec2(-1.0,  1.0),
    vec2(-1.0,  1.0),
    vec2( 1.0, -1.0),
    vec2( 1.0,  1.0)
  );
  v_ndc = quad[gl_VertexID];
  gl_Position = vec4(v_ndc, 0.0, 1.0);
}
"#;

const BG_FS_SRC: &str = r#"#version 300 es
precision highp float;

uniform vec4 u_center_and_scale;        // x: centerX_mm, y: centerY_mm, z: scale_px_per_mm, w: grid_step_mm
uniform vec4 u_viewport_and_opacities;  // x: width_px, y: height_px, z: show_grid (1/0), w: flip_board_x (1/0)
uniform vec4 u_board_bounds;            // x: min_x, y: min_y, z: max_x, w: max_y
uniform vec4 u_3d_and_hover;            // x: mode_3d, y: yaw_rad, z: pitch_rad

in vec2 v_ndc;
out vec4 outColor;

void main() {
  vec3 bg = vec3(0.055, 0.062, 0.072); // Authentic KiCad Pcbnew dark canvas (#0e1012)

  float scale = u_center_and_scale.z;
  float flip_sign = u_viewport_and_opacities.w > 0.5 ? -1.0 : 1.0;
  vec2 screen_px = vec2(
    v_ndc.x * (u_viewport_and_opacities.x * 0.5) * flip_sign,
    -v_ndc.y * (u_viewport_and_opacities.y * 0.5)
  );

  vec2 world_mm;
  if (u_3d_and_hover.x > 0.05) {
    float cy = cos(u_3d_and_hover.y);
    float sy = sin(u_3d_and_hover.y);
    float cp = max(cos(u_3d_and_hover.z), 0.12);
    float rx = screen_px.x / scale;
    float ry = (screen_px.y / scale) / cp;
    vec2 rel = vec2(rx * cy + ry * sy, -rx * sy + ry * cy);
    world_mm = u_center_and_scale.xy + rel;
  } else {
    world_mm = u_center_and_scale.xy + screen_px / scale;
  }

  vec2 bmin = u_board_bounds.xy + vec2(2.0);
  vec2 bmax = u_board_bounds.zw - vec2(2.0);
  if (world_mm.x >= bmin.x && world_mm.x <= bmax.x &&
      world_mm.y >= bmin.y && world_mm.y <= bmax.y) {
    bg = vec3(0.062, 0.084, 0.072); // Dark FR4 substrate interior
  }

  if (u_viewport_and_opacities.z > 0.5) {
    // Major 10.0 mm (1 cm) engineering grid lines
    vec2 d10 = abs(fract(world_mm / 10.0 + 0.5) - 0.5) * 10.0 * scale;
    float line10 = min(d10.x, d10.y);
    if (line10 < 0.85) {
      bg = mix(bg, vec3(0.17, 0.20, 0.24), 0.55);
    }

    // 1.0 mm minor grid dots (visible when zoomed past 4 px/mm)
    if (scale > 4.0) {
      vec2 d1 = abs(fract(world_mm + 0.5) - 0.5) * scale;
      float dot1 = length(d1);
      if (dot1 < 1.15) {
        float fade = clamp((scale - 4.0) / 6.0, 0.0, 1.0);
        bg = mix(bg, vec3(0.28, 0.32, 0.38), 0.68 * fade);
      }
    }

    // Active user grid step dots when zoomed in
    float gstep = max(u_center_and_scale.w, 0.05);
    if (gstep < 0.99 && scale * gstep > 6.0) {
      vec2 dg = abs(fract(world_mm / gstep + 0.5) - 0.5) * (scale * gstep);
      float dotg = length(dg);
      if (dotg < 0.95) {
        bg = mix(bg, vec3(0.24, 0.28, 0.34), 0.45);
      }
    }
  }

  // Auxiliary board origin crosshair at bottom-left of board bounds
  vec2 orig_px = abs(world_mm - vec2(bmin.x, bmax.y)) * scale;
  if ((orig_px.x < 0.85 && orig_px.y < 14.0) || (orig_px.y < 0.85 && orig_px.x < 14.0)) {
    bg = vec3(0.62, 0.26, 0.26);
  }

  outColor = vec4(bg, 1.0);
}
"#;

const VS_SRC: &str = r#"#version 300 es
precision highp float;

layout(location = 0) in vec4 a_p1_p2;
layout(location = 1) in vec4 a_params;

uniform vec4 u_center_and_scale;        // x: centerX_mm, y: centerY_mm, z: scale_px_per_mm, w: active_layer_mask
uniform vec4 u_viewport_and_opacities;  // x: width_px, y: height_px, z: f_cu_alpha, w: b_cu_alpha
uniform vec4 u_diff_params;             // x: is_diff_view (0=normal, 1=blend, 2=wipe), y: wipe_ratio (0..1), z: silk_alpha, w: mask_alpha
uniform vec4 u_3d_and_hover;            // x: mode_3d (0=2D, >0=3D Z-explosion), y: yaw_rad, z: pitch_rad, w: hovered_instance_id (-1 = none)
uniform vec4 u_display_opts;            // x: high_contrast (0=normal,1=dim,2=hide), y: flip_board_x (0/1), z: sketch_mode (0/1), w: active_draw_layer_bit
uniform int u_selected_net_id;          // 0 = no net selected, >0 = highlight this electrical net across all layers
uniform int u_selected_inst_id;         // -1 = no primitive selected, >=0 = highlight selected primitive

out vec2 v_local_pos;
out vec2 v_seg_len_and_r;
out vec2 v_drill_and_type;
out vec4 v_color;
out float v_screen_x_ratio;
out float v_diff_flags;
out float v_is_hovered;

void main() {
  uint layer_mask = uint(a_params.z + 0.5);
  uint active_mask = uint(u_center_and_scale.w + 0.5);
  uint active_draw_bit = uint(u_display_opts.w + 0.5);

  // Bitmask hardware layer filter across all 18 KiCad layers + overlays (bits 0..22)
  if ((layer_mask & active_mask & 0x7FFFFFu) == 0u) {
    gl_Position = vec4(-2.0, -2.0, 0.0, 1.0);
    return;
  }

  // High-Contrast Mode (H): 2 = Hide Inactive Layers
  if (u_display_opts.x > 1.5 && (layer_mask & active_draw_bit) == 0u && (layer_mask & 388u) == 0u) {
    gl_Position = vec4(-2.0, -2.0, 0.0, 1.0);
    return;
  }

  vec2 p1 = a_p1_p2.xy;
  vec2 p2 = a_p1_p2.zw;
  float w = max(a_params.x, 0.06);
  float h = max(a_params.y, 0.04);

  // Decode net_id and prim_type from packed a_params.w = net_id * 16.0 + prim_type
  int net_id = int(floor(a_params.w / 16.0 + 0.001));
  float prim_type = a_params.w - float(net_id) * 16.0;
  float scale = u_center_and_scale.z;

  // OPTIMIZATION 1: Hardware Sub-Pixel LOD Culling in Vertex Stage (Instance-uniform)
  float max_dim_px = max(w, h) * scale;
  float seg_span_mm = length(p2 - p1);
  if (prim_type < 4.5 && max_dim_px < 0.35 && seg_span_mm * scale < 0.50) {
    gl_Position = vec4(-2.0, -2.0, 0.0, 1.0);
    return;
  }

  // OPTIMIZATION 2: Instance-Level Frustum Culling
  if (u_3d_and_hover.x < 0.05) {
    vec2 inst_mid_mm = 0.5 * (p1 + p2);
    float inst_rad_mm = 0.5 * seg_span_mm + max(w, h);
    vec2 mid_px = (inst_mid_mm - u_center_and_scale.xy) * scale;
    vec2 rad_px = vec2(inst_rad_mm * scale);
    vec2 half_vp = u_viewport_and_opacities.xy * 0.5;
    if (abs(mid_px.x) - rad_px.x > half_vp.x * 1.15 ||
        abs(mid_px.y) - rad_px.y > half_vp.y * 1.15) {
      gl_Position = vec4(-2.0, -2.0, 0.0, 1.0);
      return;
    }
  }

  vec2 quad[6] = vec2[6](
    vec2(-1.0, -1.0),
    vec2( 1.0, -1.0),
    vec2(-1.0,  1.0),
    vec2(-1.0,  1.0),
    vec2( 1.0, -1.0),
    vec2( 1.0,  1.0)
  );
  vec2 uv = quad[gl_VertexID];

  vec2 world_mm;
  float seg_len = 0.0;
  if (prim_type < 0.5) {
    // Capsule Track / Silkscreen Line / Courtyard / Edge.Cuts Line
    vec2 delta = p2 - p1;
    seg_len = length(delta);
    vec2 dir = seg_len > 0.0001 ? (delta / seg_len) : vec2(1.0, 0.0);
    vec2 norm = vec2(-dir.y, dir.x);
    float r = w * 0.5;
    float along = uv.x > 0.0 ? (seg_len + r) : -r;
    float perp = uv.y * r;
    world_mm = p1 + dir * along + norm * perp;
    v_local_pos = vec2(along, perp);
    v_seg_len_and_r = vec2(seg_len, r);
  } else if (prim_type > 1.5 && prim_type < 3.5) {
    // SMD Pad (2.0 = Rect/Roundrect, 3.0 = Circle/Oval): w = width, h = height
    float rx = w * 0.5;
    float ry = h * 0.5;
    world_mm = p1 + vec2(uv.x * rx, uv.y * ry);
    v_local_pos = vec2(uv.x * rx, uv.y * ry);
    v_seg_len_and_r = vec2(rx, ry);
  } else {
    // Vias (1.0), Rect PTH Pads (4.0), Markers (5.0..7.0), Circles (8.0)
    float rx = w * 0.5;
    float ry = w * 0.5;
    world_mm = p1 + vec2(uv.x * rx, uv.y * ry);
    v_local_pos = vec2(uv.x * rx, uv.y * ry);
    v_seg_len_and_r = vec2(rx, ry);
  }

  v_drill_and_type = vec2(a_params.y * 0.5, prim_type);
  if (u_selected_inst_id >= 0 && u_selected_inst_id == gl_InstanceID) {
    v_is_hovered = 3.0;
  } else if (int(u_3d_and_hover.w) == gl_InstanceID) {
    v_is_hovered = 1.0;
  } else {
    v_is_hovered = 0.0;
  }

  // Check diff flags: bit 9 (512)=unchanged, bit 10 (1024)=removed (only in A), bit 11 (2048)=added (only in B)
  if (u_diff_params.x > 0.5) {
    if ((layer_mask & 2048u) != 0u) {
      v_diff_flags = 2.0; // Added in Rev B
      v_color = vec4(0.18, 0.80, 0.44, 0.94);
    } else if ((layer_mask & 1024u) != 0u) {
      v_diff_flags = 1.0; // Removed from Rev A
      v_color = vec4(0.91, 0.25, 0.25, 0.94);
    } else {
      v_diff_flags = 0.0; // Unchanged
      v_color = vec4(0.28, 0.32, 0.38, 0.52);
    }
  } else {
    v_diff_flags = 0.0;
    if ((layer_mask & 256u) != 0u) {
      // Review Annotation / CAD Preview Overlay
      v_color = prim_type > 6.5 ? vec4(0.16, 0.74, 0.48, 0.96) : vec4(0.96, 0.78, 0.22, 0.96);
    } else if ((layer_mask & 128u) != 0u) {
      // DRC Violation Ring
      v_color = vec4(0.96, 0.62, 0.08, 0.96);
    } else if ((layer_mask & 4u) != 0u) {
      // Edge.Cuts (#D8C238)
      v_color = vec4(0.85, 0.76, 0.22, 0.98);
    } else if ((layer_mask & 4194304u) != 0u) {
      // Margin (#D838B8)
      v_color = vec4(0.85, 0.22, 0.72, 0.92);
    } else if ((layer_mask & 65536u) != 0u && (active_mask & 65536u) != 0u) {
      // F.Courtyard (#E038D8 KiCad Magenta)
      v_color = vec4(0.88, 0.22, 0.85, 0.90);
    } else if ((layer_mask & 131072u) != 0u && (active_mask & 131072u) != 0u) {
      // B.Courtyard (#28C8E0 KiCad Cyan)
      v_color = vec4(0.16, 0.78, 0.88, 0.90);
    } else if ((layer_mask & 262144u) != 0u && (active_mask & 262144u) != 0u) {
      // F.Fab (#888E96)
      v_color = vec4(0.54, 0.56, 0.59, 0.84);
    } else if ((layer_mask & 524288u) != 0u && (active_mask & 524288u) != 0u) {
      // B.Fab (#586278)
      v_color = vec4(0.35, 0.39, 0.47, 0.84);
    } else if ((layer_mask & 1048576u) != 0u && (active_mask & 1048576u) != 0u) {
      // User.Drawings (#94A2B0)
      v_color = vec4(0.58, 0.64, 0.69, 0.88);
    } else if ((layer_mask & 2097152u) != 0u && (active_mask & 2097152u) != 0u) {
      // User.Comments (#5C94D8)
      v_color = vec4(0.36, 0.58, 0.85, 0.88);
    } else if ((prim_type > 0.5 && prim_type < 1.5) || (prim_type > 3.5 && prim_type < 4.5)) {
      // Plated Through-Hole Via / PTH Pad (#C8B43C KiCad Gold/Tan)
      v_color = vec4(0.80, 0.70, 0.32, 0.96);
    } else if ((layer_mask & 8u) != 0u && (active_mask & 8u) != 0u && (prim_type < 1.5 || prim_type > 7.5)) {
      // F.Silkscreen (#F2EDA1 KiCad Warm Cream)
      v_color = vec4(0.95, 0.93, 0.63, u_diff_params.z);
    } else if ((layer_mask & 16u) != 0u && (active_mask & 16u) != 0u && (prim_type < 1.5 || prim_type > 7.5)) {
      // B.Silkscreen (#E8B2A6 KiCad Rose)
      v_color = vec4(0.91, 0.70, 0.65, u_diff_params.z);
    } else if ((layer_mask & 1u) != 0u && (active_mask & 1u) != 0u) {
      // KiCad F.Cu (#C83434 Front Copper Red)
      v_color = vec4(0.78, 0.20, 0.20, u_viewport_and_opacities.z);
    } else if ((layer_mask & 4096u) != 0u && (active_mask & 4096u) != 0u) {
      // KiCad In1.Cu (#C2B238 Inner 1 Gold-Olive)
      v_color = vec4(0.76, 0.70, 0.22, 0.88);
    } else if ((layer_mask & 8192u) != 0u && (active_mask & 8192u) != 0u) {
      // KiCad In2.Cu (#D47B28 Inner 2 Orange-Bronze)
      v_color = vec4(0.83, 0.48, 0.16, 0.88);
    } else if ((layer_mask & 2u) != 0u && (active_mask & 2u) != 0u) {
      // KiCad B.Cu (#2878C8 Back Copper Blue)
      v_color = vec4(0.16, 0.47, 0.78, u_viewport_and_opacities.w);
    } else if ((layer_mask & 16384u) != 0u && (active_mask & 16384u) != 0u) {
      // F.Paste (#9E8282)
      v_color = vec4(0.62, 0.51, 0.51, 0.72);
    } else if ((layer_mask & 32768u) != 0u && (active_mask & 32768u) != 0u) {
      // B.Paste (#6E9E9E)
      v_color = vec4(0.43, 0.62, 0.62, 0.72);
    } else if ((layer_mask & 32u) != 0u && (active_mask & 32u) != 0u) {
      // F.Mask (#6B285C KiCad Purple-Magenta Solder Mask)
      v_color = vec4(0.42, 0.16, 0.36, u_diff_params.w);
    } else if ((layer_mask & 64u) != 0u && (active_mask & 64u) != 0u) {
      // B.Mask (#1E6E64 KiCad Teal Solder Mask)
      v_color = vec4(0.12, 0.43, 0.39, u_diff_params.w);
    } else {
      v_color = vec4(0.75, 0.78, 0.82, 0.80);
    }

    // High-Contrast Mode (H): 1 = Dim Inactive Layers
    if (u_display_opts.x > 0.5 && u_display_opts.x < 1.5) {
      if ((layer_mask & active_draw_bit) == 0u && (layer_mask & 388u) == 0u) {
        v_color = vec4(v_color.rgb * 0.35, v_color.a * 0.22);
      }
    }

    // GPU Netlist Highlighting
    if (u_selected_net_id > 0) {
      if (net_id == u_selected_net_id) {
        if (v_is_hovered < 2.5) v_is_hovered = 2.0;
      } else if ((layer_mask & 388u) == 0u && v_is_hovered < 2.5) {
        v_color = vec4(v_color.rgb * 0.30, v_color.a * 0.15);
      }
    }
  }

  vec2 rel = world_mm - u_center_and_scale.xy;

  // OPTIMIZATION 3: Hardware 3D Multi-Layer Stackup Isometric/Orbit Projection
  if (u_3d_and_hover.x > 0.05) {
    float z_layer = 0.0;
    if ((layer_mask & 256u) != 0u || (layer_mask & 128u) != 0u) {
      z_layer = 7.5;
    } else if ((layer_mask & 65536u) != 0u || (layer_mask & 262144u) != 0u) {
      z_layer = 6.2;  // F.CrtYd / F.Fab
    } else if ((layer_mask & 8u) != 0u) {
      z_layer = 5.5;  // F.SilkS
    } else if ((layer_mask & 32u) != 0u || (layer_mask & 16384u) != 0u) {
      z_layer = 3.8;  // F.Mask / F.Paste
    } else if ((layer_mask & 1u) != 0u) {
      z_layer = 2.2;  // F.Cu
    } else if ((layer_mask & 4096u) != 0u) {
      z_layer = 0.8;  // In1.Cu
    } else if ((layer_mask & 8192u) != 0u) {
      z_layer = -0.8; // In2.Cu
    } else if ((layer_mask & 4u) != 0u) {
      z_layer = 0.0;  // FR4 Core / Edge.Cuts
    } else if ((layer_mask & 2u) != 0u) {
      z_layer = -2.2; // B.Cu
    } else if ((layer_mask & 64u) != 0u || (layer_mask & 32768u) != 0u) {
      z_layer = -3.8; // B.Mask / B.Paste
    } else if ((layer_mask & 16u) != 0u) {
      z_layer = -5.5; // B.SilkS
    } else if ((layer_mask & 131072u) != 0u || (layer_mask & 524288u) != 0u) {
      z_layer = -6.2; // B.CrtYd / B.Fab
    }
    z_layer *= u_3d_and_hover.x;

    float cy = cos(u_3d_and_hover.y);
    float sy = sin(u_3d_and_hover.y);
    float cp = cos(u_3d_and_hover.z);
    float sp = sin(u_3d_and_hover.z);

    float rx = rel.x * cy - rel.y * sy;
    float ry = rel.x * sy + rel.y * cy;
    float proj_y = ry * cp - z_layer * sp;
    rel = vec2(rx, proj_y);
  }

  float flip_sign = u_display_opts.y > 0.5 ? -1.0 : 1.0;
  vec2 screen_px = vec2(rel.x * scale * flip_sign, rel.y * scale);
  vec2 ndc = vec2(
    screen_px.x / (u_viewport_and_opacities.x * 0.5),
    -screen_px.y / (u_viewport_and_opacities.y * 0.5)
  );

  v_screen_x_ratio = ndc.x * 0.5 + 0.5;
  gl_Position = vec4(ndc, 0.0, 1.0);
}
"#;

const FS_SRC: &str = r#"#version 300 es
precision highp float;

uniform vec4 u_diff_params;
uniform vec4 u_display_opts; // z: sketch_mode (0/1)

in vec2 v_local_pos;
in vec2 v_seg_len_and_r;
in vec2 v_drill_and_type;
in vec4 v_color;
in float v_screen_x_ratio;
in float v_diff_flags;
in float v_is_hovered;

out vec4 outColor;

float sdRoundedBox(vec2 p, vec2 b, float r) {
  vec2 q = abs(p) - b + r;
  return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
}

void main() {
  if (u_diff_params.x > 1.5) {
    float wipe = u_diff_params.y;
    if (v_screen_x_ratio < wipe) {
      if (v_diff_flags > 1.5) discard;
      outColor = vec4(0.78, 0.20, 0.20, 0.92);
    } else {
      if (v_diff_flags > 0.5 && v_diff_flags < 1.5) discard;
      outColor = vec4(0.16, 0.74, 0.48, 0.92);
    }
    if (abs(v_screen_x_ratio - wipe) < 0.0015) {
      outColor = vec4(0.22, 0.78, 0.95, 1.0);
      return;
    }
  } else {
    outColor = v_color;
  }

  float prim_type = v_drill_and_type.y;
  float drill_r = v_drill_and_type.x;
  bool sketch = u_display_opts.z > 0.5;

  if (prim_type < 0.5) {
    // 0.0: Capsule Track / Silkscreen Line
    float clamped_x = clamp(v_local_pos.x, 0.0, v_seg_len_and_r.x);
    float dist = length(vec2(v_local_pos.x - clamped_x, v_local_pos.y));
    float r = v_seg_len_and_r.y;
    if (dist > r) discard;
    if (sketch && r > 0.09 && dist < r * 0.72) discard;
  } else if (prim_type < 1.5) {
    // 1.0: Plated Via or Circular/Oval PTH Pad with Dark Plated Drill Hole
    float dist = length(v_local_pos);
    float outer_r = v_seg_len_and_r.x;
    if (dist > outer_r) discard;
    if (dist < drill_r * 0.86) {
      outColor = vec4(0.038, 0.045, 0.062, 1.0);
      return;
    } else if (dist < drill_r * 1.06) {
      outColor = vec4(0.55, 0.44, 0.18, 1.0);
    } else if (sketch && dist < outer_r * 0.82) {
      outColor.a *= 0.22;
    }
  } else if (prim_type < 2.5) {
    // 2.0: Rounded-Rectangle SMD Pad
    vec2 half_ext = v_seg_len_and_r;
    float corner_r = min(half_ext.x, half_ext.y) * 0.22;
    float d = sdRoundedBox(v_local_pos, half_ext, corner_r);
    if (d > 0.0) discard;
    if (sketch && d < -min(half_ext.x, half_ext.y) * 0.22) {
      outColor.a *= 0.18;
    }
  } else if (prim_type < 3.5) {
    // 3.0: Circular / Oval SMD Pad
    vec2 norm_p = v_local_pos / max(v_seg_len_and_r, vec2(0.01));
    float l = length(norm_p);
    if (l > 1.0) discard;
    if (sketch && l < 0.78) {
      outColor.a *= 0.18;
    }
  } else if (prim_type < 4.5) {
    // 4.0: Square / Rectangular Plated Through-Hole Pad (Pin 1) with Circular Drill Hole
    vec2 half_ext = v_seg_len_and_r;
    float corner_r = min(half_ext.x, half_ext.y) * 0.14;
    float d = sdRoundedBox(v_local_pos, half_ext, corner_r);
    if (d > 0.0) discard;
    float dist = length(v_local_pos);
    if (dist < drill_r * 0.86) {
      outColor = vec4(0.038, 0.045, 0.062, 1.0);
      return;
    } else if (dist < drill_r * 1.06) {
      outColor = vec4(0.55, 0.44, 0.18, 1.0);
    }
  } else if (prim_type < 7.5) {
    // 5.0..7.0: DRC Marker Ring / Review Annotation Target
    float dist = length(v_local_pos);
    float outer_r = v_seg_len_and_r.x;
    if (dist > outer_r) discard;
    if (prim_type < 5.5) {
      if (dist < outer_r * 0.55) discard;
    } else {
      if (dist > outer_r * 0.76) {
        outColor = vec4(0.95, 0.97, 1.0, 1.0);
      }
    }
  } else {
    // 8.0: SDF Circle Outline (fp_circle / gr_circle)
    float dist = length(v_local_pos);
    float outer_r = v_seg_len_and_r.x;
    float inner_r = drill_r;
    if (dist > outer_r || dist < inner_r) discard;
  }

  // Selected Primitive Highlight (v_is_hovered > 2.5), Net Highlight (v_is_hovered > 1.5), or Cursor Hover (v_is_hovered > 0.5)
  if (v_is_hovered > 2.5) {
    outColor = vec4(mix(outColor.rgb, vec3(1.0, 0.92, 0.38), 0.85), 1.0);
  } else if (v_is_hovered > 1.5) {
    outColor = vec4(mix(outColor.rgb, vec3(0.20, 0.98, 0.72), 0.82), 1.0);
  } else if (v_is_hovered > 0.5) {
    outColor = vec4(mix(outColor.rgb, vec3(0.35, 0.92, 1.0), 0.75), 1.0);
  }
}
"#;

pub struct RustGpuRenderer {
    canvas: HtmlCanvasElement,
    gl: GL,
    bg_program: WebGlProgram,
    program: WebGlProgram,
    vao: WebGlVertexArrayObject,
    instance_buf: WebGlBuffer,
    pub instance_count: i32,
    pub center_mm: (f32, f32),
    pub scale_px_per_mm: f32,
    pub bounds: [f32; 4],
    pub active_layer_mask: u32,
    pub opacities: [f32; 4], // [F.Cu, B.Cu, SilkS, Mask]
    pub diff_mode: f32,      // 0.0 = normal PCB, 1.0 = boolean diff, 2.0 = split wipe
    pub wipe_ratio: f32,     // 0.0 .. 1.0
    pub mode_3d_explode: f32, // 0.0 = 2D Ortho, >0.0 = 3D Stackup Explosion factor
    pub yaw_rad: f32,
    pub pitch_rad: f32,
    pub hovered_instance_id: i32,
    pub selected_net_id: i32,
    pub selected_instance_id: i32,
    pub high_contrast_mode: u8, // 0 = Normal, 1 = Dim Inactive, 2 = Hide Inactive
    pub flip_board_view: bool,
    pub sketch_mode: bool,
    pub show_grid: bool,
    pub grid_step_mm: f32,
    pub active_draw_layer_bit: u32,
}

impl RustGpuRenderer {
    pub fn new(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let gl = canvas
            .get_context("webgl2")
            .map_err(|_| "Failed to get webgl2 context")?
            .ok_or("WebGL2 not supported")?
            .dyn_into::<GL>()
            .map_err(|_| "Failed to cast to WebGl2RenderingContext")?;

        let bg_vs = compile_shader(&gl, GL::VERTEX_SHADER, BG_VS_SRC)?;
        let bg_fs = compile_shader(&gl, GL::FRAGMENT_SHADER, BG_FS_SRC)?;
        let bg_program = link_program(&gl, &bg_vs, &bg_fs)?;

        let vs = compile_shader(&gl, GL::VERTEX_SHADER, VS_SRC)?;
        let fs = compile_shader(&gl, GL::FRAGMENT_SHADER, FS_SRC)?;
        let program = link_program(&gl, &vs, &fs)?;

        let vao = gl
            .create_vertex_array()
            .ok_or("Failed to create WebGL2 VAO")?;
        gl.bind_vertex_array(Some(&vao));

        let instance_buf = gl.create_buffer().ok_or("Failed to create WebGL2 Buffer")?;
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&instance_buf));

        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 4, GL::FLOAT, false, 32, 0);
        gl.vertex_attrib_divisor(0, 1);

        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_with_i32(1, 4, GL::FLOAT, false, 32, 16);
        gl.vertex_attrib_divisor(1, 1);

        gl.bind_vertex_array(None);

        Ok(Self {
            canvas,
            gl,
            bg_program,
            program,
            vao,
            instance_buf,
            instance_count: 0,
            center_mm: (130.0, 95.0),
            scale_px_per_mm: 8.0,
            bounds: [70.0, 40.0, 190.0, 150.0],
            active_layer_mask: 0x7FFFFF,
            opacities: [0.92, 0.78, 0.90, 0.45],
            diff_mode: 0.0,
            wipe_ratio: 0.5,
            mode_3d_explode: 0.0,
            yaw_rad: -0.45,
            pitch_rad: 0.62,
            hovered_instance_id: -1,
            selected_net_id: 0,
            selected_instance_id: -1,
            high_contrast_mode: 0,
            flip_board_view: false,
            sketch_mode: false,
            show_grid: true,
            grid_step_mm: 0.5,
            active_draw_layer_bit: 1,
        })
    }

    pub fn resize_to_parent(&mut self) {
        if let Some(parent) = self.canvas.parent_element() {
            let rect = parent.get_bounding_client_rect();
            let dpr = web_sys::window()
                .map(|w| w.device_pixel_ratio())
                .unwrap_or(1.0);
            let w = (rect.width() * dpr).max(300.0) as u32;
            let h = (rect.height() * dpr).max(200.0) as u32;
            self.canvas.set_width(w);
            self.canvas.set_height(h);
        }
    }

    /// Zero-Copy upload directly from Rust's Vec<f32> slice into GPU VRAM
    pub fn upload_instances(&mut self, instances: &[f32], bounds: [f32; 4]) {
        self.bounds = bounds;
        self.instance_count = (instances.len() / 8) as i32;
        self.gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.instance_buf));
        unsafe {
            let view = js_sys::Float32Array::view(instances);
            self.gl
                .buffer_data_with_array_buffer_view(GL::ARRAY_BUFFER, &view, GL::DYNAMIC_DRAW);
        }
    }

    pub fn fit_to_board(&mut self) {
        self.resize_to_parent();
        let [min_x, min_y, max_x, max_y] = self.bounds;
        self.center_mm = ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5);
        let w_mm = (max_x - min_x).max(10.0);
        let h_mm = (max_y - min_y).max(10.0);
        let cw = self.canvas.width() as f32;
        let ch = self.canvas.height() as f32;
        self.scale_px_per_mm = ((cw * 0.84) / w_mm).min((ch * 0.84) / h_mm).max(2.0);
    }

    pub fn screen_to_board_mm(&self, client_x: f64, client_y: f64) -> (f32, f32) {
        let rect = self.canvas.get_bounding_client_rect();
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .unwrap_or(1.0) as f32;
        let flip_sign = if self.flip_board_view { -1.0 } else { 1.0 };
        let px_x = ((client_x - rect.left()) - rect.width() * 0.5) as f32 * dpr * flip_sign;
        let px_y = ((client_y - rect.top()) - rect.height() * 0.5) as f32 * dpr;
        let rx = px_x / self.scale_px_per_mm;
        let sy_mm = px_y / self.scale_px_per_mm;
        if self.mode_3d_explode > 0.05 {
            let cy = self.yaw_rad.cos();
            let sy = self.yaw_rad.sin();
            let cp = self.pitch_rad.cos().max(0.12);
            let ry = sy_mm / cp;
            let rel_x = rx * cy + ry * sy;
            let rel_y = -rx * sy + ry * cy;
            (self.center_mm.0 + rel_x, self.center_mm.1 + rel_y)
        } else {
            (self.center_mm.0 + rx, self.center_mm.1 + sy_mm)
        }
    }

    /// Projects board (mm_x, mm_y, z_mm) into device pixel coordinates on the canvas
    pub fn board_mm_to_device_px(&self, mm_x: f32, mm_y: f32, z_layer: f32) -> (f64, f64) {
        let cw = self.canvas.width() as f32;
        let ch = self.canvas.height() as f32;
        let mut rel_x = mm_x - self.center_mm.0;
        let mut rel_y = mm_y - self.center_mm.1;
        if self.mode_3d_explode > 0.05 {
            let z = z_layer * self.mode_3d_explode;
            let cy = self.yaw_rad.cos();
            let sy = self.yaw_rad.sin();
            let cp = self.pitch_rad.cos();
            let sp = self.pitch_rad.sin();
            let rx = rel_x * cy - rel_y * sy;
            let ry = rel_x * sy + rel_y * cy;
            rel_x = rx;
            rel_y = ry * cp - z * sp;
        }
        let flip_sign = if self.flip_board_view { -1.0 } else { 1.0 };
        let px_x = cw * 0.5 + rel_x * self.scale_px_per_mm * flip_sign;
        let px_y = ch * 0.5 + rel_y * self.scale_px_per_mm;
        (px_x as f64, px_y as f64)
    }

    pub fn zoom_at_cursor(&mut self, factor: f32, client_x: f64, client_y: f64) {
        let before = self.screen_to_board_mm(client_x, client_y);
        self.scale_px_per_mm = (self.scale_px_per_mm * factor).clamp(1.5, 500.0);
        let after = self.screen_to_board_mm(client_x, client_y);
        self.center_mm.0 += before.0 - after.0;
        self.center_mm.1 += before.1 - after.1;
    }

    pub fn pan_by_css_px(&mut self, dx: f64, dy: f64) {
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .unwrap_or(1.0) as f32;
        let flip_sign = if self.flip_board_view { -1.0 } else { 1.0 };
        let dx_mm = (dx as f32 * dpr * flip_sign) / self.scale_px_per_mm;
        let dy_mm = (dy as f32 * dpr) / self.scale_px_per_mm;
        if self.mode_3d_explode > 0.05 {
            let cy = self.yaw_rad.cos();
            let sy = self.yaw_rad.sin();
            let cp = self.pitch_rad.cos().max(0.12);
            let rx = -dx_mm;
            let ry = -dy_mm / cp;
            self.center_mm.0 += rx * cy + ry * sy;
            self.center_mm.1 += -rx * sy + ry * cy;
        } else {
            self.center_mm.0 -= dx_mm;
            self.center_mm.1 -= dy_mm;
        }
    }

    pub fn orbit_by_css_px(&mut self, dx: f64, dy: f64) {
        self.yaw_rad += (dx as f32) * 0.0085;
        self.pitch_rad = (self.pitch_rad + (dy as f32) * 0.0085).clamp(0.0, 1.38);
    }

    pub fn draw(&mut self) {
        self.resize_to_parent();
        let w = self.canvas.width() as i32;
        let h = self.canvas.height() as i32;

        self.gl.viewport(0, 0, w, h);
        self.gl.clear_color(0.055, 0.062, 0.072, 1.0);
        self.gl.clear(GL::COLOR_BUFFER_BIT);

        self.gl.bind_vertex_array(Some(&self.vao));

        // 1. Render CAD Metric Grid & FR4 Board Substrate Pass
        self.gl.disable(GL::BLEND);
        self.gl.use_program(Some(&self.bg_program));
        if let Some(u1) = self.gl.get_uniform_location(&self.bg_program, "u_center_and_scale") {
            self.gl.uniform4f(
                Some(&u1),
                self.center_mm.0,
                self.center_mm.1,
                self.scale_px_per_mm,
                self.grid_step_mm,
            );
        }
        if let Some(u2) = self
            .gl
            .get_uniform_location(&self.bg_program, "u_viewport_and_opacities")
        {
            self.gl.uniform4f(
                Some(&u2),
                w as f32,
                h as f32,
                if self.show_grid { 1.0 } else { 0.0 },
                if self.flip_board_view { 1.0 } else { 0.0 },
            );
        }
        if let Some(ub) = self.gl.get_uniform_location(&self.bg_program, "u_board_bounds") {
            self.gl.uniform4f(
                Some(&ub),
                self.bounds[0],
                self.bounds[1],
                self.bounds[2],
                self.bounds[3],
            );
        }
        if let Some(u4) = self.gl.get_uniform_location(&self.bg_program, "u_3d_and_hover") {
            self.gl.uniform4f(
                Some(&u4),
                self.mode_3d_explode,
                self.yaw_rad,
                self.pitch_rad,
                self.hovered_instance_id as f32,
            );
        }
        self.gl.draw_arrays(GL::TRIANGLES, 0, 6);

        if self.instance_count <= 0 {
            self.gl.bind_vertex_array(None);
            return;
        }

        // 2. Render Instanced SDF PCB Primitives Pass
        self.gl.enable(GL::BLEND);
        self.gl.blend_func(GL::SRC_ALPHA, GL::ONE_MINUS_SRC_ALPHA);
        self.gl.use_program(Some(&self.program));

        if let Some(u1) = self
            .gl
            .get_uniform_location(&self.program, "u_center_and_scale")
        {
            self.gl.uniform4f(
                Some(&u1),
                self.center_mm.0,
                self.center_mm.1,
                self.scale_px_per_mm,
                self.active_layer_mask as f32,
            );
        }

        if let Some(u2) = self
            .gl
            .get_uniform_location(&self.program, "u_viewport_and_opacities")
        {
            self.gl.uniform4f(
                Some(&u2),
                w as f32,
                h as f32,
                self.opacities[0],
                self.opacities[1],
            );
        }

        if let Some(u3) = self.gl.get_uniform_location(&self.program, "u_diff_params") {
            self.gl.uniform4f(
                Some(&u3),
                self.diff_mode,
                self.wipe_ratio,
                self.opacities[2],
                self.opacities[3],
            );
        }

        if let Some(u4) = self.gl.get_uniform_location(&self.program, "u_3d_and_hover") {
            self.gl.uniform4f(
                Some(&u4),
                self.mode_3d_explode,
                self.yaw_rad,
                self.pitch_rad,
                self.hovered_instance_id as f32,
            );
        }

        if let Some(ud) = self.gl.get_uniform_location(&self.program, "u_display_opts") {
            self.gl.uniform4f(
                Some(&ud),
                self.high_contrast_mode as f32,
                if self.flip_board_view { 1.0 } else { 0.0 },
                if self.sketch_mode { 1.0 } else { 0.0 },
                self.active_draw_layer_bit as f32,
            );
        }

        if let Some(u5) = self
            .gl
            .get_uniform_location(&self.program, "u_selected_net_id")
        {
            self.gl.uniform1i(Some(&u5), self.selected_net_id);
        }

        if let Some(u6) = self
            .gl
            .get_uniform_location(&self.program, "u_selected_inst_id")
        {
            self.gl.uniform1i(Some(&u6), self.selected_instance_id);
        }

        self.gl
            .draw_arrays_instanced(GL::TRIANGLES, 0, 6, self.instance_count);
        self.gl.bind_vertex_array(None);
    }
}

fn compile_shader(gl: &GL, shader_type: u32, src: &str) -> Result<WebGlShader, String> {
    let shader = gl
        .create_shader(shader_type)
        .ok_or("Unable to create shader")?;
    gl.shader_source(&shader, src);
    gl.compile_shader(&shader);
    if gl
        .get_shader_parameter(&shader, GL::COMPILE_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Ok(shader)
    } else {
        Err(gl
            .get_shader_info_log(&shader)
            .unwrap_or_else(|| "Unknown shader error".into()))
    }
}

fn link_program(gl: &GL, vs: &WebGlShader, fs: &WebGlShader) -> Result<WebGlProgram, String> {
    let program = gl.create_program().ok_or("Unable to create GL program")?;
    gl.attach_shader(&program, vs);
    gl.attach_shader(&program, fs);
    gl.link_program(&program);
    if gl
        .get_program_parameter(&program, GL::LINK_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Ok(program)
    } else {
        Err(gl
            .get_program_info_log(&program)
            .unwrap_or_else(|| "Unknown program link error".into()))
    }
}
