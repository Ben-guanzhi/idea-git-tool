// Working-tree snapshot cluster: repo path, status/conflicts/branches/stashes/
// tags, the shared refresh bookkeeping, checked-path pruning and the info/error
// banners. Split verbatim from App.tsx (states 47-51, 58-61, 65-68, 75, 87;
// helpers 148-170; refresh 182-233; effects 235-255). The info/error banners
// live here only so the four effects keep their original relative order
// (refresh, persist, info-dismiss, error-dismiss) — timing-preserving merge.

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  REPO_KEY,
  type BranchInfo,
  type ConflictInfo,
  type StashInfo,
  type StatusInfo,
  type TagInfo,
} from "../types";

export function useRepoStatus() {
  const [repoPath, setRepoPath] = useState(
    () => localStorage.getItem(REPO_KEY) ?? ""
  );
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [conflicts, setConflicts] = useState<ConflictInfo[]>([]);
  const [rebaseInProgress, setRebaseInProgress] = useState(false);
  const [stashes, setStashes] = useState<StashInfo[]>([]);
  const [tags, setTags] = useState<TagInfo[]>([]);
  const [stashSel, setStashSel] = useState<number | null>(null);
  const [tagSel, setTagSel] = useState("");
  const [info, setInfo] = useState("");
  const [error, setError] = useState("");
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [checkedPaths, setCheckedPaths] = useState<Set<string>>(new Set());

  function toggleFileChecked(path: string) {
    setCheckedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  function setDirChecked(dirPaths: string[], on: boolean) {
    setCheckedPaths((prev) => {
      const next = new Set(prev);
      for (const p of dirPaths) {
        if (on) next.add(p);
        else next.delete(p);
      }
      return next;
    });
  }

  function clearChecked() {
    setCheckedPaths(new Set());
  }

  const refreshSeq = useRef(0);

  const refresh = useCallback(async () => {
    setError("");
    const seq = ++refreshSeq.current;
    try {
      // The queries are independent, so fetch them in parallel; they used to
      // be awaited serially, each round-trip adding latency.
      const [s, c, b, st, tg, rip] = await Promise.all([
        invoke<StatusInfo>("get_status", { repoPath }),
        invoke<ConflictInfo[]>("get_conflicts", { repoPath }),
        invoke<BranchInfo[]>("list_branches", { repoPath }),
        invoke<StashInfo[]>("list_stashes", { repoPath }),
        invoke<TagInfo[]>("list_tags", { repoPath }),
        invoke<boolean>("rebase_in_progress", { repoPath }),
      ]);
      // A newer refresh may have started while this one was in flight (F5
      // pressed during a mutation, repoPath changed, etc.). Only the latest
      // snapshot may apply, or a stale one could overwrite a fresher view.
      if (seq !== refreshSeq.current) return;
      setStatus(s);
      // refresh is the single bookkeeper for checkbox validity: checked paths
      // that no longer exist in the tree are pruned here, right where the new
      // tree arrives. Stage/unstage uncheck their own paths on top of this.
      setCheckedPaths((prev) => {
        if (prev.size === 0) return prev;
        const valid = new Set([
          ...(s.staged ?? []).map((f) => f.path),
          ...(s.unstaged ?? []).map((f) => f.path),
        ]);
        const next = new Set([...prev].filter((p) => valid.has(p)));
        return next.size === prev.size ? prev : next;
      });
      setConflicts(c);
      setBranches(b);
      setStashes(st);
      setTags(tg);
      setRebaseInProgress(rip);
      // The new snapshots may have invalidated the current selections
      // (stash dropped, tag deleted by someone else): prune like checkedPaths.
      // 保留已知缺陷（原 App.tsx:222-224）：stale-seq 竞态吞结果——seq 检查不通过时
      // 整次快照（含这里的 stash/tag 剪枝）被静默丢弃，不做任何补偿。
      setStashSel((prev) =>
        prev !== null && !st.some((x) => x.index === prev) ? null : prev
      );
      setTagSel((prev) => (prev && !tg.some((t) => t.name === prev) ? "" : prev));
    } catch (e) {
      // A newer refresh may have started while this one was in flight; a stale
      // failure must not clobber the error of a fresher view (mirror of the
      // seq check on the success path above).
      if (seq !== refreshSeq.current) return;
      setError(String(e));
    }
  }, [repoPath]);

  useEffect(() => {
    if (repoPath.trim()) refresh();
  }, [repoPath, refresh]);

  useEffect(() => {
    if (!repoPath.trim()) return;
    localStorage.setItem(REPO_KEY, repoPath);
  }, [repoPath]);

  // Auto-dismiss banners.
  useEffect(() => {
    if (!info) return;
    const t = setTimeout(() => setInfo(""), 4000);
    return () => clearTimeout(t);
  }, [info]);

  useEffect(() => {
    if (!error) return;
    const t = setTimeout(() => setError(""), 8000);
    return () => clearTimeout(t);
  }, [error]);

  return {
    repoPath,
    setRepoPath,
    status,
    conflicts,
    branches,
    stashes,
    tags,
    rebaseInProgress,
    stashSel,
    setStashSel,
    tagSel,
    setTagSel,
    checkedPaths,
    setCheckedPaths,
    toggleFileChecked,
    setDirChecked,
    clearChecked,
    info,
    error,
    setInfo,
    setError,
    refresh,
  };
}
