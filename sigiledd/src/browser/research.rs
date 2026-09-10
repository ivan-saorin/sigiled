//! Fixed typed research/service operations. Browser credentials never enter storage.
use super::*;
use axum::extract::{Path, Query};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf};
mod handoff;
mod operations;
pub(super) use handoff::apply as apply_handoff;
pub(super) use operations::CREATE_BODY_LIMIT;
pub(super) use operations::{create, mutate, operations, recover};
#[derive(Clone, Copy)]
enum Engine {
    Sde,
    Adhd,
    Genie,
}
impl Engine {
    fn name(self) -> &'static str {
        match self {
            Self::Sde => "sde",
            Self::Adhd => "adhd",
            Self::Genie => "genie",
        }
    }
}
#[derive(Clone)]
pub(super) struct Services {
    client: reqwest::Client,
    bases: BTreeMap<String, String>,
    store: operations::Store,
}
impl Default for Services {
    fn default() -> Self {
        let seed: Value = serde_json::from_str(crate::catalog::TEXT).expect("validated catalog");
        let bases = seed["services"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| {
                matches!(s["name"].as_str(), Some("sde" | "adhd" | "genie"))
                    && s["machine"]["gate"] == "stack-bearer"
            })
            .map(|s| {
                (
                    s["name"].as_str().unwrap().into(),
                    s["machine"]["base"].as_str().unwrap().into(),
                )
            })
            .collect();
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(6))
                .build()
                .expect("research HTTP client"),
            bases,
            store: operations::Store::new(
                std::env::var("SIGILED_STATE_DIR").ok().map(PathBuf::from),
            ),
        }
    }
}
fn unavailable() -> Error {
    Error(StatusCode::BAD_GATEWAY, "service_unavailable")
}
#[cfg(test)]
impl Services {
    pub(super) fn fixture(base: &str, dir: std::path::PathBuf) -> Self {
        let mut s = Self::default();
        s.bases.insert("sde".into(), base.into());
        s.store = operations::Store::new(Some(dir));
        s
    }
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn checked_id(s: &str) -> Result<(), Error> {
    if identifier(s) {
        Ok(())
    } else {
        Err(Error(StatusCode::BAD_REQUEST, "invalid_run_id"))
    }
}
fn revision(s: &str) -> Result<u64, Error> {
    s.parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == s)
        .ok_or(Error(StatusCode::BAD_REQUEST, "invalid_revision"))
}
fn project_revision(v: &mut Value) {
    if let Some(n) = v.get("revision").and_then(Value::as_u64) {
        v["revision"] = json!(n.to_string());
    }
}
fn ready(v: &Value) -> bool {
    v["api"]["run_contract"] == "sde-runs-v1"
        && v["api"]["operation_key"] == "uuid_v4"
        && v["api"]["association"] == "unverified"
        && [
            "pagination",
            "durable_transitions",
            "expected_revision",
            "catalog_attempt_snapshot",
        ]
        .iter()
        .all(|k| v["api"][k] == true)
        && v["persistence"]["mode"] == "durable"
        && v["persistence"]["degraded"] == false
        && v["persistence"]["recovery"] == "none"
}
impl Services {
    async fn call(
        &self,
        e: Engine,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        token: &str,
        body: Option<&Value>,
    ) -> Result<Value, Error> {
        // Only internal typed methods can select an engine/path. No browser URL or headers are proxied.
        let allowed = match e {
            Engine::Sde => {
                path == "healthz" || path == "runs" || {
                    let p: Vec<_> = path.split('/').collect();
                    p.first() == Some(&"runs")
                        && p.get(1).is_some_and(|id| identifier(id))
                        && (p.len() == 2
                            || (p.len() == 3 && matches!(p[2], "resume" | "handoff"))
                            || (p.len() == 4
                                && p[2] == "stages"
                                && matches!(p[3], "categorize" | "pick" | "converge")))
                }
            }
            Engine::Adhd => {
                method == Method::GET && path.strip_prefix("runs/").is_some_and(identifier)
            }
            Engine::Genie => method == Method::GET && matches!(path, "info" | "status" | "usage"),
        };
        if !allowed {
            return Err(Error(
                StatusCode::BAD_REQUEST,
                "unsupported_service_operation",
            ));
        }
        let base = self.bases.get(e.name()).ok_or_else(unavailable)?;
        let mut url = reqwest::Url::parse(base).map_err(|_| unavailable())?;
        let loopback = cfg!(test) && matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
        if (url.scheme() != "https" && !loopback)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(unavailable());
        }
        url.set_path(&format!("{}/{}", url.path().trim_end_matches('/'), path));
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        let mut req = self.client.request(method.clone(), url).bearer_auth(token);
        if let Some(body) = body {
            req = req.json(body);
        }
        let mut response = req.send().await.map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                401 | 403 => Error(StatusCode::UNAUTHORIZED, "service_authorization_required"),
                409 => Error(StatusCode::CONFLICT, "service_state_conflict"),
                404 => Error(StatusCode::NOT_FOUND, "service_record_not_found"),
                _ => unavailable(),
            });
        }
        // SDE stores at most 16 MiB of pretty-serialized RunRecord plus private metadata.
        // A bare compact detail is no larger. Other engines and handoff retain their own cap.
        let max = if matches!(e, Engine::Sde)
            && method == Method::GET
            && path.strip_prefix("runs/").is_some_and(identifier)
        {
            16 * 1024 * 1024
        } else {
            2 * 1024 * 1024
        };
        if response.content_length().is_some_and(|n| n > max as u64) {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > max {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| Error(StatusCode::BAD_GATEWAY, "invalid_service_response"))
    }
    async fn readiness(&self, token: &str) -> Result<(), Error> {
        // Every deliberate mutation probes the running binary; source/catalog metadata is insufficient.
        let v = self
            .call(Engine::Sde, Method::GET, "healthz", &[], token, None)
            .await?;
        if ready(&v) {
            Ok(())
        } else {
            Err(Error(
                StatusCode::SERVICE_UNAVAILABLE,
                "research_service_update_or_storage_repair_required",
            ))
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListQuery {
    pub project: Option<String>,
    pub cursor: Option<String>,
}
#[derive(Deserialize, Serialize)]
struct Summary {
    run_id: String,
    problem: String,
    status: String,
    created_at: String,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    revision: u64,
}
#[derive(Deserialize)]
struct Runs {
    runs: Vec<Summary>,
    #[serde(default)]
    next_cursor: Option<String>,
}
fn association(s: &Services, actor: &str, id: &str) -> Value {
    match s.store.association(actor, id) {
        Ok(Some(p)) => json!({"state":"associated","project":p}),
        Ok(None) => json!({"state":"unassociated"}),
        Err(_) => json!({"state":"unavailable"}),
    }
}
const SDE_PAGE_LIMIT: usize = 5;
pub(super) async fn list(
    c: BrowserContext,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, Error> {
    let s = state.browser.inner()?.research.clone();
    let mut query = vec![("limit", SDE_PAGE_LIMIT.to_string())];
    if let Some(cursor) = q.cursor {
        if cursor.len() > 2048 {
            return Err(Error(StatusCode::BAD_REQUEST, "invalid_cursor"));
        }
        query.push(("cursor", cursor));
    }
    // Upstream project references are unverified. Filter only by durable local association.
    if q.project
        .as_ref()
        .is_some_and(|p| !crate::project::valid_name(p))
    {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_project"));
    }
    let (runs, health) = tokio::join!(
        s.call(
            Engine::Sde,
            Method::GET,
            "runs",
            &query,
            &c.access_token,
            None
        ),
        s.call(
            Engine::Sde,
            Method::GET,
            "healthz",
            &[],
            &c.access_token,
            None
        )
    );
    let readiness = health
        .as_ref()
        .map(|v| {
            if ready(v) {
                "ready"
            } else {
                "update_or_storage_repair_required"
            }
        })
        .unwrap_or("unavailable");
    let mut result = json!({"observed_at":auth::now_epoch(),"state":"unavailable","readiness":readiness,"runs":[],"next_cursor":null});
    match runs {
        Ok(v) => {
            let runs: Runs = serde_json::from_value(v).map_err(|_| unavailable())?;
            if runs.runs.len() > 100 {
                return Err(unavailable());
            }
            let rows: Vec<_> = runs
                .runs
                .into_iter()
                .filter_map(|r| {
                    let a = association(&s, &c.actor.driver, &r.run_id);
                    if q.project
                        .as_ref()
                        .is_some_and(|p| a["project"].as_str() != Some(p))
                    {
                        return None;
                    }
                    let mut v = serde_json::to_value(r).unwrap();
                    project_revision(&mut v);
                    v["association"] = a;
                    Some(v)
                })
                .collect();
            result["state"] = json!("observed");
            result["runs"] = json!(rows);
            result["next_cursor"] = json!(runs.next_cursor);
        }
        Err(e) if e.0 == StatusCode::UNAUTHORIZED => return Err(e),
        Err(_) => {}
    }
    scrub(&mut result, &c.access_token);
    Ok(Json(result))
}
#[derive(Deserialize, Serialize)]
struct Stage {
    stage: String,
    status: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    computed_by: Option<String>,
    #[serde(default)]
    catalog_attempt: Option<u32>,
}
#[derive(Deserialize, Serialize)]
struct Run {
    run_id: String,
    problem: String,
    status: String,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    detail: Option<String>,
    stages: Vec<Stage>,
    #[serde(default)]
    awaiting: Value,
    #[serde(default)]
    artifacts: Value,
    #[serde(default)]
    catalog_attempts: Vec<Value>,
    #[serde(default)]
    options: Value,
}
// Remove reserved private fields at every nested boundary, even if a mismatched service sends them.
fn scrub(v: &mut Value, token: &str) {
    match v {
        Value::Object(m) => {
            m.retain(|k, _| {
                !matches!(
                    k.as_str(),
                    "operation_key"
                        | "fingerprint"
                        | "access_token"
                        | "refresh_token"
                        | "authorization"
                        | "token"
                )
            });
            for v in m.values_mut() {
                scrub(v, token);
            }
        }
        Value::Array(a) => {
            for v in a {
                scrub(v, token);
            }
        }
        Value::String(s) if !token.is_empty() && s.contains(token) => {
            *s = s.replace(token, "[redacted]");
        }
        _ => {}
    }
}
pub(super) async fn detail(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    checked_id(&id)?;
    let s = state.browser.inner()?.research.clone();
    let raw = s
        .call(
            Engine::Sde,
            Method::GET,
            &format!("runs/{id}"),
            &[],
            &c.access_token,
            None,
        )
        .await?;
    let run: Run = serde_json::from_value(raw).map_err(|_| unavailable())?;
    if run.run_id != id {
        return Err(unavailable());
    }
    let mut v = serde_json::to_value(run).unwrap();
    project_revision(&mut v);
    scrub(&mut v, &c.access_token);
    v["association"] = association(&s, &c.actor.driver, &id);
    v["project_present"] = json!(v["association"]["project"]
        .as_str()
        .is_some_and(|p| state.registry.contains(p)));
    v["observed_at"] = json!(auth::now_epoch());
    v["handoff_available"] = json!(v["status"] == "done" && !v["artifacts"]["outcome"].is_null());
    Ok(Json(v))
}
pub(super) async fn adhd(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    checked_id(&id)?;
    let s = state.browser.inner()?.research.clone();
    let run = s
        .call(
            Engine::Sde,
            Method::GET,
            &format!("runs/{id}"),
            &[],
            &c.access_token,
            None,
        )
        .await?;
    let linked = run["artifacts"]["adhd_run_id"]
        .as_str()
        .filter(|id| identifier(id))
        .ok_or(Error(StatusCode::NOT_FOUND, "no_recorded_adhd_run"))?;
    let mut value = s
        .call(
            Engine::Adhd,
            Method::GET,
            &format!("runs/{linked}"),
            &[],
            &c.access_token,
            None,
        )
        .await?;
    scrub(&mut value, &c.access_token);
    Ok(Json(
        json!({"observed_at":auth::now_epoch(),"adhd_run_id":linked,"record":value}),
    ))
}
pub(super) async fn handoff_preview(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    checked_id(&id)?;
    let s = state.browser.inner()?.research.clone();
    let project = s.store.association(&c.actor.driver, &id)?.ok_or(Error(
        StatusCode::CONFLICT,
        "research_project_association_required",
    ))?;
    for r in state.sessions.live_records() {
        if r.actor.driver == c.actor.driver && r.project == project {
            if let Some(m) = &r.handoff {
                if m["run_id"] == id {
                    return Ok(Json(
                        json!({"project":project,"run_id":id,"bundle":{"digest":m["digest"],"files":m["request"]["files"],"slug":m["request"]["slug"]},"recovery":{"session_id":r.session_id,"generation":r.generation.to_string()},"state":m["phase"]}),
                    ));
                }
            }
        }
    }
    let mut bundle = s
        .call(
            Engine::Sde,
            Method::GET,
            &format!("runs/{id}/handoff"),
            &[],
            &c.access_token,
            None,
        )
        .await?;
    scrub(&mut bundle, &c.access_token);
    let bundle = handoff::preview(bundle, &id)?;
    Ok(Json(
        json!({"project":project,"run_id":id,"bundle":bundle,"state":"preview","accepted_master":false,"indexed":false}),
    ))
}
#[derive(Deserialize, Serialize)]
struct Slot {
    provider: String,
    size: String,
    kind: String,
    model: String,
    downloaded: bool,
    loaded: bool,
    #[serde(default)]
    download_progress: Value,
    #[serde(default)]
    download_error: Option<String>,
}
#[derive(Deserialize, Serialize)]
struct Slots {
    slots: Vec<Slot>,
    #[serde(default)]
    lock: Value,
}
#[derive(Deserialize, Serialize)]
struct UsageRow {
    ts_ms: u64,
    request_id: String,
    provider: String,
    size: String,
    model: String,
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    latency_ms: u64,
    status: u16,
}
#[derive(Deserialize, Serialize)]
struct Usage {
    entries: Vec<UsageRow>,
}
pub(super) async fn models(
    c: BrowserContext,
    State(state): State<AppState>,
) -> Result<Json<Value>, Error> {
    let s = state.browser.inner()?.research.clone();
    let usage_query = [("limit", "50".into())];
    let (info, status, usage) = tokio::join!(
        s.call(
            Engine::Genie,
            Method::GET,
            "info",
            &[],
            &c.access_token,
            None
        ),
        s.call(
            Engine::Genie,
            Method::GET,
            "status",
            &[],
            &c.access_token,
            None
        ),
        s.call(
            Engine::Genie,
            Method::GET,
            "usage",
            &usage_query,
            &c.access_token,
            None
        )
    );
    let mut v = json!({"observed_at":auth::now_epoch(),"window":"recent_observations","project_attribution":false,"complete_billing":false});
    for (name, r) in [("info", info), ("status", status), ("usage", usage)] {
        let parsed = r.and_then(|r| match name {
            "status" => serde_json::from_value::<Slots>(r)
                .and_then(serde_json::to_value)
                .map_err(|_| unavailable()),
            "usage" => serde_json::from_value::<Usage>(r)
                .and_then(serde_json::to_value)
                .map_err(|_| unavailable()),
            _ => Ok(r),
        });
        match parsed {
            Ok(mut data) => {
                scrub(&mut data, &c.access_token);
                v[name] = json!({"state":"observed","data":data});
            }
            Err(e) => {
                v[name] = json!({"state":if e.0==StatusCode::UNAUTHORIZED{"authorization_required"}else{"unavailable"}})
            }
        }
    }
    Ok(Json(v))
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod fix1_tests;
