use super::*;
#[path = "../../../../shared/file_target.rs"]
mod file_target;
pub(super) use file_target::FileTarget;
pub(super) fn generation(raw: &str) -> Result<u64, Error> {
    let parsed = raw.parse::<u64>().ok().filter(|n| n.to_string() == raw);
    parsed.ok_or(Error(StatusCode::BAD_REQUEST, "invalid_generation"))
}
pub(super) fn domain(raw: &str) -> bool {
    raw.parse::<std::net::IpAddr>().is_err()
        && raw.len() <= 190
        && raw.split('.').count() >= 3
        && raw.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}
pub(super) fn origin(domain: &str, id: &str, g: u64) -> Result<String, Error> {
    if !self::domain(domain)
        || id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_identity"));
    }
    Ok(format!("https://{id}-g{g}.{domain}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_generation_and_dns_identity() {
        assert_eq!(generation("18446744073709551615").unwrap(), u64::MAX);
        assert_eq!(generation("9007199254740993").unwrap(), 9007199254740993);
        for s in ["", "01", "+1", "-1", "18446744073709551616", "1.0", " 1"] {
            assert!(generation(s).is_err());
        }
        let id = "a".repeat(32);
        let o = origin("ide.example.test", &id, u64::MAX).unwrap();
        assert_eq!(
            o,
            format!("https://{id}-g18446744073709551615.ide.example.test")
        );
        assert!(o[8..].split('.').all(|l| l.len() <= 63));
        for id in ["other.project", "/evil", "UPPER", "a-g1"] {
            assert!(origin("ide.example.test", id, 1).is_err());
        }
    }
}

pub(super) fn preview_origin(domain: &str, id: &str, g: u64, port: u16) -> Result<String, Error> {
    let _ = origin(domain, id, g)?;
    if !crate::declaration::preview_port(port) {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_preview_port"));
    }
    Ok(format!("https://{id}-g{g}-p{port}.{domain}"))
}
#[cfg(test)]
mod preview_tests {
    use super::*;
    #[test]
    fn preview_port_and_generation_origin_are_explicit() {
        let id = "a".repeat(32);
        let origin = preview_origin("preview.example.test", &id, u64::MAX, 65535).unwrap();
        assert!(origin[8..].split('.').all(|s| s.len() <= 63));
        for port in [0, 22, 2375, 2376, 8000, 8080, 8090, 8091] {
            assert!(preview_origin("preview.example.test", &id, 1, port).is_err());
        }
    }
}

pub(super) fn destination(origin: &str, target: Option<&FileTarget>) -> Result<String, Error> {
    let mut url = reqwest::Url::parse(origin)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, "invalid_origin"))?;
    url.query_pairs_mut().append_pair("folder", "/workspace");
    if let Some(target) = target {
        if !target.validate() {
            return Err(Error(StatusCode::BAD_REQUEST, "invalid_file_target"));
        }
        let mut file = reqwest::Url::parse(&format!("vscode-remote://{}", &origin[8..]))
            .map_err(|_| Error(StatusCode::BAD_REQUEST, "invalid_file_target"))?;
        let mut path = format!("/workspace/{}", target.path);
        if let Some(line) = target.line {
            path.push_str(&format!(":{line}"));
            if let Some(column) = target.column {
                path.push_str(&format!(":{column}"));
            }
        }
        file.set_path(&path);
        let mut payload = vec![];
        if target.line.is_some() {
            payload.push(["gotoLineMode", "true"]);
        }
        payload.push(["openFile", file.as_str()]);
        url.query_pairs_mut()
            .append_pair("payload", &serde_json::to_string(&payload).unwrap());
    }
    Ok(format!("/?{}", url.query().unwrap()))
}
pub(super) async fn probe(
    state: &AppState,
    r: &SessionRecord,
    target: &FileTarget,
) -> Result<(), Error> {
    if !target.validate() {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_file_target"));
    }
    let _guard = state
        .sessions
        .session_lock(&r.session_id)
        .lock_owned()
        .await;
    if !state
        .sessions
        .record(&r.session_id)
        .is_some_and(|v| v.generation == r.generation && v.lifecycle == Lifecycle::Active)
    {
        return Err(Error(StatusCode::CONFLICT, "generation_changed"));
    }
    let token = &r
        .binding
        .as_ref()
        .and_then(|v| v.ide.as_ref())
        .ok_or(Error(StatusCode::CONFLICT, "provider_image_required"))?
        .token;
    let target_url = format!("http://{}:8090/file-target", r.container());
    #[cfg(test)]
    let target_url = state
        .browser
        .inner()?
        .gateway
        .upstreams
        .lock()
        .unwrap()
        .get(&r.session_id)
        .map(|addr| format!("http://{addr}/file-target"))
        .unwrap_or(target_url);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "file_probe_unavailable"))?;
    let response = client
        .post(target_url)
        .bearer_auth(token)
        .json(&serde_json::json!({"generation":r.generation.to_string(),"target":target}))
        .send()
        .await
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "file_probe_unavailable"))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(Error(StatusCode::NOT_FOUND, "source_missing_or_moved"));
    }
    if !response.status().is_success() {
        return Err(Error(StatusCode::CONFLICT, "file_probe_not_supported"));
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "file_probe_unavailable"))?
    {
        if body.len() + chunk.len() > 4096 {
            return Err(Error(StatusCode::BAD_GATEWAY, "invalid_file_probe"));
        }
        body.extend_from_slice(&chunk);
    }
    let v: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "invalid_file_probe"))?;
    let expected_generation = r.generation.to_string();
    if v["exists"] != true || v["generation"].as_str() != Some(expected_generation.as_str()) {
        return Err(Error(StatusCode::CONFLICT, "source_missing_or_moved"));
    }
    Ok(())
}
