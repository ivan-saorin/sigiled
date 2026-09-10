# SIGILED — the driving contract for the stack

*Session Execution And Lifecycle — the session is sigiled, with a sigil.*

## The driving contract for the automa stack — v2

**Contract version:** 2.5.0 · **Source:** `docs/sigiled-contract.md` in `ivan-saorin/sigiled`, served by `GET /sigiled/contract` at the deployed sha. 2.5.0 adds live project descriptors, declared service discovery and read-only overview projections (§12). 2.4.0 adds independent session runtime bindings, recoverable lifecycle failures and redacted session inspection. 2.1.0 adds per-project session images (DEC-25): the `image` field in open/recycle responses, `[workspace] dockerfile` in the manifest (§8). 2.2.0 adds the stack service catalog (DEC-27): `GET /services` and the `services` command, public and embedded like this contract. 2.3.0 adds the change-notification step to `open`: the `changed` service (catalog) leaves one memory chunk per detected change in the project's index — surface them before writing.
**Status:** ratified — DEC-01…10 ratified by the operator on 2026-08-03 (see `docs/sigiled-v2.md` §8); the established v2 verbs were live-verified; the 2.4.0 session foundation and 2.5.0 registry/overview are implemented and hermetically tested on their isolated branch, not deployed. SIGILED is the only orchestrator of the stack.

This is the complete operating contract for SIGILED (v2 of SIGILED). It is
vendor-neutral: any LLM that can issue HTTPS requests can drive the system
with only this document. If you are reading this, you are the driver.

## 0. Mental model

SIGILED rents you a disposable Linux container (a "workspace") wired to exactly
one GitHub repo. You edit files, run commands and commit through a narrow
HTTP API. The container is cattle: it can be destroyed at any moment and
nothing is lost, because every commit is pushed to its branch immediately
and anything not in git (or a declared volume) does not exist. The repo is
the only memory — yours across sessions, and the memory you share with any
other model that drives the same project.

New in v2: **independent workspaces.** Each session gets its own branch
and container. Lifecycle transitions serialize per session; mirror reads,
refreshes, branch allocation and merges serialize per project. Work inside
separate workspaces proceeds independently. Master is the arbiter at close,
with merge debt (§5) preserving conflicts. Authentication is per-driver
(OAuth2 against the stack IdP), and human approval is a first-class,
auditable object.

Workload classes: **session** (interactive, yours), **job** (cron-run
batch, append-only history), **app** (resident service). You mostly drive
sessions.

## 1. Bases and credentials — two-legged auth

| Surface | Base | Auth |
|---|---|---|
| SIGILED verbs | `https://api.016180.xyz/sigiled` | `Authorization: Bearer <access-token>` |
| Workspace | returned `endpoint` (`https://api.016180.xyz/s/s-{session_id}-g{generation}/` for new sessions) | `X-Session-Token: {token}` (send your Bearer too — the edge does not inspect it here, the token is the auth) |
| Web search | `https://search.016180.xyz` | stack search credential (operator-provided; a stack service, not part of SIGILED auth) |

**Machine leg (yours).** Each driver is an OAuth2 client of the stack IdP
(`auth.016180.xyz`, Authentik): your skill carries a per-driver
`client_id` + `client_secret` (providers `sigiled-claude`, `sigiled-kimi`, …).
Mint your own short-lived access token when needed:

```
POST https://auth.016180.xyz/application/o/token/
  grant_type=client_credentials&client_id=…&client_secret=…
```

Token expired → mint another. 401 on a fresh token → the operator rotated
your credentials: ask for the new pair. Never echo credentials or tokens
into chat, logs, or committed files. The dual-auth window is **closed**
(2026-08-03): per-driver tokens are the only machine credential — there is
no shared bearer.

**Human leg (the operator's).** Operations that require "the operator
approved this" (see capability map, §4) go through the device flow:

```
POST /sigiled/auth/elevate        → { verification_uri, user_code, expires }
```

Relay the URL + code to the operator in chat; they approve in the browser
once. SIGILED polls, then keeps the approval tokens in its own DB (never in
skills, never in transcripts) and auto-refreshes them. Inspect with
`GET /sigiled/auth/approvals`. An approval names a human and an expiry; it
rides alongside your driver identity as `actor: {driver, approval}` on
every session and job record.

The workspace contract is two-header by design: the edge swaps
`X-Session-Token` into `Authorization` before the request reaches the
container, and the container validates it — the per-session, 192-bit,
per-life token IS the workspace authentication (whatever `Authorization`
you sent on `/s/` is not inspected at the edge). The token comes from
`POST .../sessions`; a token minted for one session is rejected by another session's
container, including another session of the same project — never reuse tokens after
`recycle`/`close`.

Request and response bodies are JSON unless noted (`git/diff` and
`git/show` return plain text; `GET /sigiled/contract` returns markdown).

## 2. Commands — the normal operations

Invoked as `/sigiled <command>`, as skill args, or as bare words in an automa
context ("status" alone means `/sigiled status`). Session commands keep
`session_id`, `token` and `endpoint` in conversation memory. Anything not
covered here falls through to the full API (§4, §6).

| Command | Procedure |
|---|---|
| `status` | `GET /sigiled/healthz` + `GET /sigiled/projects`. Report version and, per project, `merge_debt` queue, `template_behind`, `needs_merge`; **any merge debt is shouted first**. |
| `projects` | `GET /sigiled/projects` — full records (incl. `template_version`, `template_behind`). |
| `new <name>` | Requires approval for `stack:drivers`. `POST /sigiled/projects` `{name}` (lowercase alnum+dash, 2–39 chars, letter first). Warn first: there is no delete verb — projects are permanent. |
| `open <project>` | `POST /sigiled/projects/{p}/sessions`. Store `session_id`, `token`, `endpoint`. **If the response carries `merge_debt`, resolving it is your first and only job (§5).** Then rule 1: `GET /git/log?limit=15` and summarize the handoff before any write. Then what changed upstream since the last close: `GET https://memory.016180.xyz/search?q=changed&idx={p},mem0&tags=changed&since=<last close>` (the `changed` service, catalog entry). `404 index {p} not found` = nothing is watched for this project yet — retry with `idx=mem0` alone: the ownerless `chg0` watches (Anthropic release notes, this contract, Arctic Shift) apply to everyone. Surface the hits; each carries a `report: <branch>:<path>` pointer readable with `git show` in a session on `changed`. |
| `close` | Commit pending work, then `POST /sigiled/sessions/{id}/close`. Report the merge outcome (`ff` / `merged` / `debt`) and `log_operativo_touched`. |
| `recycle` | `POST /sigiled/sessions/{id}/recycle`. Replace the stored token **and endpoint** after success (the old token is dead), confirm with `GET {endpoint}/health`. |
| `elevate` | `POST /sigiled/auth/elevate` → relay URL + code to the operator; poll status via `GET /sigiled/auth/approvals`. |
| `log <project>` | `GET /sigiled/projects/{p}/log` — the machine layer of history (sessions, merges, job runs). The narrative layer is `docs/log-operativo.md` in the repo. |
| `jobs <project>` | `GET /sigiled/projects/{p}/branches` filtered to `job-*`, plus `GET /sigiled/projects/{p}/jobs/{j}/runs` per job of interest; summarize outcomes newest-first. |
| `run <project> <job>` | `POST /sigiled/projects/{p}/jobs/{j}/run`. 202 → run started; 409 → a run of the same job is in flight; 422 → broken `[jobs]` table on master. |
| `recap <project> [job]` | Recap flow (§7): runs → branches → in a session, `GET /git/log?ref=origin/job-…` and `GET /git/show?ref=…&path=…`. Never merge or delete job branches. |
| `apps <name> [action]` | `GET /apps/{a}` for status. `start`/`stop`/`restart`/`upgrade` require approval for `stack:drivers`. 202 `action=building` = background build: poll status, never re-fire the verb. |
| `sync <project>` | Template recepimento (§8): run `tools/sync-template.sh` in a session; it stops on drift. Never auto-update. |
| `search <query>` | `GET https://search.016180.xyz/search?q=<urlencoded>&format=json`; summarize `results[]`. |
| `services [status]` | `GET /sigiled/services` (optional `?status=` filter: `live`, `building`, `planned`) — the stack service catalog: purpose, machine/human legs with gates, `spec`, per-service `skill`. When a service names a `skill`, prefer it over raw calls. |

Coding-task ritual: `open` → merge debt? resolve first → git log → work/commit
loop → **log operativo entry** → `recycle` or `close`. Never leave a session
dangling (rule 6).

## 3. The rules

1. **Read before you write.** First workspace action in every session:
   `GET /git/log?limit=15`. The log is the handoff from previous drivers,
   you included. If the session started `stale: true`, also check
   `GET /git/status` and the newest commit — a `wip: … autosave` message
   means the last session ended unattended.
2. **Write intent-carrying commit messages.** They are the only channel to
   future drivers — and they are the context package when your branch ends
   up in merge debt. "fix" is vandalism; "fix: reaper race on close — claim
   state before flush" is memory. Commit at every coherent step.
3. **Every commit pushes automatically.** You never push, you never lose
   committed work, and container destruction is always safe.
4. **One branch per workload; master is the arbiter at close.** Sessions
   execute independently inside their workspaces. Lifecycle operations serialize
   per session; mirror preparation, image builds, branch listing and merges
   serialize per project. An open or recycle can wait for an image build, and
   cancelling its request does not release the mirror while that build runs.
   Never retry-hammer a pending operation.
5. **The sigil is the container.** `exec` is full bash, but only the
   container filesystem + declared mounts exist. Host paths, other
   projects, docker, ssh: structurally out of reach. Do not try.
6. **Do not idle.** ~1 h without API calls and the reaper auto-closes the
   session: uncommitted work is autosave-committed and pushed, nothing is
   merged, the container is destroyed. A failed checkpoint leaves the container and record intact with a recoverable error; after successful reap your token is
   dead and the next start is stale. Done? `close`. Pausing? Say so in a
   commit message first.
7. **master only moves through `close`** — fast-forward when possible,
   merge commit otherwise (never rebase: the merge boundary is memory).
   Job branches NEVER merge — they are append-only history.
8. **Secrets never touch git.** They arrive as container env, injected at
   creation. Never echo env values into committed files or command output.
9. **Merge debt outranks everything.** If `open` hands you a `merge_debt`
   package, resolving it is your first act — whatever model you are,
   whatever you came to do. Read both sides' commit messages, resolve,
   commit explaining *what you kept and why*, close. If you cannot decide:
   ask the operator, do not guess.
10. **Close coherent work → log operativo entry.** Add a dated entry on top
    of `docs/log-operativo.md` (where were we / where were we going / what
    was done + deviations, state, next step). `close` reports
    `log_operativo_touched` — an honest mirror, not enforcement.
11. **Scruple build.** If recent history shows multiple merges, run the
    build/tests (or a coherence pass on doc-only repos) even if your own
    change is trivial. Git can merge cleanly and still be wrong. If it is
    broken: fix it before proceeding.

## 4. SIGILED verbs

Base `https://api.016180.xyz/sigiled` — bearer only. Nothing exists beyond
this table. Capability map: `stack:admins` do everything; `stack:drivers`
need a live approval for `projects new`, app verbs, and any session on
`sigiled` / `sigiled-supervisor` (the control plane demands a present operator).

| Verb | Returns |
|---|---|
| `GET /healthz` | `{status, version}` |
| `GET /contract` | this document (markdown), at the deployed sha |
| `GET /services` | the stack service catalog — `catalog.json` at the repo root, embedded at build, boot-validated; `{catalog_version, services[]}`, optional `?status=` filter (`live`, `building`, `planned`); public like the contract |
| `POST /auth/elevate` | `{verification_uri, user_code, expires}` — device flow via the stack IdP |
| `GET /auth/approvals` | live approvals `{human, driver, expires}` |
| `POST /projects` `{name}` | 201 project record · 409 already registered · 422 bad name |
| `GET /projects` | all project records: `template_version`, `template_behind`, `merge_debt` queue, `needs_merge` |
| `GET /projects/{p}/log` | machine history: sessions (with `actor`), merge outcomes, job runs — JSON; `?format=md` renders markdown |
| `GET /projects/{p}/branches` | `[{name, sha}]` — job-recap entry point |
| `POST /projects/{p}/sessions` | 201 (§5; `merge_debt` on top when present) · 503 `{retry:true}` repo not ready — wait ~5 s, retry |
| `GET /sessions?project={p}` | authenticated `{sessions:[...]}`; optional project filter; safe operational metadata only |
| `GET /sessions/{id}` | authenticated safe metadata: session ID, project, branch, actor, state, generation, endpoint, runtime name, image name, error code; 404 if absent |
| `POST /sessions/{id}/close` | `{closed, merge: "ff"\|"merged"\|"debt", sha, flushed, log_operativo_touched}` |
| `POST /sessions/{id}/recycle` | fresh `{token, endpoint, sha_at_recycle, image}` — old token dead |
| `POST /projects/{p}/jobs/{j}/run` | 202 run record · 404 unknown job · 409 same job in flight · 422 broken `[jobs]` |
| `GET /projects/{p}/jobs/{j}/runs` | last 20 run records, newest first |
| `GET /apps/{a}` · `POST /apps/{a}/start\|stop\|restart\|upgrade` | app status / action — approval territory for drivers |

## 5. Session lifecycle — the main loop

**Start**: `POST /sigiled/projects/{p}/sessions` → 201:

```json
{"session_id": "…", "project": "…", "branch": "session/…", "token": "…",
 "endpoint": "https://api.016180.xyz/s/s-{session_id}-g1/", "head": "<sha>",
 "generation": 1, "state": "active",
 "stale": false, "last_commit": null,
 "merge_debt": null,
 "image": {"used": "vm-{p}:df-…"},
 "actor": {"driver": "sigiled-claude", "approval": null}}
```

- `stale: true` — you are resuming an existing branch after an auto-close
  or a lost container; `last_commit` is its pushed head. Rule 1 applies
  double.
- `image` — what the container runs on (DEC-25). A project that declares
  `[workspace] dockerfile = "…"` in its sigiled.toml gets a per-project
  image built from that file at master, content-addressed (only editing the
  dockerfile rebuilds — the first open after an edit pays the build). A
  failed build does NOT block the open: the session falls back to the
  global base image and the field shouts —
  `{"used": "<base>", "requested": "vm-{p}:df-…", "build_error": "<tail>"}`
  — fixing the dockerfile is then that session's first job, and a
  `recycle` picks the repaired image up. No `[workspace]` = the base
  image, silently. Null on control planes without a container runtime.
- `merge_debt` non-null — rule 9. The container starts from the debtor
  branch with the merge in progress and conflict markers in the files:

```json
"merge_debt": {"branch": "session/…", "conflicted_files": ["…"],
  "ours": {"sha": "…", "commit_messages": ["…"]},
  "theirs": {"sha": "…", "commit_messages": ["…"]}, "since": "…Z"}
```

**Work**: everything through `endpoint` with both headers (§6). The cycle
is read → edit → `exec` to build/test → `POST /git/commit`.

**Recycle** (`POST /sigiled/sessions/{id}/recycle`): flush, destroy the
container, recreate it **from your branch** with a freshly minted token.
Use when handing the session to another provider, when the container is
wedged — or to pick up the project's session image after a dockerfile fix
(the fresh container rides what master declares now; `image` in the
response says what you got). Replace your stored token **and endpoint** with
the returned values. Generation increments; recycle migrates a legacy binding
to a unique session runtime. Flush and branch fetch must succeed before the
old runtime is removed. A failure returns 409 with a typed `error`,
`session_id`, and `recoverable: true`; retain the session and inspect it.

**Close** (`POST /sigiled/sessions/{id}/close`): flush, then under the
project's mirror/merge lock: fast-forward if master has not moved,
three-way merge if it has and the changes are disjoint, **merge debt**
otherwise — master stays put, your branch survives, the debt package is
recorded, and the next session on the project inherits it (rule 9).
Simultaneous closes serialize on the lock: one wins, the other sees a
moved master and takes the merge path.

### Independent runtimes and recovery

Each new open allocates a cryptographically random identity and persists its
binding before runtime creation. Two opens for the same project keep separate
containers, endpoints, tokens and branches. Orphan resume retains the branch
but allocates a fresh session ID. Always use the returned endpoint; never
construct it from the project name. New routing slugs are
`s-{full session ID}-g{generation}`; project identity remains in metadata and
container labels. Names fit one DNS label even for a 39-character project and
the largest u64 generation. Generation exhaustion returns a recoverable
`generation_exhausted` error before recycle flushes or destroys anything.
No edge reload is needed for this routing.

One lifecycle transition runs at a time per session. Mirror refresh, branch
allocation, image builds, merge and push serialize per project across session,
app, job and branch-listing callers; workspace execution remains independent.
Drivers may mutate only sessions they own, subject to existing project
approval policy. Admins may manage all sessions. Inspection shares the existing
project visibility policy, never returns tokens or container logs, and never
contacts a workspace or extends its idle lifetime.

A failed flush/checkpoint, fetch, merge or master push preserves recoverable
work and returns 409; it must not be treated as a closed session. Reaper follows
the same preservation rule. Inspection states are `creating`, `active`,
`recycling`, `closing`, and `failed`. Successful close/reap removes the record
and retains the existing machine event, so subsequent detail returns 404.
Interrupted transitions after a restart surface as `failed` / `interrupted`.
Only fixed error codes enter public inspection; repository/container output
and custodied tokens are never serialized there.

Old snapshot records still load with generation zero and their legacy
`vm-{project}` binding. When multiple legacy records claim one project runtime,
all are quarantined with `legacy_ownership_ambiguous`; no lifecycle verb may
flush or destroy that runtime until an operator reconciles ownership. See
[session recovery runbook](session-lifecycle.md). Implementation rollout must
include a state backup and this ownership check; this contract change alone
does not deploy the new binary.

## 6. Workspace API

Base = `endpoint` from start/recycle. Both headers on every call. Every
authorized call except `GET /health` counts as activity (rule 6). Paths
are absolute, confined to `/workspace` + declared mounts.

| Endpoint | Contract |
|---|---|
| `GET /health` | `{status, version, last_activity, idle_secs, uptime_secs}` |
| `GET /fs/list?path=` | `[{name, kind: "file"\|"dir", size}]` |
| `POST /fs/read` `{path}` | `{encoding: "utf-8"\|"base64", content}` |
| `POST /fs/write` `{path, content, encoding?}` | `{written}` — parents created; `encoding: "base64"` for binary |
| `POST /fs/delete` `{path, recursive?}` | `{deleted}` |
| `GET /git/status` | `{branch, dirty, files[]}` |
| `GET /git/diff?ref=` | plain-text diff (vs HEAD when `ref` omitted) |
| `GET /git/log?ref=&limit=` | `[{sha, author, date, message}]` — any ref; default 20, max 200 |
| `GET /git/branches` | `[{name, sha}]` — refreshes and includes `origin/*` remotes |
| `GET /git/show?ref=&path=` | plain-text file at any ref (commit stat when `path` omitted) — no checkout needed |
| `POST /git/commit` `{message}` | `{committed, pushed, sha}` — add -A → commit → immediate push; clean tree = no-op with current sha |
| `POST /exec` `{cmd, cwd?, timeout_secs?}` | `{exit, stdout, stderr, timed_out, truncated}` — `bash -lc`; default 300 s, max 3600 s; 1 MiB capture per stream |
| `/x/{ext}/…` | project extension routes (`ext-<lang>/`), same auth |

Prefer the first-class `fs`/`git` endpoints over their `exec` equivalents —
they are the tested recovery paths. `exec` is for builds, tests, tooling.

## 7. Jobs, apps and the recap flow

Jobs are declared in the project repo's `sigiled.toml` **on master**:

```toml
[jobs.<name>]
cron = "30 3 * * *"         # container-local time
command = "./jobs/x.sh"     # bash -lc in /workspace
timeout_minutes = 30        # 1..60 — hard wall clock
hc_ping = "MY_HC_URL"       # optional stack-env ref, pinged on finish
[jobs.<name>.secrets]
SOME_KEY = "STACK_ENV_VAR"  # container env <- SIGILED env, resolved at creation
```

Changing job definitions = editing `sigiled.toml` in a session and closing it
(definitions are read from master, refreshed within ~5 min). A broken
`[jobs]` table disables that project's jobs until fixed; a manual trigger
surfaces the parse error as 422.

Each run: fresh container from master → branch `job-<name>-<YYYYMMDD-HHMMSS>`
pushed at creation → command → leftover output committed → destroyed.
Run states: `running · succeeded · failed · timeout · error ·
skipped_locked · aborted`.

Job containers ride the project's session image (DEC-25, §5) — but where a
session falls back with a shout, a job with an unbuildable image **errors
the run** with the build tail as its detail: batch missing its declared
toolchain must fail, not lie.

**Recap flow** ("what did the jobs do last week"):
1. `GET /sigiled/projects/{p}/jobs/{j}/runs` — outcome metadata.
2. `GET /sigiled/projects/{p}/branches` — filter `job-*`, sort by stamp.
3. In a session on that project: `GET /git/log?ref=origin/job-…` then
   `GET /git/show?ref=origin/job-…&path=…` — read the content itself.

Never merge or delete job branches (rule 7).

**Resident apps** live in the same sigiled.toml — at most **one** `[app]` per
project (singular table), read from master on the same refresh as jobs:

```toml
[app]
name = "reddit-mine"          # container = DNS name — the edge routes it
dockerfile = "Dockerfile.api" # default "Dockerfile"; built from the repo at master
[app.volumes]
reddit-mine-data = "/data:rw" # named volume -> "/abs/target:ro|rw"
[app.secrets]
TZ = "TZ"                     # container env <- stack env, resolved at creation
```

`upgrade` is the deploy verb (refresh sha → build if absent → recreate;
same-sha = config refresh); `start` never recreates. A 202
`action=building` means a background build is running — poll
`GET /apps/{a}` for the build record.

## 8. Template versioning — the recepimento

Project repos are born from **vm-tmpl v2** and pinned to it:

- `sigiled.toml` on master carries `template = "vm-tmpl@x.y.z"`; the project
  record exposes `template_version` and `template_behind`.
- The project Dockerfile is thin: `FROM vm-base:x.y.z` + the project's own
  toolchain layers (DEC-17). Adopting a new agent = bumping the tag.
- `[workspace] dockerfile = "Dockerfile"` in sigiled.toml is what makes
  SIGILED **build and use** that file for the project's session and job
  containers (DEC-25, §5): path relative to the repo root, read on master.
  Remove the table to ride the global base image.
- `ext-<lang>/` is the extension point (DEC-18): `ext-rust/` crates are
  compiled into vm-base; `ext-py/`, `ext-go/`, … run as supervised local
  processes proxied at `/x/<name>` — one contract, same token gate.
- **Sync is on-demand, never automatic**: `tools/sync-template.sh` (from
  the template) replaces template-owned paths at the pinned tag, with
  **drift detection** — local modifications to template-owned paths stop
  the sync and get reported. Rollback = `git revert` or re-pin.
- `docs/log-operativo.md` is born from the template and is project-owned
  forever — the template never touches it again.

## 9. Errors

| Code | Meaning | Do |
|---|---|---|
| 401 | expired/invalid access token, or bad session token on `/s/` | mint a fresh token; if a fresh one still 401s, the operator rotated your credentials — ask |
| 403 | capability requires approval | `POST /sigiled/auth/elevate`, relay code to operator, retry after approval |
| 404 | unknown project / session / job / app | typo, or job defs not on master yet |
| 409 | recoverable lifecycle failure, job already in flight, or name taken | inspect the typed error and retained session; repair the cause before retrying; never hammer |
| 422 | invalid name / broken manifest | fix the input; do not retry as-is |
| 503 + `retry` | fresh repo still materializing | wait ~5 s, retry the start |
| 502 | upstream down | report to the operator |

## 10. Provider handoff

Multiple providers drive one project — now concurrently, each on its own
branch, each with its own identity (`actor.driver`). The repo is the only
shared memory — rules 1, 2 and 10 are the entire handoff protocol. To hand
one *session* to another provider, `recycle`: new container, new token,
the previous driver structurally cut off.


## 12. Live registry, service declarations and overview (2.5.0)

These APIs are implemented in source; deployment remains a separate operator action.
Existing `GET /sigiled/projects` remains an authenticated array, and existing
machine authorization and workload verbs retain their behavior. Reads never
open a workspace, touch its activity, fetch Git, or start an app/job/inference.
All authenticated actors currently share the same project-read access as `/projects`.
Browser identity and per-human authorization are not configured by this change.

### Manifest additions

`[project]` accepts optional `display_name` (1–100 bytes) and `description`
(1–1000 bytes), without control characters. `[ide]` has `enabled = true` and
`provider = "code-server"` defaults. `[memory]` defaults enabled and private,
accepts `sources` (up to 64 repository-relative documentation paths),
`sharing = "private" | "project"`, and `source_code = false`. An omitted
sharing field retains the last explicitly observed policy. These fields
record desired intent only: no mem0 sharing/enrollment or source indexing is
performed. IDE/memory remain pending with adapter-not-configured reasons.
Unknown fields in these new tables, unsafe paths, remote sources and
unsupported modes are rejected. Old manifests, `[workspace] dockerfile`,
app/jobs/compose and the `mgr.toml` fallback remain supported; no manifest is
valid and means empty declarations. A malformed preferred file does not
fall through to the legacy file or remove the last valid declaration.

`[service]` explicitly publishes service discovery metadata. Its `purpose`
is public, separate from the private project description. Required fields
are `name`, `purpose`, `origin`, `gate`, `status`. The name must equal the
project's declared `[app] name`, be 2–39 lowercase alphanumeric/dash characters,
and not conflict with the built-in seed, platform/auth/gateway names, or any
other project's declaration. Conflicts exclude every involved dynamic
entry and appear as per-project setup errors. The fixed route policy permits
only `https://<service-name>.<DOMAIN>` (optional trailing slash), with no
userinfo, explicit ports (including 443), query, fragment or path. `DOMAIN`
is required to publish dynamic services. This is metadata publication, not
route provisioning or permission to forward credentials.

`gate` uses the existing machine gate values (`stack-bearer`, `service-token`,
`sso-only`, `edge-open`); `status` uses `live`, `building`, `planned` and is
always a declaration, never a health result. Optional `[service.capabilities]`
has `version = 1` and optional `operations`, `health`, `ui` relative routes.
Routes cannot contain traversal, query, fragment, escaping or absolute URLs.
Public `/services` preserves `catalog_version`, existing fields and `?status=`
filtering. Built-ins stay in seed order; dynamic entries follow sorted by name,
with additive `capabilities`, `status_source = "declaration"`, `observed_at`
and `stale`. No secret/env/command/private-project data is projected.

### Reconciliation and observation

Registration and hydration wake the repair loop. Each sorted round-robin batch
considers at most 16 projects; two global blocking-worker permits limit active
mirror operations. Busy mirrors are skipped by repair. Shared refresh used by
the serial job scheduler bounds mirror plus worker-capacity waiting to 2 seconds,
so an unavailable project cannot indefinitely stop later projects. Native work
has a single 30-second absolute deadline spanning clone/fetch/reset/manifest
reads. Linux commands run in private process groups with nonblocking pipes;
deadline/error/4-MiB-per-stream output overflow initiates group termination.
The leader stays unreaped, reserving its PID/PGID, until `waitid(WNOWAIT)` confirms
its exit and complete bounded `/proc` process/thread scans twice acknowledge that
all group members are dead. Only then are the leader reaped, repository locks or
owned clone directories cleaned, and mirror guards/worker permits released.
Automatic Git maintenance/gc/detach and hooks remain disabled command-locally.

The caller waits at most 2 more seconds after the 30-second work budget for exit
acknowledgement. If it remains uncertain, `termination_unconfirmed` is failed/stale:
the blocking supervisor retains the mirror, permit and leader identity, retries
verification, and prevents repository cleanup. Busy retries cannot conceal this
quarantine. Once exit is verified, cleanup completes and the descriptor records
the underlying result (normally `deadline_exceeded`), permitting later retry.
Cancellation also leaves ownership with the blocking supervisor. Persistent kernel
or `/proc` verification failures quarantine capacity indefinitely; later projects
still have the shared 2-second mirror/capacity wait limit. `refresh_busy` is
warning/pending; `deadline_exceeded` means termination was confirmed.

A new clone occupies only an operation-owned temporary directory until valid,
then publishes atomically without replacing any incumbent path. Failed/timed-out
partial clones are cleaned up and can be retried. Existing mirrors reject
pre-existing Git locks unchanged; locks created by a supervised refresh under
the exclusive project guard are cleaned only after its processes stop. An
incumbent lock or cleanup failure becomes visible `repository_locked`, retaining
last-valid metadata. Managed mirror writers must obey the project guard.
These guarantees are scoped to `Runtime::ensure_mirror_until`; the established
session lifecycle `ensure_mirror` path is unchanged. The helper fails closed on
non-Linux platforms. Successful observations refresh after 300 seconds; failed
attempts retry with bounded backoff (60–960 seconds).

The job scheduler and manual job manifest reads use the same refresh path.
Snapshots add a defaultable `ecosystem` map keyed by registered project name;
legacy snapshots load without migration. Replaying registration preserves the
existing record. Last valid metadata, app/job declarations and services survive
repository/manifest failure and are marked stale. A valid newer manifest can
remove declarations. An invalid newer manifest records desired revision while
observed revision still identifies the last valid descriptor. Attempts include
safe enum errors, timestamps and retry timing; no raw Git/TOML errors escape.
A persistence failure remains visible in memory and is retried; a failed disk
write cannot be claimed durable. The repair loop never creates workloads.

### Read projections

`GET /sigiled/overview?offset=0&limit=25` returns:

```json
{
  "observed_at": 1788998400,
  "counts": {"projects": 1, "sessions": 0, "attention": 3},
  "projects": {"items": [], "total": 1, "offset": 0, "limit": 25, "next_offset": null},
  "attention": {"items": [], "total": 3, "truncated": false}
}
```

The example elides item contents. Project rows contain name/display name,
private description, template flags, desired repository revision, observed
revision, merge flag, per-capability readiness, registry setup observation,
app summary, bounded job summaries, session count and latest activity time.
Projects sort by name; only the requested project page is projected. Sessions
are snapshotted/indexed once per request for counts, attention and safe views. Root attention is capped at 100, ordered failures,
warnings, pending, then stable source reference. Counts cover all records,
not just the page. The envelope observation time is response construction
time; each cached subsystem exposes its own observation time separately.

`GET /sigiled/projects/{project}?offset=0&limit=25` adds memory/IDE intent,
redacted merge debt (no commit messages), safe A1 session views, job definitions
and latest results, attention, and paginated activity. Session/job/debt/attention
sections are bounded at 100 and report total/next offset; use existing session
and job APIs for complete inventories. `offset`/`limit` on detail page activity
only. Limits are 1–100, offset 0–1,000,000; malformed/unknown query fields return
400 or 422. Missing project returns 404. No internal session record, token,
job command, secret mapping, build log or arbitrary failure detail is included.

Every project immediately has desired dashboard/activity/pending/workspace/
IDE/memory capabilities. States are `ready`, `pending`, `disabled`; registry
setup can also be `failed`. `ready` for workspace describes an observed mirror
and configured runtime, not a live session. Registry readiness is scoped
`registry_manifest`, not whole-project provisioning. Pending adapters have no
observed revision/time. Browser readiness remains pending. Capability desired
revision and registry desired/observed revision/time allow adapters to reconcile
later without inferring success from enabled intent.

App declared with no deployed record is `not_deployed`. Deployed revision,
repository revision and image are separate; an abbreviated hexadecimal deployed
SHA matching the repository SHA prefix is not drift. Runtime probe state is
`unknown`/`unavailable`, `observed_at: null`, with `not_probed` or
`runtime_not_configured`; the overview performs no live Docker/network probes.
A job without runs is `not_run`. Latest successful run clears older failure
attention. Stable attention sources cover merge debt, failed/unprotected
sessions, pending adapters, repository stale/failure, service conflicts,
latest failed build/job and revision drift. No inferred TODO/SDE/work-item
records are synthesized. Revisioned explicit work items remain a separate API.

See `docs/registry-rollout.md` for a complete example and deployment checks.


## Browser dashboard and explicit work items (B2)

The host-bound browser shell is served at `/` and `/ui/*`, with self-hosted `/browser/assets/*`. It consumes B1 session identity and A2 safe projections; cookie authentication never extends to bare or `/sigiled` machine endpoints. The complete route/body/persistence contract is in [browser-dashboard.md](browser-dashboard.md).

Added browser-only adapters: POST `/browser/api/projects`; GET `/browser/api/projects/{project}/jobs/{job}/runs`; GET/POST `/browser/api/projects/{project}/work-items`; GET/PATCH `/browser/api/projects/{project}/work-items/{id}`. Mutations require actual B1 Origin/CSRF and identity. Project creation keeps the existing approval policy, serializes per-project work, and safely resumes partial provisioning without replacing incumbent keys. Explicit work items use durable UUID create deduplication and revision CAS with atomic audit history; they do not dismiss derived system attention.

IDE controls, research/model actions and memory curation remain pending their reviewed adapters. Browser disconnect never closes/merges workspaces. See the dashboard contract for explicit origin selection, limits, uncertain-save recovery and release prerequisites.

## Inherited editor runtime (C1/C3 source contract)

`GET /sessions/{id}/ide` is an owner/admin-authorized safe capability/status
projection. `POST /sessions/{id}/ide` accepts an exact `generation` and `action`
`start`, `stop`, `checkpoint` or `finish`, under the existing project approval
policy. Full-write start requires the verified master-update restriction and
distinct host User merge transport described in [ide-runtime.md](ide-runtime.md).
Desired IDE defaults do not imply readiness. Unsupported/missing layers and
policy uncertainty do not disable ordinary agent workspaces. Checkpoint means
saved files committed and pushed to the session ref; finish invokes normal close
only after checkpoint success. Browser disconnect does not merge. Settings and
extensions use operator volumes with separate session/generation live profiles;
unsaved browser buffers are outside the durability promise. C2/browser and live
edge deployment remain separate release prerequisites.

### IDE lifecycle durability receipt (C1/C3 review correction)

IDE-used close, recycle and reap require internal companion POST /finish: owned editor process group stopped, pending preferences published, and the expected session commit pushed with a verified clean working tree. Receipt: state=finished, exact generation, valid pushed SHA and dirty=false. Concurrent saved changes or failed verification preserve the workspace. Manual checkpoint alone is insufficient for destruction. Public IDE action names remain start/stop/checkpoint/finish. Activity command events now require a stable execution_id alongside the existing generation string; unmatched/duplicate ends cannot release another known command.

## C2 browser IDE access

[ide-gateway.md](ide-gateway.md) defines the implemented human launch, safe status, checkpoint/finish and isolated preview adapters. Browser generation fields are canonical decimal strings; machine contracts stay numeric. Durable actor/project allocation intents survive lost responses and never adopt agent sessions. Runtime tokens remain server-side. Feature readiness defaults off pending actual deployment validation.
