# SIGILED — Design document

*(born "SIGILED v2", then "SIGILED" — renamed 2026-08-03 with DEC-12 as amended; the driving contract remains SIGILED)*

**Version:** 0.3-en · **Date:** 2026-08-03 · **Status:** ALL decisions ratified — DEC-01…10 ratified by il Re on 2026-08-03 (in chat, with the platform complete and live-verified); DEC-11…20, DEC-22 and DEC-23 ratified earlier, DEC-21 and DEC-24 registered, DEC-25 ratified 2026-08-08, DEC-26 ratified 2026-08-09, DEC-27 ratified 2026-08-15; DEC-12 **amended** 2026-08-03 → platform **SIGILED** (`ivan-saorin/sigiled`, domains `sigiled.dev`/`sigilled.dev`). The contract is 2.2.0, no longer draft.
**Origin:** design session of 2026-08-02 (driver: Kimi K3), started from reading the updates of `tomes-and-tales` (auth deployed with Authentik) and `torchio` (v0.3 requirements, DEC-17/18): the patterns born inside the projects **move up one level**, into the platform.
**Language note:** this is the official English version; the Italian original lives in `sigiled-v2_it.md` and remains the project's working memory. On divergence, the file that records the later ratification wins.
**Operational memory:** `docs/log-operativo.md` — every session updates it (convention §2).

---

## 0. Summary

SIGILED v2 is **four changes** carried by a single principle: **git is the mental model of the whole stack**, not just of the repos.

1. **Two-legged auth** — LLM drivers become OAuth2 clients of Authentik (`client_credentials`, per-driver machine identity); human approval arrives via *device flow* with tokens held **on the SIGILED side**, never in the skills. The single shared bearer dies.
2. **Two-layer operational log** — SIGILED exposes the mechanical history from its own DB (`GET /sigiled/projects/{p}/log`); the narrative file `docs/log-operativo.md` is born from the template and belongs to the project forever.
3. **Template versioning** — adoption (torchio DEC-17) moves up a level: vm-tmpl versioned with tags, pin in `sigiled.toml`, on-demand sync with drift detection, **never auto-update**. The engine serves its own contract (`GET /sigiled/contract`).
4. **Reconciliation concurrency** — goodbye session lock: one branch per workload, merge at close, and a **merge debt** with a context package that the next session **must** resolve before any other work, whatever model is driving it.

The four compose: auth gives the log real authors; the log records template updates and merge debt; adoption distributes the new contract to the projects; concurrency makes everything multi-driver without locks.

---

## 1. Auth — two legs

### 1.1 The doctrine, generalized from tnt

As in tnt (docs/auth.md §1): **the IdP knows only membership; each application maps groups → capabilities locally**. Authentik is the stack's IdP, already live on the reference instance. SIGILED becomes a relying party like the others — no private IdP, no new console: Authentik IS the console.

### 1.2 Evidence gathered live (2026-08-02)

Verified with anonymous probes:

- `GET /application/o/tomes-and-tales/.well-known/openid-configuration` → **200**, full public discovery (OIDC is born discoverable).
- Declared grants: `authorization_code`, `refresh_token`, `implicit`, **`client_credentials`**, `password`, **`urn:ietf:params:oauth:grant-type:device_code`** (with a dedicated `device_authorization_endpoint`).
- `POST /application/o/token/` without credentials → `400 invalid_client`: the token endpoint is alive and guarded; the machine door already exists.
- `POST /application/o/device/` with only tnt's public `client_id` → `400 invalid_client`: the device endpoint is active; it needs a provider configured for device flow.
- The tnt provider's JWKS = `{}` because it signs **HS256** (symmetric). For the SIGILED providers: **RS256**, so SIGILED validates JWTs locally via JWKS.

### 1.3 Machine leg — client_credentials per driver

- **One OAuth2 provider per driver** (`sigiled-kimi`, `sigiled-claude`, …): each with its own `client_id`/`client_secret` pair, grant restricted to `client_credentials`, RS256 signing. In Authentik one provider = one pair → per-driver means per-driver revocation and per-driver audit.
- **The driver mints its own tokens**: `POST /application/o/token/` → short access token → `Authorization: Bearer` toward the API edge. Expired → take another. **Zero humans in the loop after setup.**
- **Concurrency-safe by construction**: every token is independent, no shared mutable state between parallel chats — the refresh-token rotation problem (§1.5) does not exist here.
- **In the SIGILED skill**: `client_id` + `client_secret` in place of the monolithic bearer. Same handling rules (never echo, never commit), same rotation story (401 → ask for the new value). A leak buys only short tokens of *that* driver, revocable in one click.

### 1.4 Human leg — device flow, custody on the SIGILED side

For the operations that must carry "the operator approved":

```
1. driver → POST /sigiled/auth/elevate
2. SIGILED (OIDC client) → POST <idp>/application/o/device/   [provider sigiled-device]
3. SIGILED → driver → chat: "go to <idp>/device, code ABCD-1234"
4. the operator approves from the browser (once)
5. SIGILED polls the token endpoint, obtains access+refresh tokens
   → holds them in its own DB, auto-refresh serialized inside its process
```

- **Access token 12 h, refresh 30 days** (provider configuration): the human approves ~**once a month per driver**, not 1-2 times a day.
- **The token never touches skill, PC or transcript**: SIGILED is the only component with persistent state and an always-alive process — the natural custodian. No rotation races: serialization is internal to SIGILED.
- Printing the `user_code` in chat is safe by construction: it only authorizes; the tokens go to whoever holds the `device_code`, which stays server-side.

### 1.5 Why not "long-lived token in the skill"

Hypothesis evaluated and discarded (device flow + auto-refresh with the refresh token saved in the skill):

1. **Rotation races** — Authentik rotates the refresh token on every use; multiple parallel chats with the same token invalidate each other, and the problem climbs back in through the window exactly during parallel sessions.
2. **Leak surface** — the skill is read whole into context on every activation: a 30-day secret in every transcript of every provider.
3. **File ≠ database** — the skill is a document shared across surfaces; making it rewrite itself on every refresh is fragile or impossible (some surfaces cannot write it).

### 1.6 Actor and capabilities

Every session/job record gains:

```
actor: { driver: "sigiled-kimi", approval: "ivan (device, expires 2026-08-03T01:00)" | null }
```

Capability map v1 (SIGILED-local):

| | `stack:admins` | `stack:drivers` |
|---|---|---|
| sessions open/close/recycle, git, exec | ✓ | ✓ |
| jobs run/recap | ✓ | ✓ |
| projects new | ✓ | with approval |
| apps verbs (start/stop/restart/upgrade) | ✓ | with approval |
| skill render (`GET /skill/{driver}`, DEC-24) | ✓ | with approval |

### 1.7 Migration from the single bearer

**Dual-auth** window: the edge accepts the legacy bearer (= bootstrap admin) and Authentik tokens; the `sigiled-*` providers get created, the skills get updated, then the legacy dies. The skill already handles rotation via 401: a compatible story. *(Executed: the window closed 2026-08-03.)*

### 1.8 Security notes

- `password` grant **disabled** on the SIGILED providers — only `client_credentials` (drivers) and `device_code` (sigiled-device).
- Driver secrets long and generated; verify rate-limiting at the edge on the token-endpoint route.
- Validation in SIGILED: **JWKS + RS256** (local, no per-request call); introspection as fallback/debug.
- Group claims in the tokens via Authentik property mapping — verified in configuration (needed for `stack:drivers` as a claim).

---

## 2. Operational log — two layers

- **Machine layer (SIGILED-owned)**: SIGILED already has all the data (sessions, closes with merge outcome, job runs, app builds) in its own DB. It exposes them: **`GET /sigiled/projects/{p}/log`**. Zero writes into project repos, zero violations of content ownership.
- **Narrative layer (driver-owned)**: `docs/log-operativo.md` with its contract at the top (three questions: where were we / where did we plan to go / what was done + discards, state, next step; entries are never deleted, they are corrected with new entries). **The skeleton is born from the template at project creation and from that moment belongs to the project, forever** — the torchio DEC-18 rule generalized. The template never touches it after creation: collision resolved by construction.
- **New SIGILED rule**: *close coherent work → add an entry on top of the operational log*.
- **Honest hint**: `close` answers with `log_operativo_touched: false` when the file was not modified — a mirror, not enforcement.

## 3. Template versioning — adoption moves up a level

The torchio DEC-17 mechanics, applied to vm-tmpl:

- **vm-tmpl versioned**: semver tags + CHANGELOG.
- **Pin at creation**: `sigiled.toml` on master gains `template = "vm-tmpl@x.y.z"` — its natural home, it is already the file SIGILED reads.
- **Adoption**: allowlist of SIGILED-owned paths + `sync` script + **drift detection** (if you touched SIGILED-owned files, it stops and reports). Rollback = `git revert` or re-pin to the previous tag.
- **Never auto-update** — symmetry with torchio DEC-07: SIGILED never rewrites project repos on its own initiative. Sync v1 in-session (close = the harness), v2 as a job.
- **Visibility**: `status` shows `template_behind: true` next to `needs_merge`.
- **The contract served by the engine**: **`GET /sigiled/contract`** — the canonical SIGILED text, versioned. With the already-existing `healthz.version`, every driver can verify the freshness of its own skill and regenerate it (now mechanical via `GET /skill/{driver}`, DEC-24).

### 3.1 The v2 workspace — base image + per-language ext (ratified 2026-08-02)

A fact surfaced reading vm-tmpl v1: every project vendors `server/` + `ext/` + `build-ext.sh` + the cargo stage of the `Dockerfile`, and recompiles vm-base on every build (with `COPY . .` busting the cache on every commit). v2 flips it:

- **Pre-built base image per tag**: vm-base is published as a tagged image (`vm-base:x.y.z`, stack registry). The project Dockerfile shrinks to `FROM vm-base:x.y.z` + the project's toolchain layers (python, go, …). Agent adoption becomes a **tag bump** (pin point fixed by DEC-25: `[workspace] dockerfile` in `sigiled.toml` names the file, the Dockerfile's `FROM` carries the tag). From the project repos disappear `server/`, vendored `ext/`, `build-ext.sh` and the cargo stage: with them dies the per-build recompilation.
- **Per-language ext**: the extension point generalizes to `ext-<lang>/`:
  - `ext-rust/` — the current convention: crates compiled **inside** vm-base (static, zero extra runtime);
  - `ext-py/`, `ext-go/`, … — local supervised processes in the container; vm-base reverse-proxies to port/socket.
  - Single contract unchanged: HTTP mounted at **`/x/<name>`**, inside the same token gate. The toolchain follows the ext: a project with `ext-py/` brings python into the image via a project layer.
- **vm-tmpl v2** accordingly: thin Dockerfile (FROM + toolchain hook), `docs/` skeleton (incl. log-operativo), commented `sigiled.toml`, empty example `ext-rust/`. The SIGILED-owned allowlist shrinks almost to zero — docs skeleton and little more: the bulk of adoption travels on the image tag.

## 4. Concurrency — from exclusion to reconciliation

### 4.1 The structural change

The lock does not disappear: **it shrinks from the whole life of the session to a critical section of a few seconds at merge time**. Coordination is paid for only when conflicts actually exist, and git is built to minimize them.

1. `open` → branch `session/{id}` from current master. **No more 409**: N concurrent sessions, N isolated containers, N per-session tokens (the token model already supports it, nothing changes).
2. Work, commit, automatic push — identical to today.
3. `close` → SIGILED acquires the project's merge-lock (seconds) and tries in sequence: **fast-forward** (master unmoved: the common case) → **three-way merge** (master moved but disjoint changes: git merges alone) → **conflict**.
4. Simultaneous closes: the critical section serializes them — one wins, the other sees master moved and takes the merge path.

### 4.2 Merge debt

On conflict: master stays where it is, the branch stays, and SIGILED records the context package — because whoever resolves it **wrote neither of the two halves**:

```json
merge_debt: {
  "branch": "session/…",
  "conflicted_files": ["docs/requisiti.md", "spina/linter.py"],
  "ours":   { "sha": "…", "commit_messages": ["…"] },
  "theirs": { "sha": "…", "commit_messages": ["…"] },
  "since": "…Z"
}
```

Intent-carrying commit messages (rule 2) pay off here a second time: they are the context for deciding.

- **`open` on a project with debt** → `merge_debt` at the top of the response, shouted.
- **Hard rule**: *resolve the merge debt BEFORE any other work, whatever model is driving you.*
- **Resolution protocol**: the container starts from the indebted branch with the merge in progress and the markers in the files → read both sides' commit messages → resolve → verify → commit explaining *what you kept and why* → close. **If you cannot decide: ask the operator, do not guess.**
- **Merge commits, not rebase**: they record the session boundary and who merged. Linearity is aesthetics; that trace is memory.
- The log's machine layer records failure and resolution; `status` shows the debt queue per project.

### 4.3 Semantic conflicts — the scruple build

Git can merge clean and produce a broken result (two sessions touch different parts of interdependent code). Rule:

> A session that notices **multiple merges in the project's recent history** runs the **scruple build** (build/test if the repo has them; a coherence re-read of the documents if it is a docs-only repo). **If it is broken: it MUST be fixed before proceeding.**

Future (hook, not v1): optional post-merge hook declared in `sigiled.toml` to automate the check.

### 4.4 The rewritten rules

- **Rule 4** — from "one workload per project" to: **one branch per workload; master is the arbiter at close.** The 409 on sessions disappears; the ban on retry-hammering the merge lock stays.
- **Rule 7** — from "master moves only via close (fast-forward)" to: **master moves only via close (FF preferred, merge otherwise).** Job branches remain append-only and never merge. The invariant that matters — master moves only via close — is intact.

---

## 5. Implementation sequence

*(Executed 2026-08-02/03 — see `v2-build-plan.md` and the operational log; kept for the record.)*

1. `GET /sigiled/contract` + vm-tmpl tag + pin in `sigiled.toml` (unlocks everything, low cost)
2. Operational log: machine layer + skeleton in the template + close hint
3. Auth: Authentik `sigiled-*` providers, dual-auth, actor on the records, death of the bearer
4. Adoption: sync script + drift detection + `template_behind`
5. Concurrency: merge-lock, merge at close, merge debt, new rules 4/7

## 6. Open questions

1. **Auth v1 scope**: operator + LLM agents only; the human group lattice (family/friends) arrives with the first real use case.
2. ~~Is SIGILED's own repo SIGILED-registered?~~ **Resolved 2026-08-02: yes — §7, DEC-11…15.**
3. Group claims in tokens via property mapping — verified on Authentik 2026.5.6.
4. Definitive list of approval-gated operations — ratified: `projects new`, apps verbs, skill render (DEC-24), sessions on platform projects (DEC-15).
5. Merge-debt threshold above which `status` shouts (candidate: 1 — any debt is shouted).
6. Definitive token durations (proposals: short auto-minted driver access ~1 h; approval 12 h; refresh 30 d).
7. ~~Workspace toolchain~~ **Resolved 2026-08-02: base image per tag + per-language ext — §3.1, DEC-17/18.**

---

## 7. Self-management — SIGILED is SIGILED-registered (ratified 2026-08-02)

The platform is called **SIGILED**: project and code in `ivan-saorin/sigiled`; the driving contract remains **SIGILED**. The engraved sentence: **SIGILED manages everything about itself except its own resurrection.**

| What | Where it lives | Who moves it |
|---|---|---|
| SIGILED's code | repo `ivan-saorin/sigiled` | SIGILED sessions, like every project |
| The SIGILED contract | `docs/` of this repo | sessions; served by `GET /sigiled/contract` at the deployed sha |
| The running service | the box, pinned sha | out-of-band deploy via **sigiled-supervisor** — never SIGILED on SIGILED |
| Runtime state | DB on the box (+ backup) | SIGILED; expand-contract migrations |
| Resurrection | `sigiled-supervisor` | its own API: calling it restarts sigiled |

Specific rules:

- **sigiled-supervisor**: minimal external supervisor (~100 lines — if it grows, it is doing it wrong), its own repo `ivan-saorin/sigiled-supervisor`, deployed independently of SIGILED (on the box, **never as a SIGILED `[app]`**: it is on the resurrection path, it cannot depend on what it resurrects). It exposes its own API: calling it = restart of sigiled (pull at the pinned sha → build → restart → health check → report). Protected and logged endpoint; it must remain reachable with the stack half dead, hence its own simple auth, not OIDC.
- **Platform bootstrap**: projects created fresh, code enters via the first session — works around the session-start 503 on adopted projects (known bug).
- **Mandatory approval**: sessions on `sigiled` and `sigiled-supervisor` require a valid approval (DEC-02/03) — the repo that governs all repos requires the operator present.
- **Never auto-deploy at close**: master moves via close; deploy remains a separate, human act.
- **Rollback runbook** in `docs/runbook-deploy.md` — readable from GitHub even with SIGILED dead.

---

## 8. Decision register

| # | Decision |
|---|---|
| DEC-01 | Two-legged auth: per-driver `client_credentials` (machine) + device flow with human approval (human). The single bearer dies after a dual-auth window. **Ratified 2026-08-03.** |
| DEC-02 | Custody of human tokens **on the SIGILED side** (DB + serialized auto-refresh); never in the skills, never in transcripts. In the skills only the driver's `client_id`/`client_secret`. **Ratified 2026-08-03.** |
| DEC-03 | Two-component `actor` `{driver, approval}` on sessions and jobs; SIGILED-local capability map per the "IdP membership-only" doctrine. **Ratified 2026-08-03.** |
| DEC-04 | Two-layer operational log: machine via API from SIGILED's DB; narrative `docs/log-operativo.md` from the template, project-owned forever. SIGILED rule: close coherent work → entry on top. **Ratified 2026-08-03.** |
| DEC-05 | Template versioning with pin `template = "vm-tmpl@x.y.z"` in `sigiled.toml`, on-demand adoption with drift detection; **never auto-update**. **Ratified 2026-08-03.** |
| DEC-06 | The engine serves its own contract: `GET /sigiled/contract`, versioned; skills self-verify against `healthz.version`. **Ratified 2026-08-03.** |
| DEC-07 | Reconciliation concurrency: one branch per workload, lock only in the merge critical section; no more 409 on open. **Ratified 2026-08-03.** |
| DEC-08 | Merge debt with context package; resolution **mandatory before any other work, whatever the model**; if uncertain, ask the operator. **Ratified 2026-08-03.** |
| DEC-09 | Merge commits, not rebase: the session-boundary trace is memory. **Ratified 2026-08-03.** |
| DEC-10 | Semantic conflicts: after recent multiple merges, mandatory scruple build; if broken, it MUST be fixed before proceeding. **Ratified 2026-08-03.** |
| DEC-11 | SIGILED is SIGILED-registered (self-management): the platform's code lives in the project's repo; the service is a pinned-sha deployment; SIGILED manages everything about itself except its own resurrection (§7). **Ratified 2026-08-02.** |
| DEC-12 | ~~v2 naming~~ **Amended 2026-08-03: the platform is called SIGILED.** Domains `sigiled.dev` + `sigilled.dev` (spelling guardian) purchased by il Re; the project continues in `ivan-saorin/sigiled`; the previous repo is archived as the foundation. |
| DEC-13 | Resurrection is a service: `sigiled-supervisor`, ~100 lines, its own repo and deploy (never a SIGILED `[app]`), autonomous API with simple auth — calling it restarts sigiled. **Ratified 2026-08-02.** |
| DEC-14 | Platform-project bootstrap: fresh creation, code via the first session (no adoption; the session-start 503 on adopted repos remains a known bug). **Ratified 2026-08-02.** |
| DEC-15 | Sessions on `sigiled` and `sigiled-supervisor` require a valid approval: the human leg is mandatory for the control plane. **Ratified 2026-08-02; project name updated 2026-08-03.** |
| DEC-16 | Control-plane language: **Rust** — confirms the existing reality (the workspace agent `vm-base` is already an axum server; `ext/` are Rust crates) and extends it to sigiled and sigiled-supervisor. **Ratified 2026-08-02.** |
| DEC-17 | Workspace v2 = **pre-built base image per tag**: `FROM vm-base:x.y.z` + project toolchain layers. End of vendoring `server/`+`ext/`+`build-ext.sh` in the repos and of per-build recompilation (§3.1). **Ratified 2026-08-02.** |
| DEC-18 | Per-language ext: `ext-rust/` (compiled-in, as today), `ext-py/`, `ext-go/` as local supervised processes proxied by vm-base; single HTTP contract at `/x/<name>` inside the token gate (§3.1). **Ratified 2026-08-02.** |
| DEC-19 | SIGILED v2 will be **100% open source**, with an explanatory GitHub Pages landing. The repo is written as public from the start: secrets hygiene over the history, publishable commit messages, stack-specifics extracted into config. "Flip-ready" preparation in build-plan session 1b. **Ratified 2026-08-02.** |
| DEC-20 | The base images `vm-base:x.y.z` are published **public** on ghcr (`ghcr.io/ivan-saorin/vm-base`): pull without credentials from the box and by any self-hoster; PAT only for the push. The pin (template Dockerfile and scripts) uses the full name. No secret lives in the images by construction (rule 8). **Ratified 2026-08-02.** |
| DEC-21 | Public name = **sigiled**, confirmed by the collision check (session 1b, 2026-08-02): on web searches "sigiled" exists only as an adjective (Magic cards, a Raid Shadow Legends champion, Raku prose) — no software project, library, company or product. The heavy collisions that motivated the check concerned the platform's birth name, already eradicated with DEC-12 as amended. Disambiguating tagline on the landing; no new decision required of il Re — outcome registration. |
| DEC-22 | The v1 orchestrator's short name **disappears from the repo**: the name is SIGILED everywhere — verbs at `/sigiled/*` (e.g. `GET /sigiled/contract`), manifest `sigiled.toml` (v2 reads it with fallback to the v1 name for repos born earlier), OAuth2 providers `sigiled-*`, design doc `sigiled-v2.md`. Two operational exceptions, non-negotiable with the facts: the **root** manifest keeps the v1 filename as long as the v1 orchestrator builds this repo's sessions (falls at cutover), and `mgr-smoke` stays in the v1 registry as a tombstone (no delete verb). The production v1 answers on `/mgr` until cutover: outside this repo, not renameable from here. **Ratified 2026-08-02 (in chat).** |
| DEC-23 | License: **MIT** (`LICENSE` at root, `license` field in the crates). Session 1b's Apache-2.0 proposal discarded by il Re: simplicity over the patent grant. **Ratified 2026-08-02 (in chat).** |
| DEC-24 | The per-instance driver skill is **generated, never hand-edited**: `GET /sigiled/skill/{driver}` renders `docs/skill-template.md` with the instance's values (domain, IdP base, driver identity); the `client_secret` is filled live from the IdP admin API when `AUTHENTIK_API_TOKEN` is configured, otherwise a copy-it-yourself placeholder. Approval-gated (capability row next to projects-new): the response can carry a credential. Requested by il Re and registered in chat, 2026-08-03. |
| DEC-25 | **Per-project session images** — the hook §3.1 promised, discovered missing 2026-08-08 (every workspace ran on the global vm-base; a session on this very Rust repo had neither cargo nor cc). Pin point = the manifest, closing §3.1's open choice: `[workspace] dockerfile = "…"` in sigiled.toml — never a bare-filename convention, because this repo's root Dockerfile builds vm-base (DEC-17 publisher role) and is NOT a session image. Tag `vm-{p}:df-{blob12}`, content-addressed on the dockerfile's git blob at master: unrelated commits never rebuild, an edit rebuilds at the next open (synchronous, the open IS the wait). Sessions fall back to the base **with a shout** (`image: {used, requested, build_error}` in the open/recycle response — the session is the repair tool for its own dockerfile); jobs **fail fast** (batch missing its declared toolchain must not lie). Absent table = global base, prior behavior. Registered in session f53cbce3 and live-verified the same day (build at open, tag rotation on a dockerfile edit, cargo check green inside the sigiled workspace). **Ratified by il Re 2026-08-08 (in chat).** |
| DEC-26 | **Repo files are English-primary.** Every file committed to a stack repo carries English as its primary language; an Italian copy may exist alongside (`*_it.md`), never instead. Explicit riders from the edict: chats may be Italian; names may be Italian (e.g. `spina`). Standing carve-outs preserved: `docs/log-operativo.md` stays Italian by design (working memory, not documentation) and `git log` speaks both. Elevates the 2026-08-03 sessione-1b decree (README «A note on language», commit `83e4466`) to a numbered decision. **Ratified by il Re 2026-08-09 (in chat, as an edict «in cubital letters»).** |
| DEC-27 | **The stack service catalog + the one service method.** Atomic stack services are declared in `catalog.json` at the repo root and served by `GET /sigiled/services` — embedded at build like the contract (DEC-06 by construction), validated at boot (a broken catalog fails the deploy, never a late 500), public (it names capabilities, it carries no secret). Schema per service: `name, purpose, machine{base, gate}, human?{base, gate}, spec, status, skill` — the machine leg is mandatory (the catalog is LLM-facing) and gates state how the edge answers **as it actually is, never as planned** (search ships `sso-only` until its edge grows the split). Dual-by-nature services adopt **one access method everywhere: one vhost, bearer split** — a request presenting `Authorization: Bearer` is the machine leg (bearer checked, identity headers scrubbed), everything else is the human leg (authentik forward_auth, identity copied); the `(dual)` snippet in `deploy/Caddyfile.example` is the normative pattern, and the paper/paper-api two-vhost split collapses into it at the operator's pace. The generated skill inlines the stable trio (search, paper, folio) plus the catalog pointer — growth never requires skill regeneration. **Ratified by il Re 2026-08-15 (in chat).** |
