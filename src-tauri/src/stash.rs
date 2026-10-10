//! Stash commands: list / save / apply / pop / drop (libgit2, local-only).

use serde::Serialize;

use crate::repo::open_repo;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StashInfo {
    /// Position in the stash list (0 = newest), matching `git stash list` order.
    index: usize,
    message: String,
}

#[tauri::command]
pub(crate) fn list_stashes(repo_path: String) -> Result<Vec<StashInfo>, String> {
    let mut repo = open_repo(&repo_path)?;
    let mut out = Vec::new();
    repo.stash_foreach(|index, message, _oid| {
        out.push(StashInfo {
            index,
            message: message.to_string(),
        });
        true
    })
    .map_err(|e| e.to_string())?;
    Ok(out)
}

/// `git stash push -m <message>`; an empty/absent message lets libgit2
/// generate git's own "WIP on <branch>" default.
#[tauri::command]
pub(crate) fn stash_save(repo_path: String, message: Option<String>) -> Result<String, String> {
    let mut repo = open_repo(&repo_path)?;
    let sig = repo.signature().map_err(|e| e.to_string())?;
    let msg = message.filter(|m| !m.trim().is_empty());
    repo.stash_save2(&sig, msg.as_deref(), None).map_err(|e| {
        if e.code() == git2::ErrorCode::NotFound {
            "没有可暂存的改动（工作区与索引均干净）".to_string()
        } else {
            e.to_string()
        }
    })?;
    Ok("已暂存改动（stash）".to_string())
}

/// `git stash apply <index>`: the stash is kept afterwards, mirroring git.
#[tauri::command]
pub(crate) fn stash_apply(repo_path: String, index: usize) -> Result<String, String> {
    let mut repo = open_repo(&repo_path)?;
    repo.stash_apply(index, None).map_err(|e| e.to_string())?;
    Ok(format!("已应用 stash@{{{index}}}（条目保留）"))
}

/// `git stash pop <index>`: apply, and drop only when the apply succeeded —
/// git2 has a combined operation, so a failed apply never loses the stash.
#[tauri::command]
pub(crate) fn stash_pop(repo_path: String, index: usize) -> Result<String, String> {
    let mut repo = open_repo(&repo_path)?;
    repo.stash_pop(index, None).map_err(|e| e.to_string())?;
    Ok(format!("已弹出 stash@{{{index}}}"))
}

#[tauri::command]
pub(crate) fn stash_drop(repo_path: String, index: usize) -> Result<String, String> {
    let mut repo = open_repo(&repo_path)?;
    repo.stash_drop(index).map_err(|e| e.to_string())?;
    Ok(format!("已丢弃 stash@{{{index}}}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Throwaway repo helper (same pid+counter pattern as the other modules).
    struct TempRepo {
        path: std::path::PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "idea-git-tool-stash-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let repo = git2::Repository::init(&dir).unwrap();
            {
                let mut cfg = repo.config().unwrap();
                cfg.set_str("core.autocrlf", "false").unwrap();
                cfg.set_str("user.name", "Test").unwrap();
                cfg.set_str("user.email", "test@example.com").unwrap();
            }
            TempRepo { path: dir }
        }

        fn path_str(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }

        fn commit_file(&self, rel: &str, content: &str, msg: &str) {
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
                .unwrap();
        }

        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.path.join(rel)).unwrap()
        }

        /// Reset workdir+index to HEAD (test scaffolding for apply/pop rounds).
        fn reset_hard(&self) {
            let repo = open_repo(&self.path_str()).unwrap();
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            repo.reset(head.as_object(), git2::ResetType::Hard, None)
                .unwrap();
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn stash_save_apply_pop_drop_round_trip() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "v1\n", "first");

        // save: workdir returns to HEAD, the stash appears in the list.
        std::fs::write(repo.path.join("a.txt"), "wip\n").unwrap();
        stash_save(repo.path_str(), Some("my wip".into())).unwrap();
        assert_eq!(repo.read("a.txt"), "v1\n", "save must revert the workdir");
        let stashes = list_stashes(repo.path_str()).unwrap();
        assert_eq!(stashes.len(), 1);
        assert_eq!(stashes[0].index, 0);
        assert!(
            stashes[0].message.contains("my wip"),
            "{:?}",
            stashes[0].message
        );

        // apply: change comes back, the stash stays (git semantics).
        stash_apply(repo.path_str(), 0).unwrap();
        assert_eq!(repo.read("a.txt"), "wip\n");
        assert_eq!(list_stashes(repo.path_str()).unwrap().len(), 1);
        repo.reset_hard();

        // pop: applied and removed in one step.
        stash_pop(repo.path_str(), 0).unwrap();
        assert_eq!(repo.read("a.txt"), "wip\n");
        assert!(list_stashes(repo.path_str()).unwrap().is_empty());
        repo.reset_hard();

        // drop: removes without touching the workdir.
        std::fs::write(repo.path.join("a.txt"), "gone\n").unwrap();
        stash_save(repo.path_str(), None).unwrap();
        assert_eq!(list_stashes(repo.path_str()).unwrap().len(), 1);
        stash_drop(repo.path_str(), 0).unwrap();
        assert!(list_stashes(repo.path_str()).unwrap().is_empty());
        assert_eq!(repo.read("a.txt"), "v1\n");
    }

    #[test]
    fn saving_a_clean_tree_fails_with_a_clear_message() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "v1\n", "first");
        let err = stash_save(repo.path_str(), None).unwrap_err();
        assert!(err.contains("没有可暂存的改动"), "{err}");
    }

    #[test]
    fn out_of_range_indexes_are_errors() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "v1\n", "first");
        assert!(stash_apply(repo.path_str(), 3).is_err());
        assert!(stash_pop(repo.path_str(), 3).is_err());
        assert!(stash_drop(repo.path_str(), 3).is_err());
    }
}
