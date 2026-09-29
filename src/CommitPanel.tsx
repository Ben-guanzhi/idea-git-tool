// Commit message box and the commit / stage-&-commit action button.

import type { BusyKey } from "./types";

export function CommitPanel({
  message,
  onMessage,
  busy,
  canCommit,
  stagedCount,
  unstagedCount,
  hasChanges,
  onCommit,
}: {
  message: string;
  onMessage: (value: string) => void;
  busy: BusyKey;
  canCommit: boolean;
  stagedCount: number;
  unstagedCount: number;
  hasChanges: boolean;
  onCommit: () => void;
}) {
  return (
    <div className="commit-panel">
      <textarea
        className="commit-msg"
        placeholder="Commit message (first line = subject, rest = body)…  Ctrl+Enter to commit"
        value={message}
        onChange={(e) => onMessage(e.currentTarget.value)}
      />
      <div className="commit-side">
        <button
          className="commit-btn"
          onClick={onCommit}
          disabled={!canCommit}
          title={
            stagedCount > 0
              ? `Commit ${stagedCount} staged file(s)`
              : unstagedCount > 0
                ? `Stage & commit ${unstagedCount} file(s)`
                : "Nothing to commit"
          }
        >
          {busy === "commit"
            ? "Committing…"
            : stagedCount > 0
              ? `Commit (${stagedCount})`
              : unstagedCount > 0
                ? `Stage & Commit (${unstagedCount})`
                : "Commit"}
        </button>
        <span className="commit-hint muted">
          {hasChanges
            ? `${stagedCount} staged · ${unstagedCount} unstaged`
            : "Working tree clean"}
        </span>
      </div>
    </div>
  );
}
