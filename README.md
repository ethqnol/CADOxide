# CADoxide — 100% Pure Rust Hardware Collaboration Platform

**CADoxide** is an open-core, dual-mode hardware design review, collaboration, and inspection platform built for ultra-high-density PCB designs. It features a **100% Pure Rust WASM frontend** with an **Instanced WebGL2/WGPU Signed Distance Field (SDF) GPU shader engine**, paired with a **distributed, broker-backed Rust Axum server** and **Tauri v2 desktop core**.

All JavaScript/TypeScript and React layers have been completely eliminated. The entire pipeline—from KiCad S-expression parsing and geometric vector diffing to GPU draw dispatch, DOM state management, and redlining threads—runs in compiled native Rust and WebAssembly.

---

## Architecture Overview

```
+-----------------------------------------------------------------------------------------------+
|                                     100% PURE RUST FRONTEND                                   |
|                                (crates/cadoxide-wasm -> WebAssembly)                          |
|                                                                                               |
|  * Framework: 100% Pure Rust + web-sys / js-sys (Zero JS/TS application code)                |
|  * GPU Engine: Instanced WebGL2 SDF Shader Engine (1 Draw Call for all layers/vias/pads/pins)  |
|  * Performance Optimizations:                                                                 |
|      - Vertex-Stage Sub-Pixel Level-of-Detail (LOD) Culling (discards <0.38px primitives)      |
|      - Hardware Frustum Culling (NDC [-1.35, 1.35] instance culling in vertex shader)         |
|      - O(1) Uniform Spatial Grid Hash (sub-microsecond hover & hit-testing across 100k+ prims)|
|      - Interactive 3D Multi-Layer Stackup Orbit (true physical Z-separation in vertex stage)   |
|  * Core Modules:                                                                              |
|      - parser.rs: Streaming KiCad 6-10 S-expression parser + Spatial quantized vector diff    |
|      - gl_renderer.rs: GLSL 300 es Instanced SDF shaders & VBO memory manager                 |
|      - lib.rs: Reactive UI Controller, DOM binder, Review Threads & Parts BOM Manager         |
+-----------------------------------------------------------------------------------------------+
                                                |
                         Unified Rust Data Abstraction & IPC / HTTP
                       /                                           \
       (Local Mode: Tauri v2 Core)                     (Remote Mode: Axum Cloud Server)
                     /                                               \
+-----------------------------------------+       +---------------------------------------------+
|             TAURI RUST CORE             |       |            REMOTE / SELF-HOSTED             |
|          (`src-tauri/src/*`)            |       |                CLOUD SERVER                 |
|                                         |       |             (`server/src/*`)                |
| * Local SQLite Database (`rusqlite`)    |       | * Web Framework: Axum 0.8 + Tokio Async     |
| * Git Revisions (`commands/git.rs`)     |       | * Content-Addressable Storage (CAS) Cache   |
| * Headless KiCad Subprocess Pipelines:  |       | * Work Queue Broker: Tokio mpsc channel     |
|     1. PCB Layer Render (SVG)           |       | * Worker Pool: Bounded Semaphore (4 workers)|
|     2. Schematic Hierarchy (SVG)        |       | * DB Schema: Organizations, Projects,       |
|     3. DRC / Audit Engine (JSON)        |       |              Revisions, Parts Library,      |
|     4. 3D Model Export (VRML .wrl)      |       |              Review Threads & Messages      |
| * Serves pure WASM from `../public`     |       | * Fallback Static File Server for WASM bundle|
+-----------------------------------------+       +---------------------------------------------+
                     |                                                   |
                     v                                                   v
           Local OS `kicad-cli`                                Headless Worker Pool
               (KiCad 10.0.7)                                  (Dockerized kicad-cli)
```

---

## How the Entire System Works: Deep Dive

### 1. Ingestion & Pure-Rust KiCad S-Expression Parser (`parser.rs`)
- When a `.kicad_pcb` file (such as the 4-layer `CM5_MINIMA_3.kicad_pcb` or multi-sheet `complex_hierarchy.kicad_pcb`) is opened, the raw text is parsed line-by-line without any intermediary JSON representation.
- Supported primitives:
  - `(segment)`: Routed copper traces parsed into start/end points, trace width, and layer mask.
  - `(via)`: Plated through-hole (PTH) and microvias with drill hole diameters and annular rings.
  - `(fp_line)`, `(fp_rect)`, `(gr_line)`, `(gr_rect)`, `(gr_circle)`: Component outlines and `Edge.Cuts` board perimeter geometry.
  - `(pad)`: SMD and through-hole component pads with relative offset and board-level footprint rotation matrix transformations applied in Rust.
- **Inner Copper Layer Support (`LAYER_IN_CU`):**
  - Extracts outer copper (`F.Cu`, `B.Cu`), inner plane/signal layers (`In1.Cu` through `In30.Cu`), masks (`F.Mask`, `B.Mask`), silkscreens (`F.SilkS`, `B.SilkS`), and board cutouts (`Edge.Cuts`).
- **Memory Layout:** Packed directly into an 8-float instance buffer:
  `[p1_x, p1_y, p2_x, p2_y, width, height_or_drill, layer_bitmask, primitive_type]`.

### 2. $O(1)$ Uniform Spatial Grid Index & Sub-Microsecond Hover Inspection
- The board bounds are divided into $2.5\text{ mm} \times 2.5\text{ mm}$ spatial grid cells (`HashMap<(i32, i32), Vec<u32>>`).
- Each primitive's bounding box is indexed into overlapping grid buckets during parsing in $< 2\text{ ms}$.
- When the user moves the mouse across the canvas:
  1. The cursor coordinate is converted to board space $(X, Y\text{ mm})$.
  2. `ParsedBoard::query_nearest()` checks only the current cell and its 8 adjacent neighbors ($3 \times 3$ lookup).
  3. Point-to-segment distance (for capsules) or point-to-center distance (for circular pads/vias) is evaluated against visible layer bitmasks.
  4. The hovered instance index is sent to the GPU via uniform `u_3d_and_hover.w = gl_InstanceID`.
  5. The hovered track, via, or pad glows electric cyan (`vec3(0.15, 0.96, 1.0)`) in real time without modifying or re-uploading the GPU vertex buffer.
  6. The bottom HUD immediately displays exact telemetry: `Track [F.Cu] w=0.25mm len=8.42mm @ (128.4, 82.1)`.

### 3. GPU Shader Engine & Level-of-Detail (LOD) Optimizations (`gl_renderer.rs`)
All rendering is performed in **a single instanced draw call** (`gl.draw_arrays_instanced(GL::TRIANGLES, 0, 6, instance_count)`) using WebGL2 (`#version 300 es`):
- **Vertex-Stage Sub-Pixel LOD Culling:**
  - In `VS_SRC`, screen-space pixel dimensions are computed as `max(w, h) * scale`.
  - For non-marker primitives, if `max_dim_px < 0.38` and screen segment length $< 0.55\text{ px}$, the vertex shader sets `gl_Position = vec4(-2.0, -2.0, 0.0, 1.0)`. The hardware rasterizer instantly culls the triangle, resulting in **zero fragment shader invocations** for sub-pixel geometry when viewing large boards zoomed out.
- **Hardware Frustum Culling:**
  - Compact primitives whose normalized device coordinates fall outside `[-1.35, 1.35]` are discarded in the vertex stage.
- **3D Exploded Multi-Layer Stackup Orbit Mode:**
  - Toggling `🧊 3D Stackup Orbit` elevates physical layer masks in Z:
    - Silkscreen (`F.SilkS`): $+5.5\text{ mm}$
    - Solder Mask (`F.Mask`): $+3.8\text{ mm}$
    - Top Copper (`F.Cu`): $+2.2\text{ mm}$
    - Inner Layers (`In1.Cu`..`In4.Cu`): $+0.8\text{ mm}$
    - FR4 Substrate / `Edge.Cuts`: $0.0\text{ mm}$
    - Bottom Copper (`B.Cu`): $-2.2\text{ mm}$
    - Bottom Mask & Silk: $-3.8\text{ mm}$ to $-5.5\text{ mm}$
  - The vertex shader applies 3D rotation (`yaw` and `pitch`) around the board center, enabling real-time 60 FPS orbit inspection (`Shift + Drag`).
- **Instanced Signed Distance Field (SDF) Fragment Evaluation:**
  - In `FS_SRC`, rounded track ends and annular drill holes are evaluated mathematically via Euclidean distance fields without generating complex tessellated meshes.
- **GPU Net Highlighting & Signal Length Probing:**
  - Packing `net_id * 16.0 + prim_type` into the 8th instance float (`a_params.w`) lets the GPU vertex shader highlight an entire electrical net across all layers in a single uniform update (`u_selected_net_id`) while dimming unrelated copper to 16% opacity.
  - Clicking any trace, via, or pad (or selecting from the **Probe Net** dropdown) displays total routed length ($\text{mm}$ and $\text{mils}$), estimated FR4 propagation delay ($\approx 6.5\text{ ps/mm}$), and segment/via/pad counts.

### 4. Pure-Rust Geometric Vector Diffing Engine
- Given Revision A ($R_A$) and Revision B ($R_B$), primitives are spatially quantized into coordinate hash keys `(x0, y0, x1, y1, width, layer)`.
- Set difference operations classify primitives into:
  - **Unchanged (Muted Slate `#4A5568`):** Common to both revisions.
  - **Removed (Red `#ef4444`):** Present only in $R_A$.
  - **Added (Green `#22c55e`):** Present only in $R_B$.
- **Split Wipe Shader:** An interactive draggable vertical boundary divides the viewport. Left of the wipe renders $R_A$; right of the wipe renders $R_B$, with a glowing cyan division line.

### 5. Distributed Backend, CAS Cache, and Enterprise BOM Library (`server/src/main.rs`)
- **Content-Addressable Storage (CAS):** Hashes PCB file contents and caches rendered layer SVGs and DRC analysis in memory. Cache hits bypass `kicad-cli` execution entirely.
- **Worker Pool & Message Broker:** Compute jobs (DRC verification, PCB SVG rendering, 3D VRML model generation) are pushed to a Tokio `mpsc` channel and scheduled across a bounded `Semaphore` worker pool.
- **Centralized Component Library (`parts_library`):**
  - Manages Manufacturer Part Numbers (MPNs), footprints, symbols, lifecycle statuses (`ACTIVE`, `NRND`, `OBSOLETE`), and cached DigiKey/Mouser inventory and unit pricing.
  - Accessible via the **Parts & BOM** tab in the UI with instant search filtering.
- **Collaborative Redline Review Threads:**
  - Review pins dropped at precise board coordinates $(X, Y\text{ mm})$ persist to SQLite (`review_threads`, `review_messages`) and sync in real time across clients.
  - Supports marking threads as **Resolved / Open** and jumping the GPU camera directly to annotations or DRC violations.

---

## Repository Structure

```
CADoxide/
├── crates/
│   └── cadoxide-wasm/           # 100% Pure Rust WASM Frontend & GPU Shader Engine
│       ├── Cargo.toml           # Configured for wasm32-unknown-unknown (zero JS dependencies)
│       └── src/
│           ├── gl_renderer.rs   # WebGL2 Instanced SDF Engine, LOD culling, 3D orbit
│           ├── parser.rs        # KiCad S-expression parser, O(1) spatial grid, vector diff
│           └── lib.rs           # WebAssembly entry, DOM controller, review pins & BOM UI
├── server/                      # Distributed Axum Cloud Server
│   ├── Cargo.toml
│   └── src/
│       └── main.rs              # CAS cache, worker semaphore, SQLite DB, REST & static API
├── src-tauri/                   # Tauri v2 Desktop Core
│   ├── Cargo.toml
│   ├── tauri.conf.json          # Configured to load pure-Rust bundle from ../public
│   └── src/
│       ├── lib.rs               # Desktop invoke handlers
│       └── commands/
│           ├── kicad.rs         # Subprocess pipelines (PCB, Sch, DRC, 3D VRML)
│           ├── git.rs           # Git log and revision tracking
│           └── db.rs            # Local review thread database
├── public/                      # Static web distribution directory
│   ├── index.html               # Pure-Rust WASM bootstrapper (zero React/JS)
│   ├── styles.css               # Pre-compiled standalone Tailwind CSS
│   ├── wasm/                    # Compiled release WebAssembly artifacts
│   │   ├── cadoxide_wasm_bg.wasm (250 KB)
│   │   └── cadoxide_wasm.js
│   └── samples/                 # KiCad sample boards, SVGs, DRC reports, and 3D VRML models
│       ├── complex_hierarchy/
│       ├── complex_hierarchy_revB/
│       └── cm5_minima/
├── docker-compose.yml           # Self-hosted stack (NGINX, Axum, KiCad worker, Postgres, Redis, MinIO)
├── nginx.conf                   # Reverse proxy and least-connections load balancer
├── migrations/                  # PostgreSQL database migrations
│   └── 001_init.sql
└── package.json                 # Rust WASM and server orchestration scripts
```

---

## Quick Start & Build Instructions

### Prerequisites
- **Rust Toolchain:** `rustup target add wasm32-unknown-unknown`
- **wasm-bindgen CLI:** `cargo install wasm-bindgen-cli --version 0.2.127`
- **KiCad CLI:** `kicad-cli` (v8 or v10 recommended for headless exports)

### 1. Build the Pure-Rust WASM Frontend
Compile the release WebAssembly bundle and bindgen glue:
```bash
npm run build:wasm
```
*(Or directly via cargo:)*
```bash
cargo build --manifest-path crates/cadoxide-wasm/Cargo.toml --target wasm32-unknown-unknown --release
wasm-bindgen --target web --no-typescript --out-dir public/wasm crates/cadoxide-wasm/target/wasm32-unknown-unknown/release/cadoxide_wasm.wasm
```

### 2. Run the Rust Axum Server
The Axum server serves both the REST API (`/api/*`) and the pure-Rust WebAssembly frontend from `public/`:
```bash
npm run server
# Or: cargo run --manifest-path server/Cargo.toml
```
Open **`http://localhost:8080`** in your browser.

### 3. Run Automated Rust Unit Tests
Execute the unit tests verifying KiCad parsing, 4-layer copper extraction, $O(1)$ spatial grid lookups, and geometric vector diffs:
```bash
cargo test --manifest-path crates/cadoxide-wasm/Cargo.toml --lib
```

### 4. Deploy Full Cloud Microservices
To spin up the distributed backend with NGINX load balancing, PostgreSQL, Redis, and MinIO:
```bash
docker compose up -d
```
