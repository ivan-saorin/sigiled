// reaper.rs — contract rule 6 made real (session 7): ~1h without workspace
// activity and the session is auto-closed the SAFE way — autosave flushed
// through the agent with the custodied token, container destroyed, record
// dropped. Nothing merges: the branch stays on origin as an orphan, and the
// next open() on the project resumes it stale (sessions::find_orphan).
//
// The loop lives only where a runtime does: a dev run without docker has no
// containers to reap, and the branch-only tests call reap() directly.

/// Poll cadence and idle threshold, env-tunable so a live verification does
/// not need to wait an hour (SIGILED_REAPER_POLL_SECS / _IDLE_SECS).
fn env_secs(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The endless loop main() spawns next to the server.
pub async fn run(state: crate::AppState) {
    let poll = env_secs("SIGILED_REAPER_POLL_SECS", 60);
    let idle_max = env_secs("SIGILED_REAPER_IDLE_SECS", 3600);
    tracing::info!(poll, idle_max, "reaper running");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(poll)).await;
        reap_pass(&state, idle_max).await;
    }
}

/// One sweep: ask every live workspace how long it has been idle, reap the
/// ones over the threshold. Unreachable agents are skipped loudly — a
/// transient network hiccup must not destroy a working container.
pub async fn reap_pass(state: &crate::AppState, idle_max: u64) -> usize {
    let Some(rt) = state.sessions.runtime.clone() else { return 0 };
    let mut reaped = 0;
    for rec in state.sessions.live_records() {
        let Some(tok) = &rec.token else { continue };
        let vm = crate::runtime::Runtime::vm_name(&rec.project);
        match rt.idle_secs(state.sessions.http(), &vm, tok).await {
            Ok(idle) if idle >= idle_max => {
                reap(state, &rec.session_id, &format!("idle {idle}s")).await;
                reaped += 1;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(
                session = %rec.session_id,
                project = %rec.project,
                error = %e,
                "reaper: agent unreachable — skipped, not destroyed"
            ),
        }
    }
    reaped
}

/// Auto-close one session the reaper way: flush, destroy, drop the record.
/// The branch survives on the repo as the orphan open() will resume.
pub async fn reap(state: &crate::AppState, session_id: &str, reason: &str) {
    let Some(record) = state.sessions.record(session_id) else { return };
    if let Some(rt) = &state.sessions.runtime {
        let vm = crate::runtime::Runtime::vm_name(&record.project);
        if let Some(tok) = &record.token {
            let flushed = rt
                .flush(state.sessions.http(), &vm, tok, &format!("reaper {reason}"))
                .await;
            if !flushed {
                tracing::warn!(
                    session = session_id,
                    "reaper flush failed — only already-pushed work survives"
                );
            }
        }
        rt.destroy(&vm);
    }
    state.sessions.remove_record(session_id);
    state.events.record(
        &record.project,
        crate::auth::now_epoch(),
        crate::events::Event::SessionReaped {
            session_id: session_id.to_string(),
            branch: record.branch.clone(),
        },
    );
    state.persist();
    tracing::info!(session = session_id, project = %record.project, reason, "session reaped");
}
