import { PcbLayerConfig } from "./types";

export interface WasmExports {
  memory: WebAssembly.Memory;
  wasm_alloc: (len: number) => number;
  wasm_dealloc: (ptr: number, cap: number) => void;
  wasm_instances_ptr: () => number;
  wasm_instances_float_len: () => number;
  wasm_primitive_count: () => number;
  wasm_bounds_ptr: () => number;
  wasm_diff_stats_ptr: () => number;
  wasm_parse_kicad_pcb: (ptr: number, len: number) => number;
  wasm_generate_hdi_stress_test: (targetPrimitives: number) => number;
  wasm_compute_rgba_diff: (
    revAPtr: number,
    revBPtr: number,
    outPtr: number,
    byteLen: number
  ) => void;
}

let wasmInstancePromise: Promise<WasmExports> | null = null;

export async function getWasmEngine(): Promise<WasmExports> {
  if (!wasmInstancePromise) {
    wasmInstancePromise = WebAssembly.instantiateStreaming(
      fetch("/wasm/cadenceflow_wasm.wasm"),
      {}
    ).then((res) => res.instance.exports as unknown as WasmExports);
  }
  return wasmInstancePromise;
}

const LAYER_BIT_MAP: Record<string, number> = {
  "F.Cu": 1 << 0,
  "B.Cu": 1 << 1,
  "Edge.Cuts": 1 << 2,
  "F.SilkS": 1 << 3,
  "B.SilkS": 1 << 4,
  "F.Mask": 1 << 5,
  "B.Mask": 1 << 6,
};

/**
 * WGSL Shader for Native WebGPU (navigator.gpu)
 * Renders instanced PCB traces (capsules), vias (rings), and SMD/TH pads using Signed Distance Fields (SDF).
 */
export const WGSL_PCB_SHADER = /* wgsl */ `
struct Uniforms {
  center_and_scale: vec4<f32>, // x: centerX_mm, y: centerY_mm, z: scale_px_per_mm, w: active_layer_mask
  viewport_and_opacities: vec4<f32>, // x: width_px, y: height_px, z: f_cu_alpha, w: b_cu_alpha
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct InstanceInput {
  @location(0) p1_p2: vec4<f32>,     // x1, y1, x2, y2 in board mm
  @location(1) params: vec4<f32>,    // width, height_or_drill, layer_mask, prim_type
};

struct VertexOutput {
  @builtin(position) position: vec4<f32>,
  @location(0) local_pos: vec2<f32>,
  @location(1) seg_len_and_r: vec2<f32>,
  @location(2) drill_and_type: vec2<f32>,
  @location(3) color: vec4<f32>,
};

@vertex
fn vs_main(
  @builtin(vertex_index) v_idx: u32,
  inst: InstanceInput
) -> VertexOutput {
  var out: VertexOutput;
  let layer_mask = u32(inst.params.z + 0.5);
  let active_mask = u32(u.center_and_scale.w + 0.5);

  if ((layer_mask & active_mask) == 0u) {
    out.position = vec4<f32>(-2.0, -2.0, 0.0, 1.0);
    return out;
  }

  var quad = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>( 1.0,  1.0)
  );
  let uv = quad[v_idx];

  let p1 = inst.p1_p2.xy;
  let p2 = inst.p1_p2.zw;
  let w = max(inst.params.x, 0.08);
  let h = max(inst.params.y, 0.08);
  let prim_type = inst.params.w;

  var world_mm: vec2<f32>;
  if (prim_type < 0.5) {
    // Capsule segment from p1 to p2
    let delta = p2 - p1;
    let len = length(delta);
    let dir = select(vec2<f32>(1.0, 0.0), delta / len, len > 0.0001);
    let norm = vec2<f32>(-dir.y, dir.x);
    let r = w * 0.5;
    let along = select(-r, len + r, uv.x > 0.0);
    let perp = uv.y * r;
    world_mm = p1 + dir * along + norm * perp;
    out.local_pos = vec2<f32>(along, perp);
    out.seg_len_and_r = vec2<f32>(len, r);
  } else {
    // Via ring or SMD/TH Pad centered at p1
    let rx = w * 0.5;
    let ry = select(w * 0.5, h * 0.5, prim_type > 1.5);
    world_mm = p1 + vec2<f32>(uv.x * rx, uv.y * ry);
    out.local_pos = vec2<f32>(uv.x * rx, uv.y * ry);
    out.seg_len_and_r = vec2<f32>(rx, ry);
  }

  out.drill_and_type = vec2<f32>(inst.params.y * 0.5, prim_type);

  // Layer color assignment
  if ((layer_mask & 4u) != 0u) {
    out.color = vec4<f32>(0.97, 0.98, 0.99, 0.95); // Edge.Cuts
  } else if (prim_type > 0.5 && prim_type < 1.5) {
    out.color = vec4<f32>(0.92, 0.78, 0.35, 0.95); // Via gold ring
  } else if ((layer_mask & 1u) != 0u && (active_mask & 1u) != 0u) {
    out.color = vec4<f32>(0.96, 0.62, 0.04, u.viewport_and_opacities.z); // F.Cu Amber
  } else if ((layer_mask & 2u) != 0u && (active_mask & 2u) != 0u) {
    out.color = vec4<f32>(0.23, 0.51, 0.96, u.viewport_and_opacities.w); // B.Cu Blue
  } else if ((layer_mask & 8u) != 0u) {
    out.color = vec4<f32>(0.94, 0.96, 0.98, 0.85); // F.SilkS
  } else {
    out.color = vec4<f32>(0.04, 0.59, 0.41, 0.55); // Solder Mask
  }

  let screen_px = (world_mm - u.center_and_scale.xy) * u.center_and_scale.z;
  let ndc = vec2<f32>(
    (screen_px.x / (u.viewport_and_opacities.x * 0.5)),
    -(screen_px.y / (u.viewport_and_opacities.y * 0.5))
  );
  out.position = vec4<f32>(ndc, 0.0, 1.0);
  return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
  let prim_type = in.drill_and_type.y;
  if (prim_type < 0.5) {
    // Capsule SDF
    let clamped_x = clamp(in.local_pos.x, 0.0, in.seg_len_and_r.x);
    let dist = length(vec2<f32>(in.local_pos.x - clamped_x, in.local_pos.y));
    if (dist > in.seg_len_and_r.y) {
      discard;
    }
  } else if (prim_type < 1.5) {
    // Via Ring SDF with drill hole cutout
    let dist = length(in.local_pos);
    if (dist > in.seg_len_and_r.x || dist < in.drill_and_type.x) {
      discard;
    }
  }
  return in.color;
}
`;

/**
 * WebGL2 GLSL 300 es Fallback Shader (reads the exact same zero-copy WASM Float32Array instance buffer)
 */
const GLSL_VS = `#version 300 es
precision highp float;

layout(location = 0) in vec4 a_p1_p2;
layout(location = 1) in vec4 a_params;

uniform vec4 u_center_and_scale;
uniform vec4 u_viewport_and_opacities;

out vec2 v_local_pos;
out vec2 v_seg_len_and_r;
out vec2 v_drill_and_type;
out vec4 v_color;

void main() {
  uint layer_mask = uint(a_params.z + 0.5);
  uint active_mask = uint(u_center_and_scale.w + 0.5);

  if ((layer_mask & active_mask) == 0u) {
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

  if ((layer_mask & 4u) != 0u) {
    v_color = vec4(0.97, 0.98, 0.99, 0.95);
  } else if (prim_type > 0.5 && prim_type < 1.5) {
    v_color = vec4(0.92, 0.78, 0.35, 0.95);
  } else if ((layer_mask & 1u) != 0u && (active_mask & 1u) != 0u) {
    v_color = vec4(0.96, 0.62, 0.04, u_viewport_and_opacities.z);
  } else if ((layer_mask & 2u) != 0u && (active_mask & 2u) != 0u) {
    v_color = vec4(0.23, 0.51, 0.96, u_viewport_and_opacities.w);
  } else if ((layer_mask & 8u) != 0u) {
    v_color = vec4(0.94, 0.96, 0.98, 0.85);
  } else {
    v_color = vec4(0.04, 0.59, 0.41, 0.55);
  }

  vec2 screen_px = (world_mm - u_center_and_scale.xy) * u_center_and_scale.z;
  vec2 ndc = vec2(
    screen_px.x / (u_viewport_and_opacities.x * 0.5),
    -screen_px.y / (u_viewport_and_opacities.y * 0.5)
  );
  gl_Position = vec4(ndc, 0.0, 1.0);
}
`;

const GLSL_FS = `#version 300 es
precision highp float;

in vec2 v_local_pos;
in vec2 v_seg_len_and_r;
in vec2 v_drill_and_type;
in vec4 v_color;

out vec4 outColor;

void main() {
  float prim_type = v_drill_and_type.y;
  if (prim_type < 0.5) {
    float clamped_x = clamp(v_local_pos.x, 0.0, v_seg_len_and_r.x);
    float dist = length(vec2(v_local_pos.x - clamped_x, v_local_pos.y));
    if (dist > v_seg_len_and_r.y) discard;
  } else if (prim_type < 1.5) {
    float dist = length(v_local_pos);
    if (dist > v_seg_len_and_r.x || dist < v_drill_and_type.x) discard;
  }
  outColor = v_color;
}
`;

/**
 * Zero-Copy WASM + WebGPU / WebGL2 Instanced SDF Renderer
 */
export class WasmWgpuRenderer {
  private canvas: HTMLCanvasElement | null = null;
  private backendType: "WebGPU (WGSL)" | "WebGL2 (Instanced SDF)" = "WebGPU (WGSL)";
  private wasm: WasmExports | null = null;
  private instanceCount: number = 0;
  private bounds: [number, number, number, number] = [70, 40, 190, 150];

  // Camera state: center in board mm + scale in pixels per mm
  private centerMm: { x: number; y: number } = { x: 130, y: 95 };
  private scalePxPerMm: number = 7.5;
  private layerMask: number = 0b1111;
  private fCuOpacity: number = 0.9;
  private bCuOpacity: number = 0.75;

  // WebGPU handles
  private gpuDevice: any = null;
  private gpuContext: any = null;
  private gpuPipeline: any = null;
  private gpuUniformBuffer: any = null;
  private gpuBindGroup: any = null;
  private gpuInstanceBuffer: any = null;

  // WebGL2 fallback handles
  private gl: WebGL2RenderingContext | null = null;
  private glProgram: WebGLProgram | null = null;
  private glVao: WebGLVertexArrayObject | null = null;
  private glInstanceBuffer: WebGLBuffer | null = null;

  private rafId: number | null = null;
  private isDirty: boolean = true;

  public getBackendName(): string {
    return this.backendType;
  }

  public getPrimitiveCount(): number {
    return this.instanceCount;
  }

  public async attach(canvas: HTMLCanvasElement): Promise<void> {
    this.canvas = canvas;
    this.wasm = await getWasmEngine();

    // Try Native WebGPU first
    const nav = navigator as any;
    if (nav.gpu) {
      try {
        const adapter = await nav.gpu.requestAdapter({ powerPreference: "high-performance" });
        if (adapter) {
          const device = await adapter.requestDevice();
          const context = canvas.getContext("webgpu") as any;
          if (context) {
            const format = nav.gpu.getPreferredCanvasFormat();
            context.configure({ device, format, alphaMode: "premultiplied" });
            this.initWebGpuPipeline(device, context, format);
            this.backendType = "WebGPU (WGSL)";
            this.resize();
            this.startLoop();
            return;
          }
        }
      } catch (e) {
        console.warn("WebGPU init fallback to WebGL2:", e);
      }
    }

    // Fallback to Instanced WebGL2 reading the same Zero-Copy WASM buffer
    const gl = canvas.getContext("webgl2", { antialias: true, alpha: false });
    if (gl) {
      this.initWebGl2Pipeline(gl);
      this.backendType = "WebGL2 (Instanced SDF)";
    }
    this.resize();
    this.startLoop();
  }

  public detach() {
    if (this.rafId !== null) {
      cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
    this.canvas = null;
  }

  public resize() {
    if (!this.canvas) return;
    const rect = this.canvas.parentElement?.getBoundingClientRect();
    if (rect && rect.width > 0 && rect.height > 0) {
      const dpr = window.devicePixelRatio || 1;
      this.canvas.width = Math.floor(rect.width * dpr);
      this.canvas.height = Math.floor(rect.height * dpr);
      this.canvas.style.width = `${rect.width}px`;
      this.canvas.style.height = `${rect.height}px`;
      this.isDirty = true;
    }
  }

  /**
   * Parses raw `.kicad_pcb` text inside Rust WASM linear memory and performs
   * Zero-Copy upload of the resulting Float32Array directly to GPU VRAM.
   */
  public async loadKicadPcbSource(pcbText: string): Promise<number> {
    if (!this.wasm) {
      this.wasm = await getWasmEngine();
    }
    const encoder = new TextEncoder();
    const encoded = encoder.encode(pcbText);
    const ptr = this.wasm.wasm_alloc(encoded.byteLength);

    new Uint8Array(this.wasm.memory.buffer, ptr, encoded.byteLength).set(encoded);
    const count = this.wasm.wasm_parse_kicad_pcb(ptr, encoded.byteLength);
    this.wasm.wasm_dealloc(ptr, encoded.byteLength);

    this.syncWasmBufferToGpu(count);
    this.fitToBoard();
    return count;
  }

  /**
   * Generates N synthetic HDI primitives (e.g., 100,000 traces/vias/pads) in WASM memory
   * and uploads Zero-Copy to GPU VRAM.
   */
  public async runHdiStressTest(targetPrimitives: number): Promise<number> {
    if (!this.wasm) {
      this.wasm = await getWasmEngine();
    }
    const count = this.wasm.wasm_generate_hdi_stress_test(targetPrimitives);
    this.syncWasmBufferToGpu(count);
    this.fitToBoard();
    return count;
  }

  private syncWasmBufferToGpu(count: number) {
    if (!this.wasm) return;
    this.instanceCount = count;

    const boundsPtr = this.wasm.wasm_bounds_ptr();
    const boundsArr = new Float32Array(this.wasm.memory.buffer, boundsPtr, 4);
    this.bounds = [boundsArr[0], boundsArr[1], boundsArr[2], boundsArr[3]];

    const instPtr = this.wasm.wasm_instances_ptr();
    const floatLen = this.wasm.wasm_instances_float_len();
    if (floatLen === 0) return;

    // ZERO-COPY view directly into Rust WASM linear memory!
    const zeroCopyFloatView = new Float32Array(this.wasm.memory.buffer, instPtr, floatLen);

    if (this.gpuDevice) {
      const byteSize = zeroCopyFloatView.byteLength;
      this.gpuInstanceBuffer = this.gpuDevice.createBuffer({
        size: byteSize,
        usage: 0x0020 | 0x0008, // GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST
      });
      this.gpuDevice.queue.writeBuffer(
        this.gpuInstanceBuffer,
        0,
        this.wasm.memory.buffer,
        instPtr,
        byteSize
      );
    } else if (this.gl && this.glInstanceBuffer) {
      const gl = this.gl;
      gl.bindBuffer(gl.ARRAY_BUFFER, this.glInstanceBuffer);
      gl.bufferData(gl.ARRAY_BUFFER, zeroCopyFloatView, gl.STATIC_DRAW);
    }

    this.isDirty = true;
  }

  public fitToBoard() {
    const [minX, minY, maxX, maxY] = this.bounds;
    this.centerMm = {
      x: (minX + maxX) * 0.5,
      y: (minY + maxY) * 0.5,
    };
    if (this.canvas) {
      const wMm = Math.max(10, maxX - minX);
      const hMm = Math.max(10, maxY - minY);
      const scaleX = (this.canvas.width * 0.82) / wMm;
      const scaleY = (this.canvas.height * 0.82) / hMm;
      this.scalePxPerMm = Math.min(scaleX, scaleY);
    }
    this.isDirty = true;
  }

  public setLayerConfigs(configs: PcbLayerConfig[]) {
    let mask = 0;
    for (const c of configs) {
      if (c.visible && LAYER_BIT_MAP[c.id]) {
        mask |= LAYER_BIT_MAP[c.id];
      }
      if (c.id === "F.Cu") this.fCuOpacity = c.opacity;
      if (c.id === "B.Cu") this.bCuOpacity = c.opacity;
    }
    this.layerMask = mask;
    this.isDirty = true;
  }

  public panByScreenPx(dxPx: number, dyPx: number) {
    const dpr = window.devicePixelRatio || 1;
    this.centerMm.x -= (dxPx * dpr) / this.scalePxPerMm;
    this.centerMm.y -= (dyPx * dpr) / this.scalePxPerMm;
    this.isDirty = true;
  }

  public zoomAtScreenPx(factor: number, clientX: number, clientY: number) {
    if (!this.canvas) return;
    const beforeMm = this.screenToBoardMm(clientX, clientY);
    this.scalePxPerMm = Math.max(1.5, Math.min(450, this.scalePxPerMm * factor));
    const afterMm = this.screenToBoardMm(clientX, clientY);

    this.centerMm.x += beforeMm.x - afterMm.x;
    this.centerMm.y += beforeMm.y - afterMm.y;
    this.isDirty = true;
  }

  public getZoomPercent(): number {
    return Math.round((this.scalePxPerMm / 7.5) * 100);
  }

  public screenToBoardMm(clientX: number, clientY: number): { x: number; y: number } {
    if (!this.canvas) return { x: 130, y: 95 };
    const rect = this.canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const pxX = (clientX - rect.left - rect.width * 0.5) * dpr;
    const pxY = (clientY - rect.top - rect.height * 0.5) * dpr;
    return {
      x: Number((this.centerMm.x + pxX / this.scalePxPerMm).toFixed(2)),
      y: Number((this.centerMm.y + pxY / this.scalePxPerMm).toFixed(2)),
    };
  }

  private initWebGpuPipeline(device: any, context: any, format: string) {
    this.gpuDevice = device;
    this.gpuContext = context;

    const shaderModule = device.createShaderModule({ code: WGSL_PCB_SHADER });
    this.gpuUniformBuffer = device.createBuffer({
      size: 32,
      usage: 0x0040 | 0x0008, // UNIFORM | COPY_DST
    });

    const bindGroupLayout = device.createBindGroupLayout({
      entries: [
        {
          binding: 0,
          visibility: 0x1 | 0x2, // VERTEX | FRAGMENT
          buffer: { type: "uniform" },
        },
      ],
    });

    this.gpuBindGroup = device.createBindGroup({
      layout: bindGroupLayout,
      entries: [{ binding: 0, resource: { buffer: this.gpuUniformBuffer } }],
    });

    this.gpuPipeline = device.createRenderPipeline({
      layout: device.createPipelineLayout({ bindGroupLayouts: [bindGroupLayout] }),
      vertex: {
        module: shaderModule,
        entryPoint: "vs_main",
        buffers: [
          {
            arrayStride: 32,
            stepMode: "instance",
            attributes: [
              { shaderLocation: 0, offset: 0, format: "float32x4" },
              { shaderLocation: 1, offset: 16, format: "float32x4" },
            ],
          },
        ],
      },
      fragment: {
        module: shaderModule,
        entryPoint: "fs_main",
        targets: [
          {
            format,
            blend: {
              color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha", operation: "add" },
              alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
            },
          },
        ],
      },
      primitive: { topology: "triangle-list" },
    });
  }

  private initWebGl2Pipeline(gl: WebGL2RenderingContext) {
    this.gl = gl;
    const vs = gl.createShader(gl.VERTEX_SHADER)!;
    gl.shaderSource(vs, GLSL_VS);
    gl.compileShader(vs);

    const fs = gl.createShader(gl.FRAGMENT_SHADER)!;
    gl.shaderSource(fs, GLSL_FS);
    gl.compileShader(fs);

    const prog = gl.createProgram()!;
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    this.glProgram = prog;

    const vao = gl.createVertexArray()!;
    gl.bindVertexArray(vao);
    this.glVao = vao;

    const buf = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    this.glInstanceBuffer = buf;

    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 4, gl.FLOAT, false, 32, 0);
    gl.vertexAttribDivisor(0, 1);

    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, 32, 16);
    gl.vertexAttribDivisor(1, 1);

    gl.bindVertexArray(null);
  }

  private startLoop() {
    const tick = () => {
      if (this.isDirty) {
        this.renderFrame();
        this.isDirty = false;
      }
      this.rafId = requestAnimationFrame(tick);
    };
    this.rafId = requestAnimationFrame(tick);
  }

  private renderFrame() {
    if (!this.canvas) return;
    const w = this.canvas.width;
    const h = this.canvas.height;

    const uniforms = new Float32Array([
      this.centerMm.x,
      this.centerMm.y,
      this.scalePxPerMm,
      this.layerMask,
      w,
      h,
      this.fCuOpacity,
      this.bCuOpacity,
    ]);

    if (this.gpuDevice && this.gpuContext && this.gpuPipeline) {
      this.gpuDevice.queue.writeBuffer(this.gpuUniformBuffer, 0, uniforms);
      const encoder = this.gpuDevice.createCommandEncoder();
      const view = this.gpuContext.getCurrentTexture().createView();
      const pass = encoder.beginRenderPass({
        colorAttachments: [
          {
            view,
            clearValue: { r: 0.015, g: 0.025, b: 0.06, a: 1.0 },
            loadOp: "clear",
            storeOp: "store",
          },
        ],
      });

      if (this.gpuInstanceBuffer && this.instanceCount > 0) {
        pass.setPipeline(this.gpuPipeline);
        pass.setBindGroup(0, this.gpuBindGroup);
        pass.setVertexBuffer(0, this.gpuInstanceBuffer);
        pass.draw(6, this.instanceCount, 0, 0);
      }
      pass.end();
      this.gpuDevice.queue.submit([encoder.finish()]);
    } else if (this.gl && this.glProgram && this.glVao) {
      const gl = this.gl;
      gl.viewport(0, 0, w, h);
      gl.clearColor(0.015, 0.025, 0.06, 1.0);
      gl.clear(gl.COLOR_BUFFER_BIT);

      if (this.instanceCount > 0) {
        gl.enable(gl.BLEND);
        gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
        gl.useProgram(this.glProgram);

        const loc1 = gl.getUniformLocation(this.glProgram, "u_center_and_scale");
        const loc2 = gl.getUniformLocation(this.glProgram, "u_viewport_and_opacities");
        gl.uniform4fv(loc1, uniforms.subarray(0, 4));
        gl.uniform4fv(loc2, uniforms.subarray(4, 8));

        gl.bindVertexArray(this.glVao);
        gl.drawArraysInstanced(gl.TRIANGLES, 0, 6, this.instanceCount);
        gl.bindVertexArray(null);
      }
    }
  }
}
