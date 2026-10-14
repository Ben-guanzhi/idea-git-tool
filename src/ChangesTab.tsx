import type { ReactNode } from "react";
import { CommitPanel } from "./CommitPanel";
import { ConflictEditor } from "./ConflictEditor";
import { FileList } from "./FileTree";
import { HunkPanel } from "./HunkPanel";
import type { BusyKey, ConflictInfo, StatusInfo, StagingHunk } from "./types";
import type { FileViewAction, FileViewState } from "./viewState";

interface ChangesTabProps {
  status: StatusInfo | null;
  selectedFile: string | null;
  setSelectedFile: (p: string) => void;
  checkedPaths: Set<string>;
  toggleFileChecked: (p: string) => void;
  setDirChecked: (dirPaths: string[], on: boolean) => void;
  clearChecked: () => void;
  collapsedDirs: Set<string>;
  toggleDir: (dir: string) => void;
  stagedChecked: string[];
  stagedAll: string[];
  unstagedChecked: string[];
  unstagedAll: string[];
  doStage: (paths: string[]) => void;
  doUnstage: (paths: string[]) => void;
  doDiscard: (paths: string[]) => void;
  message: string;
  setMessage: (m: string) => void;
  busy: BusyKey;
  canCommit: boolean;
  stagedCount: number;
  unstagedCount: number;
  hasChanges: boolean;
  doCommit: () => void;
  selectedConflict: ConflictInfo | null;
  doResolve: (path: string, choice: string) => Promise<void>;
  repoPath: string;
  fileView: FileViewState;
  diffView: ReactNode;
  dispatchFileView: (action: FileViewAction) => void;
  saveEdit: () => void;
  unstagedHunks: StagingHunk[];
  stagedHunks: StagingHunk[];
  doStageHunk: (h: StagingHunk) => void;
  doUnstageHunk: (h: StagingHunk) => void;
  refresh: () => void;
  setError: (e: string) => void;
}

export function ChangesTab(p: ChangesTabProps) {
  return (
    <div className="changes-pane">
      <div className="changes-body">
        <aside className="changes-tree">
          <FileList
            title="Staged"
            files={p.status?.staged ?? []}
            selected={p.selectedFile}
            onSelect={p.setSelectedFile}
            bulkLabel={
              p.stagedChecked.length > 0
                ? `Unstage (${p.stagedChecked.length})`
                : "Unstage all"
            }
            onBulk={() =>
              p.doUnstage(
                p.stagedChecked.length > 0 ? p.stagedChecked : p.stagedAll
              )
            }
            actionLabel="Unstage"
            actionIcon="↑"
            onAction={(path) => p.doUnstage([path])}
            collapsed={p.collapsedDirs}
            onToggleDir={p.toggleDir}
            checked={p.checkedPaths}
            onToggleFile={p.toggleFileChecked}
            onSetDir={p.setDirChecked}
            onClearChecked={p.clearChecked}
          />
          <FileList
            title="Unstaged"
            files={p.status?.unstaged ?? []}
            selected={p.selectedFile}
            onSelect={p.setSelectedFile}
            bulkLabel={
              p.unstagedChecked.length > 0
                ? `Stage (${p.unstagedChecked.length})`
                : "Stage all"
            }
            onBulk={() =>
              p.doStage(
                p.unstagedChecked.length > 0 ? p.unstagedChecked : p.unstagedAll
              )
            }
            actionLabel="Stage"
            actionIcon="↓"
            onAction={(path) => p.doStage([path])}
            onDiscard={(path) => p.doDiscard([path])}
            collapsed={p.collapsedDirs}
            onToggleDir={p.toggleDir}
            checked={p.checkedPaths}
            onToggleFile={p.toggleFileChecked}
            onSetDir={p.setDirChecked}
            onClearChecked={p.clearChecked}
          />
        </aside>

        <div className="diff-area">
          {p.selectedConflict ? (
            <ConflictEditor
              conflict={p.selectedConflict}
              repoPath={p.repoPath}
              onResolve={p.doResolve}
              onSaved={p.refresh}
              onError={p.setError}
            />
          ) : p.selectedFile ? (
            <div className="file-diff">
              <div className="diff-toolbar">
                <span className="diff-title">{p.selectedFile}</span>
                {p.fileView.isBinary ? (
                  <span className="muted">Binary file — editing disabled</span>
                ) : (
                  <>
                    {p.fileView.editing && (
                      <button className="btn" onClick={p.saveEdit}>
                        Save
                      </button>
                    )}
                    <button
                      className="btn"
                      onClick={() =>
                        p.dispatchFileView({ type: "toggleEditing" })
                      }
                    >
                      {p.fileView.editing ? "Done editing" : "Edit"}
                    </button>
                  </>
                )}
              </div>
              {p.fileView.editing && !p.fileView.isBinary ? (
                <textarea
                  className="edit-area"
                  value={p.fileView.editText}
                  onChange={(e) =>
                    p.dispatchFileView({
                      type: "edit",
                      text: e.currentTarget.value,
                    })
                  }
                />
              ) : (
                <div className="diff-lines">
                  {p.diffView ?? <span className="muted">Loading...</span>}
                </div>
              )}
              {(p.unstagedHunks || p.stagedHunks) && (
                <HunkPanel
                  unstagedHunks={p.unstagedHunks}
                  stagedHunks={p.stagedHunks}
                  onStageHunk={p.doStageHunk}
                  onUnstageHunk={p.doUnstageHunk}
                />
              )}
            </div>
          ) : (
            <div className="empty-pane">Select a file to view its diff.</div>
          )}
        </div>
      </div>

      <CommitPanel
        message={p.message}
        onMessage={p.setMessage}
        busy={p.busy}
        canCommit={p.canCommit}
        stagedCount={p.stagedCount}
        unstagedCount={p.unstagedCount}
        hasChanges={p.hasChanges}
        onCommit={p.doCommit}
      />
    </div>
  );
}
