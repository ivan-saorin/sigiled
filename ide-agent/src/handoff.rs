//! A fixed two-Markdown-file handoff. Uses a private index and immutable commit.
//! Arbitrary external writers cannot contribute content to this commit.
use crate::durable::{self, Repository};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    path::Path,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub path: String,
    pub content: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub generation: String,
    pub operation_id: String,
    pub run_id: String,
    pub slug: String,
    pub files: Vec<File>,
}
#[derive(Clone, Deserialize, Serialize)]
struct Journal {
    request: Request,
    base: String,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    pushed: bool,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Receipt {
    pub operation_id: String,
    pub run_id: String,
    pub generation: String,
    pub sha: String,
    pub pushed: bool,
    pub dirty: bool,
    pub state: String,
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
pub fn validate(r: &Request) -> Result<(), &'static str> {
    if !id(&r.operation_id)
        || !id(&r.run_id)
        || !id(&r.slug)
        || r.files.len() != 2
        || r.files[0].path == r.files[1].path
        || r.generation
            .parse::<u64>()
            .ok()
            .is_none_or(|n| n.to_string() != r.generation)
    {
        return Err("invalid_handoff_bundle");
    }
    let prefix = format!("docs/design/{}/", r.slug);
    for file in &r.files {
        let name = file
            .path
            .strip_prefix(&prefix)
            .ok_or("invalid_handoff_path")?;
        if !name.ends_with(".md")
            || name.starts_with('.')
            || name.len() > 100
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || file.content.len() > 512 * 1024
        {
            return Err("invalid_handoff_path");
        }
    }
    Ok(())
}
fn save(path: &Path, j: &Journal) -> Result<(), &'static str> {
    let temp = path.with_extension("pending");
    let bytes = serde_json::to_vec(j).map_err(|_| "handoff_journal_failed")?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|_| "handoff_journal_failed")?;
    f.write_all(&bytes)
        .and_then(|_| f.sync_all())
        .map_err(|_| "handoff_journal_failed")?;
    std::fs::rename(temp, path).map_err(|_| "handoff_journal_failed")?;
    std::fs::File::open(path.parent().unwrap())
        .and_then(|f| f.sync_all())
        .map_err(|_| "handoff_journal_failed")
}
// Walk and create through directory descriptors. A replaced symlink is never followed.
fn directory(root: &Path, slug: &str, create: bool) -> Result<std::fs::File, &'static str> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(|_| "handoff_path_changed")?;
    for name in ["docs", "design", slug] {
        let name = std::ffi::CString::new(name).unwrap();
        if create {
            let rc = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o755) };
            if rc != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err("handoff_write_failed");
            }
            dir.sync_all().map_err(|_| "handoff_write_failed")?;
        }
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err("handoff_path_changed");
        }
        dir = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok(dir)
}
fn write_bundle(repo: &Repository, r: &Request, recovery: bool) -> Result<(), &'static str> {
    let dir = directory(&repo.root, &r.slug, true)?;
    for file in &r.files {
        let name = std::ffi::CString::new(file.path.rsplit('/').next().unwrap()).unwrap();
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o644,
            )
        };
        if fd < 0 {
            if !recovery {
                return Err("handoff_target_exists");
            }
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return Err("handoff_target_changed");
            }
            let mut old = unsafe { std::fs::File::from_raw_fd(fd) };
            if !old
                .metadata()
                .map_err(|_| "handoff_target_changed")?
                .is_file()
            {
                return Err("handoff_target_changed");
            }
            let mut content = Vec::new();
            (&mut old)
                .take(512 * 1024 + 1)
                .read_to_end(&mut content)
                .map_err(|_| "handoff_target_changed")?;
            if content != file.content.as_bytes() {
                return Err("handoff_target_changed");
            }
        } else {
            let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
            f.write_all(file.content.as_bytes())
                .and_then(|_| f.sync_all())
                .map_err(|_| "handoff_write_failed")?;
        }
    }
    dir.sync_all().map_err(|_| "handoff_write_failed")
}
fn verify_files(repo: &Repository, r: &Request) -> Result<(), &'static str> {
    let dir = directory(&repo.root, &r.slug, false)?;
    for file in &r.files {
        let name = std::ffi::CString::new(file.path.rsplit('/').next().unwrap()).unwrap();
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err("handoff_target_changed");
        }
        let mut bytes = Vec::new();
        let file_handle = unsafe { std::fs::File::from_raw_fd(fd) };
        if !file_handle
            .metadata()
            .map_err(|_| "handoff_target_changed")?
            .is_file()
        {
            return Err("handoff_target_changed");
        }
        file_handle
            .take(512 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "handoff_target_changed")?;
        if bytes != file.content.as_bytes() {
            return Err("handoff_target_changed");
        }
    }
    Ok(())
}
fn validation_error(e: durable::Error) -> &'static str {
    match e {
        durable::Error::Git => "handoff_repository_probe_failed",
        durable::Error::Busy => "workspace_git_lock_busy",
        durable::Error::Changed => "workspace_binding_changed",
        durable::Error::Dirty => "workspace_dirty",
    }
}
fn index_git(
    repo: &Repository,
    index: &Path,
    args: &[&str],
    deadline: Instant,
) -> Result<String, &'static str> {
    let mut c = crate::bounded_process::git_command();
    c.arg("-C")
        .arg(&repo.root)
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .env("GIT_OPTIONAL_LOCKS", "0");
    let out =
        crate::bounded_process::output_until(&mut c, deadline).map_err(|_| "handoff_git_failed")?;
    if !out.status.success() {
        return Err("handoff_git_failed");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().into())
}
pub fn apply(repo: &Repository, r: Request) -> Result<Receipt, &'static str> {
    validate(&r)?;
    let _lock = durable::lock(&repo.root).map_err(|_| "workspace_busy")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let git = |args: &[&str]| repo.git(args, deadline).map_err(|_| "handoff_git_failed");
    let head = repo.validate(deadline).map_err(validation_error)?;
    let journal_path = repo
        .root
        .join(".git")
        .join(format!("sigil-handoff-{}.json", r.operation_id));
    let old = match std::fs::read(&journal_path) {
        Ok(b) if b.len() < 2 * 1024 * 1024 => {
            Some(serde_json::from_slice::<Journal>(&b).map_err(|_| "handoff_journal_invalid")?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        _ => return Err("handoff_journal_invalid"),
    };
    let recovery = old.is_some();
    let mut j = if let Some(j) = old {
        if j.request != r {
            return Err("handoff_operation_conflict");
        }
        j
    } else {
        if !git(&["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
            return Err("workspace_changes_checkpoint_and_retry");
        }
        // Refuse pre-existing ignored targets too, before writing either file.
        for file in &r.files {
            if repo.root.join(&file.path).symlink_metadata().is_ok() {
                return Err("handoff_target_exists");
            }
        }
        let j = Journal {
            request: r.clone(),
            base: head.clone(),
            commit: None,
            pushed: false,
        };
        save(&journal_path, &j)?;
        j
    };
    if head != j.base && Some(&head) != j.commit.as_ref() {
        return Err("workspace_changed");
    }
    let index_tree = git(&["write-tree"])?;
    let base_tree = git(&["rev-parse", &format!("{}^{{tree}}", j.base)])?;
    let commit_tree = j
        .commit
        .as_ref()
        .map(|sha| git(&["rev-parse", &format!("{sha}^{{tree}}")]))
        .transpose()?;
    if index_tree != base_tree && Some(&index_tree) != commit_tree.as_ref() {
        return Err("workspace_index_changed");
    }
    let original_index =
        std::fs::read(repo.root.join(".git/index")).map_err(|_| "workspace_index_changed")?;
    if j.commit.is_none() {
        // Only previously recorded bundle paths may be dirty during a partial-write recovery.
        let dirty = git(&["status", "--porcelain", "--untracked-files=all"])?;
        if dirty
            .lines()
            .any(|line| line.len() < 4 || !r.files.iter().any(|f| line[3..] == f.path))
        {
            return Err("workspace_changes_checkpoint_and_retry");
        }
        write_bundle(repo, &r, recovery)?;
        verify_files(repo, &r)?;
        let index = repo
            .root
            .join(".git")
            .join(format!("sigil-handoff-{}.index", r.operation_id));
        index_git(repo, &index, &["read-tree", &j.base], deadline)?;
        for file in &r.files {
            // Hash a private immutable copy, never a concurrently edited workspace file.
            let blob = repo
                .root
                .join(".git")
                .join(format!("sigil-handoff-{}.blob", r.operation_id));
            std::fs::write(&blob, &file.content).map_err(|_| "handoff_write_failed")?;
            let hash = git(&[
                "hash-object",
                "-w",
                "--",
                blob.to_str().ok_or("handoff_path_changed")?,
            ])?;
            index_git(
                repo,
                &index,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{hash},{}", file.path),
                ],
                deadline,
            )?;
        }
        let tree = index_git(repo, &index, &["write-tree"], deadline)?;
        let commit = git(&[
            "commit-tree",
            &tree,
            "-p",
            &j.base,
            "-m",
            &format!(
                "research: accept dossier for run {}\n\nSigil-Handoff: {}",
                r.run_id, r.operation_id
            ),
        ])?;
        verify_files(repo, &r)?;
        if repo.validate(deadline).map_err(validation_error)? != j.base
            || std::fs::read(repo.root.join(".git/index")).ok().as_ref() != Some(&original_index)
        {
            return Err("workspace_changed");
        }
        j.commit = Some(commit);
        save(&journal_path, &j)?;
    }
    let sha = j.commit.clone().ok_or("handoff_journal_invalid")?;
    // Reconstruct the exact commit index; publish only under Git's index lock and
    // only if the user's index still matches the captured precondition.
    let private = repo
        .root
        .join(".git")
        .join(format!("sigil-handoff-{}.index", r.operation_id));
    index_git(repo, &private, &["read-tree", &sha], deadline)?;
    let index_lock_path = repo.root.join(".git/index.lock");
    let mut index_lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&index_lock_path)
        .map_err(|_| "workspace_busy")?;
    struct IndexLock(std::path::PathBuf, bool);
    impl Drop for IndexLock {
        fn drop(&mut self) {
            if self.1 {
                let _ = std::fs::remove_file(&self.0);
            }
        }
    }
    let mut index_guard = IndexLock(index_lock_path.clone(), true);
    if std::fs::read(repo.root.join(".git/index")).ok().as_ref() != Some(&original_index) {
        return Err("workspace_index_changed");
    }
    verify_files(repo, &r)?;
    let current = git(&["rev-parse", "HEAD"])?;
    if current != j.base && current != sha {
        return Err("workspace_changed");
    }
    if git(&["symbolic-ref", "--quiet", "--short", "HEAD"])? != repo.branch {
        return Err("workspace_changed");
    }
    let bytes = std::fs::read(private).map_err(|_| "handoff_git_failed")?;
    index_lock
        .write_all(&bytes)
        .and_then(|_| index_lock.sync_all())
        .map_err(|_| "handoff_git_failed")?;
    if current == j.base {
        git(&[
            "update-ref",
            &format!("refs/heads/{}", repo.branch),
            &sha,
            &j.base,
        ])?;
    }
    std::fs::rename(&index_lock_path, repo.root.join(".git/index"))
        .map_err(|_| "handoff_index_publish_failed")?;
    index_guard.1 = false;
    drop(index_guard);
    std::fs::File::open(repo.root.join(".git"))
        .and_then(|f| f.sync_all())
        .map_err(|_| "handoff_index_publish_failed")?;
    // Noncooperating writes may remain on disk; they are never included in sha.
    if !j.pushed {
        git(&[
            "push",
            "--",
            &repo.remote,
            &format!("{sha}:refs/heads/{}", repo.branch),
        ])?;
        j.pushed = true;
        save(&journal_path, &j)?;
    }
    let dirty = !git(&["status", "--porcelain", "--untracked-files=all"])?.is_empty()
        || git(&["rev-parse", "HEAD"])? != sha
        || verify_files(repo, &r).is_err();
    Ok(Receipt {
        operation_id: r.operation_id,
        run_id: r.run_id,
        generation: r.generation,
        sha,
        pushed: j.pushed,
        dirty,
        state: if dirty {
            "committed_with_concurrent_changes"
        } else {
            "committed_on_session"
        }
        .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    fn git(root: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().into()
    }
    fn fixture() -> Repository {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "handoff-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("repo");
        std::fs::create_dir(&root).unwrap();
        let remote = dir.join("remote.git");
        git(&dir, &["init", "--bare", remote.to_str().unwrap()]);
        git(&root, &["init", "-b", "session/test"]);
        std::fs::write(root.join("existing"), "original").unwrap();
        git(&root, &["add", "existing"]);
        git(&root, &["commit", "-m", "base"]);
        git(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&root, &["push", "origin", "session/test"]);
        Repository {
            root,
            branch: "session/test".into(),
            remote: remote.to_str().unwrap().into(),
        }
    }
    fn request() -> Request {
        Request {
            generation: u64::MAX.to_string(),
            operation_id: "operation1".into(),
            run_id: "run1".into(),
            slug: "2026-09-10-run1".into(),
            files: vec![
                File {
                    path: "docs/design/2026-09-10-run1/dossier.md".into(),
                    content: "# Dossier\n".into(),
                },
                File {
                    path: "docs/design/2026-09-10-run1/cover.md".into(),
                    content: "Problem\n".into(),
                },
            ],
        }
    }
    #[test]
    fn staged_and_unstaged_work_fail_before_writes() {
        for staged in [false, true] {
            let repo = fixture();
            std::fs::write(repo.root.join("existing"), "user work").unwrap();
            if staged {
                git(&repo.root, &["add", "existing"]);
            }
            assert_eq!(
                apply(&repo, request()).unwrap_err(),
                "workspace_changes_checkpoint_and_retry"
            );
            assert!(!repo.root.join("docs").exists());
            assert_eq!(
                std::fs::read_to_string(repo.root.join("existing")).unwrap(),
                "user work"
            );
        }
    }
    #[test]
    fn exact_bundle_commit_replays_and_conflicting_payload_fails() {
        let repo = fixture();
        let r = request();
        let first = apply(&repo, r.clone()).unwrap();
        assert!(first.pushed);
        assert!(!first.dirty);
        let second = apply(&repo, r.clone()).unwrap();
        assert_eq!(first.sha, second.sha);
        assert_eq!(
            git(
                &repo.root,
                &["show", "--format=", "--name-only", &first.sha]
            ),
            "docs/design/2026-09-10-run1/cover.md\ndocs/design/2026-09-10-run1/dossier.md"
        );
        let mut changed = r;
        changed.files[0].content = "different".into();
        assert_eq!(
            apply(&repo, changed).unwrap_err(),
            "handoff_operation_conflict"
        );
    }
    #[test]
    fn rejected_push_preserves_commit_and_same_operation_recovers() {
        let repo = fixture();
        let hook = Path::new(&repo.remote).join("hooks/pre-receive");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(apply(&repo, request()).is_err());
        let sha = git(&repo.root, &["rev-parse", "HEAD"]);
        assert!(repo.root.join(&request().files[0].path).exists());
        std::fs::remove_file(hook).unwrap();
        assert_eq!(apply(&repo, request()).unwrap().sha, sha);
    }
    #[test]
    fn noncooperating_edits_are_preserved_and_never_enter_handoff_commit() {
        let repo = fixture();
        let hook = Path::new(&repo.remote).join("hooks/pre-receive");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nprintf changed > '{}'\nprintf unrelated > '{}'\n",
                repo.root.join(&request().files[0].path).display(),
                repo.root.join("unrelated").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        let receipt = apply(&repo, request()).unwrap();
        assert!(receipt.dirty);
        assert_eq!(receipt.state, "committed_with_concurrent_changes");
        assert_eq!(
            std::fs::read_to_string(repo.root.join("unrelated")).unwrap(),
            "unrelated"
        );
        assert_eq!(
            git(
                &repo.root,
                &[
                    "show",
                    &format!("{}:{}", receipt.sha, request().files[0].path)
                ]
            ),
            "# Dossier"
        );
        assert!(
            !git(&repo.root, &["ls-tree", "-r", "--name-only", &receipt.sha]).contains("unrelated")
        );
    }
    #[test]
    fn unsafe_paths_symlinks_and_partial_write_are_preserved() {
        let repo = fixture();
        let mut r = request();
        r.files[0].path = "docs/design/../outside.md".into();
        assert!(validate(&r).is_err());
        std::os::unix::fs::symlink(repo.root.parent().unwrap(), repo.root.join("docs")).unwrap();
        assert!(apply(&repo, request()).is_err());
        assert!(repo.root.join("docs").is_symlink());
        let repo = fixture();
        let r = request();
        let base = git(&repo.root, &["rev-parse", "HEAD"]);
        let j = Journal {
            request: r.clone(),
            base,
            commit: None,
            pushed: false,
        };
        save(&repo.root.join(".git/sigil-handoff-operation1.json"), &j).unwrap();
        let dir = directory(&repo.root, &r.slug, true).unwrap();
        drop(dir);
        std::fs::write(repo.root.join(&r.files[0].path), &r.files[0].content).unwrap();
        assert!(apply(&repo, r).unwrap().pushed);
    }
}
