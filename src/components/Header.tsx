import React from "react";
import {
  Layers,
  GitCompare,
  FileCode2,
  AlertTriangle,
  Server,
  Laptop,
  CheckCircle2,
  HardDrive,
  Cpu,
} from "lucide-react";
import { ViewMode, DualMode, ProjectSample } from "../lib/types";

interface HeaderProps {
  currentView: ViewMode;
  onViewChange: (view: ViewMode) => void;
  dualMode: DualMode;
  onDualModeChange: (mode: DualMode) => void;
  kicadInfo: { available: boolean; version: string; backend: string };
  projects: ProjectSample[];
  selectedProject: ProjectSample;
  onSelectProject: (p: ProjectSample) => void;
  onOpenCloudSync: () => void;
  drcViolationsCount: number;
  openThreadsCount: number;
}

export const Header: React.FC<HeaderProps> = ({
  currentView,
  onViewChange,
  dualMode,
  onDualModeChange,
  kicadInfo,
  projects,
  selectedProject,
  onSelectProject,
  onOpenCloudSync,
  drcViolationsCount,
  openThreadsCount,
}) => {
  return (
    <header className="h-14 bg-slate-900 border-b border-slate-800 px-4 flex items-center justify-between select-none shrink-0 z-30">
      {/* Brand & Project Selector */}
      <div className="flex items-center space-x-3">
        <div className="flex items-center space-x-2">
          <div className="w-8 h-8 rounded-lg bg-gradient-to-tr from-cyan-600 to-indigo-600 flex items-center justify-center shadow-lg shadow-indigo-500/20">
            <Cpu className="w-5 h-5 text-white" />
          </div>
          <div>
            <div className="flex items-center space-x-2">
              <span className="font-bold text-slate-100 tracking-tight text-base">CadenceFlow</span>
              <span className="text-[10px] uppercase font-mono px-1.5 py-0.5 rounded bg-indigo-500/20 text-indigo-300 border border-indigo-500/30">
                v1.0-dual
              </span>
            </div>
          </div>
        </div>

        <div className="h-4 w-px bg-slate-800 mx-1" />

        {/* Project Selector */}
        <select
          value={selectedProject.id}
          onChange={(e) => {
            const p = projects.find((proj) => proj.id === e.target.value);
            if (p) onSelectProject(p);
          }}
          className="bg-slate-950/80 border border-slate-700 hover:border-slate-600 text-xs text-slate-200 rounded-md px-2.5 py-1.5 focus:outline-none focus:ring-1 focus:ring-indigo-500"
        >
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name} ({p.pcbFile})
            </option>
          ))}
        </select>
      </div>

      {/* Navigation View Tabs */}
      <div className="flex items-center bg-slate-950/60 p-1 rounded-lg border border-slate-800">
        <button
          onClick={() => onViewChange("pcb")}
          className={`flex items-center space-x-1.5 px-3 py-1 rounded text-xs font-medium transition-all ${
            currentView === "pcb"
              ? "bg-indigo-600 text-white shadow-sm"
              : "text-slate-400 hover:text-slate-200 hover:bg-slate-800/60"
          }`}
        >
          <Layers className="w-3.5 h-3.5" />
          <span>PCB Canvas</span>
        </button>

        <button
          onClick={() => onViewChange("diff")}
          className={`flex items-center space-x-1.5 px-3 py-1 rounded text-xs font-medium transition-all ${
            currentView === "diff"
              ? "bg-indigo-600 text-white shadow-sm"
              : "text-slate-400 hover:text-slate-200 hover:bg-slate-800/60"
          }`}
        >
          <GitCompare className="w-3.5 h-3.5" />
          <span>Visual Diff (Rev A / Rev B)</span>
        </button>

        <button
          onClick={() => onViewChange("schematic")}
          className={`flex items-center space-x-1.5 px-3 py-1 rounded text-xs font-medium transition-all ${
            currentView === "schematic"
              ? "bg-indigo-600 text-white shadow-sm"
              : "text-slate-400 hover:text-slate-200 hover:bg-slate-800/60"
          }`}
        >
          <FileCode2 className="w-3.5 h-3.5" />
          <span>Schematic Sheet</span>
        </button>

        <button
          onClick={() => onViewChange("drc")}
          className={`flex items-center space-x-1.5 px-3 py-1 rounded text-xs font-medium transition-all ${
            currentView === "drc"
              ? "bg-indigo-600 text-white shadow-sm"
              : "text-slate-400 hover:text-slate-200 hover:bg-slate-800/60"
          }`}
        >
          <AlertTriangle className="w-3.5 h-3.5" />
          <span>DRC Audit</span>
          {drcViolationsCount > 0 && (
            <span className="ml-1 px-1.5 py-0.2 rounded-full text-[10px] bg-red-500/20 text-red-400 border border-red-500/30">
              {drcViolationsCount}
            </span>
          )}
        </button>
      </div>

      {/* Mode Controls & Status Badges */}
      <div className="flex items-center space-x-3">
        {/* Dual Mode Switcher */}
        <div className="flex items-center bg-slate-950 border border-slate-800 rounded-md p-0.5 text-xs">
          <button
            onClick={() => onDualModeChange("local")}
            className={`flex items-center space-x-1 px-2 py-1 rounded transition-colors ${
              dualMode === "local"
                ? "bg-slate-800 text-emerald-400 font-medium"
                : "text-slate-400 hover:text-slate-300"
            }`}
            title="Local Mode: Direct IPC with Host kicad-cli & Embedded SQLite"
          >
            <Laptop className="w-3 h-3" />
            <span>Local Offline</span>
          </button>
          <button
            onClick={() => onDualModeChange("remote")}
            className={`flex items-center space-x-1 px-2 py-1 rounded transition-colors ${
              dualMode === "remote"
                ? "bg-slate-800 text-cyan-400 font-medium"
                : "text-slate-400 hover:text-slate-300"
            }`}
            title="Remote Mode: Axum Cloud Server + Postgres + MinIO S3"
          >
            <Server className="w-3 h-3" />
            <span>Remote Cloud</span>
          </button>
        </div>

        {/* KiCad Version Badge */}
        <div className="flex items-center space-x-1.5 px-2.5 py-1 rounded-md bg-slate-950/80 border border-slate-800 text-[11px] font-mono text-slate-300">
          <span className="w-2 h-2 rounded-full bg-emerald-500 animate-pulse" />
          <span className="text-slate-400">KiCad:</span>
          <span className="text-emerald-400 font-semibold">{kicadInfo.version}</span>
        </div>

        {/* Cloud Infra Status Modal Trigger */}
        <button
          onClick={onOpenCloudSync}
          className="p-1.5 rounded-md hover:bg-slate-800 text-slate-400 hover:text-slate-200 border border-transparent hover:border-slate-700 transition-colors"
          title="Infrastructure Architecture & Sync Status"
        >
          <HardDrive className="w-4 h-4" />
        </button>
      </div>
    </header>
  );
};
