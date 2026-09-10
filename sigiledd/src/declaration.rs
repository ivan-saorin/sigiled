use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMetadata {
    pub display_name: Option<String>,
    pub description: Option<String>,
}
fn enabled() -> bool {
    true
}
fn provider() -> String {
    "code-server".into()
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ide {
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default = "provider")]
    pub provider: String,
}
impl Default for Ide {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: provider(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Memory {
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub sources: Vec<String>,
    // None means retain an existing policy, private for a new project.
    pub sharing: Option<String>,
    #[serde(default)]
    pub source_code: bool,
}
impl Default for Memory {
    fn default() -> Self {
        Self {
            enabled: true,
            sources: vec![],
            sharing: None,
            source_code: false,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceCapabilities {
    pub version: u32,
    pub operations: Option<String>,
    pub health: Option<String>,
    pub ui: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub name: String,
    pub purpose: String,
    pub origin: String,
    pub gate: String,
    pub status: String,
    pub capabilities: Option<ServiceCapabilities>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Declaration {
    pub project: ProjectMetadata,
    pub ide: Ide,
    pub memory: Memory,
    pub service: Option<Service>,
}
fn text_ok(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
pub fn relative_path(s: &str, route: bool) -> bool {
    let p = if route {
        s.strip_prefix('/').unwrap_or(s)
    } else {
        s
    };
    !p.is_empty()
        && p.len() <= 256
        && !p.starts_with('/')
        && !p.contains(['\\', ':', '?', '#', '%'])
        && !p.chars().any(|c| c.is_control() || c.is_whitespace())
        && p.split('/').all(|v| !v.is_empty() && v != "." && v != "..")
}
impl Declaration {
    pub fn validate(&self, app: Option<&crate::manifest::AppManifest>) -> Result<(), String> {
        if self
            .project
            .display_name
            .as_ref()
            .is_some_and(|s| !text_ok(s, 100))
            || self
                .project
                .description
                .as_ref()
                .is_some_and(|s| !text_ok(s, 1000))
        {
            return Err("invalid project metadata".into());
        }
        if self.ide.provider != "code-server" {
            return Err("unsupported IDE provider".into());
        }
        if self.memory.sources.len() > 64
            || self.memory.sources.iter().any(|s| !relative_path(s, false))
        {
            return Err("memory sources must be bounded repository-relative paths".into());
        }
        if self
            .memory
            .sharing
            .as_deref()
            .is_some_and(|s| !["private", "project"].contains(&s))
        {
            return Err("unsupported memory sharing".into());
        }
        if let Some(s) = &self.service {
            if !crate::project::valid_name(&s.name)
                || !text_ok(&s.purpose, 300)
                || app.map(|a| a.name.as_str()) != Some(s.name.as_str())
            {
                return Err(
                    "service must name its declared app and include a public purpose".into(),
                );
            }
            if !["stack-bearer", "service-token", "sso-only", "edge-open"]
                .contains(&s.gate.as_str())
                || !["live", "building", "planned"].contains(&s.status.as_str())
            {
                return Err("unsupported service gate or status".into());
            }
            let u = reqwest::Url::parse(&s.origin).map_err(|_| "invalid service origin")?;
            if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some()
                || u.port().is_some() || u.query().is_some() || u.fragment().is_some()
                || u.path() != "/" || u.host_str().is_none()
                // URL canonicalization hides explicit :443. Require canonical origin spelling.
                || s.origin.trim_end_matches('/') != u.origin().ascii_serialization()
            {
                return Err(
                    "service origin must be canonical HTTPS without credentials, port or path"
                        .into(),
                );
            }
            if let Some(c) = &s.capabilities {
                if c.version != 1
                    || [&c.operations, &c.health, &c.ui]
                        .iter()
                        .any(|p| p.as_ref().is_some_and(|v| !relative_path(v, true)))
                {
                    return Err("unsupported capabilities or unsafe route".into());
                }
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_and_new_manifest_fields_are_additive() {
        let old = crate::manifest::Manifest::parse("").unwrap();
        assert!(old.declaration.ide.enabled && old.declaration.memory.enabled);
        let text = r#"
[project]
display_name="Example"
description="Private description"
[workspace]
dockerfile="Dockerfile.session"
[ide]
enabled=false
[memory]
sources=["docs", "README.md"]
sharing="project"
source_code=true
[app]
name="example"
[service]
name="example"
purpose="Public purpose"
origin="https://example.stack.test"
gate="stack-bearer"
status="planned"
[service.capabilities]
version=1
operations="/openapi.json"
health="/healthz"
ui="/home"
"#;
        let new = crate::manifest::Manifest::parse(text).unwrap();
        assert_eq!(
            new.workspace_dockerfile.as_deref(),
            Some("Dockerfile.session")
        );
        assert!(!new.declaration.ide.enabled);
        assert_eq!(new.declaration.memory.sharing.as_deref(), Some("project"));
        assert!(new.declaration.service.is_some());
    }
    #[test]
    fn unsafe_sources_routes_endpoints_and_unknown_modes_are_rejected() {
        for source in [
            "../secret",
            "/etc/passwd",
            "C:/secret",
            "https://example.com",
            "docs/../secret",
            "a\\b",
            "a/%2e%2e/b",
            ".",
            "a//b",
        ] {
            assert!(!relative_path(source, false), "{source}");
        }
        for origin in [
            "http://example.stack.test",
            "https://user:pw@example.stack.test",
            "https://example.stack.test:443",
            "https://example.stack.test:9999",
            "https://example.stack.test/path",
            "https://example.stack.test?q=a",
            "https://example.stack.test#x",
        ] {
            let text=format!("[app]\nname='example'\n[service]\nname='example'\npurpose='Published'\ngate='edge-open'\nstatus='planned'\norigin='{origin}'\n");
            assert!(crate::manifest::Manifest::parse(&text).is_err(), "{origin}");
        }
        for text in [
            "[ide]\ndockerfile='other'",
            "[memory]\nsharing='all'",
            "[memory]\nenabled='yes'",
            "[project]\ndisplay_name=''",
            "[memory]\nsources=['a\u{0001}']",
        ] {
            assert!(crate::manifest::Manifest::parse(text).is_err(), "{text}");
        }
    }
}
