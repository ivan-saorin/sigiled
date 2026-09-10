# Memory browser and fixed adapter (D2)

Source implementation; not deployed. The live Memory service must expose accepted D1 before curation can run. A configured URL or a desired version does not establish live readiness.

## Exact entry and routing

Set `SIGILED_BROWSER_MEMORY_ORIGIN=https://memory.016180.xyz` and include that exact origin in `SIGILED_BROWSER_ORIGINS`, alongside the existing `SIGILED_BROWSER_DASHBOARD_ORIGIN`. The Memory selector is optional; when set it must be a distinct explicitly enrolled canonical origin. Root `/` opens Memory there. `/ui/memory` and shared `/ui/*` work on both selected sites. Only those two exact sites serve the embedded `dashboard.js`, `dashboard.css`, `memory.js`, `memory.css` assets at `/browser/assets/*`; other enrolled authentication origins still cannot serve them. Forwarded Host never selects a site.

Each hostname uses its own existing host-only `__Host-sigil_session` / login cookie and exact `/browser/callback`, never a shared Domain cookie. Authentik may provide SSO through separate authorization-code logins. Register both exact callbacks before activation. Preserve the shared auth process/state behind the routes; a cookie from Sigil is not a Memory cookie.

Edge routing example (ownership pseudocode, not a validated proxy configuration):

```
Host == memory.016180.xyz:
  exact / or /ui or prefix /ui/ or prefix /browser/ -> existing Sigil browser process
  exact /health or /capabilities or /idx or prefix /idx/ or exact /search -> existing Memory machine service
Host == configured Sigil dashboard authority:
  existing routing, including /, /ui*, /browser*
Other hosts:
  no Memory root/assets forwarding
```

Preserve Host through the trusted edge, strip spoofed forwarding/identity headers and keep machine bearer authentication unchanged. Neither this example nor source tests verify actual edge, DNS, TLS, callback registration or Memory verifier policy. Memory requires its fixed trusted daemon verifier and original human JWT pass-through; no driver/provider/workspace token is shipped to a browser.

## Browser API

Prefix below: `/browser/api/memory/indexes/{index}`. All calls use B1 BrowserContext; mutations require exact Origin and CSRF. Inputs are narrow DTOs with unknown fields rejected. The UI binds submitted snapshots to its original actor and uses the CSRF from that exact checked session, so a different cookie session cannot take over an in-flight submission. Actor fields are never forwarded as authority.

| Method / path | Input and public result |
|---|---|
| GET `/browser/api/memory` | `{indexes:[{name,rows?,last_ingest?,model?}],readiness,observed_at,project_association}`; dynamically observed index inventory, no slug ownership inference. |
| GET prefix `/chunks` | Browse `limit` 1–100, D1 `cursor,source,ref,path,tag,since,until,archived`; exact `{items,next_cursor,scanned,consistency:"live_keyset"}`. Follow empty/short continuations. |
| GET prefix `/chunks?q=…&mode=…` | Fixed search modes `bm25` (default), `vector`, `hybrid`; source/ref/tag/date filters; returns up to 200 observed candidates for local URL-offset UI pagination, `result_limit,observed_results,mode,consistency:"bounded_search_candidates"`. No exhaustive total or invented service cursor. Search cannot apply path/archived filters; use Browse. |
| GET prefix `/chunks/{id}` | Typed provenance chunk with optional authoritative target/curation. Legacy `source:manual` / `memory:` refs stay documents unless D1 verifies stable identity. |
| POST prefix `/manual` | `{create_id,text,tags?}` -> bare Manual, HTTP 201 or 202; retain route index separately. |
| GET / PUT prefix `/manual/{id}` | GET `{memory,target,curation}`; PUT `{expected_revision,text,tags?}` -> bare Manual, HTTP 200 or 202. |
| GET prefix `/manual/{id}/history` | `after` decimal string, `limit` 1–100 -> `{items,next_after}`. Current detail, not history, reports current projection. |
| POST prefix `/curation` | `{target,expected_revision,pinned?,archived?,annotation?,context_chunk_id?}` -> `{target,curation}`. Independent curation CAS. |
| POST prefix `/forget/preview` | `{selector}` -> exact bounded sample/count/confirmation/version strings. Read-only preview still uses CSRF. |
| POST prefix `/forget` | Exact `{selector,confirmation}` from the inspected preview; no legacy deletion fallback. |
| GET prefix `/ingests` | Recorded sources receive opaque `source_id` hashes bound to index/source/ref/path; recent runs, running state and observed metadata. History limits survive as an explicit process-history caveat. |
| POST prefix `/reindex` | `{source_id}` only. Re-read recorded metadata, match identity, then construct the fixed supported git/file/sqlite ingestion request. No browser URL/path/source passthrough. |

The backend uses the seed catalog's reserved `memory` stack-bearer origin, production HTTPS, no proxy, redirects, cookies or caller-controlled URLs/headers; two-second connect/six-second request, 2 MiB response, 450000-byte manual bodies (including worst-case JSON escaping), 100000-byte other allowed mutation bodies. Memory-prefixed URI allowance is 65536 bytes so percent-encoded bounded cursor/path filters fit; all other B1 URI/header/body constraints remain. Text max 65536 UTF-8 bytes and nonblank; at most 32 tags of 1–128 bytes/no commas; annotation max 8192 bytes. Revisions remain canonical signed-i64 decimal strings. Stable UUID/ID validation, bounded page/scan and typed allowlisted public projections apply. Final serialization scrubs the actual current bearer from every projected string, including cursor, search text and metadata; errors use fixed codes.

Live capability is checked on inventory/browse and freshly before every mutation (no mutation capability cache). Exact D1 contract, browse/manual/forget/projection/revision protocols, limits and configured OIDC verifier are required. New writes cannot fall back to old insert/delete. This declares protocol/config compatibility only; actual verifier transport and policy remain a release check. Legacy flat search hits use `ts_epoch`, not the legacy display `ts` string. Unknown counts/timestamps/projection/ownership remain unknown. Keyword search can remain usable with an old service. Semantic/hybrid search requires explicit Run search or Apply filters; retained results and promises are reused on polling/back/forward/login, and a failed search requires another deliberate run. Passive browse/status does not start model work.

## Draft, curation and recovery behavior

The common responsive shell has an index rail, filtered/paginated result list and details. Selection and filters are in the URL; small screens use a focused detail panel and Back to results. Imported content and source metadata are text nodes. Only safe HTTP(S) references without userinfo become links. An indexed source revision change preserves displayed text/draft until Review updated source. Source edits are unavailable pending the D3 association below.

Before a new manual POST, create a canonical UUID and capture immutable submitted text/tags. Keep draft, UUID, submitted snapshot and original actor in this tab through selection, navigation, sign-in and unknown outcomes. Newer unsent text is independent. Definite 400/422 validation rejection permits corrected deliberate submission with a fresh UUID; timeout/5xx/absent responses do not. Inspect the known `m_<UUID>` after uncertainty, or deliberately retry the identical snapshot. A 404 during an in-flight request does not prove failure. Successful POST/PUT may be durable while indexing is pending/failed; the UI says so. Accepted creates have an Open saved memory action; explicit Start another is unavailable while the prior create is uncertain. No mutation replays on login. Different-actor retry is blocked. Draft retention is in-memory only: full reload or tab closure discards unsaved drafts, and the interface states this limitation.

Curation has its own revision and preserves annotation text through polling/conflicts. An ambiguous annotation result requires inspecting current annotations before adopting their revision and submitting again. Pinning is metadata, not a ranking promise. Archive and Restore affect the actual target scope. Annotations show their original sha/span independently from the current indexed source.

Forget is preview then explicit confirmation. Scope changes/navigation/sign-in invalidate pending previews. A 409 requires a fresh preview and new confirmation. An uncertain commit remains locked across internal navigation; no repeated destructive request is offered. Source files remain and imported suppression is permanent in v1; manual forget retains a revisioned tombstone/history. Projection debt is separate from durable suppression.

## D3 and release joins

Project Memory tabs link to the general browser with an explicit enrollment-required message. They intentionally do not assume that a colliding index slug belongs to the project. D3 must supply a verified registered-project/index/accepted-document association with the exact immutable source revision and project-relative path. Only then can Edit source call C2 POST `/browser/api/projects/{project}/ide` with `{idempotency_key,target:{path,line?,column?}}`, through its authenticated launch/grant flow. Show indexed revision separately from current checkout, and keep moved/missing/setup failures explicit. D2 never infers this authority from source/ref/path strings or calls IDE automatically.

Before declaring deployment complete, verify both origin root/assets and machine-path routing, separate host-only SSO and callbacks, exact CSRF/role policy, original caller JWT at the trusted Memory verifier, live D1 capability gate including old-version fail-closed behavior, recorded-source reindex policy and actual durable/indexed outcomes using authorized disposable data. D1 and D2 source acceptance is not live readiness.

## Evidence

Focused adapter fixtures exercise exact transport, old capability, original-bearer echo projection, legacy flat hits, stable manual wrappers, decimal CAS, history, curation/forget and recorded reindex. `sigiledd/tests/memory-browser.cjs` renders the real adapter echo artifact with the real authored assets, then exercises draft/selection/login/actor/forget uncertainty, modes/cached pagination, provenance, indexing states and desktop/mobile views using controlled local fixture data. Generate `target/d2-adapter-echo.json` with `cargo test -p sigiledd memory_exact_adapter_echo_and_legacy_search -- --test-threads=2` before that browser test. All tests use in-process/loopback fixtures, no Memory live writes, embedding downloads or paid inference. Full evidence and numeric exits are retained by the orchestrating task.
