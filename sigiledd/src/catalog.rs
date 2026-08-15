// catalog.rs — GET /services: the stack service catalog (DEC-27).
// Embedded at compile time like the contract (DEC-06 by construction):
// the binary serves the catalog of the commit it was built from —
// growing the catalog = editing /catalog.json and redeploying the
// control plane. Public like the contract: it names capabilities and
// where they answer; it carries no secret.
//
// Schema, per service: name, purpose, machine{base, gate},
// human?{base, gate}, spec, status, skill. The machine leg is
// mandatory — the catalog is LLM-facing; the human leg exists only for
// services that are dual by nature. Gates state how the edge answers
// as it actually is, never as planned.
use axum::extract::Query;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::sync::LazyLock;

pub const TEXT: &str = include_str!("../../catalog.json");

const MACHINE_GATES: &[&str] = &["stack-bearer", "service-token", "sso-only", "edge-open"];
const HUMAN_GATES: &[&str] = &["sso", "basic", "open"];
const STATUSES: &[&str] = &["live", "building", "planned"];

static PARSED: LazyLock<Result<serde_json::Value, String>> = LazyLock::new(|| {
    let v: serde_json::Value =
        serde_json::from_str(TEXT).map_err(|e| format!("not valid JSON: {e}"))?;
    validate(&v)?;
    Ok(v)
});

/// Boot gate, called from main(): a broken catalog must fail the deploy,
/// never surface weeks later as a 500.
pub fn assert_valid() {
    if let Err(e) = &*PARSED {
        panic!("catalog.json invalid: {e}");
    }
}

fn check_leg(s: &serde_json::Value, name: &str, leg: &str, gates: &[&str]) -> Result<(), String> {
    let base = s[leg]["base"]
        .as_str()
        .ok_or_else(|| format!("{name}: {leg}.base must be a string"))?;
    if !base.starts_with("https://") {
        return Err(format!("{name}: {leg}.base must be https"));
    }
    let gate = s[leg]["gate"]
        .as_str()
        .ok_or_else(|| format!("{name}: {leg}.gate must be a string"))?;
    if !gates.contains(&gate) {
        return Err(format!("{name}: {leg}.gate '{gate}' not in {gates:?}"));
    }
    Ok(())
}

fn validate(v: &serde_json::Value) -> Result<(), String> {
    v["catalog_version"]
        .as_u64()
        .ok_or_else(|| "catalog_version must be a positive integer".to_string())?;
    let services = v["services"]
        .as_array()
        .ok_or_else(|| "services must be an array".to_string())?;
    let mut seen = std::collections::HashSet::new();
    for s in services {
        let name = s["name"]
            .as_str()
            .ok_or_else(|| "every service needs a string name".to_string())?;
        let shape = name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !shape {
            return Err(format!("{name}: names are lowercase alnum+dash, letter first"));
        }
        if !seen.insert(name) {
            return Err(format!("{name}: duplicate service name"));
        }
        if s["purpose"].as_str().map_or(true, str::is_empty) {
            return Err(format!("{name}: purpose must be a non-empty string"));
        }
        // The machine leg is mandatory; the human leg only where dual.
        check_leg(s, name, "machine", MACHINE_GATES)?;
        if !s["human"].is_null() {
            check_leg(s, name, "human", HUMAN_GATES)?;
        }
        let status = s["status"]
            .as_str()
            .ok_or_else(|| format!("{name}: status must be a string"))?;
        if !STATUSES.contains(&status) {
            return Err(format!("{name}: status '{status}' not in {STATUSES:?}"));
        }
        for opt in ["spec", "skill"] {
            if !s[opt].is_null() && !s[opt].is_string() {
                return Err(format!("{name}: {opt} must be a string or null"));
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct Filter {
    pub status: Option<String>,
}

pub async fn serve(Query(f): Query<Filter>) -> Response {
    let v = match &*PARSED {
        Ok(v) => v,
        // Unreachable after assert_valid(); kept as the honest fallback.
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({ "error": e })),
            )
                .into_response()
        }
    };
    let body = match f.status.as_deref() {
        None => v.clone(),
        Some(want) => {
            let mut out = v.clone();
            if let Some(arr) = out["services"].as_array_mut() {
                arr.retain(|s| s["status"].as_str() == Some(want));
            }
            out
        }
    };
    let version = HeaderValue::from_str(&crate::version())
        .unwrap_or_else(|_| HeaderValue::from_static("unknown"));
    (
        [(header::HeaderName::from_static("x-sigiled-version"), version)],
        axum::Json(body),
    )
        .into_response()
}
