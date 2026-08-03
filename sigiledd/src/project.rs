// The project registry surface. Session 1 scope: the record shape with
// template_version (DEC-05) behind GET /sigiled/projects, over an in-memory
// store. Session 2 adds template_behind (design §3: status shows it next to
// needs_merge) computed against the latest published template version.
// Persistence and the v1-registry import arrive with later sessions and the
// cutover.
use crate::manifest::Manifest;
use axum::extract::State;
use serde::Serialize;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ProjectRecord {
    pub name: String,
    /// From the sigiled.toml pin on master, e.g. "vm-tmpl@0.1.0"; null on
    /// repos that never adopted the pin.
    pub template_version: Option<String>,
    /// True when the pin trails the latest published template (design §3).
    /// An unpinned repo is not "behind": it never adopted the mechanism.
    pub template_behind: bool,
    pub needs_merge: bool,
}

impl ProjectRecord {
    pub fn new(name: &str, manifest: &Manifest, latest_template: Option<&str>) -> Self {
        let pin = manifest
            .template
            .as_ref()
            .map(|t| (t.name.clone(), t.version.clone()));
        let behind = match (&pin, latest_template) {
            (Some((_, pinned)), Some(latest)) => semver_lt(pinned, latest),
            _ => false,
        };
        ProjectRecord {
            name: name.to_string(),
            template_version: pin.map(|(n, v)| format!("{n}@{v}")),
            template_behind: behind,
            needs_merge: false,
        }
    }
}

/// x.y.z ordering, numeric per component; anything unparsable compares as 0
/// (a malformed pin never flags a project as behind by accident of parsing —
/// the manifest parser is where malformation gets loud).
fn semver_lt(a: &str, b: &str) -> bool {
    fn triple(v: &str) -> (u64, u64, u64) {
        let mut it = v.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        )
    }
    triple(a) < triple(b)
}

#[derive(Default, Clone)]
pub struct Registry {
    records: Arc<RwLock<Vec<ProjectRecord>>>,
    /// Latest published template version (e.g. "0.1.0"); None in a build
    /// that doesn't know it (records then never show behind).
    latest_template: Option<String>,
}

impl Registry {
    pub fn with_latest_template(version: Option<String>) -> Self {
        Registry {
            records: Arc::default(),
            latest_template: version,
        }
    }
    pub fn latest_template(&self) -> Option<&str> {
        self.latest_template.as_deref()
    }
    pub fn insert(&self, record: ProjectRecord) {
        self.records.write().unwrap().push(record);
    }
    pub fn contains(&self, name: &str) -> bool {
        self.records.read().unwrap().iter().any(|r| r.name == name)
    }
    pub fn snapshot(&self) -> Vec<ProjectRecord> {
        self.records.read().unwrap().clone()
    }
    pub fn replace_all(&self, records: Vec<ProjectRecord>) {
        *self.records.write().unwrap() = records;
    }
}

pub async fn list(
    _actor: crate::auth::Actor,
    State(state): State<crate::AppState>,
) -> axum::Json<serde_json::Value> {
    // Records enriched with the live merge-debt queue (design §4: status
    // shows the debt queue per project, next to needs_merge).
    let enriched: Vec<serde_json::Value> = state
        .registry
        .snapshot()
        .into_iter()
        .map(|r| {
            let mut v = serde_json::to_value(&r).unwrap();
            v["merge_debt"] = serde_json::to_value(state.sessions.debts_for(&r.name)).unwrap();
            v
        })
        .collect();
    axum::Json(serde_json::Value::Array(enriched))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_exposes_template_version_from_manifest() {
        let m = Manifest::parse("template = \"vm-tmpl@0.1.0\"\n").unwrap();
        let r = ProjectRecord::new("smoke", &m, None);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["template_version"], "vm-tmpl@0.1.0");
        assert_eq!(json["name"], "smoke");
    }

    #[test]
    fn unpinned_record_serializes_null_template_version() {
        let m = Manifest::parse("class = \"session\"\n").unwrap();
        let json = serde_json::to_value(ProjectRecord::new("legacy", &m, Some("0.2.0"))).unwrap();
        assert!(json["template_version"].is_null());
        // Unpinned is not behind: it never adopted the mechanism.
        assert_eq!(json["template_behind"], false);
    }

    #[test]
    fn pinned_behind_latest_is_flagged() {
        let m = Manifest::parse("template = \"vm-tmpl@0.1.0\"\n").unwrap();
        let r = ProjectRecord::new("smoke", &m, Some("0.2.0"));
        assert!(r.template_behind);
    }

    #[test]
    fn pinned_at_latest_is_not_behind() {
        let m = Manifest::parse("template = \"vm-tmpl@0.2.0\"\n").unwrap();
        assert!(!ProjectRecord::new("smoke", &m, Some("0.2.0")).template_behind);
        // Ahead (edge: local template newer than published) is not behind.
        let m2 = Manifest::parse("template = \"vm-tmpl@0.3.0\"\n").unwrap();
        assert!(!ProjectRecord::new("smoke", &m2, Some("0.2.0")).template_behind);
    }

    #[test]
    fn semver_compares_numerically_not_lexically() {
        assert!(semver_lt("0.9.0", "0.10.0"));
        assert!(!semver_lt("0.10.0", "0.9.0"));
        assert!(semver_lt("1.2.3", "2.0.0"));
    }
}
