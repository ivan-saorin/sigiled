use super::*;
use serde::Deserialize;
use serde_json::json;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Launch {
    pub idempotency_key: String,
    #[serde(default)]
    pub target: Option<targets::FileTarget>,
}
pub(super) fn key(actor: &str, project: &str, key: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(actor, project, key)).unwrap())
    )
}
pub(crate) async fn launch(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(body): Json<Launch>,
) -> Result<Response, Error> {
    readiness(&state)?;
    if !crate::project::valid_name(&project) || !state.registry.contains(&project) {
        return Err(Error(StatusCode::NOT_FOUND, "unknown_project"));
    }
    if !state.registry.descriptor(&project).declaration.ide.enabled {
        return Err(Error(StatusCode::CONFLICT, "project_ide_not_enabled"));
    }
    if !c.actor.driver.starts_with("human:")
        || crate::auth::authorize(
            &c.actor,
            crate::auth::Action::OpenSession,
            Some(&project),
            &state.auth.approvals,
            auth::now_epoch(),
        )
        .is_err()
    {
        return Err(Error::forbidden("workspace_approval_required"));
    }
    if body.idempotency_key.len() < 8
        || body.idempotency_key.len() > 128
        || !body
            .idempotency_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_idempotency_key"));
    }
    if body.target.as_ref().is_some_and(|t| !t.validate()) {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_file_target"));
    }
    match tokio::spawn(launch_owned(c, state, project, body)).await {
        Ok(r) => r,
        Err(_) => Err(Error(
            StatusCode::CONFLICT,
            "allocation_interrupted_retry_same_key",
        )),
    }
}
async fn launch_owned(
    c: BrowserContext,
    state: AppState,
    project: String,
    body: Launch,
) -> Result<Response, Error> {
    let digest = key(&c.actor.driver, &project, &body.idempotency_key);
    let serial = state
        .sessions
        .session_lock(&format!("human-allocation:{}:{project}", c.actor.driver));
    let _serial = serial.lock_owned().await;
    if !state.sessions.debts_for(&project).is_empty() {
        return Err(Error(
            StatusCode::CONFLICT,
            "merge_debt_requires_resolution",
        ));
    }
    let previous = state
        .sessions
        .browser_allocations
        .lock()
        .unwrap()
        .get(&digest)
        .cloned();
    let id = if let Some(id) = previous {
        id
    } else {
        // Resume only this actual human's unique existing workspace. Never guess among multiple records.
        let owned: Vec<_> = state
            .sessions
            .dump_records()
            .into_values()
            .filter(|r| r.project == project && r.actor.driver == c.actor.driver)
            .collect();
        if owned.len() > 1 {
            return Err(Error(
                StatusCode::CONFLICT,
                "multiple_owned_workspaces_require_selection",
            ));
        }
        let existing = owned.first().map(|r| r.session_id.clone());
        let id = existing.clone().unwrap_or_else(crate::sessions::reserve_id);
        {
            let mut intents = state.sessions.browser_allocations.lock().unwrap();
            if intents.len() >= 100000 {
                return Err(Error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "allocation_history_capacity",
                ));
            }
            intents.insert(digest, id.clone());
        }
        // Intent contains no login/access/refresh credential. Keep it after close as a tombstone.
        state.try_persist().map_err(|_| {
            Error(
                StatusCode::CONFLICT,
                "allocation_save_uncertain_retry_same_key",
            )
        })?;
        if existing.is_none() {
            let response = crate::sessions::open_human(
                c.actor.clone(),
                state.clone(),
                project.clone(),
                id.clone(),
            )
            .await;
            drop(response);
        }
        id
    };
    let Some(mut r) = state.sessions.record(&id) else {
        return Ok((StatusCode::CONFLICT,Json(json!({"session_id":id,"state":"allocation_recovery_required","error":"allocation_absent_or_closed","retry_same_key":true}))).into_response());
    };
    if r.project != project || r.actor.driver != c.actor.driver {
        return Err(Error::forbidden("allocation_owner_mismatch"));
    }
    if r.lifecycle != Lifecycle::Active {
        return Ok((StatusCode::CONFLICT,Json(json!({"session_id":id,"generation":r.generation.to_string(),"state":r.lifecycle,"error":"workspace_recovery_required","retry_same_key":true}))).into_response());
    }
    if r.lifecycle == Lifecycle::Active
        && !r
            .binding
            .as_ref()
            .and_then(|b| b.ide.as_ref())
            .is_some_and(|i| i.state == "ready")
    {
        let response = crate::ide::operation(
            c.actor.clone(),
            State(state.clone()),
            Path(id.clone()),
            Json(crate::ide::Operation {
                generation: r.generation,
                action: "start".into(),
            }),
        )
        .await;
        if !response.status().is_success() {
            return controls::safe_response(response).await;
        }
        r = state
            .sessions
            .record(&id)
            .ok_or(Error(StatusCode::CONFLICT, "workspace_closed"))?;
    }
    let b = state.browser.inner()?;
    let origin = targets::origin(b.config.ide_domain.as_deref().unwrap(), &id, r.generation)?;
    if let Some(target) = &body.target {
        targets::probe(&state, &r, target).await?;
    }
    let destination = targets::destination(&origin, body.target.as_ref())?;
    let binding = access::Binding {
        target: body.target,
        port: None,
        parent: c.session_id,
        actor: c.actor.driver,
        project,
        session: id.clone(),
        generation: r.generation,
        origin: origin.clone(),
        destination,
    };
    access::authorize(&state, &binding, false).await?;
    let ticket = access::issue(&b, binding)?;
    Ok(Json(json!({"session_id":id,"generation":r.generation.to_string(),"state":"ready","launch_url":format!("{origin}/_sigil/launch#{ticket}"),"preview":{"available":b.config.preview_domain.is_some() && !state.registry.descriptor(&r.project).declaration.ide.preview_ports.is_empty(),"ports":state.registry.descriptor(&r.project).declaration.ide.preview_ports},"file_navigation":{"available":true}})).into_response())
}
