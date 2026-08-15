// auth.rs — the two-legged identity of design §1, inside the dual-auth
// window of §1.7: a legacy bootstrap bearer (= admin, constant-time compared)
// OR an IdP-signed JWT (RS256, validated locally against the issuer's JWKS,
// cached; groups claim → capability). Human approval (§1.4) is a first-class
// object: POST /auth/elevate starts a device flow at the stack IdP, SIGILED
// polls and keeps the tokens in its own store — they never reach skill, PC
// or transcript. GET /auth/approvals mirrors the state.
//
// Session 3 scope: the whole auth layer + policy (capability map §1.6,
// approval required for projects new, app verbs, and any session on the
// platform projects — DEC-15). The session/app verbs that will *consume*
// authorize() arrive with session 4; the routes that exist are gated now.
// Storage is in-memory like the registry: the DB lands at cutover.
use axum::extract::FromRequestParts;
use axum::http::{request::Parts, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PLATFORM_PROJECTS: [&str; 2] = ["sigiled", "sigiled-supervisor"];

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// --- configuration ----------------------------------------------------------

#[derive(Clone, Default)]
pub struct AuthConfig {
    /// Legacy leg (§1.7): this bearer = bootstrap admin. Absent = leg off.
    pub bootstrap_bearer: Option<String>,
    /// IdP base URL, e.g. "https://auth.example.com". Absent = OIDC leg off.
    pub oidc_base: Option<String>,
    /// client_id of the device-flow provider (§1.4).
    pub device_client_id: String,
    pub admin_group: String,
    pub driver_group: String,
    /// Prefix of the per-callee group that authorizes a service-to-service
    /// call: `svc:genie` says "may call genie". A group, not an audience,
    /// because groups are what this IdP already emits (the `sigiled-groups`
    /// scope mapping) and what `Claims` already carries — see
    /// `docs/plans/2026-08-15-composed-service-auth.md`.
    pub service_group_prefix: String,
}

impl AuthConfig {
    pub fn from_env() -> Self {
        AuthConfig {
            bootstrap_bearer: std::env::var("SIGILED_BOOTSTRAP_BEARER").ok(),
            oidc_base: std::env::var("SIGILED_OIDC_BASE").ok(),
            device_client_id: std::env::var("SIGILED_DEVICE_CLIENT_ID")
                .unwrap_or_else(|_| "sigiled-device".into()),
            admin_group: std::env::var("SIGILED_ADMIN_GROUP")
                .unwrap_or_else(|_| "stack:admins".into()),
            driver_group: std::env::var("SIGILED_DRIVER_GROUP")
                .unwrap_or_else(|_| "stack:drivers".into()),
            service_group_prefix: std::env::var("SIGILED_SERVICE_GROUP_PREFIX")
                .unwrap_or_else(|_| "svc:".into()),
        }
    }

    /// The group that authorizes calling `service`.
    pub fn service_group(&self, service: &str) -> String {
        format!("{}{}", self.service_group_prefix, service)
    }
    /// No leg configured = auth off (alpha dev run): everything passes as a
    /// dev admin, loudly. A real deploy always configures at least one leg.
    pub fn enabled(&self) -> bool {
        self.bootstrap_bearer.is_some() || self.oidc_base.is_some()
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// --- actor ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    Driver,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    pub driver: String,
    pub role: Role,
    /// "human (device, expires <epoch>)" — mirrors §1.6; None = no live one.
    pub approval: Option<String>,
}

// --- capability policy (§1.6 + DEC-15) --------------------------------------

// Open/Close are consumed by sessions.rs; the other variants await their
// verbs (jobs, apps, recycle — cutover territory). Exercised by tests.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    OpenSession,
    CloseSession,
    Recycle,
    Git,
    Exec,
    JobRun,
    JobRecap,
    ProjectsNew,
    AppVerb,
    /// GET /skill/{driver} — renders a skill that can carry a credential.
    SkillRender,
}

/// The rows of the capability map where `stack:drivers` needs a live human
/// approval. Everything else is open to both groups.
pub fn requires_approval(action: Action, project: Option<&str>) -> bool {
    match action {
        Action::ProjectsNew | Action::AppVerb | Action::SkillRender => true,
        Action::OpenSession => project.is_some_and(|p| PLATFORM_PROJECTS.contains(&p)),
        _ => false,
    }
}

#[derive(Debug, PartialEq)]
pub struct Denial(pub String);

pub fn authorize(
    actor: &Actor,
    action: Action,
    project: Option<&str>,
    approvals: &ApprovalStore,
    now: u64,
) -> Result<(), Denial> {
    if actor.role == Role::Admin {
        return Ok(());
    }
    if requires_approval(action, project) && approvals.live(&actor.driver, now).is_none() {
        return Err(Denial(format!(
            "capability requires approval: driver {} has no live approval — POST /sigiled/auth/elevate",
            actor.driver
        )));
    }
    Ok(())
}

// --- JWT leg ----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub azp: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    pub iss: String,
    #[allow(dead_code)]
    pub exp: usize,
}

impl Claims {
    pub fn driver(&self) -> String {
        self.preferred_username
            .clone()
            .or_else(|| self.azp.clone())
            .unwrap_or_else(|| self.sub.clone())
    }
}

/// kid → decoding key. Populated from the issuer's JWKS on miss (prod) or
/// preloaded (tests). One store for all issuers under the configured base:
/// kids are random enough, and a hostile issuer never gets fetched because
/// the iss prefix is checked first.
#[derive(Default, Clone)]
pub struct KeyStore(Arc<RwLock<HashMap<String, DecodingKey>>>);

impl KeyStore {
    pub fn preload(&self, kid: &str, key: DecodingKey) {
        self.0.write().unwrap().insert(kid.to_string(), key);
    }
    fn get(&self, kid: &str) -> Option<DecodingKey> {
        self.0.read().unwrap().get(kid).cloned()
    }

    /// Fetch {iss}jwks/ and cache every RSA key found. Authentik serves the
    /// per-application JWKS exactly there (§1.2).
    async fn refresh_from(&self, http: &reqwest::Client, iss: &str) -> Result<(), String> {
        let url = format!("{}jwks/", ensure_slash(iss));
        let jwks: serde_json::Value = http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("jwks fetch: {e}"))?
            .json()
            .await
            .map_err(|e| format!("jwks parse: {e}"))?;
        for k in jwks["keys"].as_array().cloned().unwrap_or_default() {
            if let (Some(kid), Some(n), Some(e)) =
                (k["kid"].as_str(), k["n"].as_str(), k["e"].as_str())
            {
                if let Ok(key) = DecodingKey::from_rsa_components(n, e) {
                    self.preload(kid, key);
                }
            }
        }
        Ok(())
    }
}

fn ensure_slash(s: &str) -> String {
    if s.ends_with('/') {
        s.to_string()
    } else {
        format!("{s}/")
    }
}

/// Validate an IdP JWT: iss must live under the configured base, signature
/// RS256 against the (cached) JWKS, exp enforced. Returns the raw claims —
/// the actor mapping happens above, where the group→role table lives.
pub async fn validate_jwt(
    token: &str,
    cfg: &AuthConfig,
    keys: &KeyStore,
    http: Option<&reqwest::Client>,
) -> Result<Claims, String> {
    let base = cfg.oidc_base.as_deref().ok_or("oidc leg not configured")?;
    let header = decode_header(token).map_err(|e| format!("jwt header: {e}"))?;
    let kid = header.kid.ok_or("jwt without kid")?;

    // iss check before any fetch: never talk to an issuer we don't trust.
    let unverified = {
        let mut v = Validation::new(Algorithm::RS256);
        v.insecure_disable_signature_validation();
        v.validate_exp = false;
        v.validate_aud = false;
        decode::<Claims>(token, &DecodingKey::from_secret(b"x"), &v)
            .map_err(|e| format!("jwt claims: {e}"))?
            .claims
    };
    if !unverified.iss.starts_with(&ensure_slash(base)) {
        return Err(format!(
            "issuer outside the configured IdP: {}",
            unverified.iss
        ));
    }

    if keys.get(&kid).is_none() {
        let http = http.ok_or("unknown kid and no http client to refresh JWKS")?;
        keys.refresh_from(http, &unverified.iss).await?;
    }
    let key = keys.get(&kid).ok_or("kid not found in issuer JWKS")?;

    let mut validation = Validation::new(Algorithm::RS256);
    // aud is the per-driver client_id (§1.3): sigiledd accepts every provider
    // under the trusted issuer base, so audience is deliberately not pinned.
    validation.validate_aud = false;
    let data = decode::<Claims>(token, &key, &validation).map_err(|e| format!("jwt: {e}"))?;
    Ok(data.claims)
}

pub fn actor_from_claims(
    claims: &Claims,
    cfg: &AuthConfig,
    approvals: &ApprovalStore,
) -> Result<Actor, String> {
    let role = if claims.groups.iter().any(|g| g == &cfg.admin_group) {
        Role::Admin
    } else if claims.groups.iter().any(|g| g == &cfg.driver_group) {
        Role::Driver
    } else {
        return Err(format!(
            "token carries neither {} nor {}",
            cfg.admin_group, cfg.driver_group
        ));
    };
    let driver = claims.driver();
    let approval = approvals
        .live(&driver, now_epoch())
        .map(|a| format!("{} (device, expires {})", a.human, a.expires_epoch));
    Ok(Actor {
        driver,
        role,
        approval,
    })
}

// --- approvals (§1.4) -------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Granted,
    Denied,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub driver: String,
    pub human: String,
    pub expires_epoch: u64,
    pub state: ApprovalState,
    /// Custody (§1.4): the device-flow tokens live here — and, since the
    /// store exists, in the 0600 state file: SIGILED is their keeper. The
    /// API surface never carries them (ApprovalView is metadata only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<serde_json::Value>,
}

#[derive(Default, Clone)]
pub struct ApprovalStore(Arc<RwLock<HashMap<String, Approval>>>);

impl ApprovalStore {
    pub fn put(&self, a: Approval) {
        self.0.write().unwrap().insert(a.driver.clone(), a);
    }
    pub fn live(&self, driver: &str, now: u64) -> Option<Approval> {
        self.0
            .read()
            .unwrap()
            .get(driver)
            .filter(|a| a.state == ApprovalState::Granted && a.expires_epoch > now)
            .cloned()
    }
    pub fn snapshot(&self) -> Vec<Approval> {
        self.0.read().unwrap().values().cloned().collect()
    }
    pub fn grant(&self, driver: &str, human: &str, expires_epoch: u64, tokens: serde_json::Value) {
        self.put(Approval {
            driver: driver.into(),
            human: human.into(),
            expires_epoch,
            state: ApprovalState::Granted,
            tokens: Some(tokens),
        });
    }
    pub fn mark(&self, driver: &str, state: ApprovalState) {
        if let Some(a) = self.0.write().unwrap().get_mut(driver) {
            a.state = state;
        }
    }
    pub fn dump(&self) -> HashMap<String, Approval> {
        self.0.read().unwrap().clone()
    }
    pub fn hydrate(&self, map: HashMap<String, Approval>) {
        *self.0.write().unwrap() = map;
    }
}

// --- axum wiring ------------------------------------------------------------

#[derive(Clone)]
pub struct AuthState {
    pub config: Arc<AuthConfig>,
    pub keys: KeyStore,
    pub approvals: ApprovalStore,
    pub http: reqwest::Client,
}

impl Default for AuthState {
    fn default() -> Self {
        AuthState {
            config: Arc::new(AuthConfig::from_env()),
            keys: KeyStore::default(),
            approvals: ApprovalStore::default(),
            http: reqwest::Client::new(),
        }
    }
}

pub struct AuthError(StatusCode, String);

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "detail": self.1 }))).into_response()
    }
}

impl FromRequestParts<crate::AppState> for Actor {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = &state.auth;
        if !auth.config.enabled() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                tracing::warn!("auth disabled: no leg configured — dev run only");
            });
            return Ok(Actor {
                driver: "dev".into(),
                role: Role::Admin,
                approval: None,
            });
        }
        let header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| AuthError(StatusCode::UNAUTHORIZED, "missing bearer".into()))?;

        if let Some(bootstrap) = &auth.config.bootstrap_bearer {
            if constant_time_eq(header, bootstrap) {
                return Ok(Actor {
                    driver: "bootstrap".into(),
                    role: Role::Admin,
                    approval: None,
                });
            }
        }
        let claims = validate_jwt(header, &auth.config, &auth.keys, Some(&auth.http))
            .await
            .map_err(|e| AuthError(StatusCode::UNAUTHORIZED, e))?;
        actor_from_claims(&claims, &auth.config, &auth.approvals)
            .map_err(|e| AuthError(StatusCode::FORBIDDEN, e))
    }
}

// --- service-to-service verification (the edge's forward_auth) --------------

/// The header the edge sets to name the callee it is guarding. It is set by
/// the Caddyfile per vhost, never copied from the client — a caller that
/// sends its own `X-Sigiled-Service` is overwritten at the edge, which is
/// why this endpoint can trust it and nothing else about the request.
pub const SERVICE_HEADER: &str = "x-sigiled-service";
/// Copied back onto the upstream request on a 200 (`copy_headers`), so the
/// service behind the gate learns who called without parsing a JWT itself.
pub const CALLER_HEADER: &str = "x-sigiled-caller";

/// Why a policy decision came out the way it did — the reason travels to the
/// caller in the body, because a composed service debugging a 403 at 3am
/// should not have to guess which of six groups it is missing.
#[derive(Serialize)]
pub struct VerifyBody {
    caller: String,
    service: String,
    granted_by: &'static str,
}

/// May `claims` call `service`? The whole policy, in one place and pure so
/// it can be tested without a signing key or an HTTP stack.
///
/// One rule, deliberately: the caller carries `svc:<service>`, or it is an
/// admin. Drivers get **no** blanket access — `stack:drivers` says *may
/// drive SIGILED*, never *may call genie*. Keeping the driver group out of
/// this function is the entire separation between the identity layer and
/// the policy layer; the day it grows an `|| driver_group` arm, `[compose]`
/// stops meaning anything for sessions.
pub fn authorize_service_call(
    claims: &Claims,
    service: &str,
    cfg: &AuthConfig,
) -> Result<&'static str, Denial> {
    if claims.groups.iter().any(|g| g == &cfg.admin_group) {
        return Ok("admin");
    }
    let wanted = cfg.service_group(service);
    if claims.groups.contains(&wanted) {
        return Ok("compose");
    }
    Err(Denial(format!("caller {} lacks {wanted}", claims.driver())))
}

/// `GET|POST /sigiled/auth/verify` — the question the edge asks about every
/// machine-leg request: *may this caller call this service?*
///
/// Shaped for Caddy's `forward_auth`: **2xx = allow, anything else = deny**,
/// and the useful identity leaves through a response header the edge copies
/// onto the upstream request. Deliberately NOT behind the `Actor` extractor:
/// a composed service is not a driver, holds no `stack:drivers` membership,
/// and would be rejected by `actor_from_claims` before policy ever ran.
///
/// It is an unauthenticated endpoint in the sense that anyone may ask — but
/// it answers 200 only to a validly signed, unexpired token from the trusted
/// issuer carrying the right group. It therefore reveals nothing that making
/// the real request would not, which is the bar a forward-auth oracle has to
/// clear.
///
/// The policy itself is `authorize_service_call`; this handler is transport.
pub async fn verify(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let auth = &state.auth;
    let service = match headers.get(SERVICE_HEADER).and_then(|v| v.to_str().ok()) {
        Some(s) if !s.is_empty() => s.to_string(),
        // A misconfigured edge must fail closed and say so: this is an
        // operator error, not a caller error, hence 500 rather than 401.
        _ => {
            return AuthError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("edge did not set {SERVICE_HEADER}"),
            )
            .into_response()
        }
    };

    let token = match headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        Some(t) => t,
        None => {
            return AuthError(StatusCode::UNAUTHORIZED, "missing bearer".into()).into_response()
        }
    };

    let claims = match validate_jwt(token, &auth.config, &auth.keys, Some(&auth.http)).await {
        Ok(c) => c,
        Err(e) => return AuthError(StatusCode::UNAUTHORIZED, e).into_response(),
    };

    let granted_by = match authorize_service_call(&claims, &service, &auth.config) {
        Ok(g) => g,
        Err(Denial(d)) => return AuthError(StatusCode::FORBIDDEN, d).into_response(),
    };

    let caller = claims.driver();
    let mut res = Json(VerifyBody {
        caller: caller.clone(),
        service,
        granted_by,
    })
    .into_response();
    if let Ok(v) = axum::http::HeaderValue::from_str(&caller) {
        res.headers_mut().insert(CALLER_HEADER, v);
    }
    res
}

// --- device flow endpoints (§1.4) -------------------------------------------

#[derive(Serialize)]
pub struct ElevateResponse {
    pub verification_uri: String,
    pub user_code: String,
    pub expires: u64,
}

pub async fn elevate(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    actor: Actor,
) -> Result<Json<ElevateResponse>, AuthError> {
    let auth = state.auth.clone();
    let base = auth.config.oidc_base.clone().ok_or_else(|| {
        AuthError(
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc leg not configured".into(),
        )
    })?;
    let device_url = format!("{}application/o/device/", ensure_slash(&base));
    let resp: serde_json::Value = auth
        .http
        .post(&device_url)
        .form(&[("client_id", auth.config.device_client_id.as_str())])
        .send()
        .await
        .map_err(|e| AuthError(StatusCode::BAD_GATEWAY, format!("idp device endpoint: {e}")))?
        .json()
        .await
        .map_err(|e| AuthError(StatusCode::BAD_GATEWAY, format!("idp device response: {e}")))?;

    let device_code = resp["device_code"].as_str().unwrap_or_default().to_string();
    let user_code = resp["user_code"].as_str().unwrap_or_default().to_string();
    let verification_uri = resp["verification_uri"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if device_code.is_empty() || user_code.is_empty() {
        return Err(AuthError(
            StatusCode::BAD_GATEWAY,
            format!("idp device response missing codes: {resp}"),
        ));
    }
    let interval = resp["interval"].as_u64().unwrap_or(5);
    let expires = now_epoch() + resp["expires_in"].as_u64().unwrap_or(300);

    state.auth.approvals.put(Approval {
        driver: actor.driver.clone(),
        human: String::new(),
        expires_epoch: expires,
        state: ApprovalState::Pending,
        tokens: None,
    });
    state.persist();

    // Poll the token endpoint in the background until approved or expired
    // (§1.4 step 5). The tokens stay in the store; the driver only ever sees
    // the approval metadata. The task holds the whole AppState so every
    // outcome hits the disk too.
    let driver = actor.driver.clone();
    let app = state.clone();
    tokio::spawn(async move {
        let token_url = format!("{}application/o/token/", ensure_slash(&base));
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            if now_epoch() >= expires {
                app.auth.approvals.mark(&driver, ApprovalState::Expired);
                app.persist();
                return;
            }
            let Ok(r) = app
                .auth
                .http
                .post(&token_url)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", &device_code),
                    ("client_id", &app.auth.config.device_client_id),
                ])
                .send()
                .await
            else {
                continue;
            };
            let Ok(body) = r.json::<serde_json::Value>().await else {
                continue;
            };
            match body["error"].as_str() {
                Some("authorization_pending") | Some("slow_down") => continue,
                Some(_) => {
                    app.auth.approvals.mark(&driver, ApprovalState::Denied);
                    app.persist();
                    return;
                }
                None => {
                    // Approved. The access token came straight from the IdP
                    // over TLS: decoding without re-verifying the signature
                    // is safe here and only used to name the human.
                    let human = body["access_token"]
                        .as_str()
                        .and_then(|t| {
                            let mut v = Validation::new(Algorithm::RS256);
                            v.insecure_disable_signature_validation();
                            v.validate_exp = false;
                            v.validate_aud = false;
                            decode::<Claims>(t, &DecodingKey::from_secret(b"x"), &v).ok()
                        })
                        .map(|d| d.claims.driver())
                        .unwrap_or_else(|| "operator".into());
                    let ttl = body["expires_in"].as_u64().unwrap_or(43200);
                    app.auth
                        .approvals
                        .grant(&driver, &human, now_epoch() + ttl, body);
                    app.persist();
                    return;
                }
            }
        }
    });

    Ok(Json(ElevateResponse {
        verification_uri,
        user_code,
        expires,
    }))
}

#[derive(Serialize)]
pub struct ApprovalView {
    pub driver: String,
    pub human: String,
    pub expires: u64,
    pub state: ApprovalState,
}

pub async fn approvals(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    _actor: Actor,
) -> Json<Vec<ApprovalView>> {
    Json(
        state
            .auth
            .approvals
            .snapshot()
            .into_iter()
            .map(|a| ApprovalView {
                driver: a.driver,
                human: a.human,
                expires: a.expires_epoch,
                state: a.state,
            })
            .collect(),
    )
}

// --- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};

    const TEST_RSA_PRIVATE: &str = "-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQDWpMw1Mzy5hWFW
PWfZnF6hCB3pm/nHbFJZ820Qf3a9eSWaRJXJiuj1r5txBcvBkbc8xkzXCimUBhq1
zE0+xF+y0THmcY4aGHjRuJoDUdGMoayPkRVGgo4X5S8j7Jer2C41XXMuzSYrHVgv
5+5bczmTg8N33FbLVCmFUtTswALBw2dCCIK61IVjW6LV3QqembS6yPt4aPynvkSk
WrfAyirjdLSG6FcmpxcGSgeTJVPk16N9ab/tGBEQ+CYs+/5WIXVjW+xtEeb/Uj4l
WY//YTwM8kLG9C77oAg5x41SrS+p3xEtHngbds2lE6UVqVmCJsavvMnRBXdYyHxP
B9xMx4phAgMBAAECggEAFwIkGjVPdncV3aXYMJ9WlPYu56xmuGH0C2ygxa6WKr4W
YeylbkznxzNPayyDAJFHRjpfAQOXvKRxX0jCxH5OIFfUnKZSRFmYdOmwB7hQ6T1o
5xxXHp9uziCs/pG1WfBC1slZSBxpUaCUBBGdbzvhIX8TuFDcFHBlgYNFLAoymXSv
poJrPrgsEWd/JeYDezfYhh6KF4CKt1zKgrIZ4sfAng57VDH0Bt9gLjR5r+n+6xxL
+WoERppAPJGqyFShbVxz8LZOExhe9DWOelIQUO2hMnzBtMDv3dAJBvjtbICJrFtz
/BUEIYt8wDv4yVXz1O0u789vyMa/xwulxHhUuV/d2QKBgQDsNhAaAJksFNvhfkWp
IeOdYa5pSyfYE1+UF8dblvdIJOngfFhU/KasIh0B3o5YxEQFwWCE5IsIN7jT62nN
CBmtnQnL+XjMQCaDHZvlyDpwmvlyuPEZzTaxGe2IZ2usCUu1T3r4pLtVE7VIldle
HyOpa044dmNl3AxJPvVbQrzWyQKBgQDooC9/gJvJodF1MbSVT5qlqxDJwZ9HgJY/
fhdJKfW+sqQMpApKYvrolMsbDfeo1fviJOTxRfF8BpsUZfN6zSKYIInz1WlIkWRT
P0sJMZUo4kLHt/PIS88aIbK2cSwNqLl/7n4PoTxqG0P368goy9QbOOmD2X2mA1l+
P8rReXSq2QKBgB8IU0E3RuhVrTWIw1ofC6pHhQRsTUXD9dCc9yH/SWl/AALwEyLH
NpZyvODb/lOHJXCkISwUYnen6m5dBT9cixMWCI11rvsWini7URn1Hkhg89iwl2xO
W5sUzvIWtDyb1Ahz8rHr4nig6DYrCa2l5aeCY3pjg1eEe1C8JrvgnrKRAoGAWJfA
3x8ctZKiEa7nZkHV1KgskZnizjljfzTHK38GbyTbo1DJ9oBxrCgWnewY2Lz926dP
Za/Mgv6FCyS0sJz1QtiJkUpCeXedrLKbIho3A0YARs2A01RDwGD7Dc5WB7GtS9KJ
QeyW9JYDsaSjKx5NXjyzehpXZuU5rQIgfNxzmSkCgYAwJzAzQEg8FbOg459Hh6rn
pnuHt9a8RF8htS1xMi00e2AiCC7vnXVpvxONlQmrDp+M5wWUE/XLaiSVjCZ54+uy
9XaMUof1SbKARdvaZ3PTBC1IyPD4hs2XIHjTFIgmctvTaaygDiBf/QQj/235lOXy
+8oUanXXTlY3JRInuS+JgA==
-----END PRIVATE KEY-----";

    const TEST_RSA_PUBLIC: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA1qTMNTM8uYVhVj1n2Zxe
oQgd6Zv5x2xSWfNtEH92vXklmkSVyYro9a+bcQXLwZG3PMZM1woplAYatcxNPsRf
stEx5nGOGhh40biaA1HRjKGsj5EVRoKOF+UvI+yXq9guNV1zLs0mKx1YL+fuW3M5
k4PDd9xWy1QphVLU7MACwcNnQgiCutSFY1ui1d0Knpm0usj7eGj8p75EpFq3wMoq
43S0huhXJqcXBkoHkyVT5NejfWm/7RgREPgmLPv+ViF1Y1vsbRHm/1I+JVmP/2E8
DPJCxvQu+6AIOceNUq0vqd8RLR54G3bNpROlFalZgibGr7zJ0QV3WMh8TwfcTMeK
YQIDAQAB
-----END PUBLIC KEY-----";

    fn cfg() -> AuthConfig {
        AuthConfig {
            bootstrap_bearer: Some("legacy-bearer".into()),
            oidc_base: Some("https://idp.test".into()),
            device_client_id: "sigiled-device".into(),
            admin_group: "stack:admins".into(),
            driver_group: "stack:drivers".into(),
            service_group_prefix: "svc:".into(),
        }
    }

    fn sign(claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-kid".into());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn preloaded_keys() -> KeyStore {
        let ks = KeyStore::default();
        ks.preload(
            "test-kid",
            DecodingKey::from_rsa_pem(TEST_RSA_PUBLIC.as_bytes()).unwrap(),
        );
        ks
    }

    fn driver_actor(name: &str) -> Actor {
        Actor {
            driver: name.into(),
            role: Role::Driver,
            approval: None,
        }
    }

    fn claims_with(name: &str, groups: &[&str]) -> Claims {
        serde_json::from_value(serde_json::json!({
            "sub": "x", "preferred_username": name,
            "groups": groups,
            "iss": "https://idp.test/application/o/x/",
            "exp": now_epoch() + 600
        }))
        .unwrap()
    }

    #[test]
    fn compose_group_grants_exactly_its_own_service() {
        let c = claims_with("sde", &["svc:genie", "svc:paper"]);
        assert_eq!(authorize_service_call(&c, "genie", &cfg()), Ok("compose"));
        assert_eq!(authorize_service_call(&c, "paper", &cfg()), Ok("compose"));
        // Declared two, granted two — the third is not implied by the others.
        assert!(authorize_service_call(&c, "folio", &cfg()).is_err());
    }

    #[test]
    fn driver_membership_is_not_a_licence_to_call_services() {
        // The load-bearing test of the whole design: `stack:drivers` means
        // "may drive SIGILED", never "may call genie". If this ever passes,
        // [compose] has stopped constraining sessions and the least-privilege
        // story is gone — see docs/plans/2026-08-15-composed-service-auth.md.
        let c = claims_with("sigiled-claude", &["stack:drivers"]);
        let denial = authorize_service_call(&c, "genie", &cfg()).unwrap_err();
        assert!(
            denial.0.contains("svc:genie"),
            "denial should name the missing group: {}",
            denial.0
        );
        assert!(
            denial.0.contains("sigiled-claude"),
            "denial should name the caller: {}",
            denial.0
        );
    }

    #[test]
    fn admins_pass_every_gate() {
        let c = claims_with("ivan", &["stack:admins"]);
        assert_eq!(authorize_service_call(&c, "anything", &cfg()), Ok("admin"));
    }

    #[test]
    fn a_token_with_no_groups_is_denied_not_defaulted() {
        let c = claims_with("nobody", &[]);
        assert!(authorize_service_call(&c, "genie", &cfg()).is_err());
    }

    #[test]
    fn the_group_prefix_is_configurable_and_not_a_substring_match() {
        let mut c2 = cfg();
        c2.service_group_prefix = "stack:svc-".into();
        let c = claims_with("sde", &["stack:svc-genie"]);
        assert_eq!(authorize_service_call(&c, "genie", &c2), Ok("compose"));
        // "genie" must not be satisfied by a group for "genie-preview": the
        // comparison is whole-string, and a prefix-match bug here would
        // silently widen every grant on the stack.
        let sneaky = claims_with("sde", &["stack:svc-genie-preview"]);
        assert!(authorize_service_call(&sneaky, "genie", &c2).is_err());
    }

    #[tokio::test]
    async fn valid_driver_jwt_yields_driver_actor() {
        let claims = serde_json::json!({
            "sub": "x", "preferred_username": "sigiled-claude",
            "groups": ["stack:drivers"],
            "iss": "https://idp.test/application/o/sigiled-claude/",
            "exp": now_epoch() + 600
        });
        let c = validate_jwt(&sign(&claims), &cfg(), &preloaded_keys(), None)
            .await
            .unwrap();
        let actor = actor_from_claims(&c, &cfg(), &ApprovalStore::default()).unwrap();
        assert_eq!(actor.driver, "sigiled-claude");
        assert_eq!(actor.role, Role::Driver);
        assert!(actor.approval.is_none());
    }

    #[tokio::test]
    async fn expired_jwt_is_rejected() {
        let claims = serde_json::json!({
            "sub": "x", "groups": ["stack:drivers"],
            "iss": "https://idp.test/application/o/x/", "exp": now_epoch() - 600
        });
        assert!(
            validate_jwt(&sign(&claims), &cfg(), &preloaded_keys(), None)
                .await
                .unwrap_err()
                .contains("jwt")
        );
    }

    #[tokio::test]
    async fn foreign_issuer_is_rejected_before_any_fetch() {
        let claims = serde_json::json!({
            "sub": "x", "groups": ["stack:drivers"],
            "iss": "https://evil.test/application/o/x/", "exp": now_epoch() + 600
        });
        let err = validate_jwt(&sign(&claims), &cfg(), &preloaded_keys(), None)
            .await
            .unwrap_err();
        assert!(err.contains("issuer outside"), "{err}");
    }

    #[tokio::test]
    async fn groupless_token_gets_no_actor() {
        let claims = serde_json::json!({
            "sub": "x", "groups": ["something:else"],
            "iss": "https://idp.test/application/o/x/", "exp": now_epoch() + 600
        });
        let c = validate_jwt(&sign(&claims), &cfg(), &preloaded_keys(), None)
            .await
            .unwrap();
        assert!(actor_from_claims(&c, &cfg(), &ApprovalStore::default()).is_err());
    }

    #[test]
    fn admin_group_maps_to_admin_role() {
        let c = Claims {
            sub: "ivan".into(),
            preferred_username: Some("ivan".into()),
            azp: None,
            groups: vec!["stack:admins".into()],
            iss: "https://idp.test/application/o/x/".into(),
            exp: 0,
        };
        let actor = actor_from_claims(&c, &cfg(), &ApprovalStore::default()).unwrap();
        assert_eq!(actor.role, Role::Admin);
    }

    #[test]
    fn bootstrap_bearer_compares_constant_time() {
        assert!(constant_time_eq("legacy-bearer", "legacy-bearer"));
        assert!(!constant_time_eq("legacy-bearer", "legacy-beareR"));
        assert!(!constant_time_eq("short", "legacy-bearer"));
    }

    // --- the acceptance matrix of build plan §4, as policy ------------------

    #[test]
    fn driver_open_on_normal_project_needs_no_approval() {
        let ok = authorize(
            &driver_actor("sigiled-claude"),
            Action::OpenSession,
            Some("smoke"),
            &ApprovalStore::default(),
            1000,
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn driver_open_on_platform_project_denied_without_approval() {
        for p in PLATFORM_PROJECTS {
            let err = authorize(
                &driver_actor("sigiled-claude"),
                Action::OpenSession,
                Some(p),
                &ApprovalStore::default(),
                1000,
            )
            .unwrap_err();
            assert!(err.0.contains("requires approval"), "{p}: {}", err.0);
        }
    }

    #[test]
    fn driver_open_on_platform_project_passes_with_live_approval() {
        let store = ApprovalStore::default();
        store.grant("sigiled-claude", "ivan", 2000, serde_json::json!({}));
        assert!(authorize(
            &driver_actor("sigiled-claude"),
            Action::OpenSession,
            Some("sigiled"),
            &store,
            1000
        )
        .is_ok());
    }

    #[test]
    fn expired_approval_does_not_authorize() {
        let store = ApprovalStore::default();
        store.grant("sigiled-claude", "ivan", 500, serde_json::json!({}));
        assert!(authorize(
            &driver_actor("sigiled-claude"),
            Action::OpenSession,
            Some("sigiled"),
            &store,
            1000
        )
        .is_err());
    }

    #[test]
    fn projects_new_and_app_verbs_gate_drivers_not_admins() {
        let store = ApprovalStore::default();
        let admin = Actor {
            driver: "bootstrap".into(),
            role: Role::Admin,
            approval: None,
        };
        for action in [Action::ProjectsNew, Action::AppVerb] {
            assert!(authorize(&admin, action, None, &store, 1000).is_ok());
            assert!(authorize(&driver_actor("d"), action, None, &store, 1000).is_err());
        }
    }

    #[test]
    fn everyday_verbs_are_open_to_drivers() {
        for action in [
            Action::CloseSession,
            Action::Recycle,
            Action::Git,
            Action::Exec,
            Action::JobRun,
            Action::JobRecap,
        ] {
            assert!(authorize(
                &driver_actor("d"),
                action,
                Some("sigiled"),
                &ApprovalStore::default(),
                1000
            )
            .is_ok());
        }
    }

    #[test]
    fn approval_api_view_never_carries_tokens() {
        // The store DOES keep the tokens (custody §1.4, persisted 0600 by
        // store.rs); what must never carry them is the API surface.
        let store = ApprovalStore::default();
        store.grant(
            "d",
            "ivan",
            2000,
            serde_json::json!({"access_token": "SECRET"}),
        );
        let views: Vec<ApprovalView> = store
            .snapshot()
            .into_iter()
            .map(|a| ApprovalView {
                driver: a.driver,
                human: a.human,
                expires: a.expires_epoch,
                state: a.state,
            })
            .collect();
        let json = serde_json::to_string(&views).unwrap();
        assert!(!json.contains("SECRET"));
        // ...while the custody itself survives in the store.
        assert!(store.snapshot()[0].tokens.is_some());
    }
}
