// sigiled.toml — the workload manifest read from a project repo's master.
// Session 1 scope: the `template = "vm-tmpl@x.y.z"` pin (DEC-05). The jobs
// and [app] tables land with their features in later sessions.
use serde::Deserialize;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TemplateRef {
    pub name: String,
    pub version: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManifestError {
    Toml(String),
    BadTemplateRef(String),
    BadApp(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Toml(e) => write!(f, "sigiled.toml parse error: {e}"),
            ManifestError::BadTemplateRef(s) => {
                write!(f, "bad template ref {s:?}: expected \"<name>@<x.y.z>\"")
            }
            ManifestError::BadApp(s) => write!(f, "bad [app]: {s}"),
        }
    }
}

impl TemplateRef {
    /// Parses `"vm-tmpl@0.1.0"`. Name follows project-name rules (lowercase
    /// alnum+dash, letter first); version is plain semver x.y.z.
    pub fn parse(s: &str) -> Result<Self, ManifestError> {
        let bad = || ManifestError::BadTemplateRef(s.to_string());
        let (name, version) = s.split_once('@').ok_or_else(bad)?;
        let name_ok = !name.is_empty()
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let version_ok = version.split('.').count() == 3
            && version.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        if !(name_ok && version_ok) {
            return Err(bad());
        }
        Ok(TemplateRef { name: name.to_string(), version: version.to_string() })
    }
}

#[derive(Debug, Deserialize)]
struct RawManifest {
    template: Option<String>,
    app: Option<RawApp>,
}

#[derive(Debug, Deserialize)]
struct RawApp {
    name: String,
    dockerfile: Option<String>,
    #[serde(default)]
    volumes: std::collections::HashMap<String, String>,
    #[serde(default)]
    secrets: std::collections::HashMap<String, String>,
    #[serde(default)]
    requires: Vec<String>,
}

/// The resident app of a project (contract §7): at most ONE `[app]` per repo
/// (singular table, enforced by the TOML shape), no `image` key by design —
/// the repo IS the filesystem, the image is built from it at master.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct AppManifest {
    /// Container name = DNS name on the stack network: the edge routes it.
    pub name: String,
    pub dockerfile: String,
    /// named volume → "/abs/target:ro|rw"
    pub volumes: std::collections::HashMap<String, String>,
    /// container env ← stack env, resolved at creation (rule 8)
    pub secrets: std::collections::HashMap<String, String>,
    pub requires: Vec<String>,
}

impl AppManifest {
    fn validate(raw: RawApp) -> Result<Self, ManifestError> {
        let name_ok = !raw.name.is_empty()
            && raw.name.starts_with(|c: char| c.is_ascii_lowercase())
            && raw
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !name_ok {
            return Err(ManifestError::BadApp(format!("bad app name {:?}", raw.name)));
        }
        for (vol, target) in &raw.volumes {
            let ok = target.starts_with('/')
                && (target.ends_with(":ro") || target.ends_with(":rw"))
                && target.len() > 4;
            if !ok {
                return Err(ManifestError::BadApp(format!(
                    "volume {vol}: expected \"/abs/target:ro|rw\", got {target:?}"
                )));
            }
        }
        Ok(AppManifest {
            name: raw.name,
            dockerfile: raw.dockerfile.unwrap_or_else(|| "Dockerfile".into()),
            volumes: raw.volumes,
            secrets: raw.secrets,
            requires: raw.requires,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The vm-tmpl pin. None on pre-v2 repos that never adopted the pin —
    /// legal: template_version simply stays null in the project record.
    pub template: Option<TemplateRef>,
    pub app: Option<AppManifest>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest =
            toml::from_str(text).map_err(|e| ManifestError::Toml(e.to_string()))?;
        let template = raw.template.as_deref().map(TemplateRef::parse).transpose()?;
        let app = raw.app.map(AppManifest::validate).transpose()?;
        Ok(Manifest { template, app })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_manifest_yields_template_version() {
        let m = Manifest::parse(
            "class = \"session\"\ntemplate = \"vm-tmpl@0.1.0\"\n[volumes]\n[secrets]\n",
        )
        .unwrap();
        let t = m.template.unwrap();
        assert_eq!(t.name, "vm-tmpl");
        assert_eq!(t.version, "0.1.0");
    }

    #[test]
    fn unpinned_manifest_is_legal() {
        let m = Manifest::parse("class = \"session\"\n").unwrap();
        assert_eq!(m.template, None);
    }

    #[test]
    fn malformed_refs_are_rejected_not_guessed() {
        let bads = [
            "vm-tmpl",
            "vm-tmpl@",
            "@0.1.0",
            "vm-tmpl@0.1",
            "vm-tmpl@a.b.c",
            "VM-TMPL@0.1.0",
            "1tmpl@0.1.0",
            "vm-tmpl@0.1.0.9",
        ];
        for bad in bads {
            assert!(
                matches!(TemplateRef::parse(bad), Err(ManifestError::BadTemplateRef(_))),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn shipped_template_manifest_parses_with_pin() {
        // The template we generate projects from must never regress against
        // our own parser.
        let m = Manifest::parse(include_str!("../../template/sigiled.toml")).unwrap();
        let t = m.template.unwrap();
        assert_eq!((t.name.as_str(), t.version.as_str()), ("vm-tmpl", "0.1.0"));
    }

    #[test]
    fn broken_toml_is_a_toml_error() {
        assert!(matches!(Manifest::parse("class = "), Err(ManifestError::Toml(_))));
    }

    #[test]
    fn app_table_parses_with_defaults() {
        let m = Manifest::parse(
            "[app]\nname = \"reddit-mine\"\n[app.volumes]\nreddit-mine-data = \"/data:rw\"\n[app.secrets]\nTZ = \"TZ\"\n",
        )
        .unwrap();
        let a = m.app.unwrap();
        assert_eq!(a.name, "reddit-mine");
        assert_eq!(a.dockerfile, "Dockerfile"); // default
        assert_eq!(a.volumes["reddit-mine-data"], "/data:rw");
        assert_eq!(a.secrets["TZ"], "TZ");
        assert!(a.requires.is_empty());
    }

    #[test]
    fn app_without_table_is_none_and_bad_apps_are_loud() {
        assert_eq!(Manifest::parse("class = \"session\"\n").unwrap().app, None);
        // name rules
        assert!(matches!(
            Manifest::parse("[app]\nname = \"Bad_Name\"\n"),
            Err(ManifestError::BadApp(_))
        ));
        // volume must be absolute with :ro|:rw
        for v in ["data:rw", "/data", "/data:xx"] {
            let text = format!("[app]\nname = \"x\"\n[app.volumes]\nd = \"{v}\"\n");
            assert!(
                matches!(Manifest::parse(&text), Err(ManifestError::BadApp(_))),
                "{v} should be rejected"
            );
        }
    }

    #[test]
    fn template_app_reference_table_still_parses() {
        // The commented [app] reference in the shipped template must stay
        // commented (parse → app None); if someone uncomments it, it must
        // still be valid. Both invariants in one place.
        let m = Manifest::parse(include_str!("../../template/sigiled.toml")).unwrap();
        assert!(m.app.is_none(), "il template non deve dichiarare app di default");
    }
}
