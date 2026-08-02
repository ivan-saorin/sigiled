use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::state::{now_epoch, AppState};

/// Session token gate (§5 auth). Layered under Caddy's bearer; a token minted
/// for project A must be rejected by project B's container — enforced simply
/// because each container knows only its own token.
pub async fn require_token(
    State(st): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let ok = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|got| {
            let (a, b) = (got.as_bytes(), st.token.as_bytes());
            a.len() == b.len() && bool::from(a.ct_eq(b))
        })
        .unwrap_or(false);

    if !ok {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "detail": "bad session token" })))
            .into_response();
    }

    // Reaper input (§4): every authorized API call except /health bumps
    // last_activity. /health is exempt so the reaper's own polling (and the
    // docker healthcheck) can never keep an idle session alive.
    if req.uri().path() != "/health" {
        st.last_activity.store(now_epoch(), Ordering::Relaxed);
    }

    next.run(req).await
}
