use super::*;
use crate::browser::tests::Fixture;
use serde_json::{json, Value};
async fn fixture() -> (Fixture, access::Binding, String) {
    let f = Fixture::gateway().await;
    let (cookie, s) = f.signed_in().await;
    let id = "a".repeat(32);
    let generation = 9007199254740993;
    let actor: crate::auth::Actor = serde_json::from_value(s["actor"].clone()).unwrap();
    let r = SessionRecord {
        session_id: id.clone(),
        project: "demo".into(),
        branch: format!("session/{id}"),
        head: "a".repeat(40),
        stale: false,
        actor: actor.clone(),
        token: Some("private-workspace-token".into()),
        binding: Some(crate::sessions::WorkspaceBinding {
            container: crate::runtime::Runtime::session_container("demo", &id, generation),
            endpoint: "https://machine.invalid/private".into(),
            generation,
            ide_error: None,
            ide: Some(crate::ide::Binding {
                provider: crate::ide::PROVIDER.into(),
                helper: crate::ide::HELPER.into(),
                base_digest: String::new(),
                image: String::new(),
                generation,
                profile_volume: String::new(),
                started: true,
                state: "ready".into(),
                error: None,
                token: "private-helper-token".into(),
                activity_token: "private-activity-token".into(),
            }),
        }),
        lifecycle: Lifecycle::Active,
        generation,
        error: None,
        image: None,
        runtime_owned: true,
    };
    f.state.sessions.put(r);
    let origin = targets::origin("ide.example.test", &id, generation).unwrap();
    let binding = access::Binding {
        target: None,
        port: None,
        parent: cookie.split_once('=').unwrap().1.into(),
        actor: actor.driver,
        project: "demo".into(),
        session: id,
        generation,
        origin,
        destination: "/".into(),
    };
    (f, binding, cookie)
}
fn headers(origin: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("host", origin[8..].parse().unwrap());
    h.insert("origin", origin.parse().unwrap());
    h
}
#[tokio::test]
async fn one_use_handoff_validates_parent_generation_and_cookie() {
    let (f, binding, _) = fixture().await;
    let b = f.state.browser.inner().unwrap();
    let t = access::issue(&b, binding.clone()).unwrap();
    let r = access::consume(&f.state, &headers(&binding.origin), &t)
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let cookie = r.headers()["set-cookie"].to_str().unwrap();
    for flag in [
        "__Host-sigil_ide=",
        "Path=/",
        "Secure",
        "HttpOnly",
        "SameSite=Strict",
    ] {
        assert!(cookie.contains(flag));
    }
    assert!(!cookie.contains("Domain="));
    let v: Value =
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(v["generation"], "9007199254740993");
    assert!(!v.to_string().contains("private-"));
    assert!(access::consume(&f.state, &headers(&binding.origin), &t)
        .await
        .is_err());
    for mode in ["expiry", "generation", "actor", "logout", "origin"] {
        let (f, binding, _) = fixture().await;
        let b = f.state.browser.inner().unwrap();
        let t = access::issue(&b, binding.clone()).unwrap();
        let mut h = headers(&binding.origin);
        match mode {
            "expiry" => {
                b.gateway
                    .tickets
                    .lock()
                    .unwrap()
                    .get_mut(&t)
                    .unwrap()
                    .expires = 0
            }
            "generation" => {
                let mut r = f.state.sessions.record(&binding.session).unwrap();
                r.generation += 1;
                f.state.sessions.put(r);
            }
            "actor" => {
                b.gateway
                    .tickets
                    .lock()
                    .unwrap()
                    .get_mut(&t)
                    .unwrap()
                    .binding
                    .actor = "human:other".into()
            }
            "logout" => {
                b.store.lock().unwrap().sessions.remove(&binding.parent);
            }
            "origin" => {
                h.insert("origin", "https://preview.example.test".parse().unwrap());
            }
            _ => unreachable!(),
        }
        assert!(access::consume(&f.state, &h, &t).await.is_err(), "{mode}");
    }
}

#[tokio::test]
async fn human_allocation_keeps_reserved_identity_and_ignores_unrelated_orphan() {
    let repo = crate::merge::tests::mk_repo("human-allocation");
    crate::merge::git(&repo, &["branch", "session/unrelated-agent"]).unwrap();
    let state = crate::AppState {
        sessions: crate::sessions::SessionState::with_repos_dir(repo.parent().unwrap().into()),
        ..crate::AppState::test_without_runtime()
    };
    let actor = crate::auth::Actor {
        driver: "human:owner".into(),
        role: crate::auth::Role::Admin,
        approval: None,
    };
    let id = "b".repeat(32);
    let r = crate::sessions::open_human(
        actor,
        state.clone(),
        repo.file_name().unwrap().to_str().unwrap().into(),
        id.clone(),
    )
    .await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let value: Value =
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(
        value["session_id"], id,
        "durable identity must survive allocation"
    );
    assert_eq!(
        value["branch"],
        format!("session/{id}"),
        "human cannot adopt unrelated agent orphan"
    );
}

#[tokio::test]
async fn http_gateway_streams_known_target_and_strips_spoofed_authority() {
    let (f, binding, _) = fixture().await;
    let b = f.state.browser.inner().unwrap();
    let seen = Arc::new(Mutex::new(None));
    let captured = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    b.gateway
        .upstreams
        .lock()
        .unwrap()
        .insert(binding.session.clone(), listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(move |h: HeaderMap| {
                let seen = captured.clone();
                async move {
                    *seen.lock().unwrap() = Some(h);
                    (
                        [
                            ("content-type", "text/plain"),
                            ("set-cookie", "stolen=secret; Domain=.example.test"),
                            ("content-range", "bytes 0-4/5"),
                        ],
                        "hello",
                    )
                }
            }),
        )
        .await
        .unwrap()
    });
    let t = access::issue(&b, binding.clone()).unwrap();
    let r = access::consume(&f.state, &headers(&binding.origin), &t)
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let r = f
        .client
        .get(format!("{}/resource", f.base))
        .header("host", &binding.origin[8..])
        .header("cookie", format!("{cookie}; __Host-sigil_session=spoof"))
        .header("authorization", "Bearer attacker")
        .header("x-forwarded-host", "evil.test")
        .header("x-auth-user", "admin")
        .header("range", "bytes=0-4")
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "authenticated gateway must reach the registered helper"
    );
    assert!(!r.headers().contains_key("set-cookie"));
    assert_eq!(r.headers()["content-range"], "bytes 0-4/5");
    assert_eq!(r.text().await.unwrap(), "hello");
    let h = seen.lock().unwrap().take().unwrap();
    assert_eq!(h["authorization"], "Bearer private-helper-token");
    assert_eq!(h["host"], &binding.origin[8..]);
    for key in ["cookie", "x-auth-user", "x-forwarded-host"] {
        assert!(!h.contains_key(key));
    }
    server.abort();
}

#[tokio::test]
async fn launch_page_is_platform_owned_and_ticket_is_fragment_only() {
    let (f, binding, _) = fixture().await;
    let r = f
        .client
        .get(format!("{}/_sigil/launch", f.base))
        .header("host", &binding.origin[8..])
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "launch cannot require the not-yet-issued IDE cookie"
    );
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    assert!(r.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("default-src 'none'"));
    let html = r.text().await.unwrap();
    assert!(html.contains("/_sigil/launch.js"));
    assert!(!html.contains("private-"));
}

struct Abort(tokio::task::JoinHandle<()>);
impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[tokio::test]
async fn websocket_messages_and_lifecycle_revocation_use_actual_upgrades() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for mode in [
        "logout",
        "recycle",
        "close",
        "idle_expiry",
        "credential_expiry",
        "preview_logout",
        "absolute_expiry",
    ] {
        let (f, mut binding, _) = fixture().await;
        let b = f.state.browser.inner().unwrap();
        if mode == "preview_logout" {
            let mut d = f.state.registry.descriptor("demo");
            d.declaration.ide.preview_ports = vec![3000];
            f.state
                .registry
                .descriptors
                .write()
                .unwrap()
                .insert("demo".into(), d);
            binding.port = Some(3000);
            binding.origin = targets::preview_origin(
                "preview.example.test",
                &binding.session,
                binding.generation,
                3000,
            )
            .unwrap();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        b.gateway.upstreams.lock().unwrap().insert(
            binding
                .port
                .map(|p| format!("{}:{p}", binding.session))
                .unwrap_or(binding.session.clone()),
            listener.local_addr().unwrap(),
        );
        let _server = Abort(tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(|mut request: axum::extract::Request| async move {
                    assert_eq!(
                        request.headers()["origin"],
                        format!("https://{}", request.headers()["host"].to_str().unwrap())
                    );
                    let upgrade = hyper::upgrade::on(&mut request);
                    tokio::spawn(async move {
                        let upgraded = upgrade.await.unwrap();
                        let mut socket = hyper_util::rt::TokioIo::new(upgraded);
                        let mut frame = [0; 8];
                        socket.read_exact(&mut frame).await.unwrap();
                        assert_eq!(&frame[..2], &[0x81, 0x82]);
                        let message = [frame[6] ^ frame[2], frame[7] ^ frame[3]];
                        socket
                            .write_all(&[0x81, 2, message[0], message[1]])
                            .await
                            .unwrap();
                        let mut buf = [0; 16];
                        let _ = socket.read(&mut buf).await;
                    });
                    (
                        StatusCode::SWITCHING_PROTOCOLS,
                        [
                            ("connection", "upgrade"),
                            ("upgrade", "websocket"),
                            ("sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
                            ("sec-websocket-protocol", "sigil-test"),
                        ],
                    )
                        .into_response()
                }),
            )
            .await
            .unwrap()
        }));
        let ticket = access::issue(&b, binding.clone()).unwrap();
        let r = access::consume(&f.state, &headers(&binding.origin), &ticket)
            .await
            .unwrap();
        let cookie = r.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let mut socket = tokio::net::TcpStream::connect(f.base.trim_start_matches("http://"))
            .await
            .unwrap();
        socket.write_all(format!("GET /?reconnectionToken=synthetic HTTP/1.1\r\nHost: {}\r\nOrigin: {}\r\nCookie: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: sigil-test\r\n\r\n",&binding.origin[8..],binding.origin,cookie).as_bytes()).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !response.ends_with(b"\r\n\r\n") {
                response.push(socket.read_u8().await.unwrap());
                assert!(response.len() < 4096);
            }
        })
        .await
        .unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 101"), "{response}");
        assert!(response.contains("sigil-test"));
        socket
            .write_all(&[0x81, 0x82, 1, 2, 3, 4, b'h' ^ 1, b'i' ^ 2])
            .await
            .unwrap();
        let mut echoed = [0; 4];
        socket.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, &[0x81, 2, b'h', b'i']);
        match mode {
            "logout" | "preview_logout" => {
                b.store.lock().unwrap().sessions.remove(&binding.parent);
            }
            "recycle" => {
                let mut r = f.state.sessions.record(&binding.session).unwrap();
                r.generation += 1;
                f.state.sessions.put(r);
            }
            "close" => {
                f.state
                    .sessions
                    .mark(&binding.session, Lifecycle::Closing, None);
            }
            "idle_expiry" => {
                b.store.lock().unwrap().sessions[&binding.parent]
                    .last_seen
                    .store(0, std::sync::atomic::Ordering::Relaxed);
            }
            "absolute_expiry" => {
                let old = b.store.lock().unwrap().sessions[&binding.parent].clone();
                let credentials = old.credentials.lock().await;
                b.store.lock().unwrap().sessions.insert(
                    binding.parent.clone(),
                    Arc::new(Session {
                        origin: old.origin.clone(),
                        subject: old.subject.clone(),
                        nonce: old.nonce.clone(),
                        csrf: old.csrf.clone(),
                        absolute: 0,
                        last_seen: std::sync::atomic::AtomicU64::new(auth::now_epoch()),
                        credentials: tokio::sync::Mutex::new(Credentials {
                            actor: credentials.actor.clone(),
                            display_name: credentials.display_name.clone(),
                            access: credentials.access.clone(),
                            refresh: credentials.refresh.clone(),
                            expires: credentials.expires,
                        }),
                    }),
                );
            }
            "credential_expiry" => {
                let s = b.store.lock().unwrap().sessions[&binding.parent].clone();
                let mut c = s.credentials.lock().await;
                c.expires = 0;
                c.refresh = None;
            }
            _ => unreachable!(),
        }
        let mut byte = [0];
        let n = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut byte))
            .await
            .expect("revocation must promptly terminate established WS");
        assert!(n.is_err() || n.unwrap() == 0, "{mode}");
    }
}
#[tokio::test]
async fn sibling_origins_missing_csrf_and_unregistered_generations_fail_closed() {
    let (f, binding, _) = fixture().await;
    let b = f.state.browser.inner().unwrap();
    let t = access::issue(&b, binding.clone()).unwrap();
    let r = access::consume(&f.state, &headers(&binding.origin), &t)
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    for (path, origin) in [
        ("/", "https://evil.test"),
        ("/_sigil/activity", binding.origin.as_str()),
        ("/_sigil/operation", binding.origin.as_str()),
    ] {
        let r = f
            .client
            .post(format!("{}{path}", f.base))
            .header("host", &binding.origin[8..])
            .header("origin", origin)
            .header("cookie", cookie)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{path}");
    }
    let r = f
        .client
        .get(format!("{}/", f.base))
        .header("host", &binding.origin[8..])
        .header("sec-fetch-site", "same-site")
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = f
        .client
        .get(format!("{}/_sigil/launch", f.base))
        .header("host", format!("{}-g1.ide.example.test", binding.session))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn encoded_file_resource_has_download_sandbox_without_fetch_metadata() {
    let (f, binding, _) = fixture().await;
    let b = f.state.browser.inner().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    b.gateway
        .upstreams
        .lock()
        .unwrap()
        .insert(binding.session.clone(), listener.local_addr().unwrap());
    let _server = Abort(tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(|| async {
                (
                    [("content-type", "text/html")],
                    "<script>parent.compromised=true</script>",
                )
            }),
        )
        .await
        .unwrap()
    }));
    let t = access::issue(&b, binding.clone()).unwrap();
    let r = access::consume(&f.state, &headers(&binding.origin), &t)
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    for path in [
        "/vscode-remote-resource?path=/workspace/index.html",
        "/vscode-%72emote-resource?path=/workspace/index.html",
        "/commit/vscode-remote-resource?path=/workspace/index.html",
    ] {
        let r = f
            .client
            .get(format!("{}{path}", f.base))
            .header("host", &binding.origin[8..])
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers()
                .get("content-disposition")
                .and_then(|h| h.to_str().ok()),
            Some("attachment"),
            "{path}"
        );
        assert!(r.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox"));
    }
}

#[tokio::test]
#[ignore = "requires verified pinned provider, target-only Playwright and native libraries"]
async fn pinned_provider_gateway_browser_smoke() {
    // This fixture owns its exact child/group and output files. Avoid routing
    // browser verification through the independently qualified repository scanner.
    struct Owned(Option<std::process::Child>);
    impl Owned {
        fn start(c: &mut std::process::Command) -> Self {
            use std::os::unix::process::CommandExt;
            Self(Some(
                c.process_group(0)
                    .stdin(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            ))
        }
        fn stop(&mut self) {
            if let Some(mut child) = self.0.take() {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                loop {
                    if child.try_wait().unwrap().is_some() {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "owned child termination not confirmed"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            self.stop();
        }
    }
    use std::process::{Command, Stdio};
    let provider =
        std::env::var("SIGIL_TEST_PROVIDER").expect("explicit checksum verified provider path");
    assert_ne!(unsafe { libc::geteuid() }, 0);
    for port in [8090, 8091] {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("test ports must be unused; never stop unrelated listeners");
        drop(listener);
    }
    let root = std::path::PathBuf::from(format!(
        "/workspace/target/gateway-provider-smoke-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data/User")).unwrap();
    std::fs::write(root.join("data/User/settings.json"),r#"{"security.workspace.trust.enabled":false,"workbench.startupEditor":"none","telemetry.telemetryLevel":"off"}"#).unwrap();
    std::fs::write(
        root.join("navigation.txt"),
        "First line\nSecond navigation line\nThird line\n",
    )
    .unwrap();
    std::fs::write(
        root.join("project.html"),
        "<script>window.compromised=true</script>",
    )
    .unwrap();
    let mut command = Command::new(provider);
    command
        .args([
            "--config",
            "/dev/null",
            "--auth",
            "none",
            "--disable-proxy",
            "--disable-telemetry",
            "--bind-addr",
            "127.0.0.1:8091",
            "--user-data-dir",
        ])
        .arg(root.join("data"))
        .arg("--extensions-dir")
        .arg(root.join("extensions"))
        .arg("/workspace")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("xdg-data"))
        .env("TMPDIR", "/workspace/target/gateway-tmp")
        .stdout(Stdio::from(
            std::fs::File::create(root.join("provider.log")).unwrap(),
        ))
        .stderr(Stdio::from(
            std::fs::File::create(root.join("provider-error.log")).unwrap(),
        ));
    let mut provider = Owned::start(&mut command);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let mut ready = false;
    for _ in 0..40 {
        if http
            .get("http://127.0.0.1:8091/healthz")
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(ready, "bounded provider startup");
    let (f, mut binding, _) = fixture().await;
    let b = f.state.browser.inner().unwrap();
    let helper_token = "private-helper-token".repeat(3);
    let mut record = f.state.sessions.record(&binding.session).unwrap();
    record.binding.as_mut().unwrap().ide.as_mut().unwrap().token = helper_token.clone();
    f.state.sessions.put(record.clone());
    let config = root.join("helper.json");
    std::fs::write(&config,json!({"token":helper_token,"activity_token":"synthetic-activity-token".repeat(3),"generation":binding.generation,"session":binding.session,"branch":record.branch,"remote":"synthetic-unused","profile":"unused"}).to_string()).unwrap();
    let mut command = Command::new("/workspace/target/debug/sigil-ide-agent");
    command
        .arg(&config)
        .env("TMPDIR", "/workspace/target/gateway-tmp")
        .stdout(Stdio::from(
            std::fs::File::create(root.join("helper.log")).unwrap(),
        ))
        .stderr(Stdio::from(
            std::fs::File::create(root.join("helper-error.log")).unwrap(),
        ));
    let mut helper = Owned::start(&mut command);
    let mut ready = false;
    for _ in 0..40 {
        if http
            .get("http://127.0.0.1:8090/status")
            .bearer_auth(&helper_token)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "bounded helper startup");
    b.gateway
        .upstreams
        .lock()
        .unwrap()
        .insert(binding.session.clone(), "127.0.0.1:8090".parse().unwrap());
    let target = targets::FileTarget {
        path: format!(
            "{}/navigation.txt",
            root.strip_prefix("/workspace")
                .unwrap()
                .to_str()
                .unwrap()
                .trim_start_matches('/')
        ),
        line: Some(2),
        column: Some(3),
    };
    targets::probe(&f.state, &record, &target).await.unwrap();
    binding.destination = targets::destination(&binding.origin, Some(&target)).unwrap();
    binding.target = Some(target);
    let mut probes = Vec::new();
    for (name, url) in [
        ("direct", "http://127.0.0.1:8091/?folder=%2Fworkspace"),
        (
            "helper",
            "http://127.0.0.1:8090/editor/?folder=%2Fworkspace",
        ),
        (
            "helper-no-slash",
            "http://127.0.0.1:8090/editor?folder=%2Fworkspace",
        ),
    ] {
        let response = http
            .get(url)
            .header("host", &binding.origin[8..])
            .header("origin", &binding.origin)
            .header("x-forwarded-proto", "https")
            .bearer_auth(&helper_token)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let headers = format!("{:?}", response.headers());
        let body = response.text().await.unwrap();
        probes.push(json!({"name":name,"status":status.as_u16(),"headers":headers,"body":body.chars().take(4000).collect::<String>()}));
    }
    std::fs::write(
        root.join("http-probes.json"),
        serde_json::to_vec_pretty(&probes).unwrap(),
    )
    .unwrap();
    assert_eq!(
        probes[0]["status"],
        200,
        "direct provider must serve workbench; see {}",
        root.display()
    );
    assert_eq!(
        probes[1]["status"],
        200,
        "actual helper must serve provider; see {}",
        root.display()
    );
    assert_eq!(
        probes[2]["status"], 200,
        "non-trailing helper root must preserve query"
    );
    for probe in &probes {
        assert!(
            probe["body"].as_str().unwrap().contains("workbench"),
            "provider HTML expected"
        );
    }
    let mut descriptor = f.state.registry.descriptor("demo");
    descriptor.declaration.ide.preview_ports = vec![3000];
    f.state
        .registry
        .descriptors
        .write()
        .unwrap()
        .insert("demo".into(), descriptor);
    let preview_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let preview_addr = preview_listener.local_addr().unwrap();
    b.gateway
        .upstreams
        .lock()
        .unwrap()
        .insert(format!("{}:3000", binding.session), preview_addr);
    let preview_server = Abort(tokio::spawn(async move {
        axum::serve(preview_listener,Router::new().fallback(|h:HeaderMap|async move{assert!(!h.contains_key("authorization"));assert!(!h.contains_key("cookie"));([( "content-type","text/html"),("set-cookie","attack=1; Domain=.example.test")],"<!doctype html><title>Isolated project preview</title><h1>Isolated project preview</h1>")})).await.unwrap()
    }));
    let ticket = access::issue(&b, binding.clone()).unwrap();
    let mut command = Command::new("/workspace/target/ide-provider/lib/node");
    command
        .arg("/workspace/sigiledd/tests/ide-provider-browser.cjs")
        .env("GATEWAY_SMOKE_ROOT", &root)
        .env("GATEWAY_SMOKE_UPSTREAM", &f.base)
        .env(
            "GATEWAY_SMOKE_URL",
            format!("{}/_sigil/launch#{ticket}", binding.origin),
        )
        .env(
            "LD_LIBRARY_PATH",
            "/workspace/target/gateway-browser/sysroot/usr/lib/x86_64-linux-gnu",
        )
        .env(
            "FONTCONFIG_FILE",
            "/workspace/target/gateway-browser/fonts.conf",
        )
        .env(
            "PLAYWRIGHT_BROWSERS_PATH",
            "/workspace/target/gateway-browser/cache",
        )
        .env("TMPDIR", "/workspace/target/gateway-tmp");
    command
        .stdout(Stdio::from(
            std::fs::File::create(root.join("browser-stdout.log")).unwrap(),
        ))
        .stderr(Stdio::from(
            std::fs::File::create(root.join("browser-stderr.log")).unwrap(),
        ));
    let mut node = Owned::start(&mut command);
    let deadline = std::time::Instant::now() + Duration::from_secs(100);
    // WNOWAIT keeps ownership of the child PID/group until final cleanup.
    let result = loop {
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                node.0.as_ref().unwrap().id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        assert_eq!(rc, 0, "fixture child ownership");
        if unsafe { info.si_pid() } != 0 {
            break Some(unsafe { info.si_status() });
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    node.stop();
    drop(preview_server);
    tokio::task::yield_now().await;
    assert!(tokio::net::TcpStream::connect(preview_addr).await.is_err());
    helper.stop();
    provider.stop();
    for port in [8090, 8091] {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owned listener cleanup: {port}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    std::fs::write(root.join("fixture-cleanup.json"),serde_json::to_vec_pretty(&json!({"node_exit":result,"helper_provider_ports_closed":true,"preview_port_closed":true,"browser_cleanup":serde_json::from_str::<Value>(&std::fs::read_to_string(root.join("browser-cleanup.json")).unwrap()).unwrap()})).unwrap()).unwrap();
    assert_eq!(
        result,
        Some(0),
        "bounded browser fixture failed; stdout={} stderr={}",
        std::fs::read_to_string(root.join("browser-stdout.log")).unwrap(),
        std::fs::read_to_string(root.join("browser-stderr.log")).unwrap()
    );
}

#[tokio::test]
async fn preview_grant_has_no_credentials_controls_or_login_activity() {
    let (f, mut binding, _) = fixture().await;
    let b = f.state.browser.inner().unwrap();
    let mut descriptor = f.state.registry.descriptor("demo");
    descriptor.declaration.ide.preview_ports = vec![3000];
    f.state
        .registry
        .descriptors
        .write()
        .unwrap()
        .insert("demo".into(), descriptor);
    binding.port = Some(3000);
    binding.origin = targets::preview_origin(
        "preview.example.test",
        &binding.session,
        binding.generation,
        3000,
    )
    .unwrap();
    let before = auth::now_epoch() - 30;
    b.store.lock().unwrap().sessions[&binding.parent]
        .last_seen
        .store(before, std::sync::atomic::Ordering::Relaxed);
    let captured = Arc::new(Mutex::new(None));
    let seen = captured.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    b.gateway.upstreams.lock().unwrap().insert(
        format!("{}:3000", binding.session),
        listener.local_addr().unwrap(),
    );
    let _server = Abort(tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(move |h: HeaderMap| {
                let seen = seen.clone();
                async move {
                    *seen.lock().unwrap() = Some(h);
                    (
                        [
                            ("content-type", "text/html"),
                            ("set-cookie", "attack=1; Domain=.example.test"),
                        ],
                        "<h1>Project preview</h1>",
                    )
                }
            }),
        )
        .await
        .unwrap()
    }));
    let ticket = access::issue(&b, binding.clone()).unwrap();
    let response = access::consume(&f.state, &headers(&binding.origin), &ticket)
        .await
        .unwrap();
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(cookie.starts_with("__Host-sigil_preview="));
    let value: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(value.get("csrf_token").is_none());
    let response = f
        .client
        .get(format!("{}/", f.base))
        .header("host", &binding.origin[8..])
        .header("cookie", &cookie)
        .header("authorization", "Bearer attacker")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("set-cookie"));
    assert!(response.text().await.unwrap().contains("Project preview"));
    let h = captured.lock().unwrap().take().unwrap();
    for key in ["authorization", "cookie"] {
        assert!(!h.contains_key(key));
    }
    for path in ["/_sigil/session", "/_sigil/activity", "/_sigil/operation"] {
        let response = f
            .client
            .get(format!("{}{path}", f.base))
            .header("host", &binding.origin[8..])
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(
        b.store.lock().unwrap().sessions[&binding.parent]
            .last_seen
            .load(std::sync::atomic::Ordering::Relaxed),
        before
    );
    let ide_origin =
        targets::origin("ide.example.test", &binding.session, binding.generation).unwrap();
    let response = f
        .client
        .get(format!("{}/", f.base))
        .header("host", &ide_origin[8..])
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn durable_launch_intent_survives_response_loss_restart_and_close() {
    let (f, binding, _) = fixture().await;
    let launch = || async {
        let c = resolve_session(&f.state, &binding.parent, true)
            .await
            .unwrap();
        allocation::launch(
            c,
            State(f.state.clone()),
            Path("demo".into()),
            Json(allocation::Launch {
                idempotency_key: "durable-operation".into(),
                target: None,
            }),
        )
        .await
        .unwrap()
    };
    let (a, b) = tokio::join!(launch(), launch());
    for r in [a, b] {
        assert_eq!(r.status(), StatusCode::OK);
        let v: Value =
            serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 4096).await.unwrap())
                .unwrap();
        assert_eq!(v["session_id"], binding.session);
        assert_eq!(v["generation"], binding.generation.to_string());
    }
    assert_eq!(f.state.sessions.dump_records().len(), 1);
    f.state.try_persist().unwrap();
    f.state.sessions.browser_allocations.lock().unwrap().clear();
    let browser = f.state.browser.inner().unwrap();
    browser.gateway.grants.lock().unwrap().clear();
    browser.gateway.tickets.lock().unwrap().clear();
    f.state.hydrate_from_disk();
    assert_eq!(launch().await.status(), StatusCode::OK);
    f.state.sessions.remove_record(&binding.session);
    f.state.try_persist().unwrap();
    f.state.hydrate_from_disk();
    let response = launch().await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(f.state.sessions.dump_records().is_empty());
}
#[tokio::test]
async fn reserved_allocation_gap_and_merge_debt_do_not_allocate_again() {
    let (f, binding, _) = fixture().await;
    f.state.sessions.remove_record(&binding.session);
    let digest = allocation::key(&binding.actor, "demo", "reserved-operation");
    f.state
        .sessions
        .browser_allocations
        .lock()
        .unwrap()
        .insert(digest, binding.session.clone());
    f.state.try_persist().unwrap();
    f.state.hydrate_from_disk();
    let c = resolve_session(&f.state, &binding.parent, true)
        .await
        .unwrap();
    let response = allocation::launch(
        c,
        State(f.state.clone()),
        Path("demo".into()),
        Json(allocation::Launch {
            idempotency_key: "reserved-operation".into(),
            target: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(f.state.sessions.dump_records().is_empty());
    let debt:crate::merge::MergeDebt=serde_json::from_value(json!({"branch":"session/debtor","conflicted_files":["file"],"ours":{"sha":"a".repeat(40),"commit_messages":[]},"theirs":{"sha":"b".repeat(40),"commit_messages":[]},"since":"2026-09-10"})).unwrap();
    f.state
        .sessions
        .hydrate(HashMap::from([("demo".into(), vec![debt])]), HashMap::new());
    let c = resolve_session(&f.state, &binding.parent, true)
        .await
        .unwrap();
    let error = allocation::launch(
        c,
        State(f.state.clone()),
        Path("demo".into()),
        Json(allocation::Launch {
            idempotency_key: "new-operation".into(),
            target: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.1, "merge_debt_requires_resolution");
    assert_eq!(
        f.state.sessions.browser_allocations.lock().unwrap().len(),
        1
    );
    assert!(f.state.sessions.dump_records().is_empty());
}

#[tokio::test]
async fn deliberate_finish_retries_failed_close_through_a1_authority() {
    let (f, binding, _) = fixture().await;
    let mut state = f.state.clone();
    let repo = crate::merge::tests::mk_repo("browser-finish-retry");
    state.sessions = crate::sessions::SessionState::with_repos_dir(repo.parent().unwrap().into());
    let project = repo.file_name().unwrap().to_str().unwrap().to_owned();
    let c = resolve_session(&state, &binding.parent, true)
        .await
        .unwrap();
    let id = "c".repeat(32);
    let response = crate::sessions::open_human(c.actor, state.clone(), project, id.clone()).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    state.sessions.mark(
        &id,
        Lifecycle::Failed,
        Some(crate::sessions::Failure::PushFailed),
    );
    let c = resolve_session(&state, &binding.parent, true)
        .await
        .unwrap();
    let response = controls::operation(
        c,
        State(state.clone()),
        Path(id.clone()),
        Json(controls::Operation {
            generation: "1".into(),
            action: "finish".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "deliberate recovery must invoke the normal close durability authority"
    );
    assert!(state.sessions.record(&id).is_none());
}

#[tokio::test]
async fn workbench_has_isolated_preview_control_and_frame_boundary() {
    let (f, binding, _) = fixture().await;
    let b = f.state.browser.inner().unwrap();
    let mut d = f.state.registry.descriptor("demo");
    d.declaration.ide.preview_ports = vec![3000];
    f.state
        .registry
        .descriptors
        .write()
        .unwrap()
        .insert("demo".into(), d);
    let t = access::issue(&b, binding.clone()).unwrap();
    let r = access::consume(&f.state, &headers(&binding.origin), &t)
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let get = |path: &str| {
        f.client
            .get(format!("{}{path}", f.base))
            .header("host", &binding.origin[8..])
            .header("cookie", &cookie)
    };
    let v: Value = get("/_sigil/session")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["preview_ports"], json!([3000]));
    assert!(get("/_sigil/workbench")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
        .contains("id=\"preview\""));
    let r = f
        .client
        .post(format!("{}/_sigil/preview", f.base))
        .header("host", &binding.origin[8..])
        .header("origin", &binding.origin)
        .header("cookie", &cookie)
        .header("x-sigil-csrf", v["csrf_token"].as_str().unwrap())
        .json(&json!({"generation":binding.generation.to_string(),"port":3000}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    b.gateway
        .upstreams
        .lock()
        .unwrap()
        .insert(binding.session.clone(), listener.local_addr().unwrap());
    let _server = Abort(tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(|| async {
                (
                    [(
                        "content-security-policy",
                        "script-src 'self'; worker-src blob:",
                    )],
                    "provider",
                )
            }),
        )
        .await
        .unwrap()
    }));
    let r = get("/").send().await.unwrap();
    let csp = r
        .headers()
        .get_all("content-security-policy")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect::<Vec<_>>()
        .join(";");
    assert!(csp.contains("worker-src blob:"));
    assert!(csp.contains("frame-ancestors 'self'"));
}

#[tokio::test]
async fn human_allocation_rechecks_debt_after_waiting_for_project_lock() {
    let repo = crate::merge::tests::mk_repo("allocation-debt-lock");
    let project = repo.file_name().unwrap().to_str().unwrap().to_owned();
    let state = crate::AppState {
        sessions: crate::sessions::SessionState::with_repos_dir(repo.parent().unwrap().into()),
        ..crate::AppState::test_without_runtime()
    };
    let lock = state.sessions.merge_lock(&project);
    let guard = lock.lock_owned().await;
    let task = tokio::spawn(crate::sessions::open_human(
        crate::auth::Actor {
            driver: "human:owner".into(),
            role: crate::auth::Role::Admin,
            approval: None,
        },
        state.clone(),
        project.clone(),
        "d".repeat(32),
    ));
    tokio::task::yield_now().await;
    let debt:crate::merge::MergeDebt=serde_json::from_value(json!({"branch":"session/debtor","conflicted_files":["file"],"ours":{"sha":"a".repeat(40),"commit_messages":[]},"theirs":{"sha":"b".repeat(40),"commit_messages":[]},"since":"2026-09-10"})).unwrap();
    state
        .sessions
        .hydrate(HashMap::from([(project, vec![debt])]), HashMap::new());
    drop(guard);
    assert_eq!(task.await.unwrap().status(), StatusCode::CONFLICT);
    assert!(state.sessions.dump_records().is_empty());
}

#[tokio::test]
async fn workspace_agent_port_is_never_a_preview_even_with_invalid_registry_data() {
    let mut declaration = crate::declaration::Declaration::default();
    declaration.ide.preview_ports = vec![8000];
    assert!(
        declaration.validate(None).is_err(),
        "workspace agent port must be rejected by declaration validation"
    );
    assert!(targets::preview_origin("preview.example.test", &"a".repeat(32), 1, 8000).is_err());
    let (f, mut binding, _) = fixture().await;
    let mut d = f.state.registry.descriptor("demo");
    d.declaration.ide.preview_ports = vec![8000];
    f.state
        .registry
        .descriptors
        .write()
        .unwrap()
        .insert("demo".into(), d);
    binding.port = Some(8000);
    binding.origin = format!(
        "https://{}-g{}-p8000.preview.example.test",
        binding.session, binding.generation
    );
    assert!(
        access::record(&f.state, &binding).is_err(),
        "runtime grant check must reject invalid persisted/mutated declarations before forwarding"
    );
}
