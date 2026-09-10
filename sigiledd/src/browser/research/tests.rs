use super::*;
use std::future::IntoFuture;
#[test]
fn older_and_degraded_engines_cannot_start() {
    assert!(!ready(&json!({"status":"ok","version":"0.1.0"})));
    let mut v = health();
    assert!(ready(&v));
    v["persistence"]["degraded"] = json!(true);
    assert!(!ready(&v));
    v["persistence"]["degraded"] = json!(false);
    v["persistence"]["mode"] = json!("memory_only");
    assert!(!ready(&v));
}
fn health() -> Value {
    json!({"api":{"run_contract":"sde-runs-v1","pagination":true,"operation_key":"uuid_v4","durable_transitions":true,"expected_revision":true,"catalog_attempt_snapshot":true,"association":"unverified"},"persistence":{"mode":"durable","degraded":false,"recovery":"none"}})
}
#[test]
fn revision_and_paths_are_lossless_and_closed() {
    assert_eq!(revision("18446744073709551615").unwrap(), u64::MAX);
    assert!(revision("09007199254740993").is_err());
    for s in ["..", "a/b", "%2f", "a?b", "a#b", ""] {
        assert!(!identifier(s));
    }
    let mut v = json!({"revision":9007199254740993u64});
    project_revision(&mut v);
    assert_eq!(v["revision"], "9007199254740993");
}
#[tokio::test]
async fn exact_token_no_cookies_redirect_bounds_and_old_capability() {
    use axum::routing::get;
    let app = Router::new()
        .route(
            "/runs",
            get(|h: HeaderMap| async move {
                assert_eq!(h.get("authorization").unwrap(), "Bearer originator-token");
                assert!(!h.contains_key("cookie"));
                Json(json!({"runs":[]}))
            }),
        )
        .route(
            "/healthz",
            get(|| async { Json(json!({"status":"ok","version":"0.1.0"})) }),
        )
        .route(
            "/info",
            get(|| async {
                (
                    StatusCode::FOUND,
                    [("location", "/runs")],
                    "private upstream log",
                )
            }),
        )
        .route("/status", get(|| async { "x".repeat(2 * 1024 * 1024 + 1) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(axum::serve(listener, app).into_future());
    let mut s = Services::default();
    for name in ["sde", "genie", "adhd"] {
        s.bases.insert(name.into(), base.clone());
    }
    assert_eq!(
        s.call(
            Engine::Sde,
            Method::GET,
            "runs",
            &[],
            "originator-token",
            None
        )
        .await
        .unwrap()["runs"],
        json!([])
    );
    assert!(s.readiness("originator-token").await.is_err());
    assert!(s
        .call(
            Engine::Genie,
            Method::GET,
            "info",
            &[],
            "originator-token",
            None
        )
        .await
        .is_err());
    assert!(s
        .call(
            Engine::Genie,
            Method::GET,
            "status",
            &[],
            "originator-token",
            None
        )
        .await
        .is_err());
    assert!(s
        .call(
            Engine::Adhd,
            Method::GET,
            "runs",
            &[],
            "originator-token",
            None
        )
        .await
        .is_err());
    server.abort();
}
#[test]
fn unknown_usage_is_not_zero_and_private_fields_are_removed() {
    let row:UsageRow=serde_json::from_value(json!({"ts_ms":1,"request_id":"r","provider":"p","size":"s","model":"m","latency_ms":1,"status":200,"output_tokens":0})).unwrap();
    assert_eq!(row.input_tokens, None);
    assert_eq!(row.output_tokens, Some(0));
    let mut v = json!({"token":"hidden","nested":{"operation_key":"hidden","text":"contains caller-secret"}});
    scrub(&mut v, "caller-secret");
    let s = v.to_string();
    assert!(!s.contains("hidden"));
    assert!(!s.contains("caller-secret"));
}

#[tokio::test]
async fn durable_handoff_marker_blocks_close_recycle_reap_and_ide_actions() {
    let state = AppState::test_without_runtime();
    let actor = auth::Actor {
        driver: "human:fixture".into(),
        role: auth::Role::Admin,
        approval: None,
    };
    let record:crate::sessions::SessionRecord=serde_json::from_value(json!({"session_id":"fixture","project":"demo","branch":"session/fixture","head":"a".repeat(40),"stale":false,"actor":actor,"lifecycle":"active","generation":18446744073709551615u64,"handoff":{"phase":"pending","run_id":"run1","operation_id":"op1"}})).unwrap();
    let bytes = serde_json::to_vec(&record).unwrap();
    let record: crate::sessions::SessionRecord = serde_json::from_slice(&bytes).unwrap();
    assert!(record.handoff_pending());
    state.sessions.put(record.clone());
    assert_eq!(
        crate::sessions::close(actor.clone(), State(state.clone()), Path("fixture".into()))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        crate::sessions::recycle(actor.clone(), State(state.clone()), Path("fixture".into()))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert!(!crate::reaper::reap(&state, "fixture", "fixture").await);
    for action in ["start", "checkpoint", "finish"] {
        assert_eq!(
            crate::ide::operation(
                actor.clone(),
                State(state.clone()),
                Path("fixture".into()),
                Json(crate::ide::Operation {
                    generation: u64::MAX,
                    action: action.into()
                })
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
    }
    assert!(!crate::ide::flush_record(&state, &record, "fixture").await);
    assert!(state.sessions.record("fixture").unwrap().handoff_pending());
    let mut legacy = serde_json::to_value(record).unwrap();
    legacy.as_object_mut().unwrap().remove("handoff");
    assert!(
        !serde_json::from_value::<crate::sessions::SessionRecord>(legacy)
            .unwrap()
            .handoff_pending()
    );
}
