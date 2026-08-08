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
    BadJob(String),
    BadWorkspace(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Toml(e) => write!(f, "sigiled.toml parse error: {e}"),
            ManifestError::BadTemplateRef(s) => {
                write!(f, "bad template ref {s:?}: expected \"<name>@<x.y.z>\"")
            }
            ManifestError::BadApp(s) => write!(f, "bad [app]: {s}"),
            ManifestError::BadJob(s) => write!(f, "bad [jobs]: {s}"),
            ManifestError::BadWorkspace(s) => write!(f, "bad [workspace]: {s}"),
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
    workspace: Option<RawWorkspace>,
    app: Option<RawApp>,
    jobs: Option<std::collections::HashMap<String, RawJob>>,
}

#[derive(Debug, Deserialize)]
struct RawWorkspace {
    dockerfile: String,
}

/// `[workspace] dockerfile = "…"` — DEC-25: the pin point for the project's
/// session image. Explicitly in the manifest, never a bare-filename
/// convention: this very repo's root Dockerfile builds vm-base (DEC-17
/// publisher role) and is NOT a session image — a convention would build
/// the wrong thing. Absent table = the global base image, the pre-DEC-25
/// behavior.
fn validate_workspace_dockerfile(raw: RawWorkspace) -> Result<String, ManifestError> {
    let bad = ManifestError::BadWorkspace;
    let p = raw.dockerfile;
    if p.is_empty() {
        return Err(bad("dockerfile must not be empty".into()));
    }
    if p.starts_with('/') || p.contains('\\') || p.split('/').any(|seg| seg == "..") {
        return Err(bad(format!(
            "dockerfile must be a relative path inside the repo, got {p:?}"
        )));
    }
    Ok(p)
}

#[derive(Debug, Deserialize)]
struct RawJob {
    cron: String,
    command: String,
    timeout_minutes: Option<u64>,
    hc_ping: Option<String>,
    #[serde(default)]
    secrets: std::collections::HashMap<String, String>,
}

/// One `[jobs.<name>]` entry (contract §7): classic 5-field cron evaluated
/// in the control plane's local TZ, a command run in a fresh workspace born
/// from master on an append-only `job-*` branch, a hard wall clock.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct JobManifest {
    pub name: String,
    /// The raw 5-field expression, validated at parse.
    pub cron: String,
    pub command: String,
    /// 1..=60, default 30 — the exec wall clock.
    pub timeout_minutes: u64,
    /// Optional STACK-ENV REF (never a URL in the repo): resolved at ping.
    pub hc_ping: Option<String>,
    /// container env ← stack env, resolved at creation (rule 8)
    pub secrets: std::collections::HashMap<String, String>,
}

impl JobManifest {
    fn validate(name: &str, raw: RawJob) -> Result<Self, ManifestError> {
        let bad = ManifestError::BadJob;
        let name_ok = !name.is_empty()
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !name_ok {
            return Err(bad(format!("bad job name {name:?}")));
        }
        // The contract speaks classic 5-field cron; the cron crate wants a
        // seconds field — pinned to 0 here, never author-visible.
        if raw.cron.split_whitespace().count() != 5 {
            return Err(bad(format!(
                "{name}: cron must be the classic 5 fields, got {:?}",
                raw.cron
            )));
        }
        use std::str::FromStr;
        cron::Schedule::from_str(&format!("0 {}", raw.cron))
            .map_err(|e| bad(format!("{name}: bad cron {:?}: {e}", raw.cron)))?;
        let timeout = raw.timeout_minutes.unwrap_or(30);
        if !(1..=60).contains(&timeout) {
            return Err(bad(format!(
                "{name}: timeout_minutes must be 1..=60, got {timeout}"
            )));
        }
        Ok(JobManifest {
            name: name.to_string(),
            cron: raw.cron,
            command: raw.command,
            timeout_minutes: timeout,
            hc_ping: raw.hc_ping,
            secrets: raw.secrets,
        })
    }
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
    /// DEC-25: the session-image dockerfile, relative to the repo root on
    /// master. None = the project rides the global base image.
    pub workspace_dockerfile: Option<String>,
    pub app: Option<AppManifest>,
    /// The `[jobs.*]` table, sorted by name for determinism. Empty when the
    /// repo declares none.
    pub jobs: Vec<JobManifest>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest =
            toml::from_str(text).map_err(|e| ManifestError::Toml(e.to_string()))?;
        let template = raw.template.as_deref().map(TemplateRef::parse).transpose()?;
        let workspace_dockerfile =
            raw.workspace.map(validate_workspace_dockerfile).transpose()?;
        let app = raw.app.map(AppManifest::validate).transpose()?;
        let mut jobs = raw
            .jobs
            .unwrap_or_default()
            .into_iter()
            .map(|(name, j)| JobManifest::validate(&name, j))
            .collect::<Result<Vec<_>, _>>()?;
        jobs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Manifest { template, workspace_dockerfile, app, jobs })
    }

    /// The manifest of a repo checkout, probing the same filename pair every
    /// other reader probes (DEC-22: `sigiled.toml`, `mgr.toml` fallback for
    /// repos born under the v1 name). Ok(None) = no manifest file at all;
    /// a file that exists but does not parse is loud, not invisible.
    pub fn from_repo(repo: &std::path::Path) -> Result<Option<Self>, String> {
        for f in ["sigiled.toml", "mgr.toml"] {
            let path = repo.join(f);
            if let Ok(text) = std::fs::read_to_string(&path) {
                return Manifest::parse(&text).map(Some).map_err(|e| format!("{f}: {e}"));
            }
        }
        Ok(None)
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
    fn jobs_table_parses_with_defaults() {
        let m = Manifest::parse(
            "[jobs.mine]\ncron = \"30 3 * * *\"\ncommand = \"./jobs/mine.sh\"\n[jobs.mine.secrets]\nKEY = \"STACK_KEY\"\n",
        )
        .unwrap();
        assert_eq!(m.jobs.len(), 1);
        let j = &m.jobs[0];
        assert_eq!(j.name, "mine");
        assert_eq!(j.cron, "30 3 * * *");
        assert_eq!(j.command, "./jobs/mine.sh");
        assert_eq!(j.timeout_minutes, 30); // default
        assert_eq!(j.hc_ping, None);
        assert_eq!(j.secrets["KEY"], "STACK_KEY");
    }

    #[test]
    fn no_jobs_table_is_an_empty_list() {
        assert!(Manifest::parse("class = \"session\"\n").unwrap().jobs.is_empty());
    }

    #[test]
    fn bad_jobs_are_loud() {
        // Cron must be the classic 5 fields and must parse; timeout must be
        // 1..=60; the job name follows project-name rules.
        let cases = [
            "[jobs.x]\ncron = \"not a cron\"\ncommand = \"c\"\n",
            "[jobs.x]\ncron = \"0 30 3 * * *\"\ncommand = \"c\"\n", // 6 fields
            "[jobs.x]\ncron = \"90 3 * * *\"\ncommand = \"c\"\n",   // minute 90
            "[jobs.x]\ncron = \"30 3 * * *\"\ncommand = \"c\"\ntimeout_minutes = 0\n",
            "[jobs.x]\ncron = \"30 3 * * *\"\ncommand = \"c\"\ntimeout_minutes = 61\n",
            "[jobs.\"Bad_Name\"]\ncron = \"30 3 * * *\"\ncommand = \"c\"\n",
        ];
        for text in cases {
            assert!(
                matches!(Manifest::parse(text), Err(ManifestError::BadJob(_))),
                "{text} should be rejected as BadJob"
            );
        }
        // A missing command is a TOML-shape error, equally loud.
        assert!(Manifest::parse("[jobs.x]\ncron = \"30 3 * * *\"\n").is_err());
    }

    #[test]
    fn workspace_dockerfile_parses_and_is_optional() {
        let m = Manifest::parse("[workspace]\ndockerfile = \"Dockerfile.session\"\n").unwrap();
        assert_eq!(m.workspace_dockerfile.as_deref(), Some("Dockerfile.session"));
        // No [workspace] = the global base image (pre-DEC-25 behavior).
        assert_eq!(Manifest::parse("class = \"session\"\n").unwrap().workspace_dockerfile, None);
    }

    #[test]
    fn workspace_dockerfile_must_stay_inside_the_repo() {
        // The path reaches `docker build -f` on the control plane's own
        // mirror checkout: escaping the repo root must die at parse.
        let bads = ["", "/etc/passwd", "../evil/Dockerfile", "a/../../b", "a\\b"];
        for bad in bads {
            let text = format!("[workspace]\ndockerfile = \"{}\"\n", bad.replace('\\', "\\\\"));
            assert!(
                matches!(Manifest::parse(&text), Err(ManifestError::BadWorkspace(_))),
                "{bad:?} should be rejected"
            );
        }
        // [workspace] without the key is a TOML-shape error, equally loud.
        assert!(Manifest::parse("[workspace]\n").is_err());
    }

    #[test]
    fn shipped_template_declares_its_session_image() {
        // DEC-25: the hook exists from birth — the template pairs its thin
        // Dockerfile with the [workspace] pin that makes sigiledd build it.
        let m = Manifest::parse(include_str!("../../template/sigiled.toml")).unwrap();
        assert_eq!(m.workspace_dockerfile.as_deref(), Some("Dockerfile"));
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
