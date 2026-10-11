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
pub(crate) async fn checkout(repo_path: String, branch: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || checkout_impl(repo_path, branch))
        .await
        .map_err(|e| format!("checkout task failed: {e}"))?
}

fn checkout_impl(repo_path: String, branch: String) -> Result<String, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Throwaway repo helper (see working.rs tests for the rationale): no
    /// `tempfile` dependency, so a pid+counter temp directory removed on drop.
    struct TempRepo {
        path: std::path::PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "idea-git-tool-branch-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let repo = git2::Repository::init(&dir).unwrap();
            {
                let mut cfg = repo.config().unwrap();
                cfg.set_str("user.name", "Test").unwrap();
                cfg.set_str("user.email", "test@example.com").unwrap();
            }
            TempRepo { path: dir }
        }

        fn path_str(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }

        fn commit_file(&self, rel: &str, content: &str, msg: &str) -> git2::Oid {
            let repo = open_repo(&self.path_str()).unwrap();
            std::fs::write(self.path.join(rel), content).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(std::path::Path::new(rel)).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let sig = repo.signature().unwrap();
            let parents: Vec<git2::Commit> = repo
                .head()
                .ok()
                .and_then(|h| h.peel_to_commit().ok())
                .into_iter()
                .collect();
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)
                .unwrap()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn create_list_checkout_delete_round_trip() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "v1\n", "first");

        create_branch(repo.path_str(), "feature".to_string(), None).unwrap();

        let branches = list_branches(repo.path_str()).unwrap();
        assert!(branches
            .iter()
            .any(|b| b.name == "feature" && !b.is_current));
        assert!(branches
            .iter()
            .any(|b| b.name == "master" || b.name == "main"));

        checkout_impl(repo.path_str(), "feature".to_string()).unwrap();
        let branches = list_branches(repo.path_str()).unwrap();
        let feature = branches.iter().find(|b| b.name == "feature").unwrap();
        assert!(
            feature.is_current,
            "feature should be current after checkout"
        );

        // Deleting the current branch must fail (libgit2 refuses).
        let err = delete_branch(repo.path_str(), "feature".to_string()).unwrap_err();
        assert!(!err.is_empty());

        // Checkout back, then delete succeeds.
        let trunk = branches
            .iter()
            .find(|b| b.name == "master" || b.name == "main")
            .unwrap()
            .name
            .clone();
        checkout_impl(repo.path_str(), trunk).unwrap();
        delete_branch(repo.path_str(), "feature".to_string()).unwrap();
        let branches = list_branches(repo.path_str()).unwrap();
        assert!(!branches.iter().any(|b| b.name == "feature"));
    }
}
