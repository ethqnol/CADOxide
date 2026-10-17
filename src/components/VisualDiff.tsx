import React, { useState, useEffect } from "react";
import {
  GitCompare,
  Sliders,
  SplitSquareVertical,
  Layers,
  ZoomIn,
  ZoomOut,
  RotateCcw,
  Sparkles,
  ArrowRight,
} from "lucide-react";
import { LayerRenderResult } from "../lib/types";

interface VisualDiffProps {
  currentProjectDir: string;
}

export const VisualDiff: React.FC<VisualDiffProps> = ({ currentProjectDir }) => {
  const [diffMode, setDiffMode] = useState<"wipe" | "blend" | "sideBySide">("wipe");
  const [wipePosition, setWipePosition] = useState<number>(50); // percentage 0-100
  const [selectedLayer, setSelectedLayer] = useState<string>("F.Cu");
  const [zoom, setZoom] = useState<number>(1.1);
  const [revASvg, setRevASvg] = useState<string>("");
  const [revBSvg, setRevBSvg] = useState<string>("");
  const [loading, setLoading] = useState<boolean>(true);

  // Load Revision A and Revision B SVG assets
  useEffect(() => {
    async function loadDiffSvgs() {
      setLoading(true);
      try {
        const [resA, resB] = await Promise.all([
          fetch(`/samples/complex_hierarchy/complex_hierarchy-top_copper.svg`),
          fetch(`/samples/complex_hierarchy_revB/complex_hierarchy-top_copper.svg`),
        ]);

        if (resA.ok && resB.ok) {
          const [svgA, svgB] = await Promise.all([resA.text(), resB.text()]);
          setRevASvg(svgA);
          setRevBSvg(svgB);
        }
      } catch (e) {
        console.error("Failed to load diff SVGs:", e);
      } finally {
        setLoading(false);
      }
    }

    loadDiffSvgs();
  }, [currentProjectDir, selectedLayer]);

  return (
    <div className="flex-1 h-full flex flex-col bg-slate-950 overflow-hidden select-none">
      {/* Top Diff Header Bar */}
      <div className="h-12 bg-slate-900 border-b border-slate-800 px-4 flex items-center justify-between shrink-0">
        {/* Revision Comparison Badges */}
        <div className="flex items-center space-x-3 text-xs">
          <div className="flex items-center space-x-1.5 px-2.5 py-1 rounded bg-rose-500/10 border border-rose-500/30 text-rose-300 font-mono">
            <span className="w-2 h-2 rounded-full bg-rose-500" />
            <span className="font-semibold">Rev A:</span>
            <span>a1b2c3d (Base commit)</span>
          </div>

          <ArrowRight className="w-3.5 h-3.5 text-slate-500" />

          <div className="flex items-center space-x-1.5 px-2.5 py-1 rounded bg-emerald-500/10 border border-emerald-500/30 text-emerald-300 font-mono">
            <span className="w-2 h-2 rounded-full bg-emerald-500" />
            <span className="font-semibold">Rev B:</span>
            <span>b2c3d4e (Route modifications)</span>
          </div>
        </div>

        {/* Diff Mode Toggle */}
        <div className="flex items-center space-x-2">
          <div className="flex items-center bg-slate-950 border border-slate-800 rounded-lg p-0.5 text-xs">
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
            <button
              onClick={() => setDiffMode("blend")}
              className={`flex items-center space-x-1 px-3 py-1 rounded-md transition-colors ${
                diffMode === "blend"
                  ? "bg-indigo-600 text-white font-medium shadow-sm"
                  : "text-slate-400 hover:text-slate-200"
              }`}
            >
              <Sparkles className="w-3.5 h-3.5" />
              <span>Differential Overlay</span>
            </button>
            <button
              onClick={() => setDiffMode("sideBySide")}
              className={`flex items-center space-x-1 px-3 py-1 rounded-md transition-colors ${
                diffMode === "sideBySide"
                  ? "bg-indigo-600 text-white font-medium shadow-sm"
                  : "text-slate-400 hover:text-slate-200"
              }`}
            >
              <GitCompare className="w-3.5 h-3.5" />
              <span>Side-by-Side</span>
            </button>
          </div>

          {/* Layer Selector */}
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

      {/* Main Diff Canvas Viewport */}
      <div className="relative flex-1 overflow-hidden flex items-center justify-center bg-slate-950 p-6">
        {loading ? (
          <div className="flex flex-col items-center justify-center space-y-3 text-slate-400">
            <div className="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin" />
            <span className="text-xs font-mono">Loading vector revisions for diff analysis...</span>
          </div>
        ) : diffMode === "wipe" ? (
          /* Split Wipe View Mode */
          <div className="relative w-full h-full flex items-center justify-center">
            {/* Board Container */}
            <div
              className="relative select-none shadow-2xl rounded-xl border border-slate-800 bg-slate-900/60 p-4"
              style={{ transform: `scale(${zoom})`, transformOrigin: "center center" }}
            >
              {/* Revision A (Left side of wipe) */}
              <div
                className="relative"
                style={{
                  clipPath: `polygon(0 0, ${wipePosition}% 0, ${wipePosition}% 100%, 0 100%)`,
                }}
                dangerouslySetInnerHTML={{ __html: revASvg }}
              />

              {/* Revision B (Right side of wipe) */}
              <div
                className="absolute inset-0 p-4"
                style={{
                  clipPath: `polygon(${wipePosition}% 0, 100% 0, 100% 100%, ${wipePosition}% 100%)`,
                }}
                dangerouslySetInnerHTML={{ __html: revBSvg }}
              />

              {/* Wipe Divider Line */}
              <div
                className="absolute top-0 bottom-0 w-0.5 bg-gradient-to-b from-indigo-500 via-cyan-400 to-indigo-500 shadow-[0_0_12px_rgba(34,211,238,0.8)] z-30 pointer-events-none"
                style={{ left: `${wipePosition}%` }}
              >
                <div className="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-6 h-6 rounded-full bg-cyan-400 border-2 border-slate-950 flex items-center justify-center shadow-lg">
                  <SplitSquareVertical className="w-3.5 h-3.5 text-slate-950 font-bold" />
                </div>
              </div>
            </div>

            {/* Bottom Floating Wipe Slider Control */}
            <div className="absolute bottom-6 z-40 bg-slate-900/90 backdrop-blur-md border border-slate-800 rounded-xl px-4 py-2.5 shadow-2xl flex items-center space-x-4 min-w-[360px]">
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
              <span className="text-xs font-mono text-slate-400 w-12 text-right">
                {wipePosition}%
              </span>
            </div>
          </div>
        ) : diffMode === "blend" ? (
          /* Differential Overlay (Blend Mode) */
          <div className="relative w-full h-full flex flex-col items-center justify-center">
            {/* Color Legend HUD */}
            <div className="absolute top-4 left-6 z-20 bg-slate-900/90 backdrop-blur-md border border-slate-800 rounded-lg p-2.5 shadow-xl space-y-1.5 text-xs">
              <div className="font-semibold text-slate-200 text-[11px] uppercase tracking-wider mb-1">
                Boolean Difference
              </div>
              <div className="flex items-center space-x-2">
                <span className="w-3 h-3 rounded bg-emerald-500 shadow-sm shadow-emerald-500/50" />
                <span className="text-slate-300">Added in Rev B</span>
              </div>
              <div className="flex items-center space-x-2">
                <span className="w-3 h-3 rounded bg-rose-500 shadow-sm shadow-rose-500/50" />
                <span className="text-slate-300">Removed in Rev B</span>
              </div>
              <div className="flex items-center space-x-2">
                <span className="w-3 h-3 rounded bg-slate-600" />
                <span className="text-slate-400">Unchanged Copper</span>
              </div>
            </div>

            {/* Differential Composited Board */}
            <div
              className="relative shadow-2xl rounded-xl border border-slate-800 bg-slate-900/80 p-4"
              style={{ transform: `scale(${zoom})`, transformOrigin: "center center" }}
            >
              {/* Revision A base (Tinted Red for removed copper) */}
              <div
                className="relative filter drop-shadow-[0_0_2px_rgba(239,68,68,0.8)]"
                style={{
                  filter: "hue-rotate(180deg) saturate(3) contrast(1.2)",
                  opacity: 0.85,
                }}
                dangerouslySetInnerHTML={{ __html: revASvg }}
              />

              {/* Revision B blend (Tinted Green for added copper) */}
              <div
                className="absolute inset-0 p-4 mix-blend-screen"
                style={{
                  filter: "hue-rotate(90deg) saturate(3) contrast(1.2)",
                  opacity: 0.85,
                }}
                dangerouslySetInnerHTML={{ __html: revBSvg }}
              />
            </div>
          </div>
        ) : (
          /* Side-by-Side Dual View */
          <div className="w-full h-full grid grid-cols-2 gap-4 p-4">
            <div className="flex flex-col items-center justify-center bg-slate-900/60 rounded-xl border border-slate-800 p-4 relative">
              <div className="absolute top-3 left-3 px-2 py-1 rounded bg-rose-500/20 text-rose-300 text-xs font-mono font-semibold">
                Revision A (Commit a1b2c3d)
              </div>
              <div
                className="overflow-hidden"
                style={{ transform: `scale(${zoom * 0.8})` }}
                dangerouslySetInnerHTML={{ __html: revASvg }}
              />
            </div>

            <div className="flex flex-col items-center justify-center bg-slate-900/60 rounded-xl border border-slate-800 p-4 relative">
              <div className="absolute top-3 left-3 px-2 py-1 rounded bg-emerald-500/20 text-emerald-300 text-xs font-mono font-semibold">
                Revision B (Commit b2c3d4e)
              </div>
              <div
                className="overflow-hidden"
                style={{ transform: `scale(${zoom * 0.8})` }}
                dangerouslySetInnerHTML={{ __html: revBSvg }}
              />
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
