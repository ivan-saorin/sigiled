//! Fixed-endpoint OIDC client. Credentials and raw provider errors never implement Debug/Serialize.
use super::{config::Config, Error};
use crate::auth::{self, Actor, AuthState, Claims};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

pub struct Provider {
    http: reqwest::Client,
    cache: Mutex<Cache>,
}
#[derive(Default)]
struct Cache {
    keys: HashMap<String, DecodingKey>,
    fetched: Option<Instant>,
    attempted: Option<Instant>,
}
#[derive(Deserialize)]
pub(super) struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub token_type: String,
}
#[derive(Deserialize)]
struct IdClaims {
    sub: String,
    iss: String,
    aud: serde_json::Value,
    exp: u64,
    iat: u64,
    nonce: Option<String>,
    azp: Option<String>,
    at_hash: Option<String>,
}
pub(super) struct Verified {
    pub actor: Actor,
    pub subject: String,
    pub display_name: Option<String>,
    pub expires: u64,
}
impl Provider {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(10))
                .build()
                .expect("browser HTTP client"),
            cache: Mutex::new(Cache::default()),
        }
    }
    pub(super) async fn exchange(
        &self,
        cfg: &Config,
        fields: &[(&str, &str)],
    ) -> Result<Tokens, Error> {
        let mut form = fields.to_vec();
        form.push(("client_id", &cfg.client_id));
        if let Some(secret) = &cfg.client_secret {
            form.push(("client_secret", secret));
        }
        let response = self
            .http
            .post(&cfg.token)
            .form(&form)
            .send()
            .await
            .map_err(|_| Error::login())?;
        let tokens: Tokens = bounded_json(response).await?;
        if !tokens.token_type.eq_ignore_ascii_case("bearer")
            || tokens.access_token.is_empty()
            || tokens.access_token.len() > 16384
            || tokens
                .refresh_token
                .as_ref()
                .is_some_and(|t| t.is_empty() || t.len() > 16384)
        {
            return Err(Error::login());
        }
        Ok(tokens)
    }
    async fn key(&self, cfg: &Config, token: &str) -> Result<DecodingKey, Error> {
        let h = decode_header(token).map_err(|_| Error::login())?;
        if h.alg != Algorithm::RS256 {
            return Err(Error::login());
        }
        let kid = h
            .kid
            .filter(|k| !k.is_empty() && k.len() <= 256)
            .ok_or_else(Error::login)?;
        // One Provider belongs to one immutable Config: cache identity is issuer + fixed JWKS URL + kid.
        let mut cache = self.cache.lock().await;
        if cache
            .fetched
            .is_some_and(|t| t.elapsed() < Duration::from_secs(300))
        {
            if let Some(key) = cache.keys.get(&kid) {
                return Ok(key.clone());
            }
        }
        if cache
            .attempted
            .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return Err(Error::login());
        }
        cache.attempted = Some(Instant::now());
        let response = self
            .http
            .get(&cfg.jwks)
            .send()
            .await
            .map_err(|_| Error::login())?;
        let jwks: serde_json::Value = bounded_json(response).await?;
        let list = jwks["keys"]
            .as_array()
            .filter(|a| a.len() <= 64)
            .ok_or_else(Error::login)?;
        let mut keys = HashMap::new();
        for k in list {
            if k["kty"] != "RSA"
                || k.get("use").is_some_and(|v| v != "sig")
                || k.get("alg").is_some_and(|v| v != "RS256")
            {
                continue;
            }
            if let (Some(id), Some(n), Some(e)) =
                (k["kid"].as_str(), k["n"].as_str(), k["e"].as_str())
            {
                if id.len() > 256 || keys.contains_key(id) {
                    return Err(Error::login());
                }
                keys.insert(
                    id.to_owned(),
                    DecodingKey::from_rsa_components(n, e).map_err(|_| Error::login())?,
                );
            }
        }
        cache.keys = keys;
        cache.fetched = Some(Instant::now());
        cache.keys.get(&kid).cloned().ok_or_else(Error::login)
    }
    pub(super) async fn verify(
        &self,
        cfg: &Config,
        auth: &AuthState,
        tokens: &Tokens,
        nonce: Option<&str>,
        expected_subject: Option<&str>,
    ) -> Result<Verified, Error> {
        let key = self.key(cfg, &tokens.access_token).await?;
        let claims: Claims = auth::validate_exact_jwt(&tokens.access_token, &cfg.issuer, &key)
            .map_err(|_| Error::login())?;
        if claims.sub.is_empty()
            || claims.sub.len() > 512
            || expected_subject.is_some_and(|s| s != claims.sub)
        {
            return Err(Error::login());
        }
        if expected_subject.is_none() && tokens.id_token.is_none() {
            return Err(Error::login());
        }
        if let Some(id_token) = &tokens.id_token {
            let key = self.key(cfg, id_token).await?;
            let mut v = Validation::new(Algorithm::RS256);
            v.leeway = 0;
            v.set_issuer(&[&cfg.issuer]);
            v.set_audience(&[&cfg.client_id]);
            v.set_required_spec_claims(&["exp", "iss", "sub", "aud", "iat"]);
            let id = decode::<IdClaims>(id_token, &key, &v)
                .map_err(|_| Error::login())?
                .claims;
            let audiences: Vec<&str> = match &id.aud {
                serde_json::Value::String(s) => vec![s],
                serde_json::Value::Array(a) => a
                    .iter()
                    .map(|v| v.as_str().ok_or_else(Error::login))
                    .collect::<Result<_, _>>()?,
                _ => return Err(Error::login()),
            };
            if id.iss != cfg.issuer
                || id.sub != claims.sub
                || id.exp <= auth::now_epoch()
                || id.iat > auth::now_epoch() + 30
                || audiences.is_empty()
                || (audiences.len() > 1 && id.azp.as_deref() != Some(&cfg.client_id))
                || id.azp.as_ref().is_some_and(|s| s != &cfg.client_id)
                || nonce.is_some_and(|n| {
                    (expected_subject.is_none() || id.nonce.is_some())
                        && id.nonce.as_deref() != Some(n)
                })
            {
                return Err(Error::login());
            }
            if let Some(hash) = id.at_hash {
                // RS256 mandates the left half of SHA-256 for at_hash (OIDC Core 3.1.3.6).
                let digest = Sha256::digest(tokens.access_token.as_bytes());
                if !auth::constant_time_eq(&hash, &URL_SAFE_NO_PAD.encode(&digest[..16])) {
                    return Err(Error::login());
                }
            }
        }
        let mut actor = auth::actor_from_claims(&claims, &auth.config, &auth.approvals)
            .map_err(|_| Error::forbidden("stack_role_required"))?;
        // Identity is verified iss/sub, never username or the shared OAuth client azp.
        actor.driver = principal(&claims.iss, &claims.sub);
        actor.approval = auth
            .approvals
            .live(&actor.driver, auth::now_epoch())
            .map(|a| format!("{} (device, expires {})", a.human, a.expires_epoch));
        Ok(Verified {
            actor,
            subject: claims.sub,
            display_name: claims.preferred_username,
            expires: claims.exp as u64,
        })
    }
}
pub(crate) fn principal(issuer: &str, subject: &str) -> String {
    let tuple = serde_json::to_vec(&(issuer, subject)).expect("string tuple");
    format!("human:{}", URL_SAFE_NO_PAD.encode(Sha256::digest(tuple)))
}
async fn bounded_json<T: serde::de::DeserializeOwned>(
    mut r: reqwest::Response,
) -> Result<T, Error> {
    if !r.status().is_success() || r.content_length().is_some_and(|n| n > 65536) {
        return Err(Error::login());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|_| Error::login())? {
        if bytes.len() + chunk.len() > 65536 {
            return Err(Error::login());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::login())
}
