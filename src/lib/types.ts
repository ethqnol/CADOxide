export interface LayerRenderResult {
  layer: string;
  filename: string;
  svg_content: string;
}

export interface DrcViolation {
  description: string;
  severity: "error" | "warning" | "exclusion" | string;
  type_name?: string;
  pos_x?: number;
  pos_y?: number;
}

export interface DrcResult {
  success: boolean;
  raw_json: string;
  violations_count: number;
  violations: DrcViolation[];
}

export interface ReviewMessage {
  id: string;
  thread_id: string;
  user_id: string;
  user_name: string;
  body: string;
  created_at: string;
}

export interface ReviewThread {
  id: string;
  revision_id: string;
  file_type: "pcb" | "schematic";
  sheet_or_layer: string;
  coord_x: number;
  coord_y: number;
  status: "open" | "resolved" | "ignored";
  created_by: string;
  created_at: string;
  resolved_at?: string | null;
  messages: ReviewMessage[];
}

export interface PcbLayerConfig {
  id: string;
  name: string;
  canonicalName: string;
  color: string;
  visible: boolean;
  opacity: number;
  isCopper: boolean;
}

export type ViewMode = "pcb" | "diff" | "schematic" | "drc";
export type DualMode = "local" | "remote";

export interface ProjectSample {
  id: string;
  name: string;
  description: string;
  pcbFile: string;
  sampleDir: string;
  schematicSheets: { id: string; name: string; file: string }[];
}
