// Collapsible Git operations log panel with export/clear.

import type { OpLogEntry } from "./types";

export function OpsPanel({
  ops,
  open,
  onToggle,
  onExport,
  onClear,
}: {
  ops: OpLogEntry[];
  open: boolean;
  onToggle: () => void;
  onExport: () => void;
  onClear: () => void;
}) {
  return (
    <div className="ops-panel">
      <button
        className="ops-toggle"
        onClick={onToggle}
        title="Git operation log"
      >
        <span className={open ? "ops-caret open" : "ops-caret"}>▸</span>
        Operations
        {ops.length > 0 && (
          <span className="ops-badge">
            {ops.filter((o) => o.status === "error").length > 0 && (
              <span className="ops-err-count">
                {ops.filter((o) => o.status === "error").length} err
              </span>
            )}
            <span className="ops-count">{ops.length}</span>
          </span>
        )}
      </button>
      {open && (
        <div className="ops-body">
          <div className="ops-toolbar">
            <span className="ops-title">Git operations ({ops.length})</span>
            <div className="ops-toolbar-actions">
              <button
                className="mini-btn bulk-btn"
                onClick={onExport}
                disabled={ops.length === 0}
                title="Export operations as .log"
              >
                Export
              </button>
              <button
                className="mini-btn bulk-btn"
                onClick={onClear}
                disabled={ops.length === 0}
              >
                Clear
              </button>
            </div>
          </div>
          <div className="ops-list">
            {ops.length === 0 && (
              <div className="ops-empty muted">No operations yet.</div>
            )}
            {ops.map((o) => (
              <div key={o.id} className={`ops-row ops-${o.status}`}>
                <span className="ops-time">{o.time}</span>
                <span className="ops-op">{o.op}</span>
                <span
                  className={`ops-status`}
                  title={o.status}
                >
                  {o.status === "ok" ? "✓" : o.status === "error" ? "✗" : "•"}
                </span>
                <span className="ops-detail">{o.detail}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
