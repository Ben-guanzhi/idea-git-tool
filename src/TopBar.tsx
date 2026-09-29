// Top toolbar: repo path, branch switch, ahead/behind, and the remote actions.

import type { RefObject } from "react";
import type { BranchInfo, BusyKey, StatusInfo } from "./types";

export function TopBar({
  repoPath,
  onRepoPath,
  status,
  branches,
  busy,
  branch,
  branchInputRef,
  onCheckout,
  onFetch,
  onPull,
  onPush,
  onCreateBranch,
}: {
  repoPath: string;
  onRepoPath: (value: string) => void;
  status: StatusInfo | null;
  branches: BranchInfo[];
  busy: BusyKey;
  branch: string;
  branchInputRef: RefObject<HTMLInputElement | null>;
  onCheckout: (name: string) => void;
  onFetch: () => void;
  onPull: () => void;
  onPush: () => void;
  onCreateBranch: () => void;
}) {
  const disabledRemote = !!busy || !repoPath.trim();
  return (
    <header className="ide-topbar">
      <input
        className="repo-input"
        placeholder="Repository path..."
        value={repoPath}
        onChange={(e) => onRepoPath(e.currentTarget.value)}
      />
      <select
        className="branch-select"
        value={status?.branch ?? ""}
        onChange={(e) => onCheckout(e.currentTarget.value)}
        title="Switch branch"
      >
        {branches.map((b) => (
          <option key={b.refname} value={b.name}>
            {b.name}
            {b.isCurrent ? " *" : ""}
          </option>
        ))}
      </select>
      {status?.upstream && (
        <span className="upstream">
          {status.ahead > 0 && `↑${status.ahead} `}
          {status.behind > 0 && `↓${status.behind} `}
          {status.ahead === 0 && status.behind === 0 && "up to date"}
        </span>
      )}
      <button
        className="btn"
        onClick={onFetch}
        disabled={disabledRemote}
        title="Fetch from remote (F5 also refreshes local status)"
      >
        {busy === "fetch" ? "Fetching…" : "Fetch"}
      </button>
      <button
        className="btn"
        onClick={onPull}
        disabled={disabledRemote}
        title="Pull from remote"
      >
        {busy === "pull" ? "Pulling…" : "Pull"}
      </button>
      <button
        className="btn primary"
        onClick={onPush}
        disabled={disabledRemote}
        title="Push to remote (sets upstream on first push)"
      >
        {busy === "push" ? "Pushing…" : "Push"}
      </button>
      <button
        className="btn"
        onClick={() => {
          if (!branch.trim()) {
            branchInputRef.current?.focus();
          } else {
            onCreateBranch();
          }
        }}
        disabled={!repoPath.trim()}
        title="Create a new branch (focuses the name field if empty)"
      >
        New Branch
      </button>
    </header>
  );
}
