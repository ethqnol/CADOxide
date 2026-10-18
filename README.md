# CADoxide

**CADoxide** is a high-performance, open-core electronic design automation (EDA) and PCB CAD suite built from the ground up for modern hardware engineering. It combines a **100% Pure Rust WebAssembly frontend**, an **instanced WebGL2/GPU Signed Distance Field (SDF) graphics engine**, an **in-browser Design Rule Check (DRC) engine**, an **RS-274X Gerber & Excellon CAM exporter**, an **IPC-standard component footprint library**, and **geometric vector diffing**, with a **distributed Axum cloud backend** and a **Tauri v2 desktop core**.

All JavaScript/TypeScript and React layers have been eliminated: everything from streaming KiCad S-expression parsing, interactive track routing, via/pad placement, polygon zone rendering, real-time clearance rule verification, and CAM Gerber generation down to GPU draw dispatch and reactive DOM management runs in compiled, memory-safe native Rust and WebAssembly.

---

## Key Capabilities & Purpose

- **Interactive In-Browser & Desktop PCB CAD:**
  - Route tracks with 45° angle snapping, posture flipping (`/`), and interactive length tuning.
  - Place plated through-hole vias, pads, lines, arcs, circles, rectangles, text, and dimension callouts.
  - Define, preview, and render filled copper zones, thermal reliefs, and keepouts with customizable fill modes.
  - Configurable grid snapping ($0.05\text{ mm}$ to $2.54\text{ mm}$, mils, inches) with relative coordinate origins (`Space`) and dual Cartesian/polar HUD readouts.
- **Single-Draw-Call Instanced GPU SDF Engine:**
  - Renders entire complex multi-layer boards (traces, pads, zones, silkscreen, mask, holes) in a single instanced WebGL2 draw call.
  - Sub-pixel vertex-stage Level-of-Detail (LOD) and hardware frustum culling yielding silky-smooth 60+ FPS navigation over boards with 100k+ primitives.
  - Real-time 3D exploded multi-layer stackup orbit mode with true physical Z-height layer separation.
  - Instant GPU net highlighting and impedance/delay signal length probing across all copper layers.
- **Native In-Browser DRC Engine:**
  - Runs deep geometric clearance, minimum track width, drill hole clearance, short-circuit, and unconnected net audits directly in client-side Rust WebAssembly.
  - Visual error markers on the canvas with click-to-focus camera navigation and rule severity filtering.
- **RS-274X Gerber & Excellon Drill CAM Exporter:**
  - Client-side production-grade Gerber export for all copper, silkscreen, and solder mask layers (with automatic pad expansion aperture calculation) alongside standard NC Excellon drill files.
- **Built-in IPC Standard Footprint & Component Library:**
  - Ready-to-place standard SMD and THT footprints (0402, 0603, 0805, 1206 passives, SOT-23, SOIC-8, TSSOP-16, QFP-48, QFN-32, USB-C, 0.1" headers) with live SVG preview generator and automatic reference designator incrementing.
- **Hardware Collaboration, Vector Diffing & Review:**
  - Spatial quantized geometric diffing between PCB revisions ($R_A$ vs. $R_B$) with color-coded additions/removals and interactive split-wipe slider inspection.
  - Persistent redline review pins, threaded annotations, and synchronized design reviews backed by SQLite / PostgreSQL.
- **Dual Deployment — Desktop Native & Cloud Platform:**
  - Run natively as a lightweight desktop application via **Tauri v2** with direct filesystem and OS `kicad-cli` integrations.
  - Deploy as a scalable, containerized cloud collaboration hub with Axum, Tokio worker pools, Content-Addressable Storage (CAS), PostgreSQL, Redis, and MinIO.
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
│           ├── drc_engine.rs    # Client-side clearance, track width, & drill DRC audit engine
│           ├── gerber_exporter.rs # RS-274X Gerber & Excellon drill CAM exporter
│           ├── footprint_library.rs # IPC standard footprint templates & SVG previews
│           ├── overlay_renderer.rs  # Crosshairs, ratsnest, coordinate grid & zone overlays
│           └── lib.rs           # WebAssembly entry, DOM controller, CAD tools & review pins
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
│   │   ├── cadoxide_wasm_bg.wasm
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

### 1. Build the Pure-Rust WASM Engine
Compile the release WebAssembly bundle and bindgen glue:
```bash
npm run build:wasm
```
*(Or directly via cargo:)*
```bash
cargo build --manifest-path crates/cadoxide-wasm/Cargo.toml --target wasm32-unknown-unknown --release
wasm-bindgen --target web --no-typescript --out-dir public/wasm crates/cadoxide-wasm/target/wasm32-unknown-unknown/release/cadoxide_wasm.wasm
```

### 2. Run the Rust Axum Server (Web Mode)
The Axum server serves both the REST API (`/api/*`) and the pure-Rust WebAssembly frontend from `public/`:
```bash
npm run server
# Or: cargo run --manifest-path server/Cargo.toml
```
Open **`http://localhost:8080`** in your browser.

### 3. Build & Run Desktop App (Tauri v2)
To run or package CADoxide as a native cross-platform desktop application:

```bash
# Run desktop dev mode
npx @tauri-apps/cli dev

# Build release desktop bundle (.AppImage / .deb / .dmg / .msi)
npx @tauri-apps/cli build
```

> **Note for Linux:** Ensure WebKitGTK and libsoup development libraries are installed (`webkit2gtk4.1-devel` and `libsoup3-devel` on Fedora; `libwebkit2gtk-4.1-dev` and `libsoup-3.0-dev` on Debian/Ubuntu).

### 4. Run Automated Rust Unit Tests
Execute the unit tests verifying KiCad parsing, 4-layer copper extraction, $O(1)$ spatial grid lookups, and geometric vector diffs:
```bash
cargo test --manifest-path crates/cadoxide-wasm/Cargo.toml --lib
```

### 5. Deploy Full Cloud Microservices
To spin up the distributed backend with NGINX load balancing, PostgreSQL, Redis, and MinIO:
```bash
docker compose up -d
```

