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
    let mut id = json!({"iss":format!("{}/issuer/",fake.origin),"sub":access["sub"],"aud":"shared-browser-client","azp":"shared-browser-client","exp":auth::now_epoch()+600,"iat":auth::now_epoch(),"nonce":*fake.nonce.lock().unwrap(),"at_hash":URL_SAFE_NO_PAD.encode(&Sha256::digest(access_token.as_bytes())[..16])});
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
struct Fixture {
    state: AppState,
    fake: Fake,
    base: String,
    client: reqwest::Client,
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
        state.auth.config = Arc::new(auth::AuthConfig {
            oidc_base: Some(origin),
            admin_group: "stack:admins".into(),
            driver_group: "stack:drivers".into(),
            bootstrap_bearer: Some("synthetic-bootstrap".into()),
            ..auth::AuthConfig::default()
        });
        state.browser = BrowserState::configured(
            Config::from_map(&config_map(&fake.origin)).unwrap(),
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
    async fn signed_in(&self) -> (String, Value) {
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
        StatusCode::UNAUTHORIZED
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
