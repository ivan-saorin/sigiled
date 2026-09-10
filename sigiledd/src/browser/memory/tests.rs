use super::*;
use std::{
    future::IntoFuture,
    sync::atomic::{AtomicUsize, Ordering},
};
fn capabilities() -> Value {
    json!({"curation_contract":"1","response_budget":"encoded-pages-v1","browse":"live_keyset_v1","manual":"revisioned_uuid_v1","forget":"preview_suppression_v1","projection":"durable_outbox_v1","revision_encoding":"decimal_string","max_page":100,"max_scan":1000,"auth":{"oidc_verifier_configured":true}})
}
fn actor() -> Value {
    json!({"key":"human:fixture","principal_kind":"oidc","issuer":"https://idp.test","subject":"fixture"})
}
fn overlay() -> Value {
    json!({"revision":"9007199254740993","pinned":false,"archived":false,"annotations":[],"updated_by":null,"updated_at":null})
}
const ID: &str = "m_00000000-0000-4000-8000-000000000001";
fn manual_value() -> Value {
    json!({"id":ID,"revision":"9007199254740993","text":"controlled fixture","tags":[],"actor":actor(),"created_by":actor(),"updated_at":1,"deleted":false,"projection":"pending"})
}
fn chunk_value() -> Value {
    json!({"id":"legacy","idx":"atlas","text":"Echoed fixture-memory-bearer","source":"manual","ref":format!("memory:{ID}"),"path":"","span":"1-4","ts":1,"sha":"fixture-memory-bearer","tags":["fixture-memory-bearer"],"target":{"kind":"document","source":"manual","ref":format!("memory:{ID}"),"path":""},"curation":overlay()})
}
#[derive(Clone)]
struct Fake {
    cap: Arc<Mutex<Value>>,
    writes: Arc<AtomicUsize>,
    last: Arc<Mutex<Value>>,
    failure: Arc<Mutex<Option<u16>>>,
}
struct Fixture {
    state: AppState,
    svc: Service,
    fake: Fake,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn fixture_handler(State(f): State<Fake>, req: axum::extract::Request) -> Response {
    assert_eq!(
        req.headers().get("authorization").unwrap(),
        "Bearer fixture-memory-bearer"
    );
    assert!(!req.headers().contains_key("cookie"));
    assert!(!req.headers().contains_key("x-sigiled-actor"));
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let query = req.uri().query().unwrap_or("").to_owned();
    let bytes = axum::body::to_bytes(req.into_body(), 100000).await.unwrap();
    let b: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    *f.last.lock().unwrap() = json!({"path":path,"query":query,"body":b});
    if method != Method::GET {
        f.writes.fetch_add(1, Ordering::SeqCst);
    }
    if path == "/capabilities" {
        return Json(f.cap.lock().unwrap().clone()).into_response();
    }
    if let Some(status) = *f.failure.lock().unwrap() {
        if status == 299 {
            return "x".repeat(2 * 1024 * 1024 + 1).into_response();
        }
        return (
            StatusCode::from_u16(status).unwrap(),
            [("location", "/idx")],
            "fixture-memory-bearer",
        )
            .into_response();
    }
    let v = match path.as_str() {
        "/idx" => json!([{"name":"atlas","rows":0,"last_ingest":"2026-09-10T00:00:00Z"}]),
        "/idx/atlas/chunks" => {
            json!({"items":[chunk_value()],"next_cursor":"cursor-fixture-memory-bearer","scanned":1,"consistency":"live_keyset"})
        }
        "/idx/atlas/search" => {
            let mut h = chunk_value();
            h["ts"] = json!("2026-09-10T00:00:00Z");
            h["ts_epoch"] = json!(1);
            h.as_object_mut().unwrap().remove("target");
            h.as_object_mut().unwrap().remove("curation");
            json!({"q":"fixture","hits":[h]})
        }
        "/idx/atlas/manual" => {
            let mut m = manual_value();
            m["id"] = json!(format!("m_{}", b["create_id"].as_str().unwrap()));
            m["text"] = b["text"].clone();
            m
        }
        p if p.ends_with("/history") => json!({"items":[manual_value()],"next_after":null}),
        p if p.contains("/manual/") => {
            if method == Method::PUT {
                assert_eq!(b["expected_revision"], "9007199254740993");
                let mut m = manual_value();
                m["revision"] = json!("9007199254740994");
                m["projection"] = json!("failed");
                m
            } else {
                json!({"memory":manual_value(),"target":{"kind":"manual","id":ID},"curation":overlay()})
            }
        }
        "/idx/atlas/curation" => {
            assert_eq!(b["expected_revision"], "9007199254740993");
            let mut o = overlay();
            o["revision"] = json!("9007199254740994");
            json!({"target":b["target"],"curation":o})
        }
        "/idx/atlas/forget/preview" => {
            json!({"selector":b["selector"],"affected":1,"sample":[{"id":"legacy","path":"","source":"manual","ref":"memory:old"}],"confirmation":"opaque-fixture-confirmation","index_version":"9007199254740993","curation_version":"9007199254740994"})
        }
        "/idx/atlas/forget" => {
            assert_eq!(b["confirmation"], "opaque-fixture-confirmation");
            json!({"suppressed":true,"selector":b["selector"],"affected":1,"projection":{"durability":"ok","pending":1,"failed":0,"index_deletions_pending":0}})
        }
        "/idx/atlas/ingests" => {
            json!({"idx":"atlas","sources":[{"source":"file","ref":"recorded-safe-root","path":"docs","last_run":"2026-09-10T00:00:00Z","chunks":1,"head":null}],"running":null,"runs":[],"last_ingest":null})
        }
        "/idx/atlas/ingest" => {
            json!({"id":"ingest1","idx":"atlas","source":"file","ref":"recorded-safe-root","path":"docs","status":"running","started":"2026-09-10T00:00:00Z","finished":null,"docs":0,"chunks":0,"deleted":0,"error":null,"detail":null})
        }
        _ => chunk_value(),
    };
    (
        if method == Method::POST && path.ends_with("/manual") {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        Json(v),
    )
        .into_response()
}
impl Fixture {
    async fn new() -> Self {
        let fake = Fake {
            cap: Arc::new(Mutex::new(capabilities())),
            writes: Arc::default(),
            last: Arc::new(Mutex::new(Value::Null)),
            failure: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let svc = Service {
            base: format!("http://{}", listener.local_addr().unwrap()),
            ..Service::default()
        };
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .fallback(fixture_handler)
                    .with_state(fake.clone()),
            )
            .into_future(),
        );
        let mut state = AppState::test_without_runtime();
        let config = Config {
            origins: vec!["https://sigil.test".into(), "https://memory.test".into()],
            dashboard_origin: "https://sigil.test".into(),
            memory_origin: Some("https://memory.test".into()),
            ide_domain: None,
            preview_domain: None,
            issuer: "https://idp.test/issuer/".into(),
            authorization: "https://idp.test/authorize".into(),
            token: "https://idp.test/token".into(),
            jwks: "https://idp.test/jwks".into(),
            client_id: "fixture".into(),
            client_secret: None,
            scopes: "openid sigiled-groups".into(),
            idle_seconds: 900,
            absolute_seconds: 28800,
        };
        state.browser = BrowserState(Some(Arc::new(Inner {
            config,
            provider: provider::Provider::new(),
            store: Mutex::default(),
            gateway: ide_gateway::Gateway::default(),
            research: research::Services::default(),
            memory: svc.clone(),
        })));
        Self {
            state,
            svc,
            fake,
            server,
        }
    }
    fn context(&self) -> BrowserContext {
        BrowserContext {
            actor: auth::Actor {
                driver: "human:fixture".into(),
                role: auth::Role::Admin,
                approval: None,
            },
            issuer: "https://idp.test".into(),
            subject: "fixture".into(),
            display_name: None,
            access_token: "fixture-memory-bearer".into(),
            session_id: "unused".into(),
            csrf: "unused".into(),
            absolute: 0,
            idle_expires: 0,
        }
    }
}
#[test]
fn memory_revisions_validation_and_no_public_actor() {
    for r in ["0", "9007199254740993", "9223372036854775807"] {
        assert!(revision(r).is_ok());
    }
    for r in ["01", "-1", "+1", "9223372036854775808"] {
        assert!(revision(r).is_err());
    }
    assert!(content("   ", &[]).is_err());
    assert!(content(&"é".repeat(32769), &[]).is_err());
    assert!(content("ok", &["a,b".into()]).is_err());
    assert!(
        serde_json::from_value::<Create>(json!({"create_id":"x","text":"ok","actor":"admin"}))
            .is_err()
    );
    assert!(!uuid("00000000-0000-4000-8000-00000000000A"));
    assert!(Selector::Source {
        source: "manual".into(),
        refid: "a".into()
    }
    .validate(true)
    .is_err());
}
#[tokio::test]
async fn memory_exact_adapter_echo_and_legacy_search() {
    let f = Fixture::new().await;
    let Json(page) = browse(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Query(Browse::default()),
    )
    .await
    .unwrap();
    assert!(!page.to_string().contains("fixture-memory-bearer"));
    assert_eq!(page["next_cursor"], "cursor-[redacted]");
    assert_eq!(page["items"][0]["target"]["kind"], "document");
    let evidence = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/d2-adapter-echo.json");
    std::fs::create_dir_all(evidence.parent().unwrap()).unwrap();
    std::fs::write(evidence, serde_json::to_vec(&page).unwrap()).unwrap();
    *f.fake.cap.lock().unwrap() = json!({"version":"old"});
    assert!(browse(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Query(Browse::default())
    )
    .await
    .is_err());
    let Json(search) = browse(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Query(Browse {
            q: Some("fixture".into()),
            ..Browse::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!(search["items"][0]["ts"], 1);
    assert!(search["items"][0]["target"].is_null());
    assert!(!search.to_string().contains("fixture-memory-bearer"));
    assert!(f.fake.last.lock().unwrap()["query"]
        .as_str()
        .unwrap()
        .contains("mode=bm25"));
    let body: Create =
        decode(json!({"create_id":ID.trim_start_matches("m_"),"text":"valid"})).unwrap();
    assert!(create(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Json(body)
    )
    .await
    .is_err());
    assert_eq!(f.fake.writes.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn memory_manual_wrappers_cas_history_curation_and_forget() {
    let f = Fixture::new().await;
    let r = create(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Json(
            decode(json!({"create_id":ID.trim_start_matches("m_"),"text":"controlled fixture"}))
                .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let body: Value =
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 100000).await.unwrap())
            .unwrap();
    assert_eq!(body["id"], ID);
    assert!(body.get("memory").is_none());
    assert!(body.get("idx").is_none());
    let Json(detail) = manual(
        f.context(),
        State(f.state.clone()),
        Path(("atlas".into(), ID.into())),
    )
    .await
    .unwrap();
    assert_eq!(detail["memory"]["revision"], "9007199254740993");
    let r = edit(
        f.context(),
        State(f.state.clone()),
        Path(("atlas".into(), ID.into())),
        Json(decode(json!({"expected_revision":"9007199254740993","text":"edit"})).unwrap()),
    )
    .await
    .unwrap();
    let body: Value =
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 100000).await.unwrap())
            .unwrap();
    assert_eq!(body["revision"], "9007199254740994");
    assert_eq!(body["projection"], "failed");
    let Json(h) = history(
        f.context(),
        State(f.state.clone()),
        Path(("atlas".into(), ID.into())),
        Query(HistoryQuery {
            after: None,
            limit: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(h["items"][0]["revision"], "9007199254740993");
    let target = json!({"kind":"document","source":"git","ref":"repo","path":"docs/a"});
    let Json(c) = curate(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Json(
            decode(
                json!({"target":target,"expected_revision":"9007199254740993","annotation":"note"}),
            )
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(c["curation"]["revision"], "9007199254740994");
    let Json(p) = preview(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Json(decode(json!({"selector":target})).unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(p["index_version"], "9007199254740993");
    let Json(done) = forget(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Json(decode(json!({"selector":p["selector"],"confirmation":p["confirmation"]})).unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(done["projection"]["pending"], 1);
    *f.fake.failure.lock().unwrap() = Some(409);
    let result = edit(
        f.context(),
        State(f.state.clone()),
        Path(("atlas".into(), ID.into())),
        Json(decode(json!({"expected_revision":"9007199254740993","text":"stale"})).unwrap()),
    )
    .await;
    assert_eq!(result.unwrap_err().0, StatusCode::CONFLICT);
}
#[tokio::test]
async fn memory_transport_redirect_error_scrub_and_recorded_reindex() {
    let f = Fixture::new().await;
    let Json(data) = ingests(f.context(), State(f.state.clone()), Path("atlas".into()))
        .await
        .unwrap();
    let source = data["sources"][0]["source_id"].as_str().unwrap();
    let r = reindex(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Json(decode(json!({"source_id":source})).unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        f.fake.last.lock().unwrap()["body"]["ref"],
        "recorded-safe-root"
    );
    let before = f.fake.writes.load(Ordering::SeqCst);
    assert!(reindex(
        f.context(),
        State(f.state.clone()),
        Path("atlas".into()),
        Json(decode(json!({"source_id":"https://attacker.test"})).unwrap())
    )
    .await
    .is_err());
    assert_eq!(before, f.fake.writes.load(Ordering::SeqCst));
    for status in [299, 302, 401, 403, 409, 500] {
        *f.fake.failure.lock().unwrap() = Some(status);
        let e = f
            .svc
            .call(Method::GET, "idx", &[], "fixture-memory-bearer", None)
            .await
            .unwrap_err();
        assert!(!e.1.contains("fixture-memory-bearer"));
    }
    for path in [
        "https://attacker.test",
        "idx/../manual",
        "idx/atlas/export",
        "idx/atlas/chunks?ref=x",
    ] {
        assert!(f
            .svc
            .call(Method::GET, path, &[], "fixture-memory-bearer", None)
            .await
            .is_err());
    }
}
#[tokio::test]
async fn memory_origin_root_assets_are_exact() {
    let f = Fixture::new().await;
    let mut h = HeaderMap::new();
    h.insert("host", HeaderValue::from_static("memory.test"));
    let r = dashboard::shell(State(f.state.clone()), h.clone())
        .await
        .unwrap();
    let text = String::from_utf8(
        axum::body::to_bytes(r.into_body(), 10000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(text.contains("data-entry=\"memory\""));
    assert!(text.contains("memory.js"));
    assert!(
        dashboard::asset(State(f.state.clone()), h.clone(), Path("memory.js".into()))
            .await
            .is_ok()
    );
    h.insert("host", HeaderValue::from_static("unconfigured.test"));
    h.insert("x-forwarded-host", HeaderValue::from_static("memory.test"));
    assert!(dashboard::shell(State(f.state.clone()), h).await.is_err());
}

#[test]
fn memory_valid_utf8_content_fits_json_body_limit() {
    let text = "\u{0001}".repeat(65536);
    let tags = vec!["\u{0001}".repeat(128); 32];
    assert!(content(&text, &tags).is_ok());
    let encoded = serde_json::to_vec(
        &json!({"create_id":ID.trim_start_matches("m_"),"text":text,"tags":tags}),
    )
    .unwrap();
    assert!(
        encoded.len()
            <= body_limit(&Method::POST, "/browser/api/memory/indexes/atlas/manual").unwrap()
    );
    assert!(content("null\0text", &[]).is_err());
}

#[test]
fn memory_projection_target_must_match_provenance() {
    let mut row: Chunk = decode(chunk_value()).unwrap();
    assert!(row.validate("atlas").is_ok());
    row.target = Some(Selector::Manual { id: ID.into() });
    assert!(row.validate("atlas").is_err());
    row.target = None;
    assert!(row.validate("other").is_err());
}

#[tokio::test]
#[ignore = "requires exported actual Memory handler serialization"]
async fn memory_e_actual_producer_history_transport() {
    let path = std::env::var("SIGIL_MEMORY_HISTORY_FIXTURE").expect("actual producer fixture");
    let pages: Vec<String> = if path.ends_with("pages.json") {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    } else {
        vec![std::fs::read_to_string(path).unwrap()]
    };
    let mut revisions = vec![];
    for bytes in pages {
        let expected: Value = serde_json::from_str(&bytes).unwrap();
        let app = Router::new().fallback(move |headers: axum::http::HeaderMap| {
            let bytes = bytes.clone();
            async move {
                assert_eq!(headers["authorization"], "Bearer fixture-memory-bearer");
                assert!(!headers.contains_key("cookie"));
                ([("content-type", "application/json")], bytes)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let svc = Service::test_at(format!("http://{}", listener.local_addr().unwrap()));
        let task = tokio::spawn(axum::serve(listener, app).into_future());
        let result = svc
            .call(
                Method::GET,
                "idx/test/manual/m_00000000-0000-4000-8000-000000000099/history",
                &[],
                "fixture-memory-bearer",
                None,
            )
            .await;
        task.abort();
        let (_, actual) = result.expect("valid actual Memory history must cross adapter");
        assert_eq!(actual, expected);
        let typed: History = decode(actual).unwrap();
        revisions.extend(typed.items.into_iter().map(|m| m.revision));
    }
    assert_eq!(
        revisions,
        (1..=8).map(|n| n.to_string()).collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore = "requires exported actual Memory handler serialization"]
async fn memory_e_actual_producer_large_shapes_transport() {
    let path = std::env::var("SIGIL_MEMORY_SHAPES_FIXTURE").expect("actual producer fixture");
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for row in rows {
        let bytes = row["body"].as_str().unwrap().to_owned();
        let expected: Value = serde_json::from_str(&bytes).unwrap();
        let path = row["path"]
            .as_str()
            .unwrap()
            .trim_start_matches('/')
            .split('?')
            .next()
            .unwrap()
            .to_owned();
        let router = Router::new().fallback(move |h: axum::http::HeaderMap| {
            let bytes = bytes.clone();
            async move {
                assert_eq!(h["authorization"], "Bearer fixture-memory-bearer");
                assert!(!h.contains_key("cookie"));
                ([("content-type", "application/json")], bytes)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let svc = Service::test_at(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(axum::serve(listener, router).into_future());
        let result = svc
            .call(Method::GET, &path, &[], "fixture-memory-bearer", None)
            .await;
        server.abort();
        let (_, actual) =
            result.expect("complete large producer shape must cross finite adapter budget");
        assert_eq!(actual, expected);
        if path.ends_with("search") {
            let shape: Search = decode(actual).unwrap();
            assert!(shape.response_limited);
            assert_eq!(shape.candidate_count, Some(8));
        } else if path.contains("/manual/") {
            let shape: Detail = decode(actual).unwrap();
            shape.memory.validate().unwrap();
            assert_eq!(shape.curation.annotations.len(), 100);
        } else if path.ends_with("chunks") {
            let _: Page = decode(actual).unwrap();
        } else {
            let shape: Chunk = decode(actual).unwrap();
            assert_eq!(shape.curation.unwrap().annotations.len(), 100);
        }
    }
}

#[tokio::test]
#[ignore = "requires the independently observed accepted temporary repository"]
async fn memory_e_actual_chain_adapter_and_source() {
    let chain: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/e-memory-chain.json")).unwrap();
    let captured = chain.clone();
    let backend = Router::new().fallback(move |r: axum::extract::Request| {
        let chain = captured.clone();
        async move {
            assert_eq!(r.headers()["authorization"], "Bearer fixture-memory-bearer");
            assert!(!r.headers().contains_key("cookie"));
            let path = r.uri().path().to_owned();
            let method = r.method().to_string();
            if path == "/capabilities" {
                return (StatusCode::OK, Json(chain["capabilities"].clone()));
            }
            if path == "/idx/demo/enrollment" {
                return (StatusCode::OK, Json(chain["ownership"].clone()));
            }
            let body = axum::body::to_bytes(r.into_body(), 1048576).await.unwrap();
            let body: Value = if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body).unwrap()
            };
            let row = chain["operations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["path"] == path && v["method"] == method)
                .expect("actual captured route");
            assert_eq!(body, row["request"]);
            (
                StatusCode::from_u16(row["status"].as_u64().unwrap() as u16).unwrap(),
                Json(row["body"].clone()),
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let svc = Service::test_at(format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(axum::serve(listener, backend).into_future());
    let mut f = Fixture::new().await;
    Arc::get_mut(f.state.browser.0.as_mut().unwrap())
        .unwrap()
        .memory = svc.clone();
    let accepted: &Value = &chain["accepted"];
    let snapshot: crate::enrollment_contract::Snapshot =
        serde_json::from_value(accepted["snapshot"].clone()).unwrap();
    let source =
        std::env::var("SIGIL_E_ACCEPTED_REPO").expect("actual controlled accepted repository");
    let root = crate::merge::tests::tmp_repo("e-source-association");
    std::fs::create_dir_all(&root).unwrap();
    let repo = root.join("demo");
    crate::merge::git(&root, &["clone", &source, repo.to_str().unwrap()]).unwrap();
    crate::merge::git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:test/demo.git",
        ],
    )
    .unwrap();
    assert_eq!(
        crate::merge::git(&repo, &["rev-parse", "master"])
            .unwrap()
            .trim(),
        snapshot.commit
    );
    f.state.sessions = crate::sessions::SessionState::with_repos_dir(root.clone());
    f.state.github = Some(crate::github::GitHub {
        api_base: "http://127.0.0.1:1".into(),
        pat: "unused-fixture".into(),
        owner: "test".into(),
        template: "unused".into(),
        keys_dir: root.join("keys"),
    });
    f.state.registry.insert(crate::project::ProjectRecord::new(
        "demo",
        &crate::manifest::Manifest::parse("").unwrap(),
        None,
    ));
    let mut d = f.state.registry.descriptor("demo");
    let e = d.memory_enrollment.as_mut().unwrap();
    e.owner = snapshot.owner.clone();
    e.revision = snapshot.revision.clone();
    e.desired_commit = Some(snapshot.commit.clone());
    e.snapshot = Some(snapshot.clone());
    e.state = "indexed".into();
    f.state
        .registry
        .descriptors
        .write()
        .unwrap()
        .insert("demo".into(), d);
    let mut outputs = serde_json::Map::new();
    for op in chain["operations"].as_array().unwrap() {
        let action = op["action"].as_str().unwrap();
        let n = "demo".to_owned();
        let request = op["request"].clone();
        let value = match action {
            "browse" => {
                browse(
                    f.context(),
                    State(f.state.clone()),
                    Path(n),
                    Query(Browse::default()),
                )
                .await
                .unwrap()
                .0
            }
            "chunk" => {
                chunk(
                    f.context(),
                    State(f.state.clone()),
                    Path((n, op["body"]["id"].as_str().unwrap().into())),
                )
                .await
                .unwrap()
                .0
            }
            "manual" => {
                manual(
                    f.context(),
                    State(f.state.clone()),
                    Path((n, op["body"]["memory"]["id"].as_str().unwrap().into())),
                )
                .await
                .unwrap()
                .0
            }
            "create" => {
                let response = create(
                    f.context(),
                    State(f.state.clone()),
                    Path(n),
                    Json(decode(request).unwrap()),
                )
                .await
                .unwrap();
                assert_eq!(
                    response.status().as_u16(),
                    op["status"].as_u64().unwrap() as u16
                );
                serde_json::from_slice(
                    &axum::body::to_bytes(response.into_body(), 1048576)
                        .await
                        .unwrap(),
                )
                .unwrap()
            }
            "curate" => {
                curate(
                    f.context(),
                    State(f.state.clone()),
                    Path(n),
                    Json(decode(request).unwrap()),
                )
                .await
                .unwrap()
                .0
            }
            "preview" => {
                preview(
                    f.context(),
                    State(f.state.clone()),
                    Path(n),
                    Json(decode(request).unwrap()),
                )
                .await
                .unwrap()
                .0
            }
            "forget" => {
                forget(
                    f.context(),
                    State(f.state.clone()),
                    Path(n),
                    Json(decode(request).unwrap()),
                )
                .await
                .unwrap()
                .0
            }
            _ => panic!("unexpected action"),
        };
        outputs.insert(action.into(), value);
    }
    assert_eq!(outputs["chunk"]["source_edit"]["verified"], true);
    assert_eq!(
        outputs["chunk"]["source_edit"]["indexed_commit"],
        snapshot.commit
    );
    assert_eq!(
        outputs["chunk"]["source_edit"]["current_checkout"],
        "checked_on_launch"
    );
    let probe =
        crate::browser::ide_gateway::e_probe_source(repo.clone(), &outputs["chunk"]["source_edit"])
            .await;
    outputs.insert("source_probe".into(), probe);
    let encoded = serde_json::to_string(&outputs).unwrap();
    assert!(!encoded.contains("fixture-memory-bearer"));
    assert!(!encoded.contains("fixture-subject"));
    std::fs::write(
        "/workspace/target/e-memory-adapter-chain.json",
        serde_json::to_vec_pretty(&json!({"outputs":outputs,"operations":chain["operations"]}))
            .unwrap(),
    )
    .unwrap();
    server.abort();
}
