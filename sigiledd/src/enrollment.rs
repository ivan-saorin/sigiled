//! Durable accepted-memory intent lives in the existing project descriptor.
use crate::{
    enrollment_contract::{digest, Document, Snapshot, MAX_BODY},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrollment {
    pub namespace: Option<String>,
    pub owner: String,
    pub revision: String,
    pub desired_commit: Option<String>,
    pub state: String,
    pub actor: Option<String>,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub retry_at: Option<u64>,
    pub snapshot: Option<Snapshot>,
    pub confirmed: Option<Value>,
    #[serde(default)]
    pub handoffs: Vec<Value>,
}
impl Default for Enrollment {
    fn default() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes).expect("namespace entropy");
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        Self {
            namespace: None,
            owner: format!(
                "{}-{}-{}-{}-{}",
                &hex[..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..]
            ),
            revision: "0".into(),
            desired_commit: None,
            state: "awaiting_authorization".into(),
            actor: None,
            attempts: 0,
            last_error: None,
            retry_at: None,
            snapshot: None,
            confirmed: None,
            handoffs: vec![],
        }
    }
}
impl Enrollment {
    pub fn observe(&mut self, commit: &str, enabled: bool) {
        if self.state == "persistence_uncertain" {
            return;
        }
        if self.desired_commit.as_deref() != Some(commit) {
            self.revision = self
                .revision
                .parse::<i64>()
                .ok()
                .and_then(|n| n.checked_add(1))
                .expect("memory revision exhausted")
                .to_string();
            self.desired_commit = Some(commit.into());
            self.snapshot = None;
            self.state = "awaiting_authorization".into();
            self.last_error = None;
            self.retry_at = None;
        }
        if !enabled {
            self.state = "disabled".into();
        }
    }
    pub fn public(&self) -> Value {
        json!({"namespace":self.namespace,"state":self.state,"owner":self.owner,"revision":self.revision,"desired_commit":self.desired_commit,"actor":self.actor,"attempts":self.attempts,"last_error":self.last_error,"retry_at":self.retry_at,"confirmed":self.confirmed,"handoffs":self.handoffs,"sharing":"project_only_unless_explicit_mem0","access":"authenticated_service_policy"})
    }
}
fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error":code}))).into_response()
}
fn repo(state: &AppState, project: &str) -> Result<(PathBuf, String), &'static str> {
    if !state.registry.contains(project) {
        return Err("unknown_project");
    }
    let owner = state
        .sessions
        .runtime
        .as_ref()
        .map(|r| r.owner.as_str())
        .or_else(|| state.github.as_ref().map(|g| g.owner.as_str()))
        .ok_or("canonical_repository_unavailable")?;
    let path = state
        .sessions
        .runtime
        .as_ref()
        .map(|r| r.repo_path(project))
        .or_else(|| state.sessions.repos_dir.as_ref().map(|p| p.join(project)))
        .ok_or("canonical_repository_unavailable")?;
    Ok((path, format!("{owner}/{project}")))
}
fn git_bytes(
    repo: &std::path::Path,
    args: &[&str],
    deadline: Instant,
) -> Result<Vec<u8>, &'static str> {
    let output = crate::bounded_process::output_until(
        crate::bounded_process::git_command()
            .arg("-C")
            .arg(repo)
            .args(args),
        deadline,
    )
    .map_err(|_| "accepted_repository_read_failed")?;
    if !output.status.success() {
        return Err("accepted_repository_read_failed");
    }
    Ok(output.stdout)
}
fn git_text(
    repo: &std::path::Path,
    args: &[&str],
    deadline: Instant,
) -> Result<String, &'static str> {
    String::from_utf8(git_bytes(repo, args, deadline)?).map_err(|_| "non_utf8_repository_document")
}
fn binding(
    state: &AppState,
    project: &str,
    deadline: Instant,
) -> Result<(PathBuf, String, String), &'static str> {
    let (path, repository) = repo(state, project)?;
    let origin = git_text(&path, &["remote", "get-url", "origin"], deadline)?;
    if ![
        format!("git@github.com:{repository}.git"),
        format!("https://github.com/{repository}.git"),
    ]
    .contains(&origin.trim().to_string())
    {
        return Err("canonical_repository_mismatch");
    }
    let commit = git_text(&path, &["rev-parse", "master^{commit}"], deadline)?
        .trim()
        .to_string();
    // Production master is accepted only after remote publication, not a failed local close.
    if state.sessions.runtime.is_some() {
        let remote = git_text(
            &path,
            &["rev-parse", "refs/remotes/origin/master^{commit}"],
            deadline,
        )?
        .trim()
        .to_string();
        if remote != commit {
            return Err("master_acceptance_not_confirmed");
        }
    }
    Ok((path, repository, commit))
}
fn selected(path: &str, policy: &crate::declaration::Memory) -> bool {
    let docs = path == "README.md" || path == "README.rst" || path.starts_with("docs/");
    let document = [".md", ".rst", ".txt"]
        .iter()
        .any(|ext| path.ends_with(ext));
    let source = policy.source_code
        && [
            ".rs", ".py", ".js", ".ts", ".tsx", ".jsx", ".go", ".java", ".c", ".h", ".cpp", ".sh",
            ".toml",
        ]
        .iter()
        .any(|ext| path.ends_with(ext));
    (document || source)
        && if policy.sources.is_empty() {
            docs && document
        } else {
            policy
                .sources
                .iter()
                .any(|s| path == s || path.starts_with(&format!("{s}/")))
        }
}
fn collect(
    state: &AppState,
    project: &str,
    e: &Enrollment,
    deadline: Instant,
) -> Result<Snapshot, &'static str> {
    let (path, repository, commit) = binding(state, project, deadline)?;
    if e.desired_commit.as_deref() != Some(&commit) {
        return Err("accepted_revision_changed");
    }
    let policy = state.registry.descriptor(project).declaration.memory;
    let tree = git_bytes(&path, &["ls-tree", "-r", "-z", "-l", &commit], deadline)?;
    let mut documents = vec![];
    for entry in tree.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let entry = std::str::from_utf8(entry).map_err(|_| "unsafe_repository_path")?;
        let (meta, name) = entry.split_once('\t').ok_or("invalid_git_tree")?;
        if !selected(name, &policy) {
            continue;
        }
        let parts: Vec<_> = meta.split_whitespace().collect();
        if parts.len() != 4
            || !["100644", "100755"].contains(&parts[0])
            || parts[1] != "blob"
            || parts[3].parse::<usize>().map_err(|_| "invalid_blob_size")? > 524288
        {
            return Err("unsupported_or_oversized_document");
        }
        if documents
            .iter()
            .map(|d: &Document| d.text.len())
            .sum::<usize>()
            + parts[3].parse::<usize>().unwrap()
            > MAX_BODY
        {
            return Err("documentation_batch_too_large");
        }
        let text = git_text(&path, &["cat-file", "blob", parts[2]], deadline)?;
        documents.push(Document {
            path: name.into(),
            sha: digest(text.as_bytes()),
            text,
        });
        if documents.len() > 64 {
            return Err("documentation_batch_too_large");
        }
    }
    documents.sort_by(|a, b| a.path.cmp(&b.path));
    let snapshot = Snapshot {
        project: project.into(),
        repository,
        owner: e.owner.clone(),
        revision: e.revision.clone(),
        commit,
        documents,
        shared: policy.sharing.as_deref() == Some("mem0"),
    };
    if !snapshot.validate()
        || serde_json::to_vec(&snapshot)
            .map_err(|_| "invalid_document_batch")?
            .len()
            > MAX_BODY
    {
        return Err("encoded_document_batch_too_large_or_unsafe");
    }
    Ok(snapshot)
}
fn update(state: &AppState, p: &str, f: impl FnOnce(&mut Enrollment)) -> Result<(), &'static str> {
    {
        let mut all = state.registry.descriptors.write().unwrap();
        let d = all.get_mut(p).ok_or("unknown_project")?;
        f(d.memory_enrollment.get_or_insert_with(Default::default));
    }
    if state.try_persist().is_err() {
        let mut all = state.registry.descriptors.write().unwrap();
        if let Some(e) = all.get_mut(p).and_then(|d| d.memory_enrollment.as_mut()) {
            e.state = "persistence_uncertain".into();
        }
        return Err("enrollment_persistence_uncertain");
    }
    Ok(())
}
fn fail(state: &AppState, p: &str, revision: &str, phase: &str, code: &str) {
    let _ = update(state, p, |e| {
        if e.revision == revision {
            e.state = phase.into();
            e.last_error = Some(code.into());
            e.retry_at = Some(crate::auth::now_epoch() + 30);
        }
    });
}
pub(crate) fn schedule(state: AppState, project: String, actor: String, token: String) {
    tokio::spawn(async move {
        let _ = reconcile(state, project, actor, Some(token)).await;
    });
}
pub async fn reconcile(
    state: AppState,
    project: String,
    actor: String,
    token: Option<String>,
) -> Result<Value, &'static str> {
    reconcile_using(
        state,
        project,
        actor,
        token,
        crate::browser::memory::Service::default(),
    )
    .await
}
async fn reconcile_using(
    state: AppState,
    project: String,
    actor: String,
    token: Option<String>,
    svc: crate::browser::memory::Service,
) -> Result<Value, &'static str> {
    if !state.store.durable() {
        return Err("enrollment_durable_state_required");
    }
    // Refresh under A2's retained bounded-worker ownership, then acquire our own bounded snapshot read.
    crate::ecosystem::refresh(&state, &project)
        .await
        .map_err(|_| "accepted_repository_unavailable")?;
    let lock = state.sessions.merge_lock(&project);
    let guard = tokio::time::timeout(Duration::from_secs(2), lock.lock_owned())
        .await
        .map_err(|_| "enrollment_busy")?;
    let e = state
        .registry
        .descriptor(&project)
        .memory_enrollment
        .ok_or("enrollment_missing")?;
    if e.state == "persistence_uncertain" {
        return Err("enrollment_persistence_uncertain");
    }
    if !state
        .registry
        .descriptor(&project)
        .declaration
        .memory
        .enabled
    {
        return Ok(e.public());
    }
    let Some(token) = token.filter(|s| !s.is_empty()) else {
        drop(guard);
        update(&state, &project, |e| {
            e.state = "awaiting_authorization".into()
        })?;
        return Ok(state
            .registry
            .descriptor(&project)
            .memory_enrollment
            .unwrap()
            .public());
    };
    let worker_state = state.clone();
    let worker_project = project.clone();
    let revision = e.revision.clone();
    let snapshot = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let result = collect(
            &worker_state,
            &worker_project,
            &e,
            Instant::now() + Duration::from_secs(8),
        );
        if let Ok(ref snapshot) = result {
            // The exact bounded payload is durable before any Memory call, never a bearer.
            let bytes: usize = worker_state
                .registry
                .descriptors()
                .iter()
                .filter(|(name, _)| *name != &worker_project)
                .filter_map(|(_, d)| d.memory_enrollment.as_ref()?.snapshot.as_ref())
                .map(|s| serde_json::to_vec(s).unwrap().len())
                .sum();
            if bytes + serde_json::to_vec(snapshot).unwrap().len() > 32 * 1024 * 1024 {
                return Err("enrollment_storage_limit");
            }
            update(&worker_state, &worker_project, |e| {
                e.snapshot = Some(snapshot.clone());
                e.actor = Some(actor);
                e.attempts = e.attempts.saturating_add(1);
                e.state = "pending".into();
                e.last_error = None;
            })?;
        }
        result
    })
    .await
    .map_err(|_| "enrollment_interrupted")?;
    let snapshot = match snapshot {
        Ok(s) => s,
        Err(code) => {
            fail(&state, &project, &revision, "failed", code);
            return Err(code);
        }
    };
    // No session/project lock is held during this call or the reverse validation.
    let namespace = state
        .registry
        .descriptor(&project)
        .memory_enrollment
        .as_ref()
        .and_then(|e| e.namespace.clone())
        .unwrap_or_else(|| project.clone());
    let outcome = async {
        let (_, cap) = svc
            .call(reqwest::Method::GET, "capabilities", &[], &token, None)
            .await?;
        if cap["enrollment_contract"] != "sigil-accepted-v1"
            || cap["enrollment_validation"] != "fixed_sigil_current_intent_v1"
            || cap["auth"]["oidc_verifier_configured"] != true
        {
            return Err(crate::browser::Error(
                StatusCode::SERVICE_UNAVAILABLE,
                "memory_service_update_or_auth_setup_required",
            ));
        }
        let (_, receipt) = svc
            .call(
                reqwest::Method::POST,
                &format!("idx/{namespace}/enrollment"),
                &[],
                &token,
                Some(serde_json::to_value(&snapshot).unwrap()),
            )
            .await?;
        if receipt["verified"] != true
            || receipt["digest"] != snapshot.digest()
            || receipt["owner"] != snapshot.owner
            || receipt["revision"] != snapshot.revision
            || receipt["commit"] != snapshot.commit
        {
            return Err(crate::browser::Error(
                StatusCode::BAD_GATEWAY,
                "invalid_enrollment_receipt",
            ));
        }
        Ok(receipt)
    }
    .await;
    match outcome {
        Ok(receipt) => {
            update(&state, &project, |e| {
                if e.revision == snapshot.revision {
                    e.state = if receipt["projection"] == "indexed" {
                        "indexed"
                    } else {
                        "projection_pending"
                    }
                    .into();
                    e.confirmed = Some(
                        json!({"index":namespace,"owner":snapshot.owner,"repository":snapshot.repository,"revision":snapshot.revision,"commit":snapshot.commit,"digest":snapshot.digest(),"projection":receipt["projection"],"observed_at":crate::auth::now_epoch(),"verified":true}),
                    );
                    e.retry_at = None;
                }
            })?;
        }
        Err(error) => {
            let phase = match error.0 {
                StatusCode::UNAUTHORIZED => "awaiting_authorization",
                StatusCode::CONFLICT => "ownership_or_revision_conflict",
                StatusCode::SERVICE_UNAVAILABLE => "update_required",
                _ => "unavailable",
            };
            fail(&state, &project, &snapshot.revision, phase, error.1);
        }
    }
    Ok(state
        .registry
        .descriptor(&project)
        .memory_enrollment
        .unwrap()
        .public())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Validation {
    project: String,
    index: String,
    owner: String,
    revision: String,
    commit: String,
    digest: String,
}
pub async fn validate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(v): Json<Validation>,
) -> Response {
    // Authenticate the original OIDC token via existing signature/expiry/service policy; bootstrap never qualifies.
    let mut headers = headers;
    headers.insert(crate::auth::SERVICE_HEADER, "memory".parse().unwrap());
    let auth = crate::auth::verify(State(state.clone()), headers).await;
    if !auth.status().is_success() {
        return auth;
    }
    if !crate::project::valid_name(&v.index) || !crate::project::valid_name(&v.project) {
        return error(StatusCode::BAD_REQUEST, "invalid_project");
    }
    let lock = state.sessions.merge_lock(&v.project);
    let guard = match tokio::time::timeout(Duration::from_secs(2), lock.lock_owned()).await {
        Ok(g) => g,
        Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "accepted_intent_busy"),
    };
    let result=tokio::task::spawn_blocking(move || {let _guard=guard;let e=state.registry.descriptor(&v.project).memory_enrollment.ok_or(())?;if e.state=="persistence_uncertain" {return Err(());}
 let s=e.snapshot.ok_or(())?;
  let (_,repository,commit)=binding(&state,&v.project,Instant::now()+Duration::from_secs(3)).map_err(|_|())?;
  if !state.store.durable() || !state.registry.descriptor(&v.project).declaration.memory.enabled || s.owner!=v.owner || s.project!=v.project || e.namespace.as_deref().unwrap_or(&v.project)!=v.index || s.repository!=repository || s.revision!=v.revision || s.commit!=v.commit || commit!=v.commit || s.digest()!=v.digest {return Err(());}
  Ok(json!({"contract":"sigil-accepted-v1","index":v.index,"owner":v.owner,"revision":v.revision,"commit":v.commit,"digest":v.digest,"verified":true}))
 }).await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => error(StatusCode::CONFLICT, "accepted_intent_changed"),
    }
}
pub async fn status(
    _actor: crate::auth::Actor,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Response {
    if !state.registry.contains(&project) {
        return error(StatusCode::NOT_FOUND, "unknown_project");
    }
    Json(
        state
            .registry
            .descriptor(&project)
            .memory_enrollment
            .map(|e| e.public())
            .unwrap_or(json!({"state":"awaiting_authorization"})),
    )
    .into_response()
}
pub async fn retry(
    actor: crate::auth::Actor,
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
) -> Response {
    if crate::auth::authorize(
        &actor,
        crate::auth::Action::OpenSession,
        Some(&project),
        &state.auth.approvals,
        crate::auth::now_epoch(),
    )
    .is_err()
    {
        return error(StatusCode::FORBIDDEN, "project_authorization_required");
    }
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_owned);
    if token
        .as_deref()
        .is_some_and(|t| state.auth.config.bootstrap_bearer.as_deref() == Some(t))
    {
        return error(StatusCode::FORBIDDEN, "original_oidc_context_required");
    }
    match reconcile(state, project, actor.driver, token).await {
        Ok(v) => Json(v).into_response(),
        Err(code) => error(StatusCode::SERVICE_UNAVAILABLE, code),
    }
}

pub(crate) fn schedule_headers(
    state: AppState,
    project: String,
    actor: String,
    headers: &HeaderMap,
) {
    let Some(token) = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    if state.auth.config.bootstrap_bearer.as_deref() == Some(token) {
        return;
    }
    schedule(state, project, actor, token.into());
}

pub async fn separate_namespace(state: &AppState, project: &str) -> Result<(), &'static str> {
    let lock = state.sessions.merge_lock(project);
    let _guard = lock.lock().await;
    let e = state
        .registry
        .descriptor(project)
        .memory_enrollment
        .ok_or("enrollment_missing")?;
    if e.state != "ownership_or_revision_conflict" || e.confirmed.is_some() {
        return Err("namespace_correction_requires_unclaimed_conflict");
    }
    let prefix = &project[..project.len().min(29)];
    let namespace = format!("{}-{}", prefix, &e.owner[..8]);
    if e.namespace.as_deref() == Some(&namespace) {
        return Err("separate_namespace_also_conflicts_operator_action_required");
    }
    update(state, project, |e| {
        e.namespace = Some(namespace);
        e.revision = (e.revision.parse::<i64>().unwrap() + 1).to_string();
        e.snapshot = None;
        e.state = "awaiting_authorization".into();
        e.last_error = None;
    })
}
pub async fn source_edit(
    state: &AppState,
    svc: &crate::browser::memory::Service,
    index: &str,
    chunk: &Value,
    token: &str,
) -> Value {
    let unavailable = || json!({"verified":false,"state":"association_unavailable","current_checkout":"checked_on_launch"});
    let Some((project, e)) = state.registry.descriptors().into_iter().find_map(|(p, d)| {
        d.memory_enrollment
            .filter(|e| e.namespace.as_deref().unwrap_or(&p) == index)
            .map(|e| (p, e))
    }) else {
        return unavailable();
    };
    let Some(snapshot) = e.snapshot else {
        return unavailable();
    };
    if e.state != "indexed"
        || chunk["source"] != "git"
        || chunk["ref"] != snapshot.repository
        || !chunk["id"].as_str().is_some_and(|id| id.starts_with("e_"))
    {
        return unavailable();
    }
    let Some(doc) = snapshot
        .documents
        .iter()
        .find(|d| chunk["path"] == d.path && chunk["sha"] == d.sha)
    else {
        return unavailable();
    };
    let Ok((_, cap)) = svc
        .call(reqwest::Method::GET, "capabilities", &[], token, None)
        .await
    else {
        return unavailable();
    };
    if cap["enrollment_contract"] != "sigil-accepted-v1"
        || cap["auth"]["oidc_verifier_configured"] != true
    {
        return unavailable();
    }
    let Ok((_, observed)) = svc
        .call(
            reqwest::Method::GET,
            &format!("idx/{index}/enrollment"),
            &[],
            token,
            None,
        )
        .await
    else {
        return unavailable();
    };
    if observed["deleted"] != false
        || observed["verified"] != true
        || observed["projection"] != "indexed"
        || observed["owner"] != snapshot.owner
        || observed["revision"] != snapshot.revision
        || observed["digest"] != snapshot.digest()
        || observed["commit"] != snapshot.commit
        || observed["repository"] != snapshot.repository
        || observed["project"] != project
    {
        return unavailable();
    }
    let lock = state.sessions.merge_lock(&project);
    let Ok(guard) = tokio::time::timeout(Duration::from_secs(2), lock.lock_owned()).await else {
        return unavailable();
    };
    let state = state.clone();
    let checked_project = project.clone();
    let checked = snapshot.clone();
    let actual = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let deadline = Instant::now() + Duration::from_secs(3);
        let (path, repository, current) = binding(&state, &checked_project, deadline)?;
        if repository != checked.repository {
            return Err("repository_changed");
        }
        git_bytes(
            &path,
            &["merge-base", "--is-ancestor", &checked.commit, &current],
            deadline,
        )?;
        Ok(current)
    })
    .await;
    let Ok(Ok(current)) = actual else {
        return unavailable();
    };
    json!({"verified":true,"project":project,"path":doc.path,"indexed_commit":snapshot.commit,"current_accepted_commit":current,"current_checkout":"checked_on_launch"})
}

/// Called only after normal close has successfully published master, under its mirror lock.
pub(crate) fn accept_handoff(
    state: &AppState,
    record: &crate::sessions::SessionRecord,
    repo: &std::path::Path,
    accepted: &str,
) -> Result<(), &'static str> {
    let Some(marker) = &record.handoff else {
        return Ok(());
    };
    if marker["phase"] != "complete" {
        return Ok(());
    }
    let Some(commit) = marker["receipt"]["commit"].as_str() else {
        return Ok(());
    };
    let Some(run) = marker["run_id"].as_str() else {
        return Ok(());
    };
    let Some(files) = marker["request"]["files"].as_array() else {
        return Ok(());
    };
    if files.len() != 2 || commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    if git_bytes(
        repo,
        &["merge-base", "--is-ancestor", commit, accepted],
        deadline,
    )
    .is_err()
    {
        return Ok(());
    }
    let mut documents = vec![];
    for file in files {
        let (Some(path), Some(text)) = (file["path"].as_str(), file["content"].as_str()) else {
            return Ok(());
        };
        if !crate::declaration::relative_path(path, false)
            || !path.starts_with("docs/design/")
            || !path.ends_with(".md")
        {
            return Ok(());
        }
        let Ok(actual) = git_text(repo, &["show", &format!("{accepted}:{path}")], deadline) else {
            return Ok(());
        };
        if actual != text {
            return Ok(());
        }
        documents.push(json!({"path":path,"sha":digest(text.as_bytes())}));
    }
    let identity = digest(
        serde_json::to_string(&(&record.project, run, accepted))
            .unwrap()
            .as_bytes(),
    );
    let mut map = state.registry.descriptors.write().unwrap();
    let e = map
        .entry(record.project.clone())
        .or_default()
        .memory_enrollment
        .get_or_insert_with(Default::default);
    if !e.handoffs.iter().any(|r| r["operation_id"] == identity) {
        if e.handoffs.len() >= 256 {
            return Err("accepted_handoff_capacity_reached");
        }
        e.handoffs.push(json!({"operation_id":identity,"run_id":run,"actor":record.actor.driver,"session_commit":commit,"accepted_commit":accepted,"master_accepted":true,"documents":documents}));
    }
    Ok(())
}
pub fn research_receipt(state: &AppState, project: &str, run: &str, actor: &str) -> Option<Value> {
    let e = state.registry.descriptor(project).memory_enrollment?;
    let accepted = e
        .handoffs
        .iter()
        .rev()
        .find(|r| r["run_id"] == run && r["actor"] == actor)?;
    let indexed = e.state == "indexed"
        && e.snapshot.as_ref().is_some_and(|s| {
            accepted["documents"].as_array().is_some_and(|docs| {
                docs.iter().all(|d| {
                    s.documents
                        .iter()
                        .any(|file| d["path"] == file.path && d["sha"] == file.sha)
                })
            })
        });
    Some(
        json!({"phase":"master_accepted","receipt":accepted,"master_accepted":true,"indexed":indexed,"memory_state":e.state,"indexed_commit":if indexed{e.confirmed.as_ref().map(|v|v["commit"].clone())}else{None}}),
    )
}

#[cfg(test)]
#[path = "enrollment_tests.rs"]
mod tests;

pub async fn correct_namespace(
    actor: crate::auth::Actor,
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
) -> Response {
    if crate::auth::authorize(
        &actor,
        crate::auth::Action::OpenSession,
        Some(&project),
        &state.auth.approvals,
        crate::auth::now_epoch(),
    )
    .is_err()
    {
        return error(StatusCode::FORBIDDEN, "project_authorization_required");
    }
    if let Err(code) = separate_namespace(&state, &project).await {
        return error(StatusCode::CONFLICT, code);
    }
    retry(actor, State(state), Path(project), headers).await
}
