// sigiledd — SIGILED v2 orchestrator (control plane). Session 1 scope: healthz,
// the canonical contract, and the project registry surface with
// template_version (build plan §2). Session 2 adds the machine log
// (GET /sigiled/projects/{p}/log) and template_behind. Grows across
// sessions 3..4.
mod contract;
mod events;
mod manifest;
mod project;

use axum::{routing::get, Router};
use std::net::SocketAddr;

#[derive(Clone, Default)]
pub struct AppState {
    pub registry: project::Registry,
    pub events: events::EventLog,
}

pub fn version() -> String {
    // Build sha baked by the image build (SIGILED_BUILD_SHA); absent in dev.
    match option_env!("SIGILED_BUILD_SHA") {
        Some(sha) => format!("{}+{}", env!("CARGO_PKG_VERSION"), &sha[..12.min(sha.len())]),
        None => env!("CARGO_PKG_VERSION").to_string(),
    }
}

async fn healthz() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok", "version": version() }))
}

fn sigiled_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/contract", get(contract::serve))
        .route("/projects", get(project::list))
        .route("/projects/{project}/log", get(events::project_log))
        .with_state(state)
}

pub fn app(state: AppState) -> Router {
    // Same routes bare and under /sigiled: the edge forwards /sigiled/* verbatim,
    // a local run can use either.
    Router::new()
        .merge(sigiled_router(state.clone()))
        .nest("/sigiled", sigiled_router(state))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_target(false).init();
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(version = %version(), %addr, "sigiledd starting");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    // Latest published template version, when the deploy knows it (the image
    // build can bake it; a dev run can export it). Absent = never "behind".
    let registry =
        project::Registry::with_latest_template(std::env::var("SIGILED_TEMPLATE_LATEST").ok());
    let state = AppState { registry, events: events::EventLog::default() };
    axum::serve(listener, app(state)).await.expect("serve");
}
