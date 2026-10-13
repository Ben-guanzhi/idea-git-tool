import { describe, expect, it } from "vitest";
import { hunkLabel, hunkStagingViews } from "./hunks";
import type { ConflictInfo, StatusInfo } from "./types";

const status = (over?: Partial<StatusInfo>): StatusInfo => ({
  branch: "main",
  upstream: null,
  ahead: 0,
  behind: 0,
  staged: [],
  unstaged: [],
  ...over,
});

const conflict = (path: string): ConflictInfo => ({
  path,
  ours: "",
  theirs: "",
  base: "",
});

describe("hunkStagingViews", () => {
  it("offers nothing without a selection or status", () => {
    expect(hunkStagingViews(null, status(), [])).toEqual({
      unstaged: false,
      staged: false,
    });
    expect(hunkStagingViews("a.txt", null, [])).toEqual({
      unstaged: false,
      staged: false,
    });
  });

  it("offers unstaged hunks for a modified working-tree file", () => {
    const s = status({ unstaged: [{ path: "a.txt", status: "modified" }] });
    expect(hunkStagingViews("a.txt", s, [])).toEqual({
      unstaged: true,
      staged: false,
    });
  });

  it("offers both views for a partially staged file", () => {
    const s = status({
      staged: [{ path: "a.txt", status: "modified" }],
      unstaged: [{ path: "a.txt", status: "modified" }],
    });
    expect(hunkStagingViews("a.txt", s, [])).toEqual({
      unstaged: true,
      staged: true,
    });
  });

  it("excludes untracked/added, deleted and renamed files", () => {
    for (const kind of ["added", "deleted", "renamed", "typechanged"]) {
      const s = status({ unstaged: [{ path: "a.txt", status: kind }] });
      expect(hunkStagingViews("a.txt", s, [])).toEqual({
        unstaged: false,
        staged: false,
      });
    }
  });

  it("excludes conflicted files even when listed as modified", () => {
    const s = status({ unstaged: [{ path: "a.txt", status: "modified" }] });
    expect(hunkStagingViews("a.txt", s, [conflict("a.txt")])).toEqual({
      unstaged: false,
      staged: false,
    });
  });

  it("ignores other paths in the status lists", () => {
    const s = status({
      staged: [{ path: "b.txt", status: "modified" }],
      unstaged: [{ path: "b.txt", status: "modified" }],
    });
    expect(hunkStagingViews("a.txt", s, [])).toEqual({
      unstaged: false,
      staged: false,
    });
  });
});

describe("hunkLabel", () => {
  it("renders the @@ header with line counts", () => {
    expect(hunkLabel(3, 5, 2, 4)).toBe("@@ -3 +5 @@ (-2 +4)");
  });
});
