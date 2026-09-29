import { describe, expect, it } from "vitest";
import { splitByHighlights } from "./diffHighlights";

const join = (parts: ReturnType<typeof splitByHighlights>) =>
  parts.map((p) => `${p.hl ? "[HL]" : ""}${p.text}${p.hl ? "[/HL]" : ""}`).join("");

describe("splitByHighlights", () => {
  it("returns the whole line as one plain part when there are no highlights", () => {
    expect(splitByHighlights("hello", [])).toEqual([{ text: "hello", hl: false }]);
  });

  it("splits a single middle highlight into plain + hl + plain", () => {
    expect(
      join(splitByHighlights("hello world", [{ start: 6, end: 11 }])),
    ).toBe("hello [HL]world[/HL]");
  });

  it("keeps two distant edits as two separate highlights (backend LCS spans)", () => {
    expect(
      join(
        splitByHighlights("foo = 1; bar = 2; baz = 3;", [
          { start: 6, end: 7 },
          { start: 24, end: 25 },
        ]),
      ),
    ).toBe("foo = [HL]1[/HL]; bar = 2; baz = [HL]3[/HL];");
  });

  it("slices by UTF-16 code units, matching surrogate-pair offsets from Rust", () => {
    // 🎉 is 1 char but 2 UTF-16 units; backend reports (1,3).
    const parts = splitByHighlights("a🎉b", [{ start: 1, end: 3 }]);
    expect(parts).toEqual([
      { text: "a", hl: false },
      { text: "🎉", hl: true },
      { text: "b", hl: false },
    ]);
  });

  it("clamps degenerate or overlapping ranges instead of duplicating text", () => {
    expect(join(splitByHighlights("abcdef", [{ start: 4, end: 2 }])))
      .toBe("abcdef");
    const parts = splitByHighlights("abcdef", [
      { start: 1, end: 4 },
      { start: 2, end: 5 },
    ]);
    // No gap and no duplicated text despite the overlap…
    expect(parts.map((p) => p.text).join("")).toBe("abcdef");
    // …and the overlapping span is unioned.
    expect(parts.filter((p) => p.hl).map((p) => p.text).join("")).toBe("bcde");
    expect(join(splitByHighlights("abc", [{ start: 9, end: 99 }]))).toBe("abc");
  });

  it("reassembles the original text for well-formed spans", () => {
    const text = "价格：100 元";
    const parts = splitByHighlights(text, [{ start: 3, end: 4 }]);
    expect(parts.map((p) => p.text).join("")).toBe(text);
    expect(join(parts)).toBe("价格：[HL]1[/HL]00 元");
  });
});
