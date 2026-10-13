// Conflict view cluster: split verbatim from App.tsx (selectedConflict memo
// 314-317, doResolve 776-786).

import { useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ConflictInfo } from "../types";
import type { RunOp } from "./useCommitFlow";

export function useConflict(params: {
  repoPath: string;
  conflicts: ConflictInfo[];
  selectedFile: string | null;
  runOp: RunOp;
}) {
  const { repoPath, conflicts, selectedFile, runOp } = params;

  const selectedConflict = useMemo(
    () => conflicts.find((c) => c.path === selectedFile) ?? null,
    [conflicts, selectedFile]
  );

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

  return { selectedConflict, doResolve };
}
