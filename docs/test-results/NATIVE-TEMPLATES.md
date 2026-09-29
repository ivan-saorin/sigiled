# Native project templates — source acceptance, 2026-09-29

Implementation base: `8566354bd327973715e6b12d9ee9bcdf2afcbe1e` from the actual
`ivan-saorin/sigiled` repository. This report accompanies the implementation
commit; it is **not a deployed acceptance claim**.

## Verified locally

Linux Docker test runtime, Rust 1.97.1, two Cargo build jobs and two test threads:

- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings -A clippy::too_many_arguments`:
  passed; the allowance is the repository's established rule.
- `cargo test --workspace -- --test-threads=2`: 33 companion-library tests,
  10 companion-binary tests and 275 control-plane tests passed. Six existing
  control-plane tests and one provider integration test remain ignored by default.
- Actual dashboard assets in headless Edge/Playwright, synthetic loopback API:
  `browser-templates.cjs`, `browser-ui.cjs`, and all 11 `browser-races.cjs`
  regressions passed.

The new Git/provider tests use real source repositories and independent bare
destinations. They verify default creation and distinct Rust/Python template
contents/Dockerfiles, exact source SHA/tree provenance, branch movement between
resolution and fetch, repeat from SHA, independent edits, declared identity
adaptation, new approval namespace, durable restart, lost provider response,
concurrent exact retries, key reuse, incumbent preservation, interrupted staging,
persistence failure before remote creation, designation/revocation, authorization,
invalid manifests/refs, unknown fields, runtime bindings, LFS rejection and image
build failure without registration or opening a fallback workspace. The provider
defaults empty repositories to `main`; creation explicitly verifies `master` for
the normal SIGIL lifecycle.

Browser handler tests exercise the actual signed-in adapter, origin/CSRF checks,
shared authorization, designation and immutable provenance. Asset tests exercise
picker/ref/default payloads, uncertain retry, conflict recovery, navigation during
creation, displayed provenance and both eligibility-management actions.

Initial failing checks were resolved: an abandoned Git index lock poisoned retry;
completed concurrent retry returned 409; a fixture used the wrong browser route;
the runtime fixture did not emulate image inspection; and an existing skill
frontmatter test assumed LF despite Windows checkout CRLF. No production deadline,
supervision or authorization limits were weakened.

## Live acceptance progress — 2026-09-29

Implementation `aef78f0d69cb7a21001bedd3dc728816fb618655` was published through
normal commit/push/close (fast-forward, operational log touched). After the
operator rebuilt with `/opt/sigiled/restart.sh`, the running contract became
2.6.0 and matched the implementation source. `/healthz` is healthy but does not
attest a build SHA. These are live results on api.016180.xyz:

| Check | Evidence |
| --- | --- |
| Default creation | `sigil-tpl-default-0929`, source vm-tmpl `cf57bb5dc336c598d61845a19b46984eb210ebed`; normal open/command/commit/close/reopen passed; files and provenance persisted. |
| Rust template | `sigil-tpl-rust-0929` designated and discovered at `a41ef2aa6d8197807d2cdade4f4367a06731b057`; `sigil-tpl-film-a-0929` copied exact declared source files and used its required Rust/Cargo 1.85.1 environment to compile/run the scene. |
| Python template | `sigil-tpl-python-0929` designated and discovered at `feb67868d657f171266d7d88559090dddf5a57ca`; `sigil-tpl-film-b-0929` copied distinct Python files/Dockerfile and ran its scene with Python. |
| Normal lifecycle | Both selected destinations committed/pushed independent edits, closed with fast-forward and operational log receipts, reopened with unchanged metadata/files, reran their scene and closed cleanly. |
| Identity and authority | Declared film name/repository/project ID adapted; new independent IDs and approval namespaces verified. Synthetic approval file preserved as source data with a different source ID. This does not validate Opticon receipt migration. |
| Concurrency and retries | Two simultaneous Python creation requests returned 201 and 200 with identical provenance/provider ID. Exact completed retries returned 200; changing selected source returned 409. |
| Independence and replay | Rust source was unchanged by destination edits, then deliberately advanced to `49cb12daa7600bcad328c90b8c0e97f72978597c`. `sigil-tpl-repeat-0929` used the original full SHA and reproduced v1; the existing Rust destination still had v1 and its own edit. |

Live missing-ref validation exposed GitHub returning 422 where the fixture had
used 404. It was reported as a 502 provider conflict; no destination or creation
record was created. The correction in this source revision classifies commit
lookup 404/409/422 as `template_revision_unavailable` (HTTP 422), with advice to
select an existing branch/tag/SHA. Provider denial, rate limits and transient
failures remain separate. All 11 template/provider tests, formatting and Clippy
passed after updating the older error expectation; two contract-filtered
regressions also passed. The unchanged full source suite results above remain
the baseline. Contract is now 2.6.1; live correction verification awaits rebuild.

The operator's restart used only `/opt/sigiled/docker-compose.yml`; live browser
routes returned 404 `browser_disabled`. Operator diagnostics confirmed no
`SIGILED_BROWSER_*` settings in the container. The saved browser Compose override
exists, matches its recorded hash, and adds only environment settings for the
sigiled service. Restore it persistently in the normal Compose invocation;
do not select the historical pinned image override for the old source revision.
Browser designation/picker/create acceptance remains pending until that repair.

All six listed fixture repositories are retained; none were deleted. At this
checkpoint their test sessions are closed. The two source fixtures remain
designated for the pending browser test; remove their designation at completion.
No Opticon source, designation, approvals or videos were modified. The feature
is **not yet fully accepted**: deploy this error correction, restore the existing
browser configuration, complete live browser acceptance and final disposition.

## Return to Opticon

Repository: `ivan-saorin/opticon`; last verified published revision supplied by
the user: `a84142534ace09f49523079dcd1110f80f57b8c6`. Reverify its current head
when resuming. M2 is complete and both films were accepted: “Both are good!”
Star export: 1920 × 1080; greeting accepted at exactly 2550 × 1440. Preserve
constrained reusable characters, shoulder-dependent elbow limits, multi-scene
sequencing, fades, cached audio and saved preview approval. Prior validation:
42 passing Windows/automa tests and nine CPU benchmark runs; do not repeat them
merely to resume. Evidence: `docs/test-results/ENGINE-M2.md`,
`docs/authoring-movies.md`, `engine/results/m2/approvals/`.

Once native creation passes live acceptance, finalize the production authoring
SKILL and deliberately prepare a clean reusable template. Validate a fresh SIGIL
project, receipt portability (existing receipts carry canonical local preview
paths), new project binding and content-specific authorization. Do not designate
the current research repo directly. The more demanding M3 film/scene is undecided;
agree its target with the user before starting it.
