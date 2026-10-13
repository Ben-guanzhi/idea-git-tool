//! Hunk-level staging: partial stage/unstage of a single file.
//!
//! The file-level commands in `working.rs` move a whole file between the
//! workdir and the index. These three commands move *one hunk at a time*.
//! The interesting part — line splitting, the LCS diff, the hunk grouping
//! and the apply/validate step — is pure functions over `Vec<String>` line
//! arrays (`split_lines`, `join_lines`, `diff_to_hunks`, `apply_hunks`) so
//! they can be unit-tested without a repository; the `#[tauri::command]`s
//! are thin shells that read both sides through git2 and write the result
//! back with `Index::add_frombuffer`. The working-tree file is never
//! touched: staging only ever rewrites an index entry (like `git add -p`).

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::repo::{
    bytes_are_binary, open_repo, resolve_in_workdir, validate_rel_path,
};

/// One contiguous delete+insert region of a line diff, without context
/// lines (the MVP groups only the changed runs; the frontend renders
/// context itself from `get_diff`). Positions are 0-based line numbers on
/// the respective side.
///
/// Line content excludes the trailing newline. A CRLF file keeps its `\r`
/// as part of the line content — the same convention `diff.rs` uses when
/// it splits on `'\n'` — so `join_lines(split_lines(t)) == t` for every
/// text `t` and staged blobs byte-match what the workdir held.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StagingHunk {
    /// 0-based index into the old side (index for staging, HEAD for unstaging).
    pub old_start: usize,
    /// 0-based index into the new side (workdir for staging, index for unstaging).
    pub new_start: usize,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
}

/// Returned when a hunk no longer matches the text it would be applied to:
/// the base moved since `get_hunks` produced the hunk, so the UI must
/// refresh. Nothing is ever partially applied — the caller gets this error
/// and the index stays byte-identical.
const STALE_ERR: &str = "内容已变化，请刷新";

/// Two hunks touching the same base region cannot both be applied; the
/// pairing is the caller's problem (and would be ambiguous).
const OVERLAP_ERR: &str = "hunk 区间重叠，无法应用";

/// A file in neither the index nor HEAD is untracked: there is no "old"
/// side to splice hunks into and the first add must be a file-level stage.
const UNTRACKED_ERR: &str = "该文件请用文件级暂存";

/// Binary content cannot round-trip through `String::from_utf8_lossy`, so
/// hunk staging refuses it rather than corrupting bytes in the index.
const BINARY_ERR: &str = "二进制文件请用文件级暂存";

/// The blob mode of a plain file (0100644), used when neither the index nor
/// HEAD has an entry to copy the mode from.
const DEFAULT_MODE: u32 = 0o100_644;

/// LCS cell budget, the same ceiling `diff.rs` enforces: without it a pair
/// of large files allocates hundreds of MB inside a blocking task the UI is
/// waiting on. Only reachable for files whose differing middles multiply
/// past 6M cells, which is far beyond what hunk staging should attempt.
const MAX_DP_CELLS: u64 = 6_000_000;

/// Split text into lines without terminators, keeping the remainder of a
/// CRLF pair (`\r`) inside the line so the split is losslessly invertible.
/// Empty text is *no* lines (a missing side and an empty file both mean
/// "nothing there yet"); `"a\n"` is two lines `["a", ""]`, and joining
/// reproduces the trailing newline.
pub(crate) fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').map(str::to_owned).collect()
    }
}

/// The inverse of [`split_lines`]: lines carry no newline, join adds one
/// between each pair, so a file that ended in `\n` keeps the trailing empty
/// line and re-joins byte-for-byte.
pub(crate) fn join_lines(lines: &[String]) -> String {
    lines.join("\n")
}

/// Line-level LCS diff of two line arrays, grouped into hunks: every
/// maximal run of deletes and inserts (a match separates runs — no
/// merging across any context) becomes one [`StagingHunk`]. Pure deletes
/// get an empty `new_lines`, pure inserts an empty `old_lines`.
pub(crate) fn diff_to_hunks(
    old: &[String],
    new: &[String],
) -> Result<Vec<StagingHunk>, String> {
    // Trim the common prefix/suffix first so the O(n·m) DP below only
    // covers the region that actually differs (same trick as diff.rs).
    let mut start = 0usize;
    while start < old.len() && start < new.len() && old[start] == new[start] {
        start += 1;
    }
    let mut end_a = old.len();
    let mut end_b = new.len();
    while end_a > start && end_b > start && old[end_a - 1] == new[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let a = &old[start..end_a];
    let b = &new[start..end_b];
    let n = a.len();
    let m = b.len();
    if n == 0 && m == 0 {
        return Ok(Vec::new());
    }
    if (n as u64).saturating_mul(m as u64) > MAX_DP_CELLS {
        return Err(format!(
            "文件差异过大（{n} 行 × {m} 行），无法生成块级 diff，请用文件级暂存"
        ));
    }

    // dp[i][j] = LCS length of a[i..] and b[j..], one flat u32 buffer.
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

    let mut hunks: Vec<StagingHunk> = Vec::new();
    // The open run: (old position, new position, deleted lines, inserted lines).
    let mut run: Option<(usize, usize, Vec<String>, Vec<String>)> = None;
    let mut flush = |run: &mut Option<(usize, usize, Vec<String>, Vec<String>)>| {
        if let Some((old_start, new_start, old_lines, new_lines)) = run.take() {
            hunks.push(StagingHunk {
                old_start,
                new_start,
                old_lines,
                new_lines,
            });
        }
    };

    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            flush(&mut run);
            i += 1;
            j += 1;
        } else if dp[(i + 1) * width + j] >= dp[i * width + j + 1] {
            let r = run.get_or_insert_with(|| (start + i, start + j, Vec::new(), Vec::new()));
            r.2.push(a[i].clone());
            i += 1;
        } else {
            let r = run.get_or_insert_with(|| (start + i, start + j, Vec::new(), Vec::new()));
            r.3.push(b[j].clone());
            j += 1;
        }
    }
    while i < n {
        let r = run.get_or_insert_with(|| (start + i, start + j, Vec::new(), Vec::new()));
        r.2.push(a[i].clone());
        i += 1;
    }
    while j < m {
        let r = run.get_or_insert_with(|| (start + i, start + j, Vec::new(), Vec::new()));
        r.3.push(b[j].clone());
        j += 1;
    }
    flush(&mut run);
    Ok(hunks)
}

/// Validate-then-apply a set of hunks against one base line array.
///
/// Forward (`reverse == false`) means staging: each hunk is matched at its
/// `old_start` against the base (the index) and replaced by `new_lines`.
/// Reverse means unstaging against the index-as-new-side: matched at
/// `new_start` on `new_lines`, replaced back by `old_lines`.
///
/// All hunks are validated against the same snapshot before anything is
/// applied; the first mismatch aborts the whole operation with
/// [`STALE_ERR`] and no partial state is produced. Overlapping runs are
/// rejected too. Surviving hunks are then applied back-to-front so earlier
/// positions are not shifted by later splices.
pub(crate) fn apply_hunks(
    base: &[String],
    hunks: &[StagingHunk],
    reverse: bool,
) -> Result<Vec<String>, String> {
    // Phase 1: validate every hunk against `base`. An edit is
    // (position, lines consumed, lines produced).
    let mut edits: Vec<(usize, usize, Vec<String>)> = Vec::with_capacity(hunks.len());
    for h in hunks {
        let (at, del, ins) = if reverse {
            (h.new_start, &h.new_lines, &h.old_lines)
        } else {
            (h.old_start, &h.old_lines, &h.new_lines)
        };
        if at > base.len() || at + del.len() > base.len() || base[at..at + del.len()] != del[..] {
            return Err(STALE_ERR.to_string());
        }
        edits.push((at, del.len(), ins.clone()));
    }

    // Phase 2: the consumed base regions must not overlap.
    edits.sort_by_key(|e| e.0);
    for win in edits.windows(2) {
        if win[1].0 < win[0].0 + win[0].1 {
            return Err(OVERLAP_ERR.to_string());
        }
    }

    // Phase 3: apply in descending position order.
    let mut out = base.to_vec();
    for (at, del, ins) in edits.into_iter().rev() {
        out.splice(at..at + del, ins);
    }
    Ok(out)
}

/// The working-tree text of `path`, or `None` when the file does not exist
/// there (a side that "does not exist" is empty text upstream). Refuses
/// binary content, which could not round-trip back into a blob.
fn workdir_text(repo: &git2::Repository, path: &str) -> Result<Option<String>, String> {
    let abs = resolve_in_workdir(repo, path)?;
    if !abs.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&abs).map_err(|e| e.to_string())?;
    if bytes_are_binary(&bytes) {
        return Err(BINARY_ERR.to_string());
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// The lines of a blob, refusing binary content (see [`BINARY_ERR`]).
fn blob_lines(repo: &git2::Repository, id: git2::Oid) -> Result<Vec<String>, String> {
    let blob = repo.find_blob(id).map_err(|e| e.to_string())?;
    if bytes_are_binary(blob.content()) {
        return Err(BINARY_ERR.to_string());
    }
    Ok(split_lines(&String::from_utf8_lossy(blob.content())))
}

/// The HEAD blob id and file mode for `path`, or `None` with no commit /
/// no such file. (`git2::TreeEntry` borrows its tree, so the command shells
/// only keep the two scalars they need out of it.)
fn head_entry_info(repo: &git2::Repository, path: &str) -> Option<(git2::Oid, u32)> {
    let commit = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok());
    let tree = commit.as_ref().and_then(|c| c.tree().ok())?;
    let entry = tree.get_path(Path::new(path)).ok()?;
    Some((entry.id(), entry.filemode() as u32))
}

/// Replace (or remove) the stage-0 entry for `path` with a blob holding
/// `text`, then persist the index. The working tree is never touched.
///
/// The entry is built with epoch-1 timestamps on purpose. libgit2's
/// `git_index_add*` copies the stat fields verbatim, and the diff/status
/// layer only trusts a workdir file without a content compare when its
/// stat equals the entry's — which an epoch-1 mtime never does. That is
/// the same trick `git update-index --cacheinfo` uses for partially
/// staged files, so `get_status` keeps reporting the *remaining* unstaged
/// hunks correctly.
fn write_index_content(
    index: &mut git2::Index,
    path: &str,
    text: &str,
    mode: u32,
    remove_when_empty: bool,
) -> Result<(), String> {
    if remove_when_empty && text.is_empty() {
        // The un-applied result is "file gone": drop the entry (a staged
        // deletion) rather than leaving an empty-blob entry behind.
        if index.get_path(Path::new(path), 0).is_some() {
            index.remove_path(Path::new(path)).map_err(|e| e.to_string())?;
        }
        return index.write().map_err(|e| e.to_string());
    }
    let old = git2::IndexTime::new(1, 0);
    let entry = git2::IndexEntry {
        ctime: old,
        mtime: old,
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: text.len() as u32,
        id: git2::Oid::zero(),
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    };
    // add_frombuffer hashes the text into a blob and stores its oid.
    index
        .add_frombuffer(&entry, text.as_bytes())
        .map_err(|e| e.to_string())?;
    index.write().map_err(|e| e.to_string())
}

fn get_hunks_impl(
    repo_path: String,
    path: String,
    from_index: bool,
) -> Result<Vec<StagingHunk>, String> {
    let repo = open_repo(&repo_path)?;
    validate_rel_path(&path)?;
    let index = repo.index().map_err(|e| e.to_string())?;
    let idx_entry = index.get_path(Path::new(&path), 0);
    let head = head_entry_info(&repo, &path);

    if !from_index && idx_entry.is_none() && head.is_none() {
        return Err(UNTRACKED_ERR.to_string());
    }

    let idx_lines = match &idx_entry {
        Some(e) => blob_lines(&repo, e.id)?,
        None => Vec::new(),
    };
    let (old, new) = if from_index {
        // Staged side: HEAD vs index.
        let head_lines = match head {
            Some((id, _)) => blob_lines(&repo, id)?,
            None => Vec::new(),
        };
        (head_lines, idx_lines)
    } else {
        // Unstaged side: index vs workdir.
        let wt = workdir_text(&repo, &path)?;
        let wt_lines = match wt {
            Some(t) => split_lines(&t),
            None => Vec::new(),
        };
        (idx_lines, wt_lines)
    };
    diff_to_hunks(&old, &new)
}

fn stage_hunks_impl(
    repo_path: String,
    path: String,
    hunks: Vec<StagingHunk>,
) -> Result<(), String> {
    if hunks.is_empty() {
        return Ok(());
    }
    let repo = open_repo(&repo_path)?;
    validate_rel_path(&path)?;
    let mut index = repo.index().map_err(|e| e.to_string())?;
    let idx_entry = index.get_path(Path::new(&path), 0);

    // The old side is the index blob; a file not in the index stages onto
    // empty text (its first content must still come from a real hunk).
    let base = match &idx_entry {
        Some(e) => blob_lines(&repo, e.id)?,
        None => Vec::new(),
    };
    let result = apply_hunks(&base, &hunks, false)?;
    let text = join_lines(&result);

    let mode = idx_entry
        .map(|e| e.mode)
        .or_else(|| head_entry_info(&repo, &path).map(|(_, m)| m))
        .unwrap_or(DEFAULT_MODE);
    // Full staging of a deletion (workdir file gone, every line removed)
    // is a staged delete: drop the entry, like `git add` would.
    let wt_missing = resolve_in_workdir(&repo, &path).map(|p| !p.exists()).unwrap_or(false);
    write_index_content(&mut index, &path, &text, mode, wt_missing)
}

fn unstage_hunks_impl(
    repo_path: String,
    path: String,
    hunks: Vec<StagingHunk>,
) -> Result<(), String> {
    if hunks.is_empty() {
        return Ok(());
    }
    let repo = open_repo(&repo_path)?;
    validate_rel_path(&path)?;
    let mut index = repo.index().map_err(|e| e.to_string())?;
    let idx_entry = index.get_path(Path::new(&path), 0);
    let head = head_entry_info(&repo, &path);

    let base = match &idx_entry {
        Some(e) => blob_lines(&repo, e.id)?,
        None => Vec::new(),
    };
    // Reverse-apply against the index-as-new-side.
    let result = apply_hunks(&base, &hunks, true)?;
    let text = join_lines(&result);

    let mode = idx_entry
        .map(|e| e.mode)
        .or_else(|| head.map(|(_, m)| m))
        .unwrap_or(DEFAULT_MODE);
    // HEAD has no such file and the reversal emptied it: the file should
    // not be in the index at all (un-staging a staged addition).
    write_index_content(&mut index, &path, &text, mode, head.is_none())
}

// Hunk commands follow the same shape as the rest of the working-area
// surface: the LCS DP and the blob IO run on the blocking pool so a big
// file cannot freeze the webview, and only the thin wrapper is the command.

/// List the hunk-level differences of one file.
/// `from_index == false`: index vs working tree (offer "stage this hunk").
/// `from_index == true`: HEAD vs index (offer "unstage this hunk").
#[tauri::command]
pub(crate) async fn get_hunks(
    repo_path: String,
    path: String,
    from_index: bool,
) -> Result<Vec<StagingHunk>, String> {
    tauri::async_runtime::spawn_blocking(move || get_hunks_impl(repo_path, path, from_index))
        .await
        .map_err(|e| format!("get_hunks task failed: {e}"))?
}

/// Apply hunks to the index (workdir untouched). All-or-nothing: any hunk
/// that no longer matches the index content fails the whole call.
#[tauri::command]
pub(crate) async fn stage_hunks(
    repo_path: String,
    path: String,
    hunks: Vec<StagingHunk>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || stage_hunks_impl(repo_path, path, hunks))
        .await
        .map_err(|e| format!("stage_hunks task failed: {e}"))?
}

/// Reverse-apply hunks from the index (workdir untouched), all-or-nothing.
#[tauri::command]
pub(crate) async fn unstage_hunks(
    repo_path: String,
    path: String,
    hunks: Vec<StagingHunk>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || unstage_hunks_impl(repo_path, path, hunks))
        .await
        .map_err(|e| format!("unstage_hunks task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A throwaway git repo in the OS temp dir (same scheme as
    /// `working.rs` tests: pid + atomic counter, removed on drop).
    struct TempRepo {
        path: PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "idea-git-tool-hunks-{}-{}",
                std::process::id(),
                n
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            git2::Repository::init(&dir).unwrap();
            TempRepo { path: dir }
        }

        fn path_str(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }

        fn write(&self, rel: &str, content: &str) {
            std::fs::write(self.path.join(rel), content).unwrap();
        }

        fn read(&self, rel: &str) -> Vec<u8> {
            std::fs::read(self.path.join(rel)).unwrap()
        }

        fn remove(&self, rel: &str) {
            std::fs::remove_file(self.path.join(rel)).unwrap();
        }

        /// File-level stage + commit of everything currently on disk.
        fn commit_all(&self, message: &str) {
            let repo = open_repo(&self.path_str()).unwrap();
            let mut index = repo.index().unwrap();
            index
                .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            let parents: Vec<git2::Commit> = repo
                .head()
                .ok()
                .and_then(|h| h.target())
                .map(|oid| vec![repo.find_commit(oid).unwrap()])
                .unwrap_or_default();
            let refs: Vec<&git2::Commit> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &refs)
                .unwrap();
        }

        /// File-level stage of the current workdir content (tamper helper).
        fn stage_file(&self, rel: &str) {
            let repo = open_repo(&self.path_str()).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new(rel)).unwrap();
            index.write().unwrap();
        }

        fn index_text(&self, rel: &str) -> Option<String> {
            let repo = open_repo(&self.path_str()).unwrap();
            let index = repo.index().unwrap();
            let e = index.get_path(Path::new(rel), 0)?;
            let blob = repo.find_blob(e.id).unwrap();
            Some(String::from_utf8_lossy(blob.content()).into_owned())
        }

        fn head_text(&self, rel: &str) -> Option<String> {
            let repo = open_repo(&self.path_str()).unwrap();
            let tree = repo.head().unwrap().peel_to_tree().unwrap();
            let e = tree.get_path(Path::new(rel)).ok()?;
            let blob = repo.find_blob(e.id()).unwrap();
            Some(String::from_utf8_lossy(blob.content()).into_owned())
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn ls(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn hunk(old_start: usize, new_start: usize, old: &[&str], new: &[&str]) -> StagingHunk {
        StagingHunk {
            old_start,
            new_start,
            old_lines: ls(old),
            new_lines: ls(new),
        }
    }

    fn text_of(t: &str) -> Vec<String> {
        split_lines(t)
    }

    // ── split/join round-trip ──

    #[test]
    fn split_join_roundtrips_lf_crlf_and_empty() {
        for t in ["", "a", "a\n", "a\nb", "a\r\nb\r\n", "a\r\n", "\n"] {
            assert_eq!(join_lines(&split_lines(t)), t, "roundtrip of {t:?}");
        }
    }

    // ── diff_to_hunks：分组精确值 ──

    #[test]
    fn single_changed_line_is_one_hunk() {
        let old = text_of("a\nb\nc\n");
        let new = text_of("a\nB\nc\n");
        assert_eq!(
            diff_to_hunks(&old, &new).unwrap(),
            vec![hunk(1, 1, &["b"], &["B"])]
        );
    }

    #[test]
    fn pure_insertion_has_empty_old_lines() {
        let old = text_of("a\nb\n");
        let new = text_of("a\nN\nb\n");
        assert_eq!(
            diff_to_hunks(&old, &new).unwrap(),
            vec![hunk(1, 1, &[], &["N"])]
        );
    }

    #[test]
    fn pure_deletion_has_empty_new_lines() {
        let old = text_of("a\nX\nb\n");
        let new = text_of("a\nb\n");
        assert_eq!(
            diff_to_hunks(&old, &new).unwrap(),
            vec![hunk(1, 1, &["X"], &[])]
        );
    }

    #[test]
    fn adjacent_changed_lines_merge_into_one_hunk() {
        let old = text_of("a\nb\nc\nd\ne\n");
        let new = text_of("a\nB\nC\nd\ne\n");
        assert_eq!(
            diff_to_hunks(&old, &new).unwrap(),
            vec![hunk(1, 1, &["b", "c"], &["B", "C"])]
        );
    }

    #[test]
    fn one_context_line_splits_two_hunks() {
        // MVP 无上下文：哪怕只隔一行未变，也不合并。
        let old = text_of("a\nb\nc\nd\ne\n");
        let new = text_of("A\nb\nC\nd\ne\n");
        assert_eq!(
            diff_to_hunks(&old, &new).unwrap(),
            vec![hunk(0, 0, &["a"], &["A"]), hunk(2, 2, &["c"], &["C"])]
        );
    }

    #[test]
    fn identical_text_has_no_hunks() {
        let old = text_of("a\nb\nc\n");
        assert_eq!(diff_to_hunks(&old, &old).unwrap(), vec![]);
    }

    #[test]
    fn empty_side_is_one_pure_insertion_hunk() {
        // Missing side = empty text: the whole file is one insert run.
        let old = text_of("");
        let new = text_of("a\nb\n");
        assert_eq!(
            diff_to_hunks(&old, &new).unwrap(),
            vec![hunk(0, 0, &[], &["a", "b", ""])]
        );
    }

    // ── apply_hunks：校验、重叠、降序应用 ──

    #[test]
    fn apply_forward_matches_replaces_and_keeps_order() {
        let base = text_of("a\nb\nc\nd\ne\n");
        let h1 = hunk(0, 0, &["a"], &["A"]);
        let h2 = hunk(3, 3, &["d"], &["D"]);
        // Deliberately out of order: application must sort by position.
        let out = apply_hunks(&base, &[h2, h1], false).unwrap();
        assert_eq!(join_lines(&out), "A\nb\nc\nD\ne\n");
    }

    #[test]
    fn apply_rejects_mismatch_without_producing_state() {
        let base = text_of("a\nb\nc\n");
        let bad = hunk(1, 1, &["zzz"], &["X"]);
        assert_eq!(apply_hunks(&base, &[bad], false).unwrap_err(), STALE_ERR);
        // Out of bounds is the same "base moved" story.
        let oob = hunk(9, 9, &["a"], &["X"]);
        assert_eq!(apply_hunks(&base, &[oob], false).unwrap_err(), STALE_ERR);
    }

    #[test]
    fn apply_rejects_overlapping_runs() {
        let base = text_of("a\nb\nc\nd\n");
        let h1 = hunk(1, 0, &["b", "c"], &["X"]);
        let h2 = hunk(2, 5, &["c", "d"], &["Y"]);
        // Both individually match; together they collide on line "c".
        assert_eq!(
            apply_hunks(&base, &[h1, h2], false).unwrap_err(),
            OVERLAP_ERR
        );
    }

    #[test]
    fn apply_reverse_swaps_the_sides() {
        let base = text_of("a\nB\nc\n");
        let h = hunk(1, 1, &["b"], &["B"]);
        let out = apply_hunks(&base, &[h.clone()], true).unwrap();
        assert_eq!(join_lines(&out), "a\nb\nc\n");
        // Forward matching uses old_lines at old_start; the new side must
        // not be consulted there.
        assert_eq!(apply_hunks(&base, &[h], false).unwrap_err(), STALE_ERR);
    }

    #[test]
    fn apply_is_all_or_nothing_with_a_bad_hunk() {
        let base = text_of("a\nb\nc\nd\ne\n");
        let good = hunk(0, 0, &["a"], &["A"]);
        let bad = hunk(3, 3, &["nope"], &["D"]);
        assert_eq!(
            apply_hunks(&base, &[good, bad], false).unwrap_err(),
            STALE_ERR
        );
    }

    // ── 命令级集成测试：暂存往返 ──

    #[test]
    fn stage_round_trip_single_hunk() {
        let r = TempRepo::new();
        r.write("f.txt", "a\nb\nc\n");
        r.commit_all("base");
        r.write("f.txt", "a\nB\nc\n");

        let hunks = get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap();
        assert_eq!(hunks, vec![hunk(1, 1, &["b"], &["B"])]);

        stage_hunks_impl(r.path_str(), "f.txt".into(), hunks.clone()).unwrap();

        // Index content now equals the workdir; HEAD is untouched.
        assert_eq!(
            r.index_text("f.txt").as_deref(),
            Some(std::str::from_utf8(&r.read("f.txt")).unwrap())
        );
        assert_eq!(r.head_text("f.txt").as_deref(), Some("a\nb\nc\n"));
        // The staged list shows the very same hunk (幂等针).
        assert_eq!(
            get_hunks_impl(r.path_str(), "f.txt".into(), true).unwrap(),
            hunks
        );
        // And nothing is left unstaged.
        assert!(get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap().is_empty());
    }

    #[test]
    fn staging_one_of_two_hunks_leaves_the_other() {
        let r = TempRepo::new();
        r.write("f.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        r.commit_all("base");
        r.write("f.txt", "1\nTWO\n3\n4\n5\n6\n7\nEIGHT\n9\n10\n");

        let hunks = get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap();
        assert_eq!(
            hunks,
            vec![hunk(1, 1, &["2"], &["TWO"]), hunk(7, 7, &["8"], &["EIGHT"])]
        );

        // Stage only the second hunk.
        stage_hunks_impl(r.path_str(), "f.txt".into(), vec![hunks[1].clone()]).unwrap();
        assert_eq!(
            r.index_text("f.txt").as_deref(),
            Some("1\n2\n3\n4\n5\n6\n7\nEIGHT\n9\n10\n")
        );
        // The workdir is untouched and still holds both edits.
        assert_eq!(
            std::str::from_utf8(&r.read("f.txt")).unwrap(),
            "1\nTWO\n3\n4\n5\n6\n7\nEIGHT\n9\n10\n"
        );
        // get_hunks(false) is left with exactly the first hunk…
        assert_eq!(
            get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap(),
            vec![hunks[0].clone()]
        );
        // …and file-level status still reports the file as unstaged-modified
        // (the epoch-1 stat stamp keeps status from trusting the shortcut).
        let repo = open_repo(&r.path_str()).unwrap();
        let statuses = repo.statuses(None).unwrap();
        let entry = statuses
            .iter()
            .find(|s| s.path() == Some("f.txt"))
            .expect("f.txt in status");
        assert!(entry.status().is_wt_modified(), "status = {:?}", entry.status());
    }

    #[test]
    fn unstage_restores_index_and_never_touches_workdir() {
        let r = TempRepo::new();
        r.write("f.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        r.commit_all("base");
        r.write("f.txt", "1\nTWO\n3\n4\n5\n6\n7\nEIGHT\n9\n10\n");
        let hunks = get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap();
        stage_hunks_impl(r.path_str(), "f.txt".into(), vec![hunks[1].clone()]).unwrap();

        // The staged list shows exactly the staged hunk; unstage it again.
        let staged = get_hunks_impl(r.path_str(), "f.txt".into(), true).unwrap();
        assert_eq!(staged, vec![hunks[1].clone()]);
        unstage_hunks_impl(r.path_str(), "f.txt".into(), staged).unwrap();

        // Index is back at the HEAD version, workdir bytes are untouched.
        assert_eq!(r.index_text("f.txt"), r.head_text("f.txt"));
        assert_eq!(
            std::str::from_utf8(&r.read("f.txt")).unwrap(),
            "1\nTWO\n3\n4\n5\n6\n7\nEIGHT\n9\n10\n"
        );
        // Both hunks are unstaged again.
        assert_eq!(get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap(), hunks);
    }

    #[test]
    fn stage_fails_when_the_index_moved_and_changes_nothing() {
        let r = TempRepo::new();
        r.write("f.txt", "a\nb\nc\n");
        r.commit_all("base");
        r.write("f.txt", "a\nB\nc\n");
        let hunks = get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap();

        // Tamper: rewrite the workdir AND file-level stage it, so the index
        // no longer contains the hunk's old_lines.
        r.write("f.txt", "x\ny\nz\n");
        r.stage_file("f.txt");
        let index_before = r.index_text("f.txt").unwrap();

        let err = stage_hunks_impl(r.path_str(), "f.txt".into(), hunks).unwrap_err();
        assert_eq!(err, STALE_ERR);
        // All-or-nothing: the index is byte-identical to before the attempt.
        assert_eq!(r.index_text("f.txt").unwrap(), index_before);
    }

    #[test]
    fn restaging_an_applied_hunk_is_rejected() {
        let r = TempRepo::new();
        r.write("f.txt", "a\nb\nc\n");
        r.commit_all("base");
        r.write("f.txt", "a\nB\nc\n");
        let hunks = get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap();
        stage_hunks_impl(r.path_str(), "f.txt".into(), hunks.clone()).unwrap();

        // The hunk was already applied: its old_lines no longer match the
        // index, so a second stage must Err instead of re-inserting.
        let err = stage_hunks_impl(r.path_str(), "f.txt".into(), hunks.clone()).unwrap_err();
        assert_eq!(err, STALE_ERR);
        assert_eq!(r.index_text("f.txt").as_deref(), Some("a\nB\nc\n"));
    }

    #[test]
    fn pure_insert_and_delete_hunks_stage_end_to_end() {
        let r = TempRepo::new();
        r.write("f.txt", "a\nb\n");
        r.commit_all("base");

        // Pure insertion.
        r.write("f.txt", "a\nN\nb\n");
        let ins = get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap();
        assert_eq!(ins, vec![hunk(1, 1, &[], &["N"])]);
        stage_hunks_impl(r.path_str(), "f.txt".into(), ins).unwrap();
        assert_eq!(r.index_text("f.txt").as_deref(), Some("a\nN\nb\n"));

        // Pure deletion.
        r.write("f.txt", "a\nb\n");
        let del = get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap();
        assert_eq!(del, vec![hunk(1, 1, &["N"], &[])]);
        stage_hunks_impl(r.path_str(), "f.txt".into(), del).unwrap();
        assert_eq!(r.index_text("f.txt").as_deref(), Some("a\nb\n"));
    }

    #[test]
    fn staging_a_full_deletion_removes_the_index_entry() {
        let r = TempRepo::new();
        r.write("f.txt", "a\nb\n");
        r.commit_all("base");
        r.remove("f.txt");

        let hunks = get_hunks_impl(r.path_str(), "f.txt".into(), false).unwrap();
        assert_eq!(hunks, vec![hunk(0, 0, &["a", "b", ""], &[])]);
        stage_hunks_impl(r.path_str(), "f.txt".into(), hunks).unwrap();
        // The deletion is staged as a removal, not an empty-file entry.
        assert_eq!(r.index_text("f.txt"), None);
        assert_eq!(r.head_text("f.txt").as_deref(), Some("a\nb\n"));
    }

    #[test]
    fn untracked_and_binary_files_are_refused() {
        let r = TempRepo::new();
        r.write("new.txt", "x\n");
        let err = get_hunks_impl(r.path_str(), "new.txt".into(), false).unwrap_err();
        assert_eq!(err, UNTRACKED_ERR);
        // With nothing on either side, the staged view is simply empty.
        assert!(get_hunks_impl(r.path_str(), "new.txt".into(), true).unwrap().is_empty());

        // A binary tracked file must not be hunk-staged (lossy text would
        // corrupt the bytes).
        r.write("bin.dat", "");
        std::fs::write(r.path.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        r.commit_all("base");
        std::fs::write(r.path.join("bin.dat"), [0u8, 9, 9, 9]).unwrap();
        let err = get_hunks_impl(r.path_str(), "bin.dat".into(), false).unwrap_err();
        assert_eq!(err, BINARY_ERR);
    }

    #[test]
    fn unstage_of_a_staged_addition_removes_the_entry() {
        let r = TempRepo::new();
        r.write("f.txt", "a\n");
        r.stage_file("f.txt"); // added to index, not in HEAD

        let staged = get_hunks_impl(r.path_str(), "f.txt".into(), true).unwrap();
        assert_eq!(staged, vec![hunk(0, 0, &[], &["a", ""])]);
        unstage_hunks_impl(r.path_str(), "f.txt".into(), staged).unwrap();
        // Reverse-applied to empty text with no HEAD file: the entry goes.
        assert_eq!(r.index_text("f.txt"), None);
        // The workdir file survives untouched.
        assert_eq!(std::str::from_utf8(&r.read("f.txt")).unwrap(), "a\n");
    }

    #[test]
    fn hunk_serde_wire_format_is_camel_case() {
        let h = hunk(1, 2, &["b"], &["B"]);
        let json = serde_json::to_value(&h).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "oldStart": 1,
                "newStart": 2,
                "oldLines": ["b"],
                "newLines": ["B"]
            })
        );
        let back: StagingHunk = serde_json::from_value(json).unwrap();
        assert_eq!(back, h);
    }
}
