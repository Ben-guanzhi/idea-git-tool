//! Network commands (push / fetch / pull) via the system git client.

use crate::repo::{nonempty, repo_head_name, run_git};

// Network operations run on the blocking pool: `git push/fetch/pull` waits on
// the remote - tens of seconds on a slow link - and a synchronous command
// would hold the main thread for all of it.
#[tauri::command]
pub(crate) async fn push(
    repo_path: String,
    remote: Option<String>,
    branch: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || push_impl(repo_path, remote, branch))
        .await
        .map_err(|e| format!("push task failed: {e}"))?
}

fn push_impl(
    repo_path: String,
    remote: Option<String>,
    branch: Option<String>,
) -> Result<String, String> {
    let remote_name = nonempty(remote, "origin");
    let branch_name = branch.filter(|b| !b.trim().is_empty()).unwrap_or_else(|| {
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
    Ok(push_summary(&out, &remote_name, &branch_name, has_upstream))
}

/// What the push button should say. git's "Everything up-to-date" note is
/// localized, so "nothing was pushed" is detected by the one piece of its
/// output that no translation ever touches: a ref that actually moved is
/// always advertised as `old..new name -> name`. With no advertisement, lead
/// with our own summary instead of dumping git's prose at the user.
fn push_summary(out: &str, remote_name: &str, branch_name: &str, has_upstream: bool) -> String {
    if out.lines().any(|line| line.contains(" -> ")) {
        return out.to_string();
    }
    format!(
        "pushed {} to {remote_name}{}",
        if branch_name.is_empty() {
            "current branch"
        } else {
            branch_name
        },
        if has_upstream { "" } else { " (set upstream)" }
    )
}

#[tauri::command]
pub(crate) async fn fetch(repo_path: String, remote: Option<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || fetch_impl(repo_path, remote))
        .await
        .map_err(|e| format!("fetch task failed: {e}"))?
}

fn fetch_impl(repo_path: String, remote: Option<String>) -> Result<String, String> {
    let remote_name = nonempty(remote, "origin");
    run_git(&repo_path, &["fetch", &remote_name])?;
    Ok(format!("fetched from {remote_name}"))
}

#[tauri::command]
pub(crate) async fn pull(
    repo_path: String,
    remote: Option<String>,
    branch: Option<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || pull_impl(repo_path, remote, branch))
        .await
        .map_err(|e| format!("pull task failed: {e}"))?
}

fn pull_impl(
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

#[cfg(test)]
mod tests {
    use super::push_summary;

    /// "Everything up-to-date" is what a translated git does not say, which is
    /// why the summary is keyed off the ref advertisement instead.
    #[test]
    fn an_up_to_date_push_gets_the_summary_in_any_language() {
        for note in ["Everything up-to-date", "已经是最新的", "Todo al dia", ""] {
            let out = push_summary(note, "origin", "main", true);
            assert_eq!(out, "pushed main to origin", "{note:?}");
        }
    }

    #[test]
    fn a_push_that_moves_a_ref_keeps_gits_output() {
        let out = push_summary(
            "To github.com:o/r.git\n   a1b2c3d..e4f5g6h main -> main",
            "origin",
            "main",
            true,
        );
        assert!(out.contains(" -> main"), "{out}");
    }

    #[test]
    fn the_branch_and_upstream_flags_are_preserved() {
        assert_eq!(
            push_summary("", "origin", "", true),
            "pushed current branch to origin"
        );
        assert_eq!(
            push_summary("", "origin", "feature", false),
            "pushed feature to origin (set upstream)"
        );
    }
}
