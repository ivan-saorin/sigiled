# vm-tmpl v2 — workspace template

Template for SIGILED-managed project repos. `POST /projects` generates a new
private repo from this directory. Everything here becomes project-owned at
creation **except** the template-owned paths listed in
`tools/template-allowlist`, which the recepimento (`tools/sync-template.sh`)
may replace when the project re-pins to a newer tag.

## Layout

- `Dockerfile` — thin: `FROM vm-base:x.y.z` (DEC-17) + project toolchain
  layers. The agent (fs/git/exec/health API) lives in the base image; the
  project only adds what it needs.
- `sigiled.toml` — workload manifest, read by SIGILED from master. Carries the
  `template = "vm-tmpl@x.y.z"` pin (DEC-05).
- `docs/log-operativo.md` — narrative operating log. Born here, then
  project-owned forever (DEC-04): no sync ever touches it.
- `tools/` — the recepimento: `sync-template.sh` (vendored-replace to the
  pinned tag, stops on drift) and `template-allowlist` (the template-owned
  paths, declared by the template itself). Never auto-run (DEC-07): a driver
  invokes it in a session, reviews the diff, commits.
- `ext-rust/` — extension crates compiled into the agent, routed at
  `/x/<name>` (DEC-18). Empty by default. `ext-py/`, `ext-go/` follow the
  same route contract as supervised local processes (their runtime comes
  from the project's Dockerfile layers).

## Ownership

| Path | Owner |
|---|---|
| paths in `tools/template-allowlist` | template — replaced by sync; local edits = drift, sync stops |
| `Dockerfile` FROM line + `sigiled.toml` template pin | updated by conscious re-pin, never by sync |
| everything else, `docs/log-operativo.md` above all | project, forever |

`TEMPLATE_VERSION` at the repo root records the last synced tag plus a
checksum per template-owned file — it is how the sync detects drift.

Any LLM driving this workspace: state lives in git or declared volumes,
nothing else survives the container. Read `git log` and the log operativo
on start; write intent-carrying commit messages; add a log entry when you
close coherent work. The full contract: `GET /sigiled/contract`.
