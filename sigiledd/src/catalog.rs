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
use axum::extract::{Query, State};
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
            return Err(format!(
                "{name}: names are lowercase alnum+dash, letter first"
            ));
        }
        if !seen.insert(name) {
            return Err(format!("{name}: duplicate service name"));
        }
        if s["purpose"].as_str().is_none_or(str::is_empty) {
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

pub async fn serve(State(state): State<crate::AppState>, Query(f): Query<Filter>) -> Response {
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
    let mut combined = v.clone();
    combined["services"]
        .as_array_mut()
        .unwrap()
        .extend(dynamic(&state.registry).0);
    let v = &combined;
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
        [(
            header::HeaderName::from_static("x-sigiled-version"),
            version,
        )],
        axum::Json(body),
    )
        .into_response()
}

/// Public catalog entries are explicit declarations, never private metadata or probe results.
pub fn dynamic(
    registry: &crate::project::Registry,
) -> (
    Vec<serde_json::Value>,
    std::collections::BTreeMap<String, &'static str>,
) {
    use std::collections::{BTreeMap, BTreeSet};
    let descriptors = registry.descriptors();
    let seed = PARSED.as_ref().expect("validated seed");
    let reserved: BTreeSet<&str> = seed["services"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str())
        .chain([
            "api", "auth", "gateway", "sigiled", "www", "sso", "admin", "platform", "memory", "ide",
        ])
        .collect();
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (project, d) in &descriptors {
        if let Some(s) = &d.declaration.service {
            owners.entry(&s.name).or_default().push(project);
        }
    }
    let mut errors = BTreeMap::new();
    let mut entries = vec![];
    for (project, d) in &descriptors {
        let Some(s) = &d.declaration.service else {
            continue;
        };
        let error = if reserved.contains(s.name.as_str()) {
            Some("reserved_service_name")
        } else if owners[s.name.as_str()].len() != 1 {
            Some("duplicate_service_name")
        } else if d
            .declaration
            .validate(
                d.app
                    .as_ref()
                    .map(|name| crate::manifest::AppManifest {
                        name: name.clone(),
                        dockerfile: String::new(),
                        volumes: Default::default(),
                        secrets: Default::default(),
                        requires: vec![],
                    })
                    .as_ref(),
            )
            .is_err()
        {
            Some("invalid_service_declaration")
        } else {
            match registry.domain.as_deref() {
                None => Some("stack_domain_not_configured"),
                Some(domain)
                    if s.origin.trim_end_matches('/')
                        != format!("https://{}.{}", s.name, domain) =>
                {
                    Some("service_origin_outside_route_policy")
                }
                Some(_) => None,
            }
        };
        if let Some(error) = error {
            errors.insert(project.clone(), error);
            continue;
        }
        entries.push(serde_json::json!({
            "name":s.name,"purpose":s.purpose,"machine":{"base":s.origin,"gate":s.gate},
            "spec":null,"skill":null,"status":s.status,"capabilities":s.capabilities,
            "status_source":"declaration","observed_at":d.observed_at,
            "stale":d.error.is_some() || d.observed_at.is_none_or(|t| crate::auth::now_epoch().saturating_sub(t)>600)
        }));
    }
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    (entries, errors)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn service_text(name: &str) -> String {
        format!("[project]\ndescription='PRIVATE-SENTINEL'\n[app]\nname='{name}'\n[service]\nname='{name}'\npurpose='Published description'\norigin='https://{name}.stack.test'\ngate='stack-bearer'\nstatus='planned'\n")
    }
    fn registry() -> crate::project::Registry {
        let mut r = crate::project::Registry::default();
        r.domain = Some("stack.test".into());
        r
    }
    fn declare(r: &crate::project::Registry, project: &str, name: &str) {
        let m = crate::manifest::Manifest::parse(&service_text(name)).unwrap();
        r.insert(crate::project::ProjectRecord::new(project, &m, None));
        let mut map = r.descriptors();
        map.insert(
            project.into(),
            crate::ecosystem::Descriptor {
                declaration: m.declaration,
                app: Some(name.into()),
                observed_at: Some(crate::auth::now_epoch()),
                ..Default::default()
            },
        );
        r.hydrate_descriptors(map);
    }
    #[test]
    fn reserved_and_duplicate_services_fail_for_all_owners() {
        let r = registry();
        declare(&r, "alpha", "search");
        let (entries, errors) = dynamic(&r);
        assert!(entries.is_empty());
        assert_eq!(errors["alpha"], "reserved_service_name");
        declare(&r, "alpha", "novel");
        declare(&r, "bravo", "novel");
        let (entries, errors) = dynamic(&r);
        assert!(entries.is_empty());
        assert_eq!(errors.len(), 2);
        assert!(errors.values().all(|e| *e == "duplicate_service_name"));
    }
    #[tokio::test]
    async fn dynamic_catalog_preserves_seed_filter_and_public_allowlist() {
        let r = registry();
        declare(&r, "alpha", "novel");
        let state = crate::AppState {
            registry: r,
            ..Default::default()
        };
        let response = serve(
            State(state.clone()),
            Query(Filter {
                status: Some("planned".into()),
            }),
        )
        .await;
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("PRIVATE-SENTINEL"));
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["catalog_version"], 1);
        assert!(body["services"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["status"] == "planned"));
        let entry = body["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "novel")
            .unwrap();
        assert_eq!(entry["machine"]["base"], "https://novel.stack.test");
        assert_eq!(entry["status_source"], "declaration");
        let mut bad = state.registry.descriptors();
        bad.get_mut("alpha")
            .unwrap()
            .declaration
            .service
            .as_mut()
            .unwrap()
            .origin = "https://evil.test".into();
        state.registry.hydrate_descriptors(bad);
        assert_eq!(
            dynamic(&state.registry).1["alpha"],
            "service_origin_outside_route_policy"
        );
    }
}
