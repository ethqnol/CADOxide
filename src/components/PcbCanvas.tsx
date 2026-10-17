import React, { useState, useRef, useEffect } from "react";
import {
  ZoomIn,
  ZoomOut,
  RotateCcw,
  MapPin,
  Eye,
  EyeOff,
  AlertTriangle,
  Layers,
  Crosshair,
  Zap,
  Cpu,
  Flame,
} from "lucide-react";
import { LayerRenderResult, PcbLayerConfig, ReviewThread, DrcViolation } from "../lib/types";
import { HighPerfBoardRenderer } from "../lib/gpuRenderer";
import { WasmWgpuRenderer } from "../lib/wgpuEngine";

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
  pcbSamplePath?: string;
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
  pcbSamplePath = "/samples/complex_hierarchy/complex_hierarchy.kicad_pcb",
}) => {
  const [engineMode, setEngineMode] = useState<"wgpu-wasm" | "texture-compositor">("wgpu-wasm");
  const [wgpuStats, setWgpuStats] = useState<{
    backend: string;
    primitives: number;
    parseMs: string;
  }>({ backend: "WebGPU / WASM", primitives: 0, parseMs: "0.0" });

  const [zoomPercent, setZoomPercent] = useState<number>(100);
  const [isDragging, setIsDragging] = useState<boolean>(false);
  const [isDropPinMode, setIsDropPinMode] = useState<boolean>(false);
  const [showLayerPanel, setShowLayerPanel] = useState<boolean>(true);
  const [hoverTooltip, setHoverTooltip] = useState<{
    x: number;
    y: number;
    title: string;
    subtitle: string;
    badgeColor: string;
  } | null>(null);

  const texCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const wgpuCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const texRendererRef = useRef<HighPerfBoardRenderer | null>(null);
  const wgpuRendererRef = useRef<WasmWgpuRenderer | null>(null);

  const lastMouseRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  const dragStartRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  const coordsSpanRef = useRef<HTMLSpanElement | null>(null);

  // 1. Initialize Zero-Copy Rust WASM + WebGPU / WebGL2 Instanced SDF Engine
  useEffect(() => {
    if (engineMode !== "wgpu-wasm" || !wgpuCanvasRef.current) return;
    const wgpuEngine = new WasmWgpuRenderer();
    wgpuRendererRef.current = wgpuEngine;

    let active = true;
    wgpuEngine.attach(wgpuCanvasRef.current).then(async () => {
      if (!active) return;
      wgpuEngine.setLayerConfigs(layerConfigs);
      await loadRawPcbIntoWasm(wgpuEngine, pcbSamplePath);
    });

    const handleResize = () => wgpuEngine.resize();
    window.addEventListener("resize", handleResize);
    return () => {
      active = false;
      window.removeEventListener("resize", handleResize);
      wgpuEngine.detach();
    };
  }, [engineMode, pcbSamplePath]);

  const loadRawPcbIntoWasm = async (engine: WasmWgpuRenderer, url: string) => {
    try {
      const t0 = performance.now();
      const resp = await fetch(url);
      if (resp.ok) {
        const pcbSource = await resp.text();
        const count = await engine.loadKicadPcbSource(pcbSource);
        const dt = (performance.now() - t0).toFixed(1);
        setWgpuStats({
          backend: engine.getBackendName(),
          primitives: count,
          parseMs: dt,
        });
        setZoomPercent(engine.getZoomPercent());
      }
    } catch (e) {
      console.error("Failed to load raw .kicad_pcb into WASM:", e);
    }
  };

  const handleRun100kStressTest = async () => {
    if (engineMode !== "wgpu-wasm") {
      setEngineMode("wgpu-wasm");
      return;
    }
    const engine = wgpuRendererRef.current;
    if (!engine) return;
    const t0 = performance.now();
    const count = await engine.runHdiStressTest(100_000);
    const dt = (performance.now() - t0).toFixed(1);
    setWgpuStats({
      backend: engine.getBackendName(),
      primitives: count,
      parseMs: dt,
    });
    setZoomPercent(engine.getZoomPercent());
  };

  // 2. Initialize Texture Compositor fallback when selected
  useEffect(() => {
    if (engineMode !== "texture-compositor" || !texCanvasRef.current) return;
    const engine = new HighPerfBoardRenderer();
    texRendererRef.current = engine;
    engine.attach(texCanvasRef.current);
    engine.setTransform({ x: 0, y: 0, scale: 0.85 });

    layers.forEach((l) => {
      const cfg = layerConfigs.find((c) => c.id === l.layer);
      engine.loadLayerSvg(l.layer, l.svg_content, cfg ? cfg.color : "#f59e0b");
    });
    engine.setState({
      layers: layerConfigs,
      threads,
      drcViolations,
      showDrcPins,
      selectedThreadId,
    });

    const handleResize = () => engine.resize();
    window.addEventListener("resize", handleResize);
    return () => {
      window.removeEventListener("resize", handleResize);
      engine.detach();
    };
  }, [engineMode, layers]);

  // Sync layer visibility & opacities to active engine
  useEffect(() => {
    if (engineMode === "wgpu-wasm") {
      wgpuRendererRef.current?.setLayerConfigs(layerConfigs);
    } else {
      texRendererRef.current?.setState({
        layers: layerConfigs,
        threads,
        drcViolations,
        showDrcPins,
        selectedThreadId,
      });
    }
  }, [engineMode, layerConfigs, threads, drcViolations, showDrcPins, selectedThreadId]);

  const handleResetView = () => {
    if (engineMode === "wgpu-wasm" && wgpuRendererRef.current) {
      wgpuRendererRef.current.fitToBoard();
      setZoomPercent(wgpuRendererRef.current.getZoomPercent());
    } else if (texRendererRef.current) {
      texRendererRef.current.setTransform({ x: 0, y: 0, scale: 0.85 });
      setZoomPercent(85);
    }
  };

  const handleZoom = (delta: number) => {
    if (engineMode === "wgpu-wasm" && wgpuRendererRef.current && wgpuCanvasRef.current) {
      const rect = wgpuCanvasRef.current.getBoundingClientRect();
      wgpuRendererRef.current.zoomAtScreenPx(
        1 + delta,
        rect.left + rect.width * 0.5,
        rect.top + rect.height * 0.5
      );
      setZoomPercent(wgpuRendererRef.current.getZoomPercent());
    } else if (texRendererRef.current) {
      const cur = texRendererRef.current.getTransform();
      const nextScale = Math.max(0.15, Math.min(20, cur.scale * (1 + delta)));
      texRendererRef.current.setTransform({ ...cur, scale: nextScale });
      setZoomPercent(Math.round(nextScale * 100));
    }
  };

  const handleWheel = (e: React.WheelEvent) => {
    e.preventDefault();
    const factor = e.deltaY < 0 ? 1.16 : 0.86;

    if (engineMode === "wgpu-wasm" && wgpuRendererRef.current) {
      wgpuRendererRef.current.zoomAtScreenPx(factor, e.clientX, e.clientY);
      setZoomPercent(wgpuRendererRef.current.getZoomPercent());
      return;
    }

    const engine = texRendererRef.current;
    if (!engine || !texCanvasRef.current) return;
    const cur = engine.getTransform();
    const nextScale = Math.max(0.15, Math.min(20, cur.scale * factor));
    const rect = texCanvasRef.current.getBoundingClientRect();
    const mouseX = e.clientX - rect.left - rect.width / 2;
    const mouseY = e.clientY - rect.top - rect.height / 2;
    const nextX = mouseX - ((mouseX - cur.x) * nextScale) / cur.scale;
    const nextY = mouseY - ((mouseY - cur.y) * nextScale) / cur.scale;

    engine.setTransform({ x: nextX, y: nextY, scale: nextScale });
    setZoomPercent(Math.round(nextScale * 100));
  };

  const handleMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;

    if (isDropPinMode) {
      const mm =
        engineMode === "wgpu-wasm" && wgpuRendererRef.current
          ? wgpuRendererRef.current.screenToBoardMm(e.clientX, e.clientY)
          : texRendererRef.current?.screenToBoardMm(e.clientX, e.clientY) || { x: 130, y: 95 };
      onDropPin({ x: mm.x, y: mm.y, layer: activeLayer });
      setIsDropPinMode(false);
      return;
    }

    if (engineMode === "texture-compositor" && texRendererRef.current) {
      const hit = texRendererRef.current.hitTest(e.clientX, e.clientY);
      if (hit && hit.type === "thread") {
        onSelectThread(hit.data);
        return;
      }
      const cur = texRendererRef.current.getTransform();
      dragStartRef.current = { x: e.clientX - cur.x, y: e.clientY - cur.y };
    }

    lastMouseRef.current = { x: e.clientX, y: e.clientY };
    setIsDragging(true);
  };

  const handleMouseMove = (e: React.MouseEvent) => {
    if (engineMode === "wgpu-wasm" && wgpuRendererRef.current) {
      if (isDragging) {
        const dx = e.clientX - lastMouseRef.current.x;
        const dy = e.clientY - lastMouseRef.current.y;
        lastMouseRef.current = { x: e.clientX, y: e.clientY };
        wgpuRendererRef.current.panByScreenPx(dx, dy);
      }
      if (coordsSpanRef.current) {
        const mm = wgpuRendererRef.current.screenToBoardMm(e.clientX, e.clientY);
        coordsSpanRef.current.textContent = `X: ${mm.x.toFixed(2)} mm   Y: ${mm.y.toFixed(2)} mm`;
      }
      return;
    }

    const engine = texRendererRef.current;
    if (!engine) return;
    if (isDragging) {
      const cur = engine.getTransform();
      engine.setTransform({
        ...cur,
        x: e.clientX - dragStartRef.current.x,
        y: e.clientY - dragStartRef.current.y,
      });
    } else {
      const hit = engine.hitTest(e.clientX, e.clientY);
      if (hit && texCanvasRef.current) {
        const rect = texCanvasRef.current.getBoundingClientRect();
        if (hit.type === "thread") {
          const t: ReviewThread = hit.data;
          setHoverTooltip({
            x: e.clientX - rect.left,
            y: e.clientY - rect.top - 12,
            title: `${t.created_by} (${t.sheet_or_layer})`,
            subtitle: t.messages[0]?.body || "Review pin",
            badgeColor: t.status === "resolved" ? "text-emerald-400" : "text-rose-400",
          });
        } else {
          const v: DrcViolation = hit.data;
          setHoverTooltip({
            x: e.clientX - rect.left,
            y: e.clientY - rect.top - 12,
            title: `DRC ${v.severity.toUpperCase()}: ${v.type_name || "Rule"}`,
            subtitle: v.description,
            badgeColor: "text-amber-400",
          });
        }
      } else if (hoverTooltip) {
        setHoverTooltip(null);
      }
    }

    if (coordsSpanRef.current) {
      const mm = engine.screenToBoardMm(e.clientX, e.clientY);
      coordsSpanRef.current.textContent = `X: ${mm.x.toFixed(2)} mm   Y: ${mm.y.toFixed(2)} mm`;
    }
  };

  const handleMouseUp = () => {
    setIsDragging(false);
  };

  return (
    <div className="relative flex-1 h-full flex overflow-hidden bg-slate-950 select-none">
      {/* Top Floating Canvas Toolbar */}
      <div className="absolute top-4 left-4 z-20 flex items-center space-x-2 bg-slate-900/95 backdrop-blur-md border border-slate-800 rounded-lg p-1.5 shadow-xl">
        {/* Engine Selector Pill */}
        <div className="flex items-center bg-slate-950 border border-slate-800 rounded-md p-0.5 text-[11px]">
          <button
            onClick={() => setEngineMode("wgpu-wasm")}
            className={`flex items-center space-x-1 px-2 py-0.5 rounded transition-colors ${
              engineMode === "wgpu-wasm"
                ? "bg-indigo-600 text-white font-semibold"
                : "text-slate-400 hover:text-slate-200"
            }`}
            title="Zero-Copy Rust WASM + WebGPU / WebGL2 Instanced SDF Shader"
          >
            <Cpu className="w-3 h-3" />
            <span>WASM + WebGPU SDF</span>
          </button>
          <button
            onClick={() => setEngineMode("texture-compositor")}
            className={`flex items-center space-x-1 px-2 py-0.5 rounded transition-colors ${
              engineMode === "texture-compositor"
                ? "bg-indigo-600 text-white font-semibold"
                : "text-slate-400 hover:text-slate-200"
            }`}
            title="Offscreen KiCad SVG Texture Compositor"
          >
            <Zap className="w-3 h-3" />
            <span>SVG Texture Stack</span>
          </button>
        </div>

        {/* 100k HDI Benchmark Trigger */}
        {engineMode === "wgpu-wasm" && (
          <button
            onClick={handleRun100kStressTest}
            className="flex items-center space-x-1 px-2 py-1 rounded bg-amber-500/15 hover:bg-amber-500/25 border border-amber-500/40 text-amber-300 text-[11px] font-mono transition-colors"
            title="Generate 100,000+ instanced HDI traces, microvias, and BGA pads in Rust WASM linear memory"
          >
            <Flame className="w-3 h-3 text-amber-400" />
            <span>100k HDI Stress Test</span>
          </button>
        )}

        <div className="w-px h-4 bg-slate-800" />

        <button
          onClick={() => handleZoom(0.25)}
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
          title="Fit Board to Screen"
        >
          <RotateCcw className="w-4 h-4" />
        </button>
        <span className="text-[11px] font-mono text-slate-400 px-1 min-w-[48px] text-center">
          {zoomPercent}%
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
          title="Toggle DRC Violation Markers"
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

      {/* Zero-React-Render Coordinate & WASM Telemetry HUD */}
      <div className="absolute bottom-4 left-4 z-20 bg-slate-900/95 backdrop-blur-md border border-slate-800 rounded-md px-3 py-1.5 shadow-lg flex items-center space-x-3 text-xs font-mono">
        <div className="flex items-center space-x-1 text-slate-400">
          <Crosshair className="w-3.5 h-3.5 text-cyan-400" />
          <span>Coord:</span>
        </div>
        <span ref={coordsSpanRef} className="text-cyan-400 font-semibold min-w-[175px]">
          X: 130.00 mm &nbsp; Y: 95.00 mm
        </span>
        <div className="w-px h-3 bg-slate-700" />
        {engineMode === "wgpu-wasm" ? (
          <div className="flex items-center space-x-3 text-[11px]">
            <span className="text-emerald-400 font-semibold">{wgpuStats.backend}</span>
            <span className="text-slate-300">
              {wgpuStats.primitives.toLocaleString()} instanced primitives (1 draw call)
            </span>
            <span className="text-slate-400">WASM Parse: {wgpuStats.parseMs}ms</span>
          </div>
        ) : (
          <span className="text-slate-400">
            Layer: <span className="text-amber-400">{activeLayer}</span>
          </span>
        )}
      </div>

      {/* Hardware-Accelerated Canvas Viewport */}
      <div
        onWheel={handleWheel}
        onMouseDown={handleMouseDown}
        onMouseMove={handleMouseMove}
        onMouseUp={handleMouseUp}
        onMouseLeave={handleMouseUp}
        className={`flex-1 h-full relative overflow-hidden ${
          isDropPinMode ? "cursor-crosshair" : isDragging ? "cursor-grabbing" : "cursor-grab"
        }`}
      >
        {engineMode === "wgpu-wasm" ? (
          <canvas key="wgpu-canvas" ref={wgpuCanvasRef} className="block w-full h-full" />
        ) : (
          <canvas key="tex-canvas" ref={texCanvasRef} className="block w-full h-full" />
        )}

        {hoverTooltip && engineMode === "texture-compositor" && (
          <div
            style={{ left: hoverTooltip.x, top: hoverTooltip.y }}
            className="absolute -translate-x-1/2 -translate-y-full pointer-events-none z-40 bg-slate-900/95 border border-slate-700 rounded-lg px-3 py-1.5 shadow-2xl max-w-xs"
          >
            <div className={`text-[11px] font-bold ${hoverTooltip.badgeColor}`}>
              {hoverTooltip.title}
            </div>
            <div className="text-xs text-slate-200 truncate">{hoverTooltip.subtitle}</div>
          </div>
        )}
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
                          layer.visible
                            ? "text-indigo-400 hover:text-indigo-300"
                            : "text-slate-600 hover:text-slate-400"
                        }`}
                      >
                        {layer.visible ? (
                          <Eye className="w-3.5 h-3.5" />
                        ) : (
                          <EyeOff className="w-3.5 h-3.5" />
                        )}
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
