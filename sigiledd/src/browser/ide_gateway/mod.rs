use super::*;
use crate::sessions::{Lifecycle, SessionRecord};
use axum::extract::Path;
mod access;
mod allocation;
mod controls;
mod pages;
mod preview;
mod proxy;
mod targets;
pub(super) use allocation::launch;
pub(super) use controls::{operation, status};
pub(super) use preview::launch as preview_launch;
#[derive(Default)]
pub(super) struct Gateway {
    #[cfg(test)]
    upstreams: Mutex<HashMap<String, std::net::SocketAddr>>,
    tickets: Mutex<HashMap<String, access::Ticket>>,
    grants: Mutex<HashMap<String, access::Grant>>,
}
pub(super) fn valid_domain(domain: &str) -> bool {
    targets::domain(domain)
}
#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub(crate) async fn dispatch(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let Ok(b) = state.browser.inner() else {
        return next.run(request).await;
    };
    let Some(domain) = b.config.ide_domain.as_deref() else {
        return next.run(request).await;
    };
    let host = single(request.headers(), "host").unwrap_or("");
    let is_preview = b
        .config
        .preview_domain
        .as_ref()
        .is_some_and(|d| host.ends_with(&format!(".{d}")));
    if !host.ends_with(&format!(".{domain}")) && !is_preview {
        return next.run(request).await;
    }
    let origin = format!("https://{host}");
    if !state.sessions.dump_records().values().any(|r| {
        if is_preview {
            state
                .registry
                .descriptor(&r.project)
                .declaration
                .ide
                .preview_ports
                .iter()
                .any(|p| {
                    targets::preview_origin(
                        b.config.preview_domain.as_deref().unwrap(),
                        &r.session_id,
                        r.generation,
                        *p,
                    )
                    .is_ok_and(|o| o == origin)
                })
        } else {
            targets::origin(domain, &r.session_id, r.generation).is_ok_and(|o| o == origin)
        }
    }) {
        return Error(StatusCode::NOT_FOUND, "unknown_ide_origin").into_response();
    }
    let mut response = match handle(&state, request).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    };
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}
async fn handle(state: &AppState, request: axum::extract::Request) -> Result<Response, Error> {
    let path = request.uri().path();
    if path.starts_with("/_sigil/") && request.uri().query().is_some() {
        return Err(Error(StatusCode::BAD_REQUEST, "control_query_not_allowed"));
    }
    if request.method() == Method::GET
        && matches!(
            path,
            "/_sigil/launch" | "/_sigil/launch.js" | "/_sigil/style.css"
        )
    {
        return pages::asset(path).ok_or(Error(StatusCode::NOT_FOUND, "unknown_asset"));
    }
    if request.method() == Method::POST && path == "/_sigil/consume" {
        let headers = request.headers().clone();
        let body: pages::Consume = pages::body(request).await?;
        return access::consume(state, &headers, &body.ticket).await;
    }
    let grant = access::grant(
        state,
        request.headers(),
        request.method() != Method::GET && request.method() != Method::HEAD,
    )
    .await?;
    if request.uri().path().starts_with("/_sigil/") {
        if grant.binding.port.is_some() {
            return Err(Error::forbidden("preview_has_no_platform_authority"));
        }
        return pages::platform(state, grant, request).await;
    }
    proxy::forward(state, &grant, request).await
}

pub(super) async fn browser_response(response: Response) -> Response {
    match controls::safe_response(response).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}
