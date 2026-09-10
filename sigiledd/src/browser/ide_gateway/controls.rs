use super::*;
use serde::Deserialize;
use serde_json::{json, Value};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Operation {
    pub generation: String,
    pub action: String,
}
fn owner(state: &AppState, c: &BrowserContext, id: &str) -> Result<SessionRecord, Error> {
    let r = state
        .sessions
        .record(id)
        .ok_or(Error(StatusCode::NOT_FOUND, "unknown_session"))?;
    if r.actor.driver != c.actor.driver {
        return Err(Error::forbidden("workspace_owner_required"));
    }
    Ok(r)
}
pub(crate) async fn status(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let r = owner(&state, &c, &id)?;
    let response = crate::ide::status(c.actor, State(state), Path(id)).await;
    let response = safe_response(response).await?;
    let status = response.status();
    let mut v: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 65536)
            .await
            .map_err(|_| Error(StatusCode::BAD_GATEWAY, "invalid_workspace_response"))?,
    )
    .map_err(|_| Error(StatusCode::BAD_GATEWAY, "invalid_workspace_response"))?;
    v["generation"] = json!(r.generation.to_string());
    v["session_id"] = json!(r.session_id);
    Ok((status, Json(v)).into_response())
}
async fn operation_inner(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(op): Json<Operation>,
) -> Result<Response, Error> {
    let record = owner(&state, &c, &id)?;
    if !matches!(op.action.as_str(), "checkpoint" | "finish" | "start") {
        return Err(Error(StatusCode::BAD_REQUEST, "unsupported_ide_action"));
    }
    let generation = targets::generation(&op.generation)?;
    if op.action == "start" {
        readiness(&state)?;
    }
    // Checkpoint and finish intentionally remain available for recovery after
    // browser activation is disabled; existing C1/A1 authority still applies.
    if op.action == "finish" && record.lifecycle == Lifecycle::Failed {
        // A deliberate retry uses A1's normal close, which rechecks ownership,
        // generation and the strict C1 finish receipt before any destruction.
        return safe_response(
            crate::sessions::close_expected(c.actor, state, id, Some(generation)).await,
        )
        .await;
    }
    safe_response(
        crate::ide::operation(
            c.actor,
            State(state),
            Path(id),
            Json(crate::ide::Operation {
                generation,
                action: op.action,
            }),
        )
        .await,
    )
    .await
}
pub(crate) async fn safe_response(response: Response) -> Result<Response, Error> {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "invalid_workspace_response"))?;
    let mut v: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "invalid_workspace_response"))?;
    convert_generations(&mut v);
    Ok((status, Json(v)).into_response())
}

pub(super) fn convert_generations(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                if k == "generation" {
                    if let Some(n) = v.as_u64() {
                        *v = json!(n.to_string());
                    }
                } else {
                    convert_generations(v);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(convert_generations),
        _ => {}
    }
}

pub(crate) async fn operation(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(op): Json<Operation>,
) -> Result<Response, Error> {
    let project = state.sessions.record(&id).map(|r| r.project);
    let actor = c.actor.driver.clone();
    let token = c.access_token.clone();
    let finish = op.action == "finish";
    let response = operation_inner(c, State(state.clone()), Path(id), Json(op)).await;
    if finish && response.as_ref().is_ok_and(|r| r.status().is_success()) {
        if let Some(project) = project {
            crate::enrollment::schedule(state, project, actor, token);
        }
    }
    response
}
