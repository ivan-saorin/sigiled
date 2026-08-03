// apps.rs — resident apps, v2-pure (contract §7, per decreto del Re: niente
// app v1 convertite). The model in one breath: a project repo declares at
// most one `[app]` in its sigiled.toml on master; the image is built FROM
// THE REPO at the declared master sha and tagged `{name}:{sha12}` — no
// `image:` keys, no external filesystems: config bakes into the image;
// the container carries the app name, which on the stack network IS the
// route. `upgrade` is the deploy verb (refresh sha → build if absent →
// recreate; same sha = config refresh). `start` never recreates. Builds
// run in the background: the verb answers 202 `action: "building"` and the
// status carries the build record — poll, never re-fire (contract).
//
// App verbs are approval territory for drivers (§1.6): the same authorize()
// the sessions consume.
use crate::auth::{authorize, Action, Actor};
use crate::manifest::{AppManifest, Manifest};
use axum::extract::{Path as AxPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRecord {
    pub sha: String,
    pub ok: bool,
    pub finished_epoch: u64,
    pub log_tail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRecord {
    pub name: String,
    pub project: String,
    /// master sha12 the current image was built from.
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// "building" while a background build runs (contract: poll, don't re-fire).
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub build: Option<BuildRecord>,
}

fn now_epoch() -> u64 {
    crate::auth::now_epoch()
}

fn err(status: StatusCode, detail: impl Into<String>) -> Response {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

/// Find which project declares app `name`: persisted record first, then a
/// scan of the registry's projects (mirror refresh + manifest parse). The
/// incumbent keeps the name: the first project found wins, a second
/// declaration simply never resolves (its verbs 404 — loud enough).
fn resolve(state: &crate::AppState, name: &str) -> Result<(String, AppManifest), String> {
    let rt = state.sessions.runtime.as_ref().ok_or("runtime not configured")?;
    if let Some(rec) = state.apps.get(name) {
        if let Some(m) = read_app_manifest(rt, &rec.project)? {
            if m.name == name {
                return Ok((rec.project.clone(), m));
            }
        }
        // The declaration moved or vanished: fall through to a fresh scan.
    }
    for p in state.registry.snapshot() {
        if let Some(m) = read_app_manifest(rt, &p.name).unwrap_or(None) {
            if m.name == name {
                return Ok((p.name, m));
            }
        }
    }
    Err(format!("no project declares app {name:?}"))
}

fn read_app_manifest(
    rt: &crate::runtime::Runtime,
    project: &str,
) -> Result<Option<AppManifest>, String> {
    let repo = rt.ensure_mirror(project)?;
    for f in ["sigiled.toml", "mgr.toml"] {
        let path = repo.join(f);
        if let Ok(text) = std::fs::read_to_string(&path) {
            // Broken manifest = that project's app is disabled, loudly.
            let m = Manifest::parse(&text).map_err(|e| format!("{project}/{f}: {e}"))?;
            if m.app.is_some() {
                return Ok(m.app);
            }
        }
    }
    Ok(None)
}

/// docker create args for an app container — pure, testable: name is DNS,
/// volumes are declared mounts, secrets resolve from the stack env HERE
/// (rule 8: values reach the container as env, never any repo).
pub fn create_args(m: &AppManifest, image: &str, network: &str) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = vec![
        "create".into(),
        "--name".into(), m.name.clone(),
        "--hostname".into(), m.name.clone(),
        "--network".into(), network.into(),
        "--restart".into(), "unless-stopped".into(),
        "--label".into(), "sigiled.kind=app".into(),
    ];
    for (vol, target) in &m.volumes {
        args.push("-v".into());
        args.push(format!("{vol}:{target}"));
    }
    for (env_name, stack_var) in &m.secrets {
        let value = std::env::var(stack_var)
            .map_err(|_| format!("secret {env_name}: stack env {stack_var} is not set"))?;
        args.push("-e".into());
        args.push(format!("{env_name}={value}"));
    }
    args.push(image.into());
    Ok(args)
}

pub async fn status(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath(name): AxPath<String>,
) -> Response {
    let _ = actor; // any authenticated actor may look
    let Some(rec) = state.apps.get(&name) else {
        return err(StatusCode::NOT_FOUND, format!("unknown app: {name}"));
    };
    let container_state = state
        .sessions
        .runtime
        .as_ref()
        .map(|rt| rt.container_state(&name))
        .unwrap_or_else(|| "unknown".into());
    Json(json!({
        "name": rec.name, "project": rec.project, "sha": rec.sha,
        "image": rec.image, "action": rec.action, "build": rec.build,
        "container_state": container_state,
    }))
    .into_response()
}

pub async fn action(
    actor: Actor,
    State(state): State<crate::AppState>,
    AxPath((name, verb)): AxPath<(String, String)>,
) -> Response {
    if let Err(denial) =
        authorize(&actor, Action::AppVerb, None, &state.auth.approvals, now_epoch())
    {
        return err(StatusCode::FORBIDDEN, denial.0);
    }
    let Some(rt) = state.sessions.runtime.clone() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "runtime not configured");
    };
    match verb.as_str() {
        // start never recreates (contract): a missing container needs upgrade.
        "start" => match rt.docker_pub(&["start", &name]) {
            Ok(_) => Json(json!({"name": name, "action": "started"})).into_response(),
            Err(e) => err(
                StatusCode::CONFLICT,
                format!("start {name}: {e} — se il container non esiste, serve upgrade"),
            ),
        },
        "stop" => match rt.docker_pub(&["stop", &name]) {
            Ok(_) => Json(json!({"name": name, "action": "stopped"})).into_response(),
            Err(e) => err(StatusCode::CONFLICT, e),
        },
        "restart" => match rt.docker_pub(&["restart", &name]) {
            Ok(_) => Json(json!({"name": name, "action": "restarted"})).into_response(),
            Err(e) => err(StatusCode::CONFLICT, e),
        },
        "upgrade" => upgrade(state, rt, name).await,
        other => err(StatusCode::NOT_FOUND, format!("unknown action: {other}")),
    }
}

async fn upgrade(state: crate::AppState, rt: crate::runtime::Runtime, name: String) -> Response {
    // One build at a time per app: the record's action flag is the latch.
    if state.apps.get(&name).and_then(|r| r.action.clone()).as_deref() == Some("building") {
        return err(StatusCode::CONFLICT, format!("{name} is already building — poll status"));
    }
    let (project, manifest) = match resolve(&state, &name) {
        Ok(x) => x,
        Err(e) => return err(StatusCode::NOT_FOUND, e),
    };
    let repo = match rt.ensure_mirror(&project) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let sha = match crate::merge::git(&repo, &["rev-parse", "--short=12", "master"]) {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let image = format!("{name}:{sha}");

    if rt.image_exists(&image) {
        // Same sha (or image already built): config refresh — recreate now.
        return match recreate(&rt, &manifest, &image) {
            Ok(()) => {
                state.apps.upsert(AppRecord {
                    name: name.clone(),
                    project,
                    sha: Some(sha),
                    image: Some(image),
                    action: None,
                    build: state.apps.get(&name).and_then(|r| r.build),
                });
                state.persist();
                Json(json!({"name": name, "action": "recreated"})).into_response()
            }
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
    }

    // New sha: background build, 202, poll the status (contract).
    state.apps.upsert(AppRecord {
        name: name.clone(),
        project: project.clone(),
        sha: Some(sha.clone()),
        image: Some(image.clone()),
        action: Some("building".into()),
        build: state.apps.get(&name).and_then(|r| r.build),
    });
    state.persist();
    let app_state = state.clone();
    let (build_image, build_manifest, build_repo, build_rt) =
        (image.clone(), manifest.clone(), repo.clone(), rt.clone());
    let name_task = name.clone();
    tokio::spawn(async move {
        let name = name_task;
        let build = tokio::task::spawn_blocking(move || {
            build_rt
                .build_image(&build_image, &build_manifest.dockerfile, &build_repo)
                .and_then(|log| recreate(&build_rt, &build_manifest, &build_image).map(|()| log))
        })
        .await
        .unwrap_or_else(|e| Err(format!("build task: {e}")));
        let (ok, log_tail) = match build {
            Ok(log) => (true, log),
            Err(e) => (false, e),
        };
        if !ok {
            tracing::error!(app = %name, tail = %log_tail, "app build/recreate failed");
        }
        app_state.apps.upsert(AppRecord {
            name: name.clone(),
            project,
            sha: Some(sha.clone()),
            image: Some(image.clone()),
            action: None,
            build: Some(BuildRecord {
                sha,
                ok,
                finished_epoch: now_epoch(),
                log_tail,
            }),
        });
        app_state.persist();
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({"name": name, "action": "building", "image": null})),
    )
        .into_response()
}

fn recreate(rt: &crate::runtime::Runtime, m: &AppManifest, image: &str) -> Result<(), String> {
    let _ = rt.docker_pub(&["rm", "-f", &m.name]);
    let args = create_args(m, image, &rt.network)?;
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    rt.docker_pub(&arg_refs)?;
    rt.docker_pub(&["start", &m.name])?;
    Ok(())
}

// --- store ------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct AppsState(
    std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, AppRecord>>>,
);

impl AppsState {
    pub fn get(&self, name: &str) -> Option<AppRecord> {
        self.0.read().unwrap().get(name).cloned()
    }
    pub fn upsert(&self, rec: AppRecord) {
        self.0.write().unwrap().insert(rec.name.clone(), rec);
    }
    pub fn dump(&self) -> std::collections::HashMap<String, AppRecord> {
        self.0.read().unwrap().clone()
    }
    pub fn hydrate(&self, map: std::collections::HashMap<String, AppRecord>) {
        *self.0.write().unwrap() = map;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> AppManifest {
        Manifest::parse(
            "[app]\nname = \"reddit-mine\"\n[app.volumes]\nreddit-mine-data = \"/data:rw\"\n[app.secrets]\nTZ = \"TZ\"\n",
        )
        .unwrap()
        .app
        .unwrap()
    }

    #[test]
    fn create_args_carry_name_volumes_and_resolved_secrets() {
        std::env::set_var("TZ", "Europe/Rome");
        let args = create_args(&manifest(), "reddit-mine:abc123def456", "mgr-net").unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("--name reddit-mine"));
        assert!(joined.contains("--hostname reddit-mine"));
        assert!(joined.contains("-v reddit-mine-data:/data:rw"));
        assert!(joined.contains("-e TZ=Europe/Rome"));
        assert!(joined.ends_with("reddit-mine:abc123def456"));
    }

    #[test]
    fn missing_stack_secret_is_a_loud_failure() {
        let mut m = manifest();
        m.secrets.insert("API_KEY".into(), "SIGILED_TEST_UNSET_VAR".into());
        let e = create_args(&m, "img:sha", "mgr-net").unwrap_err();
        assert!(e.contains("SIGILED_TEST_UNSET_VAR"), "{e}");
    }

    #[test]
    fn records_round_trip_through_the_store() {
        let apps = AppsState::default();
        apps.upsert(AppRecord {
            name: "reddit-mine".into(),
            project: "reddit-mine".into(),
            sha: Some("abc123def456".into()),
            image: Some("reddit-mine:abc123def456".into()),
            action: None,
            build: Some(BuildRecord {
                sha: "abc123def456".into(),
                ok: true,
                finished_epoch: 1,
                log_tail: "done".into(),
            }),
        });
        let dumped = apps.dump();
        let fresh = AppsState::default();
        fresh.hydrate(dumped);
        assert_eq!(fresh.get("reddit-mine").unwrap().image.as_deref(), Some("reddit-mine:abc123def456"));
    }
}
