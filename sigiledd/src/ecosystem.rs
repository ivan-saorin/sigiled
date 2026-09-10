//! Persisted, non-secret manifest projection. No sessions or workloads are created here.
use crate::{declaration::Declaration, manifest::Manifest, project::Registry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshError {
    RepositoryUnavailable,
    ManifestInvalid,
    PersistFailed,
    Interrupted,
}
impl RefreshError {
    pub fn message(self) -> &'static str {
        match self {
            Self::RepositoryUnavailable => "Repository revision could not be read; retry scheduled",
            Self::ManifestInvalid => {
                "Manifest declaration is invalid; last valid descriptor retained"
            }
            Self::PersistFailed => "Descriptor persistence failed; retry scheduled",
            Self::Interrupted => "Reconciliation interrupted; retry scheduled",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobDefinition {
    pub name: String,
    pub cron: String,
    pub timeout_minutes: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Descriptor {
    pub declaration: Declaration,
    pub app: Option<String>,
    pub jobs: Vec<JobDefinition>,
    pub desired_revision: Option<String>,
    pub observed_revision: Option<String>,
    pub observed_at: Option<u64>,
    pub attempted_at: Option<u64>,
    pub retry_at: Option<u64>,
    pub error: Option<RefreshError>,
    pub failures: u32,
}
impl Registry {
    pub fn descriptors(&self) -> BTreeMap<String, Descriptor> {
        self.descriptors.read().unwrap().clone()
    }
    pub fn descriptor(&self, name: &str) -> Descriptor {
        self.descriptors
            .read()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_default()
    }
    pub fn hydrate_descriptors(&self, values: BTreeMap<String, Descriptor>) {
        *self.descriptors.write().unwrap() = values
            .into_iter()
            .filter(|(n, _)| self.contains(n))
            .collect();
    }
    fn observe(
        &self,
        name: &str,
        manifest: &Manifest,
        revision: &str,
        at: u64,
        completed_at: u64,
    ) -> bool {
        if !self.contains(name) {
            return false;
        }
        let template_changed = self.refresh(name, manifest);
        let mut map = self.descriptors.write().unwrap();
        let d = map.entry(name.into()).or_default();
        let previous = d.clone();
        let sharing = manifest
            .declaration
            .memory
            .sharing
            .clone()
            .or_else(|| d.declaration.memory.sharing.clone())
            .or_else(|| Some("private".into()));
        d.declaration = manifest.declaration.clone();
        d.declaration.memory.sharing = sharing;
        d.app = manifest.app.as_ref().map(|a| a.name.clone());
        d.jobs = manifest
            .jobs
            .iter()
            .map(|j| JobDefinition {
                name: j.name.clone(),
                cron: j.cron.clone(),
                timeout_minutes: j.timeout_minutes,
            })
            .collect();
        d.desired_revision = Some(revision.into());
        d.observed_revision = Some(revision.into());
        d.observed_at = Some(completed_at);
        d.attempted_at = Some(at);
        d.retry_at = Some(completed_at.saturating_add(300));
        d.error = None;
        d.failures = 0;
        template_changed || *d != previous
    }
    fn failed(&self, name: &str, revision: Option<String>, error: RefreshError, at: u64) -> bool {
        if !self.contains(name) {
            return false;
        }
        let mut map = self.descriptors.write().unwrap();
        let d = map.entry(name.into()).or_default();
        if revision.is_some() {
            d.desired_revision = revision;
        }
        d.error = Some(error);
        d.attempted_at = Some(at);
        d.failures = d.failures.saturating_add(1);
        d.retry_at = Some(at.saturating_add(30u64.saturating_mul(1u64 << d.failures.min(5))));
        true
    }
}
fn read_manifest(
    state: &crate::AppState,
    project: &str,
) -> Result<(Manifest, String), (RefreshError, Option<String>)> {
    let bad_repo = || (RefreshError::RepositoryUnavailable, None);
    let repo = match &state.sessions.runtime {
        Some(rt) => rt.ensure_mirror(project).map_err(|_| bad_repo())?,
        None => state
            .sessions
            .repos_dir
            .as_ref()
            .map(|p| p.join(project))
            .ok_or_else(bad_repo)?,
    };
    let revision = crate::merge::git(&repo, &["rev-parse", "master^{commit}"])
        .map_err(|_| bad_repo())?
        .trim()
        .to_string();
    // List the tree first: a failed read is never mistaken for deliberate removal.
    let files =
        crate::merge::git(&repo, &["ls-tree", "--name-only", "master"]).map_err(|_| bad_repo())?;
    for f in ["sigiled.toml", "mgr.toml"] {
        if files.lines().any(|p| p == f) {
            let text = crate::merge::git(&repo, &["show", &format!("master:{f}")])
                .map_err(|_| (RefreshError::RepositoryUnavailable, Some(revision.clone())))?;
            return Manifest::parse(&text)
                .map(|m| (m, revision.clone()))
                .map_err(|_| (RefreshError::ManifestInvalid, Some(revision)));
        }
    }
    Ok((Manifest::parse("").expect("empty manifest"), revision))
}
/// The blocking task OWNS the guard, including publication and persistence.
/// Cancellation of a caller cannot release the mirror while Git still runs.
async fn refresh_locked(
    state: crate::AppState,
    project: String,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<Manifest, RefreshError> {
    refresh_using(state, project, guard, read_manifest).await
}
type Observation = Result<(Manifest, String), (RefreshError, Option<String>)>;
async fn refresh_using<F>(
    state: crate::AppState,
    project: String,
    guard: tokio::sync::OwnedMutexGuard<()>,
    reader: F,
) -> Result<Manifest, RefreshError>
where
    F: FnOnce(&crate::AppState, &str) -> Observation + Send + 'static,
{
    static WORKERS: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)));
    let permit = WORKERS
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| RefreshError::Interrupted)?;
    tokio::task::spawn_blocking(move || {
        let _worker = permit;
        let _mirror = guard;
        let now = crate::auth::now_epoch();
        let result = reader(&state, &project);
        let changed = match &result {
            Ok((manifest, revision)) => {
                state
                    .registry
                    .observe(&project, manifest, revision, now, crate::auth::now_epoch())
            }
            Err((error, revision)) => {
                state
                    .registry
                    .failed(&project, revision.clone(), *error, now)
            }
        };
        if changed && state.try_persist().is_err() {
            state
                .registry
                .failed(&project, None, RefreshError::PersistFailed, now);
            return Err(RefreshError::PersistFailed);
        }
        result
            .map(|(manifest, _)| manifest)
            .map_err(|(error, _)| error)
    })
    .await
    .map_err(|_| RefreshError::Interrupted)?
}
pub async fn refresh(state: &crate::AppState, project: &str) -> Result<Manifest, RefreshError> {
    let guard = state.sessions.merge_lock(project).lock_owned().await;
    refresh_locked(state.clone(), project.into(), guard).await
}
/// One bounded batch, sorted with round-robin continuation. Busy mirrors are skipped.
/// Only one blocking refresh per project can survive a timeout.
pub async fn repair_batch(state: &crate::AppState, after: &mut String) {
    let mut projects = state.registry.snapshot();
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    let start = projects.partition_point(|p| p.name <= *after);
    let len = projects.len();
    for i in 0..len.min(16) {
        let name = &projects[(start + i) % len].name;
        *after = name.clone();
        let d = state.registry.descriptor(name);
        if d.retry_at.is_some_and(|t| t > crate::auth::now_epoch()) {
            continue;
        }
        if let Ok(guard) = state.sessions.merge_lock(name).try_lock_owned() {
            let operation = refresh_locked(state.clone(), name.clone(), guard);
            if tokio::time::timeout(std::time::Duration::from_secs(30), operation)
                .await
                .is_err()
            {
                // The worker still owns the lock. Its eventual result supersedes this attempt.
                state.registry.failed(
                    name,
                    None,
                    RefreshError::RepositoryUnavailable,
                    crate::auth::now_epoch(),
                );
                if state.try_persist().is_err() {
                    tracing::error!("registry retry persistence failed");
                }
            }
        }
    }
}
pub async fn repair_loop(state: crate::AppState) {
    let mut after = String::new();
    loop {
        repair_batch(&state, &mut after).await;
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {},
            _ = state.registry.reconcile.notified() => {},
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn register(state: &crate::AppState, name: &str) {
        state.registry.insert(crate::project::ProjectRecord::new(
            name,
            &Manifest::parse("").unwrap(),
            None,
        ));
    }
    #[test]
    fn old_snapshot_has_safe_defaults_and_sharing_survives_omission() {
        let snap:crate::store::StateSnapshot=serde_json::from_str(r#"{"projects":[{"name":"legacy","template_version":null,"template_behind":false,"needs_merge":true}]}"#).unwrap();
        assert!(snap.ecosystem.is_empty());
        let state = crate::AppState::default();
        state.registry.replace_all(snap.projects);
        let d = state.registry.descriptor("legacy");
        assert!(d.observed_at.is_none());
        assert!(d.declaration.memory.enabled);
        let m = Manifest::parse("[memory]\nsharing='project'").unwrap();
        state.registry.observe("legacy", &m, "abc", 1, 1);
        state
            .registry
            .observe("legacy", &Manifest::parse("").unwrap(), "def", 2, 2);
        assert_eq!(
            state
                .registry
                .descriptor("legacy")
                .declaration
                .memory
                .sharing
                .as_deref(),
            Some("project")
        );
        assert!(state.registry.snapshot()[0].needs_merge);
    }
    #[tokio::test]
    async fn reconcile_replay_hydration_failure_recovery_and_removal() {
        let repo = crate::merge::tests::mk_repo("registry-reconcile");
        let name = repo.file_name().unwrap().to_str().unwrap().to_string();
        let state = crate::AppState {
            sessions: crate::sessions::SessionState::with_repos_dir(repo.parent().unwrap().into()),
            ..Default::default()
        };
        register(&state, &name);
        let manifest="[project]\ndisplay_name='Latest'\n[app]\nname='sample'\n[service]\nname='sample'\npurpose='Published'\norigin='https://sample.stack.test'\ngate='edge-open'\nstatus='planned'\n";
        crate::merge::tests::commit_on(&repo, "master", "sigiled.toml", manifest, "descriptor");
        refresh(&state, &name).await.unwrap();
        refresh(&state, &name).await.unwrap();
        assert_eq!(state.registry.descriptors().len(), 1);
        let valid = state.registry.descriptor(&name);
        assert!(valid.declaration.service.is_some());
        let encoded = serde_json::to_string(&state.registry.descriptors()).unwrap();
        state
            .registry
            .hydrate_descriptors(serde_json::from_str(&encoded).unwrap());
        refresh(&state, &name).await.unwrap();
        assert_eq!(state.registry.descriptors().len(), 1);
        let hidden = repo.with_extension("offline");
        std::fs::rename(&repo, &hidden).unwrap();
        assert_eq!(
            refresh(&state, &name).await.unwrap_err(),
            RefreshError::RepositoryUnavailable
        );
        let failed = state.registry.descriptor(&name);
        assert_eq!(failed.observed_revision, valid.observed_revision);
        assert!(failed.error.is_some());
        assert!(failed.retry_at > failed.attempted_at);
        std::fs::rename(&hidden, &repo).unwrap();
        refresh(&state, &name).await.unwrap();
        assert!(state.registry.descriptor(&name).error.is_none());
        crate::merge::tests::commit_on(&repo, "master", "sigiled.toml", "[broken", "invalid");
        assert_eq!(
            refresh(&state, &name).await.unwrap_err(),
            RefreshError::ManifestInvalid
        );
        let bad = state.registry.descriptor(&name);
        assert!(bad.declaration.service.is_some());
        assert_ne!(bad.desired_revision, bad.observed_revision);
        crate::merge::tests::commit_on(&repo, "master", "sigiled.toml", "", "remove declaration");
        refresh(&state, &name).await.unwrap();
        let d = state.registry.descriptor(&name);
        assert!(d.declaration.service.is_none());
        assert!(d.error.is_none());
        assert_eq!(state.registry.snapshot().len(), 1);
    }
    #[tokio::test]
    async fn repair_does_not_wait_on_busy_mirror_or_create_workspaces() {
        let state = crate::AppState::default();
        register(&state, "sample");
        let guard = state.sessions.merge_lock("sample").lock_owned().await;
        repair_batch(&state, &mut String::new()).await;
        assert!(state.registry.descriptor("sample").attempted_at.is_none());
        drop(guard);
        repair_batch(&state, &mut String::new()).await;
        assert_eq!(
            state.registry.descriptor("sample").error,
            Some(RefreshError::RepositoryUnavailable)
        );
        assert!(state.sessions.live_records().is_empty());
        assert!(state.events.dump().is_empty());
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_refresh_retains_mirror_and_worker_permit_until_reader_finishes() {
        let state = crate::AppState::default();
        state.registry.insert(crate::project::ProjectRecord::new(
            "sample",
            &Manifest::parse("").unwrap(),
            None,
        ));
        let mirror = state.sessions.merge_lock("sample");
        let guard = mirror.clone().lock_owned().await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker_state = state.clone();
        let task = tokio::spawn(async move {
            refresh_using(worker_state, "sample".into(), guard, move |_, _| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok((
                    Manifest::parse("").unwrap(),
                    "abcdef0123456789abcdef0123456789abcdef0123".into(),
                ))
            })
            .await
        });
        started_rx.await.unwrap();
        task.abort();
        let _ = task.await;
        assert!(
            mirror.try_lock().is_err(),
            "cancelled caller released the worker's mirror lock"
        );
        release_tx.send(()).unwrap();
        let _done = tokio::time::timeout(std::time::Duration::from_secs(2), mirror.lock())
            .await
            .unwrap();
        assert!(state
            .registry
            .descriptor("sample")
            .observed_revision
            .is_some());
    }
}
