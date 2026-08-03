// runtime.rs — the workspace runtime: containers, deploy keys, repo mirrors.
// Same machinery the v1 proved in production, ported to the v2 shape:
//
//   * container name `vm-{project}` on the stack network IS the routing
//     table — Caddy resolves it via docker DNS with one static rule, so a
//     session never needs an edge reload;
//   * the per-project deploy key is copied into the container after create
//     and before start: it lives and dies with that container, never in an
//     image, a repo or a shared volume;
//   * a local mirror per project under SIGILED_REPOS_DIR is where close
//     does its merge (§4) before pushing master — the v1 asked GitHub to
//     fast-forward, the v2 arbitrates locally and pushes the result.
//
// Docker is driven through the CLI (docker-cli in the image): the v1 used
// the Python SDK, here shelling keeps the dependency list untouched.
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone)]
pub struct Runtime {
    pub network: String,
    pub image: String,
    pub owner: String,
    pub domain: String,
    pub repos_dir: PathBuf,
    pub keys_dir: PathBuf,
}

impl Runtime {
    /// Enabled only when SIGILED_RUNTIME=docker: without it the verbs stay
    /// on the branch-only path (dev runs and tests, where docker is absent
    /// by construction — the workspace itself is a sealed container).
    pub fn from_env() -> Option<Self> {
        if std::env::var("SIGILED_RUNTIME").as_deref() != Ok("docker") {
            return None;
        }
        let state = std::env::var("SIGILED_STATE_DIR").unwrap_or_else(|_| "/data".into());
        Some(Runtime {
            network: std::env::var("SIGILED_NETWORK").unwrap_or_else(|_| "mgr-net".into()),
            image: std::env::var("SIGILED_VM_IMAGE")
                .unwrap_or_else(|_| "ghcr.io/ivan-saorin/vm-base:0.1.0".into()),
            owner: std::env::var("GITHUB_OWNER").unwrap_or_else(|_| "ivan-saorin".into()),
            domain: std::env::var("DOMAIN").unwrap_or_else(|_| "016180.xyz".into()),
            repos_dir: std::env::var("SIGILED_REPOS_DIR")
                .unwrap_or_else(|_| format!("{state}/repos"))
                .into(),
            keys_dir: std::env::var("SIGILED_KEYS_DIR")
                .unwrap_or_else(|_| format!("{state}/keys"))
                .into(),
        })
    }

    pub fn vm_name(project: &str) -> String {
        format!("vm-{project}")
    }
    pub fn endpoint(&self, project: &str) -> String {
        format!("https://api.{}/s/{}/", self.domain, project)
    }
    pub fn repo_url(&self, project: &str) -> String {
        format!("git@github.com:{}/{}.git", self.owner, project)
    }
    pub fn repo_path(&self, project: &str) -> PathBuf {
        self.repos_dir.join(project)
    }
    fn key_path(&self, project: &str) -> PathBuf {
        self.keys_dir.join(project).join("id_ed25519")
    }

    /// The mirror sigiledd merges in. Cloned on first use, refreshed after:
    /// it is a cache of GitHub, never the source of truth. The deploy key is
    /// baked into the mirror's core.sshCommand so EVERY later git op (the
    /// refresh fetch, the branch fetch at close) authenticates by itself —
    /// the first live close taught us what happens otherwise.
    pub fn ensure_mirror(&self, project: &str) -> Result<PathBuf, String> {
        let path = self.repo_path(project);
        let key = self.key_path(project);
        if path.join(".git").exists() {
            crate::merge::git(&path, &["config", "core.sshCommand", &ssh_command(&key)])?;
            crate::merge::git(&path, &["fetch", "--prune", "origin"])?;
            // Local master tracks origin: nothing but close moves it here.
            crate::merge::git(&path, &["checkout", "-f", "master"])?;
            crate::merge::git(&path, &["reset", "--hard", "origin/master"])?;
            return Ok(path);
        }
        std::fs::create_dir_all(&self.repos_dir).map_err(|e| format!("repos dir: {e}"))?;
        let out = Command::new("git")
            .args(["clone", &self.repo_url(project)])
            .arg(&path)
            .env("GIT_SSH_COMMAND", ssh_command(&key))
            .output()
            .map_err(|e| format!("spawn git clone: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "clone {project}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        crate::merge::git(&path, &["config", "core.sshCommand", &ssh_command(&key)])?;
        Ok(path)
    }

    /// Push a ref with the project's deploy key (close, after the merge).
    pub fn push(&self, project: &str, refspec: &str) -> Result<String, String> {
        let path = self.repo_path(project);
        let out = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["push", "origin", refspec])
            .env("GIT_SSH_COMMAND", ssh_command(&self.key_path(project)))
            .output()
            .map_err(|e| format!("spawn git push: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "push {refspec}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    // --- containers ---------------------------------------------------------

    fn docker(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new("docker")
            .args(args)
            .output()
            .map_err(|e| format!("spawn docker: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "docker {}: {}",
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn destroy(&self, project: &str) -> bool {
        self.docker(&["rm", "-f", &Self::vm_name(project)]).is_ok()
    }

    /// create → inject the deploy key → start. Mirrors the v1 order for the
    /// same reason: the key must be in place before the agent boots, and it
    /// must never transit through an image layer.
    pub fn create_container(&self, project: &str, session_id: &str, token: &str) -> Result<(), String> {
        self.destroy(project); // stale container safety, as v1
        let name = Self::vm_name(project);
        let key = self.key_path(project);
        if !key.exists() {
            return Err(format!("deploy key missing for {project}: {}", key.display()));
        }
        self.docker(&[
            "create",
            "--name", &name,
            "--hostname", &name,
            "--network", &self.network,
            "--label", &format!("sigiled.kind=session"),
            "--label", &format!("sigiled.project={project}"),
            "--label", &format!("sigiled.workload={session_id}"),
            "-e", &format!("SESSION_TOKEN={token}"),
            "-e", "GIT_SSH_KEY=/secrets/deploy_key",
            &self.image,
        ])?;
        self.docker(&["cp", &key.to_string_lossy(), &format!("{name}:/secrets/deploy_key")])?;
        self.docker(&["start", &name])?;
        Ok(())
    }

    // --- workspace agent client --------------------------------------------

    fn agent_url(&self, project: &str, path: &str) -> String {
        format!("http://{}:8000{path}", Self::vm_name(project))
    }

    pub async fn wait_healthy(
        &self,
        http: &reqwest::Client,
        project: &str,
        token: &str,
    ) -> Result<(), String> {
        let url = self.agent_url(project, "/health");
        for _ in 0..30 {
            let ok = http
                .get(&url)
                .bearer_auth(token)
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if ok {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        Err(format!("{} not healthy after 30s", Self::vm_name(project)))
    }

    pub async fn exec(
        &self,
        http: &reqwest::Client,
        project: &str,
        token: &str,
        cmd: &str,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, String> {
        http.post(self.agent_url(project, "/exec"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "cmd": cmd, "timeout_secs": timeout_secs }))
            .timeout(std::time::Duration::from_secs(timeout_secs + 10))
            .send()
            .await
            .map_err(|e| format!("exec: {e}"))?
            .json()
            .await
            .map_err(|e| format!("exec decode: {e}"))
    }

    /// Bootstrap inside the fresh container: clone, then cut (or resume) the
    /// session branch and push it immediately — push-early is what makes
    /// container destruction always safe (contract rule 3).
    pub async fn boot_workspace(
        &self,
        http: &reqwest::Client,
        project: &str,
        token: &str,
        branch: &str,
        resume: bool,
    ) -> Result<String, String> {
        let clone = format!("git clone {} . 2>&1", self.repo_url(project));
        let guard = "git rev-parse -q --verify HEAD >/dev/null || exit 9";
        let branch_cmd = if resume {
            format!("git checkout {branch}")
        } else {
            format!("git checkout -b {branch} && git push -u origin {branch}")
        };
        let r = self
            .exec(http, project, token, &format!("{clone} && {guard} && {branch_cmd}"), 300)
            .await?;
        match r["exit"].as_i64() {
            Some(0) => {}
            Some(9) => return Err(format!("repo '{project}' has no commits yet")),
            _ => {
                return Err(format!(
                    "workspace bootstrap failed: {}{}",
                    r["stderr"].as_str().unwrap_or(""),
                    r["stdout"].as_str().unwrap_or("")
                ))
            }
        }
        let head = self.exec(http, project, token, "git rev-parse HEAD", 30).await?;
        Ok(head["stdout"].as_str().unwrap_or_default().trim().to_string())
    }

    /// Commit anything left uncommitted, else make sure HEAD is pushed.
    /// An unreachable container is not an error: push-early means only
    /// already-pushed work exists (the v1 learned this the hard way).
    pub async fn flush(&self, http: &reqwest::Client, project: &str, token: &str, label: &str) -> bool {
        let cmd = format!(
            "git add -A && (git diff --cached --quiet || git commit -q -m 'wip: {label} autosave') && git push -q origin HEAD"
        );
        matches!(
            self.exec(http, project, token, &cmd, 120).await,
            Ok(v) if v["exit"].as_i64() == Some(0)
        )
    }
}

fn ssh_command(key: &Path) -> String {
    format!(
        "ssh -i {} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new",
        key.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt() -> Runtime {
        Runtime {
            network: "mgr-net".into(),
            image: "ghcr.io/ivan-saorin/vm-base:0.1.0".into(),
            owner: "ivan-saorin".into(),
            domain: "016180.xyz".into(),
            repos_dir: "/data/repos".into(),
            keys_dir: "/data/keys".into(),
        }
    }

    #[test]
    fn names_and_urls_match_the_v1_routing_contract() {
        // Caddy's static rule depends on these exact shapes.
        assert_eq!(Runtime::vm_name("torchio"), "vm-torchio");
        assert_eq!(rt().endpoint("torchio"), "https://api.016180.xyz/s/torchio/");
        assert_eq!(rt().repo_url("torchio"), "git@github.com:ivan-saorin/torchio.git");
        assert_eq!(rt().agent_url("torchio", "/health"), "http://vm-torchio:8000/health");
    }

    #[test]
    fn runtime_is_off_unless_explicitly_asked() {
        // The tests and any dev run must never try to talk to docker.
        std::env::remove_var("SIGILED_RUNTIME");
        assert!(Runtime::from_env().is_none());
    }

    #[test]
    fn ssh_command_pins_the_project_key() {
        let c = ssh_command(Path::new("/data/keys/torchio/id_ed25519"));
        assert!(c.contains("-i /data/keys/torchio/id_ed25519"));
        assert!(c.contains("IdentitiesOnly=yes"));
    }

    #[test]
    fn mirror_refresh_resets_master_to_origin() {
        // A mirror that drifted (a failed close, a manual poke) must come
        // back to origin/master before the next merge — otherwise the debt
        // package would blame the wrong side.
        let origin = crate::merge::tests::mk_repo("mirror-origin");
        let dir = crate::merge::tests::tmp_repo("mirror-clone");
        std::fs::create_dir_all(&dir).unwrap();
        let clone = dir.join("proj");
        crate::merge::git(
            &origin,
            &["clone", &origin.to_string_lossy(), &clone.to_string_lossy()],
        )
        .unwrap();
        crate::merge::tests::commit_on(&origin, "master", "new.txt", "n\n", "feat: upstream");
        crate::merge::tests::commit_on(&clone, "master", "local.txt", "l\n", "wip: local drift");

        let rt = Runtime { repos_dir: dir.clone(), ..rt() };
        rt.ensure_mirror("proj").unwrap();
        assert!(clone.join("new.txt").exists(), "upstream commit not fetched");
        assert!(!clone.join("local.txt").exists(), "local drift survived the reset");
    }
}
