// sigiledd — SIGILED v2 orchestrator (control plane). Session 1 scope: healthz,
// the canonical contract, and the project registry surface with
// template_version (build plan §2). Session 2 adds the machine log
// (GET /sigiled/projects/{p}/log) and template_behind. Session 3 adds the
// two-legged auth (design §1): bootstrap bearer OR IdP JWT, capability map,
// device-flow approvals. Session 4 brings the verbs that consume them.
// Session 5 adds POST /projects (github.rs): create-from-template or adopt.
mod apps;
mod auth;
mod catalog;
mod contract;
mod events;
mod github;
mod import;
mod jobs;
mod manifest;
mod merge;
mod project;
mod reaper;
mod runtime;
mod sessions;
mod skill;
mod store;

use axum::{
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;

#[derive(Clone, Default)]
pub struct AppState {
    pub registry: project::Registry,
    pub events: events::EventLog,
    pub auth: auth::AuthState,
    pub sessions: sessions::SessionState,
    pub apps: apps::AppsState,
    pub jobs: jobs::JobsState,
    pub store: store::Store,
    /// Some only when GITHUB_PAT is configured: POST /projects needs it,
    /// nothing else does.
    pub github: Option<github::GitHub>,
}

impl AppState {
    /// Snapshot every store to disk (atomic, store.rs). Handlers call this
    /// after each mutation: the state file is always one rename behind the
    /// truth, never more.
    pub fn persist(&self) {
        self.store.save(&store::StateSnapshot {
            projects: self.registry.snapshot(),
            events: self.events.dump(),
            debts: self.sessions.dump_debts(),
            approvals: self.auth.approvals.dump(),
            sessions: self.sessions.dump_records(),
            apps: self.apps.dump(),
            job_runs: self.jobs.dump(),
        });
    }

    /// Boot-time inverse of persist().
    pub fn hydrate_from_disk(&self) {
        if let Some(snap) = self.store.load() {
            self.registry.replace_all(snap.projects);
            self.events.hydrate(snap.events);
            self.sessions.hydrate(snap.debts, snap.sessions);
            self.auth.approvals.hydrate(snap.approvals);
            self.apps.hydrate(snap.apps);
            self.jobs.hydrate(snap.job_runs);
            tracing::info!("state hydrated from disk");
        }
    }
}

pub fn version() -> String {
    // Build sha baked by the image build (SIGILED_BUILD_SHA); absent in dev.
    match option_env!("SIGILED_BUILD_SHA") {
        Some(sha) => format!(
            "{}+{}",
            env!("CARGO_PKG_VERSION"),
            &sha[..12.min(sha.len())]
        ),
        None => env!("CARGO_PKG_VERSION").to_string(),
    }
}

async fn healthz() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok", "version": version() }))
}

fn sigiled_router(state: AppState) -> Router {
    // healthz, the contract and the service catalog are public by design
    // (the contract is the product; the catalog names capabilities and
    // carries no secret); everything else takes an Actor (auth.rs —
    // dual-auth §1.7). One deliberate exception: /auth/verify takes no
    // Actor because its callers are the edge and composed services, which
    // are not drivers — it validates the token it is handed instead.
    Router::new()
        .route("/healthz", get(healthz))
        .route("/contract", get(contract::serve))
        .route("/services", get(catalog::serve))
        .route("/projects", get(project::list).post(project::create))
        .route("/projects/{project}/log", get(events::project_log))
        .route("/projects/{project}/branches", get(project::branches))
        .route("/projects/{project}/jobs/{job}/run", post(jobs::run))
        .route("/projects/{project}/jobs/{job}/runs", get(jobs::runs))
        .route("/projects/{project}/sessions", post(sessions::open))
        .route("/sessions/{session_id}/close", post(sessions::close))
        .route("/sessions/{session_id}/recycle", post(sessions::recycle))
        .route("/auth/elevate", post(auth::elevate))
        .route("/auth/verify", post(auth::verify))
        .route("/auth/approvals", get(auth::approvals))
        .route("/skill/{driver}", get(skill::serve))
        .route("/apps/{app}", get(apps::status))
        .route("/apps/{app}/{action}", post(apps::action))
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
    // DEC-27: a control plane with a broken service catalog must not boot.
    catalog::assert_valid();
    // One-shot import mode (cutover §6.2): `sigiledd import /v1` — reads the
    // v1 registry+keys from the given dir, merges into the v2 state, exits.
    if std::env::args().nth(1).as_deref() == Some("import") {
        let v1 = std::env::args().nth(2).unwrap_or_else(|| "/v1".into());
        match import::run(std::path::Path::new(&v1)) {
            Ok(report) => {
                print!("{report}");
                return;
            }
            Err(e) => {
                eprintln!("import failed: {e}");
                std::process::exit(1);
            }
        }
    }
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(version = %version(), %addr, "sigiledd starting");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    // Latest published template version, when the deploy knows it (the image
    // build can bake it; a dev run can export it). Absent = never "behind".
    let registry =
        project::Registry::with_latest_template(std::env::var("SIGILED_TEMPLATE_LATEST").ok());
    let state = AppState {
        registry,
        events: events::EventLog::default(),
        auth: auth::AuthState::default(),
        sessions: sessions::SessionState::default(),
        apps: apps::AppsState::default(),
        jobs: jobs::JobsState::default(),
        store: store::Store::from_env(),
        github: github::GitHub::from_env(),
    };
    state.hydrate_from_disk();
    // The reaper and the jobs scheduler patrol only where containers exist.
    if state.sessions.runtime.is_some() {
        tokio::spawn(reaper::run(state.clone()));
        tokio::spawn(jobs::scheduler(state.clone()));
    }
    axum::serve(listener, app(state)).await.expect("serve");
}
