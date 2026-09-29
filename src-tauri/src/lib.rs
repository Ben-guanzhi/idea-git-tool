use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Resolve a repo-relative path against the workdir, rejecting absolute paths
/// and `..` traversal. libgit2's `index.add_path` refuses paths outside the
/// workdir on its own, but the direct `std::fs` calls in `write_file`,
/// `discard`, `resolve_conflict` and `get_file_contents` must not become
/// arbitrary read/write/delete primitives: a `../` path would otherwise
/// escape the repository entirely.
fn resolve_in_workdir(repo: &git2::Repository, path: &str) -> Result<PathBuf, String> {
    validate_rel_path(path)?;
    repo.workdir()
        .map(|w| w.join(path))
        .ok_or_else(|| "repository has no workdir".to_string())
}

/// Open the repository at `repo_path`, mapping the error to the string the
/// command boundary carries. Every command used to repeat this mapping.
fn open_repo(repo_path: &str) -> Result<git2::Repository, String> {
    git2::Repository::open(repo_path).map_err(|e| e.to_string())
}

/// Reject absolute paths, drive prefixes and `..`/root components outright.
fn validate_rel_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("非法路径「{path}」：必须位于仓库工作区内"));
    }
    Ok(())
}

/// Refuse to force-checkout over a dirty working tree. The git CLI protects
/// uncommitted changes before a merge/checkout; without this check the
/// `.force()` checkouts below would silently wipe them (data loss).
/// Untracked-and-unstaged files (`WT_NEW` only) are allowed, matching git.
fn ensure_clean_workdir(repo: &git2::Repository) -> Result<(), String> {
    let statuses = repo.statuses(None).map_err(|e| e.to_string())?;
    for entry in statuses.iter() {
        if entry.status() != git2::Status::WT_NEW {
            return Err("工作区有未提交的修改，请先提交或暂存（stash）后再执行该操作".to_string());
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConflictInfo {
    path: String,
    ours: String,
    theirs: String,
    base: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitArgs {
    repo_path: String,
    message: String,
    author_name: String,
    author_email: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusInfo {
    branch: String,
    upstream: Option<String>,
    ahead: usize,
    behind: usize,
    staged: Vec<FileStatus>,
    unstaged: Vec<FileStatus>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FileStatus {
    path: String,
    /// one of: modified, added, deleted, renamed, typechanged
    status: String,
}

fn repo_head_name(repo: &git2::Repository) -> Result<String, String> {
    if repo.head_detached().map_err(|e| e.to_string())? {
        let oid = repo.head().map_err(|e| e.to_string())?.target();
        Ok(oid
            .map(|o| format!("HEAD detached at {}", o.to_string()))
            .unwrap_or_else(|| "detached".to_string()))
    } else {
        match repo.head() {
            Ok(h) => match h.shorthand() {
                Some(s) => Ok(s.to_string()),
                None => Ok("detached".to_string()),
            },
            Err(_) => Ok("unborn".to_string()),
        }
    }
}

fn sig_for(
    repo: &git2::Repository,
    name: &str,
    email: &str,
) -> Result<git2::Signature<'static>, String> {
    if !name.is_empty() && !email.is_empty() {
        git2::Signature::now(name, email).map_err(|e| e.to_string())
    } else {
        repo.signature().map_err(|e| {
            format!(
                "无法获取提交作者签名（git user.name / user.email 未配置）: {e}。\
                 请运行: git config --global user.name \"你的名字\" && \
                 git config --global user.email \"你的邮箱\""
            )
        })
    }
}

/// Convert a git2 delta status into a short string.
fn delta_to_status(d: git2::Delta) -> &'static str {
    match d {
        git2::Delta::Added => "added",
        git2::Delta::Deleted => "deleted",
        git2::Delta::Modified => "modified",
        git2::Delta::Renamed => "renamed",
        git2::Delta::Typechange => "typechanged",
        _ => "modified",
    }
}

#[tauri::command]
fn get_status(repo_path: String) -> Result<StatusInfo, String> {
    let repo = open_repo(&repo_path)?;

    let branch = repo_head_name(&repo)?;

    let (upstream, ahead, behind) = {
        let head = repo.head().ok();
        let branch = head.as_ref().and_then(|h| {
            repo.find_branch(h.shorthand().unwrap_or(""), git2::BranchType::Local)
                .ok()
        });
        let up = branch.and_then(|b| b.upstream().ok());
        match up {
            Some(u) => {
                let up_name = u.name().ok().flatten().map(str::to_owned);
                let local_oid = head
                    .as_ref()
                    .and_then(|h| h.target())
                    .unwrap_or(git2::Oid::zero());
                let (a, b) = repo
                    .graph_ahead_behind(local_oid, u.get().target().unwrap_or(git2::Oid::zero()))
                    .unwrap_or((0, 0));
                (up_name, a, b)
            }
            None => (None, 0, 0),
        }
    };

    let mut staged: Vec<FileStatus> = Vec::new();
    let mut unstaged: Vec<FileStatus> = Vec::new();

    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;

    for s in statuses.iter() {
        let path = s.path().map(|p| p.to_string()).unwrap_or_default();
        let st = s.status();
        let is_conflicted = st.contains(git2::Status::CONFLICTED);
        if st.is_index_new()
            || st.is_index_modified()
            || st.is_index_deleted()
            || st.is_index_renamed()
            || st.is_index_typechange()
        {
            staged.push(FileStatus {
                path: path.clone(),
                status: s
                    .head_to_index()
                    .map(|d| delta_to_status(d.status()))
                    .unwrap_or("modified")
                    .to_string(),
            });
        }
        if st.is_wt_new()
            || st.is_wt_modified()
            || st.is_wt_deleted()
            || st.is_wt_renamed()
            || st.is_wt_typechange()
            || is_conflicted
        {
            let status = if is_conflicted {
                "conflicted".to_string()
            } else {
                s.index_to_workdir()
                    .map(|d| delta_to_status(d.status()))
                    .unwrap_or("modified")
                    .to_string()
            };
            unstaged.push(FileStatus {
                path: path.clone(),
                status,
            });
        }
    }

    // De-duplicate paths that appear in both staged and unstaged lists.
    let staged_paths: std::collections::HashSet<&String> = staged.iter().map(|f| &f.path).collect();
    unstaged.retain(|f| !staged_paths.contains(&f.path));

    Ok(StatusInfo {
        branch,
        upstream,
        ahead,
        behind,
        staged,
        unstaged,
    })
}

#[tauri::command]
fn stage(repo_path: String, paths: Vec<String>) -> Result<(), String> {
    let repo = open_repo(&repo_path)?;
    let mut index = repo.index().map_err(|e| e.to_string())?;
    let workdir = repo.workdir().map(|w| w.to_path_buf());
    for p in &paths {
        let abs = workdir.as_ref().map(|w| w.join(p));
        let exists = abs.as_ref().map(|a| a.exists()).unwrap_or(false);
        if exists {
            index.add_path(Path::new(p)).map_err(|e| e.to_string())?;
        } else if index.get_path(Path::new(p), 0).is_some() {
            // Staging a deletion.
            index.remove_path(Path::new(p)).map_err(|e| e.to_string())?;
        }
    }
    index.write().map_err(|e| e.to_string())?;
    Ok(())
}

/// Discard working-tree / index changes for paths: restore from HEAD, or delete untracked.
#[tauri::command]
fn discard(repo_path: String, paths: Vec<String>) -> Result<(), String> {
    let repo = open_repo(&repo_path)?;
    let head_tree = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .and_then(|c| c.tree().ok());
    let mut index = repo.index().map_err(|e| e.to_string())?;

    for p in &paths {
        let abs = resolve_in_workdir(&repo, p)?;
        let in_index = index.get_path(Path::new(p), 0).is_some();
        let in_head = head_tree
            .as_ref()
            .and_then(|t| t.get_path(Path::new(p)).ok());

        match in_head {
            Some(entry) => {
                let blob = repo.find_blob(entry.id()).map_err(|e| e.to_string())?;
                if let Some(parent) = abs.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&abs, blob.content()).map_err(|e| e.to_string())?;
                // Always re-add: covers staged deletions (path missing from index).
                index.add_path(Path::new(p)).map_err(|e| e.to_string())?;
            }
            None => {
                // Not in HEAD (new file) or untracked: drop from index and disk.
                if in_index {
                    index.remove_path(Path::new(p)).map_err(|e| e.to_string())?;
                }
                if abs.exists() {
                    std::fs::remove_file(&abs).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    index.write().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn unstage(repo_path: String, paths: Vec<String>) -> Result<(), String> {
    let repo = open_repo(&repo_path)?;
    let head_commit = repo.head().and_then(|h| h.peel_to_commit()).ok();

    match head_commit {
        Some(commit) => {
            repo.reset_default(Some(commit.as_object()), &paths)
                .map_err(|e| e.to_string())?;
        }
        None => {
            // No commits yet: just remove the paths from the index.
            let mut index = repo.index().map_err(|e| e.to_string())?;
            for p in &paths {
                index.remove_path(Path::new(p)).map_err(|e| e.to_string())?;
            }
            index.write().map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

/// Resolve a conflict for the given paths by choosing one side.
/// `choice` is "ours" or "theirs".
#[tauri::command]
fn resolve_conflict(repo_path: String, paths: Vec<String>, choice: String) -> Result<(), String> {
    let repo = open_repo(&repo_path)?;
    let mut index = repo.index().map_err(|e| e.to_string())?;

    for p in &paths {
        let path = Path::new(p);
        let ours_id = index
            .get_path(path, 1)
            .or_else(|| index.get_path(path, 0))
            .map(|e| e.id);
        let theirs_id = index.get_path(path, 2).map(|e| e.id);

        let chosen_id = match choice.as_str() {
            "ours" => ours_id,
            "theirs" => theirs_id,
            _ => ours_id.or(theirs_id),
        };

        let abs = resolve_in_workdir(&repo, p)?;

        match chosen_id {
            Some(id) => {
                if let Ok(blob) = repo.find_blob(id) {
                    let _ = std::fs::write(&abs, blob.content());
                    // Stage the resolved working file.
                    index.add_path(path).map_err(|e| e.to_string())?;
                }
            }
            None => {
                index.remove_path(path).map_err(|e| e.to_string())?;
                let _ = std::fs::remove_file(&abs);
            }
        }
    }

    index.write().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn commit(args: CommitArgs) -> Result<String, String> {
    let repo = git2::Repository::open(&args.repo_path).map_err(|e| e.to_string())?;
    let mut index = repo.index().map_err(|e| e.to_string())?;
    let tree_id = index.write_tree().map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;

    let head = repo.head().ok();
    let parent_commit = match head {
        Some(head_ref) => {
            let oid = head_ref
                .target()
                .ok_or_else(|| "head has no target".to_string())?;
            let parent = repo.find_commit(oid).map_err(|e| e.to_string())?;
            if parent.tree_id() == tree_id {
                return Err("nothing to commit, working tree clean".to_string());
            }
            Some(parent)
        }
        None => None,
    };

    let sig = sig_for(&repo, &args.author_name, &args.author_email)?;
    let commit_id = if let Some(parent) = &parent_commit {
        repo.commit(Some("HEAD"), &sig, &sig, &args.message, &tree, &[parent])
    } else {
        repo.commit(Some("HEAD"), &sig, &sig, &args.message, &tree, &[])
    }
    .map_err(|e| e.to_string())?;

    Ok(commit_id.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MergeResult {
    /// One of: merged, fastForward, upToDate, conflicts.
    status: String,
    message: String,
    /// Paths with conflicts when status == "conflicts".
    conflicts: Vec<String>,
}

fn current_branch_name(repo: &git2::Repository) -> Result<String, String> {
    match repo.head() {
        Ok(h) if h.is_branch() => Ok(h.shorthand().unwrap_or_default().to_string()),
        Ok(_) => Err("HEAD is detached; cannot merge".to_string()),
        Err(_) => Err("no commits yet (unborn branch); cannot merge".to_string()),
    }
}

/// One deduplicated record per conflicted path: `(path, our, their, ancestor)`,
/// the path taken from the first side that exists.
fn index_conflicts(
    repo: &git2::Repository,
) -> Vec<(
    Vec<u8>,
    Option<git2::IndexEntry>,
    Option<git2::IndexEntry>,
    Option<git2::IndexEntry>,
)> {
    let Ok(index) = repo.index() else {
        return Vec::new();
    };
    let Ok(iter) = index.conflicts() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in iter.flatten() {
        if let Some(e) = c.our.as_ref().or(c.their.as_ref()).or(c.ancestor.as_ref()) {
            if seen.insert(e.path.clone()) {
                out.push((e.path.clone(), c.our, c.their, c.ancestor));
            }
        }
    }
    out
}

fn index_conflict_paths(repo: &git2::Repository) -> Vec<String> {
    index_conflicts(repo)
        .iter()
        .map(|(p, ..)| String::from_utf8_lossy(p).into_owned())
        .collect()
}

#[tauri::command]
fn merge(repo_path: String, branch: String) -> Result<MergeResult, String> {
    let repo = open_repo(&repo_path)?;
    let current = current_branch_name(&repo)?;

    if branch == current {
        return Err(format!("「{branch}」已经是当前分支，无需合并"));
    }

    let oid = repo
        .find_branch(&branch, git2::BranchType::Local)
        .or_else(|_| repo.find_branch(&branch, git2::BranchType::Remote))
        .and_then(|b| b.get().peel_to_commit().map(|c| c.id()))
        .or_else(|_| repo.refname_to_id(&format!("refs/heads/{branch}")))
        .or_else(|_| repo.refname_to_id(&format!("refs/remotes/origin/{branch}")))
        .map_err(|e| format!("找不到分支「{branch}」: {e}"))?;
    let annotated = repo.find_annotated_commit(oid).map_err(|e| e.to_string())?;

    let analysis = repo
        .merge_analysis(&[&annotated])
        .map_err(|e| e.to_string())?;
    let a = analysis.0;

    if a.is_up_to_date() {
        return Ok(MergeResult {
            status: "upToDate".into(),
            message: format!("「{branch}」已是最新，无需合并"),
            conflicts: vec![],
        });
    }

    if a.is_unborn() {
        // Current branch has no commits: fast-forward HEAD to the target.
        ensure_clean_workdir(&repo)?;
        let refname = format!("refs/heads/{current}");
        repo.reference(&refname, oid, true, "merge: Fast-Forward")
            .map_err(|e| e.to_string())?;
        repo.set_head(&refname).map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        repo.checkout_tree(
            commit.as_object(),
            Some(git2::build::CheckoutBuilder::new().force()),
        )
        .map_err(|e| e.to_string())?;
        return Ok(MergeResult {
            status: "fastForward".into(),
            message: format!("已将「{branch}」合并到「{current}」"),
            conflicts: vec![],
        });
    }

    if !a.is_fast_forward() && !a.is_normal() {
        return Err(format!("不支持的合并类型，无法合并「{branch}」"));
    }

    if a.is_fast_forward() {
        // Just move the branch pointer; no merge commit needed.
        ensure_clean_workdir(&repo)?;
        let refname = format!("refs/heads/{current}");
        repo.reference(
            &refname,
            oid,
            true,
            &format!("merge: fast-forward to {branch}"),
        )
        .map_err(|e| e.to_string())?;
        repo.set_head(&refname).map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        repo.checkout_tree(
            commit.as_object(),
            Some(git2::build::CheckoutBuilder::new().force()),
        )
        .map_err(|e| e.to_string())?;
        return Ok(MergeResult {
            status: "fastForward".into(),
            message: format!("已快进合并「{branch}」到「{current}」"),
            conflicts: vec![],
        });
    }

    // Perform the merge into the index / workdir (does not commit yet).
    repo.merge(&[&annotated], None, None)
        .map_err(|e| e.to_string())?;

    let conflicts = index_conflict_paths(&repo);
    if !conflicts.is_empty() {
        return Ok(MergeResult {
            status: "conflicts".into(),
            message: format!(
                "合并「{branch}」到「{current}」时有 {} 个冲突，请在 Changes 中解决",
                conflicts.len()
            ),
            conflicts,
        });
    }

    let mut index = repo.index().map_err(|e| e.to_string())?;
    let tree_id = index.write_tree().map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let head_commit = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.to_string())?;
    let merge_commit = repo.find_commit(oid).map_err(|e| e.to_string())?;

    if head_commit.id() == oid {
        return Ok(MergeResult {
            status: "upToDate".into(),
            message: format!("「{branch}」已是最新，无需合并"),
            conflicts: vec![],
        });
    }

    let sig = repo.signature().map_err(|e| e.to_string())?;
    let msg = format!("Merge branch '{branch}' into {current}");
    let new_id = repo
        .commit(
            Some("HEAD"),
            &sig,
            &sig,
            &msg,
            &tree,
            &[&head_commit, &merge_commit],
        )
        .map_err(|e| e.to_string())?;

    // Sync workdir with the new merge commit.
    ensure_clean_workdir(&repo)?;
    let new_commit = repo.find_commit(new_id).map_err(|e| e.to_string())?;
    repo.checkout_tree(
        new_commit.as_object(),
        Some(git2::build::CheckoutBuilder::new().force()),
    )
    .map_err(|e| e.to_string())?;

    Ok(MergeResult {
        status: "merged".into(),
        message: format!("已将「{branch}」合并到「{current}」"),
        conflicts: vec![],
    })
}

/// Abort an in-progress merge (equivalent to `git merge --abort`).
#[tauri::command]
fn abort_merge(repo_path: String) -> Result<String, String> {
    // Prefer system git for a full, correct abort (cleans MERGE_HEAD etc.).
    match run_git(&repo_path, &["merge", "--abort"]) {
        Ok(_) => Ok("已中止合并".to_string()),
        Err(e) => {
            // Fallback: hard reset to HEAD if merge --abort is unavailable.
            if e.contains("There is no merge to abort") || e.contains("MERGE_HEAD") {
                return Ok("当前没有进行中的合并".to_string());
            }
            let repo = open_repo(&repo_path)?;
            let head = repo
                .head()
                .and_then(|h| h.peel_to_commit())
                .map_err(|_| "无法中止：没有可重置的 HEAD".to_string())?;
            repo.reset(head.as_object(), git2::ResetType::Hard, None)
                .map_err(|e| format!("中止合并失败: {e}"))?;
            Ok("已中止合并（hard reset）".to_string())
        }
    }
}

#[tauri::command]
fn get_conflicts(repo_path: String) -> Result<Vec<ConflictInfo>, String> {
    let repo = open_repo(&repo_path)?;

    let obj2bytes = |obj: &git2::IndexEntry| -> Result<String, String> {
        let blob = repo.find_blob(obj.id).map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(blob.content()).into_owned())
    };

    let mut conflicts: Vec<ConflictInfo> = Vec::new();
    for (path, our, their, ancestor) in index_conflicts(&repo) {
        let read = |entry: &Option<git2::IndexEntry>| -> Result<String, String> {
            match entry {
                Some(obj) => obj2bytes(obj),
                None => Ok(String::new()),
            }
        };
        conflicts.push(ConflictInfo {
            path: String::from_utf8_lossy(&path).into_owned(),
            ours: read(&our)?,
            theirs: read(&their)?,
            base: read(&ancestor)?,
        });
    }

    Ok(conflicts)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileContent {
    path: String,
    /// Content from the working tree, or empty if the file does not exist there.
    working: String,
    /// True if there is an active merge conflict for this path.
    conflicted: bool,
}

#[tauri::command]
fn get_file_contents(repo_path: String, path: String) -> Result<FileContent, String> {
    let repo = open_repo(&repo_path)?;
    let conflicted = index_conflicts(&repo)
        .iter()
        .any(|(p, ..)| *p == path.as_bytes().to_vec());
    let working = resolve_in_workdir(&repo, &path)
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| std::fs::read(p).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();

    Ok(FileContent {
        path,
        working,
        conflicted,
    })
}

/// Write the given content to the working-tree file at `path`, then stage it.
/// Used to save a manually-edited conflict resolution.
#[tauri::command]
fn write_file(repo_path: String, path: String, content: String) -> Result<(), String> {
    let repo = open_repo(&repo_path)?;
    let abs = resolve_in_workdir(&repo, &path)?;
    if let Some(parent) = abs.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&abs, content).map_err(|e| e.to_string())?;

    let mut index = repo.index().map_err(|e| e.to_string())?;
    index
        .add_path(Path::new(&path))
        .map_err(|e| e.to_string())?;
    index.write().map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Highlight {
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
    /// One of: "context", "added", "removed", "modified".
    kind: String,
    /// Left column (base/HEAD). None if this is an "added" row.
    left: Option<DiffSegment>,
    /// Right column (working). None if this is a "removed" row.
    right: Option<DiffSegment>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileDiff {
    path: String,
    rows: Vec<DiffRow>,
}

/// Return the ranges of `line` that differ from `other` — the minimal inner
/// span not shared by both — as UTF-16 code-unit offsets. JS `String.slice`
/// on the frontend indexes by UTF-16 code units, and a byte- or char-indexed
/// range would misalign as soon as the line contains non-BMP or multi-byte
/// characters. Comparing per char (not per byte) also guarantees a span never
/// splits inside a multi-byte character.
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
    if common_prefix >= line_chars.len() - common_suffix {
        return Vec::new();
    }
    // char offset -> UTF-16 code-unit offset
    let utf16_len = |chars: &[char], up_to: usize| -> usize {
        chars[..up_to].iter().map(|c| c.len_utf16()).sum()
    };
    let start = utf16_len(&line_chars, common_prefix);
    let end = utf16_len(&line_chars, line_chars.len() - common_suffix);
    if end > start {
        vec![(start, end)]
    } else {
        Vec::new()
    }
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
async fn get_diff(repo_path: String, path: String) -> Result<FileDiff, String> {
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
        let rows = align_rows(diff_strings(&base, &target)?);
        Ok(FileDiff { path, rows })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?;
    diff
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitSummary {
    id: String,
    short_id: String,
    message: String,
    author: String,
    email: String,
    time: String,
    parents: Vec<String>,
    refs: Vec<String>,
}

fn commit_to_summary(repo: &git2::Repository, commit: &git2::Commit) -> CommitSummary {
    let time = commit.time().seconds();
    let refs = repo
        .references()
        .map(|it| {
            it.filter_map(|r| r.ok())
                .filter(|r| r.target() == Some(commit.id()))
                .filter_map(|r| r.shorthand().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    CommitSummary {
        id: commit.id().to_string(),
        short_id: commit
            .as_object()
            .short_id()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default(),
        message: commit.message().unwrap_or_default().to_string(),
        author: commit.author().name().unwrap_or_default().to_string(),
        email: commit.author().email().unwrap_or_default().to_string(),
        time: time.to_string(),
        parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        refs,
    }
}

#[tauri::command]
fn get_log(repo_path: String, limit: Option<u32>) -> Result<Vec<CommitSummary>, String> {
    let repo = open_repo(&repo_path)?;
    let mut revwalk = repo.revwalk().map_err(|e| e.to_string())?;
    revwalk.push_head().map_err(|e| e.to_string())?;
    revwalk
        .set_sorting(git2::Sort::TIME)
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for oid in revwalk {
        let oid = oid.map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let s = commit_to_summary(&repo, &commit);
        out.push(s);
        if let Some(l) = limit {
            if out.len() as u32 >= l {
                break;
            }
        }
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitFile {
    path: String,
    /// One of: added, deleted, modified, renamed, typechange.
    status: String,
    /// New path if renamed, else empty.
    new_path: String,
}

fn blob_to_string(repo: &git2::Repository, id: git2::Oid) -> Result<String, String> {
    match repo.find_blob(id) {
        Ok(b) => Ok(String::from_utf8_lossy(b.content()).into_owned()),
        Err(_) => Ok(String::new()),
    }
}

fn tree_blob(tree: &git2::Tree, path: &str) -> Option<git2::Oid> {
    tree.get_path(Path::new(path)).ok().map(|e| e.id())
}

/// List files changed in a single commit (diff against its first parent).
#[tauri::command]
fn get_commit_files(repo_path: String, commit_id: String) -> Result<Vec<CommitFile>, String> {
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

    let mut diff_opts = git2::DiffOptions::new();
    let mut files = Vec::new();
    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut diff_opts))
        .map_err(|e| e.to_string())?;

    for delta in diff.deltas() {
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let status = delta_to_status(delta.status());
        let path = if new_path.is_empty() {
            old_path.clone()
        } else {
            new_path.clone()
        };
        files.push(CommitFile {
            path,
            status: status.to_string(),
            new_path: if status == "renamed" {
                new_path
            } else {
                String::new()
            },
        });
    }
    Ok(files)
}

/// Side-by-side diff of one file inside a commit (parent tree vs commit tree).
///
/// The LCS is O(n*m): the whole computation runs off the main thread so a
/// huge file cannot freeze the window while it computes.
#[tauri::command]
async fn get_commit_file_diff(
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
        let rows = align_rows(ops);
        Ok(FileDiff { path, rows })
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?;
    diff
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchInfo {
    name: String,
    is_current: bool,
    /// Full ref name, e.g. refs/heads/main
    refname: String,
}

#[tauri::command]
fn list_branches(repo_path: String) -> Result<Vec<BranchInfo>, String> {
    let repo = open_repo(&repo_path)?;
    let current = repo_head_name(&repo)?;
    let mut out = Vec::new();
    let branches = repo.branches(None).map_err(|e| e.to_string())?;
    for b in branches {
        let (branch, btype) = b.map_err(|e| e.to_string())?;
        let name = branch.name().ok().flatten().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let refname = branch.get().name().unwrap_or("").to_string();
        let short = match btype {
            git2::BranchType::Local => name.clone(),
            git2::BranchType::Remote => name.clone(),
        };
        let is_current = btype == git2::BranchType::Local && short == current;
        out.push(BranchInfo {
            is_current,
            name: short,
            refname,
        });
    }
    // Locals first, then remotes; current branch first.
    out.sort_by(|a, b| b.is_current.cmp(&a.is_current).then(a.name.cmp(&b.name)));
    Ok(out)
}

#[tauri::command]
fn checkout(repo_path: String, branch: String) -> Result<String, String> {
    let repo = open_repo(&repo_path)?;
    // Guarding a branch switch, not just merge: switching to a branch that
    // differs in a file you have uncommitted local changes to can still drop
    // those changes even though checkout_tree's default options are "safe".
    // Mirror the merge/abort path so the guard is uniform across every
    // destructive-looking operation.
    ensure_clean_workdir(&repo)?;
    let object = repo
        .revparse_single(&branch)
        .or_else(|_| repo.revparse_single(&format!("refs/heads/{branch}")))
        .map_err(|e| e.to_string())?;
    repo.checkout_tree(&object, None)
        .map_err(|e| e.to_string())?;
    if !branch.starts_with("refs/") {
        repo.set_head(&format!("refs/heads/{branch}"))
            .map_err(|e| e.to_string())?;
    } else {
        repo.set_head(&branch).map_err(|e| e.to_string())?;
    }
    Ok(format!("checked out {branch}"))
}

#[tauri::command]
fn create_branch(repo_path: String, name: String, from: Option<String>) -> Result<String, String> {
    let repo = open_repo(&repo_path)?;
    let commit = match from {
        Some(f) => {
            let obj = repo.revparse_single(&f).map_err(|e| e.to_string())?;
            obj.peel_to_commit().map_err(|e| e.to_string())?
        }
        None => repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .map_err(|e| e.to_string())?,
    };
    repo.branch(&name, &commit, false)
        .map_err(|e| e.to_string())?;
    Ok(format!("created branch {name}"))
}

#[tauri::command]
fn delete_branch(repo_path: String, name: String) -> Result<String, String> {
    let repo = open_repo(&repo_path)?;
    let mut branch = repo
        .find_branch(&name, git2::BranchType::Local)
        .map_err(|e| e.to_string())?;
    branch.delete().map_err(|e| e.to_string())?;
    Ok(format!("deleted branch {name}"))
}

/// Run a system `git` subcommand in `repo_path` and return trimmed stdout.
/// Network operations (push/fetch/pull) use the system git client because
/// libssh2 on Windows cannot load OpenSSH-format ed25519 keys (git2-rs #659).
fn run_git(repo_path: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("failed to spawn git: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        let msg = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if msg.is_empty() {
            format!("git {:?} failed with {}", args, out.status)
        } else {
            msg
        });
    }
    Ok(stdout)
}

fn nonempty(v: Option<String>, default: &str) -> String {
    v.filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[tauri::command]
fn push(
    repo_path: String,
    remote: Option<String>,
    branch: Option<String>,
) -> Result<String, String> {
    let remote_name = nonempty(remote, "origin");
    let branch_name = branch
        .filter(|b| !b.trim().is_empty())
        .map(|b| b)
        .unwrap_or_else(|| {
            // Fall back to current branch for -u detection / messaging.
            git2::Repository::open(&repo_path)
                .ok()
                .and_then(|r| repo_head_name(&r).ok())
                .unwrap_or_default()
        });

    // Detect whether the current branch has an upstream; if not, push -u.
    let has_upstream = run_git(
        &repo_path,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .is_ok();

    let mut args: Vec<String> = vec!["push".into()];
    if !has_upstream {
        args.push("-u".into());
    }
    args.push(remote_name.clone());
    if !branch_name.is_empty() && branch_name != "detached" && !branch_name.starts_with("HEAD") {
        args.push(branch_name.clone());
    }

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = run_git(&repo_path, &arg_refs)?;
    let msg = if out.is_empty() || out.contains("Everything up-to-date") {
        format!(
            "pushed {} to {remote_name}{}",
            if branch_name.is_empty() {
                "current branch"
            } else {
                &branch_name
            },
            if has_upstream { "" } else { " (set upstream)" }
        )
    } else {
        out
    };
    Ok(msg)
}

#[tauri::command]
fn fetch(repo_path: String, remote: Option<String>) -> Result<String, String> {
    let remote_name = nonempty(remote, "origin");
    run_git(&repo_path, &["fetch", &remote_name])?;
    Ok(format!("fetched from {remote_name}"))
}

#[tauri::command]
fn pull(
    repo_path: String,
    remote: Option<String>,
    branch: Option<String>,
) -> Result<String, String> {
    let remote_name = nonempty(remote, "origin");
    let branch_name = branch.filter(|b| !b.trim().is_empty());
    let mut args = vec!["pull", &remote_name];
    if let Some(b) = &branch_name {
        args.push(b);
    }
    run_git(&repo_path, &args)?;
    Ok(format!(
        "pulled {} from {remote_name}",
        branch_name.unwrap_or_else(|| "current branch".to_string())
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_conflicts,
            commit,
            push,
            merge,
            abort_merge,
            get_status,
            stage,
            unstage,
            resolve_conflict,
            get_file_contents,
            get_diff,
            get_log,
            get_commit_files,
            get_commit_file_diff,
            list_branches,
            checkout,
            create_branch,
            delete_branch,
            fetch,
            pull,
            discard,
            write_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
#[cfg(test)]
mod security_tests {
    use super::*;

    // ── 路径穿越（write_file / discard / resolve_conflict / get_file_contents）──

    #[test]
    fn rel_paths_are_accepted() {
        assert!(validate_rel_path("src/main.rs").is_ok());
        assert!(validate_rel_path("a/b/c.txt").is_ok());
        assert!(validate_rel_path("子目录/文件.txt").is_ok());
    }

    #[test]
    fn traversal_and_absolute_paths_are_rejected() {
        assert!(validate_rel_path("../secrets.txt").is_err());
        assert!(validate_rel_path("a/../../b.txt").is_err());
        assert!(validate_rel_path(r"C:\Windows\system32\config").is_err());
        assert!(validate_rel_path("/etc/passwd").is_err());
        assert!(validate_rel_path(r"\\server\share\x").is_err());
    }

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
}
