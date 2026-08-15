// The project registry surface. Session 1 scope: the record shape with
// template_version (DEC-05) behind GET /sigiled/projects, over an in-memory
// store. Session 2 adds template_behind (design §3: status shows it next to
// needs_merge) computed against the latest published template version.
// Session 5 adds POST /projects (create-from-template or adopt, deploy key,
// registration) — the verb that retires the v1 fallback and its re-import
// ritual for good.
use crate::manifest::Manifest;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::json;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ProjectRecord {
    pub name: String,
    /// From the sigiled.toml pin on master, e.g. "vm-tmpl@0.1.0"; null on
    /// repos that never adopted the pin.
    pub template_version: Option<String>,
    /// True when the pin trails the latest published template (design §3).
    /// An unpinned repo is not "behind": it never adopted the mechanism.
    pub template_behind: bool,
    pub needs_merge: bool,
}

impl ProjectRecord {
    pub fn new(name: &str, manifest: &Manifest, latest_template: Option<&str>) -> Self {
        let pin = manifest
            .template
            .as_ref()
            .map(|t| (t.name.clone(), t.version.clone()));
        let behind = match (&pin, latest_template) {
            (Some((_, pinned)), Some(latest)) => semver_lt(pinned, latest),
            _ => false,
        };
        ProjectRecord {
            name: name.to_string(),
            template_version: pin.map(|(n, v)| format!("{n}@{v}")),
            template_behind: behind,
            needs_merge: false,
        }
    }
}

/// x.y.z ordering, numeric per component; anything unparsable compares as 0
/// (a malformed pin never flags a project as behind by accident of parsing —
/// the manifest parser is where malformation gets loud).
fn semver_lt(a: &str, b: &str) -> bool {
    fn triple(v: &str) -> (u64, u64, u64) {
        let mut it = v.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        )
    }
    triple(a) < triple(b)
}

/// v1 NAME_RE, kept verbatim: lowercase alnum + dashes, 2-39 chars, letter
/// first. The GitHub repo name and the container DNS name both ride on it.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (2..=39).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

#[derive(Default, Clone)]
pub struct Registry {
    records: Arc<RwLock<Vec<ProjectRecord>>>,
    /// Latest published template version (e.g. "0.1.0"); None in a build
    /// that doesn't know it (records then never show behind).
    latest_template: Option<String>,
}

impl Registry {
    pub fn with_latest_template(version: Option<String>) -> Self {
        Registry {
            records: Arc::default(),
            latest_template: version,
        }
    }
    pub fn latest_template(&self) -> Option<&str> {
        self.latest_template.as_deref()
    }
    pub fn insert(&self, record: ProjectRecord) {
        self.records.write().unwrap().push(record);
    }
    pub fn contains(&self, name: &str) -> bool {
        self.records.read().unwrap().iter().any(|r| r.name == name)
    }
    pub fn snapshot(&self) -> Vec<ProjectRecord> {
        self.records.read().unwrap().clone()
    }
    pub fn replace_all(&self, records: Vec<ProjectRecord>) {
        *self.records.write().unwrap() = records;
    }
    /// Re-derive the manifest-owned fields (template_version,
    /// template_behind) of one record from a freshly read master manifest,
    /// in place under the write lock. needs_merge belongs to the merge
    /// machinery and is never touched here. Returns true only on an actual
    /// change, so callers persist exactly when something moved.
    pub fn refresh(&self, name: &str, manifest: &Manifest) -> bool {
        let fresh = ProjectRecord::new(name, manifest, self.latest_template.as_deref());
        let mut records = self.records.write().unwrap();
        match records.iter_mut().find(|r| r.name == name) {
            Some(r)
                if r.template_version != fresh.template_version
                    || r.template_behind != fresh.template_behind =>
            {
                r.template_version = fresh.template_version;
                r.template_behind = fresh.template_behind;
                true
            }
            _ => false,
        }
    }
}

pub async fn list(
    _actor: crate::auth::Actor,
    State(state): State<crate::AppState>,
) -> axum::Json<serde_json::Value> {
    // Records enriched with the live merge-debt queue (design §4: status
    // shows the debt queue per project, next to needs_merge).
    let enriched: Vec<serde_json::Value> = state
        .registry
        .snapshot()
        .into_iter()
        .map(|r| {
            let mut v = serde_json::to_value(&r).unwrap();
            v["merge_debt"] = serde_json::to_value(state.sessions.debts_for(&r.name)).unwrap();
            v
        })
        .collect();
    axum::Json(serde_json::Value::Array(enriched))
}

/// GET /sigiled/projects/{p}/branches — `[{name, sha}]`, local and origin
/// refs merged (local wins), the job-recap entry point (contract §7).
pub async fn branches(
    actor: crate::auth::Actor,
    State(state): State<crate::AppState>,
    axum::extract::Path(project): axum::extract::Path<String>,
) -> Response {
    fn err(status: StatusCode, detail: impl Into<String>) -> Response {
        (status, Json(json!({ "detail": detail.into() }))).into_response()
    }
    if let Err(denial) = crate::auth::authorize(
        &actor,
        crate::auth::Action::JobRecap,
        Some(&project),
        &state.auth.approvals,
        crate::auth::now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, denial.0);
    }
    let repo = match &state.sessions.runtime {
        Some(rt) => match rt.ensure_mirror(&project) {
            Ok(p) => p,
            Err(e) => return err(StatusCode::NOT_FOUND, e),
        },
        None => {
            let Some(repos) = state.sessions.repos_dir.clone() else {
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "SIGILED_REPOS_DIR not configured",
                );
            };
            let p = repos.join(&project);
            if !p.join(".git").exists() {
                return err(StatusCode::NOT_FOUND, format!("unknown project: {project}"));
            }
            p
        }
    };
    let refs = match crate::merge::git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(refname:short) %(objectname)",
            "refs/heads",
            "refs/remotes/origin",
        ],
    ) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let mut seen = std::collections::BTreeMap::new();
    for line in refs.lines() {
        let Some((name, sha)) = line.trim().rsplit_once(' ') else {
            continue;
        };
        let name = name.trim().trim_start_matches("origin/");
        // `origin` alone is origin/HEAD's short name; skip the aliases.
        if name.is_empty() || name == "HEAD" || name == "origin" {
            continue;
        }
        seen.entry(name.to_string())
            .or_insert_with(|| sha.to_string());
    }
    let list: Vec<serde_json::Value> = seen
        .into_iter()
        .map(|(name, sha)| json!({ "name": name, "sha": sha }))
        .collect();
    Json(serde_json::Value::Array(list)).into_response()
}

#[derive(serde::Deserialize)]
pub struct NewProject {
    pub name: String,
}

/// POST /sigiled/projects — create from the vm-tmpl template, or adopt an
/// existing repo of that name (key + register, nothing written). Drivers
/// need a live approval (Action::ProjectsNew); there is no delete verb —
/// projects are permanent.
pub async fn create(
    actor: crate::auth::Actor,
    State(state): State<crate::AppState>,
    Json(body): Json<NewProject>,
) -> Response {
    fn err(status: StatusCode, detail: impl Into<String>) -> Response {
        (status, Json(json!({ "detail": detail.into() }))).into_response()
    }
    if let Err(denial) = crate::auth::authorize(
        &actor,
        crate::auth::Action::ProjectsNew,
        Some(&body.name),
        &state.auth.approvals,
        crate::auth::now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, denial.0);
    }
    if !valid_name(&body.name) {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "name: lowercase alnum + dashes, 2-39 chars, letter first",
        );
    }
    if state.registry.contains(&body.name) {
        return err(
            StatusCode::CONFLICT,
            format!("project '{}' already registered", body.name),
        );
    }
    let Some(gh) = &state.github else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "GITHUB_PAT not configured");
    };
    // Order is the v1's: repo first (create or adopt), then the key pair,
    // then the key on the repo, registration last — a failure anywhere
    // leaves nothing registered, and the verb can simply be retried.
    let http = reqwest::Client::new();
    let (repo_full, adopted) = match gh.create_or_adopt(&http, &body.name).await {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_GATEWAY, e),
    };
    let pubkey = match gh.generate_deploy_key(&body.name) {
        Ok(k) => k,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if let Err(e) = gh.add_deploy_key(&http, &body.name, &pubkey).await {
        return err(StatusCode::BAD_GATEWAY, e);
    }
    // template_version stays null at birth, like every imported record: the
    // pin is read from sigiled.toml on master by the flows that fetch it.
    let record = ProjectRecord {
        name: body.name.clone(),
        template_version: None,
        template_behind: false,
        needs_merge: false,
    };
    state.registry.insert(record.clone());
    state.events.record(
        &body.name,
        crate::auth::now_epoch(),
        crate::events::Event::ProjectCreated {
            repo: repo_full.clone(),
            adopted,
        },
    );
    state.persist();
    tracing::info!(project = %body.name, %repo_full, adopted, "project registered");
    let mut resp = serde_json::to_value(&record).unwrap();
    resp["repo"] = json!(repo_full);
    resp["adopted"] = json!(adopted);
    (StatusCode::CREATED, Json(resp)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[test]
    fn record_exposes_template_version_from_manifest() {
        let m = Manifest::parse("template = \"vm-tmpl@0.1.0\"\n").unwrap();
        let r = ProjectRecord::new("smoke", &m, None);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["template_version"], "vm-tmpl@0.1.0");
        assert_eq!(json["name"], "smoke");
    }

    #[test]
    fn unpinned_record_serializes_null_template_version() {
        let m = Manifest::parse("class = \"session\"\n").unwrap();
        let json = serde_json::to_value(ProjectRecord::new("legacy", &m, Some("0.2.0"))).unwrap();
        assert!(json["template_version"].is_null());
        // Unpinned is not behind: it never adopted the mechanism.
        assert_eq!(json["template_behind"], false);
    }

    #[test]
    fn pinned_behind_latest_is_flagged() {
        let m = Manifest::parse("template = \"vm-tmpl@0.1.0\"\n").unwrap();
        let r = ProjectRecord::new("smoke", &m, Some("0.2.0"));
        assert!(r.template_behind);
    }

    #[test]
    fn pinned_at_latest_is_not_behind() {
        let m = Manifest::parse("template = \"vm-tmpl@0.2.0\"\n").unwrap();
        assert!(!ProjectRecord::new("smoke", &m, Some("0.2.0")).template_behind);
        // Ahead (edge: local template newer than published) is not behind.
        let m2 = Manifest::parse("template = \"vm-tmpl@0.3.0\"\n").unwrap();
        assert!(!ProjectRecord::new("smoke", &m2, Some("0.2.0")).template_behind);
    }

    #[test]
    fn semver_compares_numerically_not_lexically() {
        assert!(semver_lt("0.9.0", "0.10.0"));
        assert!(!semver_lt("0.10.0", "0.9.0"));
        assert!(semver_lt("1.2.3", "2.0.0"));
    }

    // --- Registry::refresh — the pin-reader wiring ---------------------------

    #[test]
    fn refresh_populates_pin_fields_and_preserves_needs_merge() {
        let reg = Registry::with_latest_template(Some("0.2.0".into()));
        reg.insert(ProjectRecord {
            name: "smoke".into(),
            template_version: None,
            template_behind: false,
            needs_merge: true,
        });
        let m = Manifest::parse("template = \"vm-tmpl@0.1.0\"\n").unwrap();
        assert!(reg.refresh("smoke", &m), "first refresh reports a change");
        let r = &reg.snapshot()[0];
        assert_eq!(r.template_version.as_deref(), Some("vm-tmpl@0.1.0"));
        assert!(r.template_behind, "pin 0.1.0 trails latest 0.2.0");
        assert!(r.needs_merge, "refresh never touches needs_merge");
    }

    #[test]
    fn refresh_is_idempotent_and_ignores_unknown_projects() {
        let reg = Registry::with_latest_template(None);
        reg.insert(ProjectRecord {
            name: "smoke".into(),
            template_version: None,
            template_behind: false,
            needs_merge: false,
        });
        let m = Manifest::parse("template = \"vm-tmpl@0.1.0\"\n").unwrap();
        assert!(reg.refresh("smoke", &m));
        assert!(!reg.refresh("smoke", &m), "same manifest = no change");
        assert!(
            !reg.refresh("ghost", &m),
            "unknown project = no change, no panic"
        );
    }

    // --- POST /projects (session 5) -----------------------------------------

    fn admin() -> crate::auth::Actor {
        crate::auth::Actor {
            driver: "bootstrap".into(),
            role: crate::auth::Role::Admin,
            approval: None,
        }
    }
    fn driver() -> crate::auth::Actor {
        crate::auth::Actor {
            driver: "sigiled-claude".into(),
            role: crate::auth::Role::Driver,
            approval: None,
        }
    }

    async fn body_json(resp: Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn tmp_keys(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sigil-newproj-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A local GitHub double: template generate (201 or a canned 422),
    /// repo probe, deploy-key capture. What POST /projects talks to in
    /// tests instead of the real API.
    async fn mock_github(
        generate_status: u16,
        probe_status: u16,
    ) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
        use axum::extract::Path as AxPath;
        use axum::routing::{get, post};
        let keys: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
        let captured = keys.clone();
        let app = axum::Router::new()
            .route(
                "/repos/{owner}/{repo}/generate",
                post(move |Json(body): Json<serde_json::Value>| async move {
                    if generate_status == 201 {
                        (
                            StatusCode::CREATED,
                            Json(json!({
                                "full_name":
                                    format!("example-org/{}", body["name"].as_str().unwrap())
                            })),
                        )
                            .into_response()
                    } else {
                        (
                            StatusCode::from_u16(generate_status).unwrap(),
                            Json(json!({ "message": "Could not clone: Name already exists" })),
                        )
                            .into_response()
                    }
                }),
            )
            .route(
                "/repos/{owner}/{repo}",
                get(
                    move |AxPath((_o, _r)): AxPath<(String, String)>| async move {
                        StatusCode::from_u16(probe_status).unwrap()
                    },
                ),
            )
            .route(
                "/repos/{owner}/{repo}/keys",
                post(
                    move |AxPath((_o, repo)): AxPath<(String, String)>,
                          Json(body): Json<serde_json::Value>| {
                        let captured = captured.clone();
                        async move {
                            captured
                                .lock()
                                .unwrap()
                                .push(json!({ "repo": repo, "body": body }));
                            StatusCode::CREATED
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (base, keys)
    }

    fn state_with_github(base: &str, keys_dir: PathBuf) -> crate::AppState {
        crate::AppState {
            github: Some(crate::github::GitHub {
                api_base: base.to_string(),
                pat: "test-pat".into(),
                owner: "example-org".into(),
                template: "vm-tmpl".into(),
                keys_dir,
            }),
            ..crate::AppState::default()
        }
    }

    #[test]
    fn name_rules_are_the_v1_rules() {
        for good in ["ab", "reddit-mine", "a2", "smoke-2026"] {
            assert!(valid_name(good), "{good} should be valid");
        }
        for bad in ["a", "9abc", "Abc", "a_b", "-ab", "ab cd", ""] {
            assert!(!valid_name(bad), "{bad} should be invalid");
        }
        assert!(valid_name(&("a".repeat(39))));
        assert!(!valid_name(&("a".repeat(40))));
    }

    #[tokio::test]
    async fn branches_lists_names_and_shas() {
        let repo = crate::merge::tests::mk_repo("brl");
        let repos_dir = repo.parent().unwrap().to_path_buf();
        let renamed = repos_dir.join("brproj");
        let _ = std::fs::remove_dir_all(&renamed);
        std::fs::rename(&repo, &renamed).unwrap();
        crate::merge::tests::sh(&renamed, &["branch", "job-x-20260803-000000"]);
        crate::merge::tests::commit_on(
            &renamed,
            "job-x-20260803-000000",
            "out.txt",
            "o\n",
            "job: out",
        );
        let state = crate::AppState {
            sessions: crate::sessions::SessionState::with_repos_dir(repos_dir),
            ..crate::AppState::default()
        };
        let (status, body) =
            body_json(branches(admin(), State(state), axum::extract::Path("brproj".into())).await)
                .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        let list = body.as_array().unwrap();
        let names: Vec<&str> = list.iter().map(|b| b["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"master"), "{names:?}");
        assert!(names.contains(&"job-x-20260803-000000"), "{names:?}");
        for b in list {
            assert_eq!(b["sha"].as_str().unwrap().len(), 40);
        }
    }

    #[tokio::test]
    async fn create_invalid_name_is_422() {
        let state = crate::AppState::default();
        let (status, body) = body_json(
            create(
                admin(),
                State(state),
                Json(NewProject {
                    name: "Bad_Name".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["detail"].as_str().unwrap().contains("lowercase"));
    }

    #[tokio::test]
    async fn create_duplicate_is_409() {
        let state = crate::AppState::default();
        state.registry.insert(ProjectRecord {
            name: "torchio".into(),
            template_version: None,
            template_behind: false,
            needs_merge: false,
        });
        let (status, body) = body_json(
            create(
                admin(),
                State(state),
                Json(NewProject {
                    name: "torchio".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["detail"].as_str().unwrap().contains("torchio"));
    }

    #[tokio::test]
    async fn create_without_pat_is_503() {
        // github: None (no GITHUB_PAT in the environment of this state).
        let state = crate::AppState::default();
        let (status, body) = body_json(
            create(
                admin(),
                State(state),
                Json(NewProject {
                    name: "fresh-proj".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["detail"].as_str().unwrap().contains("GITHUB_PAT"));
    }

    #[tokio::test]
    async fn driver_without_approval_cannot_create() {
        let state = crate::AppState::default();
        let (status, body) = body_json(
            create(
                driver(),
                State(state),
                Json(NewProject {
                    name: "fresh-proj".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body["detail"].as_str().unwrap().contains("approval"));
    }

    #[tokio::test]
    async fn create_generates_repo_key_record_and_event() {
        let (base, captured) = mock_github(201, 404).await;
        let keys_dir = tmp_keys("create");
        let state = state_with_github(&base, keys_dir.clone());
        let (status, body) = body_json(
            create(
                admin(),
                State(state.clone()),
                Json(NewProject {
                    name: "smoke-new".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {body}");
        assert_eq!(body["name"], "smoke-new");
        assert_eq!(body["adopted"], false);
        assert_eq!(body["repo"], "example-org/smoke-new");
        assert!(body["template_version"].is_null());
        // Registered, key on disk in the runtime's layout, key sent to GitHub.
        assert!(state.registry.contains("smoke-new"));
        assert!(keys_dir.join("smoke-new").join("id_ed25519").exists());
        let sent = captured.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["repo"], "smoke-new");
        assert!(sent[0]["body"]["key"]
            .as_str()
            .unwrap()
            .starts_with("ssh-ed25519 "));
        assert_eq!(sent[0]["body"]["read_only"], false);
        // The machine log knows (design §2).
        let events = state.events.for_project("smoke-new");
        assert_eq!(events.len(), 1);
        let ev = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(ev["kind"], "project_created");
        assert_eq!(ev["adopted"], false);
    }

    #[tokio::test]
    async fn existing_repo_is_adopted_not_failed() {
        // generate 422 + probe 200 = the v1 §7.8 adoption path.
        let (base, captured) = mock_github(422, 200).await;
        let state = state_with_github(&base, tmp_keys("adopt"));
        let (status, body) = body_json(
            create(
                admin(),
                State(state.clone()),
                Json(NewProject {
                    name: "legacy-repo".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {body}");
        assert_eq!(body["adopted"], true);
        assert!(state.registry.contains("legacy-repo"));
        // Adoption still keys the repo: sessions need the deploy key.
        assert_eq!(captured.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn generate_rejection_without_repo_stays_loud() {
        // 422 from generate but the repo does NOT exist: a genuine
        // validation error must never silently register a phantom project.
        let (base, _) = mock_github(422, 404).await;
        let state = state_with_github(&base, tmp_keys("phantom"));
        let (status, body) = body_json(
            create(
                admin(),
                State(state.clone()),
                Json(NewProject {
                    name: "phantom-proj".into(),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "body: {body}");
        assert!(!state.registry.contains("phantom-proj"));
    }
}
