use super::*;
#[path = "../../../../shared/handoff_contract.rs"]
mod contract;
fn request_size(body: &Value) -> Result<(), Error> {
    if serde_json::to_vec(body).map_err(|_| unavailable())?.len() > contract::MAX_REQUEST_BYTES {
        return Err(Error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "handoff_bundle_too_large_reduce_content",
        ));
    }
    Ok(())
}
use crate::sessions::{Lifecycle, SessionRecord};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::browser) struct Action {
    session_id: String,
    generation: String,
    bundle_digest: String,
}
fn digest(v: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(v).unwrap()))
}
fn bundle(v: Value, id: &str) -> Result<Value, Error> {
    let slug = v["slug"]
        .as_str()
        .filter(|s| identifier(s))
        .ok_or(Error(StatusCode::BAD_GATEWAY, "invalid_handoff_bundle"))?;
    if v["run_id"] != id {
        return Err(unavailable());
    }
    let files = v["files"]
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or_else(unavailable)?;
    let prefix = format!("docs/design/{slug}/");
    let mut paths = std::collections::HashSet::new();
    for f in files {
        let p = f["path"].as_str().ok_or_else(unavailable)?;
        let name = p.strip_prefix(&prefix).ok_or_else(unavailable)?;
        if !name.ends_with(".md")
            || name.starts_with('.')
            || name.len() > 100
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || !paths.insert(p)
            || f["content"].as_str().is_none_or(|s| s.len() > 512 * 1024)
        {
            return Err(Error(StatusCode::BAD_GATEWAY, "invalid_handoff_bundle"));
        }
    }
    // Forward only the companion's exact File contract; upstream metadata must
    // not trigger a deny_unknown_fields extractor rejection after pending.
    let files: Vec<_> = files
        .iter()
        .map(|f| json!({"path":f["path"],"content":f["content"]}))
        .collect();
    Ok(json!({"run_id":id,"slug":slug,"files":files}))
}
pub(super) fn preview(v: Value, id: &str) -> Result<Value, Error> {
    let b = bundle(v, id)?;
    Ok(json!({"digest":digest(&b),"files":b["files"],"slug":b["slug"],"run_id":id}))
}
pub(in crate::browser) async fn apply(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Result<Json<Value>, Error> {
    checked_id(&id)?;
    checked_id(&a.session_id)?;
    revision(&a.generation)?;
    tokio::spawn(owned(c, state, id, a)).await.map_err(|_| {
        Error(
            StatusCode::CONFLICT,
            "handoff_interrupted_recover_same_operation",
        )
    })?
}
fn marker_pending(r: &SessionRecord) -> bool {
    r.handoff
        .as_ref()
        .is_some_and(|v| !matches!(v["phase"].as_str(), Some("complete" | "failed")))
}
fn persist(state: &AppState, r: &SessionRecord) -> Result<(), Error> {
    state.sessions.put(r.clone());
    state.try_persist().map_err(|_| {
        Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "handoff_state_save_uncertain",
        )
    })
}
// Tests bind ephemeral loopback listeners because the workspace already owns
// ports 8000/8090. Only the transport destination changes; all HTTP/extractor,
// authorization, marker and native subprocess paths remain real.
#[cfg(test)]
pub(super) static FIXTURE_ENDPOINTS: std::sync::LazyLock<Mutex<HashMap<String, (String, String)>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
fn endpoint(r: &SessionRecord, helper: bool) -> String {
    #[cfg(test)]
    if let Some((helper_url, log_url)) = FIXTURE_ENDPOINTS.lock().unwrap().get(&r.session_id) {
        return if helper {
            helper_url.clone()
        } else {
            log_url.clone()
        };
    }
    format!(
        "http://{}:{}",
        r.container(),
        if helper { 8090 } else { 8000 }
    )
}
async fn request(
    r: &SessionRecord,
    path: &str,
    body: Option<&Value>,
) -> Result<(StatusCode, Value), Error> {
    let binding = r
        .binding
        .as_ref()
        .and_then(|b| b.ide.as_ref())
        .filter(|b| b.generation == r.generation)
        .ok_or(Error(StatusCode::CONFLICT, "handoff_helper_required"))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| unavailable())?;
    let url = format!("{}/{path}", endpoint(r, true));
    let req = if let Some(body) = body {
        client.post(url).json(body)
    } else {
        client.get(url)
    };
    let mut response = req
        .bearer_auth(&binding.token)
        .send()
        .await
        .map_err(|_| Error(StatusCode::GATEWAY_TIMEOUT, "handoff_acceptance_unknown"))?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len() + chunk.len() > 65536 {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((
        status,
        serde_json::from_slice(&bytes).map_err(|_| unavailable())?,
    ))
}
async fn log(r: &SessionRecord) -> Result<(), Error> {
    let token = r.token.as_ref().ok_or_else(unavailable)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| unavailable())?;
    let mut response = client
        .get(format!("{}/git/log?limit=15", endpoint(r, false)))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| unavailable())?;
    if !response.status().is_success() {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len() + chunk.len() > 65536 {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    let rows: Vec<Value> = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if rows.is_empty()
        || rows.len() > 15
        || rows.iter().any(|r| {
            r["sha"]
                .as_str()
                .is_none_or(|s| s.len() != 40 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        })
    {
        return Err(unavailable());
    }
    Ok(())
}
async fn owned(
    c: BrowserContext,
    state: AppState,
    id: String,
    a: Action,
) -> Result<Json<Value>, Error> {
    let s = state.browser.inner()?.research.clone();
    s.store.ensure_writable()?;
    let project = s
        .store
        .association(&c.actor.driver, &id)?
        .ok_or(Error::forbidden("research_project_association_required"))?;
    let _session = state
        .sessions
        .session_lock(&a.session_id)
        .lock_owned()
        .await;
    let mut r = state
        .sessions
        .record(&a.session_id)
        .ok_or(Error(StatusCode::NOT_FOUND, "unknown_session"))?;
    if r.actor.driver != c.actor.driver || r.project != project {
        return Err(Error::forbidden("workspace_owner_required"));
    }
    if crate::sessions::authorized(&c.actor, &r, &state, crate::auth::Action::OpenSession).is_err()
    {
        return Err(Error::forbidden("workspace_approval_required"));
    }
    if r.lifecycle != Lifecycle::Active
        || r.generation.to_string() != a.generation
        || !r.runtime_owned
        || !state.sessions.binding_safe(&r)
    {
        return Err(Error(StatusCode::CONFLICT, "session_generation_not_active"));
    }
    if !state.store.durable() {
        return Err(Error(
            StatusCode::CONFLICT,
            "durable_handoff_storage_required",
        ));
    }
    let operation_id = format!(
        "{:x}",
        Sha256::digest(format!("{}:{}:{}", c.actor.driver, project, id))
    );
    if let Some(previous) = &r.handoff {
        if previous["operation_id"] != operation_id && marker_pending(&r) {
            return Err(Error(
                StatusCode::CONFLICT,
                "other_handoff_recovery_required",
            ));
        }
    }
    let _project = state.sessions.merge_lock(&project).lock_owned().await;
    if !state.sessions.debts_for(&project).is_empty() {
        return Err(Error(
            StatusCode::CONFLICT,
            "merge_debt_requires_resolution",
        ));
    }
    let previous = r
        .handoff
        .clone()
        .filter(|p| p["operation_id"] == operation_id);
    let request_body = if let Some(previous) = &previous {
        if previous["digest"] != a.bundle_digest {
            return Err(Error(StatusCode::CONFLICT, "handoff_bundle_changed"));
        }
        previous["request"].clone()
    } else {
        let b = bundle(
            s.call(
                Engine::Sde,
                Method::GET,
                &format!("runs/{id}/handoff"),
                &[],
                &c.access_token,
                None,
            )
            .await?,
            &id,
        )?;
        if digest(&b) != a.bundle_digest {
            return Err(Error(StatusCode::CONFLICT, "handoff_bundle_changed"));
        }
        json!({"operation_id":operation_id,"generation":a.generation,"run_id":id,"slug":b["slug"],"files":b["files"]})
    };
    request_size(&request_body)?;
    let (_, observation) = request(&r, &format!("handoff-status/{operation_id}"), None).await?;
    if observation["contract"] != "sigil-handoff-v1" || observation["generation"] != a.generation {
        return Err(Error(
            StatusCode::CONFLICT,
            "handoff_helper_update_required",
        ));
    }
    if marker_pending(&r) {
        match observation["observation"]["phase"].as_str() {
            Some("complete") => {
                return complete(
                    &state,
                    &s,
                    &mut r,
                    &id,
                    observation["observation"]["receipt"].clone(),
                )
            }
            Some("failed") => {
                r.handoff.as_mut().unwrap()["phase"] = json!("failed");
                persist(&state, &r)?;
            }
            _ => {
                return Err(Error(
                    StatusCode::CONFLICT,
                    "handoff_quarantined_recheck_operation",
                ))
            }
        }
    }
    // Git handoff is read after selecting this owned generation, before any write.
    log(&r).await?;
    r.handoff = Some(
        json!({"phase":"pending","operation_id":operation_id,"run_id":id,"digest":a.bundle_digest,"request":request_body}),
    );
    persist(&state, &r)?;
    let (status, response) = request(&r, "handoff", Some(&request_body)).await?;
    if !status.is_success() {
        // Confirm terminal helper state; an arbitrary error or lost response is not a release receipt.
        let (_, observed) = request(&r, &format!("handoff-status/{operation_id}"), None).await?;
        if observed["observation"]["phase"] == "failed" {
            r.handoff.as_mut().unwrap()["phase"] = json!("failed");
            persist(&state, &r)?;
            return Err(Error(
                StatusCode::CONFLICT,
                safe_error(response["error"].as_str().unwrap_or("")),
            ));
        }
        return Err(Error(
            StatusCode::CONFLICT,
            "handoff_quarantined_recheck_operation",
        ));
    }
    complete(&state, &s, &mut r, &id, response)
}
fn safe_error(e: &str) -> &'static str {
    match e {
        "workspace_changes_checkpoint_and_retry" => "workspace_changes_checkpoint_and_retry",
        "editor_ownership_unknown" => "editor_ownership_unknown",
        "handoff_target_exists" => "handoff_target_exists",
        "handoff_target_changed" => "handoff_target_changed",
        "workspace_changed" | "workspace_index_changed" => "workspace_changed",
        _ => "handoff_failed_work_preserved",
    }
}
fn complete(
    state: &AppState,
    s: &Services,
    r: &mut SessionRecord,
    id: &str,
    response: Value,
) -> Result<Json<Value>, Error> {
    let sha = response["sha"]
        .as_str()
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(unavailable)?;
    let expected_generation = r.generation.to_string();
    if response["generation"].as_str() != Some(expected_generation.as_str())
        || response["run_id"] != id
        || response["operation_id"] != r.handoff.as_ref().unwrap()["operation_id"]
        || response["pushed"] != true
        || !response["dirty"].is_boolean()
    {
        return Err(unavailable());
    }
    let receipt = json!({"state":if response["dirty"]==true{"committed_with_concurrent_changes"}else{"committed_on_session"},"run_id":id,"session_id":r.session_id,"generation":r.generation.to_string(),"commit":sha,"pushed":true,"dirty":response["dirty"],"master_accepted":false,"indexed":false,"editor":"paused"});
    s.store.handoff(&r.actor.driver, id, receipt.clone())?;
    if let Some(ide) = r.binding.as_mut().and_then(|b| b.ide.as_mut()) {
        ide.state = "stopped".into();
    }
    let marker = r.handoff.as_mut().unwrap();
    marker["phase"] = json!("complete");
    marker["receipt"] = receipt.clone();
    persist(state, r)?;
    Ok(Json(receipt))
}
