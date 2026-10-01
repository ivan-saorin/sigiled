# Project collaboration journeys — branch validation

Date: 2026-10-01. Branch: `feat/project-collaboration-journeys`. Base: `44d367fed6288ad7e6d98856f4cbe45d9a3c736e`.

The implementation and API semantics are in [project-journeys.md](../project-journeys.md). This is source validation with controlled records, not production acceptance.

## Results

| Check | Result |
| --- | --- |
| `cargo test -p sigiledd browser::tests -- --test-threads=2` in the Linux SIGIL workspace | 36 passed, 0 failed; includes both new request tests and existing work-item/authentication boundaries |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy -p sigiledd --all-targets -- -D warnings -A clippy::too_many_arguments` | Passed with the repository's existing argument-count allowance |
| `node sigiledd/tests/project-journeys.cjs` | Passed against real browser assets in installed headless Microsoft Edge |
| `node sigiledd/tests/browser-ui.cjs` | Passed |
| `node sigiledd/tests/browser-races.cjs` | All 11 regression cases passed |
| Desktop 1440px and mobile 390px screenshots | Visually inspected; mobile page has no horizontal overflow |

Backend coverage exercises real combined routes and authentication, project custody, stable request identity, exact retry, conflicting concurrent answers, immutable answer recovery, persistence across store restart, isolation from ordinary work-item editing, safe links, failed atomic saves and strict pagination. Answering creates neither sessions nor authorization approvals.

Browser coverage checks the review timestamp boundary and persistence, absence of server mutations from catch-up, draft retention across polling/navigation, uncertain-save inspection without another mutation, stale reads arriving after a newer answer receipt, retained keyboard focus, source/build/deployment separation, unknown health, stale observations, hostile text rendering, expired sign-in without automatic resend, and changed-human draft isolation. Browser fixture data is synthetic; no external agent is awakened and no real deployment is changed.

## Reproduction

Use a Playwright installation available to Node. Optional environment variables are `SIGIL_PLAYWRIGHT_PACKAGE`, `SIGIL_BROWSER_EXECUTABLE` and `SIGIL_SCREENSHOTS`. The test defaults to Playwright's Chromium; this Windows machine used installed Edge because the bundled Chromium executable was absent. `SIGIL_FIXTURE_ONLY=1` starts a loopback fixture for interactive inspection. Never point these fixture tests at a live project.

Windows Cargo could not obtain dependencies because of its local TLS setup; Rust validation therefore ran in the normal SIGIL Linux workspace. One authentication-server connection timeout was retried successfully. The initial browser uncertain-save case used a lost connection which the browser transparently retried; the final fixture returns 503 after persisting the answer to exercise an ambiguous receipt deterministically. These setup failures are not passing validation evidence.

## Release boundary

The feature stays on its dedicated branch. Production was not rebuilt, source was not merged to master, and the new routes have not been exercised on the live service. A release must include matching backend and embedded assets. Real acceptance should verify human sign-in/Origin/CSRF, driver create/read, durable state after restart, and project navigation. `agent-requests.json` belongs in state backups. Agents must read answers through the documented API; automatic continuation is not supplied by this change. Deployment diagnosis uses existing recorded evidence; live probes and build-log access remain separate capabilities.
