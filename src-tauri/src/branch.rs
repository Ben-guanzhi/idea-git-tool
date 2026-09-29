//! Branch listing / checkout / create / delete commands.

use serde::Serialize;

use crate::repo::{ensure_clean_workdir, open_repo, repo_head_name};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BranchInfo {
    name: String,
    is_current: bool,
    /// Full ref name, e.g. refs/heads/main
    refname: String,
}

#[tauri::command]
pub(crate) fn list_branches(repo_path: String) -> Result<Vec<BranchInfo>, String> {
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
pub(crate) fn checkout(repo_path: String, branch: String) -> Result<String, String> {
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
pub(crate) fn create_branch(
    repo_path: String,
    name: String,
    from: Option<String>,
) -> Result<String, String> {
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
pub(crate) fn delete_branch(repo_path: String, name: String) -> Result<String, String> {
    let repo = open_repo(&repo_path)?;
    let mut branch = repo
        .find_branch(&name, git2::BranchType::Local)
        .map_err(|e| e.to_string())?;
    branch.delete().map_err(|e| e.to_string())?;
    Ok(format!("deleted branch {name}"))
}
