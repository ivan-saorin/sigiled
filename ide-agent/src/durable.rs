//! Cooperative platform lock plus normal Git locks. Arbitrary terminals can
//! bypass the platform lock; validate around each operation and never reset.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
#[derive(Debug, PartialEq)]
pub enum Error {
    Changed,
    Busy,
    Git,
}
pub struct Repository {
    pub root: PathBuf,
    pub branch: String,
    pub remote: String,
}
pub struct Lock(std::fs::File);
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.0), libc::LOCK_UN);
        }
    }
}
pub fn lock(root: &Path) -> Result<Lock, Error> {
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".git/sigil-platform.lock"))
        .map_err(|_| Error::Git)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(Error::Busy);
    }
    Ok(Lock(file))
}
impl Repository {
    fn git(&self, args: &[&str], deadline: Instant) -> Result<String, Error> {
        let mut command = crate::bounded_process::git_command();
        command
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0");
        if std::path::Path::new("/secrets/deploy_key").is_file() {
            command.env("GIT_SSH_COMMAND","ssh -i /secrets/deploy_key -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new");
        }
        for (key, value) in [
            ("GIT_AUTHOR_NAME", "sigiled-checkpoint"),
            ("GIT_AUTHOR_EMAIL", "checkpoint@sigiled.invalid"),
            ("GIT_COMMITTER_NAME", "sigiled-checkpoint"),
            ("GIT_COMMITTER_EMAIL", "checkpoint@sigiled.invalid"),
        ] {
            if std::env::var_os(key).is_none() {
                command.env(key, value);
            }
        }
        let out =
            crate::bounded_process::output_until(&mut command, deadline).map_err(|_| Error::Git)?;
        if !out.status.success() {
            return Err(Error::Git);
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().into())
    }
    fn validate(&self, deadline: Instant) -> Result<String, Error> {
        if !self.branch.starts_with("session/")
            || self.branch.contains(['\n', ':', ' '])
            || self.remote.is_empty()
        {
            return Err(Error::Changed);
        }
        for name in ["index.lock", "HEAD.lock", "config.lock", "packed-refs.lock"] {
            if self.root.join(".git").join(name).exists() {
                return Err(Error::Busy);
            }
        }
        if self.git(&["symbolic-ref", "--quiet", "--short", "HEAD"], deadline)? != self.branch
            || self.git(&["remote", "get-url", "--all", "origin"], deadline)? != self.remote
            || self.git(
                &["remote", "get-url", "--push", "--all", "origin"],
                deadline,
            )? != self.remote
        {
            return Err(Error::Changed);
        }
        self.git(&["rev-parse", "HEAD"], deadline)
    }
    pub fn checkpoint(&self, commit: bool) -> Result<String, Error> {
        let _lock = lock(&self.root)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let before = self.validate(deadline)?;
        if commit && !self.git(&["status", "--porcelain"], deadline)?.is_empty() {
            self.git(&["add", "-A"], deadline)?;
            if self.validate(deadline)? != before {
                return Err(Error::Changed);
            }
            let tree = self.git(&["write-tree"], deadline)?;
            let commit = self.git(
                &[
                    "commit-tree",
                    &tree,
                    "-p",
                    &before,
                    "-m",
                    "checkpoint: saved workspace work\n\nSigil-Checkpoint: manual",
                ],
                deadline,
            )?;
            if self.validate(deadline)? != before {
                return Err(Error::Changed);
            }
            self.git(
                &[
                    "update-ref",
                    &format!("refs/heads/{}", self.branch),
                    &commit,
                    &before,
                ],
                deadline,
            )?;
        }
        let head = self.validate(deadline)?;
        // Use the trusted expected URL and an immutable source SHA. Never use a
        // terminal-mutated push refspec, HEAD, --force, or a master destination.
        self.git(
            &[
                "push",
                "--",
                &self.remote,
                &format!("{head}:refs/heads/{}", self.branch),
            ],
            deadline,
        )?;
        if self.validate(deadline)? != head {
            return Err(Error::Changed);
        }
        Ok(head)
    }
    pub fn dirty(&self) -> Result<bool, Error> {
        Ok(!self
            .git(
                &["status", "--porcelain"],
                Instant::now() + Duration::from_secs(5),
            )?
            .is_empty())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    fn git(root: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().into()
    }
    fn fixture() -> Repository {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/workspace/target/ide-git-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let remote = root.join("remote.git");
        git(&root, &["init", "--bare", remote.to_str().unwrap()]);
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "session/test"]);
        git(&repo, &["commit", "--allow-empty", "-m", "base"]);
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&repo, &["push", "origin", "session/test"]);
        Repository {
            root: repo,
            branch: "session/test".into(),
            remote: remote.to_string_lossy().into(),
        }
    }
    #[test]
    fn dirty_checkpoint_and_terminal_commits_are_pushed_to_session_only() {
        let r = fixture();
        std::fs::write(r.root.join("saved"), "work").unwrap();
        let sha = r.checkpoint(true).unwrap();
        assert_eq!(
            git(
                Path::new(&r.remote),
                &["rev-parse", "refs/heads/session/test"]
            ),
            sha
        );
        git(&r.root, &["commit", "--allow-empty", "-m", "terminal"]);
        let sha = r.checkpoint(false).unwrap();
        assert_eq!(
            git(
                Path::new(&r.remote),
                &["rev-parse", "refs/heads/session/test"]
            ),
            sha
        );
        assert!(git(Path::new(&r.remote), &["branch", "--list", "master"]).is_empty());
    }
    #[test]
    fn switched_branch_changed_remote_and_existing_lock_are_preserved() {
        let r = fixture();
        git(&r.root, &["checkout", "-b", "master"]);
        assert_eq!(r.checkpoint(true), Err(Error::Changed));
        git(&r.root, &["checkout", "session/test"]);
        std::fs::write(r.root.join(".git/index.lock"), "owner").unwrap();
        assert_eq!(r.checkpoint(true), Err(Error::Busy));
        assert_eq!(
            std::fs::read_to_string(r.root.join(".git/index.lock")).unwrap(),
            "owner"
        );
        std::fs::remove_file(r.root.join(".git/index.lock")).unwrap();
        git(&r.root, &["remote", "set-url", "origin", "/wrong"]);
        assert_eq!(r.checkpoint(false), Err(Error::Changed));
    }
    #[test]
    fn rejected_push_preserves_commit_and_retry_succeeds() {
        use std::os::unix::fs::PermissionsExt;
        let r = fixture();
        let hook = Path::new(&r.remote).join("hooks/pre-receive");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(r.root.join("work"), "keep").unwrap();
        assert_eq!(r.checkpoint(true), Err(Error::Git));
        let sha = git(&r.root, &["rev-parse", "HEAD"]);
        assert_eq!(
            std::fs::read_to_string(r.root.join("work")).unwrap(),
            "keep"
        );
        std::fs::remove_file(hook).unwrap();
        assert_eq!(r.checkpoint(true).unwrap(), sha);
    }
    #[test]
    fn divergent_remote_is_never_forced() {
        let r = fixture();
        git(&r.root, &["commit", "--allow-empty", "-m", "other"]);
        git(&r.root, &["push", "origin", "session/test"]);
        git(&r.root, &["reset", "--hard", "HEAD~1"]);
        git(&r.root, &["commit", "--allow-empty", "-m", "ours"]);
        let before = git(Path::new(&r.remote), &["rev-parse", "session/test"]);
        assert_eq!(r.checkpoint(false), Err(Error::Git));
        assert_eq!(
            git(Path::new(&r.remote), &["rev-parse", "session/test"]),
            before
        );
    }
}
