# SIGILED — the driving contract for the stack

*Session Execution And Lifecycle — the session is sigiled, with a sigil.*

## The driving contract for the automa stack — v2

**Contract version:** 2.0.0-draft · **Source:** `docs/sigiled-contract.md` in `ivan-saorin/sigiled`, served by `GET /mgr/contract` at the deployed sha.
**Status:** draft until DEC-01…10 are ratified (see `docs/mgr-v2.md` §8); v2 behavior lands progressively across the four build sessions. The v1 contract (single bearer, session lock) remains authoritative for a running v1 until cutover.

This is the complete operating contract for SIGILED (v2 of MGR). It is
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

New in v2: **many drivers, no waiting.** Sessions no longer lock the
project — each workload gets its own branch and container; master is the
arbiter at close. Coordination is paid only when conflicts actually exist
(merge debt, §5). Authentication is per-driver (OAuth2 against the stack
IdP), and human approval is a first-class, auditable object.

Workload classes: **session** (interactive, yours), **job** (cron-run
batch, append-only history), **app** (resident service). You mostly drive
sessions.

## 1. Bases and credentials — two-legged auth

| Surface | Base | Auth |
|---|---|---|
| MGR verbs | `https://api.016180.xyz/mgr` | `Authorization: Bearer <access-token>` |
| Workspace | `https://api.016180.xyz/s/{project}` | same Bearer **and** `X-Session-Token: {token}` |
| Web search | `https://search.016180.xyz` | same Bearer |

**Machine leg (yours).** Each driver is an OAuth2 client of the stack IdP
(`auth.016180.xyz`, Authentik): your skill carries a per-driver
`client_id` + `client_secret` (providers `mgr-claude`, `mgr-kimi`, …).
Mint your own short-lived access token when needed:

```
POST https://auth.016180.xyz/application/o/token/
  grant_type=client_credentials&client_id=…&client_secret=…
```

Token expired → mint another. 401 on a fresh token → the operator rotated
your credentials: ask for the new pair. Never echo credentials or tokens
into chat, logs, or committed files. During the dual-auth migration window
the legacy stack bearer is also accepted (maps to the bootstrap admin) —
it dies at cutover.

**Human leg (the operator's).** Operations that require "the operator
approved this" (see capability map, §4) go through the device flow:

```
POST /mgr/auth/elevate        → { verification_uri, user_code, expires }
```

Relay the URL + code to the operator in chat; they approve in the browser
once. SIGILED polls, then keeps the approval tokens in its own DB (never in
skills, never in transcripts) and auto-refreshes them. Inspect with
`GET /mgr/auth/approvals`. An approval names a human and an expiry; it
rides alongside your driver identity as `actor: {driver, approval}` on
every session and job record.

The workspace contract is two-header by design: the edge validates your
bearer, then swaps `X-Session-Token` into `Authorization` before the
request reaches the container. The token comes from `POST .../sessions`;
a token minted for project A is rejected by project B's container — never
reuse tokens across projects, or after `recycle`/`close`.

Request and response bodies are JSON unless noted (`git/diff` and
`git/show` return plain text; `GET /mgr/contract` returns markdown).

## 2. Commands — the normal operations

Invoked as `/sigiled <command>`, as skill args, or as bare words in an automa
context ("status" alone means `/sigiled status`). Session commands keep
`session_id`, `token` and `endpoint` in conversation memory. Anything not
covered here falls through to the full API (§4, §6).

| Command | Procedure |
|---|---|
| `status` | `GET /mgr/healthz` + `GET /mgr/projects`. Report version and, per project, `merge_debt` queue, `template_behind`, `needs_merge`; **any merge debt is shouted first**. |
| `projects` | `GET /mgr/projects` — full records (incl. `template_version`, `template_behind`). |
| `new <name>` | Requires approval for `stack:drivers`. `POST /mgr/projects` `{name}` (lowercase alnum+dash, 2–39 chars, letter first). Warn first: there is no delete verb — projects are permanent. |
| `open <project>` | `POST /mgr/projects/{p}/sessions`. Store `session_id`, `token`, `endpoint`. **If the response carries `merge_debt`, resolving it is your first and only job (§5).** Then rule 1: `GET /git/log?limit=15` and summarize the handoff before any write. |
| `close` | Commit pending work, then `POST /mgr/sessions/{id}/close`. Report the merge outcome (`ff` / `merged` / `debt`) and `log_operativo_touched`. |
| `recycle` | `POST /mgr/sessions/{id}/recycle`. Replace the stored token (the old one is dead), confirm with `GET {endpoint}/health`. |
| `elevate` | `POST /mgr/auth/elevate` → relay URL + code to the operator; poll status via `GET /mgr/auth/approvals`. |
| `log <project>` | `GET /mgr/projects/{p}/log` — the machine layer of history (sessions, merges, job runs). The narrative layer is `docs/log-operativo.md` in the repo. |
| `jobs <project>` | `GET /mgr/projects/{p}/branches` filtered to `job-*`, plus `GET /mgr/projects/{p}/jobs/{j}/runs` per job of interest; summarize outcomes newest-first. |
| `run <project> <job>` | `POST /mgr/projects/{p}/jobs/{j}/run`. 202 → run started; 409 → a run of the same job is in flight; 422 → broken `[jobs]` table on master. |
| `recap <project> [job]` | Recap flow (§7): runs → branches → in a session, `GET /git/log?ref=origin/job-…` and `GET /git/show?ref=…&path=…`. Never merge or delete job branches. |
| `apps <name> [action]` | `GET /apps/{a}` for status. `start`/`stop`/`restart`/`upgrade` require approval for `stack:drivers`. 202 `action=building` = background build: poll status, never re-fire the verb. |
| `sync <project>` | Template recepimento (§8): run `tools/sync-template.sh` in a session; it stops on drift. Never auto-update. |
| `search <query>` | `GET https://search.016180.xyz/search?q=<urlencoded>&format=json`; summarize `results[]`. |

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
   never block each other. The only lock left is the per-project merge lock
   — a critical section of seconds inside `close`. Never retry-hammer it.
5. **The sigil is the container.** `exec` is full bash, but only the
   container filesystem + declared mounts exist. Host paths, other
   projects, docker, ssh: structurally out of reach. Do not try.
6. **Do not idle.** ~1 h without API calls and the reaper auto-closes the
   session: uncommitted work is autosave-committed and pushed, nothing is
   merged, the container is destroyed. No work is lost, but your token is
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

## 4. MGR verbs

Base `https://api.016180.xyz/mgr` — bearer only. Nothing exists beyond
this table. Capability map: `stack:admins` do everything; `stack:drivers`
need a live approval for `projects new`, app verbs, and any session on
`sigiled` / `sigiled-supervisor` (the control plane demands a present operator).

| Verb | Returns |
|---|---|
| `GET /healthz` | `{status, version}` |
| `GET /contract` | this document (markdown), at the deployed sha |
| `POST /auth/elevate` | `{verification_uri, user_code, expires}` — device flow via the stack IdP |
| `GET /auth/approvals` | live approvals `{human, driver, expires}` |
| `POST /projects` `{name}` | 201 project record · 409 already registered · 422 bad name |
| `GET /projects` | all project records: `template_version`, `template_behind`, `merge_debt` queue, `needs_merge` |
| `GET /projects/{p}/log` | machine history: sessions (with `actor`), merge outcomes, job runs — JSON; `?format=md` renders markdown |
| `GET /projects/{p}/branches` | `[{name, sha}]` — job-recap entry point |
| `POST /projects/{p}/sessions` | 201 (§5; `merge_debt` on top when present) · 503 `{retry:true}` repo not ready — wait ~5 s, retry |
| `GET /sessions/{id}` | session record minus token; plus `container` + `logs` while running |
| `POST /sessions/{id}/close` | `{closed, merge: "ff"\|"merged"\|"debt", sha, flushed, log_operativo_touched}` |
| `POST /sessions/{id}/recycle` | fresh `{token, endpoint, sha_at_recycle}` — old token dead |
| `POST /projects/{p}/jobs/{j}/run` | 202 run record · 404 unknown job · 409 same job in flight · 422 broken `[jobs]` |
| `GET /projects/{p}/jobs/{j}/runs` | last 20 run records, newest first |
| `GET /apps/{a}` · `POST /apps/{a}/start\|stop\|restart\|upgrade` | app status / action — approval territory for drivers |

## 5. Session lifecycle — the main loop

**Start**: `POST /mgr/projects/{p}/sessions` → 201:

```json
{"session_id": "…", "project": "…", "branch": "session/…", "token": "…",
 "endpoint": "https://api.016180.xyz/s/{p}/", "head": "<sha>",
 "stale": false, "last_commit": null,
 "merge_debt": null,
 "actor": {"driver": "mgr-claude", "approval": null}}
```

- `stale: true` — you are resuming an existing branch after an auto-close
  or a lost container; `last_commit` is its pushed head. Rule 1 applies
  double.
- `merge_debt` non-null — rule 9. The container starts from the debtor
  branch with the merge in progress and conflict markers in the files:

```json
"merge_debt": {"branch": "session/…", "conflicted_files": ["…"],
  "ours": {"sha": "…", "commit_messages": ["…"]},
  "theirs": {"sha": "…", "commit_messages": ["…"]}, "since": "…Z"}
```

**Work**: everything through `endpoint` with both headers (§6). The cycle
is read → edit → `exec` to build/test → `POST /git/commit`.

**Recycle** (`POST /mgr/sessions/{id}/recycle`): flush, destroy the
container, recreate it **from your branch** with a freshly minted token.
Use when handing the session to another provider or when the container is
wedged. Replace your stored token with the returned one.

**Close** (`POST /mgr/sessions/{id}/close`): flush, then under the
project's merge lock (seconds): fast-forward if master has not moved,
three-way merge if it has and the changes are disjoint, **merge debt**
otherwise — master stays put, your branch survives, the debt package is
recorded, and the next session on the project inherits it (rule 9).
Simultaneous closes serialize on the lock: one wins, the other sees a
moved master and takes the merge path.

## 6. Workspace API

Base = `endpoint` from start/recycle. Both headers on every call. Every
authorized call except `GET /health` counts as activity (rule 6). Paths
are absolute, sigiled to `/workspace` + declared mounts.

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

Jobs are declared in the project repo's `mgr.toml` **on master**:

```toml
[jobs.<name>]
cron = "30 3 * * *"         # container-local time
command = "./jobs/x.sh"     # bash -lc in /workspace
timeout_minutes = 30        # 1..60 — hard wall clock
hc_ping = "MY_HC_URL"       # optional stack-env ref, pinged on finish
[jobs.<name>.secrets]
SOME_KEY = "STACK_ENV_VAR"  # container env <- SIGILED env, resolved at creation
```

Changing job definitions = editing `mgr.toml` in a session and closing it
(definitions are read from master, refreshed within ~5 min). A broken
`[jobs]` table disables that project's jobs until fixed; a manual trigger
surfaces the parse error as 422.

Each run: fresh container from master → branch `job-<name>-<YYYYMMDD-HHMMSS>`
pushed at creation → command → leftover output committed → destroyed.
Run states: `running · succeeded · failed · timeout · error ·
skipped_locked · aborted`.

**Recap flow** ("what did the jobs do last week"):
1. `GET /mgr/projects/{p}/jobs/{j}/runs` — outcome metadata.
2. `GET /mgr/projects/{p}/branches` — filter `job-*`, sort by stamp.
3. In a session on that project: `GET /git/log?ref=origin/job-…` then
   `GET /git/show?ref=origin/job-…&path=…` — read the content itself.

Never merge or delete job branches (rule 7).

**Resident apps** live in the same mgr.toml — at most **one** `[app]` per
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

- `mgr.toml` on master carries `template = "vm-tmpl@x.y.z"`; the project
  record exposes `template_version` and `template_behind`.
- The project Dockerfile is thin: `FROM vm-base:x.y.z` + the project's own
  toolchain layers (DEC-17). Adopting a new agent = bumping the tag.
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
| 403 | capability requires approval | `POST /mgr/auth/elevate`, relay code to operator, retry after approval |
| 404 | unknown project / session / job / app | typo, or job defs not on master yet |
| 409 | merge lock busy, job already in flight, or name taken | seconds-scale: retry close once after a beat; never hammer |
| 422 | invalid name / broken manifest | fix the input; do not retry as-is |
| 503 + `retry` | fresh repo still materializing | wait ~5 s, retry the start |
| 502 | upstream down | report to the operator |

## 10. Provider handoff

Multiple providers drive one project — now concurrently, each on its own
branch, each with its own identity (`actor.driver`). The repo is the only
shared memory — rules 1, 2 and 10 are the entire handoff protocol. To hand
one *session* to another provider, `recycle`: new container, new token,
the previous driver structurally cut off.
