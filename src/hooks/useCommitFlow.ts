// Stage/commit flow + the shared runOp mutation channel + keyboard shortcuts:
// split verbatim from App.tsx (message 53, busy 80, actionsRef 257-258,
// shortcut effect 261-275, runOp 400-426, counts 428-431, checked-path memos
// 433-448, doStage 450-468, doUnstage 470-488, doDiscard 490-511,
// doCommit 513-543).

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  BusyKey,
  OpLogEntry,
  StatusInfo,
} from "../types";

export interface RunOpOptions {
  key: Exclude<BusyKey, null>;
  op: string;
  detail: string;
  onSuccess?: (result: string) => string | void;
  fn: () => Promise<string>;
}

export type RunOp = (opts: RunOpOptions) => Promise<void>;

export function useCommitFlow(params: {
  repoPath: string;
  status: StatusInfo | null;
  refresh: () => Promise<void>;
  loadLog: (offset?: number) => Promise<void>;
  logs: unknown[];
  tab: "changes" | "log";
  logOp: (op: string, status: OpLogEntry["status"], detail: string) => void;
  setError: (message: string) => void;
  setInfo: (message: string) => void;
  checkedPaths: Set<string>;
  setCheckedPaths: Dispatch<SetStateAction<Set<string>>>;
  selectedFile: string | null;
  setSelectedFile: Dispatch<SetStateAction<string | null>>;
}) {
  const {
    repoPath,
    status,
    refresh,
    loadLog,
    logs,
    tab,
    logOp,
    setError,
    setInfo,
    checkedPaths,
    setCheckedPaths,
    selectedFile,
    setSelectedFile,
  } = params;

  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState<BusyKey>(null);

  const actionsRef = useRef({ doCommit, refresh, loadLog, tab, busy, message });
  actionsRef.current = { doCommit, refresh, loadLog, tab, busy, message };

  // Shortcuts: Ctrl/Cmd+Enter commit, F5 refresh.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const a = actionsRef.current;
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        e.preventDefault();
        if (a.tab === "changes" && !a.busy) void a.doCommit();
      } else if (e.key === "F5") {
        e.preventDefault();
        void a.refresh();
        if (a.tab === "log") void a.loadLog();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  /**
   * The one channel for every mutating operation: takes the busy lock so
   * operations cannot interleave (a stage racing a push would corrupt the
   * index view), logs the outcome, surfaces success as a banner and failure
   * as an error, and refreshes the working-tree snapshot (plus the log when
   * one is loaded or visible). `fn` runs the invokes; `onSuccess` may return
   * the banner text, defaulting to the invoke result or `detail`.
   */
  async function runOp(opts: RunOpOptions) {
    if (busy) return;
    setBusy(opts.key);
    setError("");
    setInfo("");
    try {
      const r = await opts.fn();
      const detail = opts.onSuccess?.(r) ?? (r || opts.detail);
      logOp(opts.op, "ok", detail);
      setInfo(detail);
      await refresh();
      if (logs.length > 0 || tab === "log") await loadLog();
    } catch (e) {
      const msg = String(e);
      logOp(opts.op, "error", msg);
      setError(msg);
      await refresh();
    } finally {
      setBusy(null);
    }
  }

  const stagedCount = status?.staged.length ?? 0;
  const unstagedCount = status?.unstaged.length ?? 0;
  const hasChanges = stagedCount > 0 || unstagedCount > 0;
  const canCommit = !!message.trim() && hasChanges && !busy;

  const stagedAll = useMemo(
    () => (status?.staged ?? []).map((f) => f.path),
    [status]
  );
  const unstagedAll = useMemo(
    () => (status?.unstaged ?? []).map((f) => f.path),
    [status]
  );
  const stagedChecked = useMemo(
    () => stagedAll.filter((p) => checkedPaths.has(p)),
    [stagedAll, checkedPaths]
  );
  const unstagedChecked = useMemo(
    () => unstagedAll.filter((p) => checkedPaths.has(p)),
    [unstagedAll, checkedPaths]
  );

  async function doStage(paths: string[]) {
    if (paths.length === 0) return;
    await runOp({
      key: "stage",
      op: "stage",
      detail: `${paths.length} file(s)`,
      onSuccess: () => {
        setCheckedPaths((prev) => {
          const n = new Set(prev);
          for (const p of paths) n.delete(p);
          return n;
        });
      },
      fn: async () => {
        await invoke("stage", { repoPath, paths });
        return "";
      },
    });
  }

  async function doUnstage(paths: string[]) {
    if (paths.length === 0) return;
    await runOp({
      key: "unstage",
      op: "unstage",
      detail: `${paths.length} file(s)`,
      onSuccess: () => {
        setCheckedPaths((prev) => {
          const n = new Set(prev);
          for (const p of paths) n.delete(p);
          return n;
        });
      },
      fn: async () => {
        await invoke("unstage", { repoPath, paths });
        return "";
      },
    });
  }

  async function doDiscard(paths: string[]) {
    if (paths.length === 0) return;
    const label =
      paths.length === 1 ? paths[0] : `${paths.length} files`;
    if (!window.confirm(`Discard changes for ${label}? This cannot be undone.`)) {
      logOp("discard", "info", `cancelled: ${label}`);
      return;
    }
    await runOp({
      key: "discard",
      op: "discard",
      detail: label,
      onSuccess: () => {
        if (selectedFile && paths.includes(selectedFile)) setSelectedFile(null);
        return `Discarded ${label}`;
      },
      fn: async () => {
        await invoke("discard", { repoPath, paths });
        return "";
      },
    });
  }

  async function doCommit() {
    if (!message.trim()) {
      setError("请输入提交信息 (Commit message is required)");
      return;
    }
    if (stagedCount === 0 && unstagedCount === 0) {
      setError("没有可提交的更改 (nothing to commit)");
      return;
    }
    const subject = message.trim().split("\n")[0];
    await runOp({
      key: "commit",
      op: "commit",
      detail: subject,
      onSuccess: () => setMessage(""),
      fn: async () => {
        // Nothing staged yet: stage all unstaged changes first (Stage & Commit).
        // 保留已知缺陷（原 App.tsx:530-540）：自动 stage 与 commit 非原子——stage 成功后
        // commit 失败会留下已暂存的状态，两步之间可能被外部改动插入。
        if (stagedCount === 0 && unstagedCount > 0) {
          await invoke("stage", {
            repoPath,
            paths: (status?.unstaged ?? []).map((f) => f.path),
          });
          logOp("stage", "ok", `auto (stage & commit): ${unstagedCount} file(s)`);
        }
        const r = await invoke<string>("commit", {
          args: { repoPath, message, authorName: "", authorEmail: "" },
        });
        return `Committed ${r.slice(0, 12)} — ${subject}`;
      },
    });
  }

  return {
    message,
    setMessage,
    busy,
    runOp,
    stagedCount,
    unstagedCount,
    hasChanges,
    canCommit,
    stagedAll,
    unstagedAll,
    stagedChecked,
    unstagedChecked,
    doStage,
    doUnstage,
    doDiscard,
    doCommit,
  };
}
