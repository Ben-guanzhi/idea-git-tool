// Merge-conflict editor: ours/theirs buttons plus a free-text resolve area.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ConflictInfo } from "./types";

export function ConflictEditor({
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
