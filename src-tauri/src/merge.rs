//! Merge commands and index-conflict helpers.

use serde::Serialize;

use crate::repo::{current_branch_name, ensure_clean_workdir, open_repo, run_git};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MergeResult {
    /// One of: merged, fastForward, upToDate, conflicts.
    status: String,
    message: String,
    /// Paths with conflicts when status == "conflicts".
    conflicts: Vec<String>,
}

/// One deduplicated record per conflicted path: `(path, our, their, ancestor)`,
/// the path taken from the first side that exists.
pub(crate) fn index_conflicts(
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
pub(crate) fn merge(repo_path: String, branch: String) -> Result<MergeResult, String> {
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
pub(crate) fn abort_merge(repo_path: String) -> Result<String, String> {
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
