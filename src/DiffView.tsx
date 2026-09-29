// Two-column diff row rendering, shared by the working-file and commit diff views.

import { splitByHighlights } from "./diffHighlights";
import type { DiffSegment, FileDiff } from "./types";

function SegView({ seg }: { seg: DiffSegment }) {
  const text = seg.text || "\u00A0";
  if (seg.highlights.length === 0) {
    return <span>{text}</span>;
  }
  // Backend highlight ranges are UTF-16 code-unit offsets, matching slice().
  const parts = splitByHighlights(text, seg.highlights).map((p, i) => (
    <span key={i} className={p.hl ? "hl" : undefined}>
      {p.text}
    </span>
  ));
  return <>{parts}</>;
}

// A working-file diff and a commit-file diff both render the same two-column
// line list, so the row markup lives in one place and both callers (and the
// "no diff yet" null case) share it.
export function diffRowsToJsx(diff: FileDiff | null) {
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
}
