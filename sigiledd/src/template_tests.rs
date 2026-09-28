//! Real Git repositories behind a controlled GitHub HTTP provider.
use crate::{
    auth::{Actor, Role},
    project::NewProject,
    templates, AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub(crate) fn admin() -> Actor {
    Actor {
        driver: "fixture-admin".into(),
        role: Role::Admin,
        approval: None,
    }
}

#[tokio::test]
async fn failed_runtime_validation_never_registers_or_opens_a_fallback_workspace() {
    let mut f = Fixture::new().await;
    f.provider.0.lock().unwrap().fail_key = true;
    assert_eq!(
        f.create("build-retry", Some("template-a"), None).await.0,
        StatusCode::BAD_GATEWAY
    );
    let (runtime_state, _) = crate::session_tests::setup_project("build-retry");
    let mut rt = runtime_state.sessions.runtime.clone().unwrap();
    rt.fake = None;
    let repo = rt.repos_dir.join("build-retry");
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            f.root.join("build-retry").to_str().unwrap(),
        ],
    );
    f.state.sessions.runtime = Some(rt);
    // No Docker daemon/tool in this hermetic test process: the existing image
    // resolver returns a repair fallback, which creation must reject.
    let (status, value) = f.create("build-retry", Some("template-a"), None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{value}");
    assert_eq!(value["error"], "template_workspace_build_failed");
    assert!(!f.state.registry.contains("build-retry"));
    let open =
        crate::sessions::open(admin(), State(f.state.clone()), Path("build-retry".into())).await;
    assert_eq!(open.status(), StatusCode::CONFLICT);
    assert_eq!(f.provider.0.lock().unwrap().creates, 1);
}

#[tokio::test]
async fn lfs_assets_require_preparation_before_any_destination_is_created() {
    let f = Fixture::new().await;
    write_commit(
        &f.root.join("template-a"),
        ".gitattributes",
        "*.png filter=lfs diff=lfs merge=lfs -text\n",
    );
    let (status, value) = f.create("lfs-film", Some("template-a"), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{value}");
    assert_eq!(value["error"], "template_lfs_unsupported");
    assert_eq!(f.provider.0.lock().unwrap().creates, 0);
}
fn git(root: &std::path::Path, args: &[&str]) -> String {
    crate::merge::git(root, args).unwrap().trim().into()
}
fn write_commit(root: &std::path::Path, file: &str, text: &str) -> String {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@localhost",
            "commit",
            "-qm",
            "fixture",
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}
struct Provider {
    root: PathBuf,
    repos: BTreeMap<String, Value>,
    creates: usize,
    keys: usize,
    lose_create: bool,
    fail_key: bool,
    move_source: bool,
}
#[derive(Clone)]
struct ProviderState(Arc<Mutex<Provider>>);
pub(crate) struct Fixture {
    pub state: AppState,
    provider: ProviderState,
    task: tokio::task::JoinHandle<()>,
    pub root: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn repo(
    State(p): State<ProviderState>,
    Path((_owner, name)): Path<(String, String)>,
) -> Response {
    match p.0.lock().unwrap().repos.get(&name) {
        Some(r) => Json(r.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn designate(
    State(p): State<ProviderState>,
    Path((_owner, name)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    let mut p = p.0.lock().unwrap();
    let Some(r) = p.repos.get_mut(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    for key in ["is_template", "default_branch"] {
        if let Some(value) = body.get(key) {
            r[key] = value.clone();
        }
    }
    Json(r.clone()).into_response()
}
async fn resolve(
    State(p): State<ProviderState>,
    Path((_owner, name, reference)): Path<(String, String, String)>,
) -> Response {
    let mut p = p.0.lock().unwrap();
    let root = p.root.join(&name);
    let Ok(sha) = crate::merge::git(
        &root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    ) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if p.move_source {
        p.move_source = false;
        write_commit(&root, "scene.txt", "changed after resolution");
    }
    Json(json!({"sha":sha.trim()})).into_response()
}
async fn create(State(p): State<ProviderState>, Json(body): Json<Value>) -> Response {
    let mut p = p.0.lock().unwrap();
    let name = body["name"].as_str().unwrap().to_string();
    if p.repos.contains_key(&name) {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    let root = p.root.join(&name);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--bare", "-q"]);
    p.creates += 1;
    let repo = json!({"id":100+p.creates,"name":name,"full_name":format!("fixture/{name}"),"owner":{"login":"fixture"},"private":true,"is_template":false,"description":body["description"],"clone_url":format!("file://{}",root.display()),"default_branch":"main","permissions":{"admin":true}});
    p.repos.insert(name, repo.clone());
    if p.lose_create {
        p.lose_create = false;
        return StatusCode::BAD_GATEWAY.into_response();
    }
    (StatusCode::CREATED, Json(repo)).into_response()
}
async fn key(State(p): State<ProviderState>) -> Response {
    let mut p = p.0.lock().unwrap();
    p.keys += 1;
    if p.fail_key {
        p.fail_key = false;
        StatusCode::BAD_GATEWAY.into_response()
    } else {
        (StatusCode::CREATED, Json(json!({"id":1}))).into_response()
    }
}
impl Fixture {
    pub(crate) async fn new() -> Self {
        let mut state = AppState::test_without_runtime();
        let root = state
            .sessions
            .repos_dir
            .clone()
            .unwrap()
            .join("template-fixture");
        std::fs::create_dir_all(&root).unwrap();
        let mut repos = BTreeMap::new();
        for (id, name, tool) in [
            (1, "vm-tmpl", "base"),
            (2, "template-a", "rust"),
            (3, "template-b", "python"),
            (4, "ordinary", "ordinary"),
        ] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-b", "master", "-q"]);
            std::fs::write(
                dir.join("sigiled.toml"),
                "template = \"vm-tmpl@0.1.0\"\n[workspace]\ndockerfile = \"Dockerfile\"\n",
            )
            .unwrap();
            std::fs::write(
                dir.join("Dockerfile"),
                format!("FROM vm-base:0.1.0\n# {tool} fixture\n"),
            )
            .unwrap();
            write_commit(&dir, "scene.txt", tool);
            repos.insert(name.into(),json!({"id":id,"name":name,"full_name":format!("fixture/{name}"),"owner":{"login":"fixture"},"private":true,"is_template":name!="ordinary","default_branch":"master","clone_url":format!("file://{}",dir.display()),"permissions":{"admin":true}}));
        }
        let provider = ProviderState(Arc::new(Mutex::new(Provider {
            root: root.clone(),
            repos,
            creates: 0,
            keys: 0,
            lose_create: false,
            fail_key: false,
            move_source: false,
        })));
        let router = Router::new()
            .route("/repos/{owner}/{name}", get(repo).patch(designate))
            .route("/repos/{owner}/{name}/commits/{*reference}", get(resolve))
            .route("/repos/{owner}/{name}/keys", post(key))
            .route(
                "/users/fixture",
                get(|| async { Json(json!({"type":"User"})) }),
            )
            .route("/user", get(|| async { Json(json!({"login":"fixture"})) }))
            .route(
                "/user/repos",
                get(|State(p): State<ProviderState>| async move {
                    Json(
                        p.0.lock()
                            .unwrap()
                            .repos
                            .values()
                            .cloned()
                            .collect::<Vec<_>>(),
                    )
                })
                .post(create),
            )
            .with_state(provider.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        state.github = Some(crate::github::GitHub {
            api_base: base,
            pat: "synthetic".into(),
            owner: "fixture".into(),
            template: "vm-tmpl".into(),
            keys_dir: root.join("keys"),
        });
        state.store = crate::store::Store::at_dir(&root.join("state"));
        Self {
            state,
            provider,
            task,
            root,
        }
    }
    async fn create(
        &self,
        name: &str,
        template: Option<&str>,
        reference: Option<&str>,
    ) -> (StatusCode, Value) {
        body(
            crate::project::create(
                admin(),
                State(self.state.clone()),
                Json(NewProject {
                    name: name.into(),
                    template: template.map(Into::into),
                    template_ref: reference.map(Into::into),
                }),
            )
            .await,
        )
        .await
    }
}
pub(crate) async fn body(r: Response) -> (StatusCode, Value) {
    let status = r.status();
    let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&b).unwrap())
}

#[tokio::test]
async fn default_and_distinct_templates_have_independent_roots_and_persisted_provenance() {
    let f = Fixture::new().await;
    for (name, template, content) in [
        ("new-default", None, "base"),
        ("new-a", Some("template-a"), "rust"),
        ("new-b", Some("template-b"), "python"),
    ] {
        let (status, v) = f.create(name, template, None).await;
        assert_eq!(status, StatusCode::CREATED, "{v}");
        let root = f.root.join(name);
        assert_eq!(git(&root, &["show", "master:scene.txt"]), content);
        assert!(git(&root, &["show", "master:Dockerfile"]).contains(content));
        assert_eq!(git(&root, &["rev-list", "--count", "master"]), "1");
        let receipt: Value =
            serde_json::from_str(&git(&root, &["show", "master:.sigil/project.json"])).unwrap();
        assert_eq!(receipt["project"], name);
        assert_eq!(
            receipt["template"]["commit"],
            v["provenance"]["source_commit"]
        );
        assert_eq!(receipt["approval_namespace"], v["provenance"]["operation"]);
        assert_eq!(v["template_version"], "vm-tmpl@0.1.0");
        assert!(f
            .state
            .registry
            .descriptor(name)
            .memory_enrollment
            .is_some());
    }
    let restored = AppState {
        registry: Default::default(),
        ..f.state.clone()
    };
    restored.hydrate_from_disk();
    assert_eq!(restored.registry.snapshot().len(), 3);
    assert_eq!(restored.registry.creations.read().unwrap().len(), 3);
    assert_eq!(f.provider.0.lock().unwrap().creates, 3);
}

#[tokio::test]
async fn moving_branch_and_repeat_from_sha_copy_the_resolved_contents() {
    let f = Fixture::new().await;
    let source = f.root.join("template-a");
    let original = git(&source, &["rev-parse", "HEAD"]);
    f.provider.0.lock().unwrap().move_source = true;
    let (status, v) = f
        .create("pinned-a", Some("template-a"), Some("master"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    assert_eq!(v["provenance"]["source_commit"], original);
    assert_ne!(git(&source, &["rev-parse", "HEAD"]), original);
    assert_eq!(
        git(&f.root.join("pinned-a"), &["show", "master:scene.txt"]),
        "rust"
    );
    let (status, v) = f
        .create("pinned-b", Some("template-a"), Some(&original))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    assert_eq!(
        git(&f.root.join("pinned-b"), &["show", "master:scene.txt"]),
        "rust"
    );
    assert_eq!(
        git(&source, &["show", "HEAD:scene.txt"]),
        "changed after resolution"
    );
}

#[tokio::test]
async fn lost_provider_response_restart_retry_and_concurrency_preserve_one_identity() {
    let f = Fixture::new().await;
    f.provider.0.lock().unwrap().lose_create = true;
    assert_eq!(
        f.create("retry-new", Some("template-a"), None).await.0,
        StatusCode::BAD_GATEWAY
    );
    let saved = f.state.registry.creations.read().unwrap()["retry-new"].clone();
    assert!(!f.state.registry.contains("retry-new"));
    f.state.hydrate_from_disk();
    let ((a, av), (b, bv)) = tokio::join!(
        f.create("retry-new", Some("template-a"), None),
        f.create("retry-new", Some("template-a"), None)
    );
    assert!(
        (a == StatusCode::CREATED && b == StatusCode::OK)
            || (b == StatusCode::CREATED && a == StatusCode::OK),
        "{a} {av} {b} {bv}"
    );
    let result = f.state.registry.creations.read().unwrap()["retry-new"].clone();
    assert_eq!(saved.operation, result.operation);
    assert_eq!(saved.snapshot, result.snapshot);
    assert_eq!(f.provider.0.lock().unwrap().creates, 1);
    assert_eq!(f.state.events.for_project("retry-new").len(), 1);
    assert_eq!(
        f.create("retry-new", Some("template-b"), None).await.0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn interrupted_staging_and_persistence_failure_do_not_poison_recovery() {
    let f = Fixture::new().await;
    f.provider.0.lock().unwrap().fail_key = true;
    assert_eq!(
        f.create("interrupted", Some("template-a"), None).await.0,
        StatusCode::BAD_GATEWAY
    );
    let intent = f.state.registry.creations.read().unwrap()["interrupted"].clone();
    // A killed prior process may leave a private staging lock. The next attempt
    // must use fresh staging, never remove a lock possibly owned by that process.
    let stage = std::fs::read_dir(f.root.join("template-staging").join(&intent.operation))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(stage.join("index.lock"), "incumbent").unwrap();
    let (status, v) = f.create("interrupted", Some("template-a"), None).await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    assert_eq!(
        std::fs::read_to_string(stage.join("index.lock")).unwrap(),
        "incumbent"
    );
    let broken = AppState {
        store: crate::store::Store::at_dir(&f.root.join("blocked")).fail_after(0),
        registry: Default::default(),
        ..f.state.clone()
    };
    let before = f.provider.0.lock().unwrap().creates;
    let result = crate::project::create(
        admin(),
        State(broken),
        Json(NewProject {
            name: "not-durable".into(),
            template: Some("template-a".into()),
            template_ref: None,
        }),
    )
    .await;
    assert!(!result.status().is_success());
    assert_eq!(f.provider.0.lock().unwrap().creates, before);
}

#[tokio::test]
async fn declared_identity_changes_only_declared_fields_and_approval_data_has_new_namespace() {
    let f = Fixture::new().await;
    let source = f.root.join("template-a");
    std::fs::write(
        source.join("film.toml"),
        "name = \"source-film\"\ncomment = \"source-film\"\n",
    )
    .unwrap();
    std::fs::write(
        source.join("sigiled-template.toml"),
        "[[identity]]\nfile=\"film.toml\"\npath=[\"name\"]\nvalue=\"project_name\"\n",
    )
    .unwrap();
    let original = write_commit(
        &source,
        "approvals/preview.json",
        "{\"project_id\":\"source\",\"approved\":true}",
    );
    let (s, v) = f.create("identity-film", Some("template-a"), None).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let root = f.root.join("identity-film");
    let film: toml::Value = toml::from_str(&git(&root, &["show", "master:film.toml"])).unwrap();
    assert_eq!(film["name"].as_str(), Some("identity-film"));
    assert_eq!(film["comment"].as_str(), Some("source-film"));
    let p: Value =
        serde_json::from_str(&git(&root, &["show", "master:.sigil/project.json"])).unwrap();
    let receipt: Value =
        serde_json::from_str(&git(&root, &["show", "master:approvals/preview.json"])).unwrap();
    assert_ne!(receipt["project_id"], p["approval_namespace"]);
    assert_eq!(p["inherited_approvals"], "source_data_only");
    assert_eq!(git(&source, &["rev-parse", "HEAD"]), original);
    write_commit(&source, "scene.txt", "later template edit");
    assert_eq!(git(&root, &["show", "master:scene.txt"]), "rust");
    let edit = f.root.join("independent-edit");
    git(
        &f.root,
        &["clone", root.to_str().unwrap(), edit.to_str().unwrap()],
    );
    write_commit(&edit, "scene.txt", "destination edit");
    git(&edit, &["push", "origin", "master"]);
    assert_eq!(
        git(&source, &["show", "HEAD:scene.txt"]),
        "later template edit"
    );
}

#[tokio::test]
async fn disabled_templates_unknown_fields_and_runtime_bindings_cannot_bypass_checks() {
    let f = Fixture::new().await;
    let (status, v) = body(
        templates::designate(
            admin(),
            State(f.state.clone()),
            Path("template-a".into()),
            Json(templates::Eligibility { enabled: false }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        f.create("disabled", Some("template-a"), None).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(serde_json::from_value::<NewProject>(
        json!({"name":"bad-fields","template":"template-b","template_revision":"other"})
    )
    .is_err());
    write_commit(
        &f.root.join("template-b"),
        "sigiled.toml",
        "[app]\nname=\"incumbent-service\"\n[app.secrets]\nKEY=\"SOURCE_SECRET\"\n",
    );
    let (status, v) = f.create("unsafe-template", Some("template-b"), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(v["error"], "template_runtime_binding");
    assert_eq!(f.provider.0.lock().unwrap().creates, 0);
}

#[tokio::test]
async fn designation_discovery_authorization_and_incompatible_sources_fail_closed() {
    let f = Fixture::new().await;
    let (_, v) = body(templates::list(admin(), State(f.state.clone())).await).await;
    assert_eq!(v["templates"].as_array().unwrap().len(), 3);
    assert_eq!(
        f.create("rejected-a", Some("ordinary"), None).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let driver = Actor {
        driver: "unapproved".into(),
        role: Role::Driver,
        approval: None,
    };
    assert_eq!(
        templates::designate(
            driver.clone(),
            State(f.state.clone()),
            Path("ordinary".into()),
            Json(templates::Eligibility { enabled: true })
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        crate::project::create(
            driver,
            State(f.state.clone()),
            Json(NewProject {
                name: "forbidden".into(),
                template: Some("template-a".into()),
                template_ref: None
            })
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let (status, v) = body(
        templates::designate(
            admin(),
            State(f.state.clone()),
            Path("ordinary".into()),
            Json(templates::Eligibility { enabled: true }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(
        f.create("designated", Some("ordinary"), None).await.0,
        StatusCode::CREATED
    );
    write_commit(
        &f.root.join("template-a"),
        "sigiled.toml",
        "[workspace]\ndockerfile=\"missing\"\n",
    );
    let (s, v) = f.create("invalid-source", Some("template-a"), None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(!f
        .provider
        .0
        .lock()
        .unwrap()
        .repos
        .contains_key("invalid-source"));
    assert_eq!(
        f.create("missing-ref", Some("template-b"), Some("absent"))
            .await
            .0,
        StatusCode::BAD_GATEWAY
    );
}

#[tokio::test]
async fn incumbent_repository_and_partial_selection_are_never_adopted() {
    let f = Fixture::new().await;
    let source = f.root.join("ordinary");
    let before = git(&source, &["rev-parse", "HEAD"]);
    let (s, v) = f.create("ordinary", Some("template-a"), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(git(&source, &["rev-parse", "HEAD"]), before);
    assert_eq!(f.provider.0.lock().unwrap().keys, 0);
    f.provider.0.lock().unwrap().fail_key = true;
    assert_eq!(
        f.create("key-retry", Some("template-b"), None).await.0,
        StatusCode::BAD_GATEWAY
    );
    let key = std::fs::read(f.root.join("keys/key-retry/id_ed25519")).unwrap();
    assert_eq!(
        f.create("key-retry", None, None).await.0,
        StatusCode::CONFLICT
    );
    let (s, v) = f.create("key-retry", Some("template-b"), None).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(
        key,
        std::fs::read(f.root.join("keys/key-retry/id_ed25519")).unwrap()
    );
}
