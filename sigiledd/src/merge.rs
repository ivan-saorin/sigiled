// merge.rs — the close-time reconciliation engine (design §4): under the
// project's merge lock, try fast-forward → three-way merge → merge debt.
// Master stays put on conflict; the branch survives; the debt package is
// the context for whoever resolves it (they wrote neither half — the
// intent-carrying commit messages pay off here a second time).
//
// Shells out to git like vm-base does: the repo is a normal worktree at
// SIGILED_REPOS_DIR/{project}. Container runtime is cutover territory —
// this engine is what the real close verb will call.
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebtSide {
    pub sha: String,
    pub commit_messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeDebt {
    pub branch: String,
    pub conflicted_files: Vec<String>,
    pub ours: DebtSide,
    pub theirs: DebtSide,
    /// ISO date of the merge base — since when the two sides diverged.
    pub since: String,
}

#[derive(Debug)]
pub enum MergeOutcome {
    /// Master fast-forwarded (or the branch brought nothing new).
    Ff { sha: String },
    /// Master had moved; git fused the disjoint changes on its own.
    Merged { sha: String },
    /// Conflict: master untouched, branch kept, debt recorded.
    Debt(MergeDebt),
}

pub fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "sigiledd")
        .env("GIT_AUTHOR_EMAIL", "sigiledd@sigiled.dev")
        .env("GIT_COMMITTER_NAME", "sigiledd")
        .env("GIT_COMMITTER_EMAIL", "sigiledd@sigiled.dev")
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn lines(s: String) -> Vec<String> {
    s.lines()
        .map(str::to_string)
        .filter(|l| !l.is_empty())
        .collect()
}

/// The paths a session branch changed vs the master it forked from —
/// feeds the honest close hint (events::log_operativo_touched).
pub fn changed_paths(repo: &Path, branch: &str) -> Result<Vec<String>, String> {
    let base = git(repo, &["merge-base", "master", branch])?;
    Ok(lines(git(
        repo,
        &["diff", "--name-only", &format!("{base}..{branch}")],
    )?))
}

pub fn close_merge(repo: &Path, branch: &str) -> Result<MergeOutcome, String> {
    let master = git(repo, &["rev-parse", "refs/heads/master"])?;
    let head = git(repo, &["rev-parse", branch])?;
    let base = git(repo, &["merge-base", "master", branch])?;

    // Empty session: nothing to merge, master is already the answer.
    if head == base {
        return Ok(MergeOutcome::Ff { sha: master });
    }
    // Master has not moved: fast-forward, the common case.
    if master == base {
        git(repo, &["update-ref", "refs/heads/master", &head])?;
        git(repo, &["checkout", "-f", "master"])?;
        return Ok(MergeOutcome::Ff { sha: head });
    }
    // Master moved: three-way merge on a master checkout.
    git(repo, &["checkout", "-f", "master"])?;
    let merged = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge", "--no-ff", "--no-edit", branch])
        .env("GIT_AUTHOR_NAME", "sigiledd")
        .env("GIT_AUTHOR_EMAIL", "sigiledd@sigiled.dev")
        .env("GIT_COMMITTER_NAME", "sigiledd")
        .env("GIT_COMMITTER_EMAIL", "sigiledd@sigiled.dev")
        .output()
        .map_err(|e| format!("spawn git merge: {e}"))?;
    if merged.status.success() {
        let sha = git(repo, &["rev-parse", "HEAD"])?;
        return Ok(MergeOutcome::Merged { sha });
    }

    // Conflict: harvest the debt package, then put master back untouched.
    let conflicted = lines(git(repo, &["diff", "--name-only", "--diff-filter=U"])?);
    let ours_msgs = lines(git(
        repo,
        &["log", "--format=%s", &format!("{base}..{master}")],
    )?);
    let theirs_msgs = lines(git(
        repo,
        &["log", "--format=%s", &format!("{base}..{head}")],
    )?);
    let since = git(repo, &["show", "-s", "--format=%cI", &base])?;
    git(repo, &["merge", "--abort"])?;

    Ok(MergeOutcome::Debt(MergeDebt {
        branch: branch.to_string(),
        conflicted_files: conflicted,
        ours: DebtSide {
            sha: master,
            commit_messages: ours_msgs,
        },
        theirs: DebtSide {
            sha: head,
            commit_messages: theirs_msgs,
        },
        since,
    }))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Unique-enough temp repo path: pid + a per-process counter.
    pub fn tmp_repo(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "sigiledd-test-{}-{}-{}",
            std::process::id(),
            tag,
            n
        ))
    }

    pub fn sh(repo: &Path, args: &[&str]) -> String {
        git(repo, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    pub fn write(repo: &Path, file: &str, content: &str) {
        let p = repo.join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    pub fn mk_repo(tag: &str) -> PathBuf {
        let repo = tmp_repo(tag);
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q", "-b", "master"]);
        write(&repo, "README.md", "smoke\n");
        write(&repo, "docs/log-operativo.md", "# log\n");
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "genesis"]);
        repo
    }

    pub fn commit_on(repo: &Path, branch: &str, file: &str, content: &str, msg: &str) {
        sh(repo, &["checkout", "-q", branch]);
        write(repo, file, content);
        sh(repo, &["add", "-A"]);
        sh(repo, &["commit", "-q", "-m", msg]);
    }

    #[test]
    fn still_master_fast_forwards() {
        let repo = mk_repo("ff");
        sh(&repo, &["branch", "session/a", "master"]);
        commit_on(&repo, "session/a", "a.txt", "a\n", "feat: a");
        match close_merge(&repo, "session/a").unwrap() {
            MergeOutcome::Ff { sha } => {
                assert_eq!(sha, sh(&repo, &["rev-parse", "refs/heads/master"]));
            }
            o => panic!("expected Ff, got {o:?}"),
        }
    }

    #[test]
    fn empty_session_is_a_noop_ff() {
        let repo = mk_repo("noop");
        sh(&repo, &["branch", "session/a", "master"]);
        let before = sh(&repo, &["rev-parse", "master"]);
        match close_merge(&repo, "session/a").unwrap() {
            MergeOutcome::Ff { sha } => assert_eq!(sha, before),
            o => panic!("expected Ff, got {o:?}"),
        }
    }

    #[test]
    fn moved_master_disjoint_changes_merge_clean() {
        let repo = mk_repo("threeway");
        sh(&repo, &["branch", "session/a", "master"]);
        commit_on(&repo, "master", "left.txt", "L\n", "feat: left");
        commit_on(&repo, "session/a", "right.txt", "R\n", "feat: right");
        match close_merge(&repo, "session/a").unwrap() {
            MergeOutcome::Merged { sha } => {
                assert_eq!(sha, sh(&repo, &["rev-parse", "master"]));
                // Merge commit, not rebase: two parents (design §4.2).
                let parents = sh(&repo, &["show", "-s", "--format=%P", &sha]);
                assert_eq!(parents.split_whitespace().count(), 2);
            }
            o => panic!("expected Merged, got {o:?}"),
        }
    }

    #[test]
    fn conflict_yields_debt_and_master_stays_put() {
        let repo = mk_repo("debt");
        sh(&repo, &["branch", "session/a", "master"]);
        commit_on(&repo, "master", "hot.txt", "ours\n", "fix: ours side");
        commit_on(
            &repo,
            "session/a",
            "hot.txt",
            "theirs\n",
            "fix: theirs side",
        );
        let master_before = sh(&repo, &["rev-parse", "refs/heads/master"]);
        match close_merge(&repo, "session/a").unwrap() {
            MergeOutcome::Debt(d) => {
                assert_eq!(d.branch, "session/a");
                assert_eq!(d.conflicted_files, vec!["hot.txt"]);
                assert_eq!(d.ours.commit_messages, vec!["fix: ours side"]);
                assert_eq!(d.theirs.commit_messages, vec!["fix: theirs side"]);
                assert!(!d.since.is_empty());
                assert_eq!(d.ours.sha, master_before);
            }
            o => panic!("expected Debt, got {o:?}"),
        }
        // Master untouched, branch survived, no merge left in progress.
        assert_eq!(
            sh(&repo, &["rev-parse", "refs/heads/master"]),
            master_before
        );
        assert!(sh(&repo, &["branch", "--list", "session/a"]).contains("session/a"));
        assert!(!repo.join(".git/MERGE_HEAD").exists());
    }

    #[test]
    fn debt_resolution_closes_as_merge() {
        let repo = mk_repo("resolve");
        sh(&repo, &["branch", "session/a", "master"]);
        commit_on(&repo, "master", "hot.txt", "ours\n", "fix: ours");
        commit_on(&repo, "session/a", "hot.txt", "theirs\n", "fix: theirs");
        assert!(matches!(
            close_merge(&repo, "session/a").unwrap(),
            MergeOutcome::Debt(_)
        ));
        // The resolution protocol (§4.2): on the debtor branch, merge master,
        // resolve explaining what was kept, commit, close again.
        sh(&repo, &["checkout", "-q", "session/a"]);
        // Conflict expected: git() errs on the nonzero exit, MERGE_HEAD stays.
        let _ = git(&repo, &["merge", "master"]);
        assert!(
            repo.join(".git/MERGE_HEAD").exists(),
            "merge master did not start"
        );
        write(&repo, "hot.txt", "ours+theirs reconciled\n");
        sh(&repo, &["add", "-A"]);
        sh(
            &repo,
            &[
                "commit",
                "-q",
                "-m",
                "merge: kept both sides, reconciled hot.txt",
            ],
        );
        match close_merge(&repo, "session/a").unwrap() {
            MergeOutcome::Ff { sha } | MergeOutcome::Merged { sha } => {
                assert_eq!(sha, sh(&repo, &["rev-parse", "refs/heads/master"]));
            }
            o => panic!("expected clean close after resolution, got {o:?}"),
        }
    }

    #[test]
    fn changed_paths_feed_the_close_hint() {
        let repo = mk_repo("hint");
        sh(&repo, &["branch", "session/a", "master"]);
        commit_on(
            &repo,
            "session/a",
            "docs/log-operativo.md",
            "# log\nentry\n",
            "log: entry",
        );
        let paths = changed_paths(&repo, "session/a").unwrap();
        assert!(crate::events::log_operativo_touched(&paths));
        let repo2 = mk_repo("hint2");
        sh(&repo2, &["branch", "session/b", "master"]);
        commit_on(&repo2, "session/b", "src.rs", "fn x(){}\n", "feat: x");
        assert!(!crate::events::log_operativo_touched(
            &changed_paths(&repo2, "session/b").unwrap()
        ));
    }
}
