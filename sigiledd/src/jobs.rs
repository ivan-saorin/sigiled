// jobs.rs — the cron batch engine (session 8, contract §7). A job run is a
// disposable workspace like a session, with three differences: it is born
// from master on an append-only branch job-<name>-<stamp> (never merged,
// rule 7); its container is vm-job-{project}-{job}, so it can NEVER collide
// with a live session (v2 has no project lock by design); and its life is
// one command — exec under the manifest's wall clock, leftovers committed,
// container destroyed. Definitions live in sigiled.toml ON MASTER (read via
// `git show master:` on the mirror: literal, checkout-independent) and are
// refreshed within ~5 min; the scheduler evaluates the classic 5-field cron
// in the control plane's local TZ.
use crate::auth::{authorize, Action, Actor};
use crate::manifest::{JobManifest, Manifest};
use axum::extract::{Path as AxPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRunRecord {
    pub project: String,
    pub job: String,
    pub branch: String,
    /// running · succeeded · failed · timeout · error · skipped_locked · aborted
    pub state: String,
    pub started_epoch: u64,
    #[serde(default)]
    pub finished_epoch: Option<u64>,
    #[serde(default)]
    pub exit: Option<i64>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Default, Clone)]
pub struct JobsState {
    /// "{project}/{job}" → run records, newest first, capped at 20.
    runs: Arc<RwLock<HashMap<String, Vec<JobRunRecord>>>>,
    /// One run per job at a time (contract: 409 same job in flight).
    inflight: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl JobsState {
    fn key(project: &str, job: &str) -> String {
        format!("{project}/{job}")
    }
    pub fn runs_for(&self, project: &str, job: &str) -> Vec<JobRunRecord> {
        self.runs
            .read()
            .unwrap()
            .get(&Self::key(project, job))
            .cloned()
            .unwrap_or_default()
    }
    pub fn push_run(&self, rec: JobRunRecord) {
        let mut map = self.runs.write().unwrap();
        let list = map.entry(Self::key(&rec.project, &rec.job)).or_default();
        list.insert(0, rec);
        list.truncate(20);
    }
    /// Update the newest record of (project, job) matching `branch`.
    fn update_run(&self, project: &str, job: &str, branch: &str, f: impl FnOnce(&mut JobRunRecord)) {
        let mut map = self.runs.write().unwrap();
        if let Some(list) = map.get_mut(&Self::key(project, job)) {
            if let Some(rec) = list.iter_mut().find(|r| r.branch == branch) {
                f(rec);
            }
        }
    }
    pub fn is_inflight(&self, project: &str, job: &str) -> bool {
        self.inflight.read().unwrap().contains(&Self::key(project, job))
    }
    pub fn try_claim(&self, project: &str, job: &str) -> bool {
        self.inflight.write().unwrap().insert(Self::key(project, job))
    }
    fn release(&self, project: &str, job: &str) {
        self.inflight.write().unwrap().remove(&Self::key(project, job));
    }
    pub fn dump(&self) -> HashMap<String, Vec<JobRunRecord>> {
        self.runs.read().unwrap().clone()
    }
    /// Boot-time hydrate. A record still `running` in the snapshot means the
    /// control plane died mid-run: the container is gone or orphaned, the
    /// record becomes `aborted` — honestly, not silently.
    pub fn hydrate(&self, mut map: HashMap<String, Vec<JobRunRecord>>) {
        for list in map.values_mut() {
            for r in list.iter_mut() {
                if r.state == "running" {
                    r.state = "aborted".into();
                    r.detail = Some("control plane restarted mid-run".into());
                }
            }
        }
        *self.runs.write().unwrap() = map;
    }
}

fn now_epoch() -> u64 {
    crate::auth::now_epoch()
}

fn err(status: StatusCode, detail: impl Into<String>) -> Response {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

/// job-<name>-<YYYYMMDD-HHMMSS> — the stamp is local time, same clock the
/// cron is evaluated in.
pub fn job_branch(job: &str, at: &chrono::DateTime<chrono::Local>) -> String {
    format!("job-{job}-{}", at.format("%Y%m%d-%H%M%S"))
}

/// True when the classic 5-field expression has a fire time in (from, to].
pub fn fires_between(
    cron5: &str,
    from: &chrono::DateTime<chrono::Local>,
    to: &chrono::DateTime<chrono::Local>,
) -> Result<bool, String> {
    use std::str::FromStr;
    let sched = cron::Schedule::from_str(&format!("0 {cron5}"))
        .map_err(|e| format!("bad cron {cron5:?}: {e}"))?;
    Ok(sched.after(from).next().map(|t| t <= *to).unwrap_or(false))
}

/// The project's jobs, read from sigiled.toml (mgr.toml fallback) ON MASTER.
/// Errors map straight to verb responses: Ok(vec![]) = no jobs declared,
/// Err(Some(422 detail)) = broken manifest, Err(None) = unknown project.
fn jobs_of(state: &crate::AppState, project: &str) -> Result<Vec<JobManifest>, Option<String>> {
    let repo = match &state.sessions.runtime {
        Some(rt) => rt.ensure_mirror(project).map_err(|_| None)?,
        None => {
            let repos = state.sessions.repos_dir.clone().ok_or(None)?;
            let path = repos.join(project);
            if !path.join(".git").exists() {
                return Err(None);
            }
            path
        }
    };
    for f in ["sigiled.toml", "mgr.toml"] {
        if let Ok(text) = crate::merge::git(&repo, &["show", &format!("master:{f}")]) {
            return Manifest::parse(&text)
                .map(|m| m.jobs)
                .map_err(|e| Some(format!("{project}/{f}: {e}")));
        }
    }
    Ok(vec![])
}

/// POST /sigiled/projects/{p}/jobs/{j}/run — manual trigger, same machinery
/// the scheduler uses. 202 run record · 404 unknown · 409 in flight · 422
/// broken manifest · 503 no runtime.
pub async fn run(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath((project, job)): AxPath<(String, String)>,
) -> Response {
    if let Err(denial) = authorize(
        &actor,
        Action::JobRun,
        Some(&project),
        &state.auth.approvals,
        now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, denial.0);
    }
    let jobs = match jobs_of(&state, &project) {
        Ok(j) => j,
        Err(Some(detail)) => return err(StatusCode::UNPROCESSABLE_ENTITY, detail),
        Err(None) => return err(StatusCode::NOT_FOUND, format!("unknown project: {project}")),
    };
    let Some(jm) = jobs.into_iter().find(|j| j.name == job) else {
        return err(StatusCode::NOT_FOUND, format!("unknown job: {job}"));
    };
    if state.jobs.is_inflight(&project, &job) {
        return err(StatusCode::CONFLICT, format!("job '{job}' already in flight — poll runs"));
    }
    if state.sessions.runtime.is_none() {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "runtime not configured — jobs need real containers",
        );
    }
    match trigger(&state, &project, &jm, &chrono::Local::now()) {
        Ok(rec) => (StatusCode::ACCEPTED, Json(rec)).into_response(),
        Err(e) => err(StatusCode::CONFLICT, e),
    }
}

/// The endless loop main() spawns next to the reaper. Each tick re-reads
/// every project's manifest (the ~5 min refresh of contract §7 — a fetch
/// per project per tick, so the default stays coarse) and fires every job
/// with a cron hit in the (last, now] window: a slow tick loses nothing.
pub async fn scheduler(state: crate::AppState) {
    let poll = std::env::var("SIGILED_JOBS_POLL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300u64);
    tracing::info!(poll, "jobs scheduler running");
    let mut last = chrono::Local::now();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(poll)).await;
        let now = chrono::Local::now();
        scheduler_tick(&state, &last, &now).await;
        last = now;
    }
}

async fn scheduler_tick(
    state: &crate::AppState,
    last: &chrono::DateTime<chrono::Local>,
    now: &chrono::DateTime<chrono::Local>,
) {
    for p in state.registry.snapshot() {
        let jobs = match jobs_of(state, &p.name) {
            Ok(j) => j,
            Err(Some(e)) => {
                tracing::warn!(project = %p.name, %e, "broken [jobs] — project's jobs disabled");
                continue;
            }
            Err(None) => continue,
        };
        for jm in jobs {
            match fires_between(&jm.cron, last, now) {
                Ok(true) => {
                    if let Err(e) = trigger(state, &p.name, &jm, now) {
                        tracing::warn!(project = %p.name, job = %jm.name, %e, "job fire skipped");
                    }
                }
                Ok(false) => {}
                Err(e) => tracing::warn!(project = %p.name, job = %jm.name, %e, "bad cron"),
            }
        }
    }
}

/// Claim the latch and launch one run in the background. Shared by the
/// scheduler and the manual verb. A held latch at fire time is a recorded
/// `skipped_locked` run, not silence.
fn trigger(
    state: &crate::AppState,
    project: &str,
    jm: &JobManifest,
    at: &chrono::DateTime<chrono::Local>,
) -> Result<JobRunRecord, String> {
    let branch = job_branch(&jm.name, at);
    if !state.jobs.try_claim(project, &jm.name) {
        let rec = JobRunRecord {
            project: project.to_string(),
            job: jm.name.clone(),
            branch: branch.clone(),
            state: "skipped_locked".into(),
            started_epoch: now_epoch(),
            finished_epoch: Some(now_epoch()),
            exit: None,
            detail: Some("previous run still in flight".into()),
        };
        state.jobs.push_run(rec);
        state.events.record(
            project,
            now_epoch(),
            crate::events::Event::JobRun {
                job: jm.name.clone(),
                branch,
                state: "skipped_locked".into(),
            },
        );
        state.persist();
        return Err(format!("job '{}' already in flight — skip recorded", jm.name));
    }
    let rec = JobRunRecord {
        project: project.to_string(),
        job: jm.name.clone(),
        branch: branch.clone(),
        state: "running".into(),
        started_epoch: now_epoch(),
        finished_epoch: None,
        exit: None,
        detail: None,
    };
    state.jobs.push_run(rec.clone());
    state.persist();
    tokio::spawn(execute_run(state.clone(), project.to_string(), jm.clone(), branch));
    Ok(rec)
}

/// One run, cradle to grave: secrets from the stack env, fresh container
/// from master on the append-only branch, the command under its wall clock,
/// leftovers committed (the branch is the run's testimony), container gone,
/// record + machine log updated, hc pinged, latch released.
async fn execute_run(state: crate::AppState, project: String, jm: JobManifest, branch: String) {
    let Some(rt) = state.sessions.runtime.clone() else { return };
    let container = crate::runtime::Runtime::job_container(&project, &jm.name);
    let http = state.sessions.http().clone();
    let mut exit: Option<i64> = None;
    let outcome: Result<String, String> = async {
        let mut extra: Vec<(String, String)> = Vec::new();
        for (env_name, stack_var) in &jm.secrets {
            let v = std::env::var(stack_var)
                .map_err(|_| format!("secret {env_name}: stack env {stack_var} is not set"))?;
            extra.push((env_name.clone(), v));
        }
        rt.ensure_mirror(&project)?;
        let tok = crate::sessions::mint_token();
        rt.create_container(&container, &project, "job", &jm.name, &tok, &extra)?;
        rt.wait_healthy(&http, &container, &tok).await?;
        rt.boot_workspace(&http, &container, &project, &tok, &branch, false).await?;
        let r = rt.exec(&http, &container, &tok, &jm.command, jm.timeout_minutes * 60).await?;
        exit = r["exit"].as_i64();
        let timed_out = r["timed_out"].as_bool().unwrap_or(false);
        rt.flush(&http, &container, &tok, &format!("job {}", jm.name)).await;
        Ok(if timed_out {
            "timeout".to_string()
        } else if exit == Some(0) {
            "succeeded".to_string()
        } else {
            "failed".to_string()
        })
    }
    .await;
    rt.destroy(&container);
    let (final_state, detail) = match outcome {
        Ok(s) => (s, None),
        Err(e) => ("error".to_string(), Some(e)),
    };
    // hc_ping is a STACK-ENV REF (rule 8): resolved here, success pings the
    // URL, anything else pings {url}/fail — healthchecks.io convention.
    if let Some(env_ref) = &jm.hc_ping {
        match std::env::var(env_ref) {
            Ok(url) => {
                let ping =
                    if final_state == "succeeded" { url } else { format!("{url}/fail") };
                let _ = http
                    .get(&ping)
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await;
            }
            Err(_) => {
                tracing::warn!(job = %jm.name, env_ref, "hc_ping stack env not set — ping skipped")
            }
        }
    }
    state.jobs.update_run(&project, &jm.name, &branch, |r| {
        r.state = final_state.clone();
        r.finished_epoch = Some(now_epoch());
        r.exit = exit;
        r.detail = detail.clone();
    });
    state.jobs.release(&project, &jm.name);
    state.events.record(
        &project,
        now_epoch(),
        crate::events::Event::JobRun {
            job: jm.name.clone(),
            branch: branch.clone(),
            state: final_state.clone(),
        },
    );
    state.persist();
    tracing::info!(%project, job = %jm.name, %branch, state = %final_state, "job run finished");
}

/// GET /sigiled/projects/{p}/jobs/{j}/runs — last 20, newest first.
pub async fn runs(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath((project, job)): AxPath<(String, String)>,
) -> Response {
    if let Err(denial) = authorize(
        &actor,
        Action::JobRecap,
        Some(&project),
        &state.auth.approvals,
        now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, denial.0);
    }
    if !state.registry.contains(&project) {
        return err(StatusCode::NOT_FOUND, format!("unknown project: {project}"));
    }
    Json(state.jobs.runs_for(&project, &job)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("unambiguous local time")
    }

    fn rec(project: &str, job: &str, branch: &str, state: &str, started: u64) -> JobRunRecord {
        JobRunRecord {
            project: project.into(),
            job: job.into(),
            branch: branch.into(),
            state: state.into(),
            started_epoch: started,
            finished_epoch: None,
            exit: None,
            detail: None,
        }
    }

    #[test]
    fn job_branch_carries_the_local_stamp() {
        let at = local(2026, 8, 3, 14, 15, 9);
        assert_eq!(job_branch("mine", &at), "job-mine-20260803-141509");
    }

    #[test]
    fn fires_between_matches_the_window() {
        // */5: fires at :05 — inside (04:30, 05:30], outside (06:00, 09:00].
        let c = "*/5 * * * *";
        assert!(fires_between(c, &local(2026, 8, 3, 10, 4, 30), &local(2026, 8, 3, 10, 5, 30)).unwrap());
        assert!(!fires_between(c, &local(2026, 8, 3, 10, 6, 0), &local(2026, 8, 3, 10, 9, 0)).unwrap());
        // A daily 03:30 job fires exactly once in its minute.
        let daily = "30 3 * * *";
        assert!(fires_between(daily, &local(2026, 8, 3, 3, 29, 59), &local(2026, 8, 3, 3, 30, 0)).unwrap());
        assert!(!fires_between(daily, &local(2026, 8, 3, 3, 30, 0), &local(2026, 8, 3, 3, 31, 0)).unwrap());
        // Garbage is an error, not a silent never.
        assert!(fires_between("nope", &local(2026, 8, 3, 0, 0, 0), &local(2026, 8, 3, 1, 0, 0)).is_err());
    }

    #[test]
    fn runs_are_newest_first_and_capped_at_20() {
        let js = JobsState::default();
        for i in 0..25 {
            js.push_run(rec("p", "j", &format!("job-j-{i}"), "succeeded", i));
        }
        let list = js.runs_for("p", "j");
        assert_eq!(list.len(), 20);
        assert_eq!(list[0].branch, "job-j-24", "newest first");
        assert_eq!(list[19].branch, "job-j-5");
    }

    #[test]
    fn hydrate_marks_running_as_aborted() {
        let js = JobsState::default();
        let mut map = HashMap::new();
        map.insert(
            "p/j".to_string(),
            vec![rec("p", "j", "job-j-x", "running", 1), rec("p", "j", "job-j-y", "succeeded", 0)],
        );
        js.hydrate(map);
        let list = js.runs_for("p", "j");
        assert_eq!(list[0].state, "aborted", "a running record cannot survive a reboot");
        assert_eq!(list[1].state, "succeeded");
    }

    // --- the run verb, branch-only paths ------------------------------------

    fn admin() -> Actor {
        Actor { driver: "bootstrap".into(), role: crate::auth::Role::Admin, approval: None }
    }

    async fn body_json(resp: Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// AppState over a temp repos dir with one repo carrying `manifest` as
    /// sigiled.toml on master (committed — jobs are read from master).
    fn app_state_with_manifest(project: &str, tag: &str, manifest: &str) -> crate::AppState {
        let repo = crate::merge::tests::mk_repo(tag);
        let repos_dir = repo.parent().unwrap().to_path_buf();
        let renamed = repos_dir.join(project);
        let _ = std::fs::remove_dir_all(&renamed);
        std::fs::rename(&repo, &renamed).unwrap();
        if !manifest.is_empty() {
            crate::merge::tests::commit_on(&renamed, "master", "sigiled.toml", manifest, "manifest");
        }
        let state = crate::AppState {
            sessions: crate::sessions::SessionState::with_repos_dir(repos_dir),
            ..crate::AppState::default()
        };
        state.registry.insert(crate::project::ProjectRecord {
            name: project.into(),
            template_version: None,
            template_behind: false,
            needs_merge: false,
        });
        state
    }

    #[tokio::test]
    async fn run_unknown_job_is_404() {
        let state = app_state_with_manifest("jsmoke-a", "jsa", "");
        let (status, _) = body_json(
            run(admin(), State(state), AxPath(("jsmoke-a".into(), "ghost".into()))).await,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn run_on_broken_manifest_is_422() {
        let state = app_state_with_manifest(
            "jsmoke-b",
            "jsb",
            "[jobs.x]\ncron = \"not a cron\"\ncommand = \"c\"\n",
        );
        let (status, body) = body_json(
            run(admin(), State(state), AxPath(("jsmoke-b".into(), "x".into()))).await,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    }

    #[tokio::test]
    async fn run_inflight_is_409_and_without_runtime_503() {
        let m = "[jobs.x]\ncron = \"30 3 * * *\"\ncommand = \"true\"\n";
        let state = app_state_with_manifest("jsmoke-c", "jsc", m);
        // In flight → 409 before anything else touches docker.
        assert!(state.jobs.try_claim("jsmoke-c", "x"));
        let (status, _) = body_json(
            run(admin(), State(state.clone()), AxPath(("jsmoke-c".into(), "x".into()))).await,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        // Free but no runtime → 503 honest (jobs need real containers).
        state.jobs.release("jsmoke-c", "x");
        let (status, _) = body_json(
            run(admin(), State(state), AxPath(("jsmoke-c".into(), "x".into()))).await,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn runs_listing_answers_from_the_store() {
        let state = app_state_with_manifest("jsmoke-d", "jsd", "");
        state.jobs.push_run(rec("jsmoke-d", "x", "job-x-1", "succeeded", 5));
        let (status, body) = body_json(
            runs(admin(), State(state), AxPath(("jsmoke-d".into(), "x".into()))).await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[0]["branch"], "job-x-1");
    }
}
