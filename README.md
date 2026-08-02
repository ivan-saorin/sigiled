# SIGILED

**SIGILED** is an open-source, AI-native workload orchestrator. It rents a
driver — human or LLM — a disposable Linux container wired to exactly one git
repo, drivable over a narrow HTTP API: read, write, exec, commit. Every commit
is pushed immediately; the container is cattle; **the repo is the only
memory** — across sessions, and across whatever models drive the same project.

*(sigiled, adj. — marked with a sigil. Not the MTG cards, not the Raid
champion: this one orchestrates workloads.)*

Three workload classes:

- **session** — interactive, one driver at a time, merged to master on close;
- **job** — cron-run batch, each run on its own append-only branch: history a
  language model can read back;
- **app** — resident service, built from the repo, deployed by verb.

The driving rules are not documentation *about* the platform — they are the
product. The orchestrator serves its own canonical contract at
`GET /sigiled/contract`, at the exact sha it is running: any LLM that can issue
HTTPS requests can fetch it and start driving.

## Status

v2 is under construction — in the open, by LLM drivers, in SIGILED sessions on
this very repo (it is self-managed: session `sigiled`, supervised for
resurrection by [sigiled-supervisor]). v1 runs the reference stack today.
The build plan is [docs/v2-build-plan.md](docs/v2-build-plan.md); progress is
narrated in [docs/log-operativo.md](docs/log-operativo.md) and in `git log` —
by design, the two things a new driver must read first.

## Layout

- `sigiledd/` — the orchestrator (control plane): axum service behind the
  edge. Serves `/healthz`, the canonical driving contract at `/sigiled/contract`,
  and the SIGILED verbs (built out across the v2 sessions).
- `vm-base/` — the workspace agent: fs / git / exec / health + session-token
  auth. Built into the published base image
  `ghcr.io/ivan-saorin/vm-base:x.y.z` (public, pull without credentials);
  project images start `FROM` it. `build-ext.sh` folds `ext-rust/` crates in.
- `template/` — vm-tmpl v2: what a new project repo is generated from. Thin
  Dockerfile (`FROM vm-base`), docs skeleton (log-operativo), commented
  `sigiled.toml` with template pin, empty `ext-rust/` example.
- `images/` — image build scripts (`build-vm-base.sh`).
- `docs/` — design (`sigiled-v2.md`, DEC-01…22), build plan, the canonical
  contract (`sigiled-contract.md`), deploy runbook, and the narrative
  `log-operativo.md`.
- `Dockerfile` (root) — session image of this repo under SIGILED v1 (compat until
  cutover) and source of `vm-base:x.y.z`.
- `index.html` + `CNAME` — the landing page ([sigiled.dev]), served by
  GitHub Pages.

## Security model, in short

Two-legged identity: machine credentials per driver (OAuth2
`client_credentials`), device-flow approval for the human on privileged
verbs. Workspace calls take two headers — the edge validates the stack
credential, then swaps the per-session token in. Secrets never touch git:
they arrive as container env, injected at creation. The workspace is sealed:
the container filesystem plus declared mounts, nothing else — no docker, no
ssh, no host.

## Self-hosting

Everything stack-specific is env/config with documented defaults: base-image
registry (`VM_BASE_REGISTRY`), git identity of session commits
(`GIT_AUTHOR_*`/`GIT_COMMITTER_*`). See
[docs/runbook-deploy.md](docs/runbook-deploy.md). A proper quickstart lands
with the v2 cutover.

## A note on language

The code, the contract and this README are English. The design docs and the
operational log are **Italian** — they are the project's living memory,
written in the language of its operator ("il Re", the ratifier of the DEC
decision records you'll find in `docs/sigiled-v2.md`). Translate nothing;
`git log` speaks both.

## License

[MIT](LICENSE).

[sigiled-supervisor]: https://github.com/ivan-saorin/sigiled-supervisor
[sigiled.dev]: https://sigiled.dev
