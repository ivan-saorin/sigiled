//! Native GitHub template eligibility and durable, revision-pinned creation.
use crate::{
    auth::{self, Action, Actor, Role},
    github::GitHub,
    project::{self, NewProject, ProjectRecord},
    template_snapshot::{self, Snapshot},
    AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Intent {
    pub name: String,
    pub operation: String,
    pub actor: String,
    pub repository: String,
    pub source_repository: String,
    pub source_repository_id: u64,
    pub source_commit: String,
    pub requested_ref: Option<String>,
    pub explicit_template: bool,
    pub created_at: u64,
    pub destination_id: Option<u64>,
    pub snapshot: Option<Snapshot>,
    pub complete: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Eligibility {
    pub enabled: bool,
}

pub fn error(status: StatusCode, message: impl AsRef<str>) -> Response {
    let message = message.as_ref();
    (
        status,
        Json(json!({"error":message.split(':').next().unwrap_or(message),"detail":message})),
    )
        .into_response()
}
fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("GitHub client")
}
fn authorized(actor: &Actor, state: &AppState) -> bool {
    auth::authorize(
        actor,
        Action::ProjectsNew,
        None,
        &state.auth.approvals,
        auth::now_epoch(),
    )
    .is_ok()
}

impl GitHub {
    pub(crate) async fn template_json(
        &self,
        http: &reqwest::Client,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let resolving_revision = method == reqwest::Method::GET
            && path.starts_with("/repos/")
            && path.split('/').nth(4) == Some("commits");
        let mut request = self.req(http, method, path);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| "template_provider_unavailable")?;
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                // GitHub uses 422 for an unknown ref and 409 for an empty
                // repository. No destination exists at this resolution stage.
                404 | 409 | 422 if resolving_revision => {
                    "template_revision_unavailable: choose an existing branch, tag or full commit SHA"
                }
                401 | 403 | 404 => {
                    "template_unavailable: repository, revision or provider permission unavailable"
                }
                409 | 422 => {
                    "template_provider_conflict: inspect the same destination before retrying"
                }
                _ => "template_provider_unavailable: retry the same request",
            }
            .into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "template_provider_unavailable")?
        {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err("template_provider_response_too_large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| "template_provider_response_invalid".into())
    }
    async fn repository(&self, http: &reqwest::Client, name: &str) -> Result<Value, String> {
        let repo = self
            .template_json(
                http,
                reqwest::Method::GET,
                &format!("/repos/{}/{name}", self.owner),
                None,
            )
            .await?;
        if repo["full_name"].as_str() != Some(format!("{}/{name}", self.owner).as_str())
            || repo["id"].as_u64().is_none()
        {
            return Err("template_repository_identity_mismatch".into());
        }
        Ok(repo)
    }
    fn git_url(&self, repo: &Value) -> Result<String, String> {
        let expected = format!(
            "https://github.com/{}.git",
            repo["full_name"]
                .as_str()
                .ok_or("template_repository_identity_mismatch")?
        );
        #[cfg(test)]
        if let Some(url) = repo["clone_url"]
            .as_str()
            .filter(|s| s.starts_with("file://"))
        {
            return Ok(url.into());
        }
        if repo["clone_url"].as_str() != Some(expected.as_str()) {
            return Err("template_repository_url_invalid".into());
        }
        Ok(expected)
    }
    async fn resolve(
        &self,
        http: &reqwest::Client,
        repo: &Value,
        reference: Option<&str>,
    ) -> Result<String, String> {
        let reference = reference
            .or_else(|| repo["default_branch"].as_str())
            .ok_or("template_revision_missing")?;
        if reference.is_empty() || reference.len() > 200 || reference.chars().any(char::is_control)
        {
            return Err("template_revision_invalid".into());
        }
        let mut url = reqwest::Url::parse("https://github.com").unwrap();
        url.path_segments_mut().unwrap().extend([
            "repos",
            &self.owner,
            repo["name"]
                .as_str()
                .ok_or("template_repository_identity_mismatch")?,
            "commits",
            reference,
        ]);
        let commit = self
            .template_json(http, reqwest::Method::GET, url.path(), None)
            .await?;
        let sha = commit["sha"]
            .as_str()
            .filter(|s| template_snapshot::sha(s))
            .ok_or("template_revision_invalid")?;
        Ok(sha.into())
    }
}

pub async fn list(_actor: Actor, State(state): State<AppState>) -> Response {
    let Some(gh) = state.github else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "project_creation_not_configured",
        );
    };
    let http = http();
    let mut templates = Vec::new();
    for page in 1..=20 {
        let rows = match gh
            .template_json(
                &http,
                reqwest::Method::GET,
                &format!("/user/repos?per_page=100&page={page}"),
                None,
            )
            .await
        {
            Ok(Value::Array(rows)) => rows,
            _ => return error(StatusCode::BAD_GATEWAY, "template_discovery_unavailable"),
        };
        for repo in &rows {
            if repo["owner"]["login"].as_str() == Some(&gh.owner)
                && repo["is_template"] == true
                && repo["archived"] != true
                && repo["disabled"] != true
            {
                if let Some(name) = repo["name"].as_str().filter(|n| project::valid_name(n)) {
                    templates.push(json!({"name":name,"repository":format!("{}/{name}",gh.owner),"default_ref":repo["default_branch"],"default":name==gh.template}));
                }
            }
        }
        if rows.len() < 100 {
            templates.sort_by_key(|t| t["name"].as_str().unwrap_or("").to_owned());
            return Json(json!({"templates":templates,"default":gh.template,"scope":"configured_github_owner"})).into_response();
        }
    }
    error(
        StatusCode::BAD_GATEWAY,
        "template_discovery_limit: provider inventory exceeds 2000 repositories",
    )
}

pub async fn designate(
    actor: Actor,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<Eligibility>,
) -> Response {
    if !authorized(&actor, &state) {
        return error(StatusCode::FORBIDDEN, "project_creation_approval_required");
    }
    if !project::valid_name(&name) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_template_name");
    }
    let Some(gh) = state.github.clone() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "project_creation_not_configured",
        );
    };
    // Survives the browser's request deadline; serialize metadata changes.
    match tokio::spawn(async move {
        let lock = state.sessions.merge_lock(&name);
        let _guard = lock.lock_owned().await;
        let http = http();
        let repo = gh.repository(&http, &name).await?;
        if repo["permissions"]["admin"] != true {
            return Err("template_management_forbidden".to_string());
        }
        if body.enabled {
            let commit = gh.resolve(&http, &repo, None).await?;
            let intent = new_intent(&actor, &gh, &name, &name, None, true, &repo, commit);
            let root = gh
                .keys_dir
                .parent()
                .ok_or("template_staging_unavailable")?
                .join("template-staging")
                .join(&intent.operation);
            let source = gh.git_url(&repo)?;
            let pat = gh.pat.clone();
            tokio::task::spawn_blocking(move || {
                template_snapshot::prepare(&root, &source, &pat, &intent)
            })
            .await
            .map_err(|_| "template_preparation_interrupted")??;
        }
        gh.template_json(
            &http,
            reqwest::Method::PATCH,
            &format!("/repos/{}/{name}", gh.owner),
            Some(json!({"is_template":body.enabled})),
        )
        .await?;
        let verified = gh.repository(&http, &name).await?;
        if verified["id"] != repo["id"] || verified["is_template"] != body.enabled {
            return Err("template_designation_unconfirmed".into());
        }
        Ok::<_, String>(json!({"name":name,"enabled":body.enabled}))
    })
    .await
    {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(e)) => error(StatusCode::UNPROCESSABLE_ENTITY, e),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "template_designation_interrupted",
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn new_intent(
    actor: &Actor,
    gh: &GitHub,
    name: &str,
    template: &str,
    reference: Option<String>,
    explicit: bool,
    repo: &Value,
    commit: String,
) -> Intent {
    Intent {
        name: name.into(),
        operation: crate::sessions::SessionState::session_id(),
        actor: actor.driver.clone(),
        repository: format!("{}/{name}", gh.owner),
        source_repository: format!("{}/{template}", gh.owner),
        source_repository_id: repo["id"].as_u64().unwrap(),
        source_commit: commit,
        requested_ref: reference,
        explicit_template: explicit,
        created_at: auth::now_epoch(),
        destination_id: None,
        snapshot: None,
        complete: false,
    }
}
fn save(state: &AppState, intent: &Intent) -> Result<(), String> {
    state
        .registry
        .creations
        .write()
        .unwrap()
        .insert(intent.name.clone(), intent.clone());
    state
        .try_persist()
        .map_err(|_| "project_creation_save_uncertain: retry the same name and selection".into())
}

pub async fn create(actor: Actor, state: AppState, body: NewProject) -> Response {
    if !authorized(&actor, &state) {
        return error(StatusCode::FORBIDDEN, "project_creation_approval_required");
    }
    if !project::valid_name(&body.name)
        || body
            .template
            .as_ref()
            .is_some_and(|n| !project::valid_name(n))
        || body
            .template_ref
            .as_ref()
            .is_some_and(|r| r.is_empty() || r.len() > 200 || r.chars().any(char::is_control))
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_project_or_template_selection",
        );
    }
    // The owned worker retains the destination lock after a disconnected caller.
    match tokio::spawn(async move { create_inner(actor, state, body).await }).await {
        Ok(Ok(result)) => (
            if result["existing"] == true {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            },
            Json(result),
        )
            .into_response(),
        Ok(Err((status, detail))) => error(status, detail),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "project_creation_interrupted: retry the same name",
        ),
    }
}
type Failure = (StatusCode, String);
fn failed(e: impl Into<String>) -> Failure {
    (StatusCode::BAD_GATEWAY, e.into())
}

async fn create_inner(actor: Actor, state: AppState, body: NewProject) -> Result<Value, Failure> {
    let Some(gh) = state.github.clone() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "project_creation_not_configured".into(),
        ));
    };
    let lock = state.sessions.merge_lock(&body.name);
    let _guard = lock.lock_owned().await;
    let template = body.template.as_deref().unwrap_or(&gh.template);
    if template == body.name {
        return Err((StatusCode::CONFLICT, "destination_is_template".into()));
    }
    let http = http();
    let existing = state
        .registry
        .creations
        .read()
        .unwrap()
        .get(&body.name)
        .cloned();
    let mut intent = if let Some(intent) = existing {
        if intent.actor != actor.driver && actor.role != Role::Admin {
            return Err((
                StatusCode::FORBIDDEN,
                "project_creation_owned_by_another_actor".into(),
            ));
        }
        if intent.source_repository != format!("{}/{template}", gh.owner)
            || intent.requested_ref != body.template_ref
            || intent.explicit_template != body.template.is_some()
        {
            return Err((
                StatusCode::CONFLICT,
                "project_creation_selection_conflict: retry the original template and ref".into(),
            ));
        }
        if intent.complete {
            return Ok(
                json!({"name":intent.name,"repo":intent.repository,"existing":true,"adopted":false,"provenance":intent}),
            );
        }
        intent
    } else {
        if state.registry.contains(&body.name) {
            return Err((StatusCode::CONFLICT, "project_already_registered".into()));
        }
        // Source authorization is the existing shared-owner project read policy;
        // private availability is additionally enforced by the configured PAT.
        let source = gh.repository(&http, template).await.map_err(failed)?;
        if source["is_template"] != true || source["archived"] == true || source["disabled"] == true
        {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "repository_not_an_eligible_template".into(),
            ));
        }
        let commit = gh
            .resolve(&http, &source, body.template_ref.as_deref())
            .await
            .map_err(|e| {
                if e.starts_with("template_revision_") {
                    (StatusCode::UNPROCESSABLE_ENTITY, e)
                } else {
                    failed(e)
                }
            })?;
        new_intent(
            &actor,
            &gh,
            &body.name,
            template,
            body.template_ref.clone(),
            body.template.is_some(),
            &source,
            commit,
        )
    };
    // Retry never resolves the moving ref again; recheck current source access
    // and immutable provider identity, including revocation of eligibility.
    let source = gh.repository(&http, template).await.map_err(failed)?;
    if source["id"].as_u64() != Some(intent.source_repository_id)
        || source["is_template"] != true
        || source["archived"] == true
        || source["disabled"] == true
    {
        return Err((
            StatusCode::FORBIDDEN,
            "template_access_or_eligibility_changed".into(),
        ));
    }
    // A killed Git process can leave locks. Each attempt owns fresh staging;
    // never remove another attempt's files or weaken its process custody.
    let root = gh
        .keys_dir
        .parent()
        .ok_or_else(|| failed("template_staging_unavailable"))?
        .join("template-staging")
        .join(&intent.operation)
        .join(crate::sessions::SessionState::session_id());
    let source_url = gh.git_url(&source).map_err(failed)?;
    let clone = intent.clone();
    let pat = gh.pat.clone();
    let dir = root.clone();
    let snapshot = tokio::task::spawn_blocking(move || {
        template_snapshot::prepare(&dir, &source_url, &pat, &clone)
    })
    .await
    .map_err(|_| failed("template_preparation_interrupted"))?
    .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    if intent.snapshot.as_ref().is_some_and(|s| s != &snapshot) {
        return Err((StatusCode::CONFLICT, "template_snapshot_changed".into()));
    }
    intent.snapshot = Some(snapshot.clone());
    save(&state, &intent).map_err(failed)?;
    let marker = format!("SIGILED provisioning {}", intent.operation);
    // Probe distinguishes absence from denial; only confirmed absence permits
    // creation. An ambiguous provider response resumes through this exact marker.
    let probe = gh
        .req(
            &http,
            reqwest::Method::GET,
            &format!("/repos/{}", intent.repository),
        )
        .send()
        .await
        .map_err(|_| failed("destination_probe_unavailable"))?;
    let destination =
        if probe.status() == reqwest::StatusCode::NOT_FOUND && intent.destination_id.is_none() {
            let user = gh
                .template_json(
                    &http,
                    reqwest::Method::GET,
                    &format!("/users/{}", gh.owner),
                    None,
                )
                .await
                .map_err(failed)?;
            let path = if user["type"] == "Organization" {
                format!("/orgs/{}/repos", gh.owner)
            } else {
                let principal = gh
                    .template_json(&http, reqwest::Method::GET, "/user", None)
                    .await
                    .map_err(failed)?;
                if principal["login"].as_str() != Some(&gh.owner) {
                    return Err((
                        StatusCode::FORBIDDEN,
                        "destination_owner_not_authorized".into(),
                    ));
                }
                "/user/repos".into()
            };
            gh.template_json(
            &http,
            reqwest::Method::POST,
            &path,
            Some(json!({"name":intent.name,"description":marker,"private":true,"auto_init":false})),
        )
        .await
        .map_err(failed)?
        } else if probe.status().is_success() {
            probe
                .json::<Value>()
                .await
                .map_err(|_| failed("destination_probe_invalid"))?
        } else {
            return Err(failed(
                "destination_probe_unavailable: retry the same name; no repository was adopted",
            ));
        };
    let id = destination["id"]
        .as_u64()
        .ok_or_else(|| failed("destination_identity_invalid"))?;
    if destination["full_name"].as_str() != Some(&intent.repository)
        || destination["private"] != true
        || destination["description"] != marker
        || intent.destination_id.is_some_and(|expected| expected != id)
    {
        return Err((
            StatusCode::CONFLICT,
            "destination_exists_or_changed: incumbent repository preserved; choose a new name"
                .into(),
        ));
    }
    intent.destination_id = Some(id);
    save(&state, &intent).map_err(failed)?;
    let destination_url = gh.git_url(&destination).map_err(failed)?;
    let pat = gh.pat.clone();
    let commit = snapshot.commit.clone();
    tokio::task::spawn_blocking(move || {
        template_snapshot::publish(&root, &destination_url, &pat, &commit)
    })
    .await
    .map_err(|_| failed("template_publish_interrupted"))?
    .map_err(failed)?;
    let verified = gh.repository(&http, &intent.name).await.map_err(failed)?;
    if verified["id"].as_u64() != Some(id) {
        return Err((StatusCode::CONFLICT, "destination_identity_changed".into()));
    }
    // An account may default empty repositories to main; normal SIGIL lifecycle
    // deliberately uses master. Change only this verified operation-owned repo.
    if verified["default_branch"] != "master" {
        gh.template_json(
            &http,
            reqwest::Method::PATCH,
            &format!("/repos/{}", intent.repository),
            Some(json!({"default_branch":"master"})),
        )
        .await
        .map_err(failed)?;
        let checked = gh.repository(&http, &intent.name).await.map_err(failed)?;
        if checked["id"].as_u64() != Some(id) || checked["default_branch"] != "master" {
            return Err(failed(
                "destination_default_branch_unconfirmed: retry the same request",
            ));
        }
    }
    let pubkey = gh.generate_deploy_key(&intent.name).map_err(failed)?;
    gh.add_deploy_key(&http, &intent.name, &pubkey)
        .await
        .map_err(|_| failed("destination_key_installation_uncertain: retry the same name"))?;
    // Creation cannot report success using the session repair fallback image.
    // Use the normal runtime resolver under this operation's mirror lock.
    if let Some(rt) = state.sessions.runtime.clone() {
        let name = intent.name.clone();
        let expected = snapshot.commit.clone();
        tokio::task::spawn_blocking(move || {
            let repo=rt.ensure_mirror(&name).map_err(|_|"template_runtime_checkout_failed: retry the same request")?;
            let head=crate::merge::git(&repo,&["rev-parse","HEAD"]).map_err(|_|"template_runtime_checkout_failed")?;
            if head.trim()!=expected {return Err("destination_changed: runtime checkout differs from the prepared snapshot");}
            if rt.ensure_session_image(&name,&repo).build_error.is_some() {
                return Err("template_workspace_build_failed: repair the reusable Dockerfile and create with a new name, or fix runtime connectivity and retry this request");
            }
            Ok(())
        }).await.map_err(|_|failed("template_runtime_validation_interrupted"))?.map_err(failed)?;
    }
    let manifest = crate::manifest::Manifest::parse(
        &snapshot
            .foundation
            .as_ref()
            .map(|f| format!("template = {f:?}\n"))
            .unwrap_or_default(),
    )
    .map_err(|_| failed("template_foundation_invalid"))?;
    let record = ProjectRecord::new(&intent.name, &manifest, state.registry.latest_template());
    let first = !state.registry.contains(&intent.name);
    state.registry.insert(record.clone());
    if first {
        state.events.record(
            &intent.name,
            auth::now_epoch(),
            crate::events::Event::ProjectCreated {
                repo: intent.repository.clone(),
                adopted: false,
            },
        );
    }
    intent.complete = true;
    if let Err(e) = save(&state, &intent) {
        intent.complete = false;
        state
            .registry
            .creations
            .write()
            .unwrap()
            .insert(intent.name.clone(), intent);
        return Err(failed(e));
    }
    Ok(
        json!({"name":record.name,"repo":intent.repository,"adopted":false,"template_version":record.template_version,"template_behind":record.template_behind,"provenance":intent}),
    )
}
