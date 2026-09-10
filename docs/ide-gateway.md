# Authenticated browser IDE gateway (C2)

Source implementation; no production deployment is claimed. The gateway joins the B1 human identity and credential authority to the reviewed C1 runtime. Machine bearer/JWT authentication, JWT pass-through, `/sigiled/*`, and returned workspace-machine endpoints keep their existing contracts.

## Configuration and origins

Leave the feature unset until its prerequisites are verified. Set `SIGILED_BROWSER_IDE_DOMAIN=ide.example.com` and each of `SIGILED_BROWSER_IDE_DNS_TLS_READY`, `SIGILED_BROWSER_IDE_OIDC_READY`, `SIGILED_BROWSER_IDE_PROVIDER_READY`, `SIGILED_BROWSER_IDE_POLICY_READY` to `true` only after actual verification. Browser auth configuration and a durable writable single-process state store remain required. The acknowledgments are operator assertions, not automated DNS, OIDC or repository-policy proof. Unknown/missing prerequisites fail startup closed. Domains must be canonical lower-case DNS suffixes, separate from all dashboard origins.

Optional previews require `SIGILED_BROWSER_PREVIEW_DOMAIN=preview.example.com`, `SIGILED_BROWSER_PREVIEW_DNS_TLS_READY=true`, and explicit project declaration:

```toml
[ide]
enabled = true
preview_ports = [3000, 5173]
```

Ports are unique (maximum 32), 1024–65535, excluding 2375, 2376, 8000, 8080, 8090 and 8091. Other privileged ports are consequently denied. A port is an explicit operator/project declaration, never a browser-chosen host. Dynamic registry updates supply ports and project availability without a frontend/control-plane rebuild. Preview services must listen on the container interface; a service bound only to its loopback is unavailable to this container-network gateway. No host port is published.

The exact origin is `https://{32-hex-session}-g{generation}.ide.example.com`; preview is `https://{32-hex-session}-g{generation}-p{port}.preview.example.com`. Generation is a canonical bounded u64 decimal string in every browser contract, including safe dashboard projections; machine JSON remains numeric. The longest generated labels fit DNS limits (54 IDE, 61 preview). Incoming Host is compared to origins constructed from registered records. It cannot choose a container or arbitrary upstream.

## Browser API and launch

All dashboard mutations use the actual B1 human Actor, Origin/CSRF checks and project/platform approval policy.

| Route | Request / result |
| --- | --- |
| POST `/browser/api/projects/{project}/ide` | `{idempotency_key, target?}` creates/resumes this human's workspace and returns safe readiness, generation string, session ID and fragment launch link |
| GET `/browser/api/sessions/{session}/ide` | Safe C1 status, with a string generation |
| POST `/browser/api/sessions/{session}/ide` | `{generation,action}`; action start, checkpoint or deliberate finish |
| POST `/browser/api/sessions/{session}/preview` | `{generation,port}`; one-use launch link for the declared generation/port origin |

Allocation keys are scoped by actor/project. A durable hash-to-server-reserved 128-bit session ID intent precedes allocation; session locks precede project locks. Human allocation does not adopt an unrelated orphan or agent workspace. Merge debt is checked again under the project lock. A lost response/restart resumes the same persisted binding. A reserved ID with no record, or a closed binding, remains an explicit `allocation_absent_or_closed` recovery/tombstone result: the same key never silently creates another workspace. A deliberate new operation may use a new key. Persist uncertainty requires the same key and inspection. Existing failed bindings remain visible for ordinary recovery. There is no force-cleanup route.

Open IDE preserves dashboard drafts in the existing page. It reserves a blank tab during the click; after asynchronous allocation it always exposes a usable Continue to IDE link, including when popup creation was blocked. An expired login requires sign-in and explicit session check, followed by a deliberate retry using the retained key. Authentication recovery never replays the mutation.

A random one-use 60-second ticket binds parent login, human, project, session, generation, file target and exact origin. Only a same-origin JSON POST consumes it atomically. The platform launch page immediately removes the fragment and uses no external assets. Launch/control query strings are rejected. Neither ticket nor browser response contains a workspace, helper, IdP refresh or reusable access credential. Tickets/grants are memory-only and bounded; process restart requires a new launch, while allocation identity remains durable.

`__Host-sigil_ide` and distinct `__Host-sigil_preview` cookies are Secure, HttpOnly, Path=/, SameSite=Strict with no Domain. Every request/upgrade rechecks actual B1 credentials, approval, ownership, active lifecycle, ready pinned provider and generation. Refresh uses B1's existing mutex and cancellation guarantees. Established sockets check lifecycle/login liveness every 250 ms and real credential authority every second. Logout, close, recycle, idle/absolute expiry and failed credential refresh terminate access. Closing a tab does not close a workspace.

Only trusted user input observed in the same-origin editor frame renews login idle (throttled to 15 seconds). Background HTTP, status polling and WS traffic do not. This is distinct from C1's extension/command workspace-activity authority and never invokes that activity endpoint. Absolute expiry cannot be extended. Preview grants cannot read platform session/CSRF, operate workspace controls or renew login idle. Same-site siblings are still hostile: exact Origin, Fetch Metadata, no CORS and frame policies supplement host-only cookies; SameSite alone is insufficient.

## Proxy and pinned-provider constraints

IDE requests target only the stored canonical container's helper port 8090; helper `/editor`, `/editor/` and `/editor/*` reach fixed loopback provider 8091 with queries preserved. Preview requests use the same stored container and declared port, with no injected credential. Incoming cookies, Authorization, forwarding/identity headers and hop-by-hop fields are stripped. The gateway injects only its custodied helper credential for IDE, and constructs external Host/Origin from the registered HTTPS origin. Upstream Set-Cookie, identity/CORS and authentication headers are stripped. Redirects must resolve to the same exact origin.

Connect/handshake timeout: 3 seconds; response headers and idle streaming frames: 30 seconds. Requests have 8 KiB URI, 32 KiB headers and 64 MiB streamed upload limits. Responses stream without whole-body buffering. Upstream errors become bounded typed errors, retaining HTTP failure status without private error bodies. Compression/range headers and WebSocket protocols pass through. No generic exec/control proxy is exposed by these grants.

The launch/workbench platform CSP is separate from provider CSP. The pinned provider's script/resource/worker policy is preserved with an additional `frame-ancestors 'self'` policy; preview content cannot frame it. Preview responses have `frame-ancestors 'none'`. Built-in `/proxy` and `/absproxy` routes, including canonical percent-encoded forms, are denied in addition to provider `--disable-proxy`.

The pinned code-server 4.136.2 file handler `/vscode-remote-resource` can return project HTML. Document/frame navigation is denied. All such resource responses additionally force attachment and `sandbox; default-src 'none'`, even without Fetch Metadata, while ordinary static editor resources/workers keep their provider policy. Canonical path checks cover the product prefix, encoded separators/names and reject ambiguous double encodings/traversal. The provider itself returns 404 for an encoded handler spelling; the gateway still protects a normalized handler if one is reached. This restriction can make resource documents/custom extension views unavailable; use isolated previews for project HTML. Arbitrary trusted extensions and deliberate user code execution remain part of the full-write IDE trust model.

Optional `target={path,line?,column?}` is project-relative, bounded and traversal/authority/query/control-character free. The helper checks generation, existence and canonical containment (including symlinks) before launch and again at consumption. Missing/moved files return an explicit error. The server alone constructs `/workspace` folder and the pinned provider's encoded `openFile`/`gotoLineMode` payload. It navigates, never edits a file.

## Durability and recovery

Checkpoint pushes saved files, not unsaved browser buffers. Finish invokes the accepted C1 quiescence/preferences/strict clean pushed receipt and A1 expected-generation close. Failed flush, push or merge preserves work and reports recovery. A deliberate finish of a failed close uses ordinary A1 close again; it does not bypass ownership or the receipt. Lost editor ownership requires the documented operator reconciliation. The dashboard's existing 15-second response budget can expire while the detached durable allocation continues; retry its same key. A finish can invalidate the IDE grant while reporting an error; dashboard controls remain the recovery surface.

## Verification and release

Focused Rust fixtures cover real HTTP/WS transports, fake B1 credentials/refresh, generation strings above 2^53, single-use tickets, origin/CSRF, owner isolation, previews, debt/intent recovery and normal failed-close retry. The ignored `pinned_provider_gateway_browser_smoke` uses actual pinned provider/helper plus a contained Chromium TLS edge and synthetic session, with owned listener cleanup. It verifies workers/resources, exact WSS authority, file line/column, login activity, sandboxed resource handling, isolated preview and sibling HTTP/WS denial. `sigiledd/tests/ide-dashboard.cjs` covers actual dashboard asset draft/popup/auth recovery. Existing B2 race tests remain applicable. The smoke's safe status can say provider unavailable because its synthetic fixture does not use Docker's canonical container DNS; its editor transport uses a test-only loopback mapping.

Live release still requires actual wildcard DNS/TLS, IdP compatibility and refresh/logout, pinned bundle/image, supported repository protection/host merge transport, files/search/terminal/Git/extensions, live socket revocation and persistence/recovery checks. Test-only optional provider VSDA resource 404s were observed while the editor/workers/WSS/file navigation succeeded. The pre-existing repository subprocess `termination_unconfirmed` diagnostic remains qualified and is deferred to Stage E; gateway tests do not change that supervisor or its budgets. No live Docker, SSO, policy, lifecycle, paid inference or deployment was performed for C2.

The browser fixture pins trust to its generated edge certificate SPKI and verifies the exact code-server service-worker script/scope. Its canonical product-prefixed resource handler is also tested; the provider rejects the encoded handler spelling with404. Earlier self-signed-edge service-worker failure remains recorded as fixture evidence. Dedicated WebWorkers and service workers are distinct assertions; optional absent VSDA JS/WASM resources still produce404. Fixture-only child/group ownership uses finite waits and file logs; success requires explicit browser/edge shutdown and closed provider/helper/preview listeners. It does not change the independently qualified production repository supervisor.

The workspace-agent exclusion uses the same `WORKSPACE_AGENT_PORT=8000` constant as the runtime's existing agent transport. Both declaration validation and runtime preview-origin/grant validation reject it, including injected invalid registry data. vm-base itself accepts PORT, but the accepted orchestrator health/boot/API client is fixed to8000; an image moving its sole agent to another port cannot pass normal active-workspace initialization. Such overrides/secondary privileged listeners are outside the supported inherited runtime contract; no automatic arbitrary-port discovery or forwarding is introduced.
