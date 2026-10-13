// Hunk-level staging (task ②). Loads the staged/unstaged hunk lists for the
// selected "modified" file via get_hunks and stages/unstages single hunks
// through the existing runOp channel (busy lock, op log, status refresh),
// then reloads the diff through the existing loadFileView path. On a backend
// reject ("内容已变化，请刷新") runOp's catch already shows the error banner
// and refreshes the status, which re-triggers the hunk reload below.

import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ConflictInfo, StagingHunk, StatusInfo } from "../types";
import { hunkStagingViews } from "../hunks";
import type { RunOp } from "./useCommitFlow";

export function useHunks(params: {
  repoPath: string;
  selectedFile: string | null;
  status: StatusInfo | null;
  conflicts: ConflictInfo[];
  runOp: RunOp;
  setError: (message: string) => void;
  loadFileView: (path: string) => Promise<void>;
}) {
  const { repoPath, selectedFile, status, conflicts, runOp, setError, loadFileView } =
    params;

  const views = useMemo(
    () => hunkStagingViews(selectedFile, status, conflicts),
    [selectedFile, status, conflicts]
  );

  const [hunks, setHunks] = useState<{
    unstaged: StagingHunk[];
    staged: StagingHunk[];
  }>({ unstaged: [], staged: [] });

  useEffect(() => {
    if (!selectedFile || (!views.unstaged && !views.staged)) {
      setHunks({ unstaged: [], staged: [] });
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const [u, s] = await Promise.all([
          invoke<StagingHunk[]>("get_hunks", {
            repoPath,
            path: selectedFile,
            fromIndex: false,
          }),
          invoke<StagingHunk[]>("get_hunks", {
            repoPath,
            path: selectedFile,
            fromIndex: true,
          }),
        ]);
        if (!cancelled) setHunks({ unstaged: u, staged: s });
      } catch (e) {
        if (!cancelled) {
          setError(String(e));
          setHunks({ unstaged: [], staged: [] });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selectedFile, repoPath, views, setError]);

  async function runHunkOp(hunk: StagingHunk, stage: boolean) {
    if (!selectedFile) return;
    const path = selectedFile;
    // Pass the backend's hunk object back unchanged — no field rebuilding,
    // so line numbers cannot drift.
    await runOp({
      key: stage ? "stage" : "unstage",
      op: stage ? "stage hunk" : "unstage hunk",
      detail: `${path} ${stage ? "+" : "-"}hunk @${hunk.newStart}`,
      fn: async () => {
        await invoke(stage ? "stage_hunks" : "unstage_hunks", {
          repoPath,
          path,
          hunks: [hunk],
        });
        return "";
      },
    });
    // runOp refreshed the file status (and the hunk list follows via `views`),
    // the diff itself is re-pulled through the existing load path.
    try {
      await loadFileView(path);
    } catch (e) {
      setError(String(e));
    }
  }

  function doStageHunk(hunk: StagingHunk) {
    void runHunkOp(hunk, true);
  }

  function doUnstageHunk(hunk: StagingHunk) {
    void runHunkOp(hunk, false);
  }

  return {
    views,
    unstagedHunks: hunks.unstaged,
    stagedHunks: hunks.staged,
    doStageHunk,
    doUnstageHunk,
  };
}
