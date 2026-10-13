// Stash & tag input/actions cluster: split verbatim from App.tsx (stashMsg
// 62, tagInput 63, tagMsg 64, doStashSave 659-672, doStashApply 674-683,
// doStashPop 685-695, doStashDrop 697-711, doCreateTag 713-732, doDeleteTag
// 734-748). stashSel/tagSel themselves live in useRepoStatus because refresh
// prunes them.

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Dispatch, SetStateAction } from "react";
import type { OpLogEntry } from "../types";
import type { RunOp } from "./useCommitFlow";

export function useStashTags(params: {
  repoPath: string;
  runOp: RunOp;
  logOp: (op: string, status: OpLogEntry["status"], detail: string) => void;
  setError: (message: string) => void;
  stashSel: number | null;
  setStashSel: Dispatch<SetStateAction<number | null>>;
  tagSel: string;
  setTagSel: Dispatch<SetStateAction<string>>;
}) {
  const { repoPath, runOp, logOp, setError, stashSel, setStashSel, tagSel, setTagSel } =
    params;

  const [stashMsg, setStashMsg] = useState("");
  const [tagInput, setTagInput] = useState("");
  const [tagMsg, setTagMsg] = useState("");

  async function doStashSave() {
    const msg = stashMsg.trim();
    await runOp({
      key: "stash",
      op: "stash save",
      detail: msg || "(default message)",
      onSuccess: () => {
        setStashMsg("");
        setStashSel(0);
      },
      fn: async () =>
        invoke<string>("stash_save", { repoPath, message: msg || null }),
    });
  }

  async function doStashApply() {
    if (stashSel === null) return;
    const idx = stashSel;
    await runOp({
      key: "stash",
      op: "stash apply",
      detail: `stash@{${idx}}`,
      fn: async () => invoke<string>("stash_apply", { repoPath, index: idx }),
    });
  }

  async function doStashPop() {
    if (stashSel === null) return;
    const idx = stashSel;
    await runOp({
      key: "stash",
      op: "stash pop",
      detail: `stash@{${idx}}`,
      onSuccess: () => setStashSel(null),
      fn: async () => invoke<string>("stash_pop", { repoPath, index: idx }),
    });
  }

  async function doStashDrop() {
    if (stashSel === null) return;
    const label = `stash@{${stashSel}}`;
    if (!window.confirm(`Drop ${label}? This cannot be undone.`)) {
      logOp("stash drop", "info", `cancelled: ${label}`);
      return;
    }
    await runOp({
      key: "stash",
      op: "stash drop",
      detail: label,
      onSuccess: () => setStashSel(null),
      fn: async () => invoke<string>("stash_drop", { repoPath, index: stashSel! }),
    });
  }

  async function doCreateTag() {
    const name = tagInput.trim();
    if (!name) {
      setError("请输入标签名称 (Tag name is required)");
      return;
    }
    const msg = tagMsg.trim();
    await runOp({
      key: "tag",
      op: "tag",
      detail: `create ${name}`,
      onSuccess: () => {
        setTagInput("");
        setTagMsg("");
        setTagSel(name);
      },
      fn: async () =>
        invoke<string>("create_tag", { repoPath, name, message: msg || null }),
    });
  }

  async function doDeleteTag() {
    const name = tagSel;
    if (!name) return;
    if (!window.confirm(`Delete tag "${name}"? This cannot be undone.`)) {
      logOp("tag delete", "info", `cancelled: ${name}`);
      return;
    }
    await runOp({
      key: "tag",
      op: "tag delete",
      detail: `delete ${name}`,
      onSuccess: () => setTagSel(""),
      fn: async () => invoke<string>("delete_tag", { repoPath, name }),
    });
  }

  return {
    stashMsg,
    setStashMsg,
    tagInput,
    setTagInput,
    tagMsg,
    setTagMsg,
    doStashSave,
    doStashApply,
    doStashPop,
    doStashDrop,
    doCreateTag,
    doDeleteTag,
  };
}
