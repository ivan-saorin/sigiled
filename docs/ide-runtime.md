# Inherited editor runtime (source capability; not deployed)

Code-server 4.136.2 is bundled by the platform, with the official amd64 SHA-256
`83cf05cf4013da071bffade5fe207865c80966231e4f3cf5946aff3e15a966ec` and arm64
`9feeaf0d49d01ff1baafb0cb6374437a4e7c103cff2c636548283049e60b81e7`.
The standalone provider includes Node and its modules. It needs glibc >=2.28
and glibcxx >=3.4.21; image build runs the pinned provider as the original
project user. Unsupported images retain their normal agent workspace and
show an IDE setup reason. The layer never replaces the project's vm-base,
compiled extensions, entrypoint, CMD, workdir, environment or toolchain.

`Dockerfile.sigiledd` builds a separate static musl companion and installs the
checksum-verified provider under `/usr/local/share/sigil-ide`. The operator may
override `SIGILED_IDE_BUNDLE_DIR`; project declarations cannot supply a bundle,
upstream URL, host mount or credential. Layer cache keys include the resolved
project image ID, provider version and helper/extension content hash. Build
and control subprocesses have deadlines, retained process ownership and safe
errors. Image preparation releases the project mirror lock and retains the
session generation guard. No production Docker build was performed for this
source change. The provider/helper layer Docker build still requires release
qualification on both supported host architectures.

## Authority and production prerequisites

Full-write start is gated. It requires the existing owner/admin and project
approval policy, an active owned session and exact generation, desired IDE
capability, a compatible layer, and a fresh trusted GitHub policy inspection.
Set `SIGILED_IDE_HOST_MERGE=github-pat` only after preparing a distinct host-held
PAT actor; the PAT stays in the control plane. When set, normal host merge
pushes use that PAT over fixed GitHub HTTPS through a process-local credential
helper. Neither Git remotes, command arguments, subprocess output nor workspace
configuration contains the PAT. Without this explicit setting the old deploy
key merge transport is unchanged. A bounded non-mutating `git push --dry-run`
negotiation also checks host transport authority before opening the editor.

The supported GitHub policy subset is deliberately conservative:

- All applicable active master rules are fetched, with at most four pages of
  100 rules and eight detail reads. Responses, deadlines and redirects are bounded.
- Only `update`, `deletion` and `non_fast_forward` rules are understood. At least
  one active `update` restriction is mandatory. Every referenced ruleset must
  be a repository rule for the exact owner/repository.
- Each ruleset must expose `bypass_actors`, containing exactly one `User` with
  the ID returned by the host PAT's `/user` request and `bypass_mode=always`.
  Missing bypass visibility is unknown. `DeployKey`, repository roles, teams,
  parent/org rules, evaluate/disabled rules and other combinations do not pass.
- The workspace write deploy key consequently cannot bypass master updates;
  only the verified host User can perform Sigil's normal merge push. A second
  deploy key alone is not a distinct policy principal. Repository plan and
  permissions must actually support enforcement. No rules are created or changed.

These checks support this subset, not all GitHub policy configurations. Unknown
or incomplete evidence gives a specific setup reason and retains the workspace.
The read/transport proof does not promise a later push will succeed if policy,
credentials or remote history change; failure retains runtime and session branch.
C2 must still implement browser SSO/handoff, origin authorization, logout and
socket revocation. Live DNS/TLS/SSO/policy/browser acceptance is required before
advertising a deployed IDE.

## Provider contract for C2

Machine-only `GET /sessions/{id}/ide` returns desired capability, safe persisted
image/provider/generation state and bounded observed helper status. `POST` takes
`{"generation":1,"action":"start|stop|checkpoint|finish"}`. All operations
apply existing authorization; they do not open or stop another workspace.
Start is idempotent and policy-gated. Mutations retain the session guard after
request cancellation. Finish checkpoints successfully and enters normal close
with the same expected generation. Browser disconnect never finishes/merges.

The additive custodied `WorkspaceBinding.ide` includes separate random control
and activity credentials. They are persisted only in Sigil state; public views
use a strict projection, never serialize the binding. C2 uses the recorded
container and generation, not project DNS. Internal helper HTTP is on port
8090 with `Authorization: Bearer <custodied helper token>`; no port is published.
`/status`, `/start`, `/stop`, `/checkpoint` are narrow operations. `/editor` and
`/editor/*` relay only to `127.0.0.1:8091` (code-server), strip control credentials
and cookies, and support HTTP and WebSocket upgrades. C2 must enforce browser
authority and actively terminate sockets on revocation; a companion handshake
by itself is not browser authorization. Helper/editor listeners are container
internal, with the editor itself bound to loopback. Generic provider preview
proxy routes are disabled. Isolated previews belong to C2.

Readiness separates desired/default-on from image pending/setup required,
starting, ready, stopped and failed. An existing generation without the provider
layer requires a deliberate future recycle; it is never silently replaced.
A crashed companion cannot adopt or kill an unknown process found on the editor
port; it reports ownership unknown and retains files. Stop/restart acts only on
its unreaped child process group. Log output is discarded, not exposed with
potential credentials. Existing agent sessions keep their normal path when IDE
setup/policy is unconfirmed.

## Profiles, Git and activity

Operator volume: `sigil-ide-profile-<sha256(actor ownership key)>`. No host path
is mounted. The layer creates only its own profile root with sticky permissions;
inside it the companion uses private `uid-<effective workspace uid>` storage.
Different project UIDs retain separate preference sets rather than changing the
project user. Each session/generation has an independent live data, SQLite,
extensions and IPC profile. Stop publishes an immutable preferences/extensions
snapshot and atomically selects it for later sessions; concurrent sessions never
share a writable SQLite instance. Snapshots and live profiles survive container
close/reap and require an explicit future operator retention policy. New profiles
seed defaults without replacing deliberate settings, including JSONC settings;
the bundled extension supplies `remote.autoForwardPorts=false` as a default.
Open VSX and explicit VSIX installation remain the provider's extension path.
No unsaved browser buffer durability is promised.

The helper checks session branch, fetch and push URLs and HEAD; unexpected
checkout/remote changes pause synchronization without reset or checkout. Every
30 seconds it pushes an observed immutable commit to the expected session ref
only, never master, never force. Dirty checkpoint stages saved files, creates a
commit tree and atomically updates the expected session ref against the observed
old SHA before pushing. Rejected commits/pushes retain files/commits for retry.
Durability reports dirty, committed SHA, pushed SHA and checkpoint separately;
only normal successful close reports a merge.

`.git/sigil-platform.lock` uses an OS advisory lock between the companion and
newly built vm-base commit/fetch operations. Older project vm-base binaries and
arbitrary terminal/extension processes do not acquire it; native Git locks,
immutable push sources, repeated branch/remote checks and compare-and-swap ref
updates provide additional recoverable conflict handling. No pre-existing Git
lock is removed. This is not comprehensive terminal serialization. Existing
extended project binaries are preserved; they are not replaced to add this lock.

The bundled extension reports explicit keyboard/mouse selection, manual save,
short-window edits following interaction and shell-integration command start/end.
Automatic saves, background document changes alone, polling, helper health,
checkpoint synchronization and an abandoned socket do not renew idle time.
VS Code does not expose a universal human-input signal: terminals without shell
integration and extension actions without observable interaction have limits.
A known running command or manual checkpoint prevents automatic reaping. Lost
command-end observations conservatively retain the workspace; inspect/reconcile
that terminal state instead of silently killing work. Helper failure also retains
IDE-used generations. Active command stop/checkpoint refusal is explicit.

## Verification and rollout

Use `CARGO_BUILD_JOBS=2 cargo test --workspace -- --test-threads=2` and clippy
with only the existing `too_many_arguments` allowance. Actual bare-repository
fixtures cover checkpoint/terminal commit push, divergence, branch/remote switch,
existing locks, rejected push and retry. Controlled process, HTTP/upgrade,
authorization, generation, profile and policy fixtures cover other boundaries.
`ide-agent/activity/activity.test.cjs` runs with the pinned bundled Node.
The optional ignored `provider_smoke` test is explicitly run with a checksum-
verified provider via `SIGIL_TEST_PROVIDER`; it checks non-root health and owned
process cleanup, not deployed browser acceptance.

Before rollout: back up the additive Sigil session state; retain named profile
volumes independently; validate supported image/platform builds, the actual
GitHub rule/host User transport, C2 SSO and active socket revocation, and browser
files/search/terminal/Git/extension paths. Rolling back to an older binary while
IDE-used generations exist loses the new idle/checkpoint contract: stop/finish
those sessions through the new binary first and preserve volumes/state backups.
