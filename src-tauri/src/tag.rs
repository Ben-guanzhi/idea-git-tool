//! Tag commands: list / create / delete (lightweight or annotated).

use serde::Serialize;

use crate::repo::open_repo;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TagInfo {
    name: String,
    /// Short id of the tagged commit (first 7 hex chars).
    target: String,
    annotated: bool,
    /// Annotation text; only present for annotated tags.
    message: Option<String>,
}

#[tauri::command]
pub(crate) fn list_tags(repo_path: String) -> Result<Vec<TagInfo>, String> {
    let repo = open_repo(&repo_path)?;
    let names = repo.tag_names(None).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for name in names.iter().flatten() {
        let full = format!("refs/tags/{name}");
        let obj = repo.revparse_single(&full).map_err(|e| e.to_string())?;
        let annotated = obj.kind() == Some(git2::ObjectType::Tag);
        let message = if annotated {
            obj.as_tag().and_then(|t| t.message().map(str::to_string))
        } else {
            None
        };
        let commit = obj.peel_to_commit().map_err(|e| e.to_string())?;
        out.push(TagInfo {
            name: name.to_string(),
            target: commit.id().to_string()[..7].to_string(),
            annotated,
            message,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Create a tag on `at` (defaults to HEAD). A non-empty `message` makes it
/// annotated, matching `git tag -a -m …`; an empty one is lightweight
/// (`git tag …`).
#[tauri::command]
pub(crate) fn create_tag(
    repo_path: String,
    name: String,
    message: Option<String>,
    at: Option<String>,
) -> Result<String, String> {
    let repo = open_repo(&repo_path)?;
    let obj = match at {
        Some(rev) => repo.revparse_single(&rev).map_err(|e| e.to_string())?,
        None => repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .map_err(|e| e.to_string())?
            .as_object()
            .clone(),
    };
    let sig = repo.signature().map_err(|e| e.to_string())?;
    let msg = message.filter(|m| !m.trim().is_empty());
    let annotated = msg.is_some();
    match &msg {
        Some(m) => repo
            .tag(&name, &obj, &sig, m.as_str(), false)
            .map_err(|e| e.to_string())?,
        None => repo
            .tag_lightweight(&name, &obj, false)
            .map_err(|e| e.to_string())?,
    };
    Ok(if annotated {
        format!("已创建注解标签 {name}")
    } else {
        format!("已创建轻量标签 {name}")
    })
}

#[tauri::command]
pub(crate) fn delete_tag(repo_path: String, name: String) -> Result<String, String> {
    let repo = open_repo(&repo_path)?;
    repo.tag_delete(&name).map_err(|e| e.to_string())?;
    Ok(format!("已删除标签 {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRepo {
        path: std::path::PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "idea-git-tool-tag-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let repo = git2::Repository::init(&dir).unwrap();
            {
                let mut cfg = repo.config().unwrap();
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
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn tag_create_list_delete_round_trip() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "v1\n", "first");
        let head = open_repo(&repo.path_str())
            .unwrap()
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string()[..7]
            .to_string();

        // Empty message → lightweight; non-empty → annotated.
        create_tag(repo.path_str(), "v0.1".into(), None, None).unwrap();
        create_tag(
            repo.path_str(),
            "v1.0".into(),
            Some("first release".into()),
            None,
        )
        .unwrap();

        let tags = list_tags(repo.path_str()).unwrap();
        assert_eq!(tags.len(), 2);
        let light = tags.iter().find(|t| t.name == "v0.1").unwrap();
        assert!(!light.annotated);
        assert_eq!(light.message, None);
        assert_eq!(light.target, head, "tag points at HEAD");
        let ann = tags.iter().find(|t| t.name == "v1.0").unwrap();
        assert!(ann.annotated);
        assert_eq!(ann.message.as_deref(), Some("first release"));

        // Duplicate names are refused (force = false), like `git tag`.
        let err = create_tag(repo.path_str(), "v1.0".into(), None, None).unwrap_err();
        assert!(!err.is_empty());

        delete_tag(repo.path_str(), "v0.1".into()).unwrap();
        let tags = list_tags(repo.path_str()).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0");

        let err = delete_tag(repo.path_str(), "missing".into()).unwrap_err();
        assert!(!err.is_empty());
    }
}
