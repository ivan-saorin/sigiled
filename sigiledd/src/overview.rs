//! Read-only projections: no Git, Docker, workspace activity, or external requests.
use crate::{auth::Actor, ecosystem::Descriptor, project::ProjectRecord, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}
impl Page {
    fn bounds(&self) -> Result<(usize, usize), Box<Response>> {
        let (offset, limit) = (self.offset.unwrap_or(0), self.limit.unwrap_or(25));
        if !(1..=100).contains(&limit) || offset > 1_000_000 {
            return Err(Box::new(
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({"error":"invalid_pagination"})),
                )
                    .into_response(),
            ));
        }
        Ok((offset, limit))
    }
}
fn page(items: Vec<Value>, offset: usize, limit: usize) -> Value {
    let total = items.len();
    let next = offset.saturating_add(limit);
    json!({"items":items.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"total":total,
        "offset":offset,"limit":limit,"next_offset":if next < total {Some(next)} else {None}})
}
fn capability(desired: bool, status: &str, reason: &str, d: &Descriptor) -> Value {
    json!({"desired":desired,"state":if desired {status} else {"disabled"},"reason":reason,
        "desired_revision":d.desired_revision,"observed_revision":if desired && status=="ready" {d.observed_revision.as_ref()} else {None},
        "observed_at":if desired && status=="ready" {d.observed_at} else {None},
        "retry_at":if reason.ends_with("adapter_not_configured") || reason == "browser_auth_not_configured" {None} else {d.retry_at}})
}
fn capabilities(state: &AppState, d: &Descriptor) -> Value {
    let repo_ready = d.observed_revision.is_some() && d.error.is_none();
    let workspace = if state.sessions.runtime.is_none() {
        "pending"
    } else if repo_ready {
        "ready"
    } else {
        "pending"
    };
    json!({
        "dashboard":capability(true,"ready","read_projection_available",d),
        "activity":capability(true,"ready","recorded_events_available",d),
        "pending":capability(true,"ready","derived_attention_available",d),
        "workspace":capability(true,workspace,if state.sessions.runtime.is_none(){"runtime_not_configured"}else if repo_ready{"mirror_observed_runtime_configured"}else{"repository_pending"},d),
        "ide":capability(d.declaration.ide.enabled,"pending","ide_adapter_not_configured",d),
        "memory":capability(d.declaration.memory.enabled,"pending","memory_adapter_not_configured",d),
        "browser":capability(true,"pending","browser_auth_not_configured",d)
    })
}
/// Full or abbreviated hexadecimal Git object IDs; short app SHA must match head's prefix.
pub fn revision_drift(deployed: Option<&str>, head: Option<&str>) -> Option<bool> {
    match (deployed, head) {
        (Some(a), Some(b))
            if (7..=64).contains(&a.len())
                && (a.len()..=64).contains(&b.len())
                && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_hexdigit()) =>
        {
            Some(!b.to_ascii_lowercase().starts_with(&a.to_ascii_lowercase()))
        }
        _ => None,
    }
}
fn app_summary(state: &AppState, p: &ProjectRecord, d: &Descriptor) -> Value {
    let rec = d
        .app
        .as_ref()
        .and_then(|n| state.apps.get(n))
        .filter(|a| a.project == p.name);
    match rec {
        None => json!({"name":d.app,"state":if d.app.is_some(){"not_deployed"}else{"not_declared"},
            "deployed_revision":null,"image":null,"runtime":{"state":"unknown","observed_at":null,"reason":"not_probed"}}),
        Some(a) => {
            json!({"name":a.name,"state":if a.action.as_deref()==Some("building"){"building"}else if a.sha.is_some(){"deployed"}else{"unknown"},
            "deployed_revision":a.sha,"image":a.image,
            "revision_drift":revision_drift(a.sha.as_deref(),d.desired_revision.as_deref()),
            "runtime":{"state":if state.sessions.runtime.is_some(){"unknown"}else{"unavailable"},"observed_at":null,"reason":if state.sessions.runtime.is_some(){"not_probed"}else{"runtime_not_configured"}},
            "latest_build":a.build.map(|b|json!({"revision":b.sha,"ok":b.ok,"finished_at":b.finished_epoch}))})
        }
    }
}
fn jobs(state: &AppState, d: &Descriptor, project: &str) -> Vec<Value> {
    d.jobs.iter().map(|j| {
        let runs=state.jobs.runs_for(project,&j.name);
        // Store order is newest first; reverse makes a timestamp tie prefer the newest insertion.
        let latest=runs.iter().rev().max_by_key(|r|r.started_epoch);
        json!({"name":j.name,"cron":j.cron,"timeout_minutes":j.timeout_minutes,
            "state":latest.map(|r|r.state.as_str()).unwrap_or("not_run"),
            "latest_run":latest.map(|r|json!({"branch":r.branch,"state":r.state,"started_at":r.started_epoch,"finished_at":r.finished_epoch,"exit":r.exit}))})
    }).collect()
}
fn attention(
    state: &AppState,
    p: &ProjectRecord,
    d: &Descriptor,
    service_error: Option<&&str>,
) -> Vec<Value> {
    let mut items = vec![];
    let mut add = |source: String, severity: &str, code: &str| {
        items.push(json!({"source":source,"project":p.name,"severity":severity,"code":code}))
    };
    if p.needs_merge || !state.sessions.debts_for(&p.name).is_empty() {
        add(format!("project:{}/merge", p.name), "warning", "merge_debt");
    }
    if let Some(e) = d.error {
        add(
            format!("project:{}/repository", p.name),
            "failure",
            serde_json::to_value(e).unwrap().as_str().unwrap(),
        );
    } else if d
        .observed_at
        .is_none_or(|t| crate::auth::now_epoch().saturating_sub(t) > 600)
    {
        add(
            format!("project:{}/repository", p.name),
            "warning",
            if d.observed_at.is_some() {
                "repository_stale"
            } else {
                "repository_pending"
            },
        );
    }
    for (name, enabled) in [
        ("ide", d.declaration.ide.enabled),
        ("memory", d.declaration.memory.enabled),
    ] {
        if enabled {
            add(
                format!("project:{}/setup/{name}", p.name),
                "pending",
                "adapter_not_configured",
            );
        }
    }
    if let Some(code) = service_error {
        add(format!("project:{}/service", p.name), "failure", code);
    }
    for s in state
        .sessions
        .live_records()
        .into_iter()
        .filter(|s| s.project == p.name)
    {
        if s.lifecycle == crate::sessions::Lifecycle::Failed
            || s.error.is_some()
            || !state.sessions.binding_safe(&s)
        {
            add(
                format!("session:{}", s.session_id),
                "failure",
                "session_unprotected_or_failed",
            );
        }
    }
    let a = app_summary(state, p, d);
    if a["latest_build"]["ok"] == false {
        add(
            format!("project:{}/app/build", p.name),
            "failure",
            "latest_build_failed",
        );
    }
    if a["revision_drift"] == true {
        add(
            format!("project:{}/app/revision", p.name),
            "warning",
            "revision_drift",
        );
    }
    for j in jobs(state, d, &p.name) {
        if ["failed", "timeout", "error", "aborted"].contains(&j["state"].as_str().unwrap_or("")) {
            add(
                format!("project:{}/job:{}", p.name, j["name"].as_str().unwrap()),
                "failure",
                "latest_job_failed",
            );
        }
    }
    items
}
fn summary(state: &AppState, p: &ProjectRecord, service_error: Option<&&str>) -> Value {
    let d = state.registry.descriptor(&p.name);
    json!({"name":p.name,"display_name":d.declaration.project.display_name.as_deref().unwrap_or(&p.name),
        "description":d.declaration.project.description,"template_version":p.template_version,"template_behind":p.template_behind,
        "repository_revision":d.desired_revision,"observed_revision":d.observed_revision,"needs_merge":p.needs_merge,
        "capabilities":capabilities(state,&d),
        "setup":{"state":if d.error.is_some(){"failed"}else if d.observed_revision.is_some(){"ready"}else{"pending"},
            "scope":"registry_manifest","desired_revision":d.desired_revision,"observed_revision":d.observed_revision,
            "observed_at":d.observed_at,"attempted_at":d.attempted_at,"retry_at":d.retry_at,"failures":d.failures,
            "stale":d.error.is_some() || d.observed_at.is_none_or(|t|crate::auth::now_epoch().saturating_sub(t)>600),
            "error":d.error.map(|e|json!({"code":e,"message":e.message()})),"service_error":service_error},
        "app":app_summary(state,p,&d),
        "session_count":state.sessions.live_records().iter().filter(|s|s.project==p.name).count(),
        "jobs":page(jobs(state,&d,&p.name),0,100),
        "latest_activity_at":state.events.for_project(&p.name).iter().map(|e|e.at_epoch).max()})
}
pub async fn root(
    _actor: Actor,
    State(state): State<AppState>,
    Query(query): Query<Page>,
) -> Response {
    let (offset, limit) = match query.bounds() {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let mut projects = state.registry.snapshot();
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    let (_, errors) = crate::catalog::dynamic(&state.registry);
    let mut items = vec![];
    for p in &projects {
        items.extend(attention(
            &state,
            p,
            &state.registry.descriptor(&p.name),
            errors.get(&p.name),
        ));
    }
    items.sort_by_key(|v| {
        (
            match v["severity"].as_str() {
                Some("failure") => 0,
                Some("warning") => 1,
                _ => 2,
            },
            v["source"].as_str().unwrap_or("").to_string(),
        )
    });
    let attention_count = items.len();
    items.truncate(100);
    let counts = json!({"projects":projects.len(),"sessions":state.sessions.live_records().len(),"attention":attention_count});
    let projects = projects
        .iter()
        .map(|p| summary(&state, p, errors.get(&p.name)))
        .collect();
    Json(json!({"observed_at":crate::auth::now_epoch(),"counts":counts,"projects":page(projects,offset,limit),
        "attention":{"items":items,"total":attention_count,"truncated":attention_count>100}})).into_response()
}
pub async fn detail(
    _actor: Actor,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<Page>,
) -> Response {
    let (offset, limit) = match query.bounds() {
        Ok(v) => v,
        Err(e) => return *e,
    };
    let Some(p) = state
        .registry
        .snapshot()
        .into_iter()
        .find(|p| p.name == project)
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"unknown_project"})),
        )
            .into_response();
    };
    let (_, errors) = crate::catalog::dynamic(&state.registry);
    let d = state.registry.descriptor(&project);
    let mut v = summary(&state, &p, errors.get(&project));
    v["observed_at"] = json!(crate::auth::now_epoch());
    v["memory"] = json!(d.declaration.memory);
    v["memory"]["sharing"] = json!(d.declaration.memory.sharing.as_deref().unwrap_or("private"));
    v["ide"] = json!(d.declaration.ide);
    v["merge_debt"]=page(state.sessions.debts_for(&project).iter().map(|d|json!({"branch":d.branch,"conflicted_files":d.conflicted_files,"ours_revision":d.ours.sha,"theirs_revision":d.theirs.sha,"since":d.since})).collect(),0,100);
    let mut sessions: Vec<_> = state
        .sessions
        .live_records()
        .into_iter()
        .filter(|s| s.project == project)
        .collect();
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    v["sessions"] = page(
        sessions
            .iter()
            .map(|s| s.view(state.sessions.runtime.as_ref()))
            .collect(),
        0,
        100,
    );
    v["jobs"] = page(jobs(&state, &d, &project), 0, 100);
    let mut events = state.events.for_project(&project);
    events.reverse();
    // Event variants are an explicit allowlist; never include arbitrary error or log text.
    let activity=events.iter().map(|e|json!({"at_epoch":e.at_epoch,"event":match &e.event {
        crate::events::Event::SessionOpened{session_id,..}=>json!({"kind":"session_opened","session_id":session_id}),
        crate::events::Event::SessionClosed{session_id,merged,..}=>json!({"kind":"session_closed","session_id":session_id,"merged":merged}),
        crate::events::Event::SessionRecycled{session_id,..}=>json!({"kind":"session_recycled","session_id":session_id}),
        crate::events::Event::SessionReaped{session_id,..}=>json!({"kind":"session_reaped","session_id":session_id}),
        crate::events::Event::V1Session{session_id,..}=>json!({"kind":"v1_session","session_id":session_id}),
        crate::events::Event::JobRun{job,..}=>json!({"kind":"job_run","job":job}),
        crate::events::Event::ProjectCreated{adopted,..}=>json!({"kind":"project_created","adopted":adopted}),
    }})).collect();
    v["activity"] = page(activity, offset, limit);
    v["attention"] = page(attention(&state, &p, &d, errors.get(&project)), 0, 100);
    Json(v).into_response()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn actor() -> Actor {
        Actor {
            driver: "tester".into(),
            role: crate::auth::Role::Admin,
            approval: None,
        }
    }
    fn registered() -> AppState {
        let s = AppState::default();
        for name in ["bravo", "alpha"] {
            s.registry.insert(ProjectRecord::new(
                name,
                &crate::manifest::Manifest::parse("").unwrap(),
                None,
            ));
        }
        s
    }
    async fn body(r: Response) -> (StatusCode, Value) {
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    #[tokio::test]
    async fn overview_is_immediate_paginated_and_read_only_without_runtime() {
        let s = registered();
        let before = serde_json::to_value(s.sessions.dump_records()).unwrap();
        let (status, v) = body(
            root(
                actor(),
                State(s.clone()),
                Query(Page {
                    offset: None,
                    limit: Some(1),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["counts"]["projects"], 2);
        assert_eq!(v["projects"]["items"][0]["name"], "alpha");
        assert_eq!(v["projects"]["next_offset"], 1);
        let (status, v) = body(
            detail(
                actor(),
                State(s.clone()),
                Path("alpha".into()),
                Query(Page::default()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["setup"]["state"], "pending");
        assert_eq!(v["capabilities"]["ide"]["state"], "pending");
        assert_eq!(v["capabilities"]["memory"]["state"], "pending");
        assert_eq!(
            v["capabilities"]["workspace"]["reason"],
            "runtime_not_configured"
        );
        assert_eq!(
            serde_json::to_value(s.sessions.dump_records()).unwrap(),
            before
        );
        assert!(s.events.dump().is_empty());
        assert!(s.registry.descriptors().is_empty());
        assert_eq!(
            body(
                detail(
                    actor(),
                    State(s.clone()),
                    Path("missing".into()),
                    Query(Page::default())
                )
                .await
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            body(
                root(
                    actor(),
                    State(s),
                    Query(Page {
                        offset: None,
                        limit: Some(0)
                    })
                )
                .await
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    #[tokio::test]
    async fn detail_redacts_credentials_commands_errors_build_logs_and_honors_latest_job() {
        let s = registered();
        let m=crate::manifest::Manifest::parse("[app]\nname='webapp'\n[app.secrets]\nTOKEN='SECRET-SENTINEL'\n[jobs.nightly]\ncron='0 1 * * *'\ncommand='COMMAND-SENTINEL'").unwrap();
        let mut map = s.registry.descriptors();
        map.insert(
            "alpha".into(),
            Descriptor {
                declaration: m.declaration,
                app: Some("webapp".into()),
                jobs: vec![crate::ecosystem::JobDefinition {
                    name: "nightly".into(),
                    cron: "0 1 * * *".into(),
                    timeout_minutes: 30,
                }],
                desired_revision: Some("abcdef0123456789abcdef0123456789abcdef01".into()),
                observed_revision: Some("abcdef0123456789abcdef0123456789abcdef01".into()),
                observed_at: Some(crate::auth::now_epoch()),
                ..Default::default()
            },
        );
        s.registry.hydrate_descriptors(map);
        let (_, unrun) = body(
            detail(
                actor(),
                State(s.clone()),
                Path("alpha".into()),
                Query(Page::default()),
            )
            .await,
        )
        .await;
        assert_eq!(unrun["app"]["state"], "not_deployed");
        assert_eq!(unrun["jobs"]["items"][0]["state"], "not_run");
        let rec:crate::sessions::SessionRecord=serde_json::from_value(json!({"session_id":"testing","project":"alpha","branch":"session/testing","head":"abc","stale":false,"actor":actor(),"token":"TOKEN-SENTINEL"})).unwrap();
        s.sessions.hydrate(
            Default::default(),
            [("testing".into(), rec)].into_iter().collect(),
        );
        s.apps.hydrate(
            [(
                "webapp".into(),
                crate::apps::AppRecord {
                    name: "webapp".into(),
                    project: "alpha".into(),
                    sha: Some("abcdef012345".into()),
                    image: Some("webapp:abcdef012345".into()),
                    action: None,
                    build: Some(crate::apps::BuildRecord {
                        sha: "abcdef012345".into(),
                        ok: false,
                        finished_epoch: 10,
                        log_tail: "LOG-SENTINEL".into(),
                    }),
                },
            )]
            .into_iter()
            .collect(),
        );
        let run = crate::jobs::JobRunRecord {
            project: "alpha".into(),
            job: "nightly".into(),
            branch: "job-nightly".into(),
            state: "failed".into(),
            started_epoch: 10,
            finished_epoch: Some(11),
            exit: Some(1),
            detail: Some("ERROR-SENTINEL".into()),
        };
        s.jobs.push_run(run.clone());
        let (_, v) = body(
            detail(
                actor(),
                State(s.clone()),
                Path("alpha".into()),
                Query(Page::default()),
            )
            .await,
        )
        .await;
        let text = v.to_string();
        for sentinel in [
            "TOKEN-SENTINEL",
            "SECRET-SENTINEL",
            "COMMAND-SENTINEL",
            "LOG-SENTINEL",
            "ERROR-SENTINEL",
        ] {
            assert!(!text.contains(sentinel), "{sentinel}");
        }
        assert_eq!(v["app"]["revision_drift"], false);
        assert_eq!(v["app"]["runtime"]["state"], "unavailable");
        assert!(v["app"]["runtime"].get("observed_at").is_some());
        assert!(text.contains("latest_job_failed"));
        assert!(text.contains("latest_build_failed"));
        s.jobs.push_run(crate::jobs::JobRunRecord {
            state: "succeeded".into(),
            started_epoch: 10,
            finished_epoch: Some(11),
            exit: Some(0),
            ..run
        });
        let (_, v) = body(
            detail(
                actor(),
                State(s.clone()),
                Path("alpha".into()),
                Query(Page::default()),
            )
            .await,
        )
        .await;
        assert!(!v.to_string().contains("latest_job_failed"));
        // Adapter resources have never been provisioned, even after manifest observation.
        assert!(v["capabilities"]["ide"]["observed_revision"].is_null());
        assert!(v["capabilities"]["memory"]["observed_at"].is_null());
    }
    #[test]
    fn deployed_sha_prefix_drift_is_correct() {
        assert_eq!(
            revision_drift(
                Some("abcdef012345"),
                Some("abcdef0123456789abcdef0123456789abcdef0123")
            ),
            Some(false)
        );
        assert_eq!(
            revision_drift(
                Some("fedcba012345"),
                Some("abcdef0123456789abcdef0123456789abcdef0123")
            ),
            Some(true)
        );
        assert_eq!(revision_drift(None, Some("abcdef012345")), None);
        assert_eq!(revision_drift(Some("bad"), Some("abcdef012345")), None);
    }
}
#[cfg(test)]
mod http_tests {
    use super::*;
    #[tokio::test]
    async fn actual_routes_enforce_auth_validate_query_and_preserve_legacy_shape() {
        let mut state = AppState::default();
        state.auth.config = std::sync::Arc::new(crate::auth::AuthConfig {
            bootstrap_bearer: Some("overview-test".into()),
            ..Default::default()
        });
        state.registry.insert(ProjectRecord::new(
            "sample",
            &crate::manifest::Manifest::parse("").unwrap(),
            None,
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::app(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = reqwest::Client::new();
        for path in ["overview", "projects/sample"] {
            assert_eq!(
                http.get(format!("http://{addr}/sigiled/{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                http.get(format!("http://{addr}/sigiled/{path}"))
                    .bearer_auth("overview-test")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
        for query in [
            "limit=oops",
            "offset=-1",
            "limit=101",
            "offset=1000001",
            "surprise=1",
        ] {
            let status = http
                .get(format!("http://{addr}/sigiled/overview?{query}"))
                .bearer_auth("overview-test")
                .send()
                .await
                .unwrap()
                .status();
            assert!(
                [StatusCode::BAD_REQUEST, StatusCode::UNPROCESSABLE_ENTITY].contains(&status),
                "{query}: {status}"
            );
        }
        let projects: Value = http
            .get(format!("http://{addr}/sigiled/projects"))
            .bearer_auth("overview-test")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(projects.is_array());
        assert!(state.registry.descriptors().is_empty());
        assert!(state.sessions.live_records().is_empty());
        assert!(state.events.dump().is_empty());
        server.abort();
    }
}
