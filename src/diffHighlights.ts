export interface HighlightRange {
  start: number;
  end: number;
}

export interface SegPart {
  text: string;
  hl: boolean;
}

/**
 * Slice a diff line into plain/highlighted parts. Backend highlight ranges
 * are UTF-16 code-unit offsets, matching JS String.slice; they arrive sorted
 * and non-overlapping from the Rust LCS diff, but out-of-order or degenerate
 * ranges are clamped defensively rather than trusted.
 */
export function splitByHighlights(
  text: string,
  highlights: HighlightRange[],
): SegPart[] {
  const parts: SegPart[] = [];
  let cursor = 0;
  for (const h of highlights) {
    const start = Math.min(Math.max(h.start, cursor), text.length);
    const end = Math.min(Math.max(h.end, start), text.length);
    if (start > cursor) {
      parts.push({ text: text.slice(cursor, start), hl: false });
    }
    if (end > start) {
      parts.push({ text: text.slice(start, end), hl: true });
    }
    cursor = Math.max(cursor, end);
  }
  if (cursor < text.length) {
    parts.push({ text: text.slice(cursor), hl: false });
  }
  return parts;
}
