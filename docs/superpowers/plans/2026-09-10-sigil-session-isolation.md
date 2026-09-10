# Task A1 — independent, recoverable Sigil sessions

Read this first. These are the requirements for this task, not the entire OS implementation.

## Ownership and environment

You own the `sigiled` session backend and its focused tests/docs: `sigiledd/src/sessions.rs`, `runtime.rs`, `reaper.rs`, `store.rs`, `main.rs`, necessary new support modules/dependencies, and lifecycle documentation. Do not implement project registry/UI/IDE/memory in this task. You are not alone in the codebase: do not revert others' changes, and accommodate them. Do not spawn subagents. Do not open, recycle, close, or deploy a Sigil project. The parent has already opened project `sigiled` on branch `session/877a5495`, starting at `f05cbbe6f4f69829f778c11405182732b3794d40`, read its Git handoff, verified no merge debt, and queried changes (none since the latest inspection).

Use the existing API helper at `work/sigil_client.py` from the current Windows workspace. The private live session record is `work/sigil-session-sigiled.json`; never print credentials or tokens. It reads the driver's credential from the installed Sigil skill and mints short-lived tokens without displaying them. Command input is JSON from stdin. Examples:

```powershell
@'
{"mode":"workspace","project":"sigiled","path":"/fs/read","method":"POST","data":{"path":"/workspace/sigiledd/src/sessions.rs"}}
'@ | python -u -X utf8 work/sigil_client.py
```

First-class `/fs/read`, `/fs/write`, `/git/status`, `/git/diff`, and `/git/commit` are preferred. `/exec` is for tooling/build/tests, full bash inside the sealed container, cwd `/workspace`. No host/docker/SSH access is possible or permitted through workspace exec. Code under test can use fake container commands/HTTP servers; never exercise production container lifecycle from tests. Normal commits automatically push to this isolated session branch; master only moves when the parent closes. The parent will commit the approved design and task plan before you begin code.

Local working copies under `work/` are allowed, but changes must be written through `/fs/write` to the remote session and compiled/tested there. The container has cargo. Existing tests are inline in modules, and workspace members are `sigiledd` and `vm-base`. Use a separate report at `work/foundation-report.md`. For long remote builds use a shell job that writes to a git-ignored `target/` log and poll the log; do not repeatedly start duplicate builds on timeout. Ensure test fixtures/scratch files never get committed by `/git/commit` (which stages all).

## Goal

Make each session's lifecycle target its own runtime and branch; prevent loss on flush failure; supply redacted session inspection APIs. Existing snapshot data and returned-endpoint clients remain compatible. New human IDE sessions must eventually coexist with agent sessions without replacing them; this task establishes that prerequisite only.

## Confirmed defect

`Runtime::vm_name(project)` returns `vm-{project}`. `sessions::open` passes that to `create_container`, which calls `destroy(container)` first. `close`, `recycle`, and `reaper` also derive container identity from project. Opening a second session therefore replaces the first. Do not reproduce it against production.

## Required behavior

1. **Unique runtime identity.** Persist container name and endpoint (or a versioned binding struct with those fields) per session. New names include the session ID, such as `vm-{project}-{id}`. New endpoint `https://api.<domain>/s/{project}-{id}/` works with the existing edge's `/s/<slug>` → `vm-<slug>:8000` rule, so do not require an edge reload to fix isolation. `Runtime::vm_name(project)` can remain for legacy identity and project image names; new session lifecycle must use the recorded binding. No destructive pre-clean of an arbitrary existing runtime: create collisions fail safely rather than removing the incumbent. Keep job/app naming contracts compatible.
2. **Legacy snapshots.** Add serde defaults for old records. A record without binding is treated as legacy and resolves only its old `vm-{project}`/endpoint. Conflicting legacy records for the same project must not all operate on the same container. Reconcile/deny ambiguous legacy ownership safely, recording a clear recoverable state. Never destroy a container because a different record claims the project. New records must not use legacy aliases.
3. **Lifecycle concurrency.** Serialize state transitions for one session; serialize mirror preparation/branch allocation and merge operations for a project. Acquire locks in a consistent order. Hold the mirror lock over all mirror resets/fetch/merge/push actions, not just `close_merge`. Opening two sessions must allocate different branches and runtime names even when orphan-resume logic is active. The unrelated workspace execution itself stays independent.
4. **Failure preservation.** If flush, checkpoint push, branch fetch, or master push fails, return a typed failure and retain recoverable session/container/branch state; do not report `closed` or destroy recoverable work. Reaper and recycle obey the same rule. Only remove this session's runtime after its data is safe. Failed container creation/boot cleans up only a newly acquired runtime belonging to that operation, never an incumbent.
5. **Generation and status.** Expose a modest explicit lifecycle state and generation with backward-compatible defaults, sufficient to distinguish creating/active/recycling/closing/failed/reaped or closed records as appropriate to the existing architecture. Persist a creating record before allocating resources; after restart, surface interrupted transitions honestly. Do not introduce a scheduler or rewrite the store. New tokens and session IDs must use a cryptographically secure OS RNG; a small motivated dependency already transitive in the lockfile is fine.
6. **Inspection APIs.** Implement authenticated `GET /sigiled/sessions` with optional project filtering and `GET /sigiled/sessions/{id}`. Serialize safe fields only: session ID, project, branch, actor, lifecycle state, generation, endpoint, runtime/image summary where known, and errors that contain no secrets. NEVER serialize workspace tokens. Inspection must not open sessions, execute repository commands, or keep idle workspaces alive. Access must enforce actual actor ownership for driver mutation operations; admins can manage all sessions. Do not leak session credentials through logs or error strings. Additive list/detail APIs can expose permitted operational metadata consistent with existing project visibility.
7. **Contract.** Update lifecycle/endpoint documentation for the new binding and migration. Do not change the machine auth contract or invent a new browser auth system here. Keep the current v2 command semantics and merge-debt priority.

## Interfaces proposed (adjust only where existing code makes a smaller compatible form clearly better; record adjustments)

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceBinding {
    pub container: String,
    pub endpoint: String,
    #[serde(default)]
    pub generation: u64,
}

// SessionRecord keeps its existing token field for internal persistence;
// add an optional binding with serde default, never derive public responses
// from the raw internal record without an explicit safe projection.
// New runtime helpers accept the session identity explicitly.
pub fn session_container(project: &str, session_id: &str) -> String;
pub fn session_endpoint(&self, project: &str, session_id: &str) -> String;
```

Review exact definitions before choosing a model. The binding response should not force clients to reconstruct URLs; `open` and `recycle` return the authoritative endpoint.

## Test-first sequence

- [ ] Read production lifecycle methods and existing tests to map actual side effects.
- [ ] Add a regression exercising two opens through the real handler with a controlled runtime/command boundary. It must fail against current project-only naming without touching Docker. Assert distinct runtime targets and that the first remains usable. A source-text grep is not a test.
- [ ] Add behavior tests for legacy record decoding/ambiguous ownership, targeting on close/recycle/reaper, flush failure preservation, duplicate lifecycle requests, mirror preparation serialization, token redaction, unknown session, and driver/admin mutation authority. Cover meaningful boundaries rather than private struct layout.
- [ ] Run the new tests and record the expected failing evidence before implementation.
- [ ] Implement minimal compatible binding/locking/lifecycle/inspection behavior.
- [ ] Run focused tests to green; then `cargo test --workspace` and formatting/Clippy checks appropriate to changed modules. Diagnose pre-existing failures rather than silently suppressing them.
- [ ] Update the contract/runbook and top of `docs/log-operativo.md` with what changed, why, verification, rollout requirements, and remaining scope. State explicitly that implementation is not deployed.
- [ ] Review `/git/diff` and `/git/status` for unrelated/scratch files, then commit coherent steps with intent-carrying messages via `/git/commit`.
- [ ] Write the report with commits, exact test commands/results, adjustments, remaining limits, and files changed. Return a concise status. Parent owns review, close, and deploy.

## Acceptance

A fake-runtime integration test proves independent same-project sessions and safe targeting. Flush failures preserve work. Old records load without token leakage or unsafe cleanup. All relevant tests pass. This is foundation task A1, not completion of the whole approved OS blueprint.
