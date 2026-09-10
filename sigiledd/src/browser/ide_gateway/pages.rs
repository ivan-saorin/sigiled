use super::*;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
pub(super) fn asset(path: &str) -> Option<Response> {
    let (kind, body) = match path {
        "/_sigil/launch" => ("text/html; charset=utf-8", include_str!("launch.html")),
        "/_sigil/launch.js" => ("text/javascript; charset=utf-8", include_str!("launch.js")),
        "/_sigil/workbench" => ("text/html; charset=utf-8", include_str!("workbench.html")),
        "/_sigil/workbench.js" => (
            "text/javascript; charset=utf-8",
            include_str!("workbench.js"),
        ),
        "/_sigil/style.css" => ("text/css; charset=utf-8", include_str!("style.css")),
        _ => return None,
    };
    let mut r = ([("content-type", kind)], body).into_response();
    r.headers_mut().insert("content-security-policy",HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"));
    Some(r)
}
pub(super) async fn body<T: DeserializeOwned>(request: axum::extract::Request) -> Result<T, Error> {
    if single(request.headers(), "content-type").is_none_or(|v| !v.starts_with("application/json"))
    {
        return Err(Error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "json_required"));
    }
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(request.into_body(), 4096),
    )
    .await
    .map_err(|_| Error(StatusCode::REQUEST_TIMEOUT, "request_timeout"))?
    .map_err(|_| Error(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large"))?;
    serde_json::from_slice(&bytes).map_err(|_| Error(StatusCode::BAD_REQUEST, "invalid_request"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Consume {
    pub ticket: String,
}
pub(super) async fn platform(
    state: &AppState,
    grant: access::Grant,
    request: axum::extract::Request,
) -> Result<Response, Error> {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    if method == Method::GET {
        if let Some(r) = asset(&path) {
            return Ok(r);
        }
    }
    match (method.as_str(),path.as_str()) {
        ("GET","/_sigil/session")=>Ok(Json(json!({"csrf_token":grant.csrf,"generation":grant.binding.generation.to_string(),"editor_url":grant.binding.destination,"preview_ports":if state.browser.inner()?.config.preview_domain.is_some(){state.registry.descriptor(&grant.binding.project).declaration.ide.preview_ports}else{vec![]}})).into_response()),
        ("GET","/_sigil/status")=>{let c=access::authorize(state,&grant.binding,false).await?;controls::status(c,State(state.clone()),Path(grant.binding.session)).await},
        ("POST","/_sigil/activity")=>{access::csrf(request.headers(),&grant)?;let _:serde_json::Value=body(request).await?;access::authorize(state,&grant.binding,true).await?;Ok(StatusCode::NO_CONTENT.into_response())},
        ("POST","/_sigil/preview")=>{access::csrf(request.headers(),&grant)?;let body:preview::Preview=body(request).await?;if targets::generation(&body.generation)?!=grant.binding.generation{return Err(Error(StatusCode::CONFLICT,"generation_changed"));}let c=access::authorize(state,&grant.binding,true).await?;preview::launch(c,State(state.clone()),Path(grant.binding.session),Json(body)).await},
        ("POST","/_sigil/operation")=>{access::csrf(request.headers(),&grant)?;let op:controls::Operation=body(request).await?;if targets::generation(&op.generation)?!=grant.binding.generation{return Err(Error(StatusCode::CONFLICT,"generation_changed"));}let c=access::authorize(state,&grant.binding,true).await?;controls::operation(c,State(state.clone()),Path(grant.binding.session),Json(op)).await},
        _=>Err(Error(StatusCode::NOT_FOUND,"unknown_editor_control"))
    }
}
