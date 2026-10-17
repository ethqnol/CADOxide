use wasm_bindgen::JsCast;
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as GL, WebGlBuffer, WebGlProgram, WebGlShader,
    WebGlVertexArrayObject,
};

const VS_SRC: &str = r#"#version 300 es
precision highp float;

layout(location = 0) in vec4 a_p1_p2;
layout(location = 1) in vec4 a_params;

uniform vec4 u_center_and_scale;        // x: centerX_mm, y: centerY_mm, z: scale_px_per_mm, w: active_layer_mask
uniform vec4 u_viewport_and_opacities;  // x: width_px, y: height_px, z: f_cu_alpha, w: b_cu_alpha
uniform vec4 u_diff_params;             // x: is_diff_view (0=normal, 1=blend, 2=wipe), y: wipe_ratio (0..1), z: silk_alpha, w: mask_alpha

out vec2 v_local_pos;
out vec2 v_seg_len_and_r;
out vec2 v_drill_and_type;
out vec4 v_color;
out float v_screen_x_ratio;
out float v_diff_flags;

void main() {
  uint layer_mask = uint(a_params.z + 0.5);
  uint active_mask = uint(u_center_and_scale.w + 0.5);

  if ((layer_mask & active_mask & 511u) == 0u) {
    gl_Position = vec4(-2.0, -2.0, 0.0, 1.0);
    return;
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

  vec2 p1 = a_p1_p2.xy;
  vec2 p2 = a_p1_p2.zw;
  float w = max(a_params.x, 0.08);
  float h = max(a_params.y, 0.08);
  float prim_type = a_params.w;

  vec2 world_mm;
  if (prim_type < 0.5) {
    vec2 delta = p2 - p1;
    float len = length(delta);
    vec2 dir = len > 0.0001 ? (delta / len) : vec2(1.0, 0.0);
    vec2 norm = vec2(-dir.y, dir.x);
    float r = w * 0.5;
    float along = uv.x > 0.0 ? (len + r) : -r;
    float perp = uv.y * r;
    world_mm = p1 + dir * along + norm * perp;
    v_local_pos = vec2(along, perp);
    v_seg_len_and_r = vec2(len, r);
  } else {
    float rx = w * 0.5;
    float ry = prim_type > 1.5 ? (h * 0.5) : (w * 0.5);
    world_mm = p1 + vec2(uv.x * rx, uv.y * ry);
    v_local_pos = vec2(uv.x * rx, uv.y * ry);
    v_seg_len_and_r = vec2(rx, ry);
  }

  v_drill_and_type = vec2(a_params.y * 0.5, prim_type);

  // Check diff flags: bit 9 (512)=unchanged, bit 10 (1024)=removed (only in A), bit 11 (2048)=added (only in B)
  if (u_diff_params.x > 0.5) {
    if ((layer_mask & 2048u) != 0u) {
      v_diff_flags = 2.0; // Added in Rev B
      v_color = vec4(0.13, 0.77, 0.37, 0.95); // Green (#22c55e)
    } else if ((layer_mask & 1024u) != 0u) {
      v_diff_flags = 1.0; // Removed from Rev A
      v_color = vec4(0.94, 0.27, 0.27, 0.95); // Red (#ef4444)
    } else {
      v_diff_flags = 0.0; // Unchanged
      v_color = vec4(0.29, 0.33, 0.41, 0.65); // Muted Slate (#4A5568)
    }
  } else {
    v_diff_flags = 0.0;
    if ((layer_mask & 256u) != 0u) {
      // Review Pin Marker
      v_color = prim_type > 2.5 ? vec4(0.06, 0.73, 0.51, 1.0) : vec4(0.88, 0.11, 0.28, 1.0);
    } else if ((layer_mask & 128u) != 0u) {
      // DRC Violation Marker
      v_color = vec4(0.96, 0.62, 0.04, 0.95);
    } else if ((layer_mask & 4u) != 0u) {
      v_color = vec4(0.97, 0.98, 0.99, 0.95); // Edge.Cuts
    } else if (prim_type > 0.5 && prim_type < 1.5) {
      v_color = vec4(0.92, 0.78, 0.35, 0.95); // Via gold ring
    } else if ((layer_mask & 1u) != 0u && (active_mask & 1u) != 0u) {
      v_color = vec4(0.96, 0.62, 0.04, u_viewport_and_opacities.z); // F.Cu
    } else if ((layer_mask & 2u) != 0u && (active_mask & 2u) != 0u) {
      v_color = vec4(0.23, 0.51, 0.96, u_viewport_and_opacities.w); // B.Cu
    } else if ((layer_mask & 8u) != 0u || (layer_mask & 16u) != 0u) {
      v_color = vec4(0.94, 0.96, 0.98, u_diff_params.z); // SilkS
    } else {
      v_color = vec4(0.04, 0.59, 0.41, u_diff_params.w); // Mask
    }
  }

  vec2 screen_px = (world_mm - u_center_and_scale.xy) * u_center_and_scale.z;
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

in vec2 v_local_pos;
in vec2 v_seg_len_and_r;
in vec2 v_drill_and_type;
in vec4 v_color;
in float v_screen_x_ratio;
in float v_diff_flags;

out vec4 outColor;

void main() {
  // Split-Wipe Mode (u_diff_params.x == 2.0):
  // Left of wipe_ratio displays Revision A (unchanged + removed), Right displays Revision B (unchanged + added)
  if (u_diff_params.x > 1.5) {
    float wipe = u_diff_params.y;
    if (v_screen_x_ratio < wipe) {
      if (v_diff_flags > 1.5) discard; // Hide Rev B-only on left side
      outColor = vec4(0.96, 0.45, 0.15, 0.92); // Rev A Copper
    } else {
      if (v_diff_flags > 0.5 && v_diff_flags < 1.5) discard; // Hide Rev A-only on right side
      outColor = vec4(0.13, 0.77, 0.55, 0.92); // Rev B Copper
    }
    if (abs(v_screen_x_ratio - wipe) < 0.002) {
      outColor = vec4(0.13, 0.83, 0.93, 1.0); // Cyan Split Line
      return;
    }
  } else {
    outColor = v_color;
  }

  float prim_type = v_drill_and_type.y;
  if (prim_type < 0.5) {
    float clamped_x = clamp(v_local_pos.x, 0.0, v_seg_len_and_r.x);
    float dist = length(vec2(v_local_pos.x - clamped_x, v_local_pos.y));
    if (dist > v_seg_len_and_r.y) discard;
  } else if (prim_type < 1.5) {
    float dist = length(v_local_pos);
    if (dist > v_seg_len_and_r.x || dist < v_drill_and_type.x) discard;
  } else if (prim_type > 2.2) {
    // Circular Pin / DRC Marker with white border ring
    float dist = length(v_local_pos);
    if (dist > v_seg_len_and_r.x) discard;
    if (dist > v_seg_len_and_r.x * 0.72) {
      outColor = vec4(1.0, 1.0, 1.0, 1.0);
    }
  }
}
"#;

pub struct RustGpuRenderer {
    canvas: HtmlCanvasElement,
    gl: GL,
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
}

impl RustGpuRenderer {
    pub fn new(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let gl = canvas
            .get_context("webgl2")
            .map_err(|_| "Failed to get webgl2 context")?
            .ok_or("WebGL2 not supported")?
            .dyn_into::<GL>()
            .map_err(|_| "Failed to cast to WebGl2RenderingContext")?;

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
            program,
            vao,
            instance_buf,
            instance_count: 0,
            center_mm: (130.0, 95.0),
            scale_px_per_mm: 8.0,
            bounds: [70.0, 40.0, 190.0, 150.0],
            active_layer_mask: 0b1_1111_1111,
            opacities: [0.92, 0.75, 0.85, 0.45],
            diff_mode: 0.0,
            wipe_ratio: 0.5,
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
        self.scale_px_per_mm = ((cw * 0.82) / w_mm).min((ch * 0.82) / h_mm).max(2.0);
    }

    pub fn screen_to_board_mm(&self, client_x: f64, client_y: f64) -> (f32, f32) {
        let rect = self.canvas.get_bounding_client_rect();
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .unwrap_or(1.0) as f32;
        let px_x = ((client_x - rect.left()) - rect.width() * 0.5) as f32 * dpr;
        let px_y = ((client_y - rect.top()) - rect.height() * 0.5) as f32 * dpr;
        (
            self.center_mm.0 + px_x / self.scale_px_per_mm,
            self.center_mm.1 + px_y / self.scale_px_per_mm,
        )
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
        self.center_mm.0 -= (dx as f32 * dpr) / self.scale_px_per_mm;
        self.center_mm.1 -= (dy as f32 * dpr) / self.scale_px_per_mm;
    }

    pub fn draw(&mut self) {
        self.resize_to_parent();
        let w = self.canvas.width() as i32;
        let h = self.canvas.height() as i32;

        self.gl.viewport(0, 0, w, h);
        self.gl.clear_color(0.012, 0.024, 0.055, 1.0);
        self.gl.clear(GL::COLOR_BUFFER_BIT);

        if self.instance_count <= 0 {
            return;
        }

        self.gl.enable(GL::BLEND);
        self.gl.blend_func(GL::SRC_ALPHA, GL::ONE_MINUS_SRC_ALPHA);
        self.gl.use_program(Some(&self.program));

        let u1 = self
            .gl
            .get_uniform_location(&self.program, "u_center_and_scale");
        self.gl.uniform4f(
            u1.as_ref(),
            self.center_mm.0,
            self.center_mm.1,
            self.scale_px_per_mm,
            self.active_layer_mask as f32,
        );

        let u2 = self
            .gl
            .get_uniform_location(&self.program, "u_viewport_and_opacities");
        self.gl.uniform4f(
            u2.as_ref(),
            w as f32,
            h as f32,
            self.opacities[0],
            self.opacities[1],
        );

        let u3 = self.gl.get_uniform_location(&self.program, "u_diff_params");
        self.gl.uniform4f(
            u3.as_ref(),
            self.diff_mode,
            self.wipe_ratio,
            self.opacities[2],
            self.opacities[3],
        );

        self.gl.bind_vertex_array(Some(&self.vao));
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
    let program = gl.create_program().ok_or("Unable to create program")?;
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
            .unwrap_or_else(|| "Unknown link error".into()))
    }
}
