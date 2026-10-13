// Commit-log tab cluster: split verbatim from App.tsx (tab 54, logs/logHasMore
// 73-74, explorer reducer 76-79, loadLog 325-338, loadMoreLog 340-342,
// selectCommit 344-360, selectCommitFile 362-374, commitDiffView 799-802;
// LOG_PAGE_SIZE lived at App.tsx:44).

import { useMemo, useReducer, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { diffRowsToJsx } from "../DiffView";
import {
  commitExplorerReducer,
  COMMIT_EXPLORER_INITIAL,
} from "../viewState";
import type { CommitFile, CommitSummary, FileDiff, LogPage } from "../types";

const LOG_PAGE_SIZE = 50;

export function useCommitLog(
  repoPath: string,
  setError: (message: string) => void
) {
  const [tab, setTab] = useState<"changes" | "log">("changes");
  const [logs, setLogs] = useState<CommitSummary[]>([]);
  const [logHasMore, setLogHasMore] = useState(false);
  const [explorer, dispatchExplorer] = useReducer(
    commitExplorerReducer,
    COMMIT_EXPLORER_INITIAL
  );

  async function loadLog(offset = 0) {
    setError("");
    try {
      const r = await invoke<LogPage>("get_log", {
        repoPath,
        limit: LOG_PAGE_SIZE,
        offset,
      });
      setLogs((prev) => (offset === 0 ? r.rows : [...prev, ...r.rows]));
      setLogHasMore(r.hasMore);
    } catch (e) {
      setError(String(e));
    }
  }

  async function loadMoreLog() {
    await loadLog(logs.length);
  }

  async function selectCommit(id: string) {
    dispatchExplorer({ type: "select", commit: id });
    setError("");
    try {
      const files = await invoke<CommitFile[]>("get_commit_files", {
        repoPath,
        commitId: id,
      });
      dispatchExplorer({ type: "files", commit: id, files });
      if (files.length > 0) {
        await selectCommitFile(id, files[0].path);
      }
    } catch (e) {
      setError(String(e));
      dispatchExplorer({ type: "loadFailed" });
    }
  }

  async function selectCommitFile(id: string, path: string) {
    dispatchExplorer({ type: "selectFile", file: path });
    try {
      const d = await invoke<FileDiff>("get_commit_file_diff", {
        repoPath,
        commitId: id,
        path,
      });
      dispatchExplorer({ type: "loaded", file: path, diff: d });
    } catch (e) {
      setError(String(e));
    }
  }

  const commitDiffView = useMemo(
    () => diffRowsToJsx(explorer.diff),
    [explorer.diff]
  );

  return {
    tab,
    setTab,
    logs,
    logHasMore,
    loadLog,
    loadMoreLog,
    explorer,
    selectCommit,
    selectCommitFile,
    commitDiffView,
  };
}
