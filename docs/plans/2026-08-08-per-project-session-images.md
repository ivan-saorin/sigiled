# Per-project session images — plan (2026-08-08)

**Session**: f53cbce3 (elevated, driver sigiled-claude) · **Decision**: DEC-25
**Origin**: discovered live 2026-08-08 — every session container runs on the
global `SIGILED_VM_IMAGE` (vm-base). The thin project Dockerfile that DEC-17
ratified ("FROM vm-base:x.y.z + project toolchain layers") is shipped by the
template but **read by nothing**: sigiledd has no build hook. Symptom that
exposed it: a session on the sigiled repo itself — a Rust workspace — has
neither `cargo` nor `cc`.

## Decisions (DEC-25)

1. **Pin point = the manifest, not a filename convention.** A project opts in
   with:

   ```toml
   [workspace]
   dockerfile = "Dockerfile"   # path relative to the repo root, on master
   ```

   §3.1 left "Dockerfile or sigiled.toml" open; the sigiled repo itself closes
   it: its root `Dockerfile` builds vm-base (publisher role, DEC-17) and IS NOT
   a session image. A bare-filename convention would have built the wrong
   thing. Absent `[workspace]` → global base image, exactly today's behavior.

2. **Content-addressed tag**: `vm-{project}:df-{blob12}` — first 12 hex of the
   git blob hash of the dockerfile **on master** (read in the mirror). Cache =
   `docker image inspect`: master commits that don't touch the dockerfile never
   rebuild; editing it rebuilds at the next open. Limitation, accepted and
   documented: the cache key is the dockerfile blob alone — a dockerfile that
   COPYs context files must itself change to bust the cache. Session
   dockerfiles are toolchain layers by design (§3.1); the build context is
   still the full mirror checkout so COPY works when needed.

3. **Sessions fall back with a shout, jobs fail fast.** If the manifest is
   broken or the build fails, `open`/`recycle` proceed on the global base and
   the response carries the debt loudly:

   ```json
   "image": {"used": "<base>", "requested": "vm-p:df-…", "build_error": "<tail>"}
   ```

   Rationale: the session is the repair tool — a project whose dockerfile is
   broken must stay drivable to fix its own dockerfile (same philosophy as
   merge debt: never block, surface loudly). A healthy resolve reports
   `{"used": "vm-p:df-…"}`; no `[workspace]` reports the base image. Jobs are
   batch: a failed build fails the run with the build tail as its detail —
   running a nightly on an image missing the declared toolchain would be a
   silent lie.

4. **Build is synchronous at open**, under `spawn_blocking`. First open after a
   dockerfile edit pays the build (apt layers: minutes); every later open hits
   the cache. Apps build async because nothing waits on them; a session open
   IS the wait.

## Touch list

- `manifest.rs` — `[workspace] dockerfile` key; path must be relative, no
  `..`, non-empty; `Manifest::from_repo(path)` reading sigiled.toml with
  mgr.toml fallback (the same pair apps.rs and jobs.rs already probe).
- `runtime.rs` — `session_image_tag(repo, project)` (pure: manifest + git
  hash-object → tag, testable without docker); `ensure_session_image`
  (cache check + `build_image` from the mirror); `create_container` takes the
  image explicitly instead of always `self.image`.
- `sessions.rs` — open + recycle resolve the image after `ensure_mirror`,
  pass it to `create_container`, surface the `image` object in both
  responses (null on the branch-only path).
- `jobs.rs` — `execute_run` resolves the same way; build failure = run error.
- `template/sigiled.toml` — `[workspace] dockerfile = "Dockerfile"` active by
  default: the hook exists from birth; building the thin FROM-only dockerfile
  is instant and effectively aliases vm-base.
- `mgr.toml` (this repo) + `Dockerfile.session` — sigiled declares its own
  session image: rust 1.97.1 (rustup, system-wide) + build-essential on top of
  vm-base. The dogfood that exposed the hole becomes its regression test.
- `docs/sigiled-contract.md` — the `image` field in open/recycle responses,
  the jobs note; `docs/sigiled-v2.md` (+`_it`) — DEC-25 in the registry, §3.1
  annotated.

## Verification

Unit tests colocated per module (manifest parse/validation, tag shape and
blob-hash derivation on a tmp repo, open response shape). Full
`cargo test --workspace` runs OUTSIDE the workspace (this session's container
has no cargo — the very bug) on a synced checkout. Live proof after the
operator redeploys sigiledd: reopen a session on `sigiled`, `which cargo`.

## Operator notes

No new env vars. After merge: supervisor redeploy of sigiledd, then the first
open on each opted-in project pays one image build. `docker image prune`
policy unchanged (dangling df-* tags of edited dockerfiles are prunable — any
open rebuilds what it needs).
