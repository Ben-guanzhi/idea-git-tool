//! Commit log and per-commit file listing commands.

use serde::Serialize;
use std::collections::HashMap;

use crate::repo::{delta_to_status, open_repo};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitSummary {
    id: String,
    short_id: String,
    message: String,
    author: String,
    email: String,
    time: String,
    parents: Vec<String>,
    refs: Vec<String>,
}

/// Which names point at each commit, gathered in one pass. Scanning every
/// reference for every commit made the log Θ(commits × references) and read
/// the whole refs store once per row of the graph.
fn refs_by_target(repo: &git2::Repository) -> HashMap<git2::Oid, Vec<String>> {
    let mut map: HashMap<git2::Oid, Vec<String>> = HashMap::new();
    if let Ok(refs) = repo.references() {
        for r in refs.flatten() {
            if let (Some(target), Some(name)) = (r.target(), r.shorthand()) {
                map.entry(target).or_default().push(name.to_string());
            }
        }
    }
    map
}

fn commit_to_summary(
    commit: &git2::Commit,
    refs: &HashMap<git2::Oid, Vec<String>>,
) -> CommitSummary {
    let time = commit.time().seconds();
    let refs = refs.get(&commit.id()).cloned().unwrap_or_default();
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
pub(crate) async fn get_log(
    repo_path: String,
    limit: Option<u32>,
) -> Result<Vec<CommitSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || get_log_impl(&repo_path, limit))
        .await
        .map_err(|e| format!("log task failed: {e}"))?
}

pub(crate) fn get_log_impl(
    repo_path: &str,
    limit: Option<u32>,
) -> Result<Vec<CommitSummary>, String> {
    let repo = open_repo(repo_path)?;
    let mut revwalk = repo.revwalk().map_err(|e| e.to_string())?;
    revwalk.push_head().map_err(|e| e.to_string())?;
    revwalk
        .set_sorting(git2::Sort::TIME)
        .map_err(|e| e.to_string())?;

    let refs = refs_by_target(&repo);
    let mut out = Vec::new();
    for oid in revwalk {
        let oid = oid.map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let s = commit_to_summary(&commit, &refs);
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
pub(crate) struct CommitFile {
    path: String,
    /// One of: added, deleted, modified, renamed, typechange.
    status: String,
    /// New path if renamed, else empty.
    new_path: String,
}

/// List files changed in a single commit (diff against its first parent).
#[tauri::command]
pub(crate) fn get_commit_files(
    repo_path: String,
    commit_id: String,
) -> Result<Vec<CommitFile>, String> {
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

#[cfg(test)]
mod tests {
    use super::{get_log_impl, refs_by_target};
    use std::path::PathBuf;

    fn scratch_repo(tag: &str) -> (git2::Repository, PathBuf) {
        let path = std::env::temp_dir().join(format!("idea-git-log-{tag}"));
        let _ = std::fs::remove_dir_all(&path);
        let repo = git2::Repository::init(&path).expect("init");
        std::fs::write(path.join("f.txt"), b"1").expect("write");
        let tree_oid = {
            let mut idx = repo.index().expect("index");
            idx.add_all(["f.txt"].iter(), git2::IndexAddOption::DEFAULT, None)
                .expect("add");
            idx.write_tree().expect("write tree")
        };
        {
            let tree = repo.find_tree(tree_oid).expect("tree");
            let sig = git2::Signature::now("t", "t@example.com").expect("sig");
            let oid = repo
                .commit(Some("HEAD"), &sig, &sig, "one", &tree, &[])
                .expect("commit");
            let commit = repo.find_commit(oid).expect("commit object");
            repo.branch("feature", &commit, false).expect("branch");
        }
        (repo, path)
    }

    // The map the log now builds once has to hold exactly what the old
    // per-commit scan collected: every name whose target is the commit.
    #[test]
    fn refs_by_target_groups_names_by_commit() {
        let (repo, path) = scratch_repo("map");
        let map = refs_by_target(&repo);
        let head = repo.head().expect("head").target().expect("target");
        let mut names = map.get(&head).cloned().unwrap_or_default();
        names.sort();
        drop(repo);
        assert_eq!(names, vec!["feature".to_string(), "master".to_string()]);
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn get_log_rows_carry_their_refs() {
        let (repo, path) = scratch_repo("log");
        drop(repo);
        let out = get_log_impl(path.to_str().expect("utf8"), None).expect("log");
        assert_eq!(out.len(), 1);
        let mut refs = out[0].refs.clone();
        refs.sort();
        let _ = std::fs::remove_dir_all(&path);
        assert_eq!(refs, vec!["feature".to_string(), "master".to_string()]);
    }
}
