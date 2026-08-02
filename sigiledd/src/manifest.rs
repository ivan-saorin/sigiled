// mgr.toml — the workload manifest read from a project repo's master.
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
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Toml(e) => write!(f, "mgr.toml parse error: {e}"),
            ManifestError::BadTemplateRef(s) => {
                write!(f, "bad template ref {s:?}: expected \"<name>@<x.y.z>\"")
            }
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
}

#[derive(Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The vm-tmpl pin. None on pre-v2 repos that never adopted the pin —
    /// legal: template_version simply stays null in the project record.
    pub template: Option<TemplateRef>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest =
            toml::from_str(text).map_err(|e| ManifestError::Toml(e.to_string()))?;
        let template = raw.template.as_deref().map(TemplateRef::parse).transpose()?;
        Ok(Manifest { template })
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
        let m = Manifest::parse(include_str!("../../template/mgr.toml")).unwrap();
        let t = m.template.unwrap();
        assert_eq!((t.name.as_str(), t.version.as_str()), ("vm-tmpl", "0.1.0"));
    }

    #[test]
    fn broken_toml_is_a_toml_error() {
        assert!(matches!(Manifest::parse("class = "), Err(ManifestError::Toml(_))));
    }
}
