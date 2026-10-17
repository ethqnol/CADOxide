import React, { useState } from "react";
import {
  MessageSquare,
  CheckCircle2,
  Clock,
  Send,
  MapPin,
  X,
  User,
  CornerDownRight,
  Plus,
} from "lucide-react";
import { ReviewThread } from "../lib/types";

interface ReviewPanelProps {
  threads: ReviewThread[];
  selectedThreadId: string | null;
  onSelectThread: (thread: ReviewThread) => void;
  onAddMessage: (threadId: string, author: string, body: string) => void;
  onToggleStatus: (threadId: string, newStatus: "open" | "resolved") => void;
  onClose: () => void;
  newPinDraft: { x: number; y: number; layer: string } | null;
  onCreateThread: (author: string, message: string) => void;
  onCancelNewPin: () => void;
}

export const ReviewPanel: React.FC<ReviewPanelProps> = ({
  threads,
  selectedThreadId,
  onSelectThread,
  onAddMessage,
  onToggleStatus,
  onClose,
  newPinDraft,
  onCreateThread,
  onCancelNewPin,
}) => {
  const [filter, setFilter] = useState<"all" | "open" | "resolved">("all");
  const [replyText, setReplyText] = useState<string>("");
  const [authorName, setAuthorName] = useState<string>("HardwareReviewer");

  // For new pin creation modal
  const [newThreadAuthor, setNewThreadAuthor] = useState<string>("SeniorEngineer");
  const [newThreadMessage, setNewThreadMessage] = useState<string>("");

  const filteredThreads = threads.filter((t) => {
    if (filter === "open") return t.status === "open";
    if (filter === "resolved") return t.status === "resolved";
    return true;
  });

  const selectedThread = threads.find((t) => t.id === selectedThreadId);

  const handleSendReply = (e: React.FormEvent) => {
    e.preventDefault();
    if (!selectedThreadId || !replyText.trim()) return;
    onAddMessage(selectedThreadId, authorName, replyText.trim());
    setReplyText("");
  };

  const handleCreateNewThread = (e: React.FormEvent) => {
    e.preventDefault();
    if (!newThreadMessage.trim()) return;
    onCreateThread(newThreadAuthor, newThreadMessage.trim());
    setNewThreadMessage("");
  };

  return (
    <aside className="w-80 bg-slate-900 border-l border-slate-800 flex flex-col h-full z-20 shrink-0 select-none">
      {/* Panel Header */}
      <div className="p-3 border-b border-slate-800 flex items-center justify-between">
        <div className="flex items-center space-x-2">
          <MessageSquare className="w-4 h-4 text-indigo-400" />
          <span className="text-xs font-semibold uppercase tracking-wider text-slate-200">
            Review Pins & Threads
          </span>
          <span className="text-[10px] font-mono px-1.5 py-0.5 rounded-full bg-slate-800 text-slate-400">
            {threads.length}
          </span>
        </div>
        <button
          onClick={onClose}
          className="p-1 rounded hover:bg-slate-800 text-slate-400 hover:text-slate-200"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      {/* Draft New Pin Dialog (when user clicked on board) */}
      {newPinDraft && (
        <div className="p-3 bg-rose-500/10 border-b border-rose-500/30">
          <div className="flex items-center justify-between mb-2">
            <div className="flex items-center space-x-1.5 text-xs font-semibold text-rose-400">
              <MapPin className="w-4 h-4" />
              <span>New Visual Pin at ({newPinDraft.x} mm, {newPinDraft.y} mm)</span>
            </div>
            <button
              onClick={onCancelNewPin}
              className="text-slate-400 hover:text-slate-200 text-xs"
            >
              Cancel
            </button>
          </div>

          <form onSubmit={handleCreateNewThread} className="space-y-2">
            <input
              type="text"
              placeholder="Your Name / Handle"
              value={newThreadAuthor}
              onChange={(e) => setNewThreadAuthor(e.target.value)}
              className="w-full bg-slate-950 border border-slate-700 rounded px-2.5 py-1 text-xs text-slate-200 focus:outline-none focus:border-rose-500"
            />
            <textarea
              rows={2}
              placeholder="Describe issue (e.g. Clearance violation, impedance mismatch)..."
              value={newThreadMessage}
              onChange={(e) => setNewThreadMessage(e.target.value)}
              className="w-full bg-slate-950 border border-slate-700 rounded px-2.5 py-1 text-xs text-slate-200 focus:outline-none focus:border-rose-500 resize-none"
            />
            <button
              type="submit"
              className="w-full py-1.5 rounded bg-rose-600 hover:bg-rose-500 text-white font-medium text-xs transition-colors shadow-sm"
            >
              Drop Pin & Open Thread
            </button>
          </form>
        </div>
      )}

      {/* Filter Tabs */}
      <div className="p-2 border-b border-slate-800 flex items-center bg-slate-950/60">
        <button
          onClick={() => setFilter("all")}
          className={`flex-1 py-1 rounded text-[11px] font-medium transition-colors ${
            filter === "all" ? "bg-slate-800 text-slate-100" : "text-slate-400 hover:text-slate-200"
          }`}
        >
          All ({threads.length})
        </button>
        <button
          onClick={() => setFilter("open")}
          className={`flex-1 py-1 rounded text-[11px] font-medium transition-colors ${
            filter === "open" ? "bg-rose-500/20 text-rose-300" : "text-slate-400 hover:text-slate-200"
          }`}
        >
          Open ({threads.filter((t) => t.status === "open").length})
        </button>
        <button
          onClick={() => setFilter("resolved")}
          className={`flex-1 py-1 rounded text-[11px] font-medium transition-colors ${
            filter === "resolved" ? "bg-emerald-500/20 text-emerald-300" : "text-slate-400 hover:text-slate-200"
          }`}
        >
          Resolved ({threads.filter((t) => t.status === "resolved").length})
        </button>
      </div>

      {/* Threads List */}
      <div className="flex-1 overflow-y-auto p-2 space-y-2">
        {filteredThreads.length === 0 ? (
          <div className="h-40 flex flex-col items-center justify-center text-slate-500 text-xs text-center p-4">
            <MessageSquare className="w-6 h-6 mb-2 text-slate-600" />
            <span>No review threads in this filter.</span>
            <span className="text-[10px] mt-1 text-slate-600">
              Click &quot;Drop Pin&quot; on the canvas to place a marker.
            </span>
          </div>
        ) : (
          filteredThreads.map((thread) => {
            const isSelected = selectedThreadId === thread.id;
            return (
              <div
                key={thread.id}
                onClick={() => onSelectThread(thread)}
                className={`p-2.5 rounded-lg border transition-all cursor-pointer ${
                  isSelected
                    ? "bg-slate-800 border-indigo-500 shadow-md"
                    : "bg-slate-950/60 border-slate-800 hover:border-slate-700"
                }`}
              >
                {/* Thread Top Info */}
                <div className="flex items-center justify-between mb-1.5">
                  <div className="flex items-center space-x-1.5">
                    <span
                      className={`w-2 h-2 rounded-full ${
                        thread.status === "resolved" ? "bg-emerald-500" : "bg-rose-500"
                      }`}
                    />
                    <span className="text-xs font-semibold text-slate-200">{thread.created_by}</span>
                  </div>

                  <span className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-slate-800 text-cyan-400 border border-slate-700">
                    {thread.coord_x}mm, {thread.coord_y}mm
                  </span>
                </div>

                {/* Layer Badge & Time */}
                <div className="flex items-center space-x-2 text-[10px] text-slate-500 mb-2">
                  <span className="font-mono text-amber-400">{thread.sheet_or_layer}</span>
                  <span>•</span>
                  <span>{new Date(thread.created_at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>
                </div>

                {/* Messages / Discussion */}
                <div className="space-y-1.5">
                  {thread.messages.map((msg) => (
                    <div key={msg.id} className="text-xs bg-slate-900/80 rounded p-2 border border-slate-800/80">
                      <div className="flex items-center justify-between text-[10px] text-slate-400 mb-1">
                        <span className="font-medium text-slate-300">{msg.user_name}</span>
                        <span>{new Date(msg.created_at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>
                      </div>
                      <p className="text-slate-200 select-text leading-relaxed">{msg.body}</p>
                    </div>
                  ))}
                </div>

                {/* Status Toggle Button */}
                <div className="mt-2.5 pt-2 border-t border-slate-800 flex items-center justify-between">
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      onToggleStatus(thread.id, thread.status === "open" ? "resolved" : "open");
                    }}
                    className={`flex items-center space-x-1 text-[11px] px-2 py-0.5 rounded transition-colors ${
                      thread.status === "resolved"
                        ? "text-emerald-400 hover:bg-emerald-500/10"
                        : "text-slate-400 hover:text-slate-200 hover:bg-slate-700"
                    }`}
                  >
                    <CheckCircle2 className="w-3 h-3" />
                    <span>{thread.status === "resolved" ? "Resolved (Reopen)" : "Mark Resolved"}</span>
                  </button>
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* Reply Input Box (for selected thread) */}
      {selectedThread && (
        <form onSubmit={handleSendReply} className="p-3 border-t border-slate-800 bg-slate-950/80">
          <div className="text-[10px] text-slate-400 mb-1 flex items-center justify-between">
            <span>Replying to {selectedThread.created_by}</span>
            <span className="font-mono text-cyan-400">({selectedThread.coord_x}mm, {selectedThread.coord_y}mm)</span>
          </div>
          <div className="flex items-center space-x-2">
            <input
              type="text"
              placeholder="Write a review comment..."
              value={replyText}
              onChange={(e) => setReplyText(e.target.value)}
              className="flex-1 bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-xs text-slate-200 focus:outline-none focus:border-indigo-500"
            />
            <button
              type="submit"
              disabled={!replyText.trim()}
              className="p-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-500 text-white disabled:opacity-50 transition-colors shadow-sm"
            >
              <Send className="w-3.5 h-3.5" />
            </button>
          </div>
        </form>
      )}
    </aside>
  );
};
