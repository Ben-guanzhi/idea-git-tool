//! Line/word diff engine and the diff commands.

use serde::Serialize;
use std::borrow::Cow;

use crate::repo::{blob_to_string, open_repo, resolve_in_workdir, tree_blob};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Highlight {
    start: usize,
    end: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffSegment {
    text: String,
    /// Character ranges (UTF-16 code units) to highlight within `text`.
    /// The frontend slices JS strings by UTF-16 code units, so the units
    /// must match or non-ASCII text highlights in the wrong place.
    highlights: Vec<Highlight>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffRow {
    /// One of: "context", "added", "removed", "modified", "elided".
    kind: String,
    /// Left column (base/HEAD). None if this is an "added" row.
    left: Option<DiffSegment>,
    /// Right column (working). None if this is a "removed" row.
    right: Option<DiffSegment>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileDiff {
    path: String,
    rows: Vec<DiffRow>,
}

/// One line of the linear diff, before it is windowed: the marker char
/// (`' `, `-`, `+`) and a view of the file it came from.
///
/// Nothing here owns text. A diff of a ten-thousand-line file with one edit
/// used to allocate a `String` per line on its way to a window that shows a
/// dozen of them — Θ(file) work for a Θ(window) answer. Only [`Op`], the
/// windowed form, can hold the one piece of text the files do not already
/// contain (the collapsed-run marker), and [`align_rows`] is where the lines
/// that survive are actually copied.
type Line<'a> = (char, &'a str);

/// One line of a windowed diff: the marker char (`' `, `-`, `+`, [`ELIDED`])
/// and, for everything but the collapsed-run marker, still a view of the file.
type Op<'a> = (char, Cow<'a, str>);

/// LCS DP budget for inline highlighting: if the token counts of the two
/// trimmed middles multiply past this, fall back to one coarse span instead
/// of allocating an O(m·n) matrix for a pathological single line.
const MAX_HIGHLIGHT_DP_CELLS: usize = 250_000;

/// Tokenize into maximal runs: word (alnum + `_`), whitespace, or one
/// punctuation char. Returns (start_char, len) into `chars`; the runs tile
/// the slice contiguously, so adjacent flagged tokens merge into one span.
fn tokenize_into(chars: &[char], out: &mut Vec<(usize, usize)>) {
    out.clear();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let c = chars[i];
        if c.is_alphanumeric() || c == '_' {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
        } else if c.is_whitespace() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
        } else {
            i += 1;
        }
        out.push((start, i - start));
    }
}

/// Scratch the inline highlight walk needs per line: two character arrays, two
/// token arrays, the DP direction matrix, the two LCS rows, the "flagged"
/// array, the UTF-16 offset table and the resulting spans.
///
/// [`align_rows`] keeps one of these for the whole diff and hands it to every
/// changed row. That is the difference between roughly ten heap operations per
/// row and roughly ten per file: a row is a line of source, tens of characters,
/// and a diff of a rewritten file has thousands of them.
#[derive(Default)]
struct Scratch {
    line_chars: Vec<char>,
    other_chars: Vec<char>,
    line_tokens: Vec<(usize, usize)>,
    other_tokens: Vec<(usize, usize)>,
    dir: Vec<u8>,
    prev: Vec<u16>,
    cur: Vec<u16>,
    flagged: Vec<bool>,
    utf16: Vec<usize>,
}

/// Char ranges (relative to `a`) of tokens not in the LCS of `a`/`b` token
/// sequences — the word-level diff that lets one row carry several disjoint
/// highlights instead of one span swallowing the unchanged middle.
/// The tokens of both sides are passed in: they were just computed for the
/// DP budget check, and re-tokenizing them here doubled the work.
#[allow(clippy::too_many_arguments)] // scratch fields, split by borrow-checker design
fn lcs_char_spans(
    a: &[char],
    at: &[(usize, usize)],
    b: &[char],
    bt: &[(usize, usize)],
    dir: &mut Vec<u8>,
    prev: &mut Vec<u16>,
    cur: &mut Vec<u16>,
    flagged: &mut Vec<bool>,
    spans: &mut Vec<(usize, usize)>,
) {
    let n = at.len();
    let m = bt.len();
    let width = m + 1;
    // dir[i][j]: 1 = match (diagonal), 2 = up (a-token unmatched), 3 = left
    dir.clear();
    dir.resize((n + 1) * width, 0);
    prev.clear();
    prev.resize(width, 0);
    cur.resize(width, 0);
    flagged.clear();
    flagged.resize(n, false);
    spans.clear();

    for i in 1..=n {
        cur[0] = 0;
        let a_tok = at[i - 1];
        for j in 1..=m {
            let b_tok = bt[j - 1];
            if a[a_tok.0..a_tok.0 + a_tok.1] == b[b_tok.0..b_tok.0 + b_tok.1] {
                cur[j] = prev[j - 1] + 1;
                dir[i * width + j] = 1;
            } else if prev[j] >= cur[j - 1] {
                cur[j] = prev[j];
                dir[i * width + j] = 2;
            } else {
                cur[j] = cur[j - 1];
                dir[i * width + j] = 3;
            }
        }
        // `cur` is finished; the row it replaces is overwritten cell by cell
        // on the next pass, so no clearing is needed.
        std::mem::swap(prev, cur);
    }

    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        match dir[i * width + j] {
            1 => {
                i -= 1;
                j -= 1;
            }
            2 => {
                flagged[i - 1] = true;
                i -= 1;
            }
            _ => j -= 1,
        }
    }
    while i > 0 {
        flagged[i - 1] = true;
        i -= 1;
    }
    let mut k = 0;
    while k < n {
        if flagged[k] {
            let start = at[k].0;
            let mut end = at[k].0 + at[k].1;
            while k + 1 < n && flagged[k + 1] {
                k += 1;
                end = at[k].0 + at[k].1;
            }
            spans.push((start, end));
        }
        k += 1;
    }
}

/// Fill `out` with the ranges of `line` that differ from `other` — several
/// minimal word-level spans rather than one coarse span, so two distant edits
/// don't highlight the unchanged text between them — as UTF-16 code-unit
/// offsets. JS `String.slice` on the frontend indexes by UTF-16 code units, and
/// a byte- or char-indexed range would misalign as soon as the line contains
/// non-BMP or multi-byte characters. Comparing per char (not per byte) also
/// guarantees a span never splits inside a multi-byte character.
fn char_diff_range(
    line: &str,
    other: &str,
    sc: &mut Scratch,
    out: &mut Vec<(usize, usize)>,
) {
    let Scratch {
        line_chars: a,
        other_chars: b,
        line_tokens: at,
        other_tokens: bt,
        dir,
        prev,
        cur,
        flagged,
        utf16,
    } = sc;

    a.clear();
    a.extend(line.chars());
    b.clear();
    b.extend(other.chars());

    let mut common_prefix = 0usize;
    while common_prefix < a.len()
        && common_prefix < b.len()
        && a[common_prefix] == b[common_prefix]
    {
        common_prefix += 1;
    }
    let mut common_suffix = 0usize;
    while common_suffix < a.len() - common_prefix
        && common_suffix < b.len() - common_prefix
        && a[a.len() - 1 - common_suffix] == b[b.len() - 1 - common_suffix]
    {
        common_suffix += 1;
    }
    let line_end = a.len() - common_suffix;
    out.clear();
    if common_prefix >= line_end {
        return;
    }
    let other_end = b.len() - common_suffix;

    let a_mid = &a[common_prefix..line_end];
    let b_mid = &b[common_prefix..other_end];
    tokenize_into(a_mid, at);
    tokenize_into(b_mid, bt);

    if at.len().saturating_mul(bt.len()) > MAX_HIGHLIGHT_DP_CELLS {
        out.push((common_prefix, line_end));
    } else {
        lcs_char_spans(
            a_mid,
            at,
            b_mid,
            bt,
            dir,
            prev,
            cur,
            flagged,
            out,
        );
        for span in out.iter_mut() {
            span.0 += common_prefix;
            span.1 += common_prefix;
        }
    }

    // char offset -> UTF-16 code-unit offset (cumulative prefix lengths)
    utf16.clear();
    utf16.push(0);
    for c in a.iter() {
        utf16.push(*utf16.last().unwrap() + c.len_utf16());
    }
    let mut kept = 0usize;
    for i in 0..out.len() {
        // 先读出区间再写回：同一向量上原地压缩会同时借出可变与不可变引用。
        let (s, e) = out[i];
        if e > s {
            out[kept] = (utf16[s], utf16[e]);
            kept += 1;
        }
    }
    out.truncate(kept);
}

// How many unchanged lines to keep on either side of a change. Anything beyond
// that collapses into one elided marker, so a one-line edit in a 10k-line file
// sends a dozen rows across the wire instead of ten thousand.
const DIFF_CONTEXT: usize = 3;

// The kind marker for a collapsed run of unchanged lines. Kept outside the
// printable range so it cannot be mistaken for a character a real line holds.
const ELIDED: char = '\u{00A6}';

fn elided_text(lines: usize) -> String {
    format!("已省略 {lines} 行")
}

/// Collapse runs of unchanged lines down to a window around the nearest change,
/// the way `diff -U3` breaks a long file into hunks. The DP already only covers
/// the trimmed middle, but the output still carried every context line of the
/// file, so a huge file with one edit rendered ten thousand DOM rows for one
/// changed line.
///
/// A run in front of the first change keeps its tail, one behind the last
/// change keeps its head, and one between two changes keeps both ends with the
/// middle elided. A diff with no change at all is returned untouched: there is
/// nothing to window on, and an "elided" marker would misreport a diff that has
/// no difference.
fn window_ops<'a>(ops: Vec<Line<'a>>) -> Vec<Op<'a>> {
    let mut segs: Vec<(bool, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        let is_context = ops[i].0 == ' ';
        let from = i;
        while i < ops.len() && (ops[i].0 == ' ') == is_context {
            i += 1;
        }
        segs.push((is_context, from, i - from));
    }
    if !segs.iter().any(|(is_context, _, _)| !*is_context) {
        // No change to window on: every line is shown, none was edited, so
        // borrowing the text would be the same as this copy.
        return ops.into_iter().map(|(k, t)| (k, Cow::Borrowed(t))).collect();
    }

    let mut out: Vec<Op<'a>> = Vec::new();
    for (idx, (is_context, from, len)) in segs.iter().enumerate() {
        let body = &ops[*from..*from + *len];
        if !*is_context {
            out.extend(body.iter().map(|&(k, t)| (k, Cow::Borrowed(t))));
            continue;
        }
        let keep = DIFF_CONTEXT.min(*len);
        if idx == 0 {
            let skip = *len - keep;
            if skip > 0 {
                out.push((ELIDED, Cow::Owned(elided_text(skip))));
            }
            out.extend(body[skip..].iter().map(|&(k, t)| (k, Cow::Borrowed(t))));
        } else if idx + 1 == segs.len() {
            out.extend(body[..keep].iter().map(|&(k, t)| (k, Cow::Borrowed(t))));
            let skip = *len - keep;
            if skip > 0 {
                out.push((ELIDED, Cow::Owned(elided_text(skip))));
            }
        } else {
            let head = keep;
            let tail = DIFF_CONTEXT.min(*len - head);
            let middle = *len - head - tail;
            out.extend(body[..head].iter().map(|&(k, t)| (k, Cow::Borrowed(t))));
            if middle > 0 {
                out.push((ELIDED, Cow::Owned(elided_text(middle))));
            }
            out.extend(
                body[*len - tail..]
                    .iter()
                    .map(|&(k, t)| (k, Cow::Borrowed(t))),
            );
        }
    }
    out
}

/// Split a linear diff op stream into aligned row pairs. This is the one place
/// the diff pays for text: every line that gets here was chosen by
/// [`window_ops`], so the copying is Θ(rows shown) rather than Θ(file).
fn align_rows(ops: &[Op<'_>]) -> Vec<DiffRow> {
    /// The line as the row sees it: borrowed text either way, so this never
    /// copies — [`align_rows`] copies once, when it builds the segment.
    fn text<'a, 'b>(op: &'b Op<'a>) -> &'b str {
        match &op.1 {
            Cow::Borrowed(s) => *s,
            Cow::Owned(s) => s.as_str(),
        }
    }

    let mut rows = Vec::new();
    let mut scratch = Scratch::default();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        let kind = ops[i].0;
        if kind == ' ' {
            let both = text(&ops[i]).to_string();
            rows.push(DiffRow {
                kind: "context".to_string(),
                left: Some(DiffSegment {
                    text: both.clone(),
                    highlights: Vec::new(),
                }),
                right: Some(DiffSegment {
                    text: both,
                    highlights: Vec::new(),
                }),
            });
            i += 1;
        } else if kind == ELIDED {
            // One row spanning both columns, not a line pair: it names the run
            // that was left out rather than showing it.
            rows.push(DiffRow {
                kind: "elided".to_string(),
                left: Some(DiffSegment {
                    text: text(&ops[i]).to_string(),
                    highlights: Vec::new(),
                }),
                right: None,
            });
            i += 1;
        } else {
            // Gather a change block: consecutive '-' and '+'.
            let mut removed: Vec<&str> = Vec::new();
            let mut added: Vec<&str> = Vec::new();
            while i < ops.len() && ops[i].0 != ' ' {
                match ops[i].0 {
                    '-' => removed.push(text(&ops[i])),
                    '+' => added.push(text(&ops[i])),
                    _ => {}
                }
                i += 1;
            }
            // Pair removed/added by index as modified rows.
            let pairs = removed.len().max(added.len());
            for idx in 0..pairs {
                let row = match (removed.get(idx), added.get(idx)) {
                    (Some(r), Some(a)) => {
                        char_diff_range(r, a, &mut scratch, &mut spans);
                        let left = DiffSegment {
                            text: r.to_string(),
                            highlights: highlights(&spans),
                        };
                        char_diff_range(a, r, &mut scratch, &mut spans);
                        let right = DiffSegment {
                            text: a.to_string(),
                            highlights: highlights(&spans),
                        };
                        DiffRow {
                            kind: "modified".to_string(),
                            left: Some(left),
                            right: Some(right),
                        }
                    }
                    (Some(r), None) => DiffRow {
                        kind: "removed".to_string(),
                        left: Some(DiffSegment {
                            text: r.to_string(),
                            highlights: Vec::new(),
                        }),
                        right: None,
                    },
                    (None, Some(a)) => DiffRow {
                        kind: "added".to_string(),
                        left: None,
                        right: Some(DiffSegment {
                            text: a.to_string(),
                            highlights: Vec::new(),
                        }),
                    },
                    (None, None) => continue,
                };
                rows.push(row);
            }
        }
    }
    rows
}

/// The spans the row sends to the frontend.
fn highlights(spans: &[(usize, usize)]) -> Vec<Highlight> {
    spans
        .iter()
        .map(|(start, end)| Highlight {
            start: *start,
            end: *end,
        })
        .collect()
}

/// The row that holds a line of the file: `n + 1` rows × `m + 1` columns of
/// LCS lengths, in one contiguous buffer.
///
/// A cell counts a common subsequence of lines, so it never exceeds
/// `min(n, m) ≤ √(n·m) ≤ √MAX_DP_CELLS` — `u32` is not close to filling, and
/// it halves the matrix (and the page-set the walk touches) against `usize`.
/// The row-major layout matters more than the width: the fill reads the row
/// below and the row it is writing, and the backtrack reads the same two, so
/// the scan is along contiguous memory either way. A `Vec<Vec<_>>` paid one
/// allocation and one page-set per row for that.
fn lcs_matrix(a: &[&str], b: &[&str]) -> Vec<u32> {
    let n = a.len();
    let m = b.len();
    let width = m + 1;
    let mut dp = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        let (cur, next) = (i * width, (i + 1) * width);
        for j in (0..m).rev() {
            dp[cur + j] = if a[i] == b[j] {
                dp[next + j + 1] + 1
            } else {
                dp[next + j].max(dp[cur + j + 1])
            };
        }
    }
    dp
}

fn diff_strings<'a>(base: &'a str, target: &'a str) -> Result<Vec<Line<'a>>, String> {
    let a_full: Vec<&str> = base.split('\n').collect();
    let b_full: Vec<&str> = target.split('\n').collect();

    // Trim the common prefix/suffix first so the O(n·m) DP below only covers
    // the region that actually differs — a one-line change in a 10k-line
    // lockfile then costs a tiny matrix instead of 10^8 cells.
    let mut start = 0usize;
    while start < a_full.len() && start < b_full.len() && a_full[start] == b_full[start] {
        start += 1;
    }
    let mut end_a = a_full.len();
    let mut end_b = b_full.len();
    while end_a > start && end_b > start && a_full[end_a - 1] == b_full[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let a = &a_full[start..end_a];
    let b = &b_full[start..end_b];
    let n = a.len();
    let m = b.len();

    // The DP matrix is (n+1)×(m+1) cells; without a budget a pair of large
    // files allocates hundreds of MB and blocks the webview. 6M cells ≈ 24MB
    // is the hard ceiling after prefix/suffix trimming.
    const MAX_DP_CELLS: u64 = 6_000_000;
    if (n as u64).saturating_mul(m as u64) > MAX_DP_CELLS {
        return Err(format!(
            "文件差异过大（{n} 行 × {m} 行），无法生成行级 diff，请在终端用 git diff 查看"
        ));
    }

    let dp = lcs_matrix(a, b);
    let width = m + 1;
    let cell = |i: usize, j: usize| dp[i * width + j];

    let mut out: Vec<Line> = Vec::with_capacity(n + m + 2);
    // Context lines before the first difference.
    for line in a_full[..start].iter().copied() {
        out.push((' ', line));
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if cell(i + 1, j) >= cell(i, j + 1) {
            out.push(('-', a[i]));
            i += 1;
        } else {
            out.push(('+', b[j]));
            j += 1;
        }
    }
    while i < n {
        out.push(('-', a[i]));
        i += 1;
    }
    while j < m {
        out.push(('+', b[j]));
        j += 1;
    }
    // Context lines after the last difference.
    for line in a_full[end_a..].iter().copied() {
        out.push((' ', line));
    }
    Ok(out)
}

/// Side-by-side rows for a base/target pair: the linear diff, windowed to the
/// changes, then aligned. Shared by both diff commands so the two cannot drift.
fn diff_rows<'a>(base: &'a str, target: &'a str) -> Result<Vec<DiffRow>, String> {
    Ok(align_rows(&window_ops(diff_strings(base, target)?)))
}

/// Produce a side-by-side diff of the working tree against HEAD for the given path.
///
/// Only the HEAD and working-tree contents are needed; the LCS behind
/// `diff_strings` is O(n*m), so the whole computation runs off the main
/// thread and a huge file cannot freeze the window while it computes.
#[tauri::command]
pub(crate) async fn get_diff(repo_path: String, path: String) -> Result<FileDiff, String> {
    let diff = tauri::async_runtime::spawn_blocking(move || -> Result<FileDiff, String> {
        let repo = open_repo(&repo_path)?;
        let head_tree = repo.head().and_then(|h| h.peel_to_tree()).ok();
        let base = head_tree
            .as_ref()
            .and_then(|t| tree_blob(t, &path))
            .map(|id| blob_to_string(&repo, id))
            .unwrap_or_else(|| Ok(String::new()))?;
        let target = resolve_in_workdir(&repo, &path)
            .ok()
            .filter(|p| p.exists())
            .and_then(|p| std::fs::read(p).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let rows = diff_rows(&base, &target)?;
        Ok(FileDiff { path, rows })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?;
    diff
}

/// Side-by-side diff of one file inside a commit (parent tree vs commit tree).
///
/// The LCS is O(n*m): the whole computation runs off the main thread so a
/// huge file cannot freeze the window while it computes.
#[tauri::command]
pub(crate) async fn get_commit_file_diff(
    repo_path: String,
    commit_id: String,
    path: String,
) -> Result<FileDiff, String> {
    let diff = tauri::async_runtime::spawn_blocking(move || -> Result<FileDiff, String> {
        let repo = open_repo(&repo_path)?;
        let oid: git2::Oid = commit_id
            .parse()
            .map_err(|e| format!("bad commit id: {e}"))?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let new_tree = commit.tree().map_err(|e| e.to_string())?;
        let old_tree = if commit.parent_count() > 0 {
            Some(
                commit
                    .parent(0)
                    .map_err(|e| e.to_string())?
                    .tree()
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };

        let old_content = old_tree
            .as_ref()
            .and_then(|t| tree_blob(t, &path))
            .map(|id| blob_to_string(&repo, id))
            .unwrap_or_else(|| Ok(String::new()))?;
        let new_content = tree_blob(&new_tree, &path)
            .map(|id| blob_to_string(&repo, id))
            .unwrap_or_else(|| Ok(String::new()))?;

        let rows = diff_rows(&old_content, &new_content)?;
        Ok(FileDiff { path, rows })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?;
    diff
}

#[cfg(test)]
mod tests {
    use super::{
        align_rows, char_diff_range, diff_rows, elided_text, lcs_matrix, window_ops, DiffRow,
        Scratch, DIFF_CONTEXT, ELIDED, MAX_HIGHLIGHT_DP_CELLS, Op,
    };
    use std::borrow::Cow;

    // ── helpers ──

    fn text<'b>(t: &'b Cow<'_, str>) -> &'b str {
        &**t
    }

    /// The two files of a `lines`-line file with exactly one edited line in the
    /// middle — the shape the window is supposed to make cheap.
    fn one_change(lines: usize) -> (String, String) {
        let base: Vec<String> = (0..lines).map(|i| format!("line-{i}")).collect();
        let mut target = base.clone();
        target[lines / 2] = "the only changed line".to_string();
        (base.join("\n"), target.join("\n"))
    }

    /// `pairs` line pairs that differ in one word each.
    fn many_modified(pairs: usize) -> (String, String) {
        let base: Vec<String> = (0..pairs)
            .map(|i| format!("    set_value(name_{i}, {i}); // trailing comment"))
            .collect();
        let target: Vec<String> = (0..pairs)
            .map(|i| format!("    set_value(name_{i}, {}); // trailing comment", i + 1000))
            .collect();
        (base.join("\n"), target.join("\n"))
    }

    /// Char ranges of a modified row, the way [`align_rows`] asks for them.
    fn spans(line: &str, other: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        char_diff_range(line, other, &mut Scratch::default(), &mut out);
        out
    }

    fn ops_lines(ops: &[(char, &str)]) -> String {
        ops.iter()
            .map(|(k, t)| format!("{k}{t}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ── diff：公共前后缀裁剪 + DP 规模上限 ──

    #[test]
    fn diff_output_matches_naive_shape() {
        let base = "a\nb\nc";
        let target = "a\nX\nc";
        let ops = super::diff_strings(base, target).unwrap();
        assert_eq!(ops_lines(&ops), " a\n-b\n+X\n c");
    }

    #[test]
    fn identical_files_are_all_context() {
        let ops = super::diff_strings("a\nb", "a\nb").unwrap();
        assert_eq!(ops_lines(&ops), " a\n b");
    }

    #[test]
    fn huge_changed_region_is_rejected_not_allocated() {
        // 前后缀裁剪后差异中段仍超预算：必须返回错误而不是分配 O(n·m) 矩阵。
        let big: Vec<String> = (0..4000).map(|i| format!("line-{i}")).collect();
        let base = big.join("\n");
        let target = big.iter().rev().cloned().collect::<Vec<_>>().join("\n");
        let err = match diff_rows(&base, &target) {
            Ok(_) => panic!("中段差异超预算时必须拒绝，不能分配 O(n·m) 矩阵"),
            Err(e) => e,
        };
        assert!(err.contains("文件差异过大"), "got: {err}");
    }

    #[test]
    fn small_change_in_big_file_is_cheap() {
        // 1 万行文件改 1 行：裁剪后 DP 只覆盖几行，不再触发上限。
        let (base, target) = one_change(10_000);
        let ops = super::diff_strings(&base, &target).unwrap();
        let changed: Vec<_> = ops.iter().filter(|(k, _)| *k != ' ').collect();
        assert_eq!(changed.len(), 2);
    }

    // ── diff：上下文窗口 ──

    #[test]
    fn huge_file_window_shrinks_to_the_change() {
        // 1 万行文件改 1 行：前缀裁剪让 DP 只算 1 行，但输出仍带全部上下文——
        // 窗口把整条输出收到 2×DIFF_CONTEXT 的邻行加两条省略标记。
        let (base, target) = one_change(10_000);
        let ops = window_ops(super::diff_strings(&base, &target).unwrap());

        assert!(
            ops.len() <= DIFF_CONTEXT * 2 + 2 + 2,
            "windowed to {} rows for one changed line",
            ops.len()
        );
        assert_eq!(
            ops.iter().filter(|(k, _)| matches!(k, '-' | '+')).count(),
            2
        );
        let elided: Vec<_> = ops.iter().filter(|(k, _)| *k == ELIDED).collect();
        assert_eq!(elided.len(), 2, "ops = {ops:?}");
        // 窗口要贴着改动：保留改动的直接邻行，而不是文件的头尾。
        let ctx: Vec<&str> = ops
            .iter()
            .filter(|(k, _)| *k == ' ')
            .map(|(_, t)| text(t))
            .collect();
        assert_eq!(
            ctx,
            vec![
                "line-4997",
                "line-4998",
                "line-4999",
                "line-5001",
                "line-5002",
                "line-5003"
            ]
        );
        // 省掉的对得上：6 行上下文 + 1 行改动 = 7 行，其余 9993 行被折叠。
        assert!(
            elided[0].1.contains("4997"),
            "head elision = {}",
            elided[0].1
        );
        assert!(
            elided[1].1.contains("4996"),
            "tail elision = {}",
            elided[1].1
        );
        // 守恒：显示的行 + 折叠的行必须等于原文件行数，一行都不许丢。
        let elided_lines: usize = elided
            .iter()
            .map(|(_, t)| {
                text(t)
                    .trim_start_matches("已省略 ")
                    .trim_end_matches(" 行")
                    .parse::<usize>()
                    .expect("elision marker is parseable")
            })
            .sum();
        assert_eq!(elided_lines + ctx.len() + 1, 10_000);
    }

    #[test]
    fn two_distant_changes_elide_only_the_middle() {
        let mut base: Vec<String> = (0..200).map(|i| format!("line-{i}")).collect();
        let mut target = base.clone();
        target[10] = "A'".into();
        target[190] = "Z'".into();
        base[10] = "A".into();
        base[190] = "Z".into();
        let (base, target) = (base.join("\n"), target.join("\n"));
        let ops = window_ops(super::diff_strings(&base, &target).unwrap());

        let elided: Vec<_> = ops.iter().filter(|(k, _)| *k == ELIDED).collect();
        // 头部、中段、尾部各一条标记；中段是真正被折叠的部分。
        assert_eq!(elided.len(), 3, "ops = {ops:?}");
        assert!(
            elided[1].1.contains("173"),
            "middle elision = {}",
            elided[1].1
        );
        let ctx: Vec<&str> = ops
            .iter()
            .filter(|(k, _)| *k == ' ')
            .map(|(_, t)| text(t))
            .collect();
        assert_eq!(ctx.len(), DIFF_CONTEXT * 4);
        assert_eq!(
            ctx,
            vec![
                "line-7", "line-8", "line-9", "line-11", "line-12", "line-13", "line-187",
                "line-188", "line-189", "line-191", "line-192", "line-193"
            ]
        );
    }

    #[test]
    fn short_boundary_runs_elide_their_extra() {
        // 头尾上下文不足 2×DIFF_CONTEXT 时只留紧邻改动的那几条。
        let base = "a\nb\n1\n2\n3\n4\n5\n6\n7\n8\n9\nc";
        let target = "a\nb\n1\n2\n3\nX\n5\n6\n7\n8\n9\nc";
        let ops = window_ops(super::diff_strings(base, target).unwrap());

        let ctx: Vec<&str> = ops
            .iter()
            .filter(|(k, _)| *k == ' ')
            .map(|(_, t)| text(t))
            .collect();
        assert_eq!(ctx, vec!["1", "2", "3", "5", "6", "7"]);
        let elided: Vec<_> = ops.iter().filter(|(k, _)| *k == ELIDED).collect();
        assert_eq!(elided.len(), 2, "ops = {ops:?}");
        assert!(elided[0].1.contains("2"), "head elision = {}", elided[0].1);
        assert!(elided[1].1.contains("3"), "tail elision = {}", elided[1].1);
    }

    #[test]
    fn identical_files_bypass_the_window() {
        // 没有改动就没有可依附的窗口；省略标记会谎报一个本没有差异的 diff。
        let file: String = (0..5000)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let ops = window_ops(super::diff_strings(&file, &file).unwrap());
        assert_eq!(ops.len(), 5000);
        assert!(ops.iter().all(|(k, _)| *k == ' '));
    }

    #[test]
    fn elided_marker_becomes_its_own_row() {
        // 省略标记要和 context 行一样进 align_rows，不能被并进一个 '+' 行。
        let ops: Vec<Op> = vec![
            (' ', Cow::Borrowed("keep")),
            (ELIDED, Cow::Owned("已省略 3 行".to_string())),
            ('+', Cow::Borrowed("new")),
        ];
        let rows = align_rows(&ops);
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(kinds, vec!["context", "elided", "added"]);
        let elided = &rows[1];
        assert!(elided.left.is_some());
        assert!(elided.right.is_none());
    }

    // ── diff：窗口之后的两段"不复制"契约 ──

    // The window picks a dozen lines out of the file; materializing every line
    // on the way there made the cost Θ(file) heap allocations whatever the
    // answer was. Four times the file with the same window must not cost four
    // times the allocations — it must cost the same. Measured with the window
    // ON: 95 allocations for 5k lines vs 101 for 20k. Remove the borrowing
    // (own every line before the window, as at HEAD) and this goes red: 5,104
    // vs 20,110 — the +15,006 tracks the file, not the answer.
    #[test]
    fn the_window_is_paid_for_by_the_window_not_by_the_file() {
        static ALLOCS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let small = one_change(5_000);
        let big = one_change(20_000);

        let (rows_small, allocs_small) = {
            let m = crate::alloc_count::measure(&ALLOCS);
            let r = diff_rows(&small.0, &small.1).unwrap();
            (r.len(), m.count())
        };
        let (rows_big, allocs_big) = {
            let m = crate::alloc_count::measure(&ALLOCS);
            let r = diff_rows(&big.0, &big.1).unwrap();
            (r.len(), m.count())
        };

        assert_eq!(
            rows_small, rows_big,
            "the same edit shows the same number of rows whatever the file size"
        );
        assert!(
            allocs_big < allocs_small + 100,
            "4× the file, same window: {allocs_small} vs {allocs_big} allocations"
        );
    }

    // The inline highlight walk needed about ten heap buffers per changed row.
    // One [`Scratch`] is now shared by the whole diff, so an extra row may cost
    // only what the row itself sends. Measured with reuse ON: 4.0 allocations
    // per row (totals 857 for 200 rows / 2467 for 600). Hand each row a fresh
    // scratch, as at HEAD, and this goes red at 26.0 per row (5240/15650).
    #[test]
    fn the_highlight_scratch_is_reused_across_rows() {
        static ALLOCS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let few = many_modified(200);
        let many = many_modified(600);

        let allocs_few = {
            let m = crate::alloc_count::measure(&ALLOCS);
            let r = diff_rows(&few.0, &few.1).unwrap();
            assert_eq!(r.len(), 200);
            m.count()
        };
        let allocs_many = {
            let m = crate::alloc_count::measure(&ALLOCS);
            let r = diff_rows(&many.0, &many.1).unwrap();
            assert_eq!(r.len(), 600);
            m.count()
        };

        let extra_rows = 400;
        let per_row = (allocs_many - allocs_few) as f64 / extra_rows as f64;
        assert!(
            per_row < 12.0,
            "{per_row:.1} allocations per added row (was ~30); total {allocs_few}/{allocs_many}"
        );
    }

    // The line DP used to be a `Vec<Vec<usize>>`: one heap buffer per row on top
    // of worse locality (CSAPP 6.1 — the row walk jumps between 501 separate
    // blocks instead of one). A flat `Vec<u32>` is a single allocation for any
    // matrix (measured: 1 for 500×500), and u32 suffices because a cell holds
    // an LCS length, at most `min(n, m)` ≤ √6M ≈ 2449 under the cell budget.
    // Rebuild the matrix row by row (`vec![vec![0u32; m + 1]; n + 1]` + flatten)
    // and this goes red with 512 allocations for the same output.
    #[test]
    fn the_line_matrix_is_one_flat_allocation() {
        static ALLOCS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let lines: Vec<String> = (0..1_000).map(|i| format!("a-{i}")).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let half = &refs[..500];

        let (dp, allocs) = {
            let m = crate::alloc_count::measure(&ALLOCS);
            let dp = lcs_matrix(half, half);
            (dp, m.count())
        };
        assert!(
            allocs <= 2,
            "a 500×500 matrix must be one buffer, not one per row: {allocs} allocations"
        );
        assert_eq!(
            dp[0], 500,
            "the two halves are identical, so their LCS is all of them"
        );

        // And the cell it pins is the LCS length, not something row-major-shifted.
        let a = ["x", "y", "z"];
        let b = ["y", "x", "z"];
        assert_eq!(lcs_matrix(&a, &b)[0], 2);
        assert_eq!(lcs_matrix(&a, &[])[0], 0);
    }

    // Every line the rows show must still be the file's own text: borrowing is
    // only legal while nothing was rewritten on the way.
    #[test]
    fn windowed_rows_carry_the_files_own_lines() {
        let (base, target) = one_change(2_000);
        let rows = diff_rows(&base, &target).unwrap();
        let left: Vec<&str> = rows
            .iter()
            .filter(|r| r.kind == "context")
            .map(|r| r.left.as_ref().unwrap().text.as_str())
            .collect();
        assert_eq!(
            left,
            vec![
                "line-997",
                "line-998",
                "line-999",
                "line-1001",
                "line-1002",
                "line-1003"
            ]
        );
        let modified = rows.iter().find(|r| r.kind == "modified").unwrap();
        assert_eq!(modified.left.as_ref().unwrap().text, "line-1000");
        assert_eq!(modified.right.as_ref().unwrap().text, "the only changed line");
    }

    // ── diff：行内高亮区间按 UTF-16 码元计（前端 String.slice 的单位）──

    #[test]
    fn ascii_highlights_unchanged() {
        assert_eq!(spans("hello world", "hello there"), vec![(6, 11)]);
        assert_eq!(spans("same", "same"), Vec::new());
    }

    #[test]
    fn cjk_highlights_use_utf16_not_bytes() {
        // “价格：100 元”→“价格：200 元”：差异是第 4 个字符。
        // 字节区间是 (9,12)，UTF-16 码元区间是 (3,4) —— 前端按码元切分。
        assert_eq!(spans("价格：100 元", "价格：200 元"), vec![(3, 4)]);
    }

    #[test]
    fn highlights_never_split_a_multibyte_char() {
        // “你”与“我”字节前缀部分相同：字节级比较会把字符从中间切开。
        assert_eq!(spans("你好", "我好"), vec![(0, 1)]);
    }

    #[test]
    fn surrogate_pairs_count_as_two_utf16_units() {
        // 🎉 是 1 个字符、2 个 UTF-16 码元、4 个字节；差异段必须横跨 2 个码元。
        assert_eq!(spans("a🎉b", "a🎊b"), vec![(1, 3)]);
    }

    #[test]
    fn distant_edits_highlight_separately() {
        // 两处远距修改：旧算法（公共前后缀裁剪）会把中间未变的
        // "; bar = 2; baz = " 一起标成差异；词级 LCS 只标 "1" 和 "3"。
        assert_eq!(
            spans("foo = 1; bar = 2; baz = 3;", "foo = 99; bar = 2; baz = 42;"),
            vec![(6, 7), (24, 25)]
        );
    }

    #[test]
    fn distant_edits_highlight_separately_with_cjk_between() {
        // 中段含多字节字符时区间仍按 UTF-16 码元计。
        assert_eq!(spans("x=1;中=2y", "x=9;中=8y"), vec![(2, 3), (6, 7)]);
    }

    // ── diff：新管线与 HEAD 管线的对拍 oracle ──

    // The three optimizations in this round (borrowed op stream, flat u32 DP,
    // reused highlight scratch) are all meant to be behavior-preserving. The
    // tests above pin shapes, but a shape test passes for the wrong output too
    // — so the pre-optimization pipeline is vendored here verbatim from HEAD
    // and every corpus file is run through both. Any divergence fails the
    // byte-for-byte row signature below.

    fn old_tokenize(chars: &[char]) -> Vec<(usize, usize)> {
        let mut toks = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let start = i;
            let c = chars[i];
            if c.is_alphanumeric() || c == '_' {
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
            } else if c.is_whitespace() {
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
            } else {
                i += 1;
            }
            toks.push((start, i - start));
        }
        toks
    }

    fn old_lcs_char_spans(a: &[char], b: &[char]) -> Vec<(usize, usize)> {
        let at = old_tokenize(a);
        let bt = old_tokenize(b);
        let n = at.len();
        let m = bt.len();
        let mut dir = vec![0u8; (n + 1) * (m + 1)];
        let mut prev = vec![0u16; m + 1];
        for i in 1..=n {
            let mut cur = vec![0u16; m + 1];
            let a_tok = &at[i - 1];
            for j in 1..=m {
                let b_tok = &bt[j - 1];
                if a[a_tok.0..a_tok.0 + a_tok.1] == b[b_tok.0..b_tok.0 + b_tok.1] {
                    cur[j] = prev[j - 1] + 1;
                    dir[i * (m + 1) + j] = 1;
                } else if prev[j] >= cur[j - 1] {
                    cur[j] = prev[j];
                    dir[i * (m + 1) + j] = 2;
                } else {
                    cur[j] = cur[j - 1];
                    dir[i * (m + 1) + j] = 3;
                }
            }
            prev = cur;
        }
        let mut flagged = vec![false; n];
        let (mut i, mut j) = (n, m);
        while i > 0 && j > 0 {
            match dir[i * (m + 1) + j] {
                1 => {
                    i -= 1;
                    j -= 1;
                }
                2 => {
                    flagged[i - 1] = true;
                    i -= 1;
                }
                _ => j -= 1,
            }
        }
        while i > 0 {
            flagged[i - 1] = true;
            i -= 1;
        }
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut k = 0;
        while k < n {
            if flagged[k] {
                let start = at[k].0;
                let mut end = at[k].0 + at[k].1;
                while k + 1 < n && flagged[k + 1] {
                    k += 1;
                    end = at[k].0 + at[k].1;
                }
                spans.push((start, end));
            }
            k += 1;
        }
        spans
    }

    fn old_char_diff_range(line: &str, other: &str) -> Vec<(usize, usize)> {
        let line_chars: Vec<char> = line.chars().collect();
        let other_chars: Vec<char> = other.chars().collect();
        let mut common_prefix = 0usize;
        while common_prefix < line_chars.len()
            && common_prefix < other_chars.len()
            && line_chars[common_prefix] == other_chars[common_prefix]
        {
            common_prefix += 1;
        }
        let mut common_suffix = 0usize;
        while common_suffix < line_chars.len() - common_prefix
            && common_suffix < other_chars.len() - common_prefix
            && line_chars[line_chars.len() - 1 - common_suffix]
                == other_chars[other_chars.len() - 1 - common_suffix]
        {
            common_suffix += 1;
        }
        let line_end = line_chars.len() - common_suffix;
        if common_prefix >= line_end {
            return Vec::new();
        }
        let other_end = other_chars.len() - common_suffix;
        let mid_spans: Vec<(usize, usize)> = if old_tokenize(&line_chars[common_prefix..line_end])
            .len()
            .saturating_mul(old_tokenize(&other_chars[common_prefix..other_end]).len())
            > MAX_HIGHLIGHT_DP_CELLS
        {
            vec![(common_prefix, line_end)]
        } else {
            old_lcs_char_spans(
                &line_chars[common_prefix..line_end],
                &other_chars[common_prefix..other_end],
            )
            .into_iter()
            .map(|(s, e)| (s + common_prefix, e + common_prefix))
            .collect()
        };
        let mut cu: Vec<usize> = Vec::with_capacity(line_chars.len() + 1);
        cu.push(0);
        for c in &line_chars {
            let next = *cu.last().unwrap() + c.len_utf16();
            cu.push(next);
        }
        mid_spans
            .into_iter()
            .filter(|(s, e)| e > s)
            .map(|(s, e)| (cu[s], cu[e]))
            .collect()
    }

    fn old_window_ops(ops: Vec<(char, String)>) -> Vec<(char, String)> {
        let mut segs: Vec<(bool, usize, usize)> = Vec::new();
        let mut i = 0;
        while i < ops.len() {
            let is_context = ops[i].0 == ' ';
            let from = i;
            while i < ops.len() && (ops[i].0 == ' ') == is_context {
                i += 1;
            }
            segs.push((is_context, from, i - from));
        }
        if !segs.iter().any(|(is_context, _, _)| !*is_context) {
            return ops;
        }
        let mut out: Vec<(char, String)> = Vec::new();
        for (idx, (is_context, from, len)) in segs.iter().enumerate() {
            let body = &ops[*from..*from + *len];
            if !*is_context {
                out.extend_from_slice(body);
                continue;
            }
            let keep = DIFF_CONTEXT.min(*len);
            if idx == 0 {
                let skip = *len - keep;
                if skip > 0 {
                    out.push((ELIDED, elided_text(skip)));
                }
                out.extend_from_slice(&body[skip..]);
            } else if idx + 1 == segs.len() {
                out.extend_from_slice(&body[..keep]);
                let skip = *len - keep;
                if skip > 0 {
                    out.push((ELIDED, elided_text(skip)));
                }
            } else {
                let head = keep;
                let tail = DIFF_CONTEXT.min(*len - head);
                let middle = *len - head - tail;
                out.extend_from_slice(&body[..head]);
                if middle > 0 {
                    out.push((ELIDED, elided_text(middle)));
                }
                out.extend_from_slice(&body[*len - tail..]);
            }
        }
        out
    }

    fn old_align_rows(ops: Vec<(char, String)>) -> Vec<DiffRow> {
        let mut rows = Vec::new();
        let mut i = 0;
        while i < ops.len() {
            let (kind, text) = &ops[i];
            if *kind == ' ' {
                rows.push(super::DiffRow {
                    kind: "context".to_string(),
                    left: Some(super::DiffSegment {
                        text: text.clone(),
                        highlights: Vec::new(),
                    }),
                    right: Some(super::DiffSegment {
                        text: text.clone(),
                        highlights: Vec::new(),
                    }),
                });
                i += 1;
            } else if *kind == ELIDED {
                rows.push(super::DiffRow {
                    kind: "elided".to_string(),
                    left: Some(super::DiffSegment {
                        text: text.clone(),
                        highlights: Vec::new(),
                    }),
                    right: None,
                });
                i += 1;
            } else {
                let mut removed: Vec<String> = Vec::new();
                let mut added: Vec<String> = Vec::new();
                while i < ops.len() && ops[i].0 != ' ' {
                    match ops[i].0 {
                        '-' => removed.push(ops[i].1.clone()),
                        '+' => added.push(ops[i].1.clone()),
                        _ => {}
                    }
                    i += 1;
                }
                let pairs = removed.len().max(added.len());
                for idx in 0..pairs {
                    let rm = removed.get(idx);
                    let ad = added.get(idx);
                    let row = match (rm, ad) {
                        (Some(r), Some(a)) => {
                            let rh = old_char_diff_range(r, a);
                            let ah = old_char_diff_range(a, r);
                            super::DiffRow {
                                kind: "modified".to_string(),
                                left: Some(super::DiffSegment {
                                    text: r.clone(),
                                    highlights: rh
                                        .into_iter()
                                        .map(|(s, e)| super::Highlight { start: s, end: e })
                                        .collect(),
                                }),
                                right: Some(super::DiffSegment {
                                    text: a.clone(),
                                    highlights: ah
                                        .into_iter()
                                        .map(|(s, e)| super::Highlight { start: s, end: e })
                                        .collect(),
                                }),
                            }
                        }
                        (Some(r), None) => super::DiffRow {
                            kind: "removed".to_string(),
                            left: Some(super::DiffSegment {
                                text: r.clone(),
                                highlights: Vec::new(),
                            }),
                            right: None,
                        },
                        (None, Some(a)) => super::DiffRow {
                            kind: "added".to_string(),
                            left: None,
                            right: Some(super::DiffSegment {
                                text: a.clone(),
                                highlights: Vec::new(),
                            }),
                        },
                        (None, None) => continue,
                    };
                    rows.push(row);
                }
            }
        }
        rows
    }

    fn old_diff_strings(base: &str, target: &str) -> Result<Vec<(char, String)>, String> {
        let a_full: Vec<&str> = base.split('\n').collect();
        let b_full: Vec<&str> = target.split('\n').collect();
        let mut start = 0usize;
        while start < a_full.len() && start < b_full.len() && a_full[start] == b_full[start] {
            start += 1;
        }
        let mut end_a = a_full.len();
        let mut end_b = b_full.len();
        while end_a > start && end_b > start && a_full[end_a - 1] == b_full[end_b - 1] {
            end_a -= 1;
            end_b -= 1;
        }
        let a = &a_full[start..end_a];
        let b = &b_full[start..end_b];
        let n = a.len();
        let m = b.len();
        const OLD_MAX_DP_CELLS: u64 = 6_000_000;
        if (n as u64).saturating_mul(m as u64) > OLD_MAX_DP_CELLS {
            return Err(format!(
                "文件差异过大（{n} 行 × {m} 行），无法生成行级 diff，请在终端用 git diff 查看"
            ));
        }
        let mut dp = vec![vec![0usize; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i][j] = if a[i] == b[j] {
                    dp[i + 1][j + 1] + 1
                } else {
                    dp[i + 1][j].max(dp[i][j + 1])
                };
            }
        }
        let mut out: Vec<(char, String)> = Vec::new();
        for line in &a_full[..start] {
            out.push((' ', (*line).to_string()));
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a[i] == b[j] {
                out.push((' ', a[i].to_string()));
                i += 1;
                j += 1;
            } else if dp[i + 1][j] >= dp[i][j + 1] {
                out.push(('-', a[i].to_string()));
                i += 1;
            } else {
                out.push(('+', b[j].to_string()));
                j += 1;
            }
        }
        while i < n {
            out.push(('-', a[i].to_string()));
            i += 1;
        }
        while j < m {
            out.push(('+', b[j].to_string()));
            j += 1;
        }
        for line in &a_full[end_a..] {
            out.push((' ', (*line).to_string()));
        }
        Ok(out)
    }

    /// Byte-for-byte signature of what the frontend would receive: row kind,
    /// each column's text and its highlight ranges, in order.
    fn rows_sig(rows: &[DiffRow]) -> String {
        let mut s = String::new();
        for r in rows {
            s.push_str(&r.kind);
            for seg in [&r.left, &r.right] {
                match seg {
                    Some(seg) => {
                        s.push_str(" | ");
                        s.push_str(&seg.text);
                        for h in &seg.highlights {
                            s.push_str(&format!(" ^{}:{}", h.start, h.end));
                        }
                    }
                    None => s.push_str(" | ~"),
                }
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn new_pipeline_matches_the_head_pipeline() {
        // A corpus that walks every branch the pipeline owns: empty files,
        // pure add/remove blocks, imbalanced pairs, windowed runs of both
        // boundary and interior shapes, the no-change bypass, UTF-16 offsets
        // (CJK, surrogates), the coarse-span budget fallback inside a line,
        // the rejection error, and a line whose own text is the ELIDED marker.
        let long_tokens = |shift: usize| -> String {
            (0..1200)
                .map(|i| format!("t{}", i + shift))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut corpus: Vec<(String, String)> = vec![
            ("a\nb\nc".into(), "a\nX\nc".into()),
            ("".into(), "".into()),
            ("".into(), "a".into()),
            ("a".into(), "".into()),
            ("a\nb\nc".into(), "a\nb\nc\nd\ne".into()),
            ("a\nb\nX\nY\nc".into(), "a\nb\nc".into()),
            ("a\nb\nc".into(), "a\nb\nX\nY\nc".into()),
            ("one\ntwo\nthree".into(), "ONE\n2\nIII\nmore".into()),
            ("a\n\nb".into(), "a\n\nc".into()),
            ("价格：100 元".into(), "价格：200 元".into()),
            ("a🎉b".into(), "a🎊b".into()),
            (
                "foo = 1; bar = 2; baz = 3;".into(),
                "foo = 99; bar = 2; baz = 42;".into(),
            ),
            ("a\n¦b\nc".into(), "a\nX\nc".into()),
            ("a\nb\n1\n2\n3\n4\n5\n6\n7\n8\n9\nc".into(), "a\nb\n1\n2\n3\nX\n5\n6\n7\n8\n9\nc".into()),
            // Coarse-span fallback: the trimmed middle exceeds the token budget.
            (long_tokens(0), long_tokens(600)),
        ];
        let (b, t) = one_change(10_000);
        corpus.push((b, t));
        let (b, t) = many_modified(50);
        corpus.push((b, t));
        let mut base: Vec<String> = (0..200).map(|i| format!("line-{i}")).collect();
        let mut target = base.clone();
        base[10] = "A".into();
        base[190] = "Z".into();
        target[10] = "A'".into();
        target[190] = "Z'".into();
        corpus.push((base.join("\n"), target.join("\n")));
        let file: String = (0..3000)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        corpus.push((file.clone(), file));
        let big: Vec<String> = (0..4000).map(|i| format!("line-{i}")).collect();
        corpus.push((
            big.join("\n"),
            big.iter().rev().cloned().collect::<Vec<_>>().join("\n"),
        ));

        for (idx, (base, target)) in corpus.iter().enumerate() {
            let new_out = diff_rows(base, target)
                .map(|rows| rows_sig(&rows))
                .unwrap_or_else(|e| format!("|ERR {e}"));
            // The rejection error is compared too, not unwrapped away.
            let old_out = old_diff_strings(base, target)
                .map(old_window_ops)
                .map(old_align_rows)
                .map(|rows| rows_sig(&rows))
                .unwrap_or_else(|e| format!("|ERR {e}"));
            assert_eq!(
                new_out, old_out,
                "case {idx}: new pipeline diverged from the HEAD pipeline"
            );
        }
    }

    // The reused buffers must not leak a previous row's tail into the next one:
    // each row clears what it re-fills.
    #[test]
    fn a_long_row_followed_by_a_short_one_reports_only_the_short_one() {
        let mut scratch = Scratch::default();
        let mut out = Vec::new();
        let long_a = "x = 1; ".to_string() + &"y ".repeat(200);
        let long_b = "x = 2; ".to_string() + &"y ".repeat(200);
        char_diff_range(&long_a, &long_b, &mut scratch, &mut out);
        assert_eq!(out, vec![(4, 5)]);
        char_diff_range("aa", "bb", &mut scratch, &mut out);
        assert_eq!(out, vec![(0, 2)]);
        char_diff_range("same text", "same text", &mut scratch, &mut out);
        assert_eq!(out, Vec::new());
    }
}
