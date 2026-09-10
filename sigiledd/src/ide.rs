//! Platform-managed, project-image-preserving IDE. No browser credentials.
use crate::{
    auth::{Action, Actor},
    sessions::{Lifecycle, SessionRecord},
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
pub const PROVIDER: &str = "4.136.2";
pub const HELPER: &str = "1";
#[derive(Clone, Serialize, Deserialize)]
pub struct Binding {
    pub provider: String,
    pub helper: String,
    pub base_digest: String,
    pub image: String,
    pub generation: u64,
    pub profile_volume: String,
    #[serde(default)]
    pub started: bool,
    pub state: String,
    pub error: Option<String>,
    pub token: String,
    pub activity_token: String,
}
impl Binding {
    pub fn view(&self) -> serde_json::Value {
        json!({"provider":"code-server","version":self.provider,"helper":self.helper,"base_digest":self.base_digest,"image":self.image,"generation":self.generation,"state":self.state,"error":self.error})
    }
}
pub fn key(digest: &str, version: &str, helper: &str) -> Result<String, &'static str> {
    if !digest.starts_with("sha256:")
        || digest.len() != 71
        || !digest[7..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("immutable_project_image_required");
    }
    Ok(format!(
        "sigil-ide:{:x}",
        Sha256::digest(format!("{digest}\n{version}\n{helper}"))
    ))
}
fn volume(actor: &str) -> String {
    format!("sigil-ide-profile-{:x}", Sha256::digest(actor.as_bytes()))
}
pub fn host_merge_push(
    rt: &crate::runtime::Runtime,
    project: &str,
    reference: &str,
) -> Result<String, String> {
    if reference != "master" || !crate::project::valid_name(project) {
        return Err("unsupported host merge ref".into());
    }
    let pat = std::env::var("GITHUB_PAT").map_err(|_| "host merge credential unavailable")?;
    let url = format!("https://github.com/{}/{project}.git", rt.owner);
    host_transport(&rt.repo_path(project), &url, &pat, false)
}
fn host_transport(
    repo: &std::path::Path,
    url: &str,
    pat: &str,
    probe: bool,
) -> Result<String, String> {
    let mut command = crate::bounded_process::git_command();
    command.arg("-C").arg(repo).args(["-c","credential.helper=","-c",r#"credential.helper=!f() { test "$1" = get && printf 'username=x-access-token\npassword=%s\n' "$SIGILED_MERGE_PAT"; }; f"#,"-c","http.followRedirects=false","push"]).env("SIGILED_MERGE_PAT",pat).env("GIT_TERMINAL_PROMPT","0");
    if probe {
        command.arg("--dry-run");
    }
    command.args(["--", url, "refs/heads/master:refs/heads/master"]);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_TRACE") {
            command.env_remove(key);
        }
    }
    let output = crate::bounded_process::output_until(
        &mut command,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
    .map_err(|_| "host merge push failed; workspace preserved")?;
    if output.status.success() {
        Ok(String::new())
    } else {
        Err("host merge push failed; workspace preserved".into())
    }
}
fn response(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({"error":error}))).into_response()
}
#[derive(Deserialize)]
pub struct Operation {
    pub generation: u64,
    pub action: String,
}
pub(crate) async fn request(
    state: &crate::AppState,
    record: &SessionRecord,
    path: &str,
) -> Result<serde_json::Value, &'static str> {
    let b = record
        .binding
        .as_ref()
        .and_then(|b| b.ide.as_ref())
        .ok_or("ide_setup_required")?;
    if b.generation != record.generation {
        return Err("generation_changed");
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(if path == "status" {
            1
        } else {
            40
        }))
        .build()
        .map_err(|_| "provider_unavailable")?;
    let url = format!("http://{}:8090/{path}", record.container());
    let req = if path == "status" {
        client.get(url)
    } else {
        client.post(url)
    };
    let r = req
        .bearer_auth(&b.token)
        .send()
        .await
        .map_err(|_| "provider_unavailable")?;
    let success = r.status().is_success();
    let mut r = r;
    let mut bytes = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|_| "provider_unavailable")? {
        if bytes.len() + chunk.len() > 65536 {
            return Err("provider_response_invalid");
        }
        bytes.extend_from_slice(&chunk)
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "provider_response_invalid")?;
    if !success {
        return Err(safe_error(value["error"].as_str().unwrap_or("")));
    }
    let _ = state;
    safe_response(&value, path, record.generation)
}
pub async fn status(
    actor: Actor,
    State(state): State<crate::AppState>,
    Path(id): Path<String>,
) -> Response {
    let Some(r) = state.sessions.record(&id) else {
        return response(StatusCode::NOT_FOUND, "unknown_session");
    };
    if crate::sessions::authorized(&actor, &r, &state, Action::OpenSession).is_err() {
        return response(StatusCode::FORBIDDEN, "session_not_authorized");
    }
    let mut projection=r.binding.as_ref().and_then(|b|b.ide.as_ref()).map(Binding::view).unwrap_or(json!({"state":"setup_required","error":r.binding.as_ref().and_then(|b|b.ide_error.clone()).unwrap_or_else(||"provider_image_required".into())}));
    if r.binding
        .as_ref()
        .and_then(|b| b.ide.as_ref())
        .is_some_and(|b| b.started)
    {
        match request(&state, &r, "status").await {
            Ok(v) => projection["observed"] = v,
            Err(e) => {
                projection["state"] = json!("failed");
                projection["error"] = json!(e)
            }
        }
    }
    Json(json!({"desired":state.registry.descriptor(&r.project).declaration.ide.enabled,"provider":projection})).into_response()
}
pub async fn operation(
    actor: Actor,
    State(state): State<crate::AppState>,
    Path(id): Path<String>,
    Json(op): Json<Operation>,
) -> Response {
    match tokio::spawn(operation_inner(actor, State(state), Path(id), Json(op))).await {
        Ok(r) => r,
        Err(_) => response(StatusCode::CONFLICT, "provider_operation_interrupted"),
    }
}
async fn operation_inner(
    actor: Actor,
    State(state): State<crate::AppState>,
    Path(id): Path<String>,
    Json(op): Json<Operation>,
) -> Response {
    if !matches!(
        op.action.as_str(),
        "start" | "stop" | "checkpoint" | "finish"
    ) {
        return response(StatusCode::BAD_REQUEST, "unsupported_ide_action");
    }
    let lock = state.sessions.session_lock(&id);
    let guard = lock.lock_owned().await;
    let Some(mut r) = state.sessions.record(&id) else {
        return response(StatusCode::NOT_FOUND, "unknown_session");
    };
    if crate::sessions::authorized(&actor, &r, &state, Action::OpenSession).is_err() {
        return response(StatusCode::FORBIDDEN, "session_not_authorized");
    }
    if r.lifecycle != Lifecycle::Active
        || r.generation != op.generation
        || !r.runtime_owned
        || !state.sessions.binding_safe(&r)
    {
        return response(StatusCode::CONFLICT, "session_generation_not_active");
    }
    if r.binding.as_ref().and_then(|b| b.ide.as_ref()).is_none() {
        return response(StatusCode::CONFLICT, "provider_image_required");
    }
    if op.action == "start" {
        if !state
            .registry
            .descriptor(&r.project)
            .declaration
            .ide
            .enabled
        {
            return response(StatusCode::CONFLICT, "ide_disabled");
        }
        let Some(github) = state.github.as_ref() else {
            return response(StatusCode::CONFLICT, "trusted_policy_inspection_required");
        };
        if let Err(error) = github.verify_ide_policy(&r.project).await {
            return response(StatusCode::CONFLICT, error);
        }
        let Some(runtime) = state.sessions.runtime.clone() else {
            return response(StatusCode::CONFLICT, "runtime_unavailable");
        };
        let mirror = state.sessions.merge_lock(&r.project).lock_owned().await;
        let probe_runtime = runtime.clone();
        let project = r.project.clone();
        let pat = github.pat.clone();
        let verified = tokio::task::spawn_blocking(move || {
            let _mirror = mirror;
            host_transport(
                &probe_runtime.repo_path(&project),
                &format!("https://github.com/{}/{project}.git", probe_runtime.owner),
                &pat,
                true,
            )
        })
        .await;
        if !matches!(verified, Ok(Ok(_))) {
            return response(StatusCode::CONFLICT, "host_merge_authority_unverified");
        }
        let available = request(&state, &r, "status").await.is_ok();
        if let Some(b) = r.binding.as_mut().and_then(|b| b.ide.as_mut()) {
            b.started = true;
            b.state = "starting".into();
            b.error = None;
        }
        state.sessions.put(r.clone());
        // Persist before allocation. A restart must never mistake an IDE-used
        // generation for an ordinary agent-only workspace and reap its work.
        if state.try_persist().is_err() {
            return response(StatusCode::CONFLICT, "persist_failed");
        }
        let guard = if available {
            guard
        } else {
            let record = r.clone();
            let (result, guard) = match tokio::task::spawn_blocking(move || {
                let result = runtime.start_companion(&record);
                (result, guard)
            })
            .await
            {
                Ok(value) => value,
                Err(_) => return response(StatusCode::CONFLICT, "provider_start_interrupted"),
            };
            if let Err(error) = result {
                if let Some(b) = r.binding.as_mut().and_then(|b| b.ide.as_mut()) {
                    b.state = "failed".into();
                    b.error = Some(error.into())
                }
                state.sessions.put(r);
                state.persist();
                return response(StatusCode::CONFLICT, error);
            }
            for _ in 0..20 {
                if request(&state, &r, "status").await.is_ok() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            guard
        };
        let result = request(&state, &r, "start").await;
        if let Some(b) = r.binding.as_mut().and_then(|b| b.ide.as_mut()) {
            b.state = if result.is_ok() { "ready" } else { "failed" }.into();
            b.error = result.as_ref().err().map(|s| s.to_string());
        }
        state.sessions.put(r);
        let persisted = state.try_persist().is_ok();
        drop(guard);
        if !persisted {
            return response(StatusCode::CONFLICT, "persist_failed");
        }
        return match result {
            Ok(value) => Json(value).into_response(),
            Err(error) => response(StatusCode::CONFLICT, error),
        };
    }
    let path = if op.action == "finish" {
        "checkpoint"
    } else {
        &op.action
    };
    match request(&state, &r, path).await {
        Err(e) => response(StatusCode::CONFLICT, e),
        Ok(v) => {
            if op.action == "finish" {
                drop(guard);
                crate::sessions::close_expected(actor, state, id, Some(op.generation)).await
            } else {
                if op.action == "stop" {
                    if let Some(b) = r.binding.as_mut().and_then(|b| b.ide.as_mut()) {
                        b.state = "stopped".into()
                    }
                    state.sessions.put(r);
                    state.persist();
                }
                Json(v).into_response()
            }
        }
    }
}
impl crate::runtime::Runtime {
    pub fn ide_layer(
        &self,
        base: &str,
        actor: &str,
        generation: u64,
    ) -> Result<Binding, &'static str> {
        let bundle = std::env::var("SIGILED_IDE_BUNDLE_DIR")
            .unwrap_or_else(|_| "/usr/local/share/sigil-ide".into());
        let mut hash = Sha256::new();
        for file in [
            "sigil-ide-agent",
            "activity/package.json",
            "activity/extension.js",
        ] {
            hash.update(
                std::fs::read(std::path::Path::new(&bundle).join(file))
                    .map_err(|_| "provider_bundle_required")?,
            )
        }
        let revision = format!("{HELPER}-{:x}", hash.finalize());
        let digest = self
            .ide_docker(&["image", "inspect", "--format", "{{.Id}}", base])
            .map_err(|_| "project_image_unavailable")?;
        let image = key(&digest, PROVIDER, &revision)?;
        if self
            .ide_docker(&["image", "inspect", "--format", "ok", &image])
            .is_err()
        {
            let user = self
                .ide_docker(&["image", "inspect", "--format", "{{.Config.User}}", base])
                .map_err(|_| "project_image_unavailable")?;
            let dockerfile = layer_dockerfile(&digest, &user)?;
            let input = private_input(dockerfile.as_bytes())?;
            let mut command = std::process::Command::new("docker");
            command.args(["build", "-t", &image, "-f", "-", &bundle]);
            let out = crate::bounded_process::output_until_input(
                &mut command,
                std::time::Instant::now() + std::time::Duration::from_secs(300),
                input,
            )
            .map_err(|_| "provider_build_failed")?;
            if !out.status.success() {
                return Err("incompatible_project_image");
            }
        }
        Ok(Binding {
            provider: PROVIDER.into(),
            helper: revision,
            base_digest: digest,
            image,
            generation,
            profile_volume: volume(actor),
            started: false,
            state: "stopped".into(),
            error: None,
            token: crate::sessions::mint_token(),
            activity_token: crate::sessions::mint_token(),
        })
    }
    pub fn start_companion(&self, r: &SessionRecord) -> Result<(), &'static str> {
        let b = r
            .binding
            .as_ref()
            .and_then(|b| b.ide.as_ref())
            .ok_or("provider_image_required")?;
        let config = json!({"token":b.token,"activity_token":b.activity_token,"generation":r.generation,"session":r.session_id,"branch":r.branch,"remote":self.repo_url(&r.project),"profile":format!("/sigil-profile/{}/{}",r.session_id,r.generation)});
        let path = format!("/tmp/sigil-ide-{}.json", crate::sessions::mint_token());
        let input = private_input(config.to_string().as_bytes())?;
        let mut command = std::process::Command::new("docker");
        command.args([
            "exec",
            "-i",
            &r.container(),
            "sh",
            "-c",
            "set -C; umask 077; cat > \"$1\"",
            "sigil-ide",
            &path,
        ]);
        let out = crate::bounded_process::output_until_input(
            &mut command,
            std::time::Instant::now() + std::time::Duration::from_secs(10),
            input,
        )
        .map_err(|_| "companion_setup_failed")?;
        if !out.status.success() {
            return Err("companion_setup_failed");
        }
        self.ide_docker(&[
            "exec",
            "-d",
            &r.container(),
            "/opt/sigil-ide/sigil-ide-agent",
            &path,
        ])?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immutable_cache_key_and_operator_profile_isolation() {
        let d = format!("sha256:{}", "a".repeat(64));
        assert_ne!(
            key(&d, PROVIDER, "1").unwrap(),
            key(&d, PROVIDER, "2").unwrap()
        );
        assert_ne!(
            key(&d, PROVIDER, "1").unwrap(),
            key(&format!("sha256:{}", "b".repeat(64)), PROVIDER, "1").unwrap()
        );
        assert!(key("mutable:latest", PROVIDER, HELPER).is_err());
        assert_ne!(volume("human:a"), volume("human:b"));
    }
}
impl std::fmt::Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdeBinding")
            .field("state", &self.state)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
pub async fn flush_record(state: &crate::AppState, record: &SessionRecord, label: &str) -> bool {
    if record
        .binding
        .as_ref()
        .and_then(|b| b.ide.as_ref())
        .is_some_and(|b| b.started)
    {
        return request(state, record, "checkpoint").await.is_ok()
            && request(state, record, "stop").await.is_ok();
    }
    match (&state.sessions.runtime, &record.token) {
        (Some(rt), Some(token)) => {
            rt.flush(state.sessions.http(), &record.container(), token, label)
                .await
        }
        _ => true,
    }
}
pub async fn idle_record(
    state: &crate::AppState,
    record: &SessionRecord,
    agent_idle: u64,
) -> Result<u64, &'static str> {
    if !record
        .binding
        .as_ref()
        .and_then(|b| b.ide.as_ref())
        .is_some_and(|b| b.started)
    {
        return Ok(agent_idle);
    }
    let status = request(state, record, "status").await?;
    if status["generation"].as_u64() != Some(record.generation) {
        return Err("generation_changed");
    }
    if status["busy"].as_bool() == Some(true) {
        return Ok(0);
    }
    Ok(agent_idle.min(status["idle_secs"].as_u64().ok_or("provider_unavailable")?))
}
pub async fn prepare_layer(
    rt: &crate::runtime::Runtime,
    record: &mut SessionRecord,
    choice: &mut crate::runtime::ImageChoice,
    enabled: bool,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> tokio::sync::OwnedMutexGuard<()> {
    let unavailable = if !enabled {
        Some("ide_disabled")
    } else if choice.build_error.is_some() {
        Some("project_image_repair_required")
    } else {
        None
    };
    if let Some(error) = unavailable {
        if let Some(b) = record.binding.as_mut() {
            b.ide_error = Some(error.into())
        }
        return guard;
    }
    let (runtime, base, actor, generation) = (
        rt.clone(),
        choice.used.clone(),
        record.actor.driver.clone(),
        record.generation,
    );
    let (result, guard) = tokio::task::spawn_blocking(move || {
        let result = runtime.ide_layer(&base, &actor, generation);
        (result, guard)
    })
    .await
    .expect("owned IDE build worker interrupted");
    if let Some(b) = record.binding.as_mut() {
        match result {
            Ok(ide) => {
                choice.used = ide.image.clone();
                b.ide = Some(ide);
                b.ide_error = None
            }
            Err(error) => b.ide_error = Some(error.into()),
        }
    }
    guard
}
fn private_input(bytes: &[u8]) -> Result<std::fs::File, &'static str> {
    use std::io::{Seek, Write};
    use std::os::unix::fs::OpenOptionsExt;
    let path =
        std::env::temp_dir().join(format!("sigil-ide-input-{}", crate::sessions::mint_token()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| "provider_setup_failed")?;
    std::fs::remove_file(&path).map_err(|_| "provider_setup_failed")?;
    file.write_all(bytes).map_err(|_| "provider_setup_failed")?;
    file.rewind().map_err(|_| "provider_setup_failed")?;
    Ok(file)
}
fn layer_dockerfile(digest: &str, user: &str) -> Result<String, &'static str> {
    key(digest, PROVIDER, HELPER)?;
    if !user
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_-:".contains(&b))
    {
        return Err("unsupported_project_user");
    }
    let user = if user.is_empty() { "root" } else { user };
    Ok(format!("FROM {digest}\nUSER root\nRUN test ! -e /opt/sigil-ide && test ! -e /sigil-profile\nCOPY code-server /opt/sigil-ide/code-server\nCOPY sigil-ide-agent /opt/sigil-ide/sigil-ide-agent\nCOPY activity /opt/sigil-ide/activity\nRUN mkdir -p /sigil-profile && chmod 1777 /sigil-profile\nUSER {user}\nRUN /opt/sigil-ide/code-server/bin/code-server --config /dev/null --version\n"))
}
impl crate::runtime::Runtime {
    fn ide_docker(&self, args: &[&str]) -> Result<String, &'static str> {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            return fake.docker(args).map_err(|_| "provider_runtime_failed");
        }
        let mut command = std::process::Command::new("docker");
        command.args(args);
        let out = crate::bounded_process::output_until(
            &mut command,
            std::time::Instant::now() + std::time::Duration::from_secs(15),
        )
        .map_err(|_| "provider_runtime_failed")?;
        if !out.status.success() {
            return Err("provider_runtime_failed");
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().into())
    }
}
#[cfg(test)]
mod layer_tests {
    use super::*;
    #[test]
    fn non_root_project_user_is_restored_and_project_server_is_untouched() {
        let digest = format!("sha256:{}", "a".repeat(64));
        for user in ["1000:1000", "developer", "root", ""] {
            let df = layer_dockerfile(&digest, user).unwrap();
            assert!(df.starts_with(&format!("FROM {digest}\n")));
            assert!(df.contains(&format!(
                "USER {}\nRUN /opt",
                if user.is_empty() { "root" } else { user }
            )));
            assert!(!df.contains("ENTRYPOINT"));
            assert!(!df.contains("CMD"));
            assert!(!df.contains("vm-base"));
            assert!(!df.contains("EXPOSE"));
            assert!(df.contains("chmod 1777 /sigil-profile"));
        }
        assert!(layer_dockerfile(&digest, "dev\nRUN malicious").is_err());
    }
    #[test]
    fn anonymous_private_command_input_contains_exact_bytes() {
        use std::io::Read;
        let mut input = private_input(b"secret\nbytes").unwrap();
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"secret\nbytes");
    }
}
fn safe_error(error: &str) -> &'static str {
    match error {
        "branch_or_remote_changed" => "branch_or_remote_changed",
        "git_busy" => "git_busy",
        "push_or_commit_failed" => "push_or_commit_failed",
        "user_command_running" => "user_command_running",
        "editor_ownership_unknown" => "editor_ownership_unknown",
        "editor_stop_unconfirmed" => "editor_stop_unconfirmed",
        "editor_start_failed" => "editor_start_failed",
        "profile_setup_required" => "profile_setup_required",
        "preferences_save_failed" => "preferences_save_failed",
        "activity_extension_missing" => "activity_extension_missing",
        _ => "provider_operation_failed",
    }
}
fn safe_response(
    value: &serde_json::Value,
    path: &str,
    generation: u64,
) -> Result<serde_json::Value, &'static str> {
    let state = value["state"]
        .as_str()
        .filter(|s| matches!(*s, "ready" | "stopped" | "failed" | "checkpointed"))
        .ok_or("provider_response_invalid")?;
    let sha = |v: &serde_json::Value| {
        v.as_str()
            .filter(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_string)
    };
    if path == "status" {
        if value["generation"].as_u64() != Some(generation) {
            return Err("generation_changed");
        }
        let d = &value["durability"];
        let durability = json!({"state":d["state"].as_str().filter(|s|matches!(*s,"saved_to_disk"|"dirty"|"pushed"|"paused")).unwrap_or("unknown"),"dirty":d["dirty"].as_bool(),"committed":sha(&d["committed"]),"pushed":sha(&d["pushed"]),"checkpointed":d["checkpointed"].as_bool(),"merged":false,"error":d["error"].as_str().map(safe_error)});
        Ok(
            json!({"state":state,"generation":generation,"idle_secs":value["idle_secs"].as_u64().ok_or("provider_response_invalid")?,"busy":value["busy"].as_bool().ok_or("provider_response_invalid")?,"durability":durability}),
        )
    } else {
        Ok(json!({"state":state,"generation":generation,"sha":sha(&value["sha"])}))
    }
}
#[cfg(test)]
mod safety_tests {
    use super::*;
    #[test]
    fn hostile_provider_metadata_is_allowlisted_and_generation_checked() {
        let value = json!({"state":"ready","generation":1,"idle_secs":2,"busy":false,"token":"SECRET","durability":{"state":"pushed","committed":"SECRET","error":"SECRET"}});
        let safe = safe_response(&value, "status", 1).unwrap();
        assert!(!safe.to_string().contains("SECRET"));
        assert_eq!(
            safe_response(&value, "status", 2),
            Err("generation_changed")
        );
    }
    #[test]
    fn host_merge_probe_is_read_only_and_failed_push_preserves_local_work() {
        use std::os::unix::fs::PermissionsExt;
        let repo = crate::merge::tests::mk_repo("ide-host-merge");
        let remote = repo.join("remote.git");
        crate::merge::git(&repo, &["init", "--bare", remote.to_str().unwrap()]).unwrap();
        let url = remote.to_str().unwrap();
        host_transport(&repo, url, "not-a-live-token", true).unwrap();
        assert!(
            crate::merge::git(&remote, &["show-ref", "--verify", "refs/heads/master"]).is_err()
        );
        let hook = remote.join("hooks/pre-receive");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let head = crate::merge::git(&repo, &["rev-parse", "master"]).unwrap();
        assert!(host_transport(&repo, url, "not-a-live-token", false).is_err());
        assert_eq!(
            crate::merge::git(&repo, &["rev-parse", "master"]).unwrap(),
            head
        );
        std::fs::remove_file(hook).unwrap();
        host_transport(&repo, url, "not-a-live-token", false).unwrap();
        assert_eq!(
            crate::merge::git(&remote, &["rev-parse", "master"]).unwrap(),
            head
        );
    }
}
