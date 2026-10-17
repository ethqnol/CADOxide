# CadenceFlow (TraceHub) — Open-Core Dual-Mode Hardware Collaboration Platform

CadenceFlow is an open-core, dual-mode hardware design review and collaboration platform designed for modern engineering teams. It brings Git-like review, visual vector diffing, and redlining to electronic hardware design.

---

## Architecture Overview

```
+------------------------------------------------------------------------------------+
|                                    TAURI FRONTEND                                  |
|  * Framework: React + TypeScript + Tailwind CSS                                    |
|  * 2D Canvas Engine: KiCad SVG Layer Stack with pan, smooth zoom, coordinate HUD  |
|  * State Management: Unified Data Abstraction Layer (DAL)                          |
|  * Redlining: Visual Coordinate Pins (real mm) + Threaded Comment Discussions      |
+------------------------------------------------------------------------------------+
                                          |
                      Unified Data Abstraction Layer (TypeScript)
                     /                                           \
      (Local Mode: Tauri IPC)                     (Remote Mode: HTTP/gRPC / WebSockets)
                   /                                               \
+---------------------------------------+       +------------------------------------+
|            TAURI RUST CORE            |       |         REMOTE / SELF-HOSTED       |
| * Local SQLite (Project/Diff cache)   |       |             CLOUD SERVER           |
| * Local Git Manager (`git2-rs`)       |       | * Web Framework: Axum (Rust)       |
| * Subprocess Runner (`kicad-cli`)     |       | * Auth / RBAC: Argon2, JWT / OIDC  |
| * Native File Watcher (`notify-rs`)   |       | * DB: PostgreSQL 16 + sqlx         |
| * Octopart / Mouser API proxy         |       | * Storage: MinIO / AWS S3 (Blobs)  |
+---------------------------------------+       | * Queue: Redis + Asynq / Tokio     |
                   |                            | * Headless Worker: Dockerized KiCad|
                   v                            +------------------------------------+
         Host OS `kicad-cli`                                      |
         (Installed locally: 10.0.7)                              v
                                                    Headless KiCad Container Pool
```

---

## Key Features Implemented

1. **Dual-Engine Execution Pipeline:**
   - **PCB Layer Render:** Executes `kicad-cli pcb export svg` (`--mode-multi`, `--layers F.Cu,B.Cu,Edge.Cuts,F.SilkS,B.SilkS,F.Mask,B.Mask`, `--fit-page-to-board`, `--exclude-drawing-sheet`).
   - **Schematic Sheet Pipeline:** Extracts multi-sheet schematic hierarchies into indexed SVG vector assets via `kicad-cli sch export svg`.
   - **DRC / Audit Pipeline:** Executes `kicad-cli pcb drc --format json --severity-all --units mm`, parsing violations into typed structs with precise $(X, Y)$ board coordinates.

2. **Visual Diffing Engine (Section 4):**
   - **Split Wipe Slider:** Interactive draggable slider $t \in [0, 1]$ clipping Revision A on the left and Revision B on the right.
   - **Differential Overlay (Boolean Blend Mode):**
     - **Green (`#22c55e`):** Added copper in Revision B.
     - **Red (`#ef4444`):** Removed copper from Revision A.
     - **Muted Slate (`#64748b`):** Unchanged common copper.
   - **Side-by-Side Dual View:** Synchronized dual-canvas comparison.

3. **Design Review & Redlining Engine (Section 3):**
   - Interactive pin drop mode on top of the vector canvas.
   - Captures board coordinates $(X, Y\text{ mm})$ and layer context (`F.Cu`, `Edge.Cuts`, etc.).
   - Persistent comment threads with status toggles (`open`, `resolved`).

4. **Self-Hosted Infrastructure (Section 6):**
   - `docker-compose.yml` defining Axum API server, KiCad worker pool, PostgreSQL 16, Redis 7, and MinIO S3 object storage.
   - `migrations/001_init.sql` matching the PostgreSQL DDL specification.

---

## Quick Start

### 1. Run Axum Backend Server (Rust)
```bash
cargo run --manifest-path server/Cargo.toml
# Listening on http://0.0.0.0:8080
```

### 2. Run Frontend Web UI (Vite + React)
```bash
npm run dev
# Open http://localhost:5173 in browser
```

### 3. Build Production Bundle
```bash
npm run build
```

### 4. Deploy Cloud Microservices
```bash
docker compose up -d
```
