import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
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

const STATUS_ICON: Record<string, string> = {
  modified: "🖊",
  added: "+",
  deleted: "×",
  renamed: "→",
  conflicted: "⚠",
};

function App() {
  const [repoPath, setRepoPath] = useState("");
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [conflicts, setConflicts] = useState<ConflictInfo[]>([]);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [tab, setTab] = useState<"changes" | "log">("changes");
  const [branch, setBranch] = useState("");
  const [remote, setRemote] = useState("origin");
  const [info, setInfo] = useState("");
  const [error, setError] = useState("");
  const [diff, setDiff] = useState<FileDiff | null>(null);
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState("");
  const [logs, setLogs] = useState<CommitSummary[]>([]);
  const [branches, setBranches] = useState<BranchInfo[]>([]);

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

  async function doStage(paths: string[]) {
    try {
      await invoke("stage", { repoPath, paths });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doUnstage(paths: string[]) {
    try {
      await invoke("unstage", { repoPath, paths });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doCommit() {
    setError("");
    if (!message.trim()) {
      setError("Commit message is required");
      return;
    }
    try {
      const r = await invoke<string>("commit", {
        args: { repoPath, message, authorName: "", authorEmail: "" },
      });
      setInfo(`Committed ${r}`);
      setMessage("");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doPush() {
    setError("");
    try {
      const r = await invoke<string>("push", { repoPath, remote, branch });
      setInfo(r);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doPull() {
    setError("");
    try {
      const r = await invoke<string>("pull", { repoPath, remote, branch });
      setInfo(r);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doFetch() {
    setError("");
    try {
      const r = await invoke<string>("fetch", { repoPath, remote });
      setInfo(r);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doMerge() {
    setError("");
    if (!branch.trim()) {
      setError("Branch to merge is required");
      return;
    }
    try {
      const r = await invoke<string>("merge", { repoPath, branch });
      setInfo(r);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doCheckout(name: string) {
    setError("");
    try {
      const r = await invoke<string>("checkout", { repoPath, branch: name });
      setInfo(r);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doCreateBranch() {
    setError("");
    if (!branch.trim()) {
      setError("Branch name is required");
      return;
    }
    try {
      const r = await invoke<string>("create_branch", {
        repoPath,
        name: branch,
      });
      setInfo(r);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doResolve(path: string, choice: string) {
    setError("");
    try {
      await invoke("resolve_conflict", { repoPath, paths: [path], choice });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

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
            ahead {status.ahead} / behind {status.behind}
          </span>
        )}
        <button className="btn" onClick={doFetch}>
          Fetch
        </button>
        <button className="btn" onClick={doPull}>
          Pull
        </button>
        <button className="btn" onClick={doPush}>
          Push
        </button>
        <button className="btn" onClick={doCreateBranch}>
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
              if (logs.length === 0) loadLog();
            }}
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
                onStageAll={() => {}}
                onUnstageAll={() =>
                  doUnstage((status?.staged ?? []).map((f) => f.path))
                }
                actionLabel="Unstage"
                onAction={(p) => doUnstage([p])}
              />
              <FileList
                title="Unstaged"
                files={status?.unstaged ?? []}
                selected={selectedFile}
                onSelect={setSelectedFile}
                onStageAll={() =>
                  doStage((status?.unstaged ?? []).map((f) => f.path))
                }
                onUnstageAll={() => {}}
                actionLabel="Stage"
                onAction={(p) => doStage([p])}
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
                placeholder="Commit message (first line = subject, rest = body)..."
                value={message}
                onChange={(e) => setMessage(e.currentTarget.value)}
              />
              <button className="commit-btn" onClick={doCommit}>
                Commit
              </button>
            </div>
          </div>
        )}

        {tab === "log" && (
          <div className="log-pane">
            <div className="log-list">
              {logs.map((c) => (
                <div key={c.id} className="log-item">
                  <div className="log-refs">
                    {c.refs.map((r) => (
                      <span key={r} className="log-ref">
                        {r}
                      </span>
                    ))}
                  </div>
                  <div className="log-msg">{c.message.split("\n")[0]}</div>
                  <div className="log-meta">
                    {c.author} · {new Date(Number(c.time) * 1000).toLocaleString()}{" "}
                    · {c.shortId}
                  </div>
                </div>
              ))}
              {logs.length === 0 && (
                <div className="muted">No commits yet.</div>
              )}
            </div>
          </div>
        )}
      </main>

      <div className="branch-actions">
        <input
          className="small-input"
          placeholder="branch name"
          value={branch}
          onChange={(e) => setBranch(e.currentTarget.value)}
        />
        <input
          className="small-input"
          placeholder="remote"
          value={remote}
          onChange={(e) => setRemote(e.currentTarget.value)}
        />
        <button className="btn" onClick={doMerge}>
          Merge branch into current
        </button>
        <button className="btn" onClick={doCreateBranch}>
          Create branch
        </button>
      </div>
    </div>
  );
}

function FileList({
  title,
  files,
  selected,
  onSelect,
  onStageAll,
  onUnstageAll,
  actionLabel,
  onAction,
}: {
  title: string;
  files: FileStatus[];
  selected: string | null;
  onSelect: (p: string) => void;
  onStageAll: () => void;
  onUnstageAll: () => void;
  actionLabel: string;
  onAction: (p: string) => void;
}) {
  return (
    <div className="file-group">
      <div className="file-group-header">
        <span className="file-group-title">
          {title} ({files.length})
        </span>
        <button className="mini-btn" onClick={files.length > 0 ? onStageAll : onUnstageAll}>
          {files.length > 0 ? "▽" : "△"}
        </button>
      </div>
      <ul className="file-list">
        {files.map((f) => (
          <li
            key={f.path}
            className={selected === f.path ? "file-item selected" : "file-item"}
            onClick={() => onSelect(f.path)}
          >
            <span className={`file-icon status-${f.status}`}>
              {STATUS_ICON[f.status] ?? "•"}
            </span>
            <span className="file-path">{f.path}</span>
            <button
              className="mini-btn"
              onClick={(e) => {
                e.stopPropagation();
                onAction(f.path);
              }}
              title={actionLabel}
            >
              {files.length > 0 ? "▽" : "△"}
            </button>
          </li>
        ))}
        {files.length === 0 && <li className="file-empty">No changes</li>}
      </ul>
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