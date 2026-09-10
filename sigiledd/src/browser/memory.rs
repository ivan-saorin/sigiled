//! Fixed, bounded Memory adapter. The current human bearer is held only for each call.
use super::*;
use axum::extract::{Path, Query};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
mod dto;
use dto::*;
#[derive(Clone)]
pub(super) struct Service {
    client: reqwest::Client,
    base: String,
}
impl Default for Service {
    fn default() -> Self {
        let catalog: Value = serde_json::from_str(crate::catalog::TEXT).expect("catalog");
        let base = catalog["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "memory" && s["machine"]["gate"] == "stack-bearer")
            .and_then(|s| s["machine"]["base"].as_str())
            .unwrap_or("")
            .to_owned();
        Self {
            base,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(6))
                .build()
                .expect("memory client"),
        }
    }
}
fn unavailable() -> Error {
    Error(StatusCode::BAD_GATEWAY, "memory_unavailable")
}
fn invalid() -> Error {
    Error(StatusCode::BAD_REQUEST, "invalid_memory_input")
}
fn revision(s: &str) -> Result<(), Error> {
    s.parse::<i64>()
        .ok()
        .filter(|n| *n >= 0 && n.to_string() == s)
        .map(|_| ())
        .ok_or_else(invalid)
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
fn manual_id(s: &str) -> bool {
    s.strip_prefix("m_").is_some_and(uuid)
}
fn field(s: &str, max: usize, empty: bool) -> Result<(), Error> {
    if s.len() > max || s.contains('\0') || (!empty && s.trim().is_empty()) {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn content(text: &str, tags: &[String]) -> Result<(), Error> {
    field(text, 65536, false)?;
    if tags.len() > 32 {
        return Err(invalid());
    }
    for tag in tags {
        field(tag, 128, false)?;
        if tag.contains(',') {
            return Err(invalid());
        }
    }
    Ok(())
}
fn index(name: &str) -> Result<(), Error> {
    if crate::project::valid_name(name) {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn decode<T: DeserializeOwned>(v: Value) -> Result<T, Error> {
    serde_json::from_value(v).map_err(|_| unavailable())
}
// Apply last, after every public projection, including metadata and opaque cursors.
fn scrub(v: &mut Value, token: &str) {
    match v {
        Value::String(s) => {
            if !token.is_empty() {
                *s = s.replace(token, "[redacted]");
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|v| scrub(v, token)),
        Value::Object(m) => m.values_mut().for_each(|v| scrub(v, token)),
        _ => (),
    }
}
fn public<T: Serialize>(v: T, token: &str) -> Result<Json<Value>, Error> {
    let mut v = serde_json::to_value(v).map_err(|_| unavailable())?;
    scrub(&mut v, token);
    Ok(Json(v))
}
fn ready(v: &Value) -> bool {
    [
        ("curation_contract", "1"),
        ("browse", "live_keyset_v1"),
        ("manual", "revisioned_uuid_v1"),
        ("forget", "preview_suppression_v1"),
        ("projection", "durable_outbox_v1"),
        ("revision_encoding", "decimal_string"),
    ]
    .iter()
    .all(|(k, s)| v[k] == *s)
        && v["max_page"] == 100
        && v["max_scan"] == 1000
        && v["auth"]["oidc_verifier_configured"] == true
}
impl Service {
    async fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        token: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Value), Error> {
        let p: Vec<_> = path.split('/').collect();
        let allowed = match p.as_slice() {
            ["idx"] | ["capabilities"] | ["health"] => method == Method::GET,
            ["idx", n] => crate::project::valid_name(n) && method == Method::GET,
            ["idx", n, op] => {
                crate::project::valid_name(n)
                    && match *op {
                        "chunks" | "search" | "ingests" => method == Method::GET,
                        "manual" | "curation" | "forget" | "ingest" => method == Method::POST,
                        _ => false,
                    }
            }
            ["idx", n, "manual", id] => {
                crate::project::valid_name(n)
                    && manual_id(id)
                    && (method == Method::GET || method == Method::PUT)
            }
            ["idx", n, "chunks", id] => {
                crate::project::valid_name(n) && identifier(id) && method == Method::GET
            }
            ["idx", n, "manual", id, "history"] => {
                crate::project::valid_name(n) && manual_id(id) && method == Method::GET
            }
            ["idx", n, "forget", "preview"] => {
                crate::project::valid_name(n) && method == Method::POST
            }
            _ => false,
        };
        if !allowed {
            return Err(invalid());
        }
        let mut url = reqwest::Url::parse(&self.base).map_err(|_| unavailable())?;
        let local = cfg!(test) && matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
        if (url.scheme() != "https" && !local)
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
        let mut req = self.client.request(method, url).bearer_auth(token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let mut response = req.send().await.map_err(|_| unavailable())?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 | 403 => Error(StatusCode::UNAUTHORIZED, "memory_authorization_required"),
                404 => Error(StatusCode::NOT_FOUND, "memory_record_not_found"),
                409 => Error(StatusCode::CONFLICT, "memory_revision_or_preview_conflict"),
                400 | 422 => Error(StatusCode::BAD_REQUEST, "memory_request_rejected"),
                _ => unavailable(),
            });
        }
        const MAX: usize = 2 * 1024 * 1024;
        let mut bytes = Vec::new();
        if response.content_length().is_some_and(|n| n > MAX as u64) {
            return Err(unavailable());
        }
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > MAX {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((
            status,
            serde_json::from_slice(&bytes).map_err(|_| unavailable())?,
        ))
    }
    async fn supported(&self, token: &str) -> bool {
        self.call(Method::GET, "capabilities", &[], token, None)
            .await
            .is_ok_and(|(_, v)| ready(&v))
    }
    async fn require(&self, token: &str) -> Result<(), Error> {
        if self.supported(token).await {
            Ok(())
        } else {
            Err(Error(
                StatusCode::SERVICE_UNAVAILABLE,
                "memory_service_update_or_auth_setup_required",
            ))
        }
    }
}
fn service(s: &AppState) -> Result<Service, Error> {
    Ok(s.browser.inner()?.memory.clone())
}
pub(super) fn body_limit(method: &Method, path: &str) -> Option<usize> {
    let p: Vec<_> = path.trim_start_matches('/').split('/').collect();
    if !p.starts_with(&["browser", "api", "memory"]) {
        return None;
    }
    let yes = match p.as_slice() {
        [_, _, _, "indexes", n, "manual"] => {
            crate::project::valid_name(n) && method == Method::POST
        }
        [_, _, _, "indexes", n, "manual", id] => {
            crate::project::valid_name(n) && manual_id(id) && method == Method::PUT
        }
        [_, _, _, "indexes", n, "curation" | "forget" | "reindex"] => {
            crate::project::valid_name(n) && method == Method::POST
        }
        [_, _, _, "indexes", n, "forget", "preview"] => {
            crate::project::valid_name(n) && method == Method::POST
        }
        _ => false,
    };
    Some(if yes {
        if p.get(5) == Some(&"manual") {
            450000
        } else {
            100000
        }
    } else {
        0
    })
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/browser/api/memory", get(overview))
        .route("/browser/api/memory/indexes/{index}/chunks", get(browse))
        .route(
            "/browser/api/memory/indexes/{index}/chunks/{id}",
            get(chunk),
        )
        .route("/browser/api/memory/indexes/{index}/manual", post(create))
        .route(
            "/browser/api/memory/indexes/{index}/manual/{id}",
            get(manual).put(edit),
        )
        .route(
            "/browser/api/memory/indexes/{index}/manual/{id}/history",
            get(history),
        )
        .route("/browser/api/memory/indexes/{index}/curation", post(curate))
        .route(
            "/browser/api/memory/indexes/{index}/forget/preview",
            post(preview),
        )
        .route("/browser/api/memory/indexes/{index}/forget", post(forget))
        .route("/browser/api/memory/indexes/{index}/ingests", get(ingests))
        .route("/browser/api/memory/indexes/{index}/reindex", post(reindex))
}
async fn overview(c: BrowserContext, State(s): State<AppState>) -> Result<Json<Value>, Error> {
    let svc = service(&s)?;
    let supported = svc.supported(&c.access_token).await;
    let (_, v) = svc
        .call(Method::GET, "idx", &[], &c.access_token, None)
        .await?;
    let indexes: Vec<Index> = decode(v)?;
    if indexes.len() > 10000 {
        return Err(unavailable());
    }
    public(
        json!({"indexes":indexes,"readiness":if supported{"ready"}else{"update_or_auth_setup_required"},"observed_at":auth::now_epoch(),"project_association":"unavailable_pending_enrollment"}),
        &c.access_token,
    )
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Browse {
    mode: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
    q: Option<String>,
    source: Option<String>,
    #[serde(rename = "ref")]
    refid: Option<String>,
    path: Option<String>,
    tag: Option<String>,
    since: Option<String>,
    until: Option<String>,
    archived: Option<String>,
    offset: Option<usize>,
}
async fn browse(
    c: BrowserContext,
    State(s): State<AppState>,
    Path(n): Path<String>,
    Query(q): Query<Browse>,
) -> Result<Json<Value>, Error> {
    index(&n)?;
    let svc = service(&s)?;
    let limit = q.limit.unwrap_or(30);
    if !(1..=100).contains(&limit) {
        return Err(invalid());
    }
    let mut query = vec![];
    for (k, v, max) in [
        ("cursor", &q.cursor, 8192),
        ("source", &q.source, 128),
        ("ref", &q.refid, 2048),
        ("path", &q.path, 4096),
        ("tag", &q.tag, 128),
        ("since", &q.since, 128),
        ("until", &q.until, 128),
    ] {
        if let Some(v) = v {
            field(v, max, true)?;
            query.push((k, v.clone()));
        }
    }
    let archived = q.archived.as_deref().unwrap_or("exclude");
    if !matches!(archived, "exclude" | "include" | "only") {
        return Err(invalid());
    }
    if let Some(text) = q.q.as_ref().filter(|q| !q.is_empty()) {
        field(text, 2048, false)?;
        if q.cursor.is_some() || q.path.is_some() || archived != "exclude" {
            return Err(Error(
                StatusCode::BAD_REQUEST,
                "memory_search_filter_unsupported",
            ));
        }
        query.retain(|(k, _)| *k != "tag");
        if let Some(tag) = q.tag {
            query.push(("tags", tag));
        }
        query.push(("q", text.clone()));
        query.push(("k", "200".into()));
        let mode = q.mode.as_deref().unwrap_or("bm25");
        if !matches!(mode, "bm25" | "vector" | "hybrid") {
            return Err(invalid());
        }
        query.push(("mode", mode.into()));
        let (_, v) = svc
            .call(
                Method::GET,
                &format!("idx/{n}/search"),
                &query,
                &c.access_token,
                None,
            )
            .await?;
        let page: Search = decode(v)?;
        if page.hits.len() > 200 {
            return Err(unavailable());
        }
        let offset = q.offset.unwrap_or(0);
        if offset > 200 {
            return Err(invalid());
        }
        let total = page.hits.len();
        let items: Vec<_> = page.hits.into_iter().map(Chunk::from).collect();
        return public(
            json!({"items":items,"next_cursor":null,"result_limit":200,"observed_results":total,"mode":mode,"consistency":"bounded_search_candidates","limitation":"Search uses up to 200 candidates; path and archived filters require Browse.","readiness":"detail_requires_live_contract"}),
            &c.access_token,
        );
    }
    svc.require(&c.access_token).await?;
    if q.offset.is_some() {
        return Err(invalid());
    }
    query.push(("limit", limit.to_string()));
    query.push(("archived", archived.into()));
    let (_, v) = svc
        .call(
            Method::GET,
            &format!("idx/{n}/chunks"),
            &query,
            &c.access_token,
            None,
        )
        .await?;
    let page: Page = decode(v)?;
    if page.items.len() > limit
        || page.scanned > 1000
        || page.consistency != "live_keyset"
        || page.next_cursor.as_ref().is_some_and(|v| v.len() > 8192)
    {
        return Err(unavailable());
    }
    for row in &page.items {
        row.validate(&n)?;
    }
    public(page, &c.access_token)
}
async fn chunk(
    c: BrowserContext,
    State(s): State<AppState>,
    Path((n, id)): Path<(String, String)>,
) -> Result<Json<Value>, Error> {
    index(&n)?;
    if !identifier(&id) {
        return Err(invalid());
    }
    let svc = service(&s)?;
    let (_, v) = svc
        .call(
            Method::GET,
            &format!("idx/{n}/chunks/{id}"),
            &[],
            &c.access_token,
            None,
        )
        .await?;
    let v: Chunk = decode(v)?;
    if v.id != id {
        return Err(unavailable());
    }
    v.validate(&n)?;
    public(v, &c.access_token)
}
async fn manual(
    c: BrowserContext,
    State(s): State<AppState>,
    Path((n, id)): Path<(String, String)>,
) -> Result<Json<Value>, Error> {
    index(&n)?;
    let svc = service(&s)?;
    let (_, v) = svc
        .call(
            Method::GET,
            &format!("idx/{n}/manual/{id}"),
            &[],
            &c.access_token,
            None,
        )
        .await?;
    let v: Detail = decode(v)?;
    v.memory.validate()?;
    revision(&v.curation.revision)?;
    if v.memory.id != id || !matches!(&v.target,Selector::Manual{id:i}if i==&id) {
        return Err(unavailable());
    }
    public(v, &c.access_token)
}
async fn create(
    c: BrowserContext,
    State(s): State<AppState>,
    Path(n): Path<String>,
    Json(b): Json<Create>,
) -> Result<Response, Error> {
    index(&n)?;
    if !uuid(&b.create_id) {
        return Err(invalid());
    }
    content(&b.text, &b.tags)?;
    let id = format!("m_{}", b.create_id);
    let svc = service(&s)?;
    svc.require(&c.access_token).await?;
    let (status, v) = svc
        .call(
            Method::POST,
            &format!("idx/{n}/manual"),
            &[],
            &c.access_token,
            Some(json!(b)),
        )
        .await?;
    let v: Manual = decode(v)?;
    v.validate()?;
    if v.id != id {
        return Err(unavailable());
    }
    Ok((status, public(v, &c.access_token)?).into_response())
}
async fn edit(
    c: BrowserContext,
    State(s): State<AppState>,
    Path((n, id)): Path<(String, String)>,
    Json(b): Json<Edit>,
) -> Result<Response, Error> {
    index(&n)?;
    if !manual_id(&id) {
        return Err(invalid());
    }
    revision(&b.expected_revision)?;
    content(&b.text, &b.tags)?;
    let svc = service(&s)?;
    svc.require(&c.access_token).await?;
    let (status, v) = svc
        .call(
            Method::PUT,
            &format!("idx/{n}/manual/{id}"),
            &[],
            &c.access_token,
            Some(json!(b)),
        )
        .await?;
    let v: Manual = decode(v)?;
    v.validate()?;
    if v.id != id {
        return Err(unavailable());
    }
    Ok((status, public(v, &c.access_token)?).into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    after: Option<String>,
    limit: Option<usize>,
}
async fn history(
    c: BrowserContext,
    State(s): State<AppState>,
    Path((n, id)): Path<(String, String)>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Value>, Error> {
    index(&n)?;
    let after = q.after.unwrap_or_else(|| "0".into());
    revision(&after)?;
    let limit = q.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(invalid());
    }
    let (_, v) = service(&s)?
        .call(
            Method::GET,
            &format!("idx/{n}/manual/{id}/history"),
            &[("after", after), ("limit", limit.to_string())],
            &c.access_token,
            None,
        )
        .await?;
    let v: History = decode(v)?;
    if v.items.len() > limit {
        return Err(unavailable());
    }
    for m in &v.items {
        m.validate()?;
        if m.id != id {
            return Err(unavailable());
        }
    }
    if let Some(after) = &v.next_after {
        revision(after)?;
    }
    public(v, &c.access_token)
}
async fn curate(
    c: BrowserContext,
    State(s): State<AppState>,
    Path(n): Path<String>,
    Json(b): Json<Curate>,
) -> Result<Json<Value>, Error> {
    index(&n)?;
    b.target.validate(false)?;
    revision(&b.expected_revision)?;
    if b.pinned.is_none() && b.archived.is_none() && b.annotation.is_none() {
        return Err(invalid());
    }
    if let Some(a) = &b.annotation {
        field(a, 8192, false)?;
    }
    if b.context_chunk_id.as_ref().is_some_and(|s| !identifier(s)) {
        return Err(invalid());
    }
    let svc = service(&s)?;
    svc.require(&c.access_token).await?;
    let (_, v) = svc
        .call(
            Method::POST,
            &format!("idx/{n}/curation"),
            &[],
            &c.access_token,
            Some(json!(b)),
        )
        .await?;
    let v: Curated = decode(v)?;
    if v.target != b.target {
        return Err(unavailable());
    }
    revision(&v.curation.revision)?;
    public(v, &c.access_token)
}
async fn preview(
    c: BrowserContext,
    State(s): State<AppState>,
    Path(n): Path<String>,
    Json(b): Json<PreviewInput>,
) -> Result<Json<Value>, Error> {
    index(&n)?;
    b.selector.validate(true)?;
    let svc = service(&s)?;
    svc.require(&c.access_token).await?;
    let (_, v) = svc
        .call(
            Method::POST,
            &format!("idx/{n}/forget/preview"),
            &[],
            &c.access_token,
            Some(json!(b)),
        )
        .await?;
    let v: Preview = decode(v)?;
    if v.selector != b.selector {
        return Err(unavailable());
    }
    if v.sample.len() > 10 || v.confirmation.len() > 16384 {
        return Err(unavailable());
    }
    public(v, &c.access_token)
}
async fn forget(
    c: BrowserContext,
    State(s): State<AppState>,
    Path(n): Path<String>,
    Json(b): Json<Forget>,
) -> Result<Json<Value>, Error> {
    index(&n)?;
    b.selector.validate(true)?;
    field(&b.confirmation, 16384, false)?;
    let svc = service(&s)?;
    svc.require(&c.access_token).await?;
    let (_, v) = svc
        .call(
            Method::POST,
            &format!("idx/{n}/forget"),
            &[],
            &c.access_token,
            Some(json!(b)),
        )
        .await?;
    let v: Forgotten = decode(v)?;
    public(v, &c.access_token)
}
mod ingestion;
use ingestion::{ingests, reindex};
#[cfg(test)]
mod tests;
