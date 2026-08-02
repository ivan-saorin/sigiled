# vm-tmpl v2 — workspace template

Template for SEAL-managed project repos. `POST /projects` generates a new
private repo from this directory. Everything here becomes project-owned at
creation **except** the template-owned paths listed below, which the
recepimento (`tools/sync-template.sh`, arriving with build session 2) may
replace when the project re-pins to a newer tag.

## Layout

- `Dockerfile` — thin: `FROM vm-base:x.y.z` (DEC-17) + project toolchain
  layers. The agent (fs/git/exec/health API) lives in the base image; the
  project only adds what it needs.
- `mgr.toml` — workload manifest, read by SEAL from master. Carries the
  `template = "vm-tmpl@x.y.z"` pin (DEC-05).
- `docs/log-operativo.md` — narrative operating log. Born here, then
  project-owned forever (DEC-04): no sync ever touches it.
- `ext-rust/` — extension crates compiled into the agent, routed at
  `/x/<name>` (DEC-18). Empty by default. `ext-py/`, `ext-go/` follow the
  same route contract as supervised local processes (their runtime comes
  from the project's Dockerfile layers).

## Ownership

| Path | Owner |
|---|---|
| `Dockerfile` FROM line + `mgr.toml` template pin | updated by re-pin/sync |
| everything else, `docs/log-operativo.md` above all | project, forever |

Any LLM driving this workspace: state lives in git or declared volumes,
nothing else survives the container. Read `git log` and the log operativo
on start; write intent-carrying commit messages; add a log entry when you
close coherent work. The full contract: `GET /mgr/contract`.
