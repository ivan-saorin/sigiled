# VM-TMPL — workspace template

Template repository for MGR-managed project workspaces. `POST /projects` on MGR
generates a new private repo from this template.

## Layout

- `mgr.toml` — workload manifest (class, source, routes, volumes, secrets).
  Read by MGR; always in git.
- `server/` — Rust base server `vm-base` (axum): fs / git / exec / health +
  session-token auth (§5 of the mission spec).
- `ext/` — optional Rust extension crates compiled into the base server at
  image build; routes under `/x/{crate}`. Activation: commit → `recycle`.
- `build-ext.sh` — folds `ext/*` into the server build (regenerates
  `ext_registry.rs` + dependency markers in `server/Cargo.toml`).
- `Dockerfile` — reproducible multi-stage build (`rust:1.97.1-slim` →
  `debian:13.6-slim` + git/ssh), non-root uid 1000, healthcheck via
  `vm-base health-probe`.

## Runtime contract

Env: `SESSION_TOKEN` (required, min 16 chars — minted by MGR, sole identity,
provider-blind per §3.12), `PORT` (8000), `WORKSPACE_DIR` (/workspace),
`EXTRA_PATHS` (colon-separated declared mounts), `GIT_SSH_KEY`
(/secrets/deploy_key — per-project deploy key injected by MGR).

Every endpoint requires `Authorization: Bearer $SESSION_TOKEN` (constant-time
compare). Every authorized call except `GET /health` bumps `last_activity`
(epoch secs, reported by `/health` — reaper input; polling never keeps a
session alive).

## API

- `GET  /health` → status, version, last_activity, idle_secs, uptime_secs
- `GET  /fs/list?path=` · `POST /fs/read|write|delete` — absolute paths,
  sealed to `/workspace` + `EXTRA_PATHS`; utf-8 with base64 fallback
- `GET  /git/status|diff?ref=|log?ref=&limit=|branches|show?ref=&path=` —
  branch-list and show/log on any branch are first-class (job-recap flow)
- `POST /git/commit {message}` — add -A, commit, **immediate push** (§3.5)
- `POST /exec {cmd, cwd?, timeout_secs?}` — bash -lc, 1 MiB capture/stream,
  default 300 s / max 3600 s timeout, full power inside the container seal
- `/x/{ext}/...` — extension routes, same token gate

## Extension convention

`ext/<name>/` is a crate named `<name>` exposing
`pub fn router() -> axum::Router`. It is nested at `/x/<name>` inside the
token gate. No cargo in the session hot path: MGR rebuilds the per-project
image `vm-{project}` when `ext/` changes (§3.3).

Any LLM driving a workspace: state lives in git or declared volumes, nothing
else survives the container; read recent `git log` on start and write
intent-carrying commit messages (§3.12) — the repo is the only shared memory.
