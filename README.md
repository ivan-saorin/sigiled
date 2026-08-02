# SEAL — Session Execution And Lifecycle

SEAL is the platform: the workload orchestrator of the automa stack
(`api.016180.xyz`), its workspace agent, and its project template — one repo,
self-managed as an MGR/SEAL project itself (DEC-11/12). v2 of what was MGR.

## Layout

- `seald/` — the orchestrator (control plane): axum service behind the edge.
  Serves `/healthz`, the canonical driving contract at `/mgr/contract`, and
  the MGR verbs (built out across the v2 sessions).
- `vm-base/` — the workspace agent: fs / git / exec / health + session-token
  auth. Built into the published base image `vm-base:x.y.z` (DEC-17);
  project images start `FROM` it. `build-ext.sh` folds `ext-rust/` crates in
  (DEC-18).
- `template/` — vm-tmpl v2: what a new project repo is generated from. Thin
  Dockerfile (`FROM vm-base`), docs skeleton (log-operativo), commented
  `mgr.toml` with template pin, empty `ext-rust/` example.
- `images/` — image build scripts (`build-vm-base.sh`).
- `docs/` — design (`mgr-v2.md`, DEC-01…18), build plan, the canonical
  contract (`seal-contract.md`), and the narrative `log-operativo.md`.
- `Dockerfile` (root) — session image of this repo under MGR v1 (compat until
  cutover) and source of `vm-base:x.y.z`.

## Ground rules

Rust everywhere (DEC-16). Secrets never in git — they arrive as env. Master
only moves through session close. The repo is the memory: read
`docs/log-operativo.md` and `git log` before touching anything.
