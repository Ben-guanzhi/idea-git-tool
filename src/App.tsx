// App is the composition layer: the state clusters that used to live inline
// here were moved verbatim into src/hooks/* (one hook per cluster). The hook
// call order below preserves the original App.tsx effect order:
// refresh / repo-persist (useRepoStatus), info/error dismiss (useRepoStatus),
// keyboard shortcuts (useCommitFlow), file-diff load (useSelectedFileDiff).

import { ConflictEditor } from "./ConflictEditor";
import { FileList } from "./FileTree";
import { HunkPanel } from "./HunkPanel";
import { LogPane } from "./LogPane";
import { OpsPanel } from "./OpsPanel";
import { TopBar } from "./TopBar";
import { BranchActions } from "./BranchActions";
import { StashTagBar } from "./StashTagBar";
import { CommitPanel } from "./CommitPanel";
import { useBranchOps } from "./hooks/useBranchOps";
import { useCommitFlow } from "./hooks/useCommitFlow";
import { useCommitLog } from "./hooks/useCommitLog";
import { useConflict } from "./hooks/useConflict";
import { useFileSelection } from "./hooks/useFileSelection";
import { useHunks } from "./hooks/useHunks";
import { useOpsPanel } from "./hooks/useOpsPanel";
import { useRepoStatus } from "./hooks/useRepoStatus";
import { useSelectedFileDiff } from "./hooks/useSelectedFileDiff";
import { useStashTags } from "./hooks/useStashTags";
import { useTreeUi } from "./hooks/useTreeUi";

import "./App.css";

function App() {
  const repo = useRepoStatus();
  const opsPanel = useOpsPanel(repo.repoPath);
  const treeUi = useTreeUi();
  const selection = useFileSelection();
  const commitLog = useCommitLog(repo.repoPath, repo.setError);
  const commitFlow = useCommitFlow({
    repoPath: repo.repoPath,
    status: repo.status,
    refresh: repo.refresh,
    loadLog: commitLog.loadLog,
    logs: commitLog.logs,
    tab: commitLog.tab,
    logOp: opsPanel.logOp,
    setError: repo.setError,
    setInfo: repo.setInfo,
    checkedPaths: repo.checkedPaths,
    setCheckedPaths: repo.setCheckedPaths,
    selectedFile: selection.selectedFile,
    setSelectedFile: selection.setSelectedFile,
  });
  const fileDiff = useSelectedFileDiff({
    repoPath: repo.repoPath,
    selectedFile: selection.selectedFile,
    refresh: repo.refresh,
    setError: repo.setError,
    setInfo: repo.setInfo,
  });
  const conflict = useConflict({
    repoPath: repo.repoPath,
    conflicts: repo.conflicts,
    selectedFile: selection.selectedFile,
    runOp: commitFlow.runOp,
  });
  const branchOps = useBranchOps({
    repoPath: repo.repoPath,
    branches: repo.branches,
    status: repo.status,
    runOp: commitFlow.runOp,
    setError: repo.setError,
    setInfo: repo.setInfo,
    setTab: commitLog.setTab,
    setSelectedFile: selection.setSelectedFile,
  });
  const stashTags = useStashTags({
    repoPath: repo.repoPath,
    runOp: commitFlow.runOp,
    logOp: opsPanel.logOp,
    setError: repo.setError,
    stashSel: repo.stashSel,
    setStashSel: repo.setStashSel,
    tagSel: repo.tagSel,
    setTagSel: repo.setTagSel,
  });
  const hunkOps = useHunks({
    repoPath: repo.repoPath,
    selectedFile: selection.selectedFile,
    status: repo.status,
    conflicts: repo.conflicts,
    runOp: commitFlow.runOp,
    setError: repo.setError,
    loadFileView: fileDiff.loadFileView,
  });

  return (
    <div className="ide">
      <TopBar
        repoPath={repo.repoPath}
        onRepoPath={repo.setRepoPath}
        status={repo.status}
        branches={repo.branches}
        busy={commitFlow.busy}
        branch={branchOps.branch}
        branchInputRef={branchOps.branchInputRef}
        onCheckout={branchOps.doCheckout}
        onFetch={branchOps.doFetch}
        onPull={branchOps.doPull}
        onPush={branchOps.doPush}
        onCreateBranch={branchOps.doCreateBranch}
      />

      {repo.error && <div className="ide-error">{repo.error}</div>}
      {repo.info && <div className="ide-info">{repo.info}</div>}

      <main className="ide-main">
        <div className="ide-tabs">
          <button
            className={commitLog.tab === "changes" ? "tab active" : "tab"}
            onClick={() => commitLog.setTab("changes")}
          >
            Changes
          </button>
          <button
            className={commitLog.tab === "log" ? "tab active" : "tab"}
            onClick={() => {
              commitLog.setTab("log");
              void commitLog.loadLog();
            }}
            title="Reload history (F5)"
          >
            Log
          </button>
        </div>

        {commitLog.tab === "changes" && (
          <div className="changes-pane">
            <div className="changes-body">
              <aside className="changes-tree">
              <FileList
                title="Staged"
                files={repo.status?.staged ?? []}
                selected={selection.selectedFile}
                onSelect={selection.setSelectedFile}
                bulkLabel={
                  commitFlow.stagedChecked.length > 0
                    ? `Unstage (${commitFlow.stagedChecked.length})`
                    : "Unstage all"
                }
                onBulk={() =>
                  commitFlow.doUnstage(
                    commitFlow.stagedChecked.length > 0
                      ? commitFlow.stagedChecked
                      : commitFlow.stagedAll
                  )
                }
                actionLabel="Unstage"
                actionIcon="↑"
                onAction={(p) => commitFlow.doUnstage([p])}
                collapsed={treeUi.collapsedDirs}
                onToggleDir={treeUi.toggleDir}
                checked={repo.checkedPaths}
                onToggleFile={repo.toggleFileChecked}
                onSetDir={repo.setDirChecked}
                onClearChecked={repo.clearChecked}
              />
              <FileList
                title="Unstaged"
                files={repo.status?.unstaged ?? []}
                selected={selection.selectedFile}
                onSelect={selection.setSelectedFile}
                bulkLabel={
                  commitFlow.unstagedChecked.length > 0
                    ? `Stage (${commitFlow.unstagedChecked.length})`
                    : "Stage all"
                }
                onBulk={() =>
                  commitFlow.doStage(
                    commitFlow.unstagedChecked.length > 0
                      ? commitFlow.unstagedChecked
                      : commitFlow.unstagedAll
                  )
                }
                actionLabel="Stage"
                actionIcon="↓"
                onAction={(p) => commitFlow.doStage([p])}
                onDiscard={(p) => commitFlow.doDiscard([p])}
                collapsed={treeUi.collapsedDirs}
                onToggleDir={treeUi.toggleDir}
                checked={repo.checkedPaths}
                onToggleFile={repo.toggleFileChecked}
                onSetDir={repo.setDirChecked}
                onClearChecked={repo.clearChecked}
              />
            </aside>

            <div className="diff-area">
              {conflict.selectedConflict ? (
                <ConflictEditor
                  conflict={conflict.selectedConflict}
                  repoPath={repo.repoPath}
                  onResolve={conflict.doResolve}
                  onSaved={repo.refresh}
                  onError={repo.setError}
                />
              ) : selection.selectedFile ? (
                <div className="file-diff">
                  <div className="diff-toolbar">
                    <span className="diff-title">{selection.selectedFile}</span>
                    {fileDiff.fileView.isBinary ? (
                      <span className="muted">Binary file — editing disabled</span>
                    ) : (
                      <>
                        {fileDiff.fileView.editing && (
                          <button className="btn" onClick={fileDiff.saveEdit}>
                            Save
                          </button>
                        )}
                        <button
                          className="btn"
                          onClick={() =>
                            fileDiff.dispatchFileView({ type: "toggleEditing" })
                          }
                        >
                          {fileDiff.fileView.editing ? "Done editing" : "Edit"}
                        </button>
                      </>
                    )}
                  </div>
                  {fileDiff.fileView.editing && !fileDiff.fileView.isBinary ? (
                    <textarea
                      className="edit-area"
                      value={fileDiff.fileView.editText}
                      onChange={(e) =>
                        fileDiff.dispatchFileView({
                          type: "edit",
                          text: e.currentTarget.value,
                        })
                      }
                    />
                  ) : (
                    <div className="diff-lines">
                      {fileDiff.diffView ?? <span className="muted">Loading...</span>}
                    </div>
                  )}
                  {(hunkOps.views.unstaged || hunkOps.views.staged) && (
                    <HunkPanel
                      unstagedHunks={hunkOps.unstagedHunks}
                      stagedHunks={hunkOps.stagedHunks}
                      onStageHunk={hunkOps.doStageHunk}
                      onUnstageHunk={hunkOps.doUnstageHunk}
                    />
                  )}
                </div>
              ) : (
                <div className="empty-pane">Select a file to view its diff.</div>
              )}
            </div>
            </div>

            <CommitPanel
              message={commitFlow.message}
              onMessage={commitFlow.setMessage}
              busy={commitFlow.busy}
              canCommit={commitFlow.canCommit}
              stagedCount={commitFlow.stagedCount}
              unstagedCount={commitFlow.unstagedCount}
              hasChanges={commitFlow.hasChanges}
              onCommit={commitFlow.doCommit}
            />
          </div>
        )}

        {commitLog.tab === "log" && (
          <LogPane
            logs={commitLog.logs}
            hasMore={commitLog.logHasMore}
            onLoadMore={() => void commitLog.loadMoreLog()}
            selectedCommit={commitLog.explorer.commit}
            files={commitLog.explorer.files}
            selectedFile={commitLog.explorer.file}
            diffView={commitLog.commitDiffView}
            collapsed={treeUi.logCollapsed}
            onToggleDir={treeUi.toggleLogDir}
            onSelectCommit={commitLog.selectCommit}
            onSelectFile={(p) => {
              if (commitLog.explorer.commit)
                void commitLog.selectCommitFile(commitLog.explorer.commit, p);
            }}
          />
        )}
      </main>

      <OpsPanel
        ops={opsPanel.ops}
        open={opsPanel.opsOpen}
        onToggle={opsPanel.toggleOps}
        onExport={opsPanel.exportOps}
        onClear={opsPanel.clearOps}
      />

      <StashTagBar
        repoPath={repo.repoPath}
        busy={commitFlow.busy}
        stashes={repo.stashes}
        tags={repo.tags}
        stashSel={repo.stashSel}
        onStashSel={repo.setStashSel}
        stashMsg={stashTags.stashMsg}
        onStashMsg={stashTags.setStashMsg}
        tagInput={stashTags.tagInput}
        onTagInput={stashTags.setTagInput}
        tagMsg={stashTags.tagMsg}
        onTagMsg={stashTags.setTagMsg}
        tagSel={repo.tagSel}
        onTagSel={repo.setTagSel}
        onSaveStash={stashTags.doStashSave}
        onApplyStash={stashTags.doStashApply}
        onPopStash={stashTags.doStashPop}
        onDropStash={stashTags.doStashDrop}
        onCreateTag={stashTags.doCreateTag}
        onDeleteTag={stashTags.doDeleteTag}
        onRefresh={() => {
          if (!commitFlow.busy) void repo.refresh();
        }}
      />

      <BranchActions
        repoPath={repo.repoPath}
        busy={commitFlow.busy}
        conflictsCount={repo.conflicts.length}
        branch={branchOps.branch}
        onBranch={branchOps.setBranch}
        branchInputRef={branchOps.branchInputRef}
        mergeTarget={branchOps.mergeTarget}
        onMergeTarget={branchOps.setMergeTarget}
        mergeCandidates={branchOps.mergeCandidates}
        branchCount={repo.branches.length}
        currentBranchName={branchOps.currentBranchName}
        remote={branchOps.remote}
        onRemote={branchOps.setRemote}
        onMerge={branchOps.doMerge}
        onAbortMerge={branchOps.doAbortMerge}
        onCreateBranch={branchOps.doCreateBranch}
        rebaseTarget={branchOps.rebaseTarget}
        onRebaseTarget={branchOps.setRebaseTarget}
        rebaseInProgress={repo.rebaseInProgress}
        onRebase={branchOps.doRebase}
        onRebaseContinue={branchOps.doRebaseContinue}
        onRebaseAbort={branchOps.doRebaseAbort}
        onRefresh={() => {
          if (!commitFlow.busy) void repo.refresh();
        }}
      />
    </div>
  );
}

export default App;
