// App is the composition layer: the state clusters that used to live inline
// here were moved verbatim into src/hooks/* (one hook per cluster). The hook
// call order below preserves the original App.tsx effect order:
// refresh / repo-persist (useRepoStatus), info/error dismiss (useRepoStatus),
// keyboard shortcuts (useCommitFlow), file-diff load (useSelectedFileDiff).

import { ChangesTab } from "./ChangesTab";
import { LogPane } from "./LogPane";
import { OpsPanel } from "./OpsPanel";
import { TopBar } from "./TopBar";
import { BranchActions } from "./BranchActions";
import { StashTagBar } from "./StashTagBar";
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
          <ChangesTab
            status={repo.status}
            selectedFile={selection.selectedFile}
            setSelectedFile={selection.setSelectedFile}
            checkedPaths={repo.checkedPaths}
            toggleFileChecked={repo.toggleFileChecked}
            setDirChecked={repo.setDirChecked}
            clearChecked={repo.clearChecked}
            collapsedDirs={treeUi.collapsedDirs}
            toggleDir={treeUi.toggleDir}
            stagedChecked={commitFlow.stagedChecked}
            stagedAll={commitFlow.stagedAll}
            unstagedChecked={commitFlow.unstagedChecked}
            unstagedAll={commitFlow.unstagedAll}
            doStage={commitFlow.doStage}
            doUnstage={commitFlow.doUnstage}
            doDiscard={commitFlow.doDiscard}
            message={commitFlow.message}
            setMessage={commitFlow.setMessage}
            busy={commitFlow.busy}
            canCommit={commitFlow.canCommit}
            stagedCount={commitFlow.stagedCount}
            unstagedCount={commitFlow.unstagedCount}
            hasChanges={commitFlow.hasChanges}
            doCommit={commitFlow.doCommit}
            selectedConflict={conflict.selectedConflict}
            doResolve={conflict.doResolve}
            repoPath={repo.repoPath}
            fileView={fileDiff.fileView}
            diffView={fileDiff.diffView}
            dispatchFileView={fileDiff.dispatchFileView}
            saveEdit={fileDiff.saveEdit}
            unstagedHunks={hunkOps.unstagedHunks}
            stagedHunks={hunkOps.stagedHunks}
            doStageHunk={hunkOps.doStageHunk}
            doUnstageHunk={hunkOps.doUnstageHunk}
            refresh={repo.refresh}
            setError={repo.setError}
          />
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
