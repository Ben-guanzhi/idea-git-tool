// Pure helpers for hunk-level staging (task ②), kept side-effect-free so they
// can be unit-tested without a DOM.

import type { ConflictInfo, FileStatus, StatusInfo } from "./types";

export interface HunkStagingViews {
  /** Show "stage this hunk" buttons for the working-tree (unstaged) hunks. */
  unstaged: boolean;
  /** Show "unstage this hunk" buttons for the index (staged) hunks. */
  staged: boolean;
}

/**
 * Hunk-level staging is offered only for plain "modified" files: untracked /
 * added / deleted / renamed / conflicted files keep the existing file-level
 * stage/unstage path (MVP constraint).
 */
export function hunkStagingViews(
  selectedFile: string | null,
  status: StatusInfo | null,
  conflicts: ConflictInfo[]
): HunkStagingViews {
  if (!selectedFile || !status) return { unstaged: false, staged: false };
  if (conflicts.some((c) => c.path === selectedFile)) {
    return { unstaged: false, staged: false };
  }
  const isModified = (files: FileStatus[]) =>
    files.some((f) => f.path === selectedFile && f.status === "modified");
  return {
    unstaged: isModified(status.unstaged ?? []),
    staged: isModified(status.staged ?? []),
  };
}

/** One-line hunk header, mirroring the `@@ -old +new @@` git convention. */
export function hunkLabel(oldStart: number, newStart: number, oldCount: number, newCount: number): string {
  return `@@ -${oldStart} +${newStart} @@ (-${oldCount} +${newCount})`;
}
