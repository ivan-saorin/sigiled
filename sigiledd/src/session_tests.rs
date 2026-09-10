// Hermetic runtime boundary: real handlers and Git, no Docker or network.
use crate::{
    auth::{Actor, Role},
    sessions, AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
pub struct BuildBarrier {
    pub entered: tokio::sync::Notify,
    release: Mutex<bool>,
    wake: std::sync::Condvar,
}
impl BuildBarrier {
    fn new() -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
            release: Mutex::new(false),
            wake: std::sync::Condvar::new(),
        }
    }
    pub fn block(&self) {
        self.entered.notify_one();
        let mut released = self.release.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.release.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
struct ReleaseBuild(Arc<BuildBarrier>);
impl Drop for ReleaseBuild {
    fn drop(&mut self) {
        self.0.release();
    }
}
pub struct FakeRuntime {
    pub build_barrier: Mutex<Option<Arc<BuildBarrier>>>,
    pub repo_url: String,
    pub root: PathBuf,
    pub origin: PathBuf,
    pub live: Mutex<HashMap<String, String>>,
    pub calls: Mutex<Vec<String>>,
    pub creates: Mutex<Vec<Vec<String>>>,
    pub fail_flush: AtomicBool,
    pub fail_boot: AtomicBool,
    pub fail_start: AtomicBool,
    pub drop_branch: AtomicBool,
    pub pause_health: AtomicBool,
    pub health_entered: tokio::sync::Notify,
    pub health_release: tokio::sync::Notify,
}
impl FakeRuntime {
    pub fn docker(&self, args: &[&str]) -> Result<String, String> {
        let mut live = self.live.lock().unwrap();
        match args[0] {
            "create" => {
                self.creates
                    .lock()
                    .unwrap()
                    .push(args.iter().map(|s| s.to_string()).collect());
                let name = args[args.iter().position(|a| *a == "--name").unwrap() + 1];
                let hostname = args[args.iter().position(|a| *a == "--hostname").unwrap() + 1];
                if name.len() > 63 || hostname.len() > 63 {
                    return Err("runtime DNS label exceeds 63 bytes".into());
                }
                self.calls.lock().unwrap().push(format!("create {name}"));
                if live.contains_key(name) {
                    return Err("container exists".into());
                }
                let token = args
                    .iter()
                    .find_map(|a| a.strip_prefix("SESSION_TOKEN="))
                    .unwrap();
                live.insert(name.into(), token.into());
                std::fs::create_dir_all(self.root.join(name)).unwrap();
                Ok(name.into())
            }
            "rm" => {
                let name = args.last().unwrap();
                self.calls.lock().unwrap().push(format!("destroy {name}"));
                live.remove(*name);
                let path = self.root.join(name);
                assert!(path.starts_with(&self.root));
                let _ = std::fs::remove_dir_all(path);
                Ok(String::new())
            }
            "start" if self.fail_start.load(Ordering::SeqCst) => Err("start failed".into()),
            "cp" | "start" => Ok(String::new()),
            _ => panic!("unexpected fake docker operation: {}", args[0]),
        }
    }
    pub fn healthy(&self, container: &str, token: &str) -> Result<(), String> {
        if self.live.lock().unwrap().get(container).map(String::as_str) == Some(token) {
            Ok(())
        } else {
            Err("workspace unavailable".into())
        }
    }
    pub fn exec(&self, container: &str, token: &str, cmd: &str) -> Result<Value, String> {
        self.healthy(container, token)?;
        if cmd.starts_with("git clone") && self.fail_boot.load(Ordering::SeqCst) {
            return Err("boot failed".into());
        }
        if cmd.contains("autosave") && self.fail_flush.load(Ordering::SeqCst) {
            return Ok(json!({"exit":1,"stdout":"","stderr":"checkpoint failed"}));
        }
        let cmd = cmd.replace(&self.repo_url, &self.origin.to_string_lossy());
        let out = std::process::Command::new("bash")
            .args(["-c", &cmd])
            .current_dir(self.root.join(container))
            .output()
            .unwrap();
        if cmd.contains("autosave") && self.drop_branch.load(Ordering::SeqCst) {
            let branch =
                crate::merge::git(&self.root.join(container), &["branch", "--show-current"])
                    .unwrap();
            crate::merge::git(
                &self.origin,
                &["update-ref", "-d", &format!("refs/heads/{branch}")],
            )
            .unwrap();
        }
        Ok(
            json!({"exit":out.status.code(), "stdout":String::from_utf8_lossy(&out.stdout),"stderr":String::from_utf8_lossy(&out.stderr)}),
        )
    }
}
fn actor(name: &str) -> Actor {
    Actor {
        driver: name.into(),
        role: Role::Admin,
        approval: None,
    }
}
async fn body(resp: Response) -> (StatusCode, Value) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn setup() -> (AppState, Arc<FakeRuntime>) {
    setup_project("proj")
}
fn setup_project(project: &str) -> (AppState, Arc<FakeRuntime>) {
    let root = crate::merge::tests::tmp_repo("foundation");
    std::fs::create_dir_all(&root).unwrap();
    let seed = crate::merge::tests::mk_repo("foundation-seed");
    let origin = root.join("origin.git");
    crate::merge::git(
        &seed,
        &[
            "clone",
            "--bare",
            &seed.to_string_lossy(),
            &origin.to_string_lossy(),
        ],
    )
    .unwrap();
    let repos = root.join("repos");
    std::fs::create_dir_all(&repos).unwrap();
    crate::merge::git(
        &seed,
        &[
            "clone",
            &origin.to_string_lossy(),
            &repos.join(project).to_string_lossy(),
        ],
    )
    .unwrap();
    let keys = root.join("keys").join(project);
    std::fs::create_dir_all(&keys).unwrap();
    std::fs::write(keys.join("id_ed25519"), "fake").unwrap();
    let fake = Arc::new(FakeRuntime {
        build_barrier: Mutex::new(None),
        repo_url: format!("git@github.com:test/{project}.git"),
        root: root.join("workspaces"),
        origin,
        live: Mutex::default(),
        calls: Mutex::default(),
        creates: Mutex::default(),
        fail_flush: AtomicBool::new(false),
        fail_boot: AtomicBool::new(false),
        fail_start: AtomicBool::new(false),
        drop_branch: AtomicBool::new(false),
        pause_health: AtomicBool::new(false),
        health_entered: tokio::sync::Notify::new(),
        health_release: tokio::sync::Notify::new(),
    });
    let mut sessions = sessions::SessionState::with_repos_dir(repos.clone());
    sessions.runtime = Some(crate::runtime::Runtime {
        fake: Some(fake.clone()),
        network: "fake".into(),
        image: "fake-base".into(),
        owner: "test".into(),
        domain: "example.test".into(),
        repos_dir: repos,
        keys_dir: root.join("keys"),
    });
    (
        AppState {
            sessions,
            store: crate::store::Store::at_dir(&root.join("state")),
            ..AppState::default()
        },
        fake,
    )
}
async fn open(state: &AppState) -> Value {
    let (status, v) =
        body(sessions::open(actor("owner"), State(state.clone()), Path("proj".into())).await).await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    v
}
fn target(endpoint: &Value) -> String {
    format!(
        "vm-{}",
        endpoint
            .as_str()
            .unwrap()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap()
    )
}
#[tokio::test]
async fn two_real_opens_preserve_first_runtime_and_branch() {
    let (state, fake) = setup();
    let a = open(&state).await;
    let b = open(&state).await;
    assert_ne!(a["branch"], b["branch"]);
    assert_ne!(a["endpoint"], b["endpoint"]);
    fake.healthy(&target(&a["endpoint"]), a["token"].as_str().unwrap())
        .unwrap();
    assert_eq!(fake.live.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn flush_failure_preserves_close_recycle_and_reap() {
    let (state, fake) = setup();
    let a = open(&state).await;
    let id = a["session_id"].as_str().unwrap();
    fake.fail_flush.store(true, Ordering::SeqCst);
    let (status, _) =
        body(sessions::close(actor("owner"), State(state.clone()), Path(id.into())).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) =
        body(sessions::recycle(actor("owner"), State(state.clone()), Path(id.into())).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    crate::reaper::reap(&state, id, "idle").await;
    assert!(state.sessions.record(id).is_some());
    fake.healthy(&target(&a["endpoint"]), a["token"].as_str().unwrap())
        .unwrap();
    assert!(!fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.starts_with("destroy")));
}
#[tokio::test]
async fn lifecycle_targets_only_its_binding_and_rotates_generation() {
    let (state, fake) = setup();
    let a = open(&state).await;
    let b = open(&state).await;
    let aid = a["session_id"].as_str().unwrap();
    let bid = b["session_id"].as_str().unwrap();
    let (status, recycled) =
        body(sessions::recycle(actor("owner"), State(state.clone()), Path(aid.into())).await).await;
    assert_eq!(status, StatusCode::OK, "{recycled}");
    assert_eq!(recycled["generation"], 2);
    assert_ne!(recycled["token"], a["token"]);
    assert_ne!(recycled["endpoint"], a["endpoint"]);
    fake.healthy(&target(&b["endpoint"]), b["token"].as_str().unwrap())
        .unwrap();
    // An idle observation made before recycle must not destroy its replacement.
    assert!(!crate::reaper::reap_generation(&state, aid, "stale idle observation", Some(1)).await);
    let (first, second) = tokio::join!(
        sessions::close(actor("owner"), State(state.clone()), Path(aid.into())),
        sessions::close(actor("owner"), State(state.clone()), Path(aid.into()))
    );
    let (s1, _) = body(first).await;
    let (s2, _) = body(second).await;
    assert!(matches!(
        (s1, s2),
        (StatusCode::OK, StatusCode::NOT_FOUND) | (StatusCode::NOT_FOUND, StatusCode::OK)
    ));
    fake.healthy(&target(&b["endpoint"]), b["token"].as_str().unwrap())
        .unwrap();
    assert!(crate::reaper::reap(&state, bid, "idle").await);
    assert!(fake.live.lock().unwrap().is_empty());
}
#[tokio::test]
async fn concurrent_orphan_resume_and_branch_refresh_share_mirror_lock() {
    let (state, fake) = setup();
    let rt = state.sessions.runtime.as_ref().unwrap();
    crate::merge::git(&fake.origin, &["branch", "session/orphan", "master"]).unwrap();
    let lock = state.sessions.merge_lock("proj");
    let guard = lock.lock().await;
    let s = state.clone();
    let opening = tokio::spawn(async move { open(&s).await });
    let s = state.clone();
    let listing = tokio::spawn(async move {
        crate::project::branches(actor("reader"), State(s), Path("proj".into())).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(!opening.is_finished());
    assert!(!listing.is_finished());
    assert!(fake.live.lock().unwrap().is_empty());
    drop(guard);
    let a = opening.await.unwrap();
    assert_eq!(listing.await.unwrap().status(), StatusCode::OK);
    let (b, c) = tokio::join!(open(&state), open(&state));
    assert_eq!(a["branch"], "session/orphan");
    assert_ne!(b["branch"], c["branch"]);
    assert_ne!(a["branch"], b["branch"]);
    assert_eq!(fake.live.lock().unwrap().len(), 3);
    assert!(rt.repos_dir.join("proj/.git").exists());
}
#[tokio::test]
async fn inspection_is_authenticated_filtered_redacted_and_side_effect_free() {
    let (mut state, fake) = setup();
    let a = open(&state).await;
    state.auth.config = Arc::new(crate::auth::AuthConfig {
        bootstrap_bearer: Some("inspection-admin".into()),
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = crate::app(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/sigiled/sessions");
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let before = fake.calls.lock().unwrap().clone();
    for path in [
        "?project=proj".into(),
        format!("/{}", a["session_id"].as_str().unwrap()),
    ] {
        let response = client
            .get(format!("{url}{path}"))
            .bearer_auth("inspection-admin")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let text = response.text().await.unwrap();
        assert!(!text.contains(a["token"].as_str().unwrap()));
        assert!(!text.contains("\"token\""));
        assert!(text.contains(a["session_id"].as_str().unwrap()));
    }
    let response: Value = client
        .get(format!("{url}?project=other"))
        .bearer_auth("inspection-admin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["sessions"], json!([]));
    assert_eq!(
        client
            .get(format!("{url}/unknown"))
            .bearer_auth("inspection-admin")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(before, *fake.calls.lock().unwrap());
    server.abort();
}
#[tokio::test]
async fn mutation_requires_owner_or_admin() {
    let (state, _) = setup();
    let a = open(&state).await;
    let id = a["session_id"].as_str().unwrap();
    let mut other = actor("another-driver");
    other.role = Role::Driver;
    assert_eq!(
        sessions::close(other.clone(), State(state.clone()), Path(id.into()))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        sessions::recycle(other, State(state.clone()), Path(id.into()))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut owner = actor("owner");
    owner.role = Role::Driver;
    assert_eq!(
        sessions::recycle(owner, State(state.clone()), Path(id.into()))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        sessions::close(
            actor("different-admin"),
            State(state.clone()),
            Path(id.into())
        )
        .await
        .status(),
        StatusCode::OK
    );
}
#[tokio::test]
async fn legacy_records_decode_and_ambiguous_ownership_is_quarantined() {
    let (state, fake) = setup();
    let a = open(&state).await;
    let id = a["session_id"].as_str().unwrap();
    let mut raw = serde_json::to_value(state.sessions.record(id).unwrap()).unwrap();
    for key in [
        "binding",
        "lifecycle",
        "generation",
        "error",
        "image",
        "runtime_owned",
    ] {
        raw.as_object_mut().unwrap().remove(key);
    }
    let legacy: sessions::SessionRecord = serde_json::from_value(raw).unwrap();
    assert_eq!(legacy.generation, 0);
    assert_eq!(legacy.container(), "vm-proj");
    assert_eq!(legacy.lifecycle, sessions::Lifecycle::Active);
    let mut other = legacy.clone();
    other.session_id = "legacy-other".into();
    other.branch = "session/legacy-other".into();
    state.sessions.hydrate(
        HashMap::new(),
        HashMap::from([(id.into(), legacy), (other.session_id.clone(), other)]),
    );
    let before = fake.calls.lock().unwrap().clone();
    for id in [id, "legacy-other"] {
        let rec = state.sessions.record(id).unwrap();
        assert_eq!(rec.error, Some(sessions::Failure::LegacyOwnershipAmbiguous));
        assert_eq!(
            sessions::close(actor("owner"), State(state.clone()), Path(id.into()))
                .await
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            sessions::recycle(actor("owner"), State(state.clone()), Path(id.into()))
                .await
                .status(),
            StatusCode::CONFLICT
        );
        assert!(!crate::reaper::reap(&state, id, "idle").await);
    }
    assert_eq!(before, *fake.calls.lock().unwrap());
}
#[tokio::test]
async fn single_legacy_binding_is_targeted_without_touching_new_session() {
    let (state, fake) = setup();
    let a = open(&state).await;
    let b = open(&state).await;
    let id = a["session_id"].as_str().unwrap();
    let name = target(&a["endpoint"]);
    let token = fake.live.lock().unwrap().remove(&name).unwrap();
    fake.live.lock().unwrap().insert("vm-proj".into(), token);
    std::fs::rename(fake.root.join(name), fake.root.join("vm-proj")).unwrap();
    let mut records = state.sessions.dump_records();
    records.get_mut(id).unwrap().binding = None;
    state.sessions.hydrate(HashMap::new(), records);
    assert_eq!(
        sessions::close(actor("owner"), State(state.clone()), Path(id.into()))
            .await
            .status(),
        StatusCode::OK
    );
    fake.healthy(&target(&b["endpoint"]), b["token"].as_str().unwrap())
        .unwrap();
}
#[tokio::test]
async fn collision_keeps_incumbent_and_master_push_failure_preserves_work() {
    let (state, fake) = setup();
    let a = open(&state).await;
    let id = a["session_id"].as_str().unwrap();
    let name = target(&a["endpoint"]);
    let rt = state.sessions.runtime.as_ref().unwrap();
    assert!(rt
        .create_container(
            &name,
            "proj",
            "session",
            "collision",
            "other-token",
            "fake-base",
            &[]
        )
        .is_err());
    fake.healthy(&name, a["token"].as_str().unwrap()).unwrap();
    assert!(!fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.starts_with("destroy")));
    let hook = fake.origin.join("hooks/pre-receive");
    std::fs::write(&hook,"#!/bin/sh\nwhile read old new ref; do\n [ \"$ref\" = refs/heads/master ] && exit 1\ndone\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(fake.root.join(&name).join("work.txt"), "recover me").unwrap();
    let (status, v) =
        body(sessions::close(actor("owner"), State(state.clone()), Path(id.into())).await).await;
    assert_eq!(status, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"], "push_failed");
    fake.healthy(&name, a["token"].as_str().unwrap()).unwrap();
    assert!(state.sessions.record(id).is_some());
    assert!(crate::merge::git(
        &fake.origin,
        &[
            "show",
            &format!("{}:work.txt", a["branch"].as_str().unwrap())
        ]
    )
    .unwrap()
    .contains("recover me"));
}
#[tokio::test]
async fn creation_is_persisted_and_restart_reports_interruption() {
    let (state, fake) = setup();
    let a = open(&state).await;
    let id = a["session_id"].as_str().unwrap();
    state
        .sessions
        .mark(id, sessions::Lifecycle::Recycling, None);
    state.persist();
    let snap = state.store.load().unwrap();
    assert!(snap.sessions[id].binding.is_some());
    assert_eq!(
        snap.sessions[id].token.as_ref().unwrap(),
        a["token"].as_str().unwrap()
    );
    state.sessions.hydrate(snap.debts, snap.sessions);
    let rec = state.sessions.record(id).unwrap();
    assert_eq!(rec.lifecycle, sessions::Lifecycle::Failed);
    assert_eq!(rec.error, Some(sessions::Failure::Interrupted));
    assert_eq!(fake.live.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn branch_fetch_failure_preserves_runtime_for_close_and_recycle() {
    for recycling in [false, true] {
        let (state, fake) = setup();
        let a = open(&state).await;
        let id = a["session_id"].as_str().unwrap();
        fake.drop_branch.store(true, Ordering::SeqCst);
        let response = if recycling {
            sessions::recycle(actor("owner"), State(state.clone()), Path(id.into())).await
        } else {
            sessions::close(actor("owner"), State(state.clone()), Path(id.into())).await
        };
        let (status, v) = body(response).await;
        assert_eq!(status, StatusCode::CONFLICT, "{v}");
        assert_eq!(v["error"], "fetch_failed");
        fake.healthy(&target(&a["endpoint"]), a["token"].as_str().unwrap())
            .unwrap();
        assert!(state.sessions.record(id).is_some());
    }
}
#[tokio::test]
async fn boot_failure_is_owned_and_recoverable_and_start_failure_only_cleans_new_runtime() {
    let (state, fake) = setup();
    let incumbent = open(&state).await;
    fake.fail_boot.store(true, Ordering::SeqCst);
    let (status, v) =
        body(sessions::open(actor("owner"), State(state.clone()), Path("proj".into())).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(v["error"], "boot_failed");
    let failed = state
        .sessions
        .record(v["session_id"].as_str().unwrap())
        .unwrap();
    assert!(failed.runtime_owned);
    assert!(fake.live.lock().unwrap().contains_key(&failed.container()));
    fake.fail_boot.store(false, Ordering::SeqCst);
    fake.fail_start.store(true, Ordering::SeqCst);
    let (status, v) =
        body(sessions::open(actor("owner"), State(state.clone()), Path("proj".into())).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(v["error"], "create_failed");
    let failed = state
        .sessions
        .record(v["session_id"].as_str().unwrap())
        .unwrap();
    assert!(!failed.runtime_owned);
    assert!(!fake.live.lock().unwrap().contains_key(&failed.container()));
    fake.healthy(
        &target(&incumbent["endpoint"]),
        incumbent["token"].as_str().unwrap(),
    )
    .unwrap();
}
#[tokio::test]
async fn persist_failure_prevents_allocation() {
    let (mut state, fake) = setup();
    let file = fake.origin.join("not-a-directory");
    std::fs::write(&file, "blocked").unwrap();
    state.store = crate::store::Store::at_dir(&file);
    let (status, v) =
        body(sessions::open(actor("owner"), State(state.clone()), Path("proj".into())).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(v["error"], "persist_failed");
    assert!(fake.live.lock().unwrap().is_empty());
    assert!(fake.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn creating_session_cannot_be_closed_during_boot() {
    let (state, fake) = setup();
    fake.pause_health.store(true, Ordering::SeqCst);
    let s = state.clone();
    let opening = tokio::spawn(async move { open(&s).await });
    fake.health_entered.notified().await;
    let records = state.sessions.dump_records();
    let rec = records.values().next().unwrap();
    assert_eq!(rec.lifecycle, sessions::Lifecycle::Creating);
    let disk = state.store.load().unwrap();
    assert!(disk.sessions[&rec.session_id].runtime_owned);
    let s = state.clone();
    let id = rec.session_id.clone();
    let closing =
        tokio::spawn(async move { sessions::close(actor("owner"), State(s), Path(id)).await });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(!closing.is_finished());
    assert_eq!(fake.live.lock().unwrap().len(), 1);
    fake.health_release.notify_one();
    opening.await.unwrap();
    assert_eq!(closing.await.unwrap().status(), StatusCode::OK);
}
#[tokio::test]
async fn missing_runtime_configuration_never_discards_custodied_session() {
    let (mut state, fake) = setup();
    let a = open(&state).await;
    let id = a["session_id"].as_str().unwrap();
    state.sessions.runtime = None;
    let (status, v) =
        body(sessions::close(actor("owner"), State(state.clone()), Path(id.into())).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(v["error"], "runtime_unavailable");
    assert!(state.sessions.record(id).is_some());
    assert_eq!(fake.live.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn maximum_project_name_opens_and_recycles_with_bounded_runtime_names() {
    let project = "a".repeat(39);
    let (state, fake) = setup_project(&project);
    let (status, a) =
        body(sessions::open(actor("owner"), State(state.clone()), Path(project.clone())).await)
            .await;
    assert_eq!(status, StatusCode::CREATED, "{a}");
    let id = a["session_id"].as_str().unwrap();
    let mut records = state.sessions.dump_records();
    records.get_mut(id).unwrap().generation = u64::MAX - 1;
    state.sessions.hydrate(HashMap::new(), records);
    let (status, b) =
        body(sessions::recycle(actor("owner"), State(state.clone()), Path(id.into())).await).await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["generation"], u64::MAX);
    assert!(target(&b["endpoint"]).len() <= 63);
    assert!(target(&b["endpoint"]).contains(id));
    fake.healthy(&target(&b["endpoint"]), b["token"].as_str().unwrap())
        .unwrap();
    let before = fake.calls.lock().unwrap().clone();
    let (status, c) =
        body(sessions::recycle(actor("owner"), State(state.clone()), Path(id.into())).await).await;
    assert_eq!(status, StatusCode::CONFLICT, "{c}");
    assert_eq!(c["error"], "generation_exhausted");
    assert_eq!(before, *fake.calls.lock().unwrap());
}
#[tokio::test]
async fn cancelled_open_or_recycle_build_retains_mirror_lock_until_barrier_release() {
    for recycling in [false, true] {
        let (state, fake) = setup();
        let id = if recycling {
            Some(
                open(&state).await["session_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        } else {
            None
        };
        let barrier = Arc::new(BuildBarrier::new());
        let release = ReleaseBuild(barrier.clone());
        *fake.build_barrier.lock().unwrap() = Some(barrier.clone());
        let s = state.clone();
        let awaiting = tokio::spawn(async move {
            match id {
                Some(id) => sessions::recycle(actor("owner"), State(s), Path(id)).await,
                None => sessions::open(actor("owner"), State(s), Path("proj".into())).await,
            }
        });
        barrier.entered.notified().await;
        awaiting.abort();
        assert!(awaiting.await.unwrap_err().is_cancelled());
        let lock = state.sessions.merge_lock("proj");
        assert!(
            lock.try_lock().is_err(),
            "cancelled request released a still-running build's mirror"
        );
        let s = state.clone();
        let consumer = tokio::spawn(async move {
            crate::project::branches(actor("reader"), State(s), Path("proj".into())).await
        });
        assert!(!consumer.is_finished());
        drop(release);
        assert_eq!(consumer.await.unwrap().status(), StatusCode::OK);
    }
}
#[tokio::test]
async fn cancelled_job_image_helper_keeps_owned_lock_and_returns_it_after_build() {
    let (state, fake) = setup();
    let rt = state.sessions.runtime.as_ref().unwrap().clone();
    let mirror = rt.repo_path("proj");
    let lock = state.sessions.merge_lock("proj");
    let barrier = Arc::new(BuildBarrier::new());
    let release = ReleaseBuild(barrier.clone());
    *fake.build_barrier.lock().unwrap() = Some(barrier.clone());
    let guard = lock.clone().lock_owned().await;
    let task = tokio::spawn(async move { rt.session_image_locked("proj", &mirror, guard).await });
    barrier.entered.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        lock.try_lock().is_err(),
        "job build must own the mirror after its waiter is cancelled"
    );
    let s = state.clone();
    let consumer = tokio::spawn(async move {
        crate::project::branches(actor("reader"), State(s), Path("proj".into())).await
    });
    drop(release);
    assert_eq!(consumer.await.unwrap().status(), StatusCode::OK);
    *fake.build_barrier.lock().unwrap() = None;
    let rt = state.sessions.runtime.as_ref().unwrap();
    let guard = lock.clone().lock_owned().await;
    let (_, returned_guard) = rt
        .session_image_locked("proj", &rt.repo_path("proj"), guard)
        .await
        .unwrap();
    assert!(
        lock.try_lock().is_err(),
        "the returned guard protects post-build mirror operations"
    );
    drop(returned_guard);
    assert!(lock.try_lock().is_ok());
}
#[tokio::test]
async fn ide_operations_enforce_owner_generation_policy_and_redact_binding() {
    let (state, fake) = setup();
    let opened = open(&state).await;
    let id = opened["session_id"].as_str().unwrap();
    let mut record = state.sessions.record(id).unwrap();
    record.binding.as_mut().unwrap().ide = Some(crate::ide::Binding {
        provider: crate::ide::PROVIDER.into(),
        helper: "fixture".into(),
        base_digest: format!("sha256:{}", "a".repeat(64)),
        image: "fixture-layer".into(),
        generation: record.generation,
        profile_volume: "sigil-ide-profile-fixture".into(),
        started: false,
        state: "stopped".into(),
        error: None,
        token: "private-helper-token".into(),
        activity_token: "private-activity-token".into(),
    });
    state.sessions.put(record.clone());
    let (_, projection) =
        body(crate::ide::status(actor("owner"), State(state.clone()), Path(id.into())).await).await;
    assert!(!projection.to_string().contains("private-"));
    assert_eq!(projection["provider"]["state"], "stopped");
    for (who, generation, code) in [
        ("intruder", record.generation, StatusCode::FORBIDDEN),
        ("owner", record.generation + 1, StatusCode::CONFLICT),
        ("owner", record.generation, StatusCode::CONFLICT),
    ] {
        let mut caller = actor(who);
        if who == "intruder" {
            caller.role = Role::Driver;
        }
        let (status, _) = body(
            crate::ide::operation(
                caller,
                State(state.clone()),
                Path(id.into()),
                axum::Json(crate::ide::Operation {
                    generation,
                    action: "start".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, code);
    }
    assert!(fake.live.lock().unwrap().contains_key(&record.container()));
    assert_eq!(
        state.sessions.record(id).unwrap().generation,
        record.generation
    );
    let (status, _) =
        body(sessions::close(actor("owner"), State(state.clone()), Path(id.into())).await).await;
    assert_eq!(status, StatusCode::OK);
    assert!(state.sessions.record(id).is_none());
}
#[tokio::test]
async fn ide_named_profile_never_publishes_ports_or_overrides_project_user() {
    let (state, fake) = setup();
    let rt = state.sessions.runtime.as_ref().unwrap();
    rt.create_container_profile(
        "fixture-owned",
        "proj",
        "session",
        "fixture",
        "long-secret-token",
        "project-extended-image",
        &[],
        Some("sigil-ide-profile-operator"),
    )
    .unwrap();
    let creates = fake.creates.lock().unwrap();
    let args = creates.last().unwrap();
    assert_eq!(args.last().unwrap(), "project-extended-image");
    assert!(args
        .iter()
        .any(|a| a == "type=volume,source=sigil-ide-profile-operator,target=/sigil-profile"));
    for forbidden in [
        "--publish",
        "-p",
        "-P",
        "--publish-all",
        "--entrypoint",
        "--user",
        "--privileged",
    ] {
        assert!(!args.iter().any(|a| a == forbidden));
    }
}
#[tokio::test]
async fn resolved_ide_setup_is_persisted_before_container_allocation() {
    let (state, fake) = setup();
    fake.fail_start.store(true, Ordering::SeqCst);
    let (status, value) =
        body(sessions::open(actor("owner"), State(state.clone()), Path("proj".into())).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let record = state
        .sessions
        .record(value["session_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(record.image.as_deref(), Some("fake-base"));
    assert_eq!(
        record.binding.unwrap().ide_error.as_deref(),
        Some("provider_bundle_required")
    );
}
