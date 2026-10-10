//! Repository helpers shared by all command modules.

use std::path::{Component, Path, PathBuf};

/// Open the repository at `repo_path`, mapping the error to the string the
/// command boundary carries. Every command used to repeat this mapping.
pub(crate) fn open_repo(repo_path: &str) -> Result<git2::Repository, String> {
    git2::Repository::open(repo_path).map_err(|e| e.to_string())
}

/// Resolve a repo-relative path against the workdir, rejecting absolute paths
/// and `..` traversal. libgit2's `index.add_path` refuses paths outside the
/// workdir on its own, but the direct `std::fs` calls in `write_file`,
/// `discard`, `resolve_conflict` and `get_file_contents` must not become
/// arbitrary read/write/delete primitives: a `../` path would otherwise
/// escape the repository entirely.
pub(crate) fn resolve_in_workdir(repo: &git2::Repository, path: &str) -> Result<PathBuf, String> {
    validate_rel_path(path)?;
    repo.workdir()
        .map(|w| w.join(path))
        .ok_or_else(|| "repository has no workdir".to_string())
}

/// Reject absolute paths, drive prefixes and `..`/root components outright.
pub(crate) fn validate_rel_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("非法路径「{path}」：必须位于仓库工作区内"));
    }
    Ok(())
}

/// Refuse to force-checkout over a dirty working tree. The git CLI protects
/// uncommitted changes before a merge/checkout; without this check the
/// `.force()` checkouts below would silently wipe them (data loss).
/// Untracked-and-unstaged files (`WT_NEW` only) are allowed, matching git.
pub(crate) fn ensure_clean_workdir(repo: &git2::Repository) -> Result<(), String> {
    let statuses = repo.statuses(None).map_err(|e| e.to_string())?;
    for entry in statuses.iter() {
        if entry.status() != git2::Status::WT_NEW {
            return Err("工作区有未提交的修改，请先提交或暂存（stash）后再执行该操作".to_string());
        }
    }
    Ok(())
}

pub(crate) fn repo_head_name(repo: &git2::Repository) -> Result<String, String> {
    if repo.head_detached().map_err(|e| e.to_string())? {
        let oid = repo.head().map_err(|e| e.to_string())?.target();
        Ok(oid
            .map(|o| format!("HEAD detached at {}", o.to_string()))
            .unwrap_or_else(|| "detached".to_string()))
    } else {
        match repo.head() {
            Ok(h) => match h.shorthand() {
                Some(s) => Ok(s.to_string()),
                None => Ok("detached".to_string()),
            },
            Err(_) => Ok("unborn".to_string()),
        }
    }
}

pub(crate) fn current_branch_name(repo: &git2::Repository) -> Result<String, String> {
    match repo.head() {
        Ok(h) if h.is_branch() => Ok(h.shorthand().unwrap_or_default().to_string()),
        Ok(_) => Err("HEAD is detached; cannot merge".to_string()),
        Err(_) => Err("no commits yet (unborn branch); cannot merge".to_string()),
    }
}

pub(crate) fn sig_for(
    repo: &git2::Repository,
    name: &str,
    email: &str,
) -> Result<git2::Signature<'static>, String> {
    if !name.is_empty() && !email.is_empty() {
        git2::Signature::now(name, email).map_err(|e| e.to_string())
    } else {
        repo.signature().map_err(|e| {
            format!(
                "无法获取提交作者签名（git user.name / user.email 未配置）: {e}。\
                 请运行: git config --global user.name \"你的名字\" && \
                 git config --global user.email \"你的邮箱\""
            )
        })
    }
}

/// Convert a git2 delta status into a short string.
pub(crate) fn delta_to_status(d: git2::Delta) -> &'static str {
    match d {
        git2::Delta::Added => "added",
        git2::Delta::Deleted => "deleted",
        git2::Delta::Modified => "modified",
        git2::Delta::Renamed => "renamed",
        git2::Delta::Typechange => "typechanged",
        _ => "modified",
    }
}

pub(crate) fn blob_to_string(repo: &git2::Repository, id: git2::Oid) -> Result<String, String> {
    match repo.find_blob(id) {
        Ok(b) => Ok(String::from_utf8_lossy(b.content()).into_owned()),
        Err(_) => Ok(String::new()),
    }
}

pub(crate) fn tree_blob(tree: &git2::Tree, path: &str) -> Option<git2::Oid> {
    tree.get_path(Path::new(path)).ok().map(|e| e.id())
}

/// git's own "is this text?" rule plus the round-trip guarantee the editor
/// needs: content read here is later written back as UTF-8 text, which only
/// reproduces the original bytes when the decode was lossless.
pub(crate) fn bytes_are_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0) || std::str::from_utf8(bytes).is_err()
}

/// Run a system `git` subcommand in `repo_path` and return trimmed stdout.
/// Network operations (push/fetch/pull) use the system git client because
/// libssh2 on Windows cannot load OpenSSH-format ed25519 keys (git2-rs #659).
pub(crate) fn run_git(repo_path: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("failed to spawn git: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        let msg = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if msg.is_empty() {
            format!("git {:?} failed with {}", args, out.status)
        } else {
            msg
        });
    }
    Ok(stdout)
}

pub(crate) fn nonempty(v: Option<String>, default: &str) -> String {
    v.filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::validate_rel_path;

    // ── 路径穿越（write_file / discard / resolve_conflict / get_file_contents）──

    #[test]
    fn rel_paths_are_accepted() {
        assert!(validate_rel_path("src/main.rs").is_ok());
        assert!(validate_rel_path("a/b/c.txt").is_ok());
        assert!(validate_rel_path("子目录/文件.txt").is_ok());
    }

    #[test]
    fn traversal_and_absolute_paths_are_rejected() {
        assert!(validate_rel_path("../secrets.txt").is_err());
        assert!(validate_rel_path("a/../../b.txt").is_err());
        assert!(validate_rel_path(r"C:\Windows\system32\config").is_err());
        assert!(validate_rel_path("/etc/passwd").is_err());
        assert!(validate_rel_path(r"\\server\share\x").is_err());
    }

    // ── 二进制判定：文本编辑往返只在"NUL -free 且 UTF-8 合法"时保真 ──

    #[test]
    fn text_is_not_binary() {
        assert!(!super::bytes_are_binary(
            b"hello \xe4\xbd\xa0\xe5\xa5\xbd\n"
        ));
        assert!(!super::bytes_are_binary(b""));
    }

    #[test]
    fn nul_or_invalid_utf8_is_binary() {
        assert!(super::bytes_are_binary(b"PK\x03\x04\x00\x0f"));
        assert!(super::bytes_are_binary(&[0xff, 0xfe, b'x']));
    }
}
