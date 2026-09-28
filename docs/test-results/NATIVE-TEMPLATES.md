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

## Release and live acceptance gate

The running service still serves contract 2.5.0 at source acceptance time. Health
reports `2.0.0-alpha.1` without a build SHA. The established host deployment uses
an external supervisor and a pinned launcher/image, separate from the SIGIL
driver's OAuth approval. The old launcher must not be reused with a new SHA or
bypassed with an ad-hoc compose restart. Prepare a new candidate and launcher using
current host evidence, retaining state/keys/mounts and existing browser config,
then use one supervised activation. Historical deployment scripts are references,
not scripts to rerun. The public supervisor hostname's TLS probe failed; no
supervisor credential has been provided to this driver.

No live template fixture repositories have been created yet. No Opticon source,
template designation, approvals or videos were changed. Local fixtures live only
in disposable Docker test filesystems; test-image/cache volumes are task-owned.

After supervised activation, verify contract 2.6.0 and `/templates`, then use
clearly named sources `sigil-tpl-rust-0929`, `sigil-tpl-python-0929` and generated
destinations `sigil-tpl-default-0929`, `sigil-tpl-film-a-0929`,
`sigil-tpl-film-b-0929`, `sigil-tpl-repeat-0929` (check names remain unused first).
Designate the two prepared sources explicitly. Create default/selected/pinned
destinations; inspect contents, source SHA, independent IDs and selected workspace
images. Complete open → useful build → commit/push → close → reopen on generated
projects, confirm files/provenance survived, and record final session states.
Do not delete projects; remove fixture eligibility afterward and report retained
fixture repositories. Live acceptance remains required before calling this
feature complete.

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
