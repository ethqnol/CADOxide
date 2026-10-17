import React, { useState, useEffect, useRef } from "react";
import {
  SplitSquareVertical,
  Sparkles,
  ArrowRight,
  ZoomIn,
  ZoomOut,
  RotateCcw,
  Cpu,
} from "lucide-react";
import { getWasmEngine } from "../lib/wgpuEngine";

interface VisualDiffProps {
  currentProjectDir: string;
}

export const VisualDiff: React.FC<VisualDiffProps> = ({ currentProjectDir }) => {
  const [diffMode, setDiffMode] = useState<"wipe" | "blend">("blend");
  const [wipePosition, setWipePosition] = useState<number>(50);
  const [selectedLayer, setSelectedLayer] = useState<string>("F.Cu");
  const [zoom, setZoom] = useState<number>(1.0);
  const [pan, setPan] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [isDragging, setIsDragging] = useState<boolean>(false);
  const [dragStart, setDragStart] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [loading, setLoading] = useState<boolean>(true);
  const [diffMetrics, setDiffMetrics] = useState<{
    addedPixels: number;
    removedPixels: number;
    unchangedPixels: number;
    deltaPercent: string;
    wasmTimeMs: string;
  }>({
    addedPixels: 0,
    removedPixels: 0,
    unchangedPixels: 0,
    deltaPercent: "0.00",
    wasmTimeMs: "0.0",
  });

  const diffCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const imgARef = useRef<HTMLImageElement | null>(null);
  const imgBRef = useRef<HTMLImageElement | null>(null);
  const computedDiffCanvasRef = useRef<HTMLCanvasElement | null>(null);

  const layerFileSuffix: Record<string, string> = {
    "F.Cu": "top_copper",
    "B.Cu": "bottom_copper",
    "F.SilkS": "F_Silkscreen",
    "Edge.Cuts": "Edge_Cuts",
  };

  useEffect(() => {
    let active = true;
    async function computeWasmDiff() {
      setLoading(true);
      const suffix = layerFileSuffix[selectedLayer] || "top_copper";
      const urlA = `/samples/complex_hierarchy/complex_hierarchy-${suffix}.svg`;
      const urlB = `/samples/complex_hierarchy_revB/complex_hierarchy-${suffix}.svg`;

      try {
        const [imgA, imgB, wasm] = await Promise.all([
          loadImage(urlA),
          loadImage(urlB),
          getWasmEngine(),
        ]);
        if (!active) return;
        imgARef.current = imgA;
        imgBRef.current = imgB;

        const width = 1600;
        const height = 1130;
        const byteLen = width * height * 4;

        const canvasA = document.createElement("canvas");
        canvasA.width = width;
        canvasA.height = height;
        const ctxA = canvasA.getContext("2d")!;
        ctxA.drawImage(imgA, 0, 0, width, height);
        const dataA = ctxA.getImageData(0, 0, width, height).data;

        const canvasB = document.createElement("canvas");
        canvasB.width = width;
        canvasB.height = height;
        const ctxB = canvasB.getContext("2d")!;
        ctxB.drawImage(imgB, 0, 0, width, height);
        const dataB = ctxB.getImageData(0, 0, width, height).data;

        // Allocate 3 contiguous RGBA buffers in Rust WASM linear memory
        const t0 = performance.now();
        const ptrA = wasm.wasm_alloc(byteLen);
        const ptrB = wasm.wasm_alloc(byteLen);
        const ptrOut = wasm.wasm_alloc(byteLen);

        new Uint8Array(wasm.memory.buffer, ptrA, byteLen).set(dataA);
        new Uint8Array(wasm.memory.buffer, ptrB, byteLen).set(dataB);

        // Execute compiled WASM boolean diff kernel
        wasm.wasm_compute_rgba_diff(ptrA, ptrB, ptrOut, byteLen);
        const wasmTimeMs = (performance.now() - t0).toFixed(2);

        const outCanvas = document.createElement("canvas");
        outCanvas.width = width;
        outCanvas.height = height;
        const outCtx = outCanvas.getContext("2d")!;
        const outImgData = outCtx.createImageData(width, height);
        outImgData.data.set(new Uint8ClampedArray(wasm.memory.buffer, ptrOut, byteLen));
        outCtx.putImageData(outImgData, 0, 0);
        computedDiffCanvasRef.current = outCanvas;

        // Read [added, removed, unchanged] counters from WASM memory
        const statsPtr = wasm.wasm_diff_stats_ptr();
        const stats = new Uint32Array(wasm.memory.buffer, statsPtr, 3);
        const added = stats[0];
        const removed = stats[1];
        const unchanged = stats[2];

        wasm.wasm_dealloc(ptrA, byteLen);
        wasm.wasm_dealloc(ptrB, byteLen);
        wasm.wasm_dealloc(ptrOut, byteLen);

        const totalCopper = Math.max(1, added + removed + unchanged);
        const delta = (((added + removed) / totalCopper) * 100).toFixed(2);
        setDiffMetrics({
          addedPixels: added,
          removedPixels: removed,
          unchangedPixels: unchanged,
          deltaPercent: delta,
          wasmTimeMs,
        });
      } catch (err) {
        console.error("WASM diff failed:", err);
      } finally {
        if (active) setLoading(false);
      }
    }

    computeWasmDiff();
    return () => {
      active = false;
    };
  }, [currentProjectDir, selectedLayer]);

  useEffect(() => {
    const canvas = diffCanvasRef.current;
    if (!canvas || loading) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const width = 1600;
    const height = 1130;
    canvas.width = width;
    canvas.height = height;

    ctx.fillStyle = "#090d16";
    ctx.fillRect(0, 0, width, height);

    if (diffMode === "blend" && computedDiffCanvasRef.current) {
      ctx.drawImage(computedDiffCanvasRef.current, 0, 0);
    } else if (diffMode === "wipe" && imgARef.current && imgBRef.current) {
      const splitX = (wipePosition / 100) * width;

      ctx.save();
      ctx.beginPath();
      ctx.rect(0, 0, splitX, height);
      ctx.clip();
      ctx.drawImage(imgARef.current, 0, 0, width, height);
      ctx.restore();

      ctx.save();
      ctx.beginPath();
      ctx.rect(splitX, 0, width - splitX, height);
      ctx.clip();
      ctx.drawImage(imgBRef.current, 0, 0, width, height);
      ctx.restore();

      ctx.strokeStyle = "#22d3ee";
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.moveTo(splitX, 0);
      ctx.lineTo(splitX, height);
      ctx.stroke();
    }
  }, [diffMode, wipePosition, loading]);

  return (
    <div className="flex-1 h-full flex flex-col bg-slate-950 overflow-hidden select-none">
      {/* Top Diff Header Bar */}
      <div className="h-12 bg-slate-900 border-b border-slate-800 px-4 flex items-center justify-between shrink-0">
        <div className="flex items-center space-x-3 text-xs">
          <div className="flex items-center space-x-1.5 px-2 py-0.5 rounded bg-indigo-500/15 border border-indigo-500/30 text-indigo-300 font-mono text-[11px]">
            <Cpu className="w-3.5 h-3.5" />
            <span>WASM Diff Kernel: {diffMetrics.wasmTimeMs}ms</span>
          </div>

          <div className="flex items-center space-x-1.5 px-2.5 py-1 rounded bg-rose-500/10 border border-rose-500/30 text-rose-300 font-mono">
            <span className="w-2 h-2 rounded-full bg-rose-500" />
            <span className="font-semibold">Rev A:</span>
            <span>a1b2c3d</span>
          </div>

          <ArrowRight className="w-3.5 h-3.5 text-slate-500" />

          <div className="flex items-center space-x-1.5 px-2.5 py-1 rounded bg-emerald-500/10 border border-emerald-500/30 text-emerald-300 font-mono">
            <span className="w-2 h-2 rounded-full bg-emerald-500" />
            <span className="font-semibold">Rev B:</span>
            <span>b2c3d4e</span>
          </div>

          <div className="h-4 w-px bg-slate-800 mx-1" />

          <div className="hidden lg:flex items-center space-x-3 font-mono text-[11px]">
            <span className="text-emerald-400">+{diffMetrics.addedPixels.toLocaleString()} px</span>
            <span className="text-rose-400">-{diffMetrics.removedPixels.toLocaleString()} px</span>
            <span className="text-slate-400">Δ {diffMetrics.deltaPercent}% copper</span>
          </div>
        </div>

        {/* Diff Mode & Layer Selector */}
        <div className="flex items-center space-x-2">
          <div className="flex items-center bg-slate-950 border border-slate-800 rounded-lg p-0.5 text-xs">
            <button
              onClick={() => setDiffMode("blend")}
              className={`flex items-center space-x-1 px-3 py-1 rounded-md transition-colors ${
                diffMode === "blend"
                  ? "bg-indigo-600 text-white font-medium shadow-sm"
                  : "text-slate-400 hover:text-slate-200"
              }`}
            >
              <Sparkles className="w-3.5 h-3.5" />
              <span>WASM Boolean Pass</span>
            </button>
            <button
              onClick={() => setDiffMode("wipe")}
              className={`flex items-center space-x-1 px-3 py-1 rounded-md transition-colors ${
                diffMode === "wipe"
                  ? "bg-indigo-600 text-white font-medium shadow-sm"
                  : "text-slate-400 hover:text-slate-200"
              }`}
            >
              <SplitSquareVertical className="w-3.5 h-3.5" />
              <span>Split Wipe Slider</span>
            </button>
          </div>

          <select
            value={selectedLayer}
            onChange={(e) => setSelectedLayer(e.target.value)}
            className="bg-slate-950 border border-slate-700 text-xs text-slate-200 rounded-md px-2 py-1 focus:outline-none"
          >
            <option value="F.Cu">F.Cu (Top Copper)</option>
            <option value="B.Cu">B.Cu (Bottom Copper)</option>
            <option value="F.SilkS">F.SilkS (Silkscreen)</option>
            <option value="Edge.Cuts">Edge.Cuts (Outline)</option>
          </select>
        </div>
      </div>

      {/* Main Viewport */}
      <div
        onWheel={(e) => {
          e.preventDefault();
          const factor = e.deltaY < 0 ? 1.15 : 0.87;
          setZoom((z) => Math.max(0.3, Math.min(10, z * factor)));
        }}
        onMouseDown={(e) => {
          if (e.button === 0) {
            setIsDragging(true);
            setDragStart({ x: e.clientX - pan.x, y: e.clientY - pan.y });
          }
        }}
        onMouseMove={(e) => {
          if (isDragging) {
            setPan({ x: e.clientX - dragStart.x, y: e.clientY - dragStart.y });
          }
        }}
        onMouseUp={() => setIsDragging(false)}
        className="relative flex-1 overflow-hidden flex items-center justify-center bg-slate-950 cursor-grab active:cursor-grabbing"
      >
        <div className="absolute top-4 right-4 z-20 flex items-center space-x-1 bg-slate-900/90 border border-slate-800 rounded-lg p-1 shadow-xl">
          <button
            onClick={() => setZoom((z) => Math.min(10, z * 1.25))}
            className="p-1.5 rounded hover:bg-slate-800 text-slate-300"
          >
            <ZoomIn className="w-4 h-4" />
          </button>
          <button
            onClick={() => setZoom((z) => Math.max(0.3, z * 0.8))}
            className="p-1.5 rounded hover:bg-slate-800 text-slate-300"
          >
            <ZoomOut className="w-4 h-4" />
          </button>
          <button
            onClick={() => {
              setZoom(1.0);
              setPan({ x: 0, y: 0 });
            }}
            className="p-1.5 rounded hover:bg-slate-800 text-slate-300"
          >
            <RotateCcw className="w-4 h-4" />
          </button>
        </div>

        <div className="absolute top-4 left-4 z-20 bg-slate-900/90 backdrop-blur-md border border-slate-800 rounded-lg p-3 shadow-xl space-y-1.5 text-xs">
          <div className="font-semibold text-slate-200 text-[11px] uppercase tracking-wider mb-1">
            Rust WASM Linear Memory Diff
          </div>
          <div className="flex items-center space-x-2">
            <span className="w-3 h-3 rounded bg-[#22c55e]" />
            <span className="text-slate-300">Added Copper (R_B &gt; 0 &amp; R_A = 0)</span>
          </div>
          <div className="flex items-center space-x-2">
            <span className="w-3 h-3 rounded bg-[#ef4444]" />
            <span className="text-slate-300">Removed Copper (R_A &gt; 0 &amp; R_B = 0)</span>
          </div>
          <div className="flex items-center space-x-2">
            <span className="w-3 h-3 rounded bg-[#4A5568]" />
            <span className="text-slate-400">Unchanged Copper (R_A &gt; 0 &amp; R_B &gt; 0)</span>
          </div>
        </div>

        {loading ? (
          <div className="flex flex-col items-center space-y-2 text-slate-400">
            <div className="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin" />
            <span className="text-xs font-mono">Executing Rust WASM boolean pixel pass...</span>
          </div>
        ) : (
          <div
            style={{
              transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`,
              transformOrigin: "center center",
            }}
            className="shadow-2xl rounded-xl border border-slate-800 overflow-hidden"
          >
            <canvas ref={diffCanvasRef} className="w-[960px] h-[678px] block" />
          </div>
        )}

        {diffMode === "wipe" && (
          <div
            onMouseDown={(e) => e.stopPropagation()}
            className="absolute bottom-6 z-30 bg-slate-900/95 backdrop-blur-md border border-slate-800 rounded-xl px-4 py-2.5 shadow-2xl flex items-center space-x-4 min-w-[380px]"
          >
            <span className="text-xs font-mono text-rose-400 font-semibold">Rev A</span>
            <input
              type="range"
              min="0"
              max="100"
              value={wipePosition}
              onChange={(e) => setWipePosition(Number(e.target.value))}
              className="flex-1 h-1.5 bg-slate-700 rounded-lg appearance-none cursor-ew-resize accent-cyan-400"
            />
            <span className="text-xs font-mono text-emerald-400 font-semibold">Rev B</span>
            <span className="text-xs font-mono text-slate-400 w-10 text-right">{wipePosition}%</span>
          </div>
        )}
      </div>
    </div>
  );
};

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = url;
  });
}
