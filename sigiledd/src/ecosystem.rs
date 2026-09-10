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
    DeadlineExceeded,
    RefreshBusy,
    RepositoryLocked,
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
            Self::DeadlineExceeded => "Repository refresh deadline exceeded; subprocesses stopped and retry scheduled",
            Self::RefreshBusy => "Repository refresh capacity or mirror is busy; retry scheduled",
            Self::RepositoryLocked => "Repository has an incumbent Git lock or lock cleanup failed; last valid descriptor retained",
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
    deadline: std::time::Instant,
) -> Result<(Manifest, String), (RefreshError, Option<String>)> {
    let bad_repo = || (RefreshError::RepositoryUnavailable, None);
    let repo = match &state.sessions.runtime {
        Some(rt) => rt
            .ensure_mirror_until(project, deadline)
            .map_err(|e| (process_error(e), None))?,
        None => state
            .sessions
            .repos_dir
            .as_ref()
            .map(|p| p.join(project))
            .ok_or_else(bad_repo)?,
    };
    let revision = crate::bounded_process::git(&repo, &["rev-parse", "master^{commit}"], deadline)
        .map_err(|e| (process_error(e), None))?
        .trim()
        .to_string();
    // List the tree first: a failed read is never mistaken for deliberate removal.
    let files = crate::bounded_process::git(&repo, &["ls-tree", "--name-only", "master"], deadline)
        .map_err(|e| (process_error(e), Some(revision.clone())))?;
    for f in ["sigiled.toml", "mgr.toml"] {
        if files.lines().any(|p| p == f) {
            let text =
                crate::bounded_process::git(&repo, &["show", &format!("master:{f}")], deadline)
                    .map_err(|e| (process_error(e), Some(revision.clone())))?;
            return Manifest::parse(&text)
                .map(|m| (m, revision.clone()))
                .map_err(|_| (RefreshError::ManifestInvalid, Some(revision)));
        }
    }
    Ok((Manifest::parse("").expect("empty manifest"), revision))
}

fn process_error(error: crate::bounded_process::Error) -> RefreshError {
    match error {
        crate::bounded_process::Error::Deadline => RefreshError::DeadlineExceeded,
        crate::bounded_process::Error::LockBusy => RefreshError::RepositoryLocked,
        _ => RefreshError::RepositoryUnavailable,
    }
}
#[derive(Clone, Copy)]
struct Limits {
    wait: std::time::Duration,
    work: std::time::Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            wait: std::time::Duration::from_secs(2),
            work: std::time::Duration::from_secs(30),
        }
    }
}
static WORKERS: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)));
fn failed_attempt(state: &crate::AppState, project: &str, error: RefreshError) -> RefreshError {
    state
        .registry
        .failed(project, None, error, crate::auth::now_epoch());
    if state.try_persist().is_err() {
        state.registry.failed(
            project,
            None,
            RefreshError::PersistFailed,
            crate::auth::now_epoch(),
        );
        RefreshError::PersistFailed
    } else {
        error
    }
}
/// The blocking task OWNS the guard and permit. Its reader uses one absolute
/// native-process deadline; cancellation cannot release either while Git runs.
async fn refresh_locked(
    state: crate::AppState,
    project: String,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<Manifest, RefreshError> {
    refresh_using(
        state,
        project,
        guard,
        read_manifest,
        Limits::default(),
        WORKERS.clone(),
    )
    .await
}
type Observation = Result<(Manifest, String), (RefreshError, Option<String>)>;
async fn refresh_using<F>(
    state: crate::AppState,
    project: String,
    guard: tokio::sync::OwnedMutexGuard<()>,
    reader: F,
    limits: Limits,
    workers: std::sync::Arc<tokio::sync::Semaphore>,
) -> Result<Manifest, RefreshError>
where
    F: FnOnce(&crate::AppState, &str, std::time::Instant) -> Observation + Send + 'static,
{
    let permit = match tokio::time::timeout(limits.wait, workers.acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        _ => return Err(failed_attempt(&state, &project, RefreshError::RefreshBusy)),
    };
    let deadline = std::time::Instant::now() + limits.work;
    tokio::task::spawn_blocking(move || {
        let _worker = permit;
        let _mirror = guard;
        let now = crate::auth::now_epoch();
        let mut result = reader(&state, &project, deadline);
        if result.is_ok() && std::time::Instant::now() >= deadline {
            result = Err((RefreshError::DeadlineExceeded, None));
        }
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
    let mut limits = Limits::default();
    let started = std::time::Instant::now();
    let guard =
        match tokio::time::timeout(limits.wait, state.sessions.merge_lock(project).lock_owned())
            .await
        {
            Ok(guard) => guard,
            Err(_) => return Err(failed_attempt(state, project, RefreshError::RefreshBusy)),
        };
    // Mirror + worker-capacity waits share one budget; the scheduler can move on.
    limits.wait = limits.wait.saturating_sub(started.elapsed());
    refresh_using(
        state.clone(),
        project.into(),
        guard,
        read_manifest,
        limits,
        WORKERS.clone(),
    )
    .await
}
/// One bounded round-robin batch. Busy mirrors are skipped; acquired work is
/// bounded by capacity wait plus the native deadline and mandatory child cleanup.
pub async fn repair_batch(state: &crate::AppState, after: &mut String) {
    let mut projects = state.registry.snapshot();
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    let start = projects.partition_point(|p| p.name <= *after);
    let len = projects.len();
    for i in 0..len.min(16) {
        let name = &projects[(start + i) % len].name;
        *after = name.clone();
        if state
            .registry
            .descriptor(name)
            .retry_at
            .is_some_and(|t| t > crate::auth::now_epoch())
        {
            continue;
        }
        if let Ok(guard) = state.sessions.merge_lock(name).try_lock_owned() {
            // No abandoned waiter: all native processes are stopped/reaped before
            // this returns, including timeout results. Cancellation is still safe.
            let _ = refresh_locked(state.clone(), name.clone(), guard).await;
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
        let state = crate::AppState::test_without_runtime();
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
            ..crate::AppState::test_without_runtime()
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
        let state = crate::AppState::test_without_runtime();
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
        let state = crate::AppState::test_without_runtime();
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
            refresh_using(
                worker_state,
                "sample".into(),
                guard,
                move |_, _, _| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok((
                        Manifest::parse("").unwrap(),
                        "abcdef0123456789abcdef0123456789abcdef0123".into(),
                    ))
                },
                Limits::default(),
                std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
            )
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
#[cfg(all(test, target_os = "linux"))]
mod deadline_tests {
    use super::*;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    #[tokio::test]
    async fn stalled_reader_deadline_reclaims_resources_even_after_caller_cancellation() {
        for cancelled in [false, true] {
            let repo = crate::merge::tests::mk_repo(if cancelled {
                "bounded-cancelled"
            } else {
                "bounded-completed"
            });
            let healthy = repo.file_name().unwrap().to_str().unwrap().to_string();
            let state = crate::AppState {
                sessions: crate::sessions::SessionState::with_repos_dir(
                    repo.parent().unwrap().into(),
                ),
                ..crate::AppState::test_without_runtime()
            };
            for name in ["stalled", healthy.as_str()] {
                state.registry.insert(crate::project::ProjectRecord::new(
                    name,
                    &Manifest::parse("").unwrap(),
                    None,
                ));
            }
            let workers = Arc::new(tokio::sync::Semaphore::new(1));
            let mirror = state.sessions.merge_lock("stalled");
            let guard = mirror.clone().lock_owned().await;
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let work_state = state.clone();
            let permits = workers.clone();
            let started = Instant::now();
            let task = tokio::spawn(async move {
                refresh_using(
                    work_state,
                    "stalled".into(),
                    guard,
                    move |_, _, deadline| {
                        started_tx.send(()).unwrap();
                        crate::bounded_process::output_until(
                            std::process::Command::new("sh").args(["-c", "sleep 5 & wait"]),
                            deadline,
                        )
                        .map_err(|e| (process_error(e), None))?;
                        Ok((Manifest::parse("").unwrap(), "should-never-publish".into()))
                    },
                    Limits {
                        wait: Duration::from_millis(100),
                        work: Duration::from_millis(100),
                    },
                    permits,
                )
                .await
            });
            started_rx.await.unwrap();
            if cancelled {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
                assert!(
                    mirror.try_lock().is_err(),
                    "cancellation released running reader's lock"
                );
            } else {
                assert_eq!(
                    task.await.unwrap().unwrap_err(),
                    RefreshError::DeadlineExceeded
                );
            }
            let reclaimed = tokio::time::timeout(Duration::from_secs(1), mirror.lock_owned())
                .await
                .unwrap();
            drop(reclaimed);
            let capacity =
                tokio::time::timeout(Duration::from_secs(1), workers.clone().acquire_owned())
                    .await
                    .unwrap()
                    .unwrap();
            drop(capacity);
            assert!(started.elapsed() < Duration::from_secs(1));
            assert_eq!(
                state.registry.descriptor("stalled").error,
                Some(RefreshError::DeadlineExceeded)
            );
            let healthy_guard = state.sessions.merge_lock(&healthy).lock_owned().await;
            refresh_using(
                state.clone(),
                healthy.clone(),
                healthy_guard,
                read_manifest,
                Limits {
                    wait: Duration::from_millis(100),
                    work: Duration::from_secs(1),
                },
                workers,
            )
            .await
            .unwrap();
            assert!(state
                .registry
                .descriptor(&healthy)
                .observed_revision
                .is_some());
        }
    }
    #[tokio::test]
    async fn capacity_wait_is_bounded_and_does_not_start_reader_or_keep_mirror() {
        let state = crate::AppState::test_without_runtime();
        state.registry.insert(crate::project::ProjectRecord::new(
            "sample",
            &Manifest::parse("").unwrap(),
            None,
        ));
        let workers = Arc::new(tokio::sync::Semaphore::new(1));
        let occupied = workers.clone().acquire_owned().await.unwrap();
        let mirror = state.sessions.merge_lock("sample");
        let guard = mirror.clone().lock_owned().await;
        let started = Instant::now();
        let result = refresh_using(
            state.clone(),
            "sample".into(),
            guard,
            |_, _, _| panic!("reader started without capacity"),
            Limits {
                wait: Duration::from_millis(40),
                work: Duration::from_secs(1),
            },
            workers.clone(),
        )
        .await;
        assert_eq!(result.unwrap_err(), RefreshError::RefreshBusy);
        assert!(started.elapsed() < Duration::from_millis(300));
        assert!(mirror.try_lock().is_ok());
        drop(occupied);
        assert_eq!(workers.available_permits(), 1);
        assert_eq!(
            state.registry.descriptor("sample").error,
            Some(RefreshError::RefreshBusy)
        );
    }
}
