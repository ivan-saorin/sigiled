use super::*;
use crate::merge::tests::{commit_on, mk_repo, sh, write};
use axum::{body::to_bytes, routing::post, Router};
use std::sync::{Arc, Mutex};
fn fixture() -> (PathBuf, PathBuf, AppState, String, String) {
    let path = mk_repo("enroll");
    let project = path.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(crate::project::valid_name(&project));
    sh(
        &path,
        &[
            "remote",
            "add",
            "origin",
            &format!("git@github.com:tests/{project}.git"),
        ],
    );
    let dir = crate::merge::tests::tmp_repo("memory-state");
    std::fs::create_dir_all(&dir).unwrap();
    let state = AppState {
        sessions: crate::sessions::SessionState::with_repos_dir(
            path.parent().unwrap().to_path_buf(),
        ),
        store: crate::store::Store::at_dir(&dir),
        github: Some(crate::github::GitHub {
            api_base: "http://127.0.0.1:1".into(),
            pat: "unused-fixture".into(),
            owner: "tests".into(),
            template: "unused".into(),
            keys_dir: dir.join("keys"),
        }),
        auth: crate::auth::AuthState {
            config: crate::auth::tests::cfg().into(),
            keys: crate::auth::tests::preloaded_keys(),
            ..Default::default()
        },
        ..Default::default()
    };
    state.registry.insert(crate::project::ProjectRecord::new(
        &project,
        &crate::manifest::Manifest::parse("").unwrap(),
        None,
    ));
    state.try_persist().unwrap();
    let token = crate::auth::tests::sign(
        &json!({"sub":"fixture-subject","preferred_username":"driver","groups":["stack:drivers"],"iss":"https://idp.test/application/o/driver/","exp":crate::auth::now_epoch()+600}),
    );
    (path, dir, state, project, token)
}
#[derive(Clone)]
struct FakeMemory {
    callback: String,
    token: String,
    records: Arc<Mutex<Vec<Snapshot>>>,
    cap: Arc<Mutex<bool>>,
    ambiguous: Arc<Mutex<bool>>,
}
async fn fake_memory(State(f): State<FakeMemory>, request: axum::extract::Request) -> Response {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let auth = request
        .headers()
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(auth, format!("Bearer {}", f.token));
    assert!(!request.headers().contains_key("cookie"));
    if path == "/capabilities" {
        return Json(json!({"enrollment_contract":if *f.cap.lock().unwrap(){"sigil-accepted-v1"}else{"old"},"enrollment_validation":"fixed_sigil_current_intent_v1","auth":{"oidc_verifier_configured":true}})).into_response();
    }
    let index = path.split('/').nth(2).unwrap().to_owned();
    if method == reqwest::Method::GET {
        let s = f.records.lock().unwrap().last().cloned().unwrap();
        return Json(json!({"owner":s.owner,"project":s.project,"repository":s.repository,"revision":s.revision,"commit":s.commit,"digest":s.digest(),"projection":"indexed","deleted":false,"verified":true})).into_response();
    }
    let bytes = to_bytes(request.into_body(), MAX_BODY).await.unwrap();
    let s: Snapshot = serde_json::from_slice(&bytes).unwrap();
    assert!(s.validate());
    let result=reqwest::Client::new().post(&f.callback).header("authorization",auth).json(&json!({"project":s.project,"index":index,"owner":s.owner,"revision":s.revision,"commit":s.commit,"digest":s.digest()})).send().await.unwrap();
    if !result.status().is_success() {
        return (result.status(), Json(json!({"error":"validation_failed"}))).into_response();
    }
    let proof: Value = result.json().await.unwrap();
    assert_eq!(proof["digest"], s.digest());
    let receipt = json!({"contract":"sigil-accepted-v1","owner":s.owner,"revision":s.revision,"commit":s.commit,"digest":s.digest(),"projection":"indexed","verified":true});
    f.records.lock().unwrap().push(s);
    if *f.ambiguous.lock().unwrap() {
        return "lost response fixture".into_response();
    }
    Json(receipt).into_response()
}
async fn services(
    state: AppState,
    token: String,
) -> (
    crate::browser::memory::Service,
    FakeMemory,
    Vec<tokio::task::JoinHandle<()>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let callback = format!(
        "http://{}/memory/enrollment/validate",
        listener.local_addr().unwrap()
    );
    let server = Router::new()
        .route("/memory/enrollment/validate", post(validate))
        .with_state(state);
    let one = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    let f = FakeMemory {
        callback,
        token,
        records: Default::default(),
        cap: Arc::new(Mutex::new(true)),
        ambiguous: Arc::new(Mutex::new(false)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Router::new().fallback(fake_memory).with_state(f.clone());
    let two = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    (
        crate::browser::memory::Service::test_at(base),
        f,
        vec![one, two],
    )
}
#[tokio::test]
async fn enrollment_registry_to_accepted_handler_restart_retry_and_live_version_gate() {
    let (repo, dir, state, project, token) = fixture();
    let (svc, f, tasks) = services(state.clone(), token.clone()).await;
    let registered = state
        .registry
        .descriptor(&project)
        .memory_enrollment
        .unwrap();
    assert_eq!(registered.state, "awaiting_authorization");
    let awaiting = reconcile_using(
        state.clone(),
        project.clone(),
        "driver".into(),
        None,
        svc.clone(),
    )
    .await
    .unwrap();
    assert_eq!(awaiting["state"], "awaiting_authorization");
    assert!(f.records.lock().unwrap().is_empty());
    *f.cap.lock().unwrap() = false;
    let result = reconcile_using(
        state.clone(),
        project.clone(),
        "driver".into(),
        Some(token.clone()),
        svc.clone(),
    )
    .await
    .unwrap();
    assert_eq!(result["state"], "update_required");
    assert!(f.records.lock().unwrap().is_empty());
    *f.cap.lock().unwrap() = true;
    *f.ambiguous.lock().unwrap() = true;
    let result = reconcile_using(
        state.clone(),
        project.clone(),
        "driver".into(),
        Some(token.clone()),
        svc.clone(),
    )
    .await
    .unwrap();
    assert_eq!(result["state"], "unavailable");
    let first = f.records.lock().unwrap()[0].clone();
    let restored = AppState {
        registry: Default::default(),
        ..state.clone()
    };
    restored.hydrate_from_disk();
    assert_eq!(
        restored
            .registry
            .descriptor(&project)
            .memory_enrollment
            .unwrap()
            .state,
        "awaiting_authorization"
    );
    // Stable intent and no credentials survive a disk restart; same accepted content is retried.
    let disk = std::fs::read_to_string(dir.join("state.json")).unwrap();
    assert!(!disk.contains(&token));
    *f.ambiguous.lock().unwrap() = false;
    let result = reconcile_using(
        state.clone(),
        project.clone(),
        "driver".into(),
        Some(token.clone()),
        svc.clone(),
    )
    .await
    .unwrap();
    assert_eq!(result["state"], "indexed");
    assert_eq!(f.records.lock().unwrap()[1], first);
    assert!(first
        .documents
        .iter()
        .any(|d| d.path == "docs/log-operativo.md"));
    assert_eq!(
        first
            .documents
            .iter()
            .find(|d| d.path == "README.md")
            .unwrap()
            .text,
        "smoke\n"
    );
    // A submitted commit label cannot authorize a session-only or superseded snapshot.
    commit_on(
        &repo,
        "master",
        "README.md",
        "new accepted readme\n",
        "accept new docs",
    );
    crate::ecosystem::refresh(&state, &project).await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    let response = validate(
        State(state.clone()),
        headers.clone(),
        Json(Validation {
            project: project.clone(),
            index: project.clone(),
            owner: first.owner.clone(),
            revision: first.revision.clone(),
            commit: first.commit.clone(),
            digest: first.digest(),
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    headers.insert("authorization", "Bearer legacy-bearer".parse().unwrap());
    let response = validate(
        State(state.clone()),
        headers,
        Json(Validation {
            project: project.clone(),
            index: project.clone(),
            owner: first.owner,
            revision: first.revision,
            commit: first.commit,
            digest: "forged".into(),
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let result = reconcile_using(
        state.clone(),
        project.clone(),
        "driver".into(),
        Some(token),
        svc,
    )
    .await
    .unwrap();
    assert_eq!(result["state"], "indexed");
    assert_eq!(
        result["confirmed"]["commit"],
        sh(&repo, &["rev-parse", "master"]).trim()
    );
    for task in tasks {
        task.abort();
    }
}
#[tokio::test]
async fn enrollment_exact_git_allowlist_optout_source_optin_and_encoded_bound() {
    let (repo, _dir, state, project, _token) = fixture();
    write(&repo, "src/main.rs", "fn main() {}\n");
    write(&repo, ".env", "fixture secret\n");
    write(&repo, "scratch.md", "untracked research\n");
    sh(&repo, &["add", "src/main.rs", ".env"]);
    sh(&repo, &["commit", "-qm", "accepted source files"]);
    crate::ecosystem::refresh(&state, &project).await.unwrap();
    let e = state
        .registry
        .descriptor(&project)
        .memory_enrollment
        .unwrap();
    let snapshot = collect(
        &state,
        &project,
        &e,
        Instant::now() + Duration::from_secs(8),
    )
    .unwrap();
    assert!(snapshot
        .documents
        .iter()
        .all(|d| !d.path.starts_with("src/") && d.path != ".env" && d.path != "scratch.md"));
    // Commit just the policy: do not accidentally accept the untracked research fixture.
    write(&repo, "sigiled.toml", "[memory]\nenabled=false\n");
    sh(&repo, &["add", "sigiled.toml"]);
    sh(&repo, &["commit", "-qm", "opt out"]);
    crate::ecosystem::refresh(&state, &project).await.unwrap();
    assert_eq!(
        state
            .registry
            .descriptor(&project)
            .memory_enrollment
            .unwrap()
            .state,
        "disabled"
    );
    write(
        &repo,
        "sigiled.toml",
        "[memory]\nsource_code=true\nsources=['src']\n",
    );
    sh(&repo, &["add", "sigiled.toml"]);
    sh(&repo, &["commit", "-qm", "opt in selected source"]);
    crate::ecosystem::refresh(&state, &project).await.unwrap();
    let e = state
        .registry
        .descriptor(&project)
        .memory_enrollment
        .unwrap();
    let snapshot = collect(
        &state,
        &project,
        &e,
        Instant::now() + Duration::from_secs(8),
    )
    .unwrap();
    assert_eq!(snapshot.documents.len(), 1);
    assert_eq!(snapshot.documents[0].path, "src/main.rs");
    let mut bad = snapshot.clone();
    bad.documents[0].path = "../secret".into();
    assert!(!bad.validate());
    bad = snapshot.clone();
    bad.documents[0].text = "\u{0001}".repeat(200000);
    bad.documents[0].sha = digest(bad.documents[0].text.as_bytes());
    assert!(bad.documents[0].text.len() < 524288);
    assert!(serde_json::to_vec(&bad).unwrap().len() > MAX_BODY);
    assert!(
        !bad.validate(),
        "serialized escaping must be bounded, not only decoded bytes"
    );
}
#[tokio::test]
async fn enrollment_verified_source_joins_actual_canonical_repo_and_memory_owner() {
    let (_repo, _dir, state, project, token) = fixture();
    let (svc, f, tasks) = services(state.clone(), token.clone()).await;
    reconcile_using(
        state.clone(),
        project.clone(),
        "driver".into(),
        Some(token.clone()),
        svc.clone(),
    )
    .await
    .unwrap();
    let snapshot = f.records.lock().unwrap().last().unwrap().clone();
    let doc = &snapshot.documents[0];
    let chunk = json!({"id":"e_fixture","source":"git","ref":snapshot.repository,"path":doc.path,"sha":doc.sha});
    let verified = source_edit(&state, &svc, &project, &chunk, &token).await;
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["path"], doc.path);
    assert_eq!(verified["indexed_commit"], snapshot.commit);
    let mut unsafe_chunk = chunk.clone();
    unsafe_chunk["path"] = json!("../../etc/passwd");
    assert_eq!(
        source_edit(&state, &svc, &project, &unsafe_chunk, &token).await["verified"],
        false
    );
    f.records.lock().unwrap().last_mut().unwrap().owner =
        "00000000-0000-4000-8000-000000000009".into();
    assert_eq!(
        source_edit(&state, &svc, &project, &chunk, &token).await["verified"],
        false
    );
    for task in tasks {
        task.abort();
    }
}
#[tokio::test]
async fn enrollment_namespace_correction_keeps_unrelated_owner_and_policy() {
    let (_repo, _dir, state, project, _token) = fixture();
    crate::ecosystem::refresh(&state, &project).await.unwrap();
    update(&state, &project, |e| {
        e.state = "ownership_or_revision_conflict".into()
    })
    .unwrap();
    let old = state
        .registry
        .descriptor(&project)
        .memory_enrollment
        .unwrap();
    separate_namespace(&state, &project).await.unwrap();
    let current = state
        .registry
        .descriptor(&project)
        .memory_enrollment
        .unwrap();
    assert_eq!(current.owner, old.owner);
    assert_ne!(current.namespace.as_deref(), Some(project.as_str()));
    assert!(crate::project::valid_name(
        current.namespace.as_deref().unwrap()
    ));
    assert_eq!(current.state, "awaiting_authorization");
}

#[tokio::test]
async fn enrollment_persistence_failure_prevents_outbound_and_freezes_callback() {
    let (_repo, _dir, state, project, token) = fixture();
    crate::ecosystem::refresh(&state, &project).await.unwrap();
    let (svc, f, tasks) = services(state.clone(), token.clone()).await;
    let blocked = AppState {
        store: state.store.clone().fail_after(0),
        ..state.clone()
    };
    assert!(
        reconcile_using(blocked, project.clone(), "driver".into(), Some(token), svc)
            .await
            .is_err()
    );
    assert!(f.records.lock().unwrap().is_empty());
    for task in tasks {
        task.abort();
    }
}
#[tokio::test]
async fn enrollment_normal_close_accepts_reviewed_handoff_once_and_preserves_failed_journal() {
    let (repo, _dir, state, project, _token) = fixture();
    sh(&repo, &["checkout", "-qb", "session/research"]);
    let files = json!([{"path":"docs/design/idea/dossier.md","content":"# Reviewed dossier\n"},{"path":"docs/design/idea/cover.md","content":"# Reviewed cover\n"}]);
    for f in files.as_array().unwrap() {
        write(
            &repo,
            f["path"].as_str().unwrap(),
            f["content"].as_str().unwrap(),
        );
    }
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "reviewed research"]);
    let commit = sh(&repo, &["rev-parse", "HEAD"]);
    let actor = crate::auth::Actor {
        driver: "human:fixture".into(),
        role: crate::auth::Role::Admin,
        approval: None,
    };
    let record = crate::sessions::SessionRecord {
        session_id: "research".into(),
        project: project.clone(),
        branch: "session/research".into(),
        head: commit.clone(),
        stale: false,
        actor: actor.clone(),
        token: None,
        binding: None,
        lifecycle: crate::sessions::Lifecycle::Active,
        generation: 0,
        error: None,
        image: None,
        runtime_owned: false,
        handoff: Some(
            json!({"phase":"complete","run_id":"run-one","receipt":{"commit":commit,"pushed":true},"request":{"files":files}}),
        ),
    };
    state.sessions.put(record.clone());
    state.try_persist().unwrap();
    accept_handoff(&state, &record, &repo, &sh(&repo, &["rev-parse", "master"])).unwrap();
    assert!(
        research_receipt(&state, &project, "run-one", &actor.driver).is_none(),
        "session-only commit is not accepted"
    );
    let failed = AppState {
        store: state.store.clone().fail_after(1),
        ..state.clone()
    };
    let response =
        crate::sessions::close_expected(actor.clone(), failed, "research".into(), Some(0)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(sh(&repo, &["rev-parse", "master"]), commit);
    assert!(
        state.sessions.record("research").is_some(),
        "post-acceptance journal failure retains exact handoff/session for retry"
    );
    let response =
        crate::sessions::close_expected(actor.clone(), state.clone(), "research".into(), Some(0))
            .await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted = research_receipt(&state, &project, "run-one", &actor.driver).unwrap();
    assert_eq!(accepted["master_accepted"], true);
    assert_eq!(accepted["indexed"], false);
    assert_eq!(
        state
            .registry
            .descriptor(&project)
            .memory_enrollment
            .unwrap()
            .handoffs
            .len(),
        1
    );
    accept_handoff(&state, &record, &repo, &commit).unwrap();
    assert_eq!(
        state
            .registry
            .descriptor(&project)
            .memory_enrollment
            .unwrap()
            .handoffs
            .len(),
        1,
        "acceptance identity includes project/run/accepted commit"
    );
}
#[test]
fn enrollment_aggregate_snapshot_capacity_is_enforced_by_the_real_store() {
    let dir = crate::merge::tests::tmp_repo("memory-capacity");
    let store = crate::store::Store::at_dir(&dir);
    let mut snapshot = crate::store::StateSnapshot::default();
    let mut e = Enrollment::default();
    e.actor = Some("x".repeat(1024 * 1024));
    for n in 0..33 {
        snapshot.ecosystem.insert(
            format!("project-{n}"),
            crate::ecosystem::Descriptor {
                memory_enrollment: Some(e.clone()),
                ..Default::default()
            },
        );
    }
    assert!(store.try_save(&snapshot).is_err());
    assert!(!dir.join("state.json").exists());
}

#[tokio::test]
async fn enrollment_fix1_handoff_capacity_retains_close_evidence_and_recovers() {
    let (repo, _dir, state, project, _token) = fixture();
    sh(&repo, &["checkout", "-qb", "session/research"]);
    let files = json!([{"path":"docs/design/idea/dossier.md","content":"# Reviewed dossier\n"},{"path":"docs/design/idea/cover.md","content":"# Reviewed cover\n"}]);
    for f in files.as_array().unwrap() {
        write(
            &repo,
            f["path"].as_str().unwrap(),
            f["content"].as_str().unwrap(),
        );
    }
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "reviewed research"]);
    let commit = sh(&repo, &["rev-parse", "HEAD"]);
    let actor = crate::auth::Actor {
        driver: "human:fixture".into(),
        role: crate::auth::Role::Admin,
        approval: None,
    };
    let record = crate::sessions::SessionRecord {
        session_id: "research".into(),
        project: project.clone(),
        branch: "session/research".into(),
        head: commit.clone(),
        stale: false,
        actor: actor.clone(),
        token: None,
        binding: None,
        lifecycle: crate::sessions::Lifecycle::Active,
        generation: 0,
        error: None,
        image: None,
        runtime_owned: false,
        handoff: Some(
            json!({"phase":"complete","run_id":"run-one","receipt":{"commit":commit,"pushed":true},"request":{"files":files}}),
        ),
    };
    state.sessions.put(record.clone());
    state.try_persist().unwrap();
    accept_handoff(&state, &record, &repo, &sh(&repo, &["rev-parse", "master"])).unwrap();
    assert!(
        research_receipt(&state, &project, "run-one", &actor.driver).is_none(),
        "session-only commit is not accepted"
    );
    let receipts: Vec<_> = (0..256)
        .map(|n| json!({"operation_id":format!("seeded-{n}"),"accepted_commit":commit}))
        .collect();
    update(&state, &project, |e| e.handoffs = receipts.clone()).unwrap();
    let blocked =
        crate::sessions::close_expected(actor.clone(), state.clone(), "research".into(), Some(0))
            .await;
    assert_eq!(blocked.status(), StatusCode::CONFLICT);
    assert!(state.sessions.record("research").is_some());
    assert_eq!(
        sh(&repo, &["rev-parse", "master"]),
        commit,
        "master acceptance fact survives receipt-capacity failure"
    );
    assert_eq!(
        state
            .registry
            .descriptor(&project)
            .memory_enrollment
            .unwrap()
            .handoffs,
        receipts,
        "no silent receipt eviction"
    );
    // Seed a repaired capacity state; no production eviction policy is introduced.
    update(&state, &project, |e| {
        e.handoffs.pop();
    })
    .unwrap();
    let recovered =
        crate::sessions::close_expected(actor.clone(), state.clone(), "research".into(), Some(0))
            .await;
    assert_eq!(recovered.status(), StatusCode::OK);
    assert!(state.sessions.record("research").is_none());
    assert_eq!(
        state
            .registry
            .descriptor(&project)
            .memory_enrollment
            .unwrap()
            .handoffs
            .len(),
        256
    );
    assert_eq!(
        research_receipt(&state, &project, "run-one", &actor.driver).unwrap()["master_accepted"],
        true
    );
    accept_handoff(&state, &record, &repo, &commit).unwrap();
    assert_eq!(
        state
            .registry
            .descriptor(&project)
            .memory_enrollment
            .unwrap()
            .handoffs
            .len(),
        256,
        "same accepted receipt replays even at capacity"
    );
}
