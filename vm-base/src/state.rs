use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct AppState {
    pub token: String,
    pub workspace: PathBuf,
    pub extra_roots: Vec<PathBuf>,
    pub last_activity: AtomicI64,
    pub started_at: i64,
}

pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl AppState {
    pub fn from_env() -> Self {
        let token = std::env::var("SESSION_TOKEN").expect("SESSION_TOKEN is required");
        if token.len() < 16 {
            panic!("SESSION_TOKEN too short (<16 chars)");
        }
        let workspace = PathBuf::from(
            std::env::var("WORKSPACE_DIR").unwrap_or_else(|_| "/workspace".into()),
        );
        // Declared mounts beyond /workspace, colon-separated (§5 fs scope).
        let extra_roots = std::env::var("EXTRA_PATHS")
            .unwrap_or_default()
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        let now = now_epoch();
        Self {
            token,
            workspace,
            extra_roots,
            last_activity: AtomicI64::new(now),
            started_at: now,
        }
    }
}
