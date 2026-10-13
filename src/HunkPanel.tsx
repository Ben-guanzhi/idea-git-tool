// Thin panel listing the working-tree (unstaged) and index (staged) hunks of
// the selected modified file, each with a hunk-level stage/unstage button.
// Mounted next to the diff view; the existing file-level stage/unstage path
// is untouched for all other file kinds (MVP constraint).

import type { StagingHunk } from "./types";
import { hunkLabel } from "./hunks";

function HunkItem({
  hunk,
  actionLabel,
  onAction,
}: {
  hunk: StagingHunk;
  actionLabel: string;
  onAction: (h: StagingHunk) => void;
}) {
  return (
    <div className="hunk-item">
      <div className="hunk-header">
        <span className="diff-title muted">
          {hunkLabel(
            hunk.oldStart,
            hunk.newStart,
            hunk.oldLines.length,
            hunk.newLines.length
          )}
        </span>
        {/* The hunk object from get_hunks is passed back verbatim. */}
        <button
          className="mini-btn"
          onClick={() => onAction(hunk)}
          title={actionLabel}
        >
          {actionLabel}
        </button>
      </div>
      <div className="diff-lines hunk-lines">
        {hunk.oldLines.map((l, i) => (
          <div key={`o${i}`} className="diff-row diff-removed">
            <div className="diff-cell">&minus; {l}</div>
          </div>
        ))}
        {hunk.newLines.map((l, i) => (
          <div key={`n${i}`} className="diff-row diff-added">
            <div className="diff-cell">+ {l}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

export function HunkPanel({
  unstagedHunks,
  stagedHunks,
  onStageHunk,
  onUnstageHunk,
}: {
  unstagedHunks: StagingHunk[];
  stagedHunks: StagingHunk[];
  onStageHunk: (h: StagingHunk) => void;
  onUnstageHunk: (h: StagingHunk) => void;
}) {
  if (unstagedHunks.length === 0 && stagedHunks.length === 0) return null;
  return (
    <div className="hunk-panel">
      {unstagedHunks.length > 0 && (
        <div className="hunk-group">
          <div className="file-group-header">
            <span className="file-group-title">
              Unstaged hunks ({unstagedHunks.length})
            </span>
          </div>
          {unstagedHunks.map((h, i) => (
            <HunkItem
              key={`u${i}`}
              hunk={h}
              actionLabel="暂存此块"
              onAction={onStageHunk}
            />
          ))}
        </div>
      )}
      {stagedHunks.length > 0 && (
        <div className="hunk-group">
          <div className="file-group-header">
            <span className="file-group-title">
              Staged hunks ({stagedHunks.length})
            </span>
          </div>
          {stagedHunks.map((h, i) => (
            <HunkItem
              key={`s${i}`}
              hunk={h}
              actionLabel="取消暂存此块"
              onAction={onUnstageHunk}
            />
          ))}
        </div>
      )}
    </div>
  );
}
