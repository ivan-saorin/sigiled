# Composed-service authentication — plan (2026-08-15)

**Session**: 5bb368aa (elevated, driver sigiled-claude) · **Decision**: DEC-28,
**PROPOSED — not ratified.** The code described in §"What landed" is on master
and inert; §"What the operator must do" is what turns it on.
**Origin**: `sde`, the first composed service (DEC-27 sequencing), could not be
finished. Its writeup — `docs/composed-service-auth.md` in that repo — measured
the wall from inside a session container: every stack service is gated at the
edge, so `genie/healthz`, `genie/docs`, `paper-api/openapi.json` and
`adhd/openapi.json` all answer **401**, and a freshly minted `sigiled-claude`
driver token answers 401 on all three too. **A service's public specification
is unreadable from where its client is written.** SDE stopped rather than guess
two request shapes, and said so.

That writeup proposed three options and asked the control plane to choose. This
plan is the answer, and it corrects the recommended option on two points that
only become visible from inside this repo.

## What the SDE writeup got wrong, and why

**`aud=<callee>` is not expressible here.** The recommendation was to mint
audience-scoped JWTs (`aud=genie`). But `auth.rs` deliberately sets
`validate_aud = false`, with the reason in the comment: *aud is the per-driver
client_id*. And in authentik, audience is a **provider-level setting**, not a
per-request parameter of `client_credentials` — a caller cannot ask for one.
Honouring `aud=genie` would mean one OAuth2 provider per *(caller, callee)*
pair, which is combinatorial and would need `authentik-provision.sh` to grow a
nested loop.

**Groups already do the job.** `tools/authentik-provision.sh` provisions a
`sigiled-groups` scope mapping that emits
`{"groups": [g.name for g in request.user.groups.all()]}`, and `Claims` already
parses `groups`. A per-callee **group** — `svc:genie` — is the same idea with
none of the cost: no new claim type, no new provider, and provisioning is
`POST /core/groups/{pk}/add_user/`, a call that script already makes.

**The edge does not need to learn JWTs.** Option 1 implied Caddy validating
signatures, which stock Caddy cannot do — it would mean an `xcaddy` build with
a JWT module, a second implementation of the crypto, and a second clock to skew
against `exp`. Unnecessary: sigiledd **already** validates RS256 against the
issuer's JWKS with a cached `KeyStore`, and the Caddyfile **already** uses
`forward_auth` for the human leg. The edge asks; sigiledd answers.

## Decisions (DEC-28, proposed)

1. **Identity is a group, not an audience.** A caller may call `<service>` if
   its token carries `svc:<service>`. The prefix is configurable
   (`SIGILED_SERVICE_GROUP_PREFIX`, default `svc:`) and compared **whole
   string** — a prefix match would let `svc:genie-preview` satisfy `genie` and
   silently widen every grant on the stack.

2. **`[compose]` is the policy layer, and it is separate on purpose.**

   ```toml
   [compose]
   services = ["genie", "paper", "search", "folio", "adhd"]
   ```

   It says which edges of the call graph exist; it never says how a caller
   proves who it is. Absent table = composes nothing = today's behaviour for
   every repo that predates it. Names are validated for **shape only**, never
   against `catalog.json`: the manifest is read from a project's master while
   the catalog is embedded at *this* repo's build, so cross-checking would make
   a project's manifest parse succeed or fail depending on when sigiledd was
   last deployed.

3. **The edge asks, sigiledd adjudicates.** `GET|POST /sigiled/auth/verify`,
   consumed by Caddy's `forward_auth`: 2xx allow, anything else deny, caller
   identity returned in `X-Sigiled-Caller` and copied onto the upstream
   request. `X-Sigiled-Service` is set **by the edge** and never read from the
   client — that overwrite is the only reason the header can be trusted.
   Both verbs, because `forward_auth` rewrites the method to GET.

4. **Drivers get no blanket access.** `stack:drivers` means *may drive
   SIGILED*; it never means *may call genie*. The policy function has one
   admin arm and one `svc:` arm and no third — the day it grows an
   `|| driver_group`, `[compose]` stops constraining anything and least
   privilege is gone. There is a test named for exactly that.

5. **Migrate by accepting both.** The edge keeps the static
   `{$API_BEARER_KEY}` arm and adds the adjudicated arm; static is matched
   first so the hot path never slows for a caller that has not moved. Flipping
   straight over would break every deployed app in the same instant — they
   hold the static token in their env today. The static arm is deleted when
   nothing presents it any more.

## What landed this session

- `[compose]` in `manifest.rs` — `ComposeManifest`, `composed_services()`,
  sorted + deduplicated, 4 tests.
- `authorize_service_call()` + `GET|POST /auth/verify` in `auth.rs`, 5 tests.
  Smoke-tested against a local dev run: no service header 500, no bearer 401,
  junk bearer 401, healthz 200. The junk-bearer 401 matters — with no IdP
  configured the `Actor` extractor treats every request as a dev admin, and
  `/auth/verify` deliberately does not inherit that.
- `deploy/Caddyfile.example` — the dual-accept `(dual)` snippet, plus two curl
  probes to verify a live reload.
- Incidentally: `Dockerfile.session` installed the toolchain `--profile
  minimal`, so **no session on this repo has ever been able to run `cargo fmt`
  or `cargo clippy`** — they are not installed and `RUSTUP_HOME` is root-owned.
  Components are now baked in; the repo-wide reformat that had silently
  accumulated is its own commit, and clippy's first-ever run found two latent
  lints (`catalog.rs:80`, `runtime.rs:301`) in untouched files, left alone and
  recorded here rather than fixed inside a feature commit.

**Nothing above changes the running stack.** The endpoint is additive and no
edge calls it yet.

## What did NOT land, and the reason it is a decision and not a delay

**The session token broker.** SDE's writeup claimed sessions "fall out of this
for free" by accepting the driver's own token. They do not, and the reason is
structural: `sigiled-claude` is a **driver** identity shared across all 20
projects. Its JWT carries `groups` and nothing that says *acting for project
sde*. So the edge cannot enforce a per-project `[compose]` policy from a driver
token — the project is simply not in it. The choice is:

- accept driver tokens → sessions work immediately, but `[compose]` constrains
  only jobs and apps, and a session on `torchio` may call `genie` just as well
  as one on `sde`; or
- mint a **session-scoped** token carrying the project — SDE's option 2, the
  broker: `POST /sigiled/sessions/{id}/token/{service}`, short-lived, dying
  with the session, every mint auditable against the session's `actor`.

The second is the one worth having and it is the natural next commit: it needs
a signing key for sigiledd and one more arm in `/auth/verify` to accept
sigiledd-issued tokens alongside authentik-issued ones. Deliberately not rushed
into this session — it is new credential-issuing surface in the control plane,
and it wants its own review rather than a tired appendix to this one.

**Consequence to state plainly: SDE is not yet unblocked by this session.**
Until the broker lands (or the operator grants a driver the `svc:*` groups by
hand as an interim), a session still cannot read genie's or paper's spec.

## What the operator must do

1. **Ratify or amend DEC-28**, then the register row goes into `sigiled-v2.md`
   §8. Nothing here has been added to §8 — that is the ratification act, not
   the proposal's to take.
2. **Apply the edge change by hand.** `deploy/Caddyfile.example` is an example;
   the live Caddyfile is on the host, out of reach of any session (rule 5).
   Copy the `(dual)` snippet, give each `import dual` its service-name second
   argument, reload, then run the two probes at the bottom of that file.
3. **Provision the groups.** `svc:<service>` per catalog service, and the
   caller's service account added to the ones its `[compose]` declares.
   `authentik-provision.sh` makes exactly these calls already for
   `stack:drivers`; extending it is small and should follow ratification, not
   precede it.
4. **Decide the interim for SDE**: wait for the broker, or hand
   `sigiled-claude` the five `svc:*` groups temporarily so S2 can be finished.
   The second is a real widening — it grants every project that driver touches,
   not just `sde` — so it should be a conscious, time-boxed choice.
