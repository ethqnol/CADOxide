import React, { useState, useEffect } from "react";
import { FileCode2, ZoomIn, ZoomOut, RotateCcw, ChevronRight, FolderTree } from "lucide-react";

interface SchematicViewerProps {
  sheets: { id: string; name: string; file: string }[];
}

export const SchematicViewer: React.FC<SchematicViewerProps> = ({ sheets }) => {
  const [selectedSheet, setSelectedSheet] = useState<string>(sheets[0]?.id || "root");
  const [svgContent, setSvgContent] = useState<string>("");
  const [loading, setLoading] = useState<boolean>(true);
  const [zoom, setZoom] = useState<number>(0.9);

  const currentSheet = sheets.find((s) => s.id === selectedSheet) || sheets[0];

  useEffect(() => {
    if (!currentSheet) return;
    setLoading(true);
    fetch(currentSheet.file)
      .then((res) => res.text())
      .then((svg) => {
        setSvgContent(svg);
        setLoading(false);
      })
      .catch((err) => {
        console.error("Failed to load schematic sheet:", err);
        setLoading(false);
      });
  }, [currentSheet]);

  return (
    <div className="flex-1 h-full flex bg-slate-950 overflow-hidden select-none">
      {/* Sidebar: Sheet Hierarchy Tree */}
      <aside className="w-72 bg-slate-900 border-r border-slate-800 flex flex-col shrink-0">
        <div className="p-3 border-b border-slate-800 flex items-center space-x-2">
          <FolderTree className="w-4 h-4 text-indigo-400" />
          <span className="text-xs font-semibold uppercase tracking-wider text-slate-200">
            Schematic Hierarchy
          </span>
        </div>

        <div className="p-2 space-y-1">
          {sheets.map((sheet) => {
            const isSelected = selectedSheet === sheet.id;
            return (
              <button
                key={sheet.id}
                onClick={() => setSelectedSheet(sheet.id)}
                className={`w-full flex items-center justify-between p-2 rounded-lg text-left text-xs transition-colors ${
                  isSelected
                    ? "bg-indigo-600 text-white font-medium shadow-sm"
                    : "text-slate-300 hover:bg-slate-800"
                }`}
              >
                <div className="flex items-center space-x-2 truncate">
                  <FileCode2 className="w-3.5 h-3.5 shrink-0" />
                  <span className="truncate">{sheet.name}</span>
                </div>
                <ChevronRight className={`w-3.5 h-3.5 ${isSelected ? "text-white" : "text-slate-500"}`} />
              </button>
            );
          })}
        </div>

        <div className="mt-auto p-3 border-t border-slate-800 bg-slate-950/40 text-[11px] text-slate-400">
          <div className="font-semibold text-slate-300 mb-1">KiCad Vector Export:</div>
          <div>Multi-sheet hierarchy extracted via <code className="text-indigo-400">kicad-cli sch export svg</code>.</div>
        </div>
      </aside>

      {/* Main Schematic SVG Viewport */}
      <div className="flex-1 relative flex flex-col overflow-hidden bg-slate-950">
        {/* Floating Zoom Toolbar */}
        <div className="absolute top-4 left-4 z-20 flex items-center space-x-2 bg-slate-900/90 backdrop-blur-md border border-slate-800 rounded-lg p-1.5 shadow-xl">
          <button
            onClick={() => setZoom((z) => Math.min(5, z * 1.2))}
            className="p-1.5 rounded hover:bg-slate-800 text-slate-300 hover:text-white"
            title="Zoom In"
          >
            <ZoomIn className="w-4 h-4" />
          </button>
          <button
            onClick={() => setZoom((z) => Math.max(0.2, z * 0.8))}
            className="p-1.5 rounded hover:bg-slate-800 text-slate-300 hover:text-white"
            title="Zoom Out"
          >
            <ZoomOut className="w-4 h-4" />
          </button>
          <button
            onClick={() => setZoom(0.9)}
            className="p-1.5 rounded hover:bg-slate-800 text-slate-300 hover:text-white"
            title="Reset Zoom"
          >
            <RotateCcw className="w-4 h-4" />
          </button>
          <span className="text-[11px] font-mono text-slate-400 px-1 min-w-[48px] text-center">
            {Math.round(zoom * 100)}%
          </span>
        </div>

        {/* Viewport Content */}
        <div className="flex-1 overflow-auto flex items-center justify-center p-8">
          {loading ? (
            <div className="flex flex-col items-center justify-center space-y-2 text-slate-400">
              <div className="w-8 h-8 border-2 border-indigo-500 border-t-transparent rounded-full animate-spin" />
              <span className="text-xs font-mono">Loading schematic sheet...</span>
            </div>
          ) : (
            <div
              className="bg-white rounded-lg shadow-2xl p-4 transition-transform duration-75 select-text"
              style={{ transform: `scale(${zoom})`, transformOrigin: "center center" }}
              dangerouslySetInnerHTML={{ __html: svgContent }}
            />
          )}
        </div>
      </div>
    </div>
  );
};
