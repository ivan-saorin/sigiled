// seald — SEAL v2 orchestrator (control plane). Session 1 scope: healthz,
// the canonical contract, and the project registry surface with
// template_version (build plan §2). Grows across sessions 2..4.
mod contract;
mod manifest;
mod project;

use axum::{routing::get, Router};
use std::net::SocketAddr;

pub fn version() -> String {
    // Build sha baked by the image build (SEAL_BUILD_SHA); absent in dev.
    match option_env!("SEAL_BUILD_SHA") {
        Some(sha) => format!("{}+{}", env!("CARGO_PKG_VERSION"), &sha[..12.min(sha.len())]),
        None => env!("CARGO_PKG_VERSION").to_string(),
    }
}

async fn healthz() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok", "version": version() }))
}

fn mgr_router(state: project::Registry) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/contract", get(contract::serve))
        .route("/projects", get(project::list))
        .with_state(state)
}

pub fn app(state: project::Registry) -> Router {
    // Same routes bare and under /mgr: the edge forwards /mgr/* verbatim,
    // a local run can use either.
    Router::new()
        .merge(mgr_router(state.clone()))
        .nest("/mgr", mgr_router(state))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_target(false).init();
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(version = %version(), %addr, "seald starting");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app(project::Registry::default()))
        .await
        .expect("serve");
}
