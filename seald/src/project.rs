// The project registry surface. Session 1 scope: the record shape with
// template_version (DEC-05) behind GET /mgr/projects, over an in-memory
// store. Persistence and the v1-registry import arrive with later sessions
// and the cutover.
use crate::manifest::Manifest;
use axum::extract::State;
use serde::Serialize;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize)]
pub struct ProjectRecord {
    pub name: String,
    /// From the mgr.toml pin on master, e.g. "vm-tmpl@0.1.0"; null on
    /// repos that never adopted the pin.
    pub template_version: Option<String>,
    pub needs_merge: bool,
}

impl ProjectRecord {
    pub fn new(name: &str, manifest: &Manifest) -> Self {
        ProjectRecord {
            name: name.to_string(),
            template_version: manifest
                .template
                .as_ref()
                .map(|t| format!("{}@{}", t.name, t.version)),
            needs_merge: false,
        }
    }
}

#[derive(Default, Clone)]
pub struct Registry(Arc<RwLock<Vec<ProjectRecord>>>);

impl Registry {
    pub fn insert(&self, record: ProjectRecord) {
        self.0.write().unwrap().push(record);
    }
    pub fn snapshot(&self) -> Vec<ProjectRecord> {
        self.0.read().unwrap().clone()
    }
}

pub async fn list(State(registry): State<Registry>) -> axum::Json<Vec<ProjectRecord>> {
    axum::Json(registry.snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_exposes_template_version_from_manifest() {
        let m = Manifest::parse("template = \"vm-tmpl@0.1.0\"\n").unwrap();
        let r = ProjectRecord::new("mgr-smoke", &m);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["template_version"], "vm-tmpl@0.1.0");
        assert_eq!(json["name"], "mgr-smoke");
    }

    #[test]
    fn unpinned_record_serializes_null_template_version() {
        let m = Manifest::parse("class = \"session\"\n").unwrap();
        let json = serde_json::to_value(ProjectRecord::new("legacy", &m)).unwrap();
        assert!(json["template_version"].is_null());
    }
}
