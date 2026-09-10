# Browser dashboard (B2)

The embedded dashboard is available at `/` and `/ui/*`. It uses only root `/browser/*` adapters, separate from bearer-only machine routes. `/ui/overview`, `/ui/projects`, `/ui/projects/new`, and `/ui/projects/{project}/{tab}` are stable navigable URLs. Shared Research, Memory and Models destinations currently explain their pending integrations. No live research, model, IDE, memory-curation or deployment operations are claimed.

## Origin and data boundaries

`SIGILED_BROWSER_DASHBOARD_ORIGIN` optionally selects exactly one member of `SIGILED_BROWSER_ORIGINS`; absent, the first explicitly configured origin owns the dashboard. The root shell and `/browser/assets/{dashboard.js,dashboard.css}` reject every other Host/Origin, including other configured browser sites. This leaves a separate memory origin selectable later. Existing machine bare and `/sigiled` paths are unchanged. Do not route `/ui/*` to machine `/projects/*`.

Assets are embedded in the Rust binary, self-hosted, no-store, nosniff, and protected by a restrictive CSP without inline script/style. No OAuth/workspace credentials occur in HTML or boot data. All dynamic text uses DOM text nodes. B1's BrowserContext verifies the actual actor, exact Origin and CSRF for mutations. The existing 15-second boundary and URI/header limits remain. Only POST `/browser/api/projects` admits up to 1 KiB JSON; POST/PATCH work-item paths admit up to 16 KiB. Login, callback, logout and reads remain bodyless. There is no generic proxy.

## Browser adapters

| Method and root path | Contract |
| --- | --- |
| GET `/browser/api/overview` | Existing A2 safe dynamic overview, offset/limit. |
| GET `/browser/api/projects/{project}` | Existing A2 safe project detail and paginated activity. |
| POST `/browser/api/projects` | `{name}` through existing ProjectsNew authorization, core creation/adoption, deploy-key and registration policy. 201 newly registered; 200 existing registration recovered after the same actor passes policy; 403 approval required; 422 invalid name; failed/uncertain provisioning returns a static safe error with `state: partial`, `retry_same_name: true`. |
| GET `/browser/api/projects/{project}/jobs/{job}/runs` | Paginated safe job history: job, branch, state, start/finish epochs, exit. Never commands, secrets, detail/log text or workspace credentials. |
| GET/POST `/browser/api/projects/{project}/work-items` | List page or create explicit work item. |
| GET/PATCH `/browser/api/projects/{project}/work-items/{id}` | Item with audit history, or compare-and-swap update. |

List queries accept only `offset` and `limit` (default 25, 1–100; offset at most 1,000,000). UI pages use 20. Work-item mutations require a registered project and a valid B1 human context; both Admin and Driver roles can track work. These records cannot modify derived system attention.

Creation body: `{ "id": "client-generated-UUID", "fields": { "title": "Next step", "description": "", "state": "open", "owner": "", "source_link": "/ui/projects/example/pending" } }`. UUIDs make an uncertain create retry idempotent for the same project, creator and original fields; a reused ID with different input is a 409 conflict. Update body: `{ "expected_revision": 1, "fields": { ...all fields... } }`. Revision mismatch is 409 `work_item_revision_conflict`. No last-write-wins override exists. The UI retains the draft, displays the latest saved fields, and requires an explicit choice to use that revision before the next Save.

Fields are title (required, at most 200 UTF-8 bytes), description (8,000), state `open|blocked|done`, owner (200), source link (2,048). Links allow credential-free HTTPS or `/ui/` app paths; reject executable/protocol-relative URLs, controls, backslashes and ambiguous relative escapes. IDs are opaque UUID-shaped strings. Records carry project, created/updated epoch, revision, creator/editor stable human driver, and append-only field snapshots with actor/revision/time. Lists omit audit; detail includes it. Capacity is 10,000 items/project, 1,000 audit revisions/item, 64 MiB file. Inputs reject unknown fields.

## Durable store and retry safety

Work items live in `SIGILED_STATE_DIR/work-items.json`, separate from the older general state snapshot. Without a durable directory they return 503, never an ephemeral saved acknowledgement. One process owns the file; cloned AppState instances share its transaction mutex. Each transaction reads the durable file, validates CAS, writes the item and audit together to a mode-0600 temporary file, fsyncs, atomically renames and fsyncs the directory before acknowledging. No cache is published ahead of disk. Serialization/write/rename/sync failures return 503 `work_item_save_uncertain`; the UI keeps the draft and does not auto-replay. As with any uncertain storage response, a failure after rename may require reading the saved revision before retry. Corrupt/inaccessible storage is an error, never an empty list or silent reset. Restart reads the same file.

Core project creation shares the existing per-project merge lock across browser and machine callers, serializing registration/key operations. Existing private deploy keys are preserved; a missing public half is derived from the private key. A public-only/corrupt key requires operator recovery and is never silently replaced. If GitHub says an install conflicts, success requires a listed exact matching, write-capable public key. An unknown or different key stays partial. Repeated generation adopts the existing repository through the existing policy. No rollback of an already-created remote repository is claimed. Registered browser retries recheck authorization and persistence before returning success. This is a single-control-process design; multiple replicas sharing the same state/key directory need external serialization before rollout.

## UI behavior and limits

Needs attention, active work and project rows come from A2 observations. Search/filter/sort fetch the entire registry in pages of 100 (bounded to 2,000 projects); a larger inventory produces an explicit limit error and retains the prior list, never silently substitutes a first-page search. Each overview page includes `inventory_revision`, a SHA-256 of sorted registered project names. The UI requires one revision/total across all pages and a complete unique row count before publishing a refreshed list. Concurrent membership changes produce an explicit error and keep the previous list; Refresh retries from page zero. A later membership change is seen on the next poll. Polling every 30 seconds pauses while hidden. Navigation aborts/ignores stale reads. Project activity, work items and job histories paginate. A2 session/job definition projections are bounded; they are observations, not runtime probes. Source and deployed revisions remain distinct. App deployment, latest build result and runtime observation remain distinct.

Drafts stay in current-tab memory across app navigation, polling and 401s. Expiry shows a separate-tab B1 login link and explicit Check sign-in in the original page. Checking sign-in refreshes identity/CSRF and data only. Every deliberate Save also rechecks the session; successful login never replays a mutation. A browser disconnect never closes or merges a workspace. A full page reload/closed tab discards unsaved drafts; no sensitive drafts are written to browser storage.

## Integration points for C2/D2/B3

- Reuse `browser::BrowserContext`; credentials remain crate-private server state.
- Extend `browser/dashboard.rs`'s exact route/method body allowlist for each reviewed adapter; do not weaken the B1 bodyless boundary.
- Browser session `features` now adds `project_creation: true` and `work_items: <durable-store-configured>` while workspace/memory feature flags remain false.
- `/ui/projects/{project}/{tab}` tabs: overview, workspace, research, jobs, app, memory, activity, pending. Navigation is shared; `/ui/research`, `/ui/memory`, `/ui/models` are pending integration surfaces.
- `window.Sigil` exposes small same-origin helpers: `el`, `request`, `navigate`, `showAuth`, `checkSession`. `request` never accepts/returns OAuth tokens and has no automatic mutation replay. Adapters should keep their forms outside polling replacement surfaces.
- Keep root asset/shell host ownership separate from the memory site; this change does not deploy a memory app on that origin.

## Verification and release

Rust fixtures use the actual B1 login, browser handlers and loopback GitHub doubles. They cover authorization, CSRF, body limits, durable CAS/audit/restart, failed saves, safe links, and partial project retry/two tabs without any live mutation. Controlled Edge fixtures exercise the actual committed assets at desktop/mobile widths with synthetic identities/data. They are not proof of live IdP, project provisioning or downstream compatibility.

Release still requires B1 origin/IdP/edge configuration, a persistent writable state directory with backups and single-process ownership, real origin/cookie/CSRF checks, and separately approved live provisioning checks. IDE/workspace actions, research/model adapters and memory curation remain incomplete until their reviewed APIs are integrated. No deployment was performed for B2.
