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
    /// The workspace token, kept so close can flush through the agent. It
    /// lives in the 0600 state file (as v1's registry did) and never in an
    /// API response other than the open that minted it.
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Clone)]
pub struct SessionState {
    /// Root of the project repos this control plane arbitrates. None = the
    /// verbs answer 503 (dev run without a repo store).
    pub repos_dir: Option<PathBuf>,
    /// Some = real workspaces (containers, deploy keys, mirrors). None =
    /// branch-only path: the verbs still arbitrate master, they just don't
    /// rent a container. Tests and dev runs live here.
    pub runtime: Option<crate::runtime::Runtime>,
    http: reqwest::Client,
    records: Arc<RwLock<HashMap<String, SessionRecord>>>,
    debts: Arc<RwLock<HashMap<String, Vec<MergeDebt>>>>,
    merge_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl Default for SessionState {
    fn default() -> Self {
        let runtime = crate::runtime::Runtime::from_env();
        SessionState {
            repos_dir: std::env::var("SIGILED_REPOS_DIR")
                .ok()
                .map(PathBuf::from)
                .or_else(|| runtime.as_ref().map(|r| r.repos_dir.clone())),
            runtime,
            http: reqwest::Client::new(),
            records: Arc::default(),
            debts: Arc::default(),
            merge_locks: Arc::default(),
        }
    }
}

impl SessionState {
    pub fn with_repos_dir(dir: PathBuf) -> Self {
        SessionState {
            repos_dir: Some(dir),
            runtime: None,
            ..SessionState::default()
        }
    }
    pub fn debts_for(&self, project: &str) -> Vec<MergeDebt> {
        self.debts
            .read()
            .unwrap()
            .get(project)
            .cloned()
            .unwrap_or_default()
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
    pub fn record(&self, id: &str) -> Option<SessionRecord> {
        self.records.read().unwrap().get(id).cloned()
    }
    pub fn remove_record(&self, id: &str) {
        self.records.write().unwrap().remove(id);
    }
    /// Sessions that hold a workspace token — the ones the reaper polls.
    pub fn live_records(&self) -> Vec<SessionRecord> {
        self.records
            .read()
            .unwrap()
            .values()
            .filter(|r| r.token.is_some())
            .cloned()
            .collect()
    }
    pub fn http(&self) -> &reqwest::Client {
        &self.http
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
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let n = N.fetch_add(1, Ordering::SeqCst);
        format!("{:08x}", (nanos ^ (n << 48)) & 0xffff_ffff)
    }
}

fn err(status: StatusCode, detail: impl Into<String>) -> Response {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

/// A session/* branch nobody owns: no live record rides it and it is not a
/// debtor waiting in the merge-debt queue. The reaper kills containers,
/// crashes lose control planes, the v1 left one behind — the branch on the
/// repo is the truth, and open() resumes the first orphan it finds instead
/// of cutting a new one (contract §5: stale resume).
fn find_orphan(repo: &std::path::Path, project: &str, state: &crate::AppState) -> Option<String> {
    let refs = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/session/*",
            "refs/remotes/origin/session/*",
        ],
    )
    .ok()?;
    let debts: Vec<String> = state
        .sessions
        .debts_for(project)
        .into_iter()
        .map(|d| d.branch)
        .collect();
    let live: Vec<String> = state
        .sessions
        .records
        .read()
        .unwrap()
        .values()
        .filter(|r| r.project == project)
        .map(|r| r.branch.clone())
        .collect();
    let mut names: Vec<String> = refs
        .lines()
        .map(|l| l.trim().trim_start_matches("origin/").to_string())
        .filter(|b| b.starts_with("session/"))
        .collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .find(|b| !debts.contains(b) && !live.contains(b))
}

fn now_epoch() -> u64 {
    crate::auth::now_epoch()
}

pub async fn open(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(project): AxPath<String>,
) -> Response {
    // The session-3 policy, consumed by the verb (DEC-15).
    if let Err(denial) = authorize(
        &actor,
        Action::OpenSession,
        Some(&project),
        &state.auth.approvals,
        now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, denial.0);
    }

    // Two paths, one contract. With a runtime: mirror + container + the
    // branch cut inside the workspace and pushed at once. Without: the
    // branch only, on the local repo (dev/tests). Both first look for an
    // orphan session/* branch (reaper, crash, v1 leftovers) and resume it
    // stale instead of cutting a new one (contract §5).
    fn fresh_id_branch() -> (String, String) {
        let id = SessionState::session_id();
        let branch = format!("session/{id}");
        (id, branch)
    }
    let (id, branch, resume, head, token, endpoint, image) = match &state.sessions.runtime {
        Some(rt) => {
            let mirror = match rt.ensure_mirror(&project) {
                Ok(m) => m,
                Err(e) => return err(StatusCode::NOT_FOUND, e),
            };
            let (id, branch, resume) = match find_orphan(&mirror, &project, &state) {
                Some(b) => (b.trim_start_matches("session/").to_string(), b, true),
                None => {
                    let (id, branch) = fresh_id_branch();
                    (id, branch, false)
                }
            };
            let vm = crate::runtime::Runtime::vm_name(&project);
            let tok = mint_token();
            // DEC-25: the per-project session image, from [workspace] on
            // master. The first open after a dockerfile edit pays a docker
            // build (minutes) — off the async runtime. A failed build falls
            // back to the base image with the shout in `image`.
            let image = {
                let (rt2, mirror2, p2) = (rt.clone(), mirror.clone(), project.clone());
                match tokio::task::spawn_blocking(move || rt2.ensure_session_image(&p2, &mirror2))
                    .await
                {
                    Ok(choice) => choice,
                    Err(e) => {
                        return err(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("image resolve: {e}"),
                        )
                    }
                }
            };
            if let Err(e) =
                rt.create_container(&vm, &project, "session", &id, &tok, &image.used, &[])
            {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e);
            }
            if let Err(e) = rt.wait_healthy(&state.sessions.http, &vm, &tok).await {
                rt.destroy(&vm);
                return err(StatusCode::INTERNAL_SERVER_ERROR, e);
            }
            match rt
                .boot_workspace(&state.sessions.http, &vm, &project, &tok, &branch, resume)
                .await
            {
                Ok(head) => (
                    id,
                    branch,
                    resume,
                    head,
                    Some(tok),
                    Some(rt.endpoint(&project)),
                    Some(image),
                ),
                Err(e) => {
                    rt.destroy(&vm);
                    return err(StatusCode::INTERNAL_SERVER_ERROR, e);
                }
            }
        }
        None => {
            let Some(repos) = state.sessions.repos_dir.clone() else {
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "SIGILED_REPOS_DIR not configured",
                );
            };
            let repo = repos.join(&project);
            if !repo.join(".git").exists() {
                return err(StatusCode::NOT_FOUND, format!("unknown project: {project}"));
            }
            match find_orphan(&repo, &project, &state) {
                Some(b) => {
                    let head = match git(&repo, &["rev-parse", &b]) {
                        Ok(h) => h,
                        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
                    };
                    (
                        b.trim_start_matches("session/").to_string(),
                        b,
                        true,
                        head,
                        None,
                        None,
                        None,
                    )
                }
                None => {
                    let (id, branch) = fresh_id_branch();
                    if let Err(e) = git(&repo, &["branch", &branch, "master"]) {
                        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
                    }
                    let head = git(&repo, &["rev-parse", "master"]).unwrap_or_default();
                    (id, branch, false, head, None, None, None)
                }
            }
        }
    };

    let record = SessionRecord {
        session_id: id.clone(),
        project: project.clone(),
        branch: branch.clone(),
        head: head.clone(),
        stale: resume,
        actor: actor.clone(),
        token: token.clone(),
    };
    state
        .sessions
        .records
        .write()
        .unwrap()
        .insert(id.clone(), record);
    state.events.record(
        &project,
        now_epoch(),
        Event::SessionOpened {
            session_id: id.clone(),
            branch: branch.clone(),
            stale: resume,
        },
    );

    state.persist();

    // merge_debt on top when present (rule: shout, don't whisper).
    let debts = state.sessions.debts_for(&project);
    (
        StatusCode::CREATED,
        Json(json!({
            "session_id": id, "project": project, "branch": branch,
            "token": token, "endpoint": endpoint,
            "head": head, "stale": resume,
            "last_commit": if resume { json!(head) } else { serde_json::Value::Null },
            "merge_debt": debts.first(),
            "image": image,
            "actor": actor,
        })),
    )
        .into_response()
}

pub(crate) fn mint_token() -> String {
    // 24 bytes of entropy, hex — same shape as the v1 token the edge and
    // the agent already expect. Sourced from the OS via getrandom-through-
    // std: no crypto dependency for a value that is only ever compared.
    use std::hash::{BuildHasher, Hasher, RandomState};
    let mut out = String::with_capacity(48);
    while out.len() < 48 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos() as u64,
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out.truncate(48);
    out
}

pub async fn close(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(session_id): AxPath<String>,
) -> Response {
    let Some(record) = state
        .sessions
        .records
        .read()
        .unwrap()
        .get(&session_id)
        .cloned()
    else {
        return err(
            StatusCode::NOT_FOUND,
            format!("unknown session: {session_id}"),
        );
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
    // With a runtime: flush the workspace first (its commits must exist
    // before we merge), then arbitrate on the refreshed mirror.
    let mut flushed = true;
    if let Some(rt) = &state.sessions.runtime {
        if let Some(tok) = &record.token {
            let vm = crate::runtime::Runtime::vm_name(&record.project);
            flushed = rt
                .flush(&state.sessions.http, &vm, tok, "session close")
                .await;
        }
        if let Err(e) = rt.ensure_mirror(&record.project) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    }
    let repos = state
        .sessions
        .repos_dir
        .clone()
        .expect("open required repos_dir");
    let repo = repos.join(&record.project);

    // The critical section shrank from the whole session to these few
    // lines (§4.1): simultaneous closes serialize here — one wins, the
    // other sees a moved master and takes the merge path.
    let lock = state.sessions.merge_lock(&record.project);
    let _guard = lock.lock().await;

    // With a runtime the branch lives on the remote: give the mirror a
    // local ref to merge from.
    if state.sessions.runtime.is_some() {
        let _ = git(
            &repo,
            &["fetch", "origin", &format!("{0}:{0}", record.branch)],
        );
    }

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
            // Clean close: publish the merged master, then drop the branch
            // here and on the remote; the debt (if this was a debtor being
            // resolved) is paid.
            if let Some(rt) = &state.sessions.runtime {
                if let Err(e) = rt.push(&record.project, "master") {
                    // Master moved under us between merge and push: the work
                    // is safe on the branch, the next close re-arbitrates.
                    return err(StatusCode::CONFLICT, format!("push master: {e}"));
                }
                let _ = rt.push(&record.project, &format!(":{}", record.branch));
            }
            let _ = git(&repo, &["branch", "-D", &record.branch]);
            state.sessions.clear_debt(&record.project, &record.branch);
        }
        Some(d) => {
            // Conflict: master stayed put, the branch survives as the
            // debtor, the queue inherits the package.
            state.sessions.push_debt(&record.project, d.clone());
        }
    }
    if let Some(rt) = &state.sessions.runtime {
        rt.destroy(&crate::runtime::Runtime::vm_name(&record.project));
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
        "closed": true, "merge": merge_kind, "sha": sha, "flushed": flushed,
        "log_operativo_touched": touched, "merge_debt": debt,
    }))
    .into_response()
}

/// POST /sigiled/sessions/{id}/recycle — flush, destroy, recreate from the
/// session's own branch with a freshly minted token (contract §5): the way
/// to hand a session to another provider or to unwedge a container. The old
/// token is dead the moment this returns.
pub async fn recycle(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(session_id): AxPath<String>,
) -> Response {
    let Some(record) = state
        .sessions
        .records
        .read()
        .unwrap()
        .get(&session_id)
        .cloned()
    else {
        return err(
            StatusCode::NOT_FOUND,
            format!("unknown session: {session_id}"),
        );
    };
    if let Err(denial) = authorize(
        &actor,
        Action::Recycle,
        Some(&record.project),
        &state.auth.approvals,
        now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, denial.0);
    }
    // Flush is best-effort by design: a wedged container is one of the two
    // reasons to recycle, and push-early means only unpushed leftovers are
    // at stake. The verb proceeds either way and reports honestly.
    let mut flushed = true;
    let (head, token, endpoint, image) = match &state.sessions.runtime {
        Some(rt) => {
            let vm = crate::runtime::Runtime::vm_name(&record.project);
            if let Some(tok) = &record.token {
                flushed = rt
                    .flush(&state.sessions.http, &vm, tok, "session recycle")
                    .await;
            }
            rt.destroy(&vm);
            let tok = mint_token();
            // DEC-25: the fresh container rides the image master declares
            // NOW — a recycle after a dockerfile fix is how a session picks
            // up its repaired toolchain without dying.
            let image = match rt.ensure_mirror(&record.project) {
                Ok(mirror) => {
                    let (rt2, p2) = (rt.clone(), record.project.clone());
                    match tokio::task::spawn_blocking(move || {
                        rt2.ensure_session_image(&p2, &mirror)
                    })
                    .await
                    {
                        Ok(choice) => choice,
                        Err(e) => {
                            return err(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("image resolve: {e}"),
                            )
                        }
                    }
                }
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
            };
            if let Err(e) = rt.create_container(
                &vm,
                &record.project,
                "session",
                &session_id,
                &tok,
                &image.used,
                &[],
            ) {
                return err(StatusCode::INTERNAL_SERVER_ERROR, e);
            }
            if let Err(e) = rt.wait_healthy(&state.sessions.http, &vm, &tok).await {
                rt.destroy(&vm);
                return err(StatusCode::INTERNAL_SERVER_ERROR, e);
            }
            // resume=true: the branch already exists on the remote — the
            // fresh container checks it out instead of cutting a new one.
            match rt
                .boot_workspace(
                    &state.sessions.http,
                    &vm,
                    &record.project,
                    &tok,
                    &record.branch,
                    true,
                )
                .await
            {
                Ok(head) => (
                    head,
                    Some(tok),
                    Some(rt.endpoint(&record.project)),
                    Some(image),
                ),
                Err(e) => {
                    rt.destroy(&vm);
                    return err(StatusCode::INTERNAL_SERVER_ERROR, e);
                }
            }
        }
        None => {
            let Some(repos) = state.sessions.repos_dir.clone() else {
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "SIGILED_REPOS_DIR not configured",
                );
            };
            let repo = repos.join(&record.project);
            match git(&repo, &["rev-parse", &record.branch]) {
                Ok(sha) => (sha, None, None, None),
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
        }
    };
    // The record survives with the fresh token: the old one is dead the
    // moment this swap lands (and persists).
    {
        let mut records = state.sessions.records.write().unwrap();
        if let Some(r) = records.get_mut(&session_id) {
            r.token = token.clone();
            r.head = head.clone();
        }
    }
    state.events.record(
        &record.project,
        now_epoch(),
        Event::SessionRecycled {
            session_id: session_id.clone(),
            sha: head.clone(),
        },
    );
    state.persist();
    Json(json!({
        "session_id": session_id, "project": record.project, "branch": record.branch,
        "token": token, "endpoint": endpoint, "sha_at_recycle": head, "flushed": flushed,
        "image": image,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::tests::{commit_on, mk_repo, sh};
    use axum::body::to_bytes;

    fn admin() -> Actor {
        Actor {
            driver: "bootstrap".into(),
            role: crate::auth::Role::Admin,
            approval: None,
        }
    }
    fn driver() -> Actor {
        Actor {
            driver: "sigiled-claude".into(),
            role: crate::auth::Role::Driver,
            approval: None,
        }
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
        // Branch-only path rents no container: no image to report (DEC-25).
        assert!(body["image"].is_null());
        let id = body["session_id"].as_str().unwrap().to_string();
        let branch = body["branch"].as_str().unwrap().to_string();

        commit_on(
            &repo,
            &branch,
            "docs/log-operativo.md",
            "# log\nvoce\n",
            "log: voce",
        );
        let (status, body) =
            body_json(close(admin(), State(state.clone()), AxPath(id)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["merge"], "ff");
        assert_eq!(body["log_operativo_touched"], true);
        assert_eq!(
            body["sha"].as_str().unwrap(),
            sh(&repo, &["rev-parse", "master"])
        );
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
        let (_, closed) = body_json(close(admin(), State(state.clone()), AxPath(id)).await).await;
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
        assert!(body["detail"]
            .as_str()
            .unwrap()
            .contains("requires approval"));
        // With a live approval the same open passes (session-3 acceptance).
        state
            .auth
            .approvals
            .grant("sigiled-claude", "ivan", now_epoch() + 3600, json!({}));
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
            open(
                admin(),
                State(state.clone()),
                AxPath("smoke-persist".into()),
            )
            .await,
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
    async fn reaped_session_leaves_orphan_and_open_resumes_it() {
        let (state, repo) = app_state("smoke-reap", "sreap");
        let (_, a) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-reap".into())).await).await;
        assert_eq!(a["stale"], false);
        let id = a["session_id"].as_str().unwrap().to_string();
        let branch = a["branch"].as_str().unwrap().to_string();
        commit_on(
            &repo,
            &branch,
            "midwork.txt",
            "m\n",
            "feat: interrupted work",
        );

        crate::reaper::reap(&state, &id, "idle").await;
        assert!(
            state.sessions.record(&id).is_none(),
            "reap must drop the record"
        );
        let ev = serde_json::to_value(state.events.for_project("smoke-reap")).unwrap();
        assert_eq!(ev[1]["kind"], "session_reaped");

        // The next open resumes the orphan branch, stale and honest.
        let (status, b) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-reap".into())).await).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(b["stale"], true, "body: {b}");
        assert_eq!(b["branch"], json!(branch.clone()));
        assert_eq!(b["session_id"], json!(id.clone()));
        assert_eq!(
            b["last_commit"].as_str().unwrap(),
            sh(&repo, &["rev-parse", &branch])
        );
        // The resumed session closes clean, interrupted work merged.
        let (_, closed) = body_json(close(admin(), State(state.clone()), AxPath(id)).await).await;
        assert_eq!(closed["merge"], "ff");
        sh(&repo, &["checkout", "-f", "master"]);
        assert!(repo.join("midwork.txt").exists());
    }

    #[tokio::test]
    async fn orphan_scan_ignores_live_and_debtor_branches() {
        let (state, repo) = app_state("smoke-orph", "sorph");
        // A live session: its branch is owned.
        let (_, live) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-orph".into())).await).await;
        // A debtor: closed into conflict, branch survives in the debt queue.
        let (_, d) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-orph".into())).await).await;
        let debtor_id = d["session_id"].as_str().unwrap().to_string();
        let debtor_branch = d["branch"].as_str().unwrap().to_string();
        commit_on(&repo, "master", "hot.txt", "ours\n", "fix: ours");
        commit_on(&repo, &debtor_branch, "hot.txt", "theirs\n", "fix: theirs");
        let (_, closed) =
            body_json(close(admin(), State(state.clone()), AxPath(debtor_id)).await).await;
        assert_eq!(closed["merge"], "debt");
        // A fresh open must cut a NEW branch: the live one is owned, the
        // debtor is queued — neither is an orphan.
        let (_, c) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-orph".into())).await).await;
        assert_eq!(c["stale"], false, "body: {c}");
        assert_ne!(c["branch"], live["branch"]);
        assert_ne!(c["branch"], json!(debtor_branch));
    }

    #[tokio::test]
    async fn recycle_unknown_session_is_404() {
        let state = crate::AppState::default();
        let (status, _) =
            body_json(recycle(admin(), State(state), AxPath("deadbeef".into())).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn recycle_returns_branch_head_and_session_survives() {
        let (state, repo) = app_state("smoke-rec", "srec");
        let (_, a) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-rec".into())).await).await;
        let id = a["session_id"].as_str().unwrap().to_string();
        let branch = a["branch"].as_str().unwrap().to_string();
        commit_on(
            &repo,
            &branch,
            "work.txt",
            "w\n",
            "feat: work before recycle",
        );

        let (status, body) =
            body_json(recycle(admin(), State(state.clone()), AxPath(id.clone())).await).await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        // sha_at_recycle is the branch's own head — the recreate starts there.
        assert_eq!(
            body["sha_at_recycle"].as_str().unwrap(),
            sh(&repo, &["rev-parse", &branch])
        );
        assert_eq!(body["branch"].as_str().unwrap(), branch);
        // Branch-only path mints no token, honestly (as open does).
        assert!(body["token"].is_null());
        // The record survived with the refreshed head: close still works.
        let (status, closed) =
            body_json(close(admin(), State(state.clone()), AxPath(id)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(closed["merge"], "ff");
        // Machine log: opened, recycled, closed.
        let ev = serde_json::to_value(state.events.for_project("smoke-rec")).unwrap();
        assert_eq!(ev[1]["kind"], "session_recycled");
        assert_eq!(ev[1]["sha"], body["sha_at_recycle"]);
    }

    #[tokio::test]
    async fn simultaneous_closes_serialize_one_ff_one_merged() {
        let (state, repo) = app_state("smoke-par", "spar");
        let (_, a) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-par".into())).await).await;
        let (_, b) =
            body_json(open(admin(), State(state.clone()), AxPath("smoke-par".into())).await).await;
        let (ida, bra) = (
            a["session_id"].as_str().unwrap(),
            a["branch"].as_str().unwrap(),
        );
        let (idb, brb) = (
            b["session_id"].as_str().unwrap(),
            b["branch"].as_str().unwrap(),
        );
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
