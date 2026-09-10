use super::*;
use serde::Deserialize;
use serde_json::json;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preview {
    pub(super) generation: String,
    pub(super) port: u16,
}
pub(crate) async fn launch(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Preview>,
) -> Result<Response, Error> {
    let b = state.browser.inner()?;
    let domain = b
        .config
        .preview_domain
        .as_deref()
        .ok_or(Error(StatusCode::CONFLICT, "preview_domain_not_configured"))?;
    let generation = targets::generation(&body.generation)?;
    let r = state
        .sessions
        .record(&id)
        .ok_or(Error(StatusCode::NOT_FOUND, "unknown_session"))?;
    let origin = targets::preview_origin(domain, &id, generation, body.port)?;
    let binding = access::Binding {
        target: None,
        port: Some(body.port),
        parent: c.session_id,
        actor: c.actor.driver,
        project: r.project,
        session: id.clone(),
        generation,
        origin: origin.clone(),
        destination: "/".into(),
    };
    access::authorize(&state, &binding, false).await?;
    let ticket = access::issue(&b, binding)?;
    Ok(Json(json!({"session_id":id,"generation":generation.to_string(),"port":body.port,"launch_url":format!("{origin}/_sigil/launch#{ticket}")})).into_response())
}
