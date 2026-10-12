//! Rebase commands: replay the current branch onto a target branch.
//!
//! Rebase runs through the system `git` client (same precedent as push/fetch/
//! pull in `remote.rs`): libgit2's rebase state machine would have to
//! re-implement conflict continuation (`--continue` / `--abort`, message
//! preservation, backend-specific state under `.git/rebase-merge`), while git
//! owns all of that natively. Conflict detection afterwards reads the index,
//! so the result shape is exactly the merge one and the frontend reuses the
//! same conflict UI.

use crate::merge::{index_conflict_paths, MergeResult};
use crate::repo::{ensure_clean_workdir, head_oid, open_repo, repo_head_name, run_git};

/// True while a rebase (started here or from the git CLI) is in progress.
#[tauri::command]
pub(crate) fn rebase_in_progress(repo_path: String) -> Result<bool, String> {
    rebase_in_progress_impl(&repo_path)
}

/// Filesystem form of `rebase_in_progress`. Rebase state lives under
/// `.git/rebase-merge` or `.git/rebase-apply`, so this is an O(1) reduction of
/// "is a replay running" to two `exists()` calls, and it never depends on git's
/// wording — which is localized. Everything that needs this inside a command
/// uses it instead of re-parsing git's output.
pub(crate) fn rebase_in_progress_impl(repo_path: &str) -> Result<bool, String> {
    let repo = open_repo(repo_path)?;
    let gitdir = repo.path().to_path_buf();
    Ok(gitdir.join("rebase-merge").exists() || gitdir.join("rebase-apply").exists())
}

/// `git rebase <branch>`: replay the commits of the current branch onto
/// `branch`. Returns the same `MergeResult` shape as `merge` so conflicts
/// land in the shared conflict UI.
#[tauri::command]
pub(crate) async fn rebase(repo_path: String, branch: String) -> Result<MergeResult, String> {
    tauri::async_runtime::spawn_blocking(move || rebase_impl(repo_path, branch))
        .await
        .map_err(|e| format!("rebase task failed: {e}"))?
}

fn rebase_impl(repo_path: String, branch: String) -> Result<MergeResult, String> {
    let repo = open_repo(&repo_path)?;
    let current = repo_head_name(&repo)?;
    if branch == current {
        return Err(format!("「{branch}」已经是当前分支，无需变基"));
    }
    // git refuses a dirty tree by itself; pre-check for the shared, clearer
    // message (the same guard checkout/merge use).
    ensure_clean_workdir(&repo)?;

    // Snapshot the branch before the replay: "already up to date" is the
    // outcome that most needs a locale-independent check, because git reports
    // it in words ("已经是最新的。" under zh) and the branch itself says
    // nothing happened when HEAD comes back exactly where it was.
    let head_before = head_oid(&repo);

    match run_git(&repo_path, &["rebase", &branch]) {
        Ok(_out) => {
            // Reopen: libgit2 may still hold the refs as git found them, and
            // the rebase just rewrote .git/HEAD and the packed refs.
            let head_after = head_oid(&open_repo(&repo_path)?);
            let up_to_date = head_before == head_after;
            Ok(MergeResult {
                status: if up_to_date { "upToDate" } else { "rebased" }.into(),
                message: if up_to_date {
                    format!("「{current}」已是最新，无需变基到「{branch}」")
                } else {
                    format!("已将「{current}」变基到「{branch}」")
                },
                conflicts: vec![],
            })
        }
        Err(e) => {
            // A conflicted replay leaves the rebase open with conflicted index
            // entries: a business outcome (like merge), not an exception.
            let conflicts = index_conflict_paths(&open_repo(&repo_path)?);
            if !conflicts.is_empty() {
                return Ok(MergeResult {
                    status: "conflicts".into(),
                    message: format!(
                        "变基「{current}」到「{branch}」时有 {} 个冲突，请在 Changes 中解决，\
                         然后 Rebase → Continue，或 Rebase → Abort 放弃",
                        conflicts.len()
                    ),
                    conflicts,
                });
            }
            Err(e)
        }
    }
}

/// `git rebase --continue` after every conflict is resolved. The original
/// commit message is preserved without opening an editor (`core.editor=true`
/// keeps git's buffer as-is).
#[tauri::command]
pub(crate) async fn rebase_continue(repo_path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || rebase_continue_impl(repo_path))
        .await
        .map_err(|e| format!("rebase_continue task failed: {e}"))?
}

fn rebase_continue_impl(repo_path: String) -> Result<String, String> {
    match run_git(
        &repo_path,
        &["-c", "core.editor=true", "rebase", "--continue"],
    ) {
        Ok(out) => {
            // git's verdict here is "Successfully rebased and updated ...",
            // which is localized — and some success paths print nothing at
            // all. The replay is finished exactly when .git/rebase-* is gone.
            // If git exited 0 while a rebase still runs, it paused for another
            // step, so hand its own text back to the UI.
            let finished = !rebase_in_progress_impl(&repo_path).unwrap_or(true);
            Ok(if finished {
                "变基已完成".to_string()
            } else {
                out
            })
        }
        Err(e) => {
            // "No rebase in progress" is localized too, in both
            // capitalizations. The missing .git/rebase-* directory is not, so
            // that is what decides. When the state cannot be read, assume a
            // replay is running and pass the error through untouched.
            if !rebase_in_progress_impl(&repo_path).unwrap_or(true) {
                return Err("当前没有进行中的变基".to_string());
            }
            let conflicts = index_conflict_paths(&open_repo(&repo_path)?);
            if !conflicts.is_empty() {
                return Err(format!("仍有 {} 个冲突未解决：{e}", conflicts.len()));
            }
            Err(e)
        }
    }
}

/// `git rebase --abort`: restores the pre-rebase branch state.
#[tauri::command]
pub(crate) async fn rebase_abort(repo_path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || rebase_abort_impl(repo_path))
        .await
        .map_err(|e| format!("rebase_abort task failed: {e}"))?
}

fn rebase_abort_impl(repo_path: String) -> Result<String, String> {
    match run_git(&repo_path, &["rebase", "--abort"]) {
        Ok(_) => Ok("已中止变基".to_string()),
        Err(e) => {
            // Same reduction as in `rebase_continue`: decide from
            // .git/rebase-*, not from git's wording.
            if !rebase_in_progress_impl(&repo_path).unwrap_or(true) {
                Err("当前没有进行中的变基".to_string())
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::working::resolve_conflict_impl;

    struct TempRepo {
        path: std::path::PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "idea-git-tool-rebase-{}-{}",
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

        /// Move HEAD (and the workdir) to an existing local branch.
        fn switch_branch(&self, name: &str) {
            let repo = open_repo(&self.path_str()).unwrap();
            repo.set_head(&format!("refs/heads/{name}")).unwrap();
            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
                .unwrap();
        }

        fn head_msg(&self) -> String {
            open_repo(&self.path_str())
                .unwrap()
                .head()
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .message()
                .unwrap_or("")
                .to_string()
        }

        fn head_branch(&self) -> String {
            repo_head_name(&open_repo(&self.path_str()).unwrap()).unwrap()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn rebase_replays_current_commits_onto_target() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "v1\n", "base");

        // feature keeps pace from the shared base; master adds its own commit.
        let r = open_repo(&repo.path_str()).unwrap();
        let base = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("feature", &base, false).unwrap();
        repo.commit_file("m.txt", "master\n", "master work");

        repo.switch_branch("feature");
        repo.commit_file("f.txt", "feature\n", "feature work");
        repo.switch_branch("master");

        let result = rebase_impl(repo.path_str(), "feature".to_string()).unwrap();
        assert_eq!(result.status, "rebased", "{}", result.message);
        assert!(result.conflicts.is_empty());

        // master = base → feature → master-work: replayed on top, message kept.
        assert_eq!(repo.head_branch(), "master");
        assert_eq!(repo.head_msg(), "master work");
        assert!(
            repo.path.join("f.txt").exists(),
            "feature commit contents arrived"
        );
        assert!(repo.path.join("m.txt").exists(), "master commit survives");
    }

    #[test]
    fn rebase_is_a_no_op_when_target_is_an_ancestor() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "v1\n", "base");

        let r = open_repo(&repo.path_str()).unwrap();
        let base = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("feature", &base, false).unwrap();
        // master is ahead; feature never moved — already based on the target.
        repo.commit_file("m.txt", "master\n", "master work");

        let result = rebase_impl(repo.path_str(), "feature".to_string()).unwrap();
        assert_eq!(result.status, "upToDate", "{}", result.message);
        assert_eq!(repo.head_msg(), "master work");
    }

    #[test]
    fn rebase_reports_conflicts_then_abort_restores_state() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "base\n", "base");

        let r = open_repo(&repo.path_str()).unwrap();
        let base = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("feature", &base, false).unwrap();

        // Divergent edits of the same line.
        repo.commit_file("a.txt", "trunk\n", "trunk edit");
        repo.switch_branch("feature");
        repo.commit_file("a.txt", "feature\n", "feature edit");
        repo.switch_branch("master");

        let result = rebase_impl(repo.path_str(), "feature".to_string()).unwrap();
        assert_eq!(result.status, "conflicts");
        assert!(result.conflicts.iter().any(|p| p == "a.txt"));
        assert!(rebase_in_progress(repo.path_str()).unwrap());

        rebase_abort_impl(repo.path_str()).unwrap();
        assert!(!rebase_in_progress(repo.path_str()).unwrap());
        assert_eq!(
            repo.head_msg(),
            "trunk edit",
            "abort restores the original tip"
        );
        assert_eq!(
            std::fs::read_to_string(repo.path.join("a.txt")).unwrap(),
            "trunk\n"
        );
    }

    #[test]
    fn resolve_conflicts_then_continue_finishes_the_rebase() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "base\n", "base");

        let r = open_repo(&repo.path_str()).unwrap();
        let base = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("feature", &base, false).unwrap();

        repo.commit_file("a.txt", "trunk\n", "trunk edit");
        repo.switch_branch("feature");
        repo.commit_file("a.txt", "feature\n", "feature edit");
        repo.switch_branch("master");

        let result = rebase_impl(repo.path_str(), "feature".to_string()).unwrap();
        assert_eq!(result.status, "conflicts");

        // Resolve via the shared conflict path, then continue. Choosing
        // "theirs" (the replayed commit's content) keeps that commit non-empty;
        // choosing "ours" would make it empty and git would drop it.
        resolve_conflict_impl(
            repo.path_str(),
            vec!["a.txt".to_string()],
            "theirs".to_string(),
        )
        .unwrap();
        rebase_continue_impl(repo.path_str()).unwrap();

        assert!(!rebase_in_progress(repo.path_str()).unwrap());
        // The replayed commit keeps its original message (git normalizes a
        // trailing newline when writing it after --continue) and content.
        assert_eq!(repo.head_msg().trim_end(), "trunk edit");
        assert_eq!(
            std::fs::read_to_string(repo.path.join("a.txt")).unwrap(),
            "trunk\n"
        );
    }

    #[test]
    fn rebase_rejects_same_branch_and_dirty_trees() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "v1\n", "base");
        let r = open_repo(&repo.path_str()).unwrap();
        let base = r.head().unwrap().peel_to_commit().unwrap();
        r.branch("feature", &base, false).unwrap();

        let err = rebase_impl(repo.path_str(), "master".to_string()).unwrap_err();
        assert!(err.contains("已经是当前分支"), "{err}");

        std::fs::write(repo.path.join("a.txt"), "dirty\n").unwrap();
        let err = rebase_impl(repo.path_str(), "feature".to_string()).unwrap_err();
        assert!(err.contains("未提交的修改"), "{err}");
    }

    /// A replay that changes nothing must be reported as up to date. git says
    /// so in words — "already up to date" in English, 已经是最新的 under zh —
    /// which is what this used to be a substring match on; the branch id now
    /// decides, and it is translated by nobody.
    #[test]
    fn rebase_already_up_to_date_is_detected_from_the_branch() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "base\n", "base");
        let before = {
            let r = open_repo(&repo.path_str()).unwrap();
            let base = r.head().unwrap().peel_to_commit().unwrap();
            r.branch("upstream", &base, false).unwrap();
            head_oid(&r).unwrap()
        };

        let result = rebase_impl(repo.path_str(), "upstream".to_string()).unwrap();
        assert_eq!(result.status, "upToDate", "{}", result.message);
        assert!(result.message.contains("已是最新"), "{}", result.message);

        let after = head_oid(&open_repo(&repo.path_str()).unwrap()).unwrap();
        assert_eq!(before, after, "an up-to-date rebase must not move HEAD");
    }

    /// The mirror case: a replay that really does move the branch must not be
    /// reported as up to date, so the id comparison is not always saying no.
    #[test]
    fn a_moving_rebase_is_not_reported_as_up_to_date() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "base\n", "base");
        let before = {
            let r = open_repo(&repo.path_str()).unwrap();
            let base = r.head().unwrap().peel_to_commit().unwrap();
            r.branch("feature", &base, false).unwrap();
            head_oid(&r).unwrap()
        };

        // Different files, so the replay has nothing to conflict on.
        repo.switch_branch("feature");
        repo.commit_file("b.txt", "feature\n", "feature edit");
        repo.switch_branch("master");
        repo.commit_file("a.txt", "trunk\n", "trunk edit");

        let result = rebase_impl(repo.path_str(), "feature".to_string()).unwrap();
        assert_eq!(result.status, "rebased", "{}", result.message);

        let after = head_oid(&open_repo(&repo.path_str()).unwrap()).unwrap();
        assert_ne!(before, after);
    }

    /// `--continue` and `--abort` used to recognise "No rebase in progress"
    /// from git's wording, spelling it two ways and no other language. The
    /// absence of .git/rebase-merge and .git/rebase-apply decides instead, and
    /// the user still gets the friendly message rather than raw stderr.
    #[test]
    fn continue_and_abort_with_no_rebase_running_use_the_friendly_message() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "base\n", "base");

        let err = rebase_continue_impl(repo.path_str()).unwrap_err();
        assert_eq!(err, "当前没有进行中的变基");

        let err = rebase_abort_impl(repo.path_str()).unwrap_err();
        assert_eq!(err, "当前没有进行中的变基");
    }
}
