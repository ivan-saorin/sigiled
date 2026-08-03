// skill.rs — GET /skill/{driver}: renders the per-driver, per-instance
// driving skill from the embedded template (docs/skill-template.md).
// The skill is the ONE artifact that is supposed to carry instance
// specifics — domains and credentials — so it is generated here, where
// both live, instead of hand-edited per driver (design §1, DEC-19).
//
// The client_secret is fetched live from the IdP admin API when
// AUTHENTIK_API_TOKEN is configured (same read the provision script
// does); otherwise the rendered skill tells the operator where to copy
// it from. Approval-gated like projects-new: this endpoint can emit a
// credential, so a driver only gets one with a human standing next to it.
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub const TEMPLATE: &str = include_str!("../../docs/skill-template.md");

/// Pure render: the handler resolves the values, this fills the holes.
/// No placeholder survives — a template drift breaks tests, not drivers.
pub fn render(
    driver: &str,
    api_base: &str,
    oidc_base: &str,
    search_base: &str,
    client_secret: &str,
) -> String {
    TEMPLATE
        .replace("{{driver}}", driver)
        .replace("{{api_base}}", api_base)
        .replace("{{oidc_base}}", oidc_base)
        .replace("{{search_base}}", search_base)
        .replace("{{client_secret}}", client_secret)
}

/// Read the driver's client_secret from the IdP admin API. None on any
/// miss (no token, unknown provider, API error): the caller degrades to
/// the copy-it-yourself placeholder instead of failing the render.
async fn fetch_secret(http: &reqwest::Client, oidc_base: &str, driver: &str) -> Option<String> {
    let token = std::env::var("AUTHENTIK_API_TOKEN").ok()?;
    let api = format!("{}/api/v3", oidc_base.trim_end_matches('/'));
    let list: serde_json::Value = http
        .get(format!("{api}/providers/oauth2/?name={driver}"))
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let pk = list["results"].get(0)?["pk"].as_i64()?;
    let provider: serde_json::Value = http
        .get(format!("{api}/providers/oauth2/{pk}/"))
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    provider["client_secret"].as_str().map(str::to_string)
}

pub async fn serve(
    actor: crate::auth::Actor,
    State(state): State<crate::AppState>,
    Path(driver): Path<String>,
) -> Response {
    fn err(status: StatusCode, detail: impl Into<String>) -> Response {
        (status, Json(json!({ "detail": detail.into() }))).into_response()
    }
    if let Err(denial) = crate::auth::authorize(
        &actor,
        crate::auth::Action::SkillRender,
        None,
        &state.auth.approvals,
        crate::auth::now_epoch(),
    ) {
        return err(StatusCode::FORBIDDEN, denial.0);
    }
    // Driver names ride into an IdP query: same shape rule as project
    // names keeps the URL inert.
    if !crate::project::valid_name(&driver) {
        return err(StatusCode::UNPROCESSABLE_ENTITY, format!("invalid driver name: {driver}"));
    }
    // Instance identity comes from the deploy env, exactly like the
    // runtime: without DOMAIN there is no instance to describe.
    let Ok(domain) = std::env::var("DOMAIN") else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "DOMAIN not configured");
    };
    let api_base = format!("https://api.{domain}");
    let oidc_base = state
        .auth
        .config
        .oidc_base
        .clone()
        .unwrap_or_else(|| format!("https://auth.{domain}"));
    let search_base =
        std::env::var("SEARXNG_BASE_URL").unwrap_or_else(|_| format!("https://search.{domain}"));
    let http = reqwest::Client::new();
    let secret = match fetch_secret(&http, &oidc_base, &driver).await {
        Some(s) => s,
        None => format!(
            "<client_secret — copy from Authentik: Applications → Providers → {driver} → Edit>"
        ),
    };
    let body = render(&driver, &api_base, &oidc_base, &search_base, &secret);
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/markdown; charset=utf-8"))],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_fills_every_placeholder() {
        let out = render(
            "sigiled-claude",
            "https://api.example.com",
            "https://auth.example.com",
            "https://search.example.com",
            "s3cret",
        );
        assert!(!out.contains("{{"), "unfilled placeholder in:\n{out}");
        assert!(out.contains("client_id=sigiled-claude"));
        assert!(out.contains("client_secret=s3cret"));
        assert!(out.contains("GET https://api.example.com/sigiled/contract"));
        // Skill loaders require YAML frontmatter: the rendered file must
        // open with it, name stable across drivers.
        assert!(out.starts_with("---
name: sigil
"), "missing frontmatter:
{out}");
    }

    #[test]
    fn skill_render_is_approval_gated() {
        // The endpoint can emit a credential: same row of the capability
        // map as projects-new (drivers need a live human).
        assert!(crate::auth::requires_approval(crate::auth::Action::SkillRender, None));
    }
}
