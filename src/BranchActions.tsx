// Footer action strip: merge selector, abort-merge, create-branch, remote name.

import type { RefObject } from "react";
import type { BranchInfo, BusyKey } from "./types";

export function BranchActions({
  repoPath,
  busy,
  conflictsCount,
  branch,
  onBranch,
  branchInputRef,
  mergeTarget,
  onMergeTarget,
  mergeCandidates,
  branchCount,
  currentBranchName,
  remote,
  onRemote,
  onMerge,
  onAbortMerge,
  onCreateBranch,
  rebaseTarget,
  onRebaseTarget,
  rebaseInProgress,
  onRebase,
  onRebaseContinue,
  onRebaseAbort,
  onRefresh,
}: {
  repoPath: string;
  busy: BusyKey;
  conflictsCount: number;
  branch: string;
  onBranch: (value: string) => void;
  branchInputRef: RefObject<HTMLInputElement | null>;
  mergeTarget: string;
  onMergeTarget: (value: string) => void;
  mergeCandidates: BranchInfo[];
  branchCount: number;
  currentBranchName: string;
  remote: string;
  onRemote: (value: string) => void;
  onMerge: () => void;
  onAbortMerge: () => void;
  onCreateBranch: () => void;
  rebaseTarget: string;
  onRebaseTarget: (value: string) => void;
  rebaseInProgress: boolean;
  onRebase: () => void;
  onRebaseContinue: () => void;
  onRebaseAbort: () => void;
  onRefresh: () => void;
}) {
  return (
    <div className="branch-actions">
      <span className="branch-actions-label">Merge</span>
      <select
        className="small-input"
        value={mergeTarget}
        onChange={(e) => onMergeTarget(e.currentTarget.value)}
        onFocus={onRefresh}
        title={
          mergeCandidates.length === 0
            ? "没有可合并的其它分支（含远程）；请先创建分支"
            : `将所选分支合并到 ${currentBranchName || "当前分支"}`
        }
      >
        <option value="">
          {mergeCandidates.length === 0
            ? branchCount === 0
              ? "— 无分支列表 —"
              : "— 仅当前分支 —"
            : "— 选择分支 —"}
        </option>
        {mergeCandidates.map((b) => (
          <option key={b.refname} value={b.name}>
            {b.name}
          </option>
        ))}
      </select>
      <button
        className="btn"
        onClick={onMerge}
        disabled={!mergeTarget || !!busy || mergeCandidates.length === 0}
        title={
          mergeTarget
            ? `将 ${mergeTarget} 合并到 ${currentBranchName || "当前分支"}`
            : mergeCandidates.length === 0
              ? "没有可合并的其它分支"
              : "请先选择分支"
        }
      >
        {busy === "merge"
          ? "Merging…"
          : `Merge into ${currentBranchName || "current"}`}
      </button>
      {conflictsCount > 0 && (
        <button className="btn abort" onClick={onAbortMerge} disabled={!!busy}>
          {busy === "abort" ? "Aborting…" : `Abort merge (${conflictsCount})`}
        </button>
      )}

      <span className="branch-actions-divider" />

      <span className="branch-actions-label">Rebase</span>
      {rebaseInProgress ? (
        <>
          <button
            className="btn"
            onClick={onRebaseContinue}
            disabled={!!busy}
            title="所有冲突解决完毕后继续变基（保留原提交信息）"
          >
            {busy === "rebase" ? "Continuing…" : "Continue"}
          </button>
          <button
            className="btn abort"
            onClick={onRebaseAbort}
            disabled={!!busy}
            title="放弃变基，恢复到开始前的分支状态"
          >
            Abort rebase
          </button>
        </>
      ) : (
        <>
          <select
            className="small-input"
            value={rebaseTarget}
            onChange={(e) => onRebaseTarget(e.currentTarget.value)}
            onFocus={onRefresh}
            title={
              mergeCandidates.length === 0
                ? "没有可变基到的目标分支；请先创建分支"
                : `将 ${currentBranchName || "当前分支"} 的提交重放到所选分支之上`
            }
          >
            <option value="">
              {mergeCandidates.length === 0
                ? branchCount === 0
                  ? "— 无分支列表 —"
                  : "— 仅当前分支 —"
                : "— 选择目标 —"}
            </option>
            {mergeCandidates.map((b) => (
              <option key={b.refname} value={b.name}>
                {b.name}
              </option>
            ))}
          </select>
          <button
            className="btn"
            onClick={onRebase}
            disabled={!rebaseTarget || !!busy || mergeCandidates.length === 0}
            title={
              rebaseTarget
                ? `将当前分支变基到 ${rebaseTarget}（当前提交重放其上）`
                : "请先选择目标分支"
            }
          >
            {busy === "rebase" ? "Rebasing…" : "Rebase onto"}
          </button>
        </>
      )}

      <span className="branch-actions-divider" />

      <input
        ref={branchInputRef}
        className="small-input"
        placeholder="new branch name"
        value={branch}
        onChange={(e) => onBranch(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") onCreateBranch();
        }}
      />
      <button
        className="btn"
        onClick={onCreateBranch}
        disabled={!branch.trim() || !repoPath.trim()}
      >
        Create branch
      </button>

      <span className="branch-actions-divider" />

      <input
        className="small-input"
        placeholder="remote"
        value={remote}
        onChange={(e) => onRemote(e.currentTarget.value)}
      />
    </div>
  );
}
