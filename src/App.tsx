import {
  useReducer,
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

// A working-file diff and a commit-file diff both render the same two-column
// line list, so the row markup lives in one place and both callers (and the
// "no diff yet" null case) share it.
function diffRowsToJsx(diff: FileDiff | null) {
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

type BusyKey =
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
  | null;

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

// ── File view state machine: what the Changes tab shows for the selected file.
// A file selection loads content and diff as one atomic transition; the editor
// text and the edit toggle then change independently. ──
interface FileViewState {
  diff: FileDiff | null;
  editText: string;
  editing: boolean;
}

type FileViewAction =
  | { type: "clear" }
  | { type: "loaded"; diff: FileDiff; working: string; conflicted: boolean }
  | { type: "edit"; text: string }
  | { type: "toggleEditing" };

function fileViewReducer(
  state: FileViewState,
  action: FileViewAction
): FileViewState {
  switch (action.type) {
    case "clear":
      return { diff: null, editText: "", editing: false };
    case "loaded":
      return {
        diff: action.diff,
        editText: action.working,
        editing: action.conflicted,
      };
    case "edit":
      return { ...state, editText: action.text };
    case "toggleEditing":
      return { ...state, editing: !state.editing };
  }
}

const FILE_VIEW_INITIAL: FileViewState = {
  diff: null,
  editText: "",
  editing: false,
};

// ── Commit explorer state machine: the Log tab's drill-down. Selecting a
// commit resets the file pane; files and diff arrive asynchronously, and each
// response carries the selection it belongs to so a stale reply for an older
// selection cannot land. ──
interface CommitExplorerState {
  commit: string | null;
  files: CommitFile[];
  file: string | null;
  diff: FileDiff | null;
}

type CommitExplorerAction =
  | { type: "select"; commit: string }
  | { type: "files"; commit: string; files: CommitFile[] }
  | { type: "selectFile"; file: string }
  | { type: "loaded"; file: string; diff: FileDiff }
  | { type: "loadFailed" };

function commitExplorerReducer(
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

const COMMIT_EXPLORER_INITIAL: CommitExplorerState = {
  commit: null,
  files: [],
  file: null,
  diff: null,
};

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
                  onError={setError}
                />
              ) : selectedFile ? (
                <div className="file-diff">
                  <div className="diff-toolbar">
                    <span className="diff-title">{selectedFile}</span>
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
                  </div>
                  {fileView.editing ? (
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
                      explorer.commit === c.id
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
                    Files ({explorer.files.length})
                  </span>
                </div>
                {explorer.commit && explorer.files.length === 0 && (
                  <div className="file-empty">No files</div>
                )}
                {!explorer.commit && (
                  <div className="file-empty">Select a commit</div>
                )}
                {explorer.files.length > 0 && (
                  <FileTreeView
                    files={explorer.files.map((f) => ({
                      path: f.path,
                      status: f.status,
                    }))}
                    titleKey="log"
                    collapsed={logCollapsed}
                    onToggleDir={toggleLogDir}
                    selected={explorer.file}
                    onSelect={(p) =>
                      explorer.commit && selectCommitFile(explorer.commit, p)
                    }
                  />
                )}
              </aside>

              <div className="diff-area">
                {explorer.file ? (
                  <div className="file-diff">
                    <div className="diff-toolbar">
                      <span className="diff-title">{explorer.file}</span>
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
          onFocus={() => {
            // Branch list can be stale after external git ops.
            if (!busy) void refresh();
          }}
          title={
            mergeCandidates.length === 0
              ? "没有可合并的其它分支（含远程）；请先创建分支"
              : `将所选分支合并到 ${currentBranchName || "当前分支"}`
          }
        >
          <option value="">
            {mergeCandidates.length === 0
              ? branches.length === 0
                ? "— 无分支列表 —"
                : "— 仅当前分支 —"
              : "— 选择分支 —"}
          </option>
          {mergeCandidates.map((b) => (
            <option key={b.refname} value={b.name}>
              {b.name}
            </option>
          ))}
        </select>
        <button
          className="btn"
          onClick={doMerge}
          disabled={!mergeTarget || !!busy || mergeCandidates.length === 0}
          title={
            mergeTarget
              ? `将 ${mergeTarget} 合并到 ${currentBranchName || "当前分支"}`
              : mergeCandidates.length === 0
                ? "没有可合并的其它分支"
                : "请先选择分支"
          }
        >
          {busy === "merge"
            ? "Merging…"
            : `Merge into ${currentBranchName || "current"}`}
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
  // Backend highlight ranges are UTF-16 code-unit offsets, matching slice().
  const parts: ReactNode[] = [];
  let cursor = 0;
  for (const h of seg.highlights) {
    if (h.start > cursor) {
      parts.push(<span key={`n${cursor}`}>{text.slice(cursor, h.start)}</span>);
    }
    const end = Math.max(h.end, h.start);
    parts.push(
      <span key={`h${h.start}`} className="hl">
        {text.slice(h.start, end)}
      </span>
    );
    cursor = Math.max(cursor, end);
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
  onError,
}: {
  conflict: ConflictInfo;
  repoPath: string;
  onResolve: (path: string, choice: string) => void;
  onSaved: () => void;
  onError: (message: string) => void;
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
      // Surface the write failure in the app banner instead of swallowing it.
      onError(String(e));
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