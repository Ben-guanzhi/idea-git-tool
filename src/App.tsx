import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

interface ConflictInfo {
  path: string;
  ours: string;
  theirs: string;
  base: string;
}

interface FileStatus {
  path: string;
  status: string;
}

interface StatusInfo {
  branch: string;
  upstream?: string | null;
  ahead: number;
  behind: number;
  staged: FileStatus[];
  unstaged: FileStatus[];
}

interface FileContent {
  path: string;
  working: string;
  head: string;
  staged: string;
  tracked: boolean;
  conflicted: boolean;
}

interface Highlight {
  start: number;
  end: number;
}

interface DiffSegment {
  text: string;
  highlights: Highlight[];
}

interface DiffRow {
  kind: string;
  left: DiffSegment | null;
  right: DiffSegment | null;
}

interface FileDiff {
  path: string;
  rows: DiffRow[];
}

interface CommitSummary {
  id: string;
  shortId: string;
  message: string;
  author: string;
  email: string;
  time: string;
  parents: string[];
  refs: string[];
}

interface BranchInfo {
  name: string;
  isCurrent: boolean;
  refname: string;
}

interface MergeResult {
  status: string;
  message: string;
  conflicts: string[];
}

interface CommitFile {
  path: string;
  status: string;
  newPath: string;
}

const STATUS_ICON: Record<string, string> = {
  modified: "🖊",
  added: "+",
  deleted: "×",
  renamed: "→",
  conflicted: "⚠",
};

const REPO_KEY = "idea-git-tool:repoPath";
const OPS_KEY = "idea-git-tool:opsPanelOpen";

type BusyKey = "commit" | "push" | "pull" | "fetch" | "merge" | "abort" | "stage" | null;

interface OpLogEntry {
  id: number;
  time: string;
  op: string;
  status: "ok" | "error" | "info";
  detail: string;
}

interface TreeNode {
  name: string;
  path: string;
  dir: boolean;
  status?: string;
  children?: TreeNode[];
}

function buildTree(files: FileStatus[]): TreeNode[] {
  const root: TreeNode = { name: "", path: "", dir: true, children: [] };
  for (const f of files) {
    const parts = f.path.split(/[\\/]/);
    let node = root;
    for (let i = 0; i < parts.length; i++) {
      const name = parts[i];
      const isLeaf = i === parts.length - 1;
      const path = parts.slice(0, i + 1).join("/");
      if (!node.children) node.children = [];
      let child = node.children.find(
        (c) => c.name === name && c.dir === !isLeaf
      );
      if (!child) {
        child = isLeaf
          ? { name, path, dir: false, status: f.status }
          : { name, path, dir: true, children: [] };
        node.children.push(child);
      } else if (isLeaf) {
        child.status = f.status;
      }
      node = child;
    }
  }
  const sortRec = (nodes: TreeNode[]) => {
    nodes.sort((a, b) => {
      if (a.dir !== b.dir) return a.dir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    nodes.forEach((n) => n.children && sortRec(n.children));
  };
  if (root.children) sortRec(root.children);
  return root.children ?? [];
}

function collectLeafPaths(nodes: TreeNode[], out: string[] = []): string[] {
  for (const n of nodes) {
    if (n.dir) collectLeafPaths(n.children ?? [], out);
    else out.push(n.path);
  }
  return out;
}

function checkState(
  paths: string[],
  checked: Set<string>
): "on" | "off" | "partial" {
  if (paths.length === 0) return "off";
  let n = 0;
  for (const p of paths) if (checked.has(p)) n++;
  if (n === 0) return "off";
  if (n === paths.length) return "on";
  return "partial";
}

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
  const [diff, setDiff] = useState<FileDiff | null>(null);
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState("");
  const [logs, setLogs] = useState<CommitSummary[]>([]);
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [selectedCommit, setSelectedCommit] = useState<string | null>(null);
  const [commitFiles, setCommitFiles] = useState<CommitFile[]>([]);
  const [commitFile, setCommitFile] = useState<string | null>(null);
  const [commitDiff, setCommitDiff] = useState<FileDiff | null>(null);
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
      const s = await invoke<StatusInfo>("get_status", { repoPath });
      setStatus(s);
      const c = await invoke<ConflictInfo[]>("get_conflicts", { repoPath });
      setConflicts(c);
      const b = await invoke<BranchInfo[]>("list_branches", { repoPath });
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

  // Load file content + diff when a file is selected.
  useEffect(() => {
    if (!selectedFile) {
      setDiff(null);
      setEditing(false);
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
          setDiff(d);
          setEditText(fc.working);
          setEditing(fc.conflicted);
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
    setSelectedCommit(id);
    setCommitFile(null);
    setCommitDiff(null);
    setError("");
    try {
      const files = await invoke<CommitFile[]>("get_commit_files", {
        repoPath,
        commitId: id,
      });
      setCommitFiles(files);
      if (files.length > 0) {
        await selectCommitFile(id, files[0].path);
      }
    } catch (e) {
      setError(String(e));
      setCommitFiles([]);
    }
  }

  async function selectCommitFile(id: string, path: string) {
    setCommitFile(path);
    setCommitDiff(null);
    try {
      const d = await invoke<FileDiff>("get_commit_file_diff", {
        repoPath,
        commitId: id,
        path,
      });
      setCommitDiff(d);
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
        content: editText,
      });
      setInfo("File saved");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function withBusy(
    key: Exclude<BusyKey, null>,
    op: string,
    detail: string,
    fn: () => Promise<string>
  ) {
    if (busy) return;
    setBusy(key);
    setError("");
    setInfo("");
    try {
      const r = await fn();
      logOp(op, "ok", r || detail);
      setInfo(r || detail);
    } catch (e) {
      const msg = String(e);
      logOp(op, "error", msg);
      setError(msg);
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

  // Drop checked paths that no longer exist after refresh.
  useEffect(() => {
    if (checkedPaths.size === 0) return;
    const valid = new Set([...stagedAll, ...unstagedAll]);
    let dirty = false;
    const next = new Set<string>();
    for (const p of checkedPaths) {
      if (valid.has(p)) next.add(p);
      else dirty = true;
    }
    if (dirty) setCheckedPaths(next);
  }, [stagedAll, unstagedAll, checkedPaths]);

  async function doStage(paths: string[]) {
    if (paths.length === 0) return;
    try {
      await invoke("stage", { repoPath, paths });
      logOp("stage", "ok", `${paths.length} file(s)`);
      setCheckedPaths((prev) => {
        const n = new Set(prev);
        for (const p of paths) n.delete(p);
        return n;
      });
      await refresh();
    } catch (e) {
      logOp("stage", "error", String(e));
      setError(String(e));
    }
  }

  async function doUnstage(paths: string[]) {
    if (paths.length === 0) return;
    try {
      await invoke("unstage", { repoPath, paths });
      logOp("unstage", "ok", `${paths.length} file(s)`);
      setCheckedPaths((prev) => {
        const n = new Set(prev);
        for (const p of paths) n.delete(p);
        return n;
      });
      await refresh();
    } catch (e) {
      logOp("unstage", "error", String(e));
      setError(String(e));
    }
  }

  async function doDiscard(paths: string[]) {
    if (paths.length === 0) return;
    const label =
      paths.length === 1 ? paths[0] : `${paths.length} files`;
    if (!window.confirm(`Discard changes for ${label}? This cannot be undone.`)) {
      logOp("discard", "info", `cancelled: ${label}`);
      return;
    }
    try {
      await invoke("discard", { repoPath, paths });
      logOp("discard", "ok", label);
      setInfo(`Discarded ${label}`);
      if (selectedFile && paths.includes(selectedFile)) setSelectedFile(null);
      await refresh();
    } catch (e) {
      logOp("discard", "error", String(e));
      setError(String(e));
    }
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
    await withBusy("commit", "commit", subject, async () => {
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
      setMessage("");
      await refresh();
      if (logs.length > 0 || tab === "log") await loadLog();
      return `Committed ${r.slice(0, 12)} — ${subject}`;
    });
  }

  async function doPush() {
    await withBusy("push", "push", `→ ${remote}`, async () => {
      const r = await invoke<string>("push", { repoPath, remote });
      await refresh();
      if (logs.length > 0 || tab === "log") await loadLog();
      return r;
    });
  }

  async function doPull() {
    await withBusy("pull", "pull", `← ${remote}`, async () => {
      const r = await invoke<string>("pull", { repoPath, remote });
      await refresh();
      if (logs.length > 0 || tab === "log") await loadLog();
      return r;
    });
  }

  async function doFetch() {
    await withBusy("fetch", "fetch", remote, async () => {
      const r = await invoke<string>("fetch", { repoPath, remote });
      await refresh();
      return r;
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
    await withBusy("merge", "merge", `merge ${target}`, async () => {
      const r = await invoke<MergeResult>("merge", { repoPath, branch: target });
      if (r.status === "conflicts") {
        setTab("changes");
        await refresh();
        if (r.conflicts.length > 0) {
          setSelectedFile(r.conflicts[0]);
        }
        return Promise.reject(r.message);
      }
      setMergeTarget("");
      await refresh();
      if (logs.length > 0 || tab === "log") await loadLog();
      return r.message;
    });
  }

  async function doAbortMerge() {
    await withBusy("abort", "abort merge", "", async () => {
      const r = await invoke<string>("abort_merge", { repoPath });
      setSelectedFile(null);
      await refresh();
      return r;
    });
  }

  async function doCheckout(name: string) {
    if (!name || busy) return;
    setError("");
    setInfo("");
    try {
      const r = await invoke<string>("checkout", { repoPath, branch: name });
      logOp("checkout", "ok", `→ ${name}`);
      setInfo(r);
      setSelectedFile(null);
      await refresh();
      if (logs.length > 0) await loadLog();
    } catch (e) {
      logOp("checkout", "error", String(e));
      setError(String(e));
      await refresh();
    }
  }

  async function doCreateBranch() {
    setError("");
    if (!branch.trim()) {
      setError("请输入分支名称 (Branch name is required)");
      branchInputRef.current?.focus();
      return;
    }
    try {
      const r = await invoke<string>("create_branch", {
        repoPath,
        name: branch.trim(),
      });
      logOp("branch", "ok", `create ${branch.trim()}`);
      setInfo(r);
      setBranch("");
      await refresh();
      if (logs.length > 0) await loadLog();
    } catch (e) {
      logOp("branch", "error", String(e));
      setError(String(e));
    }
  }

  async function doResolve(path: string, choice: string) {
    setError("");
    try {
      await invoke("resolve_conflict", { repoPath, paths: [path], choice });
      logOp("resolve", "ok", `${path} (${choice})`);
      await refresh();
    } catch (e) {
      logOp("resolve", "error", String(e));
      setError(String(e));
    }
  }

  function toggleDir(path: string) {
    setCollapsedDirs((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  const diffView = useMemo(() => {
    if (!diff) return null;
    return diff.rows.map((row, i) => (
      <div key={i} className={`diff-row diff-${row.kind}`}>
        <div className="diff-cell">
          {row.left ? <SegView seg={row.left} /> : <span>&nbsp;</span>}
        </div>
        <div className="diff-cell">
          {row.right ? <SegView seg={row.right} /> : <span>&nbsp;</span>}
        </div>
      </div>
    ));
  }, [diff]);

  const commitDiffView = useMemo(() => {
    if (!commitDiff) return null;
    return commitDiff.rows.map((row, i) => (
      <div key={i} className={`diff-row diff-${row.kind}`}>
        <div className="diff-cell">
          {row.left ? <SegView seg={row.left} /> : <span>&nbsp;</span>}
        </div>
        <div className="diff-cell">
          {row.right ? <SegView seg={row.right} /> : <span>&nbsp;</span>}
        </div>
      </div>
    ));
  }, [commitDiff]);

  return (
    <div className="ide">
      <header className="ide-topbar">
        <input
          className="repo-input"
          placeholder="Repository path..."
          value={repoPath}
          onChange={(e) => setRepoPath(e.currentTarget.value)}
        />
        <select
          className="branch-select"
          value={status?.branch ?? ""}
          onChange={(e) => doCheckout(e.currentTarget.value)}
          title="Switch branch"
        >
          {branches.map((b) => (
            <option key={b.refname} value={b.name}>
              {b.name}
              {b.isCurrent ? " *" : ""}
            </option>
          ))}
        </select>
        {status?.upstream && (
          <span className="upstream">
            {status.ahead > 0 && `↑${status.ahead} `}
            {status.behind > 0 && `↓${status.behind} `}
            {status.ahead === 0 && status.behind === 0 && "up to date"}
          </span>
        )}
        <button
          className="btn"
          onClick={doFetch}
          disabled={!!busy || !repoPath.trim()}
          title="Fetch from remote (F5 also refreshes local status)"
        >
          {busy === "fetch" ? "Fetching…" : "Fetch"}
        </button>
        <button
          className="btn"
          onClick={doPull}
          disabled={!!busy || !repoPath.trim()}
          title="Pull from remote"
        >
          {busy === "pull" ? "Pulling…" : "Pull"}
        </button>
        <button
          className="btn primary"
          onClick={doPush}
          disabled={!!busy || !repoPath.trim()}
          title="Push to remote (sets upstream on first push)"
        >
          {busy === "push" ? "Pushing…" : "Push"}
        </button>
        <button
          className="btn"
          onClick={() => {
            if (!branch.trim()) {
              branchInputRef.current?.focus();
            } else {
              void doCreateBranch();
            }
          }}
          disabled={!repoPath.trim()}
          title="Create a new branch (focuses the name field if empty)"
        >
          New Branch
        </button>
      </header>

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
                />
              ) : selectedFile ? (
                <div className="file-diff">
                  <div className="diff-toolbar">
                    <span className="diff-title">{selectedFile}</span>
                    {editing && (
                      <button className="btn" onClick={saveEdit}>
                        Save
                      </button>
                    )}
                    <button className="btn" onClick={() => setEditing(!editing)}>
                      {editing ? "Done editing" : "Edit"}
                    </button>
                  </div>
                  {editing ? (
                    <textarea
                      className="edit-area"
                      value={editText}
                      onChange={(e) => setEditText(e.currentTarget.value)}
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

            <div className="commit-panel">
              <textarea
                className="commit-msg"
                placeholder="Commit message (first line = subject, rest = body)…  Ctrl+Enter to commit"
                value={message}
                onChange={(e) => setMessage(e.currentTarget.value)}
              />
              <div className="commit-side">
                <button
                  className="commit-btn"
                  onClick={doCommit}
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
          </div>
        )}

        {tab === "log" && (
          <div className="log-pane">
            <div className="log-body">
              <div className="log-list">
                {logs.map((c) => (
                  <div
                    key={c.id}
                    className={
                      selectedCommit === c.id
                        ? "log-item selected"
                        : "log-item"
                    }
                    onClick={() => selectCommit(c.id)}
                  >
                    <div className="log-refs">
                      {c.refs.map((r) => (
                        <span key={r} className="log-ref">
                          {r}
                        </span>
                      ))}
                    </div>
                    <div className="log-msg">{c.message.split("\n")[0]}</div>
                    <div className="log-meta">
                      {c.author} ·{" "}
                      {new Date(Number(c.time) * 1000).toLocaleString()} ·{" "}
                      {c.shortId}
                    </div>
                  </div>
                ))}
                {logs.length === 0 && (
                  <div className="muted">No commits yet.</div>
                )}
              </div>

              <aside className="commit-files">
                <div className="file-group-header">
                  <span className="file-group-title">
                    Files ({commitFiles.length})
                  </span>
                </div>
                {selectedCommit && commitFiles.length === 0 && (
                  <div className="file-empty">No files</div>
                )}
                {!selectedCommit && (
                  <div className="file-empty">Select a commit</div>
                )}
                {commitFiles.length > 0 && (
                  <FileTreeView
                    files={commitFiles.map((f) => ({
                      path: f.path,
                      status: f.status,
                    }))}
                    titleKey="log"
                    collapsed={logCollapsed}
                    onToggleDir={toggleLogDir}
                    selected={commitFile}
                    onSelect={(p) =>
                      selectedCommit && selectCommitFile(selectedCommit, p)
                    }
                  />
                )}
              </aside>

              <div className="diff-area">
                {commitFile ? (
                  <div className="file-diff">
                    <div className="diff-toolbar">
                      <span className="diff-title">{commitFile}</span>
                    </div>
                    <div className="diff-lines">
                      {commitDiffView ?? (
                        <span className="muted">Loading...</span>
                      )}
                    </div>
                  </div>
                ) : (
                  <div className="empty-pane">
                    Select a commit to view its files.
                  </div>
                )}
              </div>
            </div>
          </div>
        )}
      </main>

      <div className="ops-panel">
        <button
          className="ops-toggle"
          onClick={toggleOps}
          title="Git operation log"
        >
          <span className={opsOpen ? "ops-caret open" : "ops-caret"}>▸</span>
          Operations
          {ops.length > 0 && (
            <span className="ops-badge">
              {ops.filter((o) => o.status === "error").length > 0 && (
                <span className="ops-err-count">
                  {ops.filter((o) => o.status === "error").length} err
                </span>
              )}
              <span className="ops-count">{ops.length}</span>
            </span>
          )}
        </button>
        {opsOpen && (
          <div className="ops-body">
            <div className="ops-toolbar">
              <span className="ops-title">Git operations ({ops.length})</span>
              <div className="ops-toolbar-actions">
                <button
                  className="mini-btn bulk-btn"
                  onClick={exportOps}
                  disabled={ops.length === 0}
                  title="Export operations as .log"
                >
                  Export
                </button>
                <button
                  className="mini-btn bulk-btn"
                  onClick={() => setOps([])}
                  disabled={ops.length === 0}
                >
                  Clear
                </button>
              </div>
            </div>
            <div className="ops-list">
              {ops.length === 0 && (
                <div className="ops-empty muted">No operations yet.</div>
              )}
              {ops.map((o) => (
                <div key={o.id} className={`ops-row ops-${o.status}`}>
                  <span className="ops-time">{o.time}</span>
                  <span className="ops-op">{o.op}</span>
                  <span
                    className={`ops-status`}
                    title={o.status}
                  >
                    {o.status === "ok" ? "✓" : o.status === "error" ? "✗" : "•"}
                  </span>
                  <span className="ops-detail">{o.detail}</span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className="branch-actions">
        <span className="branch-actions-label">Merge</span>
        <select
          className="small-input"
          value={mergeTarget}
          onChange={(e) => setMergeTarget(e.currentTarget.value)}
          title="选择要合并进当前分支的分支"
        >
          <option value="">— 选择分支 —</option>
          {branches
            .filter((b) => !b.isCurrent)
            .map((b) => (
              <option key={b.refname} value={b.name}>
                {b.name}
              </option>
            ))}
        </select>
        <button
          className="btn"
          onClick={doMerge}
          disabled={!mergeTarget || !!busy}
          title={
            mergeTarget
              ? `将 ${mergeTarget} 合并到 ${status?.branch ?? "当前分支"}`
              : "请先选择分支"
          }
        >
          {busy === "merge"
            ? "Merging…"
            : `Merge into ${status?.branch ?? "current"}`}
        </button>
        {conflicts.length > 0 && (
          <button
            className="btn abort"
            onClick={doAbortMerge}
            disabled={!!busy}
          >
            {busy === "abort" ? "Aborting…" : `Abort merge (${conflicts.length})`}
          </button>
        )}

        <span className="branch-actions-divider" />

        <input
          ref={branchInputRef}
          className="small-input"
          placeholder="new branch name"
          value={branch}
          onChange={(e) => setBranch(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void doCreateBranch();
          }}
        />
        <button
          className="btn"
          onClick={doCreateBranch}
          disabled={!branch.trim() || !repoPath.trim()}
        >
          Create branch
        </button>

        <span className="branch-actions-divider" />

        <input
          className="small-input"
          placeholder="remote"
          value={remote}
          onChange={(e) => setRemote(e.currentTarget.value)}
        />
      </div>
    </div>
  );
}

function FileList({
  title,
  files,
  selected,
  onSelect,
  bulkLabel,
  onBulk,
  actionLabel,
  actionIcon,
  onAction,
  onDiscard,
  collapsed,
  onToggleDir,
  checked,
  onToggleFile,
  onSetDir,
  onClearChecked,
}: {
  title: string;
  files: FileStatus[];
  selected: string | null;
  onSelect: (p: string) => void;
  bulkLabel: string;
  onBulk: () => void;
  actionLabel: string;
  actionIcon: string;
  onAction: (p: string) => void;
  onDiscard?: (p: string) => void;
  collapsed: Set<string>;
  onToggleDir: (path: string) => void;
  checked: Set<string>;
  onToggleFile: (path: string) => void;
  onSetDir: (paths: string[], on: boolean) => void;
  onClearChecked: () => void;
}) {
  const tree = useMemo(() => buildTree(files), [files]);
  const allPaths = useMemo(() => collectLeafPaths(tree), [tree]);
  const groupChecked = useMemo(
    () => allPaths.filter((p) => checked.has(p)),
    [allPaths, checked]
  );
  const rootState = checkState(allPaths, checked);

  const renderNode = (node: TreeNode, depth: number): ReactNode => {
    if (node.dir) {
      const dirKey = `${title}:${node.path}`;
      const isCollapsed = collapsed.has(dirKey);
      const leaves = collectLeafPaths([node]);
      const st = checkState(leaves, checked);
      const allOn = st === "on";
      return (
        <div key={`d:${node.path}`} className="tree-dir">
          <div
            className="tree-dir-row"
            style={{ paddingLeft: 8 + depth * 12 }}
            title={node.path}
          >
            <span
              className={isCollapsed ? "tree-caret" : "tree-caret open"}
              onClick={() => onToggleDir(dirKey)}
            >
              ▸
            </span>
            <button
              type="button"
              className={`cb cb-${st}`}
              title={
                allOn
                  ? `Deselect all under ${node.name}`
                  : `Select all under ${node.name}`
              }
              onClick={(e) => {
                e.stopPropagation();
                onSetDir(leaves, !allOn);
              }}
            >
              {st === "on" ? "✓" : st === "partial" ? "−" : ""}
            </button>
            <span
              className="tree-dir-icon"
              onClick={() => onToggleDir(dirKey)}
            >
              📁
            </span>
            <span
              className="tree-dir-name"
              onClick={() => onToggleDir(dirKey)}
            >
              {node.name}
            </span>
            <span className="tree-dir-count muted">{leaves.length}</span>
          </div>
          {!isCollapsed &&
            node.children?.map((c) => renderNode(c, depth + 1))}
        </div>
      );
    }
    const isOn = checked.has(node.path);
    return (
      <div
        key={`f:${node.path}`}
        className={
          selected === node.path ? "file-item selected" : "file-item"
        }
        style={{ paddingLeft: 8 + depth * 12 }}
        onClick={() => onSelect(node.path)}
        title={node.path}
      >
        <button
          type="button"
          className={`cb cb-${isOn ? "on" : "off"}`}
          title={isOn ? "Deselect" : "Select for bulk action"}
          onClick={(e) => {
            e.stopPropagation();
            onToggleFile(node.path);
          }}
        >
          {isOn ? "✓" : ""}
        </button>
        <span className={`file-icon status-${node.status ?? "modified"}`}>
          {STATUS_ICON[node.status ?? "modified"] ?? "•"}
        </span>
        <span className="file-path">{node.name}</span>
        {onDiscard && node.status !== "conflicted" && (
          <button
            className="mini-btn discard-btn"
            onClick={(e) => {
              e.stopPropagation();
              onDiscard(node.path);
            }}
            title="Discard changes"
          >
            ✕
          </button>
        )}
        <button
          className="mini-btn"
          onClick={(e) => {
            e.stopPropagation();
            onAction(node.path);
          }}
          title={actionLabel}
        >
          {actionIcon}
        </button>
      </div>
    );
  };

  return (
    <div className="file-group">
      <div className="file-group-header">
        <button
          type="button"
          className={`cb cb-${rootState}`}
          title={
            rootState === "on" ? "Deselect all" : "Select all in this group"
          }
          onClick={() => onSetDir(allPaths, rootState !== "on")}
        >
          {rootState === "on" ? "✓" : rootState === "partial" ? "−" : ""}
        </button>
        <span className="file-group-title">
          {title} ({files.length})
        </span>
        {groupChecked.length > 0 && (
          <button
            className="mini-btn bulk-btn"
            onClick={onClearChecked}
            title="Clear selection"
          >
            ✕ sel
          </button>
        )}
        <button
          className="mini-btn bulk-btn"
          onClick={onBulk}
          disabled={files.length === 0}
          title={bulkLabel}
        >
          {bulkLabel}
        </button>
      </div>
      <div className="file-list tree-list">
        {files.length === 0 && (
          <div className="file-empty">No changes</div>
        )}
        {tree.map((n) => renderNode(n, 0))}
      </div>
    </div>
  );
}

/** Read-only tree for Log commit files (collapse + select file). */
function FileTreeView({
  files,
  titleKey,
  collapsed,
  onToggleDir,
  selected,
  onSelect,
}: {
  files: FileStatus[];
  titleKey: string;
  collapsed: Set<string>;
  onToggleDir: (path: string) => void;
  selected: string | null;
  onSelect: (p: string) => void;
}) {
  const tree = useMemo(() => buildTree(files), [files]);

  const renderNode = (node: TreeNode, depth: number): ReactNode => {
    if (node.dir) {
      const key = `${titleKey}:${node.path}`;
      const isCollapsed = collapsed.has(key);
      return (
        <div key={`d:${node.path}`} className="tree-dir">
          <div
            className="tree-dir-row"
            style={{ paddingLeft: 8 + depth * 12 }}
            onClick={() => onToggleDir(key)}
            title={node.path}
          >
            <span className={isCollapsed ? "tree-caret" : "tree-caret open"}>
              ▸
            </span>
            <span className="tree-dir-icon">📁</span>
            <span className="tree-dir-name">{node.name}</span>
          </div>
          {!isCollapsed &&
            node.children?.map((c) => renderNode(c, depth + 1))}
        </div>
      );
    }
    return (
      <div
        key={`f:${node.path}`}
        className={selected === node.path ? "file-item selected" : "file-item"}
        style={{ paddingLeft: 8 + depth * 12 }}
        onClick={() => onSelect(node.path)}
        title={node.path}
      >
        <span className={`file-icon status-${node.status ?? "modified"}`}>
          {STATUS_ICON[node.status ?? "modified"] ?? "•"}
        </span>
        <span className="file-path">{node.name}</span>
      </div>
    );
  };

  return (
    <div className="file-list tree-list">
      {tree.map((n) => renderNode(n, 0))}
    </div>
  );
}

function SegView({ seg }: { seg: DiffSegment }) {
  const text = seg.text || "\u00A0";
  if (seg.highlights.length === 0) {
    return <span>{text}</span>;
  }
  const parts: ReactNode[] = [];
  let cursor = 0;
  for (const h of seg.highlights) {
    if (h.start > cursor) {
      parts.push(<span key={`n${cursor}`}>{text.slice(cursor, h.start)}</span>);
    }
    const safeEnd = Math.min(h.end, text.length);
    parts.push(
      <span key={`h${h.start}`} className="hl">
        {text.slice(h.start, safeEnd)}
      </span>
    );
    cursor = Math.max(cursor, safeEnd);
  }
  if (cursor < text.length) {
    parts.push(<span key={`n${cursor}`}>{text.slice(cursor)}</span>);
  }
  return <>{parts}</>;
}

function ConflictEditor({
  conflict,
  repoPath,
  onResolve,
  onSaved,
}: {
  conflict: ConflictInfo;
  repoPath: string;
  onResolve: (path: string, choice: string) => void;
  onSaved: () => void;
}) {
  const [text, setText] = useState(conflict.ours);

  useEffect(() => {
    setText(conflict.ours);
  }, [conflict.path, conflict.ours]);

  async function markResolved() {
    try {
      await invoke("write_file", {
        repoPath,
        path: conflict.path,
        content: text,
      });
      onSaved();
    } catch (e) {
      console.error(e);
    }
  }

  return (
    <div className="conflict-editor">
      <div className="conflict-toolbar">
        <span className="diff-title">{conflict.path}</span>
        <button className="btn ours" onClick={() => onResolve(conflict.path, "ours")}>
          Accept ours
        </button>
        <button className="btn theirs" onClick={() => onResolve(conflict.path, "theirs")}>
          Accept theirs
        </button>
        <button className="btn" onClick={markResolved}>
          Mark as Resolved
        </button>
      </div>
      <textarea
        className="edit-area conflict-area"
        value={text}
        onChange={(e) => setText(e.currentTarget.value)}
        spellCheck={false}
      />
    </div>
  );
}

export default App;