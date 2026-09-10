# Session lifecycle and recovery

## Rollout status

A1 implementation is prepared and tested on the isolated session branch. It is
**not deployed**. No production session collision was exercised. The parent
operator owns review, close/merge and deployment.

Before deploying, back up the 0600 state snapshot and inventory session and
container ownership. The existing edge `/s/<slug>` -> `vm-<slug>:8000` rule
accepts the compact `vm-s-{full session ID}-g{generation}` names without a reload.
The name is at most 59 bytes for the full 128-bit hex ID and any u64 generation,
independent of the permitted 39-character project name. Project remains in
record metadata and labels. Generation exhaustion fails before destructive work. Returned-endpoint clients continue to
work; clients that reconstruct project-only URLs must use the response value.
Recycle now changes endpoint as well as token. No machine authentication or
browser authentication system changes are included.

## Durable model

New records persist `binding` (container, endpoint, generation), lifecycle,
generation, runtime ownership, image name and a fixed error code. Tokens remain
private snapshot custody. IDs use 128 OS-random bits and tokens 192 OS-random
bits. A creating record is durably written before mirror/runtime allocation.
Snapshots are serialized and atomically replaced with mode 0600 and fsync.
Persistence failure prevents new allocation and destructive lifecycle steps.

Legacy fields have defaults. One legacy record targets only `vm-{project}`.
Ambiguous legacy claims are all failed and denied lifecycle access. Interrupted
creating/recycling/closing records become failed/interrupted at hydration.
Closed and reaped records retain the existing event-only historical model.

## Inspection and actions

Authenticated `GET /sigiled/sessions?project=...` and `GET
/sigiled/sessions/{id}` return explicit safe projections. The list envelope is
`{sessions:[...]}`. They do not query containers, fetch Git, expose logs/tokens,
or refresh idle activity. Unknown detail returns 404. Operational visibility
matches existing authenticated project visibility. Driver close/recycle also
checks the actual creating actor; admin access remains global.

On 409 `recoverable:true`, retain the returned session ID, inspect the state,
and repair the cause. `flush_failed` includes unreachable agents, local commit
failure and checkpoint push rejection: keep the runtime and original token.
`fetch_failed`, `merge_failed`, and `push_failed` preserve the branch/runtime;
retry the intended verb after recovery. Do not interpret a failed recycle as
success or replace a client token without a successful response.

`legacy_ownership_ambiguous` needs operator reconciliation against the backed-up
snapshot, container workload labels and branch history. Do not select a winner
by project name, automatically delete records, or destroy a disputed container.
Retain every branch while resolving ownership. No automatic administrative
reassignment or force-discard endpoint is introduced in A1.

`create_failed`, `boot_failed`, `runtime_not_owned`, `interrupted`,
`cleanup_failed`, `runtime_unavailable`, and persistent disk failure may require operator recovery.
Create collisions never pre-delete their incumbent. Post-create key/start
failure cleans only the container acquired by that create. Boot failures keep
the acquired runtime for diagnosis/recovery. When recycle has already verified
its checkpoint and fetched the branch, a replacement failure still leaves the
pushed branch safe, even if no usable runtime remains. No force-close fallback
silently discards unpushed work. Reaper skips failed sessions and stale idle
observations from earlier generations.

## Concurrency and implementation interfaces

Lock order: `SessionState::session_lock(id)` then
`SessionState::merge_lock(project)`. Open holds the project lock while allocating
an identity/branch and bootstrapping; it first locks the new identity before
publishing the creating record, so inspection cannot race a close into boot.
Mirror consumers in sessions, project branches, app manifest/build and job
manifest/image resolution use the same project lock. Background app builds
retain an owned guard until they finish reading the mirror. Session/job image
builds use `Runtime::session_image_locked`, which moves an owned guard into
the blocking worker and returns it with the image for post-build mirror use.
Cancellation drops neither the worker's guard nor its protection prematurely. Job/workspace
execution releases that lock when mirror use ends.

`SessionRecord::container()` resolves a stored binding or the legacy alias.
`SessionState::binding_safe()` denies duplicate ownership. Public handlers
`sessions::list` and `sessions::detail` use private `SessionRecord::view()`;
never serialize `dump_records()` into a public API because it contains tokens.
`AppState::try_persist()` returns a recoverable result; `persist()` remains the
logging compatibility wrapper for existing consumers.

## Verification and remaining scope

Hermetic handler tests exercise real local Git repos plus a per-instance fake
Docker/workspace boundary: same-project isolation, lifecycle targeting,
checkpoint/fetch/master-push failures, collision/start/boot behavior, concurrent
orphan allocation, mirror locking, duplicate close, actor authority, safe HTTP
inspection, legacy decoding/quarantine, persistence and interrupted restart.
No Docker or external lifecycle operations are used by these tests.

Final verification: `CARGO_BUILD_JOBS=2 cargo test --workspace` passed 130
control-plane tests (vm-base: 0); `cargo fmt --all --check` passed;
`CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets` exited 0. Clippy
reports only the existing catalog `unnecessary_map_or` and runtime
`too_many_arguments` warnings.

This foundation does not implement registry/overview UI, IDEs, scheduler redesign,
browser SSO, memory, forced recovery APIs, distributed control-plane locks, or
automatic orphan-container garbage collection. Run one control-plane process
per state/mirror store. Lifecycle locks and file serialization are process-local.
