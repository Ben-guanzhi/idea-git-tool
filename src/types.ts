// Shared data types mirroring the Rust command payloads (src-tauri/src/*.rs).

export interface ConflictInfo {
  path: string;
  ours: string;
  theirs: string;
  base: string;
}

export interface FileStatus {
  path: string;
  status: string;
}

export interface StatusInfo {
  branch: string;
  upstream?: string | null;
  ahead: number;
  behind: number;
  staged: FileStatus[];
  unstaged: FileStatus[];
}

export interface FileContent {
  path: string;
  working: string;
  conflicted: boolean;
  isBinary: boolean;
}

export interface Highlight {
  start: number;
  end: number;
}

export interface DiffSegment {
  text: string;
  highlights: Highlight[];
}

export interface DiffRow {
  kind: string;
  left: DiffSegment | null;
  right: DiffSegment | null;
}

export interface FileDiff {
  path: string;
  rows: DiffRow[];
}

export interface CommitSummary {
  id: string;
  shortId: string;
  message: string;
  author: string;
  email: string;
  time: string;
  parents: string[];
  refs: string[];
}

export interface LogPage {
  rows: CommitSummary[];
  hasMore: boolean;
}

export interface BranchInfo {
  name: string;
  isCurrent: boolean;
  refname: string;
}

export interface MergeResult {
  status: string;
  message: string;
  conflicts: string[];
}

export interface StashInfo {
  index: number;
  message: string;
}

export interface TagInfo {
  name: string;
  target: string;
  annotated: boolean;
  message?: string | null;
}

export interface CommitFile {
  path: string;
  status: string;
  newPath: string;
}

export const STATUS_ICON: Record<string, string> = {
  modified: "🖊",
  added: "+",
  deleted: "×",
  renamed: "→",
  conflicted: "⚠",
};

export const REPO_KEY = "idea-git-tool:repoPath";
export const OPS_KEY = "idea-git-tool:opsPanelOpen";

export type BusyKey =
  | "commit"
  | "push"
  | "pull"
  | "fetch"
  | "merge"
  | "abort"
  | "stage"
  | "unstage"
  | "discard"
  | "checkout"
  | "branch"
  | "resolve"
  | "rebase"
  | "stash"
  | "tag"
  | null;

export interface OpLogEntry {
  id: number;
  time: string;
  op: string;
  status: "ok" | "error" | "info";
  detail: string;
}

export interface TreeNode {
  name: string;
  path: string;
  dir: boolean;
  status?: string;
  children?: TreeNode[];
}
