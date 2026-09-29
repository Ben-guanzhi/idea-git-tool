// Reducers for the two async view states, kept as pure state machines.

import type { CommitFile, FileDiff } from "./types";

// ── File view state machine: what the Changes tab shows for the selected file.
// A file selection loads content and diff as one atomic transition; the editor
// text and the edit toggle then change independently. ──
export interface FileViewState {
  diff: FileDiff | null;
  editText: string;
  editing: boolean;
  isBinary: boolean;
}

export type FileViewAction =
  | { type: "clear" }
  | {
      type: "loaded";
      diff: FileDiff;
      working: string;
      conflicted: boolean;
      isBinary: boolean;
    }
  | { type: "edit"; text: string }
  | { type: "toggleEditing" };

export function fileViewReducer(
  state: FileViewState,
  action: FileViewAction
): FileViewState {
  switch (action.type) {
    case "clear":
      return { diff: null, editText: "", editing: false, isBinary: false };
    case "loaded":
      return {
        diff: action.diff,
        editText: action.working,
        editing: action.conflicted && !action.isBinary,
        isBinary: action.isBinary,
      };
    case "edit":
      return { ...state, editText: action.text };
    case "toggleEditing":
      return { ...state, editing: !state.editing };
  }
}

export const FILE_VIEW_INITIAL: FileViewState = {
  diff: null,
  editText: "",
  editing: false,
  isBinary: false,
};

// ── Commit explorer state machine: the Log tab's drill-down. Selecting a
// commit resets the file pane; files and diff arrive asynchronously, and each
// response carries the selection it belongs to so a stale reply for an older
// selection cannot land. ──
export interface CommitExplorerState {
  commit: string | null;
  files: CommitFile[];
  file: string | null;
  diff: FileDiff | null;
}

export type CommitExplorerAction =
  | { type: "select"; commit: string }
  | { type: "files"; commit: string; files: CommitFile[] }
  | { type: "selectFile"; file: string }
  | { type: "loaded"; file: string; diff: FileDiff }
  | { type: "loadFailed" };

export function commitExplorerReducer(
  state: CommitExplorerState,
  action: CommitExplorerAction
): CommitExplorerState {
  switch (action.type) {
    case "select":
      return { commit: action.commit, files: [], file: null, diff: null };
    case "files":
      return state.commit === action.commit
        ? { ...state, files: action.files }
        : state;
    case "selectFile":
      return { ...state, file: action.file, diff: null };
    case "loaded":
      return state.file === action.file
        ? { ...state, diff: action.diff }
        : state;
    case "loadFailed":
      return { ...state, files: [] };
  }
}

export const COMMIT_EXPLORER_INITIAL: CommitExplorerState = {
  commit: null,
  files: [],
  file: null,
  diff: null,
};
