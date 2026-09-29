// Pure file-tree helpers shared by FileList / FileTreeView.

import type { FileStatus, TreeNode } from "./types";

export function buildTree(files: FileStatus[]): TreeNode[] {
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

export function collectLeafPaths(
  nodes: TreeNode[],
  out: string[] = []
): string[] {
  for (const n of nodes) {
    if (n.dir) collectLeafPaths(n.children ?? [], out);
    else out.push(n.path);
  }
  return out;
}

export function checkState(
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
