// github.rs — the GitHub client behind POST /projects: the v2 port of the
// v1's github.py, narrowed to what the verb needs (repo-from-template,
// adoption probe, deploy keys). The PAT is the only credential; it lives in
// the canary's .env as GITHUB_PAT and never in git or in any response.
// GITHUB_API_BASE is overridable so tests drive a local mock instead of
// the real API — same pattern as the auth JWKS tests.
use std::path::PathBuf;

#[derive(Clone)]
pub struct GitHub {
    pub(crate) api_base: String,
    pub(crate) pat: String,
    pub(crate) owner: String,
    pub(crate) template: String,
    pub(crate) keys_dir: PathBuf,
}

impl GitHub {
    /// Some only when GITHUB_PAT is set: without it POST /projects answers
    /// 503 honestly instead of failing mid-flight.
    pub fn from_env() -> Option<Self> {
        let pat = std::env::var("GITHUB_PAT").ok()?;
        let state = std::env::var("SIGILED_STATE_DIR").unwrap_or_else(|_| "/data".into());
        Some(GitHub {
            api_base: std::env::var("GITHUB_API_BASE")
                .unwrap_or_else(|_| "https://api.github.com".into()),
            pat,
            owner: std::env::var("GITHUB_OWNER").unwrap_or_else(|_| "ivan-saorin".into()),
            template: std::env::var("VM_TMPL_REPO").unwrap_or_else(|_| "vm-tmpl".into()),
            keys_dir: std::env::var("SIGILED_KEYS_DIR")
                .unwrap_or_else(|_| format!("{state}/keys"))
                .into(),
        })
    }

    fn req(
        &self,
        http: &reqwest::Client,
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::RequestBuilder {
        http.request(method, format!("{}{}", self.api_base, path))
            .header("Authorization", format!("token {}", self.pat))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "sigiled")
            .timeout(std::time::Duration::from_secs(30))
    }

    /// Generate {owner}/{name} from the template — or, when the repo already
    /// exists, adopt it as-is: key + register, nothing written (v1 §7.8
    /// behavior). Returns (full_name, adopted). A 422 from generate is only
    /// adoption when the repo actually exists (probe): a genuine validation
    /// error stays loud instead of registering a phantom.
    pub async fn create_or_adopt(
        &self,
        http: &reqwest::Client,
        name: &str,
    ) -> Result<(String, bool), String> {
        let r = self
            .req(
                http,
                reqwest::Method::POST,
                &format!("/repos/{}/{}/generate", self.owner, self.template),
            )
            .json(&serde_json::json!({
                "owner": self.owner, "name": name, "private": true,
                "description": format!("SIGILED project '{name}' (from {})", self.template),
            }))
            .send()
            .await
            .map_err(|e| format!("github generate: {e}"))?;
        match r.status().as_u16() {
            201 => {
                let body: serde_json::Value =
                    r.json().await.map_err(|e| format!("github generate decode: {e}"))?;
                match body["full_name"].as_str() {
                    Some(full) => Ok((full.to_string(), false)),
                    None => Err("github generate: 201 without full_name".into()),
                }
            }
            422 => {
                let probe = self
                    .req(http, reqwest::Method::GET, &format!("/repos/{}/{name}", self.owner))
                    .send()
                    .await
                    .map_err(|e| format!("github probe: {e}"))?;
                if probe.status().is_success() {
                    Ok((format!("{}/{name}", self.owner), true))
                } else {
                    Err(format!(
                        "github generate rejected '{name}' (422) and no such repo exists"
                    ))
                }
            }
            s => Err(format!("github generate: {s} {}", excerpt(r).await)),
        }
    }

    pub async fn add_deploy_key(
        &self,
        http: &reqwest::Client,
        name: &str,
        pubkey: &str,
    ) -> Result<(), String> {
        let r = self
            .req(http, reqwest::Method::POST, &format!("/repos/{}/{name}/keys", self.owner))
            .json(&serde_json::json!({
                "title": format!("sigiled-{name}"), "key": pubkey, "read_only": false,
            }))
            .send()
            .await
            .map_err(|e| format!("github deploy key: {e}"))?;
        if r.status().as_u16() != 201 {
            return Err(format!("github deploy key: {} {}", r.status().as_u16(), excerpt(r).await));
        }
        Ok(())
    }

    /// ed25519 keypair under {keys_dir}/{project}/ — the exact layout the
    /// runtime reads (key_path) and the importer fills. ssh-keygen does the
    /// crypto: it ships with the openssh-client the mirror ssh already
    /// needs, so no new dependency (and it sets the 0600 itself).
    pub fn generate_deploy_key(&self, project: &str) -> Result<String, String> {
        let dir = self.keys_dir.join(project);
        std::fs::create_dir_all(&dir).map_err(|e| format!("keys dir: {e}"))?;
        let priv_path = dir.join("id_ed25519");
        // Regenerate-from-scratch, as the v1 did: a half-written pair from
        // a crashed attempt must never survive into a project's life.
        let _ = std::fs::remove_file(&priv_path);
        let _ = std::fs::remove_file(dir.join("id_ed25519.pub"));
        let out = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", &format!("sigiled-{project}"), "-f"])
            .arg(&priv_path)
            .output()
            .map_err(|e| format!("spawn ssh-keygen: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "ssh-keygen: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let pubkey = std::fs::read_to_string(dir.join("id_ed25519.pub"))
            .map_err(|e| format!("read pubkey: {e}"))?;
        Ok(pubkey.trim().to_string())
    }
}

/// First 300 chars of an error body — enough diagnosis for chat, never the
/// whole payload.
async fn excerpt(r: reqwest::Response) -> String {
    r.text()
        .await
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh(keys_dir: PathBuf) -> GitHub {
        GitHub {
            api_base: "http://unused".into(),
            pat: "test-pat".into(),
            owner: "ivan-saorin".into(),
            template: "vm-tmpl".into(),
            keys_dir,
        }
    }

    #[test]
    fn deploy_key_lands_in_the_runtime_layout() {
        // The runtime reads {keys_dir}/{project}/id_ed25519 (key_path):
        // the generator must produce exactly that, private key 0600.
        let dir = std::env::temp_dir().join("sigil-ghkey-test");
        let _ = std::fs::remove_dir_all(&dir);
        let pubkey = gh(dir.clone()).generate_deploy_key("smoke-new").unwrap();
        assert!(pubkey.starts_with("ssh-ed25519 "), "pubkey: {pubkey}");
        let priv_path = dir.join("smoke-new").join("id_ed25519");
        assert!(priv_path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = priv_path.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "private key must be 0600");
        }
    }
}
