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
    let msg = if out.is_empty() || out.contains("Everything up-to-date") {
        format!(
            "pushed {} to {remote_name}{}",
            if branch_name.is_empty() {
                "current branch"
            } else {
                &branch_name
            },
            if has_upstream { "" } else { " (set upstream)" }
        )
    } else {
        out
    };
    Ok(msg)
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
