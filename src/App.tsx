import {
  useReducer,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { ConflictEditor } from "./ConflictEditor";
import { diffRowsToJsx } from "./DiffView";
import { FileList } from "./FileTree";
import { LogPane } from "./LogPane";
import { OpsPanel } from "./OpsPanel";
import { TopBar } from "./TopBar";
import { BranchActions } from "./BranchActions";
import { CommitPanel } from "./CommitPanel";
import {
  OPS_KEY,
  REPO_KEY,
  type BranchInfo,
  type BusyKey,
  type CommitFile,
  type CommitSummary,
  type ConflictInfo,
  type FileContent,
  type FileDiff,
  type MergeResult,
  type OpLogEntry,
  type StatusInfo,
} from "./types";
import {
  fileViewReducer,
  FILE_VIEW_INITIAL,
  commitExplorerReducer,
  COMMIT_EXPLORER_INITIAL,
} from "./viewState";
import "./App.css";

function App() {
  const [repoPath, setRepoPath] = useState(
    () => localStorage.getItem(REPO_KEY) ?? ""
  );
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [conflicts, setConflicts] = useState<ConflictInfo[]>([]);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [tab, setTab] = useState<"changes" | "log">("changes");
  const [branch, setBranch] = useState("");
  const [mergeTarget, setMergeTarget] = useState("");
  const [remote, setRemote] = useState("origin");
  const [info, setInfo] = useState("");
  const [error, setError] = useState("");
  const [fileView, dispatchFileView] = useReducer(
    fileViewReducer,
    FILE_VIEW_INITIAL
  );
  const [logs, setLogs] = useState<CommitSummary[]>([]);
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [explorer, dispatchExplorer] = useReducer(
    commitExplorerReducer,
    COMMIT_EXPLORER_INITIAL
  );
  const [busy, setBusy] = useState<BusyKey>(null);
  const [ops, setOps] = useState<OpLogEntry[]>([]);
  const [opsOpen, setOpsOpen] = useState(
    () => localStorage.getItem(OPS_KEY) === "1"
  );
  const [collapsedDirs, setCollapsedDirs] = useState<Set<string>>(new Set());
  const [logCollapsed, setLogCollapsed] = useState<Set<string>>(new Set());
  const [checkedPaths, setCheckedPaths] = useState<Set<string>>(new Set());
  const opsIdRef = useRef(1);
  const branchInputRef = useRef<HTMLInputElement>(null);

  function logOp(op: string, status: OpLogEntry["status"], detail: string) {
    const entry: OpLogEntry = {
      id: opsIdRef.current++,
      time: new Date().toLocaleTimeString(),
      op,
      status,
      detail,
    };
    setOps((prev) => [entry, ...prev].slice(0, 200));
    if (status === "error" && !opsOpen) {
      setOpsOpen(true);
      localStorage.setItem(OPS_KEY, "1");
    }
  }

  function toggleOps() {
    setOpsOpen((open) => {
      const next = !open;
      localStorage.setItem(OPS_KEY, next ? "1" : "0");
      return next;
    });
  }

  function exportOps() {
    if (ops.length === 0) {
      logOp("export", "info", "nothing to export");
      return;
    }
    const header =
      `# idea-git-tool operations\n` +
      `# repo: ${repoPath || "(none)"}\n` +
      `# exported: ${new Date().toISOString()}\n` +
      `# entries: ${ops.length}\n` +
      `# time\tstatus\top\tdetail\n`;
    // ops is newest-first; reverse for chronological file.
    const lines = [...ops]
      .reverse()
      .map((o) => `${o.time}\t${o.status}\t${o.op}\t${o.detail.replace(/\t/g, " ").replace(/\n/g, " ")}`)
      .join("\n");
    const blob = new Blob([header + lines + "\n"], {
      type: "text/plain;charset=utf-8",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    const stamp = new Date()
      .toISOString()
      .replace(/[:.]/g, "-")
      .slice(0, 19);
    a.download = `git-ops-${stamp}.log`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
    logOp("export", "ok", `exported ${ops.length} entries`);
  }

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

  function toggleLogDir(path: string) {
    // `path` is already `log:<dirPath>` from FileTreeView.
    setLogCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  const refresh = useCallback(async () => {
    setError("");
    try {
      // The three status queries are independent, so fetch them in parallel;
      // they used to be awaited serially, each round-trip adding latency.
      const [s, c, b] = await Promise.all([
        invoke<StatusInfo>("get_status", { repoPath }),
        invoke<ConflictInfo[]>("get_conflicts", { repoPath }),
        invoke<BranchInfo[]>("list_branches", { repoPath }),
      ]);
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
    } catch (e) {
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

  const selectedConflict = useMemo(
    () => conflicts.find((c) => c.path === selectedFile) ?? null,
    [conflicts, selectedFile]
  );

  const mergeCandidates = useMemo(
    () => branches.filter((b) => !b.isCurrent),
    [branches]
  );
  const currentBranchName = status?.branch ?? "";

  async function loadLog() {
    setError("");
    try {
      const r = await invoke<CommitSummary[]>("get_log", {
        repoPath,
        limit: 50,
      });
      setLogs(r);
    } catch (e) {
      setError(String(e));
    }
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
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  /**
   * The one channel for every mutating operation: takes the busy lock so
   * operations cannot interleave (a stage racing a push would corrupt the
   * index view), logs the outcome, surfaces success as a banner and failure
   * as an error, and refreshes the working-tree snapshot (plus the log when
   * one is loaded or visible). `fn` runs the invokes; `onSuccess` may return
   * the banner text, defaulting to the invoke result or `detail`.
   */
  async function runOp(opts: {
    key: Exclude<BusyKey, null>;
    op: string;
    detail: string;
    onSuccess?: (result: string) => string | void;
    fn: () => Promise<string>;
  }) {
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

  async function doPush() {
    await runOp({
      key: "push",
      op: "push",
      detail: `→ ${remote}`,
      fn: async () => invoke<string>("push", { repoPath, remote }),
    });
  }

  async function doPull() {
    await runOp({
      key: "pull",
      op: "pull",
      detail: `← ${remote}`,
      fn: async () => invoke<string>("pull", { repoPath, remote }),
    });
  }

  async function doFetch() {
    await runOp({
      key: "fetch",
      op: "fetch",
      detail: remote,
      fn: async () => invoke<string>("fetch", { repoPath, remote }),
    });
  }

  async function doMerge() {
    setError("");
    setInfo("");
    const target = mergeTarget.trim();
    if (!target) {
      setError("请先选择要合并的分支");
      return;
    }
    await runOp({
      key: "merge",
      op: "merge",
      detail: `merge ${target}`,
      onSuccess: () => setMergeTarget(""),
      fn: async () => {
        const r = await invoke<MergeResult>("merge", { repoPath, branch: target });
        if (r.status === "conflicts") {
          setTab("changes");
          if (r.conflicts.length > 0) {
            setSelectedFile(r.conflicts[0]);
          }
          // The merge ran and produced conflicts: a business outcome, not an
          // exception — surface it through the error banner as-is.
          throw r.message;
        }
        return r.message;
      },
    });
  }

  async function doAbortMerge() {
    await runOp({
      key: "abort",
      op: "abort merge",
      detail: "",
      onSuccess: () => setSelectedFile(null),
      fn: async () => invoke<string>("abort_merge", { repoPath }),
    });
  }

  async function doCheckout(name: string) {
    if (!name) return;
    await runOp({
      key: "checkout",
      op: "checkout",
      detail: `→ ${name}`,
      onSuccess: () => setSelectedFile(null),
      fn: async () => invoke<string>("checkout", { repoPath, branch: name }),
    });
  }

  async function doCreateBranch() {
    if (!branch.trim()) {
      setError("请输入分支名称 (Branch name is required)");
      branchInputRef.current?.focus();
      return;
    }
    await runOp({
      key: "branch",
      op: "branch",
      detail: `create ${branch.trim()}`,
      onSuccess: () => setBranch(""),
      fn: async () => invoke<string>("create_branch", { repoPath, name: branch.trim() }),
    });
  }

  async function doResolve(path: string, choice: string) {
    await runOp({
      key: "resolve",
      op: "resolve",
      detail: `${path} (${choice})`,
      fn: async () => {
        await invoke("resolve_conflict", { repoPath, paths: [path], choice });
        return "";
      },
    });
  }

  function toggleDir(path: string) {
    setCollapsedDirs((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  const diffView = useMemo(() => diffRowsToJsx(fileView.diff), [fileView.diff]);

  const commitDiffView = useMemo(
    () => diffRowsToJsx(explorer.diff),
    [explorer.diff]
  );

  return (
    <div className="ide">
      <TopBar
        repoPath={repoPath}
        onRepoPath={setRepoPath}
        status={status}
        branches={branches}
        busy={busy}
        branch={branch}
        branchInputRef={branchInputRef}
        onCheckout={doCheckout}
        onFetch={doFetch}
        onPull={doPull}
        onPush={doPush}
        onCreateBranch={doCreateBranch}
      />

      {error && <div className="ide-error">{error}</div>}
      {info && <div className="ide-info">{info}</div>}

      <main className="ide-main">
        <div className="ide-tabs">
          <button
            className={tab === "changes" ? "tab active" : "tab"}
            onClick={() => setTab("changes")}
          >
            Changes
          </button>
          <button
            className={tab === "log" ? "tab active" : "tab"}
            onClick={() => {
              setTab("log");
              void loadLog();
            }}
            title="Reload history (F5)"
          >
            Log
          </button>
        </div>

        {tab === "changes" && (
          <div className="changes-pane">
            <div className="changes-body">
              <aside className="changes-tree">
              <FileList
                title="Staged"
                files={status?.staged ?? []}
                selected={selectedFile}
                onSelect={setSelectedFile}
                bulkLabel={
                  stagedChecked.length > 0
                    ? `Unstage (${stagedChecked.length})`
                    : "Unstage all"
                }
                onBulk={() =>
                  doUnstage(
                    stagedChecked.length > 0 ? stagedChecked : stagedAll
                  )
                }
                actionLabel="Unstage"
                actionIcon="↑"
                onAction={(p) => doUnstage([p])}
                collapsed={collapsedDirs}
                onToggleDir={toggleDir}
                checked={checkedPaths}
                onToggleFile={toggleFileChecked}
                onSetDir={setDirChecked}
                onClearChecked={clearChecked}
              />
              <FileList
                title="Unstaged"
                files={status?.unstaged ?? []}
                selected={selectedFile}
                onSelect={setSelectedFile}
                bulkLabel={
                  unstagedChecked.length > 0
                    ? `Stage (${unstagedChecked.length})`
                    : "Stage all"
                }
                onBulk={() =>
                  doStage(
                    unstagedChecked.length > 0 ? unstagedChecked : unstagedAll
                  )
                }
                actionLabel="Stage"
                actionIcon="↓"
                onAction={(p) => doStage([p])}
                onDiscard={(p) => doDiscard([p])}
                collapsed={collapsedDirs}
                onToggleDir={toggleDir}
                checked={checkedPaths}
                onToggleFile={toggleFileChecked}
                onSetDir={setDirChecked}
                onClearChecked={clearChecked}
              />
            </aside>

            <div className="diff-area">
              {selectedConflict ? (
                <ConflictEditor
                  conflict={selectedConflict}
                  repoPath={repoPath}
                  onResolve={doResolve}
                  onSaved={refresh}
                  onError={setError}
                />
              ) : selectedFile ? (
                <div className="file-diff">
                  <div className="diff-toolbar">
                    <span className="diff-title">{selectedFile}</span>
                    {fileView.isBinary ? (
                      <span className="muted">Binary file — editing disabled</span>
                    ) : (
                      <>
                        {fileView.editing && (
                          <button className="btn" onClick={saveEdit}>
                            Save
                          </button>
                        )}
                        <button
                          className="btn"
                          onClick={() => dispatchFileView({ type: "toggleEditing" })}
                        >
                          {fileView.editing ? "Done editing" : "Edit"}
                        </button>
                      </>
                    )}
                  </div>
                  {fileView.editing && !fileView.isBinary ? (
                    <textarea
                      className="edit-area"
                      value={fileView.editText}
                      onChange={(e) =>
                        dispatchFileView({
                          type: "edit",
                          text: e.currentTarget.value,
                        })
                      }
                    />
                  ) : (
                    <div className="diff-lines">{diffView ?? <span className="muted">Loading...</span>}</div>
                  )}
                </div>
              ) : (
                <div className="empty-pane">Select a file to view its diff.</div>
              )}
            </div>
            </div>

            <CommitPanel
              message={message}
              onMessage={setMessage}
              busy={busy}
              canCommit={canCommit}
              stagedCount={stagedCount}
              unstagedCount={unstagedCount}
              hasChanges={hasChanges}
              onCommit={doCommit}
            />
          </div>
        )}

        {tab === "log" && (
          <LogPane
            logs={logs}
            selectedCommit={explorer.commit}
            files={explorer.files}
            selectedFile={explorer.file}
            diffView={commitDiffView}
            collapsed={logCollapsed}
            onToggleDir={toggleLogDir}
            onSelectCommit={selectCommit}
            onSelectFile={(p) => {
              if (explorer.commit) void selectCommitFile(explorer.commit, p);
            }}
          />
        )}
      </main>

      <OpsPanel
        ops={ops}
        open={opsOpen}
        onToggle={toggleOps}
        onExport={exportOps}
        onClear={() => setOps([])}
      />

      <BranchActions
        repoPath={repoPath}
        busy={busy}
        conflictsCount={conflicts.length}
        branch={branch}
        onBranch={setBranch}
        branchInputRef={branchInputRef}
        mergeTarget={mergeTarget}
        onMergeTarget={setMergeTarget}
        mergeCandidates={mergeCandidates}
        branchCount={branches.length}
        currentBranchName={currentBranchName}
        remote={remote}
        onRemote={setRemote}
        onMerge={doMerge}
        onAbortMerge={doAbortMerge}
        onCreateBranch={doCreateBranch}
        onRefresh={() => {
          if (!busy) void refresh();
        }}
      />
    </div>
  );
}

export default App;
