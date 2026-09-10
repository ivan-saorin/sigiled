# Registry/overview rollout — A2/A3

Status: source implementation, not deployed. The existing machine auth and
legacy `/projects` array remain unchanged. The new endpoints are authenticated
`/sigiled/overview` and `/sigiled/projects/{project}`; bare aliases also exist.

## Complete additive manifest example

```toml
# Existing workspace/app/jobs/compose tables remain independent.
[project]
display_name = "Research notes"
description = "Private project context, never copied into /services"

[workspace]
dockerfile = "Dockerfile.session"

[ide]
enabled = true
provider = "code-server"

[memory]
enabled = true
sources = ["README.md", "docs"]
sharing = "private"
source_code = false

[app]
name = "research-notes"

[jobs.digest]
cron = "0 7 * * *"
command = "./jobs/digest.sh"
timeout_minutes = 10

# Explicit PUBLIC discovery declaration; this does not configure the edge.
[service]
name = "research-notes"
purpose = "Search published research notes"
origin = "https://research-notes.example.com"
gate = "stack-bearer"
status = "planned"

[service.capabilities]
version = 1
operations = "/openapi.json"
health = "/healthz"
ui = "/home"
```

For this example, the control plane must configure `DOMAIN=example.com` and
an operator must separately configure the actual edge route/auth policy.
No arbitrary gateways, alternate ports or credentials are accepted. No
credentials are sent to declared origins. Gate/status are declarations,
never proof of installed routing or runtime health. Name conflicts hide all
conflicting dynamic entries; the seed catalog cannot be overridden.

## Integration surfaces

- `declaration::{Declaration,ProjectMetadata,Ide,Memory,Service,ServiceCapabilities}`
  define validated desired intent. `Manifest.declaration` is additive.
- `ecosystem::Descriptor` persists redacted manifest metadata, app name,
  `JobDefinition { name, cron, timeout_minutes }`, desired/observed revisions,
  observed/attempt/retry times, typed `RefreshError`, and consecutive failures.
- `Registry::{descriptor,descriptors,hydrate_descriptors}` expose that map.
  `insert` is replay-safe; `replace_all` deduplicates. These methods do not
  override pre-existing records or default over an explicit sharing policy.
- `ecosystem::refresh(&AppState, project)` is the shared async mirror/manifest
  observation path. It acquires the A1 owned project mirror guard, limits
  blocking workers to two, shares a 2s mirror/capacity wait budget, bounds native
  work at 30s with termination/reaping, updates safe descriptors and fallibly persists.
  `repair_loop`/`repair_batch` provide bounded background repair only.
- `catalog::dynamic(&Registry)` returns `(public_entries, per_project_errors)`.
  Errors are fixed safe codes. `Registry.domain` is configured once from
  `DOMAIN` in main; tests inject it directly without environment mutation.
- `SessionRecord::view` is now crate-visible; callers reuse the A1 projection,
  never serialize the custodied record. No mutation authorization changed.
- `overview::{root,detail}` are Actor-authenticated handlers, `Page` validates
  limit/offset, and `revision_drift` handles deployed SHA prefixes.
- `StateSnapshot.ecosystem` has a serde default. Existing project/snapshot
  fixtures load as before. No enrollment token, service credential or secret
  mapping enters descriptors. Later enrollment must obtain its initiating
  authorization independently; startup has no credential to forward.

## Rollout and rollback

Before deploying, back up the current snapshot, inspect the diff and run the
workspace tests/format checks. Deploy through the established operator flow.
Verify machine auth, legacy `/projects`, public `/services`, the new read
routes, and pending reasons for absent browser/IDE/memory adapters. Observe
repair timestamps/errors; never infer resource readiness from enabled intent.
Pre-2.5 readers ignore the ecosystem map, but subsequently persisting with
those binaries drops the descriptor cache. Earlier 2.5 binaries do not know the
new deadline/busy/lock error enum values and may reject a newer snapshot; restore
the matching backup when rolling back to one of them. Reconciliation can rebuild it; explicit memory-sharing intent must
remain in manifests or a backup when rolling between versions.

## Known boundaries

No browser identity/UI, IDE runtime, mem0 mutation, SDE scheduling, inferred
TODOs or explicit work-item API. No live runtime health probes: responses use
unknown/unavailable with null observation time. Existing session/job APIs
serve complete inventories; detail caps those sections/debt/attention at 100,
while detail limit/offset paginates activity and root limit/offset paginates
projects. Aggregation is in-memory and recomputed from stores per request.

Repair batches consider 16 projects and skip busy mirrors. Shared refresh (also
used by the job scheduler) has one 2-second combined mirror/capacity wait budget,
then one 30-second absolute deadline across clone/fetch/reset/manifest Git reads.
On deadline/error, Linux process-group cleanup kills descendants, closes pipes
and reaps the leader before owned mirror guards/worker permits are released.
The leader remains unreaped while polling (`waitid(WNOWAIT)`), reserving its
process-group identity through cleanup. Each stdout/stderr stream is capped at
4 MiB; overflow fails safely with the same cleanup. Git automatic maintenance,
auto-gc/detach and hooks are disabled only for these supervised commands.

`Runtime::ensure_mirror_until` is the refresh-only boundary; existing
`Runtime::ensure_mirror` session-lifecycle callers remain unchanged. Initial
clones use a fresh owned temporary directory and atomic no-replace publish only
after validation; timeout/failure removes only the owned temporary target.
Retries cannot mistake that partial clone for an incumbent mirror. Existing
mirrors reject pre-existing known Git lock files without removing them. With the
project guard held, locks created by this supervised refresh are removed only
after its processes stop; cleanup failure/pre-existing lock is a visible
`repository_locked` failure with stale last-valid metadata. Do not run manual
Git writers against managed mirrors outside the project guard.

Busy mirror/capacity waits return `refresh_busy` (warning/pending), allowing the
serial scheduler to consider later projects; native deadline failures return
`deadline_exceeded`. The refresh helper fails closed outside Linux, where these
termination/publish guarantees have not been implemented. Invalid manifests
retain last-valid published metadata with stale state; valid removal withdraws
it. Disk failures remain visible in memory but cannot be made durable until the
store recovers. Pre-existing Git locks require explicit operator investigation;
they are never guessed to be disposable.

Overview snapshots/indexes sessions once per request and projects only the
selected project page. Global attention is derived from that same session
snapshot. Registry/overview test fixtures explicitly construct no-runtime,
no-auth-environment, ephemeral state and controlled temporary repositories; they
never call the environment-loading AppState default.
