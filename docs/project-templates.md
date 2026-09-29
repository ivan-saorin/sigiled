# Reusable project templates

SIGILED uses GitHub's native **Template repository** flag as eligibility, within
the configured `GITHUB_OWNER`. Ordinary repositories are never eligible merely
because SIGILED manages them. No additional catalog or marketplace is required.

## Browser

Open **New project → Manage reusable templates**, enter the repository name and
choose **Designate as template**. The source's default revision is validated
before the flag is enabled. **Remove template designation** disables future
creation; it never changes existing generated projects.

In **New project**, enter a new name, select **Template**, optionally enter a
branch, tag or full commit SHA in **Template revision**, then **Create project**.
The default option uses `VM_TMPL_REPO` (normally `vm-tmpl`). The success response
and project overview show the immutable source commit. An uncertain response
retains the exact submitted selection for retry; a definitive conflict allows
choosing a different name.

## Driver and API

The SIGIL driver is an HTTPS-capable LLM following the served contract, not a
separate executable command parser. These phrases are documented HTTP recipes:

| Driver phrase | Authenticated request under `/sigiled` |
|---|---|
| `templates` | `GET /templates` |
| `template film-engine enable` | `PUT /templates/film-engine {"enabled":true}` |
| `template film-engine disable` | `PUT /templates/film-engine {"enabled":false}` |
| `new my-film` | `POST /projects {"name":"my-film"}` |
| `new my-film from film-engine` | `POST /projects {"name":"my-film","template":"film-engine"}` |
| `new my-film from film-engine at <ref>` | Same request with `"template_ref":"<ref>"` |

Names obey existing project validation. Template names are repository names,
not arbitrary URLs or cross-owner paths. Unknown JSON fields are rejected.
The equivalent browser endpoints are `/browser/api/templates`,
`/browser/api/templates/{name}` and `/browser/api/projects`. They use the existing
signed-in actor, same authorization, same implementation, and CSRF/origin checks.
Drivers require a current human approval for designation and creation. Source
discovery uses the existing shared project-read policy: all authenticated actors
share the configured owner's inventory; there is no new per-user ACL. GitHub
PAT permissions further constrain availability, and designation requires admin
access to the source repository. User-owned destinations require the PAT user
to match `GITHUB_OWNER`; organizations use GitHub's organization create API.

## Revision and identity

An omitted ref resolves the source's current default branch. Any supplied branch,
tag or SHA is resolved once to a full commit. Git fetches **that SHA**, verifies it,
then copies its tree into an independent private repository with a new root
commit. GitHub's generate API does not offer a revision parameter, so new
creation deliberately uses empty-repository provisioning plus a pinned Git tree.
No source branch is written. No source history, tags or remotes are imported.

The durable creation record contains source repository/name, immutable GitHub
repository ID, requested ref, actual source commit/tree, destination ID,
operation/project UUID, created time and resulting initial tree/commit.
It is returned as `provenance` on creation, in `/projects`, and project overview.
The repository also contains `.sigil/project.json`, including a fresh
`project_id` and `approval_namespace`. To repeat a source revision, create a
**different name** with `template_ref` set to that recorded source SHA. The
source must remain available, eligible and fetchable. The source files match;
destination-specific identity and provenance intentionally differ.

SIGILED changes `sigiled.toml` (or legacy `mgr.toml`) `[project].display_name`.
Other project-specific TOML fields can be declared in `sigiled-template.toml`:

```toml
[[identity]]
file = "film.toml"
path = ["name"]
value = "project_name"

[[identity]]
file = "film.toml"
path = ["project_id"]
value = "project_id"
```

Allowed values are `project_name`, `repository` (`owner/name`) and `project_id`.
Bindings must address existing string fields in regular TOML files. At most 32
bindings and 8 path segments are allowed. Manifest bindings may only target
`project.display_name`. Edited TOML files are reserialized; comments/formatting in
those files are not retained. Other source blobs, modes, assets, skills and
dependencies remain unchanged. There is no blind text replacement or exclusion
engine. Prepare the source tree deliberately, including its examples and assets.

## Compatibility and authority

This version accepts **workspace templates** with a valid SIGIL manifest.
A declared workspace Dockerfile must be a regular file in that tree. Before
success, a runtime-enabled deployment builds/resolves the destination's normal
workspace image and rejects the repair fallback on build failure. A deployment
configured without a session runtime performs manifest/tree validation only.
The usual session open adds the existing IDE layer and validates runtime health.

Templates that declare live `[app]`, `[jobs]`, `[service]`, `[compose]`, `[volumes]`
or `[secrets]` bindings are rejected; prepare a reusable workspace source without
those source-runtime bindings. Git submodules are unsupported: vendor required
contents before designation. Sources declaring Git LFS filters are rejected;
use ordinary Git blobs for required assets. Source code is not executed during
tree preparation; normal Dockerfile build runs only for an authorized creation.
Review reusable code/Dockerfiles before designating them. Credentials must never
be committed to an eligible template; SIGILED does not classify arbitrary file
contents as secrets.

SIGILED deploy keys, sessions, browser credentials, runtime state, registry
records, enrollment bindings, work items and control-plane approvals are stored
outside the source tree and are freshly allocated for the destination. Copied
media approval receipts are **source data only**. SIGILED never uses them as
authorization. An application that approves media must bind its checks to the
new `.sigil/project.json` project ID/approval namespace and the actual content;
copied path-based receipts alone cannot establish new-project approval. Application
receipt migration is deliberately part of template preparation, not a generic
SIGILED grant. Opticon's approval portability must be validated when preparing
its production template.

Application origin is separate from the underlying foundation. The original
`template = "vm-tmpl@x.y.z"` pin is retained and still drives `template_version`,
`template_behind` and the existing explicit sync/drift checks. A generated
application project is shown with both its selected source and its foundation.
Application-template updates never overwrite generated projects automatically.

## Recovery

Provisioning serializes per destination and survives caller disconnects.
Before creating a remote repository, the operation and resolved snapshot must
be durably saved. The repository is recognized by both its immutable provider ID
and random operation marker. A lost provider response can recover that exact
operation; unrelated repositories are never adopted or overwritten. Initial
push requires an empty destination and uses a branch-absence lease. Keys are
generated once and reused. Registration succeeds only after repository, push,
key and configured runtime validation. An incomplete operation cannot open a
session. Exact completed retries return `200` with the same provenance; initial
success is `201`. Different source/ref requests for the same name return `409`.

Pre-upgrade registered projects and partial registrations retain their existing
recovery path. A previously unregistered incumbent repository is now a conflict,
including for default creation. No repository deletion/rollback is attempted.

| Error | Recovery |
|---|---|
| `template_unavailable` / `repository_not_an_eligible_template` | Check source access, designation and ref; ordinary repositories are rejected. |
| `template_revision_unavailable` (422) | Choose an existing branch, tag or full commit SHA. GitHub 404/409/422 during revision lookup is rejected before provisioning; no destination needs recovery. |
| `template_manifest_*` / `template_workspace_missing` / `template_runtime_binding` | Prepare a compatible source before a new request. |
| `destination_exists_or_changed` / `destination_changed` | Preserve incumbent; use a new name. Never force push. |
| `project_creation_selection_conflict` | Retry the original selection or choose a new name. |
| Provider/key/persistence interruption | Inspect and retry the exact same name, template and ref. |
| `template_workspace_build_failed` | Fix runtime connectivity and retry; source-code repairs require a new creation name/revision. |

Each Git attempt uses private staging under the state volume's `template-staging`
directory. Failed attempts are retained; retries never remove another process's
lock. Operators may reclaim abandoned staging only with no provisioning workers
running. Durable state, keys and repository mirrors remain part of normal backups.
