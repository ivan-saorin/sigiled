# SIGILED v2 — Build plan in 4 sessions

**Version:** 0.1-en · **Date:** 2026-08-02 · **Expected driver:** Claude Code (holds for any driver)
**Source of truth:** `docs/sigiled-v2.md` (design + DEC-01…24). This plan is execution; on conflict the design doc wins.
**Language note:** official English version; the Italian original is `v2-build-plan_it.md`. Sessions 1–4 and the cutover are **complete** (see the operational log) — this document is kept as the historical execution record; session 1b closed last, 2026-08-03.

**Note 2026-08-03:** the platform is called **SIGILED** (DEC-12 as amended); build sessions open on the `sigiled` project. DEC-19/20 (open source, ghcr) were ratified along the way; "session 1b" below was added by Claude Code.

---

## How to use this plan

- **One session = one section (§2…§5).** Each session: open on `sigiled` → git log → read `docs/sigiled-v2.md` + this plan + `docs/log-operativo.md` → work → intent-carrying commit at every coherent step → operational-log entry → **close**. Never leave sessions open.
- **Master always closes green**: build + tests pass at end of session.
- **Rust everywhere** (DEC-16). Minimal dependencies, motivated in the commit message.
- **The workspace has no docker/ssh**: inside the session — build, unit tests, mocks. Deploy and smoke on the box belong to the operator — leave precise instructions in the log.
- **Secrets never in git**; they arrive as env (contract rule 8).
- **If a DEC does not survive contact with the code**: do not change it silently — log entry + amendment proposal in `sigiled-v2.md`; il Re ratifies.
- **If you overrun the session**: honest wip commit + "in progress" log entry; the next session recalibrates, recording the deviation.

---

## 1. Prerequisites (operator, before session 1)

1. **Current SIGILED code**: push to `ivan-saorin/sigiled` master, or an explicit greenfield decision. If the current code is not Rust, DEC-16 implies a **guided rewrite** — the v1 driving contract (the skill) is the complete spec of v1 behavior.
2. **Authentik API token** in stack env (needed by session 3); alternatively the operator creates the providers by hand with the instructions session 3 will leave.
3. **Image registry** for `vm-base:x.y.z` reachable from the box (ghcr.io or a stack registry).
4. Sessions on `sigiled` run on SIGILED **v1** with the legacy bearer until dual-auth exists: normal administration, v2 is built from inside v1.

---

## 2. Session 1 — foundations: repo, domain, contract, base image

**Deliverables:**

- **Initial assessment** (first thing, in the log): evolve the imported code vs greenfield. Motivated in five lines.
- **Repo layout**: `sigiledd/` (orchestrator), `vm-base/` (agent: port of vm-tmpl `server/`+`build-ext.sh`, `ext-rust/` convention), `template/` (vm-tmpl v2: Dockerfile `FROM vm-base` + `docs/` skeleton with log-operativo + commented `sigiled.toml` + empty example `ext-rust/`).
- **`GET /healthz`** `{status, version}` and **`GET /sigiled/contract`**: serves the canonical contract from the repo at the deployed sha. Includes **writing `docs/sigiled-contract.md`** — the v2 contract, generated from `sigiled-v2.md` (new rules: concurrency, merge debt, operational log, two-legged auth).
- **Parsing `template = "vm-tmpl@x.y.z"`** in `sigiled.toml`: the project record gains `template_version`, exposed in `GET /sigiled/projects`.
- **Base-image script** (`images/build-vm-base.sh`): local build of `vm-base:0.1.0`; push = operator if the registry is not reachable from the workspace.

**Acceptance:** cargo build green; domain unit tests green; `/sigiled/contract` answers in a local run; log entry with assessment and discards; close.

## 2b. Session 1b — 100% open source (DEC-19/20)

Il Re decided (2026-08-02, in chat, direct ratification): SIGILED v2 will be
**100% open source**, with a GitHub Pages landing that explains the project.
The repo is written from now on as if it were public. This session makes the
repo "flip-ready": publishing must reduce to flipping the visibility switch.

**Deliverables:**

- **LICENSE**: Apache-2.0 was proposed here; **il Re ratified MIT instead**
  (DEC-23) — simplicity over the patent grant.
- **Naming check**: the collisions that motivated this item concerned the
  platform's birth name, eradicated with DEC-12 as amended. What remained was
  verifying that **sigiled** is clean and registering the outcome in a DEC
  (done: DEC-21). Landing and README use the verified name with a
  disambiguating tagline.
- **Git-history hygiene audit**: scan the whole history (bearers, tokens,
  keys, private paths) — the history is young, do it NOW while rewriting it
  is still painless. Permanent rule from here on: every commit is written
  knowing it will be public.
- **Stack-specifics generalization**: inventory of the places where the
  reference stack is hardwired (domains, registry account, provider names)
  → in sigiledd they become config/env with documented behavior — identity
  values (DOMAIN, GITHUB_OWNER) are **required, never defaulted**; the design
  docs remain free to cite the reference stack as the exemplar instance.
- **Public ghcr (DEC-20)**: `template/Dockerfile` → `FROM
  ghcr.io/ivan-saorin/vm-base:0.1.0`; `images/build-vm-base.sh` with the same
  default for `VM_BASE_REGISTRY`; instructions to mark the package public in
  `docs/runbook-deploy.md`.
- **Public README (EN) + gh-pages landing**: the page explaining the mental
  model, the contract as the product (`GET /sigiled/contract`), reconciliation
  concurrency, self-host quickstart, security model. Pages activates only at
  the flip; everything is prepared here.

**Acceptance:** LICENSE present; history audit documented clean in the log;
FROM/scripts coherent on ghcr; landing renderable; naming registered in DEC;
build+tests green; close.

## 3. Session 2 — machine operational log + adoption

**Deliverables:**

- **`GET /sigiled/projects/{p}/log`**: mechanical history from the DB (sessions, closes with merge outcome, job runs) — JSON + markdown render.
- **Close hint**: `log_operativo_touched: true|false` in the close response.
- **`template/tools/sync-template.sh`**: vendored-replace on an allowlist + pin to tag + **drift detection** (stops and reports if template-owned paths have local modifications).
- **`template_behind`** in `GET /sigiled/projects` and in `status`.
- **Test bench**: the `mgr-smoke` project already exists (tombstone name in the v1 registry: no delete verb, DEC-22) — use it for the end-to-end sync test (induce drift, verify it stops and reports).

**Acceptance:** sync on `mgr-smoke` with induced drift → stop + report; clean sync → commit + updated `TEMPLATE_VERSION`; machine log readable via API; close.

## 4. Session 3 — two-legged auth

**Deliverables:**

- **Dual-auth middleware**: accepts the legacy bearer (→ bootstrap admin) **or** an Authentik JWT (JWKS RS256 with cache; introspection as fallback).
- **`actor` {driver, approval}** on sessions and jobs; **capability map v1**: `stack:admins` / `stack:drivers`; approval required for `projects new`, apps verbs, and **sessions on `sigiled` and `sigiled-supervisor`** (DEC-15).
- **`POST /sigiled/auth/elevate`**: device flow via the `sigiled-device` provider; token held in the DB, serialized refresh; `GET /sigiled/auth/approvals` for inspection.
- **Provider provisioning** `sigiled-device` + `sigiled-<driver>`: via the Authentik API with the token from stack env, or step-by-step operator instructions (`docs/authentik-setup.md`).
- **Skill migration note** (`docs/skill-migration.md`): how a driver moves from the legacy bearer to `client_id`/`client_secret` — text ready to paste into the skills.

**Acceptance:** with a valid driver JWT → normal open/close; without approval → open on `sigiled` denied; with an operator-approved device approval → passes; legacy bearer still working (dual-auth window); close.

## 5. Session 4 — concurrency + supervisor + runbook

**Deliverables:**

- **No session lock**: open always admitted; per-project **merge-lock** (a critical section of seconds).
- **Close**: FF → three-way merge → **merge debt** with package `{branch, conflicted_files, ours/theirs + commit messages, since}`; `open` exposes `merge_debt` on top; `status` shows the queue. Update `docs/sigiled-contract.md` (new rules 4/7, resolution protocol, scruple build DEC-10).
- **`sigiled-supervisor`** (session on the other project): Rust ~100 lines — `GET /health`, `GET /sigiled/status`, `POST /sigiled/restart {sha?}` (sha = rollback), append-only log, static bearer from env. Per its own `docs/requisiti.md`.
- **`docs/runbook-deploy.md`** in sigiled: deploy via supervisor, rollback via sha, dead-stack recovery (SSH + manual steps).
- **Concurrency test on `mgr-smoke`**: two parallel sessions; clean merge in the disjoint case; induced conflict → debt → the following session resolves with the protocol.

**Acceptance:** concurrency test passed and narrated in the log; supervisor compiles and answers in a local run; runbook present; close.

---

## 6. Cutover (operator + il Re, after session 4 — out of session)

1. Deploy v2 on the box via supervisor, separate port/vhost (**canary**).
2. **Import of the v1 registry** (projects, locks, history) into the v2 DB — one-shot script, written in a dedicated session if the volume requires it.
3. Migration of the drivers' skills to the new auth (text ready from session 3), legacy bearer switch-off.
4. Edge switch → v2; v1 off but rollbackable (sha pinned at the supervisor).
5. "Cutover" log entry + `sigiled-v2.md` §5 update (sequence → done).
