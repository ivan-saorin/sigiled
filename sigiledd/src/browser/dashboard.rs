//! Fixed browser adapters. Raw workspace handlers/tokens are never exposed here.
use super::*;
use crate::work_items;
use axum::extract::{Path, Query};
use serde_json::json;

pub(super) async fn shell(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    site(&state, &headers)?;
    Ok((
        [("content-type", "text/html; charset=utf-8")],
        include_str!("assets/index.html"),
    )
        .into_response())
}
pub(super) async fn asset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response, Error> {
    site(&state, &headers)?;
    match name.as_str() {
        "dashboard.js" => Ok((
            [("content-type", "text/javascript; charset=utf-8")],
            include_str!("assets/dashboard.js"),
        )
            .into_response()),
        "dashboard.css" => Ok((
            [("content-type", "text/css; charset=utf-8")],
            include_str!("assets/dashboard.css"),
        )
            .into_response()),
        _ => Err(Error(StatusCode::NOT_FOUND, "asset_not_found")),
    }
}
fn site(state: &AppState, headers: &HeaderMap) -> Result<(), Error> {
    let b = state.browser.inner()?;
    if b.origin(headers)? != b.config.dashboard_origin {
        return Err(Error::forbidden("dashboard_origin_required"));
    }
    Ok(())
}
pub(super) async fn create(
    c: BrowserContext,
    State(state): State<AppState>,
    Json(body): Json<crate::project::NewProject>,
) -> Response {
    // Authorization precedes existing-state recovery, including a two-tab race.
    if crate::auth::authorize(
        &c.actor,
        crate::auth::Action::ProjectsNew,
        Some(&body.name),
        &state.auth.approvals,
        auth::now_epoch(),
    )
    .is_err()
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"project_creation_approval_required"})),
        )
            .into_response();
    }
    let name = body.name.clone();
    let response = crate::project::create(c.actor, State(state.clone()), Json(body)).await;
    match response.status() {
        StatusCode::CREATED=> (StatusCode::CREATED,Json(json!({"name":name,"state":"registered"}))).into_response(),
        StatusCode::CONFLICT if state.registry.contains(&name)=> {
            let lock=state.sessions.merge_lock(&name);let _guard=lock.lock().await;
            if state.try_persist().is_err() {return (StatusCode::SERVICE_UNAVAILABLE,Json(json!({"name":name,"error":"registration_save_uncertain","state":"partial","retry_same_name":true}))).into_response();}
            Json(json!({"name":name,"state":"registered","existing":true})).into_response()
        },
        StatusCode::UNPROCESSABLE_ENTITY=>(StatusCode::UNPROCESSABLE_ENTITY,Json(json!({"error":"invalid_project_name"}))).into_response(),
        status=>(status,Json(json!({"name":name,"error":if state.github.is_none(){"project_creation_not_configured"}else{"provisioning_incomplete"},"state":"partial","retry_same_name":true,"message":"A repository or deploy key may already exist. Retry this name to resume; no rollback was performed."}))).into_response(),
    }
}
fn registered(s: &AppState, p: &str) -> Result<(), work_items::Error> {
    if !s.registry.contains(p) {
        return Err(work_items::Error(StatusCode::NOT_FOUND, "unknown_project"));
    }
    Ok(())
}
pub(super) async fn list_items(
    _c: BrowserContext,
    State(s): State<AppState>,
    Path(p): Path<String>,
    Query(q): Query<crate::overview::Page>,
) -> Response {
    if let Err(e) = registered(&s, &p) {
        return e.into_response();
    }
    let (offset, limit) = match q.bounds() {
        Ok(v) => v,
        Err(e) => return *e,
    };
    match s.work_items.list(&p, offset, limit) {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}
pub(super) async fn get_item(
    _c: BrowserContext,
    State(s): State<AppState>,
    Path((p, id)): Path<(String, String)>,
) -> Response {
    if let Err(e) = registered(&s, &p) {
        return e.into_response();
    }
    match s.work_items.get(&p, &id) {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}
pub(super) async fn create_item(
    c: BrowserContext,
    State(s): State<AppState>,
    Path(p): Path<String>,
    Json(body): Json<work_items::Create>,
) -> Response {
    if let Err(e) = registered(&s, &p) {
        return e.into_response();
    }
    match s.work_items.create(&p, &c.actor.driver, body) {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => e.into_response(),
    }
}
pub(super) async fn update_item(
    c: BrowserContext,
    State(s): State<AppState>,
    Path((p, id)): Path<(String, String)>,
    Json(body): Json<work_items::Update>,
) -> Response {
    if let Err(e) = registered(&s, &p) {
        return e.into_response();
    }
    match s.work_items.update(&p, &id, &c.actor.driver, body) {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}
pub(super) async fn job_runs(
    _c: BrowserContext,
    State(s): State<AppState>,
    Path((p, job)): Path<(String, String)>,
    Query(q): Query<crate::overview::Page>,
) -> Response {
    if let Err(e) = registered(&s, &p) {
        return e.into_response();
    }
    let (offset, limit) = match q.bounds() {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let runs = s.jobs.runs_for(&p, &job);
    let total = runs.len();
    let items:Vec<_>=runs.into_iter().skip(offset).take(limit).map(|r|json!({"job":r.job,"branch":r.branch,"state":r.state,"started_at":r.started_epoch,"finished_at":r.finished_epoch,"exit":r.exit})).collect();
    Json(json!({"items":items,"total":total,"offset":offset,"limit":limit,"next_offset":if offset+limit<total{Some(offset+limit)}else{None}})).into_response()
}
// Exact method/path allowlist. Other routes retain B1's bodyless contract.
pub(super) fn body_limit(method: &Method, path: &str) -> usize {
    let parts: Vec<_> = path.split('/').collect();
    if method == Method::POST && path == "/browser/api/projects" {
        return 1024;
    }
    if parts.len() >= 6
        && parts[1..4] == ["browser", "api", "projects"]
        && crate::project::valid_name(parts[4])
        && parts[5] == "work-items"
        && ((method == Method::POST && parts.len() == 6)
            || (method == Method::PATCH && parts.len() == 7))
    {
        return 16384;
    }
    if method == Method::POST
        && parts.len() == 6
        && parts[1..4] == ["browser", "api", "projects"]
        && crate::project::valid_name(parts[4])
        && parts[5] == "ide"
    {
        return 4096;
    }
    if method == Method::POST
        && parts.len() == 6
        && parts[1..4] == ["browser", "api", "sessions"]
        && matches!(parts[5], "ide" | "preview")
    {
        return 1024;
    }
    if method == Method::POST
        && parts.len() == 6
        && parts[1..4] == ["browser", "api", "projects"]
        && crate::project::valid_name(parts[4])
        && parts[5] == "research"
    {
        return 70000;
    }
    if method == Method::POST && parts.len() == 5 && parts[1..4] == ["browser", "api", "research"] {
        return 262144;
    }
    if method == Method::POST
        && parts.len() == 6
        && parts[1..4] == ["browser", "api", "research"]
        && parts[5] == "handoff"
    {
        return 4096;
    }
    0
}
