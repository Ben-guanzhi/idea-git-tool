// File tree renderers: the checkbox-driven Staged/Unstaged list and the
// read-only Log commit-files tree.

import { useMemo, type ReactNode } from "react";
import { buildTree, checkState, collectLeafPaths } from "./tree";
import { STATUS_ICON, type FileStatus, type TreeNode } from "./types";

export function FileList({
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
export function FileTreeView({
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
