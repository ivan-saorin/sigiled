# Per-service authorization — how to turn it on

**Status:** implemented, tested, and **off by default** (DEC-28, 2026-08-15).
The stack runs `SIGILED_SERVICE_POLICY=any-authenticated`: any identity the
stack IdP vouches for may call any stack service. This document is for the day
that stops being the right trade.

**Read this first if you are debugging a 403 from the edge.** Under the default
policy the edge never emits one. If you are seeing `caller X lacks svc:Y`,
somebody has already turned this on.

## What the two policies actually differ on

Both modes validate the token identically, and that validation is the security
boundary: RS256 signature against the issuer's JWKS, `exp` enforced, and the
issuer required to sit under `SIGILED_OIDC_BASE`. A forged, expired or
foreign-issuer token is refused under both.

The modes differ only on what happens *after* the token is known to be real:

| | `any-authenticated` (default) | `per-service` |
|---|---|---|
| Rule | a genuine stack identity is enough | caller must carry `svc:<service>` |
| Grant lives in | the IdP's user set | group membership, from `[compose]` |
| Adding a caller | issue it an IdP client | issue it a client **and** grant groups |
| Blast radius of a leaked token | every stack service | the declared services only |
| Sessions | work today | need a project-scoped token (see Caveat) |

The honest summary of the default: it is the same trust boundary as the shared
static bearer it replaces — everyone inside can reach everything — but with the
shared secret gone, the caller named in the logs, revocation by disabling a
client, and rotation by an hour elapsing. That is why it was chosen: it is a
strict improvement on the status quo without a provisioning burden.

## Turning it on

Nothing in the edge changes. `deploy/Caddyfile.example` already passes each
service's name to `/auth/verify` via `X-Sigiled-Service`, and the default
policy simply ignores the value. That was deliberate, so this is a config flip
rather than a migration.

### 1. Declare the graph

In each consuming project's manifest on **master**:

```toml
[compose]
services = ["genie", "paper", "search", "folio", "adhd"]
```

Shape-validated at parse (lowercase alnum+dash, letter first); sorted and
deduplicated, so reordering the list is not a diff. Absent table = declares
nothing. Names are *not* checked against `catalog.json` — see the note in
`manifest.rs` for why that coupling is deliberately avoided.

### 2. Provision the groups

One group per catalog service, and each caller's service account added to the
groups its `[compose]` declares. `tools/authentik-provision.sh` already makes
exactly these calls for `stack:drivers`; the additions mirror them:

```sh
# create the group
curl -sSf -H "$auth" -H 'Content-Type: application/json' \
  -d '{"name": "svc:genie"}' "$API/core/groups/"

# find the caller's service account (born on its first successful mint)
curl -sSf -H "$auth" \
  "$API/core/users/?type=service_account&search=ak-sde-client_credentials"

# grant
curl -sSf -H "$auth" -H 'Content-Type: application/json' \
  -d "{\"pk\": $SA_PK}" "$API/core/groups/$GROUP_PK/add_user/"
```

The claim arrives through the existing `sigiled-groups` scope mapping
(`return {"groups": [g.name for g in request.user.groups.all()]}`) — no new
mapping, no new provider, no new claim type.

### 3. Flip the policy

```sh
SIGILED_SERVICE_POLICY=per-service   # in /opt/sigiled/.env, then redeploy
```

Optionally `SIGILED_SERVICE_GROUP_PREFIX` if `svc:` collides with something;
it is compared whole-string, so `svc:genie-preview` never satisfies `genie`.

A value that is neither `any-authenticated` nor `per-service` **panics at
boot**, on purpose: a misspelled security policy that silently resolves to the
permissive default is the worst possible outcome — the operator believes the
stack is locked down and it is not. Same doctrine as `catalog::assert_valid()`.

### 4. Verify

```sh
curl -sS -H "Authorization: Bearer <driver JWT>" \
  -H 'X-Sigiled-Service: genie' https://api.<domain>/sigiled/auth/verify
```

- before granting: `403 {"detail":"caller sigiled-claude lacks svc:genie"}`
- after granting: `200 {"caller":"...","service":"genie","granted_by":"compose"}`

Admins (`stack:admins`) pass both policies unconditionally, so test with a
driver identity or you will prove nothing.

## Caveat: sessions need one more thing

Apps and jobs have identities of their own, so per-service works for them
today. A **session** does not — it borrows its driver's identity, and
`sigiled-claude` is one client shared across every project on the stack. Its
JWT says *which driver*, never *which project*. So `[compose]` cannot
constrain a session under this mechanism: granting `svc:genie` to the driver
grants it everywhere that driver works.

Closing that gap means minting a **project-scoped, session-lived token** —
`POST /sigiled/sessions/{id}/token/{service}`, dying with the session, every
mint auditable against the session's `actor`. It is designed but not built,
because under the default policy nothing needs it. If you turn on per-service
and want it to bind sessions too, that is the piece to build first.

Until then, a stack running `per-service` should expect one of:

- driver sessions keep broad access (grant the driver every `svc:*` it needs)
  while apps and jobs are properly constrained — a coherent posture, since a
  session is a human-supervised, approval-gated thing anyway; or
- sessions lose service access entirely, and live integration work moves to
  jobs, which do get their own identity.

Neither is wrong. Choosing between them is the decision that comes with
turning this on.
