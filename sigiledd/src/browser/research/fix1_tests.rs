use super::*;
use std::future::IntoFuture;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn health() -> Value {
    json!({"api":{"run_contract":"sde-runs-v1","pagination":true,"operation_key":"uuid_v4","durable_transitions":true,"expected_revision":true,"catalog_attempt_snapshot":true,"association":"unverified"},"persistence":{"mode":"durable","degraded":false,"recovery":"none"}})
}
fn context(token: &str) -> BrowserContext {
    BrowserContext {
        actor: Actor {
            driver: "human:fix1".into(),
            role: auth::Role::Admin,
            approval: None,
        },
        issuer: "issuer".into(),
        subject: "subject".into(),
        display_name: None,
        access_token: token.into(),
        session_id: format!("login-{token}"),
        csrf: "csrf".into(),
        absolute: u64::MAX,
        idle_expires: u64::MAX,
    }
}
fn state(base: &str, dir: &std::path::Path) -> AppState {
    let mut state = AppState::test_without_runtime();
    state.store = crate::store::Store::at_dir(dir);
    state.auth.config = Arc::new(auth::AuthConfig {
        oidc_base: Some(base.into()),
        admin_group: "stack:admins".into(),
        driver_group: "stack:drivers".into(),
        ..auth::AuthConfig::default()
    });
    let config: HashMap<String, String> = [
        ("ENABLED", "true".into()),
        ("ORIGINS", "https://sigil.test".into()),
        ("ISSUER", format!("{base}/issuer/")),
        ("AUTHORIZATION_URL", format!("{base}/authorize")),
        ("TOKEN_URL", format!("{base}/token")),
        ("JWKS_URL", format!("{base}/issuer/jwks/")),
        ("CLIENT_ID", "shared-browser-client".into()),
        ("SCOPES", "openid sigiled-groups".into()),
        ("ALLOW_LOOPBACK_HTTP", "true".into()),
    ]
    .into_iter()
    .map(|(k, v)| (format!("SIGILED_BROWSER_{k}"), v))
    .collect();
    state.browser =
        BrowserState::configured(Config::from_map(&config).unwrap(), &state.auth.config).unwrap();
    let mut services = Services::default();
    services.bases.insert("sde".into(), base.into());
    services.store = operations::Store::new(Some(dir.into()));
    Arc::get_mut(state.browser.0.as_mut().unwrap())
        .unwrap()
        .research = services;
    state.registry.insert(crate::project::ProjectRecord::new(
        "demo",
        &crate::manifest::Manifest::parse("").unwrap(),
        None,
    ));
    state
}
async fn serve(app: Router) -> (String, tokio::task::JoinHandle<std::io::Result<()>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (base, tokio::spawn(axum::serve(listener, app).into_future()))
}
async fn associate(state: &AppState) {
    let body = serde_json::from_value(
        json!({"operation_id":"fix1-operation","problem":"Fixture research"}),
    )
    .unwrap();
    let Json(r) = operations::create(
        context("initial-token"),
        State(state.clone()),
        Path("demo".into()),
        Json(body),
    )
    .await
    .unwrap();
    assert_eq!(r["run_id"], "run1");
}
#[tokio::test]
async fn fix1_summary_projection_scrubs_echoed_bearer_and_pagination() {
    let app=Router::new().route("/healthz",get(||async{Json(health())})).route("/runs",get(|h:HeaderMap|async move{
        let bearer=h["authorization"].to_str().unwrap().strip_prefix("Bearer ").unwrap();
        Json(json!({"runs":[{"run_id":"run1","problem":format!("Echoed {bearer}"),"status":"failed","created_at":format!("created {bearer}"),"revision":9007199254740993u64}],"next_cursor":format!("cursor-{bearer}")}))
    }));
    let (base, server) = serve(app).await;
    let dir = std::env::temp_dir().join(format!("b3b-summary-{}", crate::sessions::mint_token()));
    let state = state(&base, &dir);
    let Json(v) = list(
        context("fixture-summary-bearer"),
        State(state),
        Query(ListQuery {
            project: None,
            cursor: None,
        }),
    )
    .await
    .unwrap();
    assert!(!v.to_string().contains("fixture-summary-bearer"));
    assert_eq!(v["runs"][0]["problem"], "Echoed [redacted]");
    assert_eq!(v["next_cursor"], "cursor-[redacted]");
    assert_eq!(v["runs"][0]["revision"], "9007199254740993");
    std::fs::create_dir_all("target").unwrap();
    std::fs::write(
        "target/b3b-fix1-summary-projection.json",
        serde_json::to_vec(&v).unwrap(),
    )
    .unwrap();
    server.abort();
}
#[tokio::test]
async fn fix1_positive_stage_and_resume_forward_exact_revision_and_fresh_token() {
    let current = Arc::new(Mutex::new(
        json!({"run_id":"run1","problem":"Fixture research","status":"awaiting_caller","created_at":"2026-09-10","updated_at":"2026-09-10","revision":9007199254740993u64,"stages":[{"stage":"categorize","status":"done","computed_by":"genie:fixture"}],"awaiting":{"stage":"pick"},"artifacts":{"categories":[{"id":"kept"}]}}),
    ));
    let posts = Arc::new(Mutex::new(Vec::<(String, String, Value)>::new()));
    let read = current.clone();
    let seen = posts.clone();
    let stage = current.clone();
    let resumed = current.clone();
    let resumed_seen = posts.clone();
    let app = Router::new()
        .route("/healthz", get(|| async { Json(health()) }))
        .route("/runs", post(|| async { Json(json!({"run_id":"run1"})) }))
        .route(
            "/runs/run1",
            get(move || {
                let v = read.lock().unwrap().clone();
                async { Json(v) }
            }),
        )
        .route(
            "/runs/run1/stages/pick",
            post(move |h: HeaderMap, body: String| {
                let seen = seen.clone();
                let stage = stage.clone();
                async move {
                    assert!(body.contains("\"expected_revision\":9007199254740993"));
                    let v: Value = serde_json::from_str(&body).unwrap();
                    assert_eq!(v["expected_revision"].as_u64(), Some(9007199254740993));
                    seen.lock().unwrap().push((
                        "pick".into(),
                        h["authorization"].to_str().unwrap().into(),
                        v,
                    ));
                    let mut run = stage.lock().unwrap();
                    run["status"] = json!("failed");
                    run["revision"] = json!(9007199254740994u64);
                    run["awaiting"] = Value::Null;
                    Json(run.clone())
                }
            }),
        )
        .route(
            "/runs/run1/resume",
            post(move |h: HeaderMap, Json(v): Json<Value>| {
                let resumed = resumed.clone();
                let seen = resumed_seen.clone();
                async move {
                    assert_eq!(v, json!({}));
                    seen.lock().unwrap().push((
                        "resume".into(),
                        h["authorization"].to_str().unwrap().into(),
                        v,
                    ));
                    let mut run = resumed.lock().unwrap();
                    run["status"] = json!("running");
                    run["revision"] = json!(9007199254740995u64);
                    Json(run.clone())
                }
            }),
        );
    let (base, server) = serve(app).await;
    let dir = std::env::temp_dir().join(format!("b3b-stages-{}", crate::sessions::mint_token()));
    let state = state(&base, &dir);
    associate(&state).await;
    let mutation=serde_json::from_value(json!({"action":"pick","expected_revision":"9007199254740993","output":{"papers":["fixture"]}})).unwrap();
    let Json(stage) = operations::mutate(
        context("stage-token"),
        State(state.clone()),
        Path("run1".into()),
        Json(mutation),
    )
    .await
    .unwrap();
    assert_eq!(stage["revision"], "9007199254740994");
    let mutation =
        serde_json::from_value(json!({"action":"resume","expected_revision":"9007199254740994"}))
            .unwrap();
    let Json(resume) = operations::mutate(
        context("fresh-resume-token"),
        State(state.clone()),
        Path("run1".into()),
        Json(mutation),
    )
    .await
    .unwrap();
    assert_eq!(resume["revision"], "9007199254740995");
    assert_eq!(resume["artifacts"]["categories"][0]["id"], "kept");
    assert_eq!(resume["stages"][0]["status"], "done");
    let Json(detail) = detail(
        context("fresh-resume-token"),
        State(state),
        Path("run1".into()),
    )
    .await
    .unwrap();
    assert_eq!(detail["artifacts"], resume["artifacts"]);
    let seen = posts.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].1, "Bearer stage-token");
    assert_eq!(seen[1].1, "Bearer fresh-resume-token");
    server.abort();
    std::fs::remove_dir_all(dir).unwrap();
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
    assert!(out.status.success(), "fixture setup/read failed");
    String::from_utf8_lossy(&out.stdout).trim().into()
}
#[tokio::test]
async fn fix1_real_companion_size_log_marker_and_receipt_contract() {
    let dir = std::env::temp_dir().join(format!("b3b-contract-{}", crate::sessions::mint_token()));
    std::fs::create_dir_all(&dir).unwrap();
    let repo = dir.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let remote = dir.join("remote.git");
    git(&dir, &["init", "--bare", remote.to_str().unwrap()]);
    git(&repo, &["init", "-b", "session/test"]);
    git(&repo, &["commit", "--allow-empty", "-m", "base"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "origin", "session/test"]);
    let helper_posts = Arc::new(AtomicUsize::new(0));
    let drop_response = Arc::new(AtomicBool::new(false));
    let count = helper_posts.clone();
    let lose = drop_response.clone();
    let helper = sigil_ide_agent::test_support::handoff_fixture_router(
        repo.clone(),
        remote.to_string_lossy().into(),
        u64::MAX,
    )
    .layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let count = count.clone();
            let lose = lose.clone();
            async move {
                let mutation = req.method() == Method::POST && req.uri().path() == "/handoff";
                if mutation {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                let response = next.run(req).await;
                if mutation && lose.swap(false, Ordering::SeqCst) {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({"error":"fixture_lost_response"})),
                    )
                        .into_response();
                }
                response
            }
        },
    ));
    let helper_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let helper_base = format!("http://{}", helper_listener.local_addr().unwrap());
    let helper_server = tokio::spawn(axum::serve(helper_listener, helper).into_future());
    let log_good = Arc::new(AtomicBool::new(false));
    let log_calls = Arc::new(AtomicUsize::new(0));
    let valid = log_good.clone();
    let seen = log_calls.clone();
    let log_repo = repo.clone();
    let workspace=Router::new().route("/git/log",get(move|h:HeaderMap,Query(q):Query<HashMap<String,String>>|{let valid=valid.clone();let seen=seen.clone();let repo=log_repo.clone();async move{
        assert_eq!(h["authorization"],"Bearer fixture-workspace-token");assert_eq!(q["limit"],"15");seen.fetch_add(1,Ordering::SeqCst);
        if !valid.load(Ordering::SeqCst){return Json(json!({"commits":[]}));}
        // Exact vm-base git_api::log response: a top-level array of four strings.
        Json(json!([{"sha":git(&repo,&["rev-parse","HEAD"]),"author":"fixture","date":"2026-09-10T00:00:00Z","message":"base"}]))
    }}));
    let log_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let log_base = format!("http://{}", log_listener.local_addr().unwrap());
    handoff::FIXTURE_ENDPOINTS
        .lock()
        .unwrap()
        .insert("fixture".into(), (helper_base.clone(), log_base));
    let log_server = tokio::spawn(axum::serve(log_listener, workspace).into_future());
    let bundle = Arc::new(Mutex::new(
        json!({"run_id":"run1","slug":"fixture","files":[{"path":"docs/design/fixture/dossier.md","content":"\n".repeat(300000)},{"path":"docs/design/fixture/cover.md","content":"\n".repeat(300000)}]}),
    ));
    let source = bundle.clone();
    let sde = Router::new()
        .route("/healthz", get(|| async { Json(health()) }))
        .route("/runs", post(|| async { Json(json!({"run_id":"run1"})) }))
        .route(
            "/runs/run1/handoff",
            get(move || {
                let b = source.lock().unwrap().clone();
                async { Json(b) }
            }),
        );
    let (base, sde_server) = serve(sde).await;
    let state = state(&base, &dir.join("state"));
    associate(&state).await;
    let record:crate::sessions::SessionRecord=serde_json::from_value(json!({"session_id":"fixture","project":"demo","branch":"session/test","head":git(&repo,&["rev-parse","HEAD"]),"stale":false,"actor":context("initial-token").actor,"token":"fixture-workspace-token","generation":u64::MAX,"binding":{"container":"127.0.0.73","endpoint":"http://127.0.0.73:8000","generation":u64::MAX,"ide":{"provider":"fixture","helper":"fixture","base_digest":"fixture","image":"fixture","generation":u64::MAX,"profile_volume":"fixture","state":"stopped","error":null,"token":"fixture-helper-control-token-00000","activity_token":"fixture-helper-activity-token-0000"}}})).unwrap();
    state.sessions.put(record);
    let action = |b: Value| {
        serde_json::from_value(json!({"session_id":"fixture","generation":u64::MAX.to_string(),"bundle_digest":handoff::preview(b,"run1").unwrap()["digest"]})).unwrap()
    };
    let large = bundle.lock().unwrap().clone();
    let helper_body = json!({"operation_id":"oversized","run_id":"run1","generation":u64::MAX.to_string(),"slug":"fixture","files":large["files"]});
    assert!(
        serde_json::to_vec(&helper_body).unwrap().len()
            > sigil_ide_agent::handoff::MAX_REQUEST_BYTES
    );
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = http
        .post(format!("{helper_base}/handoff"))
        .bearer_auth("fixture-helper-control-token-00000")
        .json(&helper_body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let observation: Value = http
        .get(format!("{helper_base}/handoff-status/oversized"))
        .bearer_auth("fixture-helper-control-token-00000")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(observation["observation"]["phase"], "unknown");
    let before = helper_posts.load(Ordering::SeqCst);
    let error = handoff::apply(
        context("fresh-token"),
        State(state.clone()),
        Path("run1".into()),
        Json(action(large)),
    )
    .await
    .unwrap_err();
    assert_eq!(error.0, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error.1, "handoff_bundle_too_large_reduce_content");
    assert!(state.sessions.record("fixture").unwrap().handoff.is_none());
    assert_eq!(helper_posts.load(Ordering::SeqCst), before);
    assert_eq!(log_calls.load(Ordering::SeqCst), 0);
    assert!(!repo.join("docs").exists());
    let normal = json!({"run_id":"run1","slug":"fixture","files":[{"path":"docs/design/fixture/dossier.md","content":"# Approved dossier","upstream_metadata":"not a companion field"},{"path":"docs/design/fixture/cover.md","content":"Original problem"}]});
    *bundle.lock().unwrap() = normal.clone();
    assert!(handoff::apply(
        context("fresh-token"),
        State(state.clone()),
        Path("run1".into()),
        Json(action(normal.clone()))
    )
    .await
    .is_err());
    assert!(state.sessions.record("fixture").unwrap().handoff.is_none());
    assert_eq!(helper_posts.load(Ordering::SeqCst), before);
    assert!(!repo.join("docs").exists());
    log_good.store(true, Ordering::SeqCst);
    drop_response.store(true, Ordering::SeqCst);
    assert!(handoff::apply(
        context("fresh-token"),
        State(state.clone()),
        Path("run1".into()),
        Json(action(normal.clone()))
    )
    .await
    .is_err());
    let pending = state.sessions.record("fixture").unwrap();
    assert!(pending.handoff_pending());
    assert_eq!(helper_posts.load(Ordering::SeqCst), before + 1);
    let Json(receipt) = handoff::apply(
        context("refreshed-token"),
        State(state.clone()),
        Path("run1".into()),
        Json(action(normal.clone())),
    )
    .await
    .unwrap();
    assert_eq!(
        helper_posts.load(Ordering::SeqCst),
        before + 1,
        "terminal recovery must not POST again"
    );
    assert!(!state.sessions.record("fixture").unwrap().handoff_pending());
    assert_eq!(
        receipt["commit"],
        git(&remote, &["rev-parse", "refs/heads/session/test"])
    );
    assert_eq!(receipt["master_accepted"], false);
    assert_eq!(receipt["indexed"], false);
    assert_eq!(
        git(
            &remote,
            &["show", "session/test:docs/design/fixture/dossier.md"]
        ),
        "# Approved dossier"
    );
    assert_eq!(
        state.store.load().unwrap().sessions["fixture"]
            .handoff
            .as_ref()
            .unwrap()["phase"],
        "complete"
    );
    let Json(direct) = handoff::apply(
        context("fresh-direct-token"),
        State(state.clone()),
        Path("run1".into()),
        Json(action(normal)),
    )
    .await
    .unwrap();
    assert_eq!(direct["commit"], receipt["commit"]);
    assert_eq!(helper_posts.load(Ordering::SeqCst), before + 2);
    assert!(!state.sessions.record("fixture").unwrap().handoff_pending());
    helper_server.abort();
    log_server.abort();
    sde_server.abort();
    // Keep the exact contract fixture and pushed objects as review evidence.
    println!("B3B_CONTRACT fixture={} receipt={}", dir.display(), receipt);
}
