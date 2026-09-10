#[path = "../../shared/file_target.rs"]
mod file_target;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use sigil_ide_agent::{activity::Activity, durable::Repository, process::Process};
use std::{path::PathBuf, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
#[derive(Deserialize)]
struct Config {
    token: String,
    activity_token: String,
    generation: u64,
    session: String,
    branch: String,
    remote: String,
    profile: String,
}
struct Agent {
    config: Config,
    profile_root: PathBuf,
    repository_root: PathBuf,
    editor_address: String,
    checkpoint_busy: Arc<std::sync::atomic::AtomicBool>,
    handoff_observation: std::sync::Mutex<serde_json::Value>,
    process: tokio::sync::Mutex<Option<Process>>,
    activity: std::sync::Mutex<Activity>,
    sync: Arc<tokio::sync::Mutex<()>>,
    last: std::sync::Mutex<serde_json::Value>,
}
fn denied() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"unauthorized"})),
    )
        .into_response()
}
fn valid(a: &Agent, r: &Request, activity: bool) -> bool {
    let expected = if activity {
        &a.config.activity_token
    } else {
        &a.config.token
    };
    let supplied = r
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    supplied.as_bytes().ct_eq(expected.as_bytes()).into()
}
fn repository(a: &Agent) -> Repository {
    Repository {
        root: a.repository_root.clone(),
        branch: a.config.branch.clone(),
        remote: a.config.remote.clone(),
    }
}
async fn gate(State(a): State<Arc<Agent>>, r: Request, next: axum::middleware::Next) -> Response {
    if !valid(&a, &r, r.uri().path() == "/activity") {
        return denied();
    }
    next.run(r).await
}
async fn status(State(a): State<Arc<Agent>>) -> Json<serde_json::Value> {
    let mut p = a.process.lock().await;
    let state = if let Some(p) = p.as_mut() {
        if p.running() {
            "ready"
        } else {
            "failed"
        }
    } else {
        "stopped"
    };
    let activity = a.activity.lock().unwrap();
    Json(
        json!({"generation":a.config.generation,"session":a.config.session,"state":state,"idle_secs":activity.idle_secs(),"busy":activity.busy() || a.checkpoint_busy.load(std::sync::atomic::Ordering::SeqCst),"durability":*a.last.lock().unwrap()}),
    )
}
async fn start_inner(State(a): State<Arc<Agent>>) -> Response {
    let mut guard = a.process.lock().await;
    let live = std::path::Path::new(&a.config.profile);
    if guard.is_none() && editor_marker(live).unwrap_or(true) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"editor_ownership_unknown"})),
        )
            .into_response();
    }
    if let Some(p) = guard.as_mut() {
        if p.running() {
            return Json(json!({"state":"ready"})).into_response();
        }
        if p.stop().is_err() {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":"editor_stop_unconfirmed"})),
            )
                .into_response();
        }
        if clear_editor_marker(live).is_err() {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":"editor_stop_unconfirmed"})),
            )
                .into_response();
        }
        *guard = None;
    }
    if tokio::net::TcpStream::connect("127.0.0.1:8091")
        .await
        .is_ok()
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"editor_ownership_unknown"})),
        )
            .into_response();
    }
    let profile = PathBuf::from(&a.config.profile);
    if sigil_ide_agent::profile::seed(&a.profile_root, &profile).is_err() {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"profile_setup_required"})),
        )
            .into_response();
    }
    if sigil_ide_agent::profile::mark_pending(&profile).is_err() {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"preferences_save_failed"})),
        )
            .into_response();
    }
    let user = profile.join("data");
    let extensions = profile.join("extensions");
    if std::fs::create_dir_all(user.join("User"))
        .and_then(|_| std::fs::create_dir_all(&extensions))
        .is_err()
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"profile_setup_required"})),
        )
            .into_response();
    }
    let settings = user.join("User/settings.json");
    if !settings.exists() {
        let _ = std::fs::write(
            &settings,
            r#"{"remote.autoForwardPorts":false,"security.workspace.trust.enabled":true}"#,
        );
    }
    // Extension state is session-specific. The bundled activity extension is not
    // downloaded from a marketplace and receives only the activity credential.
    let extension = extensions.join("sigil.activity-0.1.0");
    let _ = std::fs::create_dir_all(&extension);
    for name in ["package.json", "extension.js"] {
        if std::fs::copy(
            format!("/opt/sigil-ide/activity/{name}"),
            extension.join(name),
        )
        .is_err()
        {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":"activity_extension_missing"})),
            )
                .into_response();
        }
    }
    let mut command = std::process::Command::new("/opt/sigil-ide/code-server/bin/code-server");
    command
        .args([
            "--bind-addr",
            "127.0.0.1:8091",
            "--auth",
            "none",
            "--disable-proxy",
            "--app-name",
            "Sigil Workspace",
            "--user-data-dir",
        ])
        .arg(&user)
        .arg("--extensions-dir")
        .arg(&extensions)
        .arg("--config")
        .arg(profile.join("config.yaml"))
        .arg("/workspace")
        .env_remove("SESSION_TOKEN")
        .env("XDG_CONFIG_HOME", profile.join("config"))
        .env("XDG_DATA_HOME", profile.join("data-home"))
        .env("SIGIL_IDE_ACTIVITY_TOKEN", &a.config.activity_token)
        .env("SIGIL_IDE_GENERATION", a.config.generation.to_string());
    if std::fs::write(profile.join(".editor-running"), []).is_err() {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"profile_setup_required"})),
        )
            .into_response();
    }
    let mut child = match Process::start(&mut command) {
        Ok(p) => p,
        Err(e) => {
            let _ = clear_editor_marker(&profile);
            return (StatusCode::CONFLICT, Json(json!({"error":e}))).into_response();
        }
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    for _ in 0..30 {
        if !child.running() {
            break;
        }
        if client
            .get("http://127.0.0.1:8091/healthz")
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            *guard = Some(child);
            return Json(json!({"state":"ready","generation":a.config.generation})).into_response();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let stopped = child.stop().is_ok() && clear_editor_marker(&profile).is_ok();
    if !stopped {
        *guard = Some(child)
    }
    (StatusCode::CONFLICT,Json(json!({"state":"failed","error":if stopped{"editor_start_failed"}else{"editor_stop_unconfirmed"}}))).into_response()
}
async fn stop_inner(State(a): State<Arc<Agent>>) -> Response {
    if a.activity.lock().unwrap().busy() {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"user_command_running"})),
        )
            .into_response();
    }
    let mut p = a.process.lock().await;
    match stop_locked(&a, &mut p).await {
        Ok(()) => Json(json!({"state":"stopped"})).into_response(),
        Err(e) => (StatusCode::CONFLICT, Json(json!({"error":e}))).into_response(),
    }
}
async fn stop_locked(a: &Agent, p: &mut Option<Process>) -> Result<(), &'static str> {
    let live = std::path::Path::new(&a.config.profile);
    if p.is_some() {
        sigil_ide_agent::profile::mark_pending(live).map_err(|_| "preferences_save_failed")?;
    }
    if p.is_none() && editor_marker(live).map_err(|_| "editor_ownership_unknown")? {
        return Err("editor_ownership_unknown");
    }
    if let Some(child) = p.as_mut() {
        child.stop()?;
        clear_editor_marker(live).map_err(|_| "editor_stop_unconfirmed")?;
    }
    *p = None;
    // A live listener after owned cleanup (or a companion restart) is not ours
    // to adopt or snapshot. Only a refused connection confirms it is absent.
    match tokio::time::timeout(
        Duration::from_secs(1),
        tokio::net::TcpStream::connect(&a.editor_address),
    )
    .await
    {
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
        _ => return Err("editor_ownership_unknown"),
    }
    sigil_ide_agent::profile::save_pending(&a.profile_root, live)
        .map_err(|_| "preferences_save_failed")
}
fn editor_marker(live: &std::path::Path) -> std::io::Result<bool> {
    live.join(".editor-running").try_exists()
}
fn clear_editor_marker(live: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_file(live.join(".editor-running")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
async fn finish_action(State(a): State<Arc<Agent>>) -> Response {
    // Holding provider ownership prevents a concurrent restart while its
    // process group is quiesced and the final snapshot is verified.
    let mut process = a.process.lock().await;
    if a.activity.lock().unwrap().busy() {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"user_command_running"})),
        )
            .into_response();
    }
    if let Err(e) = stop_locked(&a, &mut process).await {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":e,"work_preserved":true})),
        )
            .into_response();
    }
    match checkpoint_mode(a.clone(), true, true).await {
        Ok(sha) => Json(
            json!({"state":"finished","generation":a.config.generation,"sha":sha,"dirty":false}),
        )
        .into_response(),
        Err(e) => (
            StatusCode::CONFLICT,
            Json(json!({"error":e,"work_preserved":true})),
        )
            .into_response(),
    }
}
async fn checkpoint_inner(a: Arc<Agent>, commit: bool) -> Result<String, &'static str> {
    checkpoint_mode(a, commit, false).await
}
async fn checkpoint_mode(
    a: Arc<Agent>,
    commit: bool,
    require_clean: bool,
) -> Result<String, &'static str> {
    let serial = a.sync.clone().lock_owned().await;
    let repo = repository(&a);
    let busy = a.checkpoint_busy.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _serial = serial;
        struct Busy(Arc<std::sync::atomic::AtomicBool>, bool);
        impl Drop for Busy {
            fn drop(&mut self) {
                if self.1 {
                    self.0.store(false, std::sync::atomic::Ordering::SeqCst)
                }
            }
        }
        let _busy = Busy(busy.clone(), commit);
        if commit {
            busy.store(true, std::sync::atomic::Ordering::SeqCst)
        }
        let result = if require_clean {
            repo.checkpoint_clean()
        } else {
            repo.checkpoint(commit)
        };
        let dirty = repo.dirty().unwrap_or(true);
        (result, dirty)
    })
    .await
    .map_err(|_| "checkpoint_failed")?;
    match result {
        (Ok(sha), dirty) => {
            *a.last.lock().unwrap() = json!({"state":if dirty{"dirty"}else{"pushed"},"saved_to_disk":true,"dirty":dirty,"committed":sha,"pushed":sha,"checkpointed":commit,"merged":false});
            if require_clean && dirty {
                Err("workspace_changed")
            } else {
                Ok(sha)
            }
        }
        (Err(e), dirty) => {
            let reason = match e {
                sigil_ide_agent::durable::Error::Changed => "branch_or_remote_changed",
                sigil_ide_agent::durable::Error::Busy => "git_busy",
                sigil_ide_agent::durable::Error::Dirty => "workspace_changed",
                _ => "push_or_commit_failed",
            };
            *a.last.lock().unwrap() =
                json!({"state":"paused","error":reason,"dirty":dirty,"work_preserved":true});
            Err(reason)
        }
    }
}
async fn checkpoint_action(State(a): State<Arc<Agent>>) -> Response {
    if a.activity.lock().unwrap().busy() {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"user_command_running"})),
        )
            .into_response();
    }
    match checkpoint_inner(a, true).await {
        Ok(sha) => Json(json!({"state":"checkpointed","sha":sha})).into_response(),
        Err(e) => (
            StatusCode::CONFLICT,
            Json(json!({"error":e,"work_preserved":true})),
        )
            .into_response(),
    }
}
#[derive(Deserialize)]
struct ActivityRequest {
    generation: String,
    event: String,
    execution_id: Option<String>,
}
async fn activity(State(a): State<Arc<Agent>>, Json(r): Json<ActivityRequest>) -> StatusCode {
    if r.generation != a.config.generation.to_string() {
        return StatusCode::CONFLICT;
    }
    if a.activity
        .lock()
        .unwrap()
        .report_execution(&r.event, r.execution_id.as_deref())
    {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::BAD_REQUEST
    }
}
/// Fixed loopback upstream only. C2 supplies per-origin browser authorization;
/// every HTTP request and WebSocket handshake also requires the custodied token.
async fn editor(State(_a): State<Arc<Agent>>, mut request: Request) -> Response {
    let upgrade = request.headers().get(header::UPGRADE).is_some();
    let browser_upgrade = if upgrade {
        Some(hyper::upgrade::on(&mut request))
    } else {
        None
    };
    request.headers_mut().remove(header::AUTHORIZATION);
    request.headers_mut().remove(header::COOKIE);
    let Some(path) = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .and_then(|p| p.strip_prefix("/editor"))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let normalized = if path.is_empty() || path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_owned()
    };
    let uri = normalized.as_str();
    let Ok(uri) = uri.parse() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    *request.uri_mut() = uri;
    let stream = match tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect("127.0.0.1:8091"),
    )
    .await
    {
        Ok(Ok(s)) => s,
        _ => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let Ok((mut sender, connection)) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await
    else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let mut response = match sender.send_request(request).await {
        Ok(r) => r,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(browser) = browser_upgrade {
            let upstream = hyper::upgrade::on(&mut response);
            tokio::spawn(async move {
                if let (Ok(b), Ok(u)) = tokio::join!(browser, upstream) {
                    let _ = tokio::io::copy_bidirectional(
                        &mut hyper_util::rt::TokioIo::new(b),
                        &mut hyper_util::rt::TokioIo::new(u),
                    )
                    .await;
                }
            });
        }
    }
    response.headers_mut().remove(header::SET_COOKIE);
    response.map(Body::new)
}
#[tokio::main]
async fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("configuration path required");
    let mut config: Config =
        serde_json::from_slice(&std::fs::read(path).expect("configuration unavailable"))
            .expect("invalid configuration");
    assert!(config.token.len() >= 32 && config.activity_token.len() >= 32);
    unsafe {
        libc::umask(0o077);
    }
    let profile_root = PathBuf::from(format!("/sigil-profile/uid-{}", unsafe { libc::geteuid() }));
    config.profile = profile_root
        .join(&config.session)
        .join(config.generation.to_string())
        .to_string_lossy()
        .into();
    let a = Arc::new(Agent {
        config,
        profile_root,
        repository_root: "/workspace".into(),
        editor_address: "127.0.0.1:8091".into(),
        checkpoint_busy: Default::default(),
        handoff_observation: Default::default(),
        process: Default::default(),
        activity: Default::default(),
        sync: Default::default(),
        last: std::sync::Mutex::new(json!({"state":"saved_to_disk"})),
    });
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8090")
        .await
        .expect("companion address in use");
    let background = a.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if background.process.lock().await.is_some() {
                let _ = checkpoint_inner(background.clone(), false).await;
            }
        }
    });
    axum::serve(listener, router(a))
        .await
        .expect("companion server failed");
}

fn router(a: Arc<Agent>) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/start", post(start))
        .route("/stop", post(stop))
        .route("/checkpoint", post(checkpoint))
        .route("/finish", post(finish))
        .route(
            "/handoff",
            post(handoff).layer(axum::extract::DefaultBodyLimit::max(1100000)),
        )
        .route("/handoff-status/{id}", get(handoff_status))
        .route("/activity", post(activity))
        .route(
            "/file-target",
            post(file_target_probe).layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route("/editor", axum::routing::any(editor))
        .route("/editor/", axum::routing::any(editor))
        .route("/editor/{*path}", axum::routing::any(editor))
        .layer(axum::middleware::from_fn_with_state(a.clone(), gate))
        .with_state(a)
}

async fn start(State(a): State<Arc<Agent>>) -> Response {
    match tokio::spawn(start_inner(State(a))).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::CONFLICT,
            Json(json!({"error":"operation_interrupted"})),
        )
            .into_response(),
    }
}

async fn stop(State(a): State<Arc<Agent>>) -> Response {
    match tokio::spawn(stop_inner(State(a))).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::CONFLICT,
            Json(json!({"error":"operation_interrupted"})),
        )
            .into_response(),
    }
}

async fn finish(State(a): State<Arc<Agent>>) -> Response {
    match tokio::spawn(finish_action(State(a))).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::CONFLICT,
            Json(json!({"error":"operation_interrupted"})),
        )
            .into_response(),
    }
}
async fn checkpoint(State(a): State<Arc<Agent>>) -> Response {
    match tokio::spawn(checkpoint_action(State(a))).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::CONFLICT,
            Json(json!({"error":"operation_interrupted"})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileProbe {
    generation: String,
    target: file_target::FileTarget,
}
async fn file_target_probe(State(a): State<Arc<Agent>>, Json(probe): Json<FileProbe>) -> Response {
    if probe.generation != a.config.generation.to_string() || !probe.target.validate() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_file_target"})),
        )
            .into_response();
    }
    let valid = std::fs::canonicalize(&a.repository_root)
        .ok()
        .and_then(|root| {
            std::fs::canonicalize(root.join(&probe.target.path))
                .ok()
                .map(|p| p.starts_with(root) && p.is_file())
        })
        .unwrap_or(false);
    if !valid {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"source_missing_or_moved"})),
        )
            .into_response();
    }
    Json(json!({"exists":true,"generation":a.config.generation.to_string()})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn agent() -> Arc<Agent> {
        Arc::new(Agent {
            config: Config {
                token: "control-generation-one-0123456789".into(),
                activity_token: "activity-generation-one-0123456789".into(),
                generation: u64::MAX,
                session: "fixture".into(),
                branch: "session/fixture".into(),
                remote: "/unreachable".into(),
                profile: "/workspace/target/fixture".into(),
            },
            profile_root: "/workspace/target/fixture".into(),
            repository_root: "/workspace".into(),
            editor_address: "127.0.0.1:0".into(),
            checkpoint_busy: Default::default(),
            handoff_observation: Default::default(),
            process: Default::default(),
            activity: Default::default(),
            sync: Default::default(),
            last: std::sync::Mutex::new(json!({"state":"saved_to_disk"})),
        })
    }
    async fn serve(a: Arc<Agent>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router(a)).await.unwrap() });
        (url, task)
    }
    struct StopFixture(Arc<Agent>);
    impl Drop for StopFixture {
        fn drop(&mut self) {
            if let Ok(mut p) = self.0.process.try_lock() {
                if let Some(p) = p.as_mut() {
                    let _ = p.stop();
                }
            }
        }
    }
    fn git(root: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().into()
    }
    #[tokio::test]
    async fn finish_quiesces_owned_writer_then_rejects_concurrent_save_and_allows_clean_retry() {
        use std::os::unix::fs::PermissionsExt;
        let root = PathBuf::from(format!(
            "/workspace/target/ide-fix-finish-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        let remote = root.join("remote.git");
        git(&root, &["init", "--bare", remote.to_str().unwrap()]);
        git(&repo, &["init", "-b", "session/fixture"]);
        git(&repo, &["commit", "--allow-empty", "-m", "initial"]);
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&repo, &["push", "origin", "session/fixture"]);
        let mut a = Arc::try_unwrap(agent()).ok().unwrap();
        a.repository_root = repo.clone();
        a.config.remote = remote.to_string_lossy().into();
        a.profile_root = root.join("profiles");
        a.config.profile = a.profile_root.join("live").to_string_lossy().into();
        sigil_ide_agent::profile::seed(&a.profile_root, std::path::Path::new(&a.config.profile))
            .unwrap();
        std::fs::write(
            std::path::Path::new(&a.config.profile).join(".editor-running"),
            [],
        )
        .unwrap();
        let pid_file = root.join("writer-pid");
        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            &format!("echo $$ > '{}'; exec sleep 60", pid_file.display()),
        ]);
        *a.process.get_mut() = Some(Process::start(&mut command).unwrap());
        let a = Arc::new(a);
        let _cleanup = StopFixture(a.clone());
        for _ in 0..100 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid = std::fs::read_to_string(pid_file).unwrap();
        assert!(
            pid.trim().parse::<u32>().unwrap() > 1,
            "fixture owns a real PID"
        );
        assert!(a.process.lock().await.as_mut().unwrap().running());
        let hook = remote.join("hooks/pre-receive");
        // Fail loudly if the owned editor is still alive when the push begins.
        // Then emulate an independent agent save inside the push barrier.
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nkill -0 {} 2>/dev/null && exit 1\nprintf concurrent > '{}'\n",
                pid.trim(),
                repo.join("saved").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(repo.join("saved"), "before-push").unwrap();
        let (url, server) = serve(a.clone()).await;
        let http = reqwest::Client::new();
        let response = http
            .post(format!("{url}/finish"))
            .bearer_auth(&a.config.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let value: serde_json::Value = response.json().await.unwrap();
        assert_eq!(value["error"], "workspace_changed");
        assert!(a.process.lock().await.is_none());
        assert!(!editor_marker(std::path::Path::new(&a.config.profile)).unwrap());
        assert_eq!(
            std::fs::read_to_string(repo.join("saved")).unwrap(),
            "concurrent"
        );
        assert_eq!(
            git(&remote, &["show", "session/fixture:saved"]),
            "before-push"
        );
        std::fs::remove_file(hook).unwrap();
        let response = http
            .post(format!("{url}/finish"))
            .bearer_auth(&a.config.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: serde_json::Value = response.json().await.unwrap();
        assert_eq!(value["state"], "finished");
        assert_eq!(value["dirty"], false);
        assert_eq!(
            git(&remote, &["show", "session/fixture:saved"]),
            "concurrent"
        );
        server.abort();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn restart_with_unknown_editor_listener_preserves_pending_preferences_and_workspace() {
        let root = PathBuf::from(format!(
            "/workspace/target/ide-fix-unknown-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let mut a = Arc::try_unwrap(agent()).ok().unwrap();
        a.profile_root = root.clone();
        a.config.profile = root.join("live").to_string_lossy().into();
        sigil_ide_agent::profile::seed(&root, std::path::Path::new(&a.config.profile)).unwrap();
        sigil_ide_agent::profile::mark_pending(std::path::Path::new(&a.config.profile)).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        a.editor_address = listener.local_addr().unwrap().to_string();
        assert!(tokio::net::TcpStream::connect(&a.editor_address)
            .await
            .is_ok());
        let a = Arc::new(a);
        let response = finish_action(State(a.clone())).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            sigil_ide_agent::profile::pending(std::path::Path::new(&a.config.profile)).unwrap(),
            "unknown writer must be quiesced before publishing preferences"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"], "editor_ownership_unknown");
        assert!(
            tokio::net::TcpStream::connect(&a.editor_address)
                .await
                .is_ok(),
            "unknown process must not be killed"
        );
        std::fs::write(
            std::path::Path::new(&a.config.profile).join(".editor-running"),
            [],
        )
        .unwrap();
        drop(listener);
        let response = finish_action(State(a.clone())).await;
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value["error"], "editor_ownership_unknown",
            "lost ownership marker also blocks a not-yet-listening editor"
        );
        assert!(
            sigil_ide_agent::profile::pending(std::path::Path::new(&a.config.profile)).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn preference_failure_remains_retryable_after_owned_process_stops() {
        let mut a = Arc::try_unwrap(agent()).ok().unwrap();
        let root = PathBuf::from(format!(
            "/workspace/target/ide-fix-preferences-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        a.profile_root = root.clone();
        a.config.profile = root.join("live").to_string_lossy().into();
        sigil_ide_agent::profile::seed(&root, std::path::Path::new(&a.config.profile)).unwrap();
        std::fs::create_dir_all(root.join("live/data/User")).unwrap();
        std::fs::write(root.join("live/data/User/settings.json"), "new preferences").unwrap();
        // A directory at the atomic pointer destination makes publication fail.
        std::fs::create_dir(root.join("preferences-current")).unwrap();
        std::fs::write(
            std::path::Path::new(&a.config.profile).join(".editor-running"),
            [],
        )
        .unwrap();
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("60");
        *a.process.get_mut() = Some(Process::start(&mut cmd).unwrap());
        let a = Arc::new(a);
        assert_eq!(
            stop_inner(State(a.clone())).await.status(),
            StatusCode::CONFLICT
        );
        assert!(a.process.lock().await.is_none());
        assert!(!editor_marker(std::path::Path::new(&a.config.profile)).unwrap());
        assert!(
            sigil_ide_agent::profile::pending(std::path::Path::new(&a.config.profile)).unwrap()
        );
        assert_eq!(
            finish_action(State(a.clone())).await.status(),
            StatusCode::CONFLICT,
            "finish cannot forget unpublished preferences"
        );
        assert_eq!(
            stop_inner(State(a.clone())).await.status(),
            StatusCode::CONFLICT,
            "retry cannot forget unpublished preferences"
        );
        // Reconstruct helper state with no process slot, as on a companion
        // restart: unresolved publication must remain observable from disk.
        let mut restarted = Arc::try_unwrap(agent()).ok().unwrap();
        restarted.profile_root = a.profile_root.clone();
        restarted.config.profile = a.config.profile.clone();
        drop(a);
        let a = Arc::new(restarted);
        assert_eq!(
            stop_inner(State(a.clone())).await.status(),
            StatusCode::CONFLICT
        );
        std::fs::remove_dir(root.join("preferences-current")).unwrap();
        assert_eq!(stop_inner(State(a.clone())).await.status(), StatusCode::OK);
        let next = root.join("next");
        sigil_ide_agent::profile::seed(&root, &next).unwrap();
        assert_eq!(
            std::fs::read_to_string(next.join("data/User/settings.json")).unwrap(),
            "new preferences"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn credentials_generation_and_background_activity_are_isolated() {
        let a = agent();
        let (url, server) = serve(a.clone()).await;
        let http = reqwest::Client::new();
        for path in ["status", "editor/workspace"] {
            assert_eq!(
                http.get(format!("{url}/{path}"))
                    .header("Cookie", "session=anything")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            http.post(format!("{url}/activity"))
                .bearer_auth(&a.config.token)
                .json(&json!({"generation":u64::MAX.to_string(),"event":"edit"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        for (generation, event, status) in [
            ("1".into(), "edit", StatusCode::CONFLICT),
            (u64::MAX.to_string(), "heartbeat", StatusCode::BAD_REQUEST),
            (
                u64::MAX.to_string(),
                "command_start",
                StatusCode::NO_CONTENT,
            ),
            (u64::MAX.to_string(), "command_end", StatusCode::NO_CONTENT),
        ] {
            assert_eq!(
                http.post(format!("{url}/activity"))
                    .bearer_auth(&a.config.activity_token)
                    .json(
                        &json!({"generation":generation,"event":event,"execution_id":"fixture:1"})
                    )
                    .send()
                    .await
                    .unwrap()
                    .status(),
                status
            );
        }
        let _background = a.sync.lock().await;
        let status: serde_json::Value = http
            .get(format!("{url}/status"))
            .bearer_auth(&a.config.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(status["busy"], false);
        assert!(!status.to_string().contains(&a.config.token));
        server.abort();
    }
    #[tokio::test]
    async fn file_target_probe_checks_generation_missing_and_symlink_escape() {
        let root = std::env::temp_dir().join(format!("gateway-file-probe-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("source.txt"), "source").unwrap();
        std::os::unix::fs::symlink("/etc/hosts", root.join("escape.txt")).unwrap();
        let mut a = agent();
        Arc::get_mut(&mut a).unwrap().repository_root = root.clone();
        let (url, server) = serve(a.clone()).await;
        let http = reqwest::Client::new();
        for (path, generation, expected) in [
            ("source.txt", u64::MAX.to_string(), StatusCode::OK),
            ("missing.txt", u64::MAX.to_string(), StatusCode::NOT_FOUND),
            ("escape.txt", u64::MAX.to_string(), StatusCode::NOT_FOUND),
            ("../outside", "1".into(), StatusCode::BAD_REQUEST),
            ("source.txt", "1".into(), StatusCode::BAD_REQUEST),
        ] {
            let response = http
                .post(format!("{url}/file-target"))
                .bearer_auth(&a.config.token)
                .json(&json!({"generation":generation,"target":{"path":path,"line":2,"column":3}}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            assert!(!response.text().await.unwrap().contains(&a.config.token));
        }
        server.abort();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn authenticated_relay_has_fixed_upstream_and_transports_http_and_upgrade() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:8091")
            .await
            .unwrap();
        let fake = tokio::spawn(async move {
            for (upgrade, path) in [
                (false, "/fixture?q=1"),
                (false, "/?folder=%2Fworkspace&payload=kept"),
                (false, "/?folder=%2Fworkspace&payload=kept"),
                (true, "/fixture"),
            ] {
                let (mut socket, _) = upstream.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut b = [0];
                    socket.read_exact(&mut b).await.unwrap();
                    request.push(b[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                assert!(
                    request.starts_with(&format!("GET {path} HTTP/1.1")),
                    "query and root path must survive relay"
                );
                assert!(!request.to_lowercase().contains("authorization:"));
                assert!(!request.to_lowercase().contains("cookie:"));
                if upgrade {
                    socket.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").await.unwrap();
                    let mut bytes = [0; 4];
                    socket.read_exact(&mut bytes).await.unwrap();
                    socket.write_all(&bytes).await.unwrap();
                } else {
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nSet-Cookie: forbidden=1\r\n\r\nok").await.unwrap();
                }
            }
        });
        let a = agent();
        let (url, server) = serve(a.clone()).await;
        let http = reqwest::Client::new();
        let r = http
            .get(format!("{url}/editor/fixture?q=1"))
            .bearer_auth(&a.config.token)
            .header("Cookie", "private=secret")
            .send()
            .await
            .unwrap();
        assert!(!r.headers().contains_key("set-cookie"));
        assert_eq!(r.text().await.unwrap(), "ok");
        for path in [
            "/editor?folder=%2Fworkspace&payload=kept",
            "/editor/?folder=%2Fworkspace&payload=kept",
        ] {
            let r = http
                .get(format!("{url}{path}"))
                .bearer_auth(&a.config.token)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::OK);
            assert_eq!(r.text().await.unwrap(), "ok");
        }
        let mut socket = tokio::net::TcpStream::connect(url.trim_start_matches("http://"))
            .await
            .unwrap();
        socket.write_all(format!("GET /editor/fixture HTTP/1.1\r\nHost: fixture\r\nAuthorization: Bearer {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",a.config.token).as_bytes()).await.unwrap();
        let mut response = Vec::new();
        loop {
            let mut b = [0];
            socket.read_exact(&mut b).await.unwrap();
            response.push(b[0]);
            if response.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        assert!(response.starts_with(b"HTTP/1.1 101"));
        socket.write_all(b"ping").await.unwrap();
        let mut echo = [0; 4];
        socket.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"ping");
        fake.await.unwrap();
        server.abort();
    }
}

async fn handoff(
    State(a): State<Arc<Agent>>,
    Json(r): Json<sigil_ide_agent::handoff::Request>,
) -> Response {
    if r.generation != a.config.generation.to_string()
        || sigil_ide_agent::handoff::validate(&r).is_err()
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"invalid_handoff_generation_or_bundle"})),
        )
            .into_response();
    }
    {
        let mut observation = a.handoff_observation.lock().unwrap();
        if observation["phase"] == "busy" {
            return (StatusCode::CONFLICT, Json(json!({"error":"handoff_busy"}))).into_response();
        }
        *observation = json!({"operation_id":r.operation_id,"phase":"busy"});
    }
    let operation_id = r.operation_id.clone();
    match tokio::spawn(async move {
        let execute = async {
            let mut process = a.process.lock().await;
            if a.activity.lock().unwrap().busy() {
                return Err("user_command_running");
            }
            stop_locked(&a, &mut process).await?;
            let serial = a.sync.clone().lock_owned().await;
            let repo = repository(&a);
            let result = tokio::task::spawn_blocking(move || {
                let _serial = serial;
                sigil_ide_agent::handoff::apply(&repo, r)
            })
            .await
            .map_err(|_| "handoff_interrupted")?;
            drop(process);
            result
        }
        .await;
        match execute {
            Ok(r) => {
                *a.handoff_observation.lock().unwrap() =
                    json!({"operation_id":operation_id,"phase":"complete","receipt":r});
                Json(r).into_response()
            }
            Err(e) => {
                *a.handoff_observation.lock().unwrap() =
                    json!({"operation_id":operation_id,"phase":"failed","error":e});
                (
                    StatusCode::CONFLICT,
                    Json(json!({"error":e,"work_preserved":true})),
                )
                    .into_response()
            }
        }
    })
    .await
    {
        Ok(r) => r,
        Err(_) => (
            StatusCode::CONFLICT,
            Json(json!({"error":"handoff_interrupted","work_preserved":true})),
        )
            .into_response(),
    }
}

async fn handoff_status(
    State(a): State<Arc<Agent>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let v = a.handoff_observation.lock().unwrap();
    Json(
        json!({"contract":"sigil-handoff-v1","generation":a.config.generation.to_string(),"operation_id":id,"observation":if v["operation_id"]==id{v.clone()}else{json!({"phase":"unknown"})}}),
    )
}

#[cfg(test)]
mod handoff_owner_tests {
    use super::*;
    #[tokio::test]
    async fn handoff_rejects_unknown_editor_before_files_and_publishes_terminal_failure() {
        let mut a = super::tests::agent();
        let dir = std::env::temp_dir().join(format!("handoff-owner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".editor-running"), []).unwrap();
        Arc::get_mut(&mut a).unwrap().config.profile = dir.to_string_lossy().into();
        let r = sigil_ide_agent::handoff::Request {
            generation: u64::MAX.to_string(),
            operation_id: "operation1".into(),
            run_id: "run1".into(),
            slug: "fixture".into(),
            files: vec![
                sigil_ide_agent::handoff::File {
                    path: "docs/design/fixture/cover.md".into(),
                    content: "cover".into(),
                },
                sigil_ide_agent::handoff::File {
                    path: "docs/design/fixture/dossier.md".into(),
                    content: "dossier".into(),
                },
            ],
        };
        let response = handoff(State(a.clone()), Json(r)).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["error"],
            "editor_ownership_unknown"
        );
        let Json(status) = handoff_status(State(a), axum::extract::Path("operation1".into())).await;
        assert_eq!(status["observation"]["phase"], "failed");
        assert!(dir.join(".editor-running").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
