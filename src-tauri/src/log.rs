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

/// One page of the log plus whether history continues past it.
///
/// `has_more` is decided by peeking one commit beyond the page, so the last
/// page reports exactly what it shows rather than guessing from "the page
/// came back full" — a boundary-sized history is otherwise indistinguishable
/// from one with a commit left on the table.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogPage {
    rows: Vec<CommitSummary>,
    has_more: bool,
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
    offset: Option<u32>,
) -> Result<LogPage, String> {
    tauri::async_runtime::spawn_blocking(move || get_log_impl(&repo_path, limit, offset))
        .await
        .map_err(|e| format!("log task failed: {e}"))?
}

pub(crate) fn get_log_impl(
    repo_path: &str,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<LogPage, String> {
    let repo = open_repo(repo_path)?;
    let mut revwalk = repo.revwalk().map_err(|e| e.to_string())?;
    revwalk.push_head().map_err(|e| e.to_string())?;
    revwalk
        .set_sorting(git2::Sort::TIME)
        .map_err(|e| e.to_string())?;

    let refs = refs_by_target(&repo);
    let skip = offset.unwrap_or(0) as usize;
    let mut rows = Vec::new();
    let mut has_more = false;
    let mut seen = 0usize;
    let mut iter = revwalk.peekable();

    while let Some(item) = iter.next() {
        // Past the page's end: whether the next object parses settles "is
        // there more". A broken object is not more history.
        if let Some(l) = limit {
            if rows.len() == l as usize {
                has_more = iter.peek().map(|next| next.is_ok()).unwrap_or(false);
                break;
            }
        }
        let oid = item.map_err(|e| e.to_string())?;
        seen += 1;
        if seen <= skip {
            continue;
        }
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        rows.push(commit_to_summary(&commit, &refs));
    }
    Ok(LogPage { rows, has_more })
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

    // Commits get explicit, strictly increasing times: sort is by commit time,
    // so two commits in the same second would tie and the page order would be
    // whatever the traversal happened to produce.
    fn commit_at(
        repo: &git2::Repository,
        path: &std::path::Path,
        msg: &str,
        body: &[u8],
        secs: i64,
    ) {
        std::fs::write(path.join("f.txt"), body).expect("write");
        let tree_oid = {
            let mut idx = repo.index().expect("index");
            idx.add_all(["f.txt"].iter(), git2::IndexAddOption::DEFAULT, None)
                .expect("add");
            idx.write_tree().expect("write tree")
        };
        let tree = repo.find_tree(tree_oid).expect("tree");
        let when = git2::Time::new(secs, 0);
        let sig = git2::Signature::new("t", "t@example.com", &when).expect("sig");
        // An empty parent list means a root commit, which libgit2 refuses to
        // create on top of an already born HEAD.
        let parent = match repo.head().map(|r| r.target()) {
            Ok(Some(target)) => Some(repo.find_commit(target).expect("parent commit")),
            _ => None,
        };
        let parents: Vec<&git2::Commit> = match parent.as_ref() {
            Some(p) => vec![p],
            None => vec![],
        };
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .expect("commit");
    }

    fn scratch_repo(tag: &str) -> (git2::Repository, PathBuf) {
        let path = std::env::temp_dir().join(format!("idea-git-log-{tag}"));
        let _ = std::fs::remove_dir_all(&path);
        let repo = git2::Repository::init(&path).expect("init");
        commit_at(&repo, &path, "one", b"1", 1_000_000_000);
        let head = repo.head().expect("head").target().expect("target");
        {
            let commit = repo.find_commit(head).expect("commit");
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
        let page = get_log_impl(path.to_str().expect("utf8"), None, None).expect("log");
        assert_eq!(page.rows.len(), 1);
        assert!(!page.has_more);
        let mut refs = page.rows[0].refs.clone();
        refs.sort();
        let _ = std::fs::remove_dir_all(&path);
        assert_eq!(refs, vec!["feature".to_string(), "master".to_string()]);
    }

    // Paging must hand back the whole history exactly once — no commit lost at
    // a page boundary, no repeat. Before the offset the log was a single fixed
    // 50-row window and everything older was simply unreachable.
    #[test]
    fn log_pages_cover_the_history_exactly_once() {
        let (repo, path) = scratch_repo("pages");
        for i in 1..=7u32 {
            commit_at(
                &repo,
                &path,
                &format!("c{i}"),
                &[10 + i as u8],
                1_000_000_000 + i as i64 * 1000,
            );
        }
        drop(repo);
        let log = path.to_str().expect("utf8");

        let mut messages = Vec::new();
        let mut offset = 0u32;
        let mut guard = 0;
        loop {
            let page = get_log_impl(log, Some(3), Some(offset)).expect("page");
            guard += 1;
            assert!(guard <= 20, "paging must terminate");
            messages.extend(page.rows.iter().map(|r| r.message.clone()));
            offset += page.rows.len() as u32;
            if !page.has_more {
                break;
            }
        }
        let _ = std::fs::remove_dir_all(&path);
        assert_eq!(
            messages,
            vec!["c7", "c6", "c5", "c4", "c3", "c2", "c1", "one"],
            "newest first, every commit exactly once"
        );
    }

    // A history whose size is an exact multiple of the page size must not be
    // reported as having more: "the page came back full" is not evidence.
    #[test]
    fn a_boundary_sized_history_reports_no_more() {
        let (repo, path) = scratch_repo("boundary");
        for i in 1..=3u32 {
            commit_at(
                &repo,
                &path,
                &format!("c{i}"),
                &[20 + i as u8],
                1_000_000_000 + i as i64 * 1000,
            );
        }
        drop(repo);
        let log = path.to_str().expect("utf8");

        let page = get_log_impl(log, Some(2), Some(0)).expect("page");
        assert_eq!(page.rows.len(), 2);
        assert!(page.has_more, "two more commits exist");
        let rest = get_log_impl(log, Some(2), Some(2)).expect("rest");
        assert_eq!(rest.rows.len(), 2);
        assert!(!rest.has_more, "the page filled the history exactly");
        let past = get_log_impl(log, Some(2), Some(4)).expect("past");
        assert!(past.rows.is_empty());
        assert!(!past.has_more, "nothing beyond the end");
        let _ = std::fs::remove_dir_all(&path);
    }

    // Offsetting must be exact: skipping rows must not shift the window.
    #[test]
    fn an_offset_skips_without_shifting_the_window() {
        let (repo, path) = scratch_repo("offset");
        for i in 1..=5u32 {
            commit_at(
                &repo,
                &path,
                &format!("c{i}"),
                &[30 + i as u8],
                1_000_000_000 + i as i64 * 1000,
            );
        }
        drop(repo);
        let log = path.to_str().expect("utf8");

        let first = get_log_impl(log, Some(2), None).expect("first");
        let second = get_log_impl(log, Some(2), Some(2)).expect("second");
        let _ = std::fs::remove_dir_all(&path);
        assert_eq!(
            first
                .rows
                .iter()
                .map(|r| r.message.clone())
                .chain(second.rows.iter().map(|r| r.message.clone()))
                .collect::<Vec<_>>(),
            vec!["c5", "c4", "c3", "c2"]
        );
    }
}
