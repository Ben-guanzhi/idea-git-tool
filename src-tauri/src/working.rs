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

#[tauri::command]
pub(crate) fn get_status(repo_path: String) -> Result<StatusInfo, String> {
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
pub(crate) fn stage(repo_path: String, paths: Vec<String>) -> Result<(), String> {
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
pub(crate) fn discard(repo_path: String, paths: Vec<String>) -> Result<(), String> {
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
pub(crate) fn unstage(repo_path: String, paths: Vec<String>) -> Result<(), String> {
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
pub(crate) fn resolve_conflict(
    repo_path: String,
    paths: Vec<String>,
    choice: String,
) -> Result<(), String> {
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
pub(crate) fn commit(args: CommitArgs) -> Result<String, String> {
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
pub(crate) fn get_conflicts(repo_path: String) -> Result<Vec<ConflictInfo>, String> {
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
pub(crate) fn get_file_contents(repo_path: String, path: String) -> Result<FileContent, String> {
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
pub(crate) fn write_file(repo_path: String, path: String, content: String) -> Result<(), String> {
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
