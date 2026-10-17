import React, { useState, useRef, useEffect } from "react";
import {
  ZoomIn,
  ZoomOut,
  Maximize2,
  RotateCcw,
  MapPin,
  Eye,
  EyeOff,
  Sliders,
  Check,
  AlertTriangle,
  Layers,
  Crosshair,
} from "lucide-react";
import { LayerRenderResult, PcbLayerConfig, ReviewThread, DrcViolation } from "../lib/types";

interface PcbCanvasProps {
  layers: LayerRenderResult[];
  layerConfigs: PcbLayerConfig[];
  onToggleLayer: (id: string) => void;
  onUpdateOpacity: (id: string, opacity: number) => void;
  threads: ReviewThread[];
  drcViolations: DrcViolation[];
  showDrcPins: boolean;
  onToggleDrcPins: () => void;
  selectedThreadId: string | null;
  onSelectThread: (thread: ReviewThread) => void;
  onDropPin: (coords: { x: number; y: number; layer: string }) => void;
  activeLayer: string;
  setActiveLayer: (layer: string) => void;
}

export const PcbCanvas: React.FC<PcbCanvasProps> = ({
  layers,
  layerConfigs,
  onToggleLayer,
  onUpdateOpacity,
  threads,
  drcViolations,
  showDrcPins,
  onToggleDrcPins,
  selectedThreadId,
  onSelectThread,
  onDropPin,
  activeLayer,
  setActiveLayer,
}) => {
  // Transform State (Pan & Zoom)
  const [zoom, setZoom] = useState<number>(1);
  const [pan, setPan] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [isDragging, setIsDragging] = useState<boolean>(false);
  const [dragStart, setDragStart] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [cursorCoords, setCursorCoords] = useState<{ x: number; y: number } | null>(null);
  const [isDropPinMode, setIsDropPinMode] = useState<boolean>(false);
  const [showLayerPanel, setShowLayerPanel] = useState<boolean>(true);

  const containerRef = useRef<HTMLDivElement>(null);

  // Center & Fit Canvas on mount / board change
  useEffect(() => {
    handleResetView();
  }, [layers]);

  const handleResetView = () => {
    setZoom(1.1);
    setPan({ x: 0, y: 0 });
  };

  // Zoom controls
  const handleZoom = (delta: number) => {
    setZoom((prev) => Math.max(0.2, Math.min(15, prev * (1 + delta))));
  };

  const handleWheel = (e: React.WheelEvent) => {
    e.preventDefault();
    const zoomFactor = e.deltaY < 0 ? 0.15 : -0.15;
    handleZoom(zoomFactor);
  };

  // Drag / Pan handlers
  const handleMouseDown = (e: React.MouseEvent) => {
    if (e.button === 0) {
      if (isDropPinMode) {
        // Drop pin at board coordinate
        if (cursorCoords) {
          onDropPin({ x: cursorCoords.x, y: cursorCoords.y, layer: activeLayer });
          setIsDropPinMode(false);
        }
        return;
      }
      setIsDragging(true);
      setDragStart({ x: e.clientX - pan.x, y: e.clientY - pan.y });
    }
  };

  const handleMouseMove = (e: React.MouseEvent) => {
    if (isDragging) {
      setPan({
        x: e.clientX - dragStart.x,
        y: e.clientY - dragStart.y,
      });
    }

    // Estimate real board coordinates in mm from SVG center
    if (containerRef.current) {
      const rect = containerRef.current.getBoundingClientRect();
      const centerX = rect.width / 2 + pan.x;
      const centerY = rect.height / 2 + pan.y;
      const scaleMmPerPixel = 0.1 / zoom;

      const boardX = Number(((e.clientX - centerX) * scaleMmPerPixel + 130).toFixed(2));
      const boardY = Number(((e.clientY - centerY) * scaleMmPerPixel + 95).toFixed(2));
      setCursorCoords({ x: boardX, y: boardY });
    }
  };

  const handleMouseUp = () => {
    setIsDragging(false);
  };

  return (
    <div className="relative flex-1 h-full flex overflow-hidden bg-slate-950 select-none">
      {/* Top Floating Canvas Toolbar */}
      <div className="absolute top-4 left-4 z-20 flex items-center space-x-2 bg-slate-900/90 backdrop-blur-md border border-slate-800 rounded-lg p-1.5 shadow-xl">
        <button
          onClick={() => handleZoom(0.2)}
          className="p-1.5 rounded hover:bg-slate-800 text-slate-300 hover:text-white"
          title="Zoom In (+)"
        >
          <ZoomIn className="w-4 h-4" />
        </button>
        <button
          onClick={() => handleZoom(-0.2)}
          className="p-1.5 rounded hover:bg-slate-800 text-slate-300 hover:text-white"
          title="Zoom Out (-)"
        >
          <ZoomOut className="w-4 h-4" />
        </button>
        <button
          onClick={handleResetView}
          className="p-1.5 rounded hover:bg-slate-800 text-slate-300 hover:text-white"
          title="Reset View"
        >
          <RotateCcw className="w-4 h-4" />
        </button>
        <div className="w-px h-4 bg-slate-800" />
        <span className="text-[11px] font-mono text-slate-400 px-1 min-w-[48px] text-center">
          {Math.round(zoom * 100)}%
        </span>
        <div className="w-px h-4 bg-slate-800" />

        {/* Pin Drop Button */}
        <button
          onClick={() => setIsDropPinMode(!isDropPinMode)}
          className={`flex items-center space-x-1 px-2.5 py-1 rounded text-xs font-medium transition-colors ${
            isDropPinMode
              ? "bg-rose-600 text-white shadow-md shadow-rose-600/30 animate-pulse"
              : "bg-slate-800 text-slate-200 hover:bg-slate-700"
          }`}
          title="Click to drop a visual review pin on PCB coordinates"
        >
          <MapPin className="w-3.5 h-3.5" />
          <span>{isDropPinMode ? "Click Board..." : "Drop Pin"}</span>
        </button>

        {/* DRC Pin Overlay Toggle */}
        <button
          onClick={onToggleDrcPins}
          className={`flex items-center space-x-1 px-2 py-1 rounded text-xs transition-colors ${
            showDrcPins
              ? "bg-amber-500/20 text-amber-400 border border-amber-500/40"
              : "text-slate-400 hover:bg-slate-800"
          }`}
          title="Toggle DRC Violation Pin Overlay"
        >
          <AlertTriangle className="w-3.5 h-3.5" />
          <span>DRC ({drcViolations.length})</span>
        </button>

        {/* Toggle Layer Panel */}
        <button
          onClick={() => setShowLayerPanel(!showLayerPanel)}
          className={`p-1.5 rounded text-xs transition-colors ${
            showLayerPanel ? "bg-indigo-600/20 text-indigo-400" : "text-slate-400 hover:bg-slate-800"
          }`}
          title="Toggle PCB Layer Stack Panel"
        >
          <Layers className="w-4 h-4" />
        </button>
      </div>

      {/* Coordinate HUD (Real Board Coordinates in mm) */}
      <div className="absolute bottom-4 left-4 z-20 bg-slate-900/90 backdrop-blur-md border border-slate-800 rounded-md px-3 py-1.5 shadow-lg flex items-center space-x-3 text-xs font-mono">
        <div className="flex items-center space-x-1 text-slate-400">
          <Crosshair className="w-3.5 h-3.5 text-cyan-400" />
          <span>Board Coord:</span>
        </div>
        <div className="flex items-center space-x-2 text-slate-200">
          <span>X: <span className="text-cyan-400 font-semibold">{cursorCoords ? cursorCoords.x : "130.00"}</span> mm</span>
          <span>Y: <span className="text-cyan-400 font-semibold">{cursorCoords ? cursorCoords.y : "95.00"}</span> mm</span>
        </div>
        <div className="w-px h-3 bg-slate-700" />
        <span className="text-slate-400">Layer: <span className="text-amber-400">{activeLayer}</span></span>
      </div>

      {/* Main SVG Render Area */}
      <div
        ref={containerRef}
        onWheel={handleWheel}
        onMouseDown={handleMouseDown}
        onMouseMove={handleMouseMove}
        onMouseUp={handleMouseUp}
        className={`flex-1 h-full relative overflow-hidden flex items-center justify-center ${
          isDropPinMode ? "cursor-crosshair" : isDragging ? "cursor-grabbing" : "cursor-grab"
        }`}
      >
        {/* Transform Container */}
        <div
          className="relative transition-transform duration-75 ease-out select-none"
          style={{
            transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`,
            transformOrigin: "center center",
          }}
        >
          {/* KiCad SVG Layers Stack */}
          {layers.length === 0 ? (
            <div className="flex flex-col items-center justify-center p-12 text-slate-500">
              <div className="w-10 h-10 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin mb-3" />
              <p className="text-sm">Rendering PCB layers via kicad-cli...</p>
            </div>
          ) : (
            <div className="relative">
              {layerConfigs.map((cfg) => {
                if (!cfg.visible) return null;
                const layerData = layers.find((l) => l.layer === cfg.id);
                if (!layerData) return null;

                return (
                  <div
                    key={cfg.id}
                    className="absolute inset-0 pointer-events-none transition-opacity duration-150"
                    style={{
                      opacity: cfg.opacity,
                      filter:
                        cfg.id === "F.Cu"
                          ? "drop-shadow(0 0 1px rgba(245, 158, 11, 0.4))"
                          : cfg.id === "B.Cu"
                          ? "drop-shadow(0 0 1px rgba(59, 130, 246, 0.4))"
                          : undefined,
                    }}
                    dangerouslySetInnerHTML={{ __html: layerData.svg_content }}
                  />
                );
              })}

              {/* Edge Cuts Layer as Base Sizing Box */}
              {layers.find((l) => l.layer === "Edge.Cuts") && (
                <div
                  className="opacity-0 pointer-events-none"
                  dangerouslySetInnerHTML={{
                    __html: layers.find((l) => l.layer === "Edge.Cuts")!.svg_content,
                  }}
                />
              )}

              {/* Review Threads Pins Overlay */}
              <div className="absolute inset-0 pointer-events-auto">
                {threads.map((thread) => {
                  const isSelected = selectedThreadId === thread.id;
                  // Map board mm to canvas % offset
                  const leftPercent = Math.max(5, Math.min(95, ((thread.coord_x - 70) / 120) * 100));
                  const topPercent = Math.max(5, Math.min(95, ((thread.coord_y - 40) / 110) * 100));

                  return (
                    <div
                      key={thread.id}
                      onClick={(e) => {
                        e.stopPropagation();
                        onSelectThread(thread);
                      }}
                      style={{
                        left: `${leftPercent}%`,
                        top: `${topPercent}%`,
                      }}
                      className={`absolute -translate-x-1/2 -translate-y-full cursor-pointer group z-30 transition-transform ${
                        isSelected ? "scale-125 z-40" : "hover:scale-110"
                      }`}
                    >
                      <div className="relative flex flex-col items-center">
                        {/* Pin Head */}
                        <div
                          className={`w-7 h-7 rounded-full flex items-center justify-center shadow-lg border-2 ${
                            thread.status === "resolved"
                              ? "bg-emerald-600 border-white text-white"
                              : "bg-rose-600 border-white text-white animate-bounce"
                          }`}
                        >
                          <MapPin className="w-4 h-4 fill-current" />
                        </div>
                        {/* Tooltip on Hover */}
                        <div className="hidden group-hover:flex absolute bottom-full mb-1 flex-col items-center pointer-events-none z-50">
                          <div className="bg-slate-900 border border-slate-700 text-slate-100 rounded px-2.5 py-1 text-[11px] whitespace-nowrap shadow-xl">
                            <span className="font-semibold text-rose-400">{thread.created_by}:</span>{" "}
                            {thread.messages[0]?.body.slice(0, 40)}...
                            <div className="text-[9px] text-slate-400 mt-0.5">
                              ({thread.coord_x}mm, {thread.coord_y}mm) • {thread.sheet_or_layer}
                            </div>
                          </div>
                        </div>
                      </div>
                    </div>
                  );
                })}

                {/* DRC Violations Pin Overlay */}
                {showDrcPins &&
                  drcViolations.map((v, idx) => {
                    if (v.pos_x === undefined || v.pos_y === undefined) return null;
                    const leftPercent = Math.max(5, Math.min(95, ((v.pos_x - 70) / 120) * 100));
                    const topPercent = Math.max(5, Math.min(95, ((v.pos_y - 40) / 110) * 100));

                    return (
                      <div
                        key={idx}
                        style={{
                          left: `${leftPercent}%`,
                          top: `${topPercent}%`,
                        }}
                        className="absolute -translate-x-1/2 -translate-y-1/2 cursor-pointer group z-20"
                      >
                        <div className="w-5 h-5 rounded-full bg-amber-500/80 border border-amber-300 flex items-center justify-center text-slate-950 font-bold text-[10px] shadow-md hover:scale-125 transition-transform">
                          !
                        </div>
                        <div className="hidden group-hover:flex absolute bottom-full mb-1 flex-col items-center pointer-events-none z-50">
                          <div className="bg-slate-900 border border-amber-500/50 text-slate-100 rounded px-2 py-1 text-[11px] whitespace-nowrap shadow-xl">
                            <span className="font-semibold text-amber-400">DRC Violation:</span> {v.description}
                            <div className="text-[9px] text-slate-400">
                              ({v.pos_x}mm, {v.pos_y}mm)
                            </div>
                          </div>
                        </div>
                      </div>
                    );
                  })}
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Layer Stack Side Panel */}
      {showLayerPanel && (
        <aside className="w-64 bg-slate-900 border-l border-slate-800 flex flex-col z-20 shrink-0">
          <div className="p-3 border-b border-slate-800 flex items-center justify-between">
            <div className="flex items-center space-x-1.5">
              <Layers className="w-4 h-4 text-indigo-400" />
              <span className="text-xs font-semibold text-slate-200 uppercase tracking-wider">
                PCB Layer Stack
              </span>
            </div>
            <span className="text-[10px] text-slate-400 font-mono">
              {layerConfigs.filter((c) => c.visible).length}/{layerConfigs.length} Active
            </span>
          </div>

          <div className="flex-1 overflow-y-auto p-2 space-y-1.5">
            {layerConfigs.map((layer) => {
              const isActive = activeLayer === layer.id;
              return (
                <div
                  key={layer.id}
                  onClick={() => setActiveLayer(layer.id)}
                  className={`p-2 rounded-lg border transition-all cursor-pointer ${
                    isActive
                      ? "bg-slate-800 border-indigo-500/50 shadow-sm"
                      : "bg-slate-950/60 border-slate-800 hover:border-slate-700"
                  }`}
                >
                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          onToggleLayer(layer.id);
                        }}
                        className={`p-1 rounded transition-colors ${
                          layer.visible ? "text-indigo-400 hover:text-indigo-300" : "text-slate-600 hover:text-slate-400"
                        }`}
                      >
                        {layer.visible ? <Eye className="w-3.5 h-3.5" /> : <EyeOff className="w-3.5 h-3.5" />}
                      </button>

                      <div
                        className="w-3 h-3 rounded-full border border-white/20"
                        style={{ backgroundColor: layer.color }}
                      />

                      <div>
                        <div className="text-xs font-medium text-slate-200">{layer.name}</div>
                        <div className="text-[10px] text-slate-400 font-mono">{layer.id}</div>
                      </div>
                    </div>

                    {layer.isCopper && (
                      <span className="text-[9px] uppercase font-mono px-1 py-0.5 rounded bg-amber-500/10 text-amber-400 border border-amber-500/20">
                        Copper
                      </span>
                    )}
                  </div>

                  {/* Opacity Slider */}
                  {layer.visible && (
                    <div className="mt-2 pt-2 border-t border-slate-800/80 flex items-center space-x-2">
                      <span className="text-[10px] text-slate-500">Opacity:</span>
                      <input
                        type="range"
                        min="0.1"
                        max="1"
                        step="0.05"
                        value={layer.opacity}
                        onChange={(e) => {
                          e.stopPropagation();
                          onUpdateOpacity(layer.id, parseFloat(e.target.value));
                        }}
                        className="flex-1 h-1 bg-slate-700 rounded-lg appearance-none cursor-pointer accent-indigo-500"
                      />
                      <span className="text-[10px] text-slate-400 font-mono w-6 text-right">
                        {Math.round(layer.opacity * 100)}%
                      </span>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </aside>
      )}
    </div>
  );
};
