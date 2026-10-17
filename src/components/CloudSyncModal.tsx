import React, { useState, useEffect } from "react";
import { X, Server, RefreshCw, Copy, Check, Activity, Cpu } from "lucide-react";

interface CloudSyncModalProps {
  isOpen: boolean;
  onClose: () => void;
  dualMode: "local" | "remote";
}

export const CloudSyncModal: React.FC<CloudSyncModalProps> = ({ isOpen, onClose }) => {
  const [serverStatus, setServerStatus] = useState<"checking" | "online" | "offline">("checking");
  const [metrics, setMetrics] = useState<any>(null);
  const [copied, setCopied] = useState<boolean>(false);

  useEffect(() => {
    if (!isOpen) return;
    checkStatus();
  }, [isOpen]);

  const checkStatus = async () => {
    setServerStatus("checking");
    try {
      const [verRes, metRes] = await Promise.all([
        fetch("http://localhost:8080/api/version"),
        fetch("http://localhost:8080/api/cluster/metrics"),
      ]);
      if (verRes.ok) {
        setServerStatus("online");
        if (metRes.ok) {
          setMetrics(await metRes.json());
        }
      } else {
        setServerStatus("offline");
      }
    } catch {
      setServerStatus("offline");
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm p-4">
      <div className="w-full max-w-xl bg-slate-900 border border-slate-800 rounded-xl shadow-2xl overflow-hidden flex flex-col">
        {/* Modal Header */}
        <div className="p-4 border-b border-slate-800 flex items-center justify-between bg-slate-950/60">
          <div className="flex items-center space-x-2">
            <Server className="w-5 h-5 text-cyan-400" />
            <h2 className="text-sm font-semibold text-slate-100">
              Load-Balanced Broker & Compute Cluster Telemetry
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
                  Axum Gateway ({metrics?.node_id || "cadence-node-primary"}):{" "}
                  {serverStatus === "online"
                    ? "Online & Load-Balanced"
                    : serverStatus === "checking"
                    ? "Checking Endpoint..."
                    : "Standby (Local Fallback)"}
                </div>
                <div className="text-[11px] font-mono text-slate-400">
                  http://localhost:8080/api/cluster/metrics
                </div>
              </div>
            </div>

            <button
              onClick={checkStatus}
              className="p-1.5 rounded hover:bg-slate-800 text-slate-300 transition-colors"
              title="Refresh telemetry"
            >
              <RefreshCw className={`w-3.5 h-3.5 ${serverStatus === "checking" ? "animate-spin" : ""}`} />
            </button>
          </div>

          {/* Live Telemetry Metrics */}
          {metrics && (
            <div className="grid grid-cols-3 gap-2.5">
              <div className="p-3 rounded-lg bg-slate-950/90 border border-slate-800">
                <div className="text-[10px] text-slate-400 uppercase font-mono">CAS Cache Hit Ratio</div>
                <div className="text-lg font-bold text-emerald-400 font-mono mt-0.5">
                  {metrics.cas_cache.hit_ratio_percent.toFixed(1)}%
                </div>
                <div className="text-[10px] text-slate-500 font-mono">
                  {metrics.cas_cache.hits} hits / {metrics.cas_cache.misses} misses
                </div>
              </div>

              <div className="p-3 rounded-lg bg-slate-950/90 border border-slate-800">
                <div className="text-[10px] text-slate-400 uppercase font-mono">Worker Semaphore</div>
                <div className="text-lg font-bold text-cyan-400 font-mono mt-0.5">
                  {metrics.worker_pool.idle_workers}/{metrics.worker_pool.max_concurrency} Idle
                </div>
                <div className="text-[10px] text-slate-500 font-mono">
                  Bounded KiCad Pool
                </div>
              </div>

              <div className="p-3 rounded-lg bg-slate-950/90 border border-slate-800">
                <div className="text-[10px] text-slate-400 uppercase font-mono">Broker Dispatched</div>
                <div className="text-lg font-bold text-indigo-400 font-mono mt-0.5">
                  {metrics.broker.total_jobs_completed} Jobs
                </div>
                <div className="text-[10px] text-slate-500 font-mono">
                  Async mpsc/Redis Queue
                </div>
              </div>
            </div>
          )}

          {/* Docker Services Architecture Grid */}
          <div className="space-y-2">
            <div className="text-[11px] font-semibold text-slate-400 uppercase tracking-wider">
              Horizontal Scaling & Broker Topology
            </div>

            <div className="grid grid-cols-2 gap-2 font-mono text-[11px]">
              <div className="p-2.5 rounded bg-slate-950/70 border border-slate-800/80 flex items-center justify-between">
                <span className="text-slate-300">NGINX / Envoy LB</span>
                <span className="text-emerald-400 font-semibold">Least-Conn</span>
              </div>
              <div className="p-2.5 rounded bg-slate-950/70 border border-slate-800/80 flex items-center justify-between">
                <span className="text-slate-300">kicad-worker Replicas</span>
                <span className="text-emerald-400 font-semibold">Autoscale (1..N)</span>
              </div>
              <div className="p-2.5 rounded bg-slate-950/70 border border-slate-800/80 flex items-center justify-between">
                <span className="text-slate-300">Redis Streams Broker</span>
                <span className="text-emerald-400 font-semibold">Priority Queues</span>
              </div>
              <div className="p-2.5 rounded bg-slate-950/70 border border-slate-800/80 flex items-center justify-between">
                <span className="text-slate-300">MinIO S3 CAS Store</span>
                <span className="text-emerald-400 font-semibold">SHA-256 Dedupe</span>
              </div>
            </div>
          </div>

          {/* Launch Command Snippet */}
          <div className="space-y-1">
            <div className="text-[11px] text-slate-400">Scale Worker Pool via Docker Compose:</div>
            <div className="flex items-center justify-between p-2 rounded bg-slate-950 border border-slate-800 font-mono text-xs">
              <span className="text-cyan-400">
                docker compose up -d --scale api=2 --scale worker=4
              </span>
              <button
                onClick={() => {
                  navigator.clipboard.writeText(
                    "docker compose up -d --scale api=2 --scale worker=4"
                  );
                  setCopied(true);
                  setTimeout(() => setCopied(false), 2000);
                }}
                className="p-1 rounded hover:bg-slate-800 text-slate-400 hover:text-slate-200"
              >
                {copied ? (
                  <Check className="w-3.5 h-3.5 text-emerald-400" />
                ) : (
                  <Copy className="w-3.5 h-3.5" />
                )}
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
