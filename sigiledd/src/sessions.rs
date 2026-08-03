// sessions.rs — the session verbs of design §4: open without locks (no more
// 409 — N concurrent sessions, N branches), close under a per-project merge
// lock of seconds (ff → three-way → debt, merge.rs). Container runtime is
// cutover territory: pre-cutover the verbs manage branches and debt on the
// repos under SIGILED_REPOS_DIR, which is exactly what the runtime will
// wrap. token/endpoint are null until then, honestly.
//
// This is also where the session-3 policy finally gets consumed: open takes
// authorize(OpenSession, project) — a driver needs a live approval for the
// platform projects (DEC-15) — and every record carries its actor (§1.6).
use crate::auth::{authorize, Action, Actor};
use crate::events::{log_operativo_touched, Event};
use crate::merge::{changed_paths, close_merge, git, MergeDebt, MergeOutcome};
use axum::extract::{Path as AxPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub project: String,
    pub branch: String,
    pub head: String,
    pub stale: bool,
    pub actor: Actor,
}

#[derive(Clone)]
pub struct SessionState {
    /// Root of the project repos this control plane arbitrates. None = the
    /// verbs answer 503 (dev run without a repo store).
    pub repos_dir: Option<PathBuf>,
    records: Arc<RwLock<HashMap<String, SessionRecord>>>,
    debts: Arc<RwLock<HashMap<String, Vec<MergeDebt>>>>,
    merge_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl Default for SessionState {
    fn default() -> Self {
        SessionState {
            repos_dir: std::env::var("SIGILED_REPOS_DIR").ok().map(PathBuf::from),
            records: Arc::default(),
            debts: Arc::default(),
            merge_locks: Arc::default(),
        }
    }
}

impl SessionState {
    pub fn with_repos_dir(dir: PathBuf) -> Self {
        SessionState { repos_dir: Some(dir), ..SessionState::default() }
    }
    pub fn debts_for(&self, project: &str) -> Vec<MergeDebt> {
        self.debts.read().unwrap().get(project).cloned().unwrap_or_default()
    }
    fn push_debt(&self, project: &str, debt: MergeDebt) {
        let mut map = self.debts.write().unwrap();
        let queue = map.entry(project.to_string()).or_default();
        queue.retain(|d| d.branch != debt.branch);
        queue.push(debt);
    }
    fn clear_debt(&self, project: &str, branch: &str) {
        if let Some(q) = self.debts.write().unwrap().get_mut(project) {
            q.retain(|d| d.branch != branch);
        }
    }
    pub fn dump_debts(&self) -> HashMap<String, Vec<MergeDebt>> {
        self.debts.read().unwrap().clone()
    }
    pub fn dump_records(&self) -> HashMap<String, SessionRecord> {
        self.records.read().unwrap().clone()
    }
    pub fn hydrate(
        &self,
        debts: HashMap<String, Vec<MergeDebt>>,
        records: HashMap<String, SessionRecord>,
    ) {
        *self.debts.write().unwrap() = debts;
        *self.records.write().unwrap() = records;
    }
    fn merge_lock(&self, project: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.merge_locks
            .lock()
            .unwrap()
            .entry(project.to_string())
            .or_default()
            .clone()
    }
    fn session_id() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static N: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
        let n = N.fetch_add(1, Ordering::SeqCst);
        format!("{:08x}", (nanos ^ (n << 48)) & 0xffff_ffff)
    }
}

fn err(status: StatusCode, detail: impl Into<String>) -> Response {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

fn now_epoch() -> u64 {
    crate::auth::now_epoch()
}

pub async fn open(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(project): AxPath<String>,
) -> Response {
    let Some(repos) = state.sessions.repos_dir.clone() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "SIGILED_REPOS_DIR not configured");
    };
    let repo = repos.join(&project);
    if !repo.join(".git").exists() {
        return err(StatusCode::NOT_FOUND, format!("unknown project: {project}"));
    }
    // The session-3 policy, finally consumed by a verb (DEC-15).
    if let Err(denial) = authorize(
        &actor,
        Action::OpenSession,
        Some(&project),
        &state.auth.approvals,
        now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, denial.0);
    }

    let id = SessionState::session_id();
    let branch = format!("session/{id}");
    if let Err(e) = git(&repo, &["branch", &branch, "master"]) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    let head = git(&repo, &["rev-parse", "master"]).unwrap_or_default();

    let record = SessionRecord {
        session_id: id.clone(),
        project: project.clone(),
        branch: branch.clone(),
        head: head.clone(),
        stale: false,
        actor: actor.clone(),
    };
    state.sessions.records.write().unwrap().insert(id.clone(), record);
    state.events.record(
        &project,
        now_epoch(),
        Event::SessionOpened { session_id: id.clone(), branch: branch.clone(), stale: false },
    );

    state.persist();

    // merge_debt on top when present (rule: shout, don't whisper).
    let debts = state.sessions.debts_for(&project);
    (
        StatusCode::CREATED,
        Json(json!({
            "session_id": id, "project": project, "branch": branch,
            "token": null, "endpoint": null,      // container runtime = cutover
            "head": head, "stale": false, "last_commit": null,
            "merge_debt": debts.first(),
            "actor": actor,
        })),
    )
        .into_response()
}

pub async fn close(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(session_id): AxPath<String>,
) -> Response {
    let Some(record) = state.sessions.records.read().unwrap().get(&session_id).cloned() else {
        return err(StatusCode::NOT_FOUND, format!("unknown session: {session_id}"));
    };
    if let Err(denial) = authorize(
        &actor,
        Action::CloseSession,
        Some(&record.project),
        &state.auth.approvals,
        now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, denial.0);
    }
    let repos = state.sessions.repos_dir.clone().expect("open required repos_dir");
    let repo = repos.join(&record.project);

    // The critical section shrank from the whole session to these few
    // lines (§4.1): simultaneous closes serialize here — one wins, the
    // other sees a moved master and takes the merge path.
    let lock = state.sessions.merge_lock(&record.project);
    let _guard = lock.lock().await;

    let touched = changed_paths(&repo, &record.branch)
        .map(|p| log_operativo_touched(&p))
        .unwrap_or(false);

    let outcome = match close_merge(&repo, &record.branch) {
        Ok(o) => o,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let (merge_kind, sha, debt) = match outcome {
        MergeOutcome::Ff { sha } => ("ff", sha, None),
        MergeOutcome::Merged { sha } => ("merged", sha, None),
        MergeOutcome::Debt(d) => {
            let master = d.ours.sha.clone();
            ("debt", master, Some(d))
        }
    };

    match &debt {
        None => {
            // Clean close: the branch merged, its debt (if it was a debtor
            // being resolved) is paid, the record goes away.
            let _ = git(&repo, &["branch", "-D", &record.branch]);
            state.sessions.clear_debt(&record.project, &record.branch);
        }
        Some(d) => {
            // Conflict: master stayed put, the branch survives as the
            // debtor, the queue inherits the package.
            state.sessions.push_debt(&record.project, d.clone());
        }
    }
    state.sessions.records.write().unwrap().remove(&session_id);
    state.events.record(
        &record.project,
        now_epoch(),
        Event::SessionClosed {
            session_id: session_id.clone(),
            merged: debt.is_none(),
            sha: sha.clone(),
            log_operativo_touched: touched,
        },
    );

    state.persist();

    Json(json!({
        "closed": true, "merge": merge_kind, "sha": sha, "flushed": true,
        "log_operativo_touched": touched, "merge_debt": debt,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::tests::{commit_on, mk_repo, sh};
    use axum::body::to_bytes;

    fn admin() -> Actor {
        Actor { driver: "bootstrap".into(), role: crate::auth::Role::Admin, approval: None }
    }
    fn driver() -> Actor {
        Actor { driver: "sigiled-claude".into(), role: crate::auth::Role::Driver, approval: None }
    }

    /// AppState over a temp repos dir containing one repo named `project`.
    fn app_state(project: &str, tag: &str) -> (crate::AppState, std::path::PathBuf) {
        let repo = mk_repo(tag);
        let repos_dir = repo.parent().unwrap().to_path_buf();
        let renamed = repos_dir.join(project);
        let _ = std::fs::remove_dir_all(&renamed);
        std::fs::rename(&repo, &renamed).unwrap();
        let state = crate::AppState {
            sessions: SessionState::with_repos_dir(repos_dir.clone()),
            ..crate::AppState::default()
        };
        (state, renamed)
    }

    async fn body_json(resp: Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn open_close_fast_forward_and_honest_hint() {
        let (state, repo) = app_state("smoke-ff", "sff");
        let resp = open(admin(), State(state.clone()), AxPath("smoke-ff".into())).await;
        let (status, body) = body_json(resp).await;
        assert_eq!(status, StatusCode::CREATED);
        assert!(body["merge_debt"].is_null());
        let id = body["session_id"].as_str().unwrap().to_string();
        let branch = body["branch"].as_str().unwrap().to_string();

        commit_on(&repo, &branch, "docs/log-operativo.md", "# log\nvoce\n", "log: voce");
        let (status, body) =
            body_json(close(admin(), State(state.clone()), AxPath(id)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["merge"], "ff");
        assert_eq!(body["log_operativo_touched"], true);
        assert_eq!(body["sha"].as_str().unwrap(), sh(&repo, &["rev-parse", "master"]));
        // Branch deleted on clean close; machine log recorded both events.
        assert!(sh(&repo, &["branch", "--list", &branch]).is_empty());
        assert_eq!(state.events.for_project("smoke-ff").len(), 2);
    }

    #[tokio::test]
    async fn conflict_close_records_debt_and_next_open_shouts_it() {
        let (state, repo) = app_state("smoke-debt", "sdebt");
        let (_, a) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-debt".into())).await).await;
        let id = a["session_id"].as_str().unwrap().to_string();
        let branch = a["branch"].as_str().unwrap().to_string();

        commit_on(&repo, "master", "hot.txt", "ours\n", "fix: ours");
        commit_on(&repo, &branch, "hot.txt", "theirs\n", "fix: theirs");
        let (_, closed) =
            body_json(close(admin(), State(state.clone()), AxPath(id)).await).await;
        assert_eq!(closed["merge"], "debt");
        assert_eq!(closed["merge_debt"]["conflicted_files"][0], "hot.txt");

        // Master stayed, the debtor branch survives.
        assert!(!sh(&repo, &["branch", "--list", &branch]).is_empty());
        // The next open inherits the debt, on top of the response.
        let (_, b) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-debt".into())).await).await;
        assert_eq!(b["merge_debt"]["branch"], branch);
    }

    #[tokio::test]
    async fn driver_without_approval_is_denied_on_platform_projects() {
        let (state, _repo) = app_state("sigiled", "splat");
        let (status, body) =
            body_json(open(driver(), State(state.clone()), AxPath("sigiled".into())).await).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body["detail"].as_str().unwrap().contains("requires approval"));
        // With a live approval the same open passes (session-3 acceptance).
        state.auth.approvals.grant("sigiled-claude", "ivan", now_epoch() + 3600, json!({}));
        let (status, _) =
            body_json(open(driver(), State(state.clone()), AxPath("sigiled".into())).await).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn debt_survives_a_reboot() {
        let (state, repo) = app_state("smoke-persist", "spers");
        // Same AppState but with a real store attached.
        let store_dir = repo.parent().unwrap().join("smoke-persist-state");
        let state = crate::AppState {
            store: crate::store::Store::at_dir(&store_dir),
            ..state
        };
        let (_, a) = body_json(
            open(admin(), State(state.clone()), AxPath("smoke-persist".into())).await,
        )
        .await;
        let id = a["session_id"].as_str().unwrap().to_string();
        let branch = a["branch"].as_str().unwrap().to_string();
        commit_on(&repo, "master", "hot.txt", "ours\n", "fix: ours");
        commit_on(&repo, &branch, "hot.txt", "theirs\n", "fix: theirs");
        let (_, closed) = body_json(close(admin(), State(state.clone()), AxPath(id)).await).await;
        assert_eq!(closed["merge"], "debt");

        // "Reboot": a fresh AppState over the same store hydrates the debt.
        let reborn = crate::AppState {
            store: crate::store::Store::at_dir(&store_dir),
            sessions: SessionState::with_repos_dir(repo.parent().unwrap().to_path_buf()),
            ..crate::AppState::default()
        };
        reborn.hydrate_from_disk();
        let debts = reborn.sessions.debts_for("smoke-persist");
        assert_eq!(debts.len(), 1);
        assert_eq!(debts[0].branch, branch);
        // And the machine log came back with it.
        assert_eq!(reborn.events.for_project("smoke-persist").len(), 2);
    }

    #[tokio::test]
    async fn simultaneous_closes_serialize_one_ff_one_merged() {
        let (state, repo) = app_state("smoke-par", "spar");
        let (_, a) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-par".into())).await).await;
        let (_, b) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-par".into())).await).await;
        let (ida, bra) = (a["session_id"].as_str().unwrap(), a["branch"].as_str().unwrap());
        let (idb, brb) = (b["session_id"].as_str().unwrap(), b["branch"].as_str().unwrap());
        // Disjoint work: no conflict, but only one can fast-forward.
        commit_on(&repo, bra, "left.txt", "L\n", "feat: left");
        commit_on(&repo, brb, "right.txt", "R\n", "feat: right");

        let (ra, rb) = tokio::join!(
            close(admin(), State(state.clone()), AxPath(ida.to_string())),
            close(admin(), State(state.clone()), AxPath(idb.to_string())),
        );
        let (_, ja) = body_json(ra).await;
        let (_, jb) = body_json(rb).await;
        let mut kinds = vec![ja["merge"].as_str().unwrap(), jb["merge"].as_str().unwrap()];
        kinds.sort();
        assert_eq!(kinds, vec!["ff", "merged"]);
        // Both files made master: reconciliation, not exclusion.
        sh(&repo, &["checkout", "-f", "master"]);
        assert!(repo.join("left.txt").exists() && repo.join("right.txt").exists());
    }
}
