//! Explicit, fixed provider and site configuration. Never derive endpoints from requests.
use reqwest::Url;
use std::collections::HashMap;

#[derive(Clone)]
pub struct Config {
    pub origins: Vec<String>,
    pub issuer: String,
    pub authorization: String,
    pub token: String,
    pub jwks: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: String,
    pub idle_seconds: u64,
    pub absolute_seconds: u64,
}
impl Config {
    pub fn from_map(env: &HashMap<String, String>) -> Result<Option<Self>, &'static str> {
        let get = |key: &str| {
            env.get(&format!("SIGILED_BROWSER_{key}"))
                .map(String::as_str)
        };
        let configured = env.keys().any(|k| k.starts_with("SIGILED_BROWSER_"));
        match get("ENABLED") {
            None if !configured => return Ok(None),
            Some("false")
                if env
                    .keys()
                    .filter(|k| k.starts_with("SIGILED_BROWSER_"))
                    .count()
                    == 1 =>
            {
                return Ok(None)
            }
            Some("true") => (),
            _ => return Err("browser: partial configuration or invalid ENABLED"),
        }
        const KEYS: &[&str] = &[
            "ENABLED",
            "ORIGINS",
            "ISSUER",
            "AUTHORIZATION_URL",
            "TOKEN_URL",
            "JWKS_URL",
            "CLIENT_ID",
            "CLIENT_SECRET",
            "SCOPES",
            "ALLOW_LOOPBACK_HTTP",
            "IDLE_SECONDS",
            "ABSOLUTE_SECONDS",
        ];
        if env
            .keys()
            .filter_map(|k| k.strip_prefix("SIGILED_BROWSER_"))
            .any(|k| !KEYS.contains(&k))
        {
            return Err("browser: unknown configuration key");
        }
        let dev = match get("ALLOW_LOOPBACK_HTTP") {
            None | Some("false") => false,
            Some("true") => true,
            _ => return Err("browser: invalid loopback option"),
        };
        let required = |key| {
            get(key)
                .filter(|v| !v.is_empty())
                .ok_or("browser: missing required configuration")
        };
        let issuer = required("ISSUER")?.to_owned();
        let provider = endpoint(&issuer, dev)?;
        if !issuer.ends_with('/') {
            return Err("browser: issuer must end with slash");
        }
        let authorization = required("AUTHORIZATION_URL")?.to_owned();
        let token = required("TOKEN_URL")?.to_owned();
        let jwks = required("JWKS_URL")?.to_owned();
        for value in [&authorization, &token, &jwks] {
            if endpoint(value, dev)?.origin() != provider.origin() {
                return Err("browser: provider endpoints must share issuer origin");
            }
        }
        let origins: Vec<String> = required("ORIGINS")?.split(',').map(str::to_owned).collect();
        if origins.is_empty() || origins.len() > 8 {
            return Err("browser: invalid origin count");
        }
        let mut seen = std::collections::HashSet::new();
        let mut authorities = std::collections::HashSet::new();
        for origin in &origins {
            let u = endpoint(origin, dev)?;
            if u.origin().ascii_serialization() != *origin
                || u.path() != "/"
                || !seen.insert(origin)
                || !authorities.insert(origin.split_once("://").unwrap().1)
            {
                return Err("browser: origins must be unique canonical exact origins without trailing slash");
            }
        }
        let scopes = required("SCOPES")?.to_owned();
        let scope_names: Vec<_> = scopes.split(' ').collect();
        if scopes.len() > 512
            || !scope_names.contains(&"openid")
            || !scope_names.contains(&"sigiled-groups")
            || scope_names.iter().any(|s| {
                s.is_empty()
                    || !s.bytes().all(|b| {
                        b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b)
                    })
            })
        {
            return Err("browser: explicit valid openid and sigiled-groups scopes required");
        }
        let client_id = required("CLIENT_ID")?.to_owned();
        if client_id.len() > 256 || client_id.chars().any(char::is_control) {
            return Err("browser: invalid client ID");
        }
        let client_secret = get("CLIENT_SECRET").map(str::to_owned);
        if client_secret
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 4096)
        {
            return Err("browser: invalid client secret");
        }
        let duration = |key, default, max| -> Result<u64, &'static str> {
            let n = get(key)
                .map(str::parse::<u64>)
                .transpose()
                .map_err(|_| "browser: invalid duration")?
                .unwrap_or(default);
            if !(60..=max).contains(&n) {
                return Err("browser: duration outside allowed range");
            }
            Ok(n)
        };
        let absolute_seconds = duration("ABSOLUTE_SECONDS", 28800, 86400)?;
        let idle_seconds = duration("IDLE_SECONDS", 900, absolute_seconds)?;
        Ok(Some(Self {
            origins,
            issuer,
            authorization,
            token,
            jwks,
            client_id,
            client_secret,
            scopes,
            idle_seconds,
            absolute_seconds,
        }))
    }
    pub fn validate_auth(&self, auth: &crate::auth::AuthConfig) -> Result<(), &'static str> {
        let base = auth
            .oidc_base
            .as_ref()
            .ok_or("browser: machine OIDC validator must be enabled")?;
        if !self
            .issuer
            .starts_with(&format!("{}/", base.trim_end_matches('/')))
            || auth.admin_group.is_empty()
            || auth.driver_group.is_empty()
        {
            return Err("browser: issuer or role mapping incompatible with machine validator");
        }
        Ok(())
    }
}
fn endpoint(raw: &str, dev: bool) -> Result<Url, &'static str> {
    let u = Url::parse(raw).map_err(|_| "browser: invalid URL")?;
    let loopback = u
        .host_str()
        .is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]");
    if (u.scheme() != "https" && !(dev && loopback && u.scheme() == "http"))
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || raw.contains('\\')
        || raw.chars().any(char::is_control)
        || raw.contains('%')
        || (u.as_str() != raw && u.as_str() != format!("{raw}/"))
        || u.host_str().is_some_and(|h| h.contains('*'))
    {
        return Err("browser: insecure or ambiguous URL");
    }
    Ok(u)
}

/// Intentionally narrow path grammar: no percent escapes, query, fragment or backslash.
/// B2 can use stable routes; it must not turn this into an arbitrary redirect parameter.
pub(super) fn return_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path.len() <= 1024
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.~".contains(&b))
        && !path.split('/').any(|s| s == "." || s == "..")
        && !path.starts_with("/browser/")
}
