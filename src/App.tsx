import React, { useState, useEffect } from "react";
import { Header } from "./components/Header";
import { PcbCanvas } from "./components/PcbCanvas";
import { VisualDiff } from "./components/VisualDiff";
import { SchematicViewer } from "./components/SchematicViewer";
import { DrcInspector } from "./components/DrcInspector";
import { ReviewPanel } from "./components/ReviewPanel";
import { CloudSyncModal } from "./components/CloudSyncModal";
import {
  ViewMode,
  DualMode,
  ProjectSample,
  LayerRenderResult,
  PcbLayerConfig,
  ReviewThread,
  DrcResult,
} from "./lib/types";
import { api } from "./lib/api";

const SAMPLE_PROJECTS: ProjectSample[] = [
  {
    id: "complex_hierarchy",
    name: "Audio Amp (Complex Hierarchy)",
    description: "Multi-sheet hierarchical PCB with dual-rail power and push-pull output stage",
    pcbFile: "complex_hierarchy.kicad_pcb",
    sampleDir: "complex_hierarchy",
    schematicSheets: [
      { id: "root", name: "Root Sheet", file: "/samples/complex_hierarchy/complex_hierarchy.svg" },
      { id: "horizontal", name: "Ampli Horizontal", file: "/samples/complex_hierarchy/complex_hierarchy-ampli_ht_horizontal.svg" },
      { id: "vertical", name: "Ampli Vertical", file: "/samples/complex_hierarchy/complex_hierarchy-ampli_ht_vertical.svg" },
    ],
  },
  {
    id: "cm5_minima",
    name: "Raspberry Pi CM5 Minima",
    description: "Compact Compute Module 5 carrier board with high density routing",
    pcbFile: "CM5_MINIMA_3.kicad_pcb",
    sampleDir: "cm5_minima",
    schematicSheets: [
      { id: "root", name: "CM5 Minima Base", file: "/samples/cm5_minima/CM5_MINIMA_3-F_Cu.svg" },
    ],
  },
];

const INITIAL_LAYERS: PcbLayerConfig[] = [
  { id: "F.Cu", name: "Front Copper", canonicalName: "top_copper", color: "#f59e0b", visible: true, opacity: 0.9, isCopper: true },
  { id: "B.Cu", name: "Bottom Copper", canonicalName: "bottom_copper", color: "#3b82f6", visible: true, opacity: 0.7, isCopper: true },
  { id: "Edge.Cuts", name: "Board Outline", canonicalName: "Edge_Cuts", color: "#f8fafc", visible: true, opacity: 1.0, isCopper: false },
  { id: "F.SilkS", name: "Front Silkscreen", canonicalName: "F_Silkscreen", color: "#f1f5f9", visible: true, opacity: 0.85, isCopper: false },
  { id: "B.SilkS", name: "Back Silkscreen", canonicalName: "B_Silkscreen", color: "#94a3b8", visible: false, opacity: 0.8, isCopper: false },
  { id: "F.Mask", name: "Front Solder Mask", canonicalName: "F_Mask", color: "#059669", visible: false, opacity: 0.4, isCopper: false },
  { id: "B.Mask", name: "Back Solder Mask", canonicalName: "B_Mask", color: "#047857", visible: false, opacity: 0.4, isCopper: false },
];

export function App() {
  const [currentView, setCurrentView] = useState<ViewMode>("pcb");
  const [dualMode, setDualMode] = useState<DualMode>("local");
  const [selectedProject, setSelectedProject] = useState<ProjectSample>(SAMPLE_PROJECTS[0]);

  // KiCad & Layer Data
  const [kicadInfo, setKicadInfo] = useState<{ available: boolean; version: string; backend: string }>({
    available: true,
    version: "10.0.7",
    backend: "Host kicad-cli",
  });
  const [layerConfigs, setLayerConfigs] = useState<PcbLayerConfig[]>(INITIAL_LAYERS);
  const [activeLayer, setActiveLayer] = useState<string>("F.Cu");
  const [renderedLayers, setRenderedLayers] = useState<LayerRenderResult[]>([]);
  const [loadingLayers, setLoadingLayers] = useState<boolean>(true);

  // DRC State
  const [drcResult, setDrcResult] = useState<DrcResult | null>(null);
  const [isRunningDrc, setIsRunningDrc] = useState<boolean>(false);
  const [showDrcPins, setShowDrcPins] = useState<boolean>(true);

  // Review Pins & Threads State
  const [threads, setThreads] = useState<ReviewThread[]>([]);
  const [selectedThreadId, setSelectedThreadId] = useState<string | null>(null);
  const [isReviewPanelOpen, setIsReviewPanelOpen] = useState<boolean>(true);
  const [newPinDraft, setNewPinDraft] = useState<{ x: number; y: number; layer: string } | null>(null);

  // Cloud Sync Modal
  const [isCloudModalOpen, setIsCloudModalOpen] = useState<boolean>(false);

  // Initial load
  useEffect(() => {
    api.getKiCadVersion().then(setKicadInfo);
    loadProjectAssets(selectedProject);
    loadThreads();
  }, [selectedProject]);

  const loadThreads = async () => {
    const threadList = await api.getReviewThreads();
    setThreads(threadList);
  };

  const loadProjectAssets = async (proj: ProjectSample) => {
    setLoadingLayers(true);
    try {
      const layersToFetch = layerConfigs.map((l) => l.id);
      const layers = await api.renderPcbLayers(
        proj.sampleDir,
        proj.pcbFile,
        layersToFetch,
        proj.sampleDir
      );
      setRenderedLayers(layers);

      // Load DRC
      const drc = await api.executeDrc(proj.sampleDir, proj.pcbFile, proj.sampleDir);
      setDrcResult(drc);
    } catch (err) {
      console.error("Failed to load project assets:", err);
    } finally {
      setLoadingLayers(false);
    }
  };

  const handleRunDrc = async () => {
    setIsRunningDrc(true);
    try {
      const drc = await api.executeDrc(
        selectedProject.sampleDir,
        selectedProject.pcbFile,
        selectedProject.sampleDir
      );
      setDrcResult(drc);
    } catch (e) {
      console.error("DRC error:", e);
    } finally {
      setIsRunningDrc(false);
    }
  };

  // Layer toggling
  const handleToggleLayer = (id: string) => {
    setLayerConfigs((prev) =>
      prev.map((l) => (l.id === id ? { ...l, visible: !l.visible } : l))
    );
  };

  const handleUpdateOpacity = (id: string, opacity: number) => {
    setLayerConfigs((prev) =>
      prev.map((l) => (l.id === id ? { ...l, opacity } : l))
    );
  };

  // Redline Pin drop
  const handleDropPin = (coords: { x: number; y: number; layer: string }) => {
    setNewPinDraft(coords);
    setIsReviewPanelOpen(true);
  };

  const handleCreateThread = async (author: string, message: string) => {
    if (!newPinDraft) return;
    const newThread = await api.createReviewThread(
      {
        revision_id: "rev-head",
        file_type: "pcb",
        sheet_or_layer: newPinDraft.layer,
        coord_x: newPinDraft.x,
        coord_y: newPinDraft.y,
        status: "open",
        created_by: author,
      },
      message
    );
    setThreads((prev) => [newThread, ...prev]);
    setSelectedThreadId(newThread.id);
    setNewPinDraft(null);
  };

  const handleAddMessage = async (threadId: string, author: string, body: string) => {
    await api.addReviewMessage(threadId, author, body);
    loadThreads();
  };

  const handleToggleThreadStatus = async (threadId: string, newStatus: "open" | "resolved") => {
    await api.updateThreadStatus(threadId, newStatus);
    loadThreads();
  };

  return (
    <div className="w-screen h-screen flex flex-col bg-slate-950 text-slate-100 overflow-hidden font-sans">
      {/* Top Application Bar */}
      <Header
        currentView={currentView}
        onViewChange={setCurrentView}
        dualMode={dualMode}
        onDualModeChange={setDualMode}
        kicadInfo={kicadInfo}
        projects={SAMPLE_PROJECTS}
        selectedProject={selectedProject}
        onSelectProject={setSelectedProject}
        onOpenCloudSync={() => setIsCloudModalOpen(true)}
        drcViolationsCount={drcResult?.violations_count || 0}
        openThreadsCount={threads.filter((t) => t.status === "open").length}
      />

      {/* Main Workspace Area */}
      <div className="flex-1 flex overflow-hidden relative">
        {currentView === "pcb" && (
          <>
            <PcbCanvas
              layers={renderedLayers}
              layerConfigs={layerConfigs}
              onToggleLayer={handleToggleLayer}
              onUpdateOpacity={handleUpdateOpacity}
              threads={threads}
              drcViolations={drcResult?.violations || []}
              showDrcPins={showDrcPins}
              onToggleDrcPins={() => setShowDrcPins(!showDrcPins)}
              selectedThreadId={selectedThreadId}
              onSelectThread={(t) => {
                setSelectedThreadId(t.id);
                setIsReviewPanelOpen(true);
              }}
              onDropPin={handleDropPin}
              activeLayer={activeLayer}
              setActiveLayer={setActiveLayer}
            />

            {isReviewPanelOpen && (
              <ReviewPanel
                threads={threads}
                selectedThreadId={selectedThreadId}
                onSelectThread={(t) => setSelectedThreadId(t.id)}
                onAddMessage={handleAddMessage}
                onToggleStatus={handleToggleThreadStatus}
                onClose={() => setIsReviewPanelOpen(false)}
                newPinDraft={newPinDraft}
                onCreateThread={handleCreateThread}
                onCancelNewPin={() => setNewPinDraft(null)}
              />
            )}
          </>
        )}

        {currentView === "diff" && (
          <VisualDiff currentProjectDir={selectedProject.sampleDir} />
        )}

        {currentView === "schematic" && (
          <SchematicViewer sheets={selectedProject.schematicSheets} />
        )}

        {currentView === "drc" && (
          <DrcInspector
            drcResult={drcResult}
            onRunDrc={handleRunDrc}
            isRunningDrc={isRunningDrc}
            onFocusCoordinate={(coords) => {
              setCurrentView("pcb");
              setShowDrcPins(true);
            }}
          />
        )}
      </div>

      {/* Cloud & Docker Architecture Sync Modal */}
      <CloudSyncModal
        isOpen={isCloudModalOpen}
        onClose={() => setIsCloudModalOpen(false)}
        dualMode={dualMode}
      />
    </div>
  );
}

export default App;
