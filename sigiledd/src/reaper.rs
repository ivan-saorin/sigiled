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
    let Some(rt) = state.sessions.runtime.clone() else {
        return 0;
    };
    let mut reaped = 0;
    for rec in state.sessions.live_records() {
        let Some(tok) = &rec.token else { continue };
        let vm = rec.container();
        match rt.idle_secs(state.sessions.http(), &vm, tok).await {
            Ok(idle) if idle >= idle_max => {
                if reap_generation(
                    state,
                    &rec.session_id,
                    &format!("idle {idle}s"),
                    Some(rec.generation),
                )
                .await
                {
                    reaped += 1;
                }
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
#[cfg(test)]
pub async fn reap(state: &crate::AppState, session_id: &str, reason: &str) -> bool {
    reap_generation(state, session_id, reason, None).await
}
pub(crate) async fn reap_generation(
    state: &crate::AppState,
    session_id: &str,
    reason: &str,
    expected: Option<u64>,
) -> bool {
    use crate::sessions::{Failure, Lifecycle};
    let lock = state.sessions.session_lock(session_id);
    let _guard = lock.lock().await;
    let Some(record) = state.sessions.record(session_id) else {
        return false;
    };
    if expected.is_some_and(|g| g != record.generation || record.lifecycle != Lifecycle::Active) {
        return false;
    }
    if record.token.is_some() && state.sessions.runtime.is_none() {
        crate::sessions::failure(state, session_id, Failure::RuntimeUnavailable);
        return false;
    }
    if !state.sessions.binding_safe(&record) {
        crate::sessions::failure(state, session_id, Failure::LegacyOwnershipAmbiguous);
        return false;
    }
    if state.sessions.runtime.is_some() && (!record.runtime_owned || record.token.is_none()) {
        crate::sessions::failure(state, session_id, Failure::RuntimeNotOwned);
        return false;
    }
    state.sessions.mark(session_id, Lifecycle::Closing, None);
    if state.try_persist().is_err() {
        crate::sessions::failure(state, session_id, Failure::PersistFailed);
        return false;
    }
    if let Some(rt) = &state.sessions.runtime {
        let Some(tok) = &record.token else {
            crate::sessions::failure(state, session_id, Failure::FlushFailed);
            return false;
        };
        if !rt
            .flush(
                state.sessions.http(),
                &record.container(),
                tok,
                "session reap",
            )
            .await
        {
            crate::sessions::failure(state, session_id, Failure::FlushFailed);
            return false;
        }
        if !rt.destroy(&record.container()) {
            crate::sessions::failure(state, session_id, Failure::CleanupFailed);
            return false;
        }
    }
    state.sessions.remove_record(session_id);
    state.events.record(
        &record.project,
        crate::auth::now_epoch(),
        crate::events::Event::SessionReaped {
            session_id: session_id.into(),
            branch: record.branch,
        },
    );
    state.persist();
    tracing::info!(session=session_id,project=%record.project,reason,"session reaped");
    true
}
