// events.rs — machine layer of the two-layer log (design §2): SIGILED owns the
// mechanical history (sessions, closes with merge outcome, job runs) and
// exposes it per project at GET /sigiled/projects/{p}/log — JSON by default,
// markdown with ?format=md. Zero writes into project repos by construction.
//
// Session 2 scope: the event model, the in-memory store (same pattern as the
// project registry — persistence and the v1-registry import arrive with the
// cutover), the honest close hint (log_operativo_touched, design §2: mirror,
// not enforcement), and both renders. The verbs that will *record* these
// events (open/close/run) grow in sessions 3-4; timestamps are epoch seconds
// until the real DB brings its own.
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    SessionOpened {
        session_id: String,
        branch: String,
        stale: bool,
    },
    SessionClosed {
        session_id: String,
        merged: bool,
        sha: String,
        /// The honest hint (design §2): false when the session closed without
        /// touching docs/log-operativo.md. Mirror, not enforcement.
        log_operativo_touched: bool,
    },
    JobRun {
        job: String,
        branch: String,
        /// running · succeeded · failed · timeout · error · skipped_locked · aborted
        state: String,
    },
    /// A session imported from the v1 registry (import.rs): the v1 did not
    /// record merge outcome or touched paths per session, so the variant
    /// carries only what was true — no fabricated fields.
    V1Session {
        session_id: String,
        branch: String,
        state: String,
    },
    /// POST /projects (session 5): born from the template, or adopted as an
    /// existing repo (key + register, nothing written).
    ProjectCreated { repo: String, adopted: bool },
    /// recycle (session 6): same branch, fresh container and token — the
    /// previous driver is structurally cut off (provider handoff, §10).
    SessionRecycled { session_id: String, sha: String },
    /// The reaper (session 7, contract rule 6): idle too long — autosave
    /// flushed, container destroyed, branch left as an orphan the next
    /// open resumes stale.
    SessionReaped { session_id: String, branch: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub at_epoch: u64,
    #[serde(flatten)]
    pub event: Event,
}

/// Close computes its hint from the list of paths the session branch changed
/// vs the master it forked from. Kept pure so the close verb (session 4) and
/// tests share the exact same judgement.
pub fn log_operativo_touched<S: AsRef<str>>(changed_paths: &[S]) -> bool {
    changed_paths
        .iter()
        .any(|p| p.as_ref() == "docs/log-operativo.md")
}

#[derive(Default, Clone)]
pub struct EventLog(Arc<RwLock<HashMap<String, Vec<LogEntry>>>>);

impl EventLog {
    pub fn record(&self, project: &str, at_epoch: u64, event: Event) {
        self.0
            .write()
            .unwrap()
            .entry(project.to_string())
            .or_default()
            .push(LogEntry { at_epoch, event });
    }

    pub fn for_project(&self, project: &str) -> Vec<LogEntry> {
        self.0
            .read()
            .unwrap()
            .get(project)
            .cloned()
            .unwrap_or_default()
    }

    pub fn dump(&self) -> HashMap<String, Vec<LogEntry>> {
        self.0.read().unwrap().clone()
    }
    pub fn hydrate(&self, map: HashMap<String, Vec<LogEntry>>) {
        *self.0.write().unwrap() = map;
    }
}

fn render_markdown(project: &str, entries: &[LogEntry]) -> String {
    let mut out = format!("# {project} — machine log\n\n");
    if entries.is_empty() {
        out.push_str("_no recorded events_\n");
        return out;
    }
    for e in entries {
        let line = match &e.event {
            Event::SessionOpened {
                session_id,
                branch,
                stale,
            } => format!(
                "- [{}] session `{}` opened on `{}`{}",
                e.at_epoch,
                session_id,
                branch,
                if *stale { " (stale resume)" } else { "" }
            ),
            Event::SessionClosed {
                session_id,
                merged,
                sha,
                log_operativo_touched,
            } => format!(
                "- [{}] session `{}` closed — {} @ `{}`{}",
                e.at_epoch,
                session_id,
                if *merged {
                    "merged"
                } else {
                    "NOT merged (branch kept)"
                },
                &sha[..12.min(sha.len())],
                if *log_operativo_touched {
                    ""
                } else {
                    " — log operativo NOT touched"
                }
            ),
            Event::JobRun { job, branch, state } => {
                format!(
                    "- [{}] job `{}` run — {} (`{}`)",
                    e.at_epoch, job, state, branch
                )
            }
            Event::V1Session {
                session_id,
                branch,
                state,
            } => format!(
                "- [{}] v1 session `{}` on `{}` — {}",
                e.at_epoch, session_id, branch, state
            ),
            Event::ProjectCreated { repo, adopted } => format!(
                "- [{}] project {} (`{}`)",
                e.at_epoch,
                if *adopted {
                    "adopted"
                } else {
                    "created from template"
                },
                repo
            ),
            Event::SessionRecycled { session_id, sha } => format!(
                "- [{}] session `{}` recycled @ `{}`",
                e.at_epoch,
                session_id,
                &sha[..12.min(sha.len())]
            ),
            Event::SessionReaped { session_id, branch } => format!(
                "- [{}] session `{}` reaped for idleness — `{}` left to resume",
                e.at_epoch, session_id, branch
            ),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[derive(Deserialize)]
pub struct LogQuery {
    format: Option<String>,
}

pub async fn project_log(
    _actor: crate::auth::Actor,
    State(state): State<crate::AppState>,
    Path(project): Path<String>,
    Query(q): Query<LogQuery>,
) -> Response {
    if !state.registry.contains(&project) {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "detail": format!("unknown project: {project}")
            })),
        )
            .into_response();
    }
    let entries = state.events.for_project(&project);
    match q.format.as_deref() {
        Some("md") => (
            [("content-type", "text/markdown; charset=utf-8")],
            render_markdown(&project, &entries),
        )
            .into_response(),
        _ => axum::Json(entries).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EventLog {
        let log = EventLog::default();
        log.record(
            "smoke",
            100,
            Event::SessionOpened {
                session_id: "abc".into(),
                branch: "session/abc".into(),
                stale: false,
            },
        );
        log.record(
            "smoke",
            200,
            Event::SessionClosed {
                session_id: "abc".into(),
                merged: true,
                sha: "0123456789abcdef".into(),
                log_operativo_touched: false,
            },
        );
        log.record(
            "smoke",
            300,
            Event::JobRun {
                job: "nightly".into(),
                branch: "job-nightly-20260802-030000".into(),
                state: "succeeded".into(),
            },
        );
        log
    }

    #[test]
    fn close_hint_sees_the_log_operativo() {
        assert!(log_operativo_touched(&[
            "src/main.rs",
            "docs/log-operativo.md"
        ]));
        assert!(!log_operativo_touched(&["src/main.rs", "README.md"]));
        // Path is exact: a project nesting the name elsewhere is not a touch.
        assert!(!log_operativo_touched(&["template/docs/log-operativo.md"]));
        assert!(!log_operativo_touched::<&str>(&[]));
    }

    #[test]
    fn events_round_trip_as_tagged_json() {
        let entries = sample().for_project("smoke");
        let json = serde_json::to_value(&entries).unwrap();
        assert_eq!(json[0]["kind"], "session_opened");
        assert_eq!(json[1]["kind"], "session_closed");
        assert_eq!(json[1]["merged"], true);
        assert_eq!(json[1]["log_operativo_touched"], false);
        assert_eq!(json[2]["kind"], "job_run");
        assert_eq!(json[2]["at_epoch"], 300);
    }

    #[test]
    fn unknown_project_yields_no_entries() {
        assert!(sample().for_project("ghost").is_empty());
    }

    #[test]
    fn markdown_render_carries_outcome_and_honest_hint() {
        let md = render_markdown("smoke", &sample().for_project("smoke"));
        assert!(md.contains("# smoke — machine log"));
        assert!(md.contains("session `abc` opened on `session/abc`"));
        assert!(md.contains("merged @ `0123456789ab`"));
        assert!(md.contains("log operativo NOT touched"));
        assert!(md.contains("job `nightly` run — succeeded"));
    }

    #[test]
    fn empty_log_renders_honestly() {
        let md = render_markdown("fresh", &[]);
        assert!(md.contains("_no recorded events_"));
    }
}
