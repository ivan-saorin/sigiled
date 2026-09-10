//! Deliberately narrow proof: active repository rules prohibit master updates,
//! with exactly the verified host PAT User as an always-bypass principal.
//! Missing bypass visibility, parent/org rules, DeployKey and other rule types
//! are unknown/unsupported, never evidence of enforcement.
use serde_json::Value;
pub fn prove(
    actor: u64,
    owner_repo: &str,
    rules: &[Value],
    details: &[Value],
) -> Result<(), &'static str> {
    if actor == 0 || rules.is_empty() {
        return Err("master_update_rule_required");
    }
    let mut protects = false;
    for rule in rules {
        if !matches!(
            rule["type"].as_str(),
            Some("update" | "deletion" | "non_fast_forward")
        ) {
            return Err("unsupported_master_policy");
        }
        let id = rule["ruleset_id"]
            .as_u64()
            .ok_or("incomplete_master_policy")?;
        let detail = details
            .iter()
            .find(|d| d["id"].as_u64() == Some(id))
            .ok_or("incomplete_master_policy")?;
        if detail["enforcement"] != "active"
            || detail["source_type"] != "Repository"
            || detail["source"] != owner_repo
        {
            return Err("unsupported_master_policy");
        }
        let bypass = detail["bypass_actors"]
            .as_array()
            .ok_or("policy_bypass_visibility_required")?;
        if bypass.len() != 1
            || bypass[0]["actor_type"] != "User"
            || bypass[0]["actor_id"].as_u64() != Some(actor)
            || bypass[0]["bypass_mode"] != "always"
        {
            return Err("distinct_host_merge_user_required");
        }
        if rule["type"] == "update" {
            protects = true
        }
    }
    if protects {
        Ok(())
    } else {
        Err("master_update_rule_required")
    }
}
impl crate::github::GitHub {
    pub async fn verify_ide_policy(&self, project: &str) -> Result<(), &'static str> {
        self.verify_policy_transport(
            project,
            std::env::var("SIGILED_IDE_HOST_MERGE").as_deref() == Ok("github-pat"),
        )
        .await
    }
    async fn verify_policy_transport(
        &self,
        project: &str,
        enabled: bool,
    ) -> Result<(), &'static str> {
        if !enabled {
            return Err("host_merge_transport_required");
        }
        if !crate::project::valid_name(project) {
            return Err("invalid_project");
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|_| "policy_unavailable")?;
        // The total request count and body bytes are bounded; no pagination truncation
        // grants readiness and the PAT never follows a redirect.
        async fn read(
            g: &crate::github::GitHub,
            c: &reqwest::Client,
            path: &str,
        ) -> Result<Value, &'static str> {
            let response = c
                .get(format!("{}{path}", g.api_base))
                .bearer_auth(&g.pat)
                .header("User-Agent", "sigiled")
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .map_err(|_| "policy_unavailable")?;
            if !response.status().is_success() {
                return Err("policy_unavailable");
            }
            if response.content_length().is_some_and(|n| n > 262144) {
                return Err("policy_incomplete");
            }
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| "policy_unavailable")? {
                if bytes.len() + chunk.len() > 262144 {
                    return Err("policy_incomplete");
                }
                bytes.extend_from_slice(&chunk)
            }
            serde_json::from_slice(&bytes).map_err(|_| "policy_incomplete")
        }
        let user = read(self, &client, "/user").await?;
        let actor = user["id"].as_u64().ok_or("host_merge_user_required")?;
        let name = format!("{}/{project}", self.owner);
        let mut rules = Vec::new();
        let mut complete = false;
        for page in 1..=4 {
            let v = read(
                self,
                &client,
                &format!("/repos/{name}/rules/branches/master?per_page=100&page={page}"),
            )
            .await?;
            let values = v.as_array().ok_or("policy_incomplete")?;
            rules.extend(values.iter().cloned());
            if values.len() < 100 {
                complete = true;
                break;
            }
        }
        if !complete {
            return Err("policy_incomplete");
        }
        let mut ids = std::collections::BTreeSet::new();
        for rule in &rules {
            ids.insert(rule["ruleset_id"].as_u64().ok_or("policy_incomplete")?);
        }
        if ids.len() > 8 {
            return Err("policy_incomplete");
        }
        let mut details = Vec::new();
        for id in ids {
            details.push(read(self, &client, &format!("/repos/{name}/rulesets/{id}")).await?)
        }
        prove(actor, &name, &rules, &details)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_actual_active_rule_with_visible_exclusive_host_user_bypass_passes() {
        let rules = vec![json!({"type":"update","ruleset_id":7})];
        let good = json!({"id":7,"source_type":"Repository","source":"o/proj","enforcement":"active","bypass_actors":[{"actor_type":"User","actor_id":12,"bypass_mode":"always"}]});
        assert!(prove(12, "o/proj", &rules, std::slice::from_ref(&good)).is_ok());
        for field in ["bypass_actors", "enforcement"] {
            let mut d = good.clone();
            d.as_object_mut().unwrap().remove(field);
            assert!(prove(12, "o/proj", &rules, &[d]).is_err())
        }
        let mut d = good.clone();
        d["bypass_actors"] = json!([{"actor_type":"DeployKey","bypass_mode":"always"}]);
        assert!(prove(12, "o/proj", &rules, &[d]).is_err());
        assert!(prove(13, "o/proj", &rules, &[good]).is_err());
        assert!(prove(12, "o/proj", &[], &[]).is_err());
    }
}
#[cfg(test)]
mod transport_tests {
    use axum::{
        extract::{OriginalUri, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        Json, Router,
    };
    use serde_json::json;
    #[tokio::test]
    async fn trusted_read_inspection_rejects_unknown_bypass_and_redirects() {
        async fn mock(
            State(mode): State<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
            OriginalUri(uri): OriginalUri,
            headers: HeaderMap,
        ) -> axum::response::Response {
            assert_eq!(headers["authorization"], "Bearer host-secret");
            let path = uri.path();
            let mode = mode.load(std::sync::atomic::Ordering::SeqCst);
            if mode == 2 {
                return (
                    StatusCode::FOUND,
                    [("location", "http://127.0.0.1:1/never")],
                )
                    .into_response();
            }
            let value = if path == "/user" {
                json!({"id":12})
            } else if path.ends_with("/rules/branches/master") {
                json!([{"type":"update","ruleset_id":7}])
            } else if mode == 1 {
                json!({"id":7,"source_type":"Repository","source":"o/proj","enforcement":"active"})
            } else {
                json!({"id":7,"source_type":"Repository","source":"o/proj","enforcement":"active","bypass_actors":[{"actor_type":"User","actor_id":12,"bypass_mode":"always"}]})
            };
            Json(value).into_response()
        }
        let mode = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(mock).with_state(mode.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let github = crate::github::GitHub {
            api_base: base,
            pat: "host-secret".into(),
            owner: "o".into(),
            template: "t".into(),
            keys_dir: "/unused".into(),
        };
        assert!(github.verify_policy_transport("proj", true).await.is_ok());
        assert_eq!(
            github.verify_policy_transport("proj", false).await,
            Err("host_merge_transport_required")
        );
        mode.store(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            github.verify_policy_transport("proj", true).await,
            Err("policy_bypass_visibility_required")
        );
        mode.store(2, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            github.verify_policy_transport("proj", true).await,
            Err("policy_unavailable")
        );
        server.abort();
    }
}
