// Tree collapse state: split verbatim from App.tsx (collapsedDirs/logCollapsed
// 85-86, toggleLogDir 172-180, toggleDir 788-795).

import { useState } from "react";

export function useTreeUi() {
  const [collapsedDirs, setCollapsedDirs] = useState<Set<string>>(new Set());
  const [logCollapsed, setLogCollapsed] = useState<Set<string>>(new Set());

  function toggleLogDir(path: string) {
    // `path` is already `log:<dirPath>` from FileTreeView.
    setLogCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
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

  return { collapsedDirs, toggleDir, logCollapsed, toggleLogDir };
}
