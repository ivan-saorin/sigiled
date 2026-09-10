use super::*;
use axum::body::Body;
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
const UPLOAD_LIMIT: usize = 64 * 1024 * 1024;
fn stream(body: Body, limit: Option<usize>) -> Body {
    Body::from_stream(futures_util::stream::unfold(
        (body, 0usize, false),
        move |(mut body, mut size, done)| async move {
            if done {
                return None;
            }
            loop {
                match tokio::time::timeout(Duration::from_secs(30), body.frame()).await {
                    Ok(Some(Ok(frame))) => {
                        if let Ok(data) = frame.into_data() {
                            size = size.saturating_add(data.len());
                            if limit.is_some_and(|n| size > n) {
                                return Some((
                                    Err(std::io::Error::other("editor upload limit")),
                                    (body, size, true),
                                ));
                            }
                            return Some((Ok(data), (body, size, false)));
                        }
                    }
                    Ok(None) => return None,
                    _ => {
                        return Some((
                            Err(std::io::Error::other("editor stream unavailable")),
                            (body, size, true),
                        ))
                    }
                }
            }
        },
    ))
}
fn request_headers(source: &HeaderMap, origin: &str, token: &str, websocket: bool) -> HeaderMap {
    let mut h = HeaderMap::new();
    // Allowlisting also strips Connection-nominated fields, forwarding/identity headers,
    // all browser cookies and caller Authorization. Metadata below is server constructed.
    let nominated: Vec<_> = source
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|v| v.trim().to_ascii_lowercase())
        .collect();
    for key in [
        "accept",
        "accept-encoding",
        "accept-language",
        "content-type",
        "content-length",
        "range",
        "if-range",
        "if-none-match",
        "if-modified-since",
        "user-agent",
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-protocol",
        "sec-websocket-extensions",
    ] {
        if !nominated.iter().any(|v| v == key) {
            for value in source.get_all(key) {
                h.append(key, value.clone());
            }
        }
    }
    h.insert("host", origin[8..].parse().unwrap());
    h.insert("origin", origin.parse().unwrap());
    h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    h.insert("authorization", format!("Bearer {token}").parse().unwrap());
    if websocket {
        h.insert("connection", HeaderValue::from_static("upgrade"));
        h.insert("upgrade", HeaderValue::from_static("websocket"));
    }
    h
}
pub(super) async fn forward(
    state: &AppState,
    grant: &access::Grant,
    mut request: axum::extract::Request,
) -> Result<Response, Error> {
    let r = access::record(state, &grant.binding)?;
    let websocket =
        single(request.headers(), "upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if request.headers().contains_key("upgrade") && !websocket {
        return Err(Error(StatusCode::BAD_REQUEST, "unsupported_upgrade"));
    }
    if websocket {
        access::exact_origin(request.headers(), &grant.binding.origin, true)?;
    }
    if request.uri().to_string().len() > 8192
        || request
            .headers()
            .iter()
            .map(|(k, v)| k.as_str().len() + v.len())
            .sum::<usize>()
            > 32768
    {
        return Err(Error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "editor_request_too_large",
        ));
    }
    if request.headers().get("content-length").is_some_and(|v| {
        v.to_str()
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .is_none_or(|n| n > UPLOAD_LIMIT)
    }) {
        return Err(Error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "editor_upload_too_large",
        ));
    }
    // The pinned provider is also launched with --disable-proxy. This deny is defense in depth.
    let preview = grant.binding.port.is_some();
    let canonical = canonical_path(request.uri().path())?;
    let file_resource = canonical.split('/').any(|v| v == "vscode-remote-resource");
    if !preview
        && file_resource
        && single(request.headers(), "sec-fetch-dest")
            .is_some_and(|v| matches!(v, "document" | "iframe" | "frame"))
    {
        return Err(Error::forbidden("project_resource_navigation_denied"));
    }
    let path = canonical.as_str();
    if !preview && (path.starts_with("/proxy") || path.starts_with("/absproxy")) {
        return Err(Error(StatusCode::CONFLICT, "isolated_preview_required"));
    }
    let browser_upgrade = websocket.then(|| hyper::upgrade::on(&mut request));
    let path = request
        .uri()
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or("/")
        .to_owned();
    let token = &r.binding.as_ref().unwrap().ide.as_ref().unwrap().token;
    let mut headers = request_headers(request.headers(), &grant.binding.origin, token, websocket);
    if preview {
        headers.remove("authorization");
    }
    *request.headers_mut() = headers;
    *request.uri_mut() = if preview {
        path.clone()
    } else {
        format!("/editor{path}")
    }
    .parse()
    .map_err(|_| Error(StatusCode::BAD_REQUEST, "invalid_editor_path"))?;
    let (parts, body) = request.into_parts();
    let request = axum::http::Request::from_parts(parts, stream(body, Some(UPLOAD_LIMIT)));
    let target = format!("{}:{}", r.container(), grant.binding.port.unwrap_or(8090));
    #[cfg(test)]
    let target = state
        .browser
        .inner()?
        .gateway
        .upstreams
        .lock()
        .unwrap()
        .get(
            &grant
                .binding
                .port
                .map(|p| format!("{}:{p}", r.session_id))
                .unwrap_or(r.session_id.clone()),
        )
        .map(ToString::to_string)
        .unwrap_or(target);
    let socket = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(target),
    )
    .await
    .map_err(|_| Error(StatusCode::GATEWAY_TIMEOUT, "editor_connect_timeout"))?
    .map_err(|_| Error(StatusCode::BAD_GATEWAY, "editor_unavailable_retry_open"))?;
    let (mut sender, connection) = tokio::time::timeout(
        Duration::from_secs(3),
        hyper::client::conn::http1::handshake(TokioIo::new(socket)),
    )
    .await
    .map_err(|_| Error(StatusCode::GATEWAY_TIMEOUT, "editor_connect_timeout"))?
    .map_err(|_| Error(StatusCode::BAD_GATEWAY, "editor_unavailable_retry_open"))?;
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let mut response = tokio::time::timeout(Duration::from_secs(30), sender.send_request(request))
        .await
        .map_err(|_| Error(StatusCode::GATEWAY_TIMEOUT, "editor_response_timeout"))?
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "editor_stream_failed"))?;
    if !access::live(state, &grant.binding) {
        return Err(Error::login());
    }
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let Some(browser) = browser_upgrade else {
            return Err(Error(StatusCode::BAD_GATEWAY, "unexpected_editor_upgrade"));
        };
        let upstream = hyper::upgrade::on(&mut response);
        let state = state.clone();
        let binding = grant.binding.clone();
        tokio::spawn(async move {
            if let Ok((Ok(browser), Ok(upstream))) =
                tokio::time::timeout(Duration::from_secs(5), async {
                    tokio::join!(browser, upstream)
                })
                .await
            {
                let mut browser = TokioIo::new(browser);
                let mut upstream = TokioIo::new(upstream);
                tokio::select! {
                    _=tokio::io::copy_bidirectional(&mut browser,&mut upstream)=>{},
                    _=async {loop {tokio::time::sleep(Duration::from_millis(250)).await;if !access::live(&state,&binding){break;}}}=>{},
                    _=async {loop {tokio::time::sleep(Duration::from_secs(1)).await;if access::authorize(&state,&binding,false).await.is_err(){break;}}}=>{}
                }
            }
        });
    } else if response.status().is_client_error() || response.status().is_server_error() {
        return Err(Error(response.status(), "editor_upstream_error"));
    }
    if let Some(location) = response.headers().get("location") {
        let value = location
            .to_str()
            .map_err(|_| Error(StatusCode::BAD_GATEWAY, "invalid_editor_redirect"))?;
        if value.contains('\\')
            || reqwest::Url::parse(&grant.binding.origin)
                .ok()
                .and_then(|base| base.join(value).ok())
                .is_none_or(|target| target.origin().ascii_serialization() != grant.binding.origin)
        {
            return Err(Error(StatusCode::BAD_GATEWAY, "invalid_editor_redirect"));
        }
    }
    let nominated: Vec<String> = response
        .headers()
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|v| v.trim().to_ascii_lowercase())
        .collect();
    for key in nominated {
        if key != "upgrade" {
            response.headers_mut().remove(key);
        }
    }
    let names: Vec<_> = response
        .headers()
        .keys()
        .filter(|k| k.as_str().starts_with("access-control-") || k.as_str().starts_with("x-auth-"))
        .cloned()
        .collect();
    for key in names {
        response.headers_mut().remove(key);
    }
    for key in [
        "set-cookie",
        "authorization",
        "proxy-authenticate",
        "proxy-authorization",
        "keep-alive",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        response.headers_mut().remove(key);
    }
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        response.headers_mut().remove("connection");
        response.headers_mut().remove("upgrade");
    }
    if !preview && file_resource {
        response.headers_mut().insert(
            "content-security-policy",
            HeaderValue::from_static("sandbox; default-src 'none'"),
        );
        response.headers_mut().insert(
            "content-disposition",
            HeaderValue::from_static("attachment"),
        );
    }
    if !preview {
        response.headers_mut().append(
            "content-security-policy",
            HeaderValue::from_static("frame-ancestors 'self'"),
        );
    }
    if preview {
        response.headers_mut().insert(
            "content-security-policy",
            HeaderValue::from_static("frame-ancestors 'none'; base-uri 'self'"),
        );
    }
    Ok(response.map(|body| stream(Body::new(body), None)))
}

fn canonical_path(raw: &str) -> Result<String, Error> {
    let fail = || Error(StatusCode::BAD_REQUEST, "ambiguous_editor_path");
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(fail());
            }
            let s = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|_| fail())?;
            out.push(u8::from_str_radix(s, 16).map_err(|_| fail())?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(out).map_err(|_| fail())?;
    if path.contains(['%', '\\'])
        || path.chars().any(char::is_control)
        || path.split('/').any(|v| v == "." || v == "..")
    {
        return Err(fail());
    }
    Ok(path)
}
