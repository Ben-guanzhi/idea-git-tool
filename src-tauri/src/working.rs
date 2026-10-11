//! Working-area commands: status, staging, commit, file contents.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::merge::index_conflicts;
use crate::repo::{
    bytes_are_binary, delta_to_status, open_repo, repo_head_name, resolve_in_workdir, sig_for,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConflictInfo {
    path: String,
    ours: String,
    theirs: String,
    base: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitArgs {
    repo_path: String,
    message: String,
    author_name: String,
    author_email: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusInfo {
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

// The working-area commands all touch the filesystem / git index and can take
// tens of seconds on large repos (a full status scan, write_tree during
// commit, checkout during discard). Synchronous commands execute on the main
// thread and would freeze the webview, so each runs on the blocking pool via
// spawn_blocking and only the thin wrapper is the #[tauri::command].

#[tauri::command]
pub(crate) async fn get_status(repo_path: String) -> Result<StatusInfo, String> {
    tauri::async_runtime::spawn_blocking(move || get_status_impl(repo_path))
        .await
        .map_err(|e| format!("get_status task failed: {e}"))?
}

fn get_status_impl(repo_path: String) -> Result<StatusInfo, String> {
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
pub(crate) async fn stage(repo_path: String, paths: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || stage_impl(repo_path, paths))
        .await
        .map_err(|e| format!("stage task failed: {e}"))?
}

fn stage_impl(repo_path: String, paths: Vec<String>) -> Result<(), String> {
    let repo = open_repo(&repo_path)?;
    let mut index = repo.index().map_err(|e| e.to_string())?;
    for p in &paths {
        // Reject paths that would stage something outside the workdir (a
        // symlink-relative name or a sibling path) before the disk exists
        // check, so a repo containing a symlinked dir cannot add files from
        // outside the repository.
        let abs = resolve_in_workdir(&repo, p)?;
        let exists = abs.exists();
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
pub(crate) async fn discard(repo_path: String, paths: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || discard_impl(repo_path, paths))
        .await
        .map_err(|e| format!("discard task failed: {e}"))?
}

fn discard_impl(repo_path: String, paths: Vec<String>) -> Result<(), String> {
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
pub(crate) async fn unstage(repo_path: String, paths: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || unstage_impl(repo_path, paths))
        .await
        .map_err(|e| format!("unstage task failed: {e}"))?
}

fn unstage_impl(repo_path: String, paths: Vec<String>) -> Result<(), String> {
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
pub(crate) async fn resolve_conflict(
    repo_path: String,
    paths: Vec<String>,
    choice: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || resolve_conflict_impl(repo_path, paths, choice))
        .await
        .map_err(|e| format!("resolve_conflict task failed: {e}"))?
}

pub(crate) fn resolve_conflict_impl(
    repo_path: String,
    paths: Vec<String>,
    choice: String,
) -> Result<(), String> {
    let repo = open_repo(&repo_path)?;
    let mut index = repo.index().map_err(|e| e.to_string())?;

    for p in &paths {
        let path = Path::new(p);
        // Index stages: 1 = merge base, 2 = ours (HEAD), 3 = theirs. (This
        // used to read stage 1 as "ours", silently resolving conflicts to the
        // *ancestor* content.) Stage 0 fallback covers already-resolved paths.
        let ours_id = index
            .get_path(path, 2)
            .or_else(|| index.get_path(path, 0))
            .map(|e| e.id);
        let theirs_id = index.get_path(path, 3).map(|e| e.id);

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
pub(crate) async fn commit(args: CommitArgs) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || commit_impl(args))
        .await
        .map_err(|e| format!("commit task failed: {e}"))?
}

fn commit_impl(args: CommitArgs) -> Result<String, String> {
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

#[tauri::command]
pub(crate) async fn get_conflicts(repo_path: String) -> Result<Vec<ConflictInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || get_conflicts_impl(repo_path))
        .await
        .map_err(|e| format!("get_conflicts task failed: {e}"))?
}

fn get_conflicts_impl(repo_path: String) -> Result<Vec<ConflictInfo>, String> {
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
pub(crate) struct FileContent {
    path: String,
    /// Content from the working tree, or empty if the file does not exist there.
    /// Empty for binary files too — those are not shown as lossy text.
    working: String,
    /// True if there is an active merge conflict for this path.
    conflicted: bool,
    /// True when the working-tree bytes cannot round-trip through the text
    /// editor (NUL byte or invalid UTF-8); the UI must not offer edits.
    is_binary: bool,
}

#[tauri::command]
pub(crate) async fn get_file_contents(
    repo_path: String,
    path: String,
) -> Result<FileContent, String> {
    tauri::async_runtime::spawn_blocking(move || get_file_contents_impl(repo_path, path))
        .await
        .map_err(|e| format!("get_file_contents task failed: {e}"))?
}

fn get_file_contents_impl(repo_path: String, path: String) -> Result<FileContent, String> {
    let repo = open_repo(&repo_path)?;
    let conflicted = index_conflicts(&repo)
        .iter()
        .any(|(p, ..)| *p == path.as_bytes().to_vec());
    let bytes = resolve_in_workdir(&repo, &path)
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| std::fs::read(p).ok())
        .unwrap_or_default();
    let is_binary = bytes_are_binary(&bytes);
    let working = if is_binary {
        String::new()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };

    Ok(FileContent {
        path,
        working,
        conflicted,
        is_binary,
    })
}

/// Write the given content to the working-tree file at `path`, then stage it.
/// Used to save a manually-edited conflict resolution.
#[tauri::command]
pub(crate) async fn write_file(
    repo_path: String,
    path: String,
    content: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || write_file_impl(repo_path, path, content))
        .await
        .map_err(|e| format!("write_file task failed: {e}"))?
}

fn write_file_impl(repo_path: String, path: String, content: String) -> Result<(), String> {
    let repo = open_repo(&repo_path)?;
    let abs = resolve_in_workdir(&repo, &path)?;
    // Refuse to overwrite a binary file with editor text: what the client
    // holds is at best a lossy rendering, and saving it would destroy bytes.
    if let Ok(existing) = std::fs::read(&abs) {
        if bytes_are_binary(&existing) {
            return Err(format!("{path} 是二进制文件，无法按文本保存"));
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A throwaway git repo in the OS temp dir. No `tempfile` crate in the tree,
    /// so the directory is named from a counter plus the process id and removed
    /// on drop; collisions across processes are avoided by the pid, within a
    /// process by the atomic counter.
    struct TempRepo {
        path: PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "idea-git-tool-test-{}-{}",
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
            let abs = self.path.join(rel);
            if let Some(p) = abs.parent() {
                std::fs::create_dir_all(p).unwrap();
            }
            std::fs::write(abs, content).unwrap();
        }

        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.path.join(rel)).unwrap_or_default()
        }

        fn exists(&self, rel: &str) -> bool {
            self.path.join(rel).exists()
        }

        fn commit_args(&self, message: &str) -> CommitArgs {
            CommitArgs {
                repo_path: self.path_str(),
                message: message.to_string(),
                author_name: "Test".to_string(),
                author_email: "test@example.com".to_string(),
            }
        }

        #[allow(dead_code)]
        fn commit_all(&self, message: &str) -> String {
            let repo = open_repo(&self.path_str()).unwrap();
            let mut index = repo.index().unwrap();
            index
                .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            index.write().unwrap();
            commit_impl(self.commit_args(message)).unwrap()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn stage_and_unstage_move_files_between_index_and_workdir() {
        let repo = TempRepo::new();
        repo.write("a.txt", "hello\n");

        // Untracked file appears in the unstaged list as "modified" (the code's
        // default when index_to_workdir yields no delta for a WT_NEW entry).
        let status = get_status_impl(repo.path_str()).unwrap();
        assert_eq!(status.unstaged.len(), 1);
        assert_eq!(status.unstaged[0].status, "modified");
        assert!(status.staged.is_empty());

        // Once staged against a HEAD-less index the entry is reported "added".
        stage_impl(repo.path_str(), vec!["a.txt".to_string()]).unwrap();
        let status = get_status_impl(repo.path_str()).unwrap();
        assert!(status.unstaged.is_empty());
        assert_eq!(status.staged.len(), 1);
        assert_eq!(status.staged[0].status, "added");

        // Unstaging returns it to the plain working-tree entry.
        unstage_impl(repo.path_str(), vec!["a.txt".to_string()]).unwrap();
        let status = get_status_impl(repo.path_str()).unwrap();
        assert!(status.staged.is_empty());
        assert_eq!(status.unstaged.len(), 1);
        assert_eq!(status.unstaged[0].status, "modified");
    }

    #[test]
    fn commit_refuses_when_nothing_is_staged() {
        let repo = TempRepo::new();
        repo.write("a.txt", "hi\n");
        stage_impl(repo.path_str(), vec!["a.txt".to_string()]).unwrap();
        commit_impl(repo.commit_args("first")).unwrap();

        // Nothing changed since the first commit: the second must be refused
        // rather than producing an empty commit.
        let err = commit_impl(repo.commit_args("second")).unwrap_err();
        assert!(err.contains("nothing to commit"), "got: {err}");
    }

    #[test]
    fn commit_records_staged_content() {
        let repo = TempRepo::new();
        repo.write("a.txt", "v1\n");
        stage_impl(repo.path_str(), vec!["a.txt".to_string()]).unwrap();
        let id = commit_impl(repo.commit_args("first")).unwrap();

        let r = open_repo(&repo.path_str()).unwrap();
        let commit = r.find_commit(git2::Oid::from_str(&id).unwrap()).unwrap();
        let tree = commit.tree().unwrap();
        let entry = tree.get_path(Path::new("a.txt")).unwrap();
        let blob = r.find_blob(entry.id()).unwrap();
        assert_eq!(blob.content(), b"v1\n");
    }

    #[test]
    fn discard_restores_tracked_file_and_removes_untracked() {
        let repo = TempRepo::new();
        repo.write("tracked.txt", "original\n");
        repo.write("untracked.txt", "temp\n");
        stage_impl(repo.path_str(), vec!["tracked.txt".to_string()]).unwrap();
        // untracked.txt stays out of the index.
        let _ = get_status_impl(repo.path_str()).unwrap();
        commit_impl(repo.commit_args("base")).unwrap();

        // Modify tracked, keep an untracked file around.
        repo.write("tracked.txt", "changed\n");
        repo.write("untracked.txt", "temp\n");

        discard_impl(
            repo.path_str(),
            vec!["tracked.txt".to_string(), "untracked.txt".to_string()],
        )
        .unwrap();

        assert_eq!(repo.read("tracked.txt"), "original\n");
        assert!(!repo.exists("untracked.txt"));
    }

    #[test]
    fn write_file_refuses_to_clobber_binary() {
        let repo = TempRepo::new();
        // NUL byte makes it binary.
        let abs = repo.path.join("bin.dat");
        std::fs::write(&abs, [0u8, 1, 2, 3]).unwrap();
        let err = write_file_impl(repo.path_str(), "bin.dat".to_string(), "text".to_string())
            .unwrap_err();
        assert!(err.contains("二进制"), "got: {err}");
        // The binary bytes must survive the refused write.
        assert_eq!(std::fs::read(&abs).unwrap(), vec![0u8, 1, 2, 3]);
    }

    #[test]
    fn write_file_rejects_path_traversal() {
        let repo = TempRepo::new();
        let err = write_file_impl(
            repo.path_str(),
            "../escape.txt".to_string(),
            "x".to_string(),
        )
        .unwrap_err();
        assert!(err.contains("非法路径"), "got: {err}");
    }
}
