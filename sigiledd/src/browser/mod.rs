//! Browser-only authentication boundary; machine routes never consume cookies.
mod config;
mod provider;
use crate::{
    auth::{self, Actor},
    AppState,
};
use axum::{
    extract::{FromRequestParts, RawQuery, State},
    http::{request::Parts, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
pub use config::Config;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const SESSION: &str = "__Host-sigil_session";
const LOGIN: &str = "__Host-sigil_login";
const LOGIN_TTL: u64 = 300;
const MAX_LOGINS: usize = 1024;
const MAX_SESSIONS: usize = 4096;

#[derive(Clone, Default)]
pub struct BrowserState(Option<Arc<Inner>>);
struct Inner {
    config: Config,
    provider: provider::Provider,
    store: Mutex<Store>,
}
#[derive(Default)]
struct Store {
    logins: HashMap<String, Transaction>,
    sessions: HashMap<String, Arc<Session>>,
}
#[derive(Clone)]
struct Transaction {
    binding: String,
    origin: String,
    nonce: String,
    verifier: String,
    return_to: String,
    expires: u64,
    in_flight: bool,
    replaces_session: Option<String>,
}
struct Session {
    origin: String,
    subject: String,
    nonce: String,
    csrf: String,
    absolute: u64,
    last_seen: std::sync::atomic::AtomicU64,
    credentials: tokio::sync::Mutex<Credentials>,
}
struct Credentials {
    actor: Actor,
    display_name: Option<String>,
    access: String,
    refresh: Option<String>,
    expires: u64,
}
impl BrowserState {
    pub fn from_env(auth: &auth::AuthConfig) -> Self {
        Self::configured(
            Config::from_map(&std::env::vars().collect()).expect("invalid browser configuration"),
            auth,
        )
        .expect("incompatible browser configuration")
    }
    pub fn configured(
        config: Option<Config>,
        auth: &auth::AuthConfig,
    ) -> Result<Self, &'static str> {
        let Some(config) = config else {
            return Ok(Self::default());
        };
        config.validate_auth(auth)?;
        Ok(Self(Some(Arc::new(Inner {
            config,
            provider: provider::Provider::new(),
            store: Mutex::new(Store::default()),
        }))))
    }
    fn inner(&self) -> Result<Arc<Inner>, Error> {
        self.0
            .clone()
            .ok_or(Error(StatusCode::NOT_FOUND, "browser_disabled"))
    }
}
impl Inner {
    fn prune(&self, store: &mut Store) {
        let now = auth::now_epoch();
        store.logins.retain(|_, t| t.expires > now);
        store.sessions.retain(|_, s| self.live(s, now));
    }
    fn live(&self, s: &Session, now: u64) -> bool {
        now < s.absolute
            && now
                < s.last_seen
                    .load(std::sync::atomic::Ordering::Relaxed)
                    .saturating_add(self.config.idle_seconds)
    }
    fn origin(&self, h: &HeaderMap) -> Result<String, Error> {
        let host = single(h, "host").ok_or_else(|| Error::forbidden("invalid_host"))?;
        let origin = self
            .config
            .origins
            .iter()
            .find(|o| {
                o.split_once("://")
                    .is_some_and(|(_, authority)| authority == host)
            })
            .ok_or_else(|| Error::forbidden("invalid_host"))?;
        if h.contains_key("origin") && single(h, "origin") != Some(origin) {
            return Err(Error::forbidden("invalid_origin"));
        }
        // Forwarded and identity headers are never policy inputs.
        Ok(origin.clone())
    }
    fn session(&self, h: &HeaderMap, origin: &str) -> Result<(String, Arc<Session>), Error> {
        let id = cookie(h, SESSION).ok_or_else(Error::login)?;
        let mut store = self.store.lock().unwrap();
        self.prune(&mut store);
        let s = store
            .sessions
            .get(&id)
            .filter(|s| s.origin == origin)
            .cloned()
            .ok_or_else(Error::login)?;
        Ok((id, s))
    }
    fn mutation(&self, h: &HeaderMap, s: &Session) -> Result<(), Error> {
        if single(h, "origin") != Some(&s.origin) {
            return Err(Error::forbidden("invalid_origin"));
        }
        if !single(h, "x-sigil-csrf").is_some_and(|v| auth::constant_time_eq(v, &s.csrf)) {
            return Err(Error::forbidden("invalid_csrf"));
        }
        Ok(())
    }
}
fn single<'a>(h: &'a HeaderMap, k: &str) -> Option<&'a str> {
    let mut values = h.get_all(k).iter();
    let v = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        None
    } else {
        Some(v)
    }
}
fn cookie(h: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for header in h.get_all("cookie") {
        for part in header.to_str().ok()?.split(';') {
            let Some((k, v)) = part.trim().split_once('=') else {
                continue;
            };
            if k == name {
                if found.is_some()
                    || v.len() != 43
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    return None;
                }
                found = Some(v.to_owned());
            }
        }
    }
    found
}
fn random() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "entropy_unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn set_cookie(r: &mut Response, name: &str, value: &str, ttl: u64) {
    let v = format!("{name}={value}; Path=/; Max-Age={ttl}; Secure; HttpOnly; SameSite=Lax");
    r.headers_mut().append(
        "set-cookie",
        HeaderValue::from_str(&v).expect("generated cookie"),
    );
}
fn redirect(path: &str) -> Response {
    let mut r = StatusCode::SEE_OTHER.into_response();
    r.headers_mut().insert(
        "location",
        HeaderValue::from_str(path).expect("validated redirect"),
    );
    r
}
#[derive(Clone, Copy, Debug)]
pub struct Error(StatusCode, &'static str);
impl Error {
    fn login() -> Self {
        Self(StatusCode::UNAUTHORIZED, "login_required")
    }
    fn forbidden(code: &'static str) -> Self {
        Self(StatusCode::FORBIDDEN, code)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error":self.1}))).into_response()
    }
}
fn query(raw: Option<String>) -> Result<HashMap<String, String>, Error> {
    let mut u = reqwest::Url::parse("https://query.invalid/").unwrap();
    u.set_query(raw.as_deref());
    let mut fields = HashMap::new();
    for (k, v) in u.query_pairs() {
        if fields.insert(k.into_owned(), v.into_owned()).is_some() {
            return Err(Error::login());
        }
    }
    Ok(fields)
}
async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response, Error> {
    let b = state.browser.inner()?;
    let origin = b.origin(&headers)?;
    let q = query(raw)?;
    if q.keys().any(|k| k != "return_to") {
        return Err(Error::login());
    }
    let return_to = q.get("return_to").map(String::as_str).unwrap_or("/");
    if !config::return_path(return_to) {
        return Err(Error::forbidden("invalid_return_path"));
    }
    let state_id = random()?;
    let binding = random()?;
    let nonce = random()?;
    let verifier = random()?;
    let mut url = reqwest::Url::parse(&b.config.authorization).expect("validated URL");
    url.query_pairs_mut().extend_pairs(&[
        ("response_type", "code"),
        ("client_id", &b.config.client_id),
        ("redirect_uri", &format!("{origin}/browser/callback")),
        ("scope", &b.config.scopes),
        ("state", &state_id),
        ("nonce", &nonce),
        (
            "code_challenge",
            &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
        ),
        ("code_challenge_method", "S256"),
    ]);
    let mut store = b.store.lock().unwrap();
    b.prune(&mut store);
    // GET initiation never revokes an authenticated session. A successfully verified,
    // still-current transaction may atomically replace this exact session later.
    let replaces_session = cookie(&headers, SESSION)
        .filter(|id| store.sessions.get(id).is_some_and(|s| s.origin == origin));
    if let Some(old) = cookie(&headers, LOGIN) {
        store.logins.retain(|_, t| t.binding != old);
    }
    if store.logins.len() >= MAX_LOGINS {
        return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "login_capacity"));
    }
    store.logins.insert(
        state_id,
        Transaction {
            binding: binding.clone(),
            origin,
            nonce,
            verifier,
            return_to: return_to.to_owned(),
            expires: auth::now_epoch() + LOGIN_TTL,
            in_flight: false,
            replaces_session,
        },
    );
    let mut r = redirect(url.as_str());
    set_cookie(&mut r, LOGIN, &binding, LOGIN_TTL);
    Ok(r)
}
async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response, Error> {
    let b = state.browser.inner()?;
    let origin = b.origin(&headers)?;
    let q = query(raw)?;
    let state_id = q.get("state").ok_or_else(Error::login)?.clone();
    let binding = cookie(&headers, LOGIN).ok_or_else(Error::login)?;
    let tx = {
        let mut store = b.store.lock().unwrap();
        b.prune(&mut store);
        let tx = store.logins.get_mut(&state_id).ok_or_else(Error::login)?;
        // Unbound requests cannot consume or revoke someone else's transaction.
        if tx.in_flight || tx.origin != origin || !auth::constant_time_eq(&binding, &tx.binding) {
            return Err(Error::login());
        }
        tx.in_flight = true;
        tx.clone()
    };
    // Membership remains visible to replacement login and authenticated logout while
    // the IdP request is in flight. Cancellation/failure consumes only this attempt.
    let _attempt = LoginGuard {
        b: b.clone(),
        state_id: state_id.clone(),
    };
    if q.contains_key("error") || q.get("iss").is_some_and(|s| s != &b.config.issuer) {
        return Err(Error::login());
    }
    let code = q
        .get("code")
        .filter(|s| !s.is_empty() && s.len() <= 2048)
        .ok_or_else(Error::login)?;
    let tokens = b
        .provider
        .exchange(
            &b.config,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &format!("{origin}/browser/callback")),
                ("code_verifier", &tx.verifier),
            ],
        )
        .await?;
    let verified = b
        .provider
        .verify(&b.config, &state.auth, &tokens, Some(&tx.nonce), None)
        .await?;
    let now = auth::now_epoch();
    if now >= tx.expires {
        return Err(Error::login());
    }
    let id = random()?;
    let csrf = random()?;
    let session = Arc::new(Session {
        origin,
        subject: verified.subject,
        nonce: tx.nonce,
        csrf,
        absolute: now + b.config.absolute_seconds,
        last_seen: std::sync::atomic::AtomicU64::new(now),
        credentials: tokio::sync::Mutex::new(Credentials {
            actor: verified.actor,
            display_name: verified.display_name,
            access: tokens.access_token,
            refresh: tokens.refresh_token,
            expires: verified.expires,
        }),
    });
    let mut store = b.store.lock().unwrap();
    b.prune(&mut store);
    if !store.logins.get(&state_id).is_some_and(|current| {
        current.in_flight && current.binding == tx.binding && current.origin == tx.origin
    }) {
        return Err(Error::login());
    }
    let replaces_live_session = tx
        .replaces_session
        .as_ref()
        .is_some_and(|old| store.sessions.contains_key(old));
    if store.sessions.len() >= MAX_SESSIONS && !replaces_live_session {
        return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "session_capacity"));
    }
    store.logins.remove(&state_id);
    if let Some(old) = &tx.replaces_session {
        store.sessions.remove(old);
    }
    store.sessions.insert(id.clone(), session);
    let mut r = redirect(&tx.return_to);
    // Never clear the login cookie from a callback: a newer login response may
    // already have installed its own binding. Consumed bindings expire naturally.
    set_cookie(&mut r, SESSION, &id, b.config.absolute_seconds);
    Ok(r)
}

// An owned callback attempt may remove only its random state generation.
// Drop also runs when the route timeout or its task cancels provider exchange.
struct LoginGuard {
    b: Arc<Inner>,
    state_id: String,
}
impl Drop for LoginGuard {
    fn drop(&mut self) {
        self.b.store.lock().unwrap().logins.remove(&self.state_id);
    }
}

/// Internal, non-serializable credential context for fixed service adapters.
/// Always use this extractor on /browser routes; it checks CSRF on every non-read method.
/// It must never be returned as JSON, logged, or forwarded to a caller-chosen URL.
pub(crate) struct BrowserContext {
    pub actor: Actor,
    pub issuer: String,
    pub subject: String,
    pub display_name: Option<String>,
    pub(crate) access_token: String,
    csrf: String,
    absolute: u64,
    idle_expires: u64,
}
// Cancellation during rotating refresh revokes locally, even if the provider already rotated.
// No uncertain refresh credential is retried; logout/removal cannot be undone by late completion.
struct RefreshGuard {
    b: Arc<Inner>,
    id: String,
    armed: bool,
}
impl Drop for RefreshGuard {
    fn drop(&mut self) {
        if self.armed {
            self.b.store.lock().unwrap().sessions.remove(&self.id);
        }
    }
}
impl FromRequestParts<AppState> for BrowserContext {
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Error> {
        let b = state.browser.inner()?;
        let origin = b.origin(&parts.headers)?;
        let (id, s) = b.session(&parts.headers, &origin)?;
        if parts.method != Method::GET && parts.method != Method::HEAD {
            b.mutation(&parts.headers, &s)?;
        }
        let mut c = s.credentials.lock().await;
        // Recheck after awaiting a concurrent refresh.
        {
            let store = b.store.lock().unwrap();
            if !store.sessions.contains_key(&id) || !b.live(&s, auth::now_epoch()) {
                return Err(Error::login());
            }
        }
        if c.expires <= auth::now_epoch() + 30 {
            let mut guard = RefreshGuard {
                b: b.clone(),
                id: id.clone(),
                armed: true,
            };
            let refresh = c.refresh.take().ok_or_else(Error::login)?;
            let tokens = b
                .provider
                .exchange(
                    &b.config,
                    &[("grant_type", "refresh_token"), ("refresh_token", &refresh)],
                )
                .await?;
            let verified = b
                .provider
                .verify(
                    &b.config,
                    &state.auth,
                    &tokens,
                    Some(&s.nonce),
                    Some(&s.subject),
                )
                .await?;
            // Never acquire credentials while holding store; logout can remove without waiting for IdP.
            let mut store = b.store.lock().unwrap();
            b.prune(&mut store);
            if !store.sessions.contains_key(&id) {
                return Err(Error::login());
            }
            *c = Credentials {
                actor: verified.actor,
                display_name: verified.display_name,
                access: tokens.access_token,
                refresh: tokens.refresh_token.or(Some(refresh)),
                expires: verified.expires,
            };
            guard.armed = false;
        }
        let now = auth::now_epoch();
        {
            let mut store = b.store.lock().unwrap();
            b.prune(&mut store);
            if !store.sessions.contains_key(&id) {
                return Err(Error::login());
            }
            s.last_seen.store(now, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(Self {
            actor: c.actor.clone(),
            issuer: b.config.issuer.clone(),
            subject: s.subject.clone(),
            display_name: c.display_name.clone(),
            access_token: c.access.clone(),
            csrf: s.csrf.clone(),
            absolute: s.absolute,
            idle_expires: (now + b.config.idle_seconds).min(s.absolute),
        })
    }
}
async fn inspect(c: BrowserContext) -> Json<serde_json::Value> {
    // A deliberate use of credential only as internal state: no serialized token or generic proxy.
    debug_assert!(!c.access_token.is_empty());
    Json(
        serde_json::json!({"actor":c.actor,"identity":{"issuer":c.issuer,"subject":c.subject,"principal_kind":"human","display_name":c.display_name},"csrf_token":c.csrf,"absolute_expires_at":c.absolute,"idle_expires_at":c.idle_expires,"features":{"overview":true,"workspace_actions":false,"memory_adapter":false},"capabilities":{"role":c.actor.role,"driver_approval_gates":true}}),
    )
}
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Error> {
    let b = state.browser.inner()?;
    let origin = b.origin(&headers)?;
    let (id, s) = b.session(&headers, &origin)?;
    b.mutation(&headers, &s)?;
    {
        let mut store = b.store.lock().unwrap();
        store.sessions.remove(&id);
        let binding = cookie(&headers, LOGIN);
        store.logins.retain(|_, tx| {
            tx.replaces_session.as_deref() != Some(&id)
                && !(tx.origin == origin && binding.as_ref().is_some_and(|v| v == &tx.binding))
        });
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    set_cookie(&mut r, SESSION, "", 0);
    set_cookie(&mut r, LOGIN, "", 0);
    Ok(r)
}
async fn overview(
    c: BrowserContext,
    state: State<AppState>,
    q: axum::extract::Query<crate::overview::Page>,
) -> Response {
    crate::overview::root(c.actor, state, q).await
}
async fn detail(
    c: BrowserContext,
    state: State<AppState>,
    p: axum::extract::Path<String>,
    q: axum::extract::Query<crate::overview::Page>,
) -> Response {
    crate::overview::detail(c.actor, state, p, q).await
}
async fn boundary(request: axum::extract::Request, next: Next) -> Response {
    let login_path = matches!(request.uri().path(), "/browser/login" | "/browser/callback");
    let mut r = if request.uri().to_string().len() > 4096
        || request
            .headers()
            .iter()
            .map(|(k, v)| k.as_str().len() + v.len())
            .sum::<usize>()
            > 16384
        || request.headers().contains_key("transfer-encoding")
        || request
            .headers()
            .get("content-length")
            .is_some_and(|v| v != "0")
    {
        Error(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large").into_response()
    } else {
        match tokio::time::timeout(Duration::from_secs(15), async {
            let (parts, body) = request.into_parts();
            if axum::body::to_bytes(body, 0).await.is_err() {
                return Error(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large").into_response();
            }
            next.run(axum::http::Request::from_parts(
                parts,
                axum::body::Body::empty(),
            ))
            .await
        })
        .await
        {
            Ok(r) => r,
            Err(_) => Error(StatusCode::GATEWAY_TIMEOUT, "request_timeout").into_response(),
        }
    };
    // Failed auth attempts must not delete a separate established session or a
    // newer login binding. Their owned server transaction cleanup is sufficient.
    if !login_path && r.status() == StatusCode::UNAUTHORIZED {
        set_cookie(&mut r, SESSION, "", 0);
    }
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    r.headers_mut()
        .insert("pragma", HeaderValue::from_static("no-cache"));
    r.headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    r.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    r
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/browser/login", get(login))
        .route("/browser/callback", get(callback))
        .route("/browser/session", get(inspect))
        .route("/browser/logout", post(logout))
        .route("/browser/api/overview", get(overview))
        .route("/browser/api/projects/{project}", get(detail))
        .layer(axum::extract::DefaultBodyLimit::max(0))
        .layer(middleware::from_fn(boundary))
        .with_state(state)
}
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
