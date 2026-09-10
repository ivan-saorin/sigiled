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

/// What a workspace container will run on (DEC-25). Serialized into the
/// open/recycle responses: a healthy resolve is just `{"used": …}`; a
/// fallback carries `requested` (the tag that should have been) and
/// `build_error` (the build log tail) — the shout.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ImageChoice {
    pub used: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_error: Option<String>,
}

impl ImageChoice {
    fn base(image: String) -> Self {
        ImageChoice {
            used: image,
            requested: None,
            build_error: None,
        }
    }
    fn built(tag: String) -> Self {
        ImageChoice {
            used: tag,
            requested: None,
            build_error: None,
        }
    }
}

#[derive(Clone)]
pub struct Runtime {
    #[cfg(test)]
    pub fake: Option<std::sync::Arc<crate::session_tests::FakeRuntime>>,
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
            #[cfg(test)]
            fake: None,
            network: std::env::var("SIGILED_NETWORK").unwrap_or_else(|_| "mgr-net".into()),
            image: std::env::var("SIGILED_VM_IMAGE")
                .unwrap_or_else(|_| "ghcr.io/ivan-saorin/vm-base:0.1.0".into()),
            // Instance identity has no default: a self-hoster who forgets
            // these must fail at boot, not silently operate against the
            // reference instance.
            owner: std::env::var("GITHUB_OWNER")
                .expect("GITHUB_OWNER is required when SIGILED_RUNTIME=docker (your GitHub user/org)"),
            domain: std::env::var("DOMAIN")
                .expect("DOMAIN is required when SIGILED_RUNTIME=docker (public base domain, e.g. example.com)"),
            repos_dir: std::env::var("SIGILED_REPOS_DIR")
                .unwrap_or_else(|_| format!("{state}/repos"))
                .into(),
            keys_dir: std::env::var("SIGILED_KEYS_DIR")
                .unwrap_or_else(|_| format!("{state}/keys"))
                .into(),
        })
    }

    /// Keep the full random identity and any u64 generation within one DNS
    /// label: vm-s- + 32 hex + -g + 20 decimal digits is at most 59 bytes.
    /// Project identity lives in metadata/labels, not in the routing slug.
    pub fn session_container(_project: &str, id: &str, generation: u64) -> String {
        format!("vm-s-{id}-g{generation}")
    }
    pub fn session_endpoint(&self, _project: &str, id: &str, generation: u64) -> String {
        self.endpoint(&format!("s-{id}-g{generation}"))
    }
    pub fn vm_name(project: &str) -> String {
        format!("vm-{project}")
    }
    /// Job containers carry their own name: sessions and jobs coexist in v2
    /// (no project lock), so a job must NEVER wear vm-{project} — the
    /// create-time `rm -f` of its own name would kill a live session.
    pub fn job_container(project: &str, job: &str) -> String {
        format!("vm-job-{project}-{job}")
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

    /// Refresh-only boundary. Session lifecycle callers continue using
    /// ensure_mirror; the reconciler owns the lock and one absolute deadline.
    pub(crate) fn ensure_mirror_until(
        &self,
        project: &str,
        deadline: std::time::Instant,
    ) -> Result<PathBuf, crate::bounded_process::Error> {
        use crate::bounded_process::{git, output_until, Error};
        if !crate::project::valid_name(project) {
            return Err(Error::Io);
        }
        let path = self.repo_path(project);
        let key = self.key_path(project);
        if path.join(".git").exists() {
            with_refresh_lock_recovery(&path, || {
                git(
                    &path,
                    &["config", "core.sshCommand", &ssh_command(&key)],
                    deadline,
                )?;
                git(&path, &["fetch", "--prune", "origin"], deadline)?;
                git(&path, &["checkout", "-f", "master"], deadline)?;
                git(&path, &["reset", "--hard", "origin/master"], deadline)?;
                Ok(())
            })?;
            Ok(path)
        } else {
            self.clone_mirror_using(project, deadline, |target, deadline| {
                let output = output_until(
                    crate::bounded_process::git_command()
                        .args(["clone", &self.repo_url(project)])
                        .arg(target)
                        .env("GIT_SSH_COMMAND", ssh_command(&key)),
                    deadline,
                )?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(Error::Exit)
                }
            })
        }
    }
    /// Only a freshly created, operation-owned temporary directory is removed
    /// on failure. A partial clone never occupies the published mirror path.
    fn clone_mirror_using<F>(
        &self,
        project: &str,
        deadline: std::time::Instant,
        clone: F,
    ) -> Result<PathBuf, crate::bounded_process::Error>
    where
        F: FnOnce(&Path, std::time::Instant) -> Result<(), crate::bounded_process::Error>,
    {
        use crate::bounded_process::{git, Error};
        if !crate::project::valid_name(project) {
            return Err(Error::Io);
        }
        let destination = self.repo_path(project);
        if destination.symlink_metadata().is_ok() {
            return Err(Error::Exit);
        }
        std::fs::create_dir_all(&self.repos_dir).map_err(|_| Error::Io)?;
        let mut nonce = [0u8; 16];
        getrandom::getrandom(&mut nonce).map_err(|_| Error::Io)?;
        let nonce: String = nonce.iter().map(|v| format!("{v:02x}")).collect();
        let path = self.repos_dir.join(format!(".sigiled-clone-{nonce}"));
        std::fs::create_dir(&path).map_err(|_| Error::Io)?;
        struct OwnedClone(PathBuf);
        impl Drop for OwnedClone {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let owned = OwnedClone(path);
        clone(&owned.0, deadline)?;
        git(
            &owned.0,
            &[
                "config",
                "core.sshCommand",
                &ssh_command(&self.key_path(project)),
            ],
            deadline,
        )?;
        git(
            &owned.0,
            &["rev-parse", "--verify", "master^{commit}"],
            deadline,
        )?;
        if std::time::Instant::now() >= deadline {
            return Err(Error::Deadline);
        }
        publish_clone(&owned.0, &destination)?;
        // Rename removed the temporary path. Drop never touches destination.
        Ok(destination)
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
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            return fake.docker(args);
        }
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

    /// Remove a container by its full name (session vm-{p} or job vm-job-…).
    pub fn destroy(&self, container: &str) -> bool {
        self.docker(&["rm", "-f", container]).is_ok()
    }

    /// Public docker access for the apps engine (apps.rs) — same plumbing,
    /// same error shape.
    pub fn docker_pub(&self, args: &[&str]) -> Result<String, String> {
        self.docker(args)
    }

    pub fn image_exists(&self, tag: &str) -> bool {
        self.docker(&["image", "inspect", "--format", "ok", tag])
            .is_ok()
    }

    // --- session images (DEC-25) -------------------------------------------

    /// Pure resolution of the per-project session image: the manifest's
    /// `[workspace] dockerfile` on master → `vm-{p}:df-{blob12}`, the tag
    /// content-addressed on the dockerfile's git blob. Master commits that
    /// don't touch the dockerfile keep the tag (and the image cache); editing
    /// it moves the tag and the next open rebuilds. `repo` is the mirror
    /// checkout, which ensure_mirror has already reset to origin/master.
    ///
    /// Ok(None) = no declaration: the global base image, pre-DEC-25 behavior.
    /// Err = the project DECLARED an image but the declaration is unusable
    /// (broken manifest, dockerfile missing on master) — the caller decides
    /// whether that is a shout (sessions) or a failure (jobs).
    pub fn session_image_tag(
        &self,
        project: &str,
        repo: &Path,
    ) -> Result<Option<(String, String)>, String> {
        let Some(manifest) = crate::manifest::Manifest::from_repo(repo)? else {
            return Ok(None);
        };
        let Some(dockerfile) = manifest.workspace_dockerfile else {
            return Ok(None);
        };
        let blob = crate::merge::git(repo, &["hash-object", "--", &dockerfile])
            .map_err(|e| format!("[workspace] dockerfile {dockerfile:?}: {e}"))?;
        let short = blob
            .get(..12)
            .ok_or_else(|| format!("bad blob hash {blob:?}"))?;
        Ok(Some((
            format!("{}:df-{short}", Self::vm_name(project)),
            dockerfile,
        )))
    }

    /// Resolve and, when needed, build the session image. Never fails: a
    /// declared image that cannot be built falls back to the global base
    /// with the debt in the choice — the session is the repair tool for its
    /// own dockerfile, so open must stay drivable (same philosophy as merge
    /// debt: never block, surface loudly). Jobs treat build_error as fatal.
    /// Blocking image builds survive cancellation of their awaiting request.
    /// Move the owned mirror guard into the worker and return it with the image,
    /// so neither cancellation nor post-build mirror use creates an unlock gap.
    pub async fn session_image_locked(
        &self,
        project: &str,
        repo: &Path,
        guard: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<(ImageChoice, tokio::sync::OwnedMutexGuard<()>), String> {
        let (rt, project, repo) = (self.clone(), project.to_owned(), repo.to_owned());
        tokio::task::spawn_blocking(move || {
            let image = rt.ensure_session_image(&project, &repo);
            (image, guard)
        })
        .await
        .map_err(|_| "image task interrupted".into())
    }

    pub fn ensure_session_image(&self, project: &str, repo: &Path) -> ImageChoice {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            let barrier = fake.build_barrier.lock().unwrap().clone();
            if let Some(barrier) = barrier {
                barrier.block();
            }
        }
        match self.session_image_tag(project, repo) {
            Ok(None) => ImageChoice::base(self.image.clone()),
            Ok(Some((tag, dockerfile))) => {
                if self.image_exists(&tag) {
                    return ImageChoice::built(tag);
                }
                match self.build_image(&tag, &dockerfile, repo) {
                    Ok(_) => ImageChoice::built(tag),
                    Err(tail) => ImageChoice {
                        used: self.image.clone(),
                        requested: Some(tag),
                        build_error: Some(tail),
                    },
                }
            }
            Err(e) => ImageChoice {
                used: self.image.clone(),
                requested: None,
                build_error: Some(e),
            },
        }
    }

    pub fn container_state(&self, name: &str) -> String {
        self.docker(&["inspect", "--format", "{{.State.Status}}", name])
            .unwrap_or_else(|_| "absent".into())
    }

    /// docker build from a mirror checkout at master — the log tail is the
    /// build record's diagnosis in chat, same philosophy as the supervisor.
    pub fn build_image(&self, tag: &str, dockerfile: &str, ctx: &Path) -> Result<String, String> {
        let out = Command::new("docker")
            .args(["build", "-f"])
            .arg(ctx.join(dockerfile))
            .args(["-t", tag])
            .arg(ctx)
            .output()
            .map_err(|e| format!("spawn docker build: {e}"))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let tail: String = text
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        if out.status.success() {
            Ok(tail)
        } else {
            Err(tail)
        }
    }

    /// create → inject the deploy key → start. Mirrors the v1 order for the
    /// same reason: the key must be in place before the agent boots, and it
    /// must never transit through an image layer. `extra_env` is where job
    /// secrets ride (rule 8: resolved by the caller, container env only).
    /// `image` comes from ensure_session_image (DEC-25): per-project when
    /// declared, the global base otherwise.
    pub fn create_container(
        &self,
        container: &str,
        project: &str,
        kind: &str,
        workload_id: &str,
        token: &str,
        image: &str,
        extra_env: &[(String, String)],
    ) -> Result<(), String> {
        // Docker create owns the name atomically. Never pre-delete an incumbent.
        let key = self.key_path(project);
        if !key.exists() {
            return Err(format!(
                "deploy key missing for {project}: {}",
                key.display()
            ));
        }
        let mut args: Vec<String> = vec![
            "create".into(),
            "--name".into(),
            container.into(),
            "--hostname".into(),
            container.into(),
            "--network".into(),
            self.network.clone(),
            "--label".into(),
            format!("sigiled.kind={kind}"),
            "--label".into(),
            format!("sigiled.project={project}"),
            "--label".into(),
            format!("sigiled.workload={workload_id}"),
            "-e".into(),
            format!("SESSION_TOKEN={token}"),
            "-e".into(),
            "GIT_SSH_KEY=/secrets/deploy_key".into(),
        ];
        for (name, value) in extra_env {
            args.push("-e".into());
            args.push(format!("{name}={value}"));
        }
        args.push(image.into());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.docker(&arg_refs)
            .map_err(|_| "container creation failed".to_string())?;
        let initialized = self
            .docker(&[
                "cp",
                &key.to_string_lossy(),
                &format!("{container}:/secrets/deploy_key"),
            ])
            .and_then(|_| self.docker(&["start", container]));
        if initialized.is_err() {
            // create above succeeded; cleanup cannot target an incumbent.
            self.destroy(container);
            return Err("container initialization failed".into());
        }
        Ok(())
    }

    // --- workspace agent client --------------------------------------------

    fn agent_url(&self, container: &str, path: &str) -> String {
        format!("http://{container}:8000{path}")
    }

    pub async fn wait_healthy(
        &self,
        http: &reqwest::Client,
        container: &str,
        token: &str,
    ) -> Result<(), String> {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            if fake.pause_health.load(std::sync::atomic::Ordering::SeqCst) {
                fake.health_entered.notify_one();
                fake.health_release.notified().await;
            }
            return fake.healthy(container, token);
        }
        let url = self.agent_url(container, "/health");
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
        Err(format!("{container} not healthy after 30s"))
    }

    /// How long the workspace has been idle (its /health, which by contract
    /// does NOT count as activity itself — the reaper can poll freely).
    pub async fn idle_secs(
        &self,
        http: &reqwest::Client,
        container: &str,
        token: &str,
    ) -> Result<u64, String> {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            fake.healthy(container, token)?;
            return Ok(7200);
        }
        let r = http
            .get(self.agent_url(container, "/health"))
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| format!("health: {e}"))?;
        if !r.status().is_success() {
            return Err(format!("health: {}", r.status()));
        }
        let v: serde_json::Value = r.json().await.map_err(|e| format!("health decode: {e}"))?;
        v["idle_secs"]
            .as_u64()
            .ok_or_else(|| "health: no idle_secs".into())
    }

    pub async fn exec(
        &self,
        http: &reqwest::Client,
        container: &str,
        token: &str,
        cmd: &str,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, String> {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            return fake.exec(container, token, cmd);
        }
        http.post(self.agent_url(container, "/exec"))
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
        container: &str,
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
            .exec(
                http,
                container,
                token,
                &format!("{clone} && {guard} && {branch_cmd}"),
                300,
            )
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
        let head = self
            .exec(http, container, token, "git rev-parse HEAD", 30)
            .await?;
        Ok(head["stdout"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string())
    }

    /// Commit anything left uncommitted, else make sure HEAD is pushed.
    /// A failed checkpoint preserves the container; unpushed work may exist.
    pub async fn flush(
        &self,
        http: &reqwest::Client,
        container: &str,
        token: &str,
        label: &str,
    ) -> bool {
        let cmd = autosave_cmd(label);
        matches!(
            self.exec(http, container, token, &cmd, 120).await,
            Ok(v) if v["exit"].as_i64() == Some(0)
        )
    }
}

/// The autosave flush() sends through /exec. exec runs bare bash: it does
/// NOT inject the neutral git identity the agent's own /git surface has —
/// a dirty-tree autosave died live with `Author identity unknown` (128)
/// at the first real recycle. So the command must be self-sufficient.
pub fn autosave_cmd(label: &str) -> String {
    // Same neutral identity as the agent's /git surface defaults
    // (vm-base git_api): -c loses to GIT_AUTHOR_* env by git's own
    // precedence, so a container with a real identity keeps it.
    format!(
        "git add -A && (git diff --cached --quiet || \
         git -c user.name=sigiled-session -c user.email=session@sigiled.dev \
         commit -q -m 'wip: {label} autosave') && git push -q origin HEAD"
    )
}

/// Caller must own the A1 mirror guard throughout this function. The managed
/// mirror has no concurrent Git writer: pre-existing locks are refused, never
/// removed. New known lock files can therefore only belong to this refresh.
fn with_refresh_lock_recovery<F, T>(
    repo: &Path,
    operation: F,
) -> Result<T, crate::bounded_process::Error>
where
    F: FnOnce() -> Result<T, crate::bounded_process::Error>,
{
    use crate::bounded_process::Error;
    fn locks(repo: &Path) -> Result<Vec<PathBuf>, Error> {
        let git = repo.join(".git");
        if !git
            .symlink_metadata()
            .map_err(|_| Error::Io)?
            .file_type()
            .is_dir()
        {
            return Err(Error::Io);
        }
        let mut out = vec![];
        for name in [
            "index.lock",
            "config.lock",
            "config.worktree.lock",
            "packed-refs.lock",
            "shallow.lock",
            "HEAD.lock",
            "FETCH_HEAD.lock",
            "ORIG_HEAD.lock",
        ] {
            let path = git.join(name);
            match path.symlink_metadata() {
                Ok(metadata) => {
                    if !metadata.is_file() {
                        return Err(Error::LockBusy);
                    }
                    out.push(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Error::LockBusy),
            }
        }
        let mut pending = vec![git.join("refs")];
        let mut visited = 0;
        while let Some(dir) = pending.pop() {
            let metadata = match dir.symlink_metadata() {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(Error::LockBusy),
            };
            if !metadata.file_type().is_dir() {
                return Err(Error::LockBusy);
            }
            for item in std::fs::read_dir(dir).map_err(|_| Error::LockBusy)? {
                visited += 1;
                if visited > 8192 {
                    return Err(Error::LockBusy);
                }
                let item = item.map_err(|_| Error::LockBusy)?;
                let kind = item.file_type().map_err(|_| Error::LockBusy)?;
                if kind.is_symlink() {
                    return Err(Error::LockBusy);
                }
                if kind.is_dir() {
                    pending.push(item.path());
                } else if item.file_name().to_string_lossy().ends_with(".lock") {
                    if !kind.is_file() {
                        return Err(Error::LockBusy);
                    }
                    out.push(item.path());
                }
            }
        }
        Ok(out)
    }
    if !locks(repo)?.is_empty() {
        return Err(Error::LockBusy);
    }
    let result = operation(); // bounded_process has already stopped/reaped children on return.
    let created = locks(repo)?;
    for path in &created {
        std::fs::remove_file(path).map_err(|_| Error::LockBusy)?;
    }
    if result.is_ok() && !created.is_empty() {
        return Err(Error::LockBusy);
    }
    result
}

/// Linux atomic no-replace publish: even an incumbent created after our
/// initial check cannot be replaced (including an empty directory or symlink).
#[cfg(target_os = "linux")]
fn publish_clone(source: &Path, destination: &Path) -> Result<(), crate::bounded_process::Error> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())
        .map_err(|_| crate::bounded_process::Error::Io)?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| crate::bounded_process::Error::Io)?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(crate::bounded_process::Error::Io)
    }
}
#[cfg(not(target_os = "linux"))]
fn publish_clone(_source: &Path, _destination: &Path) -> Result<(), crate::bounded_process::Error> {
    Err(crate::bounded_process::Error::UnsupportedPlatform)
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
            fake: None,
            network: "mgr-net".into(),
            image: "ghcr.io/example-org/vm-base:0.1.0".into(),
            owner: "example-org".into(),
            domain: "example.com".into(),
            repos_dir: "/data/repos".into(),
            keys_dir: "/data/keys".into(),
        }
    }

    #[test]
    fn names_and_urls_match_the_v1_routing_contract() {
        // Caddy's static rule depends on these exact shapes.
        assert_eq!(Runtime::vm_name("torchio"), "vm-torchio");
        assert_eq!(
            rt().endpoint("torchio"),
            "https://api.example.com/s/torchio/"
        );
        assert_eq!(
            rt().repo_url("torchio"),
            "git@github.com:example-org/torchio.git"
        );
        assert_eq!(
            rt().agent_url("vm-torchio", "/health"),
            "http://vm-torchio:8000/health"
        );
        // Jobs never wear the session name: rm -f on create must not be
        // able to kill a live session's container.
        assert_eq!(
            Runtime::job_container("torchio", "nightly"),
            "vm-job-torchio-nightly"
        );
    }

    #[test]
    fn runtime_is_off_unless_explicitly_asked() {
        // The tests and any dev run must never try to talk to docker.
        std::env::remove_var("SIGILED_RUNTIME");
        assert!(Runtime::from_env().is_none());
    }

    #[test]
    fn autosave_commit_is_identity_self_sufficient() {
        // Live lesson (first real recycle): /exec has no neutral git
        // identity, so the autosave commit must carry its own inline —
        // container env (GIT_AUTHOR_NAME, …) still wins when set.
        let cmd = autosave_cmd("session close");
        assert!(cmd.contains("wip: session close autosave"));
        assert!(
            cmd.contains("-c user.name="),
            "commit must carry identity: {cmd}"
        );
        assert!(
            cmd.contains("-c user.email="),
            "commit must carry identity: {cmd}"
        );
    }

    #[test]
    fn ssh_command_pins_the_project_key() {
        let c = ssh_command(Path::new("/data/keys/torchio/id_ed25519"));
        assert!(c.contains("-i /data/keys/torchio/id_ed25519"));
        assert!(c.contains("IdentitiesOnly=yes"));
    }

    #[test]
    fn session_image_is_manifest_driven_and_content_addressed() {
        let repo = crate::merge::tests::mk_repo("img");
        let rt = rt();
        // No manifest, then a manifest without [workspace]: the global base.
        assert_eq!(rt.session_image_tag("proj", &repo).unwrap(), None);
        crate::merge::tests::commit_on(
            &repo,
            "master",
            "sigiled.toml",
            "template = \"vm-tmpl@0.1.0\"\n",
            "chore: pin",
        );
        assert_eq!(rt.session_image_tag("proj", &repo).unwrap(), None);
        // Declared: vm-{p}:df-{blob12}, stable under commits that don't
        // touch the dockerfile — that stability IS the image cache key.
        crate::merge::tests::commit_on(&repo, "master", "Dockerfile", "FROM scratch\n", "feat: df");
        crate::merge::tests::commit_on(
            &repo,
            "master",
            "sigiled.toml",
            "template = \"vm-tmpl@0.1.0\"\n[workspace]\ndockerfile = \"Dockerfile\"\n",
            "feat: session image",
        );
        let (tag, df) = rt.session_image_tag("proj", &repo).unwrap().unwrap();
        assert_eq!(df, "Dockerfile");
        assert!(tag.starts_with("vm-proj:df-"), "tag {tag}");
        assert_eq!(
            tag.len(),
            "vm-proj:df-".len() + 12,
            "12-hex blob suffix: {tag}"
        );
        crate::merge::tests::commit_on(&repo, "master", "unrelated.txt", "x\n", "feat: other");
        assert_eq!(rt.session_image_tag("proj", &repo).unwrap().unwrap().0, tag);
        // Editing the dockerfile moves the tag: next open rebuilds.
        crate::merge::tests::commit_on(
            &repo,
            "master",
            "Dockerfile",
            "FROM scratch\nLABEL v=2\n",
            "feat: edit df",
        );
        assert_ne!(rt.session_image_tag("proj", &repo).unwrap().unwrap().0, tag);
    }

    #[test]
    fn declared_but_unusable_image_is_loud_not_invisible() {
        // A manifest that points at a dockerfile master doesn't have, or that
        // doesn't parse, is an Err — sessions turn it into a shouted
        // fallback, jobs into a failed run. Silence would run a workspace
        // without the toolchain the project declared: today's exact bug.
        let repo = crate::merge::tests::mk_repo("img-missing");
        crate::merge::tests::commit_on(
            &repo,
            "master",
            "sigiled.toml",
            "[workspace]\ndockerfile = \"Dockerfile.nope\"\n",
            "feat: dangling pin",
        );
        let e = rt().session_image_tag("proj", &repo).unwrap_err();
        assert!(e.contains("Dockerfile.nope"), "error names the file: {e}");
        crate::merge::tests::commit_on(&repo, "master", "sigiled.toml", "class = ", "break: toml");
        assert!(rt().session_image_tag("proj", &repo).is_err());
    }

    #[test]
    fn image_choice_serializes_quiet_when_healthy_loud_on_fallback() {
        // The open response carries this verbatim: healthy = just `used`,
        // fallback = the full shout. Contract shape, pinned here.
        let ok = serde_json::to_value(ImageChoice::built("vm-p:df-0123456789ab".into())).unwrap();
        assert_eq!(ok, serde_json::json!({"used": "vm-p:df-0123456789ab"}));
        let debt = serde_json::to_value(ImageChoice {
            used: "ghcr.io/example-org/vm-base:0.1.0".into(),
            requested: Some("vm-p:df-0123456789ab".into()),
            build_error: Some("apt: package rust not found".into()),
        })
        .unwrap();
        assert_eq!(debt["requested"], "vm-p:df-0123456789ab");
        assert!(debt["build_error"].as_str().unwrap().contains("apt"));
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

        let rt = Runtime {
            repos_dir: dir.clone(),
            ..rt()
        };
        rt.ensure_mirror("proj").unwrap();
        assert!(
            clone.join("new.txt").exists(),
            "upstream commit not fetched"
        );
        assert!(
            !clone.join("local.txt").exists(),
            "local drift survived the reset"
        );
    }
}
#[cfg(test)]
mod bounded_session_names {
    use super::*;
    #[test]
    fn full_session_entropy_and_every_u64_generation_fit_dns_labels() {
        let id = "0123456789abcdef0123456789abcdef";
        for generation in [0, 1, 9, 10, 999, u64::MAX] {
            let name = Runtime::session_container(&"p".repeat(39), id, generation);
            assert!(name.len() <= 63);
            assert!(name.contains(id));
            assert!(name.ends_with(&format!("-g{generation}")));
            assert!(name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod registry_clone_tests {
    use super::*;
    use crate::bounded_process::{output_until, Error};
    use std::time::{Duration, Instant};
    fn runtime() -> Runtime {
        let root = crate::AppState::test_without_runtime()
            .sessions
            .repos_dir
            .unwrap();
        Runtime {
            fake: None,
            network: "fixture".into(),
            image: "fixture".into(),
            owner: "fixture".into(),
            domain: "fixture.invalid".into(),
            repos_dir: root.join("repos"),
            keys_dir: root.join("keys"),
        }
    }
    fn local_clone(source: &Path, target: &Path, deadline: Instant) -> Result<(), Error> {
        let output = output_until(
            crate::bounded_process::git_command()
                .arg("clone")
                .arg(source)
                .arg(target),
            deadline,
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Error::Exit)
        }
    }
    #[test]
    fn timed_out_initial_clone_cleans_owned_partial_and_retry_publishes_valid_mirror() {
        let rt = runtime();
        let failed = rt.clone_mirror_using(
            "sample",
            Instant::now() + Duration::from_millis(100),
            |target, deadline| {
                std::fs::create_dir(target.join(".git")).unwrap();
                std::fs::write(target.join("partial"), "partial clone").unwrap();
                output_until(Command::new("sh").args(["-c", "sleep 5 & wait"]), deadline)?;
                Ok(())
            },
        );
        assert_eq!(failed.unwrap_err(), Error::Deadline);
        assert!(!rt.repo_path("sample").exists());
        assert_eq!(
            std::fs::read_dir(&rt.repos_dir).unwrap().count(),
            0,
            "partial temporary clone leaked"
        );
        let source = crate::merge::tests::mk_repo("registry-clone-retry");
        let published = rt
            .clone_mirror_using(
                "sample",
                Instant::now() + Duration::from_secs(2),
                |target, deadline| local_clone(&source, target, deadline),
            )
            .unwrap();
        assert_eq!(published, rt.repo_path("sample"));
        assert!(published.join(".git").exists());
        assert!(!published.join("partial").exists());
        assert_eq!(std::fs::read_dir(&rt.repos_dir).unwrap().count(), 1);
        crate::merge::tests::commit_on(&source, "master", "new.txt", "after clone", "new revision");
        rt.ensure_mirror_until("sample", Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(published.join("new.txt")).unwrap(),
            "after clone"
        );
    }
    #[test]
    fn temporary_clone_cleanup_waits_for_group_exit_acknowledgement() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        };
        let rt = runtime();
        let allowed = Arc::new(AtomicBool::new(false));
        let gate = allowed.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let task = std::thread::spawn(move || {
            let result = rt.clone_mirror_using(
                "sample",
                Instant::now() + Duration::from_millis(60),
                |target, deadline| {
                    std::fs::write(target.join("partial"), "owned clone").unwrap();
                    let target = target.to_path_buf();
                    crate::bounded_process::output_until_observed(
                        Command::new("sh").args(["-c", "sleep 5 & exit 0"]),
                        deadline,
                        move |_| {
                            let _ = started_tx.send(target.clone());
                            Ok(gate.load(Ordering::SeqCst))
                        },
                    )?;
                    Ok(())
                },
            );
            done_tx.send(result).unwrap();
        });
        let temporary = started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let premature = done_rx.recv_timeout(Duration::from_millis(120)).is_ok();
        let retained = temporary.join("partial").exists();
        allowed.store(true, Ordering::SeqCst);
        task.join().unwrap();
        assert!(!premature, "clone cleanup ran before exit acknowledgement");
        assert!(
            retained,
            "owned temporary clone was removed while exit was unknown"
        );
        assert!(
            !temporary.exists(),
            "temporary clone not cleaned after verified exit"
        );
    }
    #[test]
    fn atomic_clone_publish_never_replaces_an_incumbent_created_during_clone() {
        let rt = runtime();
        let source = crate::merge::tests::mk_repo("registry-clone-incumbent");
        let destination = rt.repo_path("sample");
        let result = rt.clone_mirror_using(
            "sample",
            Instant::now() + Duration::from_secs(2),
            |target, deadline| {
                local_clone(&source, target, deadline)?;
                std::fs::create_dir(&destination).unwrap();
                std::fs::write(destination.join("incumbent"), "keep me").unwrap();
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(destination.join("incumbent")).unwrap(),
            "keep me"
        );
        assert!(!destination.join(".git").exists());
        assert_eq!(std::fs::read_dir(&rt.repos_dir).unwrap().count(), 1);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod registry_lock_recovery_tests {
    use super::*;
    use crate::bounded_process::{git, output_until, Error};
    use std::time::{Duration, Instant};
    #[test]
    fn timed_out_refresh_reclaims_only_its_new_git_locks_and_retry_succeeds() {
        let repo = crate::merge::tests::mk_repo("registry-owned-git-lock");
        let result = with_refresh_lock_recovery(&repo, || {
            std::fs::write(repo.join(".git/index.lock"), "operation-owned lock").unwrap();
            output_until(
                Command::new("sh").args(["-c", "sleep 5 & wait"]),
                Instant::now() + Duration::from_millis(80),
            )?;
            Ok(())
        });
        assert_eq!(result.unwrap_err(), Error::Deadline);
        assert!(!repo.join(".git/index.lock").exists());
        with_refresh_lock_recovery(&repo, || {
            git(
                &repo,
                &["reset", "--hard", "master"],
                Instant::now() + Duration::from_secs(1),
            )
        })
        .unwrap();
    }
    #[test]
    fn preexisting_git_lock_is_preserved_and_no_operation_runs() {
        let repo = crate::merge::tests::mk_repo("registry-incumbent-git-lock");
        let lock = repo.join(".git/index.lock");
        std::fs::write(&lock, "incumbent").unwrap();
        let result: Result<(), Error> =
            with_refresh_lock_recovery(&repo, || panic!("ran across incumbent lock"));
        assert_eq!(result.unwrap_err(), Error::LockBusy);
        assert_eq!(std::fs::read_to_string(lock).unwrap(), "incumbent");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod registry_quiescence_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    use std::time::{Duration, Instant};
    #[test]
    fn git_lock_cleanup_waits_for_descendant_exit_acknowledgement() {
        let repo = crate::merge::tests::mk_repo("registry-quiescence");
        let lock = repo.join(".git/index.lock");
        let allowed = Arc::new(AtomicBool::new(false));
        let gate = allowed.clone();
        let (observed_tx, observed_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let task = std::thread::spawn(move || {
            let result = with_refresh_lock_recovery(&repo, || {
                std::fs::write(repo.join(".git/index.lock"), "owned").unwrap();
                crate::bounded_process::output_until_observed(
                    Command::new("sh").args(["-c", "sleep 5 & exit 0"]),
                    Instant::now() + Duration::from_millis(60),
                    move |pid| {
                        let _ = observed_tx.send(pid);
                        if gate.load(Ordering::SeqCst) {
                            Ok(true)
                        } else {
                            Err(crate::bounded_process::Error::Io)
                        }
                    },
                )
            });
            done_tx.send(result).unwrap();
        });
        let pid = observed_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let premature = done_rx.recv_timeout(Duration::from_millis(120)).is_ok();
        let retained_lock = lock.exists();
        let reserved_leader = std::path::Path::new(&format!("/proc/{pid}")).exists();
        // Always release the injected uncertainty before asserting or joining.
        allowed.store(true, Ordering::SeqCst);
        task.join().unwrap();
        assert!(
            !premature,
            "cleanup returned before descendant exit acknowledgement"
        );
        assert!(retained_lock, "Git lock removed before acknowledgement");
        assert!(
            reserved_leader,
            "leader identity reaped before acknowledgement"
        );
        assert!(
            !lock.exists(),
            "safe cleanup did not recover the owned Git lock"
        );
    }
}
