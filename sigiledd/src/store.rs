// store.rs — the persistence the in-memory stores were shaped for: one JSON
// state file, written atomically (tmp + rename) after every mutation, loaded
// at boot. Same paradigm as the v1 registry — inspectable over SSH with the
// whole stack dead, and one small step away from the v1 import.
//
// Custody note (§1.4): the device-flow tokens DO live in this file — SIGILED
// is their keeper — with 0600 permissions; the API surface keeps exposing
// approval metadata only (ApprovalView). Corrupt state = refuse to start:
// booting blind and overwriting would be silent data loss, the operator
// restores from backup instead.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StateSnapshot {
    #[serde(default)]
    pub projects: Vec<crate::project::ProjectRecord>,
    #[serde(default)]
    pub events: HashMap<String, Vec<crate::events::LogEntry>>,
    #[serde(default)]
    pub debts: HashMap<String, Vec<crate::merge::MergeDebt>>,
    #[serde(default)]
    pub approvals: HashMap<String, crate::auth::Approval>,
    #[serde(default)]
    pub sessions: HashMap<String, crate::sessions::SessionRecord>,
    #[serde(default)]
    pub apps: HashMap<String, crate::apps::AppRecord>,
    #[serde(default)]
    pub job_runs: HashMap<String, Vec<crate::jobs::JobRunRecord>>,
}

#[derive(Clone, Default)]
pub struct Store {
    path: Option<PathBuf>,
}

impl Store {
    /// SIGILED_STATE_DIR unset = ephemeral run (dev, tests): every save is a
    /// no-op and boot starts fresh — loudly, once.
    pub fn from_env() -> Self {
        let path = std::env::var("SIGILED_STATE_DIR")
            .ok()
            .map(|d| PathBuf::from(d).join("state.json"));
        if path.is_none() {
            tracing::warn!("SIGILED_STATE_DIR not set — state is ephemeral");
        }
        Store { path }
    }

    pub fn at_dir(dir: &std::path::Path) -> Self {
        Store { path: Some(dir.join("state.json")) }
    }

    pub fn load(&self) -> Option<StateSnapshot> {
        let p = self.path.as_ref()?;
        let bytes = std::fs::read(p).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(snap) => Some(snap),
            // A state file that exists but does not parse is a stop-the-world
            // condition: starting empty would overwrite it on first mutation.
            Err(e) => panic!("corrupt state file {}: {e} — restore from backup", p.display()),
        }
    }

    pub fn save(&self, snap: &StateSnapshot) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = p.with_extension("json.tmp");
        let bytes = match serde_json::to_vec_pretty(snap) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(%e, "state serialize failed — snapshot dropped");
                return;
            }
        };
        let written = std::fs::write(&tmp, bytes).and_then(|_| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            std::fs::rename(&tmp, p)
        });
        if let Err(e) = written {
            tracing::error!(%e, "state persist failed — disk state is stale");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Approval, ApprovalState};

    fn tmp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "sigiledd-store-{}-{}-{}",
            std::process::id(),
            tag,
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> StateSnapshot {
        let mut snap = StateSnapshot::default();
        snap.approvals.insert(
            "sigiled-claude".into(),
            Approval {
                driver: "sigiled-claude".into(),
                human: "ivan".into(),
                expires_epoch: 42,
                state: ApprovalState::Granted,
                tokens: Some(serde_json::json!({"access_token": "CUSTODY-SECRET"})),
            },
        );
        snap.events.insert(
            "smoke".into(),
            vec![crate::events::LogEntry {
                at_epoch: 7,
                event: crate::events::Event::SessionOpened {
                    session_id: "abc".into(),
                    branch: "session/abc".into(),
                    stale: false,
                },
            }],
        );
        snap
    }

    #[test]
    fn round_trip_preserves_everything_atomically() {
        let dir = tmp_dir("rt");
        let store = Store::at_dir(&dir);
        store.save(&sample());
        // atomic: no tmp leftover, the real file exists
        assert!(dir.join("state.json").exists());
        assert!(!dir.join("state.json.tmp").exists());

        let loaded = store.load().unwrap();
        assert_eq!(loaded.approvals["sigiled-claude"].expires_epoch, 42);
        assert_eq!(loaded.events["smoke"].len(), 1);
        // custody: the tokens survive the round trip — SIGILED is the keeper
        assert!(loaded.approvals["sigiled-claude"].tokens.is_some());
    }

    #[test]
    fn missing_file_means_fresh_start() {
        assert!(Store::at_dir(&tmp_dir("fresh")).load().is_none());
    }

    #[test]
    fn ephemeral_store_never_writes() {
        let store = Store::default();
        store.save(&sample()); // no path: no-op, no panic
        assert!(store.load().is_none());
    }

    #[test]
    #[should_panic(expected = "corrupt state file")]
    fn corrupt_state_refuses_to_boot() {
        let dir = tmp_dir("corrupt");
        std::fs::write(dir.join("state.json"), b"{not json").unwrap();
        let _ = Store::at_dir(&dir).load();
    }
}
