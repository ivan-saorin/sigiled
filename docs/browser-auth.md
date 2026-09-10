# Browser authentication rollout (B1)

This source adds an optional browser originator identity boundary. It does not deploy a dashboard, provision an Authentik client, open a workspace, or run inference. Fixture tests are not proof of live human sign-in or downstream token compatibility. Release requires the live checks below.

## Routes and contract for B2/C2/D1

Only the exact root `/browser/` paths below accept cookies. The existing bare and `/sigiled/` machine routes continue to require their existing bearer identity. Do not rewrite a browser request onto a machine route to bypass its extractor.

| Route | Contract |
| --- | --- |
| `GET /browser/login?return_to=/projects/example` | Creates a five-minute, one-use PKCE S256 transaction, sets `__Host-sigil_login`, clears any old session and redirects to the fixed authorization endpoint. Default return path `/`. |
| `GET /browser/callback?code=…&state=…` | Exact registered authorization-code callback, validates state plus independent login cookie, nonce, RS256 signature, exact issuer, ID audience/azp, expiry/iat, optional at_hash, access-token identity/group and matching subject. Rotates the session ID and clears login cookie. |
| `GET /browser/session` | Safe actor, stable identity, display name, role/capability/feature information, absolute/idle expiry epochs and CSRF token. No OAuth or workspace token. |
| `POST /browser/logout` | Requires session cookie, exact Origin and `X-Sigil-CSRF`; removes local session immediately even if refresh is in flight. Clears cookies. No provider-wide logout or revocation is implied. |
| `GET /browser/api/overview` | Existing safe overview projection with browser identity. Supports existing pagination. |
| `GET /browser/api/projects/{project}` | Existing safe project projection with browser identity. Supports existing pagination. |

All browser responses use `Cache-Control: no-store`, `Pragma: no-cache`, `Referrer-Policy: no-referrer` and `X-Content-Type-Options: nosniff`. Login/callback failures clear login and session cookies. Rejected CSRF/Origin mutations leave a valid session cookie unchanged. Error bodies contain a static `error` code, never provider errors or token/code contents. `401 {"error":"login_required"}` requires starting a new login. A signed-in human without `SIGILED_ADMIN_GROUP` or `SIGILED_DRIVER_GROUP` receives `403 {"error":"stack_role_required"}`; no role is invented. Other 403 codes include `invalid_host`, `invalid_origin`, `invalid_csrf`, and `invalid_return_path`. Disabled routes return 404 `browser_disabled`.

`return_to` accepts a local absolute path up to 1,024 bytes consisting of ASCII letters/digits and `/ - _ . ~`. It rejects `//`, dot segments, backslash, all percent escapes (including nested encoding), query/fragment, external URLs and `/browser/` paths. B2 should pass stable app routes, never external destinations. There is no arbitrary redirect or URL proxy.

Every site uses host-only `__Host-sigil_session` and `__Host-sigil_login`: `Secure; HttpOnly; SameSite=Lax; Path=/`, no Domain. The cookie ID is 256 random bits. Each record is bound to one exact allowlisted origin and a stable verified subject. A cookie copied from Sigil to memory cannot authenticate there. Authentik provides SSO through a separate login/callback on each host. CSRF tokens are separate random values; B2 fetches one from session inspection and sends it in `X-Sigil-CSRF` on every mutation, together with the browser's exact Origin. Do not store access/refresh tokens in browser storage, responses, client logs or URLs.

Sessions and login transactions are in memory only: restarting logs everyone out. Defaults are 15 minutes idle and eight hours absolute, with finite limits below. Maximum 4,096 sessions and 1,024 login transactions, opportunistically pruned on login/callback/session access; capacity returns a typed 503 rather than evicting a live session. Each session has a serialized refresh lock. An issued refresh token may refresh an access token within 30 seconds of expiry. Without refresh, expiry requires login. If a refreshed ID token includes nonce, it must match the original login nonce; omission is supported for refresh. A rejected/invalid refresh revokes the browser session. Cancellation/timeout while rotation is uncertain also revokes it; the old refresh credential is never retried. Logout and idle/absolute expiry cannot be undone by a late response, and refresh never extends absolute lifetime. Mutations are not automatically replayed.

Internal Rust interface: `browser::BrowserContext: FromRequestParts<AppState>` supplies `actor: auth::Actor`, verified `issuer`, `subject`, optional `display_name`, and crate-private `access_token: String`. It has no Serialize or Debug implementation. It checks exact Host and any present Origin on reads, and requires both exact Origin and `X-Sigil-CSRF` on every method except GET/HEAD. Use it only on dedicated browser routes behind `browser::router`'s boundary and limits. Future adapters must send the same access token to fixed, trusted service endpoints using redirect-disabled clients, with no caller-selected URLs and no copied cookie/header identity. Browser workspace credentials must remain in core custody; never return `sessions::open` JSON or skill-render outputs. No adapter or action is enabled in B1.

Human `Actor.driver` is `human:` plus unpadded base64url SHA-256 of the UTF-8 JSON tuple `[verified_issuer, verified_subject]`. Username and OAuth client ID never decide ownership; two subjects using the same client remain distinct. `display_name` is optional verified `preferred_username` and may change. Driver roles still require existing approval gates; browser login does not grant approval. Future approval flows must grant against this stable actor identity. Machine `Claims::driver` semantics remain unchanged.

For D1, `GET|POST /auth/verify` retains existing `caller`, `service`, `granted_by`, `x-sigiled-caller` and service policy. It additively returns `issuer`, `subject`, `principal_kind: "oidc"`, populated only from the signature-verified JWT. `oidc` deliberately does not claim that every service JWT is human. Bind revision authorship to `(issuer, subject)` and preserve caller separately for legacy display. `/auth/verify` continues to reject bootstrap/static bearers; a downstream service that accepts its own legacy static credential must label that identity `legacy` with no invented issuer/subject. No client-supplied identity header is trusted. Machine JWKS cache entries are now scoped by issuer and kid; browser keys are separately scoped to the immutable configured issuer/JWKS pair and kid.

## Explicit deployment configuration

Unset every `SIGILED_BROWSER_*` variable to disable, or set only `SIGILED_BROWSER_ENABLED=false`. Partial, misspelled or insecure browser settings fail startup. Browser Authentik issuer must also be under `SIGILED_OIDC_BASE`, with nonempty machine group configuration. Use a dedicated browser OAuth client, never the installed driver client's secret.

Example production values (replace example hosts/provider slug/client ID deliberately):

```dotenv
SIGILED_OIDC_BASE=https://auth.example.com
SIGILED_ADMIN_GROUP=stack:admins
SIGILED_DRIVER_GROUP=stack:drivers
SIGILED_BROWSER_ENABLED=true
SIGILED_BROWSER_ORIGINS=https://sigil.example.com,https://memory.example.com
SIGILED_BROWSER_ISSUER=https://auth.example.com/application/o/sigil-browser/
SIGILED_BROWSER_AUTHORIZATION_URL=https://auth.example.com/application/o/authorize/
SIGILED_BROWSER_TOKEN_URL=https://auth.example.com/application/o/token/
SIGILED_BROWSER_JWKS_URL=https://auth.example.com/application/o/sigil-browser/jwks/
SIGILED_BROWSER_CLIENT_ID=sigil-browser
SIGILED_BROWSER_SCOPES=openid sigiled-groups
SIGILED_BROWSER_IDLE_SECONDS=900
SIGILED_BROWSER_ABSOLUTE_SECONDS=28800
```

Optional `SIGILED_BROWSER_CLIENT_SECRET` belongs only to that dedicated client; obtain it from server-side secret storage if the registered client is confidential. Otherwise use a public PKCE client. Never copy a secret from the existing driver installation. The token client supports `client_secret_post` for confidential clients and no authentication secret for public clients; configure Authentik accordingly.

Required scopes are explicit `openid sigiled-groups`. `profile`, `email`, and `offline_access` are not assumed to exist. To enable refresh, provision Authentik's `offline_access` scope mapping on this dedicated provider **and** include `offline_access` in `SIGILED_BROWSER_SCOPES`. Absence of a refresh token is supported. Scope configuration syntax is validated locally; availability is an operator/live-test prerequisite. Configure the `sigiled-groups` mapping to emit the actual human's configured stack groups; optionally include per-service groups if the stack uses per-service authorization.

Select an asymmetric RSA signing key and RS256. An unset signing key can cause Authentik HS256/client-secret signing, which this stack intentionally rejects. Register **both exact redirect URIs**, not wildcards or first-use registration:

- `https://sigil.example.com/browser/callback`
- `https://memory.example.com/browser/callback`

Provider endpoints must be HTTPS, canonical, free of userinfo/query/fragment/percent encodings and on the exact configured issuer origin. Origins are unique canonical scheme/host/optional-port values without trailing slash. `SIGILED_BROWSER_ALLOW_LOOPBACK_HTTP=true` permits HTTP only for literal localhost, 127.0.0.1 or ::1 development endpoints/origins; it never removes cookie Secure attributes. Use local HTTPS when testing actual browser cookies. Idle lifetime allows 60 seconds through the absolute lifetime; absolute lifetime allows 60 seconds through 86,400 seconds. At most eight distinct site authorities are accepted.

No metadata is fetched from requests. Authorization, token and JWKS URLs are fixed startup configuration. Outbound provider requests reject redirects, connect within three seconds and have ten-second total timeouts. JSON responses are capped at 65,536 bytes, JWKS at 64 keys, and key fetches at one attempt per five seconds with a five-minute cache. Request URIs are capped at 4,096 bytes and headers at 16,384 bytes; browser endpoints accept no request body and complete within 15 seconds. Do not enable HTTP wire logging or request-body/query tracing.

## Required edge changes before release

Keep the existing bearer/service routes and policies. Add a dedicated fixed `/browser/*` reverse-proxy route on **both** browser site hosts that reaches this same sigiledd process; do not send these paths through the existing generic Authentik edge sign-in, because an edge login cookie alone is not a compatible downstream JWT. Use exact site matchers and preserve the canonical original Host. Do not derive the upstream from client headers or accept arbitrary hosts. Multiple sigiledd replicas require sticky routing or a shared session implementation; in-memory sessions are process-local.

A representative Caddy site fragment, repeated for the two explicitly named site hosts:

```caddyfile
sigil.example.com {
    @browser path /browser/*
    # Entire browser subtree omitted from access logs: callback code/state must never be logged.
    log_skip @browser
    handle @browser {
        reverse_proxy sigiledd:8080 {
            header_up -Forwarded
            header_up -X-Forwarded-Host
            header_up -X-Forwarded-Proto
            header_up -X-Sigiled-Caller
            header_up -X-Sigiled-Issuer
            header_up -X-Sigiled-Subject
            header_up -X-Sigiled-Principal-Kind
            header_up Host sigil.example.com
        }
    }
    # Existing static/browser UI routing follows here.
}
```

Apply equivalent header scrubbing on every upstream hop, plus all other identity assertion headers used locally. The application ignores Forwarded/X-Forwarded-* as authentication inputs. Preserve cookie, Origin and X-Sigil-CSRF only to the fixed browser handler. Do not copy identity claims from untrusted incoming headers. Exclude `/browser/callback` query strings from every edge, load balancer, tracing/APM, error and analytics log; removing app logging alone is insufficient. Validate this Caddy syntax against the deployed version before applying it. Leave existing `/sigiled/*` machine forwarding untouched.

## Release prerequisites and checks

1. Create the dedicated provider/client with exact redirects, RSA key, explicit scopes/group mapping and the configured client authentication mode. These actions have not been performed by this change.
2. Deploy the edge's fixed site routing/header scrubbing and callback log exclusion. Confirm authorization codes/state/cookies never appear in logging or telemetry. Confirm production HTTPS and canonical Host behavior.
3. Sign in as an actual human on Sigil and then memory; verify SSO, separate host-only cookies, stable issuer/subject, correct role, and no tokens in browser responses/storage. Try an ungrouped account and expect forbidden. Inspect only redacted outcomes.
4. Confirm the **same operator access JWT**, held only server-side, validates through Sigil, SDE, ADHD, Genie and memory's existing machine gates. Exact issuer and keys must be compatible in every service; fixture tests do not establish deployment compatibility. Under per-service policy, provision explicit groups without widening policy.
5. If refresh is configured, verify provider rotation with that client and scopes. Otherwise verify finite session expiry returns login_required. Test logout while refresh is slow and process restart logout.
6. Confirm legacy bearer clients and existing driver approvals continue to work. No live project/session/workspace action or inference is required for the browser authentication rollout check.

References: [Caddy log_skip](https://caddyserver.com/docs/caddyfile/directives/log_skip), [Authentik OAuth provider](https://docs.goauthentik.io/add-secure-apps/providers/oauth2/), [OIDC ID-token validation](https://openid.net/specs/openid-connect-core-1_0.html#IDTokenValidation), [OAuth security BCP](https://www.rfc-editor.org/rfc/rfc9700.html). Current live provider/client setup remains a release prerequisite.
