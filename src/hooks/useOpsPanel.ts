// Operations log panel cluster: split verbatim from App.tsx (states 81-84,
// opsIdRef 88, logOp 91-104, toggleOps 106-112, exportOps 114-146; the clear
// handler that lived inline at App.tsx:990 is exposed as clearOps).

import { useRef, useState } from "react";
import { OPS_KEY, type OpLogEntry } from "../types";

export function useOpsPanel(repoPath: string) {
  const [ops, setOps] = useState<OpLogEntry[]>([]);
  const [opsOpen, setOpsOpen] = useState(
    () => localStorage.getItem(OPS_KEY) === "1"
  );
  const opsIdRef = useRef(1);

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

  function clearOps() {
    setOps([]);
  }

  return { ops, opsOpen, logOp, toggleOps, exportOps, clearOps };
}
