import React, { useState } from "react";
import { AlertTriangle, AlertCircle, CheckCircle2, RefreshCw, Filter, Crosshair } from "lucide-react";
import { DrcResult, DrcViolation } from "../lib/types";

interface DrcInspectorProps {
  drcResult: DrcResult | null;
  onRunDrc: () => void;
  isRunningDrc: boolean;
  onFocusCoordinate: (coords: { x: number; y: number }) => void;
}

export const DrcInspector: React.FC<DrcInspectorProps> = ({
  drcResult,
  onRunDrc,
  isRunningDrc,
  onFocusCoordinate,
}) => {
  const [filterSeverity, setFilterSeverity] = useState<string>("all");
  const [searchTerm, setSearchTerm] = useState<string>("");

  const violations = drcResult?.violations || [];
  const errorsCount = violations.filter((v) => v.severity === "error").length;
  const warningsCount = violations.filter((v) => v.severity === "warning").length;

  const filteredViolations = violations.filter((v) => {
    if (filterSeverity !== "all" && v.severity !== filterSeverity) return false;
    if (searchTerm) {
      const term = searchTerm.toLowerCase();
      return (
        v.description.toLowerCase().includes(term) ||
        (v.type_name && v.type_name.toLowerCase().includes(term))
      );
    }
    return true;
  });

  return (
    <div className="flex-1 h-full flex flex-col bg-slate-950 overflow-hidden select-none">
      {/* DRC Header / Stats Bar */}
      <div className="h-14 bg-slate-900 border-b border-slate-800 px-6 flex items-center justify-between shrink-0">
        <div className="flex items-center space-x-4">
          <div className="flex items-center space-x-2">
            <AlertTriangle className="w-5 h-5 text-amber-400" />
            <span className="font-semibold text-slate-100 text-sm">
              Design Rules Check (DRC) Audit
            </span>
          </div>

          <div className="flex items-center space-x-2 font-mono text-xs">
            <span className="px-2.5 py-0.5 rounded-full bg-red-500/10 text-red-400 border border-red-500/30">
              {errorsCount} Errors
            </span>
            <span className="px-2.5 py-0.5 rounded-full bg-amber-500/10 text-amber-400 border border-amber-500/30">
              {warningsCount} Warnings
            </span>
            <span className="text-slate-400">
              Total Violations: <strong className="text-white">{violations.length}</strong>
            </span>
          </div>
        </div>

        {/* Action Button: Rerun DRC */}
        <button
          onClick={onRunDrc}
          disabled={isRunningDrc}
          className="flex items-center space-x-1.5 px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-medium transition-colors shadow-sm disabled:opacity-50"
        >
          <RefreshCw className={`w-3.5 h-3.5 ${isRunningDrc ? "animate-spin" : ""}`} />
          <span>{isRunningDrc ? "Executing DRC..." : "Re-run kicad-cli DRC"}</span>
        </button>
      </div>

      {/* Filter / Search Bar */}
      <div className="p-4 bg-slate-900/60 border-b border-slate-800 flex items-center justify-between shrink-0">
        <div className="flex items-center space-x-2">
          <Filter className="w-3.5 h-3.5 text-slate-400" />
          <div className="flex items-center bg-slate-950 border border-slate-800 rounded-lg p-0.5 text-xs">
            <button
              onClick={() => setFilterSeverity("all")}
              className={`px-2.5 py-1 rounded transition-colors ${
                filterSeverity === "all" ? "bg-slate-800 text-white font-medium" : "text-slate-400"
              }`}
            >
              All ({violations.length})
            </button>
            <button
              onClick={() => setFilterSeverity("error")}
              className={`px-2.5 py-1 rounded transition-colors ${
                filterSeverity === "error" ? "bg-red-500/20 text-red-300 font-medium" : "text-slate-400"
              }`}
            >
              Errors ({errorsCount})
            </button>
            <button
              onClick={() => setFilterSeverity("warning")}
              className={`px-2.5 py-1 rounded transition-colors ${
                filterSeverity === "warning" ? "bg-amber-500/20 text-amber-300 font-medium" : "text-slate-400"
              }`}
            >
              Warnings ({warningsCount})
            </button>
          </div>
        </div>

        <input
          type="text"
          placeholder="Search violation rule or text..."
          value={searchTerm}
          onChange={(e) => setSearchTerm(e.target.value)}
          className="bg-slate-950 border border-slate-800 rounded-lg px-3 py-1 text-xs text-slate-200 placeholder-slate-500 focus:outline-none focus:border-indigo-500 w-64"
        />
      </div>

      {/* Violations List Table / Cards */}
      <div className="flex-1 overflow-y-auto p-4 space-y-2">
        {filteredViolations.length === 0 ? (
          <div className="h-48 flex flex-col items-center justify-center text-slate-500">
            <CheckCircle2 className="w-8 h-8 text-emerald-500 mb-2" />
            <p className="text-sm font-medium text-slate-300">No DRC violations match the filter</p>
            <p className="text-xs text-slate-500">Board layout conforms to the specified design rules.</p>
          </div>
        ) : (
          filteredViolations.map((v, idx) => (
            <div
              key={idx}
              className="p-3 rounded-lg border border-slate-800 bg-slate-900/60 hover:bg-slate-800/80 transition-all flex items-start justify-between group"
            >
              <div className="flex items-start space-x-3">
                <div className="mt-0.5">
                  {v.severity === "error" ? (
                    <AlertCircle className="w-4 h-4 text-red-400 shrink-0" />
                  ) : (
                    <AlertTriangle className="w-4 h-4 text-amber-400 shrink-0" />
                  )}
                </div>

                <div>
                  <div className="flex items-center space-x-2">
                    <span className="text-xs font-semibold text-slate-200">{v.description}</span>
                    {v.type_name && (
                      <span className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-slate-800 text-slate-400 border border-slate-700">
                        {v.type_name}
                      </span>
                    )}
                  </div>

                  <div className="flex items-center space-x-3 mt-1.5 text-[11px] font-mono text-slate-400">
                    {v.pos_x !== undefined && v.pos_y !== undefined ? (
                      <span className="flex items-center space-x-1 text-cyan-400">
                        <Crosshair className="w-3 h-3" />
                        <span>({v.pos_x} mm, {v.pos_y} mm)</span>
                      </span>
                    ) : (
                      <span>Global Rule</span>
                    )}
                    <span>Severity: <span className={v.severity === "error" ? "text-red-400" : "text-amber-400"}>{v.severity}</span></span>
                  </div>
                </div>
              </div>

              {v.pos_x !== undefined && v.pos_y !== undefined && (
                <button
                  onClick={() => onFocusCoordinate({ x: v.pos_x!, y: v.pos_y! })}
                  className="px-2.5 py-1 rounded bg-slate-800 hover:bg-indigo-600 text-slate-300 hover:text-white text-xs font-medium transition-colors flex items-center space-x-1 shadow-sm"
                >
                  <Crosshair className="w-3 h-3" />
                  <span>Inspect on PCB</span>
                </button>
              )}
            </div>
          ))
        )}
      </div>
    </div>
  );
};
