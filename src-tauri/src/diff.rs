//! Line/word diff engine and the diff commands.

use serde::Serialize;

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

/// LCS DP budget for inline highlighting: if the token counts of the two
/// trimmed middles multiply past this, fall back to one coarse span instead
/// of allocating an O(m·n) matrix for a pathological single line.
const MAX_HIGHLIGHT_DP_CELLS: usize = 250_000;

/// Tokenize into maximal runs: word (alnum + `_`), whitespace, or one
/// punctuation char. Returns (start_char, len) into `chars`; the runs tile
/// the slice contiguously, so adjacent flagged tokens merge into one span.
fn tokenize(chars: &[char]) -> Vec<(usize, usize)> {
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

/// Char ranges (relative to `a`) of tokens not in the LCS of `a`/`b` token
/// sequences — the word-level diff that lets one row carry several disjoint
/// highlights instead of one span swallowing the unchanged middle.
fn lcs_char_spans(a: &[char], b: &[char]) -> Vec<(usize, usize)> {
    let at = tokenize(a);
    let bt = tokenize(b);
    let n = at.len();
    let m = bt.len();
    // dir[i][j]: 1 = match (diagonal), 2 = up (a-token unmatched), 3 = left
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

/// Return the ranges of `line` that differ from `other` — several minimal
/// word-level spans rather than one coarse span, so two distant edits don't
/// highlight the unchanged text between them — as UTF-16 code-unit offsets.
/// JS `String.slice` on the frontend indexes by UTF-16 code units, and a
/// byte- or char-indexed range would misalign as soon as the line contains
/// non-BMP or multi-byte characters. Comparing per char (not per byte) also
/// guarantees a span never splits inside a multi-byte character.
fn char_diff_range(line: &str, other: &str) -> Vec<(usize, usize)> {
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
    let mid_spans: Vec<(usize, usize)> = if tokenize(&line_chars[common_prefix..line_end])
        .len()
        .saturating_mul(tokenize(&other_chars[common_prefix..other_end]).len())
        > MAX_HIGHLIGHT_DP_CELLS
    {
        vec![(common_prefix, line_end)]
    } else {
        lcs_char_spans(
            &line_chars[common_prefix..line_end],
            &other_chars[common_prefix..other_end],
        )
        .into_iter()
        .map(|(s, e)| (s + common_prefix, e + common_prefix))
        .collect()
    };
    // char offset -> UTF-16 code-unit offset (cumulative prefix lengths)
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
fn window_ops(ops: Vec<(char, String)>) -> Vec<(char, String)> {
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

/// Split a linear diff op stream into aligned row pairs.
fn align_rows(ops: Vec<(char, String)>) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        let (kind, text) = &ops[i];
        if *kind == ' ' {
            rows.push(DiffRow {
                kind: "context".to_string(),
                left: Some(DiffSegment {
                    text: text.clone(),
                    highlights: Vec::new(),
                }),
                right: Some(DiffSegment {
                    text: text.clone(),
                    highlights: Vec::new(),
                }),
            });
            i += 1;
        } else if *kind == ELIDED {
            // One row spanning both columns, not a line pair: it names the run
            // that was left out rather than showing it.
            rows.push(DiffRow {
                kind: "elided".to_string(),
                left: Some(DiffSegment {
                    text: text.clone(),
                    highlights: Vec::new(),
                }),
                right: None,
            });
            i += 1;
        } else {
            // Gather a change block: consecutive '-' and '+'.
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
            // Pair removed/added by index as modified rows.
            let pairs = removed.len().max(added.len());
            for idx in 0..pairs {
                let rm = removed.get(idx);
                let ad = added.get(idx);
                let row = match (rm, ad) {
                    (Some(r), Some(a)) => {
                        let rh = char_diff_range(r, a);
                        let ah = char_diff_range(a, r);
                        DiffRow {
                            kind: "modified".to_string(),
                            left: Some(DiffSegment {
                                text: r.clone(),
                                highlights: rh
                                    .into_iter()
                                    .map(|(s, e)| Highlight { start: s, end: e })
                                    .collect(),
                            }),
                            right: Some(DiffSegment {
                                text: a.clone(),
                                highlights: ah
                                    .into_iter()
                                    .map(|(s, e)| Highlight { start: s, end: e })
                                    .collect(),
                            }),
                        }
                    }
                    (Some(r), None) => DiffRow {
                        kind: "removed".to_string(),
                        left: Some(DiffSegment {
                            text: r.clone(),
                            highlights: Vec::new(),
                        }),
                        right: None,
                    },
                    (None, Some(a)) => DiffRow {
                        kind: "added".to_string(),
                        left: None,
                        right: Some(DiffSegment {
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

fn diff_strings(base: &str, target: &str) -> Result<Vec<(char, String)>, String> {
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

    // The DP matrix is (n+1)×(m+1) usizes; without a budget a pair of large
    // files allocates hundreds of MB and blocks the webview. 6M cells ≈ 48MB
    // is the hard ceiling after prefix/suffix trimming.
    const MAX_DP_CELLS: u64 = 6_000_000;
    if (n as u64).saturating_mul(m as u64) > MAX_DP_CELLS {
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
    // Context lines before the first difference.
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
    // Context lines after the last difference.
    for line in &a_full[end_a..] {
        out.push((' ', (*line).to_string()));
    }
    Ok(out)
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
        let rows = align_rows(window_ops(diff_strings(&base, &target)?));
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

        let ops = diff_strings(&old_content, &new_content)?;
        let rows = align_rows(window_ops(ops));
        Ok(FileDiff { path, rows })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?;
    diff
}

#[cfg(test)]
mod tests {
    use super::{align_rows, char_diff_range, diff_strings, window_ops, DIFF_CONTEXT, ELIDED};

    // ── diff：公共前后缀裁剪 + DP 规模上限 ──

    fn ops_lines(ops: &[(char, String)]) -> String {
        ops.iter()
            .map(|(k, t)| format!("{k}{t}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn diff_output_matches_naive_shape() {
        let base = "a\nb\nc";
        let target = "a\nX\nc";
        let ops = diff_strings(base, target).unwrap();
        assert_eq!(ops_lines(&ops), " a\n-b\n+X\n c");
    }

    #[test]
    fn identical_files_are_all_context() {
        let ops = diff_strings("a\nb", "a\nb").unwrap();
        assert_eq!(ops_lines(&ops), " a\n b");
    }

    #[test]
    fn huge_changed_region_is_rejected_not_allocated() {
        // 前后缀裁剪后差异中段仍超预算：必须返回错误而不是分配 O(n·m) 矩阵。
        let big: Vec<String> = (0..4000).map(|i| format!("line-{i}")).collect();
        let base = big.join("\n");
        let target = big.iter().rev().cloned().collect::<Vec<_>>().join("\n");
        let err = diff_strings(&base, &target).unwrap_err();
        assert!(err.contains("文件差异过大"), "got: {err}");
    }

    #[test]
    fn small_change_in_big_file_is_cheap() {
        // 1 万行文件改 1 行：裁剪后 DP 只覆盖几行，不再触发上限。
        let mut base: Vec<String> = (0..10_000).map(|i| format!("line-{i}")).collect();
        let mut target = base.clone();
        target[5_000] = "changed".into();
        base[5_000] = "orig".into();
        let ops = diff_strings(&base.join("\n"), &target.join("\n")).unwrap();
        let changed: Vec<_> = ops.iter().filter(|(k, _)| *k != ' ').collect();
        assert_eq!(changed.len(), 2);
    }

    // ── diff：上下文窗口 ──

    #[test]
    fn huge_file_window_shrinks_to_the_change() {
        // 1 万行文件改 1 行：前缀裁剪让 DP 只算 1 行，但输出仍带全部上下文——
        // 窗口把整条输出收到 2×DIFF_CONTEXT 的邻行加两条省略标记。
        let mut base: Vec<String> = (0..10_000).map(|i| format!("line-{i}")).collect();
        let mut target = base.clone();
        target[5_000] = "changed".into();
        base[5_000] = "orig".into();
        let ops = window_ops(diff_strings(&base.join("\n"), &target.join("\n")).unwrap());

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
            .map(|(_, t)| t.as_str())
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
                t.trim_start_matches("已省略 ")
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
        let ops = window_ops(diff_strings(&base.join("\n"), &target.join("\n")).unwrap());

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
            .map(|(_, t)| t.as_str())
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
        let ops = window_ops(diff_strings(base, target).unwrap());

        let ctx: Vec<&str> = ops
            .iter()
            .filter(|(k, _)| *k == ' ')
            .map(|(_, t)| t.as_str())
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
        let ops = window_ops(diff_strings(&file, &file).unwrap());
        assert_eq!(ops.len(), 5000);
        assert!(ops.iter().all(|(k, _)| *k == ' '));
    }

    #[test]
    fn elided_marker_becomes_its_own_row() {
        // 省略标记要和 context 行一样进 align_rows，不能被并进一个 '+' 行。
        let ops = vec![
            (' ', "keep".to_string()),
            (ELIDED, "已省略 3 行".to_string()),
            ('+', "new".to_string()),
        ];
        let rows = align_rows(ops);
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(kinds, vec!["context", "elided", "added"]);
        let elided = &rows[1];
        assert!(elided.left.is_some());
        assert!(elided.right.is_none());
    }

    // ── diff：行内高亮区间按 UTF-16 码元计（前端 String.slice 的单位）──

    #[test]
    fn ascii_highlights_unchanged() {
        assert_eq!(char_diff_range("hello world", "hello there"), vec![(6, 11)]);
        assert_eq!(char_diff_range("same", "same"), Vec::new());
    }

    #[test]
    fn cjk_highlights_use_utf16_not_bytes() {
        // “价格：100 元”→“价格：200 元”：差异是第 4 个字符。
        // 字节区间是 (9,12)，UTF-16 码元区间是 (3,4) —— 前端按码元切分。
        assert_eq!(
            char_diff_range("价格：100 元", "价格：200 元"),
            vec![(3, 4)]
        );
    }

    #[test]
    fn highlights_never_split_a_multibyte_char() {
        // “你”与“我”字节前缀部分相同：字节级比较会把字符从中间切开。
        assert_eq!(char_diff_range("你好", "我好"), vec![(0, 1)]);
    }

    #[test]
    fn surrogate_pairs_count_as_two_utf16_units() {
        // 🎉 是 1 个字符、2 个 UTF-16 码元、4 个字节；差异段必须横跨 2 个码元。
        assert_eq!(char_diff_range("a🎉b", "a🎊b"), vec![(1, 3)]);
    }

    #[test]
    fn distant_edits_highlight_separately() {
        // 两处远距修改：旧算法（公共前后缀裁剪）会把中间未变的
        // "; bar = 2; baz = " 一起标成差异；词级 LCS 只标 "1" 和 "3"。
        assert_eq!(
            char_diff_range("foo = 1; bar = 2; baz = 3;", "foo = 99; bar = 2; baz = 42;"),
            vec![(6, 7), (24, 25)]
        );
    }

    #[test]
    fn distant_edits_highlight_separately_with_cjk_between() {
        // 中段含多字节字符时区间仍按 UTF-16 码元计。
        assert_eq!(
            char_diff_range("x=1;中=2y", "x=9;中=8y"),
            vec![(2, 3), (6, 7)]
        );
    }
}
