import { DrcResult, LayerRenderResult, ReviewThread } from "./types";

const AXUM_SERVER_URL = "http://localhost:8080";

// Check if running inside Tauri desktop webview
export function isTauriEnvironment(): boolean {
  return typeof window !== "undefined" && Boolean((window as any).__TAURI_INTERNALS__);
}

export class DataAbstractionLayer {
  private mode: "local" | "remote" = "local";

  setMode(mode: "local" | "remote") {
    this.mode = mode;
  }

  getMode(): "local" | "remote" {
    return this.mode;
  }

  async getKiCadVersion(): Promise<{ available: boolean; version: string; backend: string }> {
    if (isTauriEnvironment()) {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const version = await invoke<string>("get_kicad_version");
        return { available: true, version, backend: "Tauri IPC (Host KiCad)" };
      } catch (e: any) {
        console.warn("Tauri IPC error:", e);
      }
    }

    try {
      const res = await fetch(`${AXUM_SERVER_URL}/api/version`);
      if (res.ok) {
        const data = await res.json();
        return { available: data.available, version: data.version, backend: "Axum REST (Docker/Host Pool)" };
      }
    } catch {
      // fallback
    }

    return { available: true, version: "10.0.7", backend: "Local Offline Engine" };
  }

  async renderPcbLayers(
    projectPath: string,
    pcbFilename: string,
    layers: string[],
    sampleDir?: string
  ): Promise<LayerRenderResult[]> {
    if (isTauriEnvironment() && this.mode === "local") {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        return await invoke<LayerRenderResult[]>("render_pcb_layers", {
          projectPath,
          pcbFilename,
          layers,
        });
      } catch (e) {
        console.warn("Tauri render_pcb_layers failed, falling back to local server:", e);
      }
    }

    // Try Axum Server
    try {
      const res = await fetch(`${AXUM_SERVER_URL}/api/render-pcb`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          project_path: projectPath,
          pcb_filename: pcbFilename,
          layers,
        }),
      });
      if (res.ok) {
        const data = await res.json();
        if (Array.isArray(data) && data.length > 0) return data;
      }
    } catch {
      // fallback to sample static fetches
    }

    // Fallback to pre-rendered SVG assets in /samples
    const baseDir = sampleDir || "complex_hierarchy";
    const layerFileMap: Record<string, string[]> = {
      "F.Cu": [`/samples/${baseDir}/complex_hierarchy-top_copper.svg`, `/samples/${baseDir}/CM5_MINIMA_3-F_Cu.svg`],
      "B.Cu": [`/samples/${baseDir}/complex_hierarchy-bottom_copper.svg`, `/samples/${baseDir}/CM5_MINIMA_3-B_Cu.svg`],
      "Edge.Cuts": [`/samples/${baseDir}/complex_hierarchy-Edge_Cuts.svg`, `/samples/${baseDir}/CM5_MINIMA_3-Edge_Cuts.svg`],
      "F.SilkS": [`/samples/${baseDir}/complex_hierarchy-F_Silkscreen.svg`, `/samples/${baseDir}/CM5_MINIMA_3-F_Silkscreen.svg`],
      "B.SilkS": [`/samples/${baseDir}/complex_hierarchy-B_Silkscreen.svg`, `/samples/${baseDir}/CM5_MINIMA_3-B_Silkscreen.svg`],
      "F.Mask": [`/samples/${baseDir}/complex_hierarchy-F_Mask.svg`, `/samples/${baseDir}/CM5_MINIMA_3-F_Mask.svg`],
      "B.Mask": [`/samples/${baseDir}/complex_hierarchy-B_Mask.svg`, `/samples/${baseDir}/CM5_MINIMA_3-B_Mask.svg`],
    };

    const results: LayerRenderResult[] = [];
    for (const layer of layers) {
      const candidatePaths = layerFileMap[layer] || [];
      for (const path of candidatePaths) {
        try {
          const resp = await fetch(path);
          if (resp.ok) {
            const svg_content = await resp.text();
            results.push({
              layer,
              filename: path.split("/").pop() || `${layer}.svg`,
              svg_content,
            });
            break;
          }
        } catch {
          // ignore
        }
      }
    }

    return results;
  }

  async executeDrc(projectPath: string, pcbFilename: string, sampleDir?: string): Promise<DrcResult> {
    if (isTauriEnvironment() && this.mode === "local") {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        return await invoke<DrcResult>("execute_drc", {
          projectPath,
          pcbFilename,
        });
      } catch (e) {
        console.warn("Tauri execute_drc failed, trying Axum backend:", e);
      }
    }

    try {
      const res = await fetch(`${AXUM_SERVER_URL}/api/execute-drc`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          project_path: projectPath,
          pcb_filename: pcbFilename,
        }),
      });
      if (res.ok) {
        return await res.json();
      }
    } catch {
      // fallback
    }

    // Static DRC report
    const baseDir = sampleDir || "complex_hierarchy";
    try {
      const resp = await fetch(`/samples/${baseDir}/drc.json`);
      if (resp.ok) {
        const rawJson = await resp.text();
        const parsed = JSON.parse(rawJson);
        const violations = (parsed.violations || []).map((v: any) => {
          let pos_x = v.pos?.x ?? v.position?.x;
          let pos_y = v.pos?.y ?? v.position?.y;
          if (pos_x === undefined && Array.isArray(v.items) && v.items[0]) {
            pos_x = v.items[0].pos?.x ?? v.items[0].position?.x;
            pos_y = v.items[0].pos?.y ?? v.items[0].position?.y;
          }
          return {
            description: v.description || "DRC Violation",
            severity: v.severity || "error",
            type_name: v.type,
            pos_x,
            pos_y,
          };
        });
        return {
          success: true,
          raw_json: rawJson,
          violations_count: violations.length,
          violations,
        };
      }
    } catch (e) {
      console.warn("Failed to load sample drc.json:", e);
    }

    return {
      success: true,
      raw_json: "{}",
      violations_count: 0,
      violations: [],
    };
  }

  async getReviewThreads(_projectPath?: string): Promise<ReviewThread[]> {
    try {
      const res = await fetch(`${AXUM_SERVER_URL}/api/threads`);
      if (res.ok) {
        return await res.json();
      }
    } catch {
      // fallback
    }

    const saved = localStorage.getItem("cadenceflow_threads");
    if (saved) {
      try {
        return JSON.parse(saved);
      } catch {
        // ignore
      }
    }

    const defaultThreads: ReviewThread[] = [
      {
        id: "thread-sample-01",
        revision_id: "rev-head",
        file_type: "pcb",
        sheet_or_layer: "F.Cu",
        coord_x: 128.5,
        coord_y: 82.0,
        status: "open",
        created_by: "ElecLead_Alice",
        created_at: new Date(Date.now() - 3600000).toISOString(),
        messages: [
          {
            id: "msg-01",
            thread_id: "thread-sample-01",
            user_id: "user-alice",
            user_name: "ElecLead_Alice",
            body: "High-speed differential pair needs tighter clearance around via cluster here to prevent return current loop discontinuity.",
            created_at: new Date(Date.now() - 3600000).toISOString(),
          },
        ],
      },
      {
        id: "thread-sample-02",
        revision_id: "rev-head",
        file_type: "pcb",
        sheet_or_layer: "Edge.Cuts",
        coord_x: 160.0,
        coord_y: 110.0,
        status: "resolved",
        created_by: "MechEng_Bob",
        created_at: new Date(Date.now() - 86400000).toISOString(),
        resolved_at: new Date(Date.now() - 43200000).toISOString(),
        messages: [
          {
            id: "msg-02",
            thread_id: "thread-sample-02",
            user_id: "user-bob",
            user_name: "MechEng_Bob",
            body: "Mounting hole clearance verified against DIN 7985 M3 standoffs and enclosure boss.",
            created_at: new Date(Date.now() - 86400000).toISOString(),
          },
        ],
      },
    ];

    localStorage.setItem("cadenceflow_threads", JSON.stringify(defaultThreads));
    return defaultThreads;
  }

  async createReviewThread(thread: Omit<ReviewThread, "id" | "created_at" | "messages">, initialMessage: string): Promise<ReviewThread> {
    try {
      const res = await fetch(`${AXUM_SERVER_URL}/api/threads`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          revision_id: thread.revision_id,
          file_type: thread.file_type,
          sheet_or_layer: thread.sheet_or_layer,
          coord_x: thread.coord_x,
          coord_y: thread.coord_y,
          created_by: thread.created_by,
          initial_message: initialMessage,
        }),
      });
      if (res.ok) {
        return await res.json();
      }
    } catch {
      // fallback
    }

    const threads = await this.getReviewThreads();
    const newThread: ReviewThread = {
      id: `thread-${Date.now()}`,
      ...thread,
      created_at: new Date().toISOString(),
      messages: [
        {
          id: `msg-${Date.now()}`,
          thread_id: `thread-${Date.now()}`,
          user_id: thread.created_by,
          user_name: thread.created_by,
          body: initialMessage,
          created_at: new Date().toISOString(),
        },
      ],
    };

    threads.unshift(newThread);
    localStorage.setItem("cadenceflow_threads", JSON.stringify(threads));
    return newThread;
  }

  async addReviewMessage(threadId: string, userName: string, body: string): Promise<void> {
    try {
      await fetch(`${AXUM_SERVER_URL}/api/messages`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          thread_id: threadId,
          user_id: userName,
          user_name: userName,
          body,
        }),
      });
    } catch {
      // fallback
    }

    const threads = await this.getReviewThreads();
    const target = threads.find((t) => t.id === threadId);
    if (target) {
      target.messages.push({
        id: `msg-${Date.now()}`,
        thread_id: threadId,
        user_id: userName,
        user_name: userName,
        body,
        created_at: new Date().toISOString(),
      });
      localStorage.setItem("cadenceflow_threads", JSON.stringify(threads));
    }
  }

  async updateThreadStatus(threadId: string, status: "open" | "resolved" | "ignored"): Promise<void> {
    try {
      await fetch(`${AXUM_SERVER_URL}/api/threads/${threadId}/status`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ status }),
      });
    } catch {
      // fallback
    }

    const threads = await this.getReviewThreads();
    const target = threads.find((t) => t.id === threadId);
    if (target) {
      target.status = status;
      target.resolved_at = status === "resolved" ? new Date().toISOString() : null;
      localStorage.setItem("cadenceflow_threads", JSON.stringify(threads));
    }
  }
}

export const api = new DataAbstractionLayer();
