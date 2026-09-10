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
    checkpoint_busy: Arc<std::sync::atomic::AtomicBool>,
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
        root: "/workspace".into(),
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
    let mut child = match Process::start(&mut command) {
        Ok(p) => p,
        Err(e) => return (StatusCode::CONFLICT, Json(json!({"error":e}))).into_response(),
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
    let stopped = child.stop().is_ok();
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
    if p.is_none() {
        return Json(json!({"state":"stopped"})).into_response();
    }
    if let Some(child) = p.as_mut() {
        if let Err(e) = child.stop() {
            return (StatusCode::CONFLICT, Json(json!({"error":e}))).into_response();
        }
    }
    *p = None;
    if sigil_ide_agent::profile::save(&a.profile_root, std::path::Path::new(&a.config.profile))
        .is_err()
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"preferences_save_failed"})),
        )
            .into_response();
    }
    Json(json!({"state":"stopped"})).into_response()
}
async fn checkpoint_inner(a: Arc<Agent>, commit: bool) -> Result<String, &'static str> {
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
        let result = repo.checkpoint(commit);
        let dirty = repo.dirty().unwrap_or(true);
        (result, dirty)
    })
    .await
    .map_err(|_| "checkpoint_failed")?;
    match result {
        (Ok(sha), dirty) => {
            *a.last.lock().unwrap() = json!({"state":if dirty{"dirty"}else{"pushed"},"saved_to_disk":true,"dirty":dirty,"committed":sha,"pushed":sha,"checkpointed":commit,"merged":false});
            Ok(sha)
        }
        (Err(e), dirty) => {
            let reason = match e {
                sigil_ide_agent::durable::Error::Changed => "branch_or_remote_changed",
                sigil_ide_agent::durable::Error::Busy => "git_busy",
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
}
async fn activity(State(a): State<Arc<Agent>>, Json(r): Json<ActivityRequest>) -> StatusCode {
    if r.generation != a.config.generation.to_string() {
        return StatusCode::CONFLICT;
    }
    if a.activity.lock().unwrap().report(&r.event) {
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
    let uri = if path.is_empty() { "/" } else { path };
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
        checkpoint_busy: Default::default(),
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
        .route("/activity", post(activity))
        .route("/editor", axum::routing::any(editor))
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

#[cfg(test)]
mod tests {
    use super::*;
    fn agent() -> Arc<Agent> {
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
            checkpoint_busy: Default::default(),
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
                    .json(&json!({"generation":generation,"event":event}))
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
    async fn authenticated_relay_has_fixed_upstream_and_transports_http_and_upgrade() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:8091")
            .await
            .unwrap();
        let fake = tokio::spawn(async move {
            for upgrade in [false, true] {
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
                assert!(request.starts_with("GET /fixture"));
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
