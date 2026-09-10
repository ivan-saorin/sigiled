// Session lifecycle. Lock order is session, then project mirror; opening a
// newly allocated identity also holds its session lock before publishing.
use crate::auth::{authorize, Action, Actor, Role};
use crate::events::{log_operativo_touched, Event};
use crate::merge::{changed_paths, close_merge, git, MergeDebt, MergeOutcome};
use axum::extract::{Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceBinding {
    #[serde(default)]
    pub ide_error: Option<String>,
    #[serde(default)]
    pub ide: Option<crate::ide::Binding>,
    pub container: String,
    pub endpoint: String,
    #[serde(default)]
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Creating,
    #[default]
    Active,
    Recycling,
    Closing,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    LegacyOwnershipAmbiguous,
    Interrupted,
    PersistFailed,
    MirrorFailed,
    CreateFailed,
    BootFailed,
    FlushFailed,
    FetchFailed,
    MergeFailed,
    PushFailed,
    CleanupFailed,
    RuntimeNotOwned,
    RuntimeUnavailable,
    GenerationExhausted,
}
fn legacy_owned() -> bool {
    true
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub project: String,
    pub branch: String,
    pub head: String,
    pub stale: bool,
    pub actor: Actor,
    // Custodied persistence only. Public responses use an explicit projection.
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub binding: Option<WorkspaceBinding>,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub error: Option<Failure>,
    #[serde(default)]
    pub image: Option<String>,
    // A failed create may collide with an incumbent. Never claim that runtime.
    #[serde(default = "legacy_owned")]
    pub runtime_owned: bool,
}
// Debug must not accidentally expose the custodied token in a log or panic.
impl std::fmt::Debug for SessionRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRecord")
            .field("session_id", &self.session_id)
            .field("project", &self.project)
            .field("lifecycle", &self.lifecycle)
            .finish_non_exhaustive()
    }
}
impl SessionRecord {
    pub fn container(&self) -> String {
        self.binding
            .as_ref()
            .map(|b| b.container.clone())
            .unwrap_or_else(|| crate::runtime::Runtime::vm_name(&self.project))
    }
    pub(crate) fn view(&self, rt: Option<&crate::runtime::Runtime>) -> serde_json::Value {
        json!({"session_id":self.session_id,"project":self.project,"branch":self.branch,
            "actor":self.actor,"state":self.lifecycle,"generation":self.generation,
            "endpoint":self.binding.as_ref().map(|b|b.endpoint.clone()).or_else(||rt.map(|r|r.endpoint(&self.project))),
            "runtime": if self.token.is_some() { Some(self.container()) } else { None },
            "image":self.image,"error":self.error,"ide":self.binding.as_ref().and_then(|b|b.ide.as_ref()).map(crate::ide::Binding::view)})
    }
}
type Locks = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;
#[derive(Clone)]
pub struct SessionState {
    pub repos_dir: Option<PathBuf>,
    pub runtime: Option<crate::runtime::Runtime>,
    http: reqwest::Client,
    records: Arc<RwLock<HashMap<String, SessionRecord>>>,
    debts: Arc<RwLock<HashMap<String, Vec<MergeDebt>>>>,
    merge_locks: Locks,
    session_locks: Locks,
}
impl Default for SessionState {
    fn default() -> Self {
        let runtime = crate::runtime::Runtime::from_env();
        Self {
            repos_dir: std::env::var("SIGILED_REPOS_DIR")
                .ok()
                .map(PathBuf::from)
                .or_else(|| runtime.as_ref().map(|r| r.repos_dir.clone())),
            runtime,
            http: reqwest::Client::new(),
            records: Arc::default(),
            debts: Arc::default(),
            merge_locks: Arc::default(),
            session_locks: Arc::default(),
        }
    }
}
impl SessionState {
    pub fn with_repos_dir(dir: PathBuf) -> Self {
        Self {
            repos_dir: Some(dir),
            runtime: None,
            http: reqwest::Client::new(),
            records: Arc::default(),
            debts: Arc::default(),
            merge_locks: Arc::default(),
            session_locks: Arc::default(),
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
        let mut m = self.debts.write().unwrap();
        let q = m.entry(project.into()).or_default();
        q.retain(|d| d.branch != debt.branch);
        q.push(debt);
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
    pub fn live_records(&self) -> Vec<SessionRecord> {
        self.records
            .read()
            .unwrap()
            .values()
            .filter(|r| r.token.is_some() && r.lifecycle == Lifecycle::Active)
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
        mut records: HashMap<String, SessionRecord>,
    ) {
        let mut legacy = HashMap::<String, usize>::new();
        for r in records
            .values()
            .filter(|r| r.binding.is_none() && r.token.is_some())
        {
            *legacy.entry(r.project.clone()).or_default() += 1;
        }
        for r in records.values_mut() {
            if r.binding.is_none()
                && r.token.is_some()
                && legacy.get(&r.project).copied().unwrap_or(0) > 1
            {
                r.lifecycle = Lifecycle::Failed;
                r.error = Some(Failure::LegacyOwnershipAmbiguous);
            } else if matches!(
                r.lifecycle,
                Lifecycle::Creating | Lifecycle::Closing | Lifecycle::Recycling
            ) {
                r.lifecycle = Lifecycle::Failed;
                r.error = Some(Failure::Interrupted);
            }
        }
        *self.debts.write().unwrap() = debts;
        *self.records.write().unwrap() = records;
    }
    fn lock(locks: &Locks, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        locks.lock().unwrap().entry(key.into()).or_default().clone()
    }
    pub(crate) fn merge_lock(&self, project: &str) -> Arc<tokio::sync::Mutex<()>> {
        Self::lock(&self.merge_locks, project)
    }
    pub(crate) fn session_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Self::lock(&self.session_locks, id)
    }
    pub(crate) fn put(&self, record: SessionRecord) {
        self.records
            .write()
            .unwrap()
            .insert(record.session_id.clone(), record);
    }
    pub(crate) fn mark(&self, id: &str, lifecycle: Lifecycle, error: Option<Failure>) {
        if let Some(r) = self.records.write().unwrap().get_mut(id) {
            r.lifecycle = lifecycle;
            r.error = error;
        }
    }
    pub(crate) fn binding_safe(&self, record: &SessionRecord) -> bool {
        if record.error == Some(Failure::LegacyOwnershipAmbiguous) {
            return false;
        }
        let target = record.container();
        !self.records.read().unwrap().values().any(|r| {
            r.session_id != record.session_id && r.token.is_some() && r.container() == target
        })
    }
    fn session_id() -> String {
        random_hex(16)
    }
}
fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0; bytes];
    getrandom::getrandom(&mut buf).expect("OS random source unavailable");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
pub(crate) fn mint_token() -> String {
    random_hex(24)
}
fn err(status: StatusCode, detail: impl Into<String>) -> Response {
    (status, Json(json!({"detail":detail.into()}))).into_response()
}
pub(crate) fn failure(state: &crate::AppState, id: &str, code: Failure) -> Response {
    state.sessions.mark(id, Lifecycle::Failed, Some(code));
    state.persist();
    (StatusCode::CONFLICT,Json(json!({"error":code,"detail":"session preserved for recovery","session_id":id,"recoverable":true}))).into_response()
}
fn now_epoch() -> u64 {
    crate::auth::now_epoch()
}
pub(crate) fn authorized(
    actor: &Actor,
    record: &SessionRecord,
    state: &crate::AppState,
    action: Action,
) -> Result<(), String> {
    authorize(
        actor,
        action,
        Some(&record.project),
        &state.auth.approvals,
        now_epoch(),
    )
    .map_err(|d| d.0)?;
    if actor.role != Role::Admin && actor.driver != record.actor.driver {
        return Err("session belongs to another actor".into());
    }
    Ok(())
}
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
    let debts = state.sessions.debts_for(project);
    let records = state.sessions.dump_records();
    let mut names: Vec<String> = refs
        .lines()
        .map(|l| l.trim().trim_start_matches("origin/").into())
        .filter(|b: &String| b.starts_with("session/"))
        .collect();
    names.sort();
    names.dedup();
    names.into_iter().find(|b| {
        !debts.iter().any(|d| d.branch == *b)
            && !records
                .values()
                .any(|r| r.project == project && r.branch == *b)
    })
}
#[derive(Default, Deserialize)]
pub struct SessionQuery {
    pub project: Option<String>,
}
pub async fn list(
    _actor: Actor,
    State(state): State<crate::AppState>,
    Query(query): Query<SessionQuery>,
) -> Response {
    let mut records: Vec<_> = state
        .sessions
        .dump_records()
        .into_values()
        .filter(|r| query.project.as_ref().is_none_or(|p| p == &r.project))
        .collect();
    records.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Json(json!({"sessions":records.iter().map(|r|r.view(state.sessions.runtime.as_ref())).collect::<Vec<_>>()})).into_response()
}
pub async fn detail(
    _actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(id): AxPath<String>,
) -> Response {
    match state.sessions.record(&id) {
        Some(r) => Json(r.view(state.sessions.runtime.as_ref())).into_response(),
        None => err(StatusCode::NOT_FOUND, "unknown session"),
    }
}
pub async fn open(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(project): AxPath<String>,
) -> Response {
    if let Err(d) = authorize(
        &actor,
        Action::OpenSession,
        Some(&project),
        &state.auth.approvals,
        now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, d.0);
    }
    // Existing project slugs are shell/DNS-safe. Reject paths or shell fragments
    // before Git and runtime names are constructed, including unknown projects.
    if project.is_empty()
        || !project
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return err(StatusCode::BAD_REQUEST, "invalid project slug");
    }
    let id = SessionState::session_id();
    let session_lock = state.sessions.session_lock(&id);
    let _session = session_lock.lock_owned().await;
    let lock = state.sessions.merge_lock(&project);
    let _guard = lock.lock_owned().await;
    let rt = state.sessions.runtime.as_ref();
    let mut record = SessionRecord {
        session_id: id.clone(),
        project: project.clone(),
        branch: format!("session/{id}"),
        head: String::new(),
        stale: false,
        actor: actor.clone(),
        token: rt.map(|_| mint_token()),
        binding: rt.map(|r| WorkspaceBinding {
            ide_error: Some("provider_image_pending".into()),
            ide: None,
            container: crate::runtime::Runtime::session_container(&project, &id, 1),
            endpoint: r.session_endpoint(&project, &id, 1),
            generation: 1,
        }),
        lifecycle: Lifecycle::Creating,
        generation: 1,
        error: None,
        image: None,
        runtime_owned: false,
    };
    state.sessions.put(record.clone());
    if state.try_persist().is_err() {
        return failure(&state, &id, Failure::PersistFailed);
    }
    let repo = if let Some(rt) = rt {
        match rt.ensure_mirror(&project) {
            Ok(p) => p,
            Err(_) => return failure(&state, &id, Failure::MirrorFailed),
        }
    } else {
        let Some(repos) = state.sessions.repos_dir.as_ref() else {
            state.sessions.remove_record(&id);
            state.persist();
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "SIGILED_REPOS_DIR not configured",
            );
        };
        let repo = repos.join(&project);
        if !repo.join(".git").exists() {
            state.sessions.remove_record(&id);
            state.persist();
            return err(StatusCode::NOT_FOUND, "unknown project");
        }
        repo
    };
    if let Some(branch) = find_orphan(&repo, &project, &state) {
        record.branch = branch;
        record.stale = true;
    }
    state.sessions.put(record.clone());
    if state.try_persist().is_err() {
        return failure(&state, &id, Failure::PersistFailed);
    }
    let mut ide_guard = Some(_session);
    let mut image = None;
    if let Some(rt) = rt {
        let (choice, _guard) = match rt.session_image_locked(&project, &repo, _guard).await {
            Ok(c) => c,
            Err(_) => return failure(&state, &id, Failure::BootFailed),
        };
        drop(_guard);
        let mut choice = choice;
        let enabled = state
            .registry
            .descriptor(&record.project)
            .declaration
            .ide
            .enabled;
        ide_guard = Some(
            crate::ide::prepare_layer(
                rt,
                &mut record,
                &mut choice,
                enabled,
                ide_guard.take().unwrap(),
            )
            .await,
        );
        record.image = Some(choice.used.clone());
        state.sessions.put(record.clone());
        if state.try_persist().is_err() {
            return failure(&state, &id, Failure::PersistFailed);
        }
        let vm = record.container();
        let token = record.token.as_deref().unwrap();
        if rt
            .create_container_profile(
                &vm,
                &project,
                "session",
                &id,
                token,
                &choice.used,
                &[],
                record
                    .binding
                    .as_ref()
                    .and_then(|b| b.ide.as_ref())
                    .map(|b| b.profile_volume.as_str()),
            )
            .is_err()
        {
            return failure(&state, &id, Failure::CreateFailed);
        }
        record.runtime_owned = true;
        state.sessions.put(record.clone());
        if state.try_persist().is_err() {
            return failure(&state, &id, Failure::PersistFailed);
        }
        if rt
            .wait_healthy(&state.sessions.http, &vm, token)
            .await
            .is_err()
        {
            return failure(&state, &id, Failure::BootFailed);
        }
        record.head = match rt
            .boot_workspace(
                &state.sessions.http,
                &vm,
                &project,
                token,
                &record.branch,
                record.stale,
            )
            .await
        {
            Ok(h) => h,
            Err(_) => return failure(&state, &id, Failure::BootFailed),
        };
        image = Some(choice);
    } else {
        if !record.stale && git(&repo, &["branch", &record.branch, "master"]).is_err() {
            return failure(&state, &id, Failure::CreateFailed);
        }
        record.head = match git(&repo, &["rev-parse", &record.branch]) {
            Ok(h) => h,
            Err(_) => return failure(&state, &id, Failure::BootFailed),
        };
    }
    let _session = ide_guard.take().expect("session generation guard retained");
    record.lifecycle = Lifecycle::Active;
    state.sessions.put(record.clone());
    state.events.record(
        &project,
        now_epoch(),
        Event::SessionOpened {
            session_id: id.clone(),
            branch: record.branch.clone(),
            stale: record.stale,
        },
    );
    if state.try_persist().is_err() {
        return failure(&state, &id, Failure::PersistFailed);
    }
    (StatusCode::CREATED,Json(json!({"session_id":id,"project":project,"branch":record.branch,"token":record.token,"endpoint":record.binding.as_ref().map(|b|&b.endpoint),"head":record.head,"stale":record.stale,"last_commit":if record.stale{json!(record.head)}else{serde_json::Value::Null},"merge_debt":state.sessions.debts_for(&project).first(),"image":image,"actor":actor,"generation":record.generation,"state":record.lifecycle}))).into_response()
}
pub async fn close(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(id): AxPath<String>,
) -> Response {
    close_expected(actor, state, id, None).await
}
pub(crate) async fn close_expected(
    actor: Actor,
    state: crate::AppState,
    id: String,
    expected: Option<u64>,
) -> Response {
    let lock = state.sessions.session_lock(&id);
    let _session = lock.lock_owned().await;
    let Some(record) = state.sessions.record(&id) else {
        return err(StatusCode::NOT_FOUND, "unknown session");
    };
    if expected.is_some_and(|generation| generation != record.generation) {
        return err(StatusCode::CONFLICT, "session generation changed");
    }
    if let Err(e) = authorized(&actor, &record, &state, Action::CloseSession) {
        return err(StatusCode::FORBIDDEN, e);
    }
    if record.token.is_some() && state.sessions.runtime.is_none() {
        return failure(&state, &id, Failure::RuntimeUnavailable);
    }
    if !state.sessions.binding_safe(&record) {
        return failure(&state, &id, Failure::LegacyOwnershipAmbiguous);
    }
    if state.sessions.runtime.is_some() && (!record.runtime_owned || record.token.is_none()) {
        return failure(&state, &id, Failure::RuntimeNotOwned);
    }
    state.sessions.mark(&id, Lifecycle::Closing, None);
    if state.try_persist().is_err() {
        return failure(&state, &id, Failure::PersistFailed);
    }
    if let (Some(_rt), Some(_tok)) = (&state.sessions.runtime, &record.token) {
        if !crate::ide::flush_record(&state, &record, "session close").await {
            return failure(&state, &id, Failure::FlushFailed);
        }
    }
    let lock = state.sessions.merge_lock(&record.project);
    let _mirror = lock.lock().await;
    let repo = if let Some(rt) = &state.sessions.runtime {
        match rt.ensure_mirror(&record.project) {
            Ok(p) => p,
            Err(_) => return failure(&state, &id, Failure::MirrorFailed),
        }
    } else {
        let Some(repos) = state.sessions.repos_dir.as_ref() else {
            return failure(&state, &id, Failure::MirrorFailed);
        };
        repos.join(&record.project)
    };
    if state.sessions.runtime.is_some()
        && git(
            &repo,
            &["fetch", "origin", &format!("+{0}:{0}", record.branch)],
        )
        .is_err()
    {
        return failure(&state, &id, Failure::FetchFailed);
    }
    let touched = changed_paths(&repo, &record.branch)
        .map(|p| log_operativo_touched(&p))
        .unwrap_or(false);
    let outcome = match close_merge(&repo, &record.branch) {
        Ok(o) => o,
        Err(_) => return failure(&state, &id, Failure::MergeFailed),
    };
    let (kind, sha, debt) = match outcome {
        MergeOutcome::Ff { sha } => ("ff", sha, None),
        MergeOutcome::Merged { sha } => ("merged", sha, None),
        MergeOutcome::Debt(d) => ("debt", d.ours.sha.clone(), Some(d)),
    };
    if let Some(d) = &debt {
        state.sessions.push_debt(&record.project, d.clone());
    } else if let Some(rt) = &state.sessions.runtime {
        if rt.push(&record.project, "master").is_err() {
            return failure(&state, &id, Failure::PushFailed);
        }
    }
    // The successful checkpoint and master/debt are durable before cleanup.
    if state.try_persist().is_err() {
        return failure(&state, &id, Failure::PersistFailed);
    }
    if let Some(rt) = &state.sessions.runtime {
        if !rt.destroy(&record.container()) {
            return failure(&state, &id, Failure::CleanupFailed);
        }
    }
    if debt.is_none() {
        if let Some(rt) = &state.sessions.runtime {
            let _ = rt.push(&record.project, &format!(":{}", record.branch));
        }
        let _ = git(&repo, &["branch", "-D", &record.branch]);
        state.sessions.clear_debt(&record.project, &record.branch);
    }
    state.sessions.remove_record(&id);
    state.events.record(
        &record.project,
        now_epoch(),
        Event::SessionClosed {
            session_id: id,
            merged: debt.is_none(),
            sha: sha.clone(),
            log_operativo_touched: touched,
        },
    );
    state.persist();
    Json(json!({"closed":true,"merge":kind,"sha":sha,"flushed":true,"log_operativo_touched":touched,"merge_debt":debt})).into_response()
}
pub async fn recycle(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(id): AxPath<String>,
) -> Response {
    let lock = state.sessions.session_lock(&id);
    let _session = lock.lock_owned().await;
    let Some(mut record) = state.sessions.record(&id) else {
        return err(StatusCode::NOT_FOUND, "unknown session");
    };
    if let Err(e) = authorized(&actor, &record, &state, Action::Recycle) {
        return err(StatusCode::FORBIDDEN, e);
    }
    if record.token.is_some() && state.sessions.runtime.is_none() {
        return failure(&state, &id, Failure::RuntimeUnavailable);
    }
    if !state.sessions.binding_safe(&record) {
        return failure(&state, &id, Failure::LegacyOwnershipAmbiguous);
    }
    if state.sessions.runtime.is_some() && (!record.runtime_owned || record.token.is_none()) {
        return failure(&state, &id, Failure::RuntimeNotOwned);
    }
    let Some(next_generation) = record.generation.checked_add(1) else {
        return failure(&state, &id, Failure::GenerationExhausted);
    };
    state.sessions.mark(&id, Lifecycle::Recycling, None);
    if state.try_persist().is_err() {
        return failure(&state, &id, Failure::PersistFailed);
    }
    if let (Some(_rt), Some(_tok)) = (&state.sessions.runtime, &record.token) {
        if !crate::ide::flush_record(&state, &record, "session recycle").await {
            return failure(&state, &id, Failure::FlushFailed);
        }
    }
    let lock = state.sessions.merge_lock(&record.project);
    let _mirror = lock.lock_owned().await;
    let repo = if let Some(rt) = &state.sessions.runtime {
        match rt.ensure_mirror(&record.project) {
            Ok(p) => p,
            Err(_) => return failure(&state, &id, Failure::MirrorFailed),
        }
    } else {
        let Some(repos) = state.sessions.repos_dir.as_ref() else {
            return failure(&state, &id, Failure::MirrorFailed);
        };
        repos.join(&record.project)
    };
    if state.sessions.runtime.is_some()
        && git(
            &repo,
            &["fetch", "origin", &format!("+{0}:{0}", record.branch)],
        )
        .is_err()
    {
        return failure(&state, &id, Failure::FetchFailed);
    }
    record.head = match git(&repo, &["rev-parse", &record.branch]) {
        Ok(h) => h,
        Err(_) => return failure(&state, &id, Failure::FetchFailed),
    };
    let mut ide_guard = Some(_session);
    let mut image = None;
    record.generation = next_generation;
    if let Some(rt) = &state.sessions.runtime {
        let (choice, _mirror) = match rt
            .session_image_locked(&record.project, &repo, _mirror)
            .await
        {
            Ok(c) => c,
            Err(_) => return failure(&state, &id, Failure::BootFailed),
        };
        if !rt.destroy(&record.container()) {
            return failure(&state, &id, Failure::CleanupFailed);
        }
        // The former container's branch is confirmed on origin. Persist the
        // next binding and token before allocation; never reuse legacy aliases.
        record.binding = Some(WorkspaceBinding {
            ide_error: Some("provider_image_pending".into()),
            ide: None,
            container: crate::runtime::Runtime::session_container(
                &record.project,
                &id,
                record.generation,
            ),
            endpoint: rt.session_endpoint(&record.project, &id, record.generation),
            generation: record.generation,
        });
        record.token = Some(mint_token());
        record.runtime_owned = false;
        state.sessions.put(record.clone());
        if state.try_persist().is_err() {
            return failure(&state, &id, Failure::PersistFailed);
        }
        drop(_mirror);
        let mut choice = choice;
        let enabled = state
            .registry
            .descriptor(&record.project)
            .declaration
            .ide
            .enabled;
        ide_guard = Some(
            crate::ide::prepare_layer(
                rt,
                &mut record,
                &mut choice,
                enabled,
                ide_guard.take().unwrap(),
            )
            .await,
        );
        record.image = Some(choice.used.clone());
        record.runtime_owned = false;
        record.lifecycle = Lifecycle::Recycling;
        state.sessions.put(record.clone());
        if state.try_persist().is_err() {
            return failure(&state, &id, Failure::PersistFailed);
        }
        let vm = record.container();
        let tok = record.token.as_deref().unwrap();
        if rt
            .create_container_profile(
                &vm,
                &record.project,
                "session",
                &id,
                tok,
                &choice.used,
                &[],
                record
                    .binding
                    .as_ref()
                    .and_then(|b| b.ide.as_ref())
                    .map(|b| b.profile_volume.as_str()),
            )
            .is_err()
        {
            return failure(&state, &id, Failure::CreateFailed);
        }
        record.runtime_owned = true;
        state.sessions.put(record.clone());
        if state.try_persist().is_err() {
            return failure(&state, &id, Failure::PersistFailed);
        }
        if rt
            .wait_healthy(&state.sessions.http, &vm, tok)
            .await
            .is_err()
        {
            return failure(&state, &id, Failure::BootFailed);
        }
        record.head = match rt
            .boot_workspace(
                &state.sessions.http,
                &vm,
                &record.project,
                tok,
                &record.branch,
                true,
            )
            .await
        {
            Ok(h) => h,
            Err(_) => return failure(&state, &id, Failure::BootFailed),
        };
        image = Some(choice);
    }
    let _session = ide_guard.take().expect("session generation guard retained");
    record.lifecycle = Lifecycle::Active;
    record.error = None;
    state.sessions.put(record.clone());
    state.events.record(
        &record.project,
        now_epoch(),
        Event::SessionRecycled {
            session_id: id.clone(),
            sha: record.head.clone(),
        },
    );
    if state.try_persist().is_err() {
        return failure(&state, &id, Failure::PersistFailed);
    }
    Json(json!({"session_id":id,"project":record.project,"branch":record.branch,"token":record.token,"endpoint":record.binding.as_ref().map(|b|&b.endpoint),"sha_at_recycle":record.head,"flushed":true,"image":image,"generation":record.generation,"state":record.lifecycle})).into_response()
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
        assert_ne!(b["session_id"], json!(id.clone()));
        let id = b["session_id"].as_str().unwrap().to_string();
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
