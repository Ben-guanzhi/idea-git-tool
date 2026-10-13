// Selected working-file view cluster: split verbatim from App.tsx (fileView
// reducer 69-72, loading effect + comment 277-312, saveEdit 376-390, diffView
// memo 797). selectedFile itself lives in useFileSelection so the keyboard
// effect in useCommitFlow keeps its original position before this effect.

import { useEffect, useMemo, useReducer } from "react";
import { invoke } from "@tauri-apps/api/core";
import { diffRowsToJsx } from "../DiffView";
import {
  fileViewReducer,
  FILE_VIEW_INITIAL,
} from "../viewState";
import type { FileContent, FileDiff } from "../types";

export function useSelectedFileDiff(params: {
  repoPath: string;
  selectedFile: string | null;
  refresh: () => Promise<void>;
  setError: (message: string) => void;
  setInfo: (message: string) => void;
}) {
  const { repoPath, selectedFile, refresh, setError, setInfo } = params;

  const [fileView, dispatchFileView] = useReducer(
    fileViewReducer,
    FILE_VIEW_INITIAL
  );

  // Load file content + diff when a file is selected: one dispatch replaces
  // the whole view, so the editor text and the diff can never disagree about
  // which file they belong to.
  useEffect(() => {
    if (!selectedFile) {
      dispatchFileView({ type: "clear" });
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        // 保留已知缺陷（原 App.tsx:280-312）：串行双加载——get_file_contents 与
        // get_diff 逐个 await，两段网络往返串行叠加。
        const fc = await invoke<FileContent>("get_file_contents", {
          repoPath,
          path: selectedFile,
        });
        const d = await invoke<FileDiff>("get_diff", {
          repoPath,
          path: selectedFile,
        });
        if (!cancelled) {
          dispatchFileView({
            type: "loaded",
            diff: d,
            working: fc.working,
            conflicted: fc.conflicted,
            isBinary: fc.isBinary,
          });
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selectedFile, repoPath]);

  /**
   * Re-runs the same two invokes + one dispatch the loading effect uses. Used
   * by the hunk-level staging actions (task ②) to refresh the diff after a
   * stage/unstage — deliberately the existing load path, not a new one.
   */
  async function loadFileView(path: string) {
    const fc = await invoke<FileContent>("get_file_contents", {
      repoPath,
      path,
    });
    const d = await invoke<FileDiff>("get_diff", { repoPath, path });
    dispatchFileView({
      type: "loaded",
      diff: d,
      working: fc.working,
      conflicted: fc.conflicted,
      isBinary: fc.isBinary,
    });
  }

  async function saveEdit() {
    if (!selectedFile) return;
    setError("");
    try {
      await invoke("write_file", {
        repoPath,
        path: selectedFile,
        content: fileView.editText,
      });
      setInfo("File saved");
      // 保留已知缺陷（原 App.tsx:~297 关联，saveEdit 376-390）：保存后只 refresh()
      // 状态快照，不重新拉取文件内容与 diff（加载 effect 仅依赖 [selectedFile, repoPath]）。
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  const diffView = useMemo(() => diffRowsToJsx(fileView.diff), [fileView.diff]);

  return { fileView, dispatchFileView, saveEdit, diffView, loadFileView };
}
