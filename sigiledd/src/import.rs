// import.rs — the one-shot v1 → v2 migration (cutover §6 step 2):
//
//   docker run --rm -v mgr-data:/v1:ro -v sigiled-data:/data \
//       sigiled-sigiled import /v1
//
// Reads the v1 registry.json (projects / sessions / job_runs) and keys/,
// merges into the v2 state file and key store, and prints a report. The
// tombstones (projects that exist only because v1 has no delete verb) are
// skipped by name. Idempotent: projects merge by name, a project whose
// machine log already has entries keeps it (no duplicated history), keys
// never overwrite. The v1 side is never written — mount it read-only.
use crate::events::{Event, LogEntry};
use crate::project::ProjectRecord;
use crate::store::{StateSnapshot, Store};
use std::path::Path;

pub const TOMBSTONES: [&str; 2] = ["seal", "seal-supervisor"];

pub fn run(v1_dir: &Path) -> Result<String, String> {
    let registry: serde_json::Value = serde_json::from_slice(
        &std::fs::read(v1_dir.join("registry.json"))
            .map_err(|e| format!("read v1 registry: {e}"))?,
    )
    .map_err(|e| format!("parse v1 registry: {e}"))?;

    let store = Store::from_env();
    let mut snap = store.load().unwrap_or_default();
    let mut report = String::from("== import v1 → v2\n");

    import_projects(&registry, &mut snap, &mut report);
    import_history(&registry, &mut snap, &mut report);
    import_keys(v1_dir, &mut report)?;

    store.save(&snap);
    report.push_str("== stato v2 scritto\n");
    Ok(report)
}

fn import_projects(registry: &serde_json::Value, snap: &mut StateSnapshot, report: &mut String) {
    let Some(projects) = registry["projects"].as_object() else { return };
    let (mut imported, mut skipped) = (0, Vec::new());
    for (name, p) in projects {
        if TOMBSTONES.contains(&name.as_str()) {
            skipped.push(name.clone());
            continue;
        }
        let record = ProjectRecord {
            name: name.clone(),
            // The pin lives in the manifest on master; the registry does not
            // know it. Filled in the day the project re-pins (sync).
            template_version: None,
            template_behind: false,
            needs_merge: p["needs_merge"].as_bool().unwrap_or(false),
        };
        snap.projects.retain(|r| r.name != *name);
        snap.projects.push(record);
        imported += 1;
        if let Some(resume) = p.get("resume").filter(|r| !r.is_null()) {
            report.push_str(&format!(
                "   ATTENZIONE {name}: branch v1 da riprendere: {} (last_commit {})\n",
                resume["branch"].as_str().unwrap_or("?"),
                &resume["last_commit"].as_str().unwrap_or("?")[..12.min(
                    resume["last_commit"].as_str().unwrap_or("?").len()
                )],
            ));
        }
    }
    snap.projects.sort_by(|a, b| a.name.cmp(&b.name));
    report.push_str(&format!(
        "   progetti: {imported} importati, lapidi saltate: {}\n",
        skipped.join(", ")
    ));
}

fn import_history(registry: &serde_json::Value, snap: &mut StateSnapshot, report: &mut String) {
    let mut sessions = 0;
    let mut runs = 0;
    let mut touched: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Idempotency: a project whose log already has entries keeps it.
    let has_history = |snap: &StateSnapshot, p: &str| {
        snap.events.get(p).map(|v| !v.is_empty()).unwrap_or(false)
    };
    let frozen: std::collections::HashSet<String> =
        snap.events.iter().filter(|(_, v)| !v.is_empty()).map(|(k, _)| k.clone()).collect();
    let _ = has_history; // documented above; the frozen set is the check

    if let Some(ss) = registry["sessions"].as_object() {
        for s in ss.values() {
            let project = s["project"].as_str().unwrap_or_default().to_string();
            if project.is_empty()
                || TOMBSTONES.contains(&project.as_str())
                || frozen.contains(&project)
            {
                continue;
            }
            let at = s["closed_at"]
                .as_str()
                .or_else(|| s["created_at"].as_str())
                .and_then(iso_epoch)
                .unwrap_or(0);
            snap.events.entry(project.clone()).or_default().push(LogEntry {
                at_epoch: at,
                event: Event::V1Session {
                    session_id: s["id"].as_str().unwrap_or("?").into(),
                    branch: s["branch"].as_str().unwrap_or("?").into(),
                    state: s["state"].as_str().unwrap_or("?").into(),
                },
            });
            touched.insert(project);
            sessions += 1;
        }
    }
    if let Some(jr) = registry["job_runs"].as_object() {
        for r in jr.values() {
            let project = r["project"].as_str().unwrap_or_default().to_string();
            if project.is_empty()
                || TOMBSTONES.contains(&project.as_str())
                || frozen.contains(&project)
            {
                continue;
            }
            let at = r["finished_at"]
                .as_str()
                .or_else(|| r["started_at"].as_str())
                .and_then(iso_epoch)
                .unwrap_or(0);
            snap.events.entry(project.clone()).or_default().push(LogEntry {
                at_epoch: at,
                event: Event::JobRun {
                    job: r["job"].as_str().unwrap_or("?").into(),
                    branch: r["branch"].as_str().unwrap_or("?").into(),
                    state: r["state"].as_str().unwrap_or("?").into(),
                },
            });
            touched.insert(project);
            runs += 1;
        }
    }
    for p in touched {
        if let Some(v) = snap.events.get_mut(&p) {
            v.sort_by_key(|e| e.at_epoch);
        }
    }
    report.push_str(&format!("   storia: {sessions} sessioni v1, {runs} job run\n"));
}

fn import_keys(v1_dir: &Path, report: &mut String) -> Result<(), String> {
    let src_root = v1_dir.join("keys");
    let dst_root = std::path::PathBuf::from(
        std::env::var("SIGILED_KEYS_DIR").unwrap_or_else(|_| {
            format!("{}/keys", std::env::var("SIGILED_STATE_DIR").unwrap_or_else(|_| "/data".into()))
        }),
    );
    let (mut copied, mut kept) = (0, 0);
    let entries = std::fs::read_dir(&src_root).map_err(|e| format!("v1 keys dir: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if TOMBSTONES.contains(&name.as_str()) || !entry.path().is_dir() {
            continue;
        }
        let dst = dst_root.join(&name);
        if dst.exists() {
            kept += 1;
            continue;
        }
        std::fs::create_dir_all(&dst).map_err(|e| format!("mkdir {name}: {e}"))?;
        for f in std::fs::read_dir(entry.path()).map_err(|e| format!("{name}: {e}"))?.flatten() {
            let to = dst.join(f.file_name());
            std::fs::copy(f.path(), &to).map_err(|e| format!("copy {name}: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let private = !f.file_name().to_string_lossy().ends_with(".pub");
                let mode = if private { 0o600 } else { 0o644 };
                let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(mode));
            }
        }
        copied += 1;
    }
    report.push_str(&format!("   chiavi: {copied} copiate, {kept} già presenti\n"));
    Ok(())
}

/// "YYYY-MM-DDTHH:MM:SS…" → unix epoch (UTC). Days via the civil-date
/// algorithm; no chrono for one timestamp format the v1 always wrote UTC.
fn iso_epoch(s: &str) -> Option<u64> {
    if s.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + h * 3600 + mi * 60 + se;
    (secs >= 0).then_some(secs as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1_fixture() -> serde_json::Value {
        serde_json::json!({
            "projects": {
                "torchio": {"name": "torchio", "needs_merge": true, "resume": null},
                "seal": {"name": "seal", "needs_merge": false, "resume": null},
                "oannes-one": {"name": "oannes-one", "needs_merge": false,
                    "resume": {"branch": "session/206f26ec", "last_commit": "69704fc90aad6ee13f0d"}},
            },
            "sessions": {
                "a1": {"id": "a1", "project": "torchio", "branch": "session/a1",
                       "state": "closed", "created_at": "2026-07-20T10:00:00+00:00",
                       "closed_at": "2026-07-20T11:30:00+00:00"},
                "a2": {"id": "a2", "project": "seal", "branch": "session/a2",
                       "state": "closed", "created_at": "2026-07-01T00:00:00+00:00"},
            },
            "job_runs": {
                "r1": {"id": "r1", "project": "torchio", "job": "nightly",
                       "branch": "job-nightly-20260721-030000", "state": "succeeded",
                       "finished_at": "2026-07-21T03:05:00+00:00"},
            },
        })
    }

    #[test]
    fn epoch_parser_matches_known_dates() {
        assert_eq!(iso_epoch("1970-01-01T00:00:00"), Some(0));
        assert_eq!(iso_epoch("2000-01-01T00:00:00+00:00"), Some(946_684_800));
        assert_eq!(iso_epoch("2026-01-01T00:00:00"), Some(1_767_225_600));
        assert_eq!(iso_epoch("garbage"), None);
        assert_eq!(iso_epoch("2026-13-01T00:00:00"), None);
    }

    #[test]
    fn tombstones_are_skipped_everywhere() {
        let mut snap = StateSnapshot::default();
        let mut report = String::new();
        import_projects(&v1_fixture(), &mut snap, &mut report);
        import_history(&v1_fixture(), &mut snap, &mut report);
        let names: Vec<&str> = snap.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["oannes-one", "torchio"]);
        assert!(!snap.events.contains_key("seal"));
        assert!(report.contains("lapidi saltate: seal"));
    }

    #[test]
    fn history_maps_and_sorts_by_time() {
        let mut snap = StateSnapshot::default();
        let mut report = String::new();
        import_history(&v1_fixture(), &mut snap, &mut report);
        let ev = &snap.events["torchio"];
        assert_eq!(ev.len(), 2);
        assert!(ev[0].at_epoch < ev[1].at_epoch);
        assert!(matches!(ev[0].event, Event::V1Session { .. }));
        assert!(matches!(ev[1].event, Event::JobRun { .. }));
    }

    #[test]
    fn needs_merge_and_resume_survive_the_crossing() {
        let mut snap = StateSnapshot::default();
        let mut report = String::new();
        import_projects(&v1_fixture(), &mut snap, &mut report);
        let t = snap.projects.iter().find(|p| p.name == "torchio").unwrap();
        assert!(t.needs_merge);
        assert!(report.contains("oannes-one"), "resume non segnalato: {report}");
        assert!(report.contains("session/206f26ec"));
    }

    #[test]
    fn reimport_does_not_duplicate_history() {
        let mut snap = StateSnapshot::default();
        let mut report = String::new();
        import_history(&v1_fixture(), &mut snap, &mut report);
        import_history(&v1_fixture(), &mut snap, &mut report);
        assert_eq!(snap.events["torchio"].len(), 2, "storia duplicata al secondo giro");
    }
}
