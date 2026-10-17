import React, { useState, useEffect } from "react";
import { X, Server, Database, HardDrive, RefreshCw, CheckCircle2, AlertCircle, Copy, Check } from "lucide-react";

interface CloudSyncModalProps {
  isOpen: boolean;
  onClose: () => void;
  dualMode: "local" | "remote";
}

export const CloudSyncModal: React.FC<CloudSyncModalProps> = ({ isOpen, onClose, dualMode }) => {
  const [serverStatus, setServerStatus] = useState<"checking" | "online" | "offline">("checking");
  const [copied, setCopied] = useState<boolean>(false);

  useEffect(() => {
    if (!isOpen) return;
    checkStatus();
  }, [isOpen]);

  const checkStatus = async () => {
    setServerStatus("checking");
    try {
      const res = await fetch("http://localhost:8080/api/version");
      if (res.ok) {
        setServerStatus("online");
      } else {
        setServerStatus("offline");
      }
    } catch {
      setServerStatus("offline");
    }
  };

  if (!isOpen) return null;

  const dockerComposeSnippet = `docker compose up -d`;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm p-4">
      <div className="w-full max-w-xl bg-slate-900 border border-slate-800 rounded-xl shadow-2xl overflow-hidden flex flex-col">
        {/* Modal Header */}
        <div className="p-4 border-b border-slate-800 flex items-center justify-between bg-slate-950/60">
          <div className="flex items-center space-x-2">
            <Server className="w-5 h-5 text-cyan-400" />
            <h2 className="text-sm font-semibold text-slate-100">
              CadenceFlow Infrastructure & Dual-Engine Sync
            </h2>
          </div>
          <button
            onClick={onClose}
            className="p-1 rounded hover:bg-slate-800 text-slate-400 hover:text-slate-200"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Content */}
        <div className="p-5 space-y-4 text-xs">
          {/* Status Overview Card */}
          <div className="p-3 rounded-lg bg-slate-950 border border-slate-800 flex items-center justify-between">
            <div className="flex items-center space-x-3">
              <div
                className={`w-3 h-3 rounded-full ${
                  serverStatus === "online"
                    ? "bg-emerald-500 shadow-[0_0_8px_rgba(16,185,129,0.8)]"
                    : serverStatus === "checking"
                    ? "bg-amber-500 animate-pulse"
                    : "bg-rose-500"
                }`}
              />
              <div>
                <div className="font-semibold text-slate-200">
                  Axum API Server: {serverStatus === "online" ? "Online & Healthy" : serverStatus === "checking" ? "Checking Endpoint..." : "Standby (Local Fallback Active)"}
                </div>
                <div className="text-[11px] font-mono text-slate-400">http://localhost:8080/api</div>
              </div>
            </div>

            <button
              onClick={checkStatus}
              className="p-1.5 rounded hover:bg-slate-800 text-slate-300 transition-colors"
              title="Re-check health"
            >
              <RefreshCw className={`w-3.5 h-3.5 ${serverStatus === "checking" ? "animate-spin" : ""}`} />
            </button>
          </div>

          {/* Docker Services Architecture Grid */}
          <div className="space-y-2">
            <div className="text-[11px] font-semibold text-slate-400 uppercase tracking-wider">
              Self-Hosted Microservices Pool
            </div>

            <div className="grid grid-cols-2 gap-2 font-mono text-[11px]">
              <div className="p-2.5 rounded bg-slate-950/70 border border-slate-800/80 flex items-center justify-between">
                <span className="text-slate-300">kicad-worker (Pool)</span>
                <span className="text-emerald-400 font-semibold">Active (kicad 10)</span>
              </div>
              <div className="p-2.5 rounded bg-slate-950/70 border border-slate-800/80 flex items-center justify-between">
                <span className="text-slate-300">PostgreSQL 16</span>
                <span className="text-emerald-400 font-semibold">Port 5432</span>
              </div>
              <div className="p-2.5 rounded bg-slate-950/70 border border-slate-800/80 flex items-center justify-between">
                <span className="text-slate-300">MinIO S3 Blobs</span>
                <span className="text-emerald-400 font-semibold">Port 9000/9001</span>
              </div>
              <div className="p-2.5 rounded bg-slate-950/70 border border-slate-800/80 flex items-center justify-between">
                <span className="text-slate-300">Redis Asynq Queue</span>
                <span className="text-emerald-400 font-semibold">Port 6379</span>
              </div>
            </div>
          </div>

          {/* Dual Mode Explanation */}
          <div className="p-3 rounded-lg bg-indigo-950/30 border border-indigo-800/40 text-indigo-200 leading-relaxed text-[11px]">
            <strong className="text-white">Dual-Mode Architecture:</strong> In <em>Local Mode</em>, CadenceFlow executes local <code className="bg-slate-900 px-1 py-0.5 rounded text-cyan-300">kicad-cli</code> subprocesses directly and saves comments to embedded SQLite. In <em>Remote Cloud Mode</em>, it synchronizes design revisions, vector SVGs, and live comment threads with the Axum microservices backend.
          </div>

          {/* Launch Command Snippet */}
          <div className="space-y-1">
            <div className="text-[11px] text-slate-400">Launch Headless KiCad & Axum Server:</div>
            <div className="flex items-center justify-between p-2 rounded bg-slate-950 border border-slate-800 font-mono text-xs">
              <span className="text-cyan-400">cargo run -p cadenceflow-server --manifest-path server/Cargo.toml</span>
              <button
                onClick={() => {
                  navigator.clipboard.writeText("cargo run -p cadenceflow-server --manifest-path server/Cargo.toml");
                  setCopied(true);
                  setTimeout(() => setCopied(false), 2000);
                }}
                className="p-1 rounded hover:bg-slate-800 text-slate-400 hover:text-slate-200"
              >
                {copied ? <Check className="w-3.5 h-3.5 text-emerald-400" /> : <Copy className="w-3.5 h-3.5" />}
              </button>
            </div>
          </div>
        </div>

        {/* Footer */}
        <div className="p-3 border-t border-slate-800 bg-slate-950/60 flex justify-end">
          <button
            onClick={onClose}
            className="px-4 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-medium transition-colors"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
};
