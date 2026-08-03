# Authentik setup for SIGILED

The design is §1 of `sigiled-v2.md`: two legs — machine (`client_credentials`
per driver) and human (device flow via `sigiled-device`). This document is
the operator's runbook. *(Official English version; Italian original:
`authentik-setup_it.md`.)*

## Prerequisite: admin API token

Provisioning requires an API token of an **admin** user:

1. Admin interface → **Directory → Tokens** → Create.
2. User: yours (or a service account with admin permissions);
   Intent: **API Token**; expiry as you like.

**Experience note (2026-08-02):** the **embedded outpost**'s token
(`ak-outpost-…`) is not enough — lists come back empty (object-level
permissions) and every create answers 403. If `tools/authentik-provision.sh`
prints empty lists or 403s, you are using the wrong token.

## Automatic provisioning

```sh
DOMAIN=example.com AUTHENTIK_API_TOKEN=<admin token> tools/authentik-provision.sh
```

Idempotent. Creates: the `sigiled-groups` scope mapping (the `groups` claim
for the capability map), groups `stack:admins`/`stack:drivers`,
provider+application `sigiled-device` (public, the human leg) and one
**confidential provider per driver** (`sigiled-claude`, `sigiled-kimi`;
override with `DRIVERS="…"`), then "primes" each driver (first mint → the
service account `ak-<driver>-client_credentials` is born) and adds it to
`stack:drivers`. The script closes by printing the remaining manual steps
(YOUR user into stack:admins, device flow on the brand, where the
client_secrets live).

**Executed successfully on 2026-08-02** on the reference IdP (2026.5.6):
providers pk 2/3/4, full e2e — JWT minted with client_credentials, claim
`groups: ["stack:drivers"]`, validated by sigiledd via live JWKS.

### Trap: `grant_types` is an allowlist (recent versions)

A provider created via API with `grant_types: []` **refuses every grant**
with a *silent* `invalid_grant` (no event in Authentik's log — the refusal
happens in `TokenParams.__post_init__` before everything else). Diagnosis:
if the mint fails invalid_grant, the secret matches and the events are
silent, check `grant_types` on the provider. The script sets them
explicitly: `client_credentials` for the drivers, `device_code` for
sigiled-device — which is exactly the "narrow grant" design §1.8 prescribes.

## Manual provisioning (fallback, UI)

1. **Scope mapping**: Customization → Property mappings → Create → Scope
   mapping. Name `sigiled: groups claim`, scope `sigiled-groups`, expression:
   `return {"groups": [g.name for g in request.user.ak_groups.all()]}`.
2. **Groups**: Directory → Groups → `stack:admins`, `stack:drivers`.
   Yourself into admins.
3. **Driver provider** (one per driver, §1.3): Applications → Providers →
   Create → OAuth2/OpenID. Name/client_id `sigiled-<driver>`, Confidential,
   RS256 signing key (Authentik's self-signed is fine), property mapping
   `sigiled: groups claim`. Then Applications → Create with the same slug,
   bound to the provider.
4. **Device provider** (§1.4): as above but **Public**, client_id
   `sigiled-device`. Brand → Default flows → Device code flow enabled.
5. **Driver service account**: on the first `client_credentials` Authentik
   creates the application's service account — add it to `stack:drivers`.

## sigiledd wiring (stack env)

| Env | Value | Notes |
|---|---|---|
| `SIGILED_OIDC_BASE` | IdP URL | OIDC leg; absent = leg off |
| `SIGILED_BOOTSTRAP_BEARER` | legacy bearer | dual-auth window §1.7; removed at end of migration |
| `SIGILED_DEVICE_CLIENT_ID` | `sigiled-device` | default already right |
| `SIGILED_ADMIN_GROUP` / `SIGILED_DRIVER_GROUP` | `stack:admins` / `stack:drivers` | defaults already right |
| `AUTHENTIK_API_TOKEN` | admin token | provisioning/maintenance, **and** `GET /skill/{driver}` (DEC-24): lets sigiledd embed real client_secrets in rendered skills |

## Verification

```sh
# public discovery of the driver provider
curl -s $IDP/application/o/sigiled-claude/.well-known/openid-configuration | head

# the machine leg mints a token (the client_secret lives in the driver's skill)
curl -s -X POST $IDP/application/o/token/ \
  -d grant_type=client_credentials -d client_id=sigiled-claude \
  -d client_secret=$SECRET -d scope="openid profile sigiled-groups"
```

The resulting JWT must have `iss` under the IdP, RS256 signature and a
`groups` claim with `stack:drivers`: exactly what `sigiledd` validates
(auth.rs).

## Where the secrets go

- Each driver's `client_secret` → **only in that driver's skill** (skill
  handling rules: never echo, never commit). Skills are rendered by
  `GET /skill/{driver}` (DEC-24), not written by hand.
- Admin API token → the operator's stack env, never in the repos.
- Device-flow tokens never leave SIGILED (custody §1.4).
