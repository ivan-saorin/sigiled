# Composed-service authentication — plan (2026-08-15)

**Session**: 5bb368aa, continued in 3ec67e53 (elevated, driver sigiled-claude)
· **Decision**: DEC-28, **PROPOSED — not ratified.** The code described in
§"What landed" is on master and inert; §"What the operator must do" is what
turns it on.

> **Amended the same day, by the operator, before ratification.** The first
> draft of this plan made per-callee grants the rule. The operator's answer:
> *"It would be sufficient that the caller is correctly authenticated. I don't
> need per service granularity."* — so the default is now
> `any-authenticated`, and per-service survives as a tested, documented,
> off-by-default tightening (`docs/per-service-authorization.md`).
>
> Two consequences worth reading before the rest of this document, because
> they invert its original conclusions: **the session token broker is no
> longer on the critical path** (it existed only to carry a project claim
> that per-service granularity needed), and **SDE is unblocked** the moment
> the edge change is applied — a driver token now satisfies the gate.
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

1. **A genuine stack identity is sufficient.** `SIGILED_SERVICE_POLICY`
   defaults to `any-authenticated`: the edge accepts any token this IdP
   signed, unexpired, from the trusted issuer. This is the same trust
   boundary as the shared static bearer it replaces — everyone inside reaches
   everything — but with the shared secret gone, the caller named in the
   logs, revocation by disabling a client, and rotation by an hour elapsing.
   A strict improvement on the status quo with no provisioning burden, which
   is why it is the default rather than a compromise.

   The tightening exists and is tested: `per-service` requires the caller to
   carry `svc:<service>`, prefix configurable
   (`SIGILED_SERVICE_GROUP_PREFIX`) and compared **whole string** — a prefix
   match would let `svc:genie-preview` satisfy `genie` and silently widen
   every grant on the stack. How to turn it on:
   `docs/per-service-authorization.md`. An unrecognised policy value
   **panics at boot**: a misspelling that resolves to the permissive default
   would leave the operator believing the stack is locked down when it is
   not.

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

4. **Under the default, a driver token reaches services — and that is the
   point.** It is what lets a session read the spec of the service whose
   client it is writing, which is the entire problem SDE reported. Under
   `per-service` the same token is refused without an `svc:` grant, and there
   is a test for each direction so neither behaviour can drift into the other
   unnoticed.

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

## What did NOT land, and why it no longer blocks anything

**The session token broker — designed, not built, and no longer needed.**

SDE's writeup claimed sessions "fall out of this for free" by accepting the
driver's own token. Under the original per-callee proposal that was false, and
structurally so: `sigiled-claude` is a **driver** identity shared across every
project, and its JWT says *which driver*, never *which project* — so no
per-project `[compose]` policy can be enforced from it. The fix would have been
a project-scoped, session-lived token: SDE's option 2, the broker,
`POST /sigiled/sessions/{id}/token/{service}`.

The operator's simplification dissolves the requirement rather than solving it.
Under `any-authenticated` there is no per-project claim to carry, so the driver
token is sufficient exactly as SDE originally hoped — it just needed the edge
to accept IdP identities at all, which is what landed.

The broker therefore stays a **footnote in
`docs/per-service-authorization.md`**: the piece to build first *if* the stack
ever turns on per-service and wants it to bind sessions as well as apps and
jobs. It is new credential-issuing surface in the control plane, and it should
be built when something needs it, with its own review.

## What the operator must do

1. **Ratify or amend DEC-28**, then the register row goes into `sigiled-v2.md`
   §8. Nothing here has been added to §8 — that is the ratification act, not
   the proposal's to take.
2. **Apply the edge change by hand.** `deploy/Caddyfile.example` is an example;
   the live Caddyfile is on the host, out of reach of any session (rule 5).
   Copy the `(dual)` snippet, give each `import dual` its service-name second
   argument, reload, then run the two probes at the bottom of that file.
3. **Nothing else.** No groups to provision, no manifests to edit, no broker
   to wait for: `any-authenticated` needs only the two steps above. SDE is
   unblocked the moment step 2 is applied — it can then read genie's and
   paper's specs with the token it already mints, and finish S2 against
   documents rather than guesses.

If and when the trade changes, `docs/per-service-authorization.md` is the
how-to: declare `[compose]`, provision `svc:*` groups, flip one env var. The
edge needs no change at that point, because the Caddyfile already passes each
service's name to `/auth/verify` — the default policy simply ignores it.
