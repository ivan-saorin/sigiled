mod auth;
mod error;
mod exec_api;
mod fs_api;
mod git_api;
mod ext_registry;
mod state;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use serde_json::json;

use state::{now_epoch, AppState};

const VERSION: &str = env!("CARGO_PKG_VERSION");

async fn health(State(st): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let now = now_epoch();
    let last = st.last_activity.load(Ordering::Relaxed);
    Json(json!({
        "status": "ok",
        "version": VERSION,
        "last_activity": last,
        "idle_secs": now - last,
        "uptime_secs": now - st.started_at,
    }))
}

/// `vm-base health-probe` — in-container docker HEALTHCHECK without curl:
/// GET /health with the token from env; exit 0 on HTTP 200.
fn health_probe() -> ! {
    use std::io::{Read, Write};
    let port = std::env::var("PORT").unwrap_or_else(|_| "8000".into());
    let token = std::env::var("SESSION_TOKEN").unwrap_or_default();
    let run = || -> std::io::Result<bool> {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port.parse().unwrap_or(8000)))?;
        s.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
        write!(
            s,
            "GET /health HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        )?;
        let mut buf = [0u8; 64];
        let n = s.read(&mut buf)?;
        Ok(std::str::from_utf8(&buf[..n]).unwrap_or("").starts_with("HTTP/1.1 200"))
    };
    std::process::exit(match run() {
        Ok(true) => 0,
        _ => 1,
    });
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("health-probe") {
        health_probe();
    }

    tracing_subscriber::fmt().with_target(false).init();

    // Deploy key (injected by MGR at creation): wire GIT_SSH_COMMAND
    // process-wide so both the git API and anything under /exec can push.
    let key = std::env::var("GIT_SSH_KEY").unwrap_or_else(|_| "/secrets/deploy_key".into());
    if std::path::Path::new(&key).exists() && std::env::var("GIT_SSH_COMMAND").is_err() {
        std::env::set_var(
            "GIT_SSH_COMMAND",
            format!("ssh -i {key} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new"),
        );
    }

    let st = Arc::new(AppState::from_env());
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8000);

    let api = Router::new()
        .route("/health", get(health))
        .route("/fs/list", get(fs_api::list))
        .route("/fs/read", post(fs_api::read))
        .route("/fs/write", post(fs_api::write))
        .route("/fs/delete", post(fs_api::delete))
        .route("/git/status", get(git_api::status))
        .route("/git/diff", get(git_api::diff))
        .route("/git/commit", post(git_api::commit))
        .route("/git/log", get(git_api::log))
        .route("/git/branches", get(git_api::branches))
        .route("/git/show", get(git_api::show))
        .route("/exec", post(exec_api::exec))
        .with_state(st.clone());

    // Extensions mount inside the same token gate: the auth layer is applied
    // after mounting, so it wraps /x/* too.
    let app = ext_registry::mount(api)
        .layer(middleware::from_fn_with_state(st.clone(), auth::require_token));

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .expect("bind");
    tracing::info!("vm-base {VERSION} listening on :{port}, workspace {}", st.workspace.display());
    axum::serve(listener, app).await.expect("serve");
}
