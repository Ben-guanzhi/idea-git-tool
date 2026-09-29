// Log tab: commit list, commit-file tree, and the selected file's diff.

import type { ReactNode } from "react";
import { FileTreeView } from "./FileTree";
import type { CommitFile, CommitSummary } from "./types";

export function LogPane({
  logs,
  selectedCommit,
  files,
  selectedFile,
  diffView,
  collapsed,
  onToggleDir,
  onSelectCommit,
  onSelectFile,
}: {
  logs: CommitSummary[];
  selectedCommit: string | null;
  files: CommitFile[];
  selectedFile: string | null;
  diffView: ReactNode;
  collapsed: Set<string>;
  onToggleDir: (path: string) => void;
  onSelectCommit: (id: string) => void;
  onSelectFile: (path: string) => void;
}) {
  return (
    <div className="log-pane">
      <div className="log-body">
        <div className="log-list">
          {logs.map((c) => (
            <div
              key={c.id}
              className={
                selectedCommit === c.id ? "log-item selected" : "log-item"
              }
              onClick={() => onSelectCommit(c.id)}
            >
              <div className="log-refs">
                {c.refs.map((r) => (
                  <span key={r} className="log-ref">
                    {r}
                  </span>
                ))}
              </div>
              <div className="log-msg">{c.message.split("\n")[0]}</div>
              <div className="log-meta">
                {c.author} ·{" "}
                {new Date(Number(c.time) * 1000).toLocaleString()} ·{" "}
                {c.shortId}
              </div>
            </div>
          ))}
          {logs.length === 0 && <div className="muted">No commits yet.</div>}
        </div>

        <aside className="commit-files">
          <div className="file-group-header">
            <span className="file-group-title">
              Files ({files.length})
            </span>
          </div>
          {selectedCommit && files.length === 0 && (
            <div className="file-empty">No files</div>
          )}
          {!selectedCommit && <div className="file-empty">Select a commit</div>}
          {files.length > 0 && (
            <FileTreeView
              files={files.map((f) => ({
                path: f.path,
                status: f.status,
              }))}
              titleKey="log"
              collapsed={collapsed}
              onToggleDir={onToggleDir}
              selected={selectedFile}
              onSelect={onSelectFile}
            />
          )}
        </aside>

        <div className="diff-area">
          {selectedFile ? (
            <div className="file-diff">
              <div className="diff-toolbar">
                <span className="diff-title">{selectedFile}</span>
              </div>
              <div className="diff-lines">
                {diffView ?? <span className="muted">Loading...</span>}
              </div>
            </div>
          ) : (
            <div className="empty-pane">
              Select a commit to view its files.
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
