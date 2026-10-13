// Branch/remote operation cluster: split verbatim from App.tsx (branch input
// states 55-57, remote 66, branchInputRef 89, mergeCandidates/currentBranch
// memos 319-323, doPush 545-552, doPull 554-561, doFetch 563-570,
// doMerge 572-599, doAbortMerge 601-609, doRebase 611-638, doRebaseContinue
// 640-647, doRebaseAbort 649-657, doCheckout 750-759, doCreateBranch 761-774).

import { useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Dispatch, SetStateAction } from "react";
import type { BranchInfo, MergeResult, StatusInfo } from "../types";
import type { RunOp } from "./useCommitFlow";

export function useBranchOps(params: {
  repoPath: string;
  branches: BranchInfo[];
  status: StatusInfo | null;
  runOp: RunOp;
  setError: (message: string) => void;
  setInfo: (message: string) => void;
  setTab: Dispatch<SetStateAction<"changes" | "log">>;
  setSelectedFile: Dispatch<SetStateAction<string | null>>;
}) {
  const {
    repoPath,
    branches,
    status,
    runOp,
    setError,
    setInfo,
    setTab,
    setSelectedFile,
  } = params;

  const [branch, setBranch] = useState("");
  const [mergeTarget, setMergeTarget] = useState("");
  const [rebaseTarget, setRebaseTarget] = useState("");
  const [remote, setRemote] = useState("origin");
  const branchInputRef = useRef<HTMLInputElement>(null);

  const mergeCandidates = useMemo(
    () => branches.filter((b) => !b.isCurrent),
    [branches]
  );
  const currentBranchName = status?.branch ?? "";

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

  async function doRebase() {
    setError("");
    setInfo("");
    const target = rebaseTarget.trim();
    if (!target) {
      setError("请先选择要变基的目标分支");
      return;
    }
    await runOp({
      key: "rebase",
      op: "rebase",
      detail: `rebase onto ${target}`,
      onSuccess: () => setRebaseTarget(""),
      fn: async () => {
        const r = await invoke<MergeResult>("rebase", { repoPath, branch: target });
        if (r.status === "conflicts") {
          setTab("changes");
          if (r.conflicts.length > 0) {
            setSelectedFile(r.conflicts[0]);
          }
          // Conflicts mid-rebase: same business-outcome channel as merge —
          // the Continue/Abort buttons in the footer drive the rest.
          throw r.message;
        }
        return r.message;
      },
    });
  }

  async function doRebaseContinue() {
    await runOp({
      key: "rebase",
      op: "rebase continue",
      detail: "",
      fn: async () => invoke<string>("rebase_continue", { repoPath }),
    });
  }

  async function doRebaseAbort() {
    await runOp({
      key: "rebase",
      op: "rebase abort",
      detail: "",
      onSuccess: () => setSelectedFile(null),
      fn: async () => invoke<string>("rebase_abort", { repoPath }),
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

  return {
    branch,
    setBranch,
    mergeTarget,
    setMergeTarget,
    rebaseTarget,
    setRebaseTarget,
    remote,
    setRemote,
    branchInputRef,
    mergeCandidates,
    currentBranchName,
    doPush,
    doPull,
    doFetch,
    doMerge,
    doAbortMerge,
    doRebase,
    doRebaseContinue,
    doRebaseAbort,
    doCheckout,
    doCreateBranch,
  };
}
