use super::*;
use crate::browser_tests::{sign, TEST_RSA_PUBLIC};
use axum::extract::Form;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct Fake {
    origin: String,
    nonce: Arc<Mutex<String>>,
    challenge: Arc<Mutex<String>>,
    options: Arc<Mutex<Value>>,
    refreshes: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
async fn jwks() -> Json<Value> {
    use base64::engine::general_purpose::STANDARD;
    let der = STANDARD
        .decode(
            TEST_RSA_PUBLIC
                .lines()
                .filter(|s| !s.starts_with("---"))
                .collect::<String>(),
        )
        .unwrap();
    assert_eq!(der.len(), 294);
    Json(
        json!({"keys":[{"kty":"RSA","kid":"test-kid","alg":"RS256","use":"sig","n":URL_SAFE_NO_PAD.encode(&der[33..289]),"e":"AQAB"}]}),
    )
}
async fn other_jwks() -> Json<Value> {
    let Json(mut value) = jwks().await;
    let mut modulus = URL_SAFE_NO_PAD
        .decode(value["keys"][0]["n"].as_str().unwrap())
        .unwrap();
    modulus[100] ^= 2;
    value["keys"][0]["n"] = json!(URL_SAFE_NO_PAD.encode(modulus));
    Json(value)
}
async fn token(State(fake): State<Fake>, Form(form): Form<HashMap<String, String>>) -> Response {
    let options = fake.options.lock().unwrap().clone();
    let login_nonce = fake.nonce.lock().unwrap().clone();
    let refresh = form.get("grant_type").is_some_and(|s| s == "refresh_token");
    if refresh {
        fake.refreshes.fetch_add(1, Ordering::SeqCst);
        if options["block"] == true {
            fake.started.notify_one();
            fake.release.notified().await;
        }
        if form.get("refresh_token").map(String::as_str) != Some("synthetic-refresh") {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    } else {
        let verifier = form.get("code_verifier").unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier)),
            *fake.challenge.lock().unwrap()
        );
        assert_eq!(form["code"], "synthetic-code");
        assert_eq!(form["redirect_uri"], "https://sigil.test/browser/callback");
        if options["block_code"] == true {
            fake.started.notify_one();
            fake.release.notified().await;
        }
    }
    if options["reject"] == true {
        return (StatusCode::BAD_REQUEST, "synthetic-secret-provider-error").into_response();
    }
    if options["redirect"] == true {
        return redirect(&format!("{}/unexpected", fake.origin));
    }
    if options["oversize"] == true {
        return "x".repeat(70000).into_response();
    }
    let mut access = json!({"iss":format!("{}/issuer/",fake.origin),"sub":"human-one","azp":"shared-browser-client","preferred_username":"alice","groups":["stack:drivers"],"exp":auth::now_epoch()+600});
    if let Some(patch) = options["access"].as_object() {
        access.as_object_mut().unwrap().extend(patch.clone());
    }
    let access_token = sign(&access);
    let mut id = json!({"iss":format!("{}/issuer/",fake.origin),"sub":access["sub"],"aud":"shared-browser-client","azp":"shared-browser-client","exp":auth::now_epoch()+600,"iat":auth::now_epoch(),"nonce":login_nonce,"at_hash":URL_SAFE_NO_PAD.encode(&Sha256::digest(access_token.as_bytes())[..16])});
    if refresh {
        id.as_object_mut().unwrap().remove("nonce");
    }
    if let Some(patch) = options["id"].as_object() {
        id.as_object_mut().unwrap().extend(patch.clone());
    }
    let id_token = if options["bad_signature"] == true {
        let mut s = sign(&id);
        s.push('x');
        s
    } else {
        sign(&id)
    };
    let mut body = json!({"access_token":access_token,"id_token":id_token,"token_type":"Bearer"});
    if options["no_refresh"] != true {
        body["refresh_token"] = json!("synthetic-refresh");
    }
    if options["omit_id"] == true {
        body.as_object_mut().unwrap().remove("id_token");
    }
    Json(body).into_response()
}
pub(super) struct Fixture {
    pub(super) state: AppState,
    fake: Fake,
    pub(super) base: String,
    pub(super) client: reqwest::Client,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}
fn config_map(idp: &str) -> HashMap<String, String> {
    [
        ("ENABLED", "true".to_owned()),
        ("ORIGINS", "https://sigil.test,https://memory.test".into()),
        ("ISSUER", format!("{idp}/issuer/")),
        ("AUTHORIZATION_URL", format!("{idp}/authorize")),
        ("TOKEN_URL", format!("{idp}/token")),
        ("JWKS_URL", format!("{idp}/issuer/jwks/")),
        ("CLIENT_ID", "shared-browser-client".into()),
        ("SCOPES", "openid sigiled-groups".into()),
        ("ALLOW_LOOPBACK_HTTP", "true".into()),
    ]
    .into_iter()
    .map(|(k, v)| (format!("SIGILED_BROWSER_{k}"), v))
    .collect()
}
impl Fixture {
    async fn new() -> Self {
        Self::with_github(None).await
    }
    async fn with_github(github: Option<crate::github::GitHub>) -> Self {
        Self::with_options(github, false).await
    }
    pub(super) async fn gateway() -> Self {
        Self::with_options(None, true).await
    }
    async fn with_options(github: Option<crate::github::GitHub>, gateway: bool) -> Self {
        Self::with_readiness(github, gateway, gateway).await
    }
    async fn with_readiness(
        github: Option<crate::github::GitHub>,
        gateway: bool,
        durable: bool,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let fake = Fake {
            origin: origin.clone(),
            nonce: Arc::default(),
            challenge: Arc::default(),
            options: Arc::new(Mutex::new(json!({}))),
            refreshes: Arc::default(),
            started: Arc::default(),
            release: Arc::default(),
        };
        let router = Router::new()
            .route("/jwks", get(jwks))
            .route("/issuer/jwks/", get(jwks))
            .route("/other/jwks/", get(other_jwks))
            .route("/token", post(token))
            .with_state(fake.clone());
        let idp = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut state = AppState::test_without_runtime();
        state.github = github;
        state.auth.config = Arc::new(auth::AuthConfig {
            oidc_base: Some(origin),
            admin_group: "stack:admins".into(),
            driver_group: "stack:drivers".into(),
            bootstrap_bearer: Some("synthetic-bootstrap".into()),
            ..auth::AuthConfig::default()
        });
        let mut browser_config = config_map(&fake.origin);
        if gateway {
            browser_config.insert(
                "SIGILED_BROWSER_IDE_DOMAIN".into(),
                "ide.example.test".into(),
            );
            browser_config.insert(
                "SIGILED_BROWSER_PREVIEW_DOMAIN".into(),
                "preview.example.test".into(),
            );
            browser_config.insert(
                "SIGILED_BROWSER_PREVIEW_DNS_TLS_READY".into(),
                "true".into(),
            );
            if durable {
                state.store = crate::store::Store::at_dir(
                    &state.sessions.repos_dir.as_ref().unwrap().join("state"),
                );
            }
            state.registry.insert(crate::project::ProjectRecord::new(
                "demo",
                &crate::manifest::Manifest::parse("").unwrap(),
                None,
            ));
            for k in [
                "IDE_DNS_TLS_READY",
                "IDE_OIDC_READY",
                "IDE_PROVIDER_READY",
                "IDE_POLICY_READY",
            ] {
                browser_config.insert(format!("SIGILED_BROWSER_{k}"), "true".into());
            }
        }
        state.browser = BrowserState::configured(
            Config::from_map(&browser_config).unwrap(),
            &state.auth.config,
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = crate::app(state.clone());
        let app = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            state,
            fake,
            base,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            tasks: vec![idp, app],
        }
    }
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("host", "sigil.test")
    }
    async fn start(&self) -> (String, String) {
        self.start_cookie("").await
    }
    async fn start_cookie(&self, cookie: &str) -> (String, String) {
        let r = self
            .request(
                reqwest::Method::GET,
                "/browser/login?return_to=/projects/demo",
            )
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let binding = response_cookie(&r, LOGIN);
        let location = reqwest::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
        let q: HashMap<_, _> = location.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["scope"], "openid sigiled-groups");
        *self.fake.nonce.lock().unwrap() = q["nonce"].clone();
        *self.fake.challenge.lock().unwrap() = q["code_challenge"].clone();
        (q["state"].clone(), binding)
    }
    async fn callback(&self, state: &str, cookie: &str) -> reqwest::Response {
        self.request(
            reqwest::Method::GET,
            &format!("/browser/callback?code=synthetic-code&state={state}"),
        )
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
    }
    pub(super) async fn signed_in(&self) -> (String, Value) {
        let (state, binding) = self.start().await;
        let response = self.callback(&state, &binding).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/projects/demo");
        let cookie = response_cookie(&response, SESSION);
        let response = self
            .request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = response.json().await.unwrap();
        (cookie, value)
    }
    fn expire_access(&self, cookie: &str) {
        let b = self.state.browser.inner().unwrap();
        let store = b.store.lock().unwrap();
        let s = store
            .sessions
            .get(cookie.split_once('=').unwrap().1)
            .unwrap();
        s.credentials.try_lock().unwrap().expires = 0;
    }
    fn parts(&self, cookie: &str) -> Parts {
        axum::http::Request::builder()
            .uri("/browser/session")
            .header("host", "sigil.test")
            .header("cookie", cookie)
            .body(())
            .unwrap()
            .into_parts()
            .0
    }
}
fn response_cookie(r: &reqwest::Response, name: &str) -> String {
    let text = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|s| s.starts_with(&format!("{name}=")))
        .unwrap();
    for flag in ["Path=/", "Secure", "HttpOnly", "SameSite=Lax"] {
        assert!(text.contains(flag));
    }
    assert!(!text.contains("Domain="));
    text.split(';').next().unwrap().to_owned()
}
async fn assert_safe_error(r: reqwest::Response, status: StatusCode) {
    assert_eq!(r.status(), status);
    assert_eq!(r.headers()["cache-control"], "no-store");
    let text = r.text().await.unwrap();
    for secret in [
        "synthetic-code",
        "synthetic-refresh",
        "synthetic-secret",
        "access_token",
        "id_token",
    ] {
        assert!(!text.contains(secret), "{text}");
    }
}

#[test]
fn browser_config_fails_closed_without_environment_mutation() {
    assert!(Config::from_map(&HashMap::new()).unwrap().is_none());
    let base = config_map("https://idp.test");
    assert!(Config::from_map(&base).unwrap().is_some());
    for (key, bad) in [
        ("ENABLED", "no"),
        ("ORIGINS", "https://sigil.test/"),
        ("ORIGINS", "https://sigil.test,https://sigil.test"),
        ("ORIGINS", "https://sigil.test@evil.test"),
        ("ORIGINS", "http://sigil.test"),
        ("ISSUER", "https://idp.test/issuer/?evil"),
        ("TOKEN_URL", "https://evil.test/token"),
        ("TOKEN_URL", "https://user:secret@idp.test/token"),
        ("JWKS_URL", "https://idp.test/jwks#x"),
        ("SCOPES", "openid"),
        ("SCOPES", "openid sigiled-groups\n"),
        ("IDLE_SECONDS", "0"),
        ("ABSOLUTE_SECONDS", "999999"),
        ("CLIENT_ID", ""),
        ("UNKNOWN", "yes"),
    ] {
        let mut map = base.clone();
        map.insert(format!("SIGILED_BROWSER_{key}"), bad.into());
        assert!(Config::from_map(&map).is_err(), "{key}: {bad}");
    }
    let mut partial = base.clone();
    partial.remove("SIGILED_BROWSER_ENABLED");
    assert!(Config::from_map(&partial).is_err());
    assert!(BrowserState::configured(
        Config::from_map(&base).unwrap(),
        &auth::AuthConfig::default()
    )
    .is_err());
    for bad in [
        "//evil.test",
        "/\\evil.test",
        "/%2f%2fevil.test",
        "/%252f%252fevil.test",
        "/x%0d%0aLocation:evil",
        "https://evil.test",
        "/@evil?x=1",
        "/../evil",
        "/./evil",
        "/browser/logout",
    ] {
        assert!(!config::return_path(bad), "{bad}");
    }
}
#[tokio::test]
async fn browser_valid_flow_custody_safe_reads_and_machine_regression() {
    let f = Fixture::new().await;
    let (cookie, value) = f.signed_in().await;
    assert_eq!(value["identity"]["subject"], "human-one");
    assert_eq!(value["identity"]["principal_kind"], "human");
    assert!(value["actor"]["driver"]
        .as_str()
        .unwrap()
        .starts_with("human:"));
    assert_ne!(value["actor"]["driver"], "shared-browser-client");
    let text = value.to_string();
    for secret in [
        "access_token",
        "refresh_token",
        "synthetic-refresh",
        "synthetic-code",
    ] {
        assert!(!text.contains(secret));
    }
    assert_ne!(
        provider::principal("https://idp/", "a"),
        provider::principal("https://idp/", "b")
    );
    for path in ["/browser/api/overview", "/browser/api/projects/missing"] {
        let r = f
            .request(reqwest::Method::GET, path)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert!(r.status() == StatusCode::OK || r.status() == StatusCode::NOT_FOUND);
    }
    assert!(f.state.sessions.runtime.is_none());
    assert!(f.state.sessions.dump_records().is_empty());
    for path in ["/sessions", "/skill/human", "/overview"] {
        assert_eq!(
            f.request(reqwest::Method::GET, path)
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let r = f
        .request(reqwest::Method::GET, "/sessions")
        .bearer_auth("synthetic-bootstrap")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.json::<Value>().await.unwrap()["sessions"].is_array());
    let actor = Actor {
        driver: value["actor"]["driver"].as_str().unwrap().into(),
        role: auth::Role::Driver,
        approval: None,
    };
    assert!(auth::authorize(
        &actor,
        auth::Action::OpenSession,
        Some("sigiled"),
        &f.state.auth.approvals,
        auth::now_epoch()
    )
    .is_err());
}
#[tokio::test]
async fn browser_callback_state_cookie_expiry_and_replay() {
    let f = Fixture::new().await;
    assert_safe_error(f.callback("missing", "").await, StatusCode::UNAUTHORIZED).await;
    let (state, binding) = f.start().await;
    assert_safe_error(f.callback(&state, "").await, StatusCode::UNAUTHORIZED).await;
    assert_eq!(
        f.callback(&state, &binding).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_safe_error(f.callback(&state, &binding).await, StatusCode::UNAUTHORIZED).await;
    let (state, binding) = f.start().await;
    f.state
        .browser
        .inner()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .logins
        .get_mut(&state)
        .unwrap()
        .expires = 0;
    assert_safe_error(f.callback(&state, &binding).await, StatusCode::UNAUTHORIZED).await;
    let (state, binding) = f.start().await;
    assert_eq!(
        f.callback(&state, &binding).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_safe_error(f.callback(&state, &binding).await, StatusCode::UNAUTHORIZED).await;
    let (state, binding) = f.start().await;
    let r=f.request(reqwest::Method::GET,&format!("/browser/callback?state={state}&error=access_denied&error_description=synthetic-secret")).header("cookie",binding).send().await.unwrap();
    assert_safe_error(r, StatusCode::UNAUTHORIZED).await;
    assert!(f
        .state
        .browser
        .inner()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .logins
        .is_empty());
}
#[tokio::test]
async fn browser_rejects_oidc_validation_failures_and_groupless_human() {
    let f = Fixture::new().await;
    let bads = vec![
        json!({"id":{"nonce":"bad"}}),
        json!({"id":{"aud":"wrong"}}),
        json!({"id":{"aud":["shared-browser-client","other"],"azp":null}}),
        json!({"id":{"azp":"wrong"}}),
        json!({"id":{"iss":"https://wrong/"}}),
        json!({"id":{"exp":1}}),
        json!({"id":{"sub":"another-human"}}),
        json!({"id":{"at_hash":"bad"}}),
        json!({"bad_signature":true}),
        json!({"omit_id":true}),
        json!({"access":{"iss":"https://wrong/"}}),
        json!({"access":{"exp":1}}),
        json!({"reject":true}),
        json!({"redirect":true}),
        json!({"oversize":true}),
    ];
    for bad in bads {
        *f.fake.options.lock().unwrap() = bad.clone();
        let (state, binding) = f.start().await;
        let r = f.callback(&state, &binding).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{bad}");
        assert_safe_error(r, StatusCode::UNAUTHORIZED).await;
        assert!(f
            .state
            .browser
            .inner()
            .unwrap()
            .store
            .lock()
            .unwrap()
            .sessions
            .is_empty());
    }
    *f.fake.options.lock().unwrap() = json!({"access":{"groups":[]}});
    let (state, binding) = f.start().await;
    assert_safe_error(f.callback(&state, &binding).await, StatusCode::FORBIDDEN).await;
}
#[tokio::test]
async fn browser_host_origin_csrf_logout_rotation_and_expiry() {
    let f = Fixture::new().await;
    let (cookie, value) = f.signed_in().await;
    for (origin, csrf) in [
        (None, None),
        (Some("https://sigil.test"), None),
        (
            Some("https://evil.test"),
            Some(value["csrf_token"].as_str().unwrap()),
        ),
        (Some("https://sigil.test"), Some("wrong")),
    ] {
        let mut r = f
            .request(reqwest::Method::POST, "/browser/logout")
            .header("cookie", &cookie);
        if let Some(o) = origin {
            r = r.header("origin", o);
        }
        if let Some(c) = csrf {
            r = r.header("x-sigil-csrf", c);
        }
        assert_safe_error(r.send().await.unwrap(), StatusCode::FORBIDDEN).await;
    }
    for host in ["evil.test", "sigil.test:443", "SIGIL.TEST"] {
        let r = f
            .request(reqwest::Method::GET, "/browser/session")
            .header("host", host)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_safe_error(r, StatusCode::FORBIDDEN).await;
    }
    assert_safe_error(
        f.client
            .get(format!("{}/browser/session", f.base))
            .header("host", "memory.test")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let r = f
        .request(reqwest::Method::POST, "/browser/logout")
        .header("cookie", &cookie)
        .header("origin", "https://sigil.test")
        .header("x-sigil-csrf", value["csrf_token"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    assert_safe_error(
        f.request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let (new_cookie, _) = f.signed_in().await;
    assert_ne!(new_cookie, cookie);
    {
        let b = f.state.browser.inner().unwrap();
        let store = b.store.lock().unwrap();
        let s = store
            .sessions
            .get(new_cookie.split_once('=').unwrap().1)
            .unwrap();
        s.last_seen.store(0, Ordering::Relaxed);
    }
    assert_safe_error(
        f.request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &new_cookie)
            .send()
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
}
#[tokio::test]
async fn browser_refresh_serializes_and_revalidates_identity_role_and_expiry() {
    let f = Fixture::new().await;
    let (cookie, _) = f.signed_in().await;
    f.expire_access(&cookie);
    let mut a = f.parts(&cookie);
    let mut b = f.parts(&cookie);
    let (a, b) = tokio::join!(
        BrowserContext::from_request_parts(&mut a, &f.state),
        BrowserContext::from_request_parts(&mut b, &f.state)
    );
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(f.fake.refreshes.load(Ordering::SeqCst), 1);
    for options in [
        json!({"reject":true}),
        json!({"access":{"sub":"other"}}),
        json!({"access":{"iss":"https://evil/"}}),
        json!({"access":{"groups":[]}}),
        json!({"access":{"exp":1}}),
    ] {
        *f.fake.options.lock().unwrap() = json!({});
        let (cookie, _) = f.signed_in().await;
        f.expire_access(&cookie);
        *f.fake.options.lock().unwrap() = options;
        let mut p = f.parts(&cookie);
        assert!(BrowserContext::from_request_parts(&mut p, &f.state)
            .await
            .is_err());
        assert!(!f
            .state
            .browser
            .inner()
            .unwrap()
            .store
            .lock()
            .unwrap()
            .sessions
            .contains_key(cookie.split_once('=').unwrap().1));
    }
    *f.fake.options.lock().unwrap() = json!({"no_refresh":true});
    let (cookie, _) = f.signed_in().await;
    f.expire_access(&cookie);
    let mut p = f.parts(&cookie);
    assert!(BrowserContext::from_request_parts(&mut p, &f.state)
        .await
        .is_err());
}
#[tokio::test]
async fn browser_logout_during_refresh_and_cancellation_never_resurrect() {
    let f = Fixture::new().await;
    let (cookie, value) = f.signed_in().await;
    f.expire_access(&cookie);
    *f.fake.options.lock().unwrap() = json!({"block":true});
    let state = f.state.clone();
    let mut parts = f.parts(&cookie);
    let pending =
        tokio::spawn(async move { BrowserContext::from_request_parts(&mut parts, &state).await });
    f.fake.started.notified().await;
    let r = f
        .request(reqwest::Method::POST, "/browser/logout")
        .header("cookie", &cookie)
        .header("origin", "https://sigil.test")
        .header("x-sigil-csrf", value["csrf_token"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    f.fake.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    assert!(f
        .state
        .browser
        .inner()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .sessions
        .is_empty());
    *f.fake.options.lock().unwrap() = json!({});
    let (cookie, _) = f.signed_in().await;
    f.expire_access(&cookie);
    *f.fake.options.lock().unwrap() = json!({"block":true});
    let state = f.state.clone();
    let mut parts = f.parts(&cookie);
    let pending =
        tokio::spawn(async move { BrowserContext::from_request_parts(&mut parts, &state).await });
    f.fake.started.notified().await;
    pending.abort();
    assert!(matches!(pending.await,Err(e) if e.is_cancelled()));
    f.fake.release.notify_one();
    assert!(f
        .state
        .browser
        .inner()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .sessions
        .is_empty());
    let mut p = f.parts(&cookie);
    assert!(BrowserContext::from_request_parts(&mut p, &f.state)
        .await
        .is_err());
    assert_eq!(f.fake.refreshes.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn browser_disabled_and_size_limits_are_typed() {
    let f = Fixture::new().await;
    assert_safe_error(
        f.request(
            reqwest::Method::GET,
            &format!("/browser/login?return_to={}", "x".repeat(5000)),
        )
        .send()
        .await
        .unwrap(),
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    assert_safe_error(
        f.request(reqwest::Method::POST, "/browser/logout")
            .body("unexpected")
            .send()
            .await
            .unwrap(),
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    let mut parts = f.parts("");
    let state = AppState::test_without_runtime();
    assert_eq!(
        BrowserContext::from_request_parts(&mut parts, &state)
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn browser_security_regressions_canonical_urls_and_refresh_nonce() {
    for (key, bad) in [
        ("ISSUER", "https://idp.test/x/../issuer/"),
        ("AUTHORIZATION_URL", "https://idp.test/x/../authorize"),
        ("ORIGINS", "https://*.test"),
        ("ORIGINS", "http://localhost,https://localhost"),
    ] {
        let mut map = config_map("https://idp.test");
        map.insert(format!("SIGILED_BROWSER_{key}"), bad.into());
        assert!(Config::from_map(&map).is_err(), "{key}: {bad}");
    }
    let f = Fixture::new().await;
    let (cookie, _) = f.signed_in().await;
    f.expire_access(&cookie);
    *f.fake.options.lock().unwrap() = json!({"id":{"nonce":"different-original-login"}});
    let mut p = f.parts(&cookie);
    assert!(BrowserContext::from_request_parts(&mut p, &f.state)
        .await
        .is_err());
}
#[tokio::test]
async fn browser_absolute_expiry_during_refresh_and_stable_subjects() {
    let f = Fixture::new().await;
    let (cookie, first) = f.signed_in().await;
    *f.fake.options.lock().unwrap() =
        json!({"access":{"sub":"human-two","preferred_username":"alice"}});
    let (_, second) = f.signed_in().await;
    assert_ne!(first["actor"]["driver"], second["actor"]["driver"]);
    *f.fake.options.lock().unwrap() = json!({});
    f.expire_access(&cookie);
    {
        let b = f.state.browser.inner().unwrap();
        let mut store = b.store.lock().unwrap();
        let s = Arc::get_mut(
            store
                .sessions
                .get_mut(cookie.split_once('=').unwrap().1)
                .unwrap(),
        )
        .unwrap();
        s.absolute = auth::now_epoch() + 1;
    }
    *f.fake.options.lock().unwrap() = json!({"block":true});
    let state = f.state.clone();
    let mut parts = f.parts(&cookie);
    let pending =
        tokio::spawn(async move { BrowserContext::from_request_parts(&mut parts, &state).await });
    f.fake.started.notified().await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    f.fake.release.notify_one();
    assert!(pending.await.unwrap().is_err());
    assert!(!f
        .state
        .browser
        .inner()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .sessions
        .contains_key(cookie.split_once('=').unwrap().1));
}
#[tokio::test]
async fn browser_rejected_csrf_does_not_clear_a_live_cookie() {
    let f = Fixture::new().await;
    let (cookie, _) = f.signed_in().await;
    let r = f
        .request(reqwest::Method::POST, "/browser/logout")
        .header("cookie", &cookie)
        .header("origin", "https://evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(r.headers().get("set-cookie").is_none());
    assert_eq!(
        f.request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
#[tokio::test]
async fn browser_machine_jwt_cache_does_not_cross_issuers_with_same_kid() {
    let f = Fixture::new().await;
    let first = sign(
        &json!({"iss":format!("{}/issuer/",f.fake.origin),"sub":"machine","azp":"driver-client","groups":["stack:drivers"],"exp":auth::now_epoch()+600}),
    );
    let r = f
        .request(reqwest::Method::GET, "/overview")
        .bearer_auth(&first)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = f
        .request(reqwest::Method::GET, "/auth/verify")
        .bearer_auth(&first)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["caller"], "driver-client");
    assert_eq!(body["subject"], "machine");
    // Different issuer with same kid advertises a different modulus. First issuer's token cannot authenticate as it.
    let crossed = sign(
        &json!({"iss":format!("{}/other/",f.fake.origin),"sub":"forged","groups":["stack:admins"],"exp":auth::now_epoch()+600}),
    );
    assert_eq!(
        f.request(reqwest::Method::GET, "/auth/verify")
            .bearer_auth(crossed)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request(reqwest::Method::GET, "/auth/verify")
            .bearer_auth("synthetic-bootstrap")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn browser_session_fixation_login_rotation_and_bounded_pruning() {
    let f = Fixture::new().await;
    let (cookie, _) = f.signed_in().await;
    let (state, binding) = f.start_cookie(&cookie).await;
    assert_eq!(
        f.request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let response = f.callback(&state, &format!("{binding}; {cookie}")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_ne!(response_cookie(&response, SESSION), cookie);
    let chosen = format!("{SESSION}={}", "A".repeat(43));
    let (state, binding) = f.start_cookie(&chosen).await;
    let response = f.callback(&state, &format!("{binding}; {chosen}")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_ne!(response_cookie(&response, SESSION), chosen);
    let (state, binding) = f.start().await;
    let (_, replacement) = f.start_cookie(&binding).await;
    assert_ne!(binding, replacement);
    assert_safe_error(f.callback(&state, &binding).await, StatusCode::UNAUTHORIZED).await;
    let b = f.state.browser.inner().unwrap();
    {
        let mut store = b.store.lock().unwrap();
        store.logins.clear();
        for n in 0..MAX_LOGINS {
            store.logins.insert(
                n.to_string(),
                Transaction {
                    binding: "binding".into(),
                    origin: "https://sigil.test".into(),
                    nonce: "nonce".into(),
                    verifier: "verifier".into(),
                    return_to: "/".into(),
                    expires: auth::now_epoch() + 300,
                    in_flight: false,
                    replaces_session: None,
                },
            );
        }
    }
    assert_safe_error(
        f.request(reqwest::Method::GET, "/browser/login")
            .send()
            .await
            .unwrap(),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    {
        let mut store = b.store.lock().unwrap();
        for t in store.logins.values_mut() {
            t.expires = 0;
        }
    }
    assert_eq!(
        f.request(reqwest::Method::GET, "/browser/login")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    {
        let mut store = b.store.lock().unwrap();
        let session = store.sessions.values().next().unwrap().clone();
        store.sessions.clear();
        for n in 0..MAX_SESSIONS {
            store.sessions.insert(n.to_string(), session.clone());
        }
    }
    let (state, binding) = f.start().await;
    assert_safe_error(
        f.callback(&state, &binding).await,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    {
        let store = b.store.lock().unwrap();
        store
            .sessions
            .values()
            .next()
            .unwrap()
            .last_seen
            .store(0, Ordering::Relaxed);
    }
    let (state, binding) = f.start().await;
    assert_eq!(
        f.callback(&state, &binding).await.status(),
        StatusCode::SEE_OTHER
    );
}
#[tokio::test]
async fn browser_access_and_id_token_expiry_are_exclusive_at_current_second() {
    let mut captured = false;
    for _ in 0..3 {
        let now = auth::now_epoch();
        let jwt = sign(&json!({"iss":"https://idp.test/issuer/","sub":"human","exp":now}));
        let key = jsonwebtoken::DecodingKey::from_rsa_pem(TEST_RSA_PUBLIC.as_bytes()).unwrap();
        let result = auth::validate_exact_jwt(&jwt, "https://idp.test/issuer/", &key);
        if auth::now_epoch() == now {
            assert!(result.is_err());
            captured = true;
            break;
        }
    }
    assert!(
        captured,
        "could not capture stable one-second fixture interval"
    );
    // ID-token verification has the same exclusive boundary during code and refresh flows.
    // The signed access token remains valid so rejection must come from the expired ID token.
    let f = Fixture::new().await;
    *f.fake.options.lock().unwrap() = json!({"id":{"exp":auth::now_epoch()}});
    let (state, binding) = f.start().await;
    assert_safe_error(f.callback(&state, &binding).await, StatusCode::UNAUTHORIZED).await;
    *f.fake.options.lock().unwrap() = json!({});
    let (cookie, _) = f.signed_in().await;
    f.expire_access(&cookie);
    *f.fake.options.lock().unwrap() = json!({"id":{"exp":auth::now_epoch()}});
    let mut parts = f.parts(&cookie);
    assert!(BrowserContext::from_request_parts(&mut parts, &f.state)
        .await
        .is_err());
    assert!(f
        .state
        .browser
        .inner()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .sessions
        .is_empty());
}

#[tokio::test]
async fn browser_review_unbound_callback_preserves_established_session() {
    let f = Fixture::new().await;
    let (cookie, _) = f.signed_in().await;
    let response = f
        .request(
            reqwest::Method::GET,
            "/browser/callback?state=invalid&code=synthetic-code",
        )
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        response.headers().get("set-cookie").is_none(),
        "unbound callback must not change browser cookies"
    );
    assert_eq!(
        f.request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
#[tokio::test]
async fn browser_review_login_initiation_and_failed_attempt_preserve_established_session() {
    let f = Fixture::new().await;
    let (cookie, _) = f.signed_in().await;
    let response = f
        .request(reqwest::Method::GET, "/browser/login")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        response
            .headers()
            .get_all("set-cookie")
            .iter()
            .all(|v| !v.to_str().unwrap().starts_with(SESSION)),
        "GET login must preserve the established session cookie"
    );
    assert_eq!(
        f.request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let (state, binding) = f.start_cookie(&cookie).await;
    *f.fake.options.lock().unwrap() = json!({"reject":true});
    let response = f.callback(&state, &format!("{binding}; {cookie}")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().get("set-cookie").is_none());
    assert_eq!(
        f.request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
#[tokio::test]
async fn browser_review_superseded_inflight_callback_cannot_publish_or_clear_new_cookie() {
    let f = Fixture::new().await;
    let (state_a, binding_a) = f.start().await;
    *f.fake.options.lock().unwrap() = json!({"block_code":true});
    let request = f
        .request(
            reqwest::Method::GET,
            &format!("/browser/callback?code=synthetic-code&state={state_a}"),
        )
        .header("cookie", &binding_a);
    let pending = tokio::spawn(async move { request.send().await.unwrap() });
    f.fake.started.notified().await;
    *f.fake.options.lock().unwrap() = json!({});
    let (state_b, binding_b) = f.start_cookie(&binding_a).await;
    assert_ne!(binding_a, binding_b);
    f.fake.release.notify_one();
    let stale = pending.await.unwrap();
    assert_eq!(
        stale.status(),
        StatusCode::UNAUTHORIZED,
        "superseded callback must not publish a session"
    );
    assert!(
        stale.headers().get("set-cookie").is_none(),
        "stale response must not clear B's binding"
    );
    assert!(f
        .state
        .browser
        .inner()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .sessions
        .is_empty());
    let current = f.callback(&state_b, &binding_b).await;
    assert_eq!(current.status(), StatusCode::SEE_OTHER);
    let cookie = response_cookie(&current, SESSION);
    assert_eq!(
        f.request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[test]
fn browser_review_id_token_expiry_checks_a_deterministic_same_instant() {
    // This is the production verifier's shared initial/refresh time predicate.
    assert!(provider::validate_id_time(999, 990, 1_000).is_err());
    assert!(provider::validate_id_time(1_000, 990, 1_000).is_err());
    assert!(provider::validate_id_time(1_001, 990, 1_000).is_ok());
    assert!(provider::validate_id_time(1_100, 1_031, 1_000).is_err());
}
#[tokio::test]
async fn browser_review_logout_cancels_pending_and_inflight_login_replacement() {
    for in_flight in [false, true] {
        let f = Fixture::new().await;
        let (session_cookie, view) = f.signed_in().await;
        let (state, binding) = f.start_cookie(&session_cookie).await;
        let request = f
            .request(
                reqwest::Method::GET,
                &format!("/browser/callback?code=synthetic-code&state={state}"),
            )
            .header("cookie", format!("{binding}; {session_cookie}"));
        let pending = if in_flight {
            *f.fake.options.lock().unwrap() = json!({"block_code":true});
            let pending = tokio::spawn(async move { request.send().await.unwrap() });
            f.fake.started.notified().await;
            Some(pending)
        } else {
            None
        };
        let logout = f
            .request(reqwest::Method::POST, "/browser/logout")
            // The session linkage must cancel A even if the login cookie is absent.
            .header("cookie", &session_cookie)
            .header("origin", "https://sigil.test")
            .header("x-sigil-csrf", view["csrf_token"].as_str().unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);
        let stale = if let Some(pending) = pending {
            f.fake.release.notify_one();
            pending.await.unwrap()
        } else {
            f.callback(&state, &binding).await
        };
        assert_eq!(stale.status(), StatusCode::UNAUTHORIZED);
        assert!(stale.headers().get("set-cookie").is_none());
        let b = f.state.browser.inner().unwrap();
        let store = b.store.lock().unwrap();
        assert!(store.sessions.is_empty());
        assert!(store.logins.is_empty());
    }
}

#[tokio::test]
async fn browser_review_cancelled_callback_consumes_only_its_attempt() {
    let f = Fixture::new().await;
    let (session_cookie, _) = f.signed_in().await;
    let (state_id, binding) = f.start_cookie(&session_cookie).await;
    *f.fake.options.lock().unwrap() = json!({"block_code":true});
    let state = f.state.clone();
    let mut headers = HeaderMap::new();
    headers.insert("host", "sigil.test".parse().unwrap());
    headers.insert(
        "cookie",
        format!("{binding}; {session_cookie}").parse().unwrap(),
    );
    let query = RawQuery(Some(format!("code=synthetic-code&state={state_id}")));
    let pending = tokio::spawn(async move { callback(State(state), headers, query).await });
    f.fake.started.notified().await;
    pending.abort();
    assert!(matches!(pending.await, Err(e) if e.is_cancelled()));
    f.fake.release.notify_one();
    assert!(!f
        .state
        .browser
        .inner()
        .unwrap()
        .store
        .lock()
        .unwrap()
        .logins
        .contains_key(&state_id));
    assert_eq!(
        f.request(reqwest::Method::GET, "/browser/session")
            .header("cookie", &session_cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn dashboard_shell_is_origin_bound_and_new_mutations_require_login() {
    let f = Fixture::new().await;
    let r = f.request(reqwest::Method::GET, "/").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers().contains_key("content-security-policy"));
    assert!(r
        .text()
        .await
        .unwrap()
        .contains("/browser/assets/dashboard.js"));
    assert_eq!(
        f.client
            .get(format!("{}/", f.base))
            .header("host", "machine.test")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let r = f
        .request(
            reqwest::Method::POST,
            "/browser/api/projects/demo/work-items",
        )
        .json(&json!({"title":"test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn dashboard_work_items_auth_cas_audit_restart_and_failed_save() {
    let f = Fixture::new().await;
    f.state.registry.insert(crate::project::ProjectRecord::new(
        "demo",
        &crate::manifest::Manifest::parse("").unwrap(),
        None,
    ));
    let (cookie, session) = f.signed_in().await;
    let fields = json!({"title":"<script>fixture</script>","description":"Unsaved notes","state":"open","owner":"Operator","source_link":"/ui/projects/demo/pending"});
    let payload = json!({"id":"f0000000-0000-4000-8000-000000000001","fields":fields});
    let url = "/browser/api/projects/demo/work-items";
    assert_eq!(
        f.request(reqwest::Method::POST, url)
            .header("cookie", &cookie)
            .json(&payload)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let send = |method, path: &str, value: &Value| {
        f.request(method, path)
            .header("cookie", &cookie)
            .header("origin", "https://sigil.test")
            .header("x-sigil-csrf", session["csrf_token"].as_str().unwrap())
            .json(value)
    };
    let r = send(reqwest::Method::POST, url, &payload)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let first: Value = r.json().await.unwrap();
    assert_eq!(first["revision"], 1);
    assert_eq!(first["creator"], session["actor"]["driver"]);
    let retry: Value = send(reqwest::Method::POST, url, &payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first, retry);
    let detail = format!("{url}/{}", first["id"].as_str().unwrap());
    let update = json!({"expected_revision":1,"fields":fields});
    let mut update = update;
    update["fields"] = fields.clone();
    update["fields"]["state"] = json!("blocked");
    let (a, b) = tokio::join!(
        send(reqwest::Method::PATCH, &detail, &update).send(),
        send(reqwest::Method::PATCH, &detail, &update).send()
    );
    let mut statuses = vec![a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    let dir = f.state.sessions.repos_dir.as_ref().unwrap();
    let restarted = crate::work_items::Store::at_dir(dir);
    let saved = restarted
        .get("demo", first["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.audit.len(), 2);
    for link in [
        "javascript:alert(1)",
        "//evil.test",
        "/ui/../browser/logout",
        "https://user:password@evil.test",
    ] {
        let mut invalid = payload.clone();
        invalid["fields"]["source_link"] = json!(link);
        assert_eq!(
            send(reqwest::Method::POST, url, &invalid)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        f.request(reqwest::Method::GET, &format!("{url}?limit=0"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let list: Value = f
        .request(reqwest::Method::GET, &format!("{url}?limit=1"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["total"], 1);
    assert!(list["items"][0].get("audit").is_none());
    // Force atomic replacement failure before publication: prior durable row/audit survive.
    std::fs::create_dir(dir.join("work-items.json.tmp")).unwrap();
    let mut next = update.clone();
    next["expected_revision"] = json!(2);
    next["fields"]["title"] = json!("must not appear");
    assert_eq!(
        send(reqwest::Method::PATCH, &detail, &next)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let saved = restarted
        .get("demo", first["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.audit.len(), 2);
    assert_eq!(
        f.request(reqwest::Method::POST, "/browser/logout")
            .header("cookie", &cookie)
            .header("origin", "https://sigil.test")
            .header("x-sigil-csrf", session["csrf_token"].as_str().unwrap())
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
#[tokio::test]
async fn dashboard_project_partial_retry_two_tabs_and_approval() {
    let generated = Arc::new(AtomicUsize::new(0));
    let installed = Arc::new(Mutex::new(String::new()));
    let key_attempts = Arc::new(AtomicUsize::new(0));
    let count = generated.clone();
    let key = installed.clone();
    let attempt = key_attempts.clone();
    let read_key = installed.clone();
    let mock = Router::new()
        .route(
            "/repos/fixture/template/generate",
            post(move || {
                let count = count.clone();
                async move {
                    if count.fetch_add(1, Ordering::SeqCst) == 0 {
                        (
                            StatusCode::CREATED,
                            Json(json!({"full_name":"fixture/demo"})),
                        )
                            .into_response()
                    } else {
                        StatusCode::UNPROCESSABLE_ENTITY.into_response()
                    }
                }
            }),
        )
        .route("/repos/fixture/demo", get(|| async { StatusCode::OK }))
        .route(
            "/repos/fixture/demo/keys",
            post(move |Json(body): Json<Value>| {
                let key = key.clone();
                let attempt = attempt.clone();
                async move {
                    let mut key = key.lock().unwrap();
                    let value = body["key"]
                        .as_str()
                        .unwrap()
                        .split_whitespace()
                        .take(2)
                        .collect::<Vec<_>>()
                        .join(" ");
                    if key.is_empty() {
                        *key = value;
                    } else {
                        assert_eq!(*key, value, "retry key must match incumbent");
                    }
                    if attempt.fetch_add(1, Ordering::SeqCst) == 0 {
                        StatusCode::BAD_GATEWAY
                    } else {
                        StatusCode::UNPROCESSABLE_ENTITY
                    }
                }
            })
            .get(move || {
                let key = read_key.clone();
                async move { Json(json!([{"key":key.lock().unwrap().clone(),"read_only":false}])) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let keys = std::env::temp_dir().join(format!("dashboard-project-retry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&keys);
    let f = Fixture::with_github(Some(crate::github::GitHub {
        api_base: base,
        pat: "synthetic".into(),
        owner: "fixture".into(),
        template: "template".into(),
        keys_dir: keys.clone(),
    }))
    .await;
    *f.fake.options.lock().unwrap() = json!({"access":{"groups":["stack:admins"]}});
    let (cookie, session) = f.signed_in().await;
    let send = || {
        f.request(reqwest::Method::POST, "/browser/api/projects")
            .header("cookie", &cookie)
            .header("origin", "https://sigil.test")
            .header("x-sigil-csrf", session["csrf_token"].as_str().unwrap())
            .json(&json!({"name":"demo"}))
            .send()
    };
    let r = send().await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
    let partial: Value = r.json().await.unwrap();
    assert_eq!(partial["state"], "partial");
    assert_eq!(partial["retry_same_name"], true);
    assert!(f.state.registry.contains("demo"));
    assert!(f.state.registry.descriptor("demo").registration_pending);
    let enrollment_owner = f
        .state
        .registry
        .descriptor("demo")
        .memory_enrollment
        .unwrap()
        .owner;
    let incumbent = std::fs::read(keys.join("demo/id_ed25519")).unwrap();
    let (a, b) = tokio::join!(send(), send());
    let mut statuses = vec![a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 201]);
    assert_eq!(generated.load(Ordering::SeqCst), 2);
    assert_eq!(key_attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        incumbent,
        std::fs::read(keys.join("demo/id_ed25519")).unwrap()
    );
    assert_eq!(f.state.events.for_project("demo").len(), 1);
    assert!(!f.state.registry.descriptor("demo").registration_pending);
    assert_eq!(
        f.state
            .registry
            .descriptor("demo")
            .memory_enrollment
            .unwrap()
            .owner,
        enrollment_owner
    );
    // Even a known project's retry must satisfy the actual new actor's policy.
    *f.fake.options.lock().unwrap() =
        json!({"access":{"sub":"unapproved-human","groups":["stack:drivers"]}});
    let (other, other_session) = f.signed_in().await;
    let r = f
        .request(reqwest::Method::POST, "/browser/api/projects")
        .header("cookie", other)
        .header("origin", "https://sigil.test")
        .header(
            "x-sigil-csrf",
            other_session["csrf_token"].as_str().unwrap(),
        )
        .json(&json!({"name":"demo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(generated.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn ide_gateway_disabled_is_explicit_and_authenticated() {
    let f = Fixture::new().await;
    let (cookie, session) = f.signed_in().await;
    let r = f
        .request(reqwest::Method::POST, "/browser/api/projects/demo/ide")
        .header("cookie", cookie)
        .header("origin", "https://sigil.test")
        .header("x-sigil-csrf", session["csrf_token"].as_str().unwrap())
        .json(&json!({"idempotency_key":"one-operation"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::CONFLICT,
        "gateway must return explicit disabled capability"
    );
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"],
        "ide_gateway_not_configured"
    );
}

#[tokio::test]
async fn ide_parent_authority_shares_refresh_and_does_not_invent_idle_activity() {
    let f = Fixture::new().await;
    let (state, binding) = f.start().await;
    let r = f.callback(&state, &binding).await;
    let cookie = response_cookie(&r, SESSION);
    let id = cookie.split_once('=').unwrap().1;
    f.expire_access(&cookie);
    let b = f.state.browser.inner().unwrap();
    let session = b.store.lock().unwrap().sessions[id].clone();
    let before = auth::now_epoch() - 60;
    session.last_seen.store(before, Ordering::Relaxed);
    let (a, c) = tokio::join!(
        resolve_session(&f.state, id, false),
        resolve_session(&f.state, id, false)
    );
    assert!(
        a.is_ok() && c.is_ok(),
        "IDE must use actual B1 credential refresh authority"
    );
    assert_eq!(f.fake.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        session.last_seen.load(Ordering::Relaxed),
        before,
        "background validation is not activity"
    );
    assert!(resolve_session(&f.state, id, true).await.is_ok());
    assert!(session.last_seen.load(Ordering::Relaxed) > before);
    b.store.lock().unwrap().sessions.remove(id);
    assert!(
        resolve_session(&f.state, id, true).await.is_err(),
        "revocation wins over valid cached credentials"
    );
}

#[test]
fn gateway_readiness_and_origin_configuration_fail_closed() {
    let mut base = config_map("https://idp.test");
    base.insert(
        "SIGILED_BROWSER_IDE_DOMAIN".into(),
        "ide.example.test".into(),
    );
    for k in [
        "IDE_DNS_TLS_READY",
        "IDE_OIDC_READY",
        "IDE_PROVIDER_READY",
        "IDE_POLICY_READY",
    ] {
        base.insert(format!("SIGILED_BROWSER_{k}"), "true".into());
    }
    assert!(Config::from_map(&base).is_ok());
    for key in [
        "IDE_DNS_TLS_READY",
        "IDE_OIDC_READY",
        "IDE_PROVIDER_READY",
        "IDE_POLICY_READY",
    ] {
        let mut map = base.clone();
        map.remove(&format!("SIGILED_BROWSER_{key}"));
        assert!(Config::from_map(&map).is_err());
    }
    let mut shared = base.clone();
    shared.insert(
        "SIGILED_BROWSER_ORIGINS".into(),
        "https://dashboard.ide.example.test:8443".into(),
    );
    assert!(
        Config::from_map(&shared).is_err(),
        "cookie hostname isolation also applies across ports"
    );
    let mut orphan = base.clone();
    orphan.insert(
        "SIGILED_BROWSER_PREVIEW_DNS_TLS_READY".into(),
        "true".into(),
    );
    assert!(
        Config::from_map(&orphan).is_err(),
        "preview readiness needs an explicit domain"
    );
    for domain in [
        "ide.example.test",
        "child.ide.example.test",
        "EXAMPLE.TEST",
        "127.0.0.1",
        "https://preview.example.test",
    ] {
        let mut map = base.clone();
        map.insert("SIGILED_BROWSER_PREVIEW_DOMAIN".into(), domain.into());
        map.insert(
            "SIGILED_BROWSER_PREVIEW_DNS_TLS_READY".into(),
            "true".into(),
        );
        assert!(Config::from_map(&map).is_err(), "{domain}");
    }
}

async fn large_review_projection(detail: bool) {
    let f = Fixture::new().await;
    let (cookie, session) = f.signed_in().await;
    let actor: Actor = serde_json::from_value(session["actor"].clone()).unwrap();
    let mut descriptors = std::collections::BTreeMap::new();
    for i in 0..100 {
        let name = format!("project-{i:03}");
        let mut manifest = crate::manifest::Manifest::parse("").unwrap();
        manifest.declaration.project.description = Some("d".repeat(1000));
        manifest.declaration.project.display_name = Some(format!("Project {i:03}"));
        manifest.declaration.validate(None).unwrap();
        f.state
            .registry
            .insert(crate::project::ProjectRecord::new(&name, &manifest, None));
        let mut d = crate::ecosystem::Descriptor {
            declaration: manifest.declaration,
            ..Default::default()
        };
        if i == 0 {
            d.jobs = (0..100)
                .map(|n| crate::ecosystem::JobDefinition {
                    name: format!("nightly-{n:03}"),
                    cron: "0 1 * * *".into(),
                    timeout_minutes: 30,
                })
                .collect();
        }
        descriptors.insert(name, d);
    }
    f.state.registry.hydrate_descriptors(descriptors);
    let mut records = HashMap::new();
    for i in 0..100 {
        let id = format!("{i:032x}");
        let record:crate::sessions::SessionRecord=serde_json::from_value(json!({"session_id":id,"project":"project-000","branch":format!("session/{id}"),"head":"a".repeat(40),"stale":false,"actor":actor,"generation":9007199254740993u64,"lifecycle":"active","token":"private-review-token"})).unwrap();
        records.insert(id, record);
    }
    let debts=(0..20).map(|i|serde_json::from_value(json!({"branch":format!("session/debt-{i}"),"conflicted_files":(0..30).map(|j|format!("docs/{}/file-{j}.md","path".repeat(20))).collect::<Vec<_>>(),"ours":{"sha":"a".repeat(40),"commit_messages":[]},"theirs":{"sha":"b".repeat(40),"commit_messages":[]},"since":"2026-09-10"})).unwrap()).collect();
    f.state
        .sessions
        .hydrate(HashMap::from([("project-000".into(), debts)]), records);
    for (path, machine) in [
        (
            "/browser/api/overview?limit=100",
            crate::overview::root(
                actor.clone(),
                State(f.state.clone()),
                axum::extract::Query(crate::overview::Page {
                    offset: None,
                    limit: Some(100),
                }),
            )
            .await,
        ),
        (
            "/browser/api/projects/project-000?limit=100",
            crate::overview::detail(
                actor.clone(),
                State(f.state.clone()),
                axum::extract::Path("project-000".into()),
                axum::extract::Query(crate::overview::Page {
                    offset: None,
                    limit: Some(100),
                }),
            )
            .await,
        ),
    ]
    .into_iter()
    .filter(|(path, _)| path.contains("overview") != detail)
    {
        assert_eq!(machine.status(), StatusCode::OK);
        let baseline = axum::body::to_bytes(machine.into_body(), 4 << 20)
            .await
            .unwrap();
        assert!(
            baseline.len() > 65536,
            "fixture must exceed accidental helper cap: {}",
            baseline.len()
        );
        let expected: Value = serde_json::from_slice(&baseline).unwrap();
        let r = f
            .request(reqwest::Method::GET, path)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::OK,
            "valid supported dashboard projection: {path}"
        );
        let v: Value = r.json().await.unwrap();
        if path.contains("overview") {
            assert_eq!(v["projects"]["items"].as_array().unwrap().len(), 100);
            assert_eq!(v["projects"], expected["projects"]);
        } else {
            assert_eq!(v["sessions"]["items"].as_array().unwrap().len(), 100);
            assert_eq!(v["jobs"]["items"].as_array().unwrap().len(), 100);
            assert_eq!(v["merge_debt"], expected["merge_debt"]);
            assert_eq!(v["description"], "d".repeat(1000));
            for row in v["sessions"]["items"].as_array().unwrap() {
                assert_eq!(row["generation"], "9007199254740993");
            }
            assert_eq!(
                expected["sessions"]["items"][0]["generation"],
                9007199254740993u64
            );
        }
        assert!(!v.to_string().contains("private-review-token"));
    }
}
#[tokio::test]
async fn review_c2_i2_start_requires_browser_readiness_before_provider_lock() {
    for (enabled, durable, error) in [
        (false, false, "ide_gateway_not_configured"),
        (true, false, "durable_state_required"),
    ] {
        let f = Fixture::with_readiness(None, enabled, durable).await;
        let (cookie, session) = f.signed_in().await;
        assert_eq!(session["features"]["workspace_actions"], false);
        let id = "f".repeat(32);
        let record:crate::sessions::SessionRecord=serde_json::from_value(json!({"session_id":id,"project":"demo","branch":format!("session/{id}"),"head":"a".repeat(40),"stale":false,"actor":session["actor"],"generation":1,"lifecycle":"active","runtime_owned":true,"token":"private-review-token"})).unwrap();
        f.state.sessions.put(record);
        let lock = f.state.sessions.session_lock(&id).lock_owned().await;
        let response = f
            .request(
                reqwest::Method::POST,
                &format!("/browser/api/sessions/{id}/ide"),
            )
            .header("cookie", &cookie)
            .header("origin", "https://sigil.test")
            .header("x-sigil-csrf", session["csrf_token"].as_str().unwrap())
            .timeout(Duration::from_millis(500))
            .json(&json!({"action":"start","generation":"1"}))
            .send()
            .await
            .expect(
                "disabled browser start must return before entering C1 provider lifecycle lock",
            );
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(response.json::<Value>().await.unwrap()["error"], error);
        assert!(f.state.sessions.record(&id).unwrap().binding.is_none());
        drop(lock);
    }
}

#[tokio::test]
async fn review_c2_i1_large_valid_inventory() {
    large_review_projection(false).await;
}
#[tokio::test]
async fn review_c2_i1_populated_detail() {
    large_review_projection(true).await;
}

#[tokio::test]
async fn research_routes_require_exact_origin_csrf_and_live_browser_login() {
    let f = Fixture::new().await;
    let (cookie, session) = f.signed_in().await;
    for path in [
        "/browser/api/projects/demo/research",
        "/browser/api/research/run1",
        "/browser/api/research/run1/handoff",
    ] {
        let body = if path.ends_with("handoff") {
            json!({"session_id":"fixture","generation":"9007199254740993","bundle_digest":"x"})
        } else if path.ends_with("/run1") {
            json!({"action":"resume","expected_revision":"9007199254740993"})
        } else {
            json!({"operation_id":"operation1","problem":"fixture"})
        };
        for (origin, csrf) in [
            ("https://evil.test", session["csrf_token"].as_str().unwrap()),
            ("https://sigil.test", "wrong"),
        ] {
            assert_eq!(
                f.request(reqwest::Method::POST, path)
                    .header("cookie", &cookie)
                    .header("origin", origin)
                    .header("x-sigil-csrf", csrf)
                    .json(&body)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            f.request(reqwest::Method::POST, path)
                .header("origin", "https://sigil.test")
                .header("x-sigil-csrf", session["csrf_token"].as_str().unwrap())
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let b = f.state.browser.inner().unwrap();
    {
        let store = b.store.lock().unwrap();
        let session = store
            .sessions
            .get(cookie.split_once('=').unwrap().1)
            .unwrap();
        session
            .last_seen
            .store(0, std::sync::atomic::Ordering::SeqCst);
    }
    assert_eq!(
        f.request(reqwest::Method::GET, "/browser/api/research")
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn memory_origin_requires_explicit_distinct_enrollment() {
    let mut env = config_map("https://idp.test");
    env.insert(
        "SIGILED_BROWSER_ORIGINS".into(),
        "https://sigil.test,https://memory.test".into(),
    );
    env.insert(
        "SIGILED_BROWSER_MEMORY_ORIGIN".into(),
        "https://memory.test".into(),
    );
    assert!(
        Config::from_map(&env).is_ok(),
        "explicit Memory site must be accepted"
    );
    env.insert(
        "SIGILED_BROWSER_MEMORY_ORIGIN".into(),
        "https://attacker.test".into(),
    );
    assert!(Config::from_map(&env).is_err());
    env.insert(
        "SIGILED_BROWSER_MEMORY_ORIGIN".into(),
        "https://sigil.test".into(),
    );
    assert!(Config::from_map(&env).is_err());
}

#[tokio::test]
async fn memory_browser_routes_require_host_session_and_csrf() {
    let f = Fixture::new().await;
    for (method, path, body) in [
        (
            Method::POST,
            "/browser/api/memory/indexes/demo/manual",
            json!({"create_id":"00000000-0000-4000-8000-000000000001","text":"fixture"}),
        ),
        (
            Method::POST,
            "/browser/api/memory/indexes/demo/curation",
            json!({"target":{"kind":"manual","id":"m_00000000-0000-4000-8000-000000000001"},"expected_revision":"0","pinned":true}),
        ),
        (
            Method::POST,
            "/browser/api/memory/indexes/demo/forget/preview",
            json!({"selector":{"kind":"document","source":"git","ref":"repo","path":""}}),
        ),
        (
            Method::POST,
            "/browser/api/memory/indexes/demo/forget",
            json!({"selector":{"kind":"document","source":"git","ref":"repo","path":""},"confirmation":"fixture"}),
        ),
        (
            Method::POST,
            "/browser/api/memory/indexes/demo/reindex",
            json!({"source_id":"fixture"}),
        ),
    ] {
        assert_eq!(
            f.request(method, path)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let (state, cookie) = f.start().await;
    let response = f.callback(&state, &cookie).await;
    let session = response_cookie(&response, SESSION);
    assert_eq!(
        f.request(Method::POST, "/browser/api/memory/indexes/demo/manual")
            .header("cookie", &session)
            .header("origin", "https://sigil.test")
            .json(&json!({"create_id":"00000000-0000-4000-8000-000000000001","text":"fixture"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
}
