// Footer action strip (part 2): stash save/apply/pop/drop + tag create/delete.

import type { BusyKey, StashInfo, TagInfo } from "./types";

export function StashTagBar({
  repoPath,
  busy,
  stashes,
  tags,
  stashSel,
  onStashSel,
  stashMsg,
  onStashMsg,
  tagInput,
  onTagInput,
  tagMsg,
  onTagMsg,
  tagSel,
  onTagSel,
  onSaveStash,
  onApplyStash,
  onPopStash,
  onDropStash,
  onCreateTag,
  onDeleteTag,
  onRefresh,
}: {
  repoPath: string;
  busy: BusyKey;
  stashes: StashInfo[];
  tags: TagInfo[];
  stashSel: number | null;
  onStashSel: (value: number | null) => void;
  stashMsg: string;
  onStashMsg: (value: string) => void;
  tagInput: string;
  onTagInput: (value: string) => void;
  tagMsg: string;
  onTagMsg: (value: string) => void;
  tagSel: string;
  onTagSel: (value: string) => void;
  onSaveStash: () => void;
  onApplyStash: () => void;
  onPopStash: () => void;
  onDropStash: () => void;
  onCreateTag: () => void;
  onDeleteTag: () => void;
  onRefresh: () => void;
}) {
  const selectedTag = tags.find((t) => t.name === tagSel) ?? null;
  return (
    <div className="branch-actions">
      <span className="branch-actions-label">Stash</span>
      <input
        className="small-input"
        placeholder="stash message (optional)"
        value={stashMsg}
        onChange={(e) => onStashMsg(e.currentTarget.value)}
        title="留空则使用 git 默认的 “WIP on &lt;branch&gt;” 消息"
      />
      <button
        className="btn"
        onClick={onSaveStash}
        disabled={!!busy || !repoPath.trim()}
        title="暂存全部未提交改动（git stash push）"
      >
        {busy === "stash" ? "Stashing…" : "Save"}
      </button>
      <select
        className="small-input"
        value={stashSel ?? ""}
        onChange={(e) =>
          onStashSel(
            e.currentTarget.value === "" ? null : Number(e.currentTarget.value)
          )
        }
        onFocus={onRefresh}
        title="stash 列表（0 = 最新）"
      >
        <option value="">
          {stashes.length === 0 ? "— 无 stash —" : "— 选择 stash —"}
        </option>
        {stashes.map((s) => (
          <option key={s.index} value={s.index}>
            stash@{s.index}: {s.message}
          </option>
        ))}
      </select>
      <button
        className="btn"
        onClick={onApplyStash}
        disabled={stashSel === null || !!busy}
        title="应用该 stash，条目保留（git stash apply）"
      >
        Apply
      </button>
      <button
        className="btn"
        onClick={onPopStash}
        disabled={stashSel === null || !!busy}
        title="应用并删除该 stash（git stash pop）"
      >
        Pop
      </button>
      <button
        className="btn abort"
        onClick={onDropStash}
        disabled={stashSel === null || !!busy}
        title="丢弃该 stash，不可恢复"
      >
        Drop
      </button>

      <span className="branch-actions-divider" />

      <span className="branch-actions-label">Tag</span>
      <input
        className="small-input"
        placeholder="tag name"
        value={tagInput}
        onChange={(e) => onTagInput(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") onCreateTag();
        }}
        title="在当前 HEAD（或选中提交）上创建标签"
      />
      <input
        className="small-input"
        placeholder="annotation (empty = lightweight)"
        value={tagMsg}
        onChange={(e) => onTagMsg(e.currentTarget.value)}
        title="填写注释则为注解标签（git tag -a），留空为轻量标签"
      />
      <button
        className="btn"
        onClick={onCreateTag}
        disabled={!tagInput.trim() || !!busy}
        title="创建标签"
      >
        {busy === "tag" ? "Tagging…" : "Create tag"}
      </button>
      <select
        className="small-input"
        value={tagSel}
        onChange={(e) => onTagSel(e.currentTarget.value)}
        onFocus={onRefresh}
        title={
          selectedTag
            ? `${selectedTag.name} → ${selectedTag.target}` +
              (selectedTag.annotated ? "（注解）" : "（轻量）") +
              (selectedTag.message ? `\n${selectedTag.message}` : "")
            : "标签列表"
        }
      >
        <option value="">
          {tags.length === 0 ? "— 无标签 —" : "— 选择标签 —"}
        </option>
        {tags.map((t) => (
          <option key={t.name} value={t.name}>
            {t.name} → {t.target}
            {t.annotated ? " ⭐" : ""}
          </option>
        ))}
      </select>
      <button
        className="btn abort"
        onClick={onDeleteTag}
        disabled={!tagSel || !!busy}
        title="删除所选标签，不可恢复"
      >
        Delete tag
      </button>
    </div>
  );
}
