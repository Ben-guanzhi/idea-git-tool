//! Repository helpers shared by all command modules.

use std::path::{Component, Path, PathBuf};

/// Open the repository at `repo_path`, mapping the error to the string the
/// command boundary carries. Every command used to repeat this mapping.
pub(crate) fn open_repo(repo_path: &str) -> Result<git2::Repository, String> {
    git2::Repository::open(repo_path).map_err(|e| e.to_string())
}

/// The commit HEAD points at right now, or `None` in a repository that has no
/// named tip yet.
///
/// Callers compare this id across a `git` invocation to learn whether the
/// command moved the branch. The alternative — reading git's own verdict — is
/// not reliable in either direction: the phrases are gettext-localized
/// ("already up to date", "successfully rebased"), and `run_git` discards
/// stdout when the process fails. The branch id is written by git itself and
/// never translated.
pub(crate) fn head_oid(repo: &git2::Repository) -> Option<git2::Oid> {
    match repo.head() {
        Ok(head) => head.target(),
        Err(_) => None,
    }
}

/// Resolve a repo-relative path against the workdir, rejecting absolute paths,
/// `..` traversal, and symlinks that point outside the repository. libgit2's
/// `index.add_path` refuses paths outside the workdir on its own, but the
/// direct `std::fs` calls in `write_file`, `discard`, `resolve_conflict` and
/// `get_file_contents` must not become arbitrary read/write/delete primitives:
/// a `../` path or a symlinked directory would otherwise escape the repository
/// entirely.
pub(crate) fn resolve_in_workdir(repo: &git2::Repository, path: &str) -> Result<PathBuf, String> {
    validate_rel_path(path)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| "repository has no workdir".to_string())?;
    let abs = workdir.join(path);
    let canon_work =
        std::fs::canonicalize(workdir).map_err(|e| format!("无法解析仓库目录: {e}"))?;
    // Resolve symlinks in the deepest existing ancestor (the target itself may
    // not exist yet when restoring / creating a file), then re-join the rest.
    let canon_abs =
        canonicalize_existing(&abs).map_err(|e| format!("无法解析路径「{path}」: {e}"))?;
    if !canon_abs.starts_with(&canon_work) {
        return Err(format!("非法路径「{path}」：指向仓库之外"));
    }
    Ok(abs)
}

/// Canonicalize `path` even if its final components do not exist yet: walk up
/// to the deepest existing ancestor, canonicalize it, then re-join the missing
/// tail. This is what lets `write_file`/`discard` validate the *directory*
/// symlinks before creating a brand-new file beneath them.
fn canonicalize_existing(path: &Path) -> Result<PathBuf, std::io::Error> {
    let mut probe = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(&probe) {
            Ok(canon) => {
                let mut out = canon;
                for part in tail.iter().rev() {
                    out.push(part);
                }
                return Ok(out);
            }
            Err(_) => match probe.file_name() {
                Some(name) => {
                    tail.push(name.to_os_string());
                    if !probe.pop() {
                        break;
                    }
                }
                None => break,
            },
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no existing ancestor",
    ))
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
            .map(|o| format!("HEAD detached at {o}"))
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
/// A hard timeout bounds a hung remote: `.output()` would otherwise block the
/// calling thread (and, for a synchronous command, the main thread) forever.
pub(crate) fn run_git(repo_path: &str, args: &[&str]) -> Result<String, String> {
    run_git_timeout(repo_path, args, GIT_COMMAND_TIMEOUT)
}

/// 10 minutes: generous for slow push/fetch of large repos, but still a bound
/// so a connection that never makes progress cannot wedge the UI indefinitely.
const GIT_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

fn run_git_timeout(
    repo_path: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<String, String> {
    use std::io::Read;
    use std::time::Instant;

    let mut child = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn git: {e}"))?;

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| format!("failed to wait git: {e}"))?
        {
            Some(s) => break s,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "git {:?} 执行超时（{}s，已终止），请检查远端是否可达",
                        args,
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    };

    // The child has exited; drain the pipes to EOF (the write ends are closed).
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_end(&mut stdout);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_end(&mut stderr);
    }

    let stdout = String::from_utf8_lossy(&stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
    if !status.success() {
        let msg = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if msg.is_empty() {
            format!("git {:?} failed with {}", args, status)
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
    use super::{resolve_in_workdir, validate_rel_path};

    /// Create a directory symlink cross-platform (used only in the test below;
    /// the exact API differs by target).
    fn make_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link)
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link)
        }
    }

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

    // A symlink pointing outside the workdir must not be followed: the fs
    // primitives in write_file/discard resolve through it. (Skipped when the
    // platform cannot create a symlink, e.g. Windows without developer mode.)
    #[test]
    fn symlink_escaping_the_workdir_is_rejected() {
        let dir =
            std::env::temp_dir().join(format!("idea-git-tool-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = git2::Repository::init(&dir).unwrap();

        // Outside the repo: a sibling temp dir.
        let outside =
            std::env::temp_dir().join(format!("idea-git-tool-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();

        let link = dir.join("escape");
        if make_dir_symlink(&outside, &link).is_err() {
            // Platform refuses to create symlinks (no developer mode): skip.
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_dir_all(&outside);
            eprintln!("skipping symlink test: cannot create symlink");
            return;
        }

        // A path through the symlink escapes the workdir.
        let res = resolve_in_workdir(&repo, "escape/secrets.txt");
        assert!(res.is_err(), "symlink escape must be rejected");

        // A symlink that stays inside the workdir is fine.
        let inner_link = dir.join("inside");
        let inner_target = dir.join("real");
        std::fs::create_dir_all(&inner_target).unwrap();
        make_dir_symlink(&inner_target, &inner_link).unwrap();
        let res = resolve_in_workdir(&repo, "inside/file.txt");
        assert!(res.is_ok(), "in-workdir symlink must be allowed");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
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
